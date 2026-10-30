//! The mouse capture (`root+0x80`): what drops it, and where a press goes while one is held.

use super::common::script;
use crate::script::UiScript;

/// Two side-by-side mouse frames A (x 100..300) and B (x 400..600), both y 200..400, each logging
/// `name:down:<button>` / `name:up:<button>` / `name:enter` / `name:leave`.
fn two_frames(kind: &str) -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(&format!(
        r#"
        log = {{}}
        local function note(f, what) table.insert(log, f:GetName()..":"..what) end
        local function mk(name, x)
            local f = CreateFrame("{kind}", name)
            f:SetPoint("BOTTOMLEFT", x, 200); f:SetWidth(200); f:SetHeight(200); f:EnableMouse(true)
            f:SetScript("OnMouseDown", function() note(f, "down:"..arg1) end)
            f:SetScript("OnMouseUp", function() note(f, "up:"..arg1) end)
            f:SetScript("OnEnter", function() note(f, "enter") end)
            f:SetScript("OnLeave", function() note(f, "leave") end)
            return f
        end
        mk("A", 100)
        mk("B", 400)
    "#
    ))
    .unwrap();
    s.resolve();
    s
}

fn log_of(s: &mut UiScript) -> String {
    s.eval::<String>("return table.concat(log, ' ')").unwrap()
}

// ── EnableMouse(false): the removal tail (`0x77711a` → `0x76af50` → `0x764ba0`) ─────────────────

/// A frame that turns click-through in its own `OnMouseDown` loses the capture with the index
/// entry (`0x764ca5`–`0x764cad`), and the release goes to the capture alone (`0x766498`).
#[test]
fn a_frame_mouse_disabled_while_pressed_gets_no_release() {
    let mut s = two_frames("Frame");
    s.run(
        r#"A:SetScript("OnMouseDown", function() table.insert(log, "A:down") A:EnableMouse(false) end)"#,
    )
    .unwrap();
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    assert!(
        s.model_ref().mouse_capture.is_none(),
        "the disable cleared the capture slot"
    );
    s.mouse_button(200.0, 300.0, "LeftButton", false);
    assert_eq!(log_of(&mut s), "A:enter A:down A:leave");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The same removal runs for a disable outside the handler, and a pressed frame that is not the
/// hover loses its press just as well.
#[test]
fn a_disable_after_the_press_drops_the_release_whatever_the_hover() {
    let mut s = two_frames("Frame");
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.mouse_move(500.0, 300.0); // the hover moved on to B; A keeps the capture
    s.run("A:EnableMouse(false)").unwrap();
    s.mouse_button(500.0, 300.0, "LeftButton", false);
    assert_eq!(log_of(&mut s), "A:enter A:down:LeftButton A:leave B:enter");
}

/// The armed drag (`+0x100`/`+0x104`) goes with the leave notify (`0x76b6f0`): a frame that
/// disables its mouse in `OnMouseDown` gets no `OnDragStart` when the cursor moves.
#[test]
fn a_frame_mouse_disabled_while_pressed_gets_no_drag_start() {
    let mut s = two_frames("Frame");
    s.run(
        r#"
        drags = 0
        A:RegisterForDrag("LeftButton")
        A:SetScript("OnDragStart", function() drags = drags + 1 end)
        A:SetScript("OnMouseDown", function() A:EnableMouse(false) end)
    "#,
    )
    .unwrap();
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.mouse_move(260.0, 300.0);
    assert_eq!(s.eval::<i64>("return drags").unwrap(), 0);
}

/// The arm goes with the capture even when the frame is not the hover: its threshold poll runs
/// only through the capture and the index (`0x7661ca`, `0x7661cd`), and the frame is out of both,
/// for a mouse disable and a hide alike.
#[test]
fn a_pressed_frame_out_of_the_index_never_starts_its_drag() {
    for removal in ["A:EnableMouse(false)", "A:Hide()"] {
        let mut s = two_frames("Frame");
        s.run(
            r#"
            drags = 0
            A:RegisterForDrag("LeftButton")
            A:SetScript("OnDragStart", function() drags = drags + 1 end)
        "#,
        )
        .unwrap();
        s.mouse_move(299.0, 300.0);
        s.mouse_button(299.0, 300.0, "LeftButton", true);
        s.mouse_move(301.0, 300.0); // 2px: the hover left A, the arm is not yet past its threshold
        s.run(removal).unwrap();
        s.mouse_move(310.0, 300.0);
        assert_eq!(s.eval::<i64>("return drags").unwrap(), 0, "{removal}");
    }
}

/// The leave is at once: the hover clears and `OnLeave` runs inside the call (`0x764cd4`), the
/// re-pick it arms (`0x764cbb`) then enters the frame beneath on the next tick.
#[test]
fn a_hovered_frame_mouse_disabled_gets_its_leave_at_once() {
    let mut s = two_frames("Frame");
    s.run(
        r#"
        local under = CreateFrame("Frame", "Under")
        under:SetPoint("BOTTOMLEFT", 100, 200); under:SetWidth(200); under:SetHeight(200)
        under:EnableMouse(true)
        under:SetScript("OnEnter", function() table.insert(log, "Under:enter") end)
        A:SetFrameLevel(under:GetFrameLevel() + 5)
    "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_move(200.0, 300.0);
    s.run("A:EnableMouse(false) table.insert(log, 'focus:'..tostring(GetMouseFocus()))")
        .unwrap();
    assert_eq!(
        log_of(&mut s),
        "A:enter A:leave focus:nil",
        "OnLeave ran inside the call and the hover is already clear"
    );
    s.tick(0.016);
    assert_eq!(
        log_of(&mut s),
        "A:enter A:leave focus:nil Under:enter",
        "the armed re-pick enters the frame beneath with no mouse move"
    );
}

/// The disable removes only the frame it names: another frame's hover, capture and press stay.
#[test]
fn disabling_another_frame_keeps_the_hover_and_the_press() {
    let mut s = two_frames("Frame");
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.run("B:EnableMouse(false)").unwrap();
    s.mouse_button(200.0, 300.0, "LeftButton", false);
    assert_eq!(log_of(&mut s), "A:enter A:down:LeftButton A:up:LeftButton");
}

/// `EnableMouse(true)` inserts with flag -1 and arms no re-pick (`0x76af00(2, -1)`, gate
/// `0x764b82`): a still cursor over the frame enters it only at the next move.
#[test]
fn enabling_the_mouse_re_picks_nothing() {
    let mut s = two_frames("Frame");
    s.run("A:EnableMouse(false)").unwrap();
    s.mouse_move(200.0, 300.0);
    s.run("A:EnableMouse(true)").unwrap();
    s.tick(0.016);
    assert_eq!(log_of(&mut s), "", "no re-pick under a still cursor");
    s.mouse_move(201.0, 300.0);
    assert_eq!(log_of(&mut s), "A:enter");
}

/// A pressed Button stays pushed when it is mouse-disabled: no release reaches it, so nothing
/// un-presses it and nothing clicks.
#[test]
fn a_pressed_button_mouse_disabled_stays_pushed_and_never_clicks() {
    let mut s = two_frames("Button");
    s.run(
        r#"
        A:SetScript("OnClick", function() table.insert(log, "A:click") end)
    "#,
    )
    .unwrap();
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.run("A:EnableMouse(false)").unwrap();
    s.mouse_button(200.0, 300.0, "LeftButton", false);
    assert_eq!(
        s.eval::<String>("return A:GetButtonState()").unwrap(),
        "PUSHED"
    );
    assert_eq!(log_of(&mut s), "A:enter A:down:LeftButton A:leave");
}

// ── A chorded press goes to the capture (`0x7662c0`) ───────────────────────────────────────────

/// Left held on A, right pressed over B: the press handler takes the capture first (`0x76637f`),
/// so A gets both edges of the right button and B none (`0x7664a4`).
#[test]
fn a_chorded_press_fires_on_the_held_frame() {
    let mut s = two_frames("Frame");
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.mouse_move(500.0, 300.0);
    s.mouse_button(500.0, 300.0, "RightButton", true);
    s.mouse_button(500.0, 300.0, "RightButton", false);
    s.mouse_button(500.0, 300.0, "LeftButton", false);
    assert_eq!(
        log_of(&mut s),
        "A:enter A:down:LeftButton A:leave B:enter A:down:RightButton A:up:RightButton A:up:LeftButton"
    );
}

/// With nothing held the capture becomes the hovered frame (`0x7663e6`), so a lone press is
/// unchanged and the capture clears with the last button.
#[test]
fn a_lone_press_still_goes_to_the_frame_under_the_cursor() {
    let mut s = two_frames("Frame");
    s.mouse_button(500.0, 300.0, "RightButton", true);
    s.mouse_button(500.0, 300.0, "RightButton", false);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.mouse_button(200.0, 300.0, "LeftButton", false);
    assert_eq!(
        log_of(&mut s),
        "B:down:RightButton B:up:RightButton A:down:LeftButton A:up:LeftButton"
    );
}

/// A Button on the chorded press: the press art and the click need the cursor inside the capture
/// (`0x77925c`, `0x779335`), and the frame under the cursor is neither pushed nor clicked.
#[test]
fn a_chorded_button_press_pushes_and_clicks_only_inside_the_held_button() {
    let mut s = two_frames("Button");
    s.run(
        r#"
        A:RegisterForClicks("LeftButtonUp", "RightButtonUp")
        B:RegisterForClicks("LeftButtonUp", "RightButtonUp")
        A:SetScript("OnClick", function() table.insert(log, "A:click:"..arg1) end)
        B:SetScript("OnClick", function() table.insert(log, "B:click:"..arg1) end)
    "#,
    )
    .unwrap();
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.mouse_move(500.0, 300.0); // over B, outside A
    s.mouse_button(500.0, 300.0, "RightButton", true);
    assert_eq!(
        s.eval::<String>("return B:GetButtonState()").unwrap(),
        "NORMAL",
        "the frame under the cursor takes no press art"
    );
    s.mouse_button(500.0, 300.0, "RightButton", false);
    assert!(
        !log_of(&mut s).contains("click"),
        "outside the held button no click fires, and none on B: {}",
        log_of(&mut s)
    );
    // Inside the held button the release clicks it.
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "RightButton", true);
    s.mouse_button(200.0, 300.0, "RightButton", false);
    assert!(
        log_of(&mut s).ends_with("A:up:RightButton A:click:RightButton"),
        "{}",
        log_of(&mut s)
    );
    assert!(!log_of(&mut s).contains("B:click"));
}

/// The title-region test runs on the capture (`0x766397`): a chorded press over another frame's
/// title region starts no move there.
#[test]
fn a_chorded_press_over_another_title_region_starts_no_move() {
    let mut s = two_frames("Frame");
    s.run(r#"B:CreateTitleRegion():SetAllPoints(B)"#).unwrap();
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.mouse_move(500.0, 300.0);
    s.mouse_button(500.0, 300.0, "RightButton", true);
    s.mouse_move(560.0, 300.0);
    s.resolve();
    crate::assert_near!(s.eval::<f64>("return B:GetLeft()").unwrap(), 400.0);
    assert!(log_of(&mut s).ends_with("A:down:RightButton"));
}

/// A release lost to a focus change leaves its button held; the next press of that button must
/// not be aimed at the stale capture.
#[test]
fn a_press_after_a_lost_release_goes_to_the_frame_under_the_cursor() {
    let mut s = two_frames("Frame");
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true); // the release never arrives
    s.mouse_move(500.0, 300.0);
    s.mouse_button(500.0, 300.0, "LeftButton", true);
    s.mouse_button(500.0, 300.0, "LeftButton", false);
    assert_eq!(
        log_of(&mut s),
        "A:enter A:down:LeftButton A:leave B:enter B:down:LeftButton B:up:LeftButton"
    );
}

// ── The Button click legs on the capture (`0x779210`, `0x7792d0`) ──────────────────────────────

/// The release click tests the pressed button's own rect (`0x779335`), not whether it is the top
/// frame under the cursor: releasing over a frame that overlaps it still clicks.
#[test]
fn a_release_over_an_overlapping_frame_still_clicks_the_pressed_button() {
    let mut s = two_frames("Button");
    s.run(
        r#"
        A:SetScript("OnClick", function() table.insert(log, "A:click") end)
        local top = CreateFrame("Frame", "Top")
        top:SetPoint("BOTTOMLEFT", 200, 200); top:SetWidth(100); top:SetHeight(200)
        top:EnableMouse(true); top:SetFrameLevel(A:GetFrameLevel() + 5)
    "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_move(120.0, 300.0);
    s.mouse_button(120.0, 300.0, "LeftButton", true);
    s.mouse_move(250.0, 300.0); // inside A, under Top
    assert_eq!(s.hit_test_name(250.0, 300.0).as_deref(), Some("Top"));
    s.mouse_button(250.0, 300.0, "LeftButton", false);
    assert!(
        log_of(&mut s).ends_with("A:up:LeftButton A:click"),
        "{}",
        log_of(&mut s)
    );
}

/// The release slot clicks only a PUSHED button (`0x7792fc`): one a script set back to NORMAL
/// between the edges does not.
#[test]
fn a_button_no_longer_pushed_at_the_release_does_not_click() {
    let mut s = two_frames("Button");
    s.run(r#"A:SetScript("OnClick", function() table.insert(log, "A:click") end)"#)
        .unwrap();
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.run(r#"A:SetButtonState("NORMAL")"#).unwrap();
    s.mouse_button(200.0, 300.0, "LeftButton", false);
    assert!(!log_of(&mut s).contains("click"), "{}", log_of(&mut s));
}

/// The press slot pushes the button last: `OnMouseDown` and a Down `OnClick` see it unpushed
/// (`0x76bf70`, `0x779285`, then `0x7792b1`), and a state set in `OnMouseDown` is overwritten.
#[test]
fn the_press_art_comes_after_on_mouse_down_and_the_down_click() {
    let mut s = two_frames("Button");
    s.run(
        r#"
        A:RegisterForClicks("LeftButtonDown", "LeftButtonUp")
        A:SetScript("OnMouseDown", function() table.insert(log, "down:"..A:GetButtonState()) A:SetButtonState("NORMAL") end)
        A:SetScript("OnClick", function() table.insert(log, "click:"..A:GetButtonState()) end)
    "#,
    )
    .unwrap();
    s.mouse_move(200.0, 300.0);
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    assert_eq!(log_of(&mut s), "A:enter down:NORMAL click:NORMAL");
    assert_eq!(
        s.eval::<String>("return A:GetButtonState()").unwrap(),
        "PUSHED"
    );
}

// ── The release slot's order: OnMouseUp, then the click, then NORMAL (`0x7792d0`) ───────────────

/// A Button `A` that logs the state its `OnClick` and `OnDoubleClick` read.
fn click_state_probe(s: &mut UiScript, body: &str) {
    s.run(&format!(
        r#"
        A:SetScript("OnClick", function() table.insert(log, "click:"..A:GetButtonState()) {body} end)
        A:SetScript("OnDoubleClick", function() table.insert(log, "double:"..A:GetButtonState()) end)
    "#
    ))
    .unwrap();
    s.mouse_move(200.0, 300.0);
}

fn press_release(s: &mut UiScript) {
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.mouse_button(200.0, 300.0, "LeftButton", false);
}

fn state_of_a(s: &mut UiScript) -> String {
    s.eval::<String>("return A:GetButtonState()").unwrap()
}

/// The mouse-up handler fires `OnClick` (`0x7793a9`) and only then sets NORMAL (`0x7793de`), so a
/// click registered for the release reads PUSHED; one registered for the press reads NORMAL
/// (`0x779285` before `0x7792b1`).
#[test]
fn on_click_reads_pushed_on_a_release_click_and_normal_on_a_press_click() {
    let mut s = two_frames("Button");
    click_state_probe(&mut s, "");
    press_release(&mut s);
    assert_eq!(
        log_of(&mut s),
        "A:enter A:down:LeftButton A:up:LeftButton click:PUSHED"
    );
    assert_eq!(
        state_of_a(&mut s),
        "NORMAL",
        "…and the release then un-pushes it"
    );

    let mut s = two_frames("Button");
    s.run(r#"A:RegisterForClicks("LeftButtonDown")"#).unwrap();
    click_state_probe(&mut s, "");
    press_release(&mut s);
    assert!(
        log_of(&mut s).contains("click:NORMAL"),
        "{}",
        log_of(&mut s)
    );
    assert!(
        !log_of(&mut s).contains("click:PUSHED"),
        "{}",
        log_of(&mut s)
    );

    let mut s = two_frames("Button");
    s.run(r#"A:RegisterForClicks("LeftButtonDown", "LeftButtonUp")"#)
        .unwrap();
    click_state_probe(&mut s, "");
    press_release(&mut s);
    assert!(
        log_of(&mut s).ends_with("click:NORMAL A:up:LeftButton click:PUSHED"),
        "{}",
        log_of(&mut s)
    );
}

/// `OnDoubleClick` replaces the second `OnClick` (`0x77938d`) and runs before the same NORMAL
/// write, so it reads PUSHED too.
#[test]
fn on_double_click_reads_pushed() {
    let mut s = two_frames("Button");
    click_state_probe(&mut s, "");
    press_release(&mut s);
    press_release(&mut s);
    assert!(
        log_of(&mut s).ends_with("A:up:LeftButton double:PUSHED"),
        "{}",
        log_of(&mut s)
    );
    assert_eq!(state_of_a(&mut s), "NORMAL");
}

/// After the click the handler sets NORMAL only if `[+0x32c]` (the lock) is 0 and `[+0x328]` (the
/// state) is not DISABLED (`0x7793c2`..`0x7793de`): an `OnClick` that pushes the button unlocked is
/// released by the tail, one that locks it keeps what it set.
#[test]
fn the_tail_after_the_click_honours_what_the_click_set() {
    let mut s = two_frames("Button");
    click_state_probe(&mut s, r#"A:SetButtonState("PUSHED")"#);
    press_release(&mut s);
    assert_eq!(state_of_a(&mut s), "NORMAL", "unlocked: the tail un-pushes");

    let mut s = two_frames("Button");
    click_state_probe(&mut s, r#"A:SetButtonState("PUSHED", 1)"#);
    press_release(&mut s);
    assert_eq!(state_of_a(&mut s), "PUSHED", "locked: the tail is skipped");

    let mut s = two_frames("Button");
    click_state_probe(&mut s, "A:Disable()");
    press_release(&mut s);
    assert_eq!(
        state_of_a(&mut s),
        "DISABLED",
        "disabled: the tail is skipped"
    );
}

/// The state is tested after the base slot's `OnMouseUp` script has run (`0x76c099`, then
/// `0x7792fc`): an `OnMouseUp` that un-pushes the button cancels the click.
#[test]
fn an_on_mouse_up_that_unpushes_the_button_cancels_the_click() {
    let mut s = two_frames("Button");
    click_state_probe(&mut s, "");
    s.run(r#"A:SetScript("OnMouseUp", function() table.insert(log, "up:"..A:GetButtonState()) A:SetButtonState("NORMAL") end)"#)
        .unwrap();
    press_release(&mut s);
    assert_eq!(log_of(&mut s), "A:enter A:down:LeftButton up:PUSHED");
}

// ── The focus change (`0x764dc0`) ────────────────────────────────────────────────────────────

/// `two_frames` with a `WorldFrame` under them, the cursor on A.
fn under_the_world_frame(kind: &str) -> UiScript {
    let mut s = two_frames(kind);
    s.run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
        .unwrap();
    s.resolve();
    s.mouse_move(200.0, 300.0);
    s
}

fn focus(s: &UiScript) -> String {
    s.eval::<String>("return GetMouseFocus():GetName()")
        .unwrap()
}

/// The focus change with the world frame (`0x764dc0`): the hover's leave (`0x764ddf`), then the
/// press A holds gets its button-up with the press's button (`0x764e98`), and the capture goes
/// (`0x764e9b`): the real release reaches no frame, and no click fires at the zeroed point.
#[test]
fn the_focus_change_leaves_the_hover_then_releases_the_held_press() {
    let mut s = under_the_world_frame("Button");
    s.run(r#"A:SetScript("OnClick", function() table.insert(log, "A:click") end)"#)
        .unwrap();
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.focus_world_frame();
    assert_eq!(focus(&s), "WorldFrame");
    assert!(s.model_ref().mouse_capture.is_none());
    assert_eq!(
        log_of(&mut s),
        "A:enter A:down:LeftButton A:leave A:up:LeftButton"
    );
    s.mouse_button(200.0, 300.0, "LeftButton", false);
    assert_eq!(
        log_of(&mut s),
        "A:enter A:down:LeftButton A:leave A:up:LeftButton",
        "no second OnMouseUp and no click"
    );
    // With the world frame already the hover and nothing held, it does nothing.
    s.focus_world_frame();
    assert_eq!(
        log_of(&mut s),
        "A:enter A:down:LeftButton A:leave A:up:LeftButton"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The release is the stored event with x and y zeroed (`0x764e87`, `0x764e8e`): a Button over
/// the origin clicks with the press's button. After a move the stored event is the move's, whose
/// button is 0 (`0x76615f`), so `OnMouseUp` names it `UNKNOWN` (`0x76c0e5`) and nothing clicks.
#[test]
fn the_focus_changes_release_is_the_stored_event_at_the_origin() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"log = {}
        WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()
        O = CreateFrame("Button", "O")
        O:SetPoint("BOTTOMLEFT", 0, 0) O:SetWidth(50) O:SetHeight(50) O:EnableMouse(true)
        O:SetScript("OnMouseUp", function() table.insert(log, "up:"..arg1) end)
        O:SetScript("OnClick", function() table.insert(log, "click:"..arg1) end)"#,
    )
    .unwrap();
    s.resolve();
    s.mouse_move(25.0, 25.0);
    s.mouse_button(25.0, 25.0, "LeftButton", true);
    s.focus_world_frame();
    assert_eq!(log_of(&mut s), "up:LeftButton click:LeftButton");
    s.mouse_button(25.0, 25.0, "LeftButton", false);
    s.run("log = {}").unwrap();
    s.mouse_move(20.0, 20.0);
    s.mouse_button(20.0, 20.0, "LeftButton", true);
    s.mouse_move(30.0, 30.0);
    s.focus_world_frame();
    assert_eq!(log_of(&mut s), "up:UNKNOWN");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A started drag of the held frame stands: the move that started it is the stored event, whose
/// button 0 misses the base up's mask test (`0x76c050`), so the frame gets `OnMouseUp` with
/// `UNKNOWN` and no `OnDragStop`, and its gesture is left as the reference leaves `[+0x104]`.
#[test]
fn the_focus_change_leaves_a_started_drag_standing() {
    let mut s = under_the_world_frame("Frame");
    s.run(
        r#"A:RegisterForDrag("LeftButton")
        A:SetScript("OnDragStart", function() table.insert(log, "A:dragstart") end)
        A:SetScript("OnDragStop", function() table.insert(log, "A:dragstop") end)"#,
    )
    .unwrap();
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.mouse_move(260.0, 300.0);
    assert!(
        log_of(&mut s).ends_with("A:dragstart"),
        "{}",
        log_of(&mut s)
    );
    s.run("log = {}").unwrap();
    s.focus_world_frame();
    assert_eq!(log_of(&mut s), "A:leave A:up:UNKNOWN");
    assert!(s.model_ref().drag.as_ref().is_some_and(|d| d.started));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

// ── Who a press belongs to ───────────────────────────────────────────────────────────────────

/// A press while a frame holds a button goes to that frame (`0x76637f`), over the world too, and is
/// the interface's: the world frame's binding takes none of it.
#[test]
fn a_press_while_a_frame_holds_a_button_is_that_frames_over_the_world_too() {
    let mut s = under_the_world_frame("Frame");
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    s.mouse_move(700.0, 300.0);
    assert!(
        s.mouse_button(700.0, 300.0, "RightButton", true),
        "the interface's"
    );
    assert!(log_of(&mut s).ends_with("A:leave A:down:RightButton"));
}

/// A chord's second button over a frame joins the press the world frame holds: it goes to the
/// world frame (`0x76637f`), whose binding runs, and the frame under the cursor sees none of it.
#[test]
fn a_chords_second_button_over_a_frame_joins_the_world_frames_press() {
    let mut s = under_the_world_frame("Frame");
    s.run(
        r#"WorldFrame:SetScript("OnMouseDown", function() table.insert(log, "W:down:"..arg1) end)"#,
    )
    .unwrap();
    s.mouse_move(700.0, 300.0);
    s.mouse_button(700.0, 300.0, "RightButton", true);
    s.mouse_move(500.0, 300.0);
    assert!(
        !s.mouse_button(500.0, 300.0, "LeftButton", true),
        "the world frame's"
    );
    assert_eq!(
        log_of(&mut s),
        "A:enter A:leave W:down:RightButton B:enter W:down:LeftButton"
    );
}

/// In freelook the mouse is relative, and with nothing held the hook gives every press and
/// release to the world frame's slots (`0x492bc1`, `0x492bd9`, `0x492be5`): the frame under the
/// cursor sees neither, nothing is captured, and the world frame's binding runs.
#[test]
fn in_freelook_the_hook_gives_the_world_frame_every_press() {
    use crate::script::live::LiveAsk;
    use crate::script::{FreelookQuery, LiveReader, PointerTarget};
    struct Looking;
    impl LiveReader for Looking {
        fn read_live(&mut self, ask: &mut LiveAsk<'_>) -> bool {
            ask.answer(|_: &FreelookQuery| Some(true))
        }
    }
    let mut s = under_the_world_frame("Frame");
    s.run(
        r#"WorldFrame:SetScript("OnMouseDown", function() table.insert(log, "W:down:"..arg1) end)
        WorldFrame:SetScript("OnMouseUp", function() table.insert(log, "W:up:"..arg1) end)"#,
    )
    .unwrap();
    let (down, held, up) = s.reading(&mut Looking, |s| {
        let down = s.mouse_button_to(200.0, 300.0, "RightButton", true);
        let held = s.model_ref().mouse_capture.is_some();
        let up = s.mouse_button_to(200.0, 300.0, "RightButton", false);
        (down, held, up)
    });
    assert_eq!(
        (down, held, up),
        (PointerTarget::World, false, PointerTarget::World)
    );
    assert_eq!(
        log_of(&mut s),
        "A:enter W:down:RightButton W:up:RightButton"
    );
    // A frame holding a button keeps the press, freelook or not.
    s.run("log = {}").unwrap();
    s.mouse_button(200.0, 300.0, "LeftButton", true);
    let target = s.reading(&mut Looking, |s| {
        s.mouse_button_to(200.0, 300.0, "RightButton", true)
    });
    assert_eq!(target, PointerTarget::Ui);
    assert_eq!(log_of(&mut s), "A:down:LeftButton A:down:RightButton");
}
