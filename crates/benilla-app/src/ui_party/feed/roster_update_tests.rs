//! `RAID_ROSTER_UPDATE` as the reference signals it: once per group list that rebuilds the raid
//! roster with no member name pending (`0x4babef`), once per list that drops the roster held
//! (`0x4ba57b`), and once when the last pending name answers (`0x4bada6`), which signals
//! `PARTY_MEMBERS_CHANGED` after it (`0x4badb0`).

use super::super::pets::tests::{app, app_with, flag, frame, member, ME};
use super::*;
use benilla_ui::script::UiScript;

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
/// the given group type. `missing` are the guids whose name the cache told the handler is to come.
fn send(app: &mut App, group_type: u8, statuses: &[u8], missing: &[u64]) {
    let rows: Vec<(u8, u8)> = statuses.iter().map(|&status| (status, 0)).collect();
    send_rows(app, group_type, &rows, missing);
}

/// [`send`] with each member's (status, subgroup); ours is subgroup 0.
fn send_rows(app: &mut App, group_type: u8, rows: &[(u8, u8)], missing: &[u64]) {
    let list: Vec<GroupMemberEntry> = rows
        .iter()
        .enumerate()
        .map(|(i, &(status, subgroup))| GroupMemberEntry {
            name: format!("M{}", i + 1),
            guid: member(i as u64 + 1),
            status,
            flags: subgroup,
        })
        .collect();
    let mut group = app.world_mut().resource_mut::<GroupState>();
    let seats: Vec<(u64, bool)> = list.iter().map(|m| (m.guid, m.listed_online())).collect();
    // A list with members carries the loot tail (`Group.cpp:155-180`).
    let loot = (!list.is_empty()).then_some(GroupLootInfo {
        method: 3,
        master: 0,
        threshold: 2,
    });
    group.apply_list_awaiting(
        group_type,
        0,
        list,
        ME,
        loot,
        Some(ME),
        &|guid| (!missing.contains(&guid)).then(String::new),
        &|_| Some(String::new()),
    );
    // The handler's record leg (`net::seat_new_records`): a member new to the roster is seated at
    // 1/1 with the row's online bit.
    for (guid, online) in seats {
        group
            .stats
            .entry(guid)
            .or_insert_with(|| PartyMemberStatsInfo::placeholder(online));
    }
}

/// `SMSG_PARTY_MEMBER_STATS` carrying member `i`'s status alone, as vmangos sends it on a ghost
/// toggle or a resurrection (`SpellAuras.cpp:5600`, `Player.cpp:15513`).
fn status(app: &mut App, i: u64, status: u8) {
    app.world_mut().resource_mut::<GroupState>().apply_stats(
        member(i),
        false,
        PartyMemberStatsInfo {
            status: Some(status),
            ..Default::default()
        },
    );
}

fn raid(app: &mut App, statuses: &[u8]) {
    send(app, GROUPTYPE_RAID, statuses, &[]);
}

fn party(app: &mut App, statuses: &[u8]) {
    send(app, 0, statuses, &[]);
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

/// `GetRaidRosterInfo`'s second return for row `row`.
fn rank(app: &mut App, row: u32) -> f64 {
    let script = app.world_mut().non_send_resource::<UiScript>();
    script
        .eval(&format!(
            "local _, rank = GetRaidRosterInfo({row}) return rank"
        ))
        .unwrap()
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
/// (`0x4babef`, with no compare against the old roster). The list writes only the record's online
/// bit (`0x4ba947`-`0x4ba960`), so the dead column follows the record, which a stats packet's
/// status moves.
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
    assert_eq!(dead(&mut app, 1), None, "the list's byte reaches no column");
    status(&mut app, 1, DEAD);
    frame(&mut app);
    assert_eq!(dead(&mut app, 1), Some(1.0), "the record's does");
    raid(&mut app, &[GHOST, ON]);
    assert_eq!(fired(&mut app), 1, "member 1 released");
    status(&mut app, 1, GHOST);
    frame(&mut app);
    assert_eq!(dead(&mut app, 1), None);
    assert_eq!(fired(&mut app), 0);
}

/// Nothing gates the writer's signal on a difference: a list that says what the last said signals.
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

/// `SMSG_NAME_QUERY_RESPONSE` for member `i`, a warrior: the cache files it, then the lookups the
/// roster left waiting run (`net::on_player_name`).
fn answer(app: &mut App, i: u64) {
    app.world_mut().resource_mut::<NameCache>().insert_player(
        member(i),
        format!("M{i}"),
        Some((1, 1, 0)),
    );
    let name = format!("M{i}");
    let _ = app
        .world_mut()
        .resource_mut::<GroupState>()
        .apply_player_name(member(i), Some(&name));
}

/// A frame that counts `PARTY_MEMBERS_CHANGED` and `RAID_ROSTER_UPDATE` in the order the VM is
/// told of them; [`told`] takes them.
fn watch_both(app: &mut App) {
    let script = app.world_mut().non_send_resource_mut::<UiScript>();
    script
        .run(
            r#"
            TOLD = ""
            local f = CreateFrame("Frame")
            f:RegisterEvent("RAID_ROSTER_UPDATE")
            f:RegisterEvent("PARTY_MEMBERS_CHANGED")
            f:SetScript("OnEvent", function()
                TOLD = TOLD .. (event == "RAID_ROSTER_UPDATE" and "R" or "P")
            end)
            "#,
        )
        .unwrap();
}

/// Run a frame and take what it told, `R` a roster update and `P` a members change, in order.
fn told(app: &mut App) -> String {
    frame(app);
    let script = app.world_mut().non_send_resource_mut::<UiScript>();
    let told: String = script.eval("return TOLD").unwrap();
    script.run("TOLD = \"\"").unwrap();
    told
}

/// A raid member in another subgroup whose name is still to come when the roster lands: the writer
/// counts it and signals nothing (`0x4babdf`), the row reads no class, and the answer counts the
/// last pending name down, signalling `RAID_ROSTER_UPDATE` and then `PARTY_MEMBERS_CHANGED`
/// (`0x4bad90`-`0x4badb0`), after which the row has its class. Holding no seat of ours, the
/// member's lookup is the roster's alone.
#[test]
fn a_raid_member_whose_name_arrives_after_the_roster_fires_on_the_answer() {
    let mut app = app();
    crate::ui_unit::TestNames::english().insert(&mut app);
    watch_both(&mut app);
    // Past the VM's first look, which answers what it finds rather than the signals.
    told(&mut app);
    let class = |app: &mut App| {
        flag(
            app,
            "(function() local _, _, _, _, class = GetRaidRosterInfo(2) \
             if class then return 1 end return 0 end)()",
        )
    };

    send_rows(&mut app, GROUPTYPE_RAID, &[(ON, 0), (ON, 1)], &[member(2)]);
    assert_eq!(
        told(&mut app),
        "P",
        "the list's members change, no roster yet"
    );
    assert_eq!(class(&mut app), Some(0.0), "no class before the answer");
    answer(&mut app, 1);
    assert_eq!(
        told(&mut app),
        "",
        "another member's answer waits on nothing"
    );
    answer(&mut app, 2);
    assert_eq!(told(&mut app), "RP", "the answer's two signals, in order");
    assert_eq!(class(&mut app), Some(1.0));
    answer(&mut app, 2);
    assert_eq!(told(&mut app), "", "nothing left waiting");

    // A list that lands while a name is pending cancels the old rows' lookups and counts afresh.
    let rows = [(ON, 0), (ON, 1), (ON, 1)];
    send_rows(&mut app, GROUPTYPE_RAID, &rows, &[member(3)]);
    assert_eq!(told(&mut app), "P");
    send_rows(&mut app, GROUPTYPE_RAID, &rows, &[]);
    assert_eq!(told(&mut app), "PR", "every name held: the list signals");
    answer(&mut app, 3);
    assert_eq!(told(&mut app), "", "its lookup was cancelled");
}

/// `RAID_ROSTER_UPDATE` has four sites (`0x4ba1a2`, `0x4ba57b`, `0x4babef`, `0x4bada6`), none in the
/// stats handler: a stats packet that takes a member offline moves the row `GetRaidRosterInfo`
/// reads and signals nothing.
#[test]
fn a_stats_packet_moving_a_rows_online_bit_fires_no_raid_roster_update() {
    let mut app = app();
    watch(&mut app);
    raid(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 1, "the list");
    let online = |app: &mut App| {
        flag(
            app,
            "(function() local _, _, _, _, _, _, _, online = GetRaidRosterInfo(1) \
             if online then return 1 end return 0 end)()",
        )
    };
    assert_eq!(online(&mut app), Some(1.0));
    status(&mut app, 1, member_status::OFFLINE);
    assert_eq!(fired(&mut app), 0, "no list, no answer");
    assert_eq!(online(&mut app), Some(0.0), "though the row reads it");
}

/// A kick ends the party at once (`0x5e6850`): `PARTY_MEMBERS_CHANGED` with the slots already
/// empty and no leader, and no `PARTY_LEADER_CHANGED`, as the leader store finds the 0 the
/// emptier wrote (`0x4e85a9`, `0x4e8267`); the all-zero list after it signals its members change
/// and leads nobody new.
#[test]
fn a_kick_empties_the_party_at_once_and_names_no_new_leader() {
    let mut app = app();
    let script = app.world_mut().non_send_resource_mut::<UiScript>();
    script
        .run(
            r#"
            TOLD = ""
            local f = CreateFrame("Frame")
            f:RegisterEvent("PARTY_MEMBERS_CHANGED")
            f:RegisterEvent("PARTY_LEADER_CHANGED")
            f:SetScript("OnEvent", function()
                local mark = event == "PARTY_LEADER_CHANGED" and "L" or "P"
                TOLD = TOLD .. mark .. GetNumPartyMembers()
            end)
            "#,
        )
        .unwrap();
    told(&mut app);
    party(&mut app, &[ON, ON]);
    assert_eq!(told(&mut app), "P2L2", "the list seats two, led by us");

    app.world_mut()
        .resource_mut::<GroupState>()
        .apply_uninvited();
    assert_eq!(told(&mut app), "P0", "the slots empty with the signal");
    assert_eq!(flag(&mut app, "UnitExists('party1')"), None);
    assert_eq!(flag(&mut app, "IsPartyLeader()"), None);
    all_zero(&mut app);
    assert_eq!(told(&mut app), "P0", "the list's own, and no leader");
}

/// `RAID_TARGET_UPDATE` follows its signals one for one: each raid-target packet, a mark that
/// moves nothing included (`0x4ba348`), and each party list, which clears the board (`0x4ba5db`).
#[test]
fn raid_target_update_fires_per_packet_and_per_party_list() {
    let mut app = app();
    let script = app.world_mut().non_send_resource_mut::<UiScript>();
    script
        .run(
            r#"
            MARKS = 0
            local f = CreateFrame("Frame")
            f:RegisterEvent("RAID_TARGET_UPDATE")
            f:SetScript("OnEvent", function() MARKS = MARKS + 1 end)
            "#,
        )
        .unwrap();
    let marks = |app: &mut App| {
        frame(app);
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let n: f64 = script.eval("return MARKS").unwrap();
        script.run("MARKS = 0").unwrap();
        n as u32
    };
    marks(&mut app);
    party(&mut app, &[ON]);
    assert_eq!(marks(&mut app), 1, "the party's list");
    let mark = |app: &mut App| {
        app.world_mut()
            .resource_mut::<GroupState>()
            .apply_raid_target_update(7, member(1));
    };
    mark(&mut app);
    mark(&mut app);
    assert_eq!(
        marks(&mut app),
        2,
        "one per packet, the second moving nothing"
    );
    party(&mut app, &[ON]);
    assert_eq!(marks(&mut app), 1, "the list clears the mark");
    assert_eq!(flag(&mut app, "GetRaidTargetIndex('party1')"), None);
}

/// A party member whose name is still to come: each list that seats them waits on it, and the
/// answer signals `PARTY_MEMBERS_CHANGED` once for each (`0x4e8467`-`0x4e8495`).
#[test]
fn a_party_member_whose_name_arrives_after_the_roster_fires_once_per_list() {
    let mut app = app();
    watch_both(&mut app);
    told(&mut app);
    send(&mut app, 0, &[ON], &[member(1)]);
    send(&mut app, 0, &[ON], &[member(1)]);
    assert_eq!(told(&mut app), "PP", "the two lists");
    answer(&mut app, 1);
    assert_eq!(told(&mut app), "PP", "one per seat that waited");
    answer(&mut app, 1);
    assert_eq!(told(&mut app), "");
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

/// Our own row comes with the list that names a member (`0x5e6e73`), last, so our object's arrival
/// moves no row and signals nothing: the world entry's fill (`0x4ba140`) finds no zero row, as
/// vmangos sends our create ahead of any list (`Map.cpp:432`, `Group.cpp:1454`).
#[test]
fn our_own_row_comes_with_the_list_and_our_arrival_fires_nothing() {
    let mut app = app();
    watch(&mut app);
    raid(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 1, "the list");
    assert_eq!(rows(&mut app), 3.0, "the two, then us");

    app.world_mut().spawn((
        Guid(ME),
        ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[])),
        SelfPlayer,
    ));
    assert_eq!(fired(&mut app), 0, "our object moves no row");
    assert_eq!(rows(&mut app), 3.0);
}

/// A raid with us in a later group (`own_flags` subgroup 3, stored 2): the rows are the wire's
/// members as listed, then us (`0x5e6e73`), so `raid1` is the first listed, our row is the last
/// with our own subgroup and rank, and the party slots are our subgroup's members alone.
#[test]
fn in_a_later_group_we_are_the_last_raid_row() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_player(ME, "Me".into(), None);
    let listed = |i: u64, subgroup: u8| GroupMemberEntry {
        name: format!("M{i}"),
        guid: member(i),
        status: ON,
        flags: subgroup,
    };
    app.world_mut().resource_mut::<GroupState>().apply_list(
        GROUPTYPE_RAID,
        2,
        vec![listed(1, 0), listed(2, 2), listed(3, 1)],
        member(1),
        None,
        Some(ME),
    );
    frame(&mut app);
    let script = app.world_mut().non_send_resource::<UiScript>();
    let row = |i: u32| -> (String, f64, f64) {
        script
            .eval(&format!(
                "local name, rank, subgroup = GetRaidRosterInfo({i}) return name, rank, subgroup"
            ))
            .unwrap()
    };
    assert_eq!(
        script.eval::<f64>("return GetNumRaidMembers()").unwrap(),
        4.0
    );
    assert_eq!(
        row(1),
        ("M1".into(), 2.0, 1.0),
        "the first listed, our leader"
    );
    assert_eq!(row(3), ("M3".into(), 0.0, 2.0));
    assert_eq!(row(4), ("Me".into(), 0.0, 3.0), "us, last, in group 3");
    for (token, want) in [("raid1", "M1"), ("raid2", "M2"), ("raid4", "Me")] {
        assert_eq!(
            script
                .eval::<String>(&format!("return UnitName({token:?})"))
                .unwrap(),
            want,
            "{token}"
        );
    }
    assert_eq!(
        script
            .eval::<String>(r#"return UnitName("party1")"#)
            .unwrap(),
        "M2",
        "our subgroup's one member"
    );
    assert!(script
        .eval::<bool>(r#"return UnitName("party2") == nil"#)
        .unwrap());
}

/// A raid list that names nobody seats no row, ours included (`0x5e6e71`): the writer still
/// signals (`0x4babef`), the roster reads empty, and the party list after it finds no roster to
/// drop (`0x4ba556`).
#[test]
fn a_raid_of_one_reads_no_rows_and_drops_none() {
    let mut app = app();
    watch(&mut app);
    // Past the VM's first look, which answers what it finds rather than the lists.
    fired(&mut app);
    raid(&mut app, &[]);
    assert_eq!(fired(&mut app), 1, "the writer ran");
    assert_eq!(rows(&mut app), 0.0, "not even our own row");
    party(&mut app, &[]);
    assert_eq!(fired(&mut app), 0, "no roster held, none dropped");
}

/// A worldport takes our object and gives it back; the roster's rows hold guids, ours the list's,
/// so neither edge moves a row and neither signals (vmangos sends no list on it).
#[test]
fn a_worldport_in_a_raid_fires_no_raid_roster_update() {
    let mut app = app();
    watch(&mut app);
    let avatar = |app: &mut App| {
        app.world_mut()
            .spawn((
                Guid(ME),
                ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[])),
                SelfPlayer,
            ))
            .id()
    };
    let me = avatar(&mut app);
    raid(&mut app, &[ON, ON]);
    assert_eq!(fired(&mut app), 1, "the list");
    assert_eq!(rows(&mut app), 3.0);

    app.world_mut().despawn(me);
    assert_eq!(fired(&mut app), 0, "our object leaves");
    assert_eq!(rows(&mut app), 3.0, "our row stays");
    avatar(&mut app);
    assert_eq!(fired(&mut app), 0, "and comes back");
    assert_eq!(rows(&mut app), 3.0);
}

/// A VM minted after the lists landed was told of none: the raid it finds gets its one catch-up,
/// never the history, and a player who holds no raid gets nothing, however many lists ended raids
/// before.
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

/// One drain's packets signal in wire order, each inside its own handler: a raid list, the leader
/// change vmangos sends as `SMSG_GROUP_SET_LEADER` and then a list naming the new leader
/// (`Group::ChangeLeader`), and a mark. The VM hears the first list's members change, leader,
/// loot and roster, then the second's, then the mark (`0x5e6c66`, `0x4e82c2`, `0x4e87fa`,
/// `0x4babef`, `0x4ba348`), never the events gathered by kind.
#[test]
fn one_drains_packets_signal_in_wire_order() {
    use benilla_protocol::SessionEvent;
    let mut app = app();
    app.init_resource::<crate::ui_action::UiErrorKeys>()
        .init_resource::<crate::sound::MessageSounds>()
        .init_resource::<crate::ui_quest::QuestGiver>()
        .insert_resource(crate::net::SelfGuid(Some(ME)));
    super::super::net::register(&mut app);
    for (guid, name) in [(ME, "Me"), (member(1), "M1")] {
        app.world_mut().resource_mut::<NameCache>().insert_player(
            guid,
            name.into(),
            Some((1, 1, 0)),
        );
    }
    let script = app.world_mut().non_send_resource_mut::<UiScript>();
    script
        .run(
            r#"
            TOLD = ""
            local marks = { PARTY_MEMBERS_CHANGED = "P", PARTY_LEADER_CHANGED = "L",
                            PARTY_LOOT_METHOD_CHANGED = "O", RAID_ROSTER_UPDATE = "R",
                            RAID_TARGET_UPDATE = "T" }
            local f = CreateFrame("Frame")
            for e in pairs(marks) do f:RegisterEvent(e) end
            f:SetScript("OnEvent", function() TOLD = TOLD .. marks[event] end)
            "#,
        )
        .unwrap();
    // Past the VM's first look, which answers what it finds rather than the signals.
    told(&mut app);
    let list = |leader| SessionEvent::GroupList {
        group_type: GROUPTYPE_RAID,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: "M1".into(),
            guid: member(1),
            status: ON,
            flags: 1,
        }],
        leader,
        loot: Some(GroupLootInfo {
            method: 3,
            master: 0,
            threshold: 2,
        }),
    };
    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![
            list(ME),
            SessionEvent::GroupLeaderChanged { name: "M1".into() },
            list(member(1)),
            SessionEvent::RaidTargetSet {
                icon: 7,
                guid: member(1),
            },
        ],
    );
    assert_eq!(told(&mut app), "PLORPLORT");
    assert_eq!(told(&mut app), "", "each once");
}

/// The sandbox stands in for the server, whose echo of each group intent is a list: a raid's list
/// rows the members then us and signals `RAID_ROSTER_UPDATE` though the intent moves nobody (a
/// loot threshold), the one that ends the raid signals once, and a party's list signals none.
#[test]
fn the_sandbox_answers_a_raid_intent_with_a_raid_roster_update() {
    let mut app = app();
    watch(&mut app);
    synthetic_roster(&mut app.world_mut().resource_mut::<GroupState>(), None);
    assert_eq!(fired(&mut app), 0, "a party's list");

    let intent = |app: &mut App, req: PartyRequest| {
        let mut group = app.world_mut().resource_mut::<GroupState>();
        assert!(test_apply_local(&mut group, &req, Some(ME)));
    };
    intent(&mut app, PartyRequest::LootThreshold(4));
    assert_eq!(fired(&mut app), 0, "a party intent");
    intent(&mut app, PartyRequest::ConvertToRaid);
    assert_eq!(fired(&mut app), 1, "the party became a raid");
    assert_eq!(rows(&mut app), 5.0, "its four, then us");
    intent(&mut app, PartyRequest::LootThreshold(4));
    assert_eq!(fired(&mut app), 1, "a raid intent that moves no member");
    intent(&mut app, PartyRequest::PromoteGuid(0xF002));
    assert_eq!(
        fired(&mut app),
        1,
        "once, though the leader's rank moved too"
    );
    assert_eq!(
        (rank(&mut app, 1), rank(&mut app, 2)),
        (0.0, 2.0),
        "row 2 leads"
    );
    super::echo_raid_target(&mut app.world_mut().resource_mut::<GroupState>(), 0, ME);
    assert_eq!(fired(&mut app), 0, "a mark is no list");
    intent(&mut app, PartyRequest::Leave);
    assert_eq!(fired(&mut app), 1, "the raid ended");
}

/// The stock raid UI, loaded whole off the player's own chain: a raid member dies out of view, their
/// record takes the dead bit, and the list that follows moves nothing but their status byte. The
/// raid tab's grid also repaints on `PARTY_MEMBERS_CHANGED` (`RaidFrame_OnEvent`), which every list
/// signals, but a group pullout re-reads its rows on `RAID_ROSTER_UPDATE` alone
/// (`RaidPullout_OnEvent`); this harness writes the record with no unit events, so no
/// `UNIT_HEALTH` repaints the row in its place.
#[test]
fn the_stock_raid_ui_repaints_a_member_who_dies_out_of_view() {
    benilla_formats::wow_data_or_skip!();
    let mut script = UiScript::new().unwrap();
    script.set_screen_size(1024.0, 768.0);
    let failures = crate::ui_script::load_default_ui(&script);
    assert!(failures.is_empty(), "load failures: {failures:?}");
    crate::ui_script::test_ui::seat_chain_addon(&mut script, "Blizzard_RaidUI");
    let mut app = app_with(script);
    crate::ui_unit::TestNames::english().insert(&mut app);
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

    status(&mut app, 1, DEAD);
    raid(&mut app, &[DEAD, ON]);
    frame(&mut app);
    assert_eq!(text_colour(&mut app), red, "the grid repaints");
    assert_eq!(pullout_colour(&mut app), red, "and so does the pullout");

    status(&mut app, 1, ON);
    raid(&mut app, &[ON, ON]);
    frame(&mut app);
    assert_eq!(text_colour(&mut app), warrior);
    assert_eq!(pullout_colour(&mut app), warrior);
    let errors = app.world().non_send_resource::<UiScript>().errors();
    assert!(errors.is_empty(), "script errors: {errors:?}");
}
