//! The stable lane: what the stable master's packet handlers and the window's creature callbacks
//! signal, each fired at its packet's place among the other lanes' items ([`crate::lanes`]). In
//! the reference each signals inside itself: the list its selection's clear and `PET_STABLE_SHOW`
//! (`0x4cadf8`, `0x4cac9b`), the result its line through `DisplayError` (`0x4cacde` →
//! `0x496720`) or code 11's `PET_STABLE_UPDATE` (`0x4cad83`), and the creature callbacks theirs
//! (`0x4cb3bf`, `0x4cba5f`). A handler queues its item here, stamped where it runs, and the
//! getters its listeners call read the stable through the host ([`super::live`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::lanes::{LaneApp, Stamp, Until};
use crate::ui_action::MessageSink;

/// What an item signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StableEvent {
    /// A list's clear of the selection (`0x4cadf8`), then `PET_STABLE_SHOW` (`0x4cac9b`).
    Show,
    /// `PET_STABLE_UPDATE` (438).
    Update,
    /// `PET_STABLE_UPDATE_PAPERDOLL` (439).
    Paperdoll,
    /// A result's message line, by its `GlobalStrings` key.
    Line(&'static str),
    /// What a close core run outside a call signalled while this lane held items, parked in the
    /// VM of the session under the key ([`crate::script_calls::park_since`]).
    Parked { session: u64, key: u64 },
}

/// The stable lane's queue.
#[derive(Resource, Default)]
pub(crate) struct StableLane {
    events: VecDeque<(Stamp, StableEvent)>,
}

impl StableLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: StableEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, StableEvent)> {
        self.events.iter().copied().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_stable_lane(app: &mut App) {
    app.init_resource::<StableLane>()
        .add_lane(
            "stable",
            |world| world.get_resource::<StableLane>()?.head(),
            fire_stable,
            None,
        )
        .add_systems(
            Update,
            drop_stable_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_stable_events(mut lane: ResMut<StableLane>) {
    lane.events.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_stable(
    In(until): In<Until>,
    mut vm: crate::script_calls::VmWith<super::StableLive<'static, 'static>>,
    mut lane: ResMut<StableLane>,
    mut sink: MessageSink,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    while let Some(at) = lane.head().filter(|&s| until.admits(s)) {
        let Some((_, event)) = lane.events.pop_front() else {
            return;
        };
        // A list's copy, read before its signal.
        let listed = (event == StableEvent::Show).then(|| vm.own().listed());
        let ((), moved) = crate::lanes::firing(at, || {
            vm.run(|script| {
                if let Some(script) = script {
                    fire(script, &mut sink, event, listed);
                }
            })
        });
        if moved {
            return;
        }
    }
}

/// One item's signal: none of the events carries an argument (`0x703e50`).
fn fire(
    script: &mut UiScript,
    sink: &mut MessageSink,
    event: StableEvent,
    listed: Option<(u64, benilla_ui::script::StableView)>,
) {
    match event {
        StableEvent::Show => {
            let (list, view) = listed.unwrap_or_default();
            script.stable_listed(list, view);
            script.fire_event("PET_STABLE_SHOW", vec![]);
        }
        StableEvent::Update => script.fire_event("PET_STABLE_UPDATE", vec![]),
        StableEvent::Paperdoll => script.fire_event("PET_STABLE_UPDATE_PAPERDOLL", vec![]),
        // Shown, and spoken, as its message row says, inside the handler that adds it.
        StableEvent::Line(key) => {
            if let Some(line) = crate::ui_action::keyed_line(script, key) {
                crate::ui_action::show_now(script, sink, "ui_stable", line);
            }
        }
        StableEvent::Parked { session, key } => {
            if script.session() == session {
                crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Stable, || {
                    script.run_parked(key)
                });
            }
        }
    }
}

/// Run the close `core` behind the lane ([`crate::lanes::behind`]), so a close never fires ahead
/// of the open it closes.
pub(super) fn behind_lane(world: &mut World, core: impl FnOnce(&mut World)) {
    let head = world
        .get_resource::<StableLane>()
        .and_then(StableLane::head);
    crate::lanes::behind(world, head, core, |world, session, key| {
        world
            .resource_mut::<StableLane>()
            .push(StableEvent::Parked { session, key });
    });
}
