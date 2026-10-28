//! The merchant lane: what the vendor's packet handlers and record callbacks signal, each fired at
//! its packet's place among the other lanes' items ([`crate::lanes`]). In the reference each
//! signals inside itself: the list's open its `MERCHANT_SHOW` (`0x4fad92`), the stock update its
//! `MERCHANT_UPDATE` (`0x4fae70`, from `SMSG_BUY_ITEM` at `0x5dd042` and `SMSG_BUY_FAILED` at
//! `0x5dcdc5`), the item records' callbacks theirs (`0x4fb0f0`, `0x4fb570`), and the refusals
//! their lines through `DisplayError` (`0x496720`, `0x5dcdd8`, `0x5dd22c`). A handler queues its
//! item here, stamped where it runs, and the getters its listeners call read the window through
//! the host ([`super::live`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::lanes::{LaneApp, Stamp, Until};
use crate::ui_action::MessageSink;

/// What an item signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MerchantEvent {
    /// `MERCHANT_SHOW` (`0x4fad92`).
    Show,
    /// `MERCHANT_UPDATE` (317).
    Update,
    /// A refusal's message line, by its `GlobalStrings` key.
    Line(&'static str),
    /// What a close core run outside a call signalled while this lane held items, parked in the
    /// VM of the session under the key ([`crate::script_calls::park_since`]).
    Parked { session: u64, key: u64 },
}

/// The merchant lane's queue.
#[derive(Resource, Default)]
pub(crate) struct MerchantLane {
    events: VecDeque<(Stamp, MerchantEvent)>,
}

impl MerchantLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: MerchantEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// What the queued items signal, in order, taking them.
    #[cfg(test)]
    pub(crate) fn take(&mut self) -> Vec<MerchantEvent> {
        self.events.drain(..).map(|(_, e)| e).collect()
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, MerchantEvent)> {
        self.events.iter().copied().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_merchant_lane(app: &mut App) {
    app.init_resource::<MerchantLane>()
        .add_lane(
            "merchant",
            |world| world.get_resource::<MerchantLane>()?.head(),
            fire_merchant,
            None,
        )
        .add_systems(
            Update,
            drop_merchant_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_merchant_events(mut lane: ResMut<MerchantLane>) {
    lane.events.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_merchant(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut lane: ResMut<MerchantLane>,
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
fn fire(script: &mut UiScript, sink: &mut MessageSink, event: MerchantEvent) {
    match event {
        MerchantEvent::Show => script.fire_event("MERCHANT_SHOW", vec![]),
        MerchantEvent::Update => script.fire_event("MERCHANT_UPDATE", vec![]),
        // Shown, and spoken, as its message row says, inside the handler that adds it.
        MerchantEvent::Line(key) => {
            if let Some(line) = crate::ui_action::keyed_line(script, key) {
                crate::ui_action::show_now(script, sink, "ui_merchant", line);
            }
        }
        MerchantEvent::Parked { session, key } => {
            if script.session() == session {
                crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Merchant, || {
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
        .get_resource::<MerchantLane>()
        .and_then(MerchantLane::head);
    crate::lanes::behind(world, head, core, |world, session, key| {
        world
            .resource_mut::<MerchantLane>()
            .push(MerchantEvent::Parked { session, key });
    });
}

/// The item records' callbacks a vendor row's or a buy-back row's answer runs, a record or a
/// refusal alike: `0x4fb0f0` counts the window's waits down and signals with a vendor open at
/// the last (a count already 0 signals too: `0x4fb0f7`, `0x4fb101`), and `0x4fb570` signals.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MerchantRecordAnswers<'w> {
    /// Each absent in a harness without the merchant window.
    open: Option<ResMut<'w, super::MerchantOpen>>,
    lane: Option<ResMut<'w, MerchantLane>>,
}

impl MerchantRecordAnswers<'_> {
    /// Our create's merchant reset, owed from its packet
    /// ([`super::MerchantOpen::settle_world_reset`]).
    pub(crate) fn owe_world_reset(&mut self) {
        if let Some(open) = self.open.as_mut() {
            open.world_reset.owe();
        }
    }

    /// A vendor row's callback (`0x4fb0f0`).
    pub(crate) fn row_answered(&mut self) {
        let (Some(open), Some(lane)) = (self.open.as_deref_mut(), self.lane.as_deref_mut()) else {
            return;
        };
        open.pending = open.pending.saturating_sub(1);
        if open.pending == 0 && open.vendor.is_some() {
            lane.push(MerchantEvent::Update);
        }
    }

    /// A buy-back row's callback (`0x4fb570`).
    pub(crate) fn buyback_answered(&mut self) {
        if let Some(lane) = self.lane.as_deref_mut() {
            lane.push(MerchantEvent::Update);
        }
    }
}
