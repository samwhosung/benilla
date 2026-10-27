//! A world click in the frame's order, on the whole client headless: the clicks run among the
//! frame's input records, ahead of the net drain, as the reference's world-frame handlers run in
//! the window input (`0x423920` at `0x420d23`) before the packets (category 6 at `0x420d55`).

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use bevy::prelude::*;

use super::{click, Hovered, PressPick, Selection};
use crate::net::{ClientCommand, GuidIndex, SelfGuid};
use crate::test_support::runs_before;

const ME: u64 = 0x10;
const MOB: u64 = 0xF130_0000_0031_0001;

/// A create block of `kind` at full health, with its `OBJECT_FIELD_TYPE` mask.
fn created(guid: u64, kind: EntityKind, as_type: ObjectType, mask: u32) -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id: None,
        position: [3.0, 0.0, 0.0],
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        fields: ObjectFields::from_pairs(&[
            (2, mask),
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
        ])
        .into_created(as_type),
    }
}

/// A step of the frame the test runs, each the client's own system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The world click's select.
    Click,
    /// The net drain, which applies the frame's packets, a unit's release tearing its selection
    /// down inside it.
    Net,
}

/// Binds `$s` to the client's system for `$step` and evaluates `$body` with it.
macro_rules! with_system {
    ($step:expr, |$s:ident| $body:expr) => {
        match $step {
            Step::Click => {
                let $s = crate::ui_script::dispatch_input_records;
                $body
            }
            Step::Net => {
                let $s = crate::net::apply_net_updates;
                $body
            }
        }
    };
}

/// The steps in the order `Update` declares them; panics on a pair it leaves unordered.
fn in_frame_order(app: &mut App) -> Vec<Step> {
    let mut steps = vec![Step::Click, Step::Net];
    let mut before = Vec::new();
    for (i, a) in steps.iter().enumerate() {
        for b in &steps[i + 1..] {
            let ab = with_system!(*a, |x| with_system!(*b, |y| runs_before(app, x, y)));
            let ba = with_system!(*b, |x| with_system!(*a, |y| runs_before(app, x, y)));
            assert!(
                ab != ba,
                "{a:?} and {b:?} run in an order the executor picks"
            );
            before.push(if ab { (*a, *b) } else { (*b, *a) });
        }
    }
    steps.sort_by(|x, y| {
        if x == y {
            std::cmp::Ordering::Equal
        } else if before.contains(&(*x, *y)) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    steps
}

/// A left click released over a unit in the frame whose packets remove it, the frame's steps in
/// the schedule's order: the click selects it (`CMSG_SET_SELECTION` with its guid), and the
/// removal's release then tears the selection down with its own send (`0x5fbbe5`). With the
/// packets first, the click would select a guid whose object has gone, with no release left to
/// end it.
#[test]
fn a_click_on_a_unit_its_frames_packet_removes_selects_it_first() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let order = in_frame_order(&mut app);
    assert_eq!(order, [Step::Click, Step::Net]);
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
    for ev in crate::net::wire([
        created(ME, EntityKind::Player, ObjectType::Player, 0x19),
        created(MOB, EntityKind::Unit, ObjectType::Unit, 0x9),
        SessionEvent::ObjectUpdateEnd,
    ]) {
        inbound.send(ev).unwrap();
    }
    app.update();
    let world = app.world_mut();
    let mob = world.resource::<GuidIndex>().0[&MOB];
    sent.try_iter().for_each(drop);

    // The release over the mob, and the packet that removes it, in one frame.
    world.resource_mut::<PressPick>().hovered = Hovered {
        target: Some(mob),
        guid: Some(MOB),
        distance: 3.0,
        ..Hovered::default()
    };
    crate::script_calls::insert_test_vm(world);
    for ev in crate::net::wire([
        SessionEvent::ObjectsRemoved(vec![MOB]),
        SessionEvent::ObjectUpdateEnd,
    ]) {
        inbound.send(ev).unwrap();
    }
    for step in order {
        match step {
            Step::Click => {
                super::release_click(world, benilla_ui::script::ClickMode::Select);
            }
            Step::Net => crate::net::apply_net_updates(world),
        }
    }

    let selections: Vec<u64> = sent
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::SetSelection { guid } => Some(guid),
            _ => None,
        })
        .collect();
    assert_eq!(
        selections,
        [MOB, 0],
        "the click lands, then the removal ends it"
    );
    assert_eq!(world.resource::<Selection>().guid, None);
}

/// A right-click's cast goes out inside the click, among the frame's input records and ahead of
/// the packets: the reference's release handler reaches the click router (`0x514b70` →
/// `0x481f60`), whose dispatchers try the cast (`0x5f35c0`, `0x5f06dc` → `0x6e5a90`). A walk's
/// arrival casts in the frame time, after its stop.
#[test]
fn a_right_clicks_cast_goes_out_ahead_of_the_packets() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    assert!(crate::test_support::runs_before_in(
        &mut app,
        crate::target::WorldClickLegs,
        click::act_on_interact,
        crate::ui_action::drain_click_casts
    ));
    assert!(runs_before(
        &mut app,
        crate::ui_script::dispatch_input_records,
        crate::net::apply_net_updates
    ));
    assert!(runs_before(
        &mut app,
        click::act_on_arrival,
        crate::ui_action::drain_arrival_casts
    ));
}
