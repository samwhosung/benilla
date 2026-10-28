//! Our own player's arrival through the handler table.

use benilla_protocol::messages::ObjectType;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent, SessionEventKind};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, SelfGuid, SelfPlayer};

const ME: u64 = 0x0000_0000_0000_0007;
/// `UNIT_FIELD_BYTES_0`, whose byte 1 is the class.
const UNIT_BYTES_0: u16 = 36;
/// A mage: class 8, spell family 3.
const MAGE: u32 = 8;

/// What a handler of a packet after the create found: our player tagged, and in control.
#[derive(Resource, Default)]
struct Seen(Vec<(bool, bool)>);

/// A later packet's handler: what it reads of our player, and a send to mark its place.
fn later_packet(
    In(_): In<SessionEvent>,
    me: Query<(), With<SelfPlayer>>,
    own: Res<crate::player::OwnControlLost>,
    lock: Res<crate::player::MoverLock>,
    net: Res<NetCommands>,
    mut seen: ResMut<Seen>,
) {
    seen.0.push((
        !me.is_empty(),
        !own.0 && !lock.control_lost && lock.foreign_mover.is_none(),
    ));
    let _ = net.0.send(ClientCommand::QueryTime);
}

fn later() -> SessionEvent {
    SessionEvent::LoginQueued {
        position: None,
        realm: None,
    }
}

/// The client, logged in as [`ME`] with control lost, as a battleground's end leaves it, and
/// [`later_packet`] on a packet kind of its own.
fn client() -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx))
        .insert_resource(crate::chr_classes::ChrClassTable(
            benilla_formats::ChrClasses::from_rows([(MAGE, "Mage", "MAGE")])
                .with_spell_family(MAGE, 3),
        ))
        .init_resource::<Seen>()
        .net_handler(SessionEventKind::LoginQueued, later_packet);
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    world.resource_mut::<crate::player::OwnControlLost>().0 = true;
    world.insert_resource(crate::player::MoverLock {
        control_lost: true,
        foreign_mover: Some(0xF130_0000_0000_0099),
        reseat: false,
    });
    (app, rx)
}

fn self_create() -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid: ME,
        kind: EntityKind::Player,
        display_id: None,
        position: [0.0; 3],
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        fields: ObjectFields::from_pairs(&[(UNIT_BYTES_0, MAGE << 8)])
            .into_created(ObjectType::Player),
    }
}

fn movers(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::SetActiveMover { guid } => Some(format!("SetActiveMover {guid:#x}")),
            ClientCommand::QueryTime => Some("later packet".into()),
            _ => None,
        })
        .collect()
}

/// The arrival's writes are its packet's (`0x5dec7b` → `0x5dea50`): a packet after our own
/// create in the same drain finds our player tagged and in control (`0x5deb56`), its send
/// goes out after the mover's claim (`0x5deb5b`), and the flag's rise and the class family
/// (`0x5debcc`) are owed to the arrival's item.
#[test]
fn a_packet_after_our_create_finds_us_arrived() {
    let (mut app, rx) = client();
    let world = app.world_mut();
    crate::net::handlers::dispatch(
        world,
        crate::net::objects::packet([self_create()])
            .into_iter()
            .chain([later()])
            .collect(),
    );
    assert_eq!(
        world.resource::<Seen>().0,
        [(true, true)],
        "the later packet's handler reads our player tagged and in control"
    );
    assert_eq!(
        movers(&rx),
        [format!("SetActiveMover {ME:#x}"), "later packet".into()]
    );
    assert_eq!(
        world
            .resource::<crate::spell::SpellModifiers>()
            .class_family(),
        0,
        "the class family is the arrival item's last step (`0x5debcc`)"
    );
    assert_eq!(
        world
            .resource::<crate::ui_world_enter::ArrivalLane>()
            .entries(),
        [crate::ui_world_enter::Entry::Arrival {
            control_gained: true
        }],
        "one arrival on its lane, the flag's rise owed among its signals"
    );
    crate::ui_world_enter::fire_lane(world);
    assert_eq!(
        world
            .resource::<crate::spell::SpellModifiers>()
            .class_family(),
        3,
        "set once the item has run its cascade"
    );
}

/// Only a new object runs its create hook (`0x465d79`): a create block for our live player
/// arrives nothing again, and the create after a worldport's purge arrives afresh.
#[test]
fn only_a_fresh_create_arrives() {
    let (mut app, rx) = client();
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, crate::net::objects::packet([self_create()]));
    assert_eq!(movers(&rx).len(), 1);
    crate::net::handlers::dispatch(world, crate::net::objects::packet([self_create()]));
    assert_eq!(
        movers(&rx),
        Vec::<String>::new(),
        "a re-create of the live player"
    );

    crate::net::handlers::dispatch(
        world,
        std::iter::once(worldport())
            .chain(crate::net::objects::packet([self_create()]))
            .collect(),
    );
    assert_eq!(movers(&rx), [format!("SetActiveMover {ME:#x}")]);
    let mut tagged = world.query_filtered::<(), With<SelfPlayer>>();
    assert_eq!(tagged.iter(world).count(), 1, "the new body is tagged");
}

fn connected() -> SessionEvent {
    SessionEvent::Connected {
        self_guid: ME,
        name: "Me".into(),
        billing: Default::default(),
        tutorial_flags: None,
        addon_info: None,
    }
}

fn worldport() -> SessionEvent {
    SessionEvent::Worldport {
        map_id: 1,
        position: [0.0; 3],
        orientation: 0.0,
        needs_ack: false,
    }
}

/// `SetActiveMover` returns at once for the mover it holds (`0x6006ee`-`0x6006fb` →
/// `0x600852`): a create whose mover nothing zeroed claims nothing, though the rest of the
/// arrival runs.
#[test]
fn a_create_of_the_mover_already_held_sends_nothing() {
    let (mut app, rx) = client();
    let world = app.world_mut();
    let net = NetCommands(world.resource::<NetCommands>().0.clone());
    assert!(world
        .resource_mut::<crate::net::MoverClaim>()
        .claim(ME, &net));
    assert_eq!(movers(&rx).len(), 1, "the claim that stands");
    crate::net::handlers::dispatch(world, crate::net::objects::packet([self_create()]));
    assert_eq!(movers(&rx), Vec::<String>::new(), "no second claim");
    assert!(
        !world.resource::<crate::player::OwnControlLost>().0,
        "the restore still runs"
    );
}

/// What zeroes the mover makes the next create claim it again: the world entry
/// (`CGUnit_C::Initialize`, `0x6039a9`), and a world load's purge, whose destruction of our
/// player zeroes the mover it names (`0x401bf7` → `0x467700`, `0x5fb650`).
#[test]
fn the_world_entry_and_a_worldport_zero_the_mover() {
    let (mut app, rx) = client();
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, crate::net::objects::packet([self_create()]));
    assert_eq!(movers(&rx).len(), 1);

    crate::net::handlers::dispatch(
        world,
        std::iter::once(worldport())
            .chain(crate::net::objects::packet([self_create()]))
            .collect(),
    );
    assert_eq!(
        movers(&rx),
        [format!("SetActiveMover {ME:#x}")],
        "the worldport's create claims again"
    );

    // A new session over a world nothing tore down: its entry zeroes the mover.
    let e = world.resource_mut::<GuidIndex>().0.remove(&ME).unwrap();
    world.despawn(e);
    let net = NetCommands(world.resource::<NetCommands>().0.clone());
    world
        .resource_mut::<crate::net::MoverClaim>()
        .claim(ME, &net);
    movers(&rx);
    crate::net::handlers::dispatch(
        world,
        std::iter::once(connected())
            .chain(crate::net::objects::packet([self_create()]))
            .collect(),
    );
    assert_eq!(
        movers(&rx),
        [format!("SetActiveMover {ME:#x}")],
        "the session's first create claims"
    );
}

/// A `/reload` re-enters the world-enter cascade alone (`0x490168` → `0x4908c0`): the restore
/// and the claim are the arrival's (`0x4958a0` is called from `0x5deb5b` only, `0x5fa600` from
/// `0x5deb56` and `0x603eeb`), so a reload while feared, possessing or locked by a
/// battleground's end leaves the lock, the possession and `[0xb4b3e4]` as they were.
#[test]
fn a_reload_cascade_leaves_control_as_it_was() {
    let (mut app, _rx) = client();
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_world_enter::ArrivalLane>()
        .push(crate::ui_world_enter::Entry::Reload);
    crate::ui_world_enter::fire_lane(world);
    assert!(world.resource::<crate::player::OwnControlLost>().0);
    let lock = world.resource::<crate::player::MoverLock>();
    assert!(lock.control_lost && lock.foreign_mover.is_some());
}
