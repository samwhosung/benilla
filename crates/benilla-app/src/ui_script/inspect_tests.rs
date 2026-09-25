//! The stock `Blizzard_InspectUI` window, engine-only, fed a synthetic target and a foreign
//! equipment view: the range gate at its thresholds, the doll reading the inspected unit's gear
//! rather than ours, and the window's own lifecycle.

use std::collections::HashMap;

use benilla_ui::script::{
    InspectView, InvSlotView, InventorySlots, QuadContent, SoundRequest, UiScript, UnitReach,
    UnitState,
};

/// A live, inspectable unit at squared distance `dist_sq`. `inspectable` carries the server's
/// non-distance refusals and only `CanInspect` reads it; these tests are about distance.
fn reach(dist_sq: f64) -> UnitReach {
    UnitReach {
        dist_sq,
        inspectable: true,
    }
}

use super::test_ui::load_ui as load_xml;

/// The inspected player: a Dwarf Paladin, level 34.
fn target_unit() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Thargrim".into()),
        health: 900,
        max_health: 900,
        level: 34,
        race: Some("Dwarf".into()),
        race_file: Some("Dwarf".into()),
        class: Some("Paladin".into()),
        class_file: Some("PALADIN".into()),
        sex: 2,
        is_player: true,
        player_controlled: true,
        ..Default::default()
    }
}

/// A slot view as the inspect feed builds one: entry, icon, name and quality, with nothing an item
/// object would add (no stack count, durability or lock).
fn foreign_slot(entry: u32, icon: &str, name: &str) -> InvSlotView {
    InvSlotView {
        item_id: entry,
        icon: Some(icon.into()),
        count: 1,
        quality: 3,
        name: Some(name.into()),
        link: Some(format!("|cff0070dd|Hitem:{entry}:0:0:0|h[{name}]|h|r")),
        ..Default::default()
    }
}

/// The inspected unit's equipment: a helm in the head slot (id 1) and nothing else.
fn inspect_view(unit: &str) -> InspectView {
    let mut slots: InventorySlots = Default::default();
    slots[1] = Some(foreign_slot(
        7365,
        "Interface\\Icons\\INV_Helmet_09",
        "Mighty Helm",
    ));
    InspectView {
        unit: unit.into(),
        guid: 0x0000_0000_0000_0042,
        slots,
    }
}

/// Our own equipment: a different helm in the same slot, so a doll reading the wrong source shows.
fn own_slots() -> InventorySlots {
    let mut slots: InventorySlots = Default::default();
    slots[1] = Some(foreign_slot(
        1234,
        "Interface\\Icons\\INV_Helmet_01",
        "My Own Helm",
    ));
    slots
}

/// Whether a texture whose path contains `needle` is drawn this frame.
fn drawn(s: &mut UiScript, needle: &str) -> bool {
    s.resolve();
    s.extract().iter().any(
        |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle)),
    )
}

/// Everything the window needs to open on `"target"`, in range.
fn armed() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // `PLAYER_LEVEL`, the template the stock level line formats through.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `TEXT`, which the stock `InspectPaperDollFrame_SetLevel` formats its level line through.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    // The inspect window's tabs inherit its tab template.
    load_xml(&s, r"Interface\FrameXML\CharacterFrameTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    // Before the window, as `inherits=` resolves at load: the slot buttons inherit
    // `ItemButtonTemplate`, the honor page the honor row templates `HonorFrame.xml` brings.
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\HonorFrame.xml");
    // A LoadOnDemand addon, seated off the chain as a registry row and loaded by the stock
    // `InspectFrame_LoadUI` (`UIParent.lua:170`), which `InspectUnit` calls (`UIParent.lua:223`).
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_InspectUI");
    s.run("InspectFrame_LoadUI()").unwrap();
    s.set_unit("target", Some(target_unit()));
    s.set_inspect(Some(inspect_view("target")));
    // 4 yards away (d² = 16), inside the 10-yard gate (100.0).
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(16.0))]));
    s
}

/// The window, its 19 slots and the model pane load with no errors.
#[test]
fn shipped_inspect_frame_loads_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    // The files `armed` loads, in the same order.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\CharacterFrameTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\HonorFrame.xml");
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_InspectUI");
    s.run("InspectFrame_LoadUI()").unwrap();
    // Each slot carries its `GetInventorySlotInfo` id (1..=19, no ammo slot) as its frame ID, set
    // by the stock `InspectPaperDollItemSlotButton_OnLoad`.
    for (name, id) in [
        ("InspectHeadSlot", 1),
        ("InspectBackSlot", 15),
        ("InspectMainHandSlot", 16),
        ("InspectRangedSlot", 18),
        ("InspectTabardSlot", 19),
    ] {
        assert_eq!(
            s.eval::<i64>(&format!("return {name}:GetID()")).unwrap(),
            id,
            "{name} inventory slot id"
        );
    }
}

/// Out of range, `InspectUnit` neither requests nor opens; in range it does both.
#[test]
fn inspect_unit_refuses_out_of_range() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();
    // 11 yards (d² = 121), past the 100.0 threshold.
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(121.0))]));

    s.run(r#"InspectUnit("target")"#).unwrap();
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "out of range: the window must not open"
    );
    assert!(
        s.take_inspect_notifies().is_empty(),
        "out of range: no CMSG_INSPECT request is queued"
    );

    s.set_unit_reach(HashMap::from([("target".to_string(), reach(99.9))]));
    s.run(r#"InspectUnit("target")"#).unwrap();
    assert!(
        s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "in range: the window opens"
    );
    assert_eq!(
        s.take_inspect_notifies(),
        vec!["target".to_string()],
        "in range: NotifyInspect queued the token for the app to resolve"
    );
}

/// `CanInspect` refuses only when `100.0 < d²` (`0x48a28b`), so exactly 100.0 is in range;
/// `CheckInteractDistance` admits only when `d² < table[type - 1]` (`0x48bb0a`), so 100.0 is out.
#[test]
fn range_predicates_transcribe_the_verified_thresholds() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();
    for (d2, can_inspect, interact1) in [
        (99.9_f64, true, true),
        (100.0, true, false),
        (100.1, false, false),
    ] {
        s.set_unit_reach(HashMap::from([("target".to_string(), reach(d2))]));
        assert_eq!(
            s.eval::<bool>(r#"return CanInspect("target") ~= nil"#)
                .unwrap(),
            can_inspect,
            "CanInspect at d²={d2}"
        );
        assert_eq!(
            s.eval::<bool>(r#"return CheckInteractDistance("target", 1) ~= nil"#)
                .unwrap(),
            interact1,
            "CheckInteractDistance(type 1) at d²={d2}"
        );
    }
    // Type 4 is the 30-yard row (900.0).
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(899.0))]));
    assert!(s
        .eval::<bool>(r#"return CheckInteractDistance("target", 4) ~= nil"#)
        .unwrap());
    assert!(
        !s.eval::<bool>(r#"return CheckInteractDistance("target", 1) ~= nil"#)
            .unwrap(),
        "the same distance is out of range for the 10-yard type"
    );
    // A token with no unit in the object manager answers nil (`0x48babe`): a party member outside
    // the local area has a GUID from the roster but no object.
    assert!(
        s.eval::<bool>(r#"return CheckInteractDistance("party3", 1) == nil"#)
            .unwrap(),
        "a token with no live unit is out of range, not in"
    );
    assert!(
        s.eval::<bool>(r#"return CanInspect("party3") == nil"#)
            .unwrap(),
        "…and CanInspect agrees, through its own null-`this` tail"
    );

    // The `type` argument's degenerate arms, each the reference's answer.
    s.set_unit_reach(HashMap::from([("target".to_string(), reach(1.0))]));
    for bad in ["0", "5", "-1", "0.5"] {
        assert!(
            s.eval::<bool>(&format!(
                r#"return CheckInteractDistance("target", {bad}) == nil"#
            ))
            .unwrap(),
            "type {bad} is outside the table (unsigned compare on trunc(type) − 1)"
        );
    }
    // A fractional type inside the range truncates toward zero and answers its row.
    assert!(
        s.eval::<bool>(r#"return CheckInteractDistance("target", 1.9) ~= nil"#)
            .unwrap(),
        "1.9 chops to 1"
    );
    // A missing `type` is a usage error, not nil: the reference raises through `luaL_error`.
    assert!(
        s.eval::<bool>(r#"return CheckInteractDistance("target") ~= nil"#)
            .is_err(),
        "no distIndex is a script error"
    );

    // The token is case-folded, as every compare in the unit resolver is (`_strnicmp`).
    assert!(
        s.eval::<bool>(r#"return CheckInteractDistance("TARGET", 1) ~= nil"#)
            .unwrap(),
        "an upper-case token names the same unit"
    );
    assert!(
        s.eval::<bool>(r#"return CanInspect("Target") ~= nil"#)
            .unwrap(),
        "…for both predicates"
    );
}

/// The inspected token reads the foreign view, and `"player"` still reads our own equipment.
#[test]
fn inventory_bindings_route_by_unit_token() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();
    s.set_inventory_slots(own_slots());

    assert_eq!(
        s.eval::<String>(r#"return GetInventoryItemTexture("target", 1)"#)
            .unwrap(),
        "Interface\\Icons\\INV_Helmet_09",
        "the inspected token reads the foreign visible-item view"
    );
    assert_eq!(
        s.eval::<String>(r#"return GetInventoryItemTexture("player", 1)"#)
            .unwrap(),
        "Interface\\Icons\\INV_Helmet_01",
        "\"player\" still reads our own equipped feed"
    );
    // A token nobody is inspecting has no equipment source at all.
    assert!(s
        .eval::<bool>(r#"return GetInventoryItemTexture("party2", 1) == nil"#)
        .unwrap());
    // The view answers only for the token it was built for.
    s.set_inspect(Some(inspect_view("party1")));
    assert!(
        s.eval::<bool>(r#"return GetInventoryItemTexture("target", 1) == nil"#)
            .unwrap(),
        "a view built for \"party1\" must not answer for \"target\""
    );
}

/// One drive through the window: the open sound, the name and level lines, the slot icons, the
/// `UNIT_INVENTORY_CHANGED` repaint, the rotate buttons and `ClearInspectPlayer` on close.
#[test]
fn shipped_inspect_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();

    assert!(!s.eval::<bool>("return InspectFrame:IsVisible()").unwrap());
    assert!(s.take_sounds().is_empty());
    // `InspectModelFrame_OnLoad` sets the default facing, 0.61.
    assert!(
        (s.model_pane_facing("InspectModelFrame") - 0.61).abs() < 0.0001,
        "default facing 0.61, got {}",
        s.model_pane_facing("InspectModelFrame")
    );

    s.run(r#"InspectUnit("target")"#).unwrap();
    assert!(s.errors().is_empty(), "open errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return InspectFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoOpen".into())],
        "opening plays igCharacterInfoOpen"
    );

    assert_eq!(
        s.eval::<String>("return InspectNameText:GetText()")
            .unwrap(),
        "Thargrim",
        "the name line reads UnitName of the inspected token"
    );
    assert_eq!(
        s.eval::<String>("return InspectLevelText:GetText()")
            .unwrap(),
        "Level 34 Dwarf Paladin",
        "the level line reads the inspected unit, not the player"
    );
    assert!(
        drawn(&mut s, "INV_Helmet_09"),
        "the head slot paints the inspected item's icon"
    );
    assert!(
        drawn(&mut s, "UI-PaperDoll-Slot-Chest"),
        "an empty slot falls back to its own paper-doll art"
    );

    // The feed pushes a changed view and fires the event the slot buttons filter on `arg1`.
    let mut swapped = inspect_view("target");
    swapped.slots[1] = Some(foreign_slot(
        7366,
        "Interface\\Icons\\INV_Helmet_22",
        "Better Helm",
    ));
    s.set_inspect(Some(swapped));
    s.fire_event(
        "UNIT_INVENTORY_CHANGED",
        vec![benilla_ui::script::ScriptValue::Str("target".into())],
    );
    assert!(
        drawn(&mut s, "INV_Helmet_22"),
        "UNIT_INVENTORY_CHANGED for the inspected unit repaints the slot"
    );

    let mut other = inspect_view("target");
    other.slots[1] = Some(foreign_slot(1, "Interface\\Icons\\INV_Helmet_ZZ", "Nope"));
    s.set_inspect(Some(other));
    s.fire_event(
        "UNIT_INVENTORY_CHANGED",
        vec![benilla_ui::script::ScriptValue::Str("party4".into())],
    );
    assert!(
        drawn(&mut s, "INV_Helmet_22") && !drawn(&mut s, "INV_Helmet_ZZ"),
        "an event for another unit leaves the doll alone"
    );

    // Rotate: left subtracts 0.03 from the booth yaw and right adds it, each click with its sound.
    s.run("InspectModelRotateLeftButton:Click()").unwrap();
    assert!(
        (s.model_pane_facing("InspectModelFrame") - 0.58).abs() < 0.0001,
        "rotate-left subtracts 0.03, got {}",
        s.model_pane_facing("InspectModelFrame")
    );
    s.run("InspectModelRotateRightButton:Click()").unwrap();
    s.run("InspectModelRotateRightButton:Click()").unwrap();
    assert!(
        (s.model_pane_facing("InspectModelFrame") - 0.64).abs() < 0.0001,
        "rotate-right adds 0.03, got {}",
        s.model_pane_facing("InspectModelFrame")
    );
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igInventoryRotateCharacter".into()),
            SoundRequest::KitName("igInventoryRotateCharacter".into()),
            SoundRequest::KitName("igInventoryRotateCharacter".into()),
        ],
        "each rotate click plays the kit"
    );

    // Closing plays its sound and calls `ClearInspectPlayer`, which stops the app's inspect feed.
    assert!(
        !s.take_inspect_clear(),
        "ClearInspectPlayer not called while open"
    );
    s.run(r#"HideUIPanel(InspectFrame)"#).unwrap();
    assert!(!s.eval::<bool>("return InspectFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoClose".into())],
        "closing plays igCharacterInfoClose"
    );
    assert!(
        s.take_inspect_clear(),
        "closing called ClearInspectPlayer (the ref's InspectFrame_OnHide)"
    );
    assert!(s.errors().is_empty(), "close errors: {:?}", s.errors());
}

/// `PanelTemplates_SelectTab` disables the selected tab (`UIPanelTemplates.lua:125`), so clicking
/// it does nothing; `ToggleInspect`'s close-when-showing branch (`Blizzard_InspectUI.lua:67-83`)
/// is reached only by calling it directly.
#[test]
fn the_active_tab_is_inert_and_toggle_inspect_closes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = armed();
    s.run(r#"InspectUnit("target")"#).unwrap();
    let _ = s.take_sounds();

    // `IsEnabled` answers the number 1 or 0 (`0x7800b0`), and 0 is truthy in Lua.
    assert!(
        !s.eval::<bool>("return InspectFrameTab1:IsEnabled() ~= 0")
            .unwrap(),
        "the selected tab is disabled (PanelTemplates_SelectTab), so its click is inert"
    );
    s.run("InspectFrameTab1:Click()").unwrap();
    assert!(
        s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "clicking the disabled active tab changes nothing"
    );

    s.run(r#"ToggleInspect("InspectPaperDollFrame")"#).unwrap();
    assert!(
        !s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "ToggleInspect on the showing page closes the window (ref ToggleInspect)"
    );
    assert!(s.errors().is_empty(), "tab errors: {:?}", s.errors());
}

/// Re-opening through `ToggleInspect` after a close raises, as in the reference:
/// `InspectFrame_OnHide` clears `this.unit`, so `OnShow` runs with no token, and
/// `SetPortraitTexture` (`0x519ef0`) raises its usage error at its `lua_isstring` gate
/// (`0x519fb4`), before the portrait is touched. Stock 1.12 has no inspect binding and no caller
/// outside the hidden window's own tabs, so only an addon or a macro gets here.
#[test]
fn toggle_inspect_reopens_after_a_close_and_raises_like_the_reference() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = armed();
    s.run(r#"InspectUnit("target")"#).unwrap();
    s.run(r#"ToggleInspect("InspectPaperDollFrame")"#).unwrap();
    assert!(!s.eval::<bool>("return InspectFrame:IsVisible()").unwrap());
    let _ = s.errors();

    s.run(r#"ToggleInspect("InspectPaperDollFrame")"#).unwrap();
    assert!(
        s.eval::<bool>("return InspectFrame:IsVisible()").unwrap(),
        "the window still shows — Show() runs before OnShow raises"
    );
    // Two raises: `InspectFrame` and `InspectPaperDollFrame` each run their own `OnShow` under
    // their own pcall, the first dying at the portrait's gate, the second at `UnitLevel`'s.
    let errors = s.errors();
    assert_eq!(errors.len(), 2, "one raise per OnShow handler: {errors:?}");
    assert!(
        errors[0].contains(r#"Usage: SetPortraitTexture(texture, "unit")"#),
        "the portrait's gate, before the name line is ever reached: {errors:?}"
    );
    assert!(
        errors[1].contains(r#"Usage: UnitLevel("unit")"#),
        "and the paper doll's own handler, dying at its first getter: {errors:?}"
    );
}
