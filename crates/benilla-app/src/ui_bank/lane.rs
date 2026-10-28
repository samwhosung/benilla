//! The bank lane: what the bank's packet handlers signal, each fired at its packet's place among
//! the other lanes' items ([`crate::lanes`]). In the reference each signals inside itself:
//! `SMSG_SHOW_BANK`'s open writes the banker's cell and signals `BANKFRAME_OPENED` (`0x5e3f78` →
//! `0x4f850e`, `0x4f8522`), and `SMSG_BUY_BANK_SLOT_RESULT`'s refusal shows its line through
//! `DisplayError` (`0x5e3f8d`). A bought slot's `PLAYERBANKBAGSLOTS_CHANGED` is the field watch's,
//! on the notice lane ([`crate::field_watch`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::lanes::{LaneApp, Stamp, Until};
use crate::ui_action::MessageSink;

/// What an item signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BankEvent {
    /// `BANKFRAME_OPENED` (`0x4f8522`), behind the open's cell write (`0x4f850e`).
    Opened,
    /// A refusal's message line, by its `GlobalStrings` key.
    Line(&'static str),
    /// What a close core run outside a call signalled while this lane held items, parked in the
    /// VM of the session under the key ([`crate::script_calls::park_since`]).
    Parked { session: u64, key: u64 },
}

/// The bank lane's queue.
#[derive(Resource, Default)]
pub(crate) struct BankLane {
    events: VecDeque<(Stamp, BankEvent)>,
}

impl BankLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: BankEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, BankEvent)> {
        self.events.iter().copied().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_bank_lane(app: &mut App) {
    app.init_resource::<BankLane>()
        .add_lane(
            "bank",
            |world| world.get_resource::<BankLane>()?.head(),
            fire_bank,
            None,
        )
        .add_systems(
            Update,
            drop_bank_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_bank_events(mut lane: ResMut<BankLane>) {
    lane.events.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_bank(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut lane: ResMut<BankLane>,
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

/// One item's signal: `BANKFRAME_OPENED` carries no argument (`0x4f8522`), and stock
/// `BankFrame.lua:127` titles with `UnitName("npc")`.
fn fire(script: &mut UiScript, sink: &mut MessageSink, event: BankEvent) {
    match event {
        BankEvent::Opened => script.fire_event("BANKFRAME_OPENED", vec![]),
        // Shown, and spoken, as its message row says: insufficient funds is speech `0x16`.
        BankEvent::Line(key) => {
            if let Some(line) = crate::ui_action::keyed_line(script, key) {
                crate::ui_action::show_now(script, sink, "ui_bank", line);
            }
        }
        BankEvent::Parked { session, key } => {
            if script.session() == session {
                crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Bank, || {
                    script.run_parked(key)
                });
            }
        }
    }
}

/// Run the close `core` behind the lane ([`crate::lanes::behind`]), so a close never fires ahead
/// of the open it closes.
pub(super) fn behind_lane(world: &mut World, core: impl FnOnce(&mut World)) {
    let head = world.get_resource::<BankLane>().and_then(BankLane::head);
    crate::lanes::behind(world, head, core, |world, session, key| {
        world
            .resource_mut::<BankLane>()
            .push(BankEvent::Parked { session, key });
    });
}
