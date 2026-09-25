//! The airborne arc: the per-frame jump and step-off bookkeeping, and the wire edges it reports.

use super::{Player, FALL_FAR_DROP, FALL_FAR_TIME};

/// This frame's arc edges, read by the flag word and the wire stream.
pub(super) struct ArcEdges {
    /// A first airborne frame or a fresh jump, a same-frame land and relaunch included.
    pub(super) new_arc: bool,
    /// The arc landed this frame: send `MSG_MOVE_FALL_LAND`.
    pub(super) landed: bool,
}

impl Player {
    /// Advances the arc; call once per non-swim frame, after the mover wrote `pos` and `vel_y`.
    /// `launch_y` is the pre-step feet Y, the true takeoff height.
    ///
    /// A new arc (a first airborne frame, or any `jumped`, which the mover raises only when
    /// grounded) snapshots the fall clock, the launch speed (`StartFalling`'s `+0xa0`: the jump
    /// speed, or exactly 0 for a step-off's `StartFalling(0)`) and the launch height (`+0x7c`),
    /// and clears the far latch. The height is `launch_y`, not the post-step `pos.y` a tick
    /// higher, which would read a flat jump's descent as a far fall.
    ///
    /// FALLINGFAR (`0x633240`) splits on the launch speed: a jump latches [`FALL_FAR_DROP`] below
    /// its launch, a step-off after [`FALL_FAR_TIME`] airborne; only a landing clears it.
    pub(super) fn advance_airborne_arc(
        &mut self,
        airborne: bool,
        jumped: bool,
        now: f32,
        launch_y: f32,
    ) -> ArcEdges {
        let was_airborne = self.airborne_since.is_some();
        let new_arc = airborne && (!was_airborne || jumped);
        if new_arc {
            self.airborne_since = Some(now);
            // `launch_vz`, not `vel_y`, which the mover has already moved one `g·dt` down. The
            // reference writes `+0xa0` once, in `StartFalling`, and the jump tail sends it all arc.
            self.jump_zspeed = if jumped { self.launch_vz } else { 0.0 };
            self.fall_start_y = launch_y;
            self.fall_far = false;
        } else if !airborne {
            self.airborne_since = None;
            // The reference's `StopFalling` edge: the arc's steer nibble and knockback go with it.
            self.arc_dirs_set = false;
            self.knock_arc = false;
        }
        if airborne {
            let far = if self.jump_zspeed != 0.0 {
                self.pos.y <= self.fall_start_y - FALL_FAR_DROP
            } else {
                self.airborne_since
                    .is_some_and(|t0| now - t0 >= FALL_FAR_TIME)
            };
            if far {
                self.fall_far = true;
            }
        }
        ArcEdges {
            new_arc,
            // A same-frame relaunch stays airborne and streams a JUMP. A root ends the arc in
            // mid-air (`SetRoot`'s `StopFalling`) with no land packet (`0x602df3 test ah,0x10`
            // before opcode `0xc9`), which would hand vmangos `Player::HandleFall` the whole fall
            // clock; the release is a fresh arc, `ClearRoot`'s `StartFalling(0)`.
            landed: !airborne && was_airborne && !self.modes.rooted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{GRAVITY, JUMP_SPEED};
    use super::*;
    use bevy::prelude::Vec3;

    /// The rise within a 60 fps takeoff step: the takeoff frame's post-step `pos.y` sits this high.
    const TAKEOFF_RISE: f32 = JUMP_SPEED / 60.0;

    /// A launch height from the risen `pos.y` would latch FALLINGFAR on a flat jump's descent.
    const _: () = assert!(TAKEOFF_RISE > FALL_FAR_DROP);

    /// A step-off's first-frame descent at 60 fps, `g·dt²` from vz 0: about 5 mm.
    const FIRST_TICK_DROP: f32 = GRAVITY / (60.0 * 60.0);

    /// A grounded player about to jump from `ground`, the `launch_y` its takeoff frame passes.
    fn grounded_at(ground: f32) -> Player {
        Player {
            vel_y: JUMP_SPEED,
            // Recorded at the takeoff; by the time the arc runs, `vel_y` is a `g·dt` lower.
            launch_vz: JUMP_SPEED,
            pos: Vec3::new(0.0, ground, 0.0),
            ..Default::default()
        }
    }

    /// The takeoff frame of a jump from `ground`, leaving `pos.y` a tick risen as the mover does.
    fn take_off(ground: f32) -> Player {
        let mut p = grounded_at(ground);
        let edges = p.advance_airborne_arc(true, true, 0.0, ground);
        assert!(edges.new_arc, "the takeoff frame begins a new arc");
        p.pos.y = ground + TAKEOFF_RISE; // the post-step position the mover would have left
        p
    }

    #[test]
    fn the_arc_snapshots_the_launch_speed_not_the_already_integrated_one() {
        let mut p = grounded_at(100.0);
        // The takeoff frame as the mover leaves it: the launch recorded, `vel_y` one fall step on.
        let dt = 1.0 / 60.0;
        p.vel_y = super::super::mover::fall_step(JUMP_SPEED, dt, 60.148_003).0;
        assert!(
            p.vel_y < JUMP_SPEED,
            "the take-off frame really did integrate"
        );
        let edges = p.advance_airborne_arc(true, true, 0.0, 100.0);
        assert!(edges.new_arc);
        assert_eq!(
            p.jump_zspeed, JUMP_SPEED,
            "the wire tail carries the launch speed for the whole arc, not this frame's velocity"
        );
    }

    #[test]
    fn landing_clears_the_arc_nibble_and_the_knockback_provenance() {
        let mut p = grounded_at(100.0);
        p.advance_airborne_arc(true, true, 0.0, 100.0);
        p.arc_dirs_set = true;
        p.knock_arc = true;
        let edges = p.advance_airborne_arc(false, false, 0.5, 100.0);
        assert!(edges.landed, "the arc closed");
        assert!(!p.arc_dirs_set, "the nibble goes with the arc");
        assert!(!p.knock_arc, "so does the knockback provenance");
    }

    #[test]
    fn a_fresh_jump_snapshots_the_true_launch_ground_not_the_risen_pos() {
        let mut p = grounded_at(100.0);
        p.pos.y = 100.0 + TAKEOFF_RISE;
        let edges = p.advance_airborne_arc(true, true, 0.0, 100.0);
        assert!(edges.new_arc);
        assert_eq!(p.jump_zspeed, JUMP_SPEED, "the launch vz is the jump speed");
        assert_eq!(
            p.fall_start_y, 100.0,
            "the launch height is the pre-step GROUND, not the risen pos.y"
        );
        assert!(!p.fall_far);
    }

    #[test]
    fn a_flat_jump_landing_back_on_its_ground_never_latches_falling_far() {
        // Rise, apex and back down to exactly the launch height.
        let mut p = take_off(100.0);
        for (t, y) in [
            (0.10, 100.6),
            (0.20, 100.75),
            (0.30, 100.4),
            (0.40, 100.05),
            (0.45, 100.0), // touchdown height, still processed airborne
        ] {
            p.pos.y = y;
            p.advance_airborne_arc(true, false, t, 100.0);
            assert!(
                !p.fall_far,
                "a flat jump must never read as a far fall (y={y})"
            );
        }
    }

    #[test]
    fn snapshotting_the_risen_pos_would_have_latched_a_flat_jump() {
        let ground = 100.0;
        let risen = ground + TAKEOFF_RISE;
        assert!(
            ground <= risen - FALL_FAR_DROP,
            "the risen launch height puts the far-fall threshold above the ground"
        );
        assert!(
            ground > ground - FALL_FAR_DROP,
            "the ground launch height keeps the whole flat jump above the threshold"
        );
    }

    #[test]
    fn a_same_frame_land_and_relaunch_is_a_new_arc_not_a_far_fall() {
        // A jump from 100.3 lands and relaunches off 100.0 in one frame, so `airborne` never
        // drops; the old launch height would read the relaunch as a far fall.
        let mut p = take_off(100.3);
        p.pos.y = 100.0; // descended to the ground, still airborne
        p.vel_y = -6.0;
        p.advance_airborne_arc(true, false, 0.4, 100.3);
        assert!(
            p.pos.y <= p.fall_start_y - FALL_FAR_DROP,
            "precondition: against the OLD arc's launch height this reads as a far fall"
        );

        // The touchdown and relaunch frame, `launch_y` the 100.0 ground.
        p.vel_y = JUMP_SPEED;
        let edges = p.advance_airborne_arc(true, true, 0.42, 100.0);
        assert!(edges.new_arc, "a land+relaunch in one frame is a new arc");
        assert!(
            !edges.landed,
            "the bounce keeps airborne true, so no FALL_LAND"
        );
        assert_eq!(p.airborne_since, Some(0.42), "the fall clock restarted");
        assert_eq!(
            p.fall_start_y, 100.0,
            "the launch height re-snapshotted to the new ground"
        );
        assert!(
            !p.fall_far,
            "the relaunch is a fresh jump, not a continuation of the old far fall"
        );
    }

    #[test]
    fn a_stale_far_latch_does_not_bleed_into_the_relaunch() {
        let mut p = take_off(100.0);
        p.pos.y = 90.0; // a real long fall this arc
        p.advance_airborne_arc(true, false, 0.6, 100.0);
        assert!(p.fall_far, "the prior arc latched a far fall");

        // Land and relaunch in one frame off the low ground.
        p.vel_y = JUMP_SPEED;
        let edges = p.advance_airborne_arc(true, true, 0.62, 90.0);
        assert!(edges.new_arc);
        assert!(!p.fall_far, "the fresh jump clears the inherited far latch");
    }

    #[test]
    fn a_step_off_fall_latches_falling_far_by_time_not_distance() {
        // A step-off's first post-step `pos.y` is already a tick below its launch ground.
        let mut p = Player {
            pos: Vec3::new(0.0, 100.0 - FIRST_TICK_DROP, 0.0),
            vel_y: -0.5,
            ..Default::default()
        };
        let edges = p.advance_airborne_arc(true, false, 0.0, 100.0);
        assert!(edges.new_arc, "the step-off opens an arc");
        assert_eq!(p.jump_zspeed, 0.0, "a step-off launches at exactly 0");
        p.pos.y = 99.95;
        p.advance_airborne_arc(true, false, 0.3, 100.0);
        assert!(!p.fall_far, "under FALL_FAR_TIME: not yet a far fall");
        p.advance_airborne_arc(true, false, FALL_FAR_TIME + 0.01, 100.0);
        assert!(
            p.fall_far,
            "past FALL_FAR_TIME the step-off latches by time"
        );
    }

    /// The mover's anchor holds a rooted body in mid-air, so `airborne` drops there; the reference
    /// sends no land packet in that state (`0x602df3`).
    #[test]
    fn a_root_taken_mid_fall_ends_the_arc_without_landing_it() {
        let mut p = take_off(100.0);
        p.pos.y = 70.0;
        p.advance_airborne_arc(true, false, 1.0, 100.0);
        assert!(p.fall_far, "precondition: a real, far fall is in progress");

        p.modes.rooted = true;
        let edges = p.advance_airborne_arc(false, false, 1.2, 100.0);
        assert!(!edges.landed, "a root ENDS the arc; it does not land it");
        assert_eq!(p.airborne_since, None, "and the fall clock is cleared");
        // Control: the same frame without the root is a landing.
        let mut q = take_off(100.0);
        q.pos.y = 70.0;
        q.advance_airborne_arc(true, false, 1.0, 100.0);
        assert!(
            q.advance_airborne_arc(false, false, 1.2, 100.0).landed,
            "an unrooted body reaching the ground still reports its landing"
        );
    }

    /// A mid-air release is `ClearRoot`'s (`0x7c7370`) `StartFalling(0)`: a fresh arc from a zero
    /// clock, so the server measures the fall from the release.
    #[test]
    fn releasing_the_root_mid_air_starts_a_fresh_fall_not_a_resumption() {
        let mut p = take_off(100.0);
        p.pos.y = 70.0;
        p.advance_airborne_arc(true, false, 1.0, 100.0);
        p.modes.rooted = true;
        p.advance_airborne_arc(false, false, 1.2, 100.0);

        p.modes.rooted = false; // the aura expired; still nothing under us
        p.pos.y = 70.0 - FIRST_TICK_DROP; // and the released body has already fallen one tick
        let edges = p.advance_airborne_arc(true, false, 11.2, 70.0);
        assert!(edges.new_arc, "the release begins a new arc");
        assert_eq!(p.jump_zspeed, 0.0, "launched at exactly 0, like a walk-off");
        assert_eq!(
            p.airborne_since,
            Some(11.2),
            "a fresh fall clock — the pre-root one does not carry over"
        );
        assert_eq!(p.fall_start_y, 70.0, "and a fresh launch height");
        assert!(!p.fall_far, "the new arc starts unlatched");
    }

    #[test]
    fn a_clean_landing_clears_the_clock_and_reports_landed() {
        let mut p = take_off(100.0);
        p.pos.y = 100.0;
        p.vel_y = 0.0;
        let edges = p.advance_airborne_arc(false, false, 0.5, 100.0); // grounded, no rejump
        assert!(edges.landed, "leaving the air reports a landing");
        assert!(!edges.new_arc);
        assert_eq!(p.airborne_since, None, "the fall clock cleared on landing");
    }
}
