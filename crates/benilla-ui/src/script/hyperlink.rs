//! A text frame's hyperlink spans as the reference builds them: each span is a frame of its own, a
//! `CSimpleHyperlinkButton` (ctor `0x7a3240`) on the Button constructor (`0x7786a0`), so a
//! mouse-enabled child of the text frame with no name and no scripts. It takes the hover, the
//! press, the capture and the click like any Button, and its vtable (`0x81d5b0`) differs from the
//! Button's (`0x81c7f8`) in three code slots, each handing its link (`+0x4dc`) and text (`+0x4e0`)
//! to the text frame: the enter (`+0x4c`, `0x7a34b0`) to `OnHyperlinkEnter`, the leave (`+0x50`,
//! `0x7a34e0`) to `OnHyperlinkLeave`, and the click (`+0x94`, `0x7a3510`) to `OnHyperlinkClick`
//! (the parent's firers `0x7a3780`/`0x7a37b0`/`0x7a37e0`). The enter and the leave pass the link
//! alone (`"%s"`, `0x82e280`); the click passes the link, the text and the button name. Its click
//! mask is `LeftButtonUp` and `RightButtonUp` (`0x500`, `0x7a327d`).
//!
//! The text frame keeps a free list (`[+0x314]`) and a live list. A rebuild (`0x788c00`, and
//! SimpleHTML's at `0x78afdf`) compares nothing: it releases every live button (`0x7a3920`), which
//! hides, so a hovered one gets its leave with its old link and a held press loses its capture, and
//! goes to the free list's tail. Then each span takes the free list's head (`0x7a3880`, a new
//! button when it is empty), which is placed on its line and shown (`0x7a3350`); nothing is ever
//! freed. A SimpleHTML rebuilds from its update (`0x78a30b`) when `SetText` raised its flag, and
//! never on a resize. A ScrollingMessageFrame rebuilds from its update (`0x788595`) whenever its
//! flag is up ([`crate::widget::ScrollingMessageState::links_dirty`]), and a change of its
//! on-screen width or height relayouts it (`0x7882f0` → `0x788310` → `0x788750`), a rescale
//! included, since the rect it compares is in screen units (`0x788342` scales the insets by
//! `+0x7c`). The buttons hang
//! off their line (`0x7a3441`), so a frame that only moves carries them along with no rebuild.
//! The app lays the text out, so its spans arrive here as screen rects every frame, and a frame
//! not rebuilt has its buttons re-seated in place.

use std::collections::VecDeque;

use mlua::{Lua, Value};

use crate::layout::{Anchor, Anchors, Point, Rect};
use crate::widget::{FrameHandle, FrameKind, KindState};

use super::{event, visibility, Model, UiScript, SCREEN};

/// The text frame and the `(link, text)` a hyperlink button carries, if `h` is one.
pub(super) fn owner_link(model: &Model, h: FrameHandle) -> Option<(FrameHandle, String, String)> {
    let f = model.arena.frame(h)?;
    match &f.kind_state {
        KindState::Button(bs) => {
            let (link, text) = bs.hyperlink.clone()?;
            Some((f.parent?, link, text))
        }
        _ => None,
    }
}

/// Fire the text frame's `OnHyperlinkEnter` or `OnHyperlinkLeave` for the button `h`, with the link
/// alone; false when `h` is not a hyperlink button, whose own enter or leave then runs.
pub(super) fn hover(lua: &Lua, h: FrameHandle, enter: bool) -> bool {
    let Some((owner, link, _)) = owner_link(&lua.app_data_ref::<Model>().expect("model"), h) else {
        return false;
    };
    let script = if enter {
        "OnHyperlinkEnter"
    } else {
        "OnHyperlinkLeave"
    };
    fire(lua, owner, script, &[&link]);
    true
}

/// Fire the text frame's `OnHyperlinkClick(link, text, button)` for the button `h`; false when `h`
/// is not a hyperlink button, whose own `OnClick` then runs.
pub(super) fn click(lua: &Lua, h: FrameHandle, button: &str) -> bool {
    let Some((owner, link, text)) = owner_link(&lua.app_data_ref::<Model>().expect("model"), h)
    else {
        return false;
    };
    fire(lua, owner, "OnHyperlinkClick", &[&link, &text, button]);
    true
}

fn fire(lua: &Lua, owner: FrameHandle, script: &str, args: &[&str]) {
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        if model.arena.frame(owner).is_none() {
            return;
        }
        model.frame_id(owner)
    };
    let args: mlua::Result<Vec<Value>> = args
        .iter()
        .map(|a| lua.create_string(a).map(Value::String))
        .collect();
    let result = args.and_then(|args| event::fire_widget_handler(lua, id, script, args));
    if let Err(e) = result {
        lua.app_data_mut::<Model>()
            .expect("model")
            .record_lua_error(&e);
    }
}

/// One span: its y-up rect, link and text.
type Span = (Rect, String, String);

/// A text frame's hyperlink buttons: the live ones in placement order, each over the span at its
/// index, and the free list, taken from the head and released to the tail.
#[derive(Clone, Default)]
pub(crate) struct LinkPool {
    live: Vec<FrameHandle>,
    free: VecDeque<FrameHandle>,
    /// The text frame's on-screen size at the last pass, for the resize relayout.
    size: Option<(f32, f32)>,
}

/// Replace the spans on screen, `(text frame, y-up rect, link, text)`, a text frame's in the
/// order its rebuild places them. Only a ScrollingMessageFrame's and a SimpleHTML's take buttons;
/// a MessageFrame's stay text. A text frame due a rebuild ([`due`]) gets one: every live button
/// hides, in order, its leave reading the link it still carries, then each span takes a free
/// button, is placed, and shows. Any other has its live buttons re-seated on their spans with no hide and no show,
/// as buttons anchored to their lines follow a moved frame.
pub(super) fn set_spans(lua: &Lua, spans: Vec<(FrameHandle, Rect, String, String)>) {
    let mut by_owner: Vec<(FrameHandle, Vec<Span>)> = Vec::new();
    for (owner, rect, link, text) in spans {
        match by_owner.iter_mut().find(|(o, _)| *o == owner) {
            Some((_, list)) => list.push((rect, link, text)),
            None => by_owner.push((owner, vec![(rect, link, text)])),
        }
    }
    {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        let model = &mut *model;
        model
            .link_buttons
            .retain(|owner, _| model.arena.frame(*owner).is_some());
        // Text frames with live buttons and no span now come after the ones with spans, in a
        // fixed order over the hashed map.
        let mut emptied: Vec<FrameHandle> = model
            .link_buttons
            .iter()
            .filter(|(owner, pool)| {
                !pool.live.is_empty() && !by_owner.iter().any(|(o, _)| o == *owner)
            })
            .map(|(owner, _)| *owner)
            .collect();
        emptied.sort_by_key(|h| h.fingerprint_bits());
        // Only the SMF (`0x788ce7`) and SimpleHTML (`0x78b052`) take one.
        by_owner.retain(|(owner, _)| {
            model.simple_html.contains_key(owner)
                || model
                    .arena
                    .frame(*owner)
                    .is_some_and(|f| matches!(f.kind_state, KindState::ScrollingMessage(_)))
        });
        by_owner.extend(emptied.into_iter().map(|owner| (owner, Vec::new())));
    }
    let mut moved = false;
    for (owner, list) in by_owner {
        moved |= if due(lua, owner, &list) {
            rebuild(lua, owner, &list)
        } else {
            follow(lua, owner, &list)
        };
    }
    // The pointer hit-tests resolved rects, so a moved button is solved before the next input.
    if moved {
        UiScript::resolve_layout(&mut lua.app_data_mut::<Model>().expect("model"));
    }
}

/// Whether `owner` rebuilds this pass: when its flag is up, which this spends, or, for a
/// ScrollingMessageFrame, its on-screen size moved (`0x788394`, `0x7883b2`); a SimpleHTML's resize
/// rebuilds nothing.
/// Deviation: a span count that no longer matches the live set always rebuilds, so a state this
/// model does not flag (a ring wiped by `SetMaxLines`, a hidden frame, a frame not yet laid out,
/// which the reference's rebuild skips at `0x78af7b`) leaves no stale button.
fn due(lua: &Lua, owner: FrameHandle, list: &[Span]) -> bool {
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let model = &mut *model;
    // A message frame's lines' box, which `0x788384`-`0x7883b2` compares.
    let size = super::messageframe::message_rect(model, owner)
        .map(|r| (r.right - r.left, r.top - r.bottom));
    let (flagged, sized) = match model.arena.frame_mut(owner).map(|f| &mut f.kind_state) {
        Some(KindState::ScrollingMessage(smf)) => (std::mem::take(&mut smf.links_dirty), true),
        _ => (
            model
                .simple_html
                .get_mut(&owner)
                .is_some_and(|st| std::mem::take(&mut st.links_dirty)),
            false,
        ),
    };
    let pool = model.link_buttons.entry(owner).or_default();
    let resized = std::mem::replace(&mut pool.size, size).is_some_and(|was| Some(was) != size);
    flagged || (sized && resized) || pool.live.len() != list.len()
}

/// Re-seat `owner`'s live buttons on `list`, one each, with no hide and no show. True when a
/// button's anchors or size changed.
fn follow(lua: &Lua, owner: FrameHandle, list: &[Span]) -> bool {
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let model = &mut *model;
    let live = model
        .link_buttons
        .get(&owner)
        .map(|p| p.live.clone())
        .unwrap_or_default();
    let mut moved = false;
    for (&h, (rect, link, text)) in live.iter().zip(list) {
        moved |= place(model, h, *rect, link, text);
    }
    moved
}

/// One text frame's rebuild (`0x788c00`) onto `list`. True when a button's anchors or size
/// changed.
fn rebuild(lua: &Lua, owner: FrameHandle, list: &[Span]) -> bool {
    let released = {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        let pool = model.link_buttons.entry(owner).or_default();
        let released = std::mem::take(&mut pool.live);
        pool.free.extend(released.iter().copied());
        released
    };
    // `0x7a3920`: each hides before any is re-placed, so a hovered one leaves with its old link.
    for &h in &released {
        visibility::set_shown(lua, h, false);
    }
    let mut moved = false;
    let mut shows = Vec::with_capacity(list.len());
    {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        let model = &mut *model;
        // A leave handler may have destroyed the text frame.
        if model.arena.frame(owner).is_none() {
            model.link_buttons.remove(&owner);
            return false;
        }
        for (rect, link, text) in list {
            let free = model
                .link_buttons
                .get_mut(&owner)
                .and_then(|p| p.free.pop_front());
            let h = free.unwrap_or_else(|| build(model, owner));
            moved |= place(model, h, *rect, link, text);
            shows.push(h);
        }
        let pool = model.link_buttons.entry(owner).or_default();
        pool.live = shows.clone();
    }
    for h in shows {
        visibility::set_shown(lua, h, true);
    }
    moved
}

/// `0x7a3880`'s miss: a new button, a Button child of the text frame in its stratum one level up,
/// as a child is made, registered for `LeftButtonUp` and `RightButtonUp`. It is born hidden, so the
/// caller's show is its one transition, as `0x7a3350`'s `Show` is.
fn build(model: &mut Model, owner: FrameHandle) -> FrameHandle {
    let h = model.arena.create(FrameKind::Button, None, Some(owner));
    if let Some((strata, level)) = model.arena.frame(owner).map(|f| (f.strata, f.level)) {
        model.arena.set_frame_strata(h, strata);
        model.arena.set_frame_level(h, level + 1, false);
    }
    model.set_shown_silent(h, false);
    if let Some(KindState::Button(bs)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        bs.registered_clicks = ["LeftButtonUp", "RightButtonUp"]
            .into_iter()
            .map(String::from)
            .collect();
    }
    model.frame_id(h);
    model.layout_inputs.entry(h).or_default();
    h
}

/// `0x7a3350`: store the link and text, and seat the button over the span, one TOPLEFT on the
/// screen with an explicit size, in its own units. True when its anchors or size changed.
fn place(model: &mut Model, h: FrameHandle, rect: Rect, link: &str, text: &str) -> bool {
    if let Some(KindState::Button(bs)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        if bs
            .hyperlink
            .as_ref()
            .is_none_or(|(l, t)| l != link || t != text)
        {
            bs.hyperlink = Some((link.to_string(), text.to_string()));
        }
    }
    let scale = f64::from(super::object::eff_scale(model, h));
    let root = model.screen;
    let x = ((f64::from(rect.left) - f64::from(root.left)) / scale) as f32;
    let y = ((f64::from(rect.top) - f64::from(root.top)) / scale) as f32;
    let w = ((f64::from(rect.right) - f64::from(rect.left)) / scale) as f32;
    let ht = ((f64::from(rect.top) - f64::from(rect.bottom)) / scale) as f32;
    let pin = Anchors::from([Anchor::new(Point::TopLeft, SCREEN, Point::TopLeft, x, y)]);
    let input = model.layout_inputs.entry(h).or_default();
    let changed = !input.anchors.bits_eq(&pin)
        || input.width.bits() != crate::layout::Len::Script(w).bits()
        || input.height.bits() != crate::layout::Len::Script(ht).bits();
    if changed {
        input.anchors = pin;
        input.width = crate::layout::Len::Script(w);
        input.height = crate::layout::Len::Script(ht);
        model.note_authored_size(h);
    }
    // The points are cleared (`0x7a3415`), one set with the invalidate flag (`0x7a3441`), and the
    // size set through the setters, which invalidate (`0x7a3457`, `0x7a346b`).
    model.invalidate_frame(h, changed);
    changed
}
