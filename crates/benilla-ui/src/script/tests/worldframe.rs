//! The `WorldFrame` kind (decisions 1983/1984; vtable `0x8043e8`): a `Frame` to Lua,
//! a one-shot type, born in stratum `WORLD` with mouse and wheel enabled, and a hit the app's
//! arbiter must be able to tell apart.

use crate::script::UiScript;

fn vm() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
        .unwrap();
    s.resolve();
    s
}

/// `GetObjectType()` answers `"Frame"` and `IsObjectType("WorldFrame")` is nil: the class
/// inherits the base table's type slots (the TaxiRouteFrame precedent), so the registered name
/// never becomes an identity.
#[test]
fn the_world_frame_is_a_frame_to_lua() {
    let s = vm();
    assert_eq!(
        s.eval::<String>("return WorldFrame:GetObjectType()")
            .unwrap(),
        "Frame"
    );
    assert!(s
        .eval::<bool>(r#"return WorldFrame:IsObjectType("Frame") == 1 and WorldFrame:IsObjectType("Region") == 1"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return WorldFrame:IsObjectType("WorldFrame") == nil"#)
        .unwrap());
}

/// The registry record is destroyed on the first instantiation: a second one is an unknown type.
#[test]
fn a_second_world_frame_is_an_unknown_type() {
    let s = vm();
    let err = s
        .run(r#"CreateFrame("WorldFrame", "Another")"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown frame type 'WorldFrame'"), "{err}");
    assert!(s.eval::<bool>("return Another == nil").unwrap());
}

/// Born in stratum 0 (`WORLD`, below `BACKGROUND`) with key/mouse/wheel enabled; no XML or Lua can
/// name that stratum for another frame.
#[test]
fn the_world_frame_is_born_in_stratum_world_with_the_mouse_and_wheel() {
    let s = vm();
    assert_eq!(
        s.eval::<String>("return WorldFrame:GetFrameStrata()")
            .unwrap(),
        "WORLD"
    );
    assert!(s
        .eval::<bool>(
            "return WorldFrame:IsMouseEnabled() == 1 and WorldFrame:IsMouseWheelEnabled() == 1"
        )
        .unwrap());
    s.run(r#"plain = CreateFrame("Frame", "Plain") plain:SetFrameStrata("BACKGROUND")"#)
        .unwrap();
    let err = s
        .run(r#"plain:SetFrameStrata("WORLD")"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown frameStrata 'WORLD'"), "{err}");
    assert_eq!(
        s.eval::<String>("return plain:GetFrameStrata()").unwrap(),
        "BACKGROUND"
    );
}

/// The hit test answers the world frame where nothing else is under the cursor — the addon-facing
/// scripts on it fire — and the app can tell that hit apart from a UI hit.
#[test]
fn a_hit_on_the_world_frame_is_told_apart_from_a_ui_hit() {
    let mut s = vm();
    let id = s
        .hit_test(100.0, 100.0)
        .expect("the world frame is under the cursor");
    assert!(s.is_world_frame(id));
    s.run(
        r#"plate = CreateFrame("Frame", "Plate", WorldFrame) plate:SetWidth(50) plate:SetHeight(50)
           plate:SetPoint("BOTTOMLEFT", WorldFrame, "BOTTOMLEFT", 75, 75) plate:EnableMouse(true)"#,
    )
    .unwrap();
    s.resolve();
    let over_plate = s
        .hit_test(100.0, 100.0)
        .expect("the plate is under the cursor now");
    assert!(
        !s.is_world_frame(over_plate),
        "a child frame's hit is the UI's"
    );
    assert!(s.is_world_frame(s.hit_test(600.0, 600.0).unwrap()));
}
