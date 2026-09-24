//! **The melee swing refusal** — what the client does with the server's answers to a
//! `CMSG_ATTACKSWING` it will not honour.
//!
//! **Six opcodes, FOUR arms.** `0x145` NOTINRANGE and `0x146` BADFACING take one each; `0x148`
//! DEADTARGET and `0x149` CANT_ATTACK share arm 4 verbatim; `0x147` NOTSTANDING is never wired at
//! all; and `0x14e` `SMSG_CANCEL_COMBAT` — the forced cancel, registered by the *spell* TU
//! (`0x5e3308` → handler `0x5e7dd0`), which is why `0x625520`'s eight-opcode census never names it
//! — is arm 4's body **byte-identical**, down to the `__LINE__` it pushes. So the whole family is
//! two arms that latch a message and two that silently stop the attack.
//!
//! The server refuses a swing on the **edge** alone (vmangos `Unit::AttackerStateUpdate`'s
//! `swingError` switch, `Objects/Unit.cpp:405-475`: each arm sends only when
//! `GetLastSwingErrorMsg() != <result>`), so a player standing out of range gets **one** packet,
//! not one per swing timer. The repeat the player actually sees is the client's own, and it is a
//! **latch plus a cooldown**, never a per-packet toast:
//!
//! ```text
//! 0x625a8a  (0x145 NOTINRANGE)  ecx = 1 ; call 0x5ecdb0     ; latch code 1
//! 0x625aa1  (0x146 BADFACING)   ecx = 2 ; call 0x5ecdb0     ; latch code 2
//! 0x625ab8  (0x148 DEADTARGET / 0x149 CANT_ATTACK)          ; NO message — StopAttack, nothing else
//!
//! 0x5ecdb0  [0xc4d758] = code ; [0xc4c270] = OsGetAsyncTimeMs()    ; the code + "show at"
//!
//! 0x5ec950  the local player's tick:
//!             this is the active player      (0x468550)      else out
//!             [this+0xc48] != 0              (0x47bf60)      else out   ; an attack target is set
//!             now - [0xc4c270] >= 0          (5ec999 js)     else out
//!             [0xc4d758] == 1 -> DisplayError(0xd8) | == 2 -> DisplayError(0xd7) | else out
//!             [0xc4c270] = now + 0xfa0                                  ; re-arm, 4000 ms
//! ```
//!
//! Three consequences, all of them visible, none of them guessable from the packet alone:
//!
//! 1. **The first line is immediate** — the latch writes `now`, and the tick's test is
//!    `now - stored >= 0`, so the very next tick shows it.
//! 2. **It repeats every 4 s while the refusal stands**, because nothing about the *packet*
//!    drives the display; the latch does, and it survives until something clears it.
//! 3. **A dead or unattackable target says nothing at all.** `0x148` and `0x149` share arm 4
//!    verbatim — the client cannot even tell them apart — and that arm raises no message on any
//!    surface: it resolves the active player and calls StopAttack
//!    ([`crate::creature_anim::stop_attack_local`]) on it. Which is right: the target is dead or
//!    friendly, the attack ends, and the reason is already on screen from elsewhere.
//!
//! **What clears the latch is a landed swing.** `SMSG_ATTACKERSTATEUPDATE`'s arm calls `0x5ea800`
//! (at `0x6259b6`) when the attacker **is the active player** (`0x5fa6d0`, a guid compare against
//! `0x468550`) and the **victim resolves** as a streamed unit — and `0x5ea800`'s first act, ahead
//! of everything else it does, is `0x5ecdb0(0)`. So the moment a swing connects the stale "too far
//! away" stops repeating; while none does, it keeps saying so. `0x5e2510`, a module init, clears
//! it the same way.
//!
//! Both edges ride ONE message ([`SwingRefusalEdge`]) written by two arms of the same net drain,
//! so they are consumed in **packet order** — the reference processes them in one synchronous
//! dispatch, and a refusal and a landing that arrive in the same frame must not be reordered.
//!
//! Two consequences of the latch being written **unconditionally** — both reproduced here, both
//! easy to lose in a refactor:
//!
//! - a **second** refusal packet resets the deadline to `now`, so it **defeats the 4 s cooldown**
//!   and re-shows on the next tick; a NOTINRANGE→BADFACING flip shows at once for the same reason;
//! - the **re-arm precedes the print** (`0x5ec9bc` then `0x5ec9c2`), so the 4 s advance happens
//!   even when the line is then swallowed by a missing GlobalString.
//!
//! `SMSG_ATTACKSWING_NOTSTANDING` (`0x147`) is in neither this module nor
//! [`benilla_protocol::messages::opcode`]: the reference never registers it, and vmangos never
//! sends it (`Player::SendAttackSwingNotStanding` has zero callers).
//!
//! Decision 2037.

use std::time::Duration;

use benilla_protocol::messages::AttackSwingError;
use bevy::prelude::*;

use crate::creature_anim::{AttackSeam, Engaged};
use crate::net::SelfPlayer;
use crate::ui_action::{UiError, UiErrorKeys};

/// The reference's re-arm interval — `0x5ec9b6 add ecx,0xfa0`.
const REPEAT: Duration = Duration::from_millis(0xfa0);

/// One edge of the swing-refusal seam, off the net drain in **packet order** — see the module
/// header for why they share a message rather than taking one each.
#[derive(Message, Clone, Copy, Debug)]
pub(crate) enum SwingRefusalEdge {
    /// The server refused our swing (`0x145`/`0x146`/`0x148`/`0x149`).
    Refused(AttackSwingError),
    /// `SMSG_CANCEL_COMBAT` (`0x14e`) — the server forced our attack to stop
    /// (`Unit::CombatStop`, `Unit::StopAttackFaction`, `Unit::InterruptAttacksOnMe`, a resisted
    /// feign death). Its own message rather than a fifth `AttackSwingError`, because it is a
    /// different opcode family; that its ARM is the same one is the finding, and it is expressed
    /// where the arms are dispatched.
    CombatCancelled,
    /// One of OUR swings landed on a victim we can see — arm 5's `0x5ea800` call at `0x6259b6`,
    /// whose first act is the latch clear. **Both** of the reference's gates are applied at the
    /// write site, which is the only place holding the guids: GATE A, the attacker is the local
    /// player (`0x6259a8 test al,al; je`, `0x5fa6d0`), and GATE B, the victim resolves
    /// (`0x6259b1 test eax,eax; je`). The leg is entered by fall-through alone — 0 branches into
    /// `[0x6259ac, 0x6259b7]`, 1 rel32 caller of `0x5ea800`, 0 address-takes.
    ///
    /// GATE B is exactly `0x468460(ecx = 8)`, i.e. **"resolves, as a UNIT"** — its two zero
    /// returns are "guid absent from the object index" and "resolved but the type mask misses the
    /// bit". benilla's index lookup covers the first and not the second, and the gap is
    /// unreachable: this packet's victim is a unit or a player server-side, and a player's mask
    /// carries the UNIT bit too. Stated rather than tightened, because the sibling uses of the
    /// same lookup two lines away (the full-block synthesis, the impact fallback) ask the index
    /// the same question, and one arm asking it differently would read as a distinction.
    Landed,
}

/// The latched refusal — the reference's `[0xc4d758]` (which code) and `[0xc4c270]` (when the next
/// line is due).
///
/// **One `Option` over the pair**, not two fields, because the reference's timestamp has no meaning
/// while its code is `0`: `0x5ecdb0` writes both in the same breath, and the tick reads the second
/// only after the first has passed its `cmp eax,1`/`cmp eax,2`. `None` IS code `0` — the value both
/// clear sites push, and the value the tick falls through on. The `Duration` is an absolute time,
/// not a countdown, exactly as the reference stores it.
///
/// A re-implementation modelling this as `(Option<Code>, Instant)` must not assume the
/// timestamp is zero when the code is — the reference's clear writes `now`, not `0`. Pairing them
/// inside the `Option` is how that warning is answered rather than obeyed: there is no timestamp
/// to be wrong about while the code is absent, and every transition into `Some` writes a fresh
/// one.
#[derive(Resource, Default)]
struct SwingRefusal(Option<(Latched, Duration)>);

/// The two refusals that latch. [`AttackSwingError::DeadOrUnattackable`] is deliberately absent:
/// arm 4 writes no latch, so there is no third code to hold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Latched {
    /// Code `1` → `DisplayError(0xd8)` = `ERR_BADATTACKPOS`, "You are too far away!".
    NotInRange,
    /// Code `2` → `DisplayError(0xd7)` = `ERR_BADATTACKFACING`, "You are facing the wrong way!".
    BadFacing,
}

impl Latched {
    /// The `GlobalStrings.lua` key the reference's error id resolves to — catalog rows `0xd8`/
    /// `0xd7` ([`benilla_ui::messages`]) — so the text and its localization come out of the
    /// player's own file like every other line.
    const fn key(self) -> &'static str {
        match self {
            Self::NotInRange => "ERR_BADATTACKPOS",
            Self::BadFacing => "ERR_BADATTACKFACING",
        }
    }
}

impl SwingRefusal {
    /// `0x5ecdb0(ecx = code)` — latch the refusal and make the next line due **now**.
    fn latch(&mut self, code: Latched, now: Duration) {
        self.0 = Some((code, now));
    }

    /// `0x5ecdb0(ecx = 0)` — the clear both of the reference's zero sites push.
    fn clear(&mut self) {
        self.0 = None;
    }
}

/// Consume this frame's edges in packet order: latch a refusal, clear on a landing, and run the
/// shared `0x148`/`0x149` arm's one and only act.
///
/// The StopAttack lives here rather than in the net drain because [`AttackSeam`] is the whole
/// write set `0x5ecac0` needs, and because putting it beside the latch keeps arm 4's "stop, and
/// say nothing" legible as one fact.
fn apply_swing_refusals(
    mut edges: MessageReader<SwingRefusalEdge>,
    time: Res<Time>,
    mut refusal: ResMut<SwingRefusal>,
    mut seam: AttackSeam,
    engaged: Query<(), (With<SelfPlayer>, With<Engaged>)>,
) {
    for edge in edges.read() {
        match edge {
            SwingRefusalEdge::Refused(AttackSwingError::NotInRange) => {
                debug!("swing refused: out of range");
                refusal.latch(Latched::NotInRange, time.elapsed());
            }
            SwingRefusalEdge::Refused(AttackSwingError::BadFacing) => {
                debug!("swing refused: bad facing");
                refusal.latch(Latched::BadFacing, time.elapsed());
            }
            // The two SILENT arms — arm 4 (`0x625ab8`, for `0x148`/`0x149`) and
            // `SMSG_CANCEL_COMBAT`'s own `0x5e7dd0`, whose bodies are byte-identical. No latch, no
            // message on any surface; only the stop, and `0x5ecac0`'s own engaged guard decides
            // whether even that sends anything.
            SwingRefusalEdge::Refused(AttackSwingError::DeadOrUnattackable)
            | SwingRefusalEdge::CombatCancelled => {
                debug!("swing stopped by the server — silently ({edge:?})");
                seam.stop(!engaged.is_empty());
            }
            SwingRefusalEdge::Landed => refusal.clear(),
        }
    }
}

/// The reference's per-player tick `0x5ec950` — the whole display side of this seam.
///
/// Every gate is the reference's, in its order: the active player, an attack target set
/// ([`Engaged`] is benilla's `[+0xc48]`, see [`crate::creature_anim::stop_attack_local`]), the
/// cooldown, then the code. The re-arm happens **only when a line is actually shown** — the
/// reference reaches `0x5ec9b6` from the two code arms alone and its `else` jumps past it.
///
/// **Once per rendered frame is the exact cadence, not merely a fast-enough one.** `0x5ec950` is
/// `CGPlayer_C` vtable `0x80af78` **slot 14** (`+0x38`), and the only dispatcher of that slot
/// image-wide is `0x48160c` inside `0x4815d0`, on the `CWorldFrame::OnPaint` chain
/// (`0x483460` → `0x48361d` → `0x681070` → `0x683dd0` → `0x481540`). Culling does not skip it —
/// it only changes the pass — and neither does a first-person camera. A Bevy `Update` system is
/// that, and the 4000 ms gate makes the match exact rather than approximate.
fn show_swing_refusal(
    time: Res<Time>,
    mut refusal: ResMut<SwingRefusal>,
    engaged: Query<(), (With<SelfPlayer>, With<Engaged>)>,
    mut errors: ResMut<UiErrorKeys>,
) {
    let Some((code, due)) = refusal.0 else {
        return;
    };
    if engaged.is_empty() {
        return; // `[+0xc48] == 0` — no attack target, so nothing to complain about
    }
    let now = time.elapsed();
    if now < due {
        return;
    }
    refusal.0 = Some((code, now + REPEAT));
    errors.0.push(UiError::key(code.key()));
}

/// **Entering** the world drops the latch — the reference's `0x5e2510`, reached from
/// `ClientInitializeGame 0x401570` (`0x401648` → `0x5e253a`), which runs **once per
/// world entry, not once per process**. So a fresh world cannot inherit the last one's refusal.
///
/// Nothing else clears it, and never the passage of time: what actually silences a repeating
/// refusal in play is a landed swing, or the attack target going away (the tick's second gate).
fn on_world_enter(mut refusal: ResMut<SwingRefusal>) {
    refusal.clear();
}

pub(crate) struct SwingRefusalPlugin;

impl Plugin for SwingRefusalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SwingRefusal>()
            .add_message::<SwingRefusalEdge>()
            .add_systems(
                Update,
                // After the drain that writes the edges, and before the unit feeds, so a key
                // queued this frame is resolved and shown by `ui_action::feed_actions` on the same
                // frame rather than the next. Chained, so a refusal that arrives this frame is
                // latched before the tick reads the latch.
                (apply_swing_refusals, show_swing_refusal)
                    .chain()
                    .after(benilla_world::schedule::WorldStage::Net)
                    .before(crate::ui_unit::UnitFeed),
            )
            .add_systems(
                OnEnter(crate::char_select::ClientState::InWorld),
                on_world_enter,
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// The latch makes the line due immediately; the re-arm is exactly 4 s later.
    #[test]
    fn the_first_line_is_immediate_and_the_repeat_is_four_seconds() {
        let mut r = SwingRefusal::default();
        r.latch(Latched::NotInRange, secs(100));
        assert_eq!(
            r.0,
            Some((Latched::NotInRange, secs(100))),
            "due now, not in 4 s"
        );
        assert_eq!(secs(100) + REPEAT, Duration::from_millis(104_000));
    }

    /// A **second** refusal defeats the 4 s cooldown: `0x5ecdb0` writes the deadline
    /// unconditionally, so the next tick shows again at once — and a NOTINRANGE→BADFACING flip
    /// shows immediately rather than 4 s later.
    #[test]
    fn a_second_refusal_packet_defeats_the_cooldown() {
        let mut r = SwingRefusal::default();
        r.latch(Latched::NotInRange, secs(100));
        // …the tick shows the line and re-arms for 104 s…
        r.0 = Some((Latched::NotInRange, secs(100) + REPEAT));
        // …and a second packet one second later puts it due NOW, not at 104 s.
        r.latch(Latched::BadFacing, secs(101));
        assert_eq!(r.0, Some((Latched::BadFacing, secs(101))));
    }

    /// A landed swing clears it — the reason the line stops instead of repeating forever.
    #[test]
    fn a_landed_swing_clears_the_latch() {
        let mut r = SwingRefusal::default();
        r.latch(Latched::BadFacing, secs(5));
        r.clear();
        assert_eq!(r.0, None);
    }

    /// Both keys are catalog rows, and they are the two the reference's error ids name — the
    /// `push 0xd8`/`push 0xd7` at `0x5ec9a5`/`0x5ec9b1`.
    #[test]
    fn the_two_keys_are_the_reference_error_ids() {
        let pos = benilla_ui::messages::by_key(Latched::NotInRange.key()).expect("0xd8 is a row");
        let facing = benilla_ui::messages::by_key(Latched::BadFacing.key()).expect("0xd7 is a row");
        assert_eq!(pos.id, 0xd8);
        assert_eq!(facing.id, 0xd7);
        // Both go to the red line, not to chat — the catalog's own `kind`, not a choice here.
        assert_eq!(pos.kind, benilla_ui::messages::MsgKind::Error);
        assert_eq!(facing.kind, benilla_ui::messages::MsgKind::Error);
    }
}
