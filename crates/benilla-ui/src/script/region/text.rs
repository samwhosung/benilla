//! The FontString-only methods: the string, its measured width, justification, non-space wrap and
//! text height. The font block it shares with EditBox comes from [`crate::script::font_block`].

use mlua::{Lua, Table, Value};

use crate::justify;
use crate::script::Model;

use super::region_handle_of;

/// Install the FontString methods into `m`.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, Option<mlua::Value>)| {
            // `text_arg`, since a Lua string is bytes and a slice of one need not be valid UTF-8.
            let text = crate::script::binding_abi::text_arg(lua, text)?;
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let data = model.region_data.entry(rh).or_default();
            data.text = text;
            // Fresh text draws whole: an armed write-on gradient belongs to the old string.
            data.alpha_gradient = None;
            // SetText must not touch the layout, since the extent has not moved yet; it names
            // itself on the measure ledger.
            model.touch_measure(rh);
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

    // GetStringWidth (`0x79e510`): the natural, unwrapped width of the current text, 0 until
    // measured. Never the declared width, which `PanelTemplates_TabResize` sets from this; on an
    // axis sized 0, `GetWidth` reads the same cell (`[fs+0xfc]`, `0x772890`). 1.12 has no
    // `GetStringHeight`; `GetHeight` (`0x7a2030`) serves.
    fn natural_w(lua: &Lua, this: &Table) -> mlua::Result<f32> {
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
        Ok(d.measured
            .filter(|m| m.key == d.measure_key(scale))
            .map(|m| m.natural_w)
            .unwrap_or(0.0))
    }
    m.set(
        "GetStringWidth",
        lua.create_function(|lua, this: Table| natural_w(lua, &this))?,
    )?;

    // SetJustifyH (XML `justifyH`): the parse, the raise on a miss and the cross-axis clear are
    // `crate::justify`'s, shared with the `<Font>` object.
    m.set(
        "SetJustifyH",
        lua.create_function(|lua, (this, j): (Table, String)| {
            let rh = region_handle_of(lua, &this)?;
            let parsed = justify::parse_h(&j);
            if parsed == justify::Set::NoMatch {
                return Err(justify::usage_h("FontString"));
            }
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let d = model.region_data.entry(rh).or_default();
            match parsed {
                justify::Set::To(jh) => d.justify.set_h(jh),
                // A cross-axis token erases the axis: `GetJustifyH()` answers "UNKNOWN" while the
                // glyphs draw centred (`0x44d420` presets 1).
                justify::Set::Clears => d.justify.clear_h(),
                justify::Set::NoMatch => unreachable!("returned above"),
            }
            // Every successful parse overrides the font object, the erasing one too: `0x79e6b0`
            // sets the per-axis mask (`+0x124`) at `0x79e780`, before the branch.
            d.font_explicit.justify_h = true;
            Ok(())
        })?,
    )?;

    // SetJustifyV (XML `justifyV`), as SetJustifyH.
    m.set(
        "SetJustifyV",
        lua.create_function(|lua, (this, j): (Table, String)| {
            let rh = region_handle_of(lua, &this)?;
            let parsed = justify::parse_v(&j);
            if parsed == justify::Set::NoMatch {
                return Err(justify::usage_v("FontString"));
            }
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let d = model.region_data.entry(rh).or_default();
            match parsed {
                justify::Set::To(jv) => d.justify.set_v(jv),
                // Addons reach this arm with `SetJustifyV("CENTER")`, meaning middle.
                justify::Set::Clears => d.justify.clear_v(),
                justify::Set::NoMatch => unreachable!("returned above"),
            }
            d.font_explicit.justify_v = true;
            Ok(())
        })?,
    )?;

    // GetJustifyH/GetJustifyV (`0x79e5f0`/`0x79e7f0`): one string; CENTER and MIDDLE when
    // untouched, the ctor's `0x212` read through each axis mask.
    fn justify_of(lua: &Lua, this: &Table) -> mlua::Result<justify::Justify> {
        let rh = region_handle_of(lua, this)?;
        let model = lua.app_data_ref::<Model>().expect("model");
        Ok(model
            .region_data
            .get(&rh)
            .map(|d| d.justify)
            .unwrap_or_default())
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
    // answers 1 or nil, and a call with no argument enables.
    m.set(
        "SetNonSpaceWrap",
        lua.create_function(|lua, (this, enable): (Table, Value)| {
            let on = match &enable {
                Value::Nil => true,
                Value::Boolean(b) => *b,
                _ => true,
            };
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().non_space_wrap = Some(on);
            Ok(())
        })?,
    )?;

    m.set(
        "CanNonSpaceWrap",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_mut::<Model>().expect("model");
            let on = model
                .region_data
                .get(&rh)
                .and_then(|d| d.non_space_wrap)
                .unwrap_or(true);
            Ok(if on { Some(1i64) } else { None })
        })?,
    )?;

    // SetTextHeight: the scaled-string mode (`0x771600` alone clears the one-to-one bit `0x200`),
    // the size magnified from the raster, uncapped. The font is untouched, so GetFont keeps the
    // face's own height.
    m.set(
        "SetTextHeight",
        lua.create_function(|lua, (this, height): (Table, f32)| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().text_height = Some(height);
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;

    // ── the shared font block ───────────────────────────────────────────────────────────────
    //
    // The font object, font, text colour and shadow getters and setters are type-guard shims over
    // one implementation each (`0x79f210` SetFont, `0x79f3b0` GetFont), shared with EditBox.
    super::super::font_block::install(lua, m, region_handle_of, "FontString")
}
