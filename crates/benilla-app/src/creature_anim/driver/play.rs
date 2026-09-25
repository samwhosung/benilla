//! Base-track playback: arming clips, the base-animation lock, entering and leaving Specials.

use std::time::Duration;

use benilla_assets::{AnimClip, ModelAnimations};
use benilla_formats::AnimDataCatalog;
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::RepeatAnimation;
use bevy::prelude::*;

use super::super::{find_resolved, AnimDriver};
use super::select::{self, jump_land_pick, Mode, Special};

/// The base-animation lock, the reference's `[unit+0xd58] & 0xc0000`: while it is held,
/// `CGUnit::PlayAnimation 0x5fe2f0` returns at once (`0x5fe3a1 jne 0x5fec29`), so a stun's
/// `Stand(0)` recompute (`0x5fda20`) is refused and a `Knockdown` runs its full 2000 ms. The arm
/// helper `0x5fdba0` sets it on the id actually armed (table `0x5fdd90`: 121 `Knockdown`, 192
/// `LiftOff`, 200 `Land`); `OnAnimationFinished 0x5fc9c6` clears it on completion or pre-emption,
/// as does a model rebuild (`0x60adee`). Held as the id, since a second arm would be refused.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BaseAnimLock(Option<u16>);

impl BaseAnimLock {
    /// The ids whose arm takes the lock (`0x5fdd90`).
    fn locking(id: u16) -> bool {
        matches!(id, select::KNOCKDOWN | select::LIFT_OFF | select::LAND)
    }

    /// Whether a play is refused: an unconditional early return, not a priority test (`0x5fe3a1`).
    pub(super) fn refuses(self) -> bool {
        self.0.is_some()
    }

    /// The setter, run on the id actually armed and only when the arm happened.
    fn took(&mut self, id: u16) {
        if Self::locking(id) {
            self.0 = Some(id);
        }
    }

    /// The clearer, keyed on the finished id (`0x5fc9c6`): run once a frame before anything
    /// re-picks the base, so a clip that completes or is pre-empted releases it.
    pub(super) fn release_finished(
        &mut self,
        player: &AnimationPlayer,
        anims: &ModelAnimations,
        catalog: Option<&AnimDataCatalog>,
    ) {
        if let Some(id) = self.0 {
            if oneshot_finished(player, anims, id, catalog) {
                self.0 = None;
            }
        }
    }

    /// Drops the lock outright, for the arms the reference never gates: the death pose
    /// (`0x5fc563`) and the mount attach (`0x607b44`). A model rebuild needs no call: it removes
    /// [`AnimDriver`].
    pub(super) fn release(&mut self) {
        self.0 = None;
    }
}

/// Cross-fades into an already-resolved clip unless the lock refuses, returning whether it armed;
/// `repeat` is always set, so a reused node keeps no stale count. On `false` the caller must not
/// record the clip as held: the reference's guard returns before the arm's `[bone+0xf8] = animId`.
pub(super) fn play_clip(
    lock: &mut BaseAnimLock,
    tr: &mut AnimationTransitions,
    player: &mut AnimationPlayer,
    c: &AnimClip,
    repeat: RepeatAnimation,
    rate: f32,
) -> bool {
    // The guard comes first. `arm` stays ungated: the reference reaches it past the guard from the
    // dismount, the sheath family and the weapon attach.
    if lock.refuses() {
        return false;
    }
    arm(
        tr,
        player,
        c,
        repeat,
        rate,
        Duration::from_secs_f32(c.blend_time.max(0.0)),
    );
    lock.took(c.anim_id);
    true
}

/// Arms a looping clip with no cross-fade, as the dismount teardown `0x607ce0` calls op4
/// `0x7121a0` with `crossFadeFlag = 0`: `0x71253e`–`0x712543` skips the blend block and its
/// primary-to-secondary copy (`0x7125d9`–`0x7125ea`), so the clip is the pose at once and a
/// full-body secondary overlay survives, while `[bone+0xf8] = animId` (`0x71252f`) still runs.
pub(super) fn cut_loop(
    tr: &mut AnimationTransitions,
    player: &mut AnimationPlayer,
    anims: &ModelAnimations,
    id: u16,
    catalog: Option<&AnimDataCatalog>,
    rng: &mut benilla_assets::AnimRng,
    window: &mut Option<(bevy::animation::graph::AnimationNodeIndex, u32)>,
) {
    let Some(head) = find_resolved(anims, id, catalog) else {
        return;
    };
    // op4's arg3 is a literal `-1` here (`0x607d25`): the variation always rolls.
    let (c, r) = roll_loop(anims, head, true, rng);
    *window = Some((c.node, r));
    arm(tr, player, c, RepeatAnimation::Forever, 1.0, Duration::ZERO);
}

fn arm(
    tr: &mut AnimationTransitions,
    player: &mut AnimationPlayer,
    c: &AnimClip,
    repeat: RepeatAnimation,
    rate: f32,
    blend: Duration,
) {
    let active = tr.play(player, c.node, blend);
    active.set_repeat(repeat);
    active.set_speed(rate);
}

/// A one-shot arm's rolls in op4's order: the variation (`0x71249a`), then the picked clip's replay
/// budget `R` (`0x712698`), which multiplies the play window and plays here as `Count(R)`.
pub(super) fn roll_oneshot<'a>(
    anims: &'a ModelAnimations,
    head: &'a AnimClip,
    rng: &mut benilla_assets::AnimRng,
) -> (&'a AnimClip, RepeatAnimation) {
    let c = anims
        .pick_variation(head.anim_id, rng.draw())
        .unwrap_or(head);
    let repeat = match rng.replay_count(c.replay) {
        r if r > 1 => RepeatAnimation::Count(r),
        _ => RepeatAnimation::Never,
    };
    (c, repeat)
}

/// A looping arm's variation (`variationIdx = −1`, `0x5fe697`): relaxed, the weighted walk (the
/// idle fidget); combat or cast, the head. Only the watchdog's re-arm re-rolls, never the kernel.
pub(super) fn pick_loop_variation<'a>(
    anims: &'a ModelAnimations,
    head: &'a AnimClip,
    relaxed: bool,
    rng: &mut benilla_assets::AnimRng,
) -> &'a AnimClip {
    if relaxed {
        anims
            .pick_variation(head.anim_id, rng.draw())
            .unwrap_or(head)
    } else {
        head
    }
}

/// A looping arm's rolls in op4's order (`0x71249a`, `0x712692`); the budget `R ∈ [min, max−1]`,
/// floored to 1, sets the watchdog window `windowHi = arm + span·R`.
pub(super) fn roll_loop<'a>(
    anims: &'a ModelAnimations,
    head: &'a AnimClip,
    relaxed: bool,
    rng: &mut benilla_assets::AnimRng,
) -> (&'a AnimClip, u32) {
    let c = pick_loop_variation(anims, head, relaxed, rng);
    let r = rng.replay_count(c.replay);
    (c, r)
}

/// Resolves `id` through the model's baked fallback and cross-fades into it with its rolls; a loop
/// publishes its `(node, R)` to `window` for the watchdog, a one-shot clears it.
pub(super) fn play(
    lock: &mut BaseAnimLock,
    tr: &mut AnimationTransitions,
    player: &mut AnimationPlayer,
    anims: &ModelAnimations,
    id: u16,
    looping: bool,
    relaxed: bool,
    rate: f32,
    catalog: Option<&AnimDataCatalog>,
    rng: &mut benilla_assets::AnimRng,
    window: &mut Option<(bevy::animation::graph::AnimationNodeIndex, u32)>,
) {
    // The guard returns (`0x5fe3a1`) before the arm helper `0x5fdba0` rolls, so a refused play
    // draws nothing from the one shared random stream.
    if lock.refuses() {
        return;
    }
    if let Some(c) = find_resolved(anims, id, catalog) {
        let (c, repeat) = if looping {
            let (c, r) = roll_loop(anims, c, relaxed, rng);
            *window = Some((c.node, r));
            (c, RepeatAnimation::Forever)
        } else {
            *window = None;
            roll_oneshot(anims, c, rng)
        };
        play_clip(lock, tr, player, c, repeat, rate);
    }
}

/// Writes the rate of whatever bone 0 holds, once a frame in every mode: the client's per-frame
/// write over the armed clip sits outside the selector (`0x5fe2f0`), and a creature's JumpLandRun
/// (187) resolves to its Run (5) through the model's lookup, a gait cycle that needs the rate.
/// Only a clip the scaler covers is written, so the fast path's 2× and the whiff's 0.5× survive;
/// the frozen node is skipped even so. [`AnimDriver::gait_rate`] gets the rate read back off the
/// node.
pub(super) fn sync_base_rate(
    drv: &mut AnimDriver,
    tr: &AnimationTransitions,
    player: &mut AnimationPlayer,
    anims: &ModelAnimations,
    speed: f32,
    model_scale: f32,
) {
    let Some(node) = tr.get_main_animation() else {
        return;
    };
    if drv.frozen != Some(node) {
        drv.frozen = None;
        if let Some(rate) = anims
            .clips
            .iter()
            .find(|c| c.node == node)
            .and_then(|c| select::scaled_rate(c, speed, model_scale))
        {
            if let Some(active) = player.animation_mut(node) {
                active.set_speed(rate);
            }
        }
    }
    drv.gait_rate = player.animation(node).map_or(1.0, |a| a.speed());
}

/// Whether one-shot `id`, resolved as [`play`] resolved it, has finished in every variation (the
/// play rolled one); a model with no clip for it counts as finished.
pub(super) fn oneshot_finished(
    player: &AnimationPlayer,
    anims: &ModelAnimations,
    id: u16,
    catalog: Option<&AnimDataCatalog>,
) -> bool {
    match find_resolved(anims, id, catalog) {
        Some(head) => anims
            .clips
            .iter()
            .filter(|c| c.anim_id == head.anim_id)
            .all(|c| player.animation(c.node).is_none_or(|a| a.is_finished())),
        None => true,
    }
}

/// Whether bone 0 holds `sp`'s own enter or loop clip, compared on the resolved id so a rolled
/// variation counts. The airborne freeze must still only that clip: a refused arc leaves the locked
/// clip on bone 0, and a stilled clip never finishes, so it would never release the lock.
pub(super) fn holds_own_clip(
    anims: &ModelAnimations,
    catalog: Option<&AnimDataCatalog>,
    sp: Special,
    node: bevy::animation::graph::AnimationNodeIndex,
) -> bool {
    let Some(cur) = anims.clips.iter().find(|c| c.node == node) else {
        return false;
    };
    [sp.enter(), sp.loop_id()]
        .into_iter()
        .filter_map(|id| find_resolved(anims, id, catalog))
        .any(|head| head.anim_id == cur.anim_id)
}

/// Enters Special `sp`, returning the mode to adopt. Fall has no enter: the client loops Fall(40)
/// from the tick FALLINGFAR latches (`0x602c40`).
pub(super) fn enter_special(
    lock: &mut BaseAnimLock,
    sp: Special,
    relaxed: bool,
    tr: &mut AnimationTransitions,
    player: &mut AnimationPlayer,
    anims: &ModelAnimations,
    catalog: Option<&AnimDataCatalog>,
    rng: &mut benilla_assets::AnimRng,
    window: &mut Option<(bevy::animation::graph::AnimationNodeIndex, u32)>,
) -> Mode {
    if sp == Special::Fall {
        play(
            lock,
            tr,
            player,
            anims,
            sp.loop_id(),
            true,
            relaxed,
            1.0,
            catalog,
            rng,
            window,
        );
        Mode::Looping(sp)
    } else {
        play(
            lock,
            tr,
            player,
            anims,
            sp.enter(),
            false,
            false,
            1.0,
            catalog,
            rng,
            window,
        );
        Mode::Entering(sp)
    }
}

/// Leaves Special `sp`, returning the mode to adopt: a new Special's entry, the land pick after an
/// airborne state, the gait for a pose cut by movement, else `sp`'s exit one-shot.
pub(super) fn leave_special(
    lock: &mut BaseAnimLock,
    sp: Special,
    special: Option<Special>,
    moving: bool,
    relaxed: bool,
    flags: u32,
    tr: &mut AnimationTransitions,
    player: &mut AnimationPlayer,
    anims: &ModelAnimations,
    catalog: Option<&AnimDataCatalog>,
    rng: &mut benilla_assets::AnimRng,
    window: &mut Option<(bevy::animation::graph::AnimationNodeIndex, u32)>,
    frozen: &mut Option<bevy::animation::graph::AnimationNodeIndex>,
) -> Mode {
    // Stills the cut airborne clip, so the gait fades in over a held kick as the reference shows.
    // How the reference holds it is untraced, and it is no pose snapshot (its blend source keeps
    // its own clock: `0x7125ea`, `0x7146b2`–`0x7147a5`), so this must not spread to other
    // cross-fades. Only the arc's own clip is stilled; `frozen` names it for the rate write.
    if matches!(sp, Special::Jump | Special::Fall) {
        if let Some(node) = tr.get_main_animation() {
            if holds_own_clip(anims, catalog, sp, node) {
                if let Some(active) = player.animation_mut(node) {
                    active.set_speed(0.0);
                    *frozen = Some(node);
                }
            } else if benilla_assets::trace::enabled() {
                // Traced, since a declined freeze is otherwise invisible.
                let held = anims
                    .clips
                    .iter()
                    .find(|c| c.node == node)
                    .map(|c| c.anim_id);
                benilla_assets::trace::line(
                    "anim",
                    &format!(
                        "anim freeze DECLINED leaving {sp:?}: bone 0 holds {held:?}, not this \
                         arc's clip"
                    ),
                );
            }
        }
    }
    if let Some(next) = special {
        enter_special(lock, next, relaxed, tr, player, anims, catalog, rng, window)
    } else if matches!(sp, Special::Jump | Special::Fall) {
        // The land pick (`0x602c60`) from the flags at touchdown, freely overwritten: any flag
        // change re-picks. A backpedal or walk landing picks none and the gait starts at once.
        match jump_land_pick(flags) {
            Some(id) => {
                play(
                    lock, tr, player, anims, id, false, false, 1.0, catalog, rng, window,
                );
                Mode::Land { id, flags }
            }
            None => Mode::Gait,
        }
    } else if sp.interruptible_by_move() && moving {
        Mode::Gait
    } else {
        let exit = sp.exit();
        play(
            lock, tr, player, anims, exit, false, false, 1.0, catalog, rng, window,
        );
        Mode::Exiting(sp, exit)
    }
}
