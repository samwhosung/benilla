//! The social window's raid tab (`RaidFrame.xml` and `Blizzard_RaidUI`): the 8x5 grid's seating
//! and colouring, the drag, the row menu, the ready check and the saved-instance panel.

use benilla_ui::script::{
    PartyMemberInfo, PartyRequest, PartyState, RaidMemberInfo, SavedInstanceInfo, SelectionRequest,
    UiScript,
};

fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_social_ui(&mut s);
    s
}

fn visible(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsVisible()"))
        .unwrap()
}

fn text_of(s: &UiScript, region: &str) -> String {
    s.eval::<String>(&format!("return {region}:GetText() or \"\""))
        .unwrap()
}

/// One raid row; `rank` is 2 leader, 1 assistant, 0 member. `subgroup` is the 1-based number the
/// pane shows: the record stores it 0-based and the binding adds one (`0x4bb61a`), so a 1-based
/// record would seat every row one group to the right.
fn row(
    name: &str,
    rank: u32,
    subgroup: u32,
    class: &str,
    online: bool,
    dead: bool,
) -> RaidMemberInfo {
    RaidMemberInfo {
        name: name.to_string(),
        guid: 0xF000 + u64::from(subgroup) * 16 + name.len() as u64,
        rank,
        subgroup: subgroup - 1,
        level: 60,
        class: Some(class.to_string()),
        class_file: Some(class.to_uppercase()),
        zone: Some("Molten Core".to_string()),
        online,
        ninth: dead,
    }
}

/// Pushes a roster and fires its event, as `ui_party`'s `feed_party` does.
fn push_raid(s: &mut UiScript, raid: Vec<RaidMemberInfo>) {
    // `members` is our own subgroup's slice; non-empty, it makes `IsPartyLeader()` and
    // `IsRaidLeader()` answer 1 for `leader_index` 0.
    let members = raid
        .iter()
        .skip(1)
        .take(4)
        .map(|r| PartyMemberInfo {
            name: r.name.clone(),
            guid: r.guid,
        })
        .collect();
    s.set_party(PartyState {
        members,
        leader_index: 0,
        leader_guid: 0, // the player leads; their guid is unset in this fixture
        own_guid: 0,
        raid,
        loot_method: "group".into(),
        master_looter: None,
        loot_threshold: 2,
    });
    s.fire_event("RAID_ROSTER_UPDATE", Vec::new());
}

/// A 12-member raid across three subgroups, led by us, with row 2 an assistant.
fn twelve() -> Vec<RaidMemberInfo> {
    let mut raid = vec![row("Me", 2, 1, "Warrior", true, false)];
    for i in 1..12u32 {
        let subgroup = i / 4 + 1;
        raid.push(row(
            &format!("Member{i}"),
            if i == 1 { 1 } else { 0 },
            subgroup,
            "Priest",
            true,
            false,
        ));
    }
    raid
}

/// Opens the tab and drains the `RequestRaidInfo()` its OnShow sends (`RaidFrame.xml:350`).
fn open_raid_tab(s: &mut UiScript) {
    s.run("ToggleFriendsFrame(4)").unwrap();
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::RequestRaidInfo],
        "showing the pane asks the server for our lockouts"
    );
}

// ── The tab ──────────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_fourth_tab_opens_the_raid_pane() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    assert_eq!(
        s.eval::<i64>("return FriendsFrame.numTabs").unwrap(),
        4,
        "PanelTemplates_SetNumTabs says four now — it said three while the pane did not exist"
    );
    open_raid_tab(&mut s);
    assert!(visible(&s, "RaidFrame"), "the pane opens");
    assert_eq!(text_of(&s, "FriendsFrameTitleText"), "Raid");
    for other in [
        "FriendsListFrame",
        "IgnoreListFrame",
        "WhoFrame",
        "GuildFrame",
    ] {
        assert!(!visible(&s, other), "{other} goes away");
    }
    s.run("FriendsFrameTab1:Click()").unwrap();
    assert!(!visible(&s, "RaidFrame"));
    s.run("FriendsFrameTab4:Click()").unwrap();
    assert!(visible(&s, "RaidFrame"), "the tab button opens it too");
}

/// `FriendsFrame_OnHide` hides `RaidInfoFrame` too (`FriendsFrame.lua:136`).
#[test]
fn hiding_the_window_closes_the_raid_info_panel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    s.run("RaidInfoFrame:Show()").unwrap();
    assert!(visible(&s, "RaidInfoFrame"));
    s.run("HideUIPanel(FriendsFrame)").unwrap();
    assert!(
        !visible(&s, "RaidInfoFrame"),
        "the flyout goes with the window"
    );
}

// ── The not-in-a-raid state ──────────────────────────────────────────────────────────────────────

/// Out of a raid the pane is the blurb and Convert To Raid, live only for a party's leader
/// (`RaidFrame.lua:62-69`).
#[test]
fn convert_to_raid_is_live_only_for_a_party_leader() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    assert!(visible(&s, "RaidFrameRaidDescription"), "solo: the blurb");
    assert!(visible(&s, "RaidFrameConvertToRaidButton"));
    assert_eq!(
        s.eval::<i64>("return RaidFrameConvertToRaidButton:IsEnabled()")
            .unwrap(),
        0,
        "solo there is no party to convert"
    );
    assert!(
        !visible(&s, "RaidFrameReadyCheckButton"),
        "and no raid verbs"
    );
    assert!(!visible(&s, "RaidFrameAddMemberButton"));

    // A party we lead: `leader_index == 0` with members.
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Alice".into(),
            guid: 0xA,
        }],
        leader_index: 0,
        ..Default::default()
    });
    s.fire_event("PARTY_MEMBERS_CHANGED", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameConvertToRaidButton:IsEnabled()")
            .unwrap(),
        1,
        "a party leader can convert"
    );
    s.run("RaidFrameConvertToRaidButton:Click()").unwrap();
    assert!(
        s.take_party_requests()
            .contains(&PartyRequest::ConvertToRaid),
        "and the button really sends it"
    );

    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Alice".into(),
            guid: 0xA,
        }],
        leader_index: 1,
        ..Default::default()
    });
    s.fire_event("PARTY_LEADER_CHANGED", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameConvertToRaidButton:IsEnabled()")
            .unwrap(),
        0,
        "a party member cannot"
    );
}

// ── The grid ─────────────────────────────────────────────────────────────────────────────────────

/// Row order and slot order differ; every drag, kick and menu action addresses a row by row index.
#[test]
fn the_grid_seats_each_row_in_its_own_subgroups_next_free_slot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());

    // Groups 1-3 hold 4/4/4; the roster is 12 with us at row 1 in group 1.
    assert_eq!(
        s.eval::<String>("return RaidGroupButton1.slot").unwrap(),
        "RaidGroup1Slot1",
        "row 1 (us) takes group 1's first seat"
    );
    assert_eq!(
        s.eval::<String>("return RaidGroupButton5.slot").unwrap(),
        "RaidGroup2Slot1",
        "row 5 is the first of group 2 — NOT group 1's fifth seat"
    );
    assert_eq!(
        s.eval::<String>("return RaidGroupButton12.slot").unwrap(),
        "RaidGroup3Slot4"
    );
    // The slot knows its occupant, which the drop reads to choose move or swap.
    assert_eq!(
        s.eval::<String>("return RaidGroup2Slot1.button").unwrap(),
        "RaidGroupButton5"
    );

    assert_eq!(text_of(&s, "RaidGroupButton1Name"), "Me");
    assert_eq!(
        text_of(&s, "RaidGroupButton1Rank"),
        "(L)",
        "the leader token"
    );
    assert_eq!(
        text_of(&s, "RaidGroupButton2Rank"),
        "(A)",
        "the assistant token"
    );
    assert_eq!(
        text_of(&s, "RaidGroupButton3Rank"),
        "",
        "and a plain member has none"
    );
    assert_eq!(text_of(&s, "RaidGroupButton1Level"), "60");

    assert!(visible(&s, "RaidGroupButton12"), "the last real row shows");
    assert!(
        !visible(&s, "RaidGroupButton13"),
        "and the first empty one does not"
    );
    assert!(
        visible(&s, "RaidGroup8"),
        "all eight groups show while in a raid"
    );

    push_raid(&mut s, Vec::new());
    assert!(!visible(&s, "RaidGroup1"));
    assert!(!visible(&s, "RaidGroupButton1"));
    assert!(visible(&s, "RaidFrameRaidDescription"));
}

/// Offline grey is tested first, then dead red, then the class colour, on all three columns
/// (`Blizzard_RaidUI.lua:135-152`).
#[test]
fn a_rows_colour_is_offline_then_dead_then_class() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(
        &mut s,
        vec![
            row("Me", 2, 1, "Warrior", true, false),
            row("Corpse", 0, 1, "Mage", true, true),
            row("Gone", 0, 1, "Mage", false, false),
            row("GoneDead", 0, 1, "Mage", false, true),
        ],
    );
    let color = |button: &str| {
        s.eval::<(f32, f32, f32)>(&format!(
            "local r, g, b = {button}Name:GetTextColor() return r, g, b"
        ))
        .unwrap()
    };
    let round = |(r, g, b): (f32, f32, f32)| {
        (
            (r * 100.0).round() / 100.0,
            (g * 100.0).round() / 100.0,
            (b * 100.0).round() / 100.0,
        )
    };
    // WARRIOR is 0.78, 0.61, 0.43 (`RAID_CLASS_COLORS`, `Fonts.xml:52`).
    let warrior: (f32, f32, f32) = s
        .eval("return RAID_CLASS_COLORS.WARRIOR.r, RAID_CLASS_COLORS.WARRIOR.g, RAID_CLASS_COLORS.WARRIOR.b")
        .unwrap();
    assert_eq!(
        round(color("RaidGroupButton1")),
        round(warrior),
        "alive: class colour"
    );
    assert_eq!(
        round(color("RaidGroupButton2")),
        (1.0, 0.1, 0.1),
        "dead: RED_FONT_COLOR"
    );
    assert_eq!(
        round(color("RaidGroupButton3")),
        (0.5, 0.5, 0.5),
        "offline: GRAY_FONT_COLOR"
    );
    assert_eq!(
        round(color("RaidGroupButton4")),
        (0.5, 0.5, 0.5),
        "offline AND dead is grey — offline is tested first"
    );
    // Name and level share the row's colour; the class column is a Button, whose text colour
    // this engine cannot read back.
    assert_eq!(
        round(color("RaidGroupButton2")),
        round(
            s.eval::<(f32, f32, f32)>(
                "local r, g, b = RaidGroupButton2Level:GetTextColor() return r, g, b"
            )
            .unwrap()
        ),
        "name and level share the row's colour"
    );
}

/// `UNIT_HEALTH` and `UNIT_LEVEL` repaint one `raidN` row without the roster event
/// (`Blizzard_RaidUI.lua:27-38`).
#[test]
fn a_units_health_and_level_repaint_only_that_row() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    let mut raid = twelve();
    push_raid(&mut s, raid.clone());
    assert_eq!(text_of(&s, "RaidGroupButton3Level"), "60");

    // The row dies; only the per-unit event fires.
    raid[2].ninth = true;
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Member1".into(),
            guid: raid[1].guid,
        }],
        leader_index: 0,
        raid: raid.clone(),
        ..Default::default()
    });
    s.fire_event(
        "UNIT_HEALTH",
        vec![benilla_ui::script::ScriptValue::Str("raid3".into())],
    );
    let red = s
        .eval::<f32>("local r, g = RaidGroupButton3Name:GetTextColor() return g")
        .unwrap();
    assert!(red < 0.2, "row 3 went red without a roster rebuild ({red})");

    // A unit event for a token that is not `raidN` is ignored.
    s.fire_event(
        "UNIT_HEALTH",
        vec![benilla_ui::script::ScriptValue::Str("target".into())],
    );
    s.fire_event(
        "UNIT_LEVEL",
        vec![benilla_ui::script::ScriptValue::Str("player".into())],
    );
    assert_eq!(
        text_of(&s, "RaidGroupButton3Level"),
        "60",
        "and nothing else moved"
    );
}

// ── The management buttons ───────────────────────────────────────────────────────────────────────

/// Out of a raid both hide, sharing a seat with Convert To Raid (`Blizzard_RaidUI.lua:653-672`).
#[test]
fn ready_check_is_the_leaders_and_add_member_is_everyones() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());
    assert!(
        visible(&s, "RaidFrameReadyCheckButton"),
        "we lead, so we may ask"
    );
    assert!(visible(&s, "RaidFrameAddMemberButton"));
    assert!(
        !visible(&s, "RaidFrameConvertToRaidButton"),
        "already a raid"
    );
    assert!(!visible(&s, "RaidFrameRaidDescription"));

    s.run("RaidFrameReadyCheckButton:Click()").unwrap();
    assert!(s
        .take_party_requests()
        .contains(&PartyRequest::ReadyCheckStart));

    let raid = twelve();
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Member1".into(),
            guid: raid[1].guid,
        }],
        leader_index: 1,
        raid,
        ..Default::default()
    });
    s.fire_event("RAID_ROSTER_UPDATE", Vec::new());
    assert!(!visible(&s, "RaidFrameReadyCheckButton"));
    assert!(
        visible(&s, "RaidFrameAddMemberButton"),
        "but Add Member still shows"
    );
}

// ── The drag ─────────────────────────────────────────────────────────────────────────────────────

/// An empty slot in another group moves, an occupied one swaps, anything else springs the row home
/// (`Blizzard_RaidUI.lua:257-273`); `TARGET_RAID_SLOT` stands in for the hover sweep.
#[test]
fn dragging_a_row_moves_swaps_or_springs_back() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());

    let drag = |s: &UiScript, button: &str, onto: &str| {
        s.run(&format!(
            "MOVING_RAID_MEMBER = {button}; TARGET_RAID_SLOT = {onto}; \
             RaidGroupButton_OnDragStop({button})"
        ))
        .unwrap();
    };

    // Row 12 (group 3, seat 4) onto group 3's fifth seat, its own group: nothing goes out.
    drag(&s, "RaidGroupButton12", "RaidGroup3Slot5");
    assert!(
        s.take_party_requests().is_empty(),
        "a drop inside the row's own group is not a move"
    );

    // Row 12 onto group 5's empty first seat: a move, with the 1-based subgroup.
    drag(&s, "RaidGroupButton12", "RaidGroup5Slot1");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::SetSubgroup {
            index: 12,
            group: 5
        }]
    );

    // Row 12 onto group 1's second seat, which row 2 holds: a swap, by the two row indices.
    drag(&s, "RaidGroupButton12", "RaidGroup1Slot2");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::SwapSubgroup {
            index: 12,
            other: 2
        }]
    );
}

/// `OnDragStart` and `OnDragStop` both return for a non-leader (`Blizzard_RaidUI.lua:236`, `:248`).
#[test]
fn only_the_leader_may_drag() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    let raid = twelve();
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Member1".into(),
            guid: raid[1].guid,
        }],
        leader_index: 1,
        raid,
        ..Default::default()
    });
    s.fire_event("RAID_ROSTER_UPDATE", Vec::new());
    s.run(
        "MOVING_RAID_MEMBER = RaidGroupButton12; TARGET_RAID_SLOT = RaidGroup5Slot1; \
         RaidGroupButton_OnDragStop(RaidGroupButton12)",
    )
    .unwrap();
    assert!(
        s.take_party_requests().is_empty(),
        "a member's drag sends nothing"
    );
}

// ── The row's click and menu ─────────────────────────────────────────────────────────────────────

#[test]
fn left_clicking_a_row_targets_its_raid_token() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());
    s.run("RaidGroupButton7:Click(\"LeftButton\")").unwrap();
    assert_eq!(
        s.take_selection_requests(),
        vec![SelectionRequest::Unit("raid7".into())]
    );
}

/// The RAID menu's rank rules re-read the row's rank by row index (`UnitPopup.lua:382-401`).
#[test]
fn the_row_menu_offers_the_rank_verbs_by_rank() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());

    s.run("RaidGroupButton3:Click(\"RightButton\")").unwrap();
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the menu opens"
    );
    let shown = |s: &UiScript, key: &str| {
        s.eval::<bool>(&format!(
            "for i = 1, UIDROPDOWNMENU_MAXBUTTONS do \
                 local b = getglobal(\"DropDownList1Button\"..i) \
                 if b and b:IsShown() and b.value == \"{key}\" then return true end \
             end return false"
        ))
        .unwrap()
    };
    assert!(
        shown(&s, "RAID_PROMOTE"),
        "a plain member can be made an assistant"
    );
    assert!(!shown(&s, "RAID_DEMOTE"), "and cannot be demoted from one");
    assert!(shown(&s, "RAID_LEADER"), "and can be handed the lead");
    assert!(shown(&s, "RAID_REMOVE"));

    // Row 2 is the assistant: demote, not promote.
    s.run("HideDropDownMenu(1) RaidGroupButton2:Click(\"RightButton\")")
        .unwrap();
    assert!(shown(&s, "RAID_DEMOTE"));
    assert!(!shown(&s, "RAID_PROMOTE"));

    s.run("HideDropDownMenu(1) RaidGroupButton1:Click(\"RightButton\")")
        .unwrap();
    assert!(!shown(&s, "RAID_LEADER"));
    assert!(
        !shown(&s, "RAID_REMOVE"),
        "the leader cannot be kicked, not even by themself"
    );
}

/// Three verbs go by name and the kick by row index (`UnitPopup.lua:624-631`).
#[test]
fn the_menu_verbs_queue_the_right_requests() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());
    let click = |s: &UiScript, key: &str| {
        s.run(&format!(
            "for i = 1, UIDROPDOWNMENU_MAXBUTTONS do \
                 local b = getglobal(\"DropDownList1Button\"..i) \
                 if b and b:IsShown() and b.value == \"{key}\" then b:Click() return end \
             end error(\"{key} not shown\")"
        ))
        .unwrap();
    };
    s.run("RaidGroupButton3:Click(\"RightButton\")").unwrap();
    click(&s, "RAID_PROMOTE");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::AssistantLeader {
            name: "Member2".into(),
            grant: true
        }]
    );

    s.run("HideDropDownMenu(1) RaidGroupButton3:Click(\"RightButton\")")
        .unwrap();
    click(&s, "RAID_REMOVE");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::UninviteRaid(3)],
        "the kick goes by ROW INDEX, which is what the reference passes"
    );
}

// ── Raid Info ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_raid_info_panel_lists_the_saved_lockouts() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    // The first `UPDATE_INSTANCE_INFO` only arms `RaidFrame.hasRaidInfo` (`RaidFrame.lua:43-47`).
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    s.set_saved_instances(vec![
        SavedInstanceInfo {
            name: "Molten Core".into(),
            instance: 1234,
            reset: 3 * 86_400,
        },
        SavedInstanceInfo {
            name: "Onyxia's Lair".into(),
            instance: 77,
            reset: 3_600,
        },
    ]);
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameRaidInfoButton:IsEnabled()")
            .unwrap(),
        1
    );
    // The rows live inside the flyout, so `IsVisible` is false until it opens.
    s.run("RaidInfoFrame:Show()").unwrap();
    assert!(visible(&s, "RaidInfoInstance1"));
    assert!(visible(&s, "RaidInfoInstance2"));
    assert!(
        !visible(&s, "RaidInfoInstance3"),
        "rows past the list stay away"
    );
    assert_eq!(text_of(&s, "RaidInfoInstance1Name"), "Molten Core");
    assert_eq!(text_of(&s, "RaidInfoInstance1ID"), "1234");
    assert!(
        text_of(&s, "RaidInfoInstance1Reset").starts_with("Resets in 3 Days"),
        "the reset is a REMAINING duration through SecondsToTime, not a timestamp: {:?}",
        text_of(&s, "RaidInfoInstance1Reset")
    );
    assert!(
        !visible(&s, "RaidInfoScrollFrameScrollBar"),
        "four rows fit, so two need no bar"
    );

    s.set_saved_instances(Vec::new());
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameRaidInfoButton:IsEnabled()")
            .unwrap(),
        0
    );
    // The stale row stays, as in the reference: `RaidInfoFrame_Update` wraps its row loop in
    // `if ( savedInstances > 0 )` (`RaidFrame.lua:94`), so a count of zero never reaches the
    // `Hide()`. The dead button keeps the flyout from reopening onto it.
    assert!(
        visible(&s, "RaidInfoInstance1"),
        "the reference leaves the last row standing when the count hits zero"
    );

    assert!(visible(&s, "RaidInfoFrame"));
    s.run("RaidFrameRaidInfoButton:GetScript(\"OnClick\")()")
        .unwrap();
    assert!(!visible(&s, "RaidInfoFrame"), "and closes it again");
}

/// An ordinary session gets two empty answers, for `PLAYER_ENTERING_WORLD`'s `RequestRaidInfo` and
/// the pane's OnShow; the first arms the latch, the second disables the button.
#[test]
fn a_player_with_no_lockouts_loses_the_raid_info_button_on_the_second_answer() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);

    // Answer one only arms the latch; `RaidFrame_OnLoad` leaves the button as it loaded.
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrame.hasRaidInfo or 0").unwrap(),
        1,
        "the first answer only arms the latch"
    );

    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameRaidInfoButton:IsEnabled()")
            .unwrap(),
        0,
        "an empty lockout list is a dead button"
    );

    // As in the reference, the row loop never runs (`RaidFrame.lua:94`), so the rows keep the
    // visibility `RaidFrame.xml` declares; only a frame shown by hand reaches them.
    s.run("RaidInfoFrame:Show()").unwrap();
    assert!(
        visible(&s, "RaidInfoInstance1") && visible(&s, "RaidInfoInstance10"),
        "with no lockouts the reference's update never runs, so the XML's own rows stand"
    );
    s.run("RaidInfoFrame:Hide()").unwrap();

    // The button is dead to a real click through the pointer, not merely drawn grey.
    let (bx, by) = {
        let l: f32 = s.eval("return RaidFrameRaidInfoButton:GetLeft()").unwrap();
        let r: f32 = s.eval("return RaidFrameRaidInfoButton:GetRight()").unwrap();
        let t: f32 = s.eval("return RaidFrameRaidInfoButton:GetTop()").unwrap();
        let b: f32 = s
            .eval("return RaidFrameRaidInfoButton:GetBottom()")
            .unwrap();
        ((l + r) / 2.0, (t + b) / 2.0)
    };
    s.mouse_button(bx, by, "LeftButton", true);
    s.mouse_button(bx, by, "LeftButton", false);
    s.tick(0.016);
    assert!(
        !visible(&s, "RaidInfoFrame"),
        "a disabled button does not open the panel"
    );
}

/// Past four lockouts `RaidInfoFrame_Update` seats the bar at `(8, -3)` off the scroll frame
/// (`RaidFrame.lua:108-110`), centred on the trough art, not at the template's `(6, -16)`.
#[test]
fn the_scroll_bar_is_seated_on_the_trough_the_panel_draws_behind_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    s.set_saved_instances(
        (1..=6)
            .map(|i| SavedInstanceInfo {
                name: format!("Instance {i}"),
                instance: 1000 + i,
                reset: i * 3_600,
            })
            .collect(),
    );
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    s.run("RaidInfoFrame:Show()").unwrap();
    assert!(
        visible(&s, "RaidInfoScrollFrameScrollBar"),
        "six lockouts do not fit in four rows"
    );

    let mid_x = |s: &UiScript, f: &str| -> f32 {
        let l: f32 = s.eval(&format!("return {f}:GetLeft()")).unwrap();
        let r: f32 = s.eval(&format!("return {f}:GetRight()")).unwrap();
        (l + r) / 2.0
    };
    let top = |s: &UiScript, f: &str| -> f32 { s.eval(&format!("return {f}:GetTop()")).unwrap() };

    let bar = mid_x(&s, "RaidInfoScrollFrameScrollBar");
    let trough = mid_x(&s, "RaidInfoScrollFrameTop");
    assert!(
        (bar - trough).abs() < 0.01,
        "the bar rides the middle of its trough: bar {bar} vs trough {trough}"
    );
    assert!(
        (top(&s, "RaidInfoScrollFrameScrollBar") - (top(&s, "RaidInfoScrollFrame") - 3.0)).abs()
            < 0.01,
        "and hangs 3 px under the frame's top, not the template's 16"
    );
    // The up arrow rides above the bar, so the re-seat lifts it into the trough's cap.
    assert!(
        top(&s, "RaidInfoScrollFrameScrollBarScrollUpButton") > top(&s, "RaidInfoScrollFrame"),
        "the up arrow clears the scroll frame, where the template left it 13 px inside"
    );
}

// ── The ready check ──────────────────────────────────────────────────────────────────────────────

/// `READY_CHECK` opens the popup naming the leader; Yes calls `ConfirmReadyCheck(1)`, No calls it
/// with no argument (`Blizzard_RaidUI.xml:710`, `:728`).
#[test]
fn the_ready_check_popup_opens_and_answers() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());
    assert!(!visible(&s, "ReadyCheckFrame"), "nothing to answer yet");

    s.fire_event("READY_CHECK", Vec::new());
    assert!(visible(&s, "ReadyCheckFrame"), "UIParent's arm opens it");
    assert!(
        text_of(&s, "ReadyCheckFrameText").starts_with("Me has initiated a ready check."),
        "named for the rank-2 row: {:?}",
        text_of(&s, "ReadyCheckFrameText")
    );

    s.run("ReadyCheckFrameYesButton:Click()").unwrap();
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::ReadyCheckAnswer(true)]
    );
    assert!(!visible(&s, "ReadyCheckFrame"));

    s.fire_event("READY_CHECK", Vec::new());
    s.run("ReadyCheckFrameNoButton:Click()").unwrap();
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::ReadyCheckAnswer(false)],
        "No passes no argument at all, and absent must mean not-ready"
    );

    s.fire_event("READY_CHECK", Vec::new());
    assert!(visible(&s, "ReadyCheckFrame"));
    push_raid(&mut s, Vec::new());
    assert!(
        !visible(&s, "ReadyCheckFrame"),
        "the raid went, so the question went"
    );
}

/// No release reaches a gesture once the OS pointer leaves the window, so
/// `UiScript::pointer_left_window` ends the drag: `OnDragStop` runs, the row goes home, and the
/// next row still drags.
#[test]
fn a_drag_carried_off_the_window_edge_ends_instead_of_gluing_the_row_to_the_cursor() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());

    let left_of =
        |s: &UiScript, f: &str| -> f32 { s.eval(&format!("return {f}:GetLeft()")).unwrap() };
    let centre = |s: &UiScript, f: &str| -> (f32, f32) {
        let l: f32 = s.eval(&format!("return {f}:GetLeft()")).unwrap();
        let r: f32 = s.eval(&format!("return {f}:GetRight()")).unwrap();
        let t: f32 = s.eval(&format!("return {f}:GetTop()")).unwrap();
        let b: f32 = s.eval(&format!("return {f}:GetBottom()")).unwrap();
        ((l + r) / 2.0, (t + b) / 2.0)
    };

    let home = left_of(&s, "RaidGroupButton7");
    let (fx, fy) = centre(&s, "RaidGroupButton7");
    s.mouse_button(fx, fy, "LeftButton", true);
    s.mouse_move(fx + 40.0, fy + 40.0); // past the threshold ⇒ OnDragStart ⇒ StartMoving
    s.tick(0.016);
    assert_eq!(
        s.eval::<String>("return MOVING_RAID_MEMBER:GetName()")
            .unwrap(),
        "RaidGroupButton7",
        "the drag is in flight"
    );

    s.pointer_left_window();
    s.tick(0.016);
    assert!(
        s.eval::<bool>("return MOVING_RAID_MEMBER == nil").unwrap(),
        "the pane's own drag state is cleared by the OnDragStop the abandon fires"
    );
    assert_eq!(
        left_of(&s, "RaidGroupButton7"),
        home,
        "the row springs back to its slot — a drop on nothing is not a move"
    );

    s.mouse_move(400.0, 300.0);
    s.tick(0.016);
    s.mouse_move(500.0, 200.0);
    s.tick(0.016);
    assert_eq!(
        left_of(&s, "RaidGroupButton7"),
        home,
        "…and it stays there however far the cursor travels"
    );

    let (gx, gy) = centre(&s, "RaidGroupButton8");
    let (tx, ty) = centre(&s, "RaidGroup5Slot1");
    s.mouse_button(gx, gy, "LeftButton", true);
    s.mouse_move(gx + 10.0, gy + 10.0);
    s.tick(0.016);
    s.mouse_move(tx, ty);
    s.tick(0.016);
    s.mouse_button(tx, ty, "LeftButton", false);
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::SetSubgroup { index: 8, group: 5 }],
        "the row after the abandoned one still moves"
    );
    assert!(
        s.errors().is_empty(),
        "and nothing raised: {:?}",
        s.errors()
    );
}

/// Three whole gestures through the real pointer path, with the roster echo `/partytest raid`
/// supplies in between.
#[test]
fn one_drag_does_not_cost_the_next_one() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    let mut raid = vec![row("Me", 2, 1, "Warrior", true, false)];
    for i in 1..25u32 {
        raid.push(row(
            &format!("Member{i}"),
            0,
            i / 5 + 1,
            "Priest",
            true,
            false,
        ));
    }
    push_raid(&mut s, raid.clone());

    let centre = |s: &UiScript, f: &str| -> (f32, f32) {
        let l: f32 = s.eval(&format!("return {f}:GetLeft()")).unwrap();
        let r: f32 = s.eval(&format!("return {f}:GetRight()")).unwrap();
        let t: f32 = s.eval(&format!("return {f}:GetTop()")).unwrap();
        let b: f32 = s.eval(&format!("return {f}:GetBottom()")).unwrap();
        ((l + r) / 2.0, (t + b) / 2.0)
    };

    for (row_index, group) in [(7u32, 6u32), (8, 6), (9, 7)] {
        let from = format!("RaidGroupButton{row_index}");
        let seat = if group == 6 && row_index == 8 { 2 } else { 1 };
        let to = format!("RaidGroup{group}Slot{seat}");
        let (fx, fy) = centre(&s, &from);
        let (tx, ty) = centre(&s, &to);
        s.mouse_button(fx, fy, "LeftButton", true);
        s.mouse_move(fx + 10.0, fy + 10.0);
        s.tick(0.016);
        s.mouse_move(tx, ty);
        s.tick(0.016);
        s.mouse_button(tx, ty, "LeftButton", false);
        s.tick(0.016);
        assert_eq!(
            s.take_party_requests(),
            vec![PartyRequest::SetSubgroup {
                index: row_index,
                group
            }],
            "drag {row_index} onto group {group}"
        );
        assert!(s.errors().is_empty(), "{from}: {:?}", s.errors());
        // The echo, so the next drag starts from a repainted grid.
        raid[row_index as usize - 1].subgroup = group - 1;
        push_raid(&mut s, raid.clone());
    }
}
