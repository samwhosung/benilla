//! The GameTooltip Lua methods over the parent module's line pool, plus the content builders'.

use mlua::{Lua, Table, Value, Variadic};

use crate::script::binding_abi::{optional_string, string_arg};
use crate::script::object::frame_handle_of;
use crate::script::region::region_handle_of;
use crate::script::Model;
use crate::widget::{KindState, TooltipAnchor};

use super::{
    append_line, clear, color_num, parse_line_color, parse_line_tail, set_owner_core, set_shown,
    text_of, with_tip, wrap_arg, REG_TOOLTIP_METHODS,
};

pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // SetOwner(owner, anchorType [, x, y]) (`0x5310d0`): first the tooltip's own hide, the shown
    // bit cleared (`0x531179`) and the hide slot (`0x531183`), so a shown tooltip fires
    // `OnTooltipCleared` and `OnHide` and stays hidden until a content call shows it; then the
    // core with the new owner ([`set_owner_core`], `0x5313cf`), which clears and fires again.
    //
    // A missing, non-string or unknown anchorType is mode 0, ANCHOR_LEFT, with no error: the
    // binding's local starts at 0 (`0x53120d`). The core's pre-store of 7 (`0x530012`) survives
    // only a NULL owner, the un-own path (`0x530a60`) of every hide.
    m.set(
        "SetOwner",
        lua.create_function(
            |lua, (this, owner, anchor, x, y): (Table, Value, Value, Option<f32>, Option<f32>)| {
                let h = frame_handle_of(lua, &this)?;
                set_shown(lua, h, false);
                // The owner is read after the hide (`0x531189`), so a bad one raises on a
                // hidden tooltip.
                let owner: Table = lua.unpack(owner)?;
                let owner_h = frame_handle_of(lua, &owner)?;
                // The gate is `lua_isstring` (`0x6f3510`): a string or number is read, and any
                // other type counts as absent rather than raising.
                let anchor = optional_string(lua, &anchor);
                // The warning on an unknown name is ours; the reference is silent, and Lua sees
                // no difference.
                let anchor = match anchor
                    .as_deref()
                    .map(str::to_ascii_uppercase)
                    .as_deref()
                    .unwrap_or("")
                {
                    "ANCHOR_LEFT" => TooltipAnchor::Left,
                    "ANCHOR_RIGHT" => TooltipAnchor::Right,
                    "ANCHOR_BOTTOMLEFT" => TooltipAnchor::BottomLeft,
                    "ANCHOR_BOTTOMRIGHT" => TooltipAnchor::BottomRight,
                    "ANCHOR_TOPLEFT" => TooltipAnchor::TopLeft,
                    "ANCHOR_TOPRIGHT" => TooltipAnchor::TopRight,
                    "ANCHOR_CURSOR" => TooltipAnchor::Cursor,
                    "ANCHOR_NONE" => TooltipAnchor::None,
                    "ANCHOR_PRESERVE" => TooltipAnchor::Preserve,
                    "" => TooltipAnchor::Left,
                    other => {
                        lua.app_data_mut::<Model>()
                            .expect("model app_data")
                            .record_warning(format!(
                                "SetOwner: unknown anchor '{other}' (mode 0, ANCHOR_LEFT)"
                            ));
                        TooltipAnchor::Left
                    }
                };
                set_owner_core(
                    lua,
                    h,
                    Some(owner_h),
                    anchor,
                    x.unwrap_or(0.0),
                    y.unwrap_or(0.0),
                );
                Ok(())
            },
        )?,
    )?;
    // GetAnchorType(): `[+0x318]` by name (`0x5313e0`, name table `0x531530`), never nil. While
    // owned it is the last SetOwner's mode; a never-owned tooltip, and any tooltip since a hide
    // (whose un-own stores mode 7, `0x530012`), answers "ANCHOR_NONE".
    m.set(
        "GetAnchorType",
        lua.create_function(|lua, this: Table| {
            let anchor = with_tip(lua, &this, |t| t.anchor)?;
            Ok(anchor.name())
        })?,
    )?;
    // IsOwned(frame) (`0x530fe0`): 1 (`0x5310ad`) or nil (`0x5310c0`), the hover loop's OnUpdate
    // gate (`ContainerFrame.lua:657`); the owner drops on hide, so a stale OnUpdate never brings a
    // hidden tooltip back.
    m.set(
        "IsOwned",
        lua.create_function(|lua, (this, frame): (Table, Table)| {
            let owner = with_tip(lua, &this, |t| t.owner)?;
            let fh = frame_handle_of(lua, &frame)?;
            Ok(crate::script::binding_abi::flag(owner == Some(fh)))
        })?,
    )?;

    // SetText(text, r, g, b, alpha, wrap) (`0x531b90`): alpha has its own number gate, default 1,
    // and a text that is neither string nor number raises the binding's usage error. It clears,
    // writes line 1 and shows: the empty-slot hover calls no Show after it
    // (PaperDollFrame.lua:747).
    m.set(
        "SetText",
        lua.create_function(|lua, (this, args): (Table, Variadic<Value>)| {
            let h = frame_handle_of(lua, &this)?;
            if !matches!(
                args.first(),
                Some(Value::String(_)) | Some(Value::Number(_)) | Some(Value::Integer(_))
            ) {
                return Err(mlua::Error::RuntimeError(
                    "Usage: SetText(\"text\" [, color])".into(),
                ));
            }
            let text = text_of(lua, args.first());
            let [r, g, b, _] = parse_line_color(lua, args.get(1), args.get(2), args.get(3));
            // The alpha is its own `lua_isnumber` gate (`0x531ddc`'s neighbour at idx 6), so a
            // numeric string counts.
            let a = color_num(lua, args.get(4)).unwrap_or(1.0);
            let wrap = wrap_arg(args.get(5));
            clear(lua, h);
            append_line(lua, &this, (text, [r, g, b, a]), None, wrap)?;
            set_shown(lua, h, true);
            Ok(())
        })?,
    )?;
    // AppendText(text) (`0x531e30`): a `lua_isstring` value (`0x531ebc`), a number as its
    // `%.14g` text, else `Usage: AppendText("text")` (`0x8551a8`). Its helper (`0x5305b0`) does
    // nothing with no lines (`0x5305c4`); otherwise it prints line 1's left text and the argument
    // through `%s%s` (`0x82e424`) into 256 bytes, an empty or unset text printing as `(null)`
    // (`0x5305d4`-`0x5305dd`, the CRT's NULL `%s`), and the result cut at 255 bytes (`0x64a865`).
    // That goes to the cell's `SetText` alone (`0x53060e`), its colour and shown bit untouched,
    // and the tooltip is sized again (`0x530615`). The bag tooltips hang a keybinding on their
    // SetText title with it (ContainerFrame.xml:198, MainMenuBarBagButtons.lua:91).
    m.set(
        "AppendText",
        lua.create_function(|lua, (this, text): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let extra = string_arg(lua, text, r#"Usage: AppendText("text")"#)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let lh = match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Tooltip(t)) if t.num_lines > 0 => t.left_lines[0],
                Some(KindState::Tooltip(_)) => return Ok(()),
                _ => return Err(mlua::Error::runtime("not a GameTooltip")),
            };
            let current = model
                .region_data
                .get(&lh)
                .and_then(|d| d.text.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "(null)".to_string());
            let mut joined = current + &extra;
            // A cut inside a UTF-8 sequence drops its partial bytes, which the reference's decoder
            // reads as one malformed code point (`0x41aab0`) that neither draws nor measures.
            let mut cut = joined.len().min(255);
            while !joined.is_char_boundary(cut) {
                cut -= 1;
            }
            joined.truncate(cut);
            super::set_cell_text(&mut model, lh, &joined);
            super::size_tooltip(&mut model, h);
            Ok(())
        })?,
    )?;
    // AddLine(text [, r, g, b [, wrap]]): positional, and appends without showing; the
    // `(text, "", r, g, b)` shape has a non-number r-slot, so it renders default gold.
    m.set(
        "AddLine",
        lua.create_function(|lua, (this, args): (Table, Variadic<Value>)| {
            let text = text_of(lua, args.first());
            let (color, wrap) = parse_line_tail(lua, &args[1.min(args.len())..]);
            append_line(lua, &this, (text, color), None, wrap)
        })?,
    )?;
    // AddDoubleLine(textL, textR, rL, gL, bL, rR, gR, bR [, wrap]) (`0x531840`): each side's
    // colour gates on its own r-slot, and the wrap flag is the tenth argument.
    m.set(
        "AddDoubleLine",
        lua.create_function(
            |lua, (this, tl, tr, cl): (Table, Value, Value, Variadic<Value>)| {
                let left_color = parse_line_color(lua, cl.first(), cl.get(1), cl.get(2));
                let right_color = parse_line_color(lua, cl.get(3), cl.get(4), cl.get(5));
                let right = text_of(lua, Some(&tr));
                let wrap = wrap_arg(cl.get(6));
                append_line(
                    lua,
                    &this,
                    (text_of(lua, Some(&tl)), left_color),
                    Some((right, right_color)),
                    wrap,
                )
            },
        )?,
    )?;
    m.set(
        "ClearLines",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            clear(lua, h);
            Ok(())
        })?,
    )?;
    m.set(
        "NumLines",
        lua.create_function(|lua, this: Table| with_tip(lua, &this, |t| t.num_lines as i64))?,
    )?;
    // AddFontStrings(left, right) (`0x530c40`): the caller's FontStrings become the next line
    // pair and raise the line limit by one (`0x529940`), keeping the caller's fonts.
    m.set(
        "AddFontStrings",
        lua.create_function(|lua, (this, left, right): (Table, Table, Table)| {
            let lh = region_handle_of(lua, &left)?;
            let rh = region_handle_of(lua, &right)?;
            super::add_font_strings(lua, &this, lh, rh)
        })?,
    )?;
    // SetMinimumWidth(width) (`0x530e50`): the sizer's floor (`SetTooltipMoney` passes the money
    // row's, GameTooltip.lua:91), stored as given (`0x530f05`) and read at the next sizing. A
    // width `lua_isnumber` refuses raises the usage error (`0x530edc`, `0x8550a0`).
    m.set(
        "SetMinimumWidth",
        lua.create_function(|lua, (this, w): (Table, Value)| {
            let Some(w) = lua.coerce_number(w).ok().flatten() else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: SetMinimumWidth(width)".into(),
                ));
            };
            with_tip(lua, &this, |t| t.min_width = w as f32)
        })?,
    )?;
    // SetPadding(width) (`0x530f10`): extra plate width that survives a clear (ItemRefTooltip's 16
    // keeps its close button off the text, ItemRef.xml:40), read by `lua_tonumber` with no type
    // test, so a non-number is 0 (`0x530fa0`), stored as given (`0x530630`) and read at the next
    // sizing.
    m.set(
        "SetPadding",
        lua.create_function(|lua, (this, w): (Table, Value)| {
            let w = lua.coerce_number(w).ok().flatten().unwrap_or(0.0);
            with_tip(lua, &this, |t| t.padding = w as f32)
        })?,
    )?;
    // FadeOut() (`0x531f00`): the fade request ([`super::fade_out`]).
    m.set(
        "FadeOut",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            super::fade_out(lua, h);
            Ok(())
        })?,
    )?;
    // The content methods: items, spells and auras, talents, units.
    crate::script::tooltip_item::install_methods(lua, &m)?;
    crate::script::tooltip_spell::install_methods(lua, &m)?;
    crate::script::talent::install_tooltip_method(lua, &m)?;
    crate::script::tooltip_unit::install_methods(lua, &m)?;

    lua.set_named_registry_value(REG_TOOLTIP_METHODS, m)?;
    Ok(())
}
