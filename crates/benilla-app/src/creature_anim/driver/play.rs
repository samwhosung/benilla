//! Playback primitives for [`super::drive_animations`]: resolving and cross-fading into a clip
//! ([`play_clip`], [`play`]), checking whether a one-shot has finished ([`oneshot_finished`]), and
//! leaving a Special state ([`leave_special`]) — split out of [`super`] as its own concern.

use std::time::Duration;

use benilla_assets::{AnimClip, ModelAnimations};
use benilla_formats::AnimDataCatalog;
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::RepeatAnimation;
use bevy::prelude::*;

use super::super::{find_resolved, AnimDriver};
use super::select::{self, jump_land_pick, Mode, Special};

/// The **base-animation lock** — the reference's `[unit+0xd58]` bits `0xc0000`, which are the whole
/// reason a Lashed player visibly falls over (correcting 2085's §3).
///
/// `CGUnit::PlayAnimation 0x5fe2f0` opens with a guard that returns having done nothing at all:
///
/// ```text
/// 5fe396  mov  eax,[ebx+0xd58]
/// 5fe39c  test eax,0xc0000
/// 5fe3a1  jne  0x5fec29        ; bare epilogue — no routing, no arm, no deferral
/// ```
///
/// The bits are set by `PlayAnimation`'s **own** arm helper `0x5fdba0`, in a tail keyed on the id
/// *actually armed* (`0x5fdcf7`'s byte table `0x5fdd90`): **121 `Knockdown` → `0x80000`**, **192
/// `LiftOff` / 200 `Land` → `0x40000`**. They are cleared in `CGUnit::OnAnimationFinished
/// `0x5fc9c6`, above that function's reason branch — so on natural completion *and* on pre-emption
/// — and again on a model rebuild (`0x60adee`).
///
/// So a stun's root **does** recompute the base and the selector **does** resolve `Stand(0)`, and
/// the `0x5fda20 call 0x5fe2f0(0)` that would overwrite the Knockdown is simply **refused**. The
/// clip runs its full 2000 ms. Order-independent, which is why it holds whatever the packet timing.
///
/// The reference carries two independent bits; a unit can only ever hold one, because the second
/// arm would itself be refused — so this models the holder as the id, which also expresses the
/// "keyed on the FINISHED id" clearing rule directly. The two weaker relatives (`0x4` on 39
/// `JumpEnd`, which only makes the fallback idle decline, and `0x8` on 127 `Birth`, which makes the
/// whole selector return keep-current) are separate mechanisms at their own sites and are not
/// modelled here.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BaseAnimLock(Option<u16>);

impl BaseAnimLock {
    /// The three ids whose arm takes the lock — `0x5fdd90`'s table, `0xc0000` half.
    fn locking(id: u16) -> bool {
        matches!(id, select::KNOCKDOWN | select::LIFT_OFF | select::LAND)
    }

    /// The guard: is a play refused right now? An **unconditional early return**, not a priority
    /// comparison and not a dedup (`0x5fe3a1`).
    pub(super) fn refuses(self) -> bool {
        self.0.is_some()
    }

    /// The setter, run on the id **actually armed** and only when the arm happened.
    fn took(&mut self, id: u16) {
        if Self::locking(id) {
            self.0 = Some(id);
        }
    }

    /// The clearer, keyed on the **finished** id. Called once a frame before anything re-picks the
    /// base, so completion and pre-emption both release it: an overridden clip's node still reports
    /// finished at the end of its own span, which is exactly the reference's "the base stays locked
    /// for the rest of the clip's window" when a mount arms over a knockdown (`0x607b44`).
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

    /// Drop the lock outright — the reference's other two clearing sites.
    ///
    /// A **model rebuild** needs no call here: benilla's rebuild removes [`AnimDriver`] itself
    /// (`entities::live_display`), so the lock goes with it and the unit comes back with the base
    /// unheld — which is the reference's own outcome at `0x60adee`, reached differently. What does
    /// call it is the pair of arms the reference never gates: the death pose (`0x5fc563`) and the
    /// mount attach (`0x607b44`).
    pub(super) fn release(&mut self) {
        self.0 = None;
    }
}

/// Cross-fade the player into an **already-resolved** clip over its blend-in time. `repeat` sets its
/// repetition (`Forever` for a loop, `Count(R)` for a rolled replay budget, `Never` for a plain
/// one-shot — always set, so a reused graph node never carries a stale count from a prior play);
/// `rate` sets its playback speed. The primitive [`play`] (and the gait picker, which already has
/// the resolved [`AnimClip`] in hand) both funnel through this so resolution never runs twice.
/// Returns whether the clip was actually armed — `false` is the lock's refusal, and a caller whose
/// bookkeeping says "the base now holds this" must not write it on a `false` (the reference's guard
/// returns before the arm's own `[bone+0xf8] = animId`, so nothing downstream believes it played).
pub(super) fn play_clip(
    lock: &mut BaseAnimLock,
    tr: &mut AnimationTransitions,
    player: &mut AnimationPlayer,
    c: &AnimClip,
    repeat: RepeatAnimation,
    rate: f32,
) -> bool {
    // The guard, at the very top — before any routing, descriptor build or slot election
    // ([`BaseAnimLock`]). `arm` below is the primitive the reference reaches past this guard from
    // the dismount, the sheath family and the weapon attach, so it stays ungated.
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

/// Arm an already-resolved **looping** clip with the cross-fade SUPPRESSED — op4 `0x7121a0` called
/// with `crossFadeFlag = 0`, which is a genuinely different arm, not a fast blend:
///
/// - the outgoing pose is **not** carried. `0x71253e`–`0x712543` tests arg6 and, when it is zero,
///   jumps clean past the whole blend block — including the `rep movsd` at `0x7125d9`–`0x7125ea`
///   that copies the primary block `[bone+0x98..]` into the secondary `[bone+0xc4..]`. So the new
///   clip's first frame IS the pose, immediately, and a **full-body secondary overlay survives**
///   (only the blended arm evicts it — decision 0114's shared-slot eviction is a *blend* law).
/// - the primary's own bookkeeping still runs: `[bone+0xf8] = animId` at `0x71252f` is gated on
///   arg7 (the slot selector) alone, so bone 0 changes hands either way.
///
/// The dismount teardown `0x607ce0` is the call site this exists for; the mount-up
/// build `0x607b44` passes `1` and takes [`play_clip`]'s ordinary cross-fade.
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
    // op4's arg3 here is a literal `-1` (`0x607d25`), so the variation rolls unconditionally — the
    // teardown is not one of the arms [`select::arm_forces_head`] pins to the head.
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

/// The one-shot arm's two rolls, in the client's order (op4: variation at `0x71249a`, replay count
/// at `0x712698`): pick the resolved id's **variation** (the `_rand()`-weighted
/// walk, alternating the 1H swing arcs), then roll the **replay budget** `R` from the picked
/// clip's `(minReplay, maxReplay)` — the client multiplies `R` into the play window; benilla
/// expresses the same window as a `Count(R)` repeat, which `is_finished` honors.
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

/// Pick a looping arm's **variation** (the client's base-arm `variationIdx = −1`,
/// `0x5fe697`): a **relaxed** arm makes the weighted `_rand` walk (the
/// same roll as a one-shot's — this is where a re-armed Stand lands on its rare look-around
/// variations, the fidget); a combat/cast arm is forced to the deterministic head
/// ([`select::arm_forces_head`] decides which, at the call site). The kernel itself never
/// re-rolls — the advance between variations is the WATCHDOG's re-arm ([`roll_loop`]'s window,
/// decision 0516), not a kernel cycle.
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

/// The looping arm's two rolls in op4's order (variation `0x71249a`, then the replay budget
/// `0x712692..` — the same two `_rand` sites as a one-shot's): the budget is live
/// for **loops** too — not as a repeat cap but
/// as the watchdog **window**, `R` clip-lengths wide (`windowHi = arm + span·R`). Returns the
/// armed clip + `R` = total passes before the watchdog re-arms (`R ∈ [min, max−1]` floored to 1
/// — `replayMax` is exclusive, and `(0,0)`/`(0,1)` both play exactly once, visibly).
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

/// Cross-fade into clip `id`, resolved through the model's own baked fallback first (
/// see [`find_resolved`]) so a model lacking `id` plays its baked substitute rather than nothing.
/// `looping` repeats it; `rate` sets its playback speed. No-op if resolution still comes up empty.
/// A **one-shot** (`!looping`) rolls the resolved id's **variation and replay budget** per play
/// ([`roll_oneshot`]); a looping play rolls its variation (when `relaxed` —
/// decision 0123) **and its budget** ([`roll_loop`]), publishing the armed
/// `(node, R)` into `window` for the watchdog's advance; a one-shot arm clears it (its budget is
/// the `Count` repeat — no window outlives the arm).
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
    // The lock's guard is `PlayAnimation`'s **front door**, above the arm helper that draws the
    // rolls (`0x5fe2f0`'s epilogue return is at `0x5fe3a1`; the `_rand()` sites live inside
    // `0x5fdba0`'s op4 call). So a refused play must not roll either: the variation and replay
    // draws come off the ONE shared stream, and rolling them for a body that
    // cannot move would perturb every other unit's picks for the clip's whole 2 s — the same
    // churn 2096 caught in the gait selector's live trace, at the event-play door instead.
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

/// Write the **playback rate** of whatever the full-body slot currently holds — run once per frame,
/// after the mode machine has settled, in every mode alike.
///
/// The client's rate write lives **outside the selector** (`0x5fe2f0`, per-frame over the armed
/// clip), so it is not the gait's private business: it applies to a landing, a bracket, a swing —
/// anything the base slot holds. Ours used to be two loops inside [`Mode::Gait`]'s arms, which left
/// every other mode playing at its arm-time literal `1.0` — and that is the jump-landing bug.
/// [`jump_land_pick`] requests JumpLandRun **187**, and **every creature model
/// resolves 187 → Run(5)** through its own baked PlayableAnimationLookup: Horse, Tiger (the druid
/// travel form) and Cat all carry `playable[187] = 5`; only character models author 187 itself. So
/// a mount's landing clip *is* its gallop cycle — a rate-scaled locomotion clip — and playing it at
/// 1× ran it at ~65% of the cadence a mount's 14 yd/s calls for (Horse Run moveSpeed 9.028) for the
/// clip's whole 0.8 s, snapping straight only when the gait re-picked after it. On foot the same
/// defect is invisible: the character's own 187 carries moveSpeed 6.944 against a run speed of 7.0,
/// so the correct rate *is* 1×.
///
/// It writes **only where the scaler applies** ([`select::scaled_rate`]) — a locomotion clip with
/// an authored design speed. Everything else keeps whatever armed it: the scaler is one rate
/// producer among several (the combat fast-path's 2×, the whiff's 0.5×, 0503's 0× freeze), and a
/// blanket `1.0` here stomps all three.
///
/// [`AnimDriver::frozen`] names the one node this must leave alone even so — the airborne snapshot
/// decision 0503 stopped on purpose ([`leave_special`]), whose clip *is* rate-scaled (Jump 38 is in
/// the locomotion set; it is only the real assets' `moveSpeed = 0` that would spare it) — and is
/// cleared as soon as anything else is armed.
///
/// It also records what the slot ended up running at in [`AnimDriver::gait_rate`] — the hover
/// card's `rate` readout and the trace's `rate=`. Read back off the node rather
/// than recomputed, so the instrument reports the swing's 2× or the freeze's 0× as faithfully as
/// it reports a gait.
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

/// Whether the one-shot clip `id` has finished playing (resolved through the model's own baked
/// fallback first, decision 0082 — matching [`play`], which is what started it) — or the model lacks
/// even the substitute, so the machine doesn't wait forever. Checked across the id's **variations**:
/// the play rolled one of them, and whichever it was, "finished" means no variation
/// of the id is still running.
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

/// Whether bone 0 still holds `sp`'s **own** clip — the arc's enter or its loop, resolved through
/// the model's baked fallback exactly as [`play`] resolved it when it armed and
/// compared on the *resolved* id, so a rolled variation counts as the same clip.
///
/// This is the predicate the airborne snapshot-freeze (scoped by 1566) never had.
/// The freeze exists to still **the cut airborne clip** before the landing cross-fades over it;
/// both its sites took whatever bone 0 happened to hold, on the unstated assumption that the arc's
/// own clip is what is there. The base-anim lock (2096) is the first thing that ever falsified it,
/// and it falsified it catastrophically: every play the arc asked for was refused, so bone 0 still
/// held the `Knockdown` that took the lock — and the landing stopped **that** dead. A clip stopped
/// dead never finishes, and the lock clears on the finished id, so the body stayed on its back for
/// the rest of the session with every later play refused (the director's report).
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

/// Enter the Special `sp`, returning the mode to adopt. A pose or a jump plays its enter one-shot
/// and settles through [`Mode::Entering`]; **Fall has no enter** — the client plays the Fall(40)
/// loop directly the tick FALLINGFAR latches (`0x602c40`) — so it goes straight to
/// [`Mode::Looping`] with a looping play.
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
        // Enter plays are one-shots — `relaxed` (a looping-arm concern) is moot for them.
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

/// Transition out of the Special flow `sp` this frame, given what the unit now wants (`special`,
/// `moving`). A *different* Special preempts with its own entry (a second jump cutting the first's
/// landing; a jump handing off to Fall when FALLINGFAR latches); a pose abandoned because the unit
/// started moving drops straight to the gait, letting the cross-fade carry the half-pose into the
/// walk; an airborne state landing plays its [`jump_land_pick`]; otherwise `sp` plays its graceful
/// exit one-shot, which [`super::drive_animations`] then waits out. Returns the mode to adopt.
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
    // Freeze the cut airborne clip before handing off, so the incoming gait fades in over a
    // still kick instead of one that actively retracts. On the swim re-latch (~0.24 s into the
    // 833 ms JumpStart) the clip's remaining frames are the leg RECOVERY, and ours read far
    // shorter than the reference's lingering mid-kick — the director's report behind 0503.
    //
    // **This is a symptom fix whose mechanism is open**. 0503 justified it with
    // "the client blends from a pose snapshot, universally", which the bytes REFUTE: the blend
    // source keeps running on its own clock — `0x7125ea` copies the outgoing track's base, rate
    // and bias, and the kernel re-derives its time every frame (`0x7146b2`–`0x7147a5`). It looks
    // still only when the source's own window has elapsed AND it is clamp-flagged, which the cut
    // JumpStart's has not. So do NOT generalise this to other cross-fades — that was 0503's
    // recorded follow-up and 1566 strikes it; it would freeze every gait and turn transition in
    // the client. It stays HERE because the director saw the symptom and their eye outranks a
    // derivation; what produces the reference's lingering kick is not yet known.
    // …and the frozen node is NAMED, so the per-frame rate write
    // ([`sync_base_rate`]) skips it instead of restarting the clock a line above just stopped.
    //
    // …and it is **the arc's own clip** that is stilled, never merely whatever bone 0 holds
    // ([`holds_own_clip`]) — the predicate this always meant and never wrote down.
    if matches!(sp, Special::Jump | Special::Fall) {
        if let Some(node) = tr.get_main_animation() {
            if holds_own_clip(anims, catalog, sp, node) {
                if let Some(active) = player.animation_mut(node) {
                    active.set_speed(0.0);
                    *frozen = Some(node);
                }
            } else if benilla_assets::trace::enabled() {
                // The declined freeze is TRACED, because the wedge it used to cause was
                // completely silent: nothing logs a clip being stopped, and a stopped clip that
                // holds the base-anim lock ends the unit's animation for the session (2098).
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
        // The landing is a plain, freely-overwritten pick (decisions 0083/0087 (d)): the clip
        // is chosen from the input *at touchdown* (`flags`) by the `0x602c60` dispatcher's rule,
        // and re-picked the instant any movement flag changes — not a non-preemptible bracket.
        // A backpedal/walk landing picks NO clip: the gait (WalkBackwards) starts the same frame.
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
