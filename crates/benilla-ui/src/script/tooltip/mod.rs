//! The `GameTooltip` method surface + engine behavior (decision 0274): the line stack, the
//! owner/anchor law, auto-size, and the fade — the modeled behavior of the client's game-layer
//! tooltip class over `CSimpleFrame` (the reference's full 38-binding Lua surface, plus its
//! money/cooldown internals; the *content* builders land per 0274's phases on top of these
//! mechanics).
//!
//! **Lines are real named FontString regions**, engine-created on demand and published as Lua
//! globals (`<name>TextLeft1` …) — reference Lua addresses them by name
//! (`GameTooltipTextLeft1:SetTextColor`, UnitFrame.lua), so they cannot be private state. The
//! real template pre-declares 30 pairs and the class grows past them with `AddFontStrings`;
//! benilla uses the one growth mechanism for all of them.
//!
//! **Layout is a `resolve` pre-pass** ([`layout_tooltips`]): frame size = max measured line width
//! (+ the double-line gap) + 2·pad × summed line heights + gaps, with `SetMinimumWidth` as a
//! floor, and each double line's right column re-pointed flush to the text inset — the job the
//! real client's C++ line layout does over the template's static anchors. It reads the measure
//! round-trip's cached extents, so a fresh tooltip converges in ~2 frames (same as the retired
//! Lua `OnUpdate` measure loop it replaces).
//!
//! `AddLine` accepts both shipped-FrameXML shapes — `(text, r, g, b[, wrap])` and the archaic
//! `(text, "", r, g, b)` (both appear across the real 1.12 corpus; the non-number second slot is
//! skipped). A wrap-flagged line has [`crate::widget::TOOLTIP_WRAP_WIDTH`] pinned onto its
//! region at APPEND time, so its very first measure comes back wrapped — one round-trip, stable
//! under the hover re-enter loop's per-frame content clears (the item description /
//! trigger-effect lines).

use mlua::{Lua, Table, Value};

use super::event;
use super::object::{frame_handle_of, publish_global};
use super::region::region_wrapper;
use super::{FontObject, Model, RegionData};
use crate::layout::{Anchor, Point};
use crate::order::DrawLayer;
use crate::widget::{
    FrameHandle, KindState, RegionKind, TooltipAnchor, TooltipState, TOOLTIP_DOUBLE_GAP,
    TOOLTIP_FADE_SECS, TOOLTIP_LINE_GAP, TOOLTIP_PAD,
};

/// Registry key of the GameTooltip method table (the MAXCSTACK discipline: named registry root).
pub(super) const REG_TOOLTIP_METHODS: &str = "__benilla_tooltip_methods";

/// Run `f` over a frame's tooltip state under one short write borrow. Errors if `this` is not a
/// live GameTooltip (unreachable through the kind dispatcher, but the table is a plain Lua value).
fn with_tip<T>(lua: &Lua, this: &Table, f: impl FnOnce(&mut TooltipState) -> T) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    tip_mut(&mut model, h).map(f)
}

pub(super) fn tip_mut(model: &mut Model, h: FrameHandle) -> mlua::Result<&mut TooltipState> {
    match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        Some(KindState::Tooltip(t)) => Ok(t),
        _ => Err(mlua::Error::runtime("not a GameTooltip")),
    }
}

/// Copy a named font object's resolved paint onto a line region (the `SetFontObject` copy,
/// minus the error on a missing name: engine tests load file subsets where Fonts.xml may be
/// absent — an unregistered name leaves the renderer's default face rather than failing the
/// hover).
fn apply_font(d: &mut RegionData, name: &str, fo: Option<&FontObject>) {
    d.font_object = Some(name.to_string());
    // The severance mask is left standing — a re-point does NOT restore inheritance in the
    // reference (see `super::font`'s module doc on the `+0x2c` inheritMask). A freshly created
    // line has a clean mask anyway; an adopted XML-declared line keeps whatever its own attributes
    // severed. Either way the line inherits the object LIVE from here: a later
    // `GameTooltipText:SetFont(…)` re-paints every tooltip line through `font::propagate`.
    if let Some(f) = fo {
        d.font_path = f.font.clone();
        d.font_height = f.height;
        d.outline = f.outline;
        d.font_shadow = f.shadow;
        if let Some(c) = f.color {
            d.vertex_color = Some(c);
        }
    }
}

/// Ensure the line-pair pool holds at least `n` pairs — ADOPTING XML-declared pairs
/// (`<name>TextLeft<i>`/`TextRight<i>`, the ref template's pre-declared ladder: their faces
/// ride, the engine owns justify/visibility/anchors) and creating the rest (faces cloned from
/// the previous pair when a ladder exists, else the header/text defaults).
/// Pair `i` (1-based): left at TOPLEFT (pad,−pad) of the frame (i = 1) or the previous left's
/// BOTTOMLEFT (0,−gap); right at RIGHT→left RIGHT (its x offset is [`layout_tooltips`]'s to
/// recompute). Line 1 wears `GameTooltipHeaderText`, the rest `GameTooltipText` — the real
/// template's fonts.
fn ensure_lines(lua: &Lua, this: &Table, n: usize) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    // (id, global name) pairs to publish once the model borrow is dropped.
    let mut publish: Vec<(u32, String)> = Vec::new();
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let frame_name = model
            .arena
            .frame(h)
            .and_then(|f| f.name.clone())
            .unwrap_or_default();
        let frame_id = model.frame_id(h);
        loop {
            let (have, prev_left, prev_right, clone_prev) =
                match model.arena.frame(h).map(|f| &f.kind_state) {
                    Some(KindState::Tooltip(t)) => (
                        t.left_lines.len(),
                        t.left_lines.last().copied(),
                        t.right_lines.last().copied(),
                        t.xml_declared_lines,
                    ),
                    _ => return Err(mlua::Error::runtime("not a GameTooltip")),
                };
            if have >= n {
                break;
            }
            let i = have + 1;

            // An XML-declared pair for this index? The ref template mechanism: FrameXML declares
            // the leading pairs (ShoppingTooltipTemplate's small-font ladder, TextLeft1..N), the
            // class ADOPTS them and, past the ladder, grows lines that clone the previous pair's
            // faces. GameTooltip declares none, so the header/text defaults hold there.
            let declared = |model: &Model, cell: &str| -> Option<crate::widget::RegionHandle> {
                if frame_name.is_empty() {
                    return None;
                }
                let id = model
                    .region_names
                    .get(&format!("{frame_name}Text{cell}{i}"))
                    .copied()?;
                let rh = model.id_to_region.get(&id).copied()?;
                (model.arena.region(rh)?.owner == h).then_some(rh)
            };
            // Font source for a CREATED cell: the previous line's declared/cloned faces when the
            // frame carries an XML ladder, else the engine defaults.
            let seed_font =
                |model: &mut Model,
                 d: &mut RegionData,
                 prev: Option<crate::widget::RegionHandle>| {
                    let cloned = clone_prev
                        .then(|| prev.and_then(|p| model.region_data.get(&p)).cloned())
                        .flatten();
                    match cloned {
                        Some(src) => {
                            d.font_object = src.font_object.clone();
                            d.font_path = src.font_path.clone();
                            d.font_height = src.font_height;
                            d.outline = src.outline;
                            d.font_shadow = src.font_shadow;
                            d.vertex_color = src.vertex_color;
                        }
                        None => {
                            let font = if i == 1 {
                                "GameTooltipHeaderText"
                            } else {
                                "GameTooltipText"
                            };
                            let fo = model.font_object(font).cloned();
                            apply_font(d, font, fo.as_ref());
                        }
                    }
                };

            let adopted_left = declared(&model, "Left");
            let left = match adopted_left {
                Some(rh) => rh,
                None => model
                    .arena
                    .create_region(h, RegionKind::FontString, DrawLayer::Artwork, 0)
                    .ok_or_else(|| mlua::Error::runtime("dead tooltip frame"))?,
            };
            {
                let mut d = if adopted_left.is_some() {
                    // Keep the declared faces; the engine owns justify/visibility/anchors.
                    model.region_data.remove(&left).unwrap_or_default()
                } else {
                    let mut d = RegionData {
                        hidden: true,
                        ..RegionData::default()
                    };
                    seed_font(&mut model, &mut d, prev_left);
                    d
                };
                d.hidden = true;
                // Left-justified like the template's line stack — invisible while the rect hugs
                // the measured text, load-bearing once the wrap consumer pins a region to the
                // wrap column.
                d.justify.set_h(super::JustifyH::Left);
                d.anchors = vec![match prev_left {
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
                }];
                model.region_data.insert(left, d);
                model.touch_measure(left); // an adopted cell can arrive text-in-hand
                model.touch_layout(); // a line row entered the layout graph (decision 0740)
            }
            let left_id = model.region_id(left);

            let adopted_right = declared(&model, "Right");
            let right = match adopted_right {
                Some(rh) => rh,
                None => model
                    .arena
                    .create_region(h, RegionKind::FontString, DrawLayer::Artwork, 0)
                    .ok_or_else(|| mlua::Error::runtime("dead tooltip frame"))?,
            };
            {
                let mut d = if adopted_right.is_some() {
                    model.region_data.remove(&right).unwrap_or_default()
                } else {
                    let mut d = RegionData {
                        hidden: true,
                        ..RegionData::default()
                    };
                    seed_font(&mut model, &mut d, prev_right);
                    d
                };
                d.hidden = true;
                d.justify.set_h(super::JustifyH::Right);
                d.anchors = vec![Anchor::new(Point::Right, left_id, Point::Right, 0.0, 0.0)];
                model.region_data.insert(right, d);
                model.touch_measure(right); // an adopted cell can arrive text-in-hand
                model.touch_layout(); // a line row entered the layout graph (decision 0740)
            }
            let right_id = model.region_id(right);

            if !frame_name.is_empty() {
                // Adopted pairs are already named + published by the loader.
                if adopted_left.is_none() {
                    let ln = format!("{frame_name}TextLeft{i}");
                    model.region_names.entry(ln.clone()).or_insert(left_id);
                    publish.push((left_id, ln));
                }
                if adopted_right.is_none() {
                    let rn = format!("{frame_name}TextRight{i}");
                    model.region_names.entry(rn.clone()).or_insert(right_id);
                    publish.push((right_id, rn));
                }
            }

            let t = tip_mut(&mut model, h)?;
            t.left_lines.push(left);
            t.right_lines.push(right);
            if i == 1 && adopted_left.is_some() {
                t.xml_declared_lines = true;
            }
        }
    }
    for (id, name) in publish {
        let wrapper = region_wrapper(lua, id)?;
        publish_global(lua, &name, &wrapper)?;
    }
    Ok(())
}

/// One text cell's write: text + color + shown. `color` alpha defaults opaque.
pub(super) fn write_cell(
    model: &mut Model,
    rh: crate::widget::RegionHandle,
    text: &str,
    color: [f32; 4],
) {
    let d = model.region_data.entry(rh).or_default();
    d.text = Some(text.to_string());
    d.vertex_color = Some(color);
    // `AddLine("…", r, g, b)`'s colour is this line's OWN colour, exactly as if Lua had called
    // `SetTextColor` on the cell — so it must survive a later mutation of the font object the
    // line inherits ([`super::types::FontExplicit`]). Without the mark, one
    // `GameTooltipText:SetTextColor(…)` would repaint every red/green line in the tooltip.
    d.font_explicit.color = true;
    d.hidden = false;
    model.touch_measure(rh);
}

/// Hide + blank every line cell and zero the counters. The caller fires `OnTooltipCleared`
/// (outside the model borrow) — the engine-fired widget script the reference wires to its
/// money-row clear.
pub(super) fn clear_content(model: &mut Model, h: FrameHandle) {
    let (lefts, rights) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (t.left_lines.clone(), t.right_lines.clone()),
        _ => return,
    };
    for rh in lefts.into_iter().chain(rights) {
        if let Some(d) = model.region_data.get_mut(&rh) {
            d.text = None;
            d.hidden = true;
            // `measured` is deliberately KEPT. It is a cache whose validity gate is its own
            // content hash ([`RegionData::measure_key`] over text/font/wrap/outline), so wiping
            // it here invalidates nothing the key would not — while destroying the one thing that
            // makes the hover re-enter loop affordable. That loop
            // (`ContainerFrameItemButton_OnUpdate`, unthrottled in 1.12 and shipped verbatim as
            // the reference's `ContainerFrameItemButton_OnUpdate`) clears and rebuilds the SAME content
            // every frame: keeping the cache lets each rebuilt line re-validate against its own
            // key, so nothing re-shapes, the measure list stays empty, its forced second resolve
            // never happens, and the layout gate's fingerprint — which exists precisely to
            // "absorb idempotent writes for free" (layout.rs) — closes over the whole loop.
            // Wiping it cost +10 CPU ms/frame on a live bag hover (measured 13.2 → 23.9 cpu_ms,
            // `WOW_CAPTURE=ui-bag` vs `ui-tooltip`); the gate is
            // `layout_gate::the_hover_re_enter_loop_neither_re_measures_nor_re_solves`.
            //
            // The wrap pin (`size`) is likewise KEPT, and for the stronger reason: wiping it here
            // was the last write that made the hover re-enter loop differ from itself. The wipe
            // and `append_line`'s re-pin are a round trip inside ONE frame — same cell, same
            // width, no observable state between them — but each half bumped the layout epoch, so
            // tier 1 went dirty every hover frame and tier 2 hashed the whole UI to conclude
            // nothing had moved (~0.65 ms/frame, `WOW_UI_COST=1` on `WOW_CAPTURE=ui-tooltip`, at
            // solves=0). The pin is now written from the line's own wrap flag at append time —
            // set for a wrap line, cleared for a plain one — which covers the case this wipe
            // existed for (a cell whose new content doesn't wrap) without a write per frame.
            // A hidden, textless cell contributes nothing to layout in between either way.
        }
    }
    if let Ok(t) = tip_mut(model, h) {
        t.num_lines = 0;
        t.min_width = 0.0;
        t.unit_token = None;
        t.world_owned = false;
    }
    // The mouseover health bar is UNIT content: it hides with the lines (the ref bar exists
    // only while a unit shows; the byte law's watcher re-shows it on the next unit render).
    // Without this, the bar from the last mob hover rode under every ITEM tooltip.
    let bar = model
        .arena
        .frame(h)
        .and_then(|f| f.name.clone())
        .and_then(|n| model.arena.lookup(&format!("{n}StatusBar")));
    if let Some(bar) = bar {
        model.arena.set_shown(bar, false);
    }
}

/// Fire `OnTooltipCleared` on the tooltip frame (errors → [`Model::errors`], never propagated).
pub(super) fn fire_cleared(lua: &Lua, h: FrameHandle) {
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if let Err(e) = event::fire_widget_handler(lua, id, "OnTooltipCleared", Vec::new()) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
}

/// A content `Set*`'s tail: show when it rendered lines, hide when it rendered none. The real
/// client's uncached path leaves a CLEARED tooltip whose size collapses to nothing — with our
/// declared-size plate the visible equivalent is a hide; the hover's re-enter repaints the
/// moment the app answers the ask. Hiding through the arena directly (not [`hide_tooltip`])
/// keeps the owner: IsOwned must stay true for the re-enter loop.
pub(super) fn show_or_hide_empty(lua: &Lua, h: FrameHandle) {
    let lines = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        match tip_mut(&mut model, h) {
            Ok(t) => t.num_lines,
            Err(_) => 0,
        }
    };
    set_shown(lua, h, lines > 0);
}

/// Cancel a running fade and restore full alpha (fresh CONTENT does this — re-hovering during
/// the fade-out resurrects the tooltip at full strength). Conditional on a fade actually running,
/// because an addon that lowered the plate's alpha itself and then added a line keeps its alpha:
/// only the three sites in [`full_alpha`] are documented to stamp it back.
fn cancel_fade(model: &mut Model, h: FrameHandle) {
    if let Ok(t) = tip_mut(model, h) {
        if t.fade_start.take().is_some() {
            model.arena.set_alpha(h, 1.0);
        }
    }
}

/// `SetAlpha(255)` — **UNCONDITIONAL**, and the FIRST thing the SetOwner core `0x52ffe0` does
/// (`0x52fff4`), before the five stores; `0x530a80`'s show arm does it too, and its self-hide arm
/// reaches it through `0x530a60` → `0x52ffe0(0, 0, 0, 0)`. So all three of SetOwner, Show and the
/// effective-hide leave the plate at full strength whether or not one of OUR fades was running
/// (`0x52ffe0` / `0x530a80`).
///
/// [`cancel_fade`] is not this: it only restores the alpha a fade of ours took away, so a plate
/// left dim by any other path never recovered where the reference recovers on the next SetOwner.
fn full_alpha(model: &mut Model, h: FrameHandle) {
    if let Ok(t) = tip_mut(model, h) {
        t.fade_start = None;
    }
    model.arena.set_alpha(h, 1.0);
}

/// The engine's `GetTime` clock (the `__benilla_now` global [`super::UiScript::tick`] advances).
fn now(lua: &Lua) -> f64 {
    lua.globals().get("__benilla_now").unwrap_or(0.0)
}

/// Show/hide through the arena with the visibility events fired (the shared Show/Hide's path).
pub(super) fn set_shown(lua: &Lua, h: FrameHandle, shown: bool) {
    let changed = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.arena.set_shown(h, shown)
    };
    event::fire_visibility_changes(lua, changed);
}

/// The engine tooltip's default text colour — the byte constant `0xc0d3e8` = `0xffffd200` gold
/// every no-colour `AddLine`/`SetText` renders with. This is why the reference's zone tooltip is
/// GOLD despite Minimap.lua's `AddLine(text, "", 1.0, 1.0, 1.0)`: 1.12's binding reads the colour
/// at its fixed positions, the `""` in the r-slot is not a number, and the whole colour tail is
/// dropped in favour of this default — the trailing 1.0s never shift into place (the last one is
/// consumed as the truthy wrap flag). Byte-pinned: the static-init writer `0x528e50` stores
/// `0xffffd200` at `0xc0d3e8`.
pub(in crate::script) const DEFAULT_TEXT_GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];

/// The client's duration-string formatter, `0x52fa50` — the one that turns a millisecond count into
/// "2 hours remaining" / "45 seconds remaining". Its parameter vector is the binary's: `(V_ms,
/// keyPrefix, roundUp)`, minus the `extraArg` slot only the enchant line fills.
///
/// **The ladder, with the thresholds decoded from the compare instructions** (`52fa76`, `52fab4`,
/// `52fb03`, falling through to `0x52fb4f`): `≥ 86_400_000` → `_DAYS` · `≥ 3_600_000` → `_HOURS` ·
/// `≥ 60_000` → `_MIN` · else `_SEC`. Each arm divides by its own threshold, so the number shown is
/// always in the unit the arm names.
///
/// **`roundUp` ceils — but only in the top three arms; `_SEC` always truncates.** That asymmetry is
/// the whole reason this is a shared function and not four `format!`s, and it is what produces the
/// reference's three surprising readings: 61 s is "2 minutes remaining" (ceil, not "1"), 3 599 999 ms
/// is "60 minutes remaining" (the hour arm needs the full hour, so there is no "1 hour" until
/// exactly 3 600 000), and the final second before an aura lapses reads "**0** seconds remaining"
/// (truncation, not "1").
///
/// **The text is the player's own `GlobalStrings.lua`, never ours.** `get` is the VM-global lookup
/// (`load_global_strings` runs the shipped file into this VM at boot, off the patch chain); the
/// template arrives as `"%d minutes remaining"` and the count fills its one `%d`. A key the string
/// table doesn't carry yields **no line at all** rather than an invented one — the same disposition
/// 0620 §5 took for a reagent whose template hasn't streamed. That is also why this house ships no
/// copy of the eight strings: on an install they are read, and off one there is nothing to render.
pub(in crate::script) fn duration_text(
    ms: u32,
    key_prefix: &str,
    round_up: bool,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    const DAY_MS: u32 = 86_400_000;
    const HOUR_MS: u32 = 3_600_000;
    const MIN_MS: u32 = 60_000;
    const SEC_MS: u32 = 1_000;

    let (suffix, divisor) = if ms >= DAY_MS {
        ("DAYS", DAY_MS)
    } else if ms >= HOUR_MS {
        ("HOURS", HOUR_MS)
    } else if ms >= MIN_MS {
        ("MIN", MIN_MS)
    } else {
        ("SEC", SEC_MS)
    };
    // `roundUp` reaches the day/hour/minute arms only — the seconds arm has no rounding branch at
    // all, so it truncates whatever `roundUp` says.
    let n = if round_up && divisor != SEC_MS {
        ms.div_ceil(divisor)
    } else {
        ms / divisor
    };
    let template = plural_template(&format!("{key_prefix}_{suffix}"), n, get)?;
    Some(template.replacen("%d", &n.to_string(), 1))
}

/// [`crate::strings::plural`] for this module's callers — the `_P1` pick behind
/// `GetText(token, gender, n)`, whose byte law (and the refuted `> 1` reading: `703c89` is a
/// *signed* test, so a zero ordinal takes the plural arm and a lapsing aura reads "0 seconds
/// remaining") now lives with the primitive rather than in a fourth private copy of it. Four
/// copies of one rule is the shape decision 2045 wrote about; `fill` had eight.
///
/// **Gender is not modelled, and that is faithful, not a gap:** `0x52fa50` pushes a literal `0`
/// on both arms (`52fb8d: 6a 00`, `52fbaa: 6a 00`), which `0x703bf0` turns into nil, and enUS
/// folds nil to no suffix.
///
/// (When BOTH the twin and the bare token are absent the reference renders an EMPTY line, off its
/// pre-seeded `0x882748`; we render no line at all. Unreachable with shipped data — all eight of
/// the aura family's keys ship — and off an install there is no string table to be faithful to.)
pub(in crate::script) fn plural_template(
    token: &str,
    n: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    crate::strings::plural(token, Some(n), get)
}

/// A colour-component argument — `lua_tonumber`'s coercion (a numeric string counts, `""` and
/// other strings don't).
fn color_num(v: Option<&Value>) -> Option<f32> {
    match v {
        Some(Value::Number(n)) => Some(*n as f32),
        Some(Value::Integer(i)) => Some(*i as f32),
        Some(Value::String(s)) => s.to_str().ok().and_then(|s| s.trim().parse::<f32>().ok()),
        _ => None,
    }
}

/// The bindings' boolean-argument read (`GetBoolOrDefault 0x6f1c10`): nil/absent, false, numeric 0,
/// and the strings `"0"`/`"off"`/`"disabled"` are false; everything else is true.
fn bool_arg(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Nil) | Some(Value::Boolean(false)) => false,
        Some(Value::Integer(i)) => *i != 0,
        Some(Value::Number(n)) => *n != 0.0,
        Some(Value::String(s)) => !s.to_str().ok().is_some_and(|s| {
            s == "0" || s.eq_ignore_ascii_case("off") || s.eq_ignore_ascii_case("disabled")
        }),
        _ => true,
    }
}

/// The colour block of an `AddLine`-family tail (the values at the r/g/b positions): applied
/// only when the r-slot is a number (`lua_isnumber 0x6f34d0` — the sole gate);
/// anything else drops the WHOLE block to the default gold [`DEFAULT_TEXT_GOLD`] — the ref's
/// gold zone tooltip, whose `(text, "", 1.0, 1.0, 1.0)` shape has `""` at the r-slot. When the
/// gate passes, g/b are UNGATED `lua_tonumber` reads: a missing/non-number component is **0.0**
/// (byte-pinned). Alpha is forced opaque (the bindings pack `0xFF`).
fn parse_line_color(r: Option<&Value>, g: Option<&Value>, b: Option<&Value>) -> [f32; 4] {
    match color_num(r) {
        Some(r) => [
            r,
            color_num(g).unwrap_or(0.0),
            color_num(b).unwrap_or(0.0),
            1.0,
        ],
        None => DEFAULT_TEXT_GOLD,
    }
}

/// Parse an `AddLine` tail (the args after `text`): positional `r, g, b, wrapText` — the
/// byte-pinned `0x531630` signature. The wrap flag is read at its position REGARDLESS of the
/// colour gate (the ref's zone-tooltip shape wraps: its trailing `1.0` lands in the wrap slot).
fn parse_line_tail(args: &[Value]) -> ([f32; 4], bool) {
    (
        parse_line_color(args.first(), args.get(1), args.get(2)),
        bool_arg(args.get(3)),
    )
}

/// A Lua text argument → the line's string: strings pass through, numbers stringify (Lua
/// coercion), nil/absent is an empty line (`GameTooltip:AddLine()` ships in the corpus).
fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.to_str().map(|s| s.to_string()).unwrap_or_default(),
        Some(Value::Number(n)) => {
            let n = *n;
            if n == n.trunc() && n.abs() < 1e15 {
                format!("{}", n as i64)
            } else {
                format!("{n}")
            }
        }
        Some(Value::Integer(i)) => format!("{i}"),
        _ => String::new(),
    }
}

/// Append one line (both columns; `right = None` leaves the right cell hidden). Grows the pool,
/// writes the cells, bumps `num_lines` (+ the line's wrap flag), cancels any fade. Does NOT show
/// the tooltip — `AddLine` leaves showing to the caller (the corpus' `AddLine … Show()`),
/// `SetText`/the content builders show themselves.
pub(super) fn append_line(
    lua: &Lua,
    this: &Table,
    left: (String, [f32; 4]),
    right: Option<(String, [f32; 4])>,
    wrap: bool,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let n = with_tip(lua, this, |t| t.num_lines + 1)?;
    ensure_lines(lua, this, n)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let (lh, rh) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (t.left_lines[n - 1], t.right_lines[n - 1]),
        _ => return Err(mlua::Error::runtime("not a GameTooltip")),
    };
    write_cell(&mut model, lh, &left.0, left.1);
    // A wrap-flagged line PINS its wrap column at write time, so its very first measure ask
    // carries the width and comes back WRAPPED (multi-row height) in the same frame. The old
    // shape — layout re-pinning after an overflowing single-line measure — needed a second
    // round-trip that the bag re-enter loop's per-frame content clear kept wiping out: wrapped
    // lines never converged live (the plate under-counted their rows forever — the bread/
    // hearthstone spill). One step, no oscillation.
    // The wrap pin is written from the LINE's own flag every append — set for a wrap line,
    // cleared for a plain one — so a cell that changes role can never keep a stale pin, and
    // `clear_content` needs no wipe of its own (which is what used to make this write differ
    // every frame). The epoch bump is gated on a REAL change: in the hover re-enter loop the
    // rebuilt line pins the same width it already carried, so tier 1 stays clean and the
    // whole-UI fingerprint (tier 2) never runs — see `clear_content`'s note.
    let pin = wrap.then_some((crate::widget::TOOLTIP_WRAP_WIDTH, 0.0));
    if let Some(d) = model.region_data.get_mut(&lh) {
        if d.size != pin {
            d.size = pin;
            // NAMED, not conservative (decision 1388). This writes one region's EXPLICIT SIZE and
            // nothing else — no anchor target, no roster membership — which is the exact shape
            // 1388 migrated every size setter to. It was missed because it is an internal write
            // rather than a `SetWidth` binding, and the miss is expensive in the one place it can
            // least afford to be: the pin flips whenever a line changes ROLE between wrapped and
            // plain, which is what every hover from one item or spell to a differently-shaped one
            // does. A conservative touch there re-derives the whole layout graph — every live
            // frame's scale re-synced, every anchored region re-hashed — on each hover.
            model.touch_layout_region(lh);
            // The wrap pin is the measure key's wrap-width input.
            model.touch_measure(lh);
        }
    }
    if let Some((text, color)) = right {
        write_cell(&mut model, rh, &text, color);
    }
    if let Ok(t) = tip_mut(&mut model, h) {
        t.num_lines = n;
    }
    cancel_fade(&mut model, h);
    Ok(())
}

mod verbs;
pub(super) use verbs::install;

/// The full hide: drop the owner, cancel the fade, clear the content, hide the frame, fire
/// `OnTooltipCleared`. Shared by the `Hide` method and the fade's end-of-ramp.
pub(super) fn hide_tooltip(lua: &Lua, h: FrameHandle) {
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        full_alpha(&mut model, h);
        if let Ok(t) = tip_mut(&mut model, h) {
            t.owner = None;
        }
        clear_content(&mut model, h);
    }
    set_shown(lua, h, false);
    fire_cleared(lua, h);
}

/// One line cell's layout input: `Some((w, h))` once the cell is shown with text AND the measure
/// round-trip has answered; one floored unit each way for a shown but EMPTY cell (the reference's
/// `AddLine("")` adds no line at all, and its virtual `GetHeight` never reads back `0.0`); `None`
/// for hidden/cleared cells and for PENDING measures. A pending cell contributes nothing — no
/// width, no height, and (in the caller) no gaps — so a fresh tooltip holds its declared default
/// size until real extents land, instead of collapsing to a gaps-only rect (the guard the first
/// cut got wrong: it summed LINE_GAP/DOUBLE_GAP for unmeasured rows, caught by the migration
/// pass).
type Cell = Option<(f32, f32)>;

fn cell(model: &Model, rh: crate::widget::RegionHandle) -> Cell {
    let d = model.region_data.get(&rh)?;
    let text = d.text.as_deref()?;
    if d.hidden {
        return None;
    }
    if text.is_empty() {
        // The same floor the sweep applies to the line's own rect (`layout::FONTSTRING_MIN_SPAN`):
        // the reference's `GetHeight 0x772a60` is virtual and ends in a one-unit clamp, so `0.0` is
        // not a height any caller — plate arithmetic included — can read back off a FontString.
        // Reporting zero here while the chain below the row resolved one unit is the B309 shape at
        // 1/14th the size, and the point of one constant is that the two cannot drift apart.
        return Some((
            super::layout::FONTSTRING_MIN_SPAN,
            super::layout::FONTSTRING_MIN_SPAN,
        ));
    }
    d.measured.map(|m| (m.w, m.h))
}

/// The auto-size + right-flush pre-pass, run at the top of every layout resolve (decision 0274):
/// for each live GameTooltip, frame size = max measured line width (a double line is
/// left + gap + right) + 2·pad, floored by `SetMinimumWidth`, × summed line heights + gaps; each
/// visible right column's anchor re-points so its right edge sits at the text inset. Skips
/// frames whose lines haven't been measured yet — the XML default size holds until the extents
/// land. With a [`TextMeasure`](super::TextMeasure) installed that is the SAME `resolve`
/// (`resolve` solves, fills the pending measures inline, and solves again, so this pre-pass runs
/// a second time with real extents); without one it is the host's batch round-trip, one frame
/// later, which is the engine-less path every measurer-free VM still takes.
/// **ANCHOR_CURSOR — mode 6, re-anchored to the live cursor every frame** (`0x530b20`, the
/// `CGameTooltip` override of `vtable+0x38` on `0x808f60`). Nine corpus files ask for it — `pfUI`'s
/// tooltip, xpbar and chat modules, `pfQuest/browser.lua`, `TipBuddy` — and until decision 2176
/// benilla warned and placed them by some other mode.
///
/// The whole of the re-anchor is one `SetPoint`:
///
/// ```text
/// 530b32  cmp [this+0x318],6 ; jne                    -- every other mode skips
/// 530b3b  eax = [this+0xa0]                           -- the CSimpleTop root == [0xcf0bd8]
/// 530b4f  ecx = [eax+0x111c] ; edx = [eax+0x1118]     -- the SAVED cursor, normalised [0,1]
/// 530b63  call 0x41ad80                               -- x *= G44 (screen w), y *= G48 (screen h)
/// 530b68  fld [this+0x7c] ; fdivr 1.0f                -- 1 / the tooltip's own effective scale
/// 530b97  call 0x767c70(BOTTOM=7, [0xcf0bd8], BOTTOMLEFT=6, x/scale, y/scale, doResolve=1)
/// ```
///
/// So **the plate's BOTTOM-centre is pinned to the screen's BOTTOMLEFT plus the cursor's absolute
/// position** — centred horizontally on the cursor and sitting directly above it. Four details are
/// worth keeping because none is guessable from the name:
///
/// * The `relativeTo` is the screen root passed **raw**, not `+0x24` like every other anchor — the
///   `CSimpleTop`'s `CLayoutFrame` base sits at offset 0. Ours is [`SCREEN`](super::SCREEN), which
///   is the same object.
/// * The **divide by the tooltip's own effective scale** is the "cursor box recip": an anchor
///   offset is stored raw and multiplied by the child's scale at resolve time, so dividing here is
///   what makes the result land at the cursor's *absolute* position. Ours does the same divide,
///   against the same scale [`GetWidth`](super::object::eff_scale) reads.
/// * **The `SetOwner`-time offsets `+0x3c4/+0x3c8` do not participate** — 12 reads of them, every
///   one on a modes-0..5 arm of `0x52fe90`.
/// * There is **no `ClearAllPoints` here** (earned zero over the function's complete call set): one
///   anchor slot is overwritten, and `0x767c70`'s own no-op gate — same target, same
///   `relativePoint`, both offsets within 2⁻²² — is what keeps a still cursor free.
///
/// It runs from the per-frame update pump, which a hidden tooltip is not in (`0x76ad9d`
/// deregisters on hide), and in the **same frame** as the placement it produces: the layout drain
/// `0x768ed0` runs at `0x765799`, right after the `+0x38` loop and before the draw. Hence its home
/// here, at the top of the resolve, beside the auto-size pass.
fn cursor_anchor(model: &mut Model, h: FrameHandle) {
    let scale = crate::script::object::eff_scale(model, h);
    let (cx, cy) = model.cursor_pos;
    let new = Anchor::new(
        Point::Bottom,
        super::SCREEN,
        Point::BottomLeft,
        cx / scale,
        cy / scale,
    );
    let input = model.layout_inputs.entry(h).or_default();
    // `0x767c70`'s change-detect, in our own terms: one slot, replaced only on a real change.
    let same =
        input.anchors.len() == 1 && crate::script::object::anchor_bits_eq(&input.anchors[0], &new);
    if same {
        return;
    }
    let old_targets: Vec<u32> = input.anchors.iter().map(|a| a.relative_to).collect();
    input.anchors = vec![new];
    model.touch_layout_retarget_frame(h, &old_targets, &[super::SCREEN]);
}

pub(super) fn layout_tooltips(model: &mut Model) {
    // The arena's tooltip registry, not the resolve's whole frame roster: this pre-pass runs at
    // the top of EVERY resolve, and finding two or three tooltips by scanning ~4000 ids was most
    // of what it cost (decision 1634). `frame_to_id` still gates each one — a tooltip outside the
    // resolve's roster would mint an id here and take the ledger's conservative branch with it.
    let tips: Vec<FrameHandle> = model
        .arena
        .tooltip_kinds()
        .iter()
        .copied()
        .filter(|h| model.frame_to_id.contains_key(h))
        .collect();
    for h in tips {
        // Mode 6 first, and **before the line gate below**: the reference's per-frame update is
        // not gated on content at all, only on the plate being in the pump's list — which is the
        // shown list. See [`cursor_anchor`].
        let cursor_mode = match model.arena.frame(h).map(|f| (&f.kind_state, f.shown)) {
            Some((KindState::Tooltip(t), shown)) => shown && t.anchor == TooltipAnchor::Cursor,
            _ => false,
        };
        if cursor_mode {
            cursor_anchor(model, h);
        }
        let (num, lefts, rights, min_w, pad_w) = match model.arena.frame(h).map(|f| &f.kind_state) {
            Some(KindState::Tooltip(t)) => (
                t.num_lines,
                t.left_lines.clone(),
                t.right_lines.clone(),
                t.min_width,
                t.padding,
            ),
            _ => continue,
        };
        if num == 0 {
            continue;
        }
        // (Wrap columns are pinned at append time — append_line — so every measure here is
        // already the wrapped one; no second round-trip.)
        // Per-line metrics next (one read pass), then the size + right-flush writes. Only
        // measured cells contribute — a row with no measured cell adds no height and no gap, and
        // a tooltip with nothing measured yet is skipped whole (its declared size holds until the
        // extents land — this resolve's own `fill_measures` with a measurer installed, the host's
        // batch round-trip a frame later without one).
        let n = num.min(lefts.len()).min(rights.len());
        let rows: Vec<(Cell, Cell)> = (0..n)
            .map(|i| (cell(model, lefts[i]), cell(model, rights[i])))
            .collect();
        let mut maxw: f32 = 0.0;
        let mut totalh: f32 = 0.0;
        let mut counted = 0usize;
        for (l, r) in &rows {
            if l.is_none() && r.is_none() {
                continue;
            }
            let (lw, lh) = l.unwrap_or((0.0, 0.0));
            let (rw, rh_h) = r.map_or((0.0, 0.0), |(w, hgt)| (TOOLTIP_DOUBLE_GAP + w, hgt));
            if counted > 0 {
                totalh += TOOLTIP_LINE_GAP;
            }
            totalh += lh.max(rh_h);
            counted += 1;
            maxw = maxw.max(lw + rw);
        }
        if counted == 0 {
            continue; // nothing measured yet — hold the declared size
        }
        maxw = maxw.max(min_w);
        if maxw <= 0.0 || totalh <= 0.0 {
            continue;
        }
        let input = model.layout_inputs.entry(h).or_default();
        // `pad_w` = SetPadding's extra width (ref ItemRefTooltip: room for the close button).
        input.width = maxw + 2.0 * TOOLTIP_PAD + pad_w;
        input.height = totalh + 2.0 * TOOLTIP_PAD;
        // This pre-pass runs INSIDE the resolve, so it must not open tier 1 (see the note method's
        // own doc) — but the per-node ledger decision 1388's incremental pass seeds from still has
        // to hear that these two inputs were written by something other than a Lua setter.
        model.note_layout_frame_write(h);
        // Right-flush each double line: right edge = left.right + (maxw − left.width)
        // = frame.left + pad + maxw = the text inset's right.
        for (i, (l, r)) in rows.iter().enumerate() {
            if r.is_none() {
                continue;
            }
            let lw = l.map_or(0.0, |(w, _)| w);
            let mut wrote = false;
            if let Some(d) = model.region_data.get_mut(&rights[i]) {
                if let Some(a) = d.anchors.first_mut() {
                    a.x_off = maxw - lw;
                    wrote = true;
                }
            }
            if wrote {
                model.note_layout_region_write(rights[i]);
            }
        }
    }
}

/// Advance every fading tooltip (called from [`super::UiScript::tick`], the ScrollingMessage/
/// Cooldown pattern): alpha ramps 1→0 over [`TOOLTIP_FADE_SECS`]; at the end the tooltip hides
/// for real (owner dropped, content cleared, `OnTooltipCleared` fired).
pub(super) fn tick_fades(lua: &Lua) {
    let t = now(lua);
    let mut ramping: Vec<(FrameHandle, f32)> = Vec::new();
    let mut finished: Vec<FrameHandle> = Vec::new();
    {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        // The arena's tooltip registry (1634) — this runs every tick, and the roster it used to
        // walk is ~4000 entries at a corpus UI. The `frame_to_id` gate is kept: `hide_tooltip`
        // below writes layout, and a frame outside the resolve's roster takes the ledger's
        // conservative branch.
        for &h in model.arena.tooltip_kinds() {
            if !model.frame_to_id.contains_key(&h) {
                continue;
            }
            let Some(frame) = model.arena.frame(h) else {
                continue;
            };
            let KindState::Tooltip(tip) = &frame.kind_state else {
                continue;
            };
            let Some(start) = tip.fade_start else {
                continue;
            };
            if !frame.shown {
                finished.push(h); // hidden mid-fade by other means — just tidy the state
                continue;
            }
            let a = 1.0 - ((t - start) / TOOLTIP_FADE_SECS) as f32;
            if a <= 0.0 {
                finished.push(h);
            } else {
                ramping.push((h, a));
            }
        }
    }
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        for (h, a) in ramping {
            model.arena.set_alpha(h, a);
        }
    }
    for h in finished {
        hide_tooltip(lua, h);
    }
}
