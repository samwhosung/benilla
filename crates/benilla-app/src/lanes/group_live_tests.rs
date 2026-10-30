//! What a listener reads of the group and the pet, as its own packet left them. The group
//! handlers write the party slots and the raid rows themselves (`0x4e82d0`, `0x4e84a0`, the raid
//! writer `0x4ba5f0`) before they signal (`0x5e6c66`), a stats packet its member's record, and the
//! getters and the relation functions read those tables, the records and the objects at the call
//! (`0x5174d0`, `0x6062b0`-`0x60631c` through `0x4e7f70`), so no copy is pushed ahead of the drain.

use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{member_status, GroupMemberEntry, PartyMemberStatsInfo};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, created, fall, frame, values};
use crate::target::relations::fixture::{factions, HUMAN, ORC, PC};

const ME: u64 = 0x7;
/// A player of the other faction, held.
const MATE: u64 = 0x22;
/// `UNIT_FIELD_FACTIONTEMPLATE`.
const TEMPLATE: u16 = 35;

/// `SMSG_GROUP_LIST` for a party of us, leading, and [`MATE`].
fn party_list() -> SessionEvent {
    party_of(MATE)
}

/// `SMSG_GROUP_LIST` for a party of us, leading, and `member`.
fn party_of(member: u64) -> SessionEvent {
    SessionEvent::GroupList {
        group_type: 0,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: String::new(),
            guid: member,
            status: member_status::ONLINE,
            flags: 0,
        }],
        leader: ME,
        loot: None,
    }
}

/// The relation a `PARTY_MEMBERS_CHANGED` listener reads off a list that seats an enemy-faction
/// player is the party's: the active player toward a party member is friendly (`0x6062b0`), which
/// the list's own party slots answer, not the slots of the frame before nor of a later list in
/// the same frame that seats another member, out of view, in the slot.
#[test]
fn a_group_list_listener_reads_the_roster_its_list_wrote() {
    let mut app = client();
    app.insert_resource(factions());
    frame(
        &mut app,
        vec![
            values(ME, &[(TEMPLATE, HUMAN), (FIELD_UNIT_FLAGS, PC)]),
            created(
                MATE,
                EntityKind::Player,
                &[
                    (TEMPLATE, ORC),
                    (FIELD_UNIT_FLAGS, PC),
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                ],
            ),
        ],
    );
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("PARTY_MEMBERS_CHANGED")
            f:SetScript("OnEvent", function()
                table.insert(Seen, "reaction:" .. tostring(UnitReaction("player", "party1")))
            end)
            "#,
        )
        .unwrap();
    let seen = frame(&mut app, vec![party_list(), party_of(ALIA)]);
    let seen: Vec<_> = seen.iter().filter(|s| s.starts_with("reaction:")).collect();
    let friendly = seen
        .iter()
        .take_while(|s| s.as_str() == "reaction:5")
        .count();
    assert!(
        friendly > 0,
        "the first list's listeners, friendly: {seen:?}"
    );
    assert!(
        friendly < seen.len()
            && seen[friendly..]
                .iter()
                .all(|s| s.as_str() == "reaction:nil"),
        "then the second list's, whose member is out of view: {seen:?}"
    );
}

/// Raid members out of view: the rows after [`MATE`]'s, two lists apart.
const ALIA: u64 = 0x31;
const BRAM: u64 = 0x32;
const CORA: u64 = 0x33;
const DUNN: u64 = 0x34;

/// `SMSG_GROUP_LIST` for a raid of us, leading, [`MATE`] in our subgroup and `rest` in the next.
fn raid_list(rest: [u64; 2]) -> SessionEvent {
    let entry = |guid: u64, subgroup: u8| GroupMemberEntry {
        name: String::new(),
        guid,
        status: member_status::ONLINE,
        flags: subgroup,
    };
    SessionEvent::GroupList {
        group_type: crate::ui_party::GROUPTYPE_RAID,
        own_flags: 0,
        members: vec![entry(MATE, 0), entry(rest[0], 1), entry(rest[1], 1)],
        leader: ME,
        loot: None,
    }
}

/// `SMSG_PARTY_MEMBER_STATS` for [`MATE`], its health alone.
fn mate_hp(cur: u16) -> SessionEvent {
    SessionEvent::PartyMemberStats {
        guid: MATE,
        full: false,
        info: Box::new(PartyMemberStatsInfo {
            cur_hp: Some(cur),
            ..Default::default()
        }),
    }
}

/// A listener inside a member's stats packet reads `party1`'s health off the record that packet
/// wrote (`0x5e54c4` before `0x5e54fa`; `0x5174d0` reads the record for a member with no object,
/// `0x496400`) and `raid3` off the rows the list before it wrote (`0x4ba5f0`), the name by its
/// guid (`0x5171eb`): each token resolves at the call, with no copy pushed ahead of the drain.
#[test]
fn a_member_update_listener_reads_party1_and_raid3_as_its_packet_wrote_them() {
    let mut app = client();
    {
        let mut names = app.world_mut().resource_mut::<crate::names::NameCache>();
        for (guid, name) in [
            (MATE, "Brisca"),
            (ALIA, "Alia"),
            (BRAM, "Bram"),
            (CORA, "Cora"),
            (DUNN, "Dunn"),
        ] {
            names.insert_player(guid, name.into(), None);
        }
    }
    frame(
        &mut app,
        vec![
            raid_list([ALIA, BRAM]),
            SessionEvent::PartyMemberStats {
                guid: MATE,
                full: true,
                info: Box::new(PartyMemberStatsInfo {
                    status: Some(member_status::ONLINE),
                    cur_hp: Some(100),
                    max_hp: Some(100),
                    ..Default::default()
                }),
            },
        ],
    );
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_HEALTH")
            f:SetScript("OnEvent", function()
                if arg1 == "party1" then
                    table.insert(Seen, UnitHealth("party1") .. ":" .. tostring(UnitName("raid3")))
                end
            end)
            "#,
        )
        .unwrap();
    let seen = frame(
        &mut app,
        vec![mate_hp(40), raid_list([CORA, DUNN]), mate_hp(30)],
    );
    let seen: Vec<_> = seen.iter().filter(|s| !s.starts_with("UNIT_")).collect();
    assert_eq!(seen, ["40:Bram", "30:Dunn"]);
}

/// A member's `UNIT_HEALTH` names the tokens its own packet's roster gives it: `0x515e50` walks the
/// party slots and the raid rows at the signal (`0x515c50`, through `0x491920` and `0x491940`).
/// So [`MATE`], moved out of our subgroup by a list later in the frame, is `party1` and `raid1` at
/// the first stats packet; at the second, after a list that seats it third, `raid3` alone.
#[test]
fn a_member_health_event_names_the_tokens_its_packets_roster_gives() {
    let mut app = client();
    let list = |rows: [(u64, u8); 3]| SessionEvent::GroupList {
        group_type: crate::ui_party::GROUPTYPE_RAID,
        own_flags: 0,
        members: rows
            .iter()
            .map(|&(guid, subgroup)| GroupMemberEntry {
                name: String::new(),
                guid,
                status: member_status::ONLINE,
                flags: subgroup,
            })
            .collect(),
        leader: ME,
        loot: None,
    };
    frame(
        &mut app,
        vec![
            list([(MATE, 0), (ALIA, 1), (BRAM, 1)]),
            SessionEvent::PartyMemberStats {
                guid: MATE,
                full: true,
                info: Box::new(PartyMemberStatsInfo {
                    status: Some(member_status::ONLINE),
                    cur_hp: Some(100),
                    max_hp: Some(100),
                    ..Default::default()
                }),
            },
        ],
    );
    let seen = frame(
        &mut app,
        vec![
            mate_hp(40),
            list([(ALIA, 1), (BRAM, 1), (MATE, 1)]),
            mate_hp(30),
        ],
    );
    assert_eq!(
        seen,
        [
            "UNIT_HEALTH:party1:40",
            "UNIT_HEALTH:raid1:40",
            "UNIT_HEALTH:raid3:30"
        ]
    );
}

/// Our pet, `UNIT_FIELD_SUMMON`'s, and the one that replaces it.
const PET: u64 = 0xF140_0000_0045_0002;
const PET2: u64 = 0xF140_0000_0045_0003;
/// `UNIT_FIELD_SUMMON`.
const SUMMON: u16 = 8;

fn summons(guid: u64) -> [(u16, u32); 2] {
    [(SUMMON, guid as u32), (SUMMON + 1, (guid >> 32) as u32)]
}

/// A listener reads `"pet"` as its own packet left it: the token is our `UNIT_FIELD_SUMMON` at the
/// call (`0x515a5e`-`0x515a6e`) and the getter reads that object (`0x5174d0`). So a fall ahead of
/// the pet's update reads the pet before it, the update's own `UNIT_HEALTH` its health, and the
/// pet that replaces it later in the frame answers only after its summon.
#[test]
fn a_pet_update_listener_reads_pet_as_its_packet_left_it() {
    let mut app = client();
    let pet = |guid: u64, hp: u32, max: u32| {
        created(
            guid,
            EntityKind::Unit,
            &[(FIELD_UNIT_HEALTH, hp), (FIELD_UNIT_MAXHEALTH, max)],
        )
    };
    frame(
        &mut app,
        vec![values(ME, &summons(PET)), pet(PET, 100, 100)],
    );
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_HEALTH")
            f:RegisterEvent("UNIT_COMBAT")
            f:SetScript("OnEvent", function()
                if arg1 == "pet" or event == "UNIT_COMBAT" then
                    table.insert(Seen, "pet " .. event .. " " .. UnitHealth("pet") .. ":"
                        .. UnitHealthMax("pet"))
                end
            end)
            "#,
        )
        .unwrap();
    let seen = frame(
        &mut app,
        vec![
            fall(1),
            values(PET, &[(FIELD_UNIT_HEALTH, 70)]),
            values(ME, &summons(PET2)),
            pet(PET2, 20, 40),
            values(PET2, &[(FIELD_UNIT_HEALTH, 15)]),
        ],
    );
    let seen: Vec<_> = seen.iter().filter(|s| s.starts_with("pet ")).collect();
    assert_eq!(
        seen,
        [
            "pet UNIT_COMBAT 100:100",
            "pet UNIT_HEALTH 70:100",
            "pet UNIT_HEALTH 15:40"
        ]
    );
}

/// A getter's read of a member whose name the cache lacks asks the server inside the call, once
/// while the ask is pending: `UnitName` of a member with no object reads the cache by guid with a
/// callback (`0x5171eb`, `0x517290`), and the tooltip's name of a held one is `GetUnitName`'s
/// (`0x609210`, its player cache asked at `0x60925e`). The list asked each name it seated
/// (`0x4e8477`); a server's `SMSG_INVALIDATE_PLAYER` drops it again (`0x555600`).
#[test]
fn a_members_uncached_name_is_asked_once_by_the_getter() {
    let mut app = client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let asked = |rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>| {
        rx.try_iter()
            .filter(|c| matches!(c, crate::net::ClientCommand::NameQuery { guid } if *guid == MATE))
            .count()
    };
    let forget = |app: &mut App| {
        app.world_mut()
            .resource_mut::<crate::names::NameCache>()
            .invalidate_player(MATE);
    };
    frame(&mut app, vec![party_list()]);
    assert_eq!(asked(&rx), 1, "the list's lookup");
    forget(&mut app);
    let run = |app: &mut App, chunk: &str| {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM").run(chunk).unwrap()
        })
    };
    run(&mut app, r#"UnitName("party1") UnitName("party1")"#);
    assert_eq!(asked(&rx), 1, "the member with no object, once");

    frame(
        &mut app,
        vec![created(
            MATE,
            EntityKind::Player,
            &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)],
        )],
    );
    let _ = asked(&rx);
    forget(&mut app);
    crate::ui_script::test_ui::load_tooltip_template(app.world().non_send_resource::<UiScript>());
    run(
        &mut app,
        r#"
        Tip = CreateFrame("GameTooltip", "Tip", nil, "TestTooltipTemplate")
        Tip:SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")
        Tip:SetUnit("party1")
        Tip:SetUnit("party1")
        "#,
    );
    assert_eq!(asked(&rx), 1, "the held member's tooltip, once");
}

/// The unit tooltip finds the group record by guid for whatever token shows the unit (`0x52a02b`,
/// through `0x496400`): a raid member off our party slots reads its raid row's record, so an
/// offline one ends on `PLAYER_OFFLINE` (`0x52a6e2`-`0x52a72b`); off the roster the record a
/// member left behind is found by no row, and the tooltip reads none.
#[test]
fn the_tooltip_reads_a_raid_rows_record_and_none_off_the_roster() {
    let mut app = client();
    let raid = |rows: &[(u64, u8)]| SessionEvent::GroupList {
        group_type: crate::ui_party::GROUPTYPE_RAID,
        own_flags: 0,
        members: rows
            .iter()
            .map(|&(guid, subgroup)| GroupMemberEntry {
                name: String::new(),
                guid,
                status: member_status::OFFLINE,
                flags: subgroup,
            })
            .collect(),
        leader: ME,
        loot: None,
    };
    frame(
        &mut app,
        vec![
            raid(&[(MATE, 1), (ALIA, 1)]),
            created(
                MATE,
                EntityKind::Player,
                &[
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                    (FIELD_UNIT_FLAGS, PC),
                ],
            ),
            SessionEvent::PartyMemberStats {
                guid: MATE,
                full: true,
                info: Box::new(PartyMemberStatsInfo {
                    status: Some(member_status::OFFLINE),
                    ..Default::default()
                }),
            },
        ],
    );
    let world = app.world_mut();
    crate::ui_script::test_ui::load_tooltip_template(world.non_send_resource::<UiScript>());
    let lines = |app: &mut App| -> Vec<String> {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM")
                .eval(
                    r#"
                    PLAYER_OFFLINE = "[OFFLINE]"
                    Tip = Tip or CreateFrame("GameTooltip", "Tip", nil, "TestTooltipTemplate")
                    Tip:SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")
                    Tip:SetUnit("raid1")
                    local out = {}
                    for i = 2, 30 do
                        local line = getglobal("TipTextLeft" .. i)
                        if not line or not line:GetText() then break end
                        table.insert(out, line:GetText())
                    end
                    return out
                    "#,
                )
                .unwrap()
        })
    };
    assert!(
        lines(&mut app).iter().any(|l| l == "[OFFLINE]"),
        "the raid row's record"
    );
    // MATE leaves, and a late stats packet stores a record no row names: the held unit is shown
    // as itself.
    frame(
        &mut app,
        vec![
            raid(&[(ALIA, 1), (BRAM, 1)]),
            SessionEvent::PartyMemberStats {
                guid: MATE,
                full: true,
                info: Box::new(PartyMemberStatsInfo {
                    status: Some(member_status::OFFLINE),
                    ..Default::default()
                }),
            },
        ],
    );
    *app.world_mut().resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: app
            .world()
            .resource::<crate::net::GuidIndex>()
            .0
            .get(&MATE)
            .copied(),
        guid: Some(MATE),
        ..Default::default()
    };
    let off = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM")
            .eval::<Vec<String>>(
                r#"
                Tip:SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")
                Tip:SetUnit("target")
                local out = {}
                for i = 2, 30 do
                    local line = getglobal("TipTextLeft" .. i)
                    if not line or not line:GetText() then break end
                    table.insert(out, line:GetText())
                end
                return out
                "#,
            )
            .unwrap()
    });
    assert!(
        app.world()
            .resource::<crate::ui_party::GroupState>()
            .stats
            .contains_key(&MATE),
        "the late record is stored"
    );
    assert!(
        !off.iter().any(|l| l == "[OFFLINE]"),
        "off the roster: {off:?}"
    );
}

/// [`MATE`]'s name answer.
fn mate_named() -> SessionEvent {
    SessionEvent::PlayerName {
        guid: MATE,
        name: "Bob".into(),
        race: 1,
        class: 1,
        gender: 0,
    }
}

/// The party with [`MATE`] in it, held as `held` says, its name not cached, a `UNIT_NAME_UPDATE`
/// listener noting the token and the name it reads, and `UnitName("party1")` run twice: the
/// events [`mate_named`] then signals.
fn name_update_after_unit_name(held: bool) -> Vec<String> {
    let mut app = client();
    frame(&mut app, vec![party_list()]);
    if held {
        frame(
            &mut app,
            vec![created(
                MATE,
                EntityKind::Player,
                &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)],
            )],
        );
    }
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .invalidate_player(MATE);
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_NAME_UPDATE")
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. " " .. arg1 .. " " .. tostring(UnitName(arg1)))
            end)
            UnitName("party1") UnitName("party1")
            Seen = {}
            "#,
        )
    })
    .expect("the listener and the reads");
    frame(&mut app, vec![mate_named()])
}

/// `UnitName` of a member it holds no unit for keeps its callback on the miss, once (`0x5171f1`,
/// `0x5171f5` with `0x517290`), and the found name signals `UNIT_NAME_UPDATE` to the tokens naming
/// the guid though no unit is held: `0x491a00` skips only the refresh without one (`0x491a23`),
/// never the signal (`0x491a3c`). So a raid frame's member out of view leaves "Unknown".
#[test]
fn a_unit_name_miss_signals_its_name_for_a_member_out_of_view() {
    assert_eq!(
        name_update_after_unit_name(false),
        ["UNIT_NAME_UPDATE party1 Bob"]
    );
}

/// The control: a held member's `UnitName` reads it through `GetUnitName` (`0x609210`), whose
/// callback is the unit's own (`0x5fb840`), so the answer signals once.
#[test]
fn a_held_members_name_answer_signals_once() {
    assert_eq!(
        name_update_after_unit_name(true),
        ["UNIT_NAME_UPDATE party1 Bob"]
    );
}
