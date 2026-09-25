//! The mode machine: what the base track (bone 0) plays this frame. It reads only [`Frame`], none
//! of the pass's frame-local flags, and writes the driver, the player and the transitions.

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

/// The per-frame facts the mode machine reads, computed by [`super::drive_animations`] first.
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
    /// Whether this body's plays go to the anim trace; `subject` labels them (rider or mount).
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
            // A swim re-latch lets JumpStart play out and the swim gait resume at its end, as the
            // reference plays it; the byte reading, a cut where `0x5fd8e8` releases, does not
            // reproduce that. A ground landing, a water exit or a new Special still cuts.
            let swim_relatch_hold = sp == select::Special::Jump
                && special.is_none()
                && mv.flags & move_flags::SWIMMING != 0
                && !oneshot_finished(player, anims, sp.enter(), catalog);
            if swim_relatch_hold {
                // Hold: the kick keeps playing; the gait recompute waits at its end.
            } else if special != Some(sp) {
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
            // The jump landing (39/187), a freely overwritten pick.
            if let Some(sp) = special {
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
                // Any movement-flag change re-picks from live state at once.
                drv.mode = Mode::Gait;
                drv.gait = None;
            } else if oneshot_finished(player, anims, id, catalog) {
                drv.mode = Mode::Gait;
                drv.gait = None;
            }
        }
        Mode::Exiting(sp, exit) => {
            // Pose stand-ups only; a jump lands through `Mode::Land`.
            if let Some(next) = special {
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
                drv.mode = Mode::Gait;
                drv.gait = None;
            } else if oneshot_finished(player, anims, exit, catalog) {
                drv.mode = Mode::Gait;
                drv.gait = None; // recompute a fresh gait next frame
            }
        }
        Mode::Swing { id, under } => {
            if special != under {
                // The replaced state changed: the next event play (a Special entry, the latch's
                // Fall, the `0x602c60` land pick) takes bone 0 plainly, but a live cast or combat
                // clip facing a locomotion request (37, 39/187) first transplants to the key-bone.
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
                // The airborne freeze (`0x5fd8e8`): nothing re-picks bone 0 mid-arc and a finished
                // clip clamps (`0x7145db`). Fall plays once, at its latch (`0x61a9eb`), never per
                // tick, so a clip armed after the latch holds until landing.
            } else if let Some(sp) = under {
                // A pose under the one-shot: on finish, straight back to its loop, never the enter.
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
                // A finished one-shot recomputes the base, fire clips too: bow ids reach the
                // dispatcher `0x5fc3f0` through its deferred site (`0x7194f5`, `0x7074b0`), and
                // 46/49/107 land on slot 22, `RecomputeBaseAnim(-1)`, so a shooter re-pulls.
                // A movement-flag edge re-arms the base over any one-shot; steady flags let it
                // play out. The edge is against `gait_flags`, what the base was armed for, as the
                // reference keeps no per-one-shot latch.
                if mv.flags != drv.gait_flags {
                    // The re-arm is a normal play, so the deferred-combat cache clears with it.
                    drv.deferred = None;
                    // Only a locomotion re-arm moves a live cast or combat clip to the key-bone
                    // (`0x5fee80` on the requested id, `0x5fe912`); Stand overwrites it, and a
                    // finished clip reads as id −1 (`0x5fe1f0`), so it never moves.
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
                // The step-off arc's airborne freeze, `0x5fd8e8` right after the death legs:
                // `FALLING && (FALLINGFAR || vz ≠ 0)` holds the takeoff gait against every pin
                // until touchdown. Death is excluded: a mid-air revive must re-select.
            } else {
                // The ordinary pick, which a step-off landing also takes: the land dispatcher
                // (`0x602c60`) plays nothing for an unlatched arc, so an unchanged target keeps its
                // clip (a re-pick would restart it) and changed keys cross-fade. A fresh walk-off's
                // vz == 0 substep, which the freeze gate leaves uncovered, takes this pick too.
                //
                // Ready pick (`0x5fcdc0`, visFlag 0): a disarmed unit stands in ReadyUnarmed(25).
                let ready =
                    (engaged && !moving).then(|| ready_anim(wielded.and_then(|w| w.armed_main())));
                // The ranged idle (`0x5fd460`), entered on the drawn ranged sheath and the local
                // auto-repeat bit `0x200` (`select::ranged_idle_gate`): the Load plays once and its
                // completion arms the Hold, 105/106/112 → 109/110/111.
                let ranged_load =
                    (!moving && select::ranged_idle_gate(auto_repeat, drv.sheath_cur)).then(|| {
                        let load = select::ranged_load_anim(wielded.and_then(|w| w.ranged));
                        let hold = select::ranged_hold_anim(load);
                        // The Load's completion arms the Hold unconditionally (slots 11/12/15); a
                        // candidate, not a latch, it re-selects itself until the volley ends.
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
                // The stationary cast/channel pin (`[CGUnit+0xb4]`), full-body over the Ready and
                // state-emote idles; stationary is `[9e8] & 0x20000f`, never the turn bits, so a
                // turning caster keeps it and a moving one gets the masked hold instead.
                let hold_cands;
                let cands: &[u16] = match cast_hold {
                    Some(h) if mv.flags & move_flags::CAST_PIN_MOVE == 0 => {
                        hold_cands = [h.anim_id, STAND];
                        &hold_cands
                    }
                    _ => cands,
                };
                // The looping state-emote idle (`UNIT_NPC_EMOTESTATE`: `/dance`, NPC work loops)
                // fills only the bare-Stand slot; whatever outranks Stand already routed `cands`.
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
                // The loot kneel (Loot 50) of a stationary, unmounted unit with the trigger up,
                // over the cast pin, the Ready and ranged idles, the chair loops and the state
                // emote: `0x5fd8b0` tries locomotion, then loot (`0x5fd260`), then the cast/channel
                // pin (`0x5fd2e0`) and the Ready idle (`0x5fd360`) before the rest.
                let loot_cands;
                let cands: &[u16] = if looting {
                    loot_cands = [select::LOOT, STAND];
                    &loot_cands
                } else {
                    cands
                };
                // The mounted pin: the rider holds Mount(91), the mount plays the locomotion. The
                // client arms 91 outside this chain, at attach (`0x607b44`) and on every play
                // (`0x5fe803`–`0x5fe816`); the mount edge arms the attach over a live one-shot.
                let mount_cands;
                let cands: &[u16] = if mounted {
                    mount_cands = [select::MOUNT, STAND];
                    &mount_cands
                } else {
                    cands
                };
                let target = cands[0];
                // A turn shuffle ends at its own clip window, not with the turn: `0x607ed0`'s
                // per-frame tail can start one but not stop it (`0x5fce30` needs a turn bit, so
                // `0x6084da je` skips), and the completion callback (`0x60781b`) ends it, so a
                // step lasts `ceil(t_turn / span) · span`. Only against Stand: other targets
                // arrive through `0x5fd9e0`'s edge sites at once.
                let target = match drv.gait {
                    Some(g @ (select::SHUFFLE_LEFT | select::SHUFFLE_RIGHT))
                        if target == STAND && !window_complete(drv, player) =>
                    {
                        g
                    }
                    _ => target,
                };
                // Each candidate in priority order, through the model's baked fallback first.
                let clip = cands
                    .iter()
                    .find_map(|&id| find_resolved(anims, id, catalog));
                // Whether the base took a clip; only the lock refuses, skipping the bookkeeping.
                let mut armed = true;
                if drv.gait == Some(target) {
                    // Already armed; the per-frame rate write keeps its rate.
                } else if let Some(c) = clip {
                    // The death pose (`0x5fc563`) and the mount attach (`0x607b44`) arm past the
                    // lock in the reference; here they are gait targets, so they drop it first.
                    if target == DEATH || target == select::MOUNT {
                        drv.base_lock.release();
                    }
                    // Locked: no roll. The reference re-picks on events (`0x5fd9e0(-1)`), so a
                    // per-frame retry would drain the shared random stream; it picks on release.
                    if drv.base_lock.refuses() {
                        armed = false;
                    } else {
                        // Variation (when relaxed) and budget, the watchdog window (`0x712784`).
                        let (c, budget) = roll_loop(anims, c, relaxed, rng);
                        if traced && benilla_assets::trace::enabled() {
                            // Every fresh gait play, same-clip replays included.
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
                        // The ranged Load and Loot 50 are authored clamp: played once, they hold
                        // their last frame (full draw, the rummage pose), where a loop would wrap.
                        let repeat = if select::is_ranged_load(target) || target == select::LOOT {
                            // No watchdog window either, so nothing re-arms them.
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
                    drv.gait = Some(target); // no clip (bind pose): record it so it does not churn
                }
                if armed {
                    // What the base was armed for: a later one-shot's flag edge tests against it.
                    drv.gait_flags = mv.flags;
                }
            }
        }
    }
}

/// Whether the base's looping clip has run its replay window, `windowHi = windowLo + span·R`
/// (`0x712784`); `true` when there is no window to wait on.
fn window_complete(drv: &AnimDriver, player: &AnimationPlayer) -> bool {
    drv.loop_window.is_none_or(|(node, budget)| {
        player
            .animation(node)
            .is_none_or(|a| a.completions() >= budget)
    })
}
