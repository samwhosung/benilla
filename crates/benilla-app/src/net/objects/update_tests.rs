//! How `SMSG_UPDATE_OBJECT` blocks apply to the streamed world, through the handler table.
use super::*;
use benilla_assets::coords::wow_to_bevy;

const ME: u64 = 0x0000_0000_0000_0007;
const MOB: u64 = 0xF130_0000_1234_0001;

fn client() -> App {
    crate::game_plugins::schedule_tests::headless_client()
}

/// An indexed entity for `guid`, as a create leaves it.
fn indexed(world: &mut World, guid: u64) -> Entity {
    let e = world.spawn((Guid(guid), Transform::default())).id();
    world.resource_mut::<GuidIndex>().0.insert(guid, e);
    e
}

/// An out-of-range block lists the active player among others: the others are released, the
/// player is not (`0x465f39`..`0x465f45` skip `0x464920`).
#[test]
fn an_out_of_range_block_never_releases_the_active_player() {
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let me = indexed(world, ME);
    let mob = indexed(world, MOB);

    super::super::handlers::dispatch(
        world,
        super::wire(vec![SessionEvent::ObjectsRemoved(vec![MOB, ME])]),
    );

    let index = &world.resource::<GuidIndex>().0;
    assert_eq!(index.get(&ME), Some(&me), "the player stays indexed");
    assert!(world.get::<Guid>(me).is_some(), "and is not torn down");
    assert!(world.get::<DespawnFade>(me).is_none(), "nor faded out");
    assert!(
        !index.contains_key(&MOB),
        "control: another guid is released"
    );
    assert!(world.get::<DespawnFade>(mob).is_some());
}

fn moved(guid: u64) -> SessionEvent {
    SessionEvent::ObjectMove {
        guid,
        position: [10.0, 20.0, 30.0],
        orientation: 0.0,
    }
}

/// A movement block names the unit this client moves, our player or a possessed unit, and is
/// dropped; any other indexed unit takes the pose (`0x4670a5`..`0x4670bb`). With control lost
/// the mover global is zero (`0x5fa600`), so the block applies.
#[test]
fn a_movement_block_for_the_unit_we_move_is_dropped() {
    let mut app = client();
    let world = app.world_mut();
    let driven = indexed(world, ME);
    world.entity_mut(driven).insert(ActiveMover);
    let possessed = indexed(world, 0xF130_0000_0000_0002);
    world.entity_mut(possessed).insert(ActiveMover);
    let other = indexed(world, MOB);
    let lost = indexed(world, 0xF130_0000_0000_0003);

    super::super::handlers::dispatch(
        world,
        super::wire(vec![
            moved(ME),
            moved(0xF130_0000_0000_0002),
            moved(MOB),
            moved(0xF130_0000_0000_0003),
        ]),
    );

    let at = |world: &World, e: Entity| world.get::<Transform>(e).unwrap().translation;
    assert_eq!(at(world, driven), Vec3::ZERO, "the player is not moved");
    assert_eq!(at(world, possessed), Vec3::ZERO, "nor the possessed unit");
    let pose = wow_to_bevy([10.0, 20.0, 30.0]);
    assert_eq!(
        at(world, other),
        pose,
        "control: another unit takes the pose"
    );
    assert_eq!(at(world, lost), pose, "control: an unsteered unit takes it");
}

fn create(guid: u64, kind: EntityKind, position: [f32; 3]) -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id: None,
        position,
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        fields: ObjectFields::default(),
    }
}

/// A GameObject create asks its template by the descriptor's `OBJECT_FIELD_ENTRY` (`0x5f7d77`,
/// `[descriptor+0xc]`), never by the guid's bits: a boat whose guid counter is not its entry.
#[test]
fn a_gameobject_create_asks_its_template_by_the_entry_field() {
    const BOAT_ENTRY: u32 = 176231;
    // `HIGHGUID_MO_TRANSPORT` with a plain counter in the low 32 bits.
    const BOAT: u64 = 0x1FC0_0000_0000_0007;
    let mut app = client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let mut boat = create(BOAT, EntityKind::GameObject, [1.0, 2.0, 3.0]);
    if let SessionEvent::ObjectCreate { fields, .. } = &mut boat {
        *fields =
            ObjectFields::from_pairs(&[(benilla_protocol::field::FIELD_OBJECT_ENTRY, BOAT_ENTRY)]);
    }
    super::super::handlers::dispatch(world, super::wire(vec![boat]));
    let asked: Vec<_> = rx
        .try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::GameObjectQuery { entry, guid } => Some((entry, guid)),
            _ => None,
        })
        .collect();
    assert_eq!(asked, vec![(BOAT_ENTRY, BOAT)]);
}

/// A create of a known guid moves only a unit (type-mask bit 3, `0x46638c`); a GameObject's
/// block position is read and dropped (`0x4664c3`), so it keeps its place.
#[test]
fn a_recreate_re_poses_units_only() {
    const GO: u64 = 0xF110_0000_0000_0009;
    let mut app = client();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let at = |world: &World, guid: u64| {
        let e = world.resource::<GuidIndex>().0[&guid];
        world.get::<Transform>(e).unwrap().translation
    };
    super::super::handlers::dispatch(
        world,
        super::wire(vec![
            create(MOB, EntityKind::Unit, [1.0, 2.0, 3.0]),
            create(GO, EntityKind::GameObject, [1.0, 2.0, 3.0]),
        ]),
    );
    let first = wow_to_bevy([1.0, 2.0, 3.0]);
    assert_eq!(
        at(world, MOB),
        first,
        "control: the first create places both"
    );
    assert_eq!(at(world, GO), first);

    super::super::handlers::dispatch(
        world,
        super::wire(vec![
            create(MOB, EntityKind::Unit, [10.0, 20.0, 30.0]),
            create(GO, EntityKind::GameObject, [10.0, 20.0, 30.0]),
        ]),
    );
    assert_eq!(
        at(world, MOB),
        wow_to_bevy([10.0, 20.0, 30.0]),
        "a unit takes the new pose"
    );
    assert_eq!(at(world, GO), first, "a GameObject stays where it stands");
}

// ── One packet's passes and the released table ──────────────────────────────────────────────────

const MATE: u64 = 0x0000_0000_0000_2001;
const XP: u16 = 716;
const STAT0: u16 = 150;
const ENTRY: u16 = 3;

/// A create block whose fields are the type's whole descriptor, as the wire's are.
fn created(guid: u64, kind: EntityKind, position: [f32; 3], pairs: &[(u16, u32)]) -> SessionEvent {
    use benilla_protocol::messages::ObjectType;
    let as_type = match kind {
        EntityKind::Player => ObjectType::Player,
        EntityKind::GameObject => ObjectType::GameObject,
        _ => ObjectType::Unit,
    };
    let SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id,
        position,
        orientation,
        scale,
        speeds,
        mover,
        transport_progress,
        transport,
        spline,
        ..
    } = create(guid, kind, position)
    else {
        unreachable!()
    };
    SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id,
        position,
        orientation,
        scale,
        speeds,
        mover,
        transport_progress,
        transport,
        spline,
        fields: ObjectFields::from_pairs(pairs).into_created(as_type),
    }
}

fn player(guid: u64, pairs: &[(u16, u32)]) -> SessionEvent {
    created(guid, EntityKind::Player, [0.0; 3], pairs)
}

fn unit(guid: u64, pairs: &[(u16, u32)]) -> SessionEvent {
    created(guid, EntityKind::Unit, [1.0, 2.0, 3.0], pairs)
}

fn values(guid: u64, pairs: &[(u16, u32)]) -> SessionEvent {
    SessionEvent::ObjectValues {
        guid,
        fields: ObjectFields::from_pairs(pairs),
    }
}

fn out_of_range(guids: &[u64]) -> SessionEvent {
    SessionEvent::ObjectsRemoved(guids.to_vec())
}

fn send(world: &mut World, blocks: impl IntoIterator<Item = SessionEvent>) {
    super::super::handlers::dispatch(world, super::packet(blocks));
}

fn notices(world: &mut World) -> Vec<crate::field_watch::FieldNotice> {
    world
        .resource_mut::<Messages<crate::field_watch::FieldNotice>>()
        .drain()
        .collect()
}

/// The field edges written, as `(guid, index, old, new)`.
fn edges(world: &mut World) -> Vec<(u64, u16, u32, u32)> {
    world
        .resource_mut::<Messages<FieldChanged>>()
        .drain()
        .map(|e| (e.guid, e.index, e.old, e.new))
        .collect()
}

fn field(world: &World, guid: u64, index: u16) -> Option<u32> {
    let e = *world.resource::<GuidIndex>().0.get(&guid)?;
    world
        .get::<ObjectStore>(e)?
        .0
        .raw_fields()
        .find(|&(i, _)| i == index)
        .map(|(_, v)| v)
}

fn held(world: &World, guid: u64) -> bool {
    world.resource::<GuidIndex>().0.contains_key(&guid)
}

/// A packet's write pass runs over all its blocks before its notify pass (`0x4651a0`), and each
/// values write re-copies every watched range of its object first (`0x46664b`-`0x4666b6`): an
/// earlier block for the same object compares against the copy the last write left. A watch
/// only the earlier block moved does not run; one both moved runs twice, its old value the last
/// write's. Another object keeps its own copy.
#[test]
fn one_packets_blocks_for_one_object_notify_against_its_last_copy() {
    use crate::field_watch::FieldNotice;
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    send(
        world,
        [
            player(ME, &[(STAT0, 20), (XP, 10)]),
            unit(MOB, &[(STAT0, 5)]),
        ],
    );
    notices(world);
    edges(world);

    send(
        world,
        [
            values(ME, &[(XP, 30)]),
            values(MOB, &[(STAT0, 6)]),
            values(ME, &[(STAT0, 21)]),
        ],
    );
    assert_eq!(
        notices(world),
        [
            FieldNotice::Unit { guid: MOB, id: 144 },
            FieldNotice::Unit { guid: ME, id: 144 },
        ],
        "the XP block compared against the copy the stat block's write took"
    );
    assert_eq!(edges(world), [(MOB, STAT0, 5, 6), (ME, STAT0, 20, 21)]);
    assert_eq!(field(world, ME, XP), Some(30), "both blocks were written");

    send(world, [values(ME, &[(XP, 40)]), values(ME, &[(XP, 50)])]);
    assert_eq!(notices(world), [FieldNotice::Player { id: 189 }; 2]);
    assert_eq!(edges(world), [(ME, XP, 40, 50); 2]);
}

/// A create for a held object writes every field, clearing those it leaves out (`0x466582`
/// passes the clearing flag to `0x466590`), and notifies every field (`0x465d8a`, `dl = 1`): an
/// omitted field reads 0 and its watch runs.
#[test]
fn a_re_create_clears_the_fields_it_omits_and_runs_their_watches() {
    use crate::field_watch::FieldNotice;
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    send(world, [unit(MOB, &[(ENTRY, 299), (STAT0, 5)])]);
    notices(world);
    edges(world);
    send(world, [unit(MOB, &[(ENTRY, 299)])]);
    assert_eq!(field(world, MOB, STAT0), None, "cleared");
    assert_eq!(notices(world), [FieldNotice::Unit { guid: MOB, id: 144 }]);
    assert_eq!(edges(world), [(MOB, STAT0, 5, 0)]);
}

/// An out-of-range object is released (`0x464920`), not destroyed: it leaves the live table,
/// fields and all. Its next create finds it (`0x46612d` → `0x464530`) and writes over the kept
/// fields, and since it was shown before (`+0xe4` bit 4, set only by `0x613af0`) its notify runs
/// every field's watch (`0x465d8e`) in place of the create hook (`0x5debe0`): no guild tag. The
/// packet's end runs the reactivated player's `PostUpdate` (`0x5dec90`), which compares its
/// party record (`0x5decb7`).
#[test]
fn a_released_player_comes_back_with_its_fields_and_no_create_hook() {
    use crate::field_watch::FieldNotice;
    use benilla_protocol::messages::PartyMemberStatsInfo;
    const HEALTH: u16 = 22;
    const MAXHEALTH: u16 = 28;
    const LEVEL: u16 = 34;
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    world
        .resource_mut::<crate::ui_party::GroupState>()
        .stats
        .insert(
            MATE,
            PartyMemberStatsInfo {
                status: Some(1),
                cur_hp: Some(100),
                max_hp: Some(100),
                power_type: Some(0),
                cur_power: Some(0),
                max_power: Some(0),
                level: Some(60),
                auras: Some(Vec::new()),
                auras_negative: Some(Vec::new()),
                ..Default::default()
            },
        );
    let mate = |stat: u32| {
        player(
            MATE,
            &[(HEALTH, 100), (MAXHEALTH, 200), (LEVEL, 60), (STAT0, stat)],
        )
    };
    let signal = |id| FieldNotice::Signal { guid: MATE, id };
    send(world, [mate(5)]);
    assert_eq!(
        notices(world),
        [FieldNotice::Global { id: 376 }, signal(345), signal(22)],
        "control: a first create runs the hook"
    );
    edges(world);

    send(world, [out_of_range(&[MATE])]);
    assert!(!held(world, MATE), "released from the live table");

    send(world, [mate(7)]);
    assert!(held(world, MATE));
    assert_eq!(
        notices(world),
        [signal(22)],
        "no bridge until its slot 3 (`0x5fbc44`, `0x5fbe36`), then the packet end's party \
         record; no guild tag"
    );
    assert_eq!(edges(world), [(MATE, STAT0, 5, 7)]);
}

/// A values block for a released guid brings the object back (`0x467160` → `0x464530`) with the
/// fields it kept; its field bridge, dropped by the release (`0x5fbc44`), runs again from the next
/// packet, once slot 3 has added it back (`0x5fbe36`). A movement block (`0x4670bf`) and a
/// near-objects block (`0x4672c8`) bring one back the same way.
#[test]
fn a_values_movement_or_near_block_brings_a_released_object_back() {
    use crate::field_watch::FieldNotice;
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    send(world, [unit(MOB, &[(ENTRY, 299), (STAT0, 5)])]);
    notices(world);

    send(world, [out_of_range(&[MOB])]);
    send(world, [values(MOB, &[(STAT0, 6)])]);
    assert!(held(world, MOB), "a values block");
    assert_eq!(field(world, MOB, ENTRY), Some(299), "the kept fields");
    assert_eq!(field(world, MOB, STAT0), Some(6));
    assert!(
        notices(world).is_empty(),
        "no bridge in the packet that brings it back"
    );
    send(world, [values(MOB, &[(STAT0, 7)])]);
    assert_eq!(
        notices(world),
        [FieldNotice::Unit { guid: MOB, id: 144 }],
        "the next packet's block runs it"
    );

    send(world, [out_of_range(&[MOB])]);
    send(world, [moved(MOB)]);
    assert!(held(world, MOB), "a movement block");
    assert_eq!(field(world, MOB, STAT0), Some(7));
    let e = world.resource::<GuidIndex>().0[&MOB];
    assert_eq!(
        world.get::<Transform>(e).unwrap().translation,
        wow_to_bevy([10.0, 20.0, 30.0])
    );

    send(world, [out_of_range(&[MOB])]);
    send(world, [SessionEvent::ObjectsNear(vec![MOB])]);
    assert!(held(world, MOB), "a near-objects block");
    assert_eq!(field(world, MOB, ENTRY), Some(299));
}

/// The movement block compares its guid with the active mover before the lookup that would bring
/// a released object back (`0x4670a5`-`0x4670bb`, ahead of `0x4670c2`): a released unit we drive
/// stays released, and a later values block still finds it in the table.
#[test]
fn a_movement_block_for_the_mover_brings_nothing_back() {
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    send(world, [unit(MOB, &[(ENTRY, 299)])]);
    send(world, [out_of_range(&[MOB])]);
    world.init_resource::<crate::player::Player>();
    world.resource_mut::<crate::player::Player>().foreign_mover = Some(MOB);

    send(world, [moved(MOB)]);
    assert!(!held(world, MOB), "the mover's block is dropped");
    send(world, [values(MOB, &[])]);
    assert!(held(world, MOB), "and the object is still released");
    let e = world.resource::<GuidIndex>().0[&MOB];
    assert_ne!(
        world.get::<Transform>(e).unwrap().translation,
        wow_to_bevy([10.0, 20.0, 30.0]),
        "with the pose it left at"
    );

    // Control lost zeroes the mover (`0x5fa600`), and the block brings the unit back.
    send(world, [out_of_range(&[MOB])]);
    world.resource_mut::<crate::player::Player>().control_lost = true;
    send(world, [moved(MOB)]);
    assert!(
        held(world, MOB),
        "control: no mover, the block brings it back"
    );
}

/// A new object takes the oldest released one of its type, which goes for good (`0x466e55` reads
/// the head of the type's reuse list, which a release appends to at `0x464a83`): a values block
/// for its guid finds nothing, and its create is a first one again, hook and all. Another type's
/// create takes nothing, and every allocation takes one, so the next released player stays.
#[test]
fn a_new_object_frees_the_oldest_released_one_of_its_type() {
    use crate::field_watch::FieldNotice;
    const P1: u64 = 0x2001;
    const P2: u64 = 0x2002;
    const P3: u64 = 0x2003;
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    send(world, [player(P1, &[]), player(P2, &[])]);
    send(world, [out_of_range(&[P1, P2])]);
    send(world, [unit(MOB, &[(ENTRY, 299)])]);
    send(world, [player(P3, &[])]);
    send(world, [values(P1, &[]), values(P2, &[])]);
    assert!(!held(world, P1), "the oldest released player went to P3");
    assert!(held(world, P2), "the next one was kept, and comes back");
    notices(world);

    send(world, [player(P1, &[])]);
    assert!(
        notices(world).contains(&FieldNotice::Signal {
            guid: P1,
            id: crate::field_watch::id::PLAYER_GUILD_UPDATE,
        }),
        "freed: a first create"
    );
}

/// `SMSG_DESTROY_OBJECT` frees an object the out-of-range list released (`0x467521`), so its
/// next create is a first one.
#[test]
fn a_destroy_frees_a_released_object() {
    use crate::field_watch::FieldNotice;
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    send(world, [player(MATE, &[])]);
    send(world, [out_of_range(&[MATE])]);
    super::super::handlers::dispatch(world, vec![SessionEvent::ObjectDestroyed(MATE)]);
    notices(world);
    send(world, [player(MATE, &[])]);
    assert!(notices(world).contains(&FieldNotice::Global { id: 376 }));
}

/// A block brings back the object that left (`0x464530`), not a new one, so the bit an XP award set
/// in its flags word (`0x60a0e6`), which only the constructor clears (`0x5fae7a`), is still set;
/// a unit made new after its destroy is a new object, and has none.
#[test]
fn a_released_unit_keeps_its_xp_mark() {
    use crate::ui_chat::combat::watch::XpAwarded;
    const OTHER: u64 = 0xF130_0000_1234_0002;
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    send(
        world,
        [unit(MOB, &[(ENTRY, 299)]), unit(OTHER, &[(ENTRY, 299)])],
    );
    let marked = |world: &World, guid: u64| {
        let e = world.resource::<GuidIndex>().0[&guid];
        world.get::<XpAwarded>(e).is_some()
    };
    let e = world.resource::<GuidIndex>().0[&MOB];
    world.entity_mut(e).insert(XpAwarded);

    send(world, [out_of_range(&[MOB, OTHER])]);
    send(
        world,
        [values(MOB, &[(STAT0, 6)]), values(OTHER, &[(STAT0, 6)])],
    );
    assert!(marked(world, MOB), "the marked unit came back marked");
    assert!(
        !marked(world, OTHER),
        "control: an unmarked one did not gain it"
    );

    super::super::handlers::dispatch(world, vec![SessionEvent::ObjectDestroyed(MOB)]);
    send(world, [unit(MOB, &[(ENTRY, 299)])]);
    assert!(!marked(world, MOB), "a destroyed unit's next create is new");
}

/// The out-of-range list's second round releases the first pass's guids, top nibble 6 or 8,
/// without the active-player test of the first round (`0x465f8f`-`0x465fa6`).
#[test]
fn the_out_of_range_lists_second_round_spares_no_active_player() {
    const FIRST_ME: u64 = 0x6000_0000_0000_0007;
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(FIRST_ME);
    indexed(world, FIRST_ME);
    indexed(world, MOB);

    send(world, [out_of_range(&[FIRST_ME, MOB])]);
    assert!(!held(world, FIRST_ME), "the second round");
    assert!(!held(world, MOB), "the first round");
}

/// A first-pass guid's values block applies only behind a set header byte (`0x46520e`), as the
/// packet's bytes carry it; its watch runs in the notify pass either way, against what moved.
#[test]
fn a_first_pass_guids_values_block_applies_behind_the_header_byte() {
    use benilla_protocol::messages::{self, ObjectType};
    const FIRST: u64 = 0x6000_0000_0000_0042;
    // Values for FIRST, `OBJECT_FIELD_ENTRY` 0x13; then values for 0xAA, entry 0x12.
    let packet = |header: &str| {
        let body: Vec<u8> = [
            "02000000",
            header,
            "00814260010800000013000000",
            "0001aa010800000012000000",
        ]
        .concat()
        .as_bytes()
        .chunks(2)
        .map(|h| u8::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap())
        .collect();
        benilla_protocol::decode(
            messages::parse_server(messages::opcode::SMSG_UPDATE_OBJECT, &body).unwrap(),
        )
    };
    let mut app = client();
    let world = app.world_mut();
    for guid in [FIRST, 0xAA] {
        let e = world
            .spawn((
                Guid(guid),
                ObjectStore(ObjectFields::default().into_created(ObjectType::GameObject)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(guid, e);
    }

    super::super::handlers::dispatch(world, packet("00"));
    assert_eq!(field(world, FIRST, ENTRY), None, "no pass wrote it");
    assert_eq!(field(world, 0xAA, ENTRY), Some(0x12));
    assert_eq!(edges(world), [(0xAA, ENTRY, 0, 0x12)]);

    super::super::handlers::dispatch(world, packet("01"));
    assert_eq!(
        field(world, FIRST, ENTRY),
        Some(0x13),
        "the first pass wrote it"
    );
    assert_eq!(
        edges(world),
        [(FIRST, ENTRY, 0, 0x13)],
        "its watch ran in the notify pass"
    );
}

/// A first create runs no watch (`0x465d80`), so the active player's flag tutorials wait for a
/// change after it: a cross-map port creates the player afresh (`0x5dec7b`). The ghost tutorial's
/// arrival leg is the arrival's own (`0x5deb2a`, `crate::tutorial`), no notice.
#[test]
fn a_first_create_raises_no_flag_tutorial() {
    use crate::field_watch::FieldNotice;
    const PLAYER_FLAGS: u16 = 190;
    const BYTES_2: u16 = 194;
    const UNIT_FLAGS: u16 = 46;
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let tutorial = |n: &[FieldNotice]| n.iter().any(|n| matches!(n, FieldNotice::Tutorial { .. }));
    send(
        world,
        [player(
            ME,
            &[
                (PLAYER_FLAGS, 0x30),
                (UNIT_FLAGS, 0x1000),
                (BYTES_2, 1 << 24),
            ],
        )],
    );
    assert!(!tutorial(&notices(world)), "the create runs the hook alone");
    send(world, [values(ME, &[(PLAYER_FLAGS, 0x20)])]);
    send(world, [values(ME, &[(PLAYER_FLAGS, 0x30)])]);
    assert_eq!(
        notices(world)
            .into_iter()
            .filter(|n| matches!(n, FieldNotice::Tutorial { .. }))
            .collect::<Vec<_>>(),
        [FieldNotice::Tutorial { id: 0x18 }],
        "the ghost bit set again; the resting bit, already set, moved nothing"
    );
}

/// `PLAYER_GUILD_TIMESTAMP`'s player watcher (`0x5e2890`) keeps each guild's last stamp: an
/// unchanged one does nothing (`0x5e28fa`), a new one drops the guild's record (`0x561d20`) and
/// asks it again with the callback `0x5e29c0`, whose answer refreshes the guild tag of every held
/// player of that guild (`0x5e29e0`; `0x5e2a14`-`0x5e2a1d` → `0x5e08e0`), walking the object list
/// from its head (`0x468380`): a player brought back joined at the tail (`0x4646c3`-`0x4646d4`).
#[test]
fn a_new_guild_timestamp_asks_the_guild_again_and_refreshes_its_members() {
    const GUILDID: u16 = 191;
    const STAMP: u16 = 197;
    const P1: u64 = 0x2001;
    const P2: u64 = 0x2002;
    const P3: u64 = 0x2003;
    let mut app = client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let answer = || {
        vec![SessionEvent::GuildQueryResponse(
            benilla_protocol::messages::GuildQueryResponse {
                guild_id: 5,
                name: "Probe Guild".into(),
                ..Default::default()
            },
        )]
    };
    super::super::handlers::dispatch(world, answer());
    send(
        world,
        [
            player(P1, &[(GUILDID, 5), (STAMP, 100)]),
            player(P2, &[(GUILDID, 5), (STAMP, 100)]),
            player(P3, &[(GUILDID, 6), (STAMP, 100)]),
        ],
    );
    // P1 leaves and comes back: the list is now P2, P3, P1, against guid order P1, P2, P3.
    send(world, [out_of_range(&[P1])]);
    send(world, [values(P1, &[])]);
    assert!(held(world, P1));
    notices(world);
    let guild_asks = |rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>| {
        crate::query_cache::queries(rx)
            .into_iter()
            .filter(|c| matches!(c, crate::net::ClientCommand::GuildQuery { .. }))
            .count()
    };
    guild_asks(&rx);

    send(world, [values(P1, &[(STAMP, 101)])]);
    assert_eq!(guild_asks(&rx), 1, "a new stamp asks again");
    assert!(notices(world).is_empty(), "nothing until the answer");
    send(world, [values(P2, &[(STAMP, 101)])]);
    assert_eq!(guild_asks(&rx), 0, "the guild's stamp is already 101");

    super::super::handlers::dispatch(world, answer());
    let tag = |guid| crate::field_watch::guild_tag(guid);
    assert_eq!(
        notices(world),
        [tag(P2), tag(P1)].concat(),
        "every held member of the guild in list order, none of another"
    );
}

// ── Holds ───────────────────────────────────────────────────────────────────────────────────────

const DOOR: u64 = 0xF110_0000_0000_0E66;

/// A gameobject, created and then held by its state machine's play (`0x5f3b27`).
fn held_door(world: &mut World) -> Entity {
    send(
        world,
        [created(
            DOOR,
            EntityKind::GameObject,
            [1.0, 2.0, 3.0],
            &[(ENTRY, 0x12)],
        )],
    );
    let e = world.resource::<GuidIndex>().0[&DOOR];
    world
        .entity_mut(e)
        .insert(crate::go_anim::GoAnim::holding());
    e
}

/// The play ends and drops its hold (`0x5f4120` → `0x468410`); the unhold's release runs.
fn end_hold(world: &mut World, e: Entity) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .get_mut::<crate::go_anim::GoAnim>(e)
        .unwrap()
        .end_hold();
    world.run_system_once(crate::net::release_unheld).unwrap();
}

/// The out-of-range list's release of a held object only marks it release-pending (`0x464942` →
/// `0x614f00`, `0x46494f`): it stays live, indexed and drawn. The unhold releases it then
/// (`0x468410` → `0x46844a`) into the released table, so a values block brings it back.
#[test]
fn a_held_object_leaves_range_only_when_its_hold_drops() {
    let mut app = client();
    let world = app.world_mut();
    let e = held_door(world);

    send(world, [out_of_range(&[DOOR])]);
    assert!(held(world, DOOR), "held: still live");
    assert!(world.get::<Guid>(e).is_some());
    assert!(world.get::<DespawnFade>(e).is_none(), "and drawn");

    end_hold(world, e);
    assert!(!held(world, DOOR), "the unhold released it");
    assert!(world.get::<DespawnFade>(e).is_some());
    send(world, [values(DOOR, &[])]);
    assert!(held(world, DOOR), "out of the released table");
    assert_eq!(field(world, DOOR, ENTRY), Some(0x12));
}

/// `SMSG_DESTROY_OBJECT` releases through the same `0x464920` (`0x4674cf`): a held object stays
/// live, and the destroy finds no released object to free (`0x467521`). The unhold releases it
/// into the released table, where a block still brings it back.
#[test]
fn a_held_object_outlives_its_destroy_until_the_hold_drops() {
    let mut app = client();
    let world = app.world_mut();
    let e = held_door(world);

    super::super::handlers::dispatch(world, vec![SessionEvent::ObjectDestroyed(DOOR)]);
    assert!(held(world, DOOR), "held: still live");
    assert!(world.get::<DespawnFade>(e).is_none());

    end_hold(world, e);
    assert!(!held(world, DOOR), "the unhold released it");
    assert!(world.get::<DespawnFade>(e).is_some());
    send(world, [values(DOOR, &[])]);
    assert!(held(world, DOOR), "released, not freed");
    assert_eq!(field(world, DOOR, ENTRY), Some(0x12));
}

/// A values or near-objects block that finds the pending object live clears its pending bit
/// (`0x464530` → `0x4645b2`), so its unhold releases nothing.
#[test]
fn a_block_that_finds_a_pending_object_cancels_its_release() {
    let mut app = client();
    let world = app.world_mut();
    let e = held_door(world);

    send(world, [out_of_range(&[DOOR])]);
    send(world, [values(DOOR, &[])]);
    end_hold(world, e);
    assert!(held(world, DOOR), "a values block");
    assert!(world.get::<DespawnFade>(e).is_none());

    world
        .entity_mut(e)
        .insert(crate::go_anim::GoAnim::holding());
    send(world, [out_of_range(&[DOOR])]);
    send(world, [SessionEvent::ObjectsNear(vec![DOOR])]);
    end_hold(world, e);
    assert!(held(world, DOOR), "a near-objects block");
    assert!(world.get::<DespawnFade>(e).is_none());
}

/// A door resting on Closed whose model owns `anims`, with a second field only its first create
/// carries.
fn animated_door(world: &mut World, guid: u64, anims: &[u16]) -> Entity {
    send(
        world,
        [created(
            guid,
            EntityKind::GameObject,
            [1.0, 2.0, 3.0],
            &[(ENTRY, 0x12), (STALE, 0x55)],
        )],
    );
    let e = world.resource::<GuidIndex>().0[&guid];
    world.entity_mut(e).insert((
        crate::go_anim::GoAnim::resting(),
        crate::go_anim::owned_anims(anims),
    ));
    e
}

/// A field the re-create below does not send.
const STALE: u16 = 8;

/// vmangos sends `SMSG_GAMEOBJECT_DESPAWN_ANIM` and then the destroy (`GameObject.cpp:860`,
/// `:2472`). The despawn holds only when the model has 157 (`0x5f424a`): without it nothing
/// plays, the destroy frees the object at once (`0x467521`), and a create of the guid makes a
/// fresh one that runs its create hook, keeping nothing of the old.
#[test]
fn a_despawn_the_model_cannot_play_holds_nothing_from_its_destroy() {
    const OWNED: u64 = 0xF110_0000_0000_0E67;
    let mut app = client();
    let world = app.world_mut();
    animated_door(world, DOOR, &[146, 147, 148, 149]);
    animated_door(world, OWNED, &[146, 147, 148, 149, 157]);

    super::super::handlers::dispatch(
        world,
        vec![
            SessionEvent::GameObjectDespawnAnim { guid: DOOR },
            SessionEvent::ObjectDestroyed(DOOR),
            SessionEvent::GameObjectDespawnAnim { guid: OWNED },
            SessionEvent::ObjectDestroyed(OWNED),
        ],
    );
    assert!(held(world, OWNED), "control: a play of 157 holds it");
    assert!(!held(world, DOOR), "no 157: the destroy frees it");

    send(world, [values(DOOR, &[])]);
    assert!(
        !held(world, DOOR),
        "freed, not released: nothing brings it back"
    );
    send(
        world,
        [created(
            DOOR,
            EntityKind::GameObject,
            [1.0, 2.0, 3.0],
            &[(ENTRY, 0x13)],
        )],
    );
    assert_eq!(field(world, DOOR, ENTRY), Some(0x13));
    assert_eq!(field(world, DOOR, STALE), None, "a fresh object");
}

/// A `GAMEOBJECT_STATE` change to a motion the model lacks (Destroy, 150) still holds: the state
/// arm plays and holds unchecked (`0x5f3af8`-`0x5f3b27`). The destroy behind it only marks the
/// object pending (`0x464920`), and the unhold parks it in the released table, where a block
/// brings back the object that left.
#[test]
fn a_state_change_to_a_motion_the_model_lacks_holds_through_the_destroy() {
    use benilla_protocol::field::FIELD_GAMEOBJECT_STATE;
    let mut app = client();
    let world = app.world_mut();
    let e = animated_door(world, DOOR, &[146, 147, 148, 149]);

    send(world, [values(DOOR, &[(FIELD_GAMEOBJECT_STATE, 2)])]);
    super::super::handlers::dispatch(world, vec![SessionEvent::ObjectDestroyed(DOOR)]);
    assert!(held(world, DOOR), "held: the destroy only marks it");

    use bevy::ecs::system::RunSystemOnce;
    world.run_system_once(crate::net::release_unheld).unwrap();
    assert!(!held(world, DOOR), "the unhold released it");
    assert!(world.get::<DespawnFade>(e).is_some());
    send(world, [values(DOOR, &[])]);
    assert!(held(world, DOOR), "parked, not freed");
    assert_eq!(
        field(world, DOOR, STALE),
        Some(0x55),
        "the object that left"
    );
}

/// `OnCharmOrSummonChanged` (`0x5e2660`), the player type watcher on the unit section's first 16
/// bytes, `CHARM` then `SUMMON` (`0x5e257a`): for the active player or a party or raid member
/// (`0x5e266b`, `0x4918e0`), an old guid the object manager does not hold that is no longer the
/// field's value has its selection torn down (`0x5e2709` → `0x5e2719`). A player outside the group
/// raises nothing, nor an old pet still held (`0x5e2702`).
#[test]
fn a_groupmates_pet_leaving_its_field_tears_down_that_pets_selection() {
    use benilla_protocol::messages::{member_status, GroupMemberEntry};
    const MATE: u64 = 0x51;
    const STRANGER: u64 = 0x52;
    const PET: u64 = 0xF140_0000_0000_0077;
    const NEAR_PET: u64 = 0xF140_0000_0000_0078;
    let guid = |g: u64, at: u16| [(at, g as u32), (at + 1, (g >> 32) as u32)];
    let mut app = client();
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    world
        .resource_mut::<crate::ui_party::GroupState>()
        .apply_list(
            0,
            0,
            vec![GroupMemberEntry {
                name: "Brisca".into(),
                guid: MATE,
                status: member_status::ONLINE,
                flags: 0,
            }],
            ME,
            None,
            Some(ME),
        );
    let torn = |world: &mut World| -> Vec<u64> {
        world
            .resource_mut::<Messages<crate::target::DeselectGuid>>()
            .drain()
            .map(|d| d.0)
            .collect()
    };
    send(
        world,
        [
            player(ME, &[]),
            player(MATE, &guid(PET, 8)),
            player(STRANGER, &guid(PET, 8)),
            // `OBJECT_FIELD_TYPE`: a unit, which the old pet's lookup asks for (`0x5e26f7`).
            unit(NEAR_PET, &[(2, 0x9)]),
        ],
    );
    torn(world);

    send(world, [values(MATE, &guid(0, 8))]);
    assert_eq!(torn(world), [PET], "a groupmate's summon");
    send(world, [values(STRANGER, &guid(0, 8))]);
    assert!(torn(world).is_empty(), "a player outside the group");
    send(world, [values(ME, &guid(PET, 6))]);
    send(world, [values(ME, &guid(NEAR_PET, 6))]);
    assert_eq!(torn(world), [PET], "our own charm, moved off");
    send(world, [values(ME, &guid(0, 6))]);
    assert!(torn(world).is_empty(), "an old pet still held");
}
