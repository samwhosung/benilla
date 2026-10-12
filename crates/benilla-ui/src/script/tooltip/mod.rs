//! The `GameTooltip` method surface and engine behaviour: the line stack, owner and anchor,
//! auto-size and the fade.
//!
//! Lines are named FontString regions published as Lua globals (`<name>TextLeft1` …), since
//! FrameXML addresses them by name (`GameTooltipTextLeft1:SetTextColor`, `UnitFrame.lua:76`). The
//! template declares 30 pairs, which are adopted; the reference grows past them through
//! `AddFontStrings`, and this also creates pairs on demand.
//!
//! Layout is a pre-pass of every `resolve` ([`layout_tooltips`]): the width is the widest line (a
//! double line adds the column gap) plus padding, floored by `SetMinimumWidth`; the height is the
//! summed line heights and gaps; each right column sits flush with the text inset.

use mlua::{Lua, Table, Value};

use super::event;
use super::object::{frame_handle_of, publish_global};
use super::region::region_wrapper;
use super::visibility;
use super::{FontObject, Model, RegionData};
use crate::layout::{Anchor, Anchors, Point};
use crate::order::DrawLayer;
use crate::widget::{
    FrameHandle, KindState, RegionKind, TooltipAnchor, TooltipState, TOOLTIP_DOUBLE_GAP,
    TOOLTIP_FADE_SECS, TOOLTIP_LINE_GAP, TOOLTIP_PAD,
};

/// Registry key of the GameTooltip method table, a named registry root (the MAXCSTACK discipline).
pub(super) const REG_TOOLTIP_METHODS: &str = "__benilla_tooltip_methods";

impl super::UiScript {
    /// The name of the frame that owns the GameTooltip named `tooltip`; `None` for no such
    /// tooltip, no owner or an unnamed one. 1.12 answers only `IsOwned(frame)`, so the host's
    /// recorders read the owner here.
    pub fn tooltip_owner_name(&self, tooltip: &str) -> Option<String> {
        let model = self.model_ref();
        let h = model.arena.lookup(tooltip)?;
        let owner = match &model.arena.frame(h)?.kind_state {
            KindState::Tooltip(t) => t.owner?,
            _ => return None,
        };
        model.arena.frame(owner)?.name.clone()
    }
}

/// Runs `f` over a GameTooltip's state under one short write borrow.
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

/// `SetFontObject` onto a line cell, except that a missing font object keeps the default face
/// rather than raising (a VM loaded without Fonts.xml).
fn apply_font(d: &mut RegionData, name: &str, fo: Option<&FontObject>) {
    d.font_object = Some(name.to_string());
    // The inherit mask (`+0x2c`) stays: a re-point does not restore inheritance in the reference.
    // From here the line follows the font object live, through `font::propagate`.
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

/// Grows the line-pair pool to `n` pairs: XML-declared pairs (`<name>TextLeft<i>`) are adopted
/// with their fonts, and created pairs clone the previous pair's fonts past a declared ladder, or
/// else wear the template's `GameTooltipHeaderText` (line 1) and `GameTooltipText`.
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

            // The template's declared pair for this index (ShoppingTooltipTemplate's ladder).
            let declared = |model: &Model, cell: &str| -> Option<crate::widget::RegionHandle> {
                if frame_name.is_empty() {
                    return None;
                }
                let id = model
                    .region_by_name
                    .get(&format!("{frame_name}Text{cell}{i}"))
                    .copied()?;
                let rh = model.id_to_region.get(&id).copied()?;
                (model.arena.region(rh)?.owner == h).then_some(rh)
            };
            // A created cell's fonts: the previous line's under an XML ladder, else the defaults.
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
            let left_drew = model.region_draws(left);
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
                // Left-justified as in the template; it shows once a wrap line pins its width.
                d.justify.set_h(super::JustifyH::Left);
                d.anchors = Anchors::from([match prev_left {
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
                }]);
                model.region_data.insert(left, d);
                model.note_region_draw(left, left_drew);
                model.touch_measure(left); // an adopted cell can arrive text-in-hand
                                           // A line row's points are set with the invalidate flag (`0x530197`).
                model.invalidate_region(left, true);
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
            let right_drew = model.region_draws(right);
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
                // RIGHT on the left cell's LEFT, the point the layout re-sets with the plate
                // width as x (`0x53094f`), as the template's `$parentTextRight1` declares it.
                d.anchors =
                    Anchors::from([Anchor::new(Point::Right, left_id, Point::Left, 0.0, 0.0)]);
                model.region_data.insert(right, d);
                model.note_region_draw(right, right_drew);
                model.touch_measure(right); // an adopted cell can arrive text-in-hand
                model.invalidate_region(right, true);
            }
            let right_id = model.region_id(right);

            if !frame_name.is_empty() {
                // Adopted pairs are already named and published by the loader.
                if adopted_left.is_none() {
                    let ln = format!("{frame_name}TextLeft{i}");
                    crate::script::region::set_region_name(&mut model, left, left_id, &ln);
                    publish.push((left_id, ln));
                }
                if adopted_right.is_none() {
                    let rn = format!("{frame_name}TextRight{i}");
                    crate::script::region::set_region_name(&mut model, right, right_id, &rn);
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

/// Writes a line cell's text and colour and shows it.
pub(super) fn write_cell(
    model: &mut Model,
    rh: crate::widget::RegionHandle,
    text: &str,
    color: [f32; 4],
) {
    let d = model.region_data.entry(rh).or_default();
    let was_shown = d.text.as_deref().is_some_and(|t| !t.is_empty());
    d.text = Some(text.to_string());
    d.vertex_color = Some(color);
    // The line's own colour, as if set by `SetTextColor`: a later
    // `GameTooltipText:SetTextColor` must not repaint it.
    d.font_explicit.color = true;
    model.set_region_hidden(rh, false);
    model.touch_measure(rh);
    model.touch_layout_text(rh, was_shown);
}

/// Blanks every line cell and resets the counters and the subject: the model half of the clear
/// ([`clear_silent`]).
fn clear_content(model: &mut Model, h: FrameHandle) {
    let (lefts, rights) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (t.left_lines.clone(), t.right_lines.clone()),
        _ => return,
    };
    for rh in lefts.into_iter().chain(rights) {
        let drew = model.region_draws(rh);
        if let Some(d) = model.region_data.get_mut(&rh) {
            let was_shown = d.text.as_deref().is_some_and(|t| !t.is_empty());
            d.text = None;
            d.hidden = true;
            model.note_region_draw(rh, drew);
            // `measured` and the wrap pin (`size`) stay: the measure cache is keyed by content
            // ([`RegionData::measure_key`]) and `append_line` rewrites the pin, so the hover
            // loop's per-frame clear and rebuild (`ContainerFrameItemButton_OnUpdate`) re-measures
            // nothing, and its lines' layout re-hashes to the converged node and solves nothing.
            model.touch_layout_text(rh, was_shown);
        }
    }
    if let Ok(t) = tip_mut(model, h) {
        t.num_lines = 0;
        t.min_width = 0.0;
        // Clear drops the subject and unregisters its health watcher (`0x5300ad`, `0x53007c`).
        t.unit_guid = None;
    }
    // New content replaces a spell render still waiting on its view.
    model.spell_tooltip_waits.remove(&h);
}

/// The tooltip's `<name>StatusBar` health bar (`[+0x348]`), if it has one.
pub(super) fn status_bar(model: &Model, h: FrameHandle) -> Option<FrameHandle> {
    let name = model.arena.frame(h)?.name.as_deref()?;
    model.arena.lookup(&format!("{name}StatusBar"))
}

/// The tooltip clear (`0x530050`) without its last act, the `OnTooltipCleared` fire: while the
/// tooltip holds a unit (`0x530070`) the health bar hides by its own transition, shown bit then
/// hide slot (`0x53009b`/`0x5300a1`), so its `OnHide` runs; then the lines, counters and subject.
pub(super) fn clear_silent(lua: &Lua, h: FrameHandle) {
    let bar = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let held = tip_mut(&mut model, h).is_ok_and(|t| t.unit_guid.is_some());
        held.then(|| status_bar(&model, h)).flatten()
    };
    if let Some(bar) = bar {
        visibility::set_shown(lua, bar, false);
    }
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    clear_content(&mut model, h);
}

/// The tooltip clear (`0x530050`): [`clear_silent`], then `OnTooltipCleared` (`0x530263`).
pub(super) fn clear(lua: &Lua, h: FrameHandle) {
    clear_silent(lua, h);
    fire_cleared(lua, h);
}

/// Fires `OnTooltipCleared`; a script error is recorded, never propagated.
pub(super) fn fire_cleared(lua: &Lua, h: FrameHandle) {
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if let Err(e) = event::fire_widget_handler(lua, id, "OnTooltipCleared", Vec::new()) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
    }
}

/// A content `Set*`'s tail: the builders' show, the shown bit and the show slot (`0x52a711`/
/// `0x52a71b`, `0x52e14e`), whose override restores full alpha and disarms a fade, or, for a plate
/// with no owner or no lines, hides and un-owns it (`0x530aba`). An item builder whose template is
/// not cached ends the same way, by its own hide (`0x52b735`–`0x52b73f`), and its clear zeroes the
/// remembered item id (`0x5300dd`) that the cache callback `0x52e530` needs (`0x52e55e`), so the
/// plate stays down until the next hover.
///
/// A spell render still waiting on its view ([`Model::spell_tooltip_waits`], filled by the host
/// ahead of a hover) is the one exception: the reference reads spells synchronously and has no
/// such state. That plate takes the base hide, keeping its owner and its wait, so the view's
/// arrival renders it in place.
pub(super) fn show_or_hide_empty(lua: &Lua, h: FrameHandle) {
    let waiting = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let empty = tip_mut(&mut model, h).is_ok_and(|t| t.num_lines == 0);
        empty && model.spell_tooltip_waits.contains_key(&h)
    };
    if waiting {
        visibility::set_shown_bit(lua, h, false);
        visibility::base_hide(lua, h);
    } else {
        visibility::set_shown(lua, h, true);
    }
}

/// The tooltip's show (`0x530a80`, slot `+0x88`), however the show comes: with an owner and a line
/// (`0x530a83`–`0x530a95`) it restores full alpha, disarms the fade (`0x530aa3`) and runs the base
/// show; else it clears its shown bit and hides through its own hide (`0x530abe`, `0x530ac8`).
/// So a parent's show brings back only a tooltip that still has an owner.
pub(in crate::script) fn show_override(lua: &Lua, h: FrameHandle) {
    let live = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let live = tip_mut(&mut model, h).is_ok_and(|t| t.owner.is_some() && t.num_lines > 0);
        if live {
            full_alpha(&mut model, h);
            owe_relayout(&mut model, h);
        }
        live
    };
    if live {
        visibility::base_show(lua, h);
    } else {
        visibility::set_shown_bit(lua, h, false);
        visibility::hide(lua, h);
    }
}

/// The tooltip's hide (`0x530a60`, slot `+0x84`), however the hide comes, its parent's included:
/// the SetOwner core with no owner ([`set_owner_core`]), then the base hide. It runs whether or
/// not the tooltip is shown: only the base hide gates on visibility (`0x76ad5b`).
pub(in crate::script) fn hide_override(lua: &Lua, h: FrameHandle) {
    let is_tip = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        tip_mut(&mut model, h).is_ok()
    };
    if is_tip {
        set_owner_core(lua, h, None, TooltipAnchor::None, 0.0, 0.0);
    }
    visibility::base_hide(lua, h);
}

/// The SetOwner core (`0x52ffe0`), reached from the Lua `SetOwner` after its hide, from the
/// tooltip's own hide with no owner, and from the engine's mode-6 seats (`0x492a1e`, `0x4eb587`).
/// It clears and fires `OnTooltipCleared` ([`clear`], `0x52ffe6`), restores full alpha
/// (`0x52fff4`), disarms the fade (`0x530002`), stores the owner (`0x53000c`) and mode 7,
/// `ANCHOR_NONE` (`0x530012`); only an owner overwrites the mode with the caller's (`0x530031`)
/// and applies the anchor (`0x52fe90`, [`apply_anchor`]).
pub(in crate::script) fn set_owner_core(
    lua: &Lua,
    h: FrameHandle,
    owner: Option<FrameHandle>,
    anchor: TooltipAnchor,
    x: f32,
    y: f32,
) {
    clear(lua, h);
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    full_alpha(&mut model, h);
    let Ok(t) = tip_mut(&mut model, h) else {
        return;
    };
    t.owner = owner;
    t.anchor = TooltipAnchor::None;
    if let Some(owner) = owner {
        t.anchor = anchor;
        apply_anchor(&mut model, h, owner, anchor, x, y);
    }
}

/// The anchor apply (`0x52fe90`, with the core's 1): `ANCHOR_PRESERVE` returns first
/// (`0x52fead`); every other mode clears all points (`0x767ed0` at `0x52fec2`); modes 0..5 then
/// set one point on the owner (`0x52ffbc`), and 6 and 7 stop there. The clear lets
/// `GameTooltip_SetDefaultAnchor` (GameTooltip.lua:76-77) set one point with no stale owner
/// anchor; mode 6 is then placed per frame by `cursor_anchor`.
fn apply_anchor(
    model: &mut Model,
    h: FrameHandle,
    owner: FrameHandle,
    anchor: TooltipAnchor,
    x: f32,
    y: f32,
) {
    let pts = match anchor {
        TooltipAnchor::Right => Some((Point::BottomLeft, Point::TopRight)),
        TooltipAnchor::Left => Some((Point::BottomRight, Point::TopLeft)),
        TooltipAnchor::TopRight => Some((Point::BottomRight, Point::TopRight)),
        TooltipAnchor::TopLeft => Some((Point::BottomLeft, Point::TopLeft)),
        TooltipAnchor::BottomRight => Some((Point::TopLeft, Point::BottomRight)),
        TooltipAnchor::BottomLeft => Some((Point::TopRight, Point::BottomLeft)),
        TooltipAnchor::Cursor | TooltipAnchor::None | TooltipAnchor::Preserve => None,
    };
    if matches!(anchor, TooltipAnchor::Cursor | TooltipAnchor::None) {
        let dropped = model
            .layout_inputs
            .get_mut(&h)
            .is_some_and(|input| input.anchors.clear());
        if dropped {
            model.touch_layout_frame(h);
        }
    }
    if let Some((own, rel)) = pts {
        let owner_id = model.frame_id(owner);
        let new = Anchors::from([Anchor::new(own, owner_id, rel, x, y)]);
        let input = model.layout_inputs.entry(h).or_default();
        // No-op when unchanged, so the bag hover's per-frame SetOwner does not dirty the layout;
        // a real one is `SetPoint` with the invalidate flag (`0x52ff12`..`0x52ffb2`).
        let changed = !input.anchors.bits_eq(&new);
        input.anchors = new;
        model.invalidate_frame(h, changed);
    }
}

/// The fade request (`0x530ae0`), from the Lua `FadeOut` (`0x531fba`) and the world mouseover's
/// change (`0x492909`). A cursor-anchored plate, mode 6, hides at once through the hide slot,
/// with no shown-bit write, and is disarmed (`0x530ae3`–`0x530af4`); any other mode arms the fade
/// with the full duration on every call (`0x530b00`–`0x530b10`), so a second call restarts it.
pub(in crate::script) fn fade_out(lua: &Lua, h: FrameHandle) {
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let Ok(t) = tip_mut(&mut model, h) else {
            return;
        };
        if t.anchor != TooltipAnchor::Cursor {
            t.fade_left = Some(TOOLTIP_FADE_SECS);
            return;
        }
    }
    visibility::hide(lua, h);
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if let Ok(t) = tip_mut(&mut model, h) {
        t.fade_left = None;
    }
}

/// `SetAlpha(255)` and the fade disarmed, unconditional: the SetOwner core `0x52ffe0` does both
/// (`0x52fff4`, `0x530002`), and Show (`0x530a80`) and its self-hide (`0x530a60`, the core with no
/// owner) reach them too.
fn full_alpha(model: &mut Model, h: FrameHandle) {
    if let Ok(t) = tip_mut(model, h) {
        t.fade_left = None;
    }
    model.arena.set_alpha(h, 1.0);
}

/// Shows or hides as the Lua verbs do, handlers included.
pub(super) fn set_shown(lua: &Lua, h: FrameHandle, shown: bool) {
    crate::script::visibility::set_shown(lua, h, shown);
}

/// The default line colour, gold `0xffffd200` (stored at `0xc0d3e8` by `0x528e50`). The zone
/// tooltip's `AddLine(text, "", 1.0, 1.0, 1.0)` (`Minimap.lua:40`) renders in it: the `""` r-slot
/// drops the whole colour, and the last `1.0` lands in the wrap slot.
pub(in crate::script) const DEFAULT_TEXT_GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];

/// The client's duration formatter (`0x52fa50`): days, hours, minutes or seconds by threshold,
/// each dividing by its own unit. `round_up` ceils every arm but seconds, which truncates, so 61 s
/// reads "2 minutes" and 3 599 999 ms "60 minutes". The text is the player's own
/// `GlobalStrings.lua` through `get`; a missing key renders no line.
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
    let n = if round_up && divisor != SEC_MS {
        ms.div_ceil(divisor)
    } else {
        ms / divisor
    };
    let template = plural_template(&format!("{key_prefix}_{suffix}"), n, get)?;
    Some(template.replacen("%d", &n.to_string(), 1))
}

/// [`crate::strings::plural`], the `_P1` pick of `GetText(token, gender, n)`. There is no gender:
/// the formatter passes a literal 0 (`0x52fa50`), which `0x703bf0` reads as nil. With neither key
/// present the lookup yields "" (`0x882748`), which the line core drops, so no line either way.
pub(in crate::script) fn plural_template(
    token: &str,
    n: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    crate::strings::plural(token, Some(n), get)
}

/// A colour component by `lua_tonumber`'s coercion: a numeric string counts, other strings do not.
fn color_num(v: Option<&Value>) -> Option<f32> {
    match v {
        Some(Value::Number(n)) => Some(*n as f32),
        Some(Value::Integer(i)) => Some(*i as f32),
        Some(Value::String(s)) => s.to_str().ok().and_then(|s| s.trim().parse::<f32>().ok()),
        _ => None,
    }
}

/// A wrap flag: nil, false, 0 and the strings "0", "off" and "disabled" are false, anything else
/// true. The reference reads it with `GetBoolOrDefault` (`0x6f1c10`,
/// `binding_abi::bool_or_default`), where "false", "no" and 0.5 are false too.
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

/// An `AddLine`-family colour, gated on the r-slot alone (`lua_isnumber`, `0x6f34d0`), else
/// [`DEFAULT_TEXT_GOLD`]; past the gate a non-number g or b reads 0, and alpha is always opaque.
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

/// An `AddLine` tail, positional `r, g, b, wrap` (`0x531630`); the wrap flag is read whatever the
/// colour gate decides.
fn parse_line_tail(args: &[Value]) -> ([f32; 4], bool) {
    (
        parse_line_color(args.first(), args.get(1), args.get(2)),
        bool_arg(args.get(3)),
    )
}

/// A text argument as a line's string: numbers stringify as Lua does, and nil is empty.
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

/// Appends one line, without showing the tooltip or touching a fade (`0x5303b0`): `AddLine`
/// callers call `Show`. The reference's line core drops a line whose sides are both empty
/// (`0x530270`); this appends it.
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
    // The wrap width is pinned at append, so the first measure already comes back wrapped. It is
    // written from this line's own flag every time, and layout is touched only on a change.
    let pin = wrap.then_some((crate::widget::TOOLTIP_WRAP_WIDTH, 0.0));
    if let Some(d) = model.region_data.get_mut(&lh) {
        if d.size != pin {
            d.size = pin;
            // `SetWidth`, which invalidates (`0x7683d0`), named: a conservative touch would
            // re-derive the whole layout graph on every hover that turns a line wrapped or plain.
            model.invalidate_region(lh, true);
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
    owe_relayout(&mut model, h);
    Ok(())
}

/// A line writer or the show ran the relayout (`0x530640`, from `0x52ad59`, `0x52b029`,
/// `0x52e5ae`, `0x52f869`, `0x530615`, `0x530aad`), which does nothing without an owner
/// (`0x530651`) and else ends in an unconditional invalidate (`0x530a52`).
pub(super) fn owe_relayout(model: &mut Model, h: FrameHandle) {
    if let Ok(t) = tip_mut(model, h) {
        if t.owner.is_some() {
            t.relayout_owed = true;
        }
    }
}

mod verbs;
pub(super) use verbs::install;

/// A line cell's measured extent; `None` for a hidden or unmeasured cell, which adds no size and
/// no gap, so a fresh tooltip keeps its declared size until its lines are measured.
type Cell = Option<(f32, f32)>;

fn cell(model: &Model, rh: crate::widget::RegionHandle) -> Cell {
    let d = model.region_data.get(&rh)?;
    let text = d.text.as_deref()?;
    if d.hidden {
        return None;
    }
    if text.is_empty() {
        // One unit each way, the floor of the line's own rect: the reference's `GetHeight`
        // (`0x772a60`) clamps to one unit, so no FontString reads back 0.
        return Some((
            super::layout::FONTSTRING_MIN_SPAN,
            super::layout::FONTSTRING_MIN_SPAN,
        ));
    }
    d.measured.map(|m| (m.w, m.h))
}

/// `ANCHOR_CURSOR` (mode 6), re-anchored every frame by the update override `0x530b20`: one
/// `SetPoint` (`0x767c70`) pins the plate's BOTTOM to the screen root's BOTTOMLEFT (`0xcf0bd8`)
/// at the cursor position divided by the tooltip's own effective scale, so it sits centred above
/// the cursor. There is no `ClearAllPoints`, and `SetPoint`'s no-op gate keeps a still cursor
/// free. The `SetOwner` offsets play no part (only `0x52fe90` reads them). Only a shown tooltip is
/// in the update pump (`0x76ad9d`), and the placement lands the same frame, before the layout
/// drain (`0x768ed0`).
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
    let units = model.layout_units();
    let input = model.layout_inputs.entry(h).or_default();
    // `0x767c70`'s change detect: the BOTTOM slot is replaced, and the plate invalidated
    // (`0x530b97`, flag 1), only on a real change.
    if input.anchors.set(new, units) {
        model.invalidate_frame(h, true);
    }
}

pub(super) fn layout_tooltips(model: &mut Model) {
    // Only tooltips in the resolve's roster: one outside it would mint an id here and take the
    // layout ledger's conservative branch.
    let tips: Vec<FrameHandle> = model
        .arena
        .tooltip_kinds()
        .iter()
        .copied()
        .filter(|h| model.frame_to_id.contains_key(h))
        .collect();
    for h in tips {
        // Mode 6 runs before the line gate: the reference re-anchors any shown plate, lines or not.
        let cursor_mode = match model.arena.frame(h).map(|f| (&f.kind_state, f.shown)) {
            Some((KindState::Tooltip(t), shown)) => shown && t.anchor == TooltipAnchor::Cursor,
            _ => false,
        };
        if cursor_mode {
            cursor_anchor(model, h);
        }
        // The relayout's closing invalidate (`0x530a52`), whatever it computed. Its own writes
        // below push when they move the plate; past those, a re-resolve gives back the rect the
        // plate holds unless that rect is stale, so only then is the invalidate paid here.
        let owed = tip_mut(model, h).is_ok_and(|t| std::mem::take(&mut t.relayout_owed));
        if owed && model.frame_holds_stale(h) {
            model.note_layout_frame_write(h);
            if let Some(&id) = model.frame_to_id.get(&h) {
                model.queue_invalidate(id);
            }
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
            continue; // nothing measured yet: the declared size holds
        }
        maxw = maxw.max(min_w);
        if maxw <= 0.0 || totalh <= 0.0 {
            continue;
        }
        let input = model.layout_inputs.entry(h).or_default();
        // `pad_w` is `SetPadding`'s extra width (ItemRefTooltip's room for its close button).
        let (w, ht) = (maxw + 2.0 * TOOLTIP_PAD + pad_w, totalh + 2.0 * TOOLTIP_PAD);
        let moved = input.width.to_bits() != w.to_bits() || input.height.to_bits() != ht.to_bits();
        input.width = w;
        input.height = ht;
        // Inside the resolve, so a note and not a touch; the incremental pass still has to hear
        // that these inputs changed. The size goes through `SetWidth`/`SetHeight` (`0x530a2e`,
        // `0x530a39`), which invalidate the tooltip.
        model.note_layout_frame_write(h);
        // A size the measures moved since the relayout ran: benilla's measures land a tick after
        // the line write, where the reference's relayout measured in the call.
        if moved {
            if let Some(&id) = model.frame_to_id.get(&h) {
                model.queue_invalidate(id);
            }
        }
        // Right-flush each shown right cell: one `SetPoint(RIGHT, leftCell, LEFT, width, 0)`
        // (`0x530910`..`0x53094f`), which leaves any other point the cell holds alone.
        let units = model.layout_units();
        for (i, (_, r)) in rows.iter().enumerate() {
            if r.is_none() {
                continue;
            }
            let Some(&left_id) = model.region_to_id.get(&lefts[i]) else {
                continue;
            };
            let new = Anchor::new(Point::Right, left_id, Point::Left, maxw, 0.0);
            let stored = model
                .region_data
                .get_mut(&rights[i])
                .is_some_and(|d| d.anchors.set(new, units));
            if stored {
                model.note_layout_region_write(rights[i]);
                if let Some(&id) = model.region_to_id.get(&rights[i]) {
                    model.queue_invalidate(id);
                }
            }
        }
    }
}

/// The tooltip's own update after its `OnUpdate` (`0x530b20`, from `0x530b9c`): an armed fade
/// counts down by the elapsed time (`0x530baa`–`0x530bc2`). At zero or below it ends in the full
/// hide (`0x530bd8` full alpha, `0x530be9` the hide slot), whose `OnHide` runs here, in the
/// tooltip's place in the walk. Otherwise the alpha is 255 while at least half the duration
/// remains (`0x7ffa24` = 0.5, `0x530c0c`), else `trunc(255 × left / half)` (`0x530c18`–`0x530c2c`,
/// `0x40a2b0` truncates). Nothing gates it on shown, owner or lines.
pub(in crate::script) fn update(lua: &Lua, h: FrameHandle, elapsed: f32) {
    let done = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        // Roster-gated as in `layout_tooltips`, since the hide writes layout.
        if !model.frame_to_id.contains_key(&h) {
            return;
        }
        let Ok(tip) = tip_mut(&mut model, h) else {
            return;
        };
        let Some(left) = tip.fade_left else {
            return;
        };
        let left = left - elapsed;
        tip.fade_left = Some(left);
        if left <= 0.0 {
            true
        } else {
            model.arena.set_alpha(h, fade_alpha(left));
            false
        }
    };
    // The end: full alpha (`0x530bd8`), then the shown bit cleared and the hide slot (`0x530bdf`,
    // `0x530be9`), as the Lua `Hide` does, so the hide's `OnTooltipCleared` sees 255.
    if done {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .arena
            .set_alpha(h, 1.0);
        set_shown(lua, h, false);
    }
}

/// The fade's alpha with `left` seconds to go, in whole 255ths as the reference's `SetAlpha`
/// takes them. The product is formed in f64, where `left × 255` is exact as in the x87 original.
fn fade_alpha(left: f32) -> f32 {
    let half = f64::from(TOOLTIP_FADE_SECS) * 0.5;
    let left = f64::from(left);
    let units = if left < half {
        (left / half * 255.0).trunc()
    } else {
        255.0
    };
    (units / 255.0) as f32
}
