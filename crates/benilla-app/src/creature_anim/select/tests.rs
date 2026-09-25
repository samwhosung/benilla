//! Tests for the pure animation selection in [`super`].

use super::*;
use bevy::animation::graph::AnimationNodeIndex;

fn moving_forward(speed: f32) -> MovementState {
    MovementState {
        speed,
        flags: move_flags::FORWARD,
        ..Default::default()
    }
}

fn clip(anim_id: u16, move_speed: f32) -> AnimClip {
    AnimClip {
        anim_id,
        seq_index: 0,
        node: AnimationNodeIndex::new(0),
        looping: true,
        duration: 1.0,
        move_speed,
        blend_time: 0.25,
        bounds_center: Vec3::ZERO,
        bounds_radius: 0.0,
        bounds_min: Vec3::ZERO,
        bounds_max: Vec3::ZERO,
        events: Vec::new().into(),
        arm_nodes: None,
        upper_node: None,
        frequency: 0,
        replay: (0, 0),
        poses_bones: true,
    }
}

#[test]
fn stationary_is_stand() {
    assert_eq!(
        gait_candidates(&MovementState::default(), 2.5, None, None),
        &[STAND]
    );
}

#[test]
fn walk_run_boundary_is_twice_walk_speed() {
    assert_eq!(gait_candidates(&moving_forward(4.9), 2.5, None, None)[0], 4);
    assert_eq!(gait_candidates(&moving_forward(5.0), 2.5, None, None)[0], 4); // exactly 2× is Walk
    assert_eq!(gait_candidates(&moving_forward(5.1), 2.5, None, None)[0], 5);
}

/// The selector reads the speed, never `MOVEFLAG_WALK_MODE`: walk mode's speed, `min(walk, run)`,
/// lands under the `2 × walkSpeed` Run boundary at any 1.12 speed set, so the speed cascade feeds
/// the gait cascade here with no number in between.
#[test]
fn the_walk_bit_reaches_the_clip_through_the_speed_and_nothing_else() {
    use crate::creature_anim::move_flags as f;
    let speeds = benilla_protocol::MoveSpeeds {
        walk: 2.5,
        run: 7.0,
        run_back: 4.5,
        swim: 4.722,
        swim_back: 2.5,
        turn_rate: std::f32::consts::PI,
    };
    let gait = |flags: u32| {
        let mv = MovementState {
            speed: crate::net::current_speed(&speeds, flags),
            flags,
            ..Default::default()
        };
        gait_candidates(&mv, speeds.walk, None, None)[0]
    };
    assert_eq!(gait(f::FORWARD), 5, "no walk bit: Run");
    assert_eq!(gait(f::FORWARD | f::WALK_MODE), 4, "walk bit: Walk");
    // Backward is WalkBackwards (13) either way: the walk bit changes only its speed and rate.
    assert_eq!(gait(f::BACKWARD), 13);
    assert_eq!(gait(f::BACKWARD | f::WALK_MODE), 13);
    let mounted = benilla_protocol::MoveSpeeds {
        run: 11.2,
        ..speeds
    };
    let mv = MovementState {
        speed: crate::net::current_speed(&mounted, f::FORWARD),
        flags: f::FORWARD,
        ..Default::default()
    };
    assert_eq!(
        gait_candidates(&mv, mounted.walk, None, None)[0],
        143,
        "Sprint above 11"
    );
}

#[test]
fn boundary_scales_with_the_units_own_walk_speed() {
    assert_eq!(gait_candidates(&moving_forward(7.0), 4.0, None, None)[0], 4);
    assert_eq!(gait_candidates(&moving_forward(7.0), 2.5, None, None)[0], 5);
}

#[test]
fn fast_run_above_eleven() {
    assert_eq!(
        gait_candidates(&moving_forward(11.0), 2.5, None, None),
        &[143, 5, 4, 0]
    );
}

#[test]
fn backward_is_walkbackwards() {
    let s = MovementState {
        speed: 9.0,
        flags: move_flags::BACKWARD | move_flags::STRAFE_LEFT,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&s, 2.5, None, None), &[13, 4, 0]);
}

#[test]
fn swimming_back_forward_strafe_and_idle() {
    // The swim row (`0x5fd137`): forward 42, back 45, turn 41, strafe SwimLeft 43 / SwimRight 44.
    let back = MovementState {
        speed: 4.0,
        flags: move_flags::SWIMMING | move_flags::BACKWARD,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&back, 2.5, None, None), &[45, 41, 0]);
    let fwd = MovementState {
        speed: 4.0,
        flags: move_flags::SWIMMING | move_flags::FORWARD,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&fwd, 2.5, None, None), &[42, 41, 0]);
    let left = MovementState {
        speed: 4.0,
        flags: move_flags::SWIMMING | move_flags::STRAFE_LEFT,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&left, 2.5, None, None), &[43, 42, 41, 0]);
    let right = MovementState {
        speed: 4.0,
        flags: move_flags::SWIMMING | move_flags::STRAFE_RIGHT,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&right, 2.5, None, None), &[44, 42, 41, 0]);
    // The `0x5fd100` cascade is turn, strafe, backward, forward: a strafe diagonal side-strokes.
    let diag = MovementState {
        speed: 4.0,
        flags: move_flags::SWIMMING | move_flags::FORWARD | move_flags::STRAFE_LEFT,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&diag, 2.5, None, None), &[43, 42, 41, 0]);
    let back_diag = MovementState {
        speed: 4.0,
        flags: move_flags::SWIMMING | move_flags::BACKWARD | move_flags::STRAFE_RIGHT,
        ..Default::default()
    };
    assert_eq!(
        gait_candidates(&back_diag, 2.5, None, None),
        &[44, 42, 41, 0]
    );
    let fwd_back = MovementState {
        speed: 4.0,
        flags: move_flags::SWIMMING | move_flags::FORWARD | move_flags::BACKWARD,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&fwd_back, 2.5, None, None), &[45, 41, 0]);
    let turn_moving = MovementState {
        speed: 4.0,
        flags: move_flags::SWIMMING | move_flags::FORWARD | move_flags::TURN_LEFT,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&turn_moving, 2.5, None, None), &[41, 0]);
    let turn = MovementState {
        flags: move_flags::SWIMMING | move_flags::TURN_LEFT,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&turn, 2.5, None, None), &[41, 0]);
    let idle = MovementState {
        flags: move_flags::SWIMMING,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&idle, 2.5, None, None), &[41, 0]);
}

#[test]
fn airborne_splits_jump_fall_and_gait_freeze() {
    // A jump arc plays the 37/38 bracket, a FALLINGFAR latch the Fall (40) loop (`0x602c40`), and a
    // step-off fall before the latch no special: the gait holds through it.
    let s = MovementState {
        flags: move_flags::FORWARD | move_flags::FALLING,
        ..Default::default()
    };
    assert_eq!(current_special(&s, true), Some(Special::Jump));
    assert_eq!(current_special(&s, false), None);
    let far = MovementState {
        flags: move_flags::FORWARD | move_flags::FALLING | move_flags::FALLING_FAR,
        ..Default::default()
    };
    assert_eq!(current_special(&far, true), Some(Special::Fall));
    assert_eq!(current_special(&far, false), Some(Special::Fall));
}

#[test]
fn fall_is_the_40_loop() {
    // Fall (40) has no enter one-shot: `enter_special` goes straight to the loop.
    assert_eq!(Special::Fall.enter(), 40);
    assert_eq!(Special::Fall.loop_id(), 40);
    assert!(!Special::Fall.interruptible_by_move());
}

#[test]
fn a_pose_yields_to_movement_but_a_jump_landing_plays_out() {
    assert!(Special::Pose(1).interruptible_by_move());
    assert!(Special::Pose(3).interruptible_by_move());
    assert!(Special::Pose(8).interruptible_by_move());
    assert!(!Special::Jump.interruptible_by_move());
}

#[test]
fn jump_sequence_ids() {
    // JumpStart 37 (`0x60e480`) → Jump 38 → the landing pick.
    assert_eq!(Special::Jump.enter(), 37);
    assert_eq!(Special::Jump.loop_id(), 38);
}

#[test]
fn jump_land_pick_is_the_0x602c60_rule() {
    // `0x602c60`: stopped JumpEnd 39, forward or strafing JumpLandRun 187; backpedaling, walking
    // or swimming no clip, the gait taking over at touchdown.
    use move_flags::*;
    assert_eq!(jump_land_pick(0), Some(39));
    assert_eq!(jump_land_pick(FORWARD), Some(187));
    assert_eq!(jump_land_pick(STRAFE_LEFT), Some(187));
    assert_eq!(jump_land_pick(FORWARD | STRAFE_RIGHT), Some(187));
    assert_eq!(jump_land_pick(BACKWARD), None);
    assert_eq!(jump_land_pick(BACKWARD | STRAFE_LEFT), None);
    assert_eq!(jump_land_pick(FORWARD | WALK_MODE), None);
    assert_eq!(jump_land_pick(SWIMMING), None);
    assert_eq!(jump_land_pick(FORWARD | SWIMMING), None);
    // Rooted, no clip: a root caught mid-air ends the fall there, and the reference sends no land
    // packet (`0x602df3`), the one this dispatcher runs on.
    assert_eq!(jump_land_pick(ROOT), None);
    assert_eq!(jump_land_pick(ROOT | FORWARD), None);
}

#[test]
fn standstate_is_a_pose_special_only_while_still() {
    let sit = MovementState {
        stand_state: 1,
        ..Default::default()
    };
    assert_eq!(current_special(&sit, false), Some(Special::Pose(1)));
    // SitDown 96 → Sit 97 → SitUp 98.
    assert_eq!(Special::Pose(1).enter(), 96);
    assert_eq!(Special::Pose(1).loop_id(), 97);
    assert_eq!(Special::Pose(1).exit(), 98);
    // Sleep and Kneel.
    assert_eq!(
        (Special::Pose(3).enter(), Special::Pose(3).loop_id()),
        (99, 100)
    );
    assert_eq!(
        (Special::Pose(8).enter(), Special::Pose(8).loop_id()),
        (114, 115)
    );
    // Moving suppresses the pose.
    let sit_moving = MovementState {
        speed: 3.0,
        flags: move_flags::FORWARD,
        stand_state: 1,
        ..Default::default()
    };
    assert_eq!(current_special(&sit_moving, false), None);
    assert_eq!(gait_candidates(&sit_moving, 2.5, None, None), &[4, 0]);
}

#[test]
fn turn_in_place_shuffles_but_moving_turn_runs() {
    // Turning in place plays the shuffle (11, 12).
    let left = MovementState {
        flags: move_flags::TURN_LEFT,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&left, 2.5, None, None), &[11, 0]);
    let right = MovementState {
        flags: move_flags::TURN_RIGHT,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&right, 2.5, None, None), &[12, 0]);
    // Turning while moving plays the gait.
    let move_turn = MovementState {
        speed: 3.0,
        flags: move_flags::FORWARD | move_flags::TURN_LEFT,
        ..Default::default()
    };
    assert_eq!(gait_candidates(&move_turn, 2.5, None, None), &[4, 0]);
}

#[test]
fn sheath_clip_is_the_0x88_test() {
    // `0x88` is the mask of sheath types 3 and 7, which play HipSheath (90); the rest play 89.
    assert_eq!(sheath_clip(3), 90);
    assert_eq!(sheath_clip(7), 90);
    for t in [0, 1, 2, 4, 5, 6, 8] {
        assert_eq!(sheath_clip(t), 89, "type {t}");
    }
}

#[test]
fn swing_ids_by_weapon_class() {
    // The `0x6246a0` mainhand table.
    assert_eq!(swing_anim_main(Some((2, 7))), 17); // 1H sword
    assert_eq!(swing_anim_main(Some((2, 5))), 18); // 2H mace
    assert_eq!(swing_anim_main(Some((2, 0xa))), 19); // staff
    assert_eq!(swing_anim_main(Some((2, 0x14))), 19); // fishing pole
    assert_eq!(swing_anim_main(Some((2, 0xf))), 85); // dagger stabs, not Attack1H
    assert_eq!(swing_anim_main(Some((2, 0xd))), 16); // fist swings unarmed
    assert_eq!(swing_anim_main(Some((2, 2))), 16); // bow in melee swings unarmed
    assert_eq!(swing_anim_main(Some((4, 6))), 16); // non-weapon class
    assert_eq!(swing_anim_main(None), 16);
    // Offhand (HitInfo & 0x4): dagger pierces, weapons swing, empty punches.
    assert_eq!(swing_anim_off(Some((2, 0xf))), 88);
    assert_eq!(swing_anim_off(Some((2, 0))), 87);
    assert_eq!(swing_anim_off(Some((4, 6))), 117);
    assert_eq!(swing_anim_off(None), 117);
}

#[test]
fn ready_ids_bucket_differently_from_swings() {
    // Fist and dagger ready as 1H (`0x5fcdc0`), though they swing 16 and 85.
    assert_eq!(ready_anim(Some((2, 0xd))), 26);
    assert_eq!(ready_anim(Some((2, 0xf))), 26);
    assert_eq!(ready_anim(Some((2, 8))), 27); // 2H sword
    assert_eq!(ready_anim(Some((2, 6))), 28); // polearm
    assert_eq!(ready_anim(Some((2, 2))), 25); // bow
    assert_eq!(ready_anim(None), 25);
}

#[test]
fn ready_idle_only_while_standing() {
    // Standing and engaged: the Ready idle, with the unarmed and Stand fallbacks.
    assert_eq!(
        gait_candidates(&MovementState::default(), 2.5, Some(26), None),
        &[26, 25, 0]
    );
    // The caller drops the Ready while moving: locomotion outranks it.
    assert_eq!(gait_candidates(&moving_forward(3.0), 2.5, None, None)[0], 4);
}

#[test]
fn reconcile_priority_is_stow_over_draw() {
    // `& 4` stows unconditionally, even engaged (swimming mid-combat).
    assert_eq!(reconcile_sheath(1, 42, 4, true, true, 1, false), Some(0));
    // `& 0x10` stows before the engaged draw: fists need empty hands.
    assert_eq!(reconcile_sheath(1, 25, 0x10, true, true, 1, false), Some(0));
    // Engaged draws melee even on a flagless clip.
    assert_eq!(reconcile_sheath(0, 0, 0, true, true, 0, false), Some(1));
    // `& 0x20` draws without engagement (readying, fishing).
    assert_eq!(
        reconcile_sheath(0, 133, 0x20, false, true, 0, false),
        Some(1)
    );
    // No flags, not engaged: the local player is left alone (a manual toggle persists).
    assert_eq!(reconcile_sheath(1, 0, 0, false, true, 0, false), None);
    assert_eq!(reconcile_sheath(0, 0, 0, false, true, 1, false), None);
}

#[test]
fn reconcile_mounted_is_a_persistent_draw_block() {
    // Mounted stows on every recompute (`0x5fdfd9`), over the engaged draw, the `& 0x20` draw and
    // a remote's server byte.
    assert_eq!(reconcile_sheath(1, 0, 0, true, true, 1, true), Some(0));
    assert_eq!(
        reconcile_sheath(1, 133, 0x20, false, true, 1, true),
        Some(0)
    );
    assert_eq!(reconcile_sheath(1, 0, 0, false, false, 1, true), Some(0));
    // Already stowed: the caller's `forced != cur` gate skips the write.
    assert_eq!(reconcile_sheath(0, 0, 0, false, true, 0, true), Some(0));
}

#[test]
fn reconcile_ranged_exemption_and_remote_pull_through() {
    // Ranged-drawn, `0x5fe180`'s nine Load/Hold/Attack ids are exempt from the `& 0x10` stow…
    for anim in [46, 49, 105, 106, 107, 109, 110, 111, 112] {
        assert_eq!(reconcile_sheath(2, anim, 0x10, false, true, 2, false), None);
    }
    // …ReadyThrown 108 is not: it stows, and the driver's snap bracket keeps the thrown wind-up…
    assert_eq!(
        reconcile_sheath(2, 108, 0x10, false, true, 2, false),
        Some(0)
    );
    // …any other `& 0x10` clip stows while ranged-drawn (an emote lowers the bow)…
    assert_eq!(
        reconcile_sheath(2, 60, 0x10, false, true, 2, false),
        Some(0)
    );
    // …and the melee draw rules do not apply while ranged-drawn.
    assert_eq!(reconcile_sheath(2, 0, 0, true, true, 2, false), None);
    // A remote unit with no force takes the server byte (a swim ends, it redraws)…
    assert_eq!(reconcile_sheath(0, 0, 0, false, false, 1, false), Some(1));
    // …but the local player's committed state is never server-reconciled.
    assert_eq!(reconcile_sheath(0, 0, 0, false, true, 1, false), None);
}

#[test]
fn backpedal_rate_speeds_up_a_slow_design_speed() {
    // Backpedaling at 4.5 yd/s on a WalkBackwards authored at 2.5: 1.8×.
    assert!((playback_rate(&clip(13, 2.5), 4.5, 1.0) - 1.8).abs() < 1e-5);
    assert!((playback_rate(&clip(5, 7.0), 7.0, 1.0) - 1.0).abs() < 1e-5);
}

#[test]
fn non_locomotion_clips_play_at_unit_rate() {
    assert_eq!(playback_rate(&clip(0, 0.0), 9.0, 1.0), 1.0); // idle
    assert_eq!(playback_rate(&clip(38, 0.0), 9.0, 1.0), 1.0); // jump hang (moveSpeed 0)
    assert_eq!(playback_rate(&clip(60, 2.0), 9.0, 1.0), 1.0); // an id outside the scaled set
}

/// The `0x5fe2f0` divisor is `moveSpeed · |modelScale|`, not `moveSpeed` alone.
#[test]
fn a_big_model_cycles_its_legs_slower_for_the_same_ground_speed() {
    // The Gordok Ogre-Mage (creature 11443, display 12472): `CreatureModelScale` 2.2, walking at
    // vmangos' `speed_walk` 1.6 × 2.5 = 4.0 yd/s on a Walk (4) authored at 2.5; 1.60× scale-blind.
    assert!((playback_rate(&clip(4, 2.5), 4.0, 2.2) - 0.727_27).abs() < 1e-4);
    // The riding sabre (model 457, `CreatureModelScale` 1.5) at a 60% mount's 11.2 yd/s on a Run
    // (5) authored at 6.94: 1.08×, not the scale-blind 1.61×.
    assert!((playback_rate(&clip(5, 6.94), 11.2, 1.5) - 1.075_89).abs() < 1e-4);
    assert!((playback_rate(&clip(5, 6.94), 11.2, 1.0) - 1.613_83).abs() < 1e-4);
}

/// The guard tests the divisor, so scale 0 (`OBJECT_FIELD_SCALE_X` is server-set) plays at 1×,
/// and `|modelScale|` makes a negative scale mirror, never reverse.
#[test]
fn a_degenerate_model_scale_falls_through_to_unit_rate() {
    assert_eq!(playback_rate(&clip(4, 2.5), 4.0, 0.0), 1.0);
    assert!((playback_rate(&clip(4, 2.5), 4.0, -2.2) - 0.727_27).abs() < 1e-4);
}

/// The sign of `moveSpeed` is load-bearing and only the scale takes `abs()`: an authored backwards
/// gait is negative (`RidingKodo.m2` seq 14, WalkBackwards at -2.5, `benilla-extract m2seq`), so
/// the strict `divisor > 0` guard leaves it at 1×, while a model without one falls back to forward
/// Walk (+2.5) and is scaled.
#[test]
fn an_authored_backwards_gait_is_not_rate_scaled_but_its_fallback_is() {
    assert_eq!(playback_rate(&clip(13, -2.5), 4.5, 1.0), 1.0);
    assert_eq!(playback_rate(&clip(13, -2.5), 4.5, 2.2), 1.0);
    assert!((playback_rate(&clip(13, 2.5), 4.5, 1.0) - 1.8).abs() < 1e-5);
}

#[test]
fn ranged_load_idle_selects_by_weapon_and_ranks_below_ready() {
    // The `0x5fd530` LUT: ranged-slot subclass → the Load/Hold clip.
    assert_eq!(ranged_load_anim(Some((2, 2))), 105); // Bow → LoadBow
    assert_eq!(ranged_load_anim(Some((2, 3))), 106); // Gun → LoadRifle
    assert_eq!(ranged_load_anim(Some((2, 18))), 106); // Crossbow → LoadRifle
    assert_eq!(ranged_load_anim(Some((2, 16))), 112); // Thrown → LoadThrown
    assert_eq!(ranged_load_anim(Some((2, 19))), 111); // Wand → HoldThrown
    assert_eq!(ranged_load_anim(None), 25); // empty ranged slot → ReadyUnarmed
    assert_eq!(ranged_load_anim(Some((4, 1))), 25); // a non-weapon item → ReadyUnarmed

    // Standing with the idle armed: its own candidate arm, ReadyUnarmed the model fallback.
    let standing = MovementState::default();
    assert_eq!(
        gait_candidates(&standing, 2.5, None, Some(105)),
        &[105, 25, 0]
    );
    // The engaged melee Ready outranks it, though the two never co-occur: auto-shot sets no engaged
    // guid.
    assert_eq!(gait_candidates(&standing, 2.5, Some(26), Some(105))[0], 26);
    assert_eq!(
        gait_candidates(&moving_forward(3.0), 2.5, None, Some(105))[0],
        4
    );
    // It takes the bare-Stand slot, so it also blocks the state-emote idle.
    assert!(!is_bare_stand(gait_candidates(
        &standing,
        2.5,
        None,
        Some(105)
    )));
}

/// `0x5fd460` claims the drawn ranged idle ([`ranged_idle_gate`]) on the ranged sheath
/// (`cmp [+0xd40],2`) and the local auto-repeat bit `0x200` (`test ah,0x2`) alone. The any-caster
/// hold `0x400` is tested only by `0x5fc3f0`'s Hold sustain: one Serpent Sting sets it and no
/// volley end clears it, so admitting it would leave a shooter aiming.
#[test]
fn the_ranged_idle_is_entered_by_the_auto_repeat_bit_and_the_ranged_sheath_alone() {
    assert!(ranged_idle_gate(true, Some(2)));
    // It needs the ranged sheath (2): a melee-drawn or stowed unit is never in the family.
    assert!(!ranged_idle_gate(true, Some(1)));
    assert!(!ranged_idle_gate(true, Some(0)));
    assert!(!ranged_idle_gate(true, None));
    // Drawn ranged without auto-repeat, as after one Serpent Sting or on any remote shooter (only
    // the local cast-send sets `0x200`): no aim pose, whatever `0x400` says.
    assert!(!ranged_idle_gate(false, Some(2)));
}

/// The completion dispatch promotes each ranged Load to its family's Hold (slots 11, 12 and 15),
/// the cycle's one looping clip. A fire clip recomputes the base like any one-shot, through the
/// dispatcher's deferred fire site (`0x7075af`).
#[test]
fn each_ranged_load_promotes_to_its_weapon_familys_hold() {
    assert_eq!(ranged_hold_anim(105), 109); // LoadBow → HoldBow
    assert_eq!(ranged_hold_anim(106), 110); // LoadRifle → HoldRifle
    assert_eq!(ranged_hold_anim(112), 111); // LoadThrown → HoldThrown
    assert_eq!(ranged_hold_anim(111), 111); // the wand's idle IS the hold: it re-arms itself
    assert_eq!(ranged_hold_anim(25), 25); // no ranged weapon holds nothing: ReadyUnarmed stays
    assert_eq!(ranged_hold_anim(46), 46); // a fire id is no Load and maps onto no hold
}

#[test]
fn state_emote_idle_only_fills_the_bare_stand_slot() {
    assert!(is_bare_stand(gait_candidates(
        &MovementState::default(),
        2.5,
        None,
        None
    )));
    assert_eq!(state_emote_gait(200), [200, STAND]);

    assert!(!is_bare_stand(gait_candidates(
        &moving_forward(3.0),
        2.5,
        None,
        None
    )));
    let turning = MovementState {
        flags: move_flags::TURN_LEFT,
        ..Default::default()
    };
    assert!(!is_bare_stand(gait_candidates(&turning, 2.5, None, None)));
    let swimming = MovementState {
        flags: move_flags::SWIMMING,
        ..Default::default()
    };
    assert!(!is_bare_stand(gait_candidates(&swimming, 2.5, None, None)));
    assert!(!is_bare_stand(gait_candidates(
        &MovementState::default(),
        2.5,
        Some(26),
        None
    )));
    // The chair stand states 4, 5 and 6 have their own slot, above bare Stand.
    for stand_state in [4, 5, 6] {
        let chair = MovementState {
            stand_state,
            ..Default::default()
        };
        assert!(!is_bare_stand(gait_candidates(&chair, 2.5, None, None)));
    }
}

// ── The per-play one-shot route ──
// `esi` is the reference's route flag (`0x5fe6c8`). Ids: Attack1H 17 (a combat swing),
// EmoteApplaud 80 and EmoteBow 66 (waist-up emotes), EmoteCheer 68 (a full-body emote);
// `stand_state` 1 is seated.
use OneShotRoute::{FullBody, Masked};

#[test]
fn route_row1_standing_swing_is_full_body() {
    assert_eq!(route_oneshot(17, 0, 0), FullBody);
}

#[test]
fn route_row2_running_swing_is_masked() {
    // Moving (`[9e8] & 0x20003f`): `esi` 1, the SpineLow overlay over the run.
    assert_eq!(route_oneshot(17, move_flags::FORWARD, 0), Masked);
    // Every direction bit commits the legs, the keyboard turn keys in `0x3f` included.
    for f in [
        move_flags::BACKWARD,
        move_flags::STRAFE_LEFT,
        move_flags::TURN_LEFT,
        move_flags::TURN_RIGHT,
        move_flags::SWIMMING,
    ] {
        assert_eq!(route_oneshot(17, f, 0), Masked, "flag {f:#x}");
    }
}

#[test]
fn route_row3_midair_swing_is_masked() {
    assert_eq!(route_oneshot(17, move_flags::FALLING, 0), Masked);
    // The airborne test is combat-gated: an emote mid-jump is not masked.
    assert_eq!(route_oneshot(68, move_flags::FALLING, 0), FullBody);
}

#[test]
fn route_row4_seated_emote_is_masked() {
    assert_eq!(route_oneshot(80, 0, 1), Masked);
    assert_eq!(route_oneshot(70, 0, 1), Masked); // EmoteLaugh family
}

#[test]
fn route_row5_standing_clap_and_bow_are_full_body() {
    // 66 and 80 are not COMBAT: `esi` 0, bone 0; their waist-up look is authored, not routed.
    assert_eq!(route_oneshot(80, 0, 0), FullBody);
    assert_eq!(route_oneshot(66, 0, 0), FullBody);
}

#[test]
fn route_row6_standing_cheer_is_full_body() {
    // Routed as row 5; cheer's legs are authored large.
    assert_eq!(route_oneshot(68, 0, 0), FullBody);
}

#[test]
fn route_row7_seated_cheer_is_masked() {
    // Row 6's clip seated: `esi` 1, and the overlay never reaches the legs, which stay seated.
    assert_eq!(route_oneshot(68, 0, 1), Masked);
}

#[test]
fn route_land_row8_picks_land_clip_from_touchdown_input() {
    // Row 8 is `drive_animations`'s `Mode::Land`; its ids are the `0x602c60` pick.
    assert_eq!(jump_land_pick(0), Some(39));
    assert_eq!(jump_land_pick(move_flags::FORWARD), Some(187));
    assert_eq!(jump_land_pick(move_flags::BACKWARD), None);
    // `Mode::Land` re-picks on any flag change.
    assert!(!Special::Jump.interruptible_by_move());
}

#[test]
fn route_classifier_memberships_are_the_decoded_bytes() {
    // COMBAT (`0x5fcc10`) is every swing, no emote; CLASS_A (`0x5fed90`) has 17, 66, 68 and 80.
    assert!(is_combat(17));
    for id in [66, 68, 80] {
        assert!(!is_combat(id), "emote {id} must not be COMBAT");
    }
    for id in [16, 17, 18, 19, 85, 87, 88, 117] {
        assert!(is_combat(id) && is_class_a(id), "swing {id}");
    }
    for id in [17, 66, 68, 80] {
        assert!(is_class_a(id), "class-A {id}");
    }
    // Forced-full-body carve-outs never mask, even seated.
    for id in [1, 6, 131, 132, 57, 58, 118] {
        assert_eq!(route_oneshot(id, 0, 1), FullBody, "forced full-body {id}");
    }
}

#[test]
fn state_emote_idle_never_reached_during_a_special() {
    // `current_special` takes jumps and poses before `gait_candidates` runs, so the order is:
    // Special, every non-bare-Stand pick, the state-emote idle, Stand.
    let jumping = MovementState {
        flags: move_flags::FALLING,
        ..Default::default()
    };
    assert!(current_special(&jumping, true).is_some());
    for stand_state in [1u8, 3, 8] {
        let posed = MovementState {
            stand_state,
            ..Default::default()
        };
        assert!(current_special(&posed, false).is_some());
    }
}

#[test]
fn strafe_body_offset_matches_the_client_sign_fold() {
    use move_flags::{BACKWARD, FORWARD, STRAFE_LEFT, STRAFE_RIGHT};
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
    // Pure strafe: ±90°, left-positive.
    assert_eq!(strafe_body_offset(STRAFE_LEFT), FRAC_PI_2);
    assert_eq!(strafe_body_offset(STRAFE_RIGHT), -FRAC_PI_2);
    // Forward diagonal: ±45°.
    assert_eq!(strafe_body_offset(STRAFE_LEFT | FORWARD), FRAC_PI_4);
    assert_eq!(strafe_body_offset(STRAFE_RIGHT | FORWARD), -FRAC_PI_4);
    // A backpedal diagonal mirrors (the client's `((flags>>1)&2)==(flags&2)` fold): back-left
    // faces the body forward-right.
    assert_eq!(strafe_body_offset(STRAFE_LEFT | BACKWARD), -FRAC_PI_4);
    assert_eq!(strafe_body_offset(STRAFE_RIGHT | BACKWARD), FRAC_PI_4);
    // Not strafing (or both strafe keys cancelling): no offset.
    assert_eq!(strafe_body_offset(FORWARD), 0.0);
    assert_eq!(strafe_body_offset(0), 0.0);
    assert_eq!(strafe_body_offset(STRAFE_LEFT | STRAFE_RIGHT), 0.0);
}

#[test]
fn strafe_flip_always_swings_around_the_front() {
    use std::f32::consts::{FRAC_PI_2, PI};
    // The left-right flip is an exact 180° turn, where the absolute shortest arc ties; easing the
    // aim-relative offset keeps it within ±90° through the front at any aim, the ±π seam included.
    for aim in [0.0_f32, 1.3, PI - 0.01, -2.6] {
        for (from, to) in [(-FRAC_PI_2, FRAC_PI_2), (FRAC_PI_2, -FRAC_PI_2)] {
            // Converged exactly on the old pose, the tie.
            let mut yaw = super::super::wrap_pi(aim + from);
            for _ in 0..120 {
                yaw = ease_strafe_yaw(yaw, aim, to, 1.0 / 60.0);
                let off = super::super::wrap_pi(yaw - aim);
                assert!(
                    off.abs() <= FRAC_PI_2 + 1e-4,
                    "left the front arc: aim {aim}, {from}→{to}, offset {off}"
                );
            }
            let end = super::super::wrap_pi(yaw - aim);
            assert!((end - to).abs() < 1e-2, "did not converge: {end} vs {to}");
        }
    }
}

#[test]
fn base_arm_head_force_is_the_combat_carveout() {
    // The `0x5fdba0` re-zero gate: a relaxed arm rolls (-1)…
    assert!(!arm_forces_head(false, false, 0), "Stand → Stand re-arm");
    assert!(
        !arm_forces_head(false, false, 11),
        "Shuffle → Stand re-arm — the fidget trigger"
    );
    assert!(
        !arm_forces_head(false, false, 60),
        "an emote falling back to Stand"
    );
    // …and engagement, a cast hold, or an outgoing combat, cast or ready id forces the head.
    assert!(arm_forces_head(true, false, 0), "auto-attack target set");
    assert!(arm_forces_head(false, true, 0), "cast/channel hold");
    assert!(arm_forces_head(false, false, 17), "outgoing Attack1H");
    assert!(arm_forces_head(false, false, 26), "outgoing Ready1H");
    assert!(
        arm_forces_head(false, false, 53),
        "outgoing SpellCastDirected"
    );
}

#[test]
fn wound_id_by_severity_then_engagement() {
    // A crit outranks engagement, which picks 9 over 8 (`0x60ea70`).
    assert_eq!(wound_anim(0x2 | 0x80, false), 10);
    assert_eq!(wound_anim(0x2 | 0x80, true), 10);
    assert_eq!(wound_anim(0x2, true), 9);
    assert_eq!(wound_anim(0x2, false), 8);
}

#[test]
fn wound_route_full_body_on_ready_stance_or_stationary_standwound() {
    use move_flags::*;
    // Any id over a ready stance (25-29), whatever the move flags: the client tests bone 0's
    // armed record alone.
    assert!(wound_full_body(9, 26, 0, false));
    assert!(wound_full_body(10, 29, FORWARD, false));
    // StandWound stationary: the client's `0x20200f` mask has no turn bits, so turning in place
    // still routes full-body.
    assert!(wound_full_body(8, 0, 0, false));
    assert!(wound_full_body(8, 0, TURN_LEFT, false));
    assert!(!wound_full_body(8, 0, FORWARD, false));
    assert!(!wound_full_body(8, 0, FALLING, false));
    assert!(!wound_full_body(8, 0, SWIMMING, false));
    // Masked otherwise: mid-swing (the base is the swing), moving, or over a non-ready base.
    assert!(!wound_full_body(9, 17, 0, false));
    assert!(!wound_full_body(9, 5, FORWARD, false));
    assert!(!wound_full_body(9, 0, 0, false));
    // Mounted masks StandWound (the `[unit+0xdc] == 0` clause): a flinch never replaces the seat.
    assert!(!wound_full_body(8, 0, 0, true));
}

#[test]
fn wound_weight_decays_smoothstep_to_zero() {
    // Fresh: λ = 0.75, so w = 0.75/0.25 = 3 against a lone base (others = 1).
    assert!((wound_weight(1.0, 1.0) - 3.0).abs() < 1e-5);
    assert_eq!(wound_weight(0.0, 1.0), 0.0);
    // Monotone smoothstep decay in between.
    let mid = wound_weight(0.6, 1.0);
    let late = wound_weight(0.3, 1.0);
    assert!(wound_weight(1.0, 1.0) > mid && mid > late && late > 0.0);
    // The same λ against a heavier subtree (base 1 and a one-shot overlay 8) scales the weight.
    assert!((wound_weight(1.0, 9.0) - 27.0).abs() < 1e-4);
}

#[test]
fn defense_anim_matches_the_byte_lut() {
    // The `0x60ec98` LUT: dagger parries 1H (unlike its swing), fist unarmed (unlike its Ready),
    // ranged or none bails; dodge, deflect and block are fixed.
    assert_eq!(defense_anim(2, None), Some(30)); // dodge needs no weapon
    assert_eq!(defense_anim(8, Some((2, 7))), Some(30)); // deflect → Dodge too
    assert_eq!(defense_anim(5, None), Some(24)); // block → ShieldBlock
    assert_eq!(defense_anim(3, Some((2, 7))), Some(21)); // sword 1H
    assert_eq!(defense_anim(3, Some((2, 0xf))), Some(21)); // dagger → Parry1H
    assert_eq!(defense_anim(3, Some((2, 8))), Some(22)); // sword 2H
    assert_eq!(defense_anim(3, Some((2, 0xa))), Some(23)); // staff → Parry2HL
    assert_eq!(defense_anim(3, Some((2, 0xd))), Some(20)); // fist → ParryUnarmed
    assert_eq!(defense_anim(3, Some((2, 2))), None); // bow: the client bails
    assert_eq!(defense_anim(3, Some((15, 0))), None); // non-weapon mainhand: bail
    assert_eq!(defense_anim(3, None), None); // empty hand: bail
    assert_eq!(defense_anim(1, Some((2, 7))), None); // a landed hit defends nothing
    assert_eq!(defense_anim(0, Some((2, 7))), None); // a miss defends nothing
}

#[test]
fn swing_ids_cover_both_hands() {
    for id in [16, 17, 18, 19, 85, 87, 88, 117] {
        assert!(is_swing_id(id));
    }
    assert!(!is_swing_id(20)); // the defense clips are not swings
    assert!(!is_swing_id(30));
}

#[test]
fn a_flying_spline_is_fly_before_backward_and_speed() {
    // `0x5fd19c`: the fly branch sits between swim and backward, so a 32 yd/s taxi plays Fly 135,
    // never Sprint 143 or WalkBackwards.
    let fly = MovementState {
        flying: true,
        ..moving_forward(32.0)
    };
    assert_eq!(gait_candidates(&fly, 2.5, None, None), &[135, 0]);
    let fly_back = MovementState {
        flags: move_flags::BACKWARD,
        ..fly
    };
    assert_eq!(gait_candidates(&fly_back, 2.5, None, None), &[135, 0]);
    // A grounded spline ride (Charge) keeps the speed cascade.
    assert_eq!(
        gait_candidates(&moving_forward(32.0), 2.5, None, None)[0],
        143
    );
}

#[test]
fn unify_stamps_flying_from_the_live_spline_on_every_leg() {
    use std::time::{Duration, Instant};
    let spline = |grounded| crate::net::Spline {
        deck: None,
        points: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
        start: Instant::now(),
        duration: Duration::from_secs(10),
        id: 1,
        grounded,
        run_mode: true,
    };
    // Self leg: the live spline says flying, not the stored state, as the client reads the
    // spline flags at select time.
    let m = moving_forward(32.0);
    assert!(unify(Some(&m), None, Some(&spline(false)), false, None).flying);
    assert!(!unify(Some(&m), None, Some(&spline(true)), false, None).flying);
    assert!(!unify(Some(&m), None, None, false, None).flying);
    // Spline leg (a remote taxi): flying, FORWARD and a speed.
    let v = unify(None, None, Some(&spline(false)), false, None);
    assert!(v.flying && v.flags & move_flags::FORWARD != 0 && v.speed > 0.0);
}

/// Server-granted modes are bits of the `CMovement+0x40` word the selector tests, and the
/// reference re-selects when a grant lands (`0x6014ec push edi; call 0x60e480`, then
/// `push -1; call 0x5fd9e0`). Our own mover is exempt: its modes are the ack'd family's, already
/// in its `MovementState`.
#[test]
fn the_granted_modes_fold_into_the_flags_word_on_every_leg_but_our_own() {
    use crate::net::UnitMoveModes;
    use std::time::{Duration, Instant};

    let rooted = UnitMoveModes(move_flags::ROOT);
    let walking = UnitMoveModes(move_flags::WALK_MODE);

    // Creature leg: a standing NPC the server rooted.
    let v = unify(None, None, None, false, Some(&rooted));
    assert_eq!(v.flags & move_flags::ROOT, move_flags::ROOT);

    // Creature leg on a path: the granted bit rides alongside the spline's own FORWARD.
    let spline = crate::net::Spline {
        deck: None,
        points: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
        start: Instant::now(),
        duration: Duration::from_secs(10),
        id: 1,
        grounded: true,
        run_mode: true,
    };
    let v = unify(None, None, Some(&spline), false, Some(&walking));
    assert_eq!(
        v.flags,
        move_flags::WALK_MODE | move_flags::FORWARD,
        "the walk grant does not displace the spline's own travel bit"
    );

    // Remote leg: OR'd onto the pose's flags.
    let rm = crate::net::RemoteMotion {
        wow_pos: [0.0; 3],
        pending: std::collections::VecDeque::new(),
        orientation: 0.0,
        flags: move_flags::FORWARD,
        pitch: 0.0,
        speed: 7.0,
        vertical_velocity: 0.0,
        jump_xy_vel: [0.0; 2],
        fall_start_z: None,
        relay: Default::default(),
        last_apply_ms: 0.0,
        last_apply_pos: [0.0; 3],
    };
    let v = unify(None, Some(&rm), None, false, Some(&walking));
    assert_eq!(v.flags, move_flags::FORWARD | move_flags::WALK_MODE);

    // Self leg: untouched.
    let m = moving_forward(32.0);
    assert_eq!(
        unify(Some(&m), None, None, false, Some(&rooted)).flags,
        m.flags,
        "our own mover's modes are the ack'd family's; this word is the controller's"
    );
}

/// The creep (`0x5fd1d3`) precedes the whole speed tail (`0x5fd202`): a stealthed mover plays 119
/// at any speed.
#[test]
fn the_prowl_creeps_at_every_speed() {
    for speed in [0.5, 3.0, 7.0, 32.0] {
        let plain = moving_forward(speed);
        let creeping = MovementState {
            stealthed: true,
            ..plain
        };
        assert_eq!(
            gait_candidates(&creeping, 2.5, None, None)[0],
            STEALTH_WALK,
            "stealthed at {speed} yd/s must creep",
        );
        assert_ne!(
            gait_candidates(&plain, 2.5, None, None)[0],
            STEALTH_WALK,
            "unstealthed at {speed} yd/s must fall through to the speed tail",
        );
    }
    // Walk is row 119's `AnimationData` fallback.
    let creeping = MovementState {
        stealthed: true,
        ..moving_forward(7.0)
    };
    assert_eq!(
        gait_candidates(&creeping, 2.5, None, None),
        &[STEALTH_WALK, 4, 0],
    );
}

/// Swim, fly and backward precede the stealth branch in the cascade, so each wins while stealthed.
#[test]
fn swim_fly_and_backpedal_all_outrank_the_prowl() {
    let stealthed = |flags: u32, flying: bool| MovementState {
        speed: 5.0,
        flags,
        stealthed: true,
        flying,
        ..Default::default()
    };
    let pick = |s: MovementState| gait_candidates(&s, 2.5, None, None)[0];
    let swim = move_flags::SWIMMING;
    assert_eq!(
        pick(stealthed(swim | move_flags::FORWARD, false)),
        42,
        "a stealthed swimmer strokes",
    );
    assert_eq!(pick(stealthed(swim, false)), 41, "and treads water at rest");
    assert_eq!(
        pick(stealthed(move_flags::FORWARD, true)),
        135,
        "a stealthed flying ride still flies",
    );
    assert_eq!(
        pick(stealthed(move_flags::BACKWARD, false)),
        13,
        "backward is tested BEFORE stealth",
    );
}

/// The prowl idle is the chain's last resolver (`0x5fd830`): every other idle outranks it, and the
/// state-emote idle may still fill its slot.
#[test]
fn the_prowl_idle_is_the_lowest_priority_stand() {
    let idle = MovementState {
        stealthed: true,
        ..Default::default()
    };
    assert_eq!(
        gait_candidates(&idle, 2.5, None, None),
        &[STEALTH_STAND, STAND],
    );
    assert!(
        is_bare_stand(gait_candidates(&idle, 2.5, None, None)),
        "the state-emote idle still owns this slot",
    );
    let chair = MovementState {
        stand_state: 4,
        ..idle
    };
    assert_eq!(gait_candidates(&chair, 2.5, None, None), &[102, 0]);
    let turning = MovementState {
        flags: move_flags::TURN_LEFT,
        ..idle
    };
    assert_eq!(gait_candidates(&turning, 2.5, None, None), &[11, 0]);
    assert_eq!(
        gait_candidates(&idle, 2.5, Some(26), None),
        &[26, 25, 0],
        "an engaged Ready idle outranks the crouch",
    );
}

/// The creep is not in the rate-scaled set (`0x5fee80`), so it plays at 1× at any speed.
#[test]
fn the_prowl_creep_is_not_rate_scaled() {
    assert_eq!(playback_rate(&clip(STEALTH_WALK, 2.5), 7.0, 1.0), 1.0);
    assert_ne!(playback_rate(&clip(4, 2.5), 7.0, 1.0), 1.0);
}
