//! What a health packet for another unit leaves alone. The reference's update handler writes the
//! unit's descriptor and signals its watchers (`0x4651a0`, `0x465277`); nothing of the bags, the
//! windows, the guild or the auction house moves, so no feed that reads them has anything to
//! push, and no change tick says otherwise.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::{EntityKind, SessionEvent};
use bevy::ecs::change_detection::Tick;
use bevy::prelude::*;

use super::unit_view_tests::{client, created, values, wire_packets};

const WOLF: u64 = 0xF130_0000_0044_0001;

/// The last-changed tick of each resource a health packet must leave alone, by name.
fn ticks(world: &World) -> Vec<(&'static str, Option<Tick>)> {
    fn tick<T: Resource>(world: &World) -> Option<Tick> {
        world.get_resource_ref::<T>().map(|r| r.last_changed())
    }
    vec![
        ("Items", tick::<crate::items::Items>(world)),
        ("GuildState", tick::<crate::ui_guild::GuildState>(world)),
        ("AuctionOpen", tick::<crate::ui_auction::AuctionOpen>(world)),
        ("TradeSession", tick::<crate::ui_trade::TradeSession>(world)),
        ("LootState", tick::<crate::ui_loot::LootState>(world)),
        ("DeathNet", tick::<crate::death::DeathNet>(world)),
        ("BagCache", tick::<crate::item_watch::BagCache>(world)),
        ("HouseLane", tick::<crate::ui_auction::HouseLane>(world)),
    ]
}

/// A health packet for a creature moves no tick of the bags', the windows', the guild's, the
/// death state's or the auction house's: its handlers write none of them.
#[test]
fn a_units_health_packet_changes_nothing_but_the_unit() {
    let mut app = client();
    let world = app.world_mut();
    let before = ticks(world);
    assert!(before.iter().all(|(_, t)| t.is_some()), "{before:?}");
    world.increment_change_tick();
    crate::net::handlers::dispatch(
        world,
        wire_packets(vec![vec![values(WOLF, &[(FIELD_UNIT_HEALTH, 70)])]]),
    );
    let after = ticks(world);
    let moved: Vec<&str> = before
        .iter()
        .zip(&after)
        .filter(|(b, a)| b.1 != a.1)
        .map(|(b, _)| b.0)
        .collect();
    assert!(moved.is_empty(), "these moved: {moved:?}");
}

/// The whole client in the world with a VM, its frame run off the inbound channel.
fn world_client() -> (App, crossbeam_channel::Sender<SessionEvent>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    // A system that needs the render world skips here.
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    std::mem::forget(sent);
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    app.insert_non_send_resource(benilla_ui::script::UiScript::new().expect("a VM"));
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(0x7);
    let mob = [(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)];
    for ev in wire_packets(vec![
        vec![created(0x7, EntityKind::Player, &mob)],
        vec![created(WOLF, EntityKind::Unit, &mob)],
    ]) {
        inbound.send(ev).expect("inbound");
    }
    for _ in 0..3 {
        app.update();
    }
    (app, inbound)
}

/// A frame with a creature's health packet moves no tick of those resources either: no feed or
/// lane of the frame writes them for it.
#[test]
fn a_frame_with_a_units_health_packet_changes_nothing_but_the_unit() {
    let (mut app, inbound) = world_client();
    let before = ticks(app.world());
    for ev in wire_packets(vec![vec![values(WOLF, &[(FIELD_UNIT_HEALTH, 70)])]]) {
        inbound.send(ev).expect("inbound");
    }
    app.update();
    let after = ticks(app.world());
    let moved: Vec<&str> = before
        .iter()
        .zip(&after)
        .filter(|(b, a)| b.1 != a.1)
        .map(|(b, _)| b.0)
        .collect();
    assert!(moved.is_empty(), "these moved: {moved:?}");
}
