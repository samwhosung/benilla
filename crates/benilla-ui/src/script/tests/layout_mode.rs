//! `SetLayoutMode` and the modifier drag it arms: while the UI root's mode (`+0xcf8`) is non-zero,
//! a press with Control or Alt held (`0x766301`–`0x76634e`) moves or sizes the toplevel window
//! under the cursor (`0x765490`) and reaches nothing else.

use super::common::script;
use crate::script::UiScript;

/// A toplevel, movable and resizable `Win` at (100, 100) 200×100 with a child `Kid` over its
/// middle, on 800×600, every press and release logged.
fn window() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        log = {}
        local function note(f)
            f:SetScript("OnMouseDown", function() table.insert(log, f:GetName()..":down") end)
            f:SetScript("OnMouseUp", function() table.insert(log, f:GetName()..":up") end)
        end
        Win = CreateFrame("Frame", "Win")
        Win:SetToplevel(true); Win:SetMovable(true); Win:SetResizable(true)
        Win:SetPoint("BOTTOMLEFT", 100, 100); Win:SetWidth(200); Win:SetHeight(100)
        Win:EnableMouse(true)
        note(Win)
        Kid = CreateFrame("Button", "Kid", Win)
        Kid:SetPoint("CENTER", Win, "CENTER", 0, 0); Kid:SetWidth(40); Kid:SetHeight(20)
        Kid:EnableMouse(true)
        note(Kid)
        "#,
    )
    .unwrap();
    s.resolve();
    s
}

/// `(left, bottom, width, height)` of a frame, after a resolve.
fn rect(s: &mut UiScript, name: &str) -> (f64, f64, f64, f64) {
    s.resolve();
    s.eval::<(f64, f64, f64, f64)>(&format!(
        "local f = getglobal('{name}') return f:GetLeft(), f:GetBottom(), f:GetWidth(), f:GetHeight()"
    ))
    .unwrap()
}

fn log_of(s: &UiScript) -> String {
    s.eval::<String>("return table.concat(log, ' ')").unwrap()
}

/// A press and a move by `(dx, dy)`, then the release, with `(ctrl, alt)` held throughout.
fn modifier_drag(
    s: &mut UiScript,
    (x, y): (f32, f32),
    (dx, dy): (f32, f32),
    (ctrl, alt): (bool, bool),
) {
    s.set_modifiers(false, ctrl, alt);
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_move(x + dx, y + dy);
    s.mouse_button(x + dx, y + dy, "LeftButton", false);
    s.set_modifiers(false, false, false);
}

/// Alt over the child moves its toplevel window (`0x766fe0` lifts the hit), marks it user-placed
/// (`0x7652e5`), and the press reaches no script: the branch returns before the capture and the
/// press slot. The release ends the move (`0x766420`, mode 1) and reaches no script either.
#[test]
fn an_alt_press_in_layout_mode_moves_the_toplevel_window() {
    let mut s = window();
    s.run("SetLayoutMode(1)").unwrap();
    assert!(!s.eval::<bool>("return Win:IsUserPlaced()").unwrap());
    modifier_drag(&mut s, (200.0, 150.0), (60.0, 20.0), (false, true));
    crate::assert_near!(rect(&mut s, "Win"), (160.0, 120.0, 200.0, 100.0));
    assert!(s.eval::<bool>("return Win:IsUserPlaced()").unwrap());
    s.mouse_move(400.0, 400.0);
    crate::assert_near!(
        rect(&mut s, "Win"),
        (160.0, 120.0, 200.0, 100.0),
        "the release ended it"
    );
    assert_eq!(
        log_of(&s),
        "",
        "neither the press nor the release reached a script"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Control sizes from the cell of a 3×3 grid the press falls in, the rect cut at a quarter and
/// three quarters per axis (`0x765384`–`0x765464`), in layout units; the middle cell moves. A
/// script-exact quarter is no layout cut: 147.5 here lands a hair short of it, in the left column.
#[test]
fn a_control_press_sizes_from_the_grid_cell_and_the_middle_moves() {
    let mut s = window();
    s.run("SetLayoutMode(1)").unwrap();
    // Each rect is the law's at 4:3, the pins and sizes moved by the cursor's layout deltas: 100
    // reads 99.99999850988391, 110 reads 109.9999983608723, and so on.
    let (b100, h110) = (99.999_998_509_883_91, 109.999_998_360_872_3);
    // Top-right cell: right of 250, above 175.
    modifier_drag(&mut s, (290.0, 195.0), (20.0, 10.0), (true, false));
    assert_eq!(
        rect(&mut s, "Win"),
        (b100, b100, 219.999_996_721_744_6, h110),
        "TOPRIGHT"
    );
    // Now 220×110 from (100, 100): the left column is x < 155, the middle row 127.5..182.5.
    modifier_drag(&mut s, (154.0, 150.0), (-10.0, 0.0), (true, false));
    assert_eq!(
        rect(&mut s, "Win"),
        (90.000_008_195_638_53, b100, 229.999_996_572_732_98, h110),
        "LEFT"
    );
    // Now 230×110 from (90, 100): x 147.5 is a quarter in by script numbers, but in layout units
    // the press is 5.6e-9 short of the cut, so it still sizes from the left.
    modifier_drag(&mut s, (147.5, 150.0), (-10.0, 0.0), (true, false));
    assert_eq!(
        rect(&mut s, "Win"),
        (80.000_008_344_650_14, b100, 239.999_996_423_721_37, h110),
        "LEFT at 147.5"
    );
    // Now 240×110 from (80, 100): x 150 is the middle column.
    modifier_drag(&mut s, (150.0, 150.0), (10.0, 10.0), (true, false));
    assert_eq!(
        rect(&mut s, "Win"),
        (90.000_008_195_638_53, h110, 239.999_996_423_721_37, h110),
        "the middle cell moves"
    );
    assert_eq!(log_of(&s), "");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The window must be movable for Alt and resizable for Control (`0x765553`–`0x765573`); a press
/// that starts nothing is consumed all the same (`0x766353`).
#[test]
fn a_window_without_the_flag_does_not_move_and_still_eats_the_press() {
    let mut s = window();
    s.run("SetLayoutMode(1) Win:SetResizable(false)").unwrap();
    modifier_drag(&mut s, (290.0, 195.0), (20.0, 10.0), (true, false));
    crate::assert_near!(
        rect(&mut s, "Win"),
        (100.0, 100.0, 200.0, 100.0),
        "Control needs resizable"
    );
    s.run("Win:SetResizable(true) Win:SetMovable(false)")
        .unwrap();
    modifier_drag(&mut s, (200.0, 150.0), (60.0, 20.0), (false, true));
    crate::assert_near!(
        rect(&mut s, "Win"),
        (100.0, 100.0, 200.0, 100.0),
        "Alt needs movable"
    );
    assert!(!s.eval::<bool>("return Win:IsUserPlaced()").unwrap());
    assert_eq!(log_of(&s), "", "both presses were consumed");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// With the mode 0, or without Control or Alt, a press takes the ordinary path. The mode starts at
/// 0 (`0x7642a5`), and `SetLayoutMode` truncates a number (`0x40a2b0`) and stores 1 for anything
/// else (`0x488573`).
#[test]
fn mode_zero_or_no_modifier_passes_the_press_through() {
    let mut s = window();
    modifier_drag(&mut s, (200.0, 150.0), (60.0, 20.0), (false, true));
    assert_eq!(log_of(&s), "Kid:down Kid:up", "mode 0 at start");
    s.run("log = {} SetLayoutMode(0.9)").unwrap();
    modifier_drag(&mut s, (200.0, 150.0), (60.0, 20.0), (false, true));
    assert_eq!(log_of(&s), "Kid:down Kid:up", "0.9 truncates to 0");
    s.run("log = {} SetLayoutMode('x')").unwrap();
    modifier_drag(&mut s, (200.0, 150.0), (60.0, 20.0), (false, false));
    assert_eq!(log_of(&s), "Kid:down Kid:up", "no modifier held");
    crate::assert_near!(rect(&mut s, "Win"), (100.0, 100.0, 200.0, 100.0));
    modifier_drag(&mut s, (200.0, 150.0), (60.0, 20.0), (false, true));
    crate::assert_near!(
        rect(&mut s, "Win"),
        (160.0, 120.0, 200.0, 100.0),
        "a non-number stored 1"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `0x765490`'s own walk: strata FULLSCREEN_DIALOG down to WORLD, never TOOLTIP (`0x7654a3` starts
/// below `root+0xcf4`), probing with `0x76b020`, which reads no mouse bit. A hit with no toplevel
/// self-or-ancestor gives up its whole stratum and the walk goes on below it (`0x76554a` →
/// `0x765532`).
#[test]
fn the_walk_skips_tooltip_and_a_stratum_whose_hit_has_no_toplevel() {
    let mut s = window();
    s.run(
        r#"
        Win:EnableMouse(false); Kid:EnableMouse(false)
        Cover = CreateFrame("Frame", "Cover")
        Cover:SetFrameStrata("HIGH")
        Cover:SetPoint("BOTTOMLEFT", 0, 0); Cover:SetWidth(800); Cover:SetHeight(600)
        Top = CreateFrame("Frame", "Top", Cover)
        Top:SetToplevel(true); Top:SetMovable(true)
        Top:SetPoint("BOTTOMLEFT", 500, 500); Top:SetWidth(50); Top:SetHeight(50)
        Cover:SetFrameLevel(5)
        Under = CreateFrame("Frame", "Under")
        Under:SetFrameStrata("HIGH"); Under:SetToplevel(true); Under:SetMovable(true)
        Under:SetPoint("BOTTOMLEFT", 150, 120); Under:SetWidth(100); Under:SetHeight(60)
        Tip = CreateFrame("Frame", "Tip")
        Tip:SetFrameStrata("TOOLTIP"); Tip:SetToplevel(true); Tip:SetMovable(true)
        Tip:SetPoint("BOTTOMLEFT", 150, 120); Tip:SetWidth(100); Tip:SetHeight(60)
        SetLayoutMode(1)
        "#,
    )
    .unwrap();
    s.resolve();
    modifier_drag(&mut s, (200.0, 150.0), (60.0, 20.0), (false, true));
    crate::assert_near!(rect(&mut s, "Win"), (160.0, 120.0, 200.0, 100.0));
    crate::assert_near!(rect(&mut s, "Tip"), (150.0, 120.0, 100.0, 60.0));
    crate::assert_near!(
        rect(&mut s, "Under"),
        (150.0, 120.0, 100.0, 60.0),
        "below Cover in HIGH"
    );
    crate::assert_near!(rect(&mut s, "Cover"), (0.0, 0.0, 800.0, 600.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A release while the modifier drag holds the slot ends it and nothing else: no `OnMouseUp` on
/// the capture a chorded press left (`0x76646d` → `0x7664e0`).
#[test]
fn a_release_that_ends_a_layout_drag_reaches_no_script() {
    let mut s = window();
    s.run("SetLayoutMode(1)").unwrap();
    s.mouse_move(120.0, 110.0);
    s.mouse_button(120.0, 110.0, "RightButton", true); // capture Win
    s.set_modifiers(false, false, true);
    s.mouse_button(120.0, 110.0, "LeftButton", true); // the layout drag
    s.set_modifiers(false, false, false);
    s.mouse_button(120.0, 110.0, "RightButton", false);
    assert_eq!(
        log_of(&s),
        "Win:down",
        "the right release reached no script"
    );
    s.mouse_move(220.0, 110.0);
    crate::assert_near!(rect(&mut s, "Win").0, 100.0, "and ended the drag");
    // The capture is still Win: the left release, the drag over, goes to it (`0x766498`–
    // `0x7664a4`) and, with no button held, clears it (`0x7664bb`).
    s.run("Win:SetScript('OnMouseUp', function() table.insert(log, 'Win:up:'..arg1) end)")
        .unwrap();
    s.mouse_button(220.0, 110.0, "LeftButton", false);
    assert_eq!(log_of(&s), "Win:down Win:up:LeftButton");
    s.mouse_move(200.0, 150.0);
    s.mouse_button(200.0, 150.0, "LeftButton", true);
    s.mouse_button(200.0, 150.0, "LeftButton", false);
    assert_eq!(
        log_of(&s),
        "Win:down Win:up:LeftButton Kid:down Kid:up",
        "no capture or held button is left behind"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A layout press is consumed even where it hits nothing: the handler returns before any frame,
/// the world's included, sees it (`0x766353`–`0x76635b`).
#[test]
fn a_layout_press_over_nothing_is_consumed() {
    let mut s = window();
    s.set_modifiers(false, false, true);
    assert!(!s.mouse_button(700.0, 500.0, "RightButton", true), "mode 0");
    s.mouse_button(700.0, 500.0, "RightButton", false);
    s.run("SetLayoutMode(1)").unwrap();
    assert!(s.mouse_button(700.0, 500.0, "RightButton", true));
}

/// `SetLayoutMode` keeps the low dword of `_ftol` (`0x40a2b0`, `fistp` to a qword): what the x87
/// cannot hold (an infinity, NaN, 2^63 and up) stores its integer-indefinite pattern, whose low
/// dword is 0, so those turn the mode off; 2^32 + 1 keeps 1.
#[test]
fn set_layout_mode_keeps_the_low_dword_of_ftol() {
    let mut s = window();
    let armed = |s: &mut UiScript, arg: &str| {
        s.run(&format!("log = {{}} SetLayoutMode({arg})")).unwrap();
        modifier_drag(s, (200.0, 150.0), (0.0, 0.0), (false, true));
        log_of(s).is_empty()
    };
    for off in ["1/0", "-1/0", "0/0", "2^63", "2^64", "2^32", "-2^63"] {
        assert!(!armed(&mut s, off), "SetLayoutMode({off}) is off");
    }
    for on in ["2^32 + 1", "-2^31", "-1.5"] {
        assert!(armed(&mut s, on), "SetLayoutMode({on}) is on");
    }
}
