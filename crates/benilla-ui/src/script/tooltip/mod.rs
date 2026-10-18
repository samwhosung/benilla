//! The `GameTooltip` method surface and engine behaviour: the line stack, owner and anchor, the
//! sizer and the fade.
//!
//! Lines are named FontString regions published as Lua globals (`<name>TextLeft1` …), since
//! FrameXML addresses them by name (`GameTooltipTextLeft1:SetTextColor`, `UnitFrame.lua:76`). The
//! post-load pass ([`post_load`]) fixes the pairs a tooltip holds, the template's 30 in the stock
//! one, and only `AddFontStrings` adds to them; a tooltip with none holds no line. Neither touches
//! a pair's points, justification or shown bit: those are the template's.
//!
//! The plate is sized by the show, `AppendText` and the cache answers ([`size::size_tooltip`]).

use mlua::{Lua, Table, Value};

use super::event;
use super::object::frame_handle_of;
use super::visibility;
use super::Model;
use crate::layout::{Anchor, Anchors, Point};
use crate::widget::{FrameHandle, KindState, TooltipAnchor, TooltipState, TOOLTIP_FADE_SECS};

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

/// The FontString a global of that name holds, as `FindFrameOfType` (`0x76c760`) finds it: a Lua
/// global lookup and a type test, no owner check.
fn global_font_string(lua: &Lua, name: &str) -> Option<crate::widget::RegionHandle> {
    let Value::Table(t) = lua.globals().get::<Value>(name).ok()? else {
        return None;
    };
    let rh = super::region::region_handle_of(lua, &t).ok()?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    (model.arena.region(rh)?.kind == crate::widget::RegionKind::FontString).then_some(rh)
}

/// A GameTooltip's post-load pass (`CGTooltip::PostLoadXML`, `0x529650`), run when the frame is
/// made, after its template: it finds `<name>TextLeft<n>` and `<name>TextRight<n>` from 1 until
/// either is missing (`0x52972d`-`0x52983d`; `<unnamed>` for an unnamed tooltip, `0x84c7f0`) and
/// keeps the pairs as the line arrays. Their count is the line limit: the stock template declares
/// 30, and a tooltip made without a template holds none.
pub(crate) fn post_load(lua: &Lua, wrapper: &Table) {
    let Ok(h) = frame_handle_of(lua, wrapper) else {
        return;
    };
    let name = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        if !matches!(
            model.arena.frame(h).map(|f| &f.kind_state),
            Some(KindState::Tooltip(_))
        ) {
            return;
        }
        crate::script::binding_abi::frame_name(&model, h)
    };
    let mut pairs = Vec::new();
    loop {
        let n = pairs.len() + 1;
        let (Some(left), Some(right)) = (
            global_font_string(lua, &format!("{name}TextLeft{n}")),
            global_font_string(lua, &format!("{name}TextRight{n}")),
        ) else {
            break;
        };
        pairs.push((left, right));
    }
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if let Ok(t) = tip_mut(&mut model, h) {
        t.left_lines = pairs.iter().map(|p| p.0).collect();
        t.right_lines = pairs.iter().map(|p| p.1).collect();
        // The second pass stores 0 in each wrap flag (`0x529830`).
        t.wrapped = vec![false; pairs.len()];
    }
}

/// `AddFontStrings` (`0x530c40` into `0x529940`): the caller's pair becomes the last of the line
/// arrays with a clear wrap flag (`0x529a26`), raising the line limit by one. The pair keeps its own
/// points, justification and shown bit.
pub(super) fn add_font_strings(
    lua: &Lua,
    this: &Table,
    left: crate::widget::RegionHandle,
    right: crate::widget::RegionHandle,
) -> mlua::Result<()> {
    with_tip(lua, this, |t| {
        t.left_lines.push(left);
        t.right_lines.push(right);
        t.wrapped.push(false);
    })
}

/// Writes a line cell's text and colour and shows it. The text goes through `SetText` with its
/// expand flag (`0x5302e7`, `0x53033d` for the two sides of a line, `0x53060e` for `AppendText`'s
/// helper `0x5305b0`), so the number separators swap while `SetEuropeanNumbers` holds.
pub(super) fn write_cell(
    model: &mut Model,
    rh: crate::widget::RegionHandle,
    text: &str,
    color: crate::color::Rgba8,
) {
    // The region colour (`0x77f750`, `0x5302d0`), which clears no inherit bit: a later change to
    // the line's font object's colour repaints it until the next write.
    model
        .region_data
        .entry(rh)
        .or_default()
        .set_vertex_color(Some(color));
    model.set_region_hidden(rh, false);
    set_cell_text(model, rh, text);
}

/// A line cell's `SetText(text, 1)` alone (`0x771d80`), touching neither its colour nor its shown
/// bit, as `AppendText`'s helper writes line 1 (`0x53060e`).
pub(super) fn set_cell_text(model: &mut Model, rh: crate::widget::RegionHandle, text: &str) {
    let text = model.localized_text(text.to_string());
    let d = model.region_data.entry(rh).or_default();
    let was_shown = d.text.as_deref().is_some_and(|t| !t.is_empty());
    d.text = Some(text);
    model.touch_measure(rh);
    model.touch_layout_text(rh, was_shown);
}

/// The model half of the clear ([`clear_silent`]), `0x530050`'s loop over the used lines: each left
/// text's width is set to 0, its height kept (`0x5301f0`), its text to "" and both cells are hidden
/// (`0x530203`-`0x530225`), and the wrap flag is zeroed (`0x530236`); a right text is left as it
/// was, so it reads back until the line is written again.
fn clear_content(model: &mut Model, h: FrameHandle) {
    let (lefts, rights) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (
            t.left_lines
                .iter()
                .take(t.num_lines)
                .copied()
                .collect::<Vec<_>>(),
            t.right_lines
                .iter()
                .take(t.num_lines)
                .copied()
                .collect::<Vec<_>>(),
        ),
        _ => return,
    };
    // A line with no authored size already reads width 0.
    for &rh in &lefts {
        if model.region_data.get(&rh).is_some_and(|d| d.size.is_some()) {
            size::set_line_width(model, rh, 0.0);
        }
    }
    for (rh, left) in lefts
        .into_iter()
        .map(|rh| (rh, true))
        .chain(rights.into_iter().map(|rh| (rh, false)))
    {
        let drew = model.region_draws(rh);
        if let Some(d) = model.region_data.get_mut(&rh) {
            let was_shown = d.text.as_deref().is_some_and(|t| !t.is_empty());
            if left {
                d.text = Some(String::new());
            }
            d.hidden = true;
            model.note_region_draw(rh, drew);
            // `measured` stays: the measure cache is keyed by content
            // ([`RegionData::measure_key`]), and the sizer's memo holds the other widths a wrapped
            // line is measured at, so the hover loop's per-frame clear and rebuild
            // (`ContainerFrameItemButton_OnUpdate`) re-measures nothing.
            if left {
                model.touch_layout_text(rh, was_shown);
            }
        }
    }
    if let Ok(t) = tip_mut(model, h) {
        let used = t.num_lines.min(t.wrapped.len());
        t.wrapped[..used].fill(false);
        t.num_lines = 0;
        t.min_width = 0.0;
        // Clear drops the subject and unregisters its health watcher (`0x5300ad`, `0x53007c`).
        t.unit_guid = None;
    }
    // New content replaces a spell render still waiting on its view.
    model.spell_tooltip_waits.remove(&h);
    // The clear zeroes the kept item entry (`0x5300dd`), so a cache answer rebuilds nothing.
    model.item_builds.remove(&h);
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
/// (`0x530a83`–`0x530a95`) it restores full alpha, disarms the fade (`0x530aa3`), sizes itself
/// (`0x530aad`), so a shown tooltip's `Show` sizes it again, and runs the base show, whose `OnShow`
/// reads the new size; else it clears its shown bit and hides through its own hide (`0x530abe`,
/// `0x530ac8`). So a parent's show brings back only a tooltip that still has an owner.
pub(in crate::script) fn show_override(lua: &Lua, h: FrameHandle) {
    let live = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let live = tip_mut(&mut model, h).is_ok_and(|t| t.owner.is_some() && t.num_lines > 0);
        if live {
            full_alpha(&mut model, h);
            size::size_tooltip(&mut model, h);
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
    t.owner_offset = (x, y);
    if let Some(owner) = owner {
        t.anchor = anchor;
        apply_anchor(&mut model, h, owner, anchor, x, y, true);
    }
}

/// The anchor apply (`0x52fe90`): `ANCHOR_PRESERVE` returns first (`0x52fead`); every other mode
/// clears all points (`0x767ed0` at `0x52fec2`) but `ANCHOR_NONE` without `clear`: the sizer
/// passes 0 (`0x530a45`), the SetOwner core 1 (`0x53002d`); modes 0..5 then set one point
/// on the owner (`0x52ffbc`), and 6 and 7 stop there. The core's clear lets
/// `GameTooltip_SetDefaultAnchor` (GameTooltip.lua:76-77) set one point with no stale owner
/// anchor; mode 6 is then placed per frame by `cursor_anchor`.
fn apply_anchor(
    model: &mut Model,
    h: FrameHandle,
    owner: FrameHandle,
    anchor: TooltipAnchor,
    x: f32,
    y: f32,
    clear: bool,
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
    if anchor == TooltipAnchor::Cursor || (anchor == TooltipAnchor::None && clear) {
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
    model.arena.set_alpha(h, 255);
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

/// A colour component by `lua_tonumber`'s coercion: a number or a string `luaO_str2d` reads as
/// one, else none.
fn color_num(lua: &Lua, v: Option<&Value>) -> Option<f32> {
    lua.coerce_number(v?.clone())
        .ok()
        .flatten()
        .map(|n| n as f32)
}

/// A line's wrap flag, `GetBoolOrDefault` (`0x6f1c10`) with default 0 at `AddLine`'s sixth
/// argument (`0x53181a`), `SetText`'s seventh (`0x531ddc`) and `AddDoubleLine`'s tenth (`0x531b69`).
fn wrap_arg(v: Option<&Value>) -> bool {
    crate::script::binding_abi::bool_or_default(v, false)
}

/// An `AddLine`-family colour, gated on the r-slot alone (`lua_isnumber`, `0x6f34d0`), else
/// [`DEFAULT_TEXT_GOLD`]; past the gate a non-number g or b reads 0, and alpha is always opaque.
fn parse_line_color(
    lua: &Lua,
    r: Option<&Value>,
    g: Option<&Value>,
    b: Option<&Value>,
) -> [f32; 4] {
    match color_num(lua, r) {
        Some(r) => [
            r,
            color_num(lua, g).unwrap_or(0.0),
            color_num(lua, b).unwrap_or(0.0),
            1.0,
        ],
        None => DEFAULT_TEXT_GOLD,
    }
}

/// An `AddLine` tail, positional `r, g, b, wrap` (`0x531630`); the wrap flag is read whatever the
/// colour gate decides.
fn parse_line_tail(lua: &Lua, args: &[Value]) -> ([f32; 4], bool) {
    (
        parse_line_color(lua, args.first(), args.get(1), args.get(2)),
        wrap_arg(args.get(3)),
    )
}

/// A line's text argument as `SetText`, `AddLine` and `AddDoubleLine` read it: `lua_isstring` then
/// `lua_tostring` (`0x531c25`, `0x5316dc`, `0x5318ef`, `0x53190f`), so a number is its `%.14g`
/// text and the bytes of a string are kept, lossily; anything else is no text.
fn text_of(lua: &mlua::Lua, v: Option<&Value>) -> String {
    v.and_then(|v| crate::script::binding_abi::optional_string(lua, v))
        .unwrap_or_default()
}

/// Appends one line, without showing the tooltip or touching a fade (`0x5303b0`): `AddLine`
/// callers call `Show`. The line core (`0x530270`) clears the wrap flag when the right text is
/// non-empty (`0x53027d`–`0x530284`), adds no line when both texts are empty
/// (`0x5302a9`–`0x5302b4`) or the array is full (`0x530291`), and writes only a non-empty side
/// (`0x5302ba`, `0x53030a`).
pub(super) fn append_line(
    lua: &Lua,
    this: &Table,
    left: (String, [f32; 4]),
    right: Option<(String, [f32; 4])>,
    wrap: bool,
) -> mlua::Result<()> {
    let right_empty = right.as_ref().is_none_or(|(text, _)| text.is_empty());
    if left.0.is_empty() && right_empty {
        return Ok(());
    }
    let wrap = wrap && right_empty;
    let h = frame_handle_of(lua, this)?;
    let (cap, have) = with_tip(lua, this, |t| (t.left_lines.len(), t.num_lines))?;
    // A full line array adds nothing (`0x530291`).
    if have >= cap {
        return Ok(());
    }
    let n = have + 1;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let (lh, rh) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (t.left_lines[n - 1], t.right_lines[n - 1]),
        _ => return Err(mlua::Error::runtime("not a GameTooltip")),
    };
    if !left.0.is_empty() {
        write_cell(
            &mut model,
            lh,
            &left.0,
            crate::color::Rgba8::from_f32(left.1),
        );
    }
    if let Some((text, color)) = right.filter(|(text, _)| !text.is_empty()) {
        write_cell(&mut model, rh, &text, crate::color::Rgba8::from_f32(color));
    }
    if let Ok(t) = tip_mut(&mut model, h) {
        // The wrap flag (`0x53036f`), read by the sizer alone; the line is not sized here.
        if let Some(flag) = t.wrapped.get_mut(n - 1) {
            *flag = wrap;
        }
        t.num_lines = n;
    }
    Ok(())
}

mod size;
mod verbs;
pub(in crate::script) use size::{size_owed, size_tooltip};
pub(super) use verbs::install;

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

/// The resolve's tooltip pre-pass: each shown `ANCHOR_CURSOR` plate is placed on the cursor
/// ([`cursor_anchor`]), lines or not.
pub(super) fn anchor_cursor_tooltips(model: &mut Model) {
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
        let cursor_mode = match model.arena.frame(h).map(|f| (&f.kind_state, f.shown)) {
            Some((KindState::Tooltip(t), shown)) => shown && t.anchor == TooltipAnchor::Cursor,
            _ => false,
        };
        if cursor_mode {
            cursor_anchor(model, h);
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
        // Roster-gated as in `anchor_cursor_tooltips`, since the hide writes layout.
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
            .set_alpha(h, 255);
        set_shown(lua, h, false);
    }
}

/// The fade's alpha byte with `left` seconds to go, as the reference hands the alpha walk whole
/// 255ths. The product is formed in f64, where `left × 255` is exact as in the x87 original.
fn fade_alpha(left: f32) -> u8 {
    let half = f64::from(TOOLTIP_FADE_SECS) * 0.5;
    let left = f64::from(left);
    let units = if left < half {
        (left / half * 255.0).trunc()
    } else {
        255.0
    };
    units as u8
}
