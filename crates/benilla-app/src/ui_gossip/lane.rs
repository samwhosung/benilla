//! The gossip lane: the greeting's `GOSSIP_SHOW`, fired at its packet's place among the other
//! lanes' items ([`crate::lanes`]). In the reference it signals inside `0x4e2010` (`0x4e22b5`),
//! right after the text's write (`0x4e229a`), which a message runs (`0x4e27df`) and so does its
//! text's arrival (`0x4e22f2`); the getters its listeners call read the store through the host
//! ([`super::live`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::lanes::{LaneApp, Stamp, Until};

/// What an item signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GossipEvent {
    /// `GOSSIP_SHOW` (`0x4e22b5`), with no argument.
    Show,
    /// What a close core run outside a call signalled while this lane held items, parked in the
    /// VM of the session under the key ([`crate::script_calls::park_since`]).
    Parked { session: u64, key: u64 },
}

/// The gossip lane's queue.
#[derive(Resource, Default)]
pub(crate) struct GossipLane {
    events: VecDeque<(Stamp, GossipEvent)>,
}

impl GossipLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: GossipEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, GossipEvent)> {
        self.events.iter().copied().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_gossip_lane(app: &mut App) {
    app.init_resource::<GossipLane>()
        .add_lane(
            "gossip",
            |world| world.get_resource::<GossipLane>()?.head(),
            fire_gossip,
            None,
        )
        .add_systems(
            Update,
            drop_gossip_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_gossip_events(mut lane: ResMut<GossipLane>) {
    lane.events.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_gossip(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut lane: ResMut<GossipLane>,
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
                    fire(script, event);
                }
            })
        });
        if moved {
            return;
        }
    }
}

fn fire(script: &mut UiScript, event: GossipEvent) {
    match event {
        GossipEvent::Show => script.fire_event("GOSSIP_SHOW", vec![]),
        GossipEvent::Parked { session, key } => {
            if script.session() == session {
                crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Gossip, || {
                    script.run_parked(key)
                });
            }
        }
    }
}

/// Run the close `core` behind the lane ([`crate::lanes::behind`]), so a close never fires ahead
/// of the show it closes.
pub(super) fn behind_lane(world: &mut World, core: impl FnOnce(&mut World)) {
    let head = world
        .get_resource::<GossipLane>()
        .and_then(GossipLane::head);
    crate::lanes::behind(world, head, core, |world, session, key| {
        world
            .resource_mut::<GossipLane>()
            .push(GossipEvent::Parked { session, key });
    });
}
