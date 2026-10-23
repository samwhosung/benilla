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
    let mut held: std::collections::HashMap<u64, u64> =
        [(ME, 0), (WOLF, 0), (BOAR, 0)].into_iter().collect();
    if app
        .world()
        .resource::<crate::net::GuidIndex>()
        .0
        .contains_key(&MATE)
    {
        held.insert(MATE, 0);
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

/// One frame: the packets, the unit and party feeds' pushes of the frame's state, then the lanes'
/// drain; what the spy saw.
fn frame(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    let world = app.world_mut();
    #[cfg(debug_assertions)]
    {
        use bevy::ecs::system::RunSystemOnce;
        world
            .run_system_once(crate::ui_unit::views::mark_frame_start)
            .unwrap();
    }
    crate::net::handlers::dispatch(world, packets);
    crate::ui_unit::run_feed_units_once(world);
    world
        .run_system_cached(crate::ui_party::feed_party)
        .expect("the party feed runs");
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
