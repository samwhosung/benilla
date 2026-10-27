//! The FontString-only methods: the string, its measured width, justification, non-space wrap and
//! text height. The font block it shares with EditBox comes from [`crate::script::font_block`].

use mlua::{Lua, MultiValue, Table, Value};

use crate::justify;
use crate::script::binding_abi;
use crate::script::Model;

use super::region_handle_of;

/// The justify word a string, or a message frame's font holder, carries: the ctor's `0x212`
/// (CENTER and MIDDLE) until set (`0x770dd3`, `0x783a87`).
pub(crate) fn justify_word(model: &Model, rh: crate::widget::RegionHandle) -> justify::Justify {
    model
        .region_data
        .get(&rh)
        .map(|d| d.justify)
        .unwrap_or_default()
}

/// `SetJustifyH` and `SetJustifyV` on the string or font holder `rh` makes: the token is parsed
/// (`0x6f1990`) and a miss raises the usage line (`0x87c77c`, `0x87c7a0`), then only the setter's
/// axis of the word is replaced (`0x79fc57`-`0x79fc64`, `0x79fd1d`), a token of the other axis
/// erasing it: `GetJustifyH()` then answers "UNKNOWN" while the glyphs draw centred (`0x44d420`
/// presets 1). A successful parse overrides the font object, the erasing one too: `0x79e6b0`
/// clears the axis of the per-bit mask (`+0x124`, `0x79e779`) and the group bit (`0x79e7a5`), so
/// the font no longer reaches the other axis either.
pub(crate) fn set_justify(
    lua: &Lua,
    this: &Table,
    token: &Value,
    horizontal: bool,
    region: impl FnOnce(&mut Model) -> Option<crate::widget::RegionHandle>,
) -> mlua::Result<()> {
    let name = || binding_abi::name_of(lua, this);
    let Some(bits) = binding_abi::optional_string(lua, token).and_then(|t| justify::parse_bits(&t))
    else {
        return Err(if horizontal {
            justify::usage_h(&name())
        } else {
            justify::usage_v(&name())
        });
    };
    let mask = if horizontal {
        justify::H_MASK
    } else {
        justify::V_MASK
    };
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let Some(rh) = region(&mut model) else {
        return Err(mlua::Error::runtime(crate::script::object::ERR_WRONG_TYPE));
    };
    let d = model.region_data.entry(rh).or_default();
    d.justify.0 = justify::set_axis(d.justify.0, mask, bits);
    d.justify_mask.0 &= !mask;
    d.font_mask.clear(super::super::FontMask::JUSTIFY);
    Ok(())
}

/// `CSimpleFontString::SetText` (`0x771d80`) past Lua's font check: the loader's `text=`
/// (`0x77103a`) reaches it directly, so a string with no font still holds its text.
pub(crate) fn store_text_of(lua: &Lua, this: &Table, text: Option<String>) -> mlua::Result<()> {
    let rh = region_handle_of(lua, this)?;
    store_text(lua, rh, text);
    Ok(())
}

/// [`store_text_of`] by handle, the tail Lua's `SetText` shares.
fn store_text(lua: &Lua, rh: crate::widget::RegionHandle, text: Option<String>) {
    let mut model = lua.app_data_mut::<Model>().expect("model");
    // The grammar rewrite and the separator pass run before the same-text test (`0x771d98`,
    // `0x771d9f`, `0x771dea`).
    let text = text.map(|t| model.localized_text(t));
    let data = model.region_data.entry(rh).or_default();
    let was_shown = data.text.as_deref().is_some_and(|t| !t.is_empty());
    let same = data.text == text;
    data.text = text;
    // Fresh text draws whole: an armed write-on gradient belongs to the old string.
    data.alpha_gradient = None;
    // New text names itself on the measure ledger, and on the layout only when it empties or
    // fills, which drops or takes up the measure the sweep reads.
    model.touch_measure(rh);
    model.touch_layout_text(rh, was_shown);
    // A new string clears the valid bit and invalidates (`0x771ea4`); the same one returns first
    // (`0x771df1`).
    if !same {
        model.clear_region_valid(rh);
    }
}

/// Install the FontString methods into `m`.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, mlua::Value)| {
            let rh = region_handle_of(lua, &this)?;
            // A string with no font raises before its argument is read and sets nothing
            // (`0x79d7e5`, `0x79d810`), naming the receiver or `<unnamed>`.
            let fonted = {
                let model = lua.app_data_ref::<Model>().expect("model");
                crate::script::font_block::has_font(&model, rh)
            };
            if !fonted {
                return Err(mlua::Error::runtime(format!(
                    "{}:SetText(): Font not set",
                    crate::script::binding_abi::name_of(lua, &this)
                )));
            }
            // `lua_tostring` (`0x79d81f`) answers NULL for anything but a string or a number
            // (`0x6f36c5`), and `0x771d80(NULL, 1)` empties the text: a boolean or a table clears
            // as nil does and never raises. Lossy, as a Lua string is bytes.
            let text = crate::script::binding_abi::optional_string(lua, &text);
            store_text(lua, rh, text);
            Ok(())
        })?,
    )?;

    // No `SetFormattedText`: not a 1.12 verb; 1.12 writes `SetText(format(fmt, ...))`.

    // GetText answers nil for "" (`0x79d690`, its first-byte test at `0x79d73b`), though `SetText`
    // (`0x771d80`) keeps a non-NULL empty buffer (`0x771e7e`). The rule is per binding, not per
    // family: `Button:GetText` (`0x780e10`, `0x780ec5`) does the same, `EditBox:GetText`
    // (`0x7985c0`, `0x79867b`) does not, and stock `MailFrame.lua:521` compares that one with "".
    m.set(
        "GetText",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let text = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model
                    .region_data
                    .get(&rh)
                    .and_then(|d| d.text.clone())
                    .filter(|t| !t.is_empty())
            };
            match text {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetStringWidth (`0x79e510`): the natural, unwrapped width of the current text through the
    // getter steps, 0 until measured. Never the declared width, which `PanelTemplates_TabResize`
    // sets from this; on an axis sized 0, `GetWidth` reads the same cell (`[fs+0xfc]`,
    // `0x772890`), so the two agree. 1.12 has no `GetStringHeight`; `GetHeight` (`0x7a2030`)
    // serves.
    fn natural_w(lua: &Lua, this: &Table) -> mlua::Result<f64> {
        let rh = region_handle_of(lua, this)?;
        // Measure now when a host font engine is installed, as the reference measures inline, so
        // `SetText` then `GetStringWidth` works in one tick; without one it is 0 until the host
        // answers.
        crate::script::measure::ensure_measured(lua, rh);
        let model = lua.app_data_ref::<Model>().expect("model");
        let Some(d) = model.region_data.get(&rh) else {
            return Ok(0.0);
        };
        let scale = model
            .arena
            .region(rh)
            .and_then(|r| model.arena.frame(r.owner))
            .map(|f| f.effective_scale)
            .unwrap_or(1.0);
        let units = model.layout_units();
        Ok(units.load(
            units.store(
                d.measured
                    .filter(|m| m.key == d.measure_key(scale, units))
                    .map(|m| m.natural_w)
                    .unwrap_or(0.0),
            ),
        ))
    }
    m.set(
        "GetStringWidth",
        lua.create_function(|lua, this: Table| natural_w(lua, &this))?,
    )?;

    // SetJustifyH (XML `justifyH`) and SetJustifyV: the parse, the raise on a miss and the
    // cross-axis clear are `crate::justify`'s, shared with the `<Font>` object.
    for (name, horizontal) in [("SetJustifyH", true), ("SetJustifyV", false)] {
        m.set(
            name,
            lua.create_function(move |lua, (this, j): (Table, Value)| {
                let rh = region_handle_of(lua, &this)?;
                set_justify(lua, &this, &j, horizontal, |_| Some(rh))
            })?,
        )?;
    }

    // GetJustifyH/GetJustifyV (`0x79e5f0`/`0x79e7f0`): one string; CENTER and MIDDLE when
    // untouched, the ctor's `0x212` read through each axis mask.
    fn justify_of(lua: &Lua, this: &Table) -> mlua::Result<justify::Justify> {
        let rh = region_handle_of(lua, this)?;
        let model = lua.app_data_ref::<Model>().expect("model");
        Ok(justify_word(&model, rh))
    }
    m.set(
        "GetJustifyH",
        lua.create_function(|lua, this: Table| Ok(justify_of(lua, &this)?.name_h()))?,
    )?;
    m.set(
        "GetJustifyV",
        lua.create_function(|lua, this: Table| Ok(justify_of(lua, &this)?.name_v()))?,
    )?;

    // SetNonSpaceWrap/CanNonSpaceWrap (`0x79e9f0`/`0x79ead0`), FontString only: the getter
    // answers 1 or nil, off from the constructor (`0x770dd3`). The flag is `GetBoolOrDefault`
    // (`0x6f1c10`, default 1, `0x79ea7e`): a call with no argument enables, an explicit nil
    // disables.
    m.set(
        "SetNonSpaceWrap",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let on = crate::script::binding_abi::bool_or_default(args.front(), true);
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().non_space_wrap = on;
            // The flag moves the wrap, so the measure.
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;

    m.set(
        "CanNonSpaceWrap",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let on = model.region_data.get(&rh).is_some_and(|d| d.non_space_wrap);
            Ok(if on { Some(1i64) } else { None })
        })?,
    )?;

    // SetTextHeight: the scaled-string mode (`0x771600` alone clears the one-to-one bit `0x200`),
    // the size magnified from the raster, uncapped. It writes the text height a font load writes
    // (`+0xe4`, `0x77163b`), so `GetFont` answers it and the next font push replaces it; a height
    // within 2^-22 of the current one changes nothing (`0x77161c`), the bit included. No inherit
    // bit is cleared. A `lua_isnumber` miss raises `Usage: %s:SetTextHeight(pixelHeight)`
    // (`0x87c728`, `0x79e4f9`); a height that compares not above 0 (`0x79e469`, `fcomp` against
    // `0x7ffd74` = 0.0, taken on greater or unordered only) raises `%s:SetTextHeight(): invalid
    // text height: %f` (`0x87c750`, `0x79e49f`), the `%f` a Lua number printed `%.14g`.
    m.set(
        "SetTextHeight",
        lua.create_function(|lua, (this, height): (Table, Value)| {
            let rh = region_handle_of(lua, &this)?;
            let wide =
                binding_abi::method_number(lua, &this, height, "SetTextHeight(pixelHeight)")?;
            // The compare reads the double (`0x79e466` `fst` stores a float copy, `fcomp` tests the
            // register), so a height too small for a float still passes; the text prints the copy.
            let height = wide as f32;
            if wide <= 0.0 {
                return Err(mlua::Error::runtime(format!(
                    "{}:SetTextHeight(): invalid text height: {}",
                    binding_abi::name_of(lua, &this),
                    binding_abi::lua_number_text(f64::from(height))
                )));
            }
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let model_units = model.layout_units();
            let d = model.region_data.entry(rh).or_default();
            let held = d.font_height.map_or(0.0, |h| h.script(model_units));
            if (height - held).abs() < f32::from_bits(0x3480_0000) {
                return Ok(());
            }
            d.font_height = Some(crate::layout::Len::Script(height));
            d.text_scaled = true;
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;

    // ── the shared font block ───────────────────────────────────────────────────────────────
    //
    // The font object, font, text colour, shadow and spacing getters and setters are type-guard
    // shims over one implementation each (`0x79f210` SetFont, `0x79f3b0` GetFont), shared with
    // EditBox; the spacing pair is the FontString's own (`0x79e2b0`, `0x79e1d0`), which clamps.
    super::super::font_block::install(
        lua,
        m,
        region_handle_of,
        super::super::font_block::FontKind::FontString,
        None,
    )
}
