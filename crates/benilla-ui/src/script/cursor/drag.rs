//! The drag gesture (decision 0216 §3) — `RegisterForDrag`/`OnDragStart`/`OnDragStop`/
//! `OnReceiveDrag`'s mechanics: arm on press, start past a threshold, resolve on release. Split
//! out of [`super`] purely for size — [`super::pointer`](crate::script::pointer) is the only
//! external caller (via [`super`]'s re-exports), and the payload types/transition seam this
//! drives live one level up.

use crate::script::Model;
use crate::widget::FrameHandle;

/// INTERIM pixel drag-start threshold (0216 §5 (d) is the byte-verified trigger; the reference
/// almost certainly uses a small OS/engine drag-distance constant, not this one). Compared with
/// **strict** `>` — a move exactly at the threshold does not start the gesture.
pub(crate) const DRAG_START_THRESHOLD: f32 = 4.0;

/// An in-flight drag gesture: armed at mouse-down on a [`Model::drag_registered`] frame,
/// `started` once the cursor has moved past [`DRAG_START_THRESHOLD`] from the press point.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DragGesture {
    pub(crate) button: String,
    pub(crate) source: FrameHandle,
    pub(crate) start: (f32, f32),
    pub(crate) started: bool,
}

/// A resolved drag release (a gesture whose button matched the release) — [`Model::drag`] is
/// always cleared alongside this, win or lose.
pub(crate) struct DragRelease {
    /// Whether the gesture crossed [`DRAG_START_THRESHOLD`] (so `OnDragStart` fired). A released
    /// gesture that never started fires nothing — the normal click path proceeds untouched.
    pub(crate) started: bool,
    pub(crate) source: FrameHandle,
}

/// Arm (or clear) the drag gesture on a mouse-DOWN. `hit` is the pressed frame (`None` = the
/// press hit nothing); a press always REPLACES any leftover in-flight gesture — its matching
/// release should already have cleared it, but a press is the one moment we know for certain no
/// gesture should still be armed against a *different* earlier press.
///
/// **The replaced gesture is abandoned, not forgotten**: the caller runs [`abandon_drag`] first
/// and fires `OnDragStop` for a started one. See that function for why a silent drop is the bug.
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

/// Advance the armed gesture on a mouse-move: once the cursor has moved past
/// [`DRAG_START_THRESHOLD`] from the press point, mark it started. Returns `(source id, button)`
/// to fire `OnDragStart` with, exactly once per gesture (already-started and no-gesture both
/// answer `None`); a source that died before starting fires nothing.
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
    // **The button's drag-start edge** — `CSimpleButton` overrides `+0x74` with `0x7793f0`, which
    // un-presses (`0x779410`, guarded on `locked == 0 && state != DISABLED`) and then forwards to
    // the base notify that fires `<OnDragStart>`. So a button you drag off releases its pushed art
    // at the THRESHOLD crossing, which is here — not when the cursor leaves its rect, and not for
    // a frame that never registered for drag (decision 2134).
    super::super::button::edge(model, source, crate::widget::ButtonState::on_drag_start);
    let id = model
        .arena
        .frame(source)
        .is_some()
        .then(|| model.frame_id(source))?;
    Some((id, button))
}

/// **Abandon** an in-flight gesture that will never see its release, and report whether anything
/// has to be told. Returns the source of a gesture that had **started**, so the caller fires the
/// same `OnDragStop` a real release would; an armed-but-unstarted press answers `None`, exactly as
/// releasing one does.
///
/// Two callers, and both are situations the reference simply cannot be in: the OS captures the
/// pointer for the whole of a button-held drag there, so the release always arrives. Ours does
/// not — the cursor can walk out of the window, and a second button can be pressed mid-gesture —
/// and dropping the gesture *silently* in either case is what leaves the UI stuck rather than
/// merely cancelled: the addon's `OnDragStop → StopMovingOrSizing` never runs, so the engine's
/// single [`Model::moving`] slot stays taken, [`super::super::object::movable::advance_move`]
/// keeps gluing that frame to the cursor for the rest of the session, and it swallows every press
/// aimed at anything underneath. (B310 — the raid grid, where one drag off the window edge cost
/// every later drag.)
pub(crate) fn abandon_drag(model: &mut Model) -> Option<FrameHandle> {
    model.drag.take().filter(|g| g.started).map(|g| g.source)
}

/// Resolve (and clear) the in-flight drag gesture matching a mouse-button release's `button`, if
/// any (case-insensitive, the `RegisterForClicks` precedent).
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
        s.mouse_move(102.0, 300.0); // 2px — under the threshold
        assert_eq!(s.eval::<i64>("return drag_starts").unwrap(), 0);

        s.mouse_move(110.0, 300.0); // 10px from the press point — starts
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
        // Without the suppression, a same-frame press+release Up would normally fire OnClick
        // (a plain Frame's OnClick fires on any same-frame release — see `input.rs`'s click
        // tests) — proving the drag path takes precedence over that default.
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
        s.mouse_move(101.0, 300.0); // 1px — never starts
        s.mouse_button(100.0, 300.0, "LeftButton", false);
        assert_eq!(
            s.eval::<i64>("return clicks").unwrap(),
            1,
            "a plain click still fires"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// A drag released over the world does NOTHING to the payload — no popup, no clear (decision
    /// 0218, byte-verified: `0x495300` runs on the WorldFrame click release only; a drag release
    /// routes as a drag, never a click — the director's "it should require an outside up+down").
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

        // The follow-up CLICK on the world (down + up, both over nothing) is the trigger.
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

    /// **The world frame IS the world** — B380 / decision 2089, the regression this file's
    /// other world-drop tests were structurally blind to.
    ///
    /// Every one of them clicks at `(-50, -50)`, where the hit test answers `None`, and the gate
    /// used to be spelled `hit_id.is_none() && pressed.is_none()`. Decision 1983 put the stock
    /// `WorldFrame` on the manifest — full-screen, mouse-enabled, `SetAllPoints` — so from that
    /// day every click a player makes on the world hits a frame, the gate went permanently false
    /// and dropping an item on the ground stopped doing anything at all. It is exactly the
    /// reference's own click target for a world click (1984: "the press runs the frame's Lua
    /// `OnMouseDown` (never consuming), then the `BUTTON1`/`BUTTON2` binding — which **is** the
    /// world click"), so it belongs on the yes side of the predicate.
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

        // The fixture's own premise: the click really does land on the world frame.
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

    /// …and a UI frame ON TOP of the world frame is still the UI: press and release both land on
    /// the plate, so no drop — the other half of [`super::over_world`]'s gate, which a predicate
    /// that merely asked "is a world frame anywhere under the cursor" would get wrong.
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

        // The mixed pair, both ways round: a press on the plate released over the world, and a
        // press on the world released over the plate. Neither is a completed world click.
        s.mouse_button(100.0, 300.0, "LeftButton", true);
        s.mouse_button(600.0, 300.0, "LeftButton", false);
        s.mouse_button(600.0, 300.0, "LeftButton", true);
        s.mouse_button(100.0, 300.0, "LeftButton", false);
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return heard").unwrap(), 0, "still no popup");
        assert!(s.cursor_item().is_some());
    }

    /// A click on the world frame with an EMPTY cursor is not the UI consuming anything — the
    /// return the app's arbiter would read if it ever asked. 1984: the world frame's press
    /// "never consum[es]".
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

    /// A world object (unit/GameObject) under the cursor suppresses the world drop entirely
    /// (decisions 0571 + 0574): the reference's object-leg dispatcher (`0x492ce0`) keeps every
    /// real payload and runs SELECT — no `DELETE_ITEM_CONFIRM`, payload untouched. The app
    /// feeds the pick (`set_world_pick`); tests/captures default `Nothing`.
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

    /// A press over a FRAME whose release lands on the world is neither a click on the frame nor
    /// a world click — the payload is untouched.
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

    /// A spell/action payload world-CLICK clears silently (`0x495300`'s non-item arms); a drag
    /// release over the world keeps it carrying, same as an item.
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

    /// **A drag the pointer carries out of the window ENDS, and the frame it was moving stops
    /// following the cursor** (B310).
    ///
    /// The reference cannot reach this state — the OS holds the pointer for a button-held drag, so
    /// the release always arrives — which is exactly why nothing in FrameXML defends against it and
    /// why the engine has to. Abandoning the gesture *silently* (what this used to do) skips the
    /// canonical `OnDragStop → StopMovingOrSizing`, so the single move slot stays taken and
    /// `advance_move` glues the frame to the cursor for the rest of the session, on top of whatever
    /// the player is trying to click next.
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
        s.mouse_move(200.0, 200.0); // past the threshold ⇒ OnDragStart ⇒ StartMoving
                                    // `StartMoving` samples the cursor where it is CALLED, so the starting move moves nothing
                                    // (`tests::movable`'s own note) — this is the one that carries the frame.
        s.mouse_move(250.0, 250.0);
        s.resolve();
        let carried: f32 = s.eval("return A:GetLeft()").unwrap();
        assert_eq!(carried, 150.0, "the frame followed the cursor's 50px");

        s.pointer_left_window();
        assert_eq!(
            s.eval::<i64>("return stops").unwrap(),
            1,
            "the abandon fires the same OnDragStop a release would"
        );

        // …and the move slot is free, so nothing follows the cursor any more.
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

    /// The same abandon, by the other door: a fresh press REPLACES an in-flight gesture, and a
    /// started one has to be told. Pressing a second button mid-drag is an ordinary slip of the
    /// hand, and it used to leave the same stuck move slot.
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

        // An armed-but-UNSTARTED press fires nothing when it is replaced — the same silence a
        // release of one gets.
        s.mouse_button(150.0, 150.0, "LeftButton", true);
        s.mouse_button(150.0, 150.0, "MiddleButton", true);
        assert_eq!(s.eval::<i64>("return stops").unwrap(), 1);
    }
}
