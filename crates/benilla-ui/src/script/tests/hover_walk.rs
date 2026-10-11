//! The cursor-move hover walk (`0x7660d0`): what its `OnEnter` and `OnLeave` see, and its early
//! return while a frame is moving or sizing.

use super::common::script;
use crate::script::UiScript;

/// The joined `LOG` entries, emptied.
fn take_log(s: &UiScript) -> String {
    s.eval::<String>("local out = table.concat(LOG, ' ') LOG = {} return out")
        .unwrap()
}

/// `OnEnter` and `OnLeave` run through the plain runner (`0x702690` → `0x704d50`), whose call
/// (`0x704e75` → `0x6f41a0`) passes no arguments and sets no global but `this`: the arg globals
/// keep what the last event left, on the walk's enter and leave and on a hide's leave.
#[test]
fn on_enter_and_on_leave_leave_the_arg_globals_untouched() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        LOG = {}
        local function rec(tag)
            return function() table.insert(LOG, tag .. "=" .. tostring(arg1) .. "," .. tostring(arg2)) end
        end
        A = CreateFrame("Frame", "ArgA")
        A:SetPoint("BOTTOMLEFT", 100, 100); A:SetWidth(100); A:SetHeight(100)
        A:EnableMouse(true)
        A:SetScript("OnEnter", rec("enterA"))
        A:SetScript("OnLeave", rec("leaveA"))
        B = CreateFrame("Frame", "ArgB")
        B:SetPoint("BOTTOMLEFT", 300, 100); B:SetWidth(100); B:SetHeight(100)
        B:EnableMouse(true)
        B:SetScript("OnEnter", rec("enterB"))
        B:SetScript("OnLeave", rec("leaveB"))
        arg1, arg2 = "sentinel", "second"
    "#,
    )
    .unwrap();
    s.resolve();

    s.mouse_move(150.0, 150.0);
    assert_eq!(take_log(&s), "enterA=sentinel,second");
    s.mouse_move(350.0, 150.0);
    assert_eq!(
        take_log(&s),
        "leaveA=sentinel,second enterB=sentinel,second",
        "a boundary crossing sets no arg global on either side"
    );
    // The removal tail's leave (`0x764cce`) runs through the same base leave and runner.
    s.run("B:Hide()").unwrap();
    assert_eq!(take_log(&s), "leaveB=sentinel,second");
    assert_eq!(
        s.eval::<String>("return arg1 .. ',' .. arg2").unwrap(),
        "sentinel,second"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Three mouse-enabled frames on a 1024×768 screen: `Here` under the cursor's start, `There` to
/// its right, and `Far`, movable and resizable, well away from both. Each logs its enter and
/// leave.
fn three_frames() -> UiScript {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        LOG = {}
        local function frame(name, x, y)
            local f = CreateFrame("Frame", name)
            f:SetPoint("BOTTOMLEFT", x, y); f:SetWidth(100); f:SetHeight(100)
            f:EnableMouse(true)
            f:SetScript("OnEnter", function() table.insert(LOG, "enter" .. name) end)
            f:SetScript("OnLeave", function() table.insert(LOG, "leave" .. name) end)
            return f
        end
        Here = frame("Here", 100, 100)
        There = frame("There", 300, 100)
        Far = frame("Far", 700, 500)
        Far:SetMovable(true)
        Far:SetResizable(true)
    "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_move(150.0, 150.0);
    assert_eq!(take_log(&s), "enterHere");
    s
}

fn focus(s: &UiScript) -> String {
    s.eval::<String>("local f = GetMouseFocus() return f and f:GetName() or 'nil'")
        .unwrap()
}

/// While `[root+0xcfc]` holds a move or size target (`0x7652fc`), the walk runs the move/size
/// tick (`0x766193` → `0x7655b0`) and returns (`0x7661a0`) before any probe: the hover stays and
/// no enter or leave fires until `StopMovingOrSizing` clears the slot (`0x765640`).
fn assert_walk_waits_for_the_drop(start: &str) {
    let mut s = three_frames();
    s.run(start).unwrap();
    s.mouse_move(350.0, 150.0);
    s.resolve();
    assert_eq!(take_log(&s), "", "{start}: no enter or leave mid-drag");
    assert_eq!(focus(&s), "Here", "{start}: the hover stays put mid-drag");
    s.mouse_move(352.0, 150.0);
    assert_eq!(take_log(&s), "", "{start}: still nothing on the next move");

    s.run("Far:StopMovingOrSizing()").unwrap();
    assert_eq!(take_log(&s), "", "{start}: the stop alone walks nothing");
    s.mouse_move(355.0, 150.0);
    assert_eq!(
        take_log(&s),
        "leaveHere enterThere",
        "{start}: the next move after the drop walks again"
    );
    assert_eq!(focus(&s), "There");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn a_frame_moving_skips_the_hover_walk_until_the_drop() {
    assert_walk_waits_for_the_drop("Far:StartMoving()");
    // The move itself still follows the cursor.
    let mut s = three_frames();
    s.run("Far:StartMoving()").unwrap();
    s.mouse_move(350.0, 150.0);
    s.resolve();
    assert_eq!(s.eval::<f64>("return Far:GetLeft()").unwrap(), 900.0);
}

#[test]
fn a_frame_sizing_skips_the_hover_walk_until_the_drop() {
    assert_walk_waits_for_the_drop("Far:StartSizing(\"RIGHT\")");
    // The size itself still follows the cursor.
    let mut s = three_frames();
    s.run("Far:StartSizing(\"RIGHT\")").unwrap();
    s.mouse_move(350.0, 150.0);
    s.resolve();
    assert_eq!(s.eval::<f64>("return Far:GetWidth()").unwrap(), 300.0);
}

/// The pump tail's re-pick (`0x7657be`) is the same walk on the saved event, so it waits for the
/// drop too: a frame shown under the cursor mid-drag gets no `OnEnter`.
#[test]
fn the_tick_re_pick_waits_for_the_drop_too() {
    let mut s = three_frames();
    s.run("Here:Hide() Far:StartMoving()").unwrap();
    assert_eq!(take_log(&s), "leaveHere");
    s.run("Here:Show()").unwrap();
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "",
        "the re-pick finds the slot set and returns"
    );
    assert_eq!(focus(&s), "nil");
    s.run("Far:StopMovingOrSizing()").unwrap();
    s.mouse_move(151.0, 150.0);
    assert_eq!(take_log(&s), "enterHere");
}

/// The drag-start check lives in the base probe (`0x76b320`), which the early return skips, so a
/// frame that starts moving on its press never reaches `OnDragStart`, however far it is dragged.
#[test]
fn a_move_started_on_the_press_never_reaches_the_drag_start() {
    let mut s = three_frames();
    s.run(
        r#"
        starts = 0
        Here:SetMovable(true)
        Here:RegisterForDrag("LeftButton")
        Here:SetScript("OnMouseDown", function() this:StartMoving() end)
        Here:SetScript("OnMouseUp", function() this:StopMovingOrSizing() end)
        Here:SetScript("OnDragStart", function() starts = starts + 1 end)
    "#,
    )
    .unwrap();
    s.mouse_button(150.0, 150.0, "LeftButton", true);
    s.mouse_move(250.0, 150.0);
    s.mouse_move(350.0, 150.0);
    s.mouse_button(350.0, 150.0, "LeftButton", false);
    assert_eq!(s.eval::<i64>("return starts").unwrap(), 0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Hiding the moving frame cancels the move: the hide's de-registration (`0x76ad9d` →
/// `0x764920`) clears the slot when it holds the frame (`0x76492e`–`0x76493a` → `0x765640`), so
/// the next move walks again.
#[test]
fn hiding_the_moving_frame_ends_the_move_and_the_walk_resumes() {
    let mut s = three_frames();
    s.run("Far:StartMoving()").unwrap();
    s.mouse_move(250.0, 150.0);
    assert_eq!(take_log(&s), "");
    s.run("Far:Hide()").unwrap();
    s.mouse_move(350.0, 150.0);
    assert_eq!(take_log(&s), "leaveHere enterThere");
    s.run("Far:Show()").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return Far:GetLeft()").unwrap(),
        800.0,
        "the move ended with the hide: the later cursor delta did not reach it"
    );
}

/// The pointer leaving the window ends the hover with the hovered frame's `OnLeave`, and the next
/// real move walks again.
#[test]
fn the_pointer_leaving_the_window_leaves_the_hovered_frame() {
    let mut s = three_frames();
    s.pointer_left_hover();
    s.pointer_left_window();
    assert_eq!(take_log(&s), "leaveHere");
    assert_eq!(focus(&s), "nil");
    s.mouse_move(150.0, 150.0);
    assert_eq!(take_log(&s), "enterHere");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A move in flight follows real cursor events only (`0x7655b0` adds each event's delta from the
/// last sample, `[root+0xd08]`/`[root+0xd0c]`): the pointer leaving moves nothing and stamps no
/// layout, and the first real move back applies its delta from the last real sample.
#[test]
fn a_frame_moving_when_the_pointer_leaves_keeps_its_offsets() {
    let mut s = three_frames();
    s.run("Far:StartMoving()").unwrap();
    s.mouse_move(250.0, 150.0);
    s.resolve();
    let point = |s: &UiScript| {
        s.eval::<(f64, f64)>("local _, _, _, x, y = Far:GetPoint() return x, y")
            .unwrap()
    };
    assert_eq!(point(&s), (800.0, 500.0));
    let saved = s.user_placed_layouts();

    s.pointer_left_hover();
    s.pointer_left_window();
    s.resolve();
    assert_eq!(point(&s), (800.0, 500.0), "the leave moves nothing");
    assert_eq!(s.eval::<f64>("return Far:GetLeft()").unwrap(), 800.0);
    assert_eq!(
        s.user_placed_layouts(),
        saved,
        "the layout cache keeps the last real position"
    );
    assert_eq!(take_log(&s), "leaveHere", "the hover ends mid-move too");

    // The move is `StartMoving`'s, which only `StopMovingOrSizing` ends: back in the window, the
    // next real event moves it by its delta from the last real one.
    s.mouse_move(260.0, 150.0);
    s.resolve();
    assert_eq!(point(&s), (810.0, 500.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The canonical drag idiom: the pointer leaving mid-drag ends the gesture with `OnDragStop`,
/// whose `StopMovingOrSizing` keeps the frame where the last real move put it.
#[test]
fn a_drag_cut_short_by_the_pointer_leaving_keeps_the_last_real_position() {
    let mut s = three_frames();
    s.run(
        r#"
        stops = 0
        Here:SetMovable(true)
        Here:RegisterForDrag("LeftButton")
        Here:SetScript("OnDragStart", function() this:StartMoving() end)
        Here:SetScript("OnDragStop", function() stops = stops + 1 this:StopMovingOrSizing() end)
    "#,
    )
    .unwrap();
    s.mouse_button(150.0, 150.0, "LeftButton", true);
    s.mouse_move(170.0, 150.0); // past the threshold: OnDragStart, StartMoving
    s.mouse_move(200.0, 160.0);
    s.resolve();
    assert_eq!(s.eval::<f64>("return Here:GetLeft()").unwrap(), 130.0);
    s.pointer_left_hover();
    s.pointer_left_window();
    s.resolve();
    assert_eq!(s.eval::<i64>("return stops").unwrap(), 1);
    assert_eq!(s.eval::<f64>("return Here:GetLeft()").unwrap(), 130.0);
    assert_eq!(s.eval::<f64>("return Here:GetBottom()").unwrap(), 110.0);
    let saved = s.user_placed_layouts();
    let here = saved
        .iter()
        .find(|l| l.name == "Here")
        .expect("Here is saved");
    assert_eq!((here.points[0].x, here.points[0].y), (130.0, 110.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
