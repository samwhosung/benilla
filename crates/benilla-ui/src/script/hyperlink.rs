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
//! The text frame keeps a pool (`[+0x314]`): a span takes a button from it (`0x7a3880`), which is
//! placed over the span and shown (`0x7a3350`), and a span gone hides its button again (`0x7a3920`),
//! freeing nothing. The app lays the text out, so its spans arrive here as screen rects.

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

/// Replace the spans on screen, `(text frame, y-up rect, link, text)`: each text frame's spans take
/// its pooled buttons in order, a new one built when the pool runs out, and the buttons left over,
/// or of a text frame with no span now, hide.
pub(super) fn set_spans(lua: &Lua, spans: Vec<(FrameHandle, Rect, String, String)>) {
    // One span: its rect, link and text.
    type Span = (Rect, String, String);
    let mut by_owner: Vec<(FrameHandle, Vec<Span>)> = Vec::new();
    for (owner, rect, link, text) in spans {
        match by_owner.iter_mut().find(|(o, _)| *o == owner) {
            Some((_, list)) => list.push((rect, link, text)),
            None => by_owner.push((owner, vec![(rect, link, text)])),
        }
    }
    let (mut show, mut hide) = (Vec::new(), Vec::new());
    let mut moved = false;
    {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        let model = &mut *model;
        model
            .link_buttons
            .retain(|owner, _| model.arena.frame(*owner).is_some());
        for (owner, list) in &by_owner {
            if model.arena.frame(*owner).is_none() {
                continue;
            }
            for (i, (rect, link, text)) in list.iter().enumerate() {
                let pooled = model
                    .link_buttons
                    .get(owner)
                    .and_then(|pool| pool.get(i))
                    .copied();
                let h = pooled.unwrap_or_else(|| build(model, *owner));
                moved |= place(model, h, *rect, link, text);
                if !model.arena.frame(h).is_some_and(|f| f.shown) {
                    show.push(h);
                }
            }
        }
        for (owner, pool) in &model.link_buttons {
            let used = by_owner
                .iter()
                .find(|(o, _)| o == owner)
                .map_or(0, |(_, list)| list.len());
            hide.extend(
                pool.iter()
                    .skip(used)
                    .filter(|&&h| model.arena.frame(h).is_some_and(|f| f.shown)),
            );
        }
    }
    for h in hide {
        visibility::set_shown(lua, h, false);
    }
    for h in show {
        visibility::set_shown(lua, h, true);
    }
    // The pointer hit-tests resolved rects, so a moved button is solved before the next input.
    if moved {
        UiScript::resolve_layout(&mut lua.app_data_mut::<Model>().expect("model"));
    }
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
    model.arena.set_shown(h, false);
    if let Some(KindState::Button(bs)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        bs.registered_clicks = ["LeftButtonUp", "RightButtonUp"]
            .into_iter()
            .map(String::from)
            .collect();
    }
    model.frame_id(h);
    model.layout_inputs.entry(h).or_default();
    model.link_buttons.entry(owner).or_default().push(h);
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
        || input.width.to_bits() != w.to_bits()
        || input.height.to_bits() != ht.to_bits();
    if changed {
        input.anchors = pin;
        input.width = w;
        input.height = ht;
        model.touch_layout_frame(h);
        model.note_authored_size(h);
    }
    changed
}
