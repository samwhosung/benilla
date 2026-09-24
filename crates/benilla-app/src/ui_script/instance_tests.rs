//! The instance-lockout **Era surface**: the three engine bindings, and the one
//! place in the shipped UI that reads them — the SELF menu's "Reset all instances" row and the
//! `CONFIRM_RESET_INSTANCES` dialog behind it.
//!
//! The four chat lines the family's other packets raise are engine-composed and have no Lua at
//! all (no FrameXML file mentions `RAID_INSTANCE_WELCOME` or its siblings); they are tested at the
//! composer, in `crate::ui_instance`.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// The three bindings answer with the reference's own shapes — `IsInInstance` a PAIR, both
/// readers `1`/`nil` rather than `true`/`false` (the reference pushes the double 1.0), and
/// `ResetInstances` a queue.
#[test]
fn the_three_bindings_have_the_reference_shapes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();

    // Nothing pushed yet: the app has not said where we are.
    assert_eq!(
        s.eval::<(Option<f64>, String)>("return IsInInstance()")
            .unwrap(),
        (None, "none".into()),
        "no map pushed reads as not-an-instance"
    );

    for (ty, inside, name) in [
        (0u32, None, "none"),
        (1, Some(1.0), "party"),
        (2, Some(1.0), "raid"),
        (3, Some(1.0), "pvp"),
        // Past the reference's own `cmp esi,4; jae` guard.
        (4, Some(1.0), "none"),
    ] {
        s.set_instance_type(Some(ty));
        assert_eq!(
            s.eval::<(Option<f64>, String)>("return IsInInstance()")
                .unwrap(),
            (inside, name.into()),
            "InstanceType {ty}"
        );
    }

    assert_eq!(
        s.eval::<Option<f64>>("return CanShowResetInstances()")
            .unwrap(),
        None,
        "false by default — no bind, no dungeon behind us"
    );
    s.set_can_reset_instances(true);
    assert_eq!(
        s.eval::<Option<f64>>("return CanShowResetInstances()")
            .unwrap(),
        Some(1.0)
    );

    assert_eq!(s.take_reset_instance_asks(), 0);
    s.run("ResetInstances(); ResetInstances()").unwrap();
    assert_eq!(s.take_reset_instance_asks(), 2, "each call is one send");
    assert_eq!(s.take_reset_instance_asks(), 0, "the drain is a take");
}

/// The whole shipped path, through the real hit test: right-click your own portrait, the row is
/// there only while `CanShowResetInstances()` is true, clicking it raises the reference's confirm,
/// and Yes is the only thing that sends.
#[test]
fn the_self_menu_row_gates_on_the_binding_and_confirms_before_sending() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The row labels a bare harness has no GlobalStrings.lua for. Verbatim 1.12 values
    // (`RESET_INSTANCES` l.3342, `CONFIRM_RESET_INSTANCES` l.851, `YES` l.5463, `NO` l.2794,
    // `GROUP` l.2029) — production runs the player's own string table at boot.
    s.run(
        r#"
        -- The stock raid pane concatenates this into each of its eight group headers inside
        -- their own OnLoad, so it has to exist before the addon loads (1874).
        GROUP = "Group"
        RESET_INSTANCES = "Reset all instances"
        CONFIRM_RESET_INSTANCES = "Do you really want to reset all of your instances?"
        YES = "Yes"
        NO = "No"
        CANCEL = "Cancel"
    "#,
    )
    .unwrap();
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua", // `TEXT`, which StaticPopup.lua and UnitPopup.lua read at file scope
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        "Interface\\FrameXML\\PetFrame.xml",
        r"Interface\FrameXML\RaidFrame.xml",
    ] {
        load_xml(&s, file);
    }
    // The window is a LoadOnDemand addon, reached the way the app reaches it: seated off the
    // chain as a registry row (1957) and loaded by the reference's own `RaidFrame_LoadUI`
    // (UIParent.xml; 1967).
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_RaidUI");
    s.run("RaidFrame_LoadUI()").unwrap();
    s.resolve();
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());

    let open_self_menu = |s: &mut UiScript| {
        s.run("CloseDropDownMenus()").unwrap();
        s.run(r#"PlayerFrame_OnClick("RightButton")"#).unwrap();
        s.resolve();
    };
    let row_labels = |s: &mut UiScript| -> Vec<String> {
        let n = s.eval::<i64>("return DropDownList1.numButtons").unwrap();
        (1..=n)
            .map(|i| {
                s.eval::<String>(&format!("return DropDownList1Button{i}:GetText() or \"\""))
                    .unwrap()
            })
            .collect()
    };

    // Off: the row is not in the menu at all — hidden, not greyed (the reference's
    // `UnitPopupShown[index] = 0`). Solo, it is the ONLY row of the SELF menu that could show
    // (the loot trio, Leave and the raid marks all need a party), so the menu's "nothing but
    // CANCEL" early-out fires and no menu opens at all. That is the reference's behaviour too,
    // and it is the sharpest possible assertion that the row is really gone.
    open_self_menu(&mut s);
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "solo with no lockout to reset, the SELF menu has nothing to show"
    );

    // On: the row appears, and with it the menu.
    s.set_can_reset_instances(true);
    open_self_menu(&mut s);
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the one showable row is enough to open the SELF menu"
    );
    let labels = row_labels(&mut s);
    let row = labels
        .iter()
        .position(|l| l == "Reset all instances")
        .unwrap_or_else(|| panic!("the row shows once the binding says so: {labels:?}"));

    // Clicking it sends NOTHING — it raises the confirm.
    let button = format!("DropDownList1Button{}", row + 1);
    let (cx, cy) = s
        .eval::<(f64, f64)>(&format!("return {button}:GetCenter()"))
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "LeftButton", true);
    s.mouse_button(cx as f32, cy as f32, "LeftButton", false);
    s.resolve();
    assert_eq!(
        s.take_reset_instance_asks(),
        0,
        "the row itself never sends — it asks"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the row raises CONFIRM_RESET_INSTANCES"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Do you really want to reset all of your instances?"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button1:GetText()")
            .unwrap(),
        "Yes"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button2:GetText()")
            .unwrap(),
        "No"
    );

    // No sends nothing; Yes sends exactly one.
    let (nx, ny) = s
        .eval::<(f64, f64)>("return StaticPopup1Button2:GetCenter()")
        .unwrap();
    s.mouse_button(nx as f32, ny as f32, "LeftButton", true);
    s.mouse_button(nx as f32, ny as f32, "LeftButton", false);
    s.resolve();
    assert_eq!(
        s.take_reset_instance_asks(),
        0,
        "No is not an answer that sends"
    );

    open_self_menu(&mut s);
    let labels = row_labels(&mut s);
    let row = labels
        .iter()
        .position(|l| l == "Reset all instances")
        .expect("still offered");
    let button = format!("DropDownList1Button{}", row + 1);
    let (cx, cy) = s
        .eval::<(f64, f64)>(&format!("return {button}:GetCenter()"))
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "LeftButton", true);
    s.mouse_button(cx as f32, cy as f32, "LeftButton", false);
    s.resolve();
    let (yx, yy) = s
        .eval::<(f64, f64)>("return StaticPopup1Button1:GetCenter()")
        .unwrap();
    s.mouse_button(yx as f32, yy as f32, "LeftButton", true);
    s.mouse_button(yx as f32, yy as f32, "LeftButton", false);
    s.resolve();
    assert_eq!(
        s.take_reset_instance_asks(),
        1,
        "Yes is the one call that sends CMSG_RESET_INSTANCES"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
