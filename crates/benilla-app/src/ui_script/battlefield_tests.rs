//! The stock battleground queue UI, `BattlefieldFrame.xml` and the minimap queue icon, driven by
//! pushed queue and list state.

use benilla_ui::script::{BattlefieldListView, BattlefieldMapInfo, BattlefieldQueueSlot, UiScript};

use super::test_ui::load_ui as load_xml;

/// The stock pair plus what they call: the panel kit, the confirm dialog, the icon's menu.
fn session() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The list verbs answer nothing without a local player, so seat one.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    s.run("function PlaySound() end").unwrap();
    // `FloatingChatFrame.lua:9`'s value, read by the icon's fade-in (`BattlefieldFrame.lua:142`).
    s.run("CHAT_FRAME_FADE_TIME = 0.15").unwrap();
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Localization.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\BattlefieldFrame.xml",
        "Interface\\FrameXML\\Minimap.xml",
    ] {
        load_xml(&s, f);
    }
    s.resolve();
    s
}

fn slot(map_id: u32, name: &str, status: u32, instance: u32) -> BattlefieldQueueSlot {
    BattlefieldQueueSlot {
        map_id,
        map_name: Some(name.into()),
        status,
        instance_id: instance,
        min_level: 20,
        max_level: 29,
        port_expiration_ms: 75_000,
        estimated_wait_ms: 90_000,
        time_waited_ms: 12_000,
    }
}

fn empty() -> BattlefieldQueueSlot {
    BattlefieldQueueSlot {
        map_name: Some("Eastern Kingdoms".into()),
        ..Default::default()
    }
}

fn visible(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsVisible()"))
        .unwrap()
}

#[test]
fn the_queue_icon_follows_the_slots_across_update_battlefield_status() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    assert!(!visible(&s, "MiniMapBattlefieldFrame"), "no queue at load");

    s.set_battlefield_queue(vec![slot(529, "Arathi Basin", 1, 0), empty(), empty()], 0);
    s.fire_event("UPDATE_BATTLEFIELD_STATUS", vec![]);
    assert!(
        visible(&s, "MiniMapBattlefieldFrame"),
        "a queued slot shows the icon"
    );
    let tooltip = s
        .eval::<String>("return MiniMapBattlefieldFrame.tooltip")
        .unwrap();
    assert!(
        tooltip.starts_with("You are in the queue for Arathi Basin\n"),
        "{tooltip}"
    );
    // `SecondsToTime` ends every unit with a space (`UIParent.lua:1023`).
    assert!(
        tooltip.contains("Average wait time: 1 Min  (Last 10 players)"),
        "{tooltip}"
    );
    assert!(tooltip.contains("Time in queue: 12 Secs \n"), "{tooltip}");
    assert!(
        tooltip.ends_with("|cffffffff<Right Click> for PvP Options|r"),
        "{tooltip}"
    );
    assert!(!visible(&s, "StaticPopup1"), "queued is not yet a question");

    // Status 2, ready to enter: the confirm dialog and the port countdown.
    s.set_battlefield_queue(vec![slot(529, "Arathi Basin", 2, 3), empty(), empty()], 0);
    s.fire_event("UPDATE_BATTLEFIELD_STATUS", vec![]);
    assert!(
        visible(&s, "StaticPopup1"),
        "confirm raises CONFIRM_BATTLEFIELD_ENTRY"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "You are now eligible to enter Arathi Basin 3, choose an action:"
    );
    let tooltip = s
        .eval::<String>("return MiniMapBattlefieldFrame.tooltip")
        .unwrap();
    assert!(
        tooltip.starts_with(
            "You are eligible to enter Arathi Basin 3\nYou will be removed from the queue in 1 Min 15 Secs "
        ),
        "{tooltip}"
    );
    // Enter Battle calls `AcceptBattlefieldPort(slot, 1)` (`StaticPopup.lua:101`).
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(s.take_battlefield_port_requests(), vec![(1, true)]);

    s.set_battlefield_queue(vec![empty(), empty(), empty()], 0);
    s.fire_event("UPDATE_BATTLEFIELD_STATUS", vec![]);
    assert!(
        !visible(&s, "MiniMapBattlefieldFrame"),
        "no queue hides the icon"
    );
}

#[test]
fn the_list_window_opens_on_battlefields_show_and_joins_the_selection() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    s.set_battlefield_list(BattlefieldListView {
        instances: vec![4, 9],
        bracket_min: 20,
        bracket_max: 29,
        info: Some(BattlefieldMapInfo {
            name: "Arathi Basin".into(),
            description: Some("The Arathi Basin is one of the main staging points.".into()),
            min_level: 20,
            max_level: 60,
            field_16: -1,
            field_17: 0.0,
            field_18: 0.0,
        }),
        group_queue: true,
    });
    s.set_battlefield_queue(vec![empty(), empty(), empty()], 0);
    assert!(!visible(&s, "BattlefieldFrame"));
    s.fire_event("BATTLEFIELDS_SHOW", vec![]);
    assert!(visible(&s, "BattlefieldFrame"), "the event shows the panel");
    assert_eq!(
        s.eval::<String>("return BattlefieldFrameFrameLabel:GetText()")
            .unwrap(),
        "Arathi Basin"
    );
    assert_eq!(
        s.eval::<String>("return BattlefieldZone1:GetText()")
            .unwrap(),
        "First Available"
    );
    assert_eq!(
        s.eval::<String>("return BattlefieldZone2:GetText()")
            .unwrap(),
        "Arathi Basin 4"
    );
    assert_eq!(
        s.eval::<String>("return BattlefieldZone3:GetText()")
            .unwrap(),
        "Arathi Basin 9"
    );
    assert!(
        !visible(&s, "BattlefieldZone4"),
        "two instances, three rows"
    );
    assert_eq!(
        s.eval::<String>("return BattlefieldFrameZoneDescription:GetText()")
            .unwrap(),
        "The Arathi Basin is one of the main staging points."
    );
    assert!(
        visible(&s, "BattlefieldFrameGroupJoinButton"),
        "the map allows group queues: the button shows"
    );
    assert!(
        !s.eval::<bool>("return BattlefieldFrameGroupJoinButton:IsEnabled() ~= 0")
            .unwrap(),
        "…disabled while not leading a group"
    );

    // Row 1, the default selection, is First Available and joins instance 0.
    s.run("BattlefieldFrameJoinButton:Click()").unwrap();
    assert_eq!(s.take_battlefield_join_requests(), vec![(0, false)]);
    assert!(!visible(&s, "BattlefieldFrame"), "joining hides the panel");
    s.fire_event("BATTLEFIELDS_SHOW", vec![]);
    s.run("BattlefieldZone3:Click()").unwrap();
    assert_eq!(s.eval::<i64>("return GetSelectedBattlefield()").unwrap(), 2);
    s.run("BattlefieldFrameJoinButton:Click()").unwrap();
    assert_eq!(s.take_battlefield_join_requests(), vec![(9, false)]);

    s.fire_event("BATTLEFIELDS_SHOW", vec![]);
    assert!(visible(&s, "BattlefieldFrame"));
    s.fire_event("BATTLEFIELDS_CLOSED", vec![]);
    assert!(
        !visible(&s, "BattlefieldFrame"),
        "the leash's event hides it"
    );
}

#[test]
fn the_list_rows_carry_the_queue_status() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    s.set_battlefield_list(BattlefieldListView {
        instances: vec![4],
        info: Some(BattlefieldMapInfo {
            name: "Arathi Basin".into(),
            ..Default::default()
        }),
        ..Default::default()
    });
    s.set_battlefield_queue(vec![slot(529, "Arathi Basin", 1, 4), empty(), empty()], 0);
    s.fire_event("BATTLEFIELDS_SHOW", vec![]);
    assert_eq!(
        s.eval::<String>("return BattlefieldZone2Status:GetText()")
            .unwrap(),
        "(In Queue)"
    );
    assert_eq!(
        s.eval::<Option<String>>("return BattlefieldZone1Status:GetText()")
            .unwrap(),
        None,
        "an un-queued row's status line is blank, and a blank FontString reads back NIL \
         (`FontString:GetText 0x79d690` substitutes nil)"
    );
}
