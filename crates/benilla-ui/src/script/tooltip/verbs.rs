//! The GameTooltip Lua methods over the parent module's line pool, plus the content builders'.

use mlua::{Lua, Table, Value, Variadic};

use crate::layout::{Anchor, Point};
use crate::script::binding_abi::optional_string;
use crate::script::object::{frame_handle_of, frame_wrapper};
use crate::script::region::region_handle_of;
use crate::script::Model;
use crate::widget::{KindState, TooltipAnchor, TOOLTIP_LINE_GAP, TOOLTIP_PAD};

use super::{
    append_line, bool_arg, clear_content, fire_cleared, full_alpha, hide_tooltip, now,
    parse_line_color, parse_line_tail, set_shown, text_of, tip_mut, with_tip, write_cell,
    REG_TOOLTIP_METHODS,
};

pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // SetOwner(owner, anchorType [, x, y]): the binding `0x5310d0` calls the core `0x52ffe0`,
    // which sets full alpha (`0x52fff4`), disarms the fade, stores the owner (`0x53000c`), mode
    // and offsets, then applies the anchor (`0x52fe90`). The content clears and OnTooltipCleared
    // fires, so a new owner never inherits the last hover's lines or money.
    //
    // A missing, non-string or unknown anchorType is mode 0, ANCHOR_LEFT, with no error: the
    // binding's local starts at 0 (`0x53120d`). The core's pre-store of 7 (`0x530012`) survives
    // only a NULL owner, the un-own path (`0x530a60`) of every hide.
    //
    // The anchor apply returns first for ANCHOR_PRESERVE (8, `0x52fead`), then clears all points
    // (`0x767ed0` at `0x52fec2`) for modes 0..7; modes 0..5 then set a point (`0x52ffbc`).
    m.set(
        "SetOwner",
        lua.create_function(
            |lua, (this, owner, anchor, x, y): (Table, Table, Value, Option<f32>, Option<f32>)| {
                let h = frame_handle_of(lua, &this)?;
                let owner_h = frame_handle_of(lua, &owner)?;
                // The gate is `lua_isstring` (`0x6f3510`): a string or number is read, and any
                // other type counts as absent rather than raising.
                let anchor = optional_string(lua, &anchor);
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    clear_content(&mut model, h);
                    full_alpha(&mut model, h);
                    tip_mut(&mut model, h)?.owner = Some(owner_h);
                    let owner_id = model.frame_id(owner_h);
                    // Resolved once: the placement and `GetAnchorType` both read this value. The
                    // warning on an unknown name is ours; the reference is silent, and Lua sees no
                    // difference.
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
                            model.record_warning(format!(
                                "SetOwner: unknown anchor '{other}' (mode 0, ANCHOR_LEFT)"
                            ));
                            TooltipAnchor::Left
                        }
                    };
                    tip_mut(&mut model, h)?.anchor = anchor;
                    let pts = match anchor {
                        TooltipAnchor::Right => Some((Point::BottomLeft, Point::TopRight)),
                        TooltipAnchor::Left => Some((Point::BottomRight, Point::TopLeft)),
                        TooltipAnchor::TopRight => Some((Point::BottomRight, Point::TopRight)),
                        TooltipAnchor::TopLeft => Some((Point::BottomLeft, Point::TopLeft)),
                        TooltipAnchor::BottomRight => Some((Point::TopLeft, Point::BottomRight)),
                        TooltipAnchor::BottomLeft => Some((Point::TopRight, Point::BottomLeft)),
                        TooltipAnchor::Cursor | TooltipAnchor::None | TooltipAnchor::Preserve => {
                            None
                        }
                    };
                    // Modes 6 and 7 clear all points and stop. The clear lets
                    // `GameTooltip_SetDefaultAnchor` (GameTooltip.lua:76-77) set one point with no
                    // stale owner anchor; mode 6 is then placed per frame by `cursor_anchor`. The
                    // drop is a named retarget to no targets, not a conservative touch.
                    if matches!(anchor, TooltipAnchor::Cursor | TooltipAnchor::None) {
                        let dropped = match model.layout_inputs.get_mut(&h) {
                            Some(input) if !input.anchors.is_empty() => {
                                Some(input.anchors.drain(..).map(|a| a.relative_to).collect())
                            }
                            _ => None,
                        };
                        if let Some(old_targets) = dropped {
                            let old_targets: Vec<u32> = old_targets;
                            model.touch_layout_retarget_frame(h, &old_targets, &[]);
                        }
                    }
                    if let Some((own, rel)) = pts {
                        let new =
                            Anchor::new(own, owner_id, rel, x.unwrap_or(0.0), y.unwrap_or(0.0));
                        let input = model.layout_inputs.entry(h).or_default();
                        // No-op when unchanged, so the bag hover's per-frame SetOwner does not
                        // dirty the layout.
                        let same = input.anchors.len() == 1
                            && crate::script::object::anchor_bits_eq(&input.anchors[0], &new);
                        if !same {
                            // A named retarget: the cached layout graph re-points this edge
                            // instead of being rebuilt.
                            let old_targets: Vec<u32> =
                                input.anchors.iter().map(|a| a.relative_to).collect();
                            input.anchors = vec![new];
                            model.touch_layout_retarget_frame(h, &old_targets, &[owner_id]);
                        }
                    }
                }
                fire_cleared(lua, h);
                Ok(())
            },
        )?,
    )?;
    // GameTooltip:BenillaGetTooltipOwner(): not a 1.12 method (1.12 has only `IsOwned`); the dev
    // hover recorder (`benilla-app/src/hover_log.rs`) reads the owner's name through it.
    m.set(
        "BenillaGetTooltipOwner",
        lua.create_function(|lua, this: Table| {
            let owner = with_tip(lua, &this, |t| t.owner)?;
            let id = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                owner
                    .filter(|&h| model.arena.frame(h).is_some())
                    .map(|h| model.frame_id(h))
            };
            match id {
                Some(id) => Ok(Value::Table(frame_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    // GetAnchorType(): the last SetOwner's mode by name (`0x5313e0`, name table `0x531530`), never
    // nil; a never-owned tooltip answers "ANCHOR_NONE". The reference also answers it after any
    // hide, whose un-own leaves mode 7; this keeps the last mode. Addons compare it with what they
    // passed to SetOwner, so the name must round-trip.
    m.set(
        "GetAnchorType",
        lua.create_function(|lua, this: Table| {
            let anchor = with_tip(lua, &this, |t| t.anchor)?;
            Ok(anchor.name())
        })?,
    )?;
    // IsOwned(frame): the hover loop's OnUpdate gate (`ContainerFrame.lua:657`); the owner drops
    // on hide, so a stale OnUpdate never brings a hidden tooltip back.
    m.set(
        "IsOwned",
        lua.create_function(|lua, (this, frame): (Table, Table)| {
            let owner = with_tip(lua, &this, |t| t.owner)?;
            let fh = frame_handle_of(lua, &frame)?;
            Ok(owner == Some(fh))
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
            let text = text_of(args.first());
            let [r, g, b, _] = parse_line_color(args.get(1), args.get(2), args.get(3));
            let a = match args.get(4) {
                Some(Value::Number(n)) => *n as f32,
                Some(Value::Integer(i)) => *i as f32,
                _ => 1.0,
            };
            let wrap = bool_arg(args.get(5));
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            append_line(lua, &this, (text, [r, g, b, a]), None, wrap)?;
            set_shown(lua, h, true);
            Ok(())
        })?,
    )?;
    // AppendText(text) (`0x531e30`): appends to line 1's left cell in place, keeping its colour,
    // and does nothing with no lines, as the reference's body does (`0x5305b0`, line 1 at
    // `0x5305cc`). The bag tooltips hang a keybinding on their SetText title with it
    // (ContainerFrame.xml:198, MainMenuBarBagButtons.lua:91).
    m.set(
        "AppendText",
        lua.create_function(|lua, (this, text): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let extra = text_of(Some(&text));
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let lh = match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Tooltip(t)) if t.num_lines > 0 => t.left_lines[0],
                Some(KindState::Tooltip(_)) => return Ok(()),
                _ => return Err(mlua::Error::runtime("not a GameTooltip")),
            };
            let (current, color) = match model.region_data.get(&lh) {
                Some(d) => (
                    d.text.clone().unwrap_or_default(),
                    d.vertex_color.unwrap_or([1.0, 1.0, 1.0, 1.0]),
                ),
                None => (String::new(), [1.0, 1.0, 1.0, 1.0]),
            };
            write_cell(&mut model, lh, &(current + &extra), color);
            Ok(())
        })?,
    )?;
    // AddLine(text [, r, g, b [, wrap]]): positional, and appends without showing; the
    // `(text, "", r, g, b)` shape has a non-number r-slot, so it renders default gold.
    m.set(
        "AddLine",
        lua.create_function(|lua, (this, args): (Table, Variadic<Value>)| {
            let text = text_of(args.first());
            let (color, wrap) = parse_line_tail(&args[1.min(args.len())..]);
            append_line(lua, &this, (text, color), None, wrap)
        })?,
    )?;
    // AddDoubleLine(textL, textR, rL, gL, bL, rR, gR, bR [, wrap]) (`0x531840`): each side's
    // colour gates on its own r-slot. The core forces wrap off when the right text is non-empty;
    // this never wraps.
    m.set(
        "AddDoubleLine",
        lua.create_function(
            |lua, (this, tl, tr, cl): (Table, Value, Value, Variadic<Value>)| {
                let left_color = parse_line_color(cl.first(), cl.get(1), cl.get(2));
                let right_color = parse_line_color(cl.get(3), cl.get(4), cl.get(5));
                append_line(
                    lua,
                    &this,
                    (text_of(Some(&tl)), left_color),
                    Some((text_of(Some(&tr)), right_color)),
                    false,
                )
            },
        )?,
    )?;
    m.set(
        "ClearLines",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            Ok(())
        })?,
    )?;
    m.set(
        "NumLines",
        lua.create_function(|lua, this: Table| with_tip(lua, &this, |t| t.num_lines as i64))?,
    )?;
    // AddFontStrings(left, right) (`0x530c40`): adopts two caller-made FontStrings as the next
    // line pair, placed like an engine-grown pair but keeping the caller's fonts.
    m.set(
        "AddFontStrings",
        lua.create_function(|lua, (this, left, right): (Table, Table, Table)| {
            let h = frame_handle_of(lua, &this)?;
            let lh = region_handle_of(lua, &left)?;
            let rh = region_handle_of(lua, &right)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let frame_id = model.frame_id(h);
            let prev_left = match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Tooltip(t)) => t.left_lines.last().copied(),
                _ => return Err(mlua::Error::runtime("not a GameTooltip")),
            };
            let anchor = match prev_left {
                None => Anchor::new(
                    Point::TopLeft,
                    frame_id,
                    Point::TopLeft,
                    TOOLTIP_PAD,
                    -TOOLTIP_PAD,
                ),
                Some(prev) => {
                    let prev_id = model.region_id(prev);
                    Anchor::new(
                        Point::TopLeft,
                        prev_id,
                        Point::BottomLeft,
                        0.0,
                        -TOOLTIP_LINE_GAP,
                    )
                }
            };
            let left_id = model.region_id(lh);
            {
                let d = model.region_data.entry(lh).or_default();
                d.hidden = true;
                d.justify.set_h(crate::script::JustifyH::Left);
                d.anchors = vec![anchor];
            }
            {
                let d = model.region_data.entry(rh).or_default();
                d.hidden = true;
                d.justify.set_h(crate::script::JustifyH::Right);
                d.anchors = vec![Anchor::new(Point::Right, left_id, Point::Right, 0.0, 0.0)];
            }
            model.touch_layout(); // two line rows entered the layout graph
            let t = tip_mut(&mut model, h)?;
            t.left_lines.push(lh);
            t.right_lines.push(rh);
            Ok(())
        })?,
    )?;
    // SetMinimumWidth(w): a floor on the auto-sized width (`SetTooltipMoney` passes the money
    // row's, GameTooltip.lua:91).
    m.set(
        "SetMinimumWidth",
        lua.create_function(|lua, (this, w): (Table, f32)| {
            with_tip(lua, &this, |t| t.min_width = w.max(0.0))
        })?,
    )?;
    // SetPadding(w): extra auto-size width that survives a content clear (ItemRefTooltip's 16
    // keeps its close button off the text, ItemRef.xml:40).
    m.set(
        "SetPadding",
        lua.create_function(|lua, (this, w): (Table, f32)| {
            with_tip(lua, &this, |t| t.padding = w.max(0.0))
        })?,
    )?;
    // FadeOut(): alpha ramps to 0 over `TOOLTIP_FADE_SECS`, then the full hide; new content or
    // Show cancels it.
    m.set(
        "FadeOut",
        lua.create_function(|lua, this: Table| {
            let t = now(lua);
            with_tip(lua, &this, |tip| {
                if tip.fade_start.is_none() {
                    tip.fade_start = Some(t);
                }
            })
        })?,
    )?;
    // Show and Hide shadow the shared pair. Show (`0x530a80`, the override the generic Show at
    // `0x7a3350` calls) shows only a plate with an owner and at least one line, checked at Show
    // time only; otherwise it takes the full hide (`0x530a60`): un-owned, cleared,
    // OnTooltipCleared fired. Hide is the full hide.
    m.set(
        "Show",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let live = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                full_alpha(&mut model, h);
                tip_mut(&mut model, h)
                    .map(|t| t.owner.is_some() && t.num_lines > 0)
                    .unwrap_or(false)
            };
            match live {
                true => set_shown(lua, h, true),
                false => hide_tooltip(lua, h),
            }
            Ok(())
        })?,
    )?;
    m.set(
        "Hide",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            hide_tooltip(lua, h);
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
