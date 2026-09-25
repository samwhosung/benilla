//! The `toplevel` flag and the raise: `SetToplevel`/`IsToplevel`, `Raise`/`Lower`, and the raise
//! on show, press and drag, gated on occlusion. Tests assert `GetFrameLevel()` and the paint order.

use super::common::script;
use crate::script::{QuadContent, UiScript};

/// Texture paths in paint order: the list `extract` returns is the draw order.
fn painted(s: &mut UiScript) -> Vec<String> {
    s.resolve();
    s.extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn level(s: &mut UiScript, frame: &str) -> i64 {
    s.eval::<i64>(&format!("return {frame}:GetFrameLevel()"))
        .unwrap()
}

/// Two overlapping MEDIUM frames: `Board`, shown at level 5, and `Dialog`, hidden at level 0. The
/// level outranks the link stamp in the draw key, so only a real level bump puts `Dialog` on top.
fn board_and_dialog() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Board = CreateFrame("Frame", "Board")
        Board:SetPoint("BOTTOMLEFT", 100, 100)
        Board:SetWidth(300); Board:SetHeight(300)
        Board:SetFrameLevel(5)
        Board:CreateTexture(nil, "ARTWORK"):SetTexture("Board.blp")

        Dialog = CreateFrame("Frame", "Dialog")
        Dialog:SetPoint("BOTTOMLEFT", 200, 200)   -- overlaps Board's top-right quadrant
        Dialog:SetWidth(300); Dialog:SetHeight(300)
        Dialog:CreateTexture(nil, "ARTWORK"):SetTexture("Dialog.blp")
        Dialog:Hide()
        "#,
    )
    .unwrap();
    s.resolve();
    s
}

// ── The flag ──

/// `SetToplevel`'s optional argument defaults to true (`0x775440`), unlike `SetMovable`'s.
#[test]
fn the_flag_round_trips_and_defaults_off() {
    let s = script();
    s.run(r#"F = CreateFrame("Frame", "F")"#).unwrap();
    assert!(
        !s.eval::<bool>("return F:IsToplevel()").unwrap(),
        "no frame is born toplevel"
    );
    s.run("F:SetToplevel(true)").unwrap();
    assert!(s.eval::<bool>("return F:IsToplevel()").unwrap());
    s.run("F:SetToplevel(false)").unwrap();
    assert!(!s.eval::<bool>("return F:IsToplevel()").unwrap());
    s.run("F:SetToplevel()").unwrap();
    assert!(
        s.eval::<bool>("return F:IsToplevel()").unwrap(),
        "SetToplevel() with no argument sets the bit"
    );
    // Lua truthiness: addons write `SetToplevel(1)`.
    s.run("F:SetToplevel(nil); F:SetToplevel(1)").unwrap();
    assert!(s.eval::<bool>("return F:IsToplevel()").unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The XML `toplevel` and `enableKeyboard` attributes set the flags their methods read; in the
/// reference the keyboard flag is separate from any handler (`0x76af00`).
#[test]
fn the_xml_toplevel_and_enable_keyboard_attributes_both_reach_their_methods() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
             <Frame name="XmlTop" toplevel="true" enableKeyboard="true">
               <Size><AbsDimension x="100" y="50"/></Size>
               <Anchors>
                 <Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="10" y="10"/></Offset></Anchor>
               </Anchors>
             </Frame>
           </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(
        !report.warnings.iter().any(|w| w.contains("SetToplevel")),
        "no gap warning any more: {:?}",
        report.warnings
    );
    assert!(
        !report.warnings.iter().any(|w| w.contains("EnableKeyboard")),
        "enableKeyboard is built now, not warned: {:?}",
        report.warnings
    );
    assert!(s.eval::<bool>("return XmlTop:IsToplevel()").unwrap());
    assert!(
        s.eval::<bool>("return XmlTop:IsKeyboardEnabled()").unwrap(),
        "the XML attribute must reach the flag"
    );
    // Off by default: `0x76af00` runs from the attribute or a call, never a constructor.
    s.run("Plain = CreateFrame('Frame', 'PlainKbd')").unwrap();
    assert!(!s.eval::<bool>("return Plain:IsKeyboardEnabled()").unwrap());
    s.run("Plain:EnableKeyboard(true)").unwrap();
    assert!(s.eval::<bool>("return Plain:IsKeyboardEnabled()").unwrap());
    s.run("Plain:EnableKeyboard(false)").unwrap();
    assert!(!s.eval::<bool>("return Plain:IsKeyboardEnabled()").unwrap());
}

// ── The raise on Show ──

/// The control: a frame that is not toplevel keeps its level when shown, and paints under `Board`.
#[test]
fn a_non_toplevel_frame_does_not_move_when_shown() {
    let mut s = board_and_dialog();
    s.run("Dialog:Show()").unwrap();
    assert_eq!(level(&mut s, "Dialog"), 0, "no raise, no level change");
    assert_eq!(level(&mut s, "Board"), 5, "and no compaction either");
    assert_eq!(
        painted(&mut s),
        vec!["Dialog.blp".to_string(), "Board.blp".to_string()],
        "the dialog opens BEHIND the window it overlaps"
    );
}

/// A toplevel frame raises on show (`0x76ae10` at `0x76aee0`): compaction (`0x764eb0`) renumbers
/// the occupied levels `{0, 5}` to `[0, 2)`, then the raise writes `level := bucket->count`, 2.
#[test]
fn a_raise_is_top_occupied_level_plus_one_after_compaction() {
    let mut s = board_and_dialog();
    s.run("Dialog:SetToplevel(true)").unwrap();
    s.run("Dialog:Show()").unwrap();

    assert_eq!(
        level(&mut s, "Board"),
        1,
        "compaction renumbered the stratum's occupied levels into [0, count)"
    );
    assert_eq!(
        level(&mut s, "Dialog"),
        2,
        "the raise is bucket->count, read AFTER the compaction"
    );
    assert_eq!(
        painted(&mut s),
        vec!["Board.blp".to_string(), "Dialog.blp".to_string()],
        "the dialog is now in front"
    );
}

/// The trigger is the effective-visibility transition, not the `Show` call. `Holder` is hidden
/// before `Dialog:Show()` so that call is no transition; otherwise `Dialog` raises there and has
/// nothing left to raise over.
#[test]
fn the_trigger_is_the_effective_visibility_transition_not_the_show_call() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Holder = CreateFrame("Frame", "Holder")
        Holder:SetPoint("BOTTOMLEFT", 200, 200)
        Holder:SetWidth(300); Holder:SetHeight(300)
        Holder:Hide()
        Dialog:SetParent(Holder)
        Dialog:SetToplevel(true)
        Dialog:Show()          -- own bit set, parent hidden: NOT effective-visible, no transition
        "#,
    )
    .unwrap();
    assert_eq!(
        level(&mut s, "Dialog"),
        1,
        "a Show that moves no effective visibility raises nothing — the 1 is not a raise, it is \
         SetParent's own level := Holder(0)+1 (its re-level law)"
    );
    assert_eq!(level(&mut s, "Board"), 5, "and compacts nothing");

    s.run("Holder:Show()").unwrap();
    // Levels at the raise are Holder 0, Dialog 1, Board 5; compaction makes Board 2, count 3, and
    // the raise writes Dialog := 3.
    assert_eq!(
        level(&mut s, "Dialog"),
        3,
        "Dialog became effective-visible through its parent's Show and raised itself"
    );
    assert_eq!(level(&mut s, "Board"), 2, "the compaction ran with it");
}

// ── The gate ──

/// The occlusion gate runs before the compaction in `0x7650f0`, so a declined raise moves no level.
#[test]
fn a_raise_on_a_frame_that_overlaps_nothing_is_a_total_no_op() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Dialog:ClearAllPoints()
        Dialog:SetPoint("BOTTOMLEFT", 600, 450)   -- clear of Board's (100,100)-(400,400)
        Dialog:SetWidth(100); Dialog:SetHeight(100)
        Dialog:SetToplevel(true)
        Dialog:Show()
        "#,
    )
    .unwrap();
    assert_eq!(level(&mut s, "Dialog"), 0, "nothing to raise over");
    assert_eq!(
        level(&mut s, "Board"),
        5,
        "and the gate declined before compaction"
    );
}

/// The occlusion scan starts at the frame's own level (`for lvl = T->+0xc4; lvl < bucket->count`).
#[test]
fn the_scan_ignores_frames_below_the_raised_frames_own_level() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Board:SetFrameLevel(3)
        Dialog:SetFrameLevel(5)
        Dialog:SetToplevel(true)
        Dialog:Show()
        "#,
    )
    .unwrap();
    assert_eq!(
        level(&mut s, "Dialog"),
        5,
        "the overlap is below it; no raise"
    );
    assert_eq!(level(&mut s, "Board"), 3);
}

/// The scan excludes the raised frame's own subtree (`0x767010`).
#[test]
fn the_scan_excludes_the_raised_frames_own_subtree() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Board:Hide()                              -- leave the dialog alone on screen
        Dialog:SetToplevel(true)
        Inner = CreateFrame("Frame", "Inner", Dialog)
        Inner:SetAllPoints()                      -- exactly covers its parent
        Dialog:Show()
        "#,
    )
    .unwrap();
    assert_eq!(
        level(&mut s, "Dialog"),
        0,
        "its own child is not an occlusion"
    );
}

/// `0x7650f0` never writes the stratum (`+0xc0`), so a LOW toplevel frame raises within LOW only.
#[test]
fn a_raise_can_never_lift_a_frame_out_of_its_stratum() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Med = CreateFrame("Frame", "Med")
        Med:SetPoint("BOTTOMLEFT", 100, 100)
        Med:SetWidth(300); Med:SetHeight(300)
        Med:CreateTexture(nil, "ARTWORK"):SetTexture("Med.blp")

        LowOther = CreateFrame("Frame", "LowOther")
        LowOther:SetFrameStrata("LOW")
        LowOther:SetPoint("BOTTOMLEFT", 100, 100)
        LowOther:SetWidth(300); LowOther:SetHeight(300)
        LowOther:SetFrameLevel(4)
        LowOther:CreateTexture(nil, "ARTWORK"):SetTexture("LowOther.blp")

        LowTop = CreateFrame("Frame", "LowTop")
        LowTop:SetFrameStrata("LOW")
        LowTop:SetPoint("BOTTOMLEFT", 150, 150)
        LowTop:SetWidth(300); LowTop:SetHeight(300)
        LowTop:SetToplevel(true)
        LowTop:CreateTexture(nil, "ARTWORK"):SetTexture("LowTop.blp")
        LowTop:Hide()
        "#,
    )
    .unwrap();
    s.resolve();
    s.run("LowTop:Show()").unwrap();

    assert_eq!(
        s.eval::<String>("return LowTop:GetFrameStrata()").unwrap(),
        "LOW",
        "the raise never writes the stratum"
    );
    assert!(
        level(&mut s, "LowTop") > level(&mut s, "LowOther"),
        "it did raise, within LOW"
    );
    assert_eq!(
        painted(&mut s),
        vec![
            "LowOther.blp".to_string(),
            "LowTop.blp".to_string(),
            "Med.blp".to_string(),
        ],
        "still under every MEDIUM frame — a stratum is not something a raise can cross"
    );
}

// ── Propagation and compaction ──

/// The raise's `propagate = 1` shifts same-strata children by its delta (`0x76a4f0` at
/// `0x76a58a`) and skips the others (`0x76a582`). Levels `{0, 1, 2, 5}` compact to `{0, 1, 2, 3}`,
/// so Dialog goes to 4 and Kid and Grandkid move by the same +4.
#[test]
fn the_raised_subtree_shifts_by_one_delta_and_keeps_its_internal_order() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Kid = CreateFrame("Frame", "Kid", Dialog)          -- level 1 (parent + 1)
        Grandkid = CreateFrame("Frame", "Grandkid", Kid)   -- level 2
        Cross = CreateFrame("Frame", "Cross", Dialog)      -- level 1, then a different stratum
        Cross:SetFrameStrata("DIALOG")
        Dialog:SetToplevel(true)
        Dialog:Show()
        "#,
    )
    .unwrap();

    assert_eq!(level(&mut s, "Board"), 3, "compacted 5 -> 3");
    assert_eq!(level(&mut s, "Dialog"), 4, "raised to bucket->count");
    assert_eq!(level(&mut s, "Kid"), 5, "same delta (+4) as its parent");
    assert_eq!(level(&mut s, "Grandkid"), 6, "and so does the grandchild");
    assert_eq!(
        level(&mut s, "Cross"),
        1,
        "a cross-strata child is skipped by the propagate"
    );
    assert_eq!(
        s.eval::<String>("return Cross:GetFrameStrata()").unwrap(),
        "DIALOG"
    );
}

/// Compaction (`0x764eb0`) keeps `level := top + 1` from climbing: twenty alternating raises stay
/// within a two-level band.
#[test]
fn compaction_bounds_the_raise_across_repeated_shows() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        A = CreateFrame("Frame", "A")
        A:SetPoint("BOTTOMLEFT", 100, 100)
        A:SetWidth(300); A:SetHeight(300)
        A:SetToplevel(true)
        B = CreateFrame("Frame", "B")
        B:SetPoint("BOTTOMLEFT", 200, 200)
        B:SetWidth(300); B:SetHeight(300)
        B:SetToplevel(true)
        "#,
    )
    .unwrap();
    s.resolve();

    for _ in 0..10 {
        s.run("A:Hide(); A:Show()").unwrap();
        assert!(
            level(&mut s, "A") > level(&mut s, "B"),
            "the window just shown is in front"
        );
        s.run("B:Hide(); B:Show()").unwrap();
        assert!(level(&mut s, "B") > level(&mut s, "A"));
    }
    assert!(
        level(&mut s, "A") <= 2 && level(&mut s, "B") <= 2,
        "twenty raises stay inside the band compaction defines, not 20 levels up: A={} B={}",
        level(&mut s, "A"),
        level(&mut s, "B")
    );
}

// ── The Lua verbs ──

/// `Raise` (`0x775a50`, then `0x76a5b0`, then `0x7650f0` with force 1) acts on the nearest toplevel
/// self or ancestor, and on nothing when there is none.
#[test]
fn lua_raise_acts_on_the_nearest_toplevel_ancestor_and_is_silent_without_one() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Dialog:SetToplevel(true)
        Dialog:Show()
        Kid = CreateFrame("Frame", "Kid", Dialog)
        Loose = CreateFrame("Frame", "Loose")
        Loose:SetPoint("BOTTOMLEFT", 200, 200)
        Loose:SetWidth(300); Loose:SetHeight(300)
        "#,
    )
    .unwrap();
    s.resolve();
    // Re-lower the dialog under Board so there is something to raise over again.
    s.run("Board:SetFrameLevel(9); Dialog:SetFrameLevel(0)")
        .unwrap();
    s.run("Kid:Raise()").unwrap();
    assert!(
        level(&mut s, "Dialog") > level(&mut s, "Board"),
        "raising a child raised the toplevel window it lives in"
    );

    // No toplevel in the chain: a no-op, not an error.
    let board_before = level(&mut s, "Board");
    let loose_before = level(&mut s, "Loose");
    s.run("Loose:Raise()").unwrap();
    assert_eq!(level(&mut s, "Loose"), loose_before);
    assert_eq!(level(&mut s, "Board"), board_before);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The reference's `Lower` does nothing (`0x7652a0`: `xor eax,eax; ret 4`), and neither does ours.
#[test]
fn lua_lower_exists_and_does_nothing() {
    let mut s = board_and_dialog();
    s.run("Dialog:SetToplevel(true); Dialog:Show()").unwrap();
    let (d, b) = (level(&mut s, "Dialog"), level(&mut s, "Board"));
    s.run("Dialog:Lower(); Board:Lower()").unwrap();
    assert_eq!((level(&mut s, "Dialog"), level(&mut s, "Board")), (d, b));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `StartMoving` raises a toplevel window before the drag begins (`0x7652b0` at `0x7652d7`).
#[test]
fn starting_a_move_raises_the_dragged_window() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Dialog:SetToplevel(true)
        Dialog:SetMovable(true)
        Dialog:EnableMouse(true)
        Dialog:Show()
        Board:SetFrameLevel(9)          -- put the dialog back underneath
        Dialog:SetFrameLevel(0)
        "#,
    )
    .unwrap();
    s.resolve();
    assert!(level(&mut s, "Dialog") < level(&mut s, "Board"));

    s.run("Dialog:StartMoving()").unwrap();
    assert!(
        level(&mut s, "Dialog") > level(&mut s, "Board"),
        "the grab brought it to the front"
    );
    s.run("Dialog:StopMovingOrSizing()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

// ── The mouse-down trigger ──

/// The centre of `name`'s resolved rect, in the coordinate space [`UiScript::mouse_button`] takes.
fn centre(s: &UiScript, name: &str) -> (f32, f32) {
    let (x, y) = s
        .eval::<(f64, f64)>(&format!(
            "return ({name}:GetLeft() + {name}:GetRight()) / 2, \
                    ({name}:GetBottom() + {name}:GetTop()) / 2"
        ))
        .unwrap();
    (x as f32, y as f32)
}

/// A press raises the window: the trigger at `0x766392` in the mouse-down handler `0x7662c0`.
#[test]
fn pressing_a_toplevel_window_brings_it_to_the_front() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Dialog:SetToplevel(true)
        Dialog:EnableMouse(true)
        Dialog:Show()
        Board:SetFrameLevel(9)          -- put the dialog back underneath
        Dialog:SetFrameLevel(0)
        "#,
    )
    .unwrap();
    s.resolve();
    assert!(level(&mut s, "Dialog") < level(&mut s, "Board"));

    // Board overlaps Dialog's centre but takes no mouse, so the hit is Dialog.
    let (x, y) = centre(&s, "Dialog");
    assert_eq!(s.hit_test_name(x, y).as_deref(), Some("Dialog"));
    s.mouse_button(x, y, "LeftButton", true);
    assert!(
        level(&mut s, "Dialog") > level(&mut s, "Board"),
        "the press brought it to the front"
    );
    s.mouse_button(x, y, "LeftButton", false);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The press trigger has no toplevel test; the worker raises the nearest toplevel self or ancestor.
#[test]
fn pressing_a_child_of_a_toplevel_window_raises_the_window() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Dialog:SetToplevel(true)
        Dialog:Show()
        Knob = CreateFrame("Frame", "Knob", Dialog)
        Knob:SetPoint("CENTER", Dialog, "CENTER")
        Knob:SetWidth(40); Knob:SetHeight(40)
        Knob:EnableMouse(true)
        Board:SetFrameLevel(9)
        Dialog:SetFrameLevel(0)
        "#,
    )
    .unwrap();
    s.resolve();
    assert!(level(&mut s, "Dialog") < level(&mut s, "Board"));

    let (x, y) = centre(&s, "Knob");
    assert_eq!(s.hit_test_name(x, y).as_deref(), Some("Knob"));
    s.mouse_button(x, y, "LeftButton", true);
    assert!(
        level(&mut s, "Dialog") > level(&mut s, "Board"),
        "pressing the child raised its toplevel ancestor"
    );
    s.mouse_button(x, y, "LeftButton", false);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The mouse-up handler (`0x766420`, category `0xe`) has no raise; only the down edge raises.
#[test]
fn releasing_over_a_toplevel_window_does_not_raise_it() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Dialog:SetToplevel(true)
        Dialog:EnableMouse(true)
        Dialog:Show()
        Board:SetFrameLevel(9)
        Dialog:SetFrameLevel(0)
        "#,
    )
    .unwrap();
    s.resolve();
    let (x, y) = centre(&s, "Dialog");
    let before = level(&mut s, "Dialog");
    // A release with no press before it.
    s.mouse_button(x, y, "LeftButton", false);
    assert_eq!(
        level(&mut s, "Dialog"),
        before,
        "the release raised nothing"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The press raises the capture (`root+0x80`) before the hover (`root+0x7c`), so a second button
/// raises the held frame; the capture clears when the last button comes up (`0x7664bb`).
#[test]
fn a_chorded_press_raises_the_held_frame_not_the_one_under_the_cursor() {
    let mut s = board_and_dialog();
    s.run(
        r#"
        Dialog:SetToplevel(true)
        Dialog:EnableMouse(true)
        Dialog:Show()

        -- A second window well clear of the first, with something of its own to raise over: the
        -- gate is occlusion, so a frame overlapping nothing would decline whatever we press.
        Other = CreateFrame("Frame", "Other")
        Other:SetPoint("BOTTOMLEFT", 500, 100)
        Other:SetWidth(200); Other:SetHeight(200)
        Other:EnableMouse(true)
        Other:SetToplevel(true)
        Tile = CreateFrame("Frame", "Tile")
        Tile:SetPoint("BOTTOMLEFT", 660, 260)     -- clips Other's far corner only
        Tile:SetWidth(60); Tile:SetHeight(60)
        Tile:SetFrameLevel(9)

        Board:SetFrameLevel(9)
        Dialog:SetFrameLevel(0)
        Other:SetFrameLevel(0)
        "#,
    )
    .unwrap();
    s.resolve();

    // Hold the left button on Dialog.
    let (dx, dy) = centre(&s, "Dialog");
    s.mouse_button(dx, dy, "LeftButton", true);
    let dialog_held = level(&mut s, "Dialog");
    let other_before = level(&mut s, "Other");

    // Then press the right button over Other: the capture is still Dialog's.
    let (ox, oy) = centre(&s, "Other");
    assert_eq!(s.hit_test_name(ox, oy).as_deref(), Some("Other"));
    s.mouse_button(ox, oy, "RightButton", true);
    assert_eq!(
        level(&mut s, "Other"),
        other_before,
        "the frame under the cursor is not the raise target while a capture is held"
    );
    assert_eq!(level(&mut s, "Dialog"), dialog_held);

    // Both buttons up: only now does the capture clear.
    s.mouse_button(ox, oy, "LeftButton", false);
    s.mouse_button(ox, oy, "RightButton", false);
    let other_now = level(&mut s, "Other");
    s.mouse_button(ox, oy, "LeftButton", true);
    assert!(
        level(&mut s, "Other") > other_now,
        "with no capture held the press raises what the cursor is over"
    );
    s.mouse_button(ox, oy, "LeftButton", false);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Gatherer's Report window: `Win` raises while its full-cover `Body` is still hidden, then `Body`
/// shows. Siblings `Close` and `Body` must stay level through the compaction, so the reference's
/// hit sweep gives the tie to the earlier-linked `Close`.
#[test]
fn a_raise_with_a_hidden_child_keeps_the_windows_own_siblings_level() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        -- The window already on screen that `Win` has to raise over.
        Other = CreateFrame("Frame", "Other")
        Other:SetFrameStrata("DIALOG")
        Other:SetFrameLevel(1)
        Other:SetPoint("BOTTOMLEFT", 50, 50)
        Other:SetWidth(400); Other:SetHeight(400)
        Other:EnableMouse(true)
        Other:SetToplevel(true)
        OtherKid = CreateFrame("Frame", "OtherKid", Other)   -- level 2

        Win = CreateFrame("Frame", "Win")
        Win:SetFrameStrata("DIALOG")
        Win:SetFrameLevel(1)
        Win:SetPoint("BOTTOMLEFT", 200, 100)
        Win:SetWidth(400); Win:SetHeight(400)
        Win:EnableMouse(true)
        Win:SetToplevel(true)

        Close = CreateFrame("Button", "Close", Win)          -- level 2, visible with Win
        Close:SetPoint("BOTTOMRIGHT", Win, "BOTTOMRIGHT", -15, 15)
        Close:SetWidth(100); Close:SetHeight(21)
        Close:EnableMouse(true)
        Close:SetScript("OnEnter", function(self) hovered = self:GetName() end)
        Close:SetScript("OnClick", function(self) clicked = self:GetName() end)

        Body = CreateFrame("Frame", "Body", Win)             -- level 2, covers the whole window
        Body:SetAllPoints(Win)
        Body:EnableMouse(true)
        Body:SetScript("OnEnter", function(self) hovered = self:GetName() end)
        Body:Hide()

        Win:Hide()
        "#,
    )
    .unwrap();
    s.resolve();

    // The addon's two lines: the raise fires on the first, with `Body` still hidden.
    s.run("Win:Show()").unwrap();
    s.run("Body:Show()").unwrap();
    s.resolve();

    assert!(
        level(&mut s, "Win") > level(&mut s, "Other"),
        "the window that was just shown is in front: Win={} Other={}",
        level(&mut s, "Win"),
        level(&mut s, "Other")
    );

    // The Close button takes its own click and hover, asserted before the levels behind them.
    let (cx, cy) = centre(&s, "Close");
    assert_eq!(
        s.hit_test_name(cx, cy).as_deref(),
        Some("Close"),
        "the click at the Close button's centre lands on the Close button"
    );
    s.mouse_move(cx, cy);
    assert_eq!(
        s.eval::<String>("return tostring(hovered)").unwrap(),
        "Close",
        "and so does the hover — the highlight is the reference's OnEnter"
    );
    s.mouse_button(cx, cy, "LeftButton", true);
    s.mouse_button(cx, cy, "LeftButton", false);
    assert_eq!(
        s.eval::<String>("return tostring(clicked)").unwrap(),
        "Close"
    );

    assert_eq!(
        level(&mut s, "Close"),
        level(&mut s, "Body"),
        "two children of one parent are level with each other — the raise's compaction may not \
         split them just because one of them was hidden when it ran"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetFrameLevel` (`0x774560`) calls `0x76a4f0` with `propagate=0`, so the children stay put;
/// stock `BonusActionBarFrame.xml:13-15` raises a button and then its cooldown by hand.
#[test]
fn a_script_level_change_leaves_the_children_where_they_were() {
    let mut s = script();
    s.run(
        r#"
        Parent = CreateFrame("Frame", "Parent")
        Child = CreateFrame("Frame", "Child", Parent)
        Grandchild = CreateFrame("Frame", "Grandchild", Child)
        "#,
    )
    .unwrap();
    let (child, grandchild) = (level(&mut s, "Child"), level(&mut s, "Grandchild"));
    s.run("Parent:SetFrameLevel(7)").unwrap();
    assert_eq!(level(&mut s, "Parent"), 7);
    assert_eq!(
        (level(&mut s, "Child"), level(&mut s, "Grandchild")),
        (child, grandchild),
        "the children keep their absolute levels"
    );
}
