//! `RAID_ROSTER_UPDATE` as the reference signals it: once per group list that rebuilds the raid
//! roster with no member name pending (`0x4babef`) and once per list that drops the roster held
//! (`0x4ba57b`), with the roster edge standing in for the two signals no list carries.

use super::super::pets::tests::{app, app_with, flag, frame, member, ME};
use super::*;

const ON: u8 = member_status::ONLINE;
const DEAD: u8 = member_status::ONLINE | member_status::DEAD;
const GHOST: u8 = member_status::ONLINE | member_status::GHOST;

/// A frame that counts the `RAID_ROSTER_UPDATE`s the VM is told of; [`fired`] takes them.
fn watch(app: &mut App) {
    let script = app.world_mut().non_send_resource_mut::<UiScript>();
    script
        .run(
            r#"
            ROSTER = 0
            local f = CreateFrame("Frame")
            f:RegisterEvent("RAID_ROSTER_UPDATE")
            f:SetScript("OnEvent", function() ROSTER = ROSTER + 1 end)
            "#,
        )
        .unwrap();
}

/// Run a frame and count the `RAID_ROSTER_UPDATE`s it fired.
fn fired(app: &mut App) -> u32 {
    frame(app);
    let script = app.world_mut().non_send_resource_mut::<UiScript>();
    let fired: f64 = script.eval("return ROSTER").unwrap();
    script.run("ROSTER = 0").unwrap();
    fired as u32
}

/// `SMSG_GROUP_LIST` as the server sends it: members `1..` with their status byte, led by us, of
/// the given group type. `names_pending` is what the name cache told the handler.
fn send(app: &mut App, group_type: u8, statuses: &[u8], names_pending: bool) {
    let list = statuses
        .iter()
        .enumerate()
        .map(|(i, &status)| GroupMemberEntry {
            name: format!("M{}", i + 1),
            guid: member(i as u64 + 1),
            status,
            flags: 0,
        })
        .collect();
    app.world_mut()
        .resource_mut::<GroupState>()
        .apply_list_awaiting(group_type, 0, list, ME, None, Some(ME), names_pending);
}

fn raid(app: &mut App, statuses: &[u8]) {
    send(app, GROUPTYPE_RAID, statuses, false);
}

fn party(app: &mut App, statuses: &[u8]) {
    send(app, 0, statuses, false);
}

/// The all-zero list: "not in a group".
fn all_zero(app: &mut App) {
    app.world_mut()
        .resource_mut::<GroupState>()
        .apply_list(0, 0, vec![], 0, None, Some(ME));
}

/// `GetNumRaidMembers()`, the rows the VM reads.
fn rows(app: &mut App) -> f64 {
    let script = app.world_mut().non_send_resource::<UiScript>();
    script.eval("return GetNumRaidMembers()").unwrap()
}

/// `GetRaidRosterInfo`'s ninth return for row `row`.
fn dead(app: &mut App, row: u32) -> Option<f64> {
    flag(
        app,
        &format!(
            "(function() local _, _, _, _, _, _, _, _, dead = GetRaidRosterInfo({row}) \
             return dead end)()"
        ),
    )
}

/// The reported shape: a raid member dies, then releases, out of view. The list that says so moves
/// their status alone, no guid, rank, subgroup or online bit, and the writer signals all the same
/// (`0x4babef`, with no compare against the old roster).
#[test]
fn a_raid_list_that_moves_only_a_members_dead_bit_fires_raid_roster_update() {
    let mut app = app();
    watch(&mut app);
    raid(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 1, "the first look answers the list");
    assert_eq!(fired(&mut app), 0, "a steady frame is silent");
    assert_eq!(dead(&mut app, 1), None);

    raid(&mut app, &[DEAD, ON]);
    assert_eq!(fired(&mut app), 1, "member 1 died");
    assert_eq!(dead(&mut app, 1), Some(1.0), "and the roster says so");
    raid(&mut app, &[GHOST, ON]);
    assert_eq!(fired(&mut app), 1, "member 1 released");
    assert_eq!(dead(&mut app, 1), None);
    assert_eq!(fired(&mut app), 0);
}

/// The writer holds no memory of the last roster: a list that says what the last said signals.
#[test]
fn an_identical_raid_list_fires_raid_roster_update_again() {
    let mut app = app();
    watch(&mut app);
    raid(&mut app, &[ON, ON]);
    fired(&mut app);

    raid(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 1);
    raid(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 1);
    assert_eq!(fired(&mut app), 0, "a frame with no list is silent");
}

/// A party's list runs no roster writer: the raid leg of the handler is the only caller (`0x5e6eb4`),
/// and the other leg finds no roster to drop (`0x4ba550`).
#[test]
fn a_party_list_fires_no_raid_roster_update() {
    let mut app = app();
    watch(&mut app);
    party(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 0, "the list that seats the party");
    party(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 0, "an identical one");
    party(&mut app, &[DEAD, ON]);
    assert_eq!(fired(&mut app), 0, "a status alone");
    all_zero(&mut app);
    assert_eq!(
        fired(&mut app),
        0,
        "and the all-zero list with no raid held"
    );
}

/// With a member name pending the writer signals nothing (`0x4babdf`); the name answer does later
/// (`0x4bada6`), which is not built, so the roster edge signals a list that moves the roster's
/// identity, as it always has.
#[test]
fn a_raid_list_with_a_name_pending_fires_only_on_the_roster_edge() {
    let mut app = app();
    watch(&mut app);
    raid(&mut app, &[ON, ON]);
    fired(&mut app);

    send(&mut app, GROUPTYPE_RAID, &[DEAD, ON], true);
    assert_eq!(fired(&mut app), 0, "a status alone, a name pending");
    send(&mut app, GROUPTYPE_RAID, &[DEAD, ON], true);
    assert_eq!(fired(&mut app), 0, "an identical list, a name pending");
    send(&mut app, GROUPTYPE_RAID, &[DEAD, ON, ON], true);
    assert_eq!(fired(&mut app), 1, "a member joined: the roster moved");
    send(&mut app, GROUPTYPE_RAID, &[DEAD, ON, ON], false);
    assert_eq!(fired(&mut app), 1, "the names held: the list fires");
}

/// A list that is not a raid, with a raid held, drops it and signals once (`0x4ba57b`): the raid
/// turned party and the all-zero list alike, each list of the drain apiece.
#[test]
fn a_list_that_drops_the_raid_fires_raid_roster_update_once() {
    let mut app = app();
    watch(&mut app);
    raid(&mut app, &[ON, ON]);
    fired(&mut app);

    party(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 1, "the raid became a party");
    party(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 0, "no raid held");

    raid(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 1);
    all_zero(&mut app);
    assert_eq!(fired(&mut app), 1, "the group ended");
    all_zero(&mut app);
    assert_eq!(fired(&mut app), 0, "and nothing to end");

    // In one drain: the list that rebuilds, then the one that drops.
    raid(&mut app, &[ON, ON]);
    fired(&mut app);
    raid(&mut app, &[DEAD, ON]);
    all_zero(&mut app);
    assert_eq!(fired(&mut app), 2, "one apiece");
}

/// Each packet signals, so the lists one drain applies fire once apiece.
#[test]
fn each_raid_list_of_one_drain_fires_its_own_raid_roster_update() {
    let mut app = app();
    watch(&mut app);
    raid(&mut app, &[ON, ON]);
    fired(&mut app);

    raid(&mut app, &[DEAD, ON]);
    raid(&mut app, &[DEAD, DEAD]);
    raid(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 3);
}

/// World entry fills in our own row (`0x4ba1a2`), with no list behind it: the roster edge fires as
/// our row arrives, once.
#[test]
fn our_own_row_arriving_fires_raid_roster_update_once() {
    let mut app = app();
    watch(&mut app);
    raid(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 1, "the list, before we are in the world");
    assert_eq!(rows(&mut app), 2.0);

    app.world_mut().spawn((
        Guid(ME),
        ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[])),
        SelfPlayer,
    ));
    assert_eq!(fired(&mut app), 1, "our row filled in");
    assert_eq!(rows(&mut app), 3.0, "us, then the two");
    assert_eq!(fired(&mut app), 0);
}

/// A VM minted after the lists landed was told of none: the raid it finds gets its one catch-up
/// from the roster edge, never the history, and a player who holds no raid gets nothing, however
/// many lists ended raids before.
#[test]
fn a_fresh_vm_finds_a_raid_once_and_a_party_not_at_all() {
    let mut app = app();
    watch(&mut app);
    raid(&mut app, &[ON, ON]);
    raid(&mut app, &[DEAD, ON]);
    raid(&mut app, &[DEAD, DEAD]);
    assert_eq!(fired(&mut app), 1);
    assert_eq!(fired(&mut app), 0);

    let mut ended = self::app();
    watch(&mut ended);
    raid(&mut ended, &[ON, ON]);
    party(&mut ended, &[ON, ON]);
    assert_eq!(fired(&mut ended), 0, "the raid ended before the first look");

    let mut solo = self::app();
    watch(&mut solo);
    assert_eq!(fired(&mut solo), 0);
}

#[test]
fn the_roster_updates_owed_are_the_count_since_the_last_look() {
    assert_eq!(roster_updates_owed(3, 3, false), 0, "nothing landed");
    assert_eq!(roster_updates_owed(5, 3, false), 2, "one apiece");
    assert_eq!(
        roster_updates_owed(2, 4, false),
        2,
        "the session ended and began again"
    );
    assert_eq!(
        roster_updates_owed(7, 0, true),
        0,
        "a fresh VM's catch-up is the roster edge's"
    );
}

/// The sandbox stands in for the server, whose echo of each group intent is a list: a raid's list
/// signals `RAID_ROSTER_UPDATE` though the intent moves nobody (a loot threshold), the one that
/// ends the raid signals once, and a party's list signals none.
#[test]
fn the_sandbox_answers_a_raid_intent_with_a_raid_roster_update() {
    let mut app = app();
    watch(&mut app);
    synthetic_roster(&mut app.world_mut().resource_mut::<GroupState>(), None);
    assert_eq!(fired(&mut app), 0, "a party's list");

    let intent = |app: &mut App, req: PartyRequest| {
        let mut group = app.world_mut().resource_mut::<GroupState>();
        assert!(test_apply_local(&mut group, &req, Some(ME), None));
    };
    intent(&mut app, PartyRequest::LootThreshold(4));
    assert_eq!(fired(&mut app), 0, "a party intent");
    intent(&mut app, PartyRequest::ConvertToRaid);
    assert_eq!(fired(&mut app), 1, "the party became a raid");
    intent(&mut app, PartyRequest::LootThreshold(4));
    assert_eq!(fired(&mut app), 1, "a raid intent that moves no member");
    intent(&mut app, PartyRequest::PromoteUnit("party1".into()));
    assert_eq!(
        fired(&mut app),
        1,
        "once, though the leader's rank moved too"
    );
    intent(
        &mut app,
        PartyRequest::SetRaidTarget {
            unit: "player".into(),
            index: 1,
        },
    );
    assert_eq!(fired(&mut app), 0, "a mark is no list");
    intent(&mut app, PartyRequest::Leave);
    assert_eq!(fired(&mut app), 1, "the raid ended");
}

/// The stock raid UI, loaded whole off the player's own chain: a raid member dies out of view and
/// the list that says so moves nothing but their dead bit. The raid tab's grid also repaints on
/// `PARTY_MEMBERS_CHANGED` (`RaidFrame_OnEvent`), which every list signals, but a group pullout
/// re-reads its rows on `RAID_ROSTER_UPDATE` alone (`RaidPullout_OnEvent`), and the member's own
/// `UNIT_HEALTH` never comes for a unit that is not in view.
#[test]
fn the_stock_raid_ui_repaints_a_member_who_dies_out_of_view() {
    benilla_formats::wow_data_or_skip!();
    let mut script = UiScript::new().unwrap();
    script.set_screen_size(1024.0, 768.0);
    let failures = crate::ui_script::load_default_ui(&script);
    assert!(failures.is_empty(), "load failures: {failures:?}");
    crate::ui_script::test_ui::seat_chain_addon(&mut script, "Blizzard_RaidUI");
    let mut app = app_with(script);
    // A warrior, so the alive colour is the class's and the dead one is not.
    app.world_mut().resource_mut::<NameCache>().insert_player(
        member(1),
        "M1".into(),
        Some((1, 1, 0)),
    );
    let colour = |app: &mut App, call: &str| {
        let (r, g, b): (f32, f32, f32) = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval(&format!("return {call}"))
            .unwrap();
        [r, g, b].map(|c| (c * 100.0).round() as i32)
    };
    let text_colour = |app: &mut App| colour(app, "RaidGroupButton1Name:GetTextColor()");
    let pullout_colour = |app: &mut App| colour(app, "RaidPullout1Button1Name:GetTextColor()");
    let (warrior, red) = ([78, 61, 43], [100, 10, 10]);

    // The first list loads the raid UI (`RaidFrame_LoadUI`); the drag on group 1's label, whose
    // handler runs with `this` set, opens the group's pullout (`Blizzard_RaidUI.xml:172`).
    raid(&mut app, &[ON, ON]);
    frame(&mut app);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run("this = RaidGroup1Label RaidPullout_GenerateGroupFrame()")
        .unwrap();
    assert_eq!(text_colour(&mut app), warrior, "the grid's row");
    assert_eq!(pullout_colour(&mut app), warrior, "the pullout's row");

    raid(&mut app, &[DEAD, ON]);
    frame(&mut app);
    assert_eq!(text_colour(&mut app), red, "the grid repaints");
    assert_eq!(pullout_colour(&mut app), red, "and so does the pullout");

    raid(&mut app, &[ON, ON]);
    frame(&mut app);
    assert_eq!(text_colour(&mut app), warrior);
    assert_eq!(pullout_colour(&mut app), warrior);
    let errors = app.world().non_send_resource::<UiScript>().errors();
    assert!(errors.is_empty(), "script errors: {errors:?}");
}
