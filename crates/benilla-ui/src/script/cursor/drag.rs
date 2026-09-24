//! The `RegisterForDrag` gesture: armed on a press, started past a threshold, ended on release.

use crate::script::Model;
use crate::widget::FrameHandle;

/// The drag-start distance in pixels, exceeded strictly; the reference starts at 0.01 frame units
/// or more from the press (`0x81c468`, squared by `0x76b300`).
pub(crate) const DRAG_START_THRESHOLD: f32 = 4.0;

/// A drag armed at a press on a [`Model::drag_registered`] frame, `started` past the threshold.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DragGesture {
    pub(crate) button: String,
    pub(crate) source: FrameHandle,
    pub(crate) start: (f32, f32),
    pub(crate) started: bool,
}

/// A gesture whose button matched the release; [`Model::drag`] is cleared either way.
pub(crate) struct DragRelease {
    /// `OnDragStart` fired; an unstarted release fires nothing and the click proceeds.
    pub(crate) started: bool,
    pub(crate) source: FrameHandle,
}

/// Arm or clear the gesture on a press, replacing any leftover one; the caller runs
/// [`abandon_drag`] first so a started gesture still gets its `OnDragStop`.
pub(crate) fn arm_drag(model: &mut Model, hit: Option<FrameHandle>, button: &str, pos: (f32, f32)) {
    model.drag = hit
        .filter(|&h| {
            model
                .drag_registered
                .get(&h)
                .is_some_and(|set| set.iter().any(|b| b.eq_ignore_ascii_case(button)))
        })
        .map(|h| DragGesture {
            button: button.to_string(),
            source: h,
            start: pos,
            started: false,
        });
}

/// Start the armed gesture once the cursor passes the threshold, returning the `OnDragStart`
/// source id and button once per gesture; a source that died fires nothing.
pub(crate) fn maybe_start_drag(model: &mut Model, pos: (f32, f32)) -> Option<(u32, String)> {
    let (source, button) = {
        let g = model.drag.as_ref()?;
        if g.started {
            return None;
        }
        let (sx, sy) = g.start;
        let dist = ((pos.0 - sx).powi(2) + (pos.1 - sy).powi(2)).sqrt();
        if dist <= DRAG_START_THRESHOLD {
            return None;
        }
        (g.source, g.button.clone())
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

/// Drop a gesture whose release will never come, because the pointer left the window or a second
/// button was pressed; the reference cannot lose one, as the OS captures the pointer. Returns a
/// started one's source, which must get `OnDragStop` or a frame it is moving follows the cursor.
pub(crate) fn abandon_drag(model: &mut Model) -> Option<FrameHandle> {
    model.drag.take().filter(|g| g.started).map(|g| g.source)
}

/// Take the gesture whose button matches the release.
pub(crate) fn take_drag(model: &mut Model, button: &str) -> Option<DragRelease> {
    let g = model
        .drag
        .take_if(|g| g.button.eq_ignore_ascii_case(button))?;
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
            a:SetScript("OnDragStart", function(self, button) drag_starts = drag_starts + 1; drag_button = button end)
            a:SetScript("OnDragStop", function(self) drag_stops = drag_stops + 1 end)
            a:SetScript("OnClick", function(self) click_a = click_a + 1 end)
            local b = CreateFrame("Frame", "B")
            b:SetPoint("BOTTOMLEFT", 400, 0); b:SetWidth(400); b:SetHeight(600); b:EnableMouse(true)
            b:SetScript("OnReceiveDrag", function(self) receives = receives + 1 end)
            b:SetScript("OnClick", function(self) click_b = click_b + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();

        s.mouse_button(100.0, 300.0, "LeftButton", true); // press inside A
        s.mouse_move(102.0, 300.0); // 2px: under the threshold
        assert_eq!(s.eval::<i64>("return drag_starts").unwrap(), 0);

        s.mouse_move(110.0, 300.0); // 10px from the press: starts
        assert_eq!(s.eval::<i64>("return drag_starts").unwrap(), 1);
        assert_eq!(
            s.eval::<String>("return drag_button").unwrap(),
            "LeftButton"
        );

        s.mouse_move(120.0, 300.0); // further movement doesn't re-fire
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
            a:SetScript("OnClick", function(self) clicks = clicks + 1 end)
            "#,
        )
        .unwrap();
        s.resolve();

        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_move(110.0, 300.0); // starts the drag
        s.mouse_button(100.0, 300.0, "LeftButton", false); // released back on A
        assert_eq!(s.eval::<i64>("return clicks").unwrap(), 0);
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
            a:SetScript("OnClick", function(self) clicks = clicks + 1 end)
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
            f:SetScript("OnEvent", function(self, event, n, q) heard = heard + 1; name = n; quality = q end)
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

    #[test]
    fn a_press_that_replaces_an_in_flight_gesture_stops_it_first() {
        let mut s = drag_script();
        s.run(
            r#"
            stops, starts = 0, 0
            local a = CreateFrame("Frame", "A")
            a:SetPoint("BOTTOMLEFT", 100, 100); a:SetWidth(200); a:SetHeight(100)
            a:EnableMouse(true); a:SetMovable(true)
            a:RegisterForDrag("LeftButton")
            a:SetScript("OnDragStart", function() starts = starts + 1; this:StartMoving() end)
            a:SetScript("OnDragStop", function() stops = stops + 1; this:StopMovingOrSizing() end)
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
            1,
            "the left drag the right press displaced was stopped, not forgotten"
        );

        // Replacing an unstarted press fires nothing, as releasing it would not.
        s.mouse_button(150.0, 150.0, "LeftButton", true);
        s.mouse_button(150.0, 150.0, "MiddleButton", true);
        assert_eq!(s.eval::<i64>("return stops").unwrap(), 1);
    }
}
