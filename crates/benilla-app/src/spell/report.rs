//! The cast's two reporters, which every refusal and every cancel goes through: the failure
//! reporter `ReportCastFailure 0x6e1a00` and the cancel `CancelCast 0x6e4940`. Both run inside the
//! call that reaches them, so what they signal is queued in order on [`CastReports`] and signalled
//! inside that call ([`crate::script_calls`]), or by the frame's feed for a packet's.

use std::time::Instant;

use bevy::prelude::*;

use super::inflight::{inflight, Inflight};
use super::{PendingCast, QueuedMeleeSpell, SpellTargeting};
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::CastFail;
use crate::ui_cast::{CastBarEdge, CastReports};

/// The reason `TryCast`'s supersede, `StopSpellTargeting` and `SpellStopCasting` pass the cancel:
/// `SPELL_FAILED_ERROR` (`0x6e4d6d`, `0x6e490b`, `0x6e6ec3`).
pub(crate) const REASON_ERROR: u8 = 0x1c;

/// The failure the reporter fires no event for: `SPELL_FAILED_SPELL_IN_PROGRESS` (`0x6e2234`).
const REASON_IN_PROGRESS: u8 = 0x61;

/// The edge a failure signals: `SPELLCAST_INTERRUPTED` (340) for `SPELL_FAILED_INTERRUPTED` and
/// `_INTERRUPTED_COMBAT`, `SPELLCAST_FAILED` (339) for every other (`0x6e2238`, `0x6e4a24`).
fn failure_edge(reason: u8) -> CastBarEdge {
    match reason {
        0x23 | 0x24 => CastBarEdge::Interrupted,
        _ => CastBarEdge::Failed,
    }
}

/// The in-flight slot, the record's caster word and the targeting word as the two reporters reach
/// them, borrowed from whichever applier holds them.
pub(crate) struct CastCore<'a> {
    pub(crate) pending: &'a mut PendingCast,
    pub(crate) queued_melee: &'a mut QueuedMeleeSpell,
    pub(crate) ground: &'a mut SpellTargeting,
    pub(crate) reports: &'a mut CastReports,
    pub(crate) net: &'a NetCommands,
    /// The self `Casting`'s spell, a started cast that never armed the send guard.
    pub(crate) started: Option<u32>,
}

impl CastCore<'_> {
    /// What the in-flight slot `0xceca88` holds.
    pub(crate) fn slot(&self) -> Option<Inflight> {
        inflight(
            self.pending,
            self.queued_melee,
            self.ground,
            self.started,
            Instant::now(),
        )
    }

    /// `CancelCast 0x6e4940(failed, send, reason)`. A targeting word up is cleared with no packet
    /// (`0x6e4955`) and signalled (`0x6e495c`); else a spell in flight is cancelled on the wire
    /// when `send` is set (`0x6e4970`). The pop restores what the push saved (`0x6e49d6`), a
    /// change of the slot that signals (`SetInFlightSpell 0x6e4ad0`). While the record's caster is
    /// the player (`0x6e4a04`-`0x6e4a1e`) it fires `SPELLCAST_STOP` for `failed` 0, else the
    /// failure's edge, and clears the caster (`0x6e4a5a`); with `failed` set it ends a tradeskill
    /// repeat last (`0x6e4ac2`). Every caller gates it on a slot or a word but the soulstone's.
    /// What it popped is returned for the caller's local teardown.
    pub(crate) fn cancel(&mut self, failed: bool, send: bool, reason: u8) -> Option<Inflight> {
        let slot = self.slot();
        match slot {
            Some(Inflight::Targeting(_)) => {
                self.ground.clear();
                self.reports.changed();
            }
            Some(Inflight::Cast(spell_id) | Inflight::Strike(spell_id)) if send => {
                let _ = self.net.0.send(ClientCommand::CancelCast { spell_id });
            }
            _ => {}
        }
        match slot {
            Some(Inflight::Cast(spell_id)) => self.pending.clear_if(spell_id),
            Some(Inflight::Strike(spell_id)) => self.queued_melee.clear_if(spell_id),
            Some(Inflight::Targeting(_)) | None => {}
        }
        // The cursor's spell sits in the slot too, pushed by its attempt (`0x6e5026`).
        if slot.is_some() {
            self.reports.changed();
        }
        if self.pending.take_caster_event() {
            self.reports.bar(if failed {
                failure_edge(reason)
            } else {
                CastBarEdge::Stop
            });
        }
        if failed {
            self.reports.trade_skill_disarm();
        }
        slot
    }

    /// `StopSpellTargeting 0x6e4900`: while the word is up, the cancel with its failed flag and
    /// `0x1c`. Whether the cursor was up.
    pub(crate) fn stop_targeting(&mut self) -> bool {
        if !self.ground.active() {
            return false;
        }
        self.cancel(true, true, REASON_ERROR);
        true
    }

    /// `ReportCastFailure 0x6e1a00`: the line, then the tail (`0x6e2201`-`0x6e224a`), which
    /// `reaches_tail` says the reporter gets to ([`crate::ui_action::report_reaches_pop`]): the
    /// spell in flight is cancelled with its failed flag, sending only for the client's own
    /// refusal (`0x6e2213 sete`); another spell in flight fires nothing (`0x6e2228`), nor does
    /// "Another action is in progress" (`0x6e2234`); anything else fires the failure's edge.
    pub(crate) fn report(&mut self, fail: CastFail, from_server: bool, reaches_tail: bool) {
        let (spell_id, reason) = (fail.spell_id, fail.reason);
        self.reports.refused(fail);
        if !reaches_tail {
            return;
        }
        match self.slot() {
            Some(slot) if slot.spell_id() == spell_id => {
                self.cancel(true, !from_server, reason);
            }
            Some(_) => {}
            None if reason == REASON_IN_PROGRESS => {}
            None => self.reports.bar(failure_edge(reason)),
        }
    }
}

/// `StopTargeting 0x6e4900`'s state for a system that holds no applier of it: the right-button
/// cancel.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CursorCancel<'w> {
    pending: ResMut<'w, PendingCast>,
    queued_melee: ResMut<'w, QueuedMeleeSpell>,
    ground: ResMut<'w, SpellTargeting>,
    reports: ResMut<'w, CastReports>,
    net: Res<'w, NetCommands>,
}

impl CursorCancel<'_> {
    /// Whether the targeting word is up.
    pub(crate) fn active(&self) -> bool {
        self.ground.active()
    }

    /// [`CastCore::stop_targeting`]. While the word is up the in-flight slot holds its spell (the
    /// join ranks it first), so a started cast is never read.
    pub(crate) fn stop_targeting(&mut self) -> bool {
        CastCore {
            pending: &mut self.pending,
            queued_melee: &mut self.queued_melee,
            ground: &mut self.ground,
            reports: &mut self.reports,
            net: &self.net,
            started: None,
        }
        .stop_targeting()
    }
}
