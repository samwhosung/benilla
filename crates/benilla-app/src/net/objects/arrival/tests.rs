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
    net: Res<NetCommands>,
    mut seen: ResMut<Seen>,
) {
    seen.0.push((!me.is_empty(), !own.0));
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
/// goes out after the mover's claim (`0x5deb5b`), the flag's rise is on the notice lane, and
/// the class family is stored (`0x5debcc`).
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
    let signals: Vec<_> = world
        .resource_mut::<crate::field_watch::NoticeLane>()
        .drain()
        .collect();
    assert!(
        signals.contains(&crate::field_watch::FieldNotice::PlayerControl { in_control: true }),
        "{signals:?}"
    );
    assert_eq!(
        world
            .resource::<crate::spell::SpellModifiers>()
            .class_family(),
        3
    );
    let cascades = world.resource::<Messages<crate::net::WorldEnterCascadeMessage>>();
    assert_eq!(cascades.len(), 1, "one cascade for its readers");
}

/// Only a new object runs its create hook (`0x465d79`): a create block for our live player
/// arrives nothing again, and the create after a purge (a worldport) arrives afresh.
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

    let e = world.resource_mut::<GuidIndex>().0.remove(&ME).unwrap();
    world.despawn(e);
    crate::net::handlers::dispatch(world, crate::net::objects::packet([self_create()]));
    assert_eq!(movers(&rx), [format!("SetActiveMover {ME:#x}")]);
    let mut tagged = world.query_filtered::<(), With<SelfPlayer>>();
    assert_eq!(tagged.iter(world).count(), 1, "the new body is tagged");
}
