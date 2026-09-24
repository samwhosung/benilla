//! **The walk/run gait toggle** — the `TOGGLERUN` keybind, the reference's `ToggleRun 0x513d50`.
//!
//! One latched bit ([`Player::walking`]) and one refusal chain, kept here beside its concern like
//! [`super::posture`] and [`super::gait`]. Everything downstream is already built and reads the
//! bit through the ordinary paths: the flag word ([`super::flags`]) carries `MOVEFLAG_WALK_MODE`
//! `0x100` out onto every packet, the speed cascade ([`crate::net::current_speed`]) turns it into
//! `min(MOVE_WALK, MOVE_RUN)`, and the animation selector needs nothing at all — it picks Walk(4)
//! over Run(5) purely on `speed > 2 × walkSpeed`, so a correct speed *is* the correct clip and a
//! correct playback rate (`0x5fd224`: walk-vs-run is speed-driven, not the walk-mode bit).
//!
//! **The one thing the toggle owns that the differ does not**: nothing else in the client would
//! ever announce this. The reference's own move-state broadcaster gates every send on the
//! locomotion nibble (`0x61a99d test al,0xf`), so a toggle pressed while standing still would
//! never reach the server through it — which is exactly why `ToggleRun` enqueues its own move
//! event (`0x513dd2 call 0x60e080` → `0x60e060` → `0x617de0` → `0x617570`, kind `0xf` when the
//! walk bit is currently clear and `0xe` when it is set). Ours goes out of the flag differ in
//! [`super::movement_net`], which is likewise not nibble-gated.

use super::{BodyQuery, Player};

/// Does the reference **refuse** this `ToggleRun` press? The handler's own guard chain, in its byte
/// order (`0x513d50`–`0x513dcf`; every gate falls to the same silent `0x513dd7` — no packet, no
/// local flip, no message, exactly like the sheath and stand-state chains):
///
/// ```text
/// 513d8e  mov esi,[descr+0x40] ; test esi,esi ; jle    →  health <= 0
/// 513d95  mov ecx,[unit+0x118] ; mov ecx,[ecx+0xa4]    →  the active spline object, if any
/// 513da5  test byte [ecx+0x18],0x4 ; je                →  …its FINALIZE latch must be SET
/// 513dab  (health, re-read and re-tested)
/// 513dbe  mov ebx,[cmov+0x40] ; test bh,0x12 ; jne     →  MOVEMENTFLAGS & 0x1200
/// 513dc6  cmp byte [descr+0x210],0x7 ; je              →  UNIT_FIELD_BYTES_1.standState == DEAD
/// ```
///
/// `descr+0x40` is `UNIT_FIELD_HEALTH`, not a guess: benilla already byte-pinned it at the
/// reference's own death predicate `0x605f90` ("really dead — `[descr+0x40] <= 0`", see
/// `ObjectFields::unit_reads_dead`), and `descr+0x210` is `UNIT_FIELD_BYTES_1` (field index
/// `0x84`), whose byte 0 is the stand state. So the death leg is health-or-standstate — and
/// notably **not** the feign-death dynflag that `unit_reads_dead` also carries, which is why this
/// takes the narrower [`ObjectFields::unit_is_dead`]. A **ghost** is health 1 and stand state 0,
/// so a corpse run may be walked, and the corpse it is running to may not toggle anything.
///
/// The spline gate reads the other way round from how it looks: bit `0x4` of the spline flags is
/// the **finalize / `MOVE_SPLINE_DONE` latch** (`0x619de0` sets it; the FLY gate reads
/// `(flags & 0x204) == 0x200`), so "the latch must be set" means *the spline is finished* —
/// i.e. **a mover on a live server spline is refused**. Ours is [`Player::server_riding`]: the
/// taxi flight and the charge/knock hand-offs are the states where a spline is actually flying.
///
/// `0x1200` is two bits and only one of them is settled: `0x1000` is `MOVEFLAG_ROOT`
/// (`SetRoot 0x7c7340` writes it), and `0x200` is a bit benilla does not model at all. The same
/// pair gates the autorun emitter `0x514560`, where [`super::input`] records the identical caveat;
/// until `0x200` is identified, the modelled half is the whole of what we can refuse on.
pub(super) fn toggle_refused(dead: bool, rooted: bool, on_spline: bool) -> bool {
    dead || rooted || on_spline
}

/// Run this frame's `TOGGLERUN` press. Nothing else in the client writes [`Player::walking`]
/// except the server-authored merge in [`super::wire_in`].
pub(super) fn update(
    player: &mut Player,
    body: &BodyQuery,
    binds: &crate::bindings::BindingsState,
) {
    if !binds.fired(crate::bindings::cmd::TOGGLE_RUN) {
        return;
    }
    let dead = body
        .single()
        .ok()
        .and_then(|(.., store, _, _, _, _, _)| store.map(|s| s.0.unit_is_dead()))
        .unwrap_or(false);
    let (rooted, on_spline) = (player.modes.rooted, player.server_riding);
    // A read-and-invert of the *current* bit, which is the reference's shape exactly: `0x60e080`
    // fetches `[unit+0x9e8] & 0x100` and `0x617de0` picks the event kind off it (`neg`/`sbb`/
    // `add 0xf` → `0xe` when set, `0xf` when clear). There is no "walk on"/"walk off" command in
    // 1.12 — one binding, one flip.
    let want = !player.walking;
    if toggle_refused(dead, rooted, on_spline) {
        // Silent, like the reference — no packet, no flip, no message. The trace tag is the only
        // way a refusal is readable from a live run ([`super::move_trace::gait`]).
        super::move_trace::gait("REFUSED", want, dead, rooted, on_spline);
        return;
    }
    player.walking = want;
    super::move_trace::gait("commit", want, dead, rooted, on_spline);
}

#[cfg(test)]
mod tests {
    use super::toggle_refused;

    /// The three refusals we can actually state, and the control that says the predicate is a
    /// refusal chain rather than a blanket "not while anything is unusual".
    #[test]
    fn the_guard_chain_refuses_exactly_the_three_states_it_names() {
        assert!(!toggle_refused(false, false, false), "standing: granted");
        assert!(toggle_refused(true, false, false), "a corpse cannot toggle");
        assert!(
            toggle_refused(false, true, false),
            "rooted: `0x1200`'s modelled half"
        );
        assert!(
            toggle_refused(false, false, true),
            "a live server spline (taxi, charge) refuses — the finalize latch is clear"
        );
    }
}
