//! What a listener reads of a group unit, as the packets before it left it. A stats packet's
//! handler writes the member's record, then signals what changed (`0x5e5110`: the record copies at
//! `0x5e54c4` and `0x5e54d8`, then `0x5e54fa` → `0x5e5720`), and `UnitHealth` reads the record for a
//! member with no object (`0x5174d0`). An update packet writes the object layer before its notify
//! pass (`0x4651a0`), and `UnitExists` reads the token's object or roster leg at the call
//! (`0x515ff8` → `0x491900`). So each listener reads its own packet's state.

use benilla_protocol::messages::{member_status, GroupMemberEntry, PartyMemberStatsInfo};
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
            vec![created(
                MATE,
                EntityKind::Player,
                &[(benilla_protocol::field::FIELD_UNIT_HEALTH, 100)],
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

/// `"pet"` reads our summon as each packet left it.
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
    assert_eq!(seen, ["UNIT_COMBAT:player:60", "UNIT_COMBAT:player:30"]);
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
