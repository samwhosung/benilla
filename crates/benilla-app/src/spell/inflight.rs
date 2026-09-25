//! **The in-flight slot** — the spell's own cast state, in the reference's terms: the pending
//! cast request (`SPELLCAST` @`0xceac48` + the inflight id `0xceca88` → [`PendingCast`]), the
//! queued on-next-swing strike ([`QueuedMeleeSpell`], our second slot for what the reference
//! keeps in the one id — [`Inflight`] re-joins them), the running channel (`0xceac58` →
//! [`ActiveChannel`]), the auto-repeat key (`0xceac30` → [`AutoRepeatActive`]) — and the local
//! abort that ends a cast the same tick a move, a jump or Esc lands (`AbortCast 0x6e4940` →
//! [`local_self_cancel`]).
//!
//! Written by the spell's packet handlers ([`super::net`]), the one cast path
//! ([`super::CastLadder`]) and the attack seams; read by the feeds — the cast bar
//! (`crate::ui_cast`), the action state, the target chain. Until decision 2328 this lived in
//! `ui_cast` and `ui_action`; it is the state 2265 §A7 named as having no owner.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::creature_anim::{CastEvent, CastEventKind, Casting, PlaySeq};
use crate::net::{ClientCommand, GuidIndex, NetCommands, SelfGuid};
use crate::ui_action::Spells;
use crate::ui_cast::{CastBarEdge, CastBarFeed};

/// Provisional in-flight window armed at send, before `SMSG_SPELL_START` names the real cast time.
/// It only has to outlast the send→START round trip; an instant, whose `SMSG_SPELL_GO` clears the
/// guard first, never reaches it. Generous on purpose — the resolution packets do the real clearing.
const SEND_PROVISIONAL: Duration = Duration::from_secs(5);

/// The same provisional for an **item** use's cast ([`PendingCast::arm_item`]) — deliberately much
/// tighter, because the two arms have different worst cases *on this wire*. Every
/// `CMSG_CAST_SPELL` vmangos accepts is answered (`Spell::SendCastResult` on any rejection,
/// `SMSG_SPELL_START`/`GO` otherwise), so a spell's guard is always cleared by a packet and
/// [`SEND_PROVISIONAL`] never actually governs. `CMSG_USE_ITEM` has legs that answer with
/// `SMSG_INVENTORY_CHANGE_FAILURE` and **no cast result at all** — `HandleUseItemOpcode`'s
/// equip-error gates (`CanUseItem`, the in-combat non-combat-spell refusal, item-not-found) —
/// and there the provisional IS the safety net, so it has to be short enough that a silently
/// refused use cannot lock casting out for long. It only ever covers the send→`SMSG_SPELL_START`
/// window anyway: [`PendingCast::refine`] stretches the guard to the server's real cast time the
/// moment START names it, so a use that *does* become a cast is guarded for its whole length.
const ITEM_SEND_PROVISIONAL: Duration = Duration::from_millis(1_500);

/// Pushback slack added to the server's cast time at `SMSG_SPELL_START`, so a cast the server
/// delays (`SMSG_SPELL_DELAYED`) still guards past its stretched end.
const CAST_SLACK: Duration = Duration::from_secs(2);

/// Our own outstanding cast — the client's optimistic in-flight guard, and the fix for the
/// spam-cancel bug. `TryCast 0x6e4b60` refuses to send a second
/// `CMSG_CAST_SPELL` while `IsCasting 0x6e3d30` (the inflight-spell-id `0xceca88`) is set — the
/// same-spell press bails **silently** at `6e4d43`; a different spell hits the
/// `[SpellRec+0x18] & 0x404` gate at `6e4d97`: when the *inflight* rec lacks those bits (an
/// ordinary cast — Fireball is `0x10000`), it errors 0x61 without sending; when the inflight IS
/// on-next-swing (a queued Heroic Strike), the new cast passes and nests around it (exactly
/// `0x404`, on the inflight rec, no channel leg: Slam mid-cast blocks Battle Shout). In our
/// model the on-next-swing class never occupies this guard at all — it arms [`QueuedMeleeSpell`]
/// instead — so this guard only ever holds ordinary casts and the gate needs no attribute test.
/// Ours was server-driven: every mashed key fired a *duplicate* cast, the
/// server rejected the dupe with `SMSG_CAST_RESULT` failure, and that turned the running cast's bar
/// red while the original completed later. This marker drops the duplicate at the source, exactly
/// as the client does.
///
/// It is **optimistic** — armed the instant we send, not when the server's `SMSG_SPELL_START`
/// echoes back — because the spam lands during that round trip. Cleared by the cast's resolution
/// (`SMSG_SPELL_GO` / a failing `SMSG_CAST_RESULT` / `SMSG_SPELL_FAILED_OTHER`), spell-id-keyed like
/// the `Casting` reap — and by the local self-cancel directly ([`local_self_cancel`], the
/// client's `AbortCast 0x6e4940` mirrored since 0444). `deadline` is the safety net for a
/// resolution that goes missing on the wire: armed generously and tightened to the real cast
/// time once `SMSG_SPELL_START` names it.
#[derive(Resource, Default)]
pub(crate) struct PendingCast(Option<PendingCastState>);

struct PendingCastState {
    spell_id: u32,
    deadline: Instant,
    /// Does this record **guard** — i.e. does it refuse the next press, light the in-flight ring,
    /// and gate the by-name walk? True for the classes benilla lets occupy the guard: ordinary
    /// casts and item uses.
    ///
    /// The reference has no such flag, because it has no such split: `[0xceca88]` is written for
    /// **every** committed cast (`0x6e5026 call 0x6e4ad0` inside TryCast) and the *gate* does the
    /// discriminating instead, on the inflight rec's `Attributes & 0x404` at `6e4d97`. Ours grew
    /// up the other way round — the guard is armed only for the classes it refuses on, so a
    /// ranged shot was never recorded as committed **at all**.
    ///
    /// That gap is what shipped B280 broken twice: the `modalNextSpell` chain
    /// (`super::net`'s `cast_result`) has to answer the reference's `0x6e7408 cmp
    /// ecx,[0xceca88]` — "is this reply for the cast I just sent?" — and every hunter shot is
    /// `Attributes & 0x2` ranged, so the answer was always no and the chain never fired in play.
    /// The flag closes it without touching the three consumers that were right: the record is now
    /// armed for every commit ([`PendingCast::committed`] reads it), and `guards` keeps
    /// [`PendingCast::in_flight`] / [`PendingCast::current`] answering exactly what they did.
    guards: bool,
}

impl PendingCast {
    /// Whether a **guarding** cast we sent is still outstanding (unresolved and inside its safety
    /// deadline) — the refusal's own question. A ranged shot is committed but does not guard.
    pub(crate) fn in_flight(&self, now: Instant) -> bool {
        self.0
            .as_ref()
            .is_some_and(|p| p.guards && now < p.deadline)
    }

    /// **The reference's `[0xceca88]` proper** — the id of the cast we last committed and have not
    /// yet had a resolution for, whatever its class. This is what `HandleCastResult 0x6e7330`
    /// tests at `0x6e7408` before it reads `modalNextSpell`, and the only read that must see a
    /// ranged shot. Everything else about the record is unchanged; see [`PendingCastState::guards`]
    /// for why the two questions are separate fields here and one global there.
    pub(crate) fn committed(&self, now: Instant) -> Option<u32> {
        self.0
            .as_ref()
            .filter(|p| now < p.deadline)
            .map(|p| p.spell_id)
    }

    /// The outstanding cast's spell id, if one is in flight — the app-side mirror of the client's
    /// current-cast read (`0xceca88`), which `IsCurrentAction`'s checked state keys on (decision
    /// 0137 phase 4).
    pub(crate) fn current(&self, now: Instant) -> Option<u32> {
        self.0
            .as_ref()
            .filter(|p| p.guards && now < p.deadline)
            .map(|p| p.spell_id)
    }

    /// Arm on a fresh send — the optimistic `0xceca88` write at the client's cast-send. `guards`
    /// says whether this class also occupies the refusal ([`PendingCastState::guards`]): ordinary
    /// casts do, a ranged shot is recorded and does not.
    pub(crate) fn arm(&mut self, spell_id: u32, now: Instant, guards: bool) {
        self.0 = Some(PendingCastState {
            spell_id,
            deadline: now + SEND_PROVISIONAL,
            guards,
        });
    }

    /// Arm the guard on an **item** use's send. The reference has no separate item-cast slot: the
    /// item-use dispatcher's cast tail (`CGItem::Use 0x5d8d00` @ `0x5d9258`) calls `0x6e5a90`,
    /// whose entire 54-byte body is `call 0x6e4b60` — `TryCast` itself (the action bar's SPELL
    /// route takes the same `0x6e5a90` → `CastSpell 0x6e4b60`). So an item use writes
    /// the SAME inflight id (`0xceca88`) and is refused by the same IsCasting gate — which is why
    /// [`crate::ui_items::send_item_use`] runs both. Differs from [`PendingCast::arm`] only in the
    /// deadline ([`ITEM_SEND_PROVISIONAL`], whose doc is the why).
    pub(crate) fn arm_item(&mut self, spell_id: u32, now: Instant) {
        self.0 = Some(PendingCastState {
            spell_id,
            deadline: now + ITEM_SEND_PROVISIONAL,
            guards: true,
        });
    }

    /// Tighten the deadline to the server's real cast time once `SMSG_SPELL_START` names it.
    pub(crate) fn refine(&mut self, cast_time_ms: u32, now: Instant) {
        if let Some(p) = &mut self.0 {
            p.deadline = now + Duration::from_millis(u64::from(cast_time_ms)) + CAST_SLACK;
        }
    }

    /// Push the deadline out by a pushback (`SMSG_SPELL_DELAYED`), so the guard keeps holding past
    /// the stretched cast end. Extends from whichever is later (the current deadline or now), so a
    /// deadline already lapsed from an under-estimate still re-arms.
    pub(crate) fn delay(&mut self, delay_ms: u32, now: Instant) {
        if let Some(p) = &mut self.0 {
            p.deadline = p.deadline.max(now) + Duration::from_millis(u64::from(delay_ms));
        }
    }

    /// Clear the guard when the resolving spell is our outstanding one (spell-id-keyed, like the
    /// `Casting` reap — a triggered proc's `SMSG_SPELL_GO` mid-cast must not open the gate early).
    pub(crate) fn clear_if(&mut self, spell_id: u32) {
        if self.0.as_ref().is_some_and(|p| p.spell_id == spell_id) {
            self.0 = None;
        }
    }
}

/// Our own queued **on-next-swing** spell (Heroic Strike, Cleave — `Attributes & 0x404`) — the
/// melee-slot half of the client's cast tracking. In the reference the queued spell *occupies*
/// the inflight id (`0xceca88`) until the swing fires it, and the already-casting refusal at
/// `6e4d97` exempts new casts because the inflight rec has the `0x404` bits — so a queued Heroic
/// Strike never blocks Rend, and the new cast nests around it (`PushPopNestedCast 0x6e4ad0`
/// restores the queued id when the nested cast resolves). We model the
/// same observable with a second slot instead of the push/pop pair: [`PendingCast`] keeps
/// ordinary casts, this keeps the melee queue, and the checked ring reads both (the ref's
/// `IsCurrentAction 0x4e53a0` leg — spell == inflight — holds through the nesting either way, minus
/// the ref's one-RTT un-check blink while the nested cast displaces the slot).
///
/// **Deadline-less, wire-cleared** — like the ref's inflight id, which no timer touches. The
/// clear set is deliberately wider than the ref's: the ref clears inflight/saved ONLY on a
/// matching `SMSG_CAST_RESULT` (`0x6e7330`;
/// the GO handler never touches `0xceca88`) — but vmangos never sends an OK `CAST_RESULT` at
/// all, so on our wire the resolution is `SMSG_SPELL_GO` when the swing fires the strike, and a
/// failing `CAST_RESULT` + `SPELL_FAILED_OTHER` when it dies (vmangos `Spell::cancel` on the
/// PREPARING melee slot sends both — target death, manual cancel, replacement). We clear on any
/// of the three, id-keyed: identical observable where `0x130` does arrive, correct where it
/// never does. Re-arming replaces silently: the server holds a single `CURRENT_MELEE_SPELL`
/// slot. Re-pressing the queued spell itself is the ref's silent same-spell bail
/// (`6e4d43`: debug-log, `xor al,al`, no CMSG, no error): 1.12 has no re-press-to-unqueue. The
/// real un-queue is the StopAttack chain (`0x5ecac0` → `CancelQueuedCast 0x6e6f30`, sending
/// `CMSG_ATTACKSTOP` + `CMSG_CANCEL_CAST`), reached by /stopattack, the Attack-button toggle,
/// **an auto-repeat press** (`0x6e5976`), target change/death, mount, interact — never movement.
/// That chain is [`crate::creature_anim::stop_attack_local`], and the auto-repeat arm inside
/// [`crate::spell::cast_send`]'s commit runs it whole: starting Auto Shot un-queues the
/// strike, which is what darkens its checked ring. Every attack edge reaches that seam since
/// 1044 — the Attack toggle, a target switch, a click-off, the ring's death teardown.
///
/// **Escape is the other route, and it is not that chain at all** (1049). In the reference a
/// queued strike simply *is* the inflight spell, so `Script::SpellStopCasting 0x6e6e80`'s plain
/// `IsCasting 0x6e3d30` branch cancels it like any cast — no melee-specific code anywhere in it.
/// [`Inflight`] is where our two slots are re-joined for that reader.
#[derive(Resource, Default)]
pub(crate) struct QueuedMeleeSpell(Option<u32>);

impl QueuedMeleeSpell {
    /// The queued spell's id, if one is waiting on the next swing — what the checked ring and
    /// the re-press bail read.
    pub(crate) fn current(&self) -> Option<u32> {
        self.0
    }

    /// Queue a fresh on-next-swing send (replacing any prior queue — the server's single slot).
    pub(crate) fn arm(&mut self, spell_id: u32) {
        self.0 = Some(spell_id);
    }

    /// Clear on the wire resolution (`SMSG_SPELL_GO` / a failing `SMSG_CAST_RESULT` /
    /// `SMSG_SPELL_FAILED_OTHER`), spell-id-keyed like every reap.
    pub(crate) fn clear_if(&mut self, spell_id: u32) {
        if self.0 == Some(spell_id) {
            self.0 = None;
        }
    }
}

/// What the reference's single inflight spell id (`0xceca88`) holds — as our **two** slots see it.
///
/// The client has one slot, with one save beneath it for the nesting. An ordinary cast and a
/// queued on-next-swing strike both live in `0xceca88`, which is exactly why `IsCasting 0x6e3d30`
/// — a bare `ceca88 != 0` — answers true for either, and why `AbortCast 0x6e4940` cancels
/// whichever is there without ever asking which kind it was. [`PendingCast`] and
/// [`QueuedMeleeSpell`] split that one slot in two (its doc says why), which leaves every reader
/// of *"what is in flight"* to re-join them — and the one reader that forgot was the Esc ladder,
/// which is the bug decision 1049 fixes. So the join gets a name, once, here.
///
/// **The order is the reference's nesting, not a preference.** An ordinary cast started over a
/// queued strike is allowed through `TryCast`'s already-casting refusal precisely because the
/// inflight rec has `Attributes & 0x404` (`0x6e4d97`/`0x6e4dbe`), and it *pushes*
/// (`PushPopNestedCast 0x6e4ad0` @ `0x6e5026`): the cast becomes `0xceca88`, the strike the save
/// in `0xcecaa8`. `AbortCast` cancels the cast and pops the strike back up — so the cast dies on
/// the first Esc and the strike on the second. Reading `PendingCast` first reproduces that
/// without our needing the push/pop pair.
///
/// One knowing divergence there, weighed in decision 1058: the ref's pop (`0x6e4b2c`–`0x6e4b4e`)
/// *reads* `0xcecaa8` and never writes it, so a
/// second Esc re-asserts the same strike locally and only the server echo converges it. We have
/// no separate save slot to go stale, so ours clears. The two agree everywhere it shows: in the
/// un-nested case the ref's pop reads a zero save and clears too, and in the nested case vmangos
/// drops the melee slot on press **1** regardless (`HandleCancelCastOpcode` is spellId-blind
/// about `CURRENT_MELEE_SPELL`), so both clients converge off that echo. Reproducing the
/// re-assert would mean modelling `0xcecaa8` as a third slot to hold a value that is stale by
/// construction, for a window one localhost round trip wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Inflight {
    /// An ordinary cast — ours to send-guard, to paint a bar for, and to reap.
    Cast(u32),
    /// A queued on-next-swing strike waiting for the swing. No bar ever opened for it and it
    /// holds no [`Casting`], so cancelling it is the wire message and the slot, nothing else.
    Strike(u32),
}

impl Inflight {
    /// The spell id in the slot — what a cancel names on the wire, either way.
    pub(crate) fn spell_id(self) -> u32 {
        match self {
            Self::Cast(id) | Self::Strike(id) => id,
        }
    }
}

/// Resolve the reference's inflight slot from our two — see [`Inflight`] for the order and the
/// why. `started` is the self [`Casting`] spell id, which covers a cast that never armed the send
/// guard (an item use) exactly as the ref's one id covers it.
pub(crate) fn inflight(
    pending: &PendingCast,
    queued_melee: &QueuedMeleeSpell,
    started: Option<u32>,
    now: Instant,
) -> Option<Inflight> {
    pending
        .current(now)
        .or(started)
        .map(Inflight::Cast)
        .or_else(|| queued_melee.current().map(Inflight::Strike))
}

/// Slack past the server-named channel end, so a lost/late `MSG_CHANNEL_UPDATE(0)` can't pin the
/// checked ring on forever — the natural end clears through the deadline instead.
const CHANNEL_SLACK: Duration = Duration::from_secs(2);

/// Our own running channel — the app-side mirror of the client's current-channel id (`0xceac58`,
/// read live-gated by the channeling word: `IsCurrentAction 0x4e53a0`'s channel leg — a
/// channeled spell's button stays checked while its channel
/// runs). Set at `MSG_CHANNEL_START`, refreshed by nonzero `MSG_CHANNEL_UPDATE`s, cleared by the
/// `UPDATE(0)` that ends both the natural finish and the interrupt.
#[derive(Resource, Default)]
pub(crate) struct ActiveChannel(Option<(u32, Instant)>);

impl ActiveChannel {
    /// The running channel's spell id, if one is live.
    pub(crate) fn current(&self, now: Instant) -> Option<u32> {
        self.0
            .filter(|&(_, until)| now < until)
            .map(|(spell, _)| spell)
    }

    /// `MSG_CHANNEL_START`: open the window for the named duration (+ slack).
    pub(crate) fn start(&mut self, spell_id: u32, duration_ms: u32, now: Instant) {
        self.0 = Some((
            spell_id,
            now + Duration::from_millis(u64::from(duration_ms)) + CHANNEL_SLACK,
        ));
    }

    /// `MSG_CHANNEL_UPDATE`: `0` ends the channel; anything else re-times the window (pushback
    /// shortens a channel, so the deadline moves in as well as out).
    pub(crate) fn update(&mut self, remaining_ms: u32, now: Instant) {
        if remaining_ms == 0 {
            self.0 = None;
        } else if let Some((_, until)) = &mut self.0 {
            *until = now + Duration::from_millis(u64::from(remaining_ms)) + CHANNEL_SLACK;
        }
    }
}

/// The live auto-repeat spell — the client's autorepeat key `0xceac30` (written at the local
/// cast-send `0x6e54f0` for `AttributesEx2 & 0x20` spells, cleared by
/// `SMSG_CANCEL_AUTO_REPEAT`'s `0x6ea080` and by a matching cast-fail). Distinct from the sticky
/// `creature_anim::AutoRepeatArmed` (the Load/Hold idle gate, never cleared): THIS one is what
/// `IsAutoRepeatAction` and the button flash read, and it goes out when the shooting stops.
#[derive(Resource, Default)]
pub(crate) struct AutoRepeatActive(pub Option<u32>);

/// ── The local self-cancel (decisions 0256 open item 2 / 0444 / 0445): move/jump/Esc mid-cast
/// ends the cast **locally**, the same client tick — the app-side mirror of `AbortCast
/// 0x6e4940`. The whole trigger chain: the `Script::Move*` keybind handlers funnel into the
/// shared dispatcher
/// `0x515090`, whose interrupt mask `0x10f0` = {forward, backward, strafe L/R, autorun-toggle}
/// — turn (`0x100/0x200`) and pitch (`0x400/0x800`) are OUTSIDE the mask and never cancel —
/// and `Script::Jump 0x513bd0` inlines the same gate. Both call `AbortCast(cl=0, dl=1,
/// reason=0x1c)`: event **0x152 SPELLCAST_STOP** (the silent close — `cl=0` selects it;
/// `0x153/0x154` need `cl!=0`) + `CMSG_CANCEL_CAST`. The server stays the safety net: vmangos's
/// own movement interrupt (`Spell::update`'s 0.5-yd position delta) still kills anything we
/// miss. The per-spell gates, byte-verified at the call sites (`f6 46 54 01` @`0x51511e`,
/// `f6 42 5c 08` @`0x515175`; identities corroborated by vmangos `SpellDefines.h`):
/// (Also the entry test of the cast-INITIATION moving gate — `ui_action::state`'s
/// `cast_moving_refusal`, decision 0862 — which is a different mechanism from this self-cancel.)
pub(crate) const SPELL_INTERRUPT_MOVEMENT: u32 = 0x1; // SpellRec+0x54 InterruptFlags (SPELL_INTERRUPT_FLAG_MOVEMENT)
const CHANNEL_INTERRUPT_MOVING: u32 = 0x8; // SpellRec+0x5c ChannelInterruptFlags (AURA_INTERRUPT_MOVING_CANCELS)

/// The controller's report that a cancel-worthy local move edge happened this frame — a
/// *directional* start (forward/back/strafe: our wire-axis mirror of the verified `0x10f0`
/// mask's flags) or a jump takeoff. The mask's fifth member, the autorun toggle, is dormant
/// until benilla grows an autorun. Written by `player::control` at the same spot the
/// wire-axis transitions are computed, consumed (and cleared) by [`local_self_cancel`] next
/// frame — one engine frame (~16 ms) from key to bar-kill, vs the server round trip (~150 ms+:
/// the 0.5-yd delta has to accumulate before vmangos even notices).
#[derive(Resource, Default)]
pub(crate) struct LocalMoveStart(pub(crate) bool);

/// Move/jump/Esc mid-cast: run the client's local cancel. One system for both triggers so the
/// resolution can't drift between them:
///
/// - **the auto-repeat half (Esc only)** — the ref's branch order inside
///   `Script::SpellStopCasting 0x6e6e80`: a
///   running auto-repeat dies FIRST — [`crate::creature_anim::cancel_auto_repeat_local`], the
///   client's `0x6ea080` (key + idle gates + nocked ammo + `CMSG_CANCEL_AUTO_REPEAT_SPELL`)
///   — and SPENDS the press: one press stops one thing, the cast survives to the next.
///   Movement never touches auto-repeat.
/// - **the cast half** — clear [`PendingCast`], ship `CMSG_CANCEL_CAST`, reap the self
///   `Casting` state + fire the `Fail` cast event (the anim/visual teardown `AbortCast` does via
///   its stop-cast-anim tail), and push the **`Stop` + `Interrupted`** bar edges — the ref's
///   own two-step (its local STOP, then the echo's red repaint) at RTT→0: red "Interrupted",
///   held 1 s, the armed flash bursting white, then the fade (the transcribed `CastingBar.xml`
///   machinery runs the rest). Esc bypasses the movement flags gate (the ref's
///   `SpellStopCasting → AbortCast` has no `InterruptFlags` test — the gate belongs to the
///   movement path alone).
/// - **the channel half — MOVEMENT-ONLY** — ship `CMSG_CANCEL_CHANNELLING` and NOTHING else:
///   the verified asymmetry. Esc never reaches a channel — `0x6e6e80`'s whole callee
///   closure never calls the channel canceler `0x6e9b70`, and its inflight gate (`0xceca88`)
///   is already 0 mid-channel (the launch `CAST_RESULT(OKAY)` clears it at `0x6e7408`): the
///   vanilla "/stopcasting can't stop a channel" quirk, kept faithfully. The ref's
///   `0x6e9b70` fires no FrameScript event and clears no
///   local state — the channel bar closes on the server's `SMSG_CHANNEL_UPDATE(0)`
///   (`0x6e75f0`), which also clears [`ActiveChannel`] through the normal wire path (its slack
///   deadline self-heals a lost packet).
///
/// Why `Interrupted` when the client's own event is VERIFIED as the silent `0x152` STOP (every
/// self-cancel caller passes `cl=0`)? Because the REF'S NET OBSERVABLE is the red bar — the
/// director's falsifier, decision 0449, now pinned end to end: vmangos `Spell::cancel`
/// answers the cancel with `SMSG_SPELL_FAILED_OTHER` and then a failing `SMSG_CAST_RESULT`
/// reason 0x23 (`SPELL_FAILED_INTERRUPTED`, counted against the 1.12.1 build guards), the
/// client routes a failing result through `HandleCastFailed 0x6e1a00`, and reasons
/// {0x23,0x24} select FrameScript **0x154 SPELLCAST_INTERRUPTED** — red, "Interrupted"
/// (GlobalStrings-exact), `holdTime = now + 1 s`. Our bar-edge producers are KEYED to the
/// self `Casting` (the 0137/0256 law that stops a proc's failure repainting the bar), and
/// this cancel's reap removes it — so the echo can't do the repainting here. The local
/// `Stop` + `Interrupted` pair replays the ref's exact sequence at RTT→0 through our keyed
/// architecture; the reap then keeps the echo idempotent (re-clears open guards, shows the
/// red error line, touches no bar). On-screen: the 1 s hold + ~25 bar ticks of flash+fade —
/// ~1.9 s in the reference bottle (whose `Config.wtf` caps `maxfps` at 30, and the ref steps
/// alpha PER TICK), the director's observed "2–3 s"; our transcription normalizes those steps
/// to the same 30 Hz reference tick so the tail matches at any render rate (`CastingBar.xml`).
pub(super) fn local_self_cancel(
    script: Option<NonSendMut<UiScript>>,
    mut moved: ResMut<LocalMoveStart>,
    mut pending: ResMut<PendingCast>,
    mut queued_melee: ResMut<QueuedMeleeSpell>,
    channel: Res<ActiveChannel>,
    mut auto_repeat: ResMut<AutoRepeatActive>,
    mut feed: ResMut<CastBarFeed>,
    spells: Option<Res<Spells>>,
    net: Res<NetCommands>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    casting: Query<&Casting>,
    mut cast_events: MessageWriter<CastEvent>,
    mut play_seq: ResMut<PlaySeq>,
    mut ecs: Commands,
) {
    let esc = match script {
        Some(mut s) => s.take_spell_stop(),
        None => false,
    };
    let moved = std::mem::take(&mut moved.0);
    if !esc && !moved {
        return;
    }
    let now = Instant::now();
    let self_e = self_guid.0.as_ref().and_then(|g| index.0.get(g)).copied();
    // The Esc press stops ONE thing, auto-repeat first — the ref's branch order (`0x6e6e80`).
    let esc = if esc && auto_repeat.0.is_some() {
        if *crate::net::CAST_TRACE {
            info!("cast-trace: LOCAL auto-repeat self-cancel (esc), CMSG_CANCEL_AUTO_REPEAT_SPELL");
        }
        crate::creature_anim::cancel_auto_repeat_local(self_e, &mut auto_repeat, &mut ecs, &net);
        false // the press is spent
    } else {
        esc
    };
    if !esc && !moved {
        return;
    }
    let flags_open = |pick: fn(&benilla_formats::SpellDisplay) -> bool, spell_id: u32| {
        // An uncataloged spell cancels: the gate exists to spare the rare cast-while-moving
        // spell, and failing open matches the server's own verdict for everything ordinary.
        spells
            .as_ref()
            .and_then(|s| s.catalog.get(spell_id))
            .is_none_or(pick)
    };
    // The cast half — the ref's ONE inflight slot ([`Inflight`]), which is an ordinary cast or a
    // queued on-next-swing strike. `AbortCast` doesn't distinguish, so neither does the send or
    // the movement gate; only the local teardown differs, because only a cast ever had a bar.
    let started = self_e.and_then(|e| casting.get(e).ok()).map(|c| c.spell_id);
    if let Some(slot) = inflight(&pending, &queued_melee, started, now) {
        let spell_id = slot.spell_id();
        // Esc bypasses the flags gate; movement consults the slot's own `InterruptFlags`, which
        // is what spares a queued strike (Heroic Strike 78, Cleave 845, Raptor Strike 2973 all
        // ship `InterruptFlags = 0x0` — verified against Spell.dbc, vs Fireball's `0xf`). So
        // charging in with a strike queued keeps it queued, and only Esc takes it.
        if esc
            || flags_open(
                |d| d.interrupt_flags & SPELL_INTERRUPT_MOVEMENT != 0,
                spell_id,
            )
        {
            if *crate::net::CAST_TRACE {
                info!(
                    "cast-trace: LOCAL self-cancel — {slot:?} ({}), CMSG_CANCEL_CAST",
                    if esc { "esc" } else { "moved" }
                );
            }
            let _ = net.0.send(ClientCommand::CancelCast { spell_id });
            match slot {
                Inflight::Cast(_) => {
                    pending.clear_if(spell_id);
                    // The ref's two-step at RTT→0: STOP arms the bar's flash overlay, the echo's
                    // INTERRUPTED repaints it red and starts the hold — so after the hold the
                    // flash BURSTS white over the red bar, then the fade runs. One edge alone
                    // misses the burst (which the enemy-interrupt path — no prior STOP —
                    // correctly lacks).
                    feed.0.push(CastBarEdge::Stop);
                    feed.0.push(CastBarEdge::Interrupted);
                    // The reap — `spell_failed_other`'s self arm, run early so the echo finds it
                    // done.
                    if let Some(e) = self_e {
                        if casting.get(e).is_ok_and(|c| c.spell_id == spell_id) {
                            ecs.entity(e).remove::<Casting>();
                        }
                        cast_events.write(CastEvent {
                            entity: e,
                            spell_id,
                            kind: CastEventKind::Fail,
                            seq: play_seq.next(),
                        });
                    }
                }
                Inflight::Strike(_) => {
                    // Clearing the slot IS the un-toggle: the checked ring reads it
                    // (`ui_action::state`), so the button darkens this frame instead of waiting
                    // for vmangos's FAILED_OTHER + CAST_RESULT echo (which then finds the slot
                    // already open and is idempotent). `AbortCast` fires its `0x152`
                    // SPELLCAST_STOP here too — inert on a bar that never opened
                    // (`CastingBar.xml` guards every arm on `IsShown`), so it is fired for
                    // fidelity, not effect. No red INTERRUPTED: there is nothing to repaint.
                    queued_melee.clear_if(spell_id);
                    feed.0.push(CastBarEdge::Stop);
                }
            }
        }
    }
    // The channel half — MOVEMENT-ONLY (Esc can't stop a channel: the doc above / 0454), and
    // the send and nothing else (the verified asymmetry: the ref's `0x6e9b70` fires no event
    // and clears no state; the bar and [`ActiveChannel`] both close on the server's
    // `SMSG_CHANNEL_UPDATE(0)`, ~one localhost frame later).
    if moved {
        if let Some(spell_id) = channel.current(now) {
            if flags_open(
                |d| d.channel_interrupt_flags & CHANNEL_INTERRUPT_MOVING != 0,
                spell_id,
            ) {
                if *crate::net::CAST_TRACE {
                    info!("cast-trace: LOCAL channel self-cancel — spell {spell_id}, CMSG_CANCEL_CHANNELLING");
                }
                let _ = net.0.send(ClientCommand::CancelChannelling { spell_id });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREBALL: u32 = 133;

    /// The suppressed channel still arms [`ActiveChannel`], so its action button stays lit: the
    /// reference's channel mirror is `SPELLCAST+0x10` (`0xceac58`), written at the cast SEND
    /// (`0x6e4f88`) and never touched by `0x6e7550`. That is why all four of the handler's gates
    /// live in the DRAIN and none of them in `super::net`'s `channel_start`, which arms this.
    #[test]
    fn suppressing_the_channel_bar_does_not_clear_the_channel_mirror() {
        let t0 = Instant::now();
        // What `MSG_CHANNEL_START` does for Blood Siphon 24322 — the suppressed row.
        let mut channel = ActiveChannel::default();
        channel.start(24322, 8_000, t0);
        assert_eq!(
            channel.current(t0 + Duration::from_secs(1)),
            Some(24322),
            "the wire edge arms the mirror regardless of what the bar does with it"
        );
    }

    #[test]
    fn a_fresh_guard_is_open() {
        let g = PendingCast::default();
        assert!(
            !g.in_flight(Instant::now()),
            "nothing sent yet — casting allowed"
        );
    }

    #[test]
    fn arming_closes_the_guard_and_a_matching_resolution_opens_it() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true);
        assert!(
            g.in_flight(t0),
            "a cast is in flight — a second one is refused"
        );
        g.clear_if(FIREBALL); // SMSG_SPELL_GO / a failing SMSG_CAST_RESULT for our cast
        assert!(
            !g.in_flight(t0),
            "our cast resolved — casting allowed again"
        );
    }

    #[test]
    fn a_different_spells_resolution_leaves_the_guard_closed() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true);
        g.clear_if(FIREBALL + 1); // a triggered proc's GO for some other spell mid-cast
        assert!(
            g.in_flight(t0),
            "a different spell's resolution must not open the guard on our cast"
        );
    }

    #[test]
    fn the_guard_opens_when_its_safety_deadline_lapses() {
        // The net that saves us if a cast resolves with no packet (a local move-cancel we don't
        // model): the guard cannot wedge casting shut forever.
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true);
        assert!(g.in_flight(t0 + SEND_PROVISIONAL - Duration::from_secs(1)));
        assert!(!g.in_flight(t0 + SEND_PROVISIONAL + Duration::from_secs(1)));
    }

    #[test]
    fn a_pushback_extends_the_guard_so_it_holds_past_the_stretched_cast() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true);
        g.refine(1_000, t0); // a 1 s cast: deadline t0 + 1s + 2s slack = t0 + 3s
        g.delay(500, t0); // a hit at t0 pushes it +0.5s ⇒ t0 + 3.5s
        assert!(
            g.in_flight(t0 + Duration::from_millis(3_200)),
            "the pushback kept the guard shut past the original refined deadline"
        );
    }

    #[test]
    fn spell_start_tightens_the_deadline_to_the_real_cast_time() {
        let t0 = Instant::now();
        let mut g = PendingCast::default();
        g.arm(FIREBALL, t0, true); // provisional 5s window
        g.refine(1_500, t0); // SMSG_SPELL_START: a 1.5s cast (+ 2s pushback slack ⇒ 3.5s)
        let at_four = t0 + Duration::from_secs(4);
        assert!(
            !g.in_flight(at_four),
            "refined to 1.5s + slack, the guard has opened by t+4s (it would still be shut at the \
             5s provisional)"
        );
    }

    /// The queued on-next-swing slot: deadline-less (no clock in its API at all), keyed clears
    /// only, and a re-arm replaces — the server's single `CURRENT_MELEE_SPELL` slot.
    #[test]
    fn queued_melee_spell_is_wire_cleared_and_single_slot() {
        const HEROIC_STRIKE: u32 = 78;
        const CLEAVE: u32 = 845;
        let mut q = QueuedMeleeSpell::default();
        assert_eq!(q.current(), None);

        q.arm(HEROIC_STRIKE);
        assert_eq!(q.current(), Some(HEROIC_STRIKE), "queued and checked");

        // A different spell's resolution (Rend's GO while HS waits) must not open the queue.
        q.clear_if(772);
        assert_eq!(q.current(), Some(HEROIC_STRIKE));

        // A second on-next-swing cast replaces the queue (vmangos holds ONE melee slot).
        q.arm(CLEAVE);
        assert_eq!(q.current(), Some(CLEAVE));

        // The swing fired it (SPELL_GO) — or the slot died (failing CAST_RESULT /
        // SPELL_FAILED_OTHER): the matching id clears.
        q.clear_if(CLEAVE);
        assert_eq!(q.current(), None, "the wire resolution opened the queue");
    }

    /// The local self-cancel harness: a mini-App with the real [`local_self_cancel`] system, a
    /// self player mid-cast/mid-channel, and the net channel's receive half to assert the wire.
    /// The harness registers no `UiScript`, so the ESC trigger reads `false` by default; the
    /// ESC-leg tests insert a real VM and drive the actual `SpellStopCasting()` binding (the
    /// ladder's 1/nil semantics are pinned Lua-side in `ui_script::escape_tests`).
    mod local_cancel {
        use super::*;
        use crate::creature_anim::PlaySeq;
        use crate::net::{Guid, GuidIndex, NetCommands, SelfGuid, SelfPlayer};
        use crate::ui_cast::feed_cast_bar;
        use benilla_formats::{SpellCatalog, SpellDisplay};
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
                .init_resource::<CastBarFeed>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<ActiveChannel>()
                .init_resource::<AutoRepeatActive>()
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

        /// Mark the self player mid-cast on `spell_id` (the `SMSG_SPELL_START` echo's `Casting`).
        fn mark_casting(app: &mut App, spell_id: u32) {
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            app.world_mut().entity_mut(self_e).insert(Casting {
                spell_id,
                until: None,
            });
        }

        fn run(app: &mut App, moved: bool) {
            app.world_mut().resource_mut::<LocalMoveStart>().0 = moved;
            app.world_mut().run_system_once(local_self_cancel).unwrap();
        }

        /// The headline feel fix: a directional start / jump mid-cast kills the cast locally —
        /// wire cancel out, guard open, the STOP + INTERRUPTED bar-edge pair (the ref's own
        /// two-step: its silent 0x152 STOP arms the flash, the server echo's repaint holds the
        /// red "Interrupted"; our keyed reap suppresses the real echo, so the local pair
        /// replays the sequence at RTT→0), `Casting` reaped — instead of
        /// waiting ~150 ms+ for the server's 0.5-yd position-delta interrupt.
        #[test]
        fn a_move_edge_mid_cast_cancels_locally() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf, // the ordinary timed-cast mask (movement bit set)
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);
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
                !app.world()
                    .resource::<PendingCast>()
                    .in_flight(Instant::now()),
                "the in-flight guard opens with the local cancel"
            );
            let feed = &app.world().resource::<CastBarFeed>().0;
            assert_eq!(feed.len(), 2);
            assert!(
                matches!(
                    (&feed[0], &feed[1]),
                    (CastBarEdge::Stop, CastBarEdge::Interrupted)
                ),
                "the bar edges are the ref's own two-step at RTT→0 — STOP (arms the flash \
                 overlay) then the red INTERRUPTED (the echo's repaint: hold, burst, fade); \
                 the keyed reap silences the real echo (0449/0454)"
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

        /// The per-spell gate (VERIFIED — `test [SpellRec+0x54],1` at both call sites): a spell
        /// whose `InterruptFlags` lacks the movement bit keeps casting while we move — no wire
        /// cancel, no bar edge.
        #[test]
        fn the_interrupt_flags_gate_spares_a_movement_castable_spell() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xe, // movement bit CLEAR (pushback/stun/combat only)
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);

            run(&mut app, true);

            assert!(rx.try_recv().is_err(), "no wire cancel for a spared spell");
            assert!(
                app.world()
                    .resource::<PendingCast>()
                    .in_flight(Instant::now()),
                "the guard keeps holding — the cast runs on"
            );
            assert!(app.world().resource::<CastBarFeed>().0.is_empty());
        }

        /// A spell the catalog doesn't know fails OPEN (cancels): the gate exists to spare the
        /// rare cast-while-moving spell, and the server's own verdict for everything ordinary is
        /// to interrupt.
        #[test]
        fn an_uncataloged_spell_fails_open() {
            let (mut app, rx) = harness(HashMap::new());
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "an unknown spell's cast still cancels on movement"
            );
        }

        /// The union's other half: an ITEM use's cast (a hearthstone) never arms the send
        /// guard — the started self `Casting` (the `SMSG_SPELL_START` echo) alone makes it
        /// move-cancelable, mirroring the ref's ONE inflight id (`0xceca88`), which item casts
        /// set like any spell cast.
        #[test]
        fn a_started_item_cast_cancels_without_the_send_guard() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf,
                    ..Default::default()
                },
            )]));
            mark_casting(&mut app, FIREBALL); // PendingCast stays empty — item casts never arm it

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "the started cast cancels through the `Casting` half of the union"
            );
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            assert!(app.world().entity(self_e).get::<Casting>().is_none());
        }

        /// No move edge, cast in flight: nothing happens — the system is edge-triggered, never
        /// level-triggered (holding W through a whole cast is one cancel at the start, not one
        /// per frame; and standing still cancels nothing).
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
                .arm(FIREBALL, Instant::now(), true);

            run(&mut app, false);

            assert!(rx.try_recv().is_err());
            assert!(app
                .world()
                .resource::<PendingCast>()
                .in_flight(Instant::now()));
            assert!(app.world().resource::<CastBarFeed>().0.is_empty());
        }

        /// Esc CANNOT stop a channel — `0x6e6e80` never reaches the channel
        /// canceler `0x6e9b70`, and its inflight gate is already 0 mid-channel: the
        /// mirror answers `SpellStopCasting()` nil (the
        /// ladder falls through to the next rung) and the drain ships nothing.
        #[test]
        fn esc_cannot_stop_a_channel() {
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            let now = Instant::now();
            app.world_mut()
                .resource_mut::<ActiveChannel>()
                .start(ARCANE_MISSILES, 5_000, now);

            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                None,
                "mid-channel the binding answers nil — the vanilla /stopcasting quirk"
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(
                rx.try_recv().is_err(),
                "no wire cancel — channels break only on the movement path"
            );
            assert_eq!(
                app.world().resource::<ActiveChannel>().current(now),
                Some(ARCANE_MISSILES)
            );
        }

        /// The ref's branch order inside `SpellStopCasting` (`0x6e6e80`): a running auto-repeat
        /// dies FIRST — `CMSG_CANCEL_AUTO_REPEAT_SPELL`, no bar edge — and SPENDS the press;
        /// the in-flight cast survives to the next one. One press stops one thing.
        #[test]
        fn esc_cancels_the_auto_repeat_first_and_the_cast_on_the_next_press() {
            const AUTO_SHOT: u32 = 75;
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut().resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);
            mark_casting(&mut app, FIREBALL);

            // Press 1: truthy mirror (auto-repeat ∪ cast) → trigger queued → the drain stops
            // the auto-repeat ONLY.
            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelAutoRepeat)
                ),
                "the auto-repeat cancel ships first — the ref's priority branch"
            );
            assert!(rx.try_recv().is_err(), "the cast survives the first press");
            assert!(app.world().resource::<AutoRepeatActive>().0.is_none());
            assert!(
                app.world()
                    .resource::<PendingCast>()
                    .in_flight(Instant::now()),
                "the send guard still holds — one press stopped one thing"
            );
            assert!(
                app.world().resource::<CastBarFeed>().0.is_empty(),
                "an auto-repeat cancel paints no bar edge"
            );

            // Press 2: the auto-repeat is gone — now the cast dies, the ordinary Esc cancel.
            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast { spell_id: FIREBALL })
                ),
                "the second press reaches the cast half"
            );
        }

        /// **The director's report, decision 1049: Esc with Heroic Strike queued un-toggles the
        /// STRIKE and keeps the target.** In the reference a queued on-next-swing spell simply
        /// *is* the inflight spell (`0xceca88`), so `SpellStopCasting 0x6e6e80` takes its plain
        /// `IsCasting 0x6e3d30` branch — `AbortCast` → `CMSG_CANCEL_CAST` — and returns `1`,
        /// which EATS the press so `ToggleGameMenu`'s ladder never reaches `ClearTarget()`. We
        /// split that one slot in two and the Esc reader saw only the cast half, so the binding
        /// answered nil: the strike stayed lit *and* the target dropped, both from one cause.
        #[test]
        fn esc_unqueues_a_strike_and_eats_the_press() {
            let (mut app, rx) = harness(HashMap::from([(
                HEROIC_STRIKE,
                SpellDisplay {
                    attributes: 0x5_0014, // the shipped row: on-next-swing (`& 0x404`)
                    interrupt_flags: 0,   // …and no movement bit, also shipped
                    ..Default::default()
                },
            )]));
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE);

            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1),
                "the queued strike makes IsCasting true, so the binding EATS the press — which \
                 is what spares the target three rungs further down the ladder"
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
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
                    app.world().resource::<CastBarFeed>().0[..],
                    [CastBarEdge::Stop]
                ),
                "the ref's `0x152` STOP and nothing else: no red INTERRUPTED for a spell that \
                 never opened a bar"
            );
        }

        /// …but MOVEMENT does not take it. The movement path consults the inflight spell's own
        /// `InterruptFlags`, and Heroic Strike 78 / Cleave 845 / Raptor Strike 2973 all ship
        /// `InterruptFlags = 0x0` (read off the shipped `Spell.dbc`; Fireball is `0xf`). So
        /// charging in with a strike queued keeps it queued — Esc is the only local taker.
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

        /// The nesting order (`Inflight`'s doc): an ordinary cast started over a queued strike
        /// pushes it down (`PushPopNestedCast 0x6e4ad0`), so `0xceca88` holds the CAST. One Esc
        /// takes the cast; `AbortCast`'s pop restores the strike, and the next Esc takes that.
        /// One press, one thing — the ladder's law all the way down.
        ///
        /// This is the **local** law, which is all a harness with no server can show, and it is
        /// the ref's local law too — with the one knowing
        /// difference `Inflight`'s doc names: the ref's press 2 re-asserts the strike instead of
        /// clearing it. On a live wire neither client gets this far, because vmangos drops the
        /// melee slot on press **1** regardless of the id in the packet, and both converge off
        /// that echo. So what this pins is the ordering, not a two-press ritual a player sees.
        #[test]
        fn esc_takes_the_cast_first_and_the_strike_on_the_next_press() {
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE);
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, Instant::now(), true);
            mark_casting(&mut app, FIREBALL);

            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1)
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
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

            // Press 2 — the pop put the strike back in the slot.
            app.world_mut().run_system_once(feed_cast_bar).unwrap();
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<Option<i64>>("return SpellStopCasting()")
                    .unwrap(),
                Some(1),
                "still stoppable — so this press is eaten too, and the target still survives"
            );
            app.world_mut().run_system_once(local_self_cancel).unwrap();
            assert!(matches!(
                rx.try_recv(),
                Ok(crate::net::ClientCommand::CancelCast {
                    spell_id: HEROIC_STRIKE
                })
            ));
            assert_eq!(app.world().resource::<QueuedMeleeSpell>().current(), None);
        }

        /// The channel half is the SEND and nothing else — the verified asymmetry: the
        /// ref's `0x6e9b70` ships CMSG_CANCEL_CHANNELLING but fires no FrameScript event and
        /// clears no state; the channel bar and the mirror both close on the server's
        /// `SMSG_CHANNEL_UPDATE(0)` through the normal wire path.
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
                app.world().resource::<CastBarFeed>().0.is_empty(),
                "no local bar edge — the bar closes on the wire's SPELLCAST_CHANNEL_STOP"
            );
        }
    }

    /// The channel mirror's lifecycle: START opens the window for the whole duration, a nonzero
    /// UPDATE re-times it (pushback shortens channels), UPDATE(0) ends it — and the slack deadline
    /// self-clears a channel whose closing packet never arrived.
    #[test]
    fn active_channel_tracks_the_wire_lifecycle() {
        let t0 = Instant::now();
        let mut ch = ActiveChannel::default();
        assert_eq!(ch.current(t0), None);

        ch.start(10, 5_000, t0); // Arcane Missiles-shaped: 5 s channel
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

        // Pushback: an update naming LESS time moves the deadline in.
        ch.start(10, 5_000, t0);
        ch.update(1_000, t0);
        assert_eq!(ch.current(t0 + Duration::from_secs(4)), None);

        // The closing update ends it immediately.
        ch.start(10, 5_000, t0);
        ch.update(0, t0);
        assert_eq!(ch.current(t0), None);
    }
}
