//! The `RegisterForDrag` gesture: armed on a press, started past a threshold, ended on release.

use crate::script::Model;
use crate::widget::FrameHandle;

/// The drag-start threshold, 0.01 layout units squared: `0x76b300` stores `0.01f × 0.01f`
/// (`0x81c468` = `0.01f`) in `[0xcf0c14]` as an f32.
const DRAG_START_DISTANCE_SQ: f32 = 0.01 * 0.01;

/// Whether `pos` is far enough from the press point `start` to start the drag (`0x76b34c`–`0x76b372`).
/// The reference measures in layout units, where the screen diagonal is 1 (`0x41ad80`, from the
/// press and move converters `0x425666`/`0x425766`), so the pointer, in root units, is divided by
/// [`Model::layout_unit`]: 1% of the diagonal at every screen size and UI scale. The squared
/// distance is taken in `f64` against the f32 threshold (x87 at 53 bits: `WinMain`'s
/// `_controlfp(0x9001f)`, `D3DCREATE_FPU_PRESERVE`), and only a smaller one skips (`0x76b372`
/// `jne`), so a NaN, which also sets the carry flag, skips too.
fn past_start_distance(model: &Model, start: (f32, f32), pos: (f32, f32)) -> bool {
    let unit = model.layout_unit();
    let axis = |a: f32, b: f32| f64::from(a / unit) - f64::from(b / unit);
    let (dx, dy) = (axis(pos.0, start.0), axis(pos.1, start.1));
    dx * dx + dy * dy >= f64::from(DRAG_START_DISTANCE_SQ)
}

/// The event button bits in order, `1`..`0x10` (`0x76bf70`'s name table).
const HELD_ORDER: [&str; 5] = [
    "LeftButton",
    "MiddleButton",
    "RightButton",
    "Button4",
    "Button5",
];

/// A drag armed at a press on a [`Model::drag_registered`] frame, `started` past the threshold.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DragGesture {
    pub(crate) button: String,
    pub(crate) source: FrameHandle,
    pub(crate) start: (f32, f32),
    pub(crate) started: bool,
}

/// A gesture whose frame's drag mask held the released button; [`Model::drag`] is cleared either way.
pub(crate) struct DragRelease {
    /// `OnDragStart` fired; an unstarted release fires nothing and the click proceeds.
    pub(crate) started: bool,
    pub(crate) source: FrameHandle,
}

/// The press arm `0x76bf70`, run on the frame the press lands on (the capture, else the hover). A
/// button in the frame's drag mask (`0x76bf7d`) re-arms: armed, not started, this button, this
/// point (`0x76bf82`..`0x76bfab`), with no `OnDragStop` for a started drag it replaces. Any other
/// button skips the arm (`0x76bf80` je `0x76bfb1`) and leaves the armed or started drag alone.
///
/// The reference keeps this state per frame and benilla has the one [`Model::drag`], which holds
/// the same thing because a press always lands on the capture, the frame of any live gesture. A
/// gesture on another frame, or one whose button's release was lost (`release_lost`; the
/// reference cannot lose one), can no longer be moved or released there, so it is abandoned and
/// the returned source of a started one must get `OnDragStop`, or a frame it is moving follows the
/// cursor.
pub(crate) fn press_drag(
    model: &mut Model,
    target: Option<FrameHandle>,
    button: &str,
    pos: (f32, f32),
    release_lost: bool,
) -> Option<FrameHandle> {
    let elsewhere = model
        .drag
        .as_ref()
        .is_some_and(|g| Some(g.source) != target);
    let displaced = if release_lost || elsewhere {
        abandon_drag(model)
    } else {
        None
    };
    if let Some(h) = target.filter(|h| is_drag_button(model, *h, button)) {
        model.drag = Some(DragGesture {
            button: button.to_string(),
            source: h,
            start: pos,
            started: false,
        });
    }
    displaced
}

/// Whether `button` is in the frame's `RegisterForDrag` mask (`[+0xfc]`).
fn is_drag_button(model: &Model, frame: FrameHandle, button: &str) -> bool {
    model
        .drag_registered
        .get(&frame)
        .is_some_and(|set| set.iter().any(|b| b.eq_ignore_ascii_case(button)))
}

/// Start the armed gesture once the cursor passes the threshold, returning the `OnDragStart`
/// source id and button once per gesture; a source that died fires nothing.
pub(crate) fn maybe_start_drag(model: &mut Model, pos: (f32, f32)) -> Option<(u32, String)> {
    let (source, button) = {
        let g = model.drag.as_ref()?;
        if g.started {
            return None;
        }
        if !past_start_distance(model, g.start, pos) {
            return None;
        }
        // `OnDragStart`'s argument is the lowest button of the move event's held mask
        // (`0x76c1e0` over `[evt+0x18]`), not the armed one: a chord re-armed by the later button
        // starts as the earlier. The held set is the buttons whose press took a frame.
        let held = HELD_ORDER.iter().find(|name| {
            model
                .mouse_down_on
                .keys()
                .any(|k| k.eq_ignore_ascii_case(name))
        });
        (
            g.source,
            held.map_or_else(|| g.button.clone(), |name| (*name).to_string()),
        )
    };
    model.drag.as_mut().expect("checked Some above").started = true;
    // A dragged button un-presses at the threshold, not on leaving its rect: `CSimpleButton`'s
    // `+0x74` override (`0x7793f0`) un-presses (`0x779410`) before firing `OnDragStart`.
    super::super::button::edge(model, source, crate::widget::ButtonState::on_drag_start);
    let id = model
        .arena
        .frame(source)
        .is_some()
        .then(|| model.frame_id(source))?;
    Some((id, button))
}

/// Drop a gesture whose release will never come, because the pointer left the window; the
/// reference cannot lose one, as the OS captures the pointer. Returns a started one's source,
/// which must get `OnDragStop` or a frame it is moving follows the cursor.
pub(crate) fn abandon_drag(model: &mut Model) -> Option<FrameHandle> {
    model.drag.take().filter(|g| g.started).map(|g| g.source)
}

/// Take the gesture a release ends. `0x76c050` tests the released button against the frame's
/// drag mask, not the armed button `[+0x108]`, so any mask button's release clears the arm
/// (`0x76c05d`) and, with the drag started, is the drag release (`0x76c06e`).
pub(crate) fn take_drag(model: &mut Model, button: &str) -> Option<DragRelease> {
    let source = model.drag.as_ref()?.source;
    if !is_drag_button(model, source, button) {
        return None;
    }
    let g = model.drag.take()?;
    Some(DragRelease {
        started: g.started,
        source: g.source,
    })
}

#[cfg(test)]
mod tests {
    use crate::script::cursor::{CursorItem, CursorPayload};
    use crate::script::UiScript;

    fn drag_script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s
    }

    #[test]
    fn drag_start_stop_and_receive_suppress_onclick() {
        let mut s = drag_script();
        s.run(
            r#"
            drag_starts, drag_stops, receives = 0, 0, 0
            click_a, click_b = 0, 0
            drag_button = nil
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:RegisterForDrag("LeftButton")
            a:SetScript("OnDragStart", function() local self, button = this, arg1 drag_starts = drag_starts + 1; drag_button = button end)
            a:SetScript("OnDragStop", function() local self = this drag_stops = drag_stops + 1 end)
            a:SetScript("OnClick", function() local self = this click_a = click_a + 1 end)
            local b = CreateFrame("Frame", "B")
            b:SetPoint("BOTTOMLEFT", 400, 0); b:SetWidth(400); b:SetHeight(600); b:EnableMouse(true)
            b:SetScript("OnReceiveDrag", function() local self = this receives = receives + 1 end)
            b:SetScript("OnClick", function() local self = this click_b = click_b + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();

        s.mouse_button(100.0, 300.0, "LeftButton", true); // press inside A
        s.mouse_move(102.0, 300.0); // 2 units: under the threshold
        assert_eq!(s.eval::<i64>("return drag_starts").unwrap(), 0);

        s.mouse_move(120.0, 300.0); // 20 units from the press: starts
        assert_eq!(s.eval::<i64>("return drag_starts").unwrap(), 1);
        assert_eq!(
            s.eval::<String>("return drag_button").unwrap(),
            "LeftButton"
        );

        s.mouse_move(140.0, 300.0); // further movement doesn't re-fire
        assert_eq!(s.eval::<i64>("return drag_starts").unwrap(), 1);

        s.mouse_move(600.0, 300.0); // over B now
        let consumed = s.mouse_button(600.0, 300.0, "LeftButton", false); // release over B
        assert!(consumed);
        assert_eq!(
            s.eval::<i64>("return drag_stops").unwrap(),
            1,
            "OnDragStop on the source"
        );
        assert_eq!(
            s.eval::<i64>("return receives").unwrap(),
            1,
            "OnReceiveDrag on the target"
        );
        assert_eq!(
            s.eval::<i64>("return click_a").unwrap(),
            0,
            "no OnClick on the source"
        );
        assert_eq!(
            s.eval::<i64>("return click_b").unwrap(),
            0,
            "no OnClick on the target"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    #[test]
    fn drag_release_on_the_source_still_suppresses_onclick() {
        // A plain Frame fires OnClick on a same-frame release; a started drag suppresses it.
        let mut s = drag_script();
        s.run(
            r#"
            clicks = 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:RegisterForDrag("LeftButton")
            a:SetScript("OnClick", function() local self = this clicks = clicks + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();

        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_move(120.0, 300.0); // starts the drag
        s.mouse_button(100.0, 300.0, "LeftButton", false); // released back on A
        assert_eq!(s.eval::<i64>("return clicks").unwrap(), 0);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// A frame whose every mouse script appends to `log`, registered for a LeftButton drag.
    fn logged_drag_frame() -> UiScript {
        let mut s = drag_script();
        s.run(
            r#"
            log = {}
            local function note(name) return function() table.insert(log, name) end end
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:RegisterForDrag("LeftButton")
            a:SetScript("OnMouseDown", note("down"))
            a:SetScript("OnMouseUp", note("up"))
            a:SetScript("OnDragStart", note("dragstart"))
            a:SetScript("OnDragStop", note("dragstop"))
            a:SetScript("OnReceiveDrag", note("receive"))
            "#,
        )
        .unwrap();
        s.resolve();
        s
    }

    fn log(s: &UiScript) -> String {
        s.eval::<String>("return table.concat(log, ' ')").unwrap()
    }

    /// `0x76c040` on a release for a drag-registered button whose drag has started clears the arm,
    /// fires `OnDragStop` (`0x76c06e`) and the hover frame's drop (`0x76c081`) and returns 1
    /// (`0x76c08f`), never reaching its `OnMouseUp` (`0x76c099`).
    #[test]
    fn a_drag_release_fires_dragstop_and_no_mouseup() {
        let mut s = logged_drag_frame();
        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_move(130.0, 300.0); // 30 units: past the threshold
        s.mouse_button(130.0, 300.0, "LeftButton", false);
        assert_eq!(log(&s), "down dragstart dragstop receive");
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// A press that never crossed the threshold falls through to `OnMouseUp` (`0x76c067` je).
    #[test]
    fn a_plain_click_on_a_drag_frame_fires_mouseup() {
        let mut s = logged_drag_frame();
        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_button(100.0, 300.0, "LeftButton", false);
        assert_eq!(log(&s), "down up");
    }

    /// The gate `0x76c050` tests the released button against the frame's registered buttons: a
    /// frame registered for LeftButton drag still gets `OnMouseUp` for a RightButton release.
    #[test]
    fn a_button_not_registered_for_drag_still_fires_mouseup() {
        let mut s = logged_drag_frame();
        s.mouse_button(100.0, 300.0, "RightButton", true);
        s.mouse_move(130.0, 300.0);
        s.mouse_button(130.0, 300.0, "RightButton", false);
        assert_eq!(log(&s), "down up");
    }

    /// A stock resize grip starts sizing in `OnMouseDown` and is not drag-registered, so its
    /// `OnMouseUp` (which stops the sizing) fires even after the cursor left it.
    #[test]
    fn a_resize_grip_still_gets_mouseup() {
        let mut s = drag_script();
        s.run(
            r#"
            ups = 0
            local f = CreateFrame("Frame", "Sized", UIParent)
            f:SetResizable(true)
            f:SetPoint("BOTTOMLEFT", 100, 100); f:SetWidth(200); f:SetHeight(100)
            local g = CreateFrame("Button", "Grip", f)
            g:SetPoint("BOTTOMRIGHT", 0, 0); g:SetWidth(16); g:SetHeight(16)
            g:EnableMouse(true)
            g:SetScript("OnMouseDown", function() Sized:StartSizing("BOTTOMRIGHT") end)
            g:SetScript("OnMouseUp", function() ups = ups + 1; Sized:StopMovingOrSizing() end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.mouse_button(295.0, 105.0, "LeftButton", true);
        s.mouse_move(395.0, 55.0); // dragged 100 right, 50 down, off the grip
        s.mouse_button(395.0, 55.0, "LeftButton", false);
        s.resolve();
        assert_eq!(s.eval::<i64>("return ups").unwrap(), 1);
        assert_eq!(s.eval::<f32>("return Sized:GetWidth()").unwrap(), 300.0);
        s.mouse_move(500.0, 300.0);
        s.resolve();
        assert_eq!(
            s.eval::<f32>("return Sized:GetWidth()").unwrap(),
            300.0,
            "the sizing ended on the release"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    #[test]
    fn drag_not_started_leaves_the_ordinary_click_path_untouched() {
        let mut s = drag_script();
        s.run(
            r#"
            clicks = 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:RegisterForDrag("LeftButton")
            a:SetScript("OnClick", function() local self = this clicks = clicks + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();

        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_move(101.0, 300.0); // 1px: never starts
        s.mouse_button(100.0, 300.0, "LeftButton", false);
        assert_eq!(
            s.eval::<i64>("return clicks").unwrap(),
            1,
            "a plain click still fires"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// The world drop (`0x495300`) runs on a world click's release, never on a drag release.
    #[test]
    fn drag_release_over_nothing_keeps_carrying_no_popup() {
        let mut s = drag_script();
        s.run(
            r#"
            heard = 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:RegisterForDrag("LeftButton")
            local f = CreateFrame("Frame", "Listener")
            f:RegisterEvent("DELETE_ITEM_CONFIRM")
            f:SetScript("OnEvent", function() heard = heard + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
            count: None,
            quality: Some(3),
            equip_slots: Vec::new(),
        }));

        s.mouse_button(100.0, 300.0, "LeftButton", true); // press inside A
        s.mouse_move(110.0, 300.0); // starts the drag
        s.mouse_button(-50.0, -50.0, "LeftButton", false); // released over the world
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 0, "no popup");
        assert!(s.cursor_item().is_some(), "the payload keeps carrying");

        // The trigger is a full click on the world, down and up over nothing.
        s.mouse_button(-50.0, -50.0, "LeftButton", true);
        assert_eq!(
            s.eval::<i64>("return heard").unwrap(),
            0,
            "the press alone is not the trigger"
        );
        let consumed = s.mouse_button(-50.0, -50.0, "LeftButton", false);
        assert!(consumed, "a world-drop click consumes the event");
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 1);
        assert!(
            s.cursor_item().is_some(),
            "the payload stays until the popup decides"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    #[test]
    fn world_click_over_nothing_drops_a_held_item() {
        let mut s = drag_script();
        s.run(
            r#"
            heard, name, quality = 0, nil, nil
            local f = CreateFrame("Frame", "Listener")
            f:RegisterEvent("DELETE_ITEM_CONFIRM")
            f:SetScript("OnEvent", function() local self, event, n, q = this, event, arg1, arg2 heard = heard + 1; name = n; quality = q end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None, // no link ⇒ an empty name, not an error
            count: None,
            quality: None, // unknown quality ⇒ 0
            equip_slots: Vec::new(),
        }));

        s.mouse_button(-50.0, -50.0, "LeftButton", true);
        let consumed = s.mouse_button(-50.0, -50.0, "LeftButton", false); // the completed click
        assert!(consumed, "a world drop consumes the event");
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 1);
        assert_eq!(s.eval::<String>("return name").unwrap(), "");
        assert_eq!(s.eval::<i64>("return quality").unwrap(), 0);
        assert!(s.cursor_item().is_some(), "the payload stays held");
    }

    /// A click on the full-screen stock `WorldFrame` is a world click: in the reference, its press
    /// runs the frame's `OnMouseDown` without consuming, then the `BUTTON1`/`BUTTON2` binding.
    #[test]
    fn a_click_on_the_world_frame_is_a_world_drop() {
        let mut s = drag_script();
        s.run(
            r#"
            WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()
            heard = 0
            local f = CreateFrame("Frame", "Listener")
            f:RegisterEvent("DELETE_ITEM_CONFIRM")
            f:SetScript("OnEvent", function() heard = heard + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        }));

        // The premise: the click lands on the world frame.
        let hit = s
            .hit_test(400.0, 300.0)
            .expect("the world frame is full-screen");
        assert!(s.is_world_frame(hit));

        s.mouse_button(400.0, 300.0, "LeftButton", true);
        let consumed = s.mouse_button(400.0, 300.0, "LeftButton", false);
        assert!(consumed, "a world drop consumes the completed click");
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 1, "the popup fires");
        assert!(s.cursor_item().is_some(), "the payload stays held");
    }

    #[test]
    fn a_click_on_a_frame_above_the_world_frame_is_not_a_world_drop() {
        let mut s = drag_script();
        s.run(
            r#"
            WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()
            heard = 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            local f = CreateFrame("Frame", "Listener")
            f:RegisterEvent("DELETE_ITEM_CONFIRM")
            f:SetScript("OnEvent", function() heard = heard + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        }));

        s.mouse_button(100.0, 300.0, "LeftButton", true);
        let consumed = s.mouse_button(100.0, 300.0, "LeftButton", false);
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 0, "no popup");
        assert!(s.cursor_item().is_some());
        assert!(
            consumed,
            "the plate ate the click — that IS the UI consuming it"
        );

        // A press and release split across the plate and the world, either way: no world click.
        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_button(600.0, 300.0, "LeftButton", false);
        s.mouse_button(600.0, 300.0, "LeftButton", true);
        s.mouse_button(100.0, 300.0, "LeftButton", false);
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 0, "still no popup");
        assert!(s.cursor_item().is_some());
    }

    #[test]
    fn a_world_frame_click_with_no_payload_consumes_nothing() {
        let mut s = drag_script();
        s.run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
            .unwrap();
        s.resolve();
        s.mouse_button(400.0, 300.0, "LeftButton", true);
        assert!(
            !s.mouse_button(400.0, 300.0, "LeftButton", false),
            "a bare world click is the world's, not the interface's"
        );
    }

    /// Over a world object the reference's object leg (`0x492ce0`) selects and keeps the payload.
    #[test]
    fn world_click_over_a_world_object_is_not_a_world_drop() {
        let mut s = drag_script();
        s.run(
            r#"
            heard = 0
            local f = CreateFrame("Frame", "Listener")
            f:RegisterEvent("DELETE_ITEM_CONFIRM")
            f:SetScript("OnEvent", function() heard = heard + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        }));
        s.set_world_pick(crate::script::WorldPick::Object);

        s.mouse_button(-50.0, -50.0, "LeftButton", true);
        let consumed = s.mouse_button(-50.0, -50.0, "LeftButton", false);
        assert!(
            !consumed,
            "over an object the click is the world's, not a drop"
        );
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 0);
        assert!(s.cursor_item().is_some(), "the item payload survives");
    }

    #[test]
    fn frame_press_world_release_is_not_a_world_drop() {
        let mut s = drag_script();
        s.run(
            r#"
            heard = 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            local f = CreateFrame("Frame", "Listener")
            f:RegisterEvent("DELETE_ITEM_CONFIRM")
            f:SetScript("OnEvent", function() heard = heard + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        }));

        s.mouse_button(100.0, 300.0, "LeftButton", true); // press on A (no RegisterForDrag)
        s.mouse_button(-50.0, -50.0, "LeftButton", false); // release over the world
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 0);
        assert!(s.cursor_item().is_some());
    }

    /// A press whose frame hid (the removal cleared the capture, `0x764cad`) reaches no one on the
    /// release: `0x766420` dispatches to the capture alone, and the world hook `0x492b50` runs
    /// only in freelook, so the world click `0x495300` never runs.
    #[test]
    fn a_press_whose_frame_hid_is_not_a_world_drop() {
        let mut s = drag_script();
        s.run(
            r#"
            WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()
            heard = 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:SetScript("OnMouseDown", function() A:Hide() end)
            local f = CreateFrame("Frame", "Listener")
            f:RegisterEvent("DELETE_ITEM_CONFIRM")
            f:SetScript("OnEvent", function() heard = heard + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        }));

        s.mouse_button(100.0, 300.0, "LeftButton", true); // press on A, which hides itself
        s.mouse_button(600.0, 300.0, "LeftButton", false); // release over the world
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 0, "no popup");
        assert!(s.cursor_item().is_some());
    }

    /// A non-item payload's world click clears it silently (`0x495300`); a drag release keeps it.
    #[test]
    fn world_click_with_a_spell_payload_clears_silently() {
        let mut s = drag_script();
        s.run(
            r#"
            heard = 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:RegisterForDrag("LeftButton")
            local f = CreateFrame("Frame", "Listener")
            f:RegisterEvent("DELETE_ITEM_CONFIRM")
            f:SetScript("OnEvent", function() heard = heard + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.set_cursor_for_test(CursorPayload::Spell(crate::script::cursor::CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 1,
            texture: None,
        }));

        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_move(110.0, 300.0);
        s.mouse_button(-50.0, -50.0, "LeftButton", false); // drag release: keeps carrying
        assert!(s.cursor_payload().is_some(), "a drag release never drops");

        s.mouse_button(-50.0, -50.0, "LeftButton", true);
        s.mouse_button(-50.0, -50.0, "LeftButton", false); // the world click clears
        s.tick(0.01);
        assert_eq!(
            s.eval::<i64>("return heard").unwrap(),
            0,
            "no delete popup for a spell"
        );
        assert!(s.cursor_payload().is_none(), "cleared silently");
    }

    #[test]
    fn a_gesture_the_pointer_carries_out_of_the_window_still_fires_its_stop() {
        let mut s = drag_script();
        s.run(
            r#"
            stops = 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 100, 100); a:SetWidth(200); a:SetHeight(100)
            a:EnableMouse(true); a:SetMovable(true)
            a:RegisterForDrag("LeftButton")
            a:SetScript("OnDragStart", function() this:StartMoving() end)
            a:SetScript("OnDragStop", function() stops = stops + 1; this:StopMovingOrSizing() end)
            "#,
        )
        .unwrap();
        s.resolve();

        s.mouse_button(150.0, 150.0, "LeftButton", true);
        s.mouse_move(200.0, 200.0); // past the threshold: OnDragStart, StartMoving
        s.mouse_move(250.0, 250.0); // StartMoving anchors on its call; this move carries
        s.resolve();
        let carried: f32 = s.eval("return A:GetLeft()").unwrap();
        assert_eq!(carried, 150.0, "the frame followed the cursor's 50px");

        s.pointer_left_window();
        assert_eq!(
            s.eval::<i64>("return stops").unwrap(),
            1,
            "the abandon fires the same OnDragStop a release would"
        );

        // The move slot is free: nothing follows the cursor.
        s.mouse_move(600.0, 500.0);
        s.resolve();
        assert_eq!(
            s.eval::<f32>("return A:GetLeft()").unwrap(),
            carried,
            "the frame stayed where the abandoned drag left it"
        );
        // Fires once, not once per frame the pointer stays outside.
        s.pointer_left_window();
        s.pointer_left_window();
        assert_eq!(s.eval::<i64>("return stops").unwrap(), 1);
    }

    /// Each press runs the arm `0x76bf70` on the capture: a button outside the frame's drag mask
    /// (`0x76bf7d` je `0x76bfb1`) skips the arm and touches neither `[+0x100]` nor `[+0x104]`, so
    /// the started left drag runs on and only its own release (`0x76c040`) fires `OnDragStop`.
    #[test]
    fn a_press_of_a_button_the_frame_does_not_drag_leaves_the_drag_running() {
        let mut s = drag_script();
        s.run(
            r#"
            stops, starts, ups = 0, 0, 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 100, 100); a:SetWidth(200); a:SetHeight(100)
            a:EnableMouse(true); a:SetMovable(true)
            a:RegisterForDrag("LeftButton")
            a:SetScript("OnDragStart", function() starts = starts + 1; this:StartMoving() end)
            a:SetScript("OnDragStop", function() stops = stops + 1; this:StopMovingOrSizing() end)
            a:SetScript("OnMouseUp", function() ups = ups + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();

        s.mouse_button(150.0, 150.0, "LeftButton", true);
        s.mouse_move(200.0, 200.0);
        assert_eq!(s.eval::<i64>("return starts").unwrap(), 1);

        s.mouse_button(200.0, 200.0, "RightButton", true);
        assert_eq!(
            s.eval::<i64>("return stops").unwrap(),
            0,
            "the right press does not touch the left drag"
        );
        s.mouse_move(250.0, 250.0);
        s.mouse_move(300.0, 300.0);
        s.resolve();
        assert_eq!(
            s.eval::<f32>("return A:GetLeft()").unwrap(),
            200.0,
            "the frame still follows the cursor"
        );
        assert_eq!(
            s.eval::<i64>("return starts").unwrap(),
            1,
            "no second start"
        );

        // The right release is no drag release (`0x76c050` je `0x76c099`): its `OnMouseUp` fires.
        s.mouse_button(300.0, 300.0, "RightButton", false);
        assert_eq!(s.eval::<i64>("return stops").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return ups").unwrap(), 1);

        s.mouse_button(300.0, 300.0, "LeftButton", false);
        assert_eq!(
            s.eval::<i64>("return stops").unwrap(),
            1,
            "the left release ends the drag"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// A frame registered for both buttons.
    fn logged_two_button_drag_frame() -> UiScript {
        let mut s = drag_script();
        s.run(
            r#"
            log = {}
            local function note(name) return function() table.insert(log, name) end end
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:RegisterForDrag("LeftButton", "RightButton")
            a:SetScript("OnMouseDown", note("down"))
            a:SetScript("OnMouseUp", note("up"))
            a:SetScript("OnDragStart", note("dragstart"))
            a:SetScript("OnDragStop", note("dragstop"))
            a:SetScript("OnReceiveDrag", note("receive"))
            "#,
        )
        .unwrap();
        s.resolve();
        s
    }

    /// A press of a registered button re-arms (`0x76bf82`..`0x76bfab`: armed 1, started 0, the new
    /// button and start point) and fires no `OnDragStop`. `0x76b320` then starts again from the new
    /// point (`0x76b34c`), a second `OnDragStart`; the release of a mask button ends a started
    /// drag whichever button armed it (`0x76c050`..`0x76c06e` read the released button only).
    #[test]
    fn a_press_of_a_registered_button_re_arms_without_a_stop() {
        let mut s = logged_two_button_drag_frame();
        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_move(130.0, 300.0);
        assert_eq!(log(&s), "down dragstart");

        s.mouse_button(130.0, 300.0, "RightButton", true);
        assert_eq!(log(&s), "down dragstart down", "no OnDragStop");

        s.mouse_move(132.0, 300.0); // 2px from the new arm point: under the threshold
        assert_eq!(log(&s), "down dragstart down");
        s.mouse_move(160.0, 300.0); // past it: started again
        assert_eq!(log(&s), "down dragstart down dragstart");

        s.mouse_button(160.0, 300.0, "RightButton", false);
        assert_eq!(
            log(&s),
            "down dragstart down dragstart dragstop receive",
            "the release ends the started drag, no OnMouseUp"
        );
        // The drag is over: the left release finds it unarmed and is a plain release.
        s.mouse_button(160.0, 300.0, "LeftButton", false);
        assert_eq!(log(&s), "down dragstart down dragstart dragstop receive up");
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// `0x76c050` tests the released button against the mask, not `[+0x108]`: the first button's
    /// release after the re-arm clears the arm (`0x76c05d`), and with the drag not restarted
    /// (`[+0x104]` 0, `0x76c067` je `0x76c099`) is a plain `OnMouseUp`. Nothing starts afterwards.
    #[test]
    fn the_first_buttons_release_after_a_re_arm_disarms_it() {
        let mut s = logged_two_button_drag_frame();
        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_move(130.0, 300.0);
        s.mouse_button(130.0, 300.0, "RightButton", true);
        s.mouse_button(130.0, 300.0, "LeftButton", false);
        assert_eq!(log(&s), "down dragstart down up", "no OnDragStop");

        s.mouse_move(200.0, 300.0); // the arm is cleared: the right button drags nothing
        assert_eq!(log(&s), "down dragstart down up");
        s.mouse_button(200.0, 300.0, "RightButton", false);
        assert_eq!(log(&s), "down dragstart down up up");
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// `OnDragStart`'s button is not the armed one: `0x76c1e0` names the lowest bit of the move
    /// event's held mask (`[evt+0x18]`, the producer `0x423fe0` stores `ds:0x883f40` there), so
    /// with Left still held a drag re-armed by Right starts as `LeftButton`.
    #[test]
    fn on_drag_start_names_the_lowest_held_button() {
        let mut s = drag_script();
        s.run(
            r#"
            names = {}
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:RegisterForDrag("LeftButton", "RightButton")
            a:SetScript("OnDragStart", function() table.insert(names, arg1) end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_move(130.0, 300.0);
        s.mouse_button(130.0, 300.0, "RightButton", true); // re-arms for Right, Left still held
        s.mouse_move(160.0, 300.0);
        s.mouse_button(160.0, 300.0, "RightButton", false);
        s.mouse_button(160.0, 300.0, "LeftButton", false);
        s.mouse_button(160.0, 300.0, "RightButton", true); // Right alone
        s.mouse_move(200.0, 300.0);
        assert_eq!(
            s.eval::<String>("return table.concat(names, ' ')").unwrap(),
            "LeftButton LeftButton RightButton"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// The cursor payload is no part of the arm: a right click during an item drag leaves the
    /// carried item and the drag, and the left release still drops on the frame under the cursor.
    #[test]
    fn an_item_drag_survives_a_right_click() {
        let mut s = drag_script();
        s.run(
            r#"
            stops, receives = 0, 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 0, 0); a:SetWidth(400); a:SetHeight(600); a:EnableMouse(true)
            a:RegisterForDrag("LeftButton")
            a:SetScript("OnDragStop", function() stops = stops + 1 end)
            local b = CreateFrame("Frame", "B")
            b:SetPoint("BOTTOMLEFT", 400, 0); b:SetWidth(400); b:SetHeight(600); b:EnableMouse(true)
            b:SetScript("OnReceiveDrag", function() receives = receives + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        }));

        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_move(110.0, 300.0); // the drag starts
        s.mouse_button(110.0, 300.0, "RightButton", true);
        s.mouse_button(110.0, 300.0, "RightButton", false);
        assert_eq!(s.eval::<i64>("return stops").unwrap(), 0);
        assert!(s.cursor_item().is_some(), "the item is still carried");

        s.mouse_move(600.0, 300.0);
        s.mouse_button(600.0, 300.0, "LeftButton", false);
        assert_eq!(s.eval::<i64>("return stops").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return receives").unwrap(),
            1,
            "the drop reaches the frame under the cursor"
        );
        assert!(s.cursor_item().is_some(), "a drag release never drops");
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}
