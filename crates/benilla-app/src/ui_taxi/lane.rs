//! The taxi lane: what the flight master's packet handlers signal, each fired at its packet's
//! place among the other lanes' items ([`crate::lanes`]). In the reference each signals inside
//! itself: the map's builder its `TAXIMAP_OPENED` (`0x4dba96`) and then the Travel tutorial
//! (`0x4dbaa2`), the activate reply's refusal (`0x5ed1e0`) and the first-visit learn
//! (`0xf2`, `ERR_NEWTAXIPATH`) their lines through `DisplayError` (`0x496720`), the learn's then
//! the gossip close (`0x5e3e68`), whose signals wait parked behind it. A handler queues
//! its item here, stamped where it runs, and the getters its listeners call read the map through
//! the host ([`super::live`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::lanes::{LaneApp, Stamp, Until};
use crate::ui_action::MessageSink;

/// What an item signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaxiEvent {
    /// `TAXIMAP_OPENED` (`0x4dba96`), then the Travel tutorial with no delay (`0x4dbaa2`).
    Opened,
    /// A message line, by its `GlobalStrings` key.
    Line(&'static str),
    /// The close core's `TAXIMAP_CLOSED` (`0x4dc15f`), queued by a close that follows the open
    /// still on this lane.
    Closed,
}

/// The taxi lane's queue.
#[derive(Resource, Default)]
pub(crate) struct TaxiLane {
    events: VecDeque<(Stamp, TaxiEvent)>,
}

impl TaxiLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: TaxiEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, TaxiEvent)> {
        self.events.iter().copied().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_taxi_lane(app: &mut App) {
    app.init_resource::<TaxiLane>()
        .add_lane(
            "taxi",
            |world| world.get_resource::<TaxiLane>()?.head(),
            fire_taxi,
            None,
        )
        .add_systems(
            Update,
            drop_taxi_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_taxi_events(mut lane: ResMut<TaxiLane>) {
    lane.events.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_taxi(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut lane: ResMut<TaxiLane>,
    mut sink: MessageSink,
    // Absent in a harness with no tutorials.
    tutorials: Option<Res<crate::tutorial::Tutorials>>,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    while let Some(at) = lane.head().filter(|&s| until.admits(s)) {
        let Some((_, event)) = lane.events.pop_front() else {
            return;
        };
        let ((), moved) = crate::lanes::firing(at, || {
            vm.run(|script| {
                if let Some(script) = script {
                    fire(script, &mut sink, tutorials.as_deref(), event);
                }
            })
        });
        if moved {
            return;
        }
    }
}

/// One item's signal: the event carries no argument (`0x703e50`).
fn fire(
    script: &mut UiScript,
    sink: &mut MessageSink,
    tutorials: Option<&crate::tutorial::Tutorials>,
    event: TaxiEvent,
) {
    match event {
        TaxiEvent::Opened => {
            script.fire_event("TAXIMAP_OPENED", vec![]);
            // The Travel tutorial straight after, whatever a listener did to the map.
            let published = tutorials.and_then(|t| t.lock().fire_now(crate::tutorial::id::TRAVEL));
            if let Some(published) = published {
                crate::tutorial::fire_published(script, &sink.sounds, published);
            }
        }
        // Shown, and spoken, as its message row says, inside the handler that adds it.
        TaxiEvent::Line(key) => {
            if let Some(line) = crate::ui_action::keyed_line(script, key) {
                crate::ui_action::show_now(script, sink, "ui_taxi", line);
            }
        }
        TaxiEvent::Closed => {
            crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Taxi, || {
                script.fire_event("TAXIMAP_CLOSED", vec![])
            })
        }
    }
}

/// Whether a close starting now waits behind the lane ([`crate::lanes::waits_behind`]): the open it
/// closes, or any earlier signal, has not fired yet.
pub(super) fn close_waits(world: &World) -> bool {
    crate::lanes::waits_behind(world.get_resource::<TaxiLane>().and_then(TaxiLane::head))
}

/// The close core's `TAXIMAP_CLOSED`: queued on the lane when the close `waits` ([`close_waits`]),
/// so it fires after the `TAXIMAP_OPENED` it closes; else fired now with the host lent.
pub(super) fn close_signal(world: &mut World, waits: bool) {
    if waits {
        if let Some(mut lane) = world.get_resource_mut::<TaxiLane>() {
            lane.push(TaxiEvent::Closed);
            return;
        }
    }
    crate::ui_session::fire(world, |script| {
        script.fire_event("TAXIMAP_CLOSED", vec![]);
    });
}
