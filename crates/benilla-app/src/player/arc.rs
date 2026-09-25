//! The airborne-arc lifecycle bookkeeping — an `impl Player` block beside its concern (the
//! `mover.rs`/`movement_net.rs` pattern): the per-frame jump/step-off arc advance
//! ([`Player::advance_airborne_arc`]) and the wire lifecycle edges it reports ([`ArcEdges`]).
//! `control` calls it once per non-swim frame after the mover has written this frame's pose; the
//! `MSG_MOVE_*` opcode selection and the FALLINGFAR pose gate both read the edges. Decisions
//! 0053/0058/0179.

use super::{Player, FALL_FAR_DROP, FALL_FAR_TIME};

/// The airborne-arc lifecycle edges [`Player::advance_airborne_arc`] reports for the frame — what the
/// live flags (`new_arc` re-seeds the frozen airborne direction bits) and the wire stream
/// (`landed` selects the `MSG_MOVE_*` opcode) read.
pub(super) struct ArcEdges {
    /// This frame began a new airborne arc (a first airborne frame or a fresh jump, incl. a
    /// same-frame land+relaunch) — the airborne direction flags re-seed from the current keys.
    pub(super) new_arc: bool,
    /// The arc ended this frame (airborne last frame, grounded now) — emit `MSG_MOVE_FALL_LAND`.
    pub(super) landed: bool,
}

impl Player {
    /// Advance the airborne-arc bookkeeping one frame and return the wire lifecycle edges.
    /// Call once per non-swim frame, **after** the mover has written
    /// this frame's `pos`/`vel_y`. `airborne` is this frame's airborne state; `jumped` whether a
    /// fresh jump launched this frame; `launch_y` the ground height the mover started this frame at
    /// (the pre-step feet Y) — the true takeoff height.
    ///
    /// A **new arc** is a first airborne frame OR any fresh `jumped`. The mover only raises
    /// `jumped` on a *grounded* frame, so a land-and-relaunch inside a single frame — spamming
    /// Space until its just-pressed edge lands on the touchdown frame, so `airborne` never drops —
    /// is a brand-new jump, not a continuation. Keying the launch snapshot on that (rather than on
    /// the airborne rising-edge alone) is what stops the relaunch from inheriting the previous
    /// arc's launch height / far-latch and flashing Fall(40) for one jump.
    ///
    /// A new arc snapshots the launch state: the fall clock, the (constant) launch vertical speed —
    /// the client's `StartFalling` argument `+0xa0`: `vel_y` (`JUMP_SPEED`) for a jump, **exactly
    /// 0** for a step-off (the walk election calls `StartFalling(0)`, not the gravity-polluted
    /// first tick), which the wire tail and the FALLINGFAR leg both key on — the launch height, and
    /// clears the per-arc far latch. The height is `launch_y` (the client's `+0x7c` Z snapshot,
    /// taken at takeoff), **not** the post-step `pos.y`: on the takeoff frame the mover has already
    /// integrated one jump-tick upward (`pos.y ≈ ground + JUMP_SPEED·dt ≈ 0.13 yd at 60 fps`), so
    /// snapshotting `pos.y` would seat the launch height a tick too high and the descent back to the
    /// real ground would read as a `FALL_FAR_DROP` (1/9 yd) fall — Fall(40) for a frame near every
    /// landing, worse the lower the frame rate (the bug the director hit spamming jumps). Landing
    /// clears the clock.
    ///
    /// Then the FALLINGFAR latch (`0x633240`): the two legs are **exclusive on the
    /// launch vz** — a jump (vz ≠ 0) latches by DESCENT [`FALL_FAR_DROP`] below its launch, a
    /// step-off fall (vz = 0) by TIME [`FALL_FAR_TIME`] airborne. Latched once per arc (only a
    /// landing clears `fall_far`, like the client's StopFalling). A flat jump returns exactly to its
    /// launch ground, staying strictly above `launch_y − 1/9`, so it never latches — its hang stays
    /// Jump(38).
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
            // From `launch_vz`, NOT `vel_y`: the mover now integrates gravity on
            // the take-off frame too, so `vel_y` has already moved `g·dt` down the parabola by the
            // time this runs and would seat the arc's launch speed a step short — visible on the
            // wire, where the jump tail sends this value on every packet of the arc, and in the
            // FALLINGFAR leg split. The reference writes `+0xa0` once, in `StartFalling`.
            self.jump_zspeed = if jumped { self.launch_vz } else { 0.0 };
            self.fall_start_y = launch_y;
            self.fall_far = false;
        } else if !airborne {
            self.airborne_since = None;
            // The arc is over: its direction nibble and its knockback provenance go with it, so
            // the next standstill jump gets its one steer back and the wire stops planting FORWARD.
            // The reference's `StopFalling` is the same edge.
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
            // A landing closes the arc (→ MSG_MOVE_FALL_LAND). A same-frame relaunch keeps
            // `airborne` true, so no FALL_LAND fires — the bounce streams a fresh JUMP instead.
            //
            // **A root ends the arc without landing it**. `SetRoot`'s `StopFalling`
            // clears FALLING wherever the body happens to be, and the reference *separately*
            // suppresses the land packet while rooted (`0x602df3 test ah,0x10` gating opcode
            // `0xc9`). Both halves matter here: without the suppression a stun caught mid-fall would
            // report a touchdown in mid-air — a `MSG_MOVE_FALL_LAND` carrying the whole accumulated
            // fall clock, which vmangos `Player::HandleFall` reads as a killing drop, plus the local
            // wound grunt and dust puff of a landing that never happened. On release the fall
            // restarts as a fresh arc from a zero clock, which is `ClearRoot`'s `StartFalling(0)`.
            landed: !airborne && was_airborne && !self.modes.rooted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{GRAVITY, JUMP_SPEED};
    use super::*;
    use bevy::prelude::Vec3;

    /// One jump-tick of upward travel the mover integrates *within* the takeoff step (60 fps) — so
    /// the post-step `pos.y` on the takeoff frame sits this far above the true launch ground.
    const TAKEOFF_RISE: f32 = JUMP_SPEED / 60.0;

    /// The bug's precondition, as a compile-time invariant: one takeoff tick of rise exceeds the
    /// far-fall drop, so snapshotting the *risen* takeoff `pos.y` as the launch height (instead of
    /// the pre-step ground) would latch FALLINGFAR on the descent back to ground at 60 fps — the
    /// Fall(40) flash. The fix seats the launch height at the ground, defeating this.
    const _: () = assert!(TAKEOFF_RISE > FALL_FAR_DROP);

    /// What a **real** step-off's first frame descends at 60 fps: the walk election launches at
    /// vz 0, gravity gives it `g·dt`, and the step moves it `g·dt²`. Small — ~5 mm — and that is
    /// the point: 1458's report edge asks only whether the body went down *at all*, so the margin
    /// between a genuine walk-off and a body that went nowhere is this, not a threshold anyone
    /// picked.
    const FIRST_TICK_DROP: f32 = GRAVITY / (60.0 * 60.0);

    /// A grounded player about to jump from ground height `ground`. `launch_y` passed to
    /// `advance_airborne_arc` is this pre-step feet Y; the takeoff frame's post-step `pos.y` is
    /// already `ground + TAKEOFF_RISE` (the mover moved up within the step).
    fn grounded_at(ground: f32) -> Player {
        Player {
            vel_y: JUMP_SPEED,
            // The mover records this the instant the take-off is decided, and the arc snapshot
            // reads it rather than `vel_y` — by the time the bookkeeping runs, the
            // real mover has already stepped `vel_y` one `g·dt` down the parabola.
            launch_vz: JUMP_SPEED,
            pos: Vec3::new(0.0, ground, 0.0),
            ..Default::default()
        }
    }

    /// Run the takeoff frame of a jump from `ground`: `launch_y` = the pre-step ground, `pos.y`
    /// already risen one tick (as the real mover leaves it). Returns the player mid-arc.
    fn take_off(ground: f32) -> Player {
        let mut p = grounded_at(ground);
        let edges = p.advance_airborne_arc(true, true, 0.0, ground);
        assert!(edges.new_arc, "the takeoff frame begins a new arc");
        p.pos.y = ground + TAKEOFF_RISE; // the post-step position the mover would have left
        p
    }

    /// **The launch speed on the wire is the LAUNCH's, not the post-step velocity** (decision
    /// 1740). The mover integrates gravity on the take-off frame too, so `vel_y` is already
    /// `g·dt` down by the time the arc bookkeeping runs; reading it would send observers a jump
    /// tail one step short (0.32 yd/s at 60 fps, 0.64 at 30 — and frame-rate dependent, so two
    /// clients watching the same jump would disagree). The reference writes `+0xa0` once, in
    /// `StartFalling`, and never touches it again for the arc.
    #[test]
    fn the_arc_snapshots_the_launch_speed_not_the_already_integrated_one() {
        let mut p = grounded_at(100.0);
        // What the mover leaves behind on the take-off frame: the launch recorded, `vel_y` already
        // advanced one step by the exact fall step.
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

    /// The arc's direction nibble and its knockback provenance are **arc-scoped**: a landing
    /// clears both, so the next standstill jump gets its one steer back and the wire stops
    /// planting FORWARD (the reference's `StopFalling` edge).
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
        // Takeoff: pos.y is already one tick up (mover integrated the jump within the step), but the
        // launch height must be the pre-step ground, or the descent back to it reads as a far fall.
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
        // The director's bug: a plain jump on flat ground, descending back to exactly the launch
        // height. With the launch height seated at the ground (not one tick up), the descent stays
        // strictly above ground − 1/9 yd, so FALLINGFAR never latches — the hang stays Jump(38).
        let mut p = take_off(100.0);
        // Rise, apex, then descend all the way back to the ground (the last airborne frame clips to
        // the floor at exactly the launch height).
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
        // Locks in *why* the launch height is the pre-step ground: had we snapshotted the risen
        // takeoff pos.y instead, the same flat jump WOULD latch — the regression this fix removes.
        let ground = 100.0;
        let risen = ground + TAKEOFF_RISE;
        // The distance leg with the (wrong) risen launch height, evaluated at the touchdown ground.
        assert!(
            ground <= risen - FALL_FAR_DROP,
            "the risen launch height puts the far-fall threshold above the ground"
        );
        // And with the (correct) ground launch height, it does not.
        assert!(
            ground > ground - FALL_FAR_DROP,
            "the ground launch height keeps the whole flat jump above the threshold"
        );
    }

    #[test]
    fn a_same_frame_land_and_relaunch_is_a_new_arc_not_a_far_fall() {
        // Spamming Space so its just-pressed edge lands on the touchdown frame: the mover reports
        // grounded+jumped in one frame, `airborne` never drops. The prior jump launched from a
        // touch HIGHER (100.3) than the ground we relaunch off (100.0); a stale launch height would
        // classify the relaunch as a far fall. The fix re-snapshots the new arc's ground.
        let mut p = take_off(100.3);
        p.pos.y = 100.0; // descended to the ground, still airborne
        p.vel_y = -6.0;
        p.advance_airborne_arc(true, false, 0.4, 100.3);
        assert!(
            p.pos.y <= p.fall_start_y - FALL_FAR_DROP,
            "precondition: against the OLD arc's launch height this reads as a far fall"
        );

        // The touchdown+relaunch frame: grounded (launch_y = the 100.0 ground) and jumped.
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
        // A prior arc that genuinely latched FALLINGFAR must not carry the latch into a same-frame
        // relaunch — else the new jump plays Fall(40) from its very first frame.
        let mut p = take_off(100.0);
        p.pos.y = 90.0; // a real long fall this arc
        p.advance_airborne_arc(true, false, 0.6, 100.0);
        assert!(p.fall_far, "the prior arc latched a far fall");

        // Land+relaunch in one frame off the low ground.
        p.vel_y = JUMP_SPEED;
        let edges = p.advance_airborne_arc(true, true, 0.62, 90.0);
        assert!(edges.new_arc);
        assert!(!p.fall_far, "the fresh jump clears the inherited far latch");
    }

    #[test]
    fn a_step_off_fall_latches_falling_far_by_time_not_distance() {
        // A step-off (airborne, not jumped) launches at vz 0 — the walk election's StartFalling(0)
        // — so it latches by TIME, not descent, even before dropping a full 1/9 yd.
        // `pos.y` is the POST-step pose, so a real step-off's first frame is already one tick of
        // gravity below its launch ground ([`FIRST_TICK_DROP`]).
        let mut p = Player {
            pos: Vec3::new(0.0, 100.0 - FIRST_TICK_DROP, 0.0),
            vel_y: -0.5,
            ..Default::default()
        };
        let edges = p.advance_airborne_arc(true, false, 0.0, 100.0);
        assert!(edges.new_arc, "the step-off opens an arc");
        assert_eq!(p.jump_zspeed, 0.0, "a step-off launches at exactly 0");
        // Barely descended, but not yet FALL_FAR_TIME airborne.
        p.pos.y = 99.95;
        p.advance_airborne_arc(true, false, 0.3, 100.0);
        assert!(!p.fall_far, "under FALL_FAR_TIME: not yet a far fall");
        // Past the timer leg.
        p.advance_airborne_arc(true, false, FALL_FAR_TIME + 0.01, 100.0);
        assert!(
            p.fall_far,
            "past FALL_FAR_TIME the step-off latches by time"
        );
    }

    /// A root or a stun caught mid-fall **ends the arc without landing it**. The
    /// mover holds the body where it was (its anchor), so `airborne` goes false in mid-air — and the
    /// reference suppresses the land packet on exactly that state (`0x602df3`). Were it not
    /// suppressed, the frame the stun lands would report a touchdown 30 yd up carrying the whole
    /// accumulated fall clock, which vmangos `Player::HandleFall` reads as a killing drop, and would
    /// fire the local wound grunt + dust puff for a landing that never happened.
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
        // The control that gives the assertion its teeth: the identical frame with no root is a
        // landing. The gate is the root, not "any arc that ends".
        let mut q = take_off(100.0);
        q.pos.y = 70.0;
        q.advance_airborne_arc(true, false, 1.0, 100.0);
        assert!(
            q.advance_airborne_arc(false, false, 1.2, 100.0).landed,
            "an unrooted body reaching the ground still reports its landing"
        );
    }

    /// Releasing the root in mid-air is `ClearRoot 0x7c7370`'s `StartFalling` — a **fresh** arc from
    /// a zero clock, not a resumption of the one the root ate. That is what keeps the drop honest:
    /// the fall damage the server assesses is measured from the release, not from before the stun.
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
