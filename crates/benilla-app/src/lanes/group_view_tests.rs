//! What a listener reads of a group unit, as the packets before it left it. A stats packet's
//! handler writes the member's record, then signals what changed (`0x5e5110`: the record copies at
//! `0x5e54c4` and `0x5e54d8`, then `0x5e54fa` → `0x5e5720`), and `UnitHealth` reads the record for a
//! member with no object (`0x5174d0`). An update packet writes the object layer before its notify
//! pass (`0x4651a0`), and `UnitExists` reads the token's object or roster leg at the call
//! (`0x515ff8` → `0x491900`). So each listener reads its own packet's state.

use benilla_protocol::messages::{
    member_status, GroupLootInfo, GroupMemberEntry, PartyMemberStatsInfo,
};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::{UiScript, UnitGuids};
use bevy::prelude::*;

use super::unit_view_tests::{client, created, fall, values};

const ME: u64 = 0x7;
const WOLF: u64 = 0xF130_0000_0044_0001;
const BOAR: u64 = 0xF130_0000_0045_0002;
/// A party member, a player.
const MATE: u64 = 0x22;
/// The member's pet.
const PET: u64 = 0xF140_0000_0000_0077;
/// The pet that replaces it.
const PET2: u64 = 0xF140_0000_0000_0078;
/// A creature a member targets, typed as a unit so a chain may end on it.
const ADD: u64 = 0xF130_0000_0046_0003;
/// `OBJECT_FIELD_TYPE` and a unit's type mask (object and unit bits).
const OBJECT_TYPE: u16 = 2;
const TYPE_UNIT: u32 = 0x9;
/// `UNIT_FIELD_SUMMON`, the pet a member's descriptor names (`0x4e81d0`).
const SUMMON: u16 = 8;

/// `UNIT_FIELD_SUMMON` naming `guid`.
fn summons(guid: u64) -> [(u16, u32); 2] {
    [(SUMMON, guid as u32), (SUMMON + 1, (guid >> 32) as u32)]
}

/// A stats packet for [`MATE`] carrying `info`.
fn stats(full: bool, info: PartyMemberStatsInfo) -> SessionEvent {
    SessionEvent::PartyMemberStats {
        guid: MATE,
        full,
        info: Box::new(info),
    }
}

/// [`MATE`]'s health in its record.
fn hp(cur: u16) -> SessionEvent {
    stats(
        false,
        PartyMemberStatsInfo {
            cur_hp: Some(cur),
            ..Default::default()
        },
    )
}

/// The resolver's seat: [`super::unit_view_tests::seat`]'s, with [`MATE`] as `party1` and `pet`
/// as `partypet1`.
fn seat_party(app: &mut App, pet: u64) {
    seat_party_targeting(app, pet, 0);
}

/// [`seat_party`], with a held [`MATE`]'s target `target`.
fn seat_party_targeting(app: &mut App, pet: u64, target: u64) {
    let mut held: std::collections::HashMap<u64, u64> =
        [(ME, 0), (WOLF, 0), (BOAR, 0)].into_iter().collect();
    if app
        .world()
        .resource::<crate::net::GuidIndex>()
        .0
        .contains_key(&MATE)
    {
        held.insert(MATE, target);
    }
    if pet != 0 {
        held.insert(pet, 0);
    }
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_unit_guids(&UnitGuids {
            player: ME,
            target: WOLF,
            party: [MATE, 0, 0, 0],
            party_pets: [pet, 0, 0, 0],
            held,
            ..Default::default()
        });
}

/// One frame: the packets, the unit, party, pet and chain feeds' pushes of the frame's state,
/// then the lanes' drain; what the spy saw.
fn frame(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    frame_after(app, |_| {}, packets)
}

/// [`frame`], with `before` run on the world between the frame-start snapshot and the packets.
fn frame_after(
    app: &mut App,
    before: impl FnOnce(&mut World),
    packets: Vec<SessionEvent>,
) -> Vec<String> {
    let world = app.world_mut();
    #[cfg(debug_assertions)]
    {
        use bevy::ecs::system::RunSystemOnce;
        world
            .run_system_once(crate::ui_unit::views::mark_frame_start)
            .unwrap();
    }
    before(world);
    crate::net::handlers::dispatch(world, packets);
    crate::ui_unit::run_feed_units_once(world);
    world
        .run_system_cached(crate::ui_party::feed_party)
        .expect("the party feed runs");
    world
        .run_system_cached(crate::ui_pet::feed_pet_unit)
        .expect("the pet feed runs");
    crate::ui_unit::run_feed_chain_units_once(world);
    crate::lanes::fire_in_order(world);
    let s = world.non_send_resource_mut::<UiScript>();
    let out = s.eval::<Vec<String>>("return Seen").unwrap();
    s.run("Seen = {}").unwrap();
    let errors = s.errors();
    assert!(errors.is_empty(), "script errors: {errors:?}");
    out
}

/// [`client`] in a party with [`MATE`], online at 100 of 100 health, whose object the client does
/// not hold.
fn party_client() -> App {
    let mut app = client();
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    frame(
        &mut app,
        vec![
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
            },
            stats(
                true,
                PartyMemberStatsInfo {
                    status: Some(member_status::ONLINE),
                    cur_hp: Some(100),
                    max_hp: Some(100),
                    ..Default::default()
                },
            ),
        ],
    );
    seat_party(&mut app, 0);
    frame(&mut app, vec![]);
    app
}

/// Two stats packets for an unheld member in one frame: each `UNIT_HEALTH` listener reads the
/// health its own packet wrote into the record (`0x5e54c4` before `0x5e54fa`), not the frame's
/// last.
#[test]
fn each_party_health_listener_reads_its_own_packets_record() {
    let mut app = party_client();
    assert_eq!(
        frame(&mut app, vec![hp(70), hp(40)]),
        ["UNIT_HEALTH:party1:70", "UNIT_HEALTH:party1:40"]
    );
}

/// A member's pet that one packet names and creates reads absent to a listener of an earlier
/// packet: `UnitExists("partypet1")` resolves the pet off the member's descriptor at the call
/// (`0x4e81d0`), which names no pet until that packet.
#[test]
fn a_party_pet_created_mid_frame_does_not_exist_before_its_create() {
    let mut app = party_client();
    frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![vec![created(
            MATE,
            EntityKind::Player,
            &[(benilla_protocol::field::FIELD_UNIT_HEALTH, 100)],
        )]]),
    );
    seat_party(&mut app, PET);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitExists("partypet1") end"#)
        .unwrap();
    frame(&mut app, vec![]);
    let seen = frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![
            vec![fall(1)],
            vec![
                values(MATE, &summons(PET)),
                created(PET, EntityKind::Unit, &[]),
            ],
            vec![fall(1)],
        ]),
    );
    assert_eq!(seen, ["UNIT_COMBAT:player:nil", "UNIT_COMBAT:player:1"]);
}

/// A member's pet replaced mid-frame reads as each packet left it: a listener before the swap
/// reads the old pet through `partypet1`, which the member's `UNIT_FIELD_SUMMON` named then
/// (`0x4e81d0`), though no view names it once the frame ends.
#[test]
fn a_party_pet_replaced_mid_frame_reads_the_old_pet_before_the_swap() {
    let mut app = party_client();
    let mut mate = vec![(benilla_protocol::field::FIELD_UNIT_HEALTH, 100)];
    mate.extend(summons(PET));
    frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![vec![
            created(MATE, EntityKind::Player, &mate),
            created(
                PET,
                EntityKind::Unit,
                &[(benilla_protocol::field::FIELD_UNIT_HEALTH, 10)],
            ),
        ]]),
    );
    seat_party(&mut app, PET);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitHealth("partypet1") end"#)
        .unwrap();
    frame(&mut app, vec![]);
    // The resolver's inputs as the feeds push them ahead of the drain: the frame's last pet.
    let seat_new_pet = |world: &mut World| {
        world
            .non_send_resource_mut::<UiScript>()
            .set_unit_guids(&UnitGuids {
                player: ME,
                target: WOLF,
                party: [MATE, 0, 0, 0],
                party_pets: [PET2, 0, 0, 0],
                held: [(ME, 0), (WOLF, 0), (BOAR, 0), (MATE, 0), (PET2, 0)]
                    .into_iter()
                    .collect(),
                ..Default::default()
            });
    };
    let seen = frame_after(
        &mut app,
        seat_new_pet,
        crate::lanes::unit_view_tests::wire_packets(vec![
            vec![fall(1)],
            vec![
                SessionEvent::ObjectsRemoved(vec![PET]),
                values(MATE, &summons(PET2)),
                created(
                    PET2,
                    EntityKind::Unit,
                    &[(benilla_protocol::field::FIELD_UNIT_HEALTH, 20)],
                ),
            ],
            vec![fall(1)],
        ]),
    );
    assert_eq!(seen, ["UNIT_COMBAT:player:10", "UNIT_COMBAT:player:20"]);
}

/// A record written past the log fails the rewind's check against the frame-start snapshot: here
/// the member's maximum health, which no entry of the frame names.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "a write the log missed")]
fn a_record_write_the_log_missed_fails_the_rewind() {
    let mut app = party_client();
    frame_after(
        &mut app,
        |world| {
            let mut group = world.resource_mut::<crate::ui_party::GroupState>();
            if let Some(record) = group.stats.get_mut(&MATE) {
                record.max_hp = Some(90);
            }
        },
        vec![hp(70), hp(40)],
    );
}

/// A record write after the lanes and before the next frame's snapshot needs no entry: the
/// sandbox's (`/partytest off`, a sandboxed party call) runs in the drains of what the Lua asked,
/// past the lanes, and the next frame's rewind starts from the roster it left.
#[test]
fn a_record_write_between_frames_needs_no_entry() {
    let mut app = party_client();
    let mut group = app
        .world_mut()
        .resource_mut::<crate::ui_party::GroupState>();
    if let Some(record) = group.stats.get_mut(&MATE) {
        record.max_hp = Some(90);
    }
    assert_eq!(
        frame(&mut app, vec![hp(70), hp(40)]),
        ["UNIT_HEALTH:party1:70", "UNIT_HEALTH:party1:40"]
    );
}

/// The unit a `target` chain ends on reads as each packet left it: `party1target` resolves to
/// the add (`0x515970`), whose health `UnitHealth` reads off its object at the call
/// (`0x5174d0`).
#[test]
fn a_chain_end_reads_its_own_packets_health() {
    let mut app = party_client();
    frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![
            // The mate targets the add (`UNIT_FIELD_TARGET`, fields 16 and 17).
            vec![created(
                MATE,
                EntityKind::Player,
                &[
                    (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                    (16, ADD as u32),
                    (17, (ADD >> 32) as u32),
                ],
            )],
            vec![created(
                ADD,
                EntityKind::Unit,
                &[
                    (OBJECT_TYPE, TYPE_UNIT),
                    (benilla_protocol::field::FIELD_UNIT_HEALTH, 50),
                ],
            )],
        ]),
    );
    seat_party_targeting(&mut app, 0, ADD);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitHealth("party1target") end"#)
        .unwrap();
    frame(&mut app, vec![]);
    let seen = frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![
            vec![fall(1)],
            vec![values(ADD, &super::unit_view_tests::health(40))],
            vec![fall(1)],
        ]),
    );
    assert_eq!(seen, ["UNIT_COMBAT:player:50", "UNIT_COMBAT:player:40"]);
}

/// `"pet"` reads our summon as each packet left it, and the pet's own health change signals
/// under `"pet"`: our live `UNIT_FIELD_SUMMON` gives it the token, whatever the seat holds
/// (`0x515cb9`).
#[test]
fn the_pet_reads_its_own_packets_health() {
    let mut app = party_client();
    frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![vec![
            created(PET, EntityKind::Unit, &super::unit_view_tests::health(60)),
            values(ME, &summons(PET)),
        ]]),
    );
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitHealth("pet") end"#)
        .unwrap();
    frame(&mut app, vec![]);
    let seen = frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![
            vec![fall(1)],
            vec![values(PET, &super::unit_view_tests::health(30))],
            vec![fall(1)],
        ]),
    );
    assert_eq!(
        seen,
        [
            "UNIT_COMBAT:player:60",
            "UNIT_HEALTH:pet:30",
            "UNIT_COMBAT:player:30"
        ]
    );
}

/// `"pet"` resolves off our own descriptor at the call (`0x515a5e`-`0x515a6e`): a listener ahead
/// of the packet that writes our `UNIT_FIELD_SUMMON` reads no pet, one after it the new pet.
#[test]
fn the_pet_is_the_summon_our_descriptor_names_as_each_packet_left_it() {
    let mut app = party_client();
    frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![vec![created(
            PET,
            EntityKind::Unit,
            &super::unit_view_tests::health(60),
        )]]),
    );
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitHealth("pet") end"#)
        .unwrap();
    frame(&mut app, vec![]);
    let seen = frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![
            vec![fall(1)],
            vec![values(ME, &summons(PET))],
            vec![fall(1)],
        ]),
    );
    assert_eq!(seen, ["UNIT_COMBAT:player:0", "UNIT_COMBAT:player:60"]);
}

/// The member a list names offline reads offline to that list's own `PARTY_MEMBERS_CHANGED`: the
/// list writes each member's record and its online bit (`AddPartyMember`, `0x5e6c4a`) before it
/// signals (`0x5e6c61`, `0x5e6c66`), and `UnitIsConnected` reads the record's bit for a member
/// with no object (`0x517dd3`).
#[test]
fn a_lists_own_signal_reads_the_records_it_wrote() {
    let mut app = party_client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            Roster = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PARTY_MEMBERS_CHANGED")
            f:SetScript("OnEvent", function()
                table.insert(Roster, tostring(UnitIsConnected("party1")))
            end)
            "#,
        )
        .unwrap();
    frame(
        &mut app,
        vec![SessionEvent::GroupList {
            group_type: 0,
            own_flags: 0,
            members: vec![GroupMemberEntry {
                name: "Brisca".into(),
                guid: MATE,
                status: member_status::OFFLINE,
                flags: 0,
            }],
            leader: ME,
            loot: None,
        }],
    );
    let heard = app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<Vec<String>>("return Roster")
        .unwrap();
    assert_eq!(heard, ["nil"]);
}

/// A member's object arriving and leaving signals at its packet's place: `PARTY_MEMBER_ENABLE`
/// from the player's post-init (`0x5dec35` → `0x4e85d0`, signalled at `0x4e86b6`) and
/// `PARTY_MEMBER_DISABLE` from its release (`0x5e9aa0`, at `0x5e9b78`), each with the member's
/// party slot, after the list ahead of it in the frame signalled (`0x5e6c66`). A worldport's purge
/// frees the object without its release (`0x467800` → its destructor `0x5dd600`), whose roster
/// search runs in mode 0 (`0x5dd6a0` → `0x4e85d0`) and signals nothing.
#[test]
fn a_members_object_signals_at_its_packet() {
    let mut app = party_client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            Roster = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PARTY_MEMBERS_CHANGED")
            f:RegisterEvent("PARTY_MEMBER_ENABLE")
            f:RegisterEvent("PARTY_MEMBER_DISABLE")
            f:SetScript("OnEvent", function()
                table.insert(Roster, event .. ":" .. tostring(arg1))
            end)
            "#,
        )
        .unwrap();
    let list = || SessionEvent::GroupList {
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
    };
    let heard = |app: &mut App| {
        let s = app.world_mut().non_send_resource_mut::<UiScript>();
        let out = s.eval::<Vec<String>>("return Roster").unwrap();
        s.run("Roster = {}").unwrap();
        out
    };
    let mut packets = vec![list()];
    packets.extend(crate::lanes::unit_view_tests::wire_packets(vec![vec![
        created(
            MATE,
            EntityKind::Player,
            &[(benilla_protocol::field::FIELD_UNIT_HEALTH, 100)],
        ),
    ]]));
    frame(&mut app, packets);
    assert_eq!(
        heard(&mut app),
        ["PARTY_MEMBERS_CHANGED:nil", "PARTY_MEMBER_ENABLE:1"]
    );
    let mut packets = vec![list()];
    packets.extend(crate::lanes::unit_view_tests::wire_packets(vec![vec![
        SessionEvent::ObjectsRemoved(vec![MATE]),
    ]]));
    frame(&mut app, packets);
    assert_eq!(
        heard(&mut app),
        ["PARTY_MEMBERS_CHANGED:nil", "PARTY_MEMBER_DISABLE:1"]
    );
    frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![vec![created(
            MATE,
            EntityKind::Player,
            &[(benilla_protocol::field::FIELD_UNIT_HEALTH, 100)],
        )]]),
    );
    assert_eq!(heard(&mut app), ["PARTY_MEMBER_ENABLE:1"]);
    frame(
        &mut app,
        vec![SessionEvent::Worldport {
            map_id: 1,
            position: [0.0; 3],
            orientation: 0.0,
            needs_ack: false,
        }],
    );
    assert_eq!(heard(&mut app), Vec::<String>::new());
}

/// The summon in two packets of one frame, as vmangos sends it: `SMSG_PET_SPELLS` first, whose
/// bar write signals `UNIT_PET("player")` (`0x4bc84f`) while our `UNIT_FIELD_SUMMON` is still 0,
/// then the update that writes it, whose watch signals `UNIT_PET` again (`0x51bbb0`, event table
/// slot 2). The stock pet frame's `UnitExists("pet")` (`PetFrame.lua:21`) reads no pet at the
/// first and the pet at the second.
#[test]
fn a_summon_signals_unit_pet_at_the_bar_and_again_at_our_summon_field() {
    let mut app = party_client();
    frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![vec![created(
            PET,
            EntityKind::Unit,
            &super::unit_view_tests::health(60),
        )]]),
    );
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_PET")
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. arg1 .. ":" .. tostring(UnitExists("pet")))
            end)
            "#,
        )
        .unwrap();
    let bar = SessionEvent::PetSpells(Box::new(benilla_protocol::messages::PetSpells {
        pet_guid: PET,
        ..Default::default()
    }));
    let seen = frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![
            vec![bar],
            vec![values(ME, &summons(PET))],
        ]),
    );
    assert_eq!(seen, ["UNIT_PET:player:nil", "UNIT_PET:player:1"]);
}

/// A spy on `UNIT_PET`, `PET_BAR_UPDATE`, `TRAINER_CLOSED` and `PET_UI_CLOSE`, each with what `UnitExists("pet")`
/// answers as it fires.
fn spy_pet_signals(app: &mut App) {
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_PET")
            f:RegisterEvent("PET_BAR_UPDATE")
            f:RegisterEvent("TRAINER_CLOSED")
            f:RegisterEvent("PET_UI_CLOSE")
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(UnitExists("pet")))
            end)
            "#,
        )
        .unwrap();
}

/// `SMSG_PET_SPELLS` naming `guid`.
fn bar_for(guid: u64) -> SessionEvent {
    SessionEvent::PetSpells(Box::new(benilla_protocol::messages::PetSpells {
        pet_guid: guid,
        ..Default::default()
    }))
}

/// `SetPetGuid` signals in its order at its packet's place (`0x4bdaf0` → `0x4bc7e0`):
/// `UNIT_PET` on the change (`0x4bc84f`), then `PET_BAR_UPDATE` (`0x4bc90c`), both ahead of the
/// next packet's summon field and its watch's `UNIT_PET`; a re-send of the same bar is
/// `PET_BAR_UPDATE` alone.
#[test]
fn the_bar_signals_unit_pet_then_its_update_at_the_packet() {
    let mut app = party_client();
    frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![vec![created(
            PET,
            EntityKind::Unit,
            &super::unit_view_tests::health(60),
        )]]),
    );
    spy_pet_signals(&mut app);
    let seen = frame(
        &mut app,
        crate::lanes::unit_view_tests::wire_packets(vec![
            vec![bar_for(PET)],
            vec![values(ME, &summons(PET))],
        ]),
    );
    assert_eq!(seen, ["UNIT_PET:nil", "PET_BAR_UPDATE:nil", "UNIT_PET:1"]);
    let seen = frame(&mut app, vec![bar_for(PET)]);
    assert_eq!(seen, ["PET_BAR_UPDATE:1"], "the same pet: no change");
}

/// A new pet with a pet trainer's list open asks for it again (`0x4bc872`-`0x4bc8b2`):
/// `TalkToTrainer` closes the window (`0x5df6bb` → `0x4d7440`) and sends `CMSG_TRAINER_LIST` for
/// it (`0x5df6fc`), between `UNIT_PET` and `PET_BAR_UPDATE`. The close signals `TRAINER_CLOSED`
/// twice, as the trainer is still the interaction target (`0x4d7474` → `0x493310`, which closes
/// again with the cell still set, `0x493434`). Another trainer type, or the same pet again,
/// leaves the window be.
#[test]
fn a_new_pet_asks_an_open_pet_trainer_for_its_list_again() {
    const TRAINER: u64 = 0xF130_0000_0055_0009;
    for (trainer_type, refreshed) in [(3, true), (0, false)] {
        let mut app = party_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        crate::ui_trainer::open_trainer(
            app.world_mut(),
            TRAINER,
            trainer_type,
            Vec::new(),
            String::new(),
        );
        spy_pet_signals(&mut app);
        let seen = frame(&mut app, vec![bar_for(PET)]);
        let asked: Vec<u64> = rx
            .try_iter()
            .filter_map(|c| match c {
                crate::net::ClientCommand::TrainerList { trainer } => Some(trainer),
                _ => None,
            })
            .collect();
        let open = app
            .world()
            .resource::<crate::ui_trainer::TrainerOpen>()
            .trainer;
        if refreshed {
            assert_eq!(
                seen,
                [
                    "UNIT_PET:nil",
                    "TRAINER_CLOSED:nil",
                    "TRAINER_CLOSED:nil",
                    "PET_BAR_UPDATE:nil"
                ]
            );
            assert_eq!(asked, [TRAINER]);
            assert_eq!(open, None, "the window closed for the new list");
            // The same pet again: no change, no refresh.
            crate::ui_trainer::open_trainer(
                app.world_mut(),
                TRAINER,
                trainer_type,
                Vec::new(),
                String::new(),
            );
            let seen = frame(&mut app, vec![bar_for(PET)]);
            assert_eq!(seen, ["PET_BAR_UPDATE:nil"]);
            assert_eq!(rx.try_iter().count(), 0);
        } else {
            assert_eq!(seen, ["UNIT_PET:nil", "PET_BAR_UPDATE:nil"]);
            assert!(asked.is_empty(), "type {trainer_type}: {asked:?}");
            assert_eq!(open, Some(TRAINER));
        }
    }
}

/// `SetPetGuid`'s `PET_UI_CLOSE` (`0x4bc8f8`-`0x4bc907`, event 428): on a change to a guid that
/// resolves as a unit (`0x4bc8e4`) and passes `IsPlayerOrHunterPet` (`0x6116e0`), between
/// `UNIT_PET` and `PET_BAR_UPDATE`. A creature passes as a pet number's holder that our charm,
/// else our summon, names while our class is Hunter (`0x611704`-`0x61175a`); a warrior's, or a
/// unit with no pet number, does not.
#[test]
fn a_hunters_new_pet_signals_pet_ui_close_before_the_bar_update() {
    /// `UNIT_FIELD_BYTES_0`, the class in byte 1, and `UNIT_FIELD_PETNUMBER`.
    const BYTES_0: u16 = 36;
    const PETNUMBER: u16 = 139;
    for (class, pet_number, closes) in [(3, 7, true), (1, 7, false), (3, 0, false)] {
        let mut app = party_client();
        frame(
            &mut app,
            crate::lanes::unit_view_tests::wire_packets(vec![vec![
                created(
                    PET,
                    EntityKind::Unit,
                    &[
                        (OBJECT_TYPE, TYPE_UNIT),
                        (22, 60),
                        (28, 60),
                        (PETNUMBER, pet_number),
                    ],
                ),
                values(ME, &[(BYTES_0, class << 8)]),
                values(ME, &summons(PET)),
            ]]),
        );
        spy_pet_signals(&mut app);
        let seen = frame(&mut app, vec![bar_for(PET)]);
        let mut heard = vec!["UNIT_PET:1"];
        if closes {
            heard.push("PET_UI_CLOSE:1");
        }
        heard.push("PET_BAR_UPDATE:1");
        assert_eq!(seen, heard, "class {class}, pet number {pet_number}");
        let seen = frame(&mut app, vec![bar_for(PET)]);
        assert_eq!(seen, ["PET_BAR_UPDATE:1"], "the same pet: no change");
    }
}

/// Another party member, whom the second list adds.
const MATE2: u64 = 0x23;
/// A third and a fourth, for the raid.
const MATE3: u64 = 0x24;
const MATE4: u64 = 0x25;

/// `SMSG_GROUP_LIST` of `members` (guid, name), led by `leader`, with the loot tail `loot`.
fn list(group_type: u8, members: &[(u64, &str)], leader: u64, loot: GroupLootInfo) -> SessionEvent {
    SessionEvent::GroupList {
        group_type,
        own_flags: 0,
        members: members
            .iter()
            .map(|&(guid, name)| GroupMemberEntry {
                name: name.into(),
                guid,
                status: member_status::ONLINE,
                flags: 0,
            })
            .collect(),
        leader,
        loot: Some(loot),
    }
}

/// The names of everyone a list may name, cached so no raid row waits on one, and the world
/// the party feed reads the area under us from.
fn know_the_names(app: &mut App) {
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    let mut names = app.world_mut().resource_mut::<crate::names::NameCache>();
    for (guid, name) in [
        (ME, "Me"),
        (MATE, "Brisca"),
        (MATE2, "Dunk"),
        (MATE3, "Egg"),
        (MATE4, "Quill"),
    ] {
        names.insert_player(guid, name.into(), None);
    }
}

/// `body` registered on `events`, appending its returned string to `Roster`.
fn listen(app: &mut App, events: &[&str], body: &str) {
    let registers: String = events
        .iter()
        .map(|e| format!("f:RegisterEvent(\"{e}\")\n"))
        .collect();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(&format!(
            r#"
            Roster = {{}}
            local f = CreateFrame("Frame")
            {registers}
            f:SetScript("OnEvent", function()
                table.insert(Roster, (function() {body} end)())
            end)
            "#
        ))
        .unwrap();
}

/// A frame with nothing in it, which the party feed's first run takes: it tells a fresh VM the
/// events it never heard, and passes over the lane's.
fn prime(app: &mut App) {
    frame(app, vec![]);
}

/// What `Roster` heard this frame.
fn heard_roster(app: &mut App) -> Vec<String> {
    let s = app.world_mut().non_send_resource_mut::<UiScript>();
    let out = s.eval::<Vec<String>>("return Roster").unwrap();
    s.run("Roster = {}").unwrap();
    out
}

/// Two group lists in one frame: each event's listener reads what its own list left of the cells
/// the event follows. The slots at `PARTY_MEMBERS_CHANGED` (`GetNumPartyMembers` `0x4e86d0`,
/// `GetPartyMember` `0x4e81a0`, `UnitInParty` `0x516290`), the leader cells at
/// `PARTY_LEADER_CHANGED` (`GetPartyLeaderIndex` `0x4e9104`, `IsPartyLeader` `0x4e9130`) and the
/// loot cells at `PARTY_LOOT_METHOD_CHANGED` (`GetLootMethod` `0x4e91b6`), not the frame's last.
/// (The reference stores the leader and the loot after the members event, `0x5e6c7e`, `0x5e6e52`;
/// a packet's cells are read whole here, so the members event is asserted on its slots alone.)
#[test]
fn each_party_listener_reads_its_own_lists_roster() {
    let mut app = client();
    know_the_names(&mut app);
    prime(&mut app);
    listen(
        &mut app,
        &[
            "PARTY_MEMBERS_CHANGED",
            "PARTY_LEADER_CHANGED",
            "PARTY_LOOT_METHOD_CHANGED",
        ],
        r#"
        if event == "PARTY_MEMBERS_CHANGED" then
            return table.concat({
                event, GetNumPartyMembers(), tostring(GetPartyMember(2)),
                tostring(UnitInParty("party2")),
            }, ":")
        elseif event == "PARTY_LEADER_CHANGED" then
            return table.concat({
                event, GetPartyLeaderIndex(), tostring(IsPartyLeader()),
            }, ":")
        end
        local method, master = GetLootMethod()
        return table.concat({event, method, tostring(master), GetLootThreshold()}, ":")
        "#,
    );
    let seen = frame(
        &mut app,
        vec![
            list(
                0,
                &[(MATE, "Brisca")],
                ME,
                GroupLootInfo {
                    method: 1,
                    master: 0,
                    threshold: 2,
                },
            ),
            list(
                0,
                &[(MATE, "Brisca"), (MATE2, "Dunk")],
                MATE,
                GroupLootInfo {
                    method: 2,
                    master: MATE2,
                    threshold: 3,
                },
            ),
        ],
    );
    assert!(seen.is_empty(), "{seen:?}");
    assert_eq!(
        heard_roster(&mut app),
        [
            "PARTY_MEMBERS_CHANGED:1:nil:nil",
            "PARTY_LEADER_CHANGED:0:1",
            "PARTY_LOOT_METHOD_CHANGED:roundrobin:nil:2",
            "PARTY_MEMBERS_CHANGED:2:1:1",
            "PARTY_LEADER_CHANGED:1:nil",
            "PARTY_LOOT_METHOD_CHANGED:master:2:3",
        ]
    );
}

/// The slots are compact: a list entry with a zero guid counts against the four (`0x5e6c4f`) but
/// `0x4e82d0` skips it (`0x4e82eb`) and fills the first empty slot, so `[A, 0, B]` puts B in
/// `party2` and the count is the nonzero slots (`0x4e86d0`).
#[test]
fn a_zero_guid_entry_leaves_no_gap_in_the_party_slots() {
    let mut app = client();
    know_the_names(&mut app);
    prime(&mut app);
    listen(
        &mut app,
        &["PARTY_MEMBERS_CHANGED"],
        r#"return table.concat({
            GetNumPartyMembers(), tostring(GetPartyMember(2)), tostring(GetPartyMember(3)),
            tostring(UnitInParty("party2")), tostring(UnitInParty("party3")),
        }, ":")"#,
    );
    let loot = GroupLootInfo {
        method: 0,
        master: 0,
        threshold: 2,
    };
    frame(
        &mut app,
        vec![list(
            0,
            &[(MATE, "Brisca"), (0, "Ghost"), (MATE2, "Dunk")],
            ME,
            loot,
        )],
    );
    assert_eq!(heard_roster(&mut app), ["2:1:nil:1:nil"]);
}

/// `UnitPlayerOrPetInParty` asks `0x4e8fa0` of the token's guid: a party slot's member or the pet
/// its record names, never the player, who is no slot; `UnitPlayerOrPetInRaid` asks `0x4bafd0`:
/// a roster row, ours included, or a row's pet. Neither needs the unit to exist.
#[test]
fn the_member_or_pet_tests_match_slots_and_their_pets_not_the_player() {
    let mut app = party_client();
    frame(
        &mut app,
        vec![stats(
            false,
            PartyMemberStatsInfo {
                pet_guid: Some(PET),
                ..Default::default()
            },
        )],
    );
    let ask = |app: &mut App| -> String {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM")
                .eval::<String>(
                    r#"return table.concat({
                        tostring(UnitPlayerOrPetInParty("party1")),
                        tostring(UnitPlayerOrPetInParty("partypet1")),
                        tostring(UnitPlayerOrPetInParty("player")),
                        tostring(UnitPlayerOrPetInParty("target")),
                        tostring(UnitPlayerOrPetInRaid("party1")),
                        tostring(UnitPlayerOrPetInRaid("player")),
                        tostring(UnitPlayerOrPetInRaid("raid1")),
                        tostring(UnitPlayerOrPetInRaid("raidpet1")),
                    }, ":")"#,
                )
                .unwrap()
        })
    };
    assert_eq!(ask(&mut app), "1:1:nil:nil:nil:nil:nil:nil", "a party");
    frame(
        &mut app,
        vec![list(
            1,
            &[(MATE, "Brisca")],
            ME,
            GroupLootInfo {
                method: 0,
                master: 0,
                threshold: 2,
            },
        )],
    );
    assert_eq!(ask(&mut app), "1:1:nil:nil:1:1:1:1", "a raid");
}

/// Two raid lists in one frame: each `RAID_ROSTER_UPDATE` listener reads the count and the rows
/// its own list rebuilt (`GetNumRaidMembers` `0xb713e0`, `GetRaidRosterInfo` `0x4bb560`,
/// `UnitInRaid` `0x516350`, `IsRaidOfficer` `0x4bb910`), and `raidN` names the row it held then.
#[test]
fn each_raid_listener_reads_its_own_lists_rows() {
    let mut app = client();
    know_the_names(&mut app);
    prime(&mut app);
    listen(
        &mut app,
        &["RAID_ROSTER_UPDATE"],
        r#"
        local name, rank, group = GetRaidRosterInfo(2)
        local last = GetRaidRosterInfo(GetNumRaidMembers())
        return table.concat({
            GetNumRaidMembers(), tostring(name), rank, group, tostring(last),
            tostring(UnitInRaid("raid3")), tostring(UnitInRaid("raid4")),
            tostring(IsRaidOfficer()), tostring(IsRaidLeader()),
        }, ":")
        "#,
    );
    let loot = GroupLootInfo {
        method: 3,
        master: 0,
        threshold: 2,
    };
    frame(
        &mut app,
        vec![
            list(1, &[(MATE, "Brisca"), (MATE2, "Dunk")], ME, loot),
            list(
                1,
                &[
                    (MATE4, "Quill"),
                    (MATE, "Brisca"),
                    (MATE2, "Dunk"),
                    (MATE3, "Egg"),
                ],
                MATE,
                loot,
            ),
        ],
    );
    // Our own row is last, after the wire's. The first list: three rows, we lead; the second:
    // five rows, row 2 is Brisca, who leads, and we are no officer.
    assert_eq!(
        heard_roster(&mut app),
        ["3:Dunk:0:1:Me:1:nil:1:1", "5:Brisca:2:1:Me:1:1:nil:nil"]
    );
}

/// `party1` names the member its own list seated, and nobody once a later list empties the slot:
/// the group tokens are built over the roster as each packet left it (`0x4e81a0`), the slot a
/// later packet vacates included.
#[test]
fn each_listener_finds_the_member_its_own_list_seated_in_party1() {
    let mut app = client();
    know_the_names(&mut app);
    prime(&mut app);
    listen(
        &mut app,
        &["PARTY_MEMBERS_CHANGED"],
        r#"return tostring(UnitName("party1")) .. ":" .. tostring(UnitExists("party1"))"#,
    );
    let loot = GroupLootInfo {
        method: 0,
        master: 0,
        threshold: 2,
    };
    frame(
        &mut app,
        vec![
            list(0, &[(MATE, "Brisca")], ME, loot),
            list(0, &[(MATE2, "Dunk")], ME, loot),
            list(0, &[], ME, loot),
        ],
    );
    assert_eq!(
        heard_roster(&mut app),
        ["Brisca:1", "Dunk:1", "nil:nil"],
        "the frame's last roster names nobody in party1"
    );
}

/// A roster cell written past the log fails the rewind's check against the frame-start snapshot:
/// here the leader cell, which no entry of the frame names.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "a write the log missed")]
fn a_roster_write_the_log_missed_fails_the_rewind() {
    let mut app = party_client();
    frame_after(
        &mut app,
        |world| {
            world.resource_mut::<crate::ui_party::GroupState>().leader = MATE;
        },
        vec![hp(70), hp(40)],
    );
}
