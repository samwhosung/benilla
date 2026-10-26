//! The tabard lane: what the designer's packet handlers signal, each fired at its packet's place
//! among the other lanes' items ([`crate::lanes`]). In the reference each signals inside itself:
//! the open its `OPEN_TABARD_FRAME` (`0x4f588d`), the save reply its line through `DisplayError`
//! (`0x5e711b` → `0x496720`) and, past a failure, `TABARD_SAVE_PENDING` (`0x5e7172`). A handler
//! queues its item here, stamped where it runs, and the getters its listeners call read the guild
//! record and the latch through the host ([`super::live`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::lanes::{LaneApp, Stamp, Until};
use crate::ui_action::MessageSink;

/// What an item signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabardEvent {
    /// `OPEN_TABARD_FRAME` (`0x4f588d`).
    Open,
    /// `TABARD_SAVE_PENDING` (`0x5e7172`).
    SavePending,
    /// A save reply's message line, by its `GlobalStrings` key.
    Line(&'static str),
    /// What a close core run outside a call signalled while this lane held items, parked in the
    /// VM of the session under the key ([`crate::script_calls::park_since`]).
    Parked { session: u64, key: u64 },
}

/// The tabard lane's queue.
#[derive(Resource, Default)]
pub(crate) struct TabardLane {
    events: VecDeque<(Stamp, TabardEvent)>,
}

impl TabardLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: TabardEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, TabardEvent)> {
        self.events.iter().copied().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_tabard_lane(app: &mut App) {
    app.init_resource::<TabardLane>()
        .add_lane(
            "tabard",
            |world| world.get_resource::<TabardLane>()?.head(),
            fire_tabard,
            None,
        )
        .add_systems(
            Update,
            drop_tabard_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_tabard_events(mut lane: ResMut<TabardLane>) {
    lane.events.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_tabard(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut lane: ResMut<TabardLane>,
    mut sink: MessageSink,
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
                    fire(script, &mut sink, event);
                }
            })
        });
        if moved {
            return;
        }
    }
}

/// One item's signal: neither event carries an argument (`0x703e50`).
fn fire(script: &mut UiScript, sink: &mut MessageSink, event: TabardEvent) {
    match event {
        TabardEvent::Open => script.fire_event("OPEN_TABARD_FRAME", vec![]),
        TabardEvent::SavePending => script.fire_event("TABARD_SAVE_PENDING", vec![]),
        // Shown, and spoken, as its message row says, inside the handler that adds it.
        TabardEvent::Line(key) => {
            if let Some(line) = crate::ui_action::keyed_line(script, key) {
                crate::ui_action::show_now(script, sink, "ui_tabard", line);
            }
        }
        TabardEvent::Parked { session, key } => {
            if script.session() == session {
                script.run_parked(key);
            }
        }
    }
}

/// Run the close `core` behind the lane ([`crate::lanes::behind`]), so a close never fires ahead
/// of the open it closes.
pub(super) fn behind_lane(world: &mut World, core: impl FnOnce(&mut World)) {
    let head = world
        .get_resource::<TabardLane>()
        .and_then(TabardLane::head);
    crate::lanes::behind(world, head, core, |world, session, key| {
        world
            .resource_mut::<TabardLane>()
            .push(TabardEvent::Parked { session, key });
    });
}
