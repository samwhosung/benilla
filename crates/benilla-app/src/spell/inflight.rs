//! The player's own cast state: the pending cast (`SPELLCAST` at `0xceac48` and the inflight id
//! `0xceca88`), the queued on-next-swing strike, the running channel, the auto-repeat key
//! (`0xceac30`), and the local abort on a move, jump or Esc (`AbortCast 0x6e4940`).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use super::SpellTargeting;
use crate::creature_anim::{CastEvent, CastEventKind, Casting, PlaySeq};
use crate::net::{ClientCommand, GuidIndex, NetCommands, SelfGuid};
use crate::ui_action::Spells;
use crate::ui_cast::CastReports;
use benilla_protocol::messages::ITEM_FLAG_PLAYERCAST;
use benilla_ui::script::CastView;

/// The in-flight window from send until `SMSG_SPELL_START` names the cast time; the server
/// answers every `CMSG_CAST_SPELL`, so a packet always clears it first.
const SEND_PROVISIONAL: Duration = Duration::from_secs(5);

/// The same for an item use, kept short: vmangos refuses some `CMSG_USE_ITEM`s with only
/// `SMSG_INVENTORY_CHANGE_FAILURE` and no cast result (`HandleUseItemOpcode`), so this deadline
/// is what reopens casting.
const ITEM_SEND_PROVISIONAL: Duration = Duration::from_millis(1_500);

/// Pushback slack added to the server's cast time at `SMSG_SPELL_START`, so a cast the server
/// delays (`SMSG_SPELL_DELAYED`) still guards past its stretched end.
const CAST_SLACK: Duration = Duration::from_secs(2);

/// Our outstanding cast: the reference's in-flight slot `0xceca88` for everything that is sent.
///
/// The attempt pushes the cast at `0x6e5026` (`SetInFlightSpell 0x6e4ad0`) whenever the caster's
/// cast time is not negative (`0x6e4ff5`; `GetSpellCastTime 0x5ee150` returns 0 or more for the
/// active player, so every cast, an instant too) and there is no item or the item's flags carry
/// `ITEM_FLAG_PLAYERCAST` (`0x6e500b`–`0x6e501f`). Only an auto-repeat spell is popped after the
/// send (`0x6e5935` → `0x6e5989`). While the slot holds a spell that is not on-next-swing, a press
/// of another spell is refused with 0x61 (`TryCast 0x6e4d97`–`0x6e4dbe`) and the same spell bails
/// with no red line (`0x6e4d43`). On-next-swing spells (`Attributes & 0x404`) live in
/// [`QueuedMeleeSpell`] instead, and the spell awaiting its click is read off [`SpellTargeting`];
/// [`inflight`] joins the three into the reference's one slot.
///
/// Cleared by the spell's `SMSG_CAST_RESULT`, success or failure (`0x6e7408` → `0x6e741a`), keyed
/// on the spell id, or by [`local_self_cancel`]. `deadline` covers a lost result, which the
/// reference has no answer for: [`reap_lapsed_cast`] pops a cast past it as its result would,
/// once a frame, so every reader in between sees the slot the same.
///
/// It also carries the pending record's caster word (`0xceac48 + 0x08`, `0xceac50`), the one gate
/// on the cancel's events: see [`Self::fill_record`] and [`Self::take_caster_event`].
#[derive(Resource, Default)]
pub(crate) struct PendingCast {
    slot: Option<PendingCastState>,
    caster_is_player: bool,
}

struct PendingCastState {
    spell_id: u32,
    deadline: Instant,
}

impl PendingCast {
    /// A cast is outstanding: the next press meets `0x6e4d97`'s refusal.
    pub(crate) fn in_flight(&self) -> bool {
        self.current().is_some()
    }

    /// The outstanding cast's spell id, which `HandleCastResult 0x6e7330` (`0x6e7408`) and
    /// `IsCurrentAction`'s checked state key on.
    pub(crate) fn current(&self) -> Option<u32> {
        self.slot.as_ref().map(|p| p.spell_id)
    }

    /// The outstanding cast, when its deadline has passed at `now`.
    pub(crate) fn lapsed(&self, now: Instant) -> Option<u32> {
        self.slot
            .as_ref()
            .filter(|p| now >= p.deadline)
            .map(|p| p.spell_id)
    }

    /// `SetInFlightSpell(spell, item)` as the attempt calls it (`0x6e5026`). `item_flags` is the
    /// used item's template flags, `None` for a cast with no item: an item whose flags lack
    /// [`ITEM_FLAG_PLAYERCAST`] never enters the slot (a potion, food, a healthstone), so it
    /// blocks nothing. An item's deadline is short: vmangos refuses some `CMSG_USE_ITEM`s with
    /// only `SMSG_INVENTORY_CHANGE_FAILURE` and no cast result (`HandleUseItemOpcode`), and this
    /// deadline is what reopens casting.
    pub(crate) fn push(&mut self, spell_id: u32, item_flags: Option<u32>, now: Instant) {
        if item_flags.is_some_and(|flags| flags & ITEM_FLAG_PLAYERCAST == 0) {
            return;
        }
        let window = if item_flags.is_some() {
            ITEM_SEND_PROVISIONAL
        } else {
            SEND_PROVISIONAL
        };
        self.slot = Some(PendingCastState {
            spell_id,
            deadline: now + window,
        });
    }

    /// Tighten the deadline to the server's real cast time once `SMSG_SPELL_START` names it.
    pub(crate) fn refine(&mut self, cast_time_ms: u32, now: Instant) {
        if let Some(p) = &mut self.slot {
            p.deadline = now + Duration::from_millis(u64::from(cast_time_ms)) + CAST_SLACK;
        }
    }

    /// `SMSG_SPELL_DELAYED`: extend from the later of the deadline and now, so a lapsed deadline
    /// re-arms.
    pub(crate) fn delay(&mut self, delay_ms: u32, now: Instant) {
        if let Some(p) = &mut self.slot {
            p.deadline = p.deadline.max(now) + Duration::from_millis(u64::from(delay_ms));
        }
    }

    /// Clear on a resolution for our spell only: a proc's `SMSG_SPELL_GO` mid-cast must not.
    pub(crate) fn clear_if(&mut self, spell_id: u32) {
        if self.slot.as_ref().is_some_and(|p| p.spell_id == spell_id) {
            self.slot = None;
        }
    }

    /// The attempt's record fill (`0x6e4fb1`, once every rung has passed and before the arm, so a
    /// cursor spell and an arm refusal have it too), and the re-arm's (`0x6e748f`): the caster
    /// word names the active player. Every player commit fills it, an item use without
    /// `ITEM_FLAG_PLAYERCAST` and a queued strike included, though neither holds this slot.
    pub(crate) fn fill_record(&mut self) {
        self.caster_is_player = true;
    }

    /// The gate of `AbortCast 0x6e4940`'s events: `SPELLCAST_STOP`, `_FAILED` or `_INTERRUPTED`
    /// fires only while the caster word is the active player's (`0x6e4a04`–`0x6e4a1e`), and firing
    /// clears it (`0x6e4a5a`), so one record fires at most once whatever pops after it.
    pub(crate) fn take_caster_event(&mut self) -> bool {
        std::mem::take(&mut self.caster_is_player)
    }
}

/// Our queued on-next-swing spell (Heroic Strike, Cleave: `Attributes & 0x404`) in the two places
/// the reference keeps it: the in-flight slot `0xceca88`, which it holds while nothing is pushed
/// over it, and the save `0xcecaa8`. `SetInFlightSpell 0x6e4ad0` saves what the slot held as it
/// pushes (`0x6e4adf`) and copies the save back as it pops (`0x6e4b2c`), never clearing it, so a
/// strike a cast popped back over is in both. Only a strike's save matters: the attempt refuses or
/// cancels anything else in flight before it pushes (`0x6e4d62`, `0x6e4d97`).
///
/// No deadline. The handler clears the save for its spell (`0x6e73ee`) and pops the slot for it
/// (`0x6e7408`); vmangos answers a dead melee slot with `SMSG_SPELL_FAILED_OTHER`, which touches
/// neither (`0x6e8e40`), then a failing `SMSG_CAST_RESULT` 0x23 (`Spell::cancel`). A re-arm
/// replaces it (the server's one `CURRENT_MELEE_SPELL`). Re-pressing the queued spell bails with
/// no red line (`0x6e4d43`); it un-queues through the StopAttack chain (`0x5ecac0`,
/// `CancelQueuedCast 0x6e6f30`), which an auto-repeat press also runs (`0x6e5976`), or through
/// Esc, which cancels it as the inflight spell (`SpellStopCasting 0x6e6e80`).
#[derive(Resource, Default)]
pub(crate) struct QueuedMeleeSpell {
    slot: Option<u32>,
    save: Option<u32>,
}

impl QueuedMeleeSpell {
    /// The strike in the in-flight slot.
    pub(crate) fn current(&self) -> Option<u32> {
        self.slot
    }

    /// The strike the save holds.
    #[cfg(test)]
    pub(crate) fn saved(&self) -> Option<u32> {
        self.save
    }

    /// The strike in the slot with nothing over it, the save untouched.
    #[cfg(test)]
    pub(crate) fn arm(&mut self, spell_id: u32) {
        self.slot = Some(spell_id);
    }

    /// The push (`0x6e4adf`-`0x6e4af8`): the save takes the strike the slot `held`, and the slot
    /// `spell`, a strike's id or none for a spell [`PendingCast`] or the cursor holds.
    pub(super) fn push(&mut self, held: Option<u32>, spell: Option<u32>) {
        self.save = held;
        self.slot = spell;
    }

    /// The pop (`0x6e4b2c`-`0x6e4b3d`): the save back into the slot, the save kept.
    pub(super) fn restore(&mut self) {
        self.slot = self.save;
    }

    /// `HandleCastResult`'s save clear (`0x6e73ee`-`0x6e73f6`).
    pub(crate) fn clear_save_if(&mut self, spell_id: u32) {
        if self.save == Some(spell_id) {
            self.save = None;
        }
    }

    /// `CancelQueuedCast`'s save leg (`0x6e701a`): the save emptied, its strike returned.
    pub(super) fn take_save(&mut self) -> Option<u32> {
        self.save.take()
    }
}

/// What the reference's one inflight id `0xceca88` holds, joined from our slots: `IsCasting
/// 0x6e3d30`, the attempt's in-flight rungs (`0x6e4d43`, `0x6e4d97`) and `AbortCast 0x6e4940` read
/// a cast, a spell awaiting its click and a queued strike alike. It holds only what an attempt
/// pushed (`0x6e5026`): a cast whose item lacks `ITEM_FLAG_PLAYERCAST` is never in it, though it
/// runs and shows its bar.
///
/// A cast started over a queued strike pushes the strike into the save `0xcecaa8`, so the cast
/// comes first and the first Esc takes it. Its pop copies the strike back from the save, which
/// keeps it, so a second Esc cancels the strike and leaves it in the slot, and the server's echo
/// of the first cancel, which ends the melee slot too (vmangos `HandleCancelCastOpcode`), pops it
/// ([`QueuedMeleeSpell`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Inflight {
    /// An ordinary cast, with a guard, a bar and a `Casting` to reap.
    Cast(u32),
    /// A queued on-next-swing strike: no bar and no [`Casting`], so a cancel is the wire message
    /// and the slot.
    Strike(u32),
    /// The spell awaiting its click, which the attempt pushed (`0x6e5026`) before the arm raised
    /// the cursor (`0x6e5250`). A bind sends with it still in the slot, where it stays until its
    /// result, but the trade slot's bind, which pops it right after its send (`0x6e695a`,
    /// `CancelCast(0, 0, 0x17)`; not built, #88). A cursor whose item lacks the flag holds no
    /// slot, and the word stands alone ([`SpellTargeting`]).
    Targeting(u32),
}

impl Inflight {
    /// What a cancel names on the wire.
    pub(crate) fn spell_id(self) -> u32 {
        match self {
            Self::Cast(id) | Self::Strike(id) | Self::Targeting(id) => id,
        }
    }
}

/// The reference's inflight slot from ours.
pub(crate) fn inflight(
    pending: &PendingCast,
    queued_melee: &QueuedMeleeSpell,
    targeting: &SpellTargeting,
) -> Option<Inflight> {
    targeting
        .in_slot()
        .map(Inflight::Targeting)
        .or_else(|| pending.current().map(Inflight::Cast))
        .or_else(|| queued_melee.current().map(Inflight::Strike))
}

/// The player's cast state as the VM reads it ([`CastView`]): what the action bar's checked tests
/// and `SpellStopCasting` hold the slots against, pushed each frame and after every script call.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CastState<'w, 's> {
    pending: Res<'w, PendingCast>,
    queued_melee: Res<'w, QueuedMeleeSpell>,
    targeting: Res<'w, SpellTargeting>,
    auto_repeat: Res<'w, AutoRepeatActive>,
    me: Query<'w, 's, SelfCastFacts, With<crate::net::SelfPlayer>>,
}

/// The player's own cast facts: the attack flag and the form byte's store.
type SelfCastFacts = (
    Has<crate::creature_anim::Engaged>,
    Option<&'static crate::net::ObjectStore>,
);

impl CastState<'_, '_> {
    /// The view as it stands now.
    pub(crate) fn view(&self) -> CastView {
        let me = self.me.iter().next();
        CastView {
            slot: inflight(&self.pending, &self.queued_melee, &self.targeting)
                .map(Inflight::spell_id),
            targeting: self.targeting.spell(),
            auto_repeat: self.auto_repeat.0,
            attacking: me.is_some_and(|(engaged, _)| engaged),
            form: me
                .and_then(|(_, store)| store)
                .map_or(0, |store| store.0.unit_shapeshift_form()),
        }
    }
}

/// Slack past the named channel end, so a lost `MSG_CHANNEL_UPDATE(0)` cannot pin the ring on.
const CHANNEL_SLACK: Duration = Duration::from_secs(2);

/// Our running channel, from `MSG_CHANNEL_START` to `MSG_CHANNEL_UPDATE(0)`, finished or
/// interrupted: what the move cancel stops (`0x515145`-`0x515193`, which reads the player's
/// `UNIT_CHANNEL_SPELL`).
#[derive(Resource, Default)]
pub(crate) struct ActiveChannel(Option<(u32, Instant)>);

impl ActiveChannel {
    pub(crate) fn current(&self, now: Instant) -> Option<u32> {
        self.0
            .filter(|&(_, until)| now < until)
            .map(|(spell, _)| spell)
    }

    /// `MSG_CHANNEL_START`.
    pub(crate) fn start(&mut self, spell_id: u32, duration_ms: u32, now: Instant) {
        self.0 = Some((
            spell_id,
            now + Duration::from_millis(u64::from(duration_ms)) + CHANNEL_SLACK,
        ));
    }

    /// `MSG_CHANNEL_UPDATE`: 0 ends the channel, anything else re-times it (pushback shortens).
    pub(crate) fn update(&mut self, remaining_ms: u32, now: Instant) {
        if remaining_ms == 0 {
            self.0 = None;
        } else if let Some((_, until)) = &mut self.0 {
            *until = now + Duration::from_millis(u64::from(remaining_ms)) + CHANNEL_SLACK;
        }
    }
}

/// The live auto-repeat spell, the client's `0xceac30`: set at cast send (`0x6e54f0`) for
/// `AttributesEx2 & 0x20`, cleared by `SMSG_CANCEL_AUTO_REPEAT` (`0x6ea080`) and a matching cast
/// failure. `IsAutoRepeatAction` and the button flash read it; the sticky
/// `creature_anim::AutoRepeatArmed` is the idle gate.
#[derive(Resource, Default)]
pub(crate) struct AutoRepeatActive(pub Option<u32>);

/// The per-spell movement gates of the local self-cancel, tested at `0x51511e` and `0x515175`.
/// The move keybinds' dispatcher `0x515090` cancels on mask `0x10f0` (forward, back, strafe,
/// autorun; not turn or pitch) and `Script::Jump 0x513bd0` likewise, both calling `AbortCast`
/// for event 0x152 `SPELLCAST_STOP` and `CMSG_CANCEL_CAST`.
pub(crate) const SPELL_INTERRUPT_MOVEMENT: u32 = 0x1; // SpellRec+0x54 InterruptFlags (SPELL_INTERRUPT_FLAG_MOVEMENT)
const CHANNEL_INTERRUPT_MOVING: u32 = 0x8; // SpellRec+0x5c ChannelInterruptFlags (AURA_INTERRUPT_MOVING_CANCELS)

/// A directional move start or a jump happened this frame (the `0x10f0` mask; there is no
/// autorun yet). Set by `player::control`, consumed by [`local_self_cancel`].
#[derive(Resource, Default)]
pub(crate) struct LocalMoveStart(pub(crate) bool);

/// The client's local cancel on a move, jump or Esc mid-cast (`AbortCast 0x6e4940`):
///
/// - Esc first stops a running auto-repeat (`SpellStopCasting 0x6e6e80`, then `0x6ea080`),
///   and that spends the press; the cast survives to the next one.
/// - The cast: `CMSG_CANCEL_CAST`, the self `Casting` reaped with a `Fail` cast event, and the
///   `Stop` bar edge the cancel's own event fires ([`super::CastCore::cancel`]). Movement tests
///   the spell's `InterruptFlags`; Esc does not.
/// - The channel, on movement only: `CMSG_CANCEL_CHANNELLING` (`0x6e9b70`) and nothing local;
///   the bar and [`ActiveChannel`] close on `MSG_CHANNEL_UPDATE(0)` (`0x6e75f0`). Esc cannot
///   stop a channel: `0x6e6e80` never calls `0x6e9b70`, and the inflight id is 0 mid-channel.
///
/// vmangos answers the cancel with `SMSG_SPELL_FAILED_OTHER` and a failing `SMSG_CAST_RESULT`
/// 0x23, whose report finds nothing in flight and fires `SPELLCAST_INTERRUPTED` itself
/// (`0x6e2245`, [`super::CastCore::report`]), a round trip after the STOP; a strike the pop
/// restored is in flight, and the report fires nothing (`0x6e2228`). The stock
/// `CastingBarFrame.lua:132-139` steps the flash once per OnUpdate after the 1 s hold, followed
/// by the fade.
pub(super) fn local_self_cancel(mut moved: ResMut<LocalMoveStart>, mut cancel: SelfCancel) {
    if std::mem::take(&mut moved.0) {
        cancel.cancel(false, true);
    }
}

/// The guard's deadline, once a frame ahead of the local cancel, so a move that frame reads the
/// slot the lapse left: a cast past it pops ([`super::CastCore::reap_lapsed`]).
pub(super) fn reap_lapsed_cast(mut slots: super::CastSlots, net: Res<NetCommands>) {
    slots.core(&net).reap_lapsed(Instant::now());
}

/// What [`local_self_cancel`] reads and writes, shared with the Esc cancel, which is a script call
/// (`SpellStopCasting`) and lands in call order ([`crate::script_calls`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SelfCancel<'w, 's> {
    pending: ResMut<'w, PendingCast>,
    queued_melee: ResMut<'w, QueuedMeleeSpell>,
    channel: Res<'w, ActiveChannel>,
    auto_repeat: ResMut<'w, AutoRepeatActive>,
    reports: ResMut<'w, CastReports>,
    spells: Option<Res<'w, Spells>>,
    net: Res<'w, NetCommands>,
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    casting: Query<'w, 's, &'static Casting>,
    cast_events: MessageWriter<'w, CastEvent>,
    play_seq: ResMut<'w, PlaySeq>,
    ecs: Commands<'w, 's>,
    ground: ResMut<'w, super::targeting::SpellTargeting>,
}

impl SelfCancel<'_, '_> {
    /// `SpellStopCasting()` (`0x6e6e80`), the ESC chain's cast rung.
    pub(crate) fn stop_casting(&mut self) {
        self.cancel(true, false);
    }

    /// `CancelCast(1, 1, 0x1c)` (`0x6e4940`) as `UseSoulstone`'s item leg calls it (`0x48ae6b`),
    /// ungated: a targeting word is dropped (`0x6e4955`), else the cast or queued strike in flight
    /// is cancelled on the wire (`0x6e4970`), whatever its `InterruptFlags`. Its failed flag fires
    /// `SPELLCAST_FAILED` while the record's caster holds and ends a tradeskill repeat
    /// (`0x6e4ac2`) on every branch, with nothing in flight too.
    pub(crate) fn abort_cast(&mut self) {
        self.cancel_inflight(true, "soulstone");
    }

    /// The player's entity.
    fn self_entity(&self) -> Option<Entity> {
        self.self_guid
            .0
            .as_ref()
            .and_then(|g| self.index.0.get(g))
            .copied()
    }

    /// `CancelCast(failed, 1, 0x1c)` ([`super::CastCore::cancel`]), then a popped cast's local
    /// teardown: its self `Casting` reaped with a `Fail` cast event, as `spell_failed_other`'s
    /// echo would, run early so the echo finds it done. A strike a cast popped back over is in the
    /// slot and the save, so its pop copies it back (`0x6e4b2c`): it stays in flight, and each
    /// press until the server's echo sends its cancel again (`0x6e6eb4`).
    fn cancel_inflight(&mut self, failed: bool, why: &str) {
        let self_e = self.self_entity();
        let Self {
            pending,
            queued_melee,
            reports,
            net,
            casting,
            cast_events,
            play_seq,
            ecs,
            ground,
            ..
        } = self;
        let popped = super::CastCore {
            pending,
            queued_melee,
            ground,
            reports,
            net,
        }
        .cancel(failed, true, super::REASON_ERROR);
        if *crate::net::CAST_TRACE {
            info!("cast-trace: LOCAL self-cancel — {popped:?} ({why})");
        }
        if let (Some(Inflight::Cast(spell_id)), Some(e)) = (popped, self_e) {
            if casting.get(e).is_ok_and(|c| c.spell_id == spell_id) {
                ecs.entity(e).remove::<Casting>();
            }
            cast_events.write(CastEvent {
                entity: e,
                spell_id,
                kind: CastEventKind::Fail,
                cast_flags: 0,
                item_cast: false,
                seq: play_seq.next(),
            });
        }
    }

    fn cancel(&mut self, esc: bool, moved: bool) {
        let now = Instant::now();
        let self_e = self.self_entity();
        // Esc stops one thing, auto-repeat first (`0x6e6e80`).
        let esc = if esc && self.auto_repeat.0.is_some() {
            if *crate::net::CAST_TRACE {
                info!("cast-trace: LOCAL auto-repeat self-cancel (esc), CMSG_CANCEL_AUTO_REPEAT_SPELL");
            }
            crate::creature_anim::cancel_auto_repeat_local(
                self_e,
                &mut self.auto_repeat,
                &mut self.ecs,
                &self.net,
            );
            false // the press is spent
        } else {
            esc
        };
        if !esc && !moved {
            return;
        }
        let flags_open = |spells: &Option<Res<Spells>>,
                          pick: fn(&benilla_formats::SpellDisplay) -> bool,
                          spell_id: u32| {
            // An uncataloged spell cancels, as the server interrupts any ordinary cast.
            spells
                .as_ref()
                .and_then(|s| s.catalog.get(spell_id))
                .is_none_or(pick)
        };
        // `AbortCast` treats a cast and a queued strike alike; only the local teardown differs.
        if let Some(slot) = inflight(&self.pending, &self.queued_melee, &self.ground) {
            let spell_id = slot.spell_id();
            // Movement tests `InterruptFlags`, which spares a queued strike: Heroic Strike 78,
            // Cleave 845 and Raptor Strike 2973 ship 0 (Fireball `0xf`). It skips the whole test
            // while the word is up (`0x5150fd`, and `Script::Jump`).
            if esc
                || (!self.ground.active()
                    && flags_open(
                        &self.spells,
                        |d| d.interrupt_flags & SPELL_INTERRUPT_MOVEMENT != 0,
                        spell_id,
                    ))
            {
                // `SpellStopCasting` (`0x6e6ebd`) and the move cancels pass `failed` 0: STOP.
                self.cancel_inflight(false, if esc { "esc" } else { "moved" });
            }
        }
        // The channel: movement only, and the send only (`0x6e9b70`).
        if moved {
            if let Some(spell_id) = self.channel.current(now) {
                if flags_open(
                    &self.spells,
                    |d| d.channel_interrupt_flags & CHANNEL_INTERRUPT_MOVING != 0,
                    spell_id,
                ) {
                    if *crate::net::CAST_TRACE {
                        info!("cast-trace: LOCAL channel self-cancel — spell {spell_id}, CMSG_CANCEL_CHANNELLING");
                    }
                    let _ = self
                        .net
                        .0
                        .send(ClientCommand::CancelChannelling { spell_id });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREBALL: u32 = 133;

    /// A channel whose bar is suppressed still arms [`ActiveChannel`]: the move cancel reads the
    /// unit's `UNIT_CHANNEL_SPELL` (`0x51514e`), which the bar's gate in the channel-start
    /// handler `0x6e7550` never touches.
    #[test]
    fn suppressing_the_channel_bar_still_arms_the_running_channel() {
        let t0 = Instant::now();
        // Blood Siphon 24322, a suppressed row.
        let mut channel = ActiveChannel::default();
        channel.start(24322, 8_000, t0);
        assert_eq!(
            channel.current(t0 + Duration::from_secs(1)),
            Some(24322),
            "the wire edge arms the mirror regardless of what the bar does with it"
        );
    }

    /// `ActionSlotIsCurrent 0x4e53a0` holds a spell slot against the in-flight spell (`0x6e3d10`)
    /// and the cursor's (`0x6e48e0`) alone: a running channel is in neither, and a strike under a
    /// cast is in the save, so only the cast's button is checked until the cast pops.
    #[test]
    fn the_checked_test_reads_the_slot_and_the_cursor_alone() {
        use bevy::ecs::system::RunSystemOnce;
        const ARCANE_MISSILES: u32 = 5143;
        const HEROIC_STRIKE: u32 = 78;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<SpellTargeting>()
            .init_resource::<ActiveChannel>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<CastReports>()
            .insert_resource(NetCommands(tx));
        let current = |app: &mut App, spell: u32| {
            app.world_mut()
                .run_system_once(move |state: CastState| {
                    benilla_ui::script::ActionState {
                        spell: Some(spell),
                        ..Default::default()
                    }
                    .is_current(&state.view(), Default::default())
                })
                .unwrap()
        };
        app.world_mut().resource_mut::<ActiveChannel>().start(
            ARCANE_MISSILES,
            5_000,
            Instant::now(),
        );
        assert!(
            !current(&mut app, ARCANE_MISSILES),
            "a channel is not in flight"
        );

        app.world_mut()
            .resource_mut::<QueuedMeleeSpell>()
            .arm(HEROIC_STRIKE);
        assert!(current(&mut app, HEROIC_STRIKE));
        app.world_mut()
            .run_system_once(
                |mut slots: super::super::CastSlots, net: Res<NetCommands>| {
                    slots.core(&net).push(
                        FIREBALL,
                        super::super::Held::Cast { item_flags: None },
                        Instant::now(),
                    );
                },
            )
            .unwrap();
        assert!(current(&mut app, FIREBALL));
        assert!(
            !current(&mut app, HEROIC_STRIKE),
            "the strike is the save's"
        );
        app.world_mut()
            .run_system_once(
                |mut slots: super::super::CastSlots, net: Res<NetCommands>| {
                    slots
                        .core(&net)
                        .cancel(false, false, super::super::REASON_ERROR);
                },
            )
            .unwrap();
        assert!(
            current(&mut app, HEROIC_STRIKE),
            "popped back into the slot"
        );
    }

    #[test]
    fn a_fresh_guard_is_open() {
        let g = PendingCast::default();
        assert!(!g.in_flight(), "nothing sent yet — casting allowed");
    }

    #[test]
    fn arming_closes_the_guard_and_a_matching_resolution_opens_it() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.push(FIREBALL, None, t0);
        assert!(
            g.in_flight(),
            "a cast is in flight — a second one is refused"
        );
        g.clear_if(FIREBALL); // SMSG_SPELL_GO / a failing SMSG_CAST_RESULT for our cast
        assert!(!g.in_flight(), "our cast resolved — casting allowed again");
    }

    /// `0x6e500b`–`0x6e501f`: an item use is pushed only when its template flags carry
    /// `ITEM_FLAG_PLAYERCAST` (`0x40`); a spell has no item and is always pushed. The item's
    /// window is the short one.
    #[test]
    fn an_item_use_enters_the_slot_only_with_the_playercast_flag() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.push(FIREBALL, Some(0), t0);
        assert!(!g.in_flight(), "flags 0 (a potion) push nothing");
        g.push(FIREBALL, Some(0x2 | 0x200), t0);
        assert!(!g.in_flight(), "other bits do not count");
        g.push(FIREBALL, Some(0x40), t0);
        assert!(g.in_flight(), "the flag pushes");
        let past_item = t0 + ITEM_SEND_PROVISIONAL + Duration::from_millis(1);
        assert_eq!(
            g.lapsed(past_item),
            Some(FIREBALL),
            "on the item's own deadline"
        );
        g.push(FIREBALL, None, t0);
        assert_eq!(g.lapsed(past_item), None);
    }

    #[test]
    fn a_different_spells_resolution_leaves_the_guard_closed() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.push(FIREBALL, None, t0);
        g.clear_if(FIREBALL + 1); // a triggered proc's GO for some other spell mid-cast
        assert!(
            g.in_flight(),
            "a different spell's resolution must not open the guard on our cast"
        );
    }

    #[test]
    fn the_guard_lapses_at_its_safety_deadline() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.push(FIREBALL, None, t0);
        assert_eq!(
            g.lapsed(t0 + SEND_PROVISIONAL - Duration::from_secs(1)),
            None
        );
        assert_eq!(
            g.lapsed(t0 + SEND_PROVISIONAL + Duration::from_secs(1)),
            Some(FIREBALL)
        );
        assert!(g.in_flight(), "until the reaper pops it");
    }

    #[test]
    fn a_pushback_extends_the_guard_so_it_holds_past_the_stretched_cast() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.push(FIREBALL, None, t0);
        g.refine(1_000, t0); // deadline t0 + 1 s + 2 s slack
        g.delay(500, t0); // pushed to t0 + 3.5 s
        assert_eq!(
            g.lapsed(t0 + Duration::from_millis(3_200)),
            None,
            "the pushback kept the guard shut past the original refined deadline"
        );
    }

    #[test]
    fn spell_start_tightens_the_deadline_to_the_real_cast_time() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.push(FIREBALL, None, t0);
        g.refine(1_500, t0); // SMSG_SPELL_START: 1.5 s + 2 s slack
        let at_four = t0 + Duration::from_secs(4);
        assert_eq!(
            g.lapsed(at_four),
            Some(FIREBALL),
            "refined to 1.5s + slack, the guard has lapsed by t+4s (it would still hold at the \
             5s provisional)"
        );
    }

    /// A lapsed cast leaves the slot as its result's pop would (`0x6e4b2c`): a strike the push
    /// saved under it comes back to the slot, signalled, so the late result finds it queued and
    /// the strike survives. Inside its deadline the reaper leaves the cast alone.
    #[test]
    fn a_lapsed_cast_pops_back_the_strike_it_was_pushed_over() {
        use bevy::ecs::system::RunSystemOnce;
        const HEROIC_STRIKE: u32 = 78;
        let mut app = App::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<SpellTargeting>()
            .init_resource::<CastReports>()
            .insert_resource(NetCommands(tx));
        let t0 = Instant::now();
        let reap = |app: &mut App, now: Instant| {
            app.world_mut()
                .run_system_once(
                    move |mut slots: super::super::CastSlots, net: Res<NetCommands>| {
                        slots.core(&net).reap_lapsed(now);
                    },
                )
                .unwrap();
            std::mem::take(&mut app.world_mut().resource_mut::<CastReports>().0).len()
        };
        app.world_mut()
            .run_system_once(
                move |mut slots: super::super::CastSlots, net: Res<NetCommands>| {
                    let mut core = slots.core(&net);
                    core.push(HEROIC_STRIKE, super::super::Held::Strike, t0);
                    core.push(FIREBALL, super::super::Held::Cast { item_flags: None }, t0);
                },
            )
            .unwrap();
        app.world_mut().resource_mut::<CastReports>().0.clear();
        assert_eq!(reap(&mut app, t0), 0, "inside its deadline");
        assert!(app.world().resource::<PendingCast>().in_flight());
        assert_eq!(
            reap(&mut app, t0 + SEND_PROVISIONAL + Duration::from_millis(1)),
            1,
            "one pop, signalled"
        );
        assert!(!app.world().resource::<PendingCast>().in_flight());
        assert_eq!(
            app.world().resource::<QueuedMeleeSpell>().current(),
            Some(HEROIC_STRIKE),
            "the strike is back in the slot"
        );
    }

    /// [`local_self_cancel`] in a small App, and the Esc cancel: the Esc tests insert a VM, call
    /// the real `SpellStopCasting()` and apply what it queued.
    mod local_cancel {
        use super::*;
        use crate::creature_anim::PlaySeq;
        use crate::net::{Guid, GuidIndex, NetCommands, SelfGuid, SelfPlayer};
        use crate::ui_cast::{feed_cast_reports, CastBarEdge};
        use benilla_formats::{SpellCatalog, SpellDisplay};
        use benilla_ui::script::{ScriptCall, UiScript};
        use bevy::ecs::system::RunSystemOnce;
        use std::collections::HashMap;

        const ARCANE_MISSILES: u32 = 5143;
        const HEROIC_STRIKE: u32 = 78;

        fn harness(
            displays: HashMap<u32, SpellDisplay>,
        ) -> (App, crossbeam_channel::Receiver<crate::net::ClientCommand>) {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .add_message::<crate::net::FieldChanged>()
                .init_resource::<CastReports>()
                .init_resource::<crate::items::Items>()
                .init_resource::<crate::ui_action::CastFailRedisplays>()
                .init_resource::<crate::ui_chat::ChatLog>()
                .init_resource::<crate::sound::MessageSounds>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<ActiveChannel>()
                .init_resource::<AutoRepeatActive>()
                .init_resource::<crate::spell::targeting::SpellTargeting>()
                .init_resource::<LocalMoveStart>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<PlaySeq>()
                .insert_resource(NetCommands(tx))
                .insert_resource(crate::ui_action::Spells {
                    catalog: SpellCatalog::from_displays(displays),
                    ..crate::ui_action::Spells::empty_for_tests()
                });
            let self_e = app.world_mut().spawn((Guid(10), SelfPlayer)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(10, self_e);
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);
            (app, rx)
        }

        /// The record fill (`0x6e4fb1`) every attempt makes before its push or its cursor.
        fn fill_record(app: &mut App) {
            app.world_mut().resource_mut::<PendingCast>().fill_record();
        }

        /// The `Casting` an `SMSG_SPELL_START` echo leaves on the self player.
        fn mark_casting(app: &mut App, spell_id: u32) {
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            app.world_mut().entity_mut(self_e).insert(Casting {
                spell_id,
                until: None,
            });
        }

        /// The attempt's push of `spell_id` (`0x6e5026`), over whatever the slot holds.
        fn push(app: &mut App, spell_id: u32, held: crate::spell::Held) {
            app.world_mut()
                .run_system_once(
                    move |mut slots: crate::spell::CastSlots, net: Res<NetCommands>| {
                        slots.core(&net).push(spell_id, held, Instant::now());
                    },
                )
                .unwrap();
            app.world_mut().resource_mut::<CastReports>().0.clear();
        }

        fn run(app: &mut App, moved: bool) {
            app.world_mut().resource_mut::<LocalMoveStart>().0 = moved;
            app.world_mut().run_system_once(local_self_cancel).unwrap();
        }

        /// Apply the `SpellStopCasting()` calls the VM queued, as `crate::script_calls` does.
        fn apply_stops(app: &mut App) {
            app.world_mut()
                .run_system_once(|mut script: NonSendMut<UiScript>, mut cancel: SelfCancel| {
                    crate::script_calls::in_call_order(&mut script, |_, call| {
                        if call == ScriptCall::SpellStopCasting {
                            cancel.stop_casting();
                        }
                    });
                })
                .unwrap();
        }

        #[test]
        fn a_move_edge_mid_cast_cancels_locally() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf, // the ordinary timed-cast mask (movement bit set)
                    ..Default::default()
                },
            )]));
            fill_record(&mut app);
            app.world_mut()
                .resource_mut::<PendingCast>()
                .push(FIREBALL, None, Instant::now());
            mark_casting(&mut app, FIREBALL);

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "the move edge ships CMSG_CANCEL_CAST for the in-flight cast"
            );
            assert!(
                !app.world().resource::<PendingCast>().in_flight(),
                "the in-flight guard opens with the local cancel"
            );
            assert!(
                matches!(
                    app.world().resource::<CastReports>().edges()[..],
                    [CastBarEdge::Stop]
                ),
                "the cancel's own event is STOP (`0x6e4a4b`), which arms the flash overlay; the \
                 red INTERRUPTED is the echo's report, one round trip later"
            );
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            assert!(
                app.world().entity(self_e).get::<Casting>().is_none(),
                "the self `Casting` state is reaped, so the server's echo (FAILED_OTHER + \
                 CAST_RESULT) finds nothing to repaint"
            );
            assert!(
                !app.world().resource::<LocalMoveStart>().0,
                "the edge is consumed"
            );
        }

        /// `InterruptFlags` without the movement bit keeps casting (`0x51511e`).
        #[test]
        fn the_interrupt_flags_gate_spares_a_movement_castable_spell() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xe, // movement bit clear
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .push(FIREBALL, None, Instant::now());

            run(&mut app, true);

            assert!(rx.try_recv().is_err(), "no wire cancel for a spared spell");
            assert!(
                app.world().resource::<PendingCast>().in_flight(),
                "the guard keeps holding — the cast runs on"
            );
            assert!(app.world().resource::<CastReports>().0.is_empty());
        }

        #[test]
        fn an_uncataloged_spell_fails_open() {
            let (mut app, rx) = harness(HashMap::new());
            app.world_mut()
                .resource_mut::<PendingCast>()
                .push(FIREBALL, None, Instant::now());

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "an unknown spell's cast still cancels on movement"
            );
        }

        /// A cast running outside the slot, an item's without `ITEM_FLAG_PLAYERCAST`, is not
        /// the move cancel's: `0x5150f2` reads the in-flight slot, which its attempt never
        /// pushed, so nothing is sent and the server's own interrupt ends it.
        #[test]
        fn a_started_cast_outside_the_slot_survives_a_move() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf,
                    ..Default::default()
                },
            )]));
            mark_casting(&mut app, FIREBALL); // PendingCast stays empty

            run(&mut app, true);

            assert!(rx.try_recv().is_err(), "no cancel goes out");
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            assert!(app.world().entity(self_e).get::<Casting>().is_some());
            assert!(app.world().resource::<CastReports>().0.is_empty());
        }

        #[test]
        fn no_edge_means_no_cancel() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf,
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .push(FIREBALL, None, Instant::now());

            run(&mut app, false);

            assert!(rx.try_recv().is_err());
            assert!(app.world().resource::<PendingCast>().in_flight());
            assert!(app.world().resource::<CastReports>().0.is_empty());
        }

        /// Mid-channel `SpellStopCasting()` answers nil and nothing is sent.
        #[test]
        fn esc_cannot_stop_a_channel() {
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            let now = Instant::now();
            app.world_mut()
                .resource_mut::<ActiveChannel>()
                .start(ARCANE_MISSILES, 5_000, now);

            app.world_mut().run_system_once(feed_cast_reports).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                None,
                "mid-channel the binding answers nil — the vanilla /stopcasting quirk"
            );
            apply_stops(&mut app);
            assert!(
                rx.try_recv().is_err(),
                "no wire cancel — channels break only on the movement path"
            );
            assert_eq!(
                app.world().resource::<ActiveChannel>().current(now),
                Some(ARCANE_MISSILES)
            );
        }

        /// `SpellStopCasting` (`0x6e6e80`) stops the auto-repeat first and spends the press.
        #[test]
        fn esc_cancels_the_auto_repeat_first_and_the_cast_on_the_next_press() {
            const AUTO_SHOT: u32 = 75;
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut().resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);
            app.world_mut()
                .resource_mut::<PendingCast>()
                .push(FIREBALL, None, Instant::now());
            mark_casting(&mut app, FIREBALL);

            // Press 1 stops the auto-repeat only.
            app.world_mut().run_system_once(feed_cast_reports).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            apply_stops(&mut app);
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelAutoRepeat)
                ),
                "the auto-repeat cancel ships first — the reference's priority branch"
            );
            assert!(rx.try_recv().is_err(), "the cast survives the first press");
            assert!(app.world().resource::<AutoRepeatActive>().0.is_none());
            assert!(
                app.world().resource::<PendingCast>().in_flight(),
                "the send guard still holds — one press stopped one thing"
            );
            assert!(
                app.world().resource::<CastReports>().0.is_empty(),
                "an auto-repeat cancel paints no bar edge"
            );

            // Press 2 cancels the cast.
            app.world_mut().run_system_once(feed_cast_reports).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            apply_stops(&mut app);
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "the second press reaches the cast half"
            );
        }

        /// Esc with a strike queued cancels it and returns 1 (`IsCasting 0x6e3d30` is true), so
        /// the Esc ladder stops before `ClearTarget()`.
        #[test]
        fn esc_unqueues_a_strike_and_eats_the_press() {
            let (mut app, rx) = harness(HashMap::from([(
                HEROIC_STRIKE,
                SpellDisplay {
                    attributes: 0x5_0014, // the shipped row: on-next-swing (`& 0x404`)
                    interrupt_flags: 0,   // no movement bit, also shipped
                    ..Default::default()
                },
            )]));
            app.insert_non_send_resource(UiScript::new().unwrap());
            fill_record(&mut app);
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE);

            app.world_mut().run_system_once(feed_cast_reports).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1),
                "the queued strike makes IsCasting true, so the binding EATS the press — which \
                 is what spares the target three rungs further down the ladder"
            );
            apply_stops(&mut app);
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast {
                        spell_id: HEROIC_STRIKE
                    })
                ),
                "AbortCast names the queued strike on the wire, exactly as it would a cast"
            );
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                None,
                "the slot opens locally — the checked ring darkens this frame, not one RTT later"
            );
            assert!(
                matches!(
                    app.world().resource::<CastReports>().edges()[..],
                    [CastBarEdge::Stop]
                ),
                "the reference's `0x152` STOP and nothing else: no red INTERRUPTED for a \
                 spell that never opened a bar"
            );
        }

        /// The spell awaiting its click is in flight (`0x6e5026` precedes the arm that raises the
        /// cursor), so `SpellStopCasting` (`0x6e6eb4`) answers 1 and `AbortCast(cl=0)` drops the
        /// word with no packet and fires `SPELLCAST_STOP` (`0x152`), where `SpellStopTargeting`'s
        /// `cl=1` would fire FAILED.
        #[test]
        fn esc_during_targeting_answers_one_drops_the_cursor_and_fires_stop() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf,
                    ..Default::default()
                },
            )]));
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .non_send_resource::<UiScript>()
                .run(
                    "STOPS, FAILS = 0, 0 \
                     local f = CreateFrame('Frame') \
                     f:RegisterEvent('SPELLCAST_STOP') \
                     f:RegisterEvent('SPELLCAST_FAILED') \
                     f:SetScript('OnEvent', function() \
                         if event == 'SPELLCAST_STOP' then STOPS = STOPS + 1 else FAILS = FAILS + 1 end \
                     end)",
                )
                .unwrap();
            fill_record(&mut app);
            app.world_mut()
                .resource_mut::<crate::spell::targeting::SpellTargeting>()
                .enter(FIREBALL, crate::spell::CastCommit::Spell, 0x40);

            app.world_mut().run_system_once(feed_cast_reports).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1),
                "the targeting spell is in flight, so IsCasting is true and the press is eaten"
            );
            apply_stops(&mut app);

            assert!(
                !app.world()
                    .resource::<crate::spell::targeting::SpellTargeting>()
                    .active(),
                "the cursor drops"
            );
            assert!(rx.try_recv().is_err(), "a targeting abort sends nothing");
            assert!(
                matches!(
                    app.world().resource::<CastReports>().edges()[..],
                    [CastBarEdge::Stop]
                ),
                "STOP and nothing else: no wire echo follows, so no INTERRUPTED"
            );
            app.world_mut().run_system_once(feed_cast_reports).unwrap();
            let script = app.world().non_send_resource::<UiScript>();
            assert_eq!(script.eval::<i64>("return STOPS").unwrap(), 1);
            assert_eq!(script.eval::<i64>("return FAILS").unwrap(), 0);
            assert_eq!(
                script
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                None,
                "and the next press finds nothing in flight"
            );
        }

        /// `0x515090` and `Script::Jump` skip their cast test while `IsTargeting`, so a move edge
        /// leaves the cursor up whatever the spell's `InterruptFlags`.
        #[test]
        fn a_move_edge_leaves_the_targeting_cursor_up() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf,
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<crate::spell::targeting::SpellTargeting>()
                .enter(FIREBALL, crate::spell::CastCommit::Spell, 0x40);

            run(&mut app, true);

            assert!(rx.try_recv().is_err());
            assert!(app
                .world()
                .resource::<crate::spell::targeting::SpellTargeting>()
                .active());
            assert!(app.world().resource::<CastReports>().0.is_empty());
        }

        /// Heroic Strike's `InterruptFlags` are 0, so movement leaves it queued.
        #[test]
        fn a_move_edge_leaves_a_queued_strike_queued() {
            let (mut app, rx) = harness(HashMap::from([(
                HEROIC_STRIKE,
                SpellDisplay {
                    interrupt_flags: 0,
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE);

            run(&mut app, true);

            assert!(rx.try_recv().is_err(), "running at the mob cancels nothing");
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                Some(HEROIC_STRIKE)
            );
        }

        /// A cast nested over a queued strike (`PushPopNestedCast 0x6e4ad0`) dies on the first
        /// Esc, whose pop copies the strike back from the save (`0x6e4b2c`); the second finds it
        /// in flight (`0x6e6eb4`), is eaten, and sends its cancel, and the pop copies it back
        /// again, so a third does the same. Only the first fires an event: one record, one caster
        /// word.
        #[test]
        fn esc_takes_the_cast_first_and_the_strike_on_the_next_press() {
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE);
            fill_record(&mut app);
            push(
                &mut app,
                FIREBALL,
                crate::spell::Held::Cast { item_flags: None },
            );
            mark_casting(&mut app, FIREBALL);

            app.world_mut().run_system_once(feed_cast_reports).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            apply_stops(&mut app);
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "the nested cast is what sits in the inflight slot — it dies first"
            );
            assert!(rx.try_recv().is_err());
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                Some(HEROIC_STRIKE),
                "the strike is the SAVE underneath, untouched by the first press"
            );
            assert!(matches!(
                app.world().resource::<CastReports>().edges()[..],
                [CastBarEdge::Stop]
            ));
            app.world_mut().resource_mut::<CastReports>().0.clear();

            // Press 2: the pop put the strike back in the slot.
            app.world_mut().run_system_once(feed_cast_reports).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1),
                "still stoppable — so this press is eaten too, and the target still survives"
            );
            apply_stops(&mut app);
            assert!(matches!(
                rx.try_recv(),
                Ok(crate::net::ClientCommand::CancelCast {
                    spell_id: HEROIC_STRIKE
                })
            ));
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                Some(HEROIC_STRIKE),
                "the pop copied the strike back: the save keeps it"
            );
            assert!(
                app.world().resource::<CastReports>().edges().is_empty(),
                "the first press's event cleared the caster word (`0x6e4a5a`), so the second \
                 fires none (`0x6e4a1e`); only the strike's pop signals"
            );

            // Press 3, still inside the round trip: the same again.
            app.world_mut().run_system_once(feed_cast_reports).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            apply_stops(&mut app);
            assert!(matches!(
                rx.try_recv(),
                Ok(crate::net::ClientCommand::CancelCast {
                    spell_id: HEROIC_STRIKE
                })
            ));
            assert!(rx.try_recv().is_err());
        }

        /// `0x6e9b70` sends the cancel and fires no event, clearing nothing local.
        #[test]
        fn a_move_edge_mid_channel_sends_the_cancel_and_touches_nothing_local() {
            let (mut app, rx) = harness(HashMap::from([(
                ARCANE_MISSILES,
                SpellDisplay {
                    channel_interrupt_flags: 0x7c0c, // the real 5143 mask (moving bit 0x8 set)
                    ..Default::default()
                },
            )]));
            let now = Instant::now();
            app.world_mut()
                .resource_mut::<ActiveChannel>()
                .start(ARCANE_MISSILES, 5_000, now);

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelChannelling {
                        spell_id: ARCANE_MISSILES
                    })
                ),
                "the move edge ships CMSG_CANCEL_CHANNELLING"
            );
            assert_eq!(
                app.world().resource::<ActiveChannel>().current(now),
                Some(ARCANE_MISSILES),
                "the mirror stays live — the server's UPDATE(0) is what clears it"
            );
            assert!(
                app.world().resource::<CastReports>().0.is_empty(),
                "no local bar edge — the bar closes on the wire's SPELLCAST_CHANNEL_STOP"
            );
        }
    }

    #[test]
    fn active_channel_tracks_the_wire_lifecycle() {
        let t0 = Instant::now();
        let mut ch = ActiveChannel::default();
        assert_eq!(ch.current(t0), None);

        ch.start(10, 5_000, t0); // a 5 s channel
        assert_eq!(ch.current(t0), Some(10));
        assert_eq!(
            ch.current(t0 + Duration::from_secs(4)),
            Some(10),
            "still live near the end"
        );
        assert_eq!(
            ch.current(t0 + Duration::from_secs(8)),
            None,
            "the slack deadline self-clears a lost UPDATE(0)"
        );

        // Pushback: an update naming less time moves the deadline in.
        ch.start(10, 5_000, t0);
        ch.update(1_000, t0);
        assert_eq!(ch.current(t0 + Duration::from_secs(4)), None);

        ch.start(10, 5_000, t0);
        ch.update(0, t0);
        assert_eq!(ch.current(t0), None);
    }
}
