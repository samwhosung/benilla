//! The walk/run toggle: the `TOGGLERUN` binding, the reference's `ToggleRun` `0x513d50`, latching
//! [`Player::walking`]. The flag word carries it as `MOVEFLAG_WALK_MODE` `0x100`, the speed cascade
//! ([`crate::net::current_speed`]) makes it `min(MOVE_WALK, MOVE_RUN)`, and the animation picks
//! Walk(4) over Run(5) on speed alone, `speed > 2 × walkSpeed` (`0x5fd224`).
//!
//! The reference's move-state broadcaster sends only with a locomotion bit set (`0x61a99d test
//! al,0xf`), so `ToggleRun` enqueues its own move event (`0x513dd2 call 0x60e080` → `0x60e060` →
//! `0x617de0` → `0x617570`), kind `0xf` when the walk bit is clear and `0xe` when set. Ours goes
//! out of the flag differ in [`super::movement_net`], which does not wait on locomotion either.

use super::{BodyQuery, Player};

/// Whether the reference refuses this `ToggleRun` press. Its guard chain (`0x513d50`–`0x513dcf`)
/// falls silently to `0x513dd7`, with no packet, flip or message, on:
///
/// - health ≤ 0 (`[descr+0x40]`, as the death predicate `0x605f90` reads it), with no feign-death
///   flag, hence `ObjectFields::unit_is_dead`; a ghost (health 1) may toggle;
/// - a live spline, its flags' bit `0x4` clear (set by `0x619de0`, inferred to mark the spline
///   done); ours is [`Player::server_riding`] (taxi, charge, knockback);
/// - `MOVEMENTFLAGS & 0x1200`: `0x1000` is `MOVEFLAG_ROOT` (`SetRoot` `0x7c7340`), and no 1.12
///   code sets `0x200` (every wire route masks it out), so root is the whole test;
/// - stand state 7 (`UNIT_FIELD_BYTES_1` byte 0, `[descr+0x210]`), not tested here.
pub(super) fn toggle_refused(dead: bool, rooted: bool, on_spline: bool) -> bool {
    dead || rooted || on_spline
}

/// Runs this frame's `TOGGLERUN` press; the only other writer of [`Player::walking`] is the
/// server-authored merge in [`super::wire_in`].
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
    // A flip of the current bit, as `0x60e080` reads `[unit+0x9e8] & 0x100` for `0x617de0`'s
    // event kind; 1.12 has no walk-on or walk-off command.
    let want = !player.walking;
    if toggle_refused(dead, rooted, on_spline) {
        // Silent, as in the reference; the trace tag ([`super::move_trace::gait`]) records it.
        super::move_trace::gait("REFUSED", want, dead, rooted, on_spline);
        return;
    }
    player.walking = want;
    super::move_trace::gait("commit", want, dead, rooted, on_spline);
}

#[cfg(test)]
mod tests {
    use super::toggle_refused;

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
