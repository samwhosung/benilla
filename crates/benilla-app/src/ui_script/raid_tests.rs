//! The social window's fourth tab — the raid pane (`RaidFrame.xml`): the tab, the
//! two states the pane has, the 8x5 grid's seating and colouring, the drag's three landings, the
//! row menu, the ready-check popup and the saved-instance panel.
//!
//! Its own module rather than more of `friends_tests` for `guild_tests`' reason: the pane is Lua
//! over a **raid** roster, so every test here has to push one first, and a file about the friends
//! list must not be in that business.
//!
//! What these guard that `ui_party`'s Rust tests structurally cannot: the pane is Lua over a
//! snapshot, so a row seated in the wrong slot, a colour rule inverted, a drag wired to the wrong
//! verb, or a menu row shown to the wrong person are all invisible there and green in the parse
//! sweep. Each test below fails on exactly one of those.

use benilla_ui::script::{
    PartyMemberInfo, PartyRequest, PartyState, RaidMemberInfo, SavedInstanceInfo, SelectionRequest,
    UiScript,
};

/// The window's slice of the manifest, in `load_default_ui` order. `UIParent.xml` is in it for
/// two functions the pane really calls — `MouseIsOver` (the drag's hover sweep) and
/// `SecondsToTime` (the lockout rows) — and for the `READY_CHECK` arm that opens the popup.
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

/// One raid row. `rank` is 2 leader / 1 assistant / 0 member; `subgroup` is the **1-based** number
/// the pane shows, which the record stores 0-based — the binding is what adds the one
/// (`RaidMemberInfo::subgroup`'s own doc, the reference's `0x4bb61a inc`). Converting here rather
/// than at every call site is the point: a fixture that hands the record a 1-based number seats
/// every row one group to the right, which looks exactly like a seating bug in the pane.
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

/// Push a roster and fire the event that follows it, exactly as `ui_party::feed_party` does.
fn push_raid(s: &mut UiScript, raid: Vec<RaidMemberInfo>) {
    // `members` is our own SUBGROUP's slice — non-empty is what makes `IsPartyLeader()`/
    // `IsRaidLeader()` answer 1 for leader_index 0 (the shared `leads_the_group` predicate).
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

/// A 12-member raid across three subgroups, us leading — the shape most tests want.
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

/// Open the tab **and drain what opening it queues**. `RaidFrame`'s OnShow calls
/// `RequestRaidInfo()` — the reference's own, and the reason the Raid Info button can ever be
/// right — so every `take_party_requests` after this would otherwise start with that ask and no
/// test about a button would be about that button.
fn open_raid_tab(s: &mut UiScript) {
    s.run("ToggleFriendsFrame(4)").unwrap();
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::RequestRaidInfo],
        "showing the pane asks the server for our lockouts"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The tab
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Tab 4 exists, opens the pane, titles the window RAID, and puts every other subframe away.
///
/// This is the test the whole decision exists for: `FriendsFrame.xml` shipped three tabs and said
/// so in its header, and the tab strip is the one place a missing pane is visible without a raid.
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
    // And the tab's own OnClick is wired — a tab that PanelTemplates enables but that does
    // nothing when clicked is exactly the dead tab the guild arc had to fix.
    s.run("FriendsFrameTab1:Click()").unwrap();
    assert!(!visible(&s, "RaidFrame"));
    s.run("FriendsFrameTab4:Click()").unwrap();
    assert!(visible(&s, "RaidFrame"), "the tab button opens it too");
}

/// Closing the window closes the Raid Info flyout with it — the ref's fourth satellite.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The not-in-a-raid state
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Out of a raid the pane is the blurb plus Convert To Raid, and the button is live only for the
/// leader of an actual party — the three states, in order.
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

    // A party we lead: `leader_index == 0` with members is the shared leads-the-group predicate.
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

    // A party we do NOT lead.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The grid
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The roster paints: each row lands in ITS SUBGROUP's next free slot, carries its rank token, and
/// rows past the roster stay hidden.
///
/// The seating is the assertion with teeth. Row order and slot order are different orders — row 5
/// of a raid whose first four are in group 1 is group 2's *first* slot — and every drag, kick and
/// menu action downstream addresses a row by the index this seating implies.
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
    // The slot knows its occupant too — the link the drop code reads to decide move-vs-swap.
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

    // Leaving the raid puts the whole grid away and brings the blurb back.
    push_raid(&mut s, Vec::new());
    assert!(!visible(&s, "RaidGroup1"));
    assert!(!visible(&s, "RaidGroupButton1"));
    assert!(visible(&s, "RaidFrameRaidDescription"));
}

/// The colour ladder: offline grey beats everything, then dead red, then the class colour — and
/// all three columns take it together.
///
/// Inverting any two of these is invisible to every other test in this file: the rows still seat,
/// the names still read, the clicks still fire.
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
            // Offline AND dead: grey wins, because a disconnected member's health is not news.
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
    // WARRIOR = 0.78, 0.61, 0.43 (Fonts.xml's RAID_CLASS_COLORS, the reference's own values).
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
    // The level column takes the same colour as the name, never a different one. (The CLASS
    // column is a Button rather than a FontString — the reference makes it one because it is the
    // class pullout's drag handle — and a Button in this engine has `SetTextColor` but no getter,
    // so it cannot be read back here. `RaidGroupButton_SetRowColor` writes all three from one
    // colour in three adjacent lines; these two are the readable half of that.)
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

/// `UNIT_HEALTH`/`UNIT_LEVEL` repaint ONE row and do not go through the roster event.
///
/// This is the reason `RAID_ROSTER_UPDATE` is fired on the roster's identity rather than on every
/// field: a 40-row rebuild per point of damage taken is what the other choice costs.
#[test]
fn a_units_health_and_level_repaint_only_that_row() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    let mut raid = twelve();
    push_raid(&mut s, raid.clone());
    assert_eq!(text_of(&s, "RaidGroupButton3Level"), "60");

    // The row dies. Only the per-unit event is fired — no RAID_ROSTER_UPDATE.
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

    // A unit event for something that is not a raid token is ignored rather than mis-parsed.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The management buttons
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Ready Check is the leader's alone; Add Member shows for everyone in a raid. Both are hidden
/// outright out of one — they share their seat with Convert To Raid.
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

    // A member, not the leader: no Ready Check button at all.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The drag
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The drag's three landings: an empty slot in another group MOVES, an occupied one SWAPS, and
/// anything else — including this row's own group — sends nothing and springs the row home.
///
/// Driven through the real handlers with `TARGET_RAID_SLOT` standing in for the hover sweep, which
/// needs a live cursor this harness has no way to place.
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

    // Row 12 (group 3, seat 4) onto group 3's fifth seat — its OWN group. Nothing goes out.
    drag(&s, "RaidGroupButton12", "RaidGroup3Slot5");
    assert!(
        s.take_party_requests().is_empty(),
        "a drop inside the row's own group is not a move"
    );

    // Row 12 onto group 5's empty first seat — a MOVE, and the subgroup is the Lua 1-based one.
    drag(&s, "RaidGroupButton12", "RaidGroup5Slot1");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::SetSubgroup {
            index: 12,
            group: 5
        }]
    );

    // Row 12 onto group 1's second seat, which row 2 is in — a SWAP, by the two ROW indices.
    drag(&s, "RaidGroupButton12", "RaidGroup1Slot2");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::SwapSubgroup {
            index: 12,
            other: 2
        }]
    );
}

/// A non-leader cannot drag at all — the gate is on both ends, so a drag that never started also
/// never stops.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The row's click and menu
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Left-clicking a row targets that row's `raidN` token — the same token the tooltip reads.
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

/// Right-clicking a row opens the RAID menu, and each of its four rows appears for exactly the
/// person the reference shows it to.
///
/// The menu is addressed by ROW INDEX, and every rank rule re-reads that row's rank — so a menu
/// opened on the wrong index would offer Promote on somebody who is already an assistant.
#[test]
fn the_row_menu_offers_the_rank_verbs_by_rank() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());

    // Row 3 is a plain member: as the leader we may promote it, not demote it, and may kick it.
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

    // Row 1 is the leader — us. Nothing may be done to the leader.
    s.run("HideDropDownMenu(1) RaidGroupButton1:Click(\"RightButton\")")
        .unwrap();
    assert!(!shown(&s, "RAID_LEADER"));
    assert!(
        !shown(&s, "RAID_REMOVE"),
        "the leader cannot be kicked, not even by themself"
    );
}

/// The menu's verbs reach the engine addressed the way the wire wants them — three by name, and
/// the kick by the row index.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Raid Info
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The saved-instance panel fills from `GetSavedInstanceInfo`, and its button is live only once
/// the server has answered with something.
#[test]
fn the_raid_info_panel_lists_the_saved_lockouts() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    // The first UPDATE_INSTANCE_INFO only arms the ref's latch (`RaidFrame.hasRaidInfo`).
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
    // The rows live inside the flyout, so `IsVisible` is false until it is open — open it, which
    // is what the button does, and then ask.
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

    // Bound to nothing: the button dies, and the panel does not keep a stale bar standing.
    s.set_saved_instances(Vec::new());
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameRaidInfoButton:IsEnabled()")
            .unwrap(),
        0
    );
    // **The stale row STAYS, and that is the reference's own defect.** `RaidInfoFrame_Update`
    // wraps its whole row loop in `if ( savedInstances > 0 )` (RaidFrame.lua:93-106), so the
    // count dropping to zero never reaches the `Hide()` in its else-arm — the last lockout
    // remains painted under an already-open panel. Our retired file hid them; the migration
    // reverts that, and it is the director's call, not ours to re-author (1874).
    //
    // What DOES protect the player is the line above: the button is disabled, so the flyout
    // cannot be reopened onto the stale row.
    assert!(
        visible(&s, "RaidInfoInstance1"),
        "the reference leaves the last row standing when the count hits zero"
    );

    // The button toggles the flyout.
    assert!(visible(&s, "RaidInfoFrame"));
    s.run("RaidFrameRaidInfoButton:GetScript(\"OnClick\")()")
        .unwrap();
    assert!(!visible(&s, "RaidInfoFrame"), "and closes it again");
}

/// **The player with no lockouts at all** — the case that shipped broken (1561). Two answers, both
/// empty, is the whole ordinary session: one for `PLAYER_ENTERING_WORLD`'s `RequestRaidInfo` and
/// one for the pane's own on show. The first arms `RaidFrame.hasRaidInfo` and returns; the second
/// is the one that has to put the button away and empty the panel behind it.
///
/// Its sibling above proves the same arithmetic from a list that had something in it. This one
/// proves it from a list that never did, which is the case a *diff* cannot reach — and the button
/// left live over an empty panel is exactly what the director saw.
#[test]
fn a_player_with_no_lockouts_loses_the_raid_info_button_on_the_second_answer() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);

    // Answer one: the latch, and nothing else. The button is still whatever it loaded as — the
    // reference does not disable it in `RaidFrame_OnLoad` either, so this window is its own.
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrame.hasRaidInfo or 0").unwrap(),
        1,
        "the first answer only arms the latch"
    );

    // Answer two, saying the same nothing.
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameRaidInfoButton:IsEnabled()")
            .unwrap(),
        0,
        "an empty lockout list is a dead button"
    );

    // **The panel IS left in the state the XML loaded, and that is the reference's own defect.**
    // `RaidInfoFrame_Update` wraps its entire row loop in `if ( savedInstances > 0 )`
    // (RaidFrame.lua:93-106), so with a lockout list that has never had anything in it the loop
    // never runs at all and no row is ever hidden — they keep the visibility stock
    // `Blizzard_RaidUI.xml` declared. Our retired file cleared them; the migration reverts that,
    // and a look the migration reverts is the director's call, not ours to re-author (1874).
    //
    // What protects the player is the line above: the button is disabled, so this panel cannot be
    // reached by a click at all. The rows below are only visible because the test showed the frame
    // by hand.
    s.run("RaidInfoFrame:Show()").unwrap();
    assert!(
        visible(&s, "RaidInfoInstance1") && visible(&s, "RaidInfoInstance10"),
        "with no lockouts the reference's update never runs, so the XML's own rows stand"
    );
    s.run("RaidInfoFrame:Hide()").unwrap();

    // The button is dead to a real click, not merely drawn grey — the half the director asked for
    // by name. Driven through the pointer, because that is the path a press actually takes.
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

/// The scroll bar the fifth lockout brings on has to land on the trough drawn behind it, not where
/// `UIPanelScrollFrameTemplate` seats a bare ScrollFrame's bar (1561).
///
/// Geometry, asserted as the relationship rather than as four numbers: the bar is CENTRED in the
/// trough art on X, and its top is the panel's own -3 rather than the template's -16. Both were
/// wrong before — 2 px left, 13 px low — and neither is visible to any test that only asks whether
/// the bar is shown.
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
    // The up arrow rides above the bar, so re-seating the bar is what lifts it into the trough's
    // own cap — the piece of this that is actually visible.
    assert!(
        top(&s, "RaidInfoScrollFrameScrollBarScrollUpButton") > top(&s, "RaidInfoScrollFrame"),
        "the up arrow clears the scroll frame, where the template left it 13 px inside"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The ready check
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `READY_CHECK` opens the popup naming the leader, and the two buttons answer opposite ways —
/// including the No button's argument-less call, which is the reference's own spelling.
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

    // The raid ending ends the question.
    s.fire_event("READY_CHECK", Vec::new());
    assert!(visible(&s, "ReadyCheckFrame"));
    push_raid(&mut s, Vec::new());
    assert!(
        !visible(&s, "ReadyCheckFrame"),
        "the raid went, so the question went"
    );
}

// The reference-geometry diff that stood here is GONE with the transcription it policed (1874).
// It compared our `RaidFrame.xml`'s numbers against the reference's; the pane is now the
// reference's own file off the player's chain, so there is nothing left to diff — the geometry is
// the reference's by construction rather than by assertion.

/// **A drag the cursor carries off the window edge ENDS — it does not glue the row to the mouse
/// for the rest of the session**.
///
/// The defect this pins is not in this file at all; it is the engine's, and the raid grid is
/// simply where it bites hardest. No release is fed once the OS pointer is outside the window, so
/// `UiScript::pointer_left_window` used to drop the gesture silently: `OnDragStop` never ran, this
/// pane's `MOVING_RAID_MEMBER` stayed set, the engine's one `StartMoving` slot stayed taken, and
/// the row followed the cursor around the screen from then on — swallowing every press aimed at
/// any row underneath it. "I moved one member and then could not move any other" is what that
/// looks like from a hand on the mouse.
///
/// The three things this asserts are the three the bug broke, in order of how visible they are:
/// the row goes home, the pane forgets it, and the next row still drags.
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

    // The row is no longer following anything.
    s.mouse_move(400.0, 300.0);
    s.tick(0.016);
    s.mouse_move(500.0, 200.0);
    s.tick(0.016);
    assert_eq!(
        left_of(&s, "RaidGroupButton7"),
        home,
        "…and it stays there however far the cursor travels"
    );

    // And the next row drags normally, which is the thing the director actually lost.
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

/// The whole gesture, through the REAL pointer path, three times over — press, cross the
/// threshold, travel, release — with the roster echo in between, exactly as `/partytest raid`
/// feeds it.
///
/// Its sibling `dragging_a_row_moves_swaps_or_springs_back` calls `RaidGroupButton_OnDragStop`
/// with the globals set by hand, which is the *landing* logic and nothing else: it never fires
/// `OnDragStart`, never calls `StartMoving`, never runs the hover sweep, and — the part that
/// mattered — never drags a SECOND row. This one does all four.
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
        // The sandbox echo `/partytest raid` supplies, so the next drag starts from a repainted
        // grid rather than a frozen one.
        raid[row_index as usize - 1].subgroup = group - 1;
        push_raid(&mut s, raid.clone());
    }
}
