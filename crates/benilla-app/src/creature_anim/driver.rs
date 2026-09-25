//! The Bevy animation driver: [`drive_animations`] runs the state machine [`super::select`] picks,
//! as a fixed sequence of phases per unit (death, mount edge, sheath, one-shots, [`mode`], rate,
//! fades, trace, wound, sheath reconcile). The phases share many frame-local bindings, so they
//! stay in one function.

use std::time::Duration;

use benilla_assets::ModelAnimations;
use benilla_formats::AnimDataCatalog;
use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::transition::AnimationTransitions;
use bevy::prelude::*;

use crate::names::type_flags::DO_NOT_PLAY_WOUND_ANIM;
use crate::net::{
    ClientCommand, FacingStep, NetCommands, ObjectStore, RemoteMotion, SelfPlayer, Spline,
    UnitSpeeds,
};
use crate::sound::EmoteSounds;

use super::select::{
    self, current_special, defense_anim, is_swing_id, route_oneshot, swing_anim_main,
    swing_anim_off, unify, Mode, OneShotRoute, DEATH, DEFAULT_WALK_SPEED, STAND,
};
use super::sheath::{advance_sheath_ceremony, start_sheath_ceremony};
use super::{
    find_resolved, move_flags, AnimData, AnimDriver, AutoRepeatArmed, BaseAnimRecompute, CastHold,
    DefenseAnim, EmoteAnim, Engaged, MovementState, NockLatch, Overlay, OverlayFade, SheathRequest,
    SheathSwapMessage, SwingImpact, SwingMessage, SwingSlowdown, Wielded, WoundAnim,
};

mod grip;
mod mode;
pub(super) mod play;
#[cfg(test)]
mod tests;
mod wound;

pub(super) use grip::drive_hand_grip;
use play::{holds_own_clip, oneshot_finished, play_clip, roll_loop, roll_oneshot};
use wound::{wound_evict, wound_trigger, wound_upkeep, WoundEdge};

/// The masked overlay's weight over the base clip on the SpineLow subtree, where the unmasked base
/// still blends: the overlay dominates about 8:1, the sheath ceremony overlay's value too.
const ONESHOT_OVERLAY_WEIGHT: f32 = 8.0;

/// The key-bone fade-to-rest: a finished one-shot's held final frame fades back to bone 0 over a
/// fixed 150 ms (`0x5fcacb` → op4 `param_3 = -1`), not the clip's own blendTime.
const ONESHOT_RELEASE_FADE: f32 = 0.150;

/// Launch vertical speed (yd/s, up) above which an airborne arc is a jump (the 37/38 bracket), not
/// a step-off fall; a jump launches near 7.96 up, a step-off at zero or below.
const JUMP_ARC_MIN_UP: f32 = 0.5;

/// A one-shot play request from this frame's messages, resolved to an anim id per unit.
enum OneShotReq {
    Swing(u32),
    Emote(u16),
}

/// The live one-shot the combat fast path tests: the key bone's clip, else bone 0's (`0x5fe422`).
fn live_oneshot(
    drv: &AnimDriver,
    player: &AnimationPlayer,
    tr: &AnimationTransitions,
    anims: &ModelAnimations,
    catalog: Option<&AnimDataCatalog>,
) -> Option<(u16, AnimationNodeIndex)> {
    if let Some(ov) = drv.overlay {
        if player.animation(ov.node).is_some_and(|a| !a.is_finished()) {
            return Some((ov.id, ov.node));
        }
    }
    if let Mode::Swing { id: m, .. } = drv.mode {
        if !oneshot_finished(player, anims, m, catalog) {
            return tr.get_main_animation().map(|n| (m, n));
        }
    }
    None
}

/// Whether a one-shot is live on the unit; the missile queue polls it to launch a missile whose
/// release keyframe never fired when the cast ends (the reference's `0x5fc920` → `0x60c9b0`).
pub(crate) fn oneshot_is_live(
    drv: &AnimDriver,
    player: &AnimationPlayer,
    tr: &AnimationTransitions,
    anims: &ModelAnimations,
    catalog: Option<&AnimDataCatalog>,
) -> bool {
    live_oneshot(drv, player, tr, anims, catalog).is_some()
}

/// Retire the key-bone clip into its cross-fade over `total` seconds: op4 snapshots the outgoing
/// pose into the secondary (`+0x98 → +0xc4`). A fade still above λ = 0.5 (`0x7ffa24`) is not
/// re-seeded (`0x7125d4` arm, `0x7123a2` release); the superseded node is dropped.
fn retire_overlay(drv: &mut AnimDriver, player: &mut AnimationPlayer, total: f32) {
    let out = drv.overlay.take().map(|ov| ov.node);
    if drv.overlay_fade.is_some_and(|f| fade_lambda(&f) > 0.5) {
        if let Some(n) = out {
            player.stop(n);
        }
        return;
    }
    if let Some(prev) = drv.overlay_fade.take().and_then(|f| f.out) {
        player.stop(prev);
    }
    drv.overlay_fade = Some(OverlayFade {
        out,
        left: total,
        total,
    });
}

/// The cross-fade's λ this frame, decaying 1 → 0 over the window.
fn fade_lambda(f: &OverlayFade) -> f32 {
    select::blend_lambda(if f.total > 0.0 { f.left / f.total } else { 0.0 })
}

/// The key-bone cross-fade's advance (λ decay `0x714880`–`0x714923`, release `0x7147b9`). A
/// release fades the old node alone against the base, `w = W·λ / (1 + W·(1−λ))`; a re-arm splits
/// the share by λ as `primary + (secondary − primary)·λ` does, `W·λ` out and `W·(1−λ)` in.
fn overlay_fade_upkeep(drv: &mut AnimDriver, player: &mut AnimationPlayer, dt: f32) {
    let Some(mut f) = drv.overlay_fade else {
        return;
    };
    f.left = (f.left - dt).max(0.0);
    let lambda = fade_lambda(&f);
    let live = drv.overlay.map(|ov| ov.node);
    let w = ONESHOT_OVERLAY_WEIGHT;
    if let Some(a) = f.out.and_then(|n| player.animation_mut(n)) {
        a.set_weight(if live.is_some() {
            w * lambda
        } else {
            w * lambda / (1.0 + w * (1.0 - lambda))
        });
    }
    if let Some(a) = live.and_then(|n| player.animation_mut(n)) {
        a.set_weight(w * (1.0 - lambda));
    }
    if f.left <= 0.0 {
        if let Some(n) = f.out {
            player.stop(n);
        }
        if let Some(a) = live.and_then(|n| player.animation_mut(n)) {
            a.set_weight(w);
        }
        drv.overlay_fade = None;
    } else {
        drv.overlay_fade = Some(f);
    }
}

/// The transplant (`0x5fe919`): a locomotion request while bone 0 plays a live cast or combat
/// one-shot moves that clip, at its id, rate and elapsed position, onto the key bone with
/// `blendFlag = 0`, so the torso finishes the cast while the legs take the request. A no-op when
/// the key bone is armed (`0x5fe912` → `0x5fe930`). The moved clip repeats `Never`, dropping any
/// replay budget; every clip this fires for is authored `(0, 0)`, so `R = 1`.
fn transplant_up(
    drv: &mut AnimDriver,
    player: &mut AnimationPlayer,
    tr: &AnimationTransitions,
    anims: &ModelAnimations,
    entity: Entity,
    id: u16,
) -> bool {
    if drv.overlay.is_some() || !(select::is_cast_anim(id) || select::is_combat_anim(id)) {
        return false;
    }
    let Some(node) = tr.get_main_animation() else {
        return false;
    };
    // Read off the armed variation node, which also names the masked twin to resume on.
    let Some((seek, speed)) = player
        .animation(node)
        .filter(|a| !a.is_finished())
        .map(|a| (a.seek_time(), a.speed()))
    else {
        return false;
    };
    let Some(upper) = anims
        .clips
        .iter()
        .find(|c| c.node == node)
        .and_then(|c| c.upper_node)
    else {
        return false;
    };
    let active = player.play(upper);
    active.replay();
    active.set_repeat(bevy::animation::RepeatAnimation::Never);
    active.seek_to(seek);
    active.set_speed(speed);
    active.set_weight(ONESHOT_OVERLAY_WEIGHT); // `blendFlag = 0`: no cross-fade
    drv.overlay = Some(Overlay {
        node: upper,
        id,
        looping: false,
    });
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "fct",
            &format!("anim transplant unit={entity} id={id} -> key-bone at {seek:.3}s (bone 0 takes the request)"),
        );
    }
    true
}

/// The per-unit animation state machine: death overrides; otherwise Specials as bracketed loops,
/// one-shots over or instead of the base, and cross-faded gaits.
#[allow(clippy::type_complexity)]
pub(super) fn drive_animations(
    mut commands: Commands,
    mut units: Query<(
        Entity,
        &ModelAnimations,
        &mut AnimationPlayer,
        &mut AnimationTransitions,
        &mut AnimDriver,
        Option<&Spline>,
        Option<&RemoteMotion>,
        Option<&MovementState>,
        Option<&UnitSpeeds>,
        Option<&ObjectStore>,
        Option<&Wielded>,
        Option<&CastHold>,
        Option<&FacingStep>,
        (
            Has<Engaged>,
            Has<AutoRepeatArmed>,
            Has<SelfPlayer>,
            Option<&crate::entities::mount::MountBody>,
            Has<crate::net::CreatureSwimming>,
            // The in-hand arrow is latched (the `$BWP`..`$BWR` window).
            Has<NockLatch>,
            // `OBJECT_FIELD_SCALE_X` on the transform; the locomotion rate divides by it.
            &Transform,
            // Server-granted movement modes, folded by `unify` into the selector's flags word.
            Option<&crate::net::UnitMoveModes>,
            // A proc-11 rate node here (the freeze auras', `0x6201d0`) refuses the wound flinch.
            Option<&crate::aura_visual::AuraNodes>,
        ),
    )>,
    // A mount child's movement view is its host's, fetched through `MountBody.host`.
    mount_hosts: Query<(
        Option<&Spline>,
        Option<&RemoteMotion>,
        Option<&MovementState>,
        Option<&UnitSpeeds>,
        Option<&FacingStep>,
        Has<crate::net::CreatureSwimming>,
        Option<&crate::net::UnitMoveModes>,
        // The rider's scale: the mount renders at the rider's scale times its own (`0x613ef0`).
        &Transform,
        // Read only by the debug trace.
        Has<SelfPlayer>,
    )>,
    mut swings: MessageReader<SwingMessage>,
    mut impacts: MessageReader<SwingImpact>,
    mut defenses: MessageReader<DefenseAnim>,
    mut slows: MessageReader<SwingSlowdown>,
    mut emotes: MessageReader<EmoteAnim>,
    mut spell_wounds: MessageReader<WoundAnim>,
    mut sheath_requests: MessageReader<SheathRequest>,
    mut sheath_swaps: MessageWriter<SheathSwapMessage>,
    anim_data: Option<Res<AnimData>>,
    net: Res<NetCommands>,
    // One tuple: this system is at Bevy's 16-SystemParam ceiling.
    aux: (
        // Its `Emotes.dbc` → `AnimID` column resolves the `UNIT_NPC_EMOTESTATE` idle.
        Option<Res<EmoteSounds>>,
        // Whether the latched loot target is one the self unit kneels at; false while cold.
        Option<Res<crate::ui_loot::LootKneel>>,
        // The key-bone cross-fade's clock: its retiring node is frozen, so it has no playback
        // clock of its own.
        Res<Time>,
        // Read for `DO_NOT_PLAY_WOUND_ANIM`; a missing cache reads as an unreceived template.
        Option<Res<crate::names::NameCache>>,
        MessageReader<BaseAnimRecompute>,
    ),
    // The variation roll's LCG, the reference's single CRT `_rand` stream shared by every play.
    mut rng: ResMut<benilla_assets::AnimRng>,
    // The last anim trace line per traced unit; the trace writes only on change.
    mut anim_trace_last: Local<std::collections::HashMap<Entity, String>>,
) {
    let (emote_sounds, loot_kneel, time, names, mut recomputes) = aux;
    let dt = time.delta_secs();
    // This frame's one-shot plays per unit, replayed in the reference's call order (`PlaySeq`
    // stamps): a later call overwrites an earlier, and the combat fast path keys on what is playing
    // when each call runs.
    let mut pending: bevy::ecs::entity::EntityHashMap<Vec<(OneShotReq, u64)>> = default();
    // Wound flinches by victim, melee (`HitInfo & 0x2`) or spell (severity 0); the last wins, as a
    // re-trigger re-seeds the same secondary slot.
    let mut pending_wound: bevy::ecs::entity::EntityHashMap<WoundEdge> = default();
    for s in swings.read() {
        // HitInfo 0x10000 suppresses the swing animation only.
        if s.hit_info & 0x10000 == 0 {
            pending
                .entry(s.attacker)
                .or_default()
                .push((OneShotReq::Swing(s.hit_info), s.seq));
        }
    }
    // The flinch fires at the swing clip's impact keyframe, not at packet receive.
    for SwingImpact {
        swing: s,
        text_only,
        ..
    } in impacts.read()
    {
        if !text_only && s.hit_info & 0x2 != 0 {
            if let Some(victim) = s.victim {
                pending_wound.insert(victim, WoundEdge::Melee(s.hit_info));
            }
        }
    }
    for w in spell_wounds.read() {
        pending_wound.insert(w.entity, WoundEdge::Spell);
    }
    // Defense reactions (`$CPP`) by victim, last wins; resolved per unit, as the parry keys the
    // victim's own mainhand.
    let mut pending_defense: bevy::ecs::entity::EntityHashMap<u32> = default();
    for d in defenses.read() {
        pending_defense.insert(d.victim, d.victim_state);
    }
    // Whiff slow-downs (`0x712910`): the attacker's in-flight swing drops to half speed.
    let mut pending_slow: bevy::ecs::entity::EntityHashSet = default();
    for s in slows.read() {
        pending_slow.insert(s.0);
    }
    for e in emotes.read() {
        pending
            .entry(e.entity)
            .or_default()
            .push((OneShotReq::Emote(e.anim_id), e.seq));
    }
    // Stage-2 base recomputes by unit, last wins: of two in a frame the second's verdict stands.
    let mut pending_recompute: bevy::ecs::entity::EntityHashMap<u16> = default();
    for r in recomputes.read() {
        pending_recompute.insert(r.entity, r.anim_id);
    }
    // Sheath requests by unit; the last SetSheatheState call wins.
    let mut pending_sheath: bevy::ecs::entity::EntityHashMap<SheathRequest> = default();
    for r in sheath_requests.read() {
        pending_sheath.insert(r.entity, *r);
    }
    // `None` until `AnimationData.dbc` loads; every lookup then resolves to itself.
    let catalog = anim_data.as_deref().map(|d| &d.0);
    // `WOW_ANIM_COST=1` counts the frame's resting rows, the ones an input memo could skip; no skip
    // is taken, as a parked rig's wake pose still needs the right clip armed.
    let anim_cost = anim_cost_enabled();
    let (mut cost_rows, mut cost_resting) = (0u32, 0u32);
    for (
        entity,
        anims,
        mut player,
        mut tr,
        mut drv,
        spline,
        remote,
        movement,
        speeds,
        store,
        wielded,
        cast_hold,
        facing_step,
        (
            engaged,
            auto_repeat,
            is_self,
            mount_body,
            creature_swimming,
            nock_latched,
            transform,
            move_modes,
            aura_nodes,
        ),
    ) in &mut units
    {
        // A mount child drives from its host's movement view, so it plays the locomotion the
        // rider suppresses; a vanished host reads as stationary. `ridden_by_self` feeds only the
        // trace, never `is_self`: the sheath reconcile and wire echoes mean the player's own unit.
        let (
            spline,
            remote,
            movement,
            speeds,
            facing_step,
            creature_swimming,
            move_modes,
            host_scale,
            ridden_by_self,
        ) = match mount_body {
            Some(mb) => match mount_hosts.get(mb.host) {
                Ok((s, r, m, sp, f, sw, md, t, host_self)) => {
                    (s, r, m, sp, f, sw, md, t.scale.x, host_self)
                }
                Err(_) => (None, None, None, None, None, false, None, 1.0, false),
            },
            None => (
                spline,
                remote,
                movement,
                speeds,
                facing_step,
                creature_swimming,
                move_modes,
                1.0,
                false,
            ),
        };
        // The `0x5fe2f0` rate divisor's `|modelScale|`: a mount child's display column composes
        // under the rider's scale.
        let model_scale = transform.scale.x * host_scale;
        let traced = is_self || ridden_by_self;
        let subject = if mount_body.is_some() { "mount " } else { "" };
        let walk = speeds.map_or(DEFAULT_WALK_SPEED, |s| s.0.walk);
        // Dead is zero health or `UNIT_DYNFLAG_DEAD` (`0x605f90`): the flag's watcher `0x600440`
        // plays Death (`0x60ea30`) and `0x5fcff0` holds it, the pair the death handler `0x625190`
        // runs, so a feign lies down and stands when the flag clears.
        // Absent health reads as zero: a create block omits zero fields, so a corpse streams in
        // with no HEALTH.
        let dead = store.is_some_and(|s| s.0.unit_reads_dead());
        // `UNIT_FIELD_MOUNTDISPLAYID` is the one mounted signal; a mount child has no store, so it
        // never reads as mounted.
        let mount_display = store.map_or(0, |s| s.0.unit_mount_display_id());
        let mounted = mount_display != 0;
        // Latched before the death override so an early return still consumes the edge: a unit
        // that mounts and dies in one frame must not arm Mount over Death later.
        let mount_edge = std::mem::replace(&mut drv.mount_display, mount_display) != mount_display;
        // Nothing chosen yet: a corpse streamed in settles on its end pose.
        let first = drv.gait.is_none() && drv.mode == Mode::Gait;
        let mut mv = unify(movement, remote, spline, creature_swimming, move_modes);
        // A unit without a controller poses from its `UNIT_FIELD_BYTES_1` stand-state byte; our own
        // avatar's controller overlays its in-flight request.
        if movement.is_none() {
            mv.stand_state = store.map_or(0, |s| s.0.unit_stand_state());
        }
        // No sit/sleep/kneel pose holds in the saddle (the standState leg's `[+0xdc]==0` gate,
        // `0x5fd550`).
        if mounted {
            mv.stand_state = 0;
        }
        // The creep vis flag off the unit's own descriptor (`[[unit+0x110]+0x213] & 2`), never
        // predicted: the crouch lands with the server's aura.
        mv.stealthed = store.is_some_and(|s| s.0.unit_is_stealthed());
        let moving = mv.flags & move_flags::ANY_MOVE != 0;
        // An arc's launch vertical speed splits a jump (the JumpStart/Jump bracket; jump
        // `0x60e480`, land `0x602c60`) from a step-off fall, which has no bracket and keeps the
        // gait frozen while `airborne_frozen` holds.
        let falling = mv.flags & move_flags::FALLING != 0;
        let was_falling = std::mem::replace(&mut drv.was_falling, falling);
        // The arc starts at the launch, not only at FALLING's rising edge: on broken ground a jump
        // can land and relaunch within one frame with FALLING set throughout.
        let prev_vertical = std::mem::replace(&mut drv.last_vertical_speed, mv.vertical_speed);
        let launched = mv.vertical_speed > JUMP_ARC_MIN_UP && prev_vertical <= JUMP_ARC_MIN_UP;
        // Resting: Gait mode, nothing in flight or pending, grounded and still.
        if anim_cost {
            cost_rows += 1;
            if drv.mode == Mode::Gait
                && drv.overlay.is_none()
                && drv.overlay_fade.is_none()
                && drv.wound.is_none()
                && drv.sheath_swap.is_none()
                && !first
                && !mount_edge
                && cast_hold.is_none()
                && !moving
                && !falling
                && !was_falling
                && !launched
                && !pending.contains_key(&entity)
                && !pending_wound.contains_key(&entity)
                && !pending_defense.contains_key(&entity)
                && !pending_slow.contains(&entity)
                && !pending_sheath.contains_key(&entity)
            {
                cost_resting += 1;
            }
        }
        if falling && (!was_falling || launched) {
            drv.jump_arc = mv.vertical_speed > JUMP_ARC_MIN_UP;
        }
        // The airborne freeze (`0x5fd8e8`): keep the current clip iff
        // `FALLING && (FALLINGFAR || vz ≠ 0)`; a walk-off's first vz == 0 substep may re-pick. It
        // also gates the deferred-cache consumer: a parked clip never plays mid-air.
        let airborne_frozen =
            falling && (mv.flags & move_flags::FALLING_FAR != 0 || mv.vertical_speed != 0.0);
        // A stationary unit easing its yaw reads as turning (`0x607ed0`), so it shuffles, and each
        // return to Stand re-rolls the fidget. Gated like `0x5fce30`: no combat or cast. A positive
        // step is counterclockwise, turning left.
        if let Some(step) = facing_step.filter(|_| !moving && !engaged && cast_hold.is_none()) {
            mv.flags |= if step.0 > 0.0 {
                move_flags::TURN_LEFT
            } else {
                move_flags::TURN_RIGHT
            };
        }
        // Whether a looping base arm may roll its variation (`variationIdx = −1`) or takes the
        // head, decided from the outgoing armed id before this frame's transitions. A stun does
        // not gate it: no `UNIT_FLAG_STUNNED` reader touches animation selection.
        let outgoing = drv.active_anim().unwrap_or(STAND);
        let relaxed = !select::arm_forces_head(engaged, cast_hold.is_some(), outgoing);

        wound_upkeep(entity, &mut drv, &mut player);

        // Death overrides every state: play Death and hold.
        if dead {
            if drv.gait != Some(DEATH) {
                debug!(
                    "death pose: unit {entity} arms Death ({DEATH}) — {}{}",
                    if store.is_some_and(|s| s.0.unit_is_dead()) {
                        "killed"
                    } else {
                        "feign (UNIT_DYNFLAG_DEAD, health intact)"
                    },
                    if first { ", streamed in dead" } else { "" }
                );
                if let Some(c) = find_resolved(anims, DEATH, catalog)
                    .or_else(|| find_resolved(anims, STAND, catalog))
                {
                    // A witnessed death rolls its variation; a streamed-in corpse takes the head.
                    let c = if first {
                        c
                    } else {
                        anims.pick_variation(c.anim_id, rng.draw()).unwrap_or(c)
                    };
                    let active =
                        tr.play(&mut player, c.node, Duration::from_secs_f32(c.blend_time));
                    if c.looping {
                        active.repeat();
                    } else if first {
                        active.seek_to(c.duration); // streamed in dead: settled, not replayed
                    }
                }
                // A blended bone-0 arm overwrites bone 0's secondary slot: it evicts a full-body
                // wound; a masked wound on the key bone decays out on its own.
                if let Some(wd) = drv.wound.take_if(|wd| !wd.masked) {
                    player.stop(wd.node);
                }
                drv.mode = Mode::Gait;
                drv.gait = Some(DEATH);
                drv.deferred = None; // a normal arm clears the cache
            }
            continue;
        }

        // The mount transition arms bone 0's primary through op4 `0x7121a0`: the build `0x607b44`
        // plays 91 Mount with a cross-fade, the teardown `0x607ce0` plays 0 Stand without one.
        // Last writer wins, so it displaces a full-body one-shot mid-clip (the mount summon's
        // SpellCastOmni, armed while still unmounted). The key-bone overlay rides it untouched;
        // while mounted `0x5fe803`–`0x5fe816` re-pins 91 on every play, rendered by the gait pin.
        if mount_edge {
            if matches!(drv.mode, Mode::Swing { .. }) {
                drv.mode = Mode::Gait;
            }
            if mounted {
                // Only the build blends (`0x607b35 push 0x1`), and a blended arm snapshots into the
                // secondary (`0x7125d9`), evicting a full-body wound; the teardown skips that copy
                // (`0x71253e`–`0x712543`).
                if let Some(wd) = drv.wound.take_if(|wd| !wd.masked) {
                    player.stop(wd.node);
                }
            } else {
                // The teardown cuts to Stand: `0x607d1a`–`0x607d2b` pushes crossFade 0 where the
                // build pushes 1. The gait re-pick that follows, `RecomputeBaseAnim(−1)` at
                // `0x607d30` (`0x5fd9e0` → `0x5fd8b0`), blends normally, from Stand.
                play::cut_loop(
                    &mut tr,
                    &mut player,
                    anims,
                    STAND,
                    catalog,
                    &mut rng,
                    &mut drv.loop_window,
                );
            }
            // Force a fresh pick even for an unchanged id: bone 0 may still hold the displaced
            // one-shot's node.
            drv.gait = None;
            drv.deferred = None; // a normal arm clears the fast-path cache (`0x5fe48e`)
        }

        // The loot kneel: self reads `LootKneel`, the open-session latch (`0x6126b0`) folded with
        // the target-class filter (`0x612710`, which refuses a fishing bobber); a remote reads
        // `UNIT_FLAG_LOOTING` (0x400) set with `0x10000000` clear. Never mounted (`[+0xdc]==0`);
        // any direction bit (`[9e8]&0xf`) outranks it, so a stationary swimmer kneels mid-tread.
        let looting = !mounted
            && mv.flags & move_flags::ANY_MOVE == 0
            && if is_self {
                loot_kneel.as_ref().is_some_and(|k| k.0)
            } else {
                store.is_some_and(|s| {
                    let f = s.0.unit_flags();
                    f & select::UNIT_FLAG_LOOTING != 0 && f & select::UNIT_FLAG_LOOT_SUPPRESS == 0
                })
            };

        // No Special claims a mounted rider: the jump arc plays on the mount child.
        let special = if mounted {
            None
        } else {
            current_special(&mv, drv.jump_arc)
        };
        // The `0x5fd8b0` chain calls loot before standState, after the airborne freeze: a looting
        // unit kneels rather than sits, but still jumps.
        let special = special.filter(|s| !(looting && matches!(s, select::Special::Pose(_))));

        // ── Sheath state: the committed cache (`[+0xd40]`), not the descriptor byte, adopting the
        // byte whenever it changes (`0x604c70`). Every SetSheatheState (`0x611cf0`) snaps except
        // the manual toggle's ceremony, which carries the draw/stow sound.
        let sheath_frame_start = drv.sheath_cur;
        let sheath_byte = store.and_then(|s| s.0.unit_sheath_state()).unwrap_or(0);
        if drv.sheath_cur.is_none() || drv.sheath_byte != Some(sheath_byte) {
            drv.sheath_cur = Some(sheath_byte);
            drv.sheath_byte = Some(sheath_byte);
        }
        // The setter: a no-op when unchanged (`newState == CUR`), else commit, volunteer
        // `CMSG_SETSHEATHED` for the local player, and for the manual toggle play the ceremony,
        // per-arm masked overlays (`0x60b770`) by each item's stow family (hip 90, back 89).
        if let Some(req) = pending_sheath.get(&entity) {
            let cur = drv.sheath_cur.unwrap_or(0);
            // Deviation: a mounted draw is refused at the setter, because our rider track never
            // replays the reference's per-play force-stow (`0x5fdf80`); the result is the same.
            if req.state != cur && !(mounted && req.state != 0) {
                debug!(
                    "sheath: unit {entity} {cur} -> {} (request, ceremony {})",
                    req.state, req.ceremony
                );
                drv.sheath_cur = Some(req.state);
                if is_self {
                    let _ = net.0.send(ClientCommand::SetSheathed {
                        state: u32::from(req.state),
                    });
                }
                if req.ceremony {
                    start_sheath_ceremony(
                        &mut commands,
                        entity,
                        &mut drv,
                        &mut player,
                        anims,
                        wielded,
                        cur,
                        req.state,
                        catalog,
                    );
                }
            }
        }
        // Ceremony upkeep: each arm's weapon moves at its clip's `$SHL`/`$SHR` event, and a
        // finished stow runs phase 2, the on-finish draw (`0x5fc920` at `0x5fca8c`/`0x5fcaa1`).
        let sheath_now = drv.sheath_cur.unwrap_or(0);
        advance_sheath_ceremony(
            &mut commands,
            entity,
            &mut drv,
            &mut player,
            anims,
            wielded,
            sheath_now,
            catalog,
            &mut sheath_swaps,
        );

        // ── This frame's one-shots, each routed per play from live state (`route_oneshot`): masked
        // onto the SpineLow overlay beside `mode`, or full-body on the base track. A wound is a
        // per-bone secondary that a blended arm on the same bone overwrites (op4 copies over
        // `+0xc4..`), so plays are tracked per bone; a play on the other bone leaves it
        // (`0x714260`).
        //
        // The base-animation lock releases on the finished id before anything re-picks the base
        // (`0x5fc9c6`), so completion and pre-emption both release it.
        drv.base_lock.release_finished(&player, anims, catalog);

        let pre_state = (drv.mode, drv.gait);
        let mut base_played = false;
        let mut masked_played = false;
        let mut played_oneshot: Option<u16> = None;
        // `DO_NOT_PLAY_WOUND_ANIM` (`type_flags` 0x8, `0x6125f0`) on the template keyed by
        // `OBJECT_FIELD_ENTRY` (`0x60b160`); false when not received. Read by the parry
        // (`0x60ec1f`) and the flinch (`0x60ea9f`).
        let no_wound_anim = store
            .and_then(|s| s.0.object_entry())
            .zip(names.as_deref())
            .and_then(|(entry, n)| n.creature_record(entry))
            .is_some_and(|r| r.type_flags & DO_NOT_PLAY_WOUND_ANIM != 0);
        // Defense plays after a same-frame own swing or emote: the `$CPP` arm is the later call.
        // Gated alive (`0x60ec00` checks IsDead and stand-state 7).
        let defense = pending_defense.get(&entity).and_then(|&vs| {
            if dead {
                return None;
            }
            // The flag refuses only the parry (victimState 3, `0x60ec1f`); dodge (30) and block
            // (24) play straight from `0x624a90`/`0x624a74`, ungated.
            if vs == 3 && no_wound_anim {
                return None;
            }
            // The parry reads the mainhand through `GetWeapon(0, 0)`: a disarmed victim gets no
            // clip.
            let id = defense_anim(vs, wielded.and_then(|w| w.armed_main()));
            if let Some(id) = id {
                debug!("defense: unit {entity} anim {id} (victimState {vs})");
            }
            id
        });
        // In `PlaySeq` order, defense last: the reference plays it from the deferred impact scan,
        // after every message handler.
        let mut requests: Vec<u16> = pending
            .remove(&entity)
            .map(|mut v| {
                v.sort_by_key(|&(_, seq)| seq);
                v.into_iter()
                    .map(|(req, _)| match req {
                        OneShotReq::Swing(hit_info) => {
                            let w = wielded.copied().unwrap_or_default();
                            // `0x6246a0` calls `GetWeapon(slot, 0)`: a disarmed attacker swings
                            // AttackUnarmed(16) / AttackUnarmedOff(117), weapon still in hand.
                            let id = if hit_info & 0x4 != 0 {
                                swing_anim_off(w.armed_off())
                            } else {
                                swing_anim_main(w.armed_main())
                            };
                            debug!("swing: unit {entity} anim {id} (hitInfo {hit_info:#x})");
                            id
                        }
                        OneShotReq::Emote(id) => id,
                    })
                    .collect()
            })
            .unwrap_or_default();
        requests.extend(defense);
        // At the play seam (`0x5fe2f0`), Special1H/2H with both hands empty becomes
        // SpecialUnarmed(118), for every request.
        {
            let w = wielded.copied().unwrap_or_default();
            for id in &mut requests {
                *id = select::unarmed_special(*id, w.armed_main(), w.armed_off());
            }
        }
        // The deferred cache (`+0xd60`): once no one-shot is live, the parked combat clip plays.
        // Fresh requests supersede it (`0x5fe48e`/`0x5fe480`). Never mid-air: the read
        // (`0x5fd392`, in `0x5fd360`) is past the airborne freeze, so the landing play clears it.
        if requests.is_empty()
            && drv.deferred.is_some()
            && !airborne_frozen
            && live_oneshot(&drv, &player, &tr, anims, catalog).is_none()
        {
            requests.extend(drv.deferred.take());
        }
        for id in requests {
            // The lock first: `0x5fe2f0`'s head guard returns before routing, the fast path and
            // the dedup, so a locked unit gets no arm and no deferral.
            if drv.base_lock.refuses() {
                continue;
            }
            // The combat fast path (`0x5fe43c`–`0x5fe48b`): a combat clip over a live combat clip
            // is not armed; the current clip's rate doubles (op6 2.0) and the request parks in
            // `+0xd60`.
            if let Some((cur, node)) = live_oneshot(&drv, &player, &tr, anims, catalog) {
                if select::is_combat_anim(cur) && select::is_combat_anim(id) {
                    if let Some(active) = player.animation_mut(node) {
                        active.set_speed(2.0);
                    }
                    drv.deferred = Some(id);
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line(
                            "fct",
                            &format!(
                                "anim fastpath unit={entity} cur={cur} req={id} (cur 2x, req deferred)"
                            ),
                        );
                    }
                    continue;
                }
            }
            // A normal arm clears the cache (`0x5fe48e` writes −1).
            drv.deferred = None;
            // Same-id dedup (`0x5fdba0`): an id still playing in its slot is not re-armed, so an
            // eat/drink loop free-runs across the server's ~5 s kit resends.
            let overlay_live = drv.overlay.is_some_and(|ov| {
                ov.id == id && player.animation(ov.node).is_some_and(|a| !a.is_finished())
            });
            let base_live = matches!(drv.mode, Mode::Swing { id: m, .. } if m == id)
                && !oneshot_finished(&player, anims, id, catalog);
            if overlay_live || base_live {
                if benilla_assets::trace::enabled() {
                    benilla_assets::trace::line(
                        "fct",
                        &format!(
                            "anim dedup-eat unit={entity} id={id} overlay_live={overlay_live} base_live={base_live}"
                        ),
                    );
                }
                continue;
            }
            // Mounted forces the masked route (`0x5fe2f0`'s mounted branch).
            let masked =
                mounted || route_oneshot(id, mv.flags, mv.stand_state) == OneShotRoute::Masked;
            // Resolve to a clip this model has, roll its variation and its replay budget (a clamp
            // one-shot authored `(min,max)` plays R times).
            let picked =
                find_resolved(anims, id, catalog).map(|h| roll_oneshot(anims, h, &mut rng));
            let upper = masked
                .then(|| picked.and_then(|(c, r)| c.upper_node.map(|n| (n, r))))
                .flatten();
            if let Some((node, repeat)) = upper {
                // Masked: a blended key-bone re-arm (op4 `blendFlag = 1`) over the incoming clip's
                // own blendTime (`0x7125f2`, `M2Sequence+0x20`). The full-body route below leaves
                // the key bone as it is (`0x5fe930`).
                retire_overlay(
                    &mut drv,
                    &mut player,
                    picked.map_or(0.0, |(c, _)| c.blend_time.max(0.0)),
                );
                let active = player.play(node);
                active.replay();
                active.set_repeat(repeat); // a reused node keeps no stale count
                active.set_speed(1.0); // nor a stale whiff slow-down
                active.set_weight(0.0); // the fade upkeep raises it this same frame
                drv.overlay = Some(Overlay {
                    node,
                    id,
                    looping: false,
                });
                masked_played = true;
                played_oneshot = Some(id);
            } else {
                // Full-body, or a model with no key bone (the −1 sentinel arms bone 0): the clip
                // replaces the base on bone 0 even over a Special, last writer wins (`0x5fe6c8`).
                // An airborne arc's own clip freezes first, op4's pose-snapshot decay.
                if let Some(sp) =
                    special.filter(|sp| matches!(sp, select::Special::Jump | select::Special::Fall))
                {
                    if let Some(active) = tr
                        .get_main_animation()
                        .filter(|&n| holds_own_clip(anims, catalog, sp, n))
                        .and_then(|n| player.animation_mut(n))
                    {
                        active.set_speed(0.0);
                    }
                }
                if let Some((c, repeat)) = picked {
                    play_clip(&mut drv.base_lock, &mut tr, &mut player, c, repeat, 1.0);
                }
                drv.mode = Mode::Swing { id, under: special };
                drv.gait = None;
                base_played = true;
                played_oneshot = Some(id);
            }
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "fct",
                    &format!(
                        "anim play unit={entity} id={id} masked={masked_played} base={base_played} under={special:?}"
                    ),
                );
            }
        }

        // The whiff slow-down (`0x712910`): a miss, dodge or evade plays the rest of the swing at
        // 0.5. Deviation: the reference writes bone 0's rate blindly; we scope it to the swing's
        // own node, because a moving attacker's gait would otherwise slow too. The id is
        // re-checked, as `Mode::Swing` also holds non-swing one-shots.
        if pending_slow.contains(&entity) {
            let node = match drv.overlay {
                Some(ov) if is_swing_id(ov.id) => Some(ov.node),
                _ if matches!(drv.mode, Mode::Swing { id: m, .. } if is_swing_id(m)) => {
                    tr.get_main_animation()
                }
                _ => None,
            };
            if let Some(active) = node.and_then(|n| player.animation_mut(n)) {
                active.set_speed(0.5);
            }
        }

        // A Special edge is a normal play, so it clears the deferred cache (`0x5fe48e`); the level
        // does not, as the airborne freeze issues no plays mid-arc.
        let special_edge = special != drv.last_special;
        drv.last_special = special;
        if special_edge {
            drv.deferred = None;
        }
        // Looping-variation advance (`0x719370`, the unit's `model+0x70` callback): when the main
        // node completes its `R` passes, a fresh weighted pick with a fresh window follows (a
        // gryphon's flap/glide, a multi-part /dance). In the gait slot the callback is
        // `RecomputeBaseAnim(−1)` (`0x5fc3f0` row `0x5fc844` → `0x5fd9e0`), a re-selection:
        // clearing the target lets `mode::run` re-pick, which is what ends a turn-shuffle. A
        // Special loop re-arms its id.
        if let Some((node, budget)) = drv.loop_window {
            if tr.get_main_animation() == Some(node)
                && player
                    .animation(node)
                    .is_some_and(|a| a.completions() >= budget)
            {
                if drv.mode == Mode::Gait {
                    drv.gait = None;
                } else {
                    let head = anims
                        .clips
                        .iter()
                        .find(|c| c.node == node)
                        .and_then(|armed| find_resolved(anims, armed.anim_id, catalog));
                    if let Some(head) = head {
                        let rate = player.animation(node).map_or(1.0, |a| a.speed());
                        let (c, fresh) = roll_loop(anims, head, relaxed, &mut rng);
                        play_clip(
                            &mut drv.base_lock,
                            &mut tr,
                            &mut player,
                            c,
                            bevy::animation::RepeatAnimation::Forever,
                            rate,
                        );
                        drv.loop_window = Some((c.node, fresh));
                    }
                }
            }
        }
        // ── Stage-2 base recompute (`0x60f389`–`0x60f399`): a state kit's id that differs from
        // the one playing (`0x5fdb50`: key bone, else bone 0) runs `0x5fd9e0(unit, -1)`; it never
        // plays the id. It must run after this frame's one-shot arms, so the state kit cuts the
        // impact kit (Charge 22911: Knockdown 121, then Stun 14). A unit inside a Special is left
        // alone: the recompute would land in the same state, and forcing Gait replays its entry.
        if let Some(&want) = pending_recompute.get(&entity) {
            let armed = drv.overlay.map(|ov| ov.id).or_else(|| {
                tr.get_main_animation()
                    .and_then(|n| anims.clips.iter().find(|c| c.node == n))
                    .map(|c| c.anim_id)
            });
            if armed != Some(want) && matches!(drv.mode, Mode::Swing { .. } | Mode::Gait) {
                drv.deferred = None; // a normal arm clears the cache (`0x5fe48e`)
                drv.mode = Mode::Gait;
                drv.gait = None; // recompute a fresh gait next frame
            }
        }

        // ── The mode machine: the base track's decision.
        mode::run(
            mode::Frame {
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
                emote_sounds: emote_sounds.as_deref(),
                walk,
                model_scale,
                traced,
                subject,
            },
            &mut drv,
            &mut tr,
            &mut player,
            &mut rng,
        );

        // ── The base slot's playback rate, written over whatever the mode machine settled on: the
        // rate write sits outside the selector (`0x5fe2f0`). JumpLandRun 187 resolves to Run 5 on
        // creature models, so a mount's landing is rate-scaled like any locomotion.
        play::sync_base_rate(&mut drv, &tr, &mut player, anims, mv.speed, model_scale);

        // ── Masked one-shot completion: a finished overlay is not stopped but faded to rest, its
        // held final frame cross-fading onto the base over 150 ms (`0x719370` → `0x5fc920`).
        if let Some(ov) = drv.overlay {
            let done = if ov.looping {
                false // a cast-hold loop: the hold block below owns its release
            } else if find_resolved(anims, ov.id, catalog).is_some_and(|c| c.looping) {
                // A hold-less looping clip (a kit's seated eat/drink) releases on movement; the
                // reference's release for this case is untraced. A Special does not release it:
                // a jump is a bone-0 play and leaves the key bone alone.
                moving
            } else {
                player.animation(ov.node).is_none_or(|a| a.is_finished())
            };
            if done {
                // Freeze on the final frame: the snapshot clamps to `seq.end` (`0x7123af`).
                if let Some(a) = player.animation_mut(ov.node) {
                    a.set_speed(0.0);
                }
                retire_overlay(&mut drv, &mut player, ONESHOT_RELEASE_FADE);
            }
        }

        // ── The moving cast hold: a caster that translates or swims (`[9e8] & 0x20000f`) loops the
        // hold clip on its torso; a merely turning one stays pinned in the gait slot. A masked
        // one-shot wins while it plays. A jump does not cancel it: `0x5fe912` sends locomotion to
        // bone 0 while the key bone is armed.
        let mut hold_played: Option<u16> = None;
        let masked_hold = cast_hold
            .filter(|_| mv.flags & move_flags::CAST_PIN_MOVE != 0)
            .and_then(|h| {
                find_resolved(anims, h.anim_id, catalog).and_then(|c| {
                    c.upper_node
                        .map(|node| (h.anim_id, node, c.blend_time.max(0.0)))
                })
            });
        match (masked_hold, drv.overlay) {
            (Some((id, node, blend)), prior)
                if prior.is_none_or(|ov| ov.looping && ov.id != id) =>
            {
                // Free slot or a stale hold loop: a blended key-bone re-arm.
                retire_overlay(&mut drv, &mut player, blend);
                let active = player.play(node);
                active.replay();
                active.repeat();
                active.set_weight(0.0); // the fade upkeep raises it this same frame
                drv.overlay = Some(Overlay {
                    node,
                    id,
                    looping: true,
                });
                hold_played = Some(id);
            }
            (None, Some(ov)) if ov.looping => {
                // The hold ended or the unit stopped: fade to rest. The reference's release of a
                // looping hold is untraced.
                retire_overlay(&mut drv, &mut player, ONESHOT_RELEASE_FADE);
            }
            _ => {}
        }
        // The cross-fade advances last, after every arm and release, as the reference's kernel
        // runs λ after the frame's PlayAnimation calls.
        overlay_fade_upkeep(&mut drv, &mut player, dt);

        // The anim half of the `WOW_MOVE_TRACE` trace: one line whenever the traced unit's settled
        // state changes, both slots included. `base=` is the resolved clip bone 0 holds (`?` for a
        // node with no clip record); `+fade` marks a fade running.
        if traced && benilla_assets::trace::enabled() {
            let base = tr
                .get_main_animation()
                .and_then(|n| anims.clips.iter().find(|c| c.node == n))
                .map_or_else(|| "?".to_string(), |c| c.anim_id.to_string());
            let state = format!(
                "{subject}mode={:?} gait={:?} base={base} special={:?} flags={:08x} \
                 speed={:.2} scale={:.2} rate={:.2} upper={:?}{}",
                drv.mode,
                drv.gait,
                special,
                mv.flags,
                mv.speed,
                model_scale,
                drv.gait_rate,
                drv.overlay.map(|ov| ov.id),
                if drv.overlay_fade.is_some() {
                    "+fade"
                } else {
                    ""
                },
            );
            if anim_trace_last.get(&entity) != Some(&state) {
                benilla_assets::trace::line("anim", &state);
                anim_trace_last.insert(entity, state);
            }
        }

        let masked_played = masked_played || hold_played.is_some();

        // A mode or gait change stands in for the mode machine's plays, except under the base-anim
        // lock, where the machine walks its brackets with every play refused.
        let base_played =
            base_played || ((drv.mode, drv.gait) != pre_state && !drv.base_lock.refuses());
        // The weapon-trail latch's edge: `0x5fe2f0` is the single animation entry point, so any
        // start counts. Written every pass, so it is this frame's alone.
        drv.started_anim = masked_played || base_played;
        wound_evict(entity, &mut drv, &mut player, masked_played, base_played);

        if let Some(&edge) = pending_wound.get(&entity) {
            // The id by severity and engagement (`0x60ea70`); a spell edge is severity 0.
            let id = match edge {
                WoundEdge::Melee(hit_info) => select::wound_anim(hit_info, engaged),
                WoundEdge::Spell => select::wound_anim(0, engaged),
            };
            // The entry gates in `0x60ea70`'s order: `DO_NOT_PLAY_WOUND_ANIM` (`0x60ea9f`), which
            // gates the animation only (the blood spurt, `0x624530` → `0x625010`, still plays);
            // then any CharProc-11 rate node (`0x60eaac`–`0x60eac8`), by presence, not rate.
            let refusal = if no_wound_anim {
                Some("the template carries DO_NOT_PLAY_WOUND_ANIM")
            } else if aura_nodes.is_some_and(|n| n.head_anim_rate().is_some()) {
                Some("a proc-11 rate node is attached")
            } else {
                None
            };
            if let Some(why) = refusal {
                if benilla_assets::trace::enabled() {
                    benilla_assets::trace::line(
                        "fct",
                        &format!("wound trigger unit={entity} id={id} REFUSED ({why})"),
                    );
                }
            } else {
                wound_trigger(
                    entity,
                    &mut drv,
                    &mut player,
                    anims,
                    catalog,
                    &mut rng,
                    id,
                    &mv,
                    mounted,
                );
            }
        }

        // ── The sheath reconcile (`0x5fdf80`, inside every PlayAnimation): the played clip's
        // `AnimationData.dbc` WeaponFlags and engagement force a snap, and a remote unit with no
        // force returns to the server byte; the local player volunteers `CMSG_SETSHEATHED`.
        // It tests the requested id, not the model's substitute: the arm descriptor that
        // `0x5fdb50` reads carries the id asked for (`0x5fde80`, stored at `+0xf8` by `0x71252f`),
        // so a model lacking the clip still stows by the requested row.
        // It runs on plays only, this frame's one-shot first: re-running it on a silent frame
        // would undo a masked stow against the base gait. A mounted unit still drawn (the byte
        // adopt bypasses the setter) forces once, as our rider track never plays.
        if played_oneshot.is_some()
            || hold_played.is_some()
            || base_played
            || (mounted && drv.sheath_cur.unwrap_or(0) != 0)
        {
            let cur = drv.sheath_cur.unwrap_or(0);
            let anim = played_oneshot
                .or(hold_played)
                .or_else(|| drv.active_anim())
                .unwrap_or(STAND);
            let flags = catalog.map_or(0, |cat| cat.weapon_flags(anim));
            if let Some(forced) =
                select::reconcile_sheath(cur, anim, flags, engaged, is_self, sheath_byte, mounted)
            {
                // The reference brackets every ranged kit play with ranged snaps (`0x60f34c`
                // before, `SetSheatheState(2,1,1)` after), so ReadyThrown 108's stow (outside the
                // `0x5fe180` exempt set) never renders; a live ranged hold resolves a stow to 2.
                // That call order implies a remote thrower's wind-up reads 0 until its GO, which
                // is untraced; every caster here stays at 2.
                let forced = if forced != 2 && cast_hold.is_some_and(|h| h.ranged) {
                    2
                } else {
                    forced
                };
                if forced != cur {
                    debug!(
                        "sheath: unit {entity} {cur} -> {forced} (anim {anim} flags {flags:#x}, \
                         engaged {engaged})"
                    );
                    drv.sheath_cur = Some(forced);
                    if is_self {
                        let _ = net.0.send(ClientCommand::SetSheathed {
                            state: u32::from(forced),
                        });
                    }
                }
            }
        }
        // Leaving sheath state 2 by any path un-nocks and drops the ammo (`0x60fc72`, gated
        // `[+0xd40] != 2`) until the next `SMSG_SPELL_START`.
        if sheath_frame_start == Some(2) && drv.sheath_cur != Some(2) {
            commands
                .entity(entity)
                .remove::<(super::NockedAmmo, super::NockLatch)>();
        }
        // Translating or swimming un-nocks too, keeping the ammo and the draw: `0x60e480` arm 0
        // (`0x60e4ac call 0x60f530`) runs on MSG_MOVE_START_{FORWARD, BACKWARD, STRAFE_LEFT,
        // STRAFE_RIGHT, SWIM} (181/182/184/185/202); turning and stopping do not. A level test
        // matches the start edge, since only the Load clips' `$BWP` re-latches.
        if mv.flags & (move_flags::ANY_MOVE | move_flags::SWIMMING) != 0 && nock_latched {
            commands.entity(entity).remove::<super::NockLatch>();
        }
    }
    if anim_cost && cost_rows > 0 {
        use std::sync::atomic::{AtomicU32, Ordering};
        static FRAME: AtomicU32 = AtomicU32::new(0);
        // About once a second at 60 fps.
        if FRAME.fetch_add(1, Ordering::Relaxed).is_multiple_of(64) {
            eprintln!("[anim-cost] rows={cost_rows} resting={cost_resting}");
        }
    }
}

/// `WOW_ANIM_COST=1` arms the resting-row counter; read once.
fn anim_cost_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_ANIM_COST").as_deref() == Ok("1"))
}
