//! The cast's two reporters, which every refusal and every cancel goes through: the failure
//! reporter `ReportCastFailure 0x6e1a00` and the cancel `CancelCast 0x6e4940`, and the in-flight
//! slot they read, pushed and popped by `SetInFlightSpell 0x6e4ad0`. Both run inside the call that
//! reaches them, so what they signal is queued in order on [`CastReports`] and signalled inside
//! that call ([`crate::script_calls`]), or by the frame's feed for a packet's.

use std::time::Instant;

use bevy::prelude::*;

use super::inflight::{inflight, Inflight};
use super::{PendingCast, QueuedMeleeSpell, SpellTargeting};
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::CastFail;
use crate::ui_cast::{CastBarEdge, CastReports};

/// The reason `TryCast`'s supersede, `StopSpellTargeting`, `SpellStopCasting` and
/// `CancelQueuedCast` pass the cancel: `SPELL_FAILED_ERROR` (`0x6e4d6d`, `0x6e490b`, `0x6e6ec3`,
/// `0x6e6f62`).
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

/// What the attempt pushes into the in-flight slot (`0x6e5026`), by where benilla holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Held {
    /// An on-next-swing strike, [`QueuedMeleeSpell`]'s.
    Strike,
    /// A cast, [`PendingCast`]'s, with its item's template flags and object guid (`None` for no
    /// item).
    Cast {
        item_flags: Option<u32>,
        item: Option<u64>,
    },
    /// The spell whose cursor the arm raises next ([`SpellTargeting::enter_as`]).
    Cursor,
}

/// The in-flight slot, the record's caster word and the targeting word as the two reporters reach
/// them, borrowed from whichever applier holds them.
pub(crate) struct CastCore<'a> {
    pub(crate) pending: &'a mut PendingCast,
    pub(crate) queued_melee: &'a mut QueuedMeleeSpell,
    pub(crate) ground: &'a mut SpellTargeting,
    pub(crate) reports: &'a mut CastReports,
    pub(crate) net: &'a NetCommands,
}

impl CastCore<'_> {
    /// What the in-flight slot `0xceca88` holds.
    pub(crate) fn slot(&self) -> Option<Inflight> {
        inflight(self.pending, self.queued_melee, self.ground)
    }

    /// `SetInFlightSpell(spell)` as the attempt calls it (`0x6e5026`): the same spell leaves the
    /// slot as it is, unsignalled (`0x6e4ad7`); another saves what the slot held (`0x6e4adf`, of
    /// which only a strike is kept), takes its place and signals (`0x6e4b13` → `0x4b3250`).
    pub(crate) fn push(&mut self, spell_id: u32, held: Held, now: Instant) {
        let slot = self.slot();
        if slot.map(Inflight::spell_id) == Some(spell_id) {
            return;
        }
        let strike = match slot {
            Some(Inflight::Strike(id)) => Some(id),
            _ => None,
        };
        match held {
            Held::Strike => self.queued_melee.push(strike, Some(spell_id)),
            Held::Cast { item_flags, item } => {
                self.queued_melee.push(strike, None);
                self.pending.push_cast(spell_id, item_flags, item, now);
            }
            Held::Cursor => self.queued_melee.push(strike, None),
        }
        self.reports.changed();
    }

    /// `SetInFlightSpell(0)` (`0x6e4b2c`): the save copied back into the slot, signalled; an
    /// empty slot is left alone (`0x6e4ad7`).
    fn pop(&mut self, slot: Option<Inflight>) {
        let Some(top) = slot else {
            return;
        };
        match top {
            Inflight::Cast(spell_id) => self.pending.clear_if(spell_id),
            Inflight::Targeting(_) => self.ground.leave_slot(),
            Inflight::Strike(_) => {}
        }
        self.queued_melee.restore();
        self.reports.changed();
    }

    /// A cast whose deadline passed with no result ([`PendingCast::lapsed`]) leaves the slot as
    /// its result's pop takes it (`0x6e4b2c`): the save copied back, a strike under it included,
    /// and signalled. The deadline is benilla's own net for a lost result, which the reference
    /// waits on.
    pub(crate) fn reap_lapsed(&mut self, now: Instant) {
        let Some(spell_id) = self.pending.lapsed(now) else {
            return;
        };
        let slot = self.slot();
        if slot == Some(Inflight::Cast(spell_id)) {
            self.pop(slot);
        } else {
            self.pending.clear_if(spell_id);
        }
    }

    /// The send's pop of an auto-repeat spell once its packet is out (`0x6e597d`-`0x6e5989`): only
    /// while the slot holds it, so a ranged shot that is not auto-repeat (Aimed Shot, Arcane Shot,
    /// Throw) stays in flight until its result. The send's other pop (`0x6e58a2`-`0x6e58bd`) is for
    /// a record the push did not make or another caster's (the pet bar), neither in this slot.
    pub(crate) fn pop_if(&mut self, spell_id: u32) {
        let slot = self.slot();
        if slot.map(Inflight::spell_id) == Some(spell_id) {
            self.pop(slot);
        }
    }

    /// `CancelCast 0x6e4940(failed, send, reason)`. A targeting word up is cleared with no packet
    /// (`0x6e4955`) and signalled (`0x6e495c`); else a spell in flight is cancelled on the wire
    /// when `send` is set (`0x6e4970`). Then the pop (`0x6e49d6`). While the record's caster is
    /// the player (`0x6e4a04`-`0x6e4a1e`) it fires `SPELLCAST_STOP` for `failed` 0, else the
    /// failure's edge, and clears the caster (`0x6e4a5a`); with `failed` set it ends a tradeskill
    /// repeat last (`0x6e4ac2`). Most callers gate it on the slot or the word; the soulstone's
    /// (`0x48ae6b`), `0x495920`'s (`0x495932`) and the death handler's (`0x5e9a6c`) do not, and
    /// `StartMeleeAttack`'s reads the caster's precast spell (`0x5ecc56`-`0x5ecc85`). What it
    /// popped is returned for the caller's local teardown.
    pub(crate) fn cancel(&mut self, failed: bool, send: bool, reason: u8) -> Option<Inflight> {
        let slot = self.slot();
        if self.ground.active() {
            self.ground.clear();
            self.reports.changed();
        } else if let (Some(top), true) = (slot, send) {
            let _ = self.net.0.send(ClientCommand::CancelCast {
                spell_id: top.spell_id(),
            });
        }
        self.pop(slot);
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

    /// The merge's range refusal re-arms the cursor as its attempt left it
    /// ([`super::targeting`]'s `merge`, a deviation): the word, the caster, and its spell over
    /// whatever the cancel's pop restored when the attempt had pushed it, unsignalled.
    pub(crate) fn rearm_cursor(
        &mut self,
        spell_id: u32,
        commit: super::CastCommit,
        word: u16,
        in_slot: bool,
    ) {
        if in_slot {
            let strike = match self.slot() {
                Some(Inflight::Strike(id)) => Some(id),
                _ => None,
            };
            self.queued_melee.push(strike, None);
        }
        self.ground.enter_as(spell_id, commit, word, in_slot);
        self.pending.fill_record();
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

    /// `CancelQueuedCast 0x6e6f30`, StopAttack's tail: a strike in the slot is cancelled as
    /// `SpellStopCasting` cancels (`0x6e6f68`, `0x6e4940(0, 1, 0x1c)`); then a strike in the save
    /// goes on the wire by itself and leaves it, unsignalled (`0x6e6f9d`-`0x6e701a`: a strike's
    /// save holds no item guid, so it sends).
    pub(crate) fn cancel_queued(&mut self) {
        if let Some(Inflight::Strike(_)) = self.slot() {
            self.cancel(false, true, REASON_ERROR);
        }
        if let Some(spell_id) = self.queued_melee.take_save() {
            let _ = self.net.0.send(ClientCommand::CancelCast { spell_id });
        }
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

/// The reporters' state as one param, for an applier that holds no [`super::CastLadder`]: the
/// right-button cancel and the attack seams' StopAttack ([`crate::creature_anim::AttackSeam`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CastSlots<'w> {
    pending: ResMut<'w, PendingCast>,
    queued_melee: ResMut<'w, QueuedMeleeSpell>,
    ground: ResMut<'w, SpellTargeting>,
    reports: ResMut<'w, CastReports>,
}

impl CastSlots<'_> {
    /// The reporters over these slots, sending on `net`.
    pub(crate) fn core<'a>(&'a mut self, net: &'a NetCommands) -> CastCore<'a> {
        CastCore {
            pending: &mut self.pending,
            queued_melee: &mut self.queued_melee,
            ground: &mut self.ground,
            reports: &mut self.reports,
            net,
        }
    }

    /// The targeting word, which `SelectTarget` tests first (`0x4935ca`).
    pub(crate) fn targeting(&self) -> &SpellTargeting {
        &self.ground
    }
}

/// `StopTargeting 0x6e4900`'s state for the right-button cancel.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CursorCancel<'w> {
    slots: CastSlots<'w>,
    net: Res<'w, NetCommands>,
}

impl CursorCancel<'_> {
    /// Whether the targeting word is up.
    pub(crate) fn active(&self) -> bool {
        self.slots.targeting().active()
    }

    /// [`CastCore::stop_targeting`].
    pub(crate) fn stop_targeting(&mut self) -> bool {
        self.slots.core(&self.net).stop_targeting()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::CastCommit;
    use crate::ui_cast::CastReport;

    const HEROIC_STRIKE: u32 = 78;
    const FIREBALL: u32 = 133;
    const AUTO_SHOT: u32 = 75;
    /// A grenade's spell, its item without `ITEM_FLAG_PLAYERCAST`.
    const GRENADE: u32 = 4064;
    const GRENADE_COMMIT: CastCommit = CastCommit::Item {
        bag_index: 255,
        slot: 23,
        entry: 4360,
        guid: None,
        spell_index: 0,
        on_object: None,
    };

    /// The slots, the reports and the wire, lent to a [`CastCore`] per step.
    #[derive(Default)]
    struct Slots {
        pending: PendingCast,
        queued_melee: QueuedMeleeSpell,
        ground: SpellTargeting,
        reports: CastReports,
    }

    impl Slots {
        fn core<'a>(&'a mut self, net: &'a NetCommands) -> CastCore<'a> {
            CastCore {
                pending: &mut self.pending,
                queued_melee: &mut self.queued_melee,
                ground: &mut self.ground,
                reports: &mut self.reports,
                net,
            }
        }

        /// The reports queued since the last call: a change, an edge by its event, a refusal
        /// by its reason.
        fn stream(&mut self) -> Vec<String> {
            self.reports
                .0
                .drain(..)
                .map(|r| match r {
                    CastReport::Changed => "CHANGED".into(),
                    CastReport::Bar(CastBarEdge::Stop) => "STOP".into(),
                    CastReport::Bar(CastBarEdge::Failed) => "FAILED".into(),
                    CastReport::Bar(CastBarEdge::Interrupted) => "INTERRUPTED".into(),
                    CastReport::Bar(_) => "BAR".into(),
                    CastReport::Refused(f) => format!("REFUSED {:#04x}", f.reason),
                    CastReport::TradeSkillDisarm => "DISARM".into(),
                })
                .collect()
        }
    }

    fn wire() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    fn cancels(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<u32> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CancelCast { spell_id } => Some(spell_id),
                _ => None,
            })
            .collect()
    }

    /// `SetInFlightSpell 0x6e4ad0`: the push saves the strike the slot held (`0x6e4adf`) and the
    /// pop copies it back without clearing the save (`0x6e4b2c`), each signalling; the same spell
    /// pushes nothing (`0x6e4ad7`).
    #[test]
    fn a_push_saves_the_strike_and_its_pop_copies_it_back_keeping_the_save() {
        let (net, _rx) = wire();
        let mut s = Slots::default();
        s.queued_melee.arm(HEROIC_STRIKE);
        let now = Instant::now();
        s.core(&net).push(
            FIREBALL,
            Held::Cast {
                item_flags: None,
                item: None,
            },
            now,
        );
        s.core(&net).push(
            FIREBALL,
            Held::Cast {
                item_flags: None,
                item: None,
            },
            now,
        );
        assert_eq!(s.core(&net).slot(), Some(Inflight::Cast(FIREBALL)));
        assert_eq!(s.queued_melee.saved(), Some(HEROIC_STRIKE));
        assert_eq!(s.stream(), ["CHANGED"], "one push, one signal");

        s.core(&net).cancel(false, false, REASON_ERROR);
        assert_eq!(s.core(&net).slot(), Some(Inflight::Strike(HEROIC_STRIKE)));
        assert_eq!(
            s.queued_melee.saved(),
            Some(HEROIC_STRIKE),
            "the pop keeps the save"
        );
        assert_eq!(s.stream(), ["CHANGED"]);
    }

    /// `CancelQueuedCast 0x6e6f30`'s first leg: a strike in the slot goes as `SpellStopCasting`'s
    /// cancel (`0x6e6f68`): its packet, its pop's signal, and `SPELLCAST_STOP` while the record's
    /// caster is the player.
    #[test]
    fn stop_attack_cancels_a_strike_in_the_slot_with_its_pair_then_stop() {
        let (net, rx) = wire();
        let mut s = Slots::default();
        s.queued_melee.arm(HEROIC_STRIKE);
        s.pending.fill_record();
        s.core(&net).cancel_queued();
        assert_eq!(cancels(&rx), [HEROIC_STRIKE]);
        assert_eq!(s.stream(), ["CHANGED", "STOP"]);
        assert_eq!(s.core(&net).slot(), None);
    }

    /// Its second leg (`0x6e6f6d`-`0x6e701a`): a strike in the save under the auto-repeat spell
    /// the slot holds goes on the wire by itself and leaves the save, unsignalled.
    #[test]
    fn stop_attack_sends_a_saved_strike_by_itself() {
        let (net, rx) = wire();
        let mut s = Slots::default();
        s.queued_melee.arm(HEROIC_STRIKE);
        s.core(&net).push(
            AUTO_SHOT,
            Held::Cast {
                item_flags: None,
                item: None,
            },
            Instant::now(),
        );
        s.stream();
        s.core(&net).cancel_queued();
        assert_eq!(cancels(&rx), [HEROIC_STRIKE]);
        assert_eq!(s.stream(), Vec::<String>::new());
        assert_eq!(s.queued_melee.saved(), None);
        assert_eq!(s.core(&net).slot(), Some(Inflight::Cast(AUTO_SHOT)));
    }

    /// A strike a cast popped back over is in the slot and the save: the first leg's pop copies it
    /// back, the second leg sends it again and empties the save, and the slot holds it until the
    /// server's echo.
    #[test]
    fn stop_attack_sends_a_strike_in_both_places_twice() {
        let (net, rx) = wire();
        let mut s = Slots::default();
        s.queued_melee.arm(HEROIC_STRIKE);
        s.core(&net).push(
            FIREBALL,
            Held::Cast {
                item_flags: None,
                item: None,
            },
            Instant::now(),
        );
        s.core(&net).cancel(false, false, REASON_ERROR);
        s.stream();
        s.core(&net).cancel_queued();
        assert_eq!(cancels(&rx), [HEROIC_STRIKE, HEROIC_STRIKE]);
        assert_eq!(s.stream(), ["CHANGED"]);
        assert_eq!(s.core(&net).slot(), Some(Inflight::Strike(HEROIC_STRIKE)));
        assert_eq!(s.queued_melee.saved(), None);
    }

    /// A cursor whose item lacks `ITEM_FLAG_PLAYERCAST` holds no slot (`0xceca88` stays as the
    /// attempt found it): its cancel signals the word's clear alone (`0x6e495c`), the pop finding
    /// nothing; one that holds the slot signals its pop too.
    #[test]
    fn a_cursor_that_holds_no_slot_cancels_with_one_pair() {
        let (net, rx) = wire();
        for (in_slot, pairs) in [(false, 1), (true, 2)] {
            let mut s = Slots::default();
            s.ground.enter_as(GRENADE, GRENADE_COMMIT, 0x40, in_slot);
            assert_eq!(
                s.core(&net).slot(),
                in_slot.then_some(Inflight::Targeting(GRENADE))
            );
            assert!(s.core(&net).stop_targeting());
            let stream = s.stream();
            assert_eq!(
                stream.iter().filter(|r| *r == "CHANGED").count(),
                pairs,
                "in_slot {in_slot}: {stream:?}"
            );
            assert!(!s.ground.active());
        }
        assert!(cancels(&rx).is_empty(), "a cursor's cancel sends nothing");
    }

    /// The pop runs whatever the cancel's branch (`0x6e49d6`): a strike the slot holds under a
    /// cursor that holds none is popped with the word, and nothing goes on the wire.
    #[test]
    fn a_cursor_cancel_pops_the_strike_under_a_cursor_that_holds_no_slot() {
        let (net, rx) = wire();
        let mut s = Slots::default();
        s.queued_melee.arm(HEROIC_STRIKE);
        s.ground.enter_as(GRENADE, GRENADE_COMMIT, 0x40, false);
        s.core(&net).stop_targeting();
        assert_eq!(s.core(&net).slot(), None);
        assert_eq!(
            s.stream(),
            ["CHANGED", "CHANGED", "DISARM"],
            "the word's clear, then the strike's pop"
        );
        assert!(cancels(&rx).is_empty());
    }

    /// The report's tail reads the slot, not the word: a refusal of a cursor's spell that holds
    /// no slot finds nothing in flight and fires `SPELLCAST_FAILED` (`0x6e224a`), and the cursor
    /// stays up.
    #[test]
    fn a_refusal_of_a_cursor_that_holds_no_slot_fails_and_keeps_the_cursor() {
        let (net, rx) = wire();
        let mut s = Slots::default();
        s.ground.enter_as(GRENADE, GRENADE_COMMIT, 0x02, false);
        s.core(&net)
            .report(CastFail::local(GRENADE, 0x59), false, true);
        assert_eq!(s.stream(), ["REFUSED 0x59", "FAILED"]);
        assert!(s.ground.active());
        assert!(cancels(&rx).is_empty());
    }
}
