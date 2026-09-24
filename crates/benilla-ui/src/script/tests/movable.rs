//! The movable-frame family (`SetMovable`, `StartMoving`, `StopMovingOrSizing`, `SetUserPlaced`,
//! `SetResizable`), title regions and the user-placed layout cache, driven through the Lua
//! bindings and the real mouse entry points.

use super::common::script;
use crate::script::UiScript;

/// A frame wired the canonical way, laid out at (100, 100) 200×80 on an 800×600 screen.
fn movable_panel() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        starts, stops = 0, 0
        Panel = CreateFrame("Frame", "MovePanel")
        Panel:SetPoint("BOTTOMLEFT", 100, 100)
        Panel:SetWidth(200); Panel:SetHeight(80)
        Panel:EnableMouse(true)
        Panel:SetMovable(true)
        Panel:RegisterForDrag("LeftButton")
        Panel:SetScript("OnDragStart", function() starts = starts + 1; this:StartMoving() end)
        Panel:SetScript("OnDragStop",  function() stops = stops + 1; this:StopMovingOrSizing() end)
        "#,
    )
    .unwrap();
    s.resolve();
    s
}

/// The frame's resolved bottom-left corner, after a resolve.
fn corner(s: &mut UiScript) -> (f32, f32) {
    s.resolve();
    let left = s.eval::<f64>("return MovePanel:GetLeft()").unwrap() as f32;
    let bottom = s.eval::<f64>("return MovePanel:GetBottom()").unwrap() as f32;
    (left, bottom)
}

#[test]
fn the_canonical_addon_idiom_moves_a_frame_and_the_position_survives_the_stop() {
    let mut s = movable_panel();
    assert_eq!(corner(&mut s), (100.0, 100.0), "born where SetPoint put it");

    s.mouse_button(150.0, 140.0, "LeftButton", true); // press inside the panel
    s.mouse_move(152.0, 140.0); // 2 px, under the drag threshold
    assert_eq!(s.eval::<i64>("return starts").unwrap(), 0);
    assert_eq!(corner(&mut s), (100.0, 100.0));

    s.mouse_move(160.0, 140.0); // crosses the threshold ⇒ OnDragStart ⇒ StartMoving
    assert_eq!(s.eval::<i64>("return starts").unwrap(), 1);
    s.mouse_move(260.0, 190.0); // +100, +50 from the grab
    assert_eq!(
        corner(&mut s),
        (200.0, 150.0),
        "the frame follows the cursor delta since StartMoving"
    );

    s.mouse_button(260.0, 190.0, "LeftButton", false); // OnDragStop ⇒ StopMovingOrSizing
    assert_eq!(s.eval::<i64>("return stops").unwrap(), 1);
    assert_eq!(corner(&mut s), (200.0, 150.0), "the position survives");

    s.mouse_move(500.0, 400.0);
    assert_eq!(corner(&mut s), (200.0, 150.0), "no longer following");
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // The reference moves the anchors in place, so GetPoint reads the frame's own point, dragged.
    let (point, x, y) = s
        .eval::<(String, f64, f64)>("local p, _, _, x, y = MovePanel:GetPoint() return p, x, y")
        .unwrap();
    assert_eq!((point.as_str(), x, y), ("BOTTOMLEFT", 200.0, 150.0));
}

/// Each move applies only its own delta: the pump re-centers its sample after every step.
#[test]
fn a_moving_frame_follows_successive_mouse_moves() {
    let mut s = movable_panel();
    s.mouse_button(150.0, 140.0, "LeftButton", true);
    s.mouse_move(160.0, 140.0); // the move that starts the drag
    assert_eq!(
        corner(&mut s),
        (100.0, 100.0),
        "the frame follows from where StartMoving was called, so the starting move moves nothing"
    );
    for (step, want) in [
        ((170.0, 140.0), (110.0, 100.0)),
        ((170.0, 160.0), (110.0, 120.0)),
        ((120.0, 160.0), (60.0, 120.0)), // back left of where it started
        ((120.0, 160.0), (60.0, 120.0)), // a zero-delta move changes nothing
    ] {
        s.mouse_move(step.0, step.1);
        assert_eq!(corner(&mut s), want, "after moving to {step:?}");
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The reference's `StartMoving` raises on a frame that is not movable (`0x776700`).
#[test]
fn start_moving_on_a_frame_that_is_not_movable_raises_and_moves_nothing() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Fixed = CreateFrame("Frame", "FixedPanel")
        Fixed:SetPoint("BOTTOMLEFT", 100, 100)
        Fixed:SetWidth(200); Fixed:SetHeight(80)
        ok, err = pcall(function() Fixed:StartMoving() end)
        "#,
    )
    .unwrap();
    s.resolve();
    assert!(!s.eval::<bool>("return FixedPanel:IsMovable()").unwrap());
    assert!(!s.eval::<bool>("return ok").unwrap(), "StartMoving raised");
    assert!(
        s.eval::<String>("return tostring(err)")
            .unwrap()
            .contains("not movable"),
        "the refusal names why"
    );

    s.mouse_move(400.0, 400.0);
    s.resolve();
    let left = s.eval::<f64>("return FixedPanel:GetLeft()").unwrap();
    assert_eq!(left, 100.0, "a refused StartMoving started no move");

    s.run("FixedPanel:SetMovable(true) FixedPanel:StartMoving()")
        .unwrap();
    s.mouse_move(450.0, 400.0);
    s.resolve();
    assert_eq!(s.eval::<f64>("return FixedPanel:GetLeft()").unwrap(), 150.0);
}

/// `StopMovingOrSizing` is harmless with nothing moving, and stops only the frame in the drag slot
/// (the reference compares `[root+0xcfc]` with self).
#[test]
fn stop_moving_or_sizing_is_harmless_with_nothing_moving_and_stops_only_its_own_frame() {
    let mut s = movable_panel();
    s.run(
        r#"
        Other = CreateFrame("Frame", "OtherPanel")
        Other:SetPoint("BOTTOMLEFT", 400, 400)
        Other:SetWidth(50); Other:SetHeight(50)
        MovePanel:StopMovingOrSizing()      -- nothing is moving
        MovePanel:StopMovingOrSizing()      -- twice
        OtherPanel:StopMovingOrSizing()
        "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert_eq!(corner(&mut s), (100.0, 100.0), "nothing moved");

    s.run("MovePanel:StartMoving()").unwrap();
    s.mouse_move(50.0, 50.0); // cursor_pos was (0,0) ⇒ +50, +50
    assert_eq!(corner(&mut s), (150.0, 150.0));
    s.run("OtherPanel:StopMovingOrSizing()").unwrap();
    s.mouse_move(60.0, 50.0);
    assert_eq!(
        corner(&mut s),
        (160.0, 150.0),
        "another frame's stop does not end this move"
    );
    s.run("MovePanel:StopMovingOrSizing()").unwrap();
    s.mouse_move(200.0, 200.0);
    assert_eq!(corner(&mut s), (160.0, 150.0), "its own stop does");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A move translates every anchor, so a frame sized by two anchors keeps its size.
#[test]
fn a_frame_stretched_between_two_anchors_moves_rigidly() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Back = CreateFrame("Frame", "StretchBack")
        Back:SetPoint("BOTTOMLEFT", 0, 0); Back:SetWidth(800); Back:SetHeight(600)
        Stretch = CreateFrame("Frame", "StretchPanel", Back)
        Stretch:SetPoint("BOTTOMLEFT", Back, "BOTTOMLEFT", 100, 100)
        Stretch:SetPoint("TOPRIGHT",   Back, "BOTTOMLEFT", 300, 200)
        Stretch:SetMovable(true)
        "#,
    )
    .unwrap();
    s.resolve();
    let size = s
        .eval::<(f64, f64)>("return StretchPanel:GetWidth(), StretchPanel:GetHeight()")
        .unwrap();
    assert_eq!(size, (200.0, 100.0), "size derived from the two anchors");

    s.run("StretchPanel:StartMoving()").unwrap();
    s.mouse_move(30.0, 40.0); // cursor_pos starts at (0,0)
    s.run("StretchPanel:StopMovingOrSizing()").unwrap();
    s.resolve();
    let left = s.eval::<f64>("return StretchPanel:GetLeft()").unwrap();
    let bottom = s.eval::<f64>("return StretchPanel:GetBottom()").unwrap();
    let size_after = s
        .eval::<(f64, f64)>("return StretchPanel:GetWidth(), StretchPanel:GetHeight()")
        .unwrap();
    assert_eq!((left, bottom), (130.0, 140.0), "both anchors translated");
    assert_eq!(size_after, (200.0, 100.0), "and the frame kept its size");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A scaled frame tracks the cursor 1:1 on screen: the offsets it writes are in local units
/// (`0x768710` divides by the frame's scale).
#[test]
fn a_scaled_frame_tracks_the_cursor_one_to_one_on_screen() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Scaled = CreateFrame("Frame", "ScaledPanel")
        Scaled:SetPoint("BOTTOMLEFT", 100, 100)
        Scaled:SetWidth(100); Scaled:SetHeight(100)
        Scaled:SetScale(2)
        Scaled:SetMovable(true)
        Scaled:StartMoving()
        "#,
    )
    .unwrap();
    s.resolve();
    // Local 100 at scale 2 ⇒ 200 screen px.
    assert_eq!(
        s.eval::<f64>("return ScaledPanel:GetLeft()").unwrap(),
        100.0
    );
    s.mouse_move(40.0, 0.0);
    s.resolve();
    // GetLeft reports local units: 40 screen px is 20 local, on top of the local 100.
    assert_eq!(
        s.eval::<f64>("return ScaledPanel:GetLeft()").unwrap(),
        120.0
    );
    let x = s
        .eval::<f64>("local _, _, _, x = ScaledPanel:GetPoint() return x")
        .unwrap();
    assert_eq!(x, 120.0, "the anchor offset is local too");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The reference refuses `SetUserPlaced` unless the frame is movable or resizable (`0x776adb`),
/// and its drag start sets the bit itself (`0x7652b0`).
#[test]
fn the_three_flags_default_off_round_trip_and_user_placed_is_guarded() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        F = CreateFrame("Frame", "FlagPanel")
        F:SetPoint("BOTTOMLEFT", 10, 10); F:SetWidth(50); F:SetHeight(50)
        "#,
    )
    .unwrap();
    let all = |s: &mut UiScript| {
        s.eval::<(bool, bool, bool)>(
            "return FlagPanel:IsMovable(), FlagPanel:IsResizable(), FlagPanel:IsUserPlaced()",
        )
        .unwrap()
    };
    assert_eq!(
        all(&mut s),
        (false, false, false),
        "no frame is born flagged"
    );

    assert!(
        !s.eval::<bool>("return (pcall(function() FlagPanel:SetUserPlaced(true) end))")
            .unwrap(),
        "SetUserPlaced on a frame that is neither movable nor resizable raises"
    );
    assert!(!s.eval::<bool>("return FlagPanel:IsUserPlaced()").unwrap());

    s.run("FlagPanel:SetResizable(true) FlagPanel:SetUserPlaced(true)")
        .unwrap();
    assert_eq!(
        all(&mut s),
        (false, true, true),
        "resizable satisfies it too"
    );
    s.run("FlagPanel:SetUserPlaced(false) FlagPanel:SetResizable(false) FlagPanel:SetMovable(1)")
        .unwrap();
    assert_eq!(
        all(&mut s),
        (true, false, false),
        "truthy 1 sets, false clears"
    );

    s.resolve();
    s.run("FlagPanel:StartMoving()").unwrap();
    assert!(
        s.eval::<bool>("return FlagPanel:IsUserPlaced()").unwrap(),
        "StartMoving sets the userPlaced bit"
    );
    s.run("FlagPanel:StopMovingOrSizing()").unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn the_xml_movable_and_resizable_attributes_reach_the_methods() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
             <Frame name="XmlMovable" movable="true" resizable="true">
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
        !report
            .warnings
            .iter()
            .any(|w| w.contains("SetMovable") || w.contains("SetResizable")),
        "no gap warning any more: {:?}",
        report.warnings
    );
    assert_eq!(
        s.eval::<(bool, bool)>("return XmlMovable:IsMovable(), XmlMovable:IsResizable()")
            .unwrap(),
        (true, true)
    );
}

/// `StartSizing(grip)` (`0x776830`, called from `FloatingChatFrame.lua:600`) moves the gripped
/// edges and plants the opposite ones. Which edges a grip moves is untraced in the reference, whose
/// verb has no inline math; this takes the anchor point's plain meaning, as that caller does.
#[test]
fn start_sizing_moves_the_gripped_edge_and_plants_the_other() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "Sizer", UIParent)
        f:SetResizable(true)
        f:SetWidth(200) f:SetHeight(100)
        f:SetPoint("BOTTOMLEFT", UIParent, "BOTTOMLEFT", 100, 50)
        "#,
    )
    .unwrap();
    s.resolve();

    // Grip the right edge and drag 40 right: the width grows, the left edge stays planted.
    s.mouse_move(300.0, 100.0);
    s.run("Sizer:StartSizing(\"RIGHT\")").unwrap();
    s.mouse_move(340.0, 100.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), 240.0);
    assert_eq!(s.eval::<f32>("return Sizer:GetLeft()").unwrap(), 100.0);
    s.run("Sizer:StopMovingOrSizing()").unwrap();

    // Grip the left edge and drag 30 right: the width shrinks and the right edge stays put.
    let right_before = s.eval::<f32>("return Sizer:GetRight()").unwrap();
    s.run("Sizer:StartSizing(\"LEFT\")").unwrap();
    s.mouse_move(370.0, 100.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), 210.0);
    assert_eq!(s.eval::<f32>("return Sizer:GetLeft()").unwrap(), 130.0);
    assert_eq!(
        s.eval::<f32>("return Sizer:GetRight()").unwrap(),
        right_before,
        "the ungripped edge must not move"
    );

    s.run("Sizer:StopMovingOrSizing()").unwrap();
    s.mouse_move(500.0, 100.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), 210.0);

    s.run("g = CreateFrame(\"Frame\", \"NotSizer\", UIParent)")
        .unwrap();
    assert!(
        s.run("NotSizer:StartSizing(\"RIGHT\")").is_err(),
        "StartSizing must refuse a frame that is not resizable"
    );
}

/// `CreateTitleRegion` (`0x773910`) and `GetTitleRegion` (`0x773820`): the object half.
#[test]
fn a_title_region_is_a_plain_region_and_creating_it_twice_is_destructive() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        TFrame = CreateFrame("Frame", "TFrame")
        TFrame:SetPoint("BOTTOMLEFT", 100, 100)
        TFrame:SetWidth(200); TFrame:SetHeight(80)
        "#,
    )
    .unwrap();

    // With none, it returns one nil, where `GetBackdrop` (`0x777370`) returns no value at all.
    assert_eq!(s.arity("TFrame:GetTitleRegion()").unwrap(), 1);
    assert!(s
        .eval::<Option<bool>>("return TFrame:GetTitleRegion() ~= nil and true or nil")
        .unwrap()
        .is_none());

    // A plain Region, not a Texture.
    s.run("TR = TFrame:CreateTitleRegion()").unwrap();
    assert_eq!(
        s.eval::<String>("return TR:GetObjectType()").unwrap(),
        "Region"
    );
    assert_eq!(
        s.eval::<i64>("return TR:IsObjectType('Region')").unwrap(),
        1
    );
    assert!(s
        .eval::<Option<i64>>("return TR:IsObjectType('Texture')")
        .unwrap()
        .is_none());
    assert!(s
        .eval::<bool>("return TFrame:GetTitleRegion() == TR")
        .unwrap());

    // It takes no argument: `CreateTitleRegion(frame)` is the no-argument call.
    assert!(s
        .eval::<bool>("return TFrame:CreateTitleRegion(TFrame) == TR")
        .unwrap());

    // A second call returns the same region with its anchors cleared.
    s.run("TR:SetAllPoints(TFrame)").unwrap();
    s.resolve();
    assert_eq!(s.eval::<i64>("return TR:GetNumPoints()").unwrap(), 2);
    assert!(s
        .eval::<bool>("return TFrame:CreateTitleRegion() == TR")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return TR:GetNumPoints()").unwrap(),
        0,
        "the second call ran ClearAllPoints on the region it returned"
    );

    // It answers the 19 Region methods and nothing else (`0x81c554`, `0x81c528`): no Show/Hide,
    // no scripts, no textures.
    for name in crate::script::REGION_MAP_METHODS {
        assert_eq!(
            s.eval::<String>(&format!("return type(TR.{name})"))
                .unwrap(),
            "function",
            "a title region answers {name}"
        );
    }
    for absent in [
        "SetTexture",
        "GetTexture",
        "SetTexCoord",
        "SetVertexColor",
        "SetDrawLayer",
        "Show",
        "Hide",
        "IsShown",
        "IsVisible",
        "SetText",
        "GetText",
        "SetAlpha",
    ] {
        assert_eq!(
            s.eval::<String>(&format!("return type(TR.{absent})"))
                .unwrap(),
            "nil",
            "a title region must NOT answer {absent} — the reference raises there"
        );
    }
    // It never draws: a title region is a hit rectangle, not a visual.
    s.run("TR:SetAllPoints(TFrame)").unwrap();
    s.resolve();
    let region_quads: Vec<_> = s
        .extract()
        .into_iter()
        .filter(|q| matches!(q.target, crate::script::ZTarget::Region(_)))
        .collect();
    assert!(
        region_quads.is_empty(),
        "a title region emits no quad of its own (the frame's own quad is not one): {region_quads:?}"
    );

    // Checked last: a real texture would add a legitimate quad to the check above.
    s.run("TTex = TFrame:CreateTexture(nil, 'ARTWORK')")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return type(TTex.SetTexture)").unwrap(),
        "function",
        "a Texture still answers its own verbs"
    );
}

/// A press in the title region starts a move that, unlike `StartMoving`, swallows `OnMouseDown`,
/// ends on release and skips the movable check.
#[test]
fn a_title_region_drag_swallows_the_press_and_ends_on_release() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        downs = 0
        TP = CreateFrame("Frame", "TP")
        TP:SetPoint("BOTTOMLEFT", 100, 100)
        TP:SetWidth(200); TP:SetHeight(80)
        TP:EnableMouse(true)
        TP:SetScript("OnMouseDown", function() downs = downs + 1 end)
        TP:CreateTitleRegion():SetAllPoints(TP)
        "#,
    )
    .unwrap();
    s.resolve();
    let corner = |s: &mut UiScript| {
        s.resolve();
        s.eval::<f64>("return TP:GetLeft()").unwrap()
    };
    assert_eq!(corner(&mut s), 100.0);

    s.mouse_button(150.0, 150.0, "LeftButton", true);
    assert_eq!(
        s.eval::<i64>("return downs").unwrap(),
        0,
        "a title-region hit swallows OnMouseDown — a miss would fall through to it"
    );
    s.mouse_move(250.0, 150.0);
    assert_eq!(corner(&mut s), 200.0, "the frame followed the cursor");

    // Release ends it: the title-region move (mode 2) cancels itself.
    s.mouse_button(250.0, 150.0, "LeftButton", false);
    s.mouse_move(400.0, 150.0);
    assert_eq!(
        corner(&mut s),
        200.0,
        "the move ended at the release; a scripted StartMoving would still be running"
    );

    // The movable bit is not read on this path (`0x7662c0`, `0x765320`, `0x7652b0`, `0x768430`).
    assert!(!s.eval::<bool>("return TP:IsMovable()").unwrap());
}

/// A region's rect getters answer in its owner's units, screen ÷ the owner's effective scale, as
/// the frame getters do.
#[test]
fn region_getters_answer_in_the_owners_units_under_scale() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"f = CreateFrame("Frame", "ScaledOwner") f:SetWidth(200) f:SetHeight(100)
           f:SetPoint("BOTTOMLEFT", 100, 50) f:SetScale(0.5)
           t = f:CreateTexture("ScaledTex") t:SetAllPoints(f)"#,
    )
    .unwrap();
    s.resolve();
    let (fl, fr, tl, tr, cx) = s
        .eval::<(f64, f64, f64, f64, f64)>(
            "local cx = t:GetCenter() return f:GetLeft(), f:GetRight(), t:GetLeft(), t:GetRight(), cx",
        )
        .unwrap();
    assert!(
        (fl - tl).abs() < 1e-3 && (fr - tr).abs() < 1e-3,
        "frame {fl}..{fr} vs region {tl}..{tr}"
    );
    assert!(
        (fr - fl - 200.0).abs() < 1e-3,
        "the owner's own width, {}",
        fr - fl
    );
    assert!((cx - (fl + fr) * 0.5).abs() < 1e-3);
}

/// The layout cache writes a frame only when it is user-placed (`0x490e8e`) and movable or
/// resizable (`0x490e97`): the drag start (`0x7652b0` at `0x7652e5`) and the cache's own apply
/// set the bit past `SetUserPlaced`'s guard (`0x776adb`), so clearing the flags stops the save.
#[test]
fn the_write_filter_is_user_placed_and_movable_or_resizable() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        F = CreateFrame("Frame", "CachePanel")
        F:SetPoint("BOTTOMLEFT", 10, 10); F:SetWidth(50); F:SetHeight(50)
        F:SetResizable(true); F:SetUserPlaced(true)
        "#,
    )
    .unwrap();
    let names = |s: &UiScript| {
        s.user_placed_layouts()
            .into_iter()
            .map(|l| l.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&s), ["CachePanel"], "stamped and resizable — written");

    s.run("CachePanel:SetResizable(false)").unwrap();
    assert!(
        s.eval::<bool>("return CachePanel:IsUserPlaced()").unwrap(),
        "the stamp itself survives the flag going away — nothing clears it"
    );
    assert!(
        names(&s).is_empty(),
        "but the row does not: the fourth conjunct is gone"
    );

    s.run("CachePanel:SetMovable(true)").unwrap();
    assert_eq!(names(&s), ["CachePanel"], "either flag satisfies it");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The cache's apply seats position only when movable (`0x490600`) and size only when resizable
/// (`0x490689`), and each arm that runs sets userPlaced (`0x49067e`, `0x490706`); a frame with
/// neither flag is left alone.
#[test]
fn the_apply_seats_position_behind_movable_and_size_behind_resizable() {
    use crate::script::{FrameLayout, LayoutPoint};

    let row = |name: &str| FrameLayout {
        name: name.to_owned(),
        width: 200.0,
        height: 80.0,
        points: vec![LayoutPoint {
            point: "BOTTOMLEFT".into(),
            relative_to: None,
            relative_point: "BOTTOMLEFT".into(),
            x: 300.0,
            y: 200.0,
        }],
    };
    // Three frames, one per flag state, all authored identically.
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    for (name, flags) in [
        ("Neither", ""),
        ("Movable", "F:SetMovable(true)"),
        ("Sizable", "F:SetResizable(true)"),
    ] {
        s.run(&format!(
            r#"F = CreateFrame("Frame", "{name}")
               F:SetPoint("BOTTOMLEFT", 10, 10); F:SetWidth(50); F:SetHeight(50)
               {flags}"#
        ))
        .unwrap();
        s.restore_user_placed_layouts([row(name)]);
    }
    s.resolve();
    let read = |s: &UiScript, n: &str| {
        s.eval::<(f64, f64, f64, bool)>(&format!(
            "local f = getglobal('{n}') \
             return f:GetLeft(), f:GetWidth(), f:GetHeight(), f:IsUserPlaced()"
        ))
        .unwrap()
    };
    assert_eq!(
        read(&s, "Neither"),
        (10.0, 50.0, 50.0, false),
        "neither flag: the row is inert, and leaves no stamp behind either"
    );
    assert_eq!(
        read(&s, "Movable"),
        (300.0, 50.0, 50.0, true),
        "movable: seated at the saved position, still its authored size"
    );
    assert_eq!(
        read(&s, "Sizable"),
        (10.0, 200.0, 80.0, true),
        "resizable: given the saved size, left on its authored anchors"
    );
    assert_eq!(
        s.user_placed_layouts()
            .into_iter()
            .map(|l| l.name)
            .collect::<Vec<_>>(),
        ["Movable", "Sizable"],
        "and only the two that took an arm are written back"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
