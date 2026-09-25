//! The instance-lockout verbs and their one stock caller, the SELF menu's reset row and its
//! confirm. The family's chat lines have no Lua; `crate::ui_instance` composes and tests them.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// `IsInInstance` returns a pair; it and `CanShowResetInstances` answer 1 or nil, not booleans.
#[test]
fn the_three_bindings_have_the_reference_shapes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();

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
        // Type 4 and up fall past the reference's `cmp esi,4; jae` guard.
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

/// Through the real hit test: right-click the player portrait, click the row, answer the confirm.
#[test]
fn the_self_menu_row_gates_on_the_binding_and_confirms_before_sending() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // Verbatim 1.12 values from `GlobalStrings.lua`.
    s.run(
        r#"
        -- The stock raid pane concatenates this into each of its eight group headers inside
        -- their own OnLoad, so it has to exist before the addon loads.
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
        "Interface\\FrameXML\\LocaleProperties.lua", // `GetText`, the gender and plural lookup
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
    // Blizzard_RaidUI is LoadOnDemand: seated as an addon row, as the app does, then loaded by
    // `RaidFrame_LoadUI` (`UIParent.lua:182`).
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

    // Off, the row is hidden, not greyed (`UnitPopup.lua:379`). Solo it is the SELF menu's only
    // candidate row, so with it gone the menu does not open at all.
    open_self_menu(&mut s);
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "solo with no lockout to reset, the SELF menu has nothing to show"
    );

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
