//! The charter lane: what the registrar's and the petition window's packet handlers and record
//! callbacks signal, each fired at its packet's place among the other lanes' items
//! ([`crate::lanes`]). In the reference each signals inside itself: the showlist's opener its
//! `GUILD_REGISTRAR_SHOW` (`0x4f5003`), the turn-in's success `GUILD_REGISTRAR_CLOSED`
//! (`0x5ef16b`), the replace `PETITION_CLOSED` (`0x4f402c`), our own signature's success its line
//! and `PETITION_CLOSED` (`0x5ef02a`, `0x5ef037`), each `PETITION_SHOW` its deferral's end
//! (`0x4f41b4`, `0x4f430c`, `0x4f433f`, `0x4f43ad`, `0x5ef29c`), and the results their lines through
//! `DisplayError` (`0x496720`). A handler queues its item here, stamped where it runs, and the
//! getters its listeners call read the charter through the host ([`super::live`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::lines::Line;
use crate::lanes::{LaneApp, Stamp, Until};
use crate::ui_action::MessageSink;

/// What an item signals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PetitionEvent {
    /// `GUILD_REGISTRAR_SHOW` (378).
    RegistrarShow,
    /// `GUILD_REGISTRAR_CLOSED` (379), bare: the turn-in's success, which leaves the cell.
    RegistrarClosed,
    /// `PETITION_SHOW` (390).
    Show,
    /// `PETITION_CLOSED` (391).
    Closed,
    /// A system line, by its catalog key and arguments.
    Line(Line),
    /// What a close core run outside a call signalled while this lane held items, parked in the
    /// VM of the session under the key ([`crate::script_calls::park_since`]).
    Parked { session: u64, key: u64 },
}

/// The charter lane's queue.
#[derive(Resource, Default)]
pub(crate) struct PetitionLane {
    events: VecDeque<(Stamp, PetitionEvent)>,
}

impl PetitionLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: PetitionEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, PetitionEvent)> {
        self.events.iter().cloned().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_petition_lane(app: &mut App) {
    app.init_resource::<PetitionLane>()
        .add_lane(
            "charter",
            |world| world.get_resource::<PetitionLane>()?.head(),
            fire_petition,
            None,
        )
        .add_systems(
            Update,
            drop_petition_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_petition_events(mut lane: ResMut<PetitionLane>) {
    lane.events.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_petition(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut lane: ResMut<PetitionLane>,
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

/// One item's signal: none of the events carries an argument (`0x703e50`).
fn fire(script: &mut UiScript, sink: &mut MessageSink, event: PetitionEvent) {
    match event {
        PetitionEvent::RegistrarShow => script.fire_event("GUILD_REGISTRAR_SHOW", vec![]),
        PetitionEvent::RegistrarClosed => script.fire_event("GUILD_REGISTRAR_CLOSED", vec![]),
        PetitionEvent::Show => script.fire_event("PETITION_SHOW", vec![]),
        PetitionEvent::Closed => script.fire_event("PETITION_CLOSED", vec![]),
        // Shown, and spoken, as its message row says, inside the handler that adds it; a key the
        // VM's `GlobalStrings.lua` lacks shows nothing.
        PetitionEvent::Line(line) => {
            if let Some(shown) = super::lines::shown(script, &line) {
                crate::ui_action::show_now(script, sink, "ui_petition", shown);
            }
        }
        PetitionEvent::Parked { session, key } => {
            if script.session() == session {
                crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Registrar, || {
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
        .get_resource::<PetitionLane>()
        .and_then(PetitionLane::head);
    crate::lanes::behind(world, head, core, |world, session, key| {
        world
            .resource_mut::<PetitionLane>()
            .push(PetitionEvent::Parked { session, key });
    });
}
