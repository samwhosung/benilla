//! The Bevy driver: [`drive_animations`], which executes the state machine [`super::select`]
//! picks, reading and writing [`super::AnimDriver`]'s private state. Kept in its own file as it
//! carries the bulk of the per-frame Bevy system logic, separate from the module face ([`super`])
//! and the pure selector logic ([`super::select`]). Split into a small family of children — the
//! playback primitives ([`play`]), the wound-flinch secondary-blend slot logic ([`wound`]), the
//! per-hand weapon-grip overlay ([`grip`]), and the base track's **mode machine** ([`mode`]) —
//! with this file the stable face, re-exporting [`grip::drive_hand_grip`] unchanged.
//!
//! What is left here is one function, [`drive_animations`], and it is a *sequence of phases* over
//! one unit, not a set of concerns: the prologue that computes the frame's facts, the death
//! override, the mount transition edge, the sheath state, the one-shot gather-and-arm, the mode
//! machine (now [`mode`]), the rate write, the fades, the trace, the wound, the sheath reconcile.
//! The phases share ~20 live bindings and a handful of frame-local flags, so they do not come
//! apart on a module boundary the way the four children did — decision 0933 records which cut was
//! taken, which was not, and why.

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

/// The weight of a masked upper-body one-shot overlay ([`AnimDriver::overlay`]) over the base clip on
/// the SpineLow subtree both drive. The base clip is *not* masked out of that subtree
/// (it animates the whole skeleton), so base and overlay blend there — this makes the overlay
/// dominate ≈ 8:1 (the torso visibly swings/emotes, not a wash with the run's torso), a small bleed
/// the cost of Bevy's weighted blend. The legs are excluded by the mask entirely. Same rationale and
/// value as the per-arm sheath ceremony's overlay weight.
const ONESHOT_OVERLAY_WEIGHT: f32 = 8.0;

/// The key-bone **fade-to-rest** window: a
/// finished upper-body one-shot is never stopped. The client's per-frame advance latches the
/// completion (`0x719370`), the deferred event reaches `CGUnit::OnAnimationFinished 0x5fc920` the
/// same frame, and that calls op4 with `param_3 = -1` (`0x5fcacb`) — which snapshots the clip's
/// **held final frame** into the bone's secondary slot, seeds `+0x100 = clock + 150`,
/// `+0x104 = 1/150`, `+0x108 = 1.0`, and disarms the primary so the bone inherits bone 0 again.
/// **Fixed 150 ms** — deliberately NOT the sequence's own blendTime, which is used only on the arm
/// path.
const ONESHOT_RELEASE_FADE: f32 = 0.150;

/// The launch vertical speed (yd/s, +up) above which a new airborne arc is a **jump** (the 37/38
/// bracket) rather than a step-off fall. A jump launches at ≈7.96 up (our own controller's
/// `JUMP_SPEED`; a remote's `-zspeed - g·t` is still ≳4 after relay latency); a step-off starts
/// level or already descending — the two never come near this line.
const JUMP_ARC_MIN_UP: f32 = 0.5;

/// A one-shot play request gathered from this frame's messages, resolved to an anim id inside
/// the unit loop (a swing's id keys the attacker's own wielded weapon).
enum OneShotReq {
    Swing(u32),
    Emote(u16),
}

/// The currently-armed one-shot the combat fast-path tests — the client's key-bone-else-bone-0
/// current-id read (`0x5fe422`): the masked overlay while
/// its node still plays, else the full-body [`Mode::Swing`] clip while unfinished. Gait and
/// Special clips are never returned; their ids aren't combat, so nothing is lost at the
/// classifier gate.
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

/// Whether **any** one-shot is currently live on the unit — [`live_oneshot`] minus the node. The
/// missile queue's anim-end flush backstop (the client's `0x5fc920` on-anim-finish → `0x60c9b0`
/// is a *callback*; ours polls this edge): a queued missile whose release keyframe never fired
/// launches when the cast one-shot ends.
pub(crate) fn oneshot_is_live(
    drv: &AnimDriver,
    player: &AnimationPlayer,
    tr: &AnimationTransitions,
    anims: &ModelAnimations,
    catalog: Option<&AnimDataCatalog>,
) -> bool {
    live_oneshot(drv, player, tr, anims, catalog).is_some()
}

/// Retire whatever holds the key-bone slot into its cross-fade ([`AnimDriver::overlay_fade`]) —
/// the client's op4 snapshot of the outgoing pose into the bone's SECONDARY (`rep movsd
/// +0x98 → +0xc4`) plus the window seed (decision 0878). `total` is the window: the incoming
/// clip's own blendTime on a blended re-arm, [`ONESHOT_RELEASE_FADE`] on a fade-to-rest. Honors
/// the client's **re-snapshot guard** (`0x7125d4` on the arm path, `0x7123a2` on the release
/// path, both against `0x7ffa24 = 0.5f`): a fade still running above λ = 0.5 is not re-seeded —
/// the older pose keeps decaying and the superseded node is simply dropped. Leaves
/// [`AnimDriver::overlay`] empty; the caller arms the incoming clip (if any) after it.
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

/// The blend weight λ of a key-bone cross-fade this frame — `smoothstep` over the fraction of the
/// window still to run, so it decays 1 → 0 ([`select::blend_lambda`]).
fn fade_lambda(f: &OverlayFade) -> f32 {
    select::blend_lambda(if f.total > 0.0 { f.left / f.total } else { 0.0 })
}

/// The key-bone cross-fade's per-frame advance — the kernel's λ decay
/// (`0x714880`–`0x714923`) and its self-release (`0x7147b9`: `+0xd0 = -1` and λ = 0 the same
/// frame). Two shapes, decided by whether an incoming clip holds the slot:
///
/// - **A release** (no incoming): the retiring node alone fades against the base, so its weight
///   must land the blended share on `λ · W/(1+W)` — the client's full-override λ, capped by the
///   standing [`ONESHOT_OVERLAY_WEIGHT`] approximation. `w = W·λ / (1 + W·(1−λ))`.
/// - **A re-arm** (incoming present): the two clips split the slot's share by λ exactly as the
///   client's `primary + (secondary − primary)·λ` does, at constant total — `W·λ` out, `W·(1−λ)`
///   in. (An incoming that arrived by *transplant* seeds no fade at all: `blendFlag = 0`.)
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

/// The **TRANSPLANT** (`0x5fe919`). A base **locomotion** clip
/// requested while bone 0 still plays a live **CAST** or **COMBAT** one-shot does not replace it:
/// the client copies the bone-0 descriptor — its id, its rate, and `+0x08` the clip's **live
/// elapsed position** — onto the key-bone with `blendFlag = 0`, then hands bone 0 the request. The
/// cast therefore keeps running on the torso *at exactly the frame it was on*, with no cross-fade
/// and no restart, while the legs take the jump or the run. This is the director's "jump right
/// after a cast: the legs jump, the arms finish the cast".
///
/// A no-op when the key-bone is already armed — `0x5fe912` then jumps straight to `0x5fe930`
/// (request → bone 0, key-bone untouched), which is exactly what leaving [`AnimDriver::overlay`]
/// alone already gives us. Named simplification: the moved clip re-arms as a plain `Never` repeat,
/// dropping any unspent replay budget (`R > 1`); every cast and swing this fires for is authored
/// `(0, 0)` ⇒ `R = 1`.
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
    // The live position + rate, read off the armed VARIATION node (the play rolled one of them),
    // which is also how we find the masked twin to resume on.
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
    active.set_weight(ONESHOT_OVERLAY_WEIGHT); // `blendFlag = 0` — no ramp, no cross-fade
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

/// The animation state machine, per unit. Death overrides; otherwise:
/// enter/loop/exit the Special states (jump, sit/sleep/kneel) as one-shot-bracketed loops, play the
/// per-packet melee swings as preemptible one-shots, and cross-fade the gaits (the engaged Ready
/// idle among them).
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
            // Is the in-hand arrow currently latched (the `$BWP`..`$BWR` window)? Read only by
            // the leaving-the-stance un-nock at the tail of this pass, which is edge-triggered
            // off it rather than issuing a remove every frame.
            Has<NockLatch>,
            // The rendered model scale — `OBJECT_FIELD_SCALE_X`, baked onto the entity transform by
            // `entities::attach`. The locomotion playback rate divides by it, so an
            // ogre at 2.2× cycles its legs 2.2× slower than a same-speed human. Read-only: nothing
            // in this system moves a unit.
            &Transform,
            // The server-granted movement modes — root, water-walk, feather-fall,
            // hover, walk-mode, swim. `unify` folds them into the flags word the selector reads, so
            // a creature the server has rooted or put in walk mode is animated as one.
            Option<&crate::net::UnitMoveModes>,
            // The unit's aura CharProc nodes — read for ONE thing here: whether a proc-11
            // animation-rate node (the freeze auras' `0x6201d0` node) is on the
            // unit, which is the client's `+0x2c & 0x4` wound-flinch refusal.
            Option<&crate::aura_visual::AuraNodes>,
        ),
    )>,
    // A mount child's movement view is its HOST's: the same
    // MovementState/RemoteMotion/Spline/speeds the rider's `unify` reads, fetched through
    // `MountBody.host` — so the untouched gait machinery locomotes the mount for self, remote
    // players, and mounted NPCs alike. Read-only over types `units` also only reads.
    mount_hosts: Query<(
        Option<&Spline>,
        Option<&RemoteMotion>,
        Option<&MovementState>,
        Option<&UnitSpeeds>,
        Option<&FacingStep>,
        Has<crate::net::CreatureSwimming>,
        // …and the host's granted modes, for the same reason `unify` reads them on the rider.
        Option<&crate::net::UnitMoveModes>,
        // …and the rider's own `OBJECT_FIELD_SCALE_X`: a mount child's transform carries only its
        // `CreatureDisplayInfo` column, and the mount renders at the PRODUCT of the two
        // (`0x613ef0`). The rate divisor wants that product
        // — the mount model's world scale — not the child's local column.
        &Transform,
        // …and whether the rider is US, which only the debug trace reads.
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
    // The shared emote-audio catalog, promoted here for its `Emotes.dbc` → `AnimID` column: the
    // looping state-emote idle (`UNIT_NPC_EMOTESTATE`) below resolves through it, same as the
    // `SMSG_EMOTE` one-shot consumer ([`emote_anim::emote_to_anim`]).
    // One tuple param (the 16-SystemParam ceiling): the shared emote-audio catalog, the
    // client-local loot-target latch, and the frame clock.
    aux: (
        Option<Res<EmoteSounds>>,
        // Predicate B's standing answer: whether the currently latched loot
        // target is one the SELF unit kneels at — the loot leg's whole self trigger, since it is
        // false while the latch is cold. `Option` for the headless test worlds that don't build
        // the loot seam.
        Option<Res<crate::ui_loot::LootKneel>>,
        // The frame clock — the key-bone cross-fade's only time source: its
        // retiring node is *frozen* on the clip's final frame, so unlike the wound's decay there
        // is no playback clock to read the λ window off.
        Res<Time>,
        // The creature-template cache — read for ONE bit, the victim's `DO_NOT_PLAY_WOUND_ANIM`,
        // which is the wound trigger's first entry gate below. `Option` for the
        // headless test worlds that build no net seam; a missing cache reads exactly like a
        // template we have not received, which is the reference's own null-record leg.
        Option<Res<crate::names::NameCache>>,
        // The **stage-2 base recomputes** ([`BaseAnimRecompute`]) — in the tuple for the same
        // reason as its neighbours: this system sits on the 16-SystemParam ceiling.
        MessageReader<BaseAnimRecompute>,
    ),
    // The variation roll's LCG state (the client's single CRT `_rand` stream,
    // shared by every play; [`select::msvc_rand`]).
    mut rng: ResMut<benilla_assets::AnimRng>,
    // The SELF unit's last-written anim state line of the `WOW_MOVE_TRACE` debug trace (the
    // diff-only filter; see the trace block after the mode machine).
    mut anim_trace_last: Local<std::collections::HashMap<Entity, String>>,
) {
    let (emote_sounds, loot_kneel, time, names, mut recomputes) = aux;
    let dt = time.delta_secs();
    // This frame's one-shot PLAY CALLS (swings + anim-emotes), gathered per unit and replayed
    // in the client's call order below ([`PlaySeq`] stamps — the net drain stamps packet order,
    // scene-time emitters stamp after it). Order matters twice over: the later call overwrites
    // the earlier on the default path, and the combat FAST-PATH
    // keys on what is *currently playing* when each call runs.
    let mut pending: bevy::ecs::entity::EntityHashMap<Vec<(OneShotReq, u64)>> = default();
    // …and by victim: a landed hit with the flinch bit (`HitInfo & 0x2` — the sole trigger gate,
    // decision 0111) plays the victim's wound-flinch **decay overlay** below, as does a spell-side
    // flinch ([`WoundAnim`] — the kit player's 8–10 branch, a harmful instant impact, a missile
    // impact; severity 0 every time). Last hit wins, matching the client, where a
    // re-trigger re-seeds the same secondary slot. Independent of the attacker-side 0x10000
    // suppressor (that bit gates the *swing* animation only).
    let mut pending_wound: bevy::ecs::entity::EntityHashMap<WoundEdge> = default();
    for s in swings.read() {
        // HitInfo bit 0x10000 suppresses the swing anim (decision 0073's verified suppressor).
        if s.hit_info & 0x10000 == 0 {
            pending
                .entry(s.attacker)
                .or_default()
                .push((OneShotReq::Swing(s.hit_info), s.seq));
        }
    }
    // The victim's wound flinch fires at the swing clip's IMPACT keyframe ([`SwingImpact`],
    // `impact.rs`), not at packet receive — the blow lands, then the victim recoils.
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
    // This frame's victim DEFENSE reactions (`$CPP`), keyed by victim — last wins
    // (a re-trigger re-arms the same primary). Resolved to an anim id inside the loop: the parry
    // pick keys the victim's OWN mainhand, and the client's alive gate needs the store.
    let mut pending_defense: bevy::ecs::entity::EntityHashMap<u32> = default();
    for d in defenses.read() {
        pending_defense.insert(d.victim, d.victim_state);
    }
    // This frame's whiff slow-downs (`0x712910`): the attacker's in-flight swing
    // drops to HALF speed for its remainder.
    let mut pending_slow: bevy::ecs::entity::EntityHashSet = default();
    for s in slows.read() {
        pending_slow.insert(s.0);
    }
    // This frame's anim-emotes join the same request list (the [`PlaySeq`] sort below puts the
    // two writer streams in emission order). Played as the same over-the-gait one-shot as a
    // swing (the talk emote on interact).
    for e in emotes.read() {
        pending
            .entry(e.entity)
            .or_default()
            .push((OneShotReq::Emote(e.anim_id), e.seq));
    }
    // This frame's **stage-2 base recomputes** — a state kit's anim id, which is never played
    // ([`BaseAnimRecompute`]). Last wins: the reference makes the comparison per kit play, and two
    // in one frame leave the second's verdict standing.
    let mut pending_recompute: bevy::ecs::entity::EntityHashMap<u16> = default();
    for r in recomputes.read() {
        pending_recompute.insert(r.entity, r.anim_id);
    }
    // This frame's sheath requests, keyed by unit (a later request replaces an earlier — the
    // client's last SetSheatheState call wins).
    let mut pending_sheath: bevy::ecs::entity::EntityHashMap<SheathRequest> = default();
    for r in sheath_requests.read() {
        pending_sheath.insert(r.entity, *r);
    }
    // The missing-clip resolver's DBC source: `None` for the brief window before
    // `AnimationData.dbc` loads, in which every lookup below degrades to identity.
    let catalog = anim_data.as_deref().map(|d| &d.0);
    // `WOW_ANIM_COST=1` — the memo-sizing counter (1370 item 9's bracket): counts the frame's
    // RESTING rows, the shape an input-memo fast path would skip. Sizing only — no skip is
    // taken here. (The EVENT scan is 1482's to gate now — a parked unflagged unit's tracks go
    // unscanned — but the driver still arms clips for every parked rig: the wake pose must be
    // the absolute-clock pose of the RIGHT clip, and only the state machine knows which clip
    // that is. The memo remains the only safe skip shape for these rows.)
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
        // A mount child drives from its HOST's movement view — same inputs the
        // rider's own pass reads, so the mount plays exactly the locomotion the rider suppresses.
        // A vanished host (teardown race) reads as stationary until the child despawns with it.
        // The host also supplies the missing half of the mount's world scale (see the query docs).
        //
        // `ridden_by_self` only feeds the debug trace: while mounted, the animation the director is
        // *looking at* is the mount child's (the rider is pinned to Mount 91), so a trace that
        // followed `SelfPlayer` alone went blind on exactly the body in question — which is how a
        // gallop cadence report had to be diagnosed from asset bytes instead of a run.
        // It deliberately does NOT feed `is_self`: the sheath reconcile and the
        // wire echoes below mean the player's OWN unit, and a mount is not it.
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
        // The rendered world scale of the model playing these clips — the `0x5fe2f0` rate divisor's
        // `|modelScale|`. A plain unit's transform already IS its world scale; a
        // mount child's is its display column, which composes under the rider's.
        let model_scale = transform.scale.x * host_scale;
        let traced = is_self || ridden_by_self;
        // Which body a trace line is about — the rider's own, or the mount it is sitting on.
        let subject = if mount_body.is_some() { "mount " } else { "" };
        let walk = speeds.map_or(DEFAULT_WALK_SPEED, |s| s.0.walk);
        // Dead ⇒ health 0 with a real max, **or** the dead-looking dynamic flag (`unit_reads_dead`
        // — the reference's own `0x605f90`): feign death leaves health untouched and
        // signals purely through `UNIT_DYNFLAG_DEAD`, and the reference runs it through this exact
        // path — the flag's watcher `0x600440` calls `0x60ea30` (play Death) and the per-frame
        // chain's top resolver `0x5fcff0` then claims and holds the corpse pose, the same pair the
        // real death handler `0x625190` runs. So a hunter feigning collapses and lies there, for
        // himself and for everyone watching, and stands back up when the flag clears.
        // Absent health counts as ZERO: a create block omits zero fields, so an already-dead corpse
        // streams in with no HEALTH at all — requiring an explicit `Some(0)` here is exactly the bug
        // where corpses stood up on relog.
        let dead = store.is_some_and(|s| s.0.unit_reads_dead());
        // Mounted: `UNIT_FIELD_MOUNTDISPLAYID` nonzero — the wire's one mounted
        // signal. The rider's base pins to Mount(91) below, Specials/one-shot full-body routes are
        // suppressed, and the sheath reconcile force-stows; the locomotion the selector would have
        // picked plays on the MOUNT child entity instead (its own driver pass, fed the host's own
        // movement view through `MountBody.host` — the `mount_hosts` fetch at the top of this
        // loop). A mount child itself has no `ObjectStore`, so it can never read as mounted here.
        let mount_display = store.map_or(0, |s| s.0.unit_mount_display_id());
        let mounted = mount_display != 0;
        // The transition EDGE, consumed after the death override below ([`AnimDriver::mount_display`]
        // carries the byte-cited why). Latched here, with the other per-frame edge bookkeeping, so
        // a pass that returns early still consumes it: a unit that mounts and dies in the same
        // breath must not have the arm fire late, over the death play that owns bone 0.
        let mount_edge = std::mem::replace(&mut drv.mount_display, mount_display) != mount_display;
        // First time we drive this unit (nothing chosen yet) — used to settle a corpse to its end pose.
        let first = drv.gait.is_none() && drv.mode == Mode::Gait;
        let mut mv = unify(movement, remote, spline, creature_swimming, move_modes);
        // Stand state (decision 0080c): a unit without a controller-fed [`MovementState`] (remote
        // players, creatures) poses from its `UNIT_FIELD_BYTES_1` stand-state byte — a seated
        // remote player renders for free. Our own avatar's rides its `MovementState` (the
        // controller overlays its in-flight request so the pose never waits on the echo).
        if movement.is_none() {
            mv.stand_state = store.map_or(0, |s| s.0.unit_stand_state());
        }
        // Mounted forces stand-state 0 on every recompute (`0x5fdf80`):
        // no sit/sleep/kneel pose can hold in the saddle.
        if mounted {
            mv.stand_state = 0;
        }
        // Stealth — the prowl pose ([`select::STEALTH_WALK`] / [`select::STEALTH_STAND`]): the CREEP
        // vis flag off the unit's OWN descriptor, read every frame for self and remote alike, which
        // is where the client reads it (`[[unit+0x110]+0x213] & 2` at select time). Never
        // controller-fed, unlike the stand state — there is no client-side prediction of stealth, so
        // the crouch lands with the server's aura. A mount child carries no store and reads `false`:
        // right by construction, since the rider's body holds Mount(91) regardless and stealth and
        // mounts are mutually exclusive anyway.
        mv.stealthed = store.is_some_and(|s| s.0.unit_is_stealthed());
        let moving = mv.flags & move_flags::ANY_MOVE != 0;
        // The airborne arc's bookkeeping (jump `0x60e480`, land `0x602c60`): on the arc's
        // FIRST airborne frame, its launch vertical speed splits a **jump** (upward — the client's
        // JumpStart/Jump bracket rides the MSG_MOVE_JUMP event) from a **step-off fall** (level or
        // downward — no bracket; the gait freezes until FALLINGFAR latches Fall 40, and an
        // unlatched landing is a full no-op: the gait keeps rolling).
        let falling = mv.flags & move_flags::FALLING != 0;
        let was_falling = std::mem::replace(&mut drv.was_falling, falling);
        // **The edge is the LAUNCH, not the FALLING bit**. The bit's rising edge is
        // the arc's start only when the body was on the ground the frame before, and on broken
        // ground it often is not: a run across a hillside micro-detaches for a frame or two, and a
        // jump pressed in that window lands and relaunches inside a single frame, with FALLING set
        // continuously across the seam. The bit never toggles, the sample never runs, and the arc
        // inherits `jump_arc = false` from the detachment — so a real jump plays the step-off
        // freeze and the body sails upward still running. Seven of the nineteen jumps in the
        // director's hillside capture took exactly that path (`grounded=0 vy=-0.65`, then
        // `vy=+7.96` the next frame).
        //
        // A launch is visible in the quantity itself: nothing but a jump drives vertical speed
        // *upward* past the threshold. The mover's own arc bookkeeping already agrees — a
        // same-frame land+relaunch is a new arc there ([`crate::player::Player::advance_airborne_arc`]).
        let prev_vertical = std::mem::replace(&mut drv.last_vertical_speed, mv.vertical_speed);
        let launched = mv.vertical_speed > JUMP_ARC_MIN_UP && prev_vertical <= JUMP_ARC_MIN_UP;
        // The resting predicate, counted (never acted on — see the counter's note above): mode
        // Gait with nothing in flight, nothing pending, grounded and motionless. A strict lower
        // bound on what an input-memo would skip.
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
        // The airborne-freeze's exact gate (`0x5fd8e8`):
        // keep-current iff `FALLING && (FALLINGFAR || vz ≠ 0)`. The vz clause pins every real
        // arc (ballistics make vz ≠ 0 from the first integrated substep); the one uncovered
        // case is a fresh walk-off's vz == 0 substep, where the client genuinely may re-pick.
        // Gates the gait slot's mid-air keep-current AND the deferred-cache consumer below
        // (the `+0xd60` read sits downstream of the freeze: a parked clip never plays mid-air).
        let airborne_frozen =
            falling && (mv.flags & move_flags::FALLING_FAR != 0 || mv.vertical_speed != 0.0);
        // The idle re-face turn-shuffle (the client's facing-delta latch
        // `0x607ed0`): a stationary creature easing its yaw
        // toward its target reads as *turning* to the anim layer, so the gait picks
        // ShuffleLeft/Right and each Shuffle→Stand return re-arms (and, relaxed, re-rolls) Stand
        // — the fidget's recurring trigger. Gated like the client's can-fidget gate (`0x5fce30`:
        // no combat/cast); positive step = counterclockwise WoW yaw = turning left.
        if let Some(step) = facing_step.filter(|_| !moving && !engaged && cast_hold.is_none()) {
            mv.flags |= if step.0 > 0.0 {
                move_flags::TURN_LEFT
            } else {
                move_flags::TURN_RIGHT
            };
        }
        // Whether a looping base arm this frame may ROLL its variation (the client's base-arm
        // `variationIdx = −1`) or is forced to the head: decided from the state
        // *before* this frame's transitions (the client tests the outgoing armed id).
        // (0880 also gated this on a stun. That gate cited `0x5eb4f2`/`0x5ec219` as "the idle/fidget
        // selectors bail on UNIT_FLAG_STUNNED" — re-read at the bytes, those two sites are
        // `ToggleSheath 0x5eb480` and `CanLootNow 0x5ec110`, and NO site in the whole `0x40000`
        // census touches animation selection. The stun does not quiet the fidget; the freeze does,
        // by stopping the clock, so the invented gate is gone.)
        let outgoing = drv.active_anim().unwrap_or(STAND);
        let relaxed = !select::arm_forces_head(engaged, cast_hold.is_some(), outgoing);

        wound_upkeep(entity, &mut drv, &mut player);

        // Death overrides every state (a corpse doesn't transition); play Death and hold.
        if dead {
            if drv.gait != Some(DEATH) {
                // One line per collapse, naming WHICH death it is — a body lying down with full
                // health is a feign, and without this the two are indistinguishable in a log.
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
                    // A witnessed death rolls its variation like any one-shot; a
                    // corpse that streamed in dead settles on the deterministic head instead.
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
                        active.seek_to(c.duration); // streamed in dead → settled corpse, not a replay
                    }
                }
                // The death play arms bone 0 — a blended primary re-arm overwrites that bone's
                // secondary slot (the shared-slot eviction): a FULL-BODY wound in
                // flight is evicted; a masked wound rides the key-bone and decays out over the
                // collapse via the upkeep above.
                if let Some(wd) = drv.wound.take_if(|wd| !wd.masked) {
                    player.stop(wd.node);
                }
                drv.mode = Mode::Gait;
                drv.gait = Some(DEATH);
                drv.deferred = None; // the death play is a normal arm — the cache clears
            }
            continue;
        }

        // **The mount transition's own bone-0 arm**. Both legs of the `UNIT_FIELD_MOUNTDISPLAYID`
        // watcher arm the BODY's bone 0 through op4 `0x7121a0` on the PRIMARY slot: the build
        // `0x607b44` plays **91 `Mount`** with a cross-fade, the teardown `0x607ce0` plays seq
        // **0 `Stand`** with the cross-fade **off** (the two legs are the same
        // seven-argument call differing in that one literal, and the difference is visible).
        // Because that is an ordinary last-writer-wins play, it **displaces a full-body one-shot
        // the transition catches mid-clip** — and our gait-slot mount pin (below) never could:
        // `Mode::Swing` holds bone 0 until its clip ends, so the pin waited.
        //
        // That is exactly B203. The mount summon's cast-stage kit (1604/1607 → **54
        // `SpellCastOmni`**) is armed at SPELL_GO, while the caster is still *unmounted*, so it
        // takes the full-body route; the mount field then lands a beat later and, without this
        // arm, the caster sat on the horse finishing a standing spellcast. The reference plays
        // the same clip and then takes bone 0 straight back — which is why the director sees "no
        // cast animation", only the poof.
        //
        // Deliberately NOT touched: the masked overlay. `0x607b44` names bone 0, the key bone is
        // a different per-bone block, and op4's replace-notice fires only for the bone's own
        // primary — so an upper-body clip rides the transition untouched. (While mounted every
        // later one-shot takes the masked route anyway, and the reference's own re-force
        // `0x5fe803`–`0x5fe816` re-pins 91 on every subsequent play; the gait pin renders that.)
        if mount_edge {
            if matches!(drv.mode, Mode::Swing { .. }) {
                drv.mode = Mode::Gait;
            }
            if mounted {
                // A FULL-BODY wound in flight is evicted by a **blended** primary re-arm — the
                // same shared-slot eviction the death play does above: op4's
                // cross-fade path snapshots the outgoing pose *into* the secondary slot
                // (`rep movsd` at `0x7125d9`). Only the BUILD leg blends (`0x607b35 push 0x1`);
                // the teardown's `crossFadeFlag = 0` jumps past that memcpy entirely
                // (`0x71253e`–`0x712543`), so a dismount leaves the secondary alone — decision
                // 0931. A masked wound rides the key bone and decays out on its own, either way.
                if let Some(wd) = drv.wound.take_if(|wd| !wd.masked) {
                    player.stop(wd.node);
                }
            } else {
                // **The teardown CUTS** (the director's "the dismount looks like
                // it's landing after a jump"). `0x607ce0`'s arm is
                // `op4(bone −1, anim 0 Stand, variation −1, bias 0, rate 1.0, crossFade 0,
                // PRIMARY)` at `0x607d1a`–`0x607d2b` — byte-for-byte the build leg's call with
                // **one** literal different: `push 0x0` where `0x607b35` pushes `0x1`. So the
                // saddle pose does not blend away, it is *gone* on the arm's first frame.
                //
                // Blending it away is what the crouch-and-recover was: Mount(91) splays and bends
                // the legs, and easing that into Stand over the clip's blend time reads as a body
                // absorbing an impact. There is no landing clip anywhere near this — the shape of
                // the cross-fade WAS the landing.
                //
                // The gait re-pick below is the arm's own successor, `RecomputeBaseAnim(−1)` at
                // `0x607d30`–`0x607d34` (`0x5fd9e0`: run the base chain `0x5fd8b0`, play the
                // result through `0x5fe2f0` unless it comes back `0xD0`). It blends normally —
                // which is right, because it now blends from **Stand**, not from the saddle.
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
            // Force a fresh gait pick even when the selector's answer is unchanged: the id may
            // still read `Stand` on both sides of a dismount, but the *node* holding bone 0 is
            // the displaced one-shot's, and only a re-arm takes it back.
            drv.gait = None;
            drv.deferred = None; // a normal arm clears the fast-path cache (`0x5fe48e`)
        }

        // The loot kneel's trigger (see [`select::LOOT`]): the
        // leg needs **two** predicates, and both SPLIT on IsActivePlayer.
        //
        // **Self.** Predicate A (`0x6126b0`) is "a loot session is open" — the client-local
        // loot-target latch. Predicate B (`0x612710`) is the per-object-CLASS filter: which
        // *kind* of target the character kneels at. Only B is read here, because
        // [`crate::ui_loot::LootKneel`] already folds in A (it is false whenever the latch is
        // cold). Keeping them apart is the correction 1477 makes to 1471: a fishing bobber arms
        // the latch exactly like a chest does, and the reference still does not kneel at it —
        // the difference is entirely B's `GAMEOBJECT_TYPE_ID == 17` test.
        //
        // **Remote.** The mirrored descriptor: `UNIT_FLAG_LOOTING` (0x400) set AND the
        // `0x10000000` bit clear.
        //
        // Never mounted (the leg's `[+0xdc]==0` gate). Movement outranks by CHAIN POSITION, not an
        // in-leg gate — the core ground selector claims first on any direction bit (`[9e8]&0xf`)
        // — transcribed as the `ANY_MOVE` test here; a stationary *swimmer* with the trigger up
        // kneels mid-tread, as the bytes order it (locomotion's swim block needs a direction
        // bit too).
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

        // No Special claims a mounted rider: the jump/fall arc plays on the mount
        // child (whose synced view carries the same flags), and poses were zeroed above.
        let special = if mounted {
            None
        } else {
            current_special(&mv, drv.jump_arc)
        };
        // The loot leg outranks the standState pose resolver (the `0x5fd8b0` chain calls loot
        // before standState): a looting unit kneels at the loot, never into a sit/sleep bracket.
        // The airborne Specials keep their precedence (the chain's airborne freeze runs first).
        let special = special.filter(|s| !(looting && matches!(s, select::Special::Pose(_))));

        // ── Sheath state. The unit's rendered sheath is the **client-side
        // committed cache** (`drv.sheath_cur`, the client's `[+0xd40]`), not the raw descriptor
        // byte: seeded from the byte at first sight (silently), re-adopted whenever the byte
        // *changes* (the `0x604c70` field-apply — a remote unit's own client volunteered the
        // change; our own echo arrives already-committed and adopts as a no-op), and written by
        // the one-setter requests here + the per-animation reconcile below. Every transition
        // **snaps by default** — across all 24 call sites of SetSheatheState `0x611cf0`: only the
        // manual toggle's request carries the ceremony, and the draw/stow sound rides the ceremony
        // playback (no clip → no sound).
        let sheath_frame_start = drv.sheath_cur;
        let sheath_byte = store.and_then(|s| s.0.unit_sheath_state()).unwrap_or(0);
        if drv.sheath_cur.is_none() || drv.sheath_byte != Some(sheath_byte) {
            drv.sheath_cur = Some(sheath_byte);
            drv.sheath_byte = Some(sheath_byte);
        }
        // The one setter's queue ([`SheathRequest`] — the Z toggle, the attack-start auto-draw,
        // the stand-state stow rider): idempotency refusal (the client's `newState == CUR`
        // abort), commit, the local player's `CMSG_SETSHEATHED` volunteer, and — for the manual
        // toggle alone — the ceremony: **per-arm masked overlays** (the client's per-slot plays,
        // `0x60b770`): the mainhand's on the right-arm subtree, the offhand's on the left, each
        // by that item's stow family (hip → HipSheath 90, back/shield → Sheath 89), composed
        // over whatever the body is doing and never cancelled; [`VisualSheath`] pins the old
        // weapon placement until the authored `$SHL`/`$SHR` event (~halfway — the hand at the
        // stow point). A ceremony without playable clips degrades to the snap.
        if let Some(req) = pending_sheath.get(&entity) {
            let cur = drv.sheath_cur.unwrap_or(0);
            // Mounted is a persistent DRAW-BLOCK (the client's
            // recompute `0x5fdf80` tests the mount model FIRST and forces stowed on every
            // PlayAnimation — nothing can stay drawn on a mount). The client's composite
            // recomputes constantly (the mount's own gait plays); our rider track goes silent
            // under the seat pin, so the equivalent is refusing the draw at the setter — no
            // commit, no volunteered CMSG (named deviation: same observable, earlier site).
            if req.state != cur && !(mounted && req.state != 0) {
                // The setter's own trace, the twin of the reconcile's below: between them every
                // sheath transition names itself and its author, so "why is the weapon out"
                // is one `RUST_LOG=benilla=debug` grep rather than a re-derivation of the
                // 24-site policy from the symptom.
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
        // Ceremony upkeep, per arm: move that arm's weapon when its clip crosses the authored
        // `$SHL`/`$SHR` event (the hand-touches-weapon moment — the draw/stow ring fires there,
        // `sound::sheathe`, since the sound can't ride the tags themselves: hip clips carry none),
        // and when a *stow* clip finishes, run the client's **phase 2** — the on-anim-finish
        // drawer `0x5fc920` @ `0x5fca8c`/`0x5fcaa1`. That deferred second movement is what makes a
        // melee → ranged toggle read as "put the swords away, come back to neutral, *then* reach
        // over the shoulder for the bow" instead of one blended swap.
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

        // ── This frame's one-shot: a melee swing (one per SMSG_ATTACKERSTATEUPDATE)
        // or an anim-emote. A swing outranks an emote when both land (combat wins).
        // Its destination is chosen **per play from the unit's live state** (
        // `route_oneshot`): **masked** onto the SpineLow overlay (moving / seated / airborne-in-combat)
        // — playing beside `mode`, the base track's legs untouched — or **full-body** on the base
        // track (standing idle), replacing whatever the base was doing.
        //
        // The shared-slot eviction's inputs: a wound-flinch overlay is the client's
        // per-bone SECONDARY, and a blended primary re-arm on the *same bone* overwrites that slot
        // (op4 `blendFlag≠0` copies the outgoing pose over `+0xc4..`). So this frame's plays are
        // tracked — full-body plays (bone 0: the base track) evict a full-body wound; masked-slot
        // plays (the key-bone) evict a masked wound. A play on the *other* bone leaves the wound
        // decaying (the kernel `0x714260`'s inherited-swing case). Mode/gait changes proxy the
        // mode machine's own base plays; the flags catch the same-id re-plays the proxy can't see.
        // ── The **base-animation lock**'s clearer, keyed on the FINISHED id and run before
        // anything re-picks the base this frame — the reference clears at `0x5fc9c6`, above
        // `OnAnimationFinished`'s own reason branch, so completion and pre-emption both release it
        // ([`play::BaseAnimLock`]).
        drv.base_lock.release_finished(&player, anims, catalog);

        let pre_state = (drv.mode, drv.gait);
        let mut base_played = false;
        let mut masked_played = false;
        let mut played_oneshot: Option<u16> = None;
        // The victim's cached creature template carries `DO_NOT_PLAY_WOUND_ANIM` (`type_flags`
        // bit `0x8`) — the reference's `0x6125f0`, read at its two consumers below: the parry
        // pick `0x60ec1f` and the wound flinch `0x60ea9f`. Keyed on the
        // descriptor's `OBJECT_FIELD_ENTRY`, the way `0x60b160` keys the query record, and false
        // on a template we have not received — `0x6125f0`'s own null leg.
        let no_wound_anim = store
            .and_then(|s| s.0.object_entry())
            .zip(names.as_deref())
            .and_then(|(entry, n)| n.creature_record(entry))
            .is_some_and(|r| r.type_flags & DO_NOT_PLAY_WOUND_ANIM != 0);
        // Defense outranks a same-frame own swing/emote: the client's `$CPP` arm is the later
        // PlayAnimation call in that (rare) frame, and the being-hit reaction is the one the
        // player must read. Gated alive (`0x60ec00` checks IsDead / stand-state 7 before the LUT).
        let defense = pending_defense.get(&entity).and_then(|&vs| {
            if dead {
                return None;
            }
            // `DO_NOT_PLAY_WOUND_ANIM` takes the **parry** with the flinch (`0x60ec1f`, the bit's
            // second and last consumer).
            // The gate is inside `0x60ec00`, which the `$CPP` ladder enters only on
            // victimState 3: DODGE/DEFLECT (30) and BLOCK (24) go straight to PlayAnimation from
            // `0x624a90`/`0x624a74` and are NOT gated — a flagged creature still dodges and still
            // raises its shield, it just never parries.
            if vs == 3 && no_wound_anim {
                return None;
            }
            // The parry LUT `0x60ec00` reads the victim's mainhand through `GetWeapon(0, 0)`
            // (1863), so a disarmed victim's parry finds no weapon and the client bails — no
            // clip, which is this function's own empty-hand leg.
            let id = defense_anim(vs, wielded.and_then(|w| w.armed_main()));
            if let Some(id) = id {
                debug!("defense: unit {entity} anim {id} (victimState {vs})");
            }
            id
        });
        // The frame's play calls, resolved to anim ids in [`PlaySeq`] call order; the defense
        // reaction runs last (the client plays it from the deferred impact scan, later in the
        // frame than any message handler — decision 0399's surviving arm).
        let mut requests: Vec<u16> = pending
            .remove(&entity)
            .map(|mut v| {
                v.sort_by_key(|&(_, seq)| seq);
                v.into_iter()
                    .map(|(req, _)| match req {
                        OneShotReq::Swing(hit_info) => {
                            let w = wielded.copied().unwrap_or_default();
                            // The COMBAT reading of the hand ([`Wielded::armed_main`]): the
                            // reference's swing selector `0x6246a0` calls `GetWeapon(slot, 0)`,
                            // so a disarmed attacker's weapon reads as absent and this lands on
                            // AttackUnarmed(16) / AttackUnarmedOff(117) — with the weapon model
                            // still in the fist.
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
        // The play-time substitution ([`select::unarmed_special`], `0x5fe2f0`): a Special1H/2H
        // asked for by a unit whose hands both read empty comes out as SpecialUnarmed(118). It
        // lives at the PLAY seam in the reference, so it is applied to every request here rather
        // than inside any one selector — a disarmed Eviscerate and a weaponless one are the same
        // case to it.
        {
            let w = wielded.copied().unwrap_or_default();
            for id in &mut requests {
                *id = select::unarmed_special(*id, w.armed_main(), w.armed_off());
            }
        }
        // The deferred-cache consumer (the client's `+0xd60` read at the base recompute,
        // decision 0406): the moment no one-shot is live, the parked combat clip plays — the
        // swing the Eviscerate spin deferred fires once the spin ends. A frame with fresh
        // requests consumes it implicitly instead (a normal arm clears the cache; a fast-path
        // hit re-parks its own request — both what the client's `0x5fe48e`/`0x5fe480` do).
        // NEVER mid-air: the read (`0x5fd392`, inside the `0x5fd360` recompute arm) sits
        // downstream of the airborne-freeze, so a park made mid-arc waits — and dies at the
        // landing play's clear.
        if requests.is_empty()
            && drv.deferred.is_some()
            && !airborne_frozen
            && live_oneshot(&drv, &player, &tr, anims, catalog).is_none()
        {
            requests.extend(drv.deferred.take());
        }
        for id in requests {
            // **The lock, first** — `0x5fe2f0`'s head guard returns before any routing, descriptor
            // build or slot election, so it precedes the fast path and the dedup below and takes
            // the deferral with it: "no routing, no arm, no deferral" ([`play::BaseAnimLock`]).
            // This is what keeps a stunned victim knocked flat for the whole clip.
            if drv.base_lock.refuses() {
                continue;
            }
            // The COMBAT FAST-PATH (`0x5fe43c`–`0x5fe48b`,
            // decision 0406): a combat clip requested while another combat clip is playing is
            // NOT armed — the CURRENT clip's rate doubles (op6 2.0f re-times its remainder,
            // pose-continuous) and the request parks in the `+0xd60` cache to play afterwards.
            // This is why the Eviscerate spin survives the auto-swings its cast triggers —
            // sped up, never cut — and why consecutive swings don't hard-cut each other.
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
            // A normal arm clears the cache (the client's `0x5fe48e` writes −1 on every
            // non-fast-path PlayAnimation).
            drv.deferred = None;
            // The arm-level same-id dedup (`0x5fdba0`): a requested id that
            // already occupies its slot and is still playing is NOT re-armed — the mechanism
            // that lets a looping eat/drink kit clip free-run across the server's ~5 s kit
            // resends. Combat same-id re-plays never reach it (the fast-path above catches
            // them first — the client's head-of-function order).
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
            // Mounted forces the masked route (`0x5fe2f0`'s mounted branch:
            // upper-body one-shots route to the key-bone while mounted, and the 91-force would
            // reclaim bone 0 on the next play anyway): a full-body /wave would replace the seat
            // pose with a standing wave floating over the saddle.
            let masked =
                mounted || route_oneshot(id, mv.flags, mv.stand_state) == OneShotRoute::Masked;
            // The play's two rolls: resolve to what this model has, then
            // pick among that id's variations — the alternating swing arcs — and roll the replay
            // budget (a clamp one-shot authored `(min,max)` plays R times before releasing).
            let picked =
                find_resolved(anims, id, catalog).map(|h| roll_oneshot(anims, h, &mut rng));
            let upper = masked
                .then(|| picked.and_then(|(c, r)| c.upper_node.map(|n| (n, r))))
                .flatten();
            if let Some((node, repeat)) = upper {
                // Masked route: the SpineLow overlay, beside `mode`. The base machine runs untouched.
                // The re-arm is **blended** (op4 `blendFlag = 1`): whatever held the
                // key-bone retires into the fade slot and this clip rises over its own blendTime
                // (`0x7125f2` — the INCOMING sequence's `M2Sequence+0x20`), so a masked swing never
                // swaps the torso in one frame. Note the full-body branch below deliberately does
                // NOT touch the overlay: on that route the client's key-bone slot keeps its current
                // descriptor (a dedup no-op), so a standing emote over a still-running masked swing
                // plays legs-only underneath it (`0x5fe930`).
                retire_overlay(
                    &mut drv,
                    &mut player,
                    picked.map_or(0.0, |(c, _)| c.blend_time.max(0.0)),
                );
                let active = player.play(node);
                active.replay();
                active.set_repeat(repeat); // always set — a reused node never keeps a stale count
                active.set_speed(1.0); // nor a stale rate (a prior whiff slow-down, decision 0279)
                active.set_weight(0.0); // the fade upkeep raises it from λ = 1 this same frame
                drv.overlay = Some(Overlay {
                    node,
                    id,
                    looping: false,
                });
                masked_played = true;
                played_oneshot = Some(id);
            } else {
                // Full-body route (standing idle / airborne non-combat), or the split-boneless
                // masked fallback (the client's −1 key-bone sentinel arms bone 0 too): the clip
                // replaces the base on bone 0 — **even over a Special**. The client never drops a
                // play: one slot, last-writer-wins (the route `0x5fe6c8` puts
                // a jump-in-place cast/emote on bone 0, replacing the hang — decision 0864 is the
                // ref's mid-air cast). Cutting an airborne clip freezes the outgoing node first —
                // the pose-snapshot decay of the client's op4 blend, scoped exactly like
                // [`leave_special`]'s.
                if let Some(sp) =
                    special.filter(|sp| matches!(sp, select::Special::Jump | select::Special::Fall))
                {
                    // …the ARC's clip, not merely whatever bone 0 holds ([`holds_own_clip`]) —
                    // the freeze's missing predicate, at its second site.
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

        // The whiff slow-down (`0x712910`): a miss/dodge/evade drops the attacker's
        // in-flight swing to HALF speed for its remainder — a slowed follow-through (the verified
        // 0.5 rate write), never a cut. The client writes bone 0's rate blindly; benilla scopes it
        // to the swing's own node (masked overlay or full-body main), so a moving attacker's gait
        // keeps its pace — a deliberate deviation, named in 0279. `Mode::Swing` holds ANY
        // full-body one-shot (a spell kit's Special1H rides it too), so the base arm re-checks
        // the id — a whiffing auto-attack must not drag a concurrent special to half speed.
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

        // A Special EDGE is a play (the jump/pose entry, the FALLINGFAR latch's Fall, the land
        // pick — each a normal PlayAnimation), and every normal arm clears the deferred-combat
        // cache (the client's `0x5fe48e`): a swing parked behind the spin dies when a jump cuts
        // the spin. The *level* must not clear (decision 0864 corrects the old per-frame kill):
        // mid-air the airborne-freeze issues no plays, so a fast-path park made mid-arc
        // survives to its clip's end, exactly like the ref.
        let special_edge = special != drv.last_special;
        drv.last_special = special;
        if special_edge {
            drv.deferred = None;
        }
        // The looping-variation ADVANCE (
        // the per-frame watchdog `0x719370`): every looping arm installed a window `R`
        // clip-lengths wide; when the armed node — still the MAIN animation (the client checks
        // the armed block: any newer arm superseded the window) — completes its `R` passes, the
        // completion callback re-arms the same id with `variationIdx = −1`: a fresh weighted,
        // MEMORYLESS pick with a fresh window. One id's weighted walk is what alternates a
        // gryphon's flap/glide (freq 26214/6553) and strings a five-part /dance together; a
        // single-variation loop just restarts — the wrap it was already playing. Units always
        // opt in (the client's `model+0x70` callback — every CGUnit controller installs it;
        // the callback-less map doodads, which loop one variation forever, are `doodad_anim`'s
        // separate concern). A combat/cast re-arm keeps the deterministic head, like any base
        // arm. The carried rate keeps the gait's scaling; the per-frame sync re-syncs it anyway.
        //
        // **In the GAIT slot the completion is a re-SELECTION, not a re-arm**.
        // The client's callback is `RecomputeBaseAnim(−1)` — `0x5fc3f0`'s jump-table row
        // `0x5fc844` is literally `push -1; call 0x5fd9e0`, which runs the whole `0x5fd8b0`
        // selector chain. For Fly and /dance the chain hands back the id it was already playing,
        // so a same-id re-arm and a re-selection are indistinguishable and this code read as
        // right for two years. For the turn-shuffle they are the difference between stepping once
        // and shuffling for ever: the shuffle's window is the ONLY thing that ends it (the
        // per-frame tail refuses to — see [`select::SHUFFLE_LEFT`] and `mode`'s hold), so if the
        // completion re-armed 11 the vendor would never stop. Clearing the target IS the
        // re-selection — `mode::run` picks fresh a few lines below, rolling the variation exactly
        // as this arm would have. Outside the gait slot (a Special loop) the id is not the
        // chain's to choose, so the same-id re-arm stands.
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
        // ── The **stage-2 base recompute** (`0x60f389`–`0x60f399`): a state kit
        // that names an animation compares it against the id the unit is already playing
        // (`0x5fdb50` — the upper-body key bone if it holds one, else bone 0) and, on a
        // difference, runs `0x5fd9e0(unit, -1)`. It never plays the id, and on a match it does
        // nothing at all.
        //
        // It runs HERE, after this frame's one-shot arms, because that ordering is the whole
        // observable effect: the impact kit (stage 1) plays first and the state kit (stage 2)
        // then cuts it. Charge (22911) is the case that names itself — `Knockdown`(121) from kit
        // 348, cut by kit 349's `Stun`(14) because 121 ≠ 14.
        //
        // Expressed as the re-selection, not a re-arm: clearing the gait target IS the recompute,
        // and dropping `Mode::Swing` is what ends a one-shot. **Named residual:**
        // a unit inside a Special (a pose, an airborne arc) is left alone — the reference's
        // recompute re-enters the selector chain, which would land back in the same state, and
        // forcing `Mode::Gait` here would replay the pose's enter clip instead.
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

        // ── **The mode machine** — the base track's whole decision, lifted into [`mode`].
        // It is the one phase of this pass with a clean input boundary: a fixed
        // set of already-computed facts in, the driver + player + transitions out, and none of
        // this loop's frame-local flags touched.
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

        // ── The base slot's **playback rate**, written once per frame over whatever the mode
        // machine settled on — the client's rate write sits OUTSIDE the selector (`0x5fe2f0`), so
        // it is not the gait's private business. The landing is why it matters:
        // JumpLandRun 187 resolves to **Run(5)** on every creature model's baked lookup, so a
        // mount's or a travel form's land clip is its own gallop cycle and must be rate-scaled
        // like any other locomotion — at the arm's literal 1× it ran ~35% slow for the clip's
        // whole 0.8 s. It also records the result in `gait_rate` — the number the hover card and
        // the trace's `rate=` read — so those stay honest in every mode too. See
        // [`play::sync_base_rate`].
        play::sync_base_rate(&mut drv, &tr, &mut player, anims, mv.speed, model_scale);

        // ── Masked one-shot completion — the **fade-to-rest** (correcting 0087 (c)):
        // a finished overlay is NOT stopped. The client latches the completion in its per-frame
        // model advance (`0x719370`) and delivers a deferred event the same frame to
        // `CGUnit::OnAnimationFinished 0x5fc920`, which disarms the key-bone through op4
        // `param_3 = -1` — snapshotting the clip's **held final frame** into the bone's secondary
        // slot and cross-fading it back onto the inherited base over a fixed 150 ms. So the torso's
        // return is a blend out of the follow-through, never the one-frame cut this used to do
        // (the director's "the end of the cast is cut off and it instantly snaps back"). A looping
        // cast-hold overlay never completes — the hold block below owns its release.
        if let Some(ov) = drv.overlay {
            let done = if ov.looping {
                false // a cast-hold loop: the hold block below owns its release
            } else if find_resolved(anims, ov.id, catalog).is_some_and(|c| c.looping) {
                // A hold-less overlay whose CLIP is authored looping (a pushed kit's seated
                // eat/drink gesture) has no natural end — release it when the body is claimed by
                // MOVEMENT. INTERIM: the client's exact release chain for this case
                // is unpinned; supersede-by-a-later-play already evicts above. A **Special** no
                // longer counts: a jump is a bone-0 play, and bone-0 plays never
                // touch the key-bone slot — cutting the torso on takeoff was the same defect the
                // fade above fixes at the other end.
                moving
            } else {
                player.animation(ov.node).is_none_or(|a| a.is_finished())
            };
            if done {
                // Freeze the held frame, then retire it: the client's snapshot copies an already
                // *expired* window, so the secondary clamps to `seq.end` and the fade runs from
                // the clip's final pose (`0x7123af`).
                if let Some(a) = player.animation_mut(ov.node) {
                    a.set_speed(0.0);
                }
                retire_overlay(&mut drv, &mut player, ONESHOT_RELEASE_FADE);
            }
        }

        // ── The committed-move cast hold: a casting/channeling unit that
        // translates or swims loops the hold clip on its torso over the gait — the masked route a
        // moving caster falls through to (the stationary case pinned the gait slot above; the
        // split is the same `[9e8] & 0x20000f` gate, [`move_flags::CAST_PIN_MOVE`] — a merely
        // TURNING caster stays pinned there). A
        // masked one-shot in the slot (a swing over the run) wins while it plays; the hold
        // re-takes the subtree the frame after it finishes (the client's last-writer-wins slot).
        // Its start is a play like any other, so the sheath reconcile below sees it (`hold_played`).
        // A **Special no longer cancels it**: jumping mid-cast is a bone-0 play
        // (JumpStart takes the legs) and the client's key-bone slot is untouched by it —
        // `0x5fe912` routes a locomotion request straight to bone 0 when the key-bone is armed.
        // Dropping the hold on takeoff was exactly the director's "jump-running cuts the cast".
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
                // Free slot, or a stale hold loop (the channel switched spells): (re)take it — a
                // blended key-bone re-arm like any other, so the outgoing pose
                // retires under the hold's own blendTime instead of vanishing.
                retire_overlay(&mut drv, &mut player, blend);
                let active = player.play(node);
                active.replay();
                active.repeat();
                active.set_weight(0.0); // the fade upkeep raises it from λ = 1 this same frame
                drv.overlay = Some(Overlay {
                    node,
                    id,
                    looping: true,
                });
                hold_played = Some(id);
            }
            (None, Some(ov)) if ov.looping => {
                // The hold ended, or the unit stopped — release the subtree (the standing pin or
                // the gait takes over). A looping clip never completes, so the client's
                // completion-driven release never fires for it and its true release chain is
                // still unpinned (0107/0280 INTERIM); it borrows the verified fade-to-rest shape
                // rather than snapping, which is right either way.
                retire_overlay(&mut drv, &mut player, ONESHOT_RELEASE_FADE);
            }
            _ => {}
        }
        // The key-bone cross-fade's advance, last — after every arm/release this frame, so the
        // weights land on the slot's settled state (the client's kernel likewise runs λ after the
        // frame's PlayAnimation calls).
        overlay_fade_upkeep(&mut drv, &mut player, dt);

        // The anim half of the `WOW_MOVE_TRACE` debug trace ([`benilla_assets::trace`]): one line per
        // frame the SELF unit's settled anim state *changes* — mode, gait, Special, flags, the
        // rate-driving speed, and **both slots** — interleaved with the mover's `move` lines on the
        // same clock, so a feel report ("the char's frames snap on landing") pins which layer moved
        // and when. It runs HERE, after every arm/release/fade this frame, so `upper` is the
        // settled key-bone state: without it a torso report ("the cast got cut") could not be told
        // from a base one at all. `+fade` marks the 150 ms fade-to-rest running —
        // a boolean, not a countdown, so a fade doesn't spam a line per frame.
        //
        // `base=` is the **resolved** clip the full-body slot actually holds — not the id the
        // selector *asked* for, which `gait=`/`mode=` already carry. A cadence report ("the run
        // goes slow when I land") is unreadable without it: the request was JumpLandRun 187, the
        // clip playing was Run 5, and `rate=` was the wrong number for exactly that clip.
        // `?` = the slot holds a node this model has no clip record for.
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
                // The rate's new input and its result: `speed` alone stopped
                // predicting the gait's playback the moment the model scale joined the divisor,
                // so a "the gait looks wrong" trace carrying only speed could no longer answer it.
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

        // A mode/gait change is a **proxy** for the mode machine's plays, and the base-anim lock
        // is the one state where the proxy is wrong for longer than the single frame its own doc
        // allows: while the lock refuses, the machine still walks its brackets — Gait →
        // Entering(Jump) → Looping(Jump) → Land — with every play declined, so a change there
        // means nothing was armed at all. Claiming otherwise evicted the victim's flinch off the
        // key-bone and latched the weapon trail's edge (2076) on a play that never happened.
        // Only the PROXY is gated: an arm that reached the player directly says
        // so on its own.
        let base_played =
            base_played || ((drv.mode, drv.gait) != pre_state && !drv.base_lock.refuses());
        // The weapon-trail latch's edge — `0x5fe2f0` is the image's single
        // animation entry point, so ANY start consumes the arm, the mode machine's gait plays
        // included. Written every pass (never OR'd) so it is exactly this frame's.
        drv.started_anim = masked_played || base_played;
        wound_evict(entity, &mut drv, &mut player, masked_played, base_played);

        if let Some(&edge) = pending_wound.get(&entity) {
            // Both edges pick the wound id by severity + the victim's engagement (decision
            // 0111's `0x60ea70`); a spell edge is the client's `severity = 0` call (decision
            // 2058), so it is CombatWound/StandWound by engagement and never the kit's own id.
            let id = match edge {
                WoundEdge::Melee(hit_info) => select::wound_anim(hit_info, engaged),
                WoundEdge::Spell => select::wound_anim(0, engaged),
            };
            // The trigger's entry gates, in the reference's own order inside `0x60ea70`:
            //
            // 1. `0x60ea9f` → `0x6125f0`: the victim's cached creature template carries
            //    `type_flags` bit `0x8`, **DO_NOT_PLAY_WOUND_ANIM** — a skeleton, a ghost, a
            //    bone golem has no flesh to recoil, and the reference refuses every flinch it
            //    would ever take, melee and spell alike. It gates the ANIMATION
            //    only: the blood spurt is a separate system reading [`SwingImpact`] itself, and
            //    nothing keyed on creature type suppresses it (`0x624530` → `0x625010`) — a
            //    skeleton bleeds.
            //    A record we have not received yet reads as NOT flagged, which is `0x6125f0`'s
            //    own null-record leg (`return record ? … : false`), so a creature whose query is
            //    still in flight flinches there too.
            // 2. `0x60eaac`–`0x60eac8`: a CharProc-11 rate-override node on the
            //    unit's effect list — the freeze auras' node, Freezing Trap / Ice Block /
            //    petrify / web wrap — refuses EVERY flinch for as long as it lives. The gate is
            //    the node's presence, not its rate (kit 3071's 1.0 gates too), so it reads the
            //    node list, not the pause it applies.
            //
            // `no_wound_anim` is read once above, where the parry pick — the flag's other
            // consumer — needs the same answer.
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

        // ── The per-animation sheath reconcile (decision 0080 structure 3 — the client's
        // `0x5fdf80`, run after every animation pick). The playing clip's `AnimationData.dbc`
        // WeaponFlags + engagement force the state, data-driven — swimming/looting/casting stow,
        // punching stows (fists need empty hands), armed attacks and the Ready idles draw — and
        // a remote unit with no force pulls back to the server byte. Every force is a SNAP; the
        // local player's force volunteers `CMSG_SETSHEATHED` (`bFireEvent = 1`), which the
        // server echoes into the descriptor for observers. This is why a vendor gossip stows:
        // the talk emote's flags, not a vendor wire (0080's resolved mystery) — future systems
        // (gossip, loot, mounts) inherit the policy with zero new sheath wiring.
        // The reconcile tests the **requested** id, not the model-resolved substitute (decision
        // 0125, director-eyes falsification of 0082's resolved-id reading): the client's arm
        // descriptor carries the id that was ASKED FOR (the cast-override `0x5fde80` writes the
        // kit's own anim id even on a model lacking the sequence), and `0x5fdb50` reads that
        // descriptor. The two differ only when a model lacks the requested clip — the ground
        // truth is GnollCaster.m2: no ReadySpellDirected(51) at all, its hold falls back to a
        // flags-less Stand for *playback*, yet the ref still stows the staff for the whole
        // windup — only 51's own `&4` row can do that. (0082's chicken rationale collapses under
        // the same lens: requested Attack2H's 0x20 vs resolved AttackUnarmed's 0x10 is
        // unobservable on a weaponless chicken.) The byte reading of `0x5fdb50`'s miss path is
        // open; the director's eyes outrank the old reading meanwhile.
        // The reconcile must see **every play** (decision 0087 (d)): the client's `0x5fdf80` runs
        // inside `PlayAnimation` itself, so a *masked* one-shot — which never touched `mode`, so
        // `active_anim()` still reports the base gait — is reconciled all the same. A masked emote
        // carrying WeaponFlags `0x10` still stows. So this frame's routed one-shot id (either route)
        // takes precedence for the reconcile; absent one, the base track's playing id is tested.
        // And it must see **only plays** (the same byte-fact's other half): `PlayAnimation` is the
        // reconcile's *sole* trigger, so a frame where nothing played keeps the committed state
        // untouched. Re-running it every frame against the base track was the caster staff bug: a
        // masked cast hold stows on its retake play, then the very next frame the base gait
        // (flags-less Stand/Run) + the engaged/server-byte re-assert yanked the weapon back out —
        // the client instead holds the stow across the base loop's silent wraps until something
        // actually plays. The server-byte adopt above needs no play, exactly like the client's
        // field-change watcher (`0x604c70`).
        // …and, beyond plays: a mounted unit whose committed state reads drawn anyway (the
        // descriptor byte-adopt above is the one write the setter gate can't refuse) re-arms
        // the reconcile directly — the client's recompute would run on the composite's next
        // mount-gait play, a beat our silent rider track never gets. Forces once (the mounted
        // branch yields 0, cur becomes 0, the re-arm goes quiet) — no every-frame oscillation
        // (the caster-staff trap below).
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
                // The ranged wind-up **bracket**: the client surrounds every ranged kit play with
                // ranged snaps — `0x60f34c` before the play, the outer send/START `SetSheatheState
                // (2,1,1)` after — so ReadyThrown 108's genuine force-stow (NOT in the `0x5fe180`
                // exempt set) exists but never renders: the snap always wins the frame. Our
                // frame-spread ECS can't reproduce the call-stack order, so the bracket is
                // stated directly: while a ranged shot's precast hold is live, a reconcile
                // stow resolves to the drawn-ranged state instead. (Named deviation: the real
                // *remote* thrower's wind-up ends CUR=0 until its GO — a derived, unobserved
                // corollary; we keep every caster at 2, the observable the director verified.)
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
        // Leaving the ranged sheath stance un-nocks (the client's `0x60fc72` un-nock, gated
        // `[+0xd40] != 2`): however the state left 2 this frame
        // — a stow/melee request, the reconcile, the server byte — the ammo model drops.
        // Re-drawing shows no ammo until the next `SMSG_SPELL_START` re-affirms it.
        if sheath_frame_start == Some(2) && drv.sheath_cur != Some(2) {
            commands
                .entity(entity)
                .remove::<(super::NockedAmmo, super::NockLatch)>();
        }
        // …and **TRANSLATING** un-nocks too — the arrow leaves the hand and the string relaxes,
        // while the cached ammo display survives for the next pull and the weapon stays DRAWN.
        // `0x60e480` is a movement-OPCODE dispatcher whose
        // jump-table arm 0 runs `0x60e4ac call 0x60f530` (un-nock) + `RecomputeBaseAnim(-1)`, and
        // arm 0 is reached by exactly MSG_MOVE_START_{FORWARD, BACKWARD, STRAFE_LEFT,
        // STRAFE_RIGHT, SWIM} (181/182/184/185/202). **Turning and stopping do NOT un-nock** —
        // they take arm 1, which recomputes only. So the mask is the four translation bits plus
        // SWIMMING, never the turn bits: [`move_flags::ANY_MOVE`] is exactly the former, and this
        // is the one place that needs the swim bit with it.
        //
        // The reference's trigger is the START opcode (an edge); a level test is equivalent here
        // because the only thing that can re-latch is `$BWP`, authored solely in the Load clips —
        // which a translating unit never plays. Sheath is untouched (`[+0xd40]` has no
        // movement-path writer: only the ctor's `0x5fafa0` and `0x611cf0` write it).
        if mv.flags & (move_flags::ANY_MOVE | move_flags::SWIMMING) != 0 && nock_latched {
            commands.entity(entity).remove::<super::NockLatch>();
        }
    }
    if anim_cost && cost_rows > 0 {
        use std::sync::atomic::{AtomicU32, Ordering};
        static FRAME: AtomicU32 = AtomicU32::new(0);
        // Once a second at 60 fps, not per frame — the counter is a sizing read, not a trace.
        if FRAME.fetch_add(1, Ordering::Relaxed).is_multiple_of(64) {
            eprintln!("[anim-cost] rows={cost_rows} resting={cost_resting}");
        }
    }
}

/// `WOW_ANIM_COST=1` — arms the memo-sizing counter above. Read once, the meter posture.
fn anim_cost_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_ANIM_COST").as_deref() == Ok("1"))
}
