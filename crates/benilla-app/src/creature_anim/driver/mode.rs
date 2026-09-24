//! The **mode machine** — the `match` over [`AnimDriver::mode`] that decides what the base track
//! (bone 0) plays this frame, and the largest single phase of [`super::drive_animations`]'s
//! per-unit pass.
//!
//! It is the one phase with a clean input boundary, which is why it is the one that lifted out:
//! it reads a fixed set of already-computed per-frame facts ([`Frame`]) and writes only the
//! driver, the animation player and the transition set. In particular it touches **none** of the
//! frame-local flags the rest of the pass threads through itself (`base_played`, `masked_played`,
//! `played_oneshot`, `hold_played`, `sheath_frame_start`) — the seam is real, not just a line
//! count. The phases either side of it are not separable that way today; see decision 0933.
//!
//! The five modes and their transitions are documented on [`super::select::Mode`]; this file is
//! the executor, not the law.

use benilla_assets::ModelAnimations;
use benilla_formats::AnimDataCatalog;
use bevy::animation::transition::AnimationTransitions;
use bevy::prelude::*;

use crate::net::ObjectStore;
use crate::sound::EmoteSounds;

use super::super::select::{
    self, gait_candidates, is_bare_stand, playback_rate, ready_anim, state_emote_gait, Mode, DEATH,
    STAND,
};
use super::super::{find_resolved, move_flags, AnimDriver, CastHold, MovementState, Wielded};
use super::play::{enter_special, leave_special, oneshot_finished, play, play_clip, roll_loop};
use super::transplant_up;

/// Everything the mode machine reads about this unit this frame — the facts
/// [`super::drive_animations`]'s prologue has already computed. All `Copy`, so the machine
/// destructures it back into the same names the code was written against.
#[derive(Clone, Copy)]
pub(super) struct Frame<'a> {
    pub(super) entity: Entity,
    pub(super) anims: &'a ModelAnimations,
    pub(super) catalog: Option<&'a AnimDataCatalog>,
    pub(super) mv: MovementState,
    pub(super) special: Option<select::Special>,
    pub(super) moving: bool,
    pub(super) relaxed: bool,
    pub(super) mounted: bool,
    pub(super) looting: bool,
    pub(super) engaged: bool,
    pub(super) airborne_frozen: bool,
    pub(super) auto_repeat: bool,
    pub(super) cast_hold: Option<&'a CastHold>,
    pub(super) wielded: Option<&'a Wielded>,
    pub(super) store: Option<&'a ObjectStore>,
    pub(super) emote_sounds: Option<&'a EmoteSounds>,
    pub(super) walk: f32,
    pub(super) model_scale: f32,
    /// Whether this body's plays go to the `WOW_MOVE_TRACE` anim trace, and under which label —
    /// the rider's own, or the mount it is sitting on (decision 0906).
    pub(super) traced: bool,
    pub(super) subject: &'a str,
}

/// Run one frame of the mode machine over `drv`.
pub(super) fn run(
    f: Frame<'_>,
    drv: &mut AnimDriver,
    tr: &mut AnimationTransitions,
    player: &mut AnimationPlayer,
    rng: &mut benilla_assets::AnimRng,
) {
    let Frame {
        entity,
        anims,
        catalog,
        mv,
        special,
        moving,
        relaxed,
        mounted,
        looting,
        engaged,
        airborne_frozen,
        auto_repeat,
        cast_hold,
        wielded,
        store,
        emote_sounds,
        walk,
        model_scale,
        traced,
        subject,
    } = f;
    match drv.mode {
        Mode::Entering(sp) => {
            // The swim re-latch does NOT cut the hop's kick: JumpStart PLAYS OUT over the
            // re-latch and the swim gait resumes only at its end (decision 0517 —
            // director-corrected against the ref; the static byte reading, a cut where the freeze
            // gate `0x5fd8e8` releases, could not reproduce the screen and is unconfirmed live).
            // Only the swim re-latch holds — a ground landing, a water exit, or a new
            // Special still cuts (0503's snapshot-freeze).
            let swim_relatch_hold = sp == select::Special::Jump
                && special.is_none()
                && mv.flags & move_flags::SWIMMING != 0
                && !oneshot_finished(player, anims, sp.enter(), catalog);
            if swim_relatch_hold {
                // Hold: the kick keeps playing; the gait recompute waits at its end.
            } else if special != Some(sp) {
                // What we're entering is no longer wanted before the enter even finished — preempt
                // to the new Special, to the gait (a pose cut by movement), or to this one's exit.
                drv.mode = leave_special(
                    &mut drv.base_lock,
                    sp,
                    special,
                    moving,
                    relaxed,
                    mv.flags,
                    tr,
                    player,
                    anims,
                    catalog,
                    rng,
                    &mut drv.loop_window,
                    &mut drv.frozen,
                );
            } else if oneshot_finished(player, anims, sp.enter(), catalog) {
                play(
                    &mut drv.base_lock,
                    tr,
                    player,
                    anims,
                    sp.loop_id(),
                    true,
                    relaxed,
                    1.0,
                    catalog,
                    rng,
                    &mut drv.loop_window,
                );
                drv.mode = Mode::Looping(sp);
            }
        }
        Mode::Looping(sp) => {
            if special != Some(sp) {
                drv.mode = leave_special(
                    &mut drv.base_lock,
                    sp,
                    special,
                    moving,
                    relaxed,
                    mv.flags,
                    tr,
                    player,
                    anims,
                    catalog,
                    rng,
                    &mut drv.loop_window,
                    &mut drv.frozen,
                );
            }
        }
        Mode::Land { id, flags } => {
            // The jump landing (39/187) as a freely-overwritten pick (decisions 0083/0087 (d)).
            if let Some(sp) = special {
                // A new jump or a pose interrupts (a second jump, or sitting right after landing).
                drv.mode = enter_special(
                    &mut drv.base_lock,
                    sp,
                    relaxed,
                    tr,
                    player,
                    anims,
                    catalog,
                    rng,
                    &mut drv.loop_window,
                );
            } else if mv.flags != flags {
                // Any movement-flag change re-picks from live state *immediately* — land-then-press
                // runs, land-then-release stands, a direction flip drops the stale-direction land.
                drv.mode = Mode::Gait;
                drv.gait = None;
            } else if oneshot_finished(player, anims, id, catalog) {
                // Input held steady through the whole landing: fall through to a fresh gait pick.
                drv.mode = Mode::Gait;
                drv.gait = None;
            }
        }
        Mode::Exiting(sp, exit) => {
            // Pose stand-ups only now (Jump lands via `Mode::Land`, not this bracket).
            if let Some(next) = special {
                // A new Special interrupts the exit — re-sitting during the stand-up, say. Enter
                // it straight away instead of waiting the stand-up out.
                drv.mode = enter_special(
                    &mut drv.base_lock,
                    next,
                    relaxed,
                    tr,
                    player,
                    anims,
                    catalog,
                    rng,
                    &mut drv.loop_window,
                );
            } else if sp.interruptible_by_move() && moving {
                // Started moving mid stand-up: drop the rest, let the gait cross-fade take over.
                drv.mode = Mode::Gait;
                drv.gait = None;
            } else if oneshot_finished(player, anims, exit, catalog) {
                drv.mode = Mode::Gait;
                drv.gait = None; // recompute a fresh gait next frame
            }
        }
        Mode::Swing { id, under } => {
            if special != under {
                // The state this one-shot replaced CHANGED — the client's next event play
                // supersedes it: a fresh jump/pose entry (`None → Some`), the FALLINGFAR
                // latch's Fall (`Jump → Fall`), the `0x602c60` land pick at touchdown
                // (`Some → None`) — each a plain PlayAnimation over bone 0, never a
                // "restore". Leaving an airborne/pose `under` routes through
                // [`leave_special`] exactly like the un-replaced machine: the latch
                // handoff, the landing pick, and the pose exits all apply unchanged.
                //
                // …but FIRST the **transplant** (decision 0878): when what the base is about
                // to play is a LOCOMOTION clip — a jump entry (37), a land pick (39/187) — and
                // this one-shot is a live CAST/COMBAT clip, the client moves it up onto the
                // key-bone rather than letting the request overwrite it. Fall(40) and the pose
                // enters/exits are NOT locomotion ids, so a FALLINGFAR latch or a sit-down
                // still replaces the clip on bone 0, exactly as the bytes order it.
                let incoming_locomotion = match (under, special) {
                    (_, Some(next)) => select::is_locomotion(next.enter()),
                    (Some(select::Special::Jump | select::Special::Fall), None) => {
                        select::jump_land_pick(mv.flags).is_some_and(select::is_locomotion)
                    }
                    _ => false,
                };
                if incoming_locomotion {
                    transplant_up(drv, player, tr, anims, entity, id);
                }
                drv.mode = if let Some(sp) = under {
                    leave_special(
                        &mut drv.base_lock,
                        sp,
                        special,
                        moving,
                        relaxed,
                        mv.flags,
                        tr,
                        player,
                        anims,
                        catalog,
                        rng,
                        &mut drv.loop_window,
                        &mut drv.frozen,
                    )
                } else if let Some(sp) = special {
                    enter_special(
                        &mut drv.base_lock,
                        sp,
                        relaxed,
                        tr,
                        player,
                        anims,
                        catalog,
                        rng,
                        &mut drv.loop_window,
                    )
                } else {
                    Mode::Gait // unreachable: special != under with under None ⇒ special Some
                };
            } else if matches!(under, Some(select::Special::Jump | select::Special::Fall)) {
                // The airborne-freeze (`0x5fd8e8` keep-current): mid-arc nothing re-picks
                // bone 0 — a finished clip clamps and holds its last frame for the rest of
                // the arc (the clamp at `0x7145db`), and a mid-air flag change is a keep-current
                // no-op. The only exits are the edges above: the FALLINGFAR latch's Fall
                // and the land pick at touchdown. This holds over Fall too: 0864's per-tick
                // Fall(40) re-assert was refuted (decision 0868 — Fall plays ONCE, at the
                // latch edge `0x61a820@0x61a9eb`; `0x5ff030` is a wire-apply path, not a
                // tick), so a clip that takes bone 0 after the latch holds until landing.
            } else if let Some(sp) = under {
                // A pose held under the one-shot: on finish, back to the pose LOOP directly
                // (decision 0083 (c) — the enter never replays after an interruption).
                if oneshot_finished(player, anims, id, catalog) {
                    play(
                        &mut drv.base_lock,
                        tr,
                        player,
                        anims,
                        sp.loop_id(),
                        true,
                        relaxed,
                        1.0,
                        catalog,
                        rng,
                        &mut drv.loop_window,
                    );
                    drv.mode = Mode::Looping(sp);
                }
            } else if oneshot_finished(player, anims, id, catalog) || mv.flags != drv.gait_flags {
                // A finished one-shot recomputes the base — the ranged FIRE clips included
                // (decision 1544, superseding 0994 §1). 0994 held them out on the claim that
                // the completion dispatcher `0x5fc3f0` is never reached for a bow id; it is,
                // through a second fire site — the natural-completion path
                // enqueues the callback as a plain ARGUMENT (`0x7194f5` pushes mode 0) and
                // `0x7074b0` invokes it later as `call [esi+4]`, so a scan for
                // `call dword ptr [reg+0x70]` misses it by construction. 46/49/107 land on the
                // dispatcher's slot 22: a bare `RecomputeBaseAnim(-1)`.
                //
                // That recompute is the whole mid-volley cycle: for an armed shooter the base
                // re-picks the Load clip, whose own completion promotes to the Hold. Fire →
                // re-pull → hold, once per shot — and the re-pull's `$BWP` is what puts the
                // arrow back on the string, since it is the only clip that authors the tag.
                // Holding it out left every shot after the first firing from an empty hand
                // (bug B307).
                // Finished — or a movement-flag change: the client's base re-arm lands on the
                // change and blindly overwrites bone 0, one-shot or not (the same re-arm
                // decision 0280 named for the un-finishable looping kit clip, and
                // Mode::Land's flag-change re-pick). Holding the clip out instead slides the
                // post-shot runner over the ground on straight legs (director-observed vs
                // ref). An EDGE, not a level — the split-boneless masked fallback enters
                // here already moving, and steady flags must let that clip play out.
                //
                // Against `drv.gait_flags` — what the **base** was last armed for — not this
                // one-shot's own arm-time `flags` (decision 0894). The reference keeps no
                // per-one-shot latch: a movement-state change requests the base and bone 0
                // takes it, whenever the one-shot happened to start. Ice Block is the case that
                // separates them — its root wipes the direction bits in the SAME frame the cast
                // one-shot arrives, so the arm-time compare sees no edge ever and the cast held
                // bone 0 for the whole block; against the base's flags the edge is still there,
                // Stand overwrites the cast, and the character is neutral when the freeze lands.
                if mv.flags != drv.gait_flags {
                    // The locomotion re-arm is a normal PlayAnimation — the deferred-combat
                    // cache clears with it (decision 0406; a finished clip instead had its
                    // cache consumed by the injection above, before this machine ran).
                    drv.deferred = None;
                    // …and **if** the re-arm resolves to a LOCOMOTION id, a still-playing
                    // CAST/COMBAT clip transplants up to the key-bone instead of being
                    // overwritten (decision 0878; the client's gate is `0x5fee80` on the
                    // *requested* id, `0x5fe912`). A *finished* clip has nothing to move: the
                    // client's descriptor probe reports a completed slot as id −1 (`0x5fe1f0`
                    // reads the completion latch, not the armed record), so the transplant
                    // predicates never see it.
                    //
                    // The gate was missing here, and Ice Block is what it costs (decision
                    // 0894): a stun's root wipes the direction bits, so the flag change re-arms
                    // to **Stand(0)** — not locomotion — and the reference *overwrites* the
                    // cast on bone 0, leaving the character fully neutral for the freeze to
                    // catch. Transplanting unconditionally moved it to the torso instead and
                    // froze an arm out.
                    if select::gait_is_locomotion(&mv, walk) {
                        transplant_up(drv, player, tr, anims, entity, id);
                    }
                }
                drv.mode = Mode::Gait;
                drv.gait = None; // recompute a fresh gait next frame
            }
        }
        Mode::Gait => {
            if let Some(sp) = special {
                // Enter a Special state.
                drv.mode = enter_special(
                    &mut drv.base_lock,
                    sp,
                    relaxed,
                    tr,
                    player,
                    anims,
                    catalog,
                    rng,
                    &mut drv.loop_window,
                );
                drv.gait = None;
            } else if airborne_frozen && drv.gait.is_some_and(|g| g != DEATH) {
                // The airborne-freeze on the STEP-OFF arc — the exact gate
                // (`0x5fd8e8`: `FALLING && (FALLINGFAR || vz ≠ 0)`, decisions 0864/0868;
                // the selector chain's leg right after death): mid-air the selector never
                // re-picks, so the takeoff-frozen gait keeps rolling mid-cycle AND the live
                // pins further down the chain (the stationary cast hold, the loot kneel, the
                // Ready/ranged idles, the state-emote idle) cannot swap the clip until
                // touchdown. Rate stays synced — the client's per-frame rate write is outside
                // the selector, so it is [`play::sync_base_rate`]'s below and this arm does
                // nothing at all. (DEATH is excluded: the dead-override owns that gait, and a
                // mid-air revive must re-select, not hold the corpse pose.)
            } else {
                // A bracket-less step-off fall landing needs NO case of its own: the arc never
                // latched FALLINGFAR, so the `0x602c60` land dispatcher is a verified no-op
                // (decision 0179) — and a no-op means the gait must keep rolling mid-cycle,
                // not be re-picked (a re-pick replays the run cycle from its head: the
                // landing-frame leg pop, decision 0187). Falling through keeps the clip when
                // the flags still agree and cross-fades normally when they changed mid-air.
                // Normal gait: select, cross-fade on change, keep the rate synced each frame.
                // The engaged standing idle: the weapon-class Ready pick (decision 0073).
                // `GetWeapon(0, 0)` again ([`Wielded::armed_main`]) — `0x5fcdc0` passes
                // visFlag 0, so a disarmed unit stands in ReadyUnarmed(25) (decision 1863).
                let ready =
                    (engaged && !moving).then(|| ready_anim(wielded.and_then(|w| w.armed_main())));
                // The ranged standing idle (0099 phase 5): the byte-verified entry gate
                // ([`select::ranged_idle_gate`]) → the ranged weapon's Load clip, played
                // ONCE, then promoted to the Hold by its own completion. ENTRY is the local
                // `0x200` ([`auto_repeat`]) alone — `0x5fd460`'s own and only claim test
                // besides the sheath. The HOLD twin is real and this is where it comes back
                // (decision 1544, superseding 0994 §2, which deleted it on a refuted absence
                // proof): 105 → 109, 106 → 110, 112 → 111, unconditionally at the Load's
                // completion. Mid-volley the base IS re-picked — every fire clip's completion
                // recomputes — so the cycle is fire → re-pull → hold, per shot.
                let ranged_load =
                    (!moving && select::ranged_idle_gate(auto_repeat, drv.sheath_cur)).then(|| {
                        let load = select::ranged_load_anim(wielded.and_then(|w| w.ranged));
                        let hold = select::ranged_hold_anim(load);
                        // The pull's own completion promotes it to the Hold — the dispatcher's
                        // slot 11/12/15 arm, UNCONDITIONAL ([`select::ranged_hold_anim`] carries
                        // which gate belongs to whom). Expressed as a CANDIDATE rather than a
                        // latch: once armed, the Hold re-selects itself every frame, and the
                        // volley's end drops `ranged_idle_gate` and recomputes straight out of it
                        // — so there is no second piece of state to keep in step, which is what
                        // 0409's re-latch was and why 0994 was right to delete that much.
                        let holding = drv.gait == Some(hold)
                            || (drv.gait == Some(load)
                                && oneshot_finished(player, anims, load, catalog));
                        if holding {
                            hold
                        } else {
                            load
                        }
                    });
                let cands = gait_candidates(&mv, walk, ready, ranged_load);
                // The stationary cast/channel hold pins its pose **full-body in the gait slot**
                // (decision 0107 — the client's `[CGUnit+0xb4]` stationary-cast gate),
                // outranking the Ready idle and the state-emote idle below. "Stationary" is
                // the client's `[9e8] & 0x20000f` test ([`move_flags::CAST_PIN_MOVE`]:
                // translation + swim, NEVER the turn bits) — a turning caster keeps the pin,
                // feet sliding; only a translating/swimming one falls through to the masked
                // hold overlay (the hold block after the mode machine). Testing the turn bits
                // here was the frostbolt right-drag jitter (decision 0491).
                let hold_cands;
                let cands: &[u16] = match cast_hold {
                    Some(h) if mv.flags & move_flags::CAST_PIN_MOVE == 0 => {
                        hold_cands = [h.anim_id, STAND];
                        &hold_cands
                    }
                    _ => cands,
                };
                // The looping state-emote idle (`UNIT_NPC_EMOTESTATE`: `/dance`, NPC
                // cooking/sweeping flavor loops): fills exactly the bare-Stand slot
                // (`is_bare_stand`) — everything that already outranks Stand (movement, turn,
                // swim, the Ready idle, a chair-loop stand-state) has already routed `cands`
                // elsewhere, and Special is handled entirely above this arm. Cleared field
                // (`unit_emote_state() == 0`, or the catalog has no `AnimID` for it) falls
                // straight through to `cands` unchanged — back to Stand.
                let state_emote_cands;
                let cands: &[u16] = if is_bare_stand(cands) {
                    let emote_anim = store
                        .and_then(|s| emote_sounds.and_then(|e| e.anim(s.0.unit_emote_state())));
                    match emote_anim {
                        Some(id) => {
                            state_emote_cands = state_emote_gait(id as u16);
                            &state_emote_cands
                        }
                        None => cands,
                    }
                } else {
                    cands
                };
                // The loot kneel (see [`select::LOOT`]): while the loot trigger is up
                // (self: the latch; remote: the flag — the `looting` predicate above) on a
                // stationary, unmounted unit, the gait slot holds Loot 50 — over the cast
                // pin, the Ready/ranged idles, the chair loops and the state-emote idle
                // alike (the `0x5fd8b0` chain order: locomotion → LOOT →
                // standState → combat/channel). The trigger dropping cross-fades back to
                // whatever the slot picks next.
                let loot_cands;
                let cands: &[u16] = if looting {
                    loot_cands = [select::LOOT, STAND];
                    &loot_cands
                } else {
                    cands
                };
                // The mounted pin outranks the whole gait slot (decision 0442 confirms 0441's
                // B1): the rider holds Mount(91) — moving, turning, engaged — while the mount
                // child's own driver plays the locomotion this selector would have picked. The
                // real client has no mount leg in this chain at all — it arms 91 once at
                // attach (`0x607b44`) and re-forces it on every PlayAnimation
                // (`0x5fe803`–`0x5fe816`) instead of selecting it here. The pin renders that
                // **steady state** exactly; what it does NOT render is the *attach* arm's
                // displacement of a full-body one-shot already holding bone 0 (the pin waits
                // on `Mode::Swing`, the reference's play does not) — which is why the
                // transition edge above arms separately (decision 0927, B203). 91 is not
                // rate-scaled, and the resolver's Stand fallback covers a body that doesn't
                // author it.
                let mount_cands;
                let cands: &[u16] = if mounted {
                    mount_cands = [select::MOUNT, STAND];
                    &mount_cands
                } else {
                    cands
                };
                let target = cands[0];
                // **The turn-shuffle is released by its own clip window, not by the turn ending**
                // (decision 1655).
                //
                // The client's only per-frame poll of a standing unit's base animation is
                // `0x607ed0`'s tail, and it *refuses* this exact transition: with the shuffle
                // armed and no turn bit set it computes target 0, sees `0xb != 0`, and calls
                // `0x5fce30` — whose first gate (`0x5fce33`/`0x5fce42`) needs `moveflags & 0x30`
                // or `d58 & 0x1800`, precisely what just went false. `0x6084da je` then skips the
                // recompute. **The tail can only START a shuffle, never stop one.** What stops it
                // is the animation-completion callback every in-world unit installs
                // (`0x60781b`), one clip window later — which is why the reference plays a
                // discrete little step rather than a stub of one: with `replay = (0,0)` on all
                // 1130 shipped shuffle records the window is exactly one span, 500 ms on 1100 of
                // them, so the step is `ceil(t_turn / span) · span` long however brief the turn.
                //
                // Only against **Stand**. Every other target — a walk, the mount pin, the cast
                // hold, the Ready idle, a state emote, the loot kneel — arrives in the client
                // through one of `0x5fd9e0`'s 37 edge-triggered call sites, which fire regardless
                // of the turn bits and preempt the shuffle at once. The refusal is the tail's
                // alone, and the tail's target is a hardcoded 0.
                let target = match drv.gait {
                    Some(g @ (select::SHUFFLE_LEFT | select::SHUFFLE_RIGHT))
                        if target == STAND && !window_complete(drv, player) =>
                    {
                        g
                    }
                    _ => target,
                };
                // Each `0x5fd8b0` candidate, in priority order, resolved through the model's own
                // baked fallback (decision 0082) before moving to the next candidate — a model
                // missing the exact id still plays its baked substitute rather than stepping
                // down the selector's own list early. The state-emote idle's id (above) resolves
                // through the same call, like every other candidate.
                let clip = cands
                    .iter()
                    .find_map(|&id| find_resolved(anims, id, catalog));
                // Did the base actually take a clip this pass? Only the lock can say no, and
                // when it does the bookkeeping below must not run ([`play::BaseAnimLock`]).
                let mut armed = true;
                if drv.gait == Some(target) {
                    // The gait is already armed and stays armed — its rate is
                    // [`play::sync_base_rate`]'s per-frame write below, which finds whichever
                    // *rolled variation* (decision 0123) is actually the live one rather than
                    // sweeping every node of the id. A completed ranged Load simply clamps at
                    // full draw and stays there; nothing promotes it (0994).
                } else if let Some(c) = clip {
                    // **The two bypasses the reference keeps** (`0x5fc563`,
                    // `0x607b44`): the death poses and the mount attach
                    // write the arm primitive directly and are not gated by the base-anim lock. In
                    // benilla both arrive as ordinary gait targets, so they say so here — a body
                    // knocked flat and then killed must still fall dead, and mounting during a
                    // `Knockdown` overrides the clip.
                    if target == DEATH || target == select::MOUNT {
                        drv.base_lock.release();
                    }
                    // Held: the selector does not even roll. The reference re-picks the base on an
                    // EVENT (`0x5fd9e0(-1)` from a packet handler), never once a frame, so a
                    // per-frame retry while the lock refuses would burn the shared variation RNG
                    // (decision 0114's single `_rand` stream) for the clip's whole 2 s and perturb
                    // every other unit's rolls with it. `drv.gait` stays `None`, so the pick
                    // happens exactly once — the frame the clip releases the lock.
                    if drv.base_lock.refuses() {
                        armed = false;
                    } else {
                        // A looping base arm rolls its variation when relaxed (decision 0123 —
                        // the client's base-arm `variationIdx = −1`; a combat/cast arm keeps the
                        // deterministic head) AND its replay budget (decision 0516, `0x712784`
                        // — the watchdog window). A re-armed Stand landing on its rare look-around
                        // variations IS the idle fidget.
                        let (c, budget) = roll_loop(anims, c, relaxed, rng);
                        if traced && benilla_assets::trace::enabled() {
                            // Every fresh gait play, including a same-clip replay (which the
                            // settled-state diff below cannot see) — the exact restart-from-head
                            // event a "frames snap" report is hunting.
                            benilla_assets::trace::line(
                                "anim",
                                &format!(
                                    "{subject}PLAY gait {} (was {:?}) rate {:.2}",
                                    c.anim_id,
                                    drv.gait,
                                    playback_rate(c, mv.speed, model_scale)
                                ),
                            );
                        }
                        // The ranged Load plays ONCE and freezes at full draw: as a Forever
                        // gait it WRAPPED to its head at completion — the frames + cross-fade
                        // against the restarted reach-to-quiver were the director's "jumps
                        // back to the start the moment it gets fully pulled", in every build
                        // that replayed a pull (trace-caught, decision 2276). The clamp IS the
                        // drawn pose; nothing follows it (0994).
                        // Loot 50 is likewise authored clamp — one 0.5 s kneel-down that must
                        // FREEZE in the rummage pose; as Forever it would wrap back to standing
                        // and re-kneel every half second.
                        let repeat = if select::is_ranged_load(target) || target == select::LOOT {
                            // A deliberate freeze — no window either: a watchdog re-pull is
                            // exactly what the reference's missing completion dispatch rules out.
                            drv.loop_window = None;
                            bevy::animation::RepeatAnimation::Never
                        } else {
                            drv.loop_window = Some((c.node, budget));
                            bevy::animation::RepeatAnimation::Forever
                        };
                        drv.gait_rate = playback_rate(c, mv.speed, model_scale);
                        let rate = drv.gait_rate;
                        armed = play_clip(&mut drv.base_lock, tr, player, c, repeat, rate);
                        if armed {
                            drv.gait = Some(target);
                        } else {
                            drv.loop_window = None;
                        }
                    }
                } else {
                    drv.gait = Some(target); // no clip (bind pose) — record the target so we don't churn
                }
                if armed {
                    // The base is now arm-consistent with this movement state — every branch above
                    // leaves `drv.gait == Some(target)`. A one-shot that displaces it later reads
                    // this, not its own arm-time flags, to know whether the movement state has
                    // moved on since (decision 0894).
                    drv.gait_flags = mv.flags;
                }
            }
        }
    }
}

/// Has the base slot's armed looping clip finished its **replay window** — the client's
/// `windowHi = windowLo + span·R` (`0x712784`, decision 0516)?
///
/// `true` when there is no window to wait on at all: a slot with nothing armed, or one whose
/// window a newer arm superseded, is not something to hold a shuffle against. The driver's own
/// watchdog reads the same pair one phase earlier and, in the gait slot, answers a completion by
/// clearing the target — so a `true` here and a re-selection there are the same event.
fn window_complete(drv: &AnimDriver, player: &AnimationPlayer) -> bool {
    drv.loop_window.is_none_or(|(node, budget)| {
        player
            .animation(node)
            .is_none_or(|a| a.completions() >= budget)
    })
}
