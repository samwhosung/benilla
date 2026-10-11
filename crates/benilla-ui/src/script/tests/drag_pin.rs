//! Every drag pins its frame to one TOPLEFT anchor against the screen at its current rect, with an
//! explicit size (`0x7652b0` → `0x768430`), and the pump (`0x768710`) moves or resizes that one
//! anchor and size.

use super::common::script;
use crate::script::{FrameLayout, LayoutPoint, UiScript};

/// A 200×100 frame CENTER-anchored in a full-screen holder on an 800×600 screen: its rect is
/// left 300, right 500, bottom 250, top 350.
fn centred_box() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Holder = CreateFrame("Frame", "Holder")
        Holder:SetPoint("BOTTOMLEFT", 0, 0); Holder:SetWidth(800); Holder:SetHeight(600)
        Box = CreateFrame("Frame", "Box", Holder)
        Box:SetPoint("CENTER", Holder, "CENTER", 0, 0)
        Box:SetWidth(200); Box:SetHeight(100)
        Box:SetMovable(true); Box:SetResizable(true)
        "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_move(400.0, 300.0);
    s
}

/// `(left, right, bottom, top)` after a resolve.
fn edges(s: &mut UiScript) -> (f32, f32, f32, f32) {
    s.resolve();
    let (l, r, b, t) = s
        .eval::<(f64, f64, f64, f64)>(
            "return Box:GetLeft(), Box:GetRight(), Box:GetBottom(), Box:GetTop()",
        )
        .unwrap();
    (l as f32, r as f32, b as f32, t as f32)
}

/// The one anchor: `(count, point, relativeTo is the screen, relativePoint, x, y)`.
fn anchor(s: &UiScript) -> (i64, String, bool, String, f32, f32) {
    let (n, p, screen, rp, x, y) = s
        .eval::<(i64, String, bool, String, f64, f64)>(
            "local p, rel, rp, x, y = Box:GetPoint(1) \
             return Box:GetNumPoints(), p, rel == nil, rp, x, y",
        )
        .unwrap();
    (n, p, screen, rp, x as f32, y as f32)
}

fn pinned_at(x: f32, y: f32) -> (i64, String, bool, String, f32, f32) {
    (1, "TOPLEFT".into(), true, "TOPLEFT".into(), x, y)
}

#[test]
fn a_move_pins_one_topleft_anchor_against_the_screen() {
    let mut s = centred_box();
    s.run("Box:StartMoving()").unwrap();
    assert_eq!(
        anchor(&s),
        pinned_at(300.0, -250.0),
        "the drag entry pins before any cursor move: top 350 on a 600 screen is y -250"
    );
    s.mouse_move(430.0, 280.0);
    assert_eq!(edges(&mut s), (330.0, 530.0, 230.0, 330.0));
    assert_eq!(
        anchor(&s),
        pinned_at(330.0, -270.0),
        "the move rides that anchor"
    );
    s.run("Box:StopMovingOrSizing()").unwrap();
    assert_eq!(
        anchor(&s),
        pinned_at(330.0, -270.0),
        "the stop writes nothing"
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return Box:GetWidth(), Box:GetHeight()")
            .unwrap(),
        (200.0, 100.0),
        "with the size made explicit"
    );

    // The old target no longer carries it.
    s.run("Holder:SetPoint(\"BOTTOMLEFT\", 50, 0)").unwrap();
    assert_eq!(edges(&mut s), (330.0, 530.0, 230.0, 330.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `FCF_Set_NormalChat` (`FloatingChatFrame.lua:1422-1423`) pins a window with a bare
/// `StartMoving()`/`StopMovingOrSizing()` pair; a frame sized only by two anchors gets the size
/// its rect has.
#[test]
fn a_bare_start_and_stop_pins_a_stretched_frame_at_its_rect() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Box = CreateFrame("Frame", "Box")
        Box:SetPoint("BOTTOMLEFT", 100, 100)
        Box:SetPoint("TOPRIGHT", UIParent, "BOTTOMLEFT", 300, 200)
        Box:SetMovable(true)
        Box:StartMoving() Box:StopMovingOrSizing()
        "#,
    )
    .unwrap();
    assert_eq!(anchor(&s), pinned_at(100.0, -400.0));
    assert_eq!(
        s.eval::<(f64, f64)>("return Box:GetWidth(), Box:GetHeight()")
            .unwrap(),
        (200.0, 100.0)
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Each grip moves only its own edges, whatever the frame was anchored by (`0x768710`'s eight
/// size cases re-pin TOPLEFT shifted by the left and top travel only, `0x76898a`).
#[test]
fn each_grip_moves_only_its_own_edges() {
    for grip in [
        "TOPLEFT",
        "TOP",
        "TOPRIGHT",
        "LEFT",
        "RIGHT",
        "BOTTOMLEFT",
        "BOTTOM",
        "BOTTOMRIGHT",
    ] {
        let mut s = centred_box();
        s.run(&format!("Box:StartSizing(\"{grip}\")")).unwrap();
        s.mouse_move(420.0, 290.0);
        s.mouse_move(430.0, 285.0); // +30, -15 in all
        let (dl, dr) = match (grip.contains("LEFT"), grip.contains("RIGHT")) {
            (true, _) => (30.0, 0.0),
            (_, true) => (0.0, 30.0),
            _ => (0.0, 0.0),
        };
        let (db, dt) = match (grip.contains("BOTTOM"), grip.contains("TOP")) {
            (true, _) => (-15.0, 0.0),
            (_, true) => (0.0, -15.0),
            _ => (0.0, 0.0),
        };
        let want = (300.0 + dl, 500.0 + dr, 250.0 + db, 350.0 + dt);
        assert_eq!(edges(&mut s), want, "{grip}");
        assert_eq!(
            anchor(&s),
            pinned_at(want.0, want.3 - 600.0),
            "{grip} leaves the one TOPLEFT pin"
        );
        s.run("Box:StopMovingOrSizing()").unwrap();
        assert!(s.errors().is_empty(), "{grip}: {:?}", s.errors());
    }
}

/// The bounds clamp each case. Where the grip moves the anchor (a LEFT or TOP edge), the residual
/// comes back out of the travel, so the edge stops on the bound (`0x768770`, `0x7687a2`,
/// `0x7687dd`, `0x768817`); the far edges just clamp. The cursor sample always advances by the
/// whole delta (`0x765610`), so a grip held past its bound answers the first move back.
#[test]
fn the_bounds_clamp_and_the_anchored_edge_stops_on_them() {
    let bounded = || {
        let s = centred_box();
        s.run("Box:SetMinResize(150, 80) Box:SetMaxResize(260, 120)")
            .unwrap();
        s
    };

    let mut s = bounded();
    s.run("Box:StartSizing(\"LEFT\")").unwrap();
    s.mouse_move(480.0, 300.0);
    assert_eq!(
        edges(&mut s),
        (350.0, 500.0, 250.0, 350.0),
        "LEFT at the min"
    );
    s.mouse_move(460.0, 300.0);
    assert_eq!(
        edges(&mut s),
        (330.0, 500.0, 250.0, 350.0),
        "and back at once"
    );

    let mut s = bounded();
    s.run("Box:StartSizing(\"TOP\")").unwrap();
    s.mouse_move(400.0, 350.0);
    assert_eq!(
        edges(&mut s),
        (300.0, 500.0, 250.0, 370.0),
        "TOP at the max"
    );

    let mut s = bounded();
    s.run("Box:StartSizing(\"TOPLEFT\")").unwrap();
    s.mouse_move(300.0, 400.0);
    assert_eq!(
        edges(&mut s),
        (240.0, 500.0, 250.0, 370.0),
        "TOPLEFT at both maxima"
    );

    let mut s = bounded();
    s.run("Box:StartSizing(\"BOTTOMRIGHT\")").unwrap();
    s.mouse_move(500.0, 360.0);
    assert_eq!(
        edges(&mut s),
        (300.0, 560.0, 270.0, 350.0),
        "BOTTOMRIGHT at the max width and the min height"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `StartSizing` starts from BOTTOMRIGHT and takes a grip only from a string naming a point,
/// matched case-insensitively (`0x776913`, `0x6f1840`).
#[test]
fn a_missing_or_unknown_grip_is_bottomright() {
    for call in [
        "Box:StartSizing()",
        "Box:StartSizing(\"bogus\")",
        "Box:StartSizing(7)",
        "Box:StartSizing(\"bottomright\")",
    ] {
        let mut s = centred_box();
        s.run(call).unwrap();
        s.mouse_move(420.0, 290.0);
        assert_eq!(edges(&mut s), (300.0, 520.0, 240.0, 350.0), "{call}");
    }
    let mut s = centred_box();
    s.run("Box:StartSizing(\"left\")").unwrap();
    s.mouse_move(420.0, 290.0);
    assert_eq!(
        edges(&mut s),
        (320.0, 500.0, 250.0, 350.0),
        "a lower-case name"
    );
}

/// The pin needs a rect (`0x768320` fails, `0x7652cd`), so a frame that has none starts no drag:
/// no userPlaced stamp and no slot.
#[test]
fn a_frame_with_no_rect_starts_no_drag() {
    let mut s = centred_box();
    s.run(
        r#"
        Lost = CreateFrame("Frame", "Lost")
        Lost:SetWidth(50); Lost:SetHeight(50)
        Lost:SetMovable(true)
        Lost:StartMoving()
        "#,
    )
    .unwrap();
    assert!(!s.eval::<bool>("return Lost:IsUserPlaced()").unwrap());
    assert_eq!(s.eval::<i64>("return Lost:GetNumPoints()").unwrap(), 0);
    s.run("Box:StartMoving()").unwrap();
    s.mouse_move(410.0, 300.0);
    assert_eq!(
        edges(&mut s),
        (310.0, 510.0, 250.0, 350.0),
        "the slot was free"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// After a drag the layout cache holds the one TOPLEFT anchor and the size, the reference's `X:`,
/// `Y:`, `W:` and `H:` (`0x490e60`), and its apply seats that back.
#[test]
fn the_layout_cache_round_trips_the_pin() {
    let mut s = centred_box();
    s.run("Box:StartMoving()").unwrap();
    s.mouse_move(430.0, 280.0);
    s.run("Box:StopMovingOrSizing()").unwrap();
    let want = FrameLayout {
        name: "Box".into(),
        width: 200.0,
        height: 100.0,
        points: vec![LayoutPoint {
            point: "TOPLEFT".into(),
            relative_to: None,
            relative_point: "TOPLEFT".into(),
            x: 330.0,
            y: -270.0,
        }],
    };
    let saved = s.user_placed_layouts();
    assert_eq!(saved, std::slice::from_ref(&want));

    let mut fresh = centred_box();
    fresh.restore_user_placed_layouts(saved);
    assert_eq!(edges(&mut fresh), (330.0, 530.0, 230.0, 330.0));
    assert_eq!(anchor(&fresh), pinned_at(330.0, -270.0));
    assert_eq!(fresh.user_placed_layouts(), [want]);
    assert!(fresh.errors().is_empty(), "{:?}", fresh.errors());
}
