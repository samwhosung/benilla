//! Region method-table cluster: **text** — what a `FontString` carries that other text-bearing
//! widgets do not. Split out of `region.rs` at the 0716 file-size budget.
//!
//! **This file is now the FontString-only half.** The ten names it used to hand-write a second copy
//! of — `Set/GetFontObject`, `Set/GetFont`, `Set/GetTextColor`, `Set/GetShadowColor`,
//! `Set/GetShadowOffset` — are not FontString-specific at all: in the binary each is a thin
//! type-guard shim tail-calling one shared implementation, so they come from
//! [`crate::script::font_block`] via one `install` at the bottom of this file, exactly as the
//! EditBox's table gets them. The justify law likewise lives once in [`crate::justify`].
//!
//! What legitimately stays here is the surface a FontString alone has: the string itself
//! (`SetText`/`GetText`), the measured extents, `Set/GetJustifyH`/`V`, `SetNonSpaceWrap`/
//! `CanNonSpaceWrap`, and `SetTextHeight`.
//!
//! The per-property override a region has over its inherited font object is unaffected — that is
//! the severance mask (`FontExplicit`), which the shared block writes the same way this file did.

use mlua::{Lua, Table, Value};

use crate::justify;
use crate::script::Model;

/// Resolve `self` (a region wrapper) to its live [`RegionHandle`].
use super::region_handle_of;

/// Populate `m`'s text methods (see the module doc).
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, Option<mlua::Value>)| {
            // `text_arg`, not `Option<String>`: a Lua string is bytes and a sliced one need not be
            // valid UTF-8 (this raise took the whole handler down).
            let text = crate::script::binding_abi::text_arg(lua, text)?;
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let data = model.region_data.entry(rh).or_default();
            data.text = text;
            // Fresh text draws whole — an armed write-on gradient belongs to the old string.
            data.alpha_gradient = None;
            // SetText is the measure lane's canonical SILENT write (it must not touch the
            // layout — the extent hasn't moved yet), so it names itself on the measure ledger.
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;

    // **No `SetFormattedText`.** A later-expansion name: absent from the client's 32-entry
    // FontString map, from the stock 1.12 chain, and from both addon corpora (2142's census). The
    // era spelling is `SetText(format(fmt, ...))`, and `format` here is already the
    // positional-aware one ([`crate::strings`]), so `%N$s` behaves at the call site the same way
    // it behaved inside this shim.

    // GetText — **an EMPTY string comes back as `nil`, and that substitution is the getter's own**
    // (`FontString:GetText 0x79d690`):
    //
    // ```text
    // 79d72f  mov  eax,[eax+0xf0]        ; the text cell
    // 79d735  test eax,eax
    // 79d739  je   0x79d740              ; NULL      -> substitute
    // 79d73b  cmp  byte ptr [eax],0x0    ; the FIRST-BYTE test
    // 79d73e  jne  0x79d742              ; non-empty -> keep
    // 79d740  xor  eax,eax               ; EMPTY     -> NULL
    // 79d746  call 0x6f3890              ; pushstring; NULL -> pushnil
    // ```
    //
    // So `FontString:GetText` **cannot return `""`**. Not the setter's doing: `SetText 0x771d80`
    // never writes NULL to `+0xf0` on any leg — NULL and `""` share one leg that truncates the
    // buffer in place (`0x771e7e mov byte ptr [eax],0x0`, the pointer surviving) — so the cell
    // really does hold a non-NULL empty string and this getter is what collapses it.
    //
    // The law is **per binding, not per family**, and the two neighbours differ:
    // `Button:GetText 0x780e10` carries the same substitution (`0x780ec5`, three nil conditions —
    // see `button.rs`), while `EditBox:GetText 0x7985c0` carries **none** (`0x79867b` reads
    // `[edit+0x32c]` straight through), which is what stock `MailFrame.lua`'s
    // `GetText() == ""`/`strlen(GetText())` is written against. Do not hoist this.
    //
    // What it costs to get wrong: the stock world map blanks
    // `WorldMapFrameAreaDescription` with `SetText("")` on every POI hover, and Cartographer 2.02
    // reads `if WorldMapFrameAreaDescription:GetText() then` as "this POI has a status line" —
    // answering `""` there put the zone's level range on the description's own line under a
    // whitened label and left it on screen forever.
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

    // GetStringWidth (FontString): the host-measured text extent from the measure
    //
    // **There is no `GetStringHeight` beside it, and that asymmetry is the client's.** 1.12 has no
    // such method on any table — byte-verified absent in every encoding, with `GetStringWidth`
    // itself as the positive control — and Blizzard's own FrameXML calls it zero times. Ours was a
    // byte-identical duplicate of `GetHeight`, which is what the reference uses for this
    // (`0x7a2030` falls through to the same cached measurement), so it was pruned rather than
    // reimplemented (1251).
    // round-trip ([`super::UiScript::set_measured_text`]) — the client asks its font engine for the
    // laid-out string's metrics exactly here, and the tooltip's auto-size sums
    // these to fit its lines. `0` until the string has been measured (a frame's latency; converges).
    // The stored measure only counts while its key matches the CURRENT text/font/wrap
    // ([`RegionData::measure_key`]): after a SetText the old string's width is not this string's
    // metric — serving it is how the whisper header's `GetWidth()` latched the edit-box insets on
    // the previous header's width. A poll-until-nonzero caller (the chat header machine) now
    // converges on the RIGHT measure instead of settling on a stale one.
    // `GetWidth`/`GetHeight` are the neighbouring law and NOT this one ([`super::virtual_span`]):
    // they take the authored size first and the measure only on an axis authored `0` — and on that
    // axis the width they return is *this* number, because the reference reads both getters out of
    // the one cell `[fs+0xfc]` (`0x772890` is `0x79e510`'s callee). So the two agree exactly where
    // the reference makes them agree, and differ exactly where a width was declared.
    // GetStringWidth is the **natural, unwrapped** extent — never the declared box, and never the
    // wrapped one (the reference's getter re-measures the raw text with NO wrap constraint).
    // Unlike `GetWidth` below it deliberately
    // does NOT fall back to a declared `SetWidth`: that width is the very thing a caller
    // asks this to be independent of. A kit that sizes a box from this number and then sets a width
    // on the string — which is what the reference's own `PanelTemplates_TabResize` does — would
    // otherwise read its own output back as its next input and never settle (the
    // macro window's character tab changing width every frame). `0` until measured, as ever.
    fn natural_w(lua: &Lua, this: &Table) -> mlua::Result<f32> {
        let rh = region_handle_of(lua, this)?;
        // Measure NOW if a host font engine is installed — the reference answers this getter from
        // its font engine inline (`0x79e510` → `0x772890`), and a same-tick `SetText` →
        // `GetStringWidth` is the corpus's own idiom (`Bagnon_Forever/database/ui.lua:58-59`).
        // Without an engine this is a no-op and the number below stays 0 until the round-trip.
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

    // SetJustifyH("LEFT"|"CENTER"|"RIGHT") — a FontString's horizontal justification (XML
    // `justifyH`). The token table, the whole-string match, the raise on a miss and the cross-axis
    // clear are all [`crate::justify`]'s, shared with the `<Font>` object's identical pair
    // so the two cannot drift apart again.
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
                // A cross-axis token erases the axis. `GetJustifyH()` then answers "UNKNOWN"
                // while the glyphs draw centred — `0x44d420`'s pre-set `1`.
                justify::Set::Clears => d.justify.clear_h(),
                justify::Set::NoMatch => unreachable!("returned above"),
            }
            // Severance is UNCONDITIONAL on a successful parse — the FontString's own setter
            // `0x79e6b0` stores the per-axis mask `+0x124` at `0x79e780`, *before* the `je`, so
            // the erasing path severs too. Only a failed parse escapes, and that raised already.
            d.font_explicit.justify_h = true;
            Ok(())
        })?,
    )?;

    // SetJustifyV("TOP"|"MIDDLE"|"BOTTOM") — a FontString's vertical justification (XML `justifyV`).
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
                // 13 corpus sites reach this arm: `SetJustifyV("CENTER")` meaning "middle".
                justify::Set::Clears => d.justify.clear_v(),
                justify::Set::NoMatch => unreachable!("returned above"),
            }
            d.font_explicit.justify_v = true;
            Ok(())
        })?,
    )?;

    // GetJustifyH()/GetJustifyV() → **1 string**. Real entries on the reference's FontString table
    // (`0x79e5f0` / `0x79e7f0`, the FontString column of a two-column accessor table) that
    // this side had simply never grown: a FontString could set its justification and not read it
    // back, while the `<Font>` object — the same law, transcribed separately — could do both.
    //
    // An untouched FontString answers CENTER/MIDDLE, which is `RegionData`'s default *and* the
    // client's ctor default `0x212` (`CENTER | MIDDLE | 0x200`) read through each axis mask.
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

    // SetNonSpaceWrap(enable) / CanNonSpaceWrap() — FontString only (`0x79e9f0` / `0x79ead0`).
    //
    // Two contract details, both easy to get wrong:
    //  · the getter is **`CanNonSpaceWrap`**, not `GetNonSpaceWrap`, and it answers **`1` or nil**,
    //    not a boolean — 1.12 predates that convention and an addon may compare against 1.
    //  · a **no-argument call ENABLES** it (the default is on), rather than being a query.
    //
    // `oRA2/Leader/Item.lua:561` is `f.textname:SetNonSpaceWrap(false)`, reached by two addons.
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

    // SetTextHeight(height) — switch the FontString to the scaled-string regime (`0x771600` is the
    // ONLY clearer of the one-to-one bit `0x200`; the literal size then flows through UNCAPPED,
    // magnified from the raster). Stored
    // as the distinct [`RegionData::text_height`] — the font object is untouched, so GetFont
    // keeps reporting the face's own height like the real API.
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
    // `SetFontObject · GetFontObject · SetFont · GetFont · Set/GetTextColor · Set/GetShadowColor ·
    // Set/GetShadowOffset` are **not** FontString-specific. In the binary each is a thin type-guard
    // shim that tail-calls one shared implementation (`0x79f210` SetFont, `0x79f3b0` GetFont, …),
    // which is what [`super::super::font_block`] models — so a FontString and an EditBox are two
    // entry points to one routine, not two routines that happen to agree.
    //
    // They lived here as a hand-written second copy because `region.rs` was over the file-size
    // budget when the block was written; that split has since landed, and `font_block`'s own doc
    // named this collapse as the follow-on. 226 lines of duplicate go with it.
    super::super::font_block::install(lua, m, region_handle_of, "FontString")
}
