//! What a listener reads of the group and the pet, as its own packet left them. The group
//! handlers write the party slots and the raid rows themselves (`0x4e82d0`, `0x4e84a0`, the raid
//! writer `0x4ba5f0`) before they signal (`0x5e6c66`), a stats packet its member's record, and the
//! getters and the relation functions read those tables, the records and the objects at the call
//! (`0x5174d0`, `0x6062b0`-`0x60631c` through `0x4e7f70`), so no copy is pushed ahead of the drain.

use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{member_status, GroupMemberEntry, PartyMemberStatsInfo};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::{UiScript, UnitGuids};
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
    SessionEvent::GroupList {
        group_type: 0,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: "Brisca".into(),
            guid: MATE,
            status: member_status::ONLINE,
            flags: 0,
        }],
        leader: ME,
        loot: None,
    }
}

/// The relation a `PARTY_MEMBERS_CHANGED` listener reads off a list that seats an enemy-faction
/// player is the party's: the active player toward a party member is friendly (`0x6062b0`), which
/// the list's own party slots answer, not the slots of the frame before.
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
                table.insert(Seen, event .. ":" .. tostring(UnitReaction("player", "party1")))
            end)
            "#,
        )
        .unwrap();
    let seen = frame(&mut app, vec![party_list()]);
    assert!(!seen.is_empty(), "the list signals");
    assert!(
        seen.iter().all(|s| s == "PARTY_MEMBERS_CHANGED:5"),
        "friendly to its new party member at every signal: {seen:?}"
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
    // The resolver's inputs an event's token walk reads (`0x515c50`), as the feeds push them
    // ahead of the drain: the frame's last rows.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_unit_guids(&UnitGuids {
            player: ME,
            party: [MATE, 0, 0, 0],
            raid: vec![MATE, CORA, DUNN, ME],
            ..Default::default()
        });
    let seen = frame(
        &mut app,
        vec![mate_hp(40), raid_list([CORA, DUNN]), mate_hp(30)],
    );
    let seen: Vec<_> = seen.iter().filter(|s| !s.starts_with("UNIT_")).collect();
    assert_eq!(seen, ["40:Bram", "30:Dunn"]);
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
