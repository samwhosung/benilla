//! GameObject state animation: `GAMEOBJECT_STATE` drives a skeletal M2 sequence, so a door
//! swings, a button depresses and a chest lid opens and closes.
//!
//! The 1.12 client keeps one stored state per object (`go+0x27c`), set only through `SetGoState`
//! (`0x5f8bd0`); [`GoAnim::state`] is that state. The server never flips a chest's wire state, so
//! an open-lock cast and the loot release move its lid.
//!
//! A state maps, with no inversion, to a held rest pose ([`rest_anim`]), and a change plays a
//! one-shot motion onto it ([`motion_anim`]). Keyed by `AnimationData.dbc` id: the machine's
//! internal index and the debug state names at `0x860850` are off by one.
//!
//! The loop bit (`flags` bit 0, `0x714585`) does not end a motion; the object layer's completion
//! does, so every arm is one window ([`RepeatAnimation::Never`]) and [`retire_transient_anim`]
//! advances the machine at its end.
//!
//! Not built: `GAMEOBJECT_ANIMPROGRESS`, which picks the motion at spawn and seeks it to
//! `duration × progress / 100` (`0x5f3ac5`), and the reverse blend of an interrupted swing.

use avian3d::prelude::{Collider, ColliderDisabled};
use benilla_assets::ModelAnimations;
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::RepeatAnimation;
use bevy::prelude::*;
use std::time::Duration;

use crate::creature_anim::{advance_track, scan_events, AnimSoundEvent};
use crate::net::{GuidIndex, ObjectStore};
use benilla_world::schedule::WorldStage;

/// `GO_STATE_ACTIVE` (vmangos `GOState`): open, and passable.
const GO_STATE_ACTIVE: u32 = 0;
/// `GO_STATE_READY` (vmangos `GOState`): closed, the one state in which a door or button blocks.
const GO_STATE_READY: u32 = 1;

/// `AnimationData.dbc` 157 Despawn, the one-shot channel's code 6 (`0x80b0e0[6]` = substate 12,
/// `0x8607e4[12]` = 157): the object plays it once, then goes.
const ANIM_DESPAWN: u16 = 157;

/// An animated GameObject's client-side state, tagged by [`crate::entities::attach`] in place of a
/// creature's `AnimDriver`, and driven by [`drive_go_anim`].
#[derive(Component, Default)]
pub(crate) struct GoAnim {
    /// The stored `GAMEOBJECT_STATE` (`go+0x27c`), the one truth for animation and collision;
    /// `None` until first sight.
    state: Option<u32>,
    /// The state last animated to, which picks the next motion; first sight settles silently, so a
    /// door that streams in open does not swing.
    shown: Option<u32>,
    /// A pending one-shot (an `AnimationData.dbc` id) on the reference's one slot, fed by
    /// `0x5f8c50(GO, code)` (slot 15) through `0x80b0e0`: code 1 Spawn on an update type 3 create
    /// (not built), 2..5 Custom0..3 from opcode `0xb3`, 6 Despawn from opcode `0x215`.
    one_shot: Option<u16>,
    /// The armed transient substate (a motion, or Custom0..3), the reference's one
    /// `[handler+0x10]` slot; [`retire_transient_anim`] advances it at its window end.
    transient: Option<Transient>,
    /// The rest pose's armed clip while it is on the re-arm cycle; `None` while a transient owns
    /// the model, on a frozen leg, and once the already-playing skip has refused a re-arm.
    rest_window: Option<AnimationNodeIndex>,
    /// The state last dispatched (`0x5f3cb0(old, new)`, [`GoStateDispatch`]); the retire's
    /// re-resolve is not a dispatch.
    dispatched: Option<u32>,
    /// The requested id currently armed (`[block0+0xf8]`, written by op4 `0x71252f`, read by
    /// `0x712090(model, -1)`), which the already-playing skip compares.
    armed_id: Option<u16>,
}

/// The dispatch `0x5f3cb0` ran on this entity; its first act (`0x5f3cc8 call 0x5f40c0`) releases
/// the display-sound loop [`crate::sound::gameobject`] owns. A rest pose's own re-arm
/// (`0x5f4167`) is no dispatch, so a brazier's `TorchLoop` hums on.
#[derive(Message)]
pub(crate) struct GoStateDispatch(pub(crate) Entity);

/// Slot 14 sends a completed Spawn (145) or Custom0..3 (153..156) back through the dispatch
/// (`0x5f4190`); a motion advances (`0x5f413d`, `0x5f414b`, `0x5f4159`) and a rest pose re-arms
/// (`0x5f4167`) without it.
fn completion_redispatches(id: u16) -> bool {
    matches!(id, 145 | 153..=156)
}

/// The armed transient clip. The arm rolls a variation, so [`retire_transient_anim`] watches the
/// node that was armed, not the id's head.
#[derive(Clone, Copy)]
struct Transient {
    /// The `AnimationData.dbc` id, the reference's `[handler+0x10]` substate.
    id: u16,
    node: AnimationNodeIndex,
}

/// The stored state (`go+0x27c`) every consumer reads (the animation, `usable`, the lock chain's
/// per-slot Action gate): [`GoAnim::state`] when animated, since it holds what the wire never
/// sends, else the wire field, else `0`, ACTIVE, the value vmangos omits.
pub(crate) fn go_state(anim: Option<&GoAnim>, store: &ObjectStore) -> u32 {
    anim.and_then(|a| a.state)
        .or_else(|| store.0.gameobject_state())
        .unwrap_or(GO_STATE_ACTIVE)
}

/// The inspector's readout: the armed id (the newest arm, the smallest seek), whether it is a
/// transient, and its repeat mode; `None` for a static mesh or before any arm.
pub(crate) fn armed_anim(
    go: &GoAnim,
    player: &AnimationPlayer,
    anims: &ModelAnimations,
) -> Option<(u16, bool, RepeatAnimation)> {
    let (clip, active) = anims
        .clips
        .iter()
        .filter_map(|c| player.animation(c.node).map(|a| (c, a)))
        .min_by(|a, b| a.1.seek_time().total_cmp(&b.1.seek_time()))?;
    Some((
        clip.anim_id,
        go.transient.is_some_and(|t| t.id == clip.anim_id),
        active.repeat_mode(),
    ))
}

/// Which types run the state machine: `CGGameObject::LoadBaseObject` switches on the type through
/// the 31-entry table at `0x5f76cc` (`cmp ecx,0x1e`, unsigned `ja`) into a handler at
/// `[GO+0x210]`; a `0x1c`-byte handler gets the real `0x5f3c30`/`0x5f3b50` arm, every other size
/// the base's empty bodies. Type 30 is past vmangos's enum but has a real arm; 21 GUARDPOST has no
/// case and takes the default, which logs `"BADBASEGAMEOBJECT|%d"`.
pub(crate) fn go_animates(type_id: i32) -> bool {
    matches!(
        type_id,
        0 | 1 | 2 | 3 | 6 | 8 | 9 | 10 | 12 | 16 | 17 | 18 | 19 | 23 | 24 | 26 | 27 | 28 | 29 | 30
    )
}

/// `GAMEOBJECT_TYPE_DOOR`, the one type a ghost's mover trace drops; also what an absent
/// `GAMEOBJECT_TYPE_ID` means, since vmangos omits zero-valued fields.
const GO_TYPE_DOOR: i32 = 0;

/// Types whose collider follows the state: DOOR and BUTTON. The static hull cannot swing with the
/// mesh, so an open one disables it; a chest keeps its collider in every state.
fn collision_follows_state(type_id: i32) -> bool {
    matches!(type_id, 0 | 1)
}

/// An absent wire state is `0`, ACTIVE, open: vmangos omits zero-valued fields, so a door that
/// spawns open (Zul'Gurub's Forcefield 180497) sends none.
fn collider_is_solid(wire_state: Option<u32>) -> bool {
    wire_state.unwrap_or(GO_STATE_ACTIVE) == GO_STATE_READY
}

/// A cast at a GameObject (`SMSG_SPELL_GO` with `TARGET_FLAG_GAMEOBJECT`), bridged from the net
/// layer for [`open_go_lid`].
#[derive(Message, Clone, Copy)]
pub(crate) struct GoLidOpen {
    pub(crate) go_guid: u64,
    pub(crate) spell_id: u32,
}

/// `SMSG_GAMEOBJECT_CUSTOM_ANIM`, bridged from the net layer; the fishing bobber's bite sends
/// `anim_id` 0 beside the forced state flip and the server's `SMSG_PLAY_OBJECT_SOUND`.
#[derive(Message, Clone, Copy)]
pub(crate) struct GoCustomAnim {
    pub(crate) go_guid: u64,
    pub(crate) anim_id: u32,
}

/// What a `GAMEOBJECT_STATE` plays: a held rest pose, or a one-shot motion onto the new pose.
#[derive(Clone, Copy, Debug)]
enum Play {
    /// Snap to a held pose, no swing (first sight).
    Rest(u16),
    /// Play a transition motion once, onto the destination rest pose.
    Motion(u16),
}

impl Play {
    fn anim_id(self) -> u16 {
        match self {
            Play::Rest(id) | Play::Motion(id) => id,
        }
    }
}

/// The missing-sequence remap (`0x5f3930`): what the arm requests when the model does not own the
/// id ([`ModelAnimations::owns`], the reference's `0x711960`), by `0x5f3972`'s jump table
/// `0x5f3b40`, for 146..149 only (`lea eax,[esi-0x92]; cmp eax,3; ja`). Ids: 0 Stand, 146 Close,
/// 147 Closed, 148 Open, 149 Opened, 150 Destroy, 151 Destroyed; a kept id is one op4 resolves
/// onward. The flag marks the frozen legs: a motion armed at rate 0 holds its frame 0, the pose it
/// departs from, in place of the missing rest pose.
fn remap_missing(anims: &ModelAnimations, id: u16) -> (u16, bool) {
    if anims.owns(id) {
        return (id, false);
    }
    match id {
        146 if anims.owns(148) => (146, false),
        146 => (147, false),
        147 if anims.owns(146) => (147, false),
        147 if anims.owns(148) => (148, true),
        147 => (0, false),
        148 if anims.owns(146) => (148, false),
        148 if anims.owns(150) => (150, false),
        148 => (149, false),
        149 if anims.owns(148) => (149, false),
        149 if anims.owns(146) => (146, true),
        149 => (151, false),
        other => (other, false),
    }
}

/// The held rest pose for a wire `GAMEOBJECT_STATE` (`0x5f3c30`).
fn rest_anim(state: u32) -> Option<u16> {
    match state {
        0 => Some(0x95), // ACTIVE  → Opened (held open)
        1 => Some(0x93), // READY   → Closed (held closed)
        2 => Some(0x97), // ALT     → Destroyed
        _ => None,
    }
}

/// The motion for a `prev → cur` change. The reference (`0x5f3cb0`) dispatches on the new state and
/// plays a motion from one old state only; each condition is also met by `ANIMPROGRESS < 100`,
/// which benilla does not read.
fn motion_anim(prev: u32, cur: u32) -> Option<u16> {
    match (prev, cur) {
        (1, 0) => Some(0x94), // closed → open      : Open
        (0, 1) => Some(0x92), // open   → closed    : Close
        (2, 1) => Some(0x98), // rebuilt → closed   : Rebuild
        (1, 2) => Some(0x96), // closed → destroyed : Destroy
        _ => None,
    }
}

/// First sight snaps the rest pose; a change plays its motion, else snaps the new rest pose.
fn resolve(prev: Option<u32>, cur: u32) -> Option<Play> {
    match prev {
        None => rest_anim(cur).map(Play::Rest),
        Some(p) => motion_anim(p, cur)
            .map(Play::Motion)
            .or_else(|| rest_anim(cur).map(Play::Rest)),
    }
}

/// Caller 1, the wire (`0x5f89e0`): seeds the state at first sight, then follows the
/// `GAMEOBJECT_STATE` edge, so an unrelated field change never re-closes a looted chest.
#[allow(clippy::type_complexity)]
fn sync_wire_go_state(
    mut edges: MessageReader<crate::net::FieldChanged>,
    // The seed leg and the edge leg both write `GoAnim`; one `ParamSet`, two disjoint views.
    mut gos: ParamSet<(
        Query<(&ObjectStore, &mut GoAnim), Added<GoAnim>>,
        Query<&mut GoAnim>,
    )>,
) {
    for (store, mut anim) in &mut gos.p0() {
        // Absent is `0`, ACTIVE: vmangos omits zero fields.
        anim.state = Some(store.0.gameobject_state().unwrap_or(GO_STATE_ACTIVE));
    }
    let mut live = gos.p1();
    for e in edges.read() {
        if e.kind != benilla_protocol::messages::ObjectType::GameObject
            || e.index != benilla_protocol::field::FIELD_GAMEOBJECT_STATE
        {
            continue;
        }
        if let Ok(mut anim) = live.get_mut(e.entity) {
            anim.state = Some(e.new);
        }
    }
}

/// Caller 2, the open-lock spell-go: a cast at an animated object opens it when the spell has an
/// open-lock effect (`[spell+0xf4] ∈ {OPEN_LOCK, OPEN_LOCK_ITEM}`), whoever cast it.
fn open_go_lid(
    mut opens: MessageReader<GoLidOpen>,
    spells: Option<Res<crate::ui_action::Spells>>,
    index: Res<GuidIndex>,
    mut gos: Query<&mut GoAnim>,
) {
    for GoLidOpen { go_guid, spell_id } in opens.read().copied() {
        let is_open_lock = spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id))
            .is_some_and(|d| d.open_lock.is_some());
        if !is_open_lock {
            continue;
        }
        let Some(&e) = index.0.get(&go_guid) else {
            continue;
        };
        if let Ok(mut anim) = gos.get_mut(e) {
            anim.state = Some(GO_STATE_ACTIVE);
        }
    }
}

/// Caller 4, the custom-anim opcode: queues a one-shot on the separate `0x5f8c50` channel; a guid
/// with no [`GoAnim`] drops it, and ownership is judged in [`drive_go_anim`].
fn queue_custom_anim(
    mut plays: MessageReader<GoCustomAnim>,
    index: Res<GuidIndex>,
    mut gos: Query<&mut GoAnim>,
) {
    for GoCustomAnim { go_guid, anim_id } in plays.read().copied() {
        let Some(id) = custom_anim_id(anim_id) else {
            continue; // the reference handler's reject (`0x5f8971`)
        };
        let Some(&e) = index.0.get(&go_guid) else {
            continue;
        };
        if let Ok(mut anim) = gos.get_mut(e) {
            anim.one_shot = Some(id);
        }
    }
}

/// The wire Custom index to its `AnimationData` id (`153 + n`); the reference handler (`0x5f8971`)
/// rejects `anim_id >= 4` and plays substate `8 + n`.
fn custom_anim_id(anim_id: u32) -> Option<u16> {
    (anim_id < 4).then(|| 153 + anim_id as u16)
}

/// `SMSG_GAMEOBJECT_DESPAWN_ANIM` arrived: a component inserted at packet time, so it is on the
/// entity before the same tick's `SMSG_DESTROY_OBJECT` frees it. It asserts nothing alone:
/// `SendObjectDeSpawnAnim` also fires for totems, DynamicObjects and two boss scripts' living
/// objects, so it only arms a [`GoAnim`] and defers a destroy that arrives.
#[derive(Component)]
pub(crate) struct DespawnAnimAnnounced;

/// The reference's pending-destroy mark (`[GO+0xe4]` bit `0x10`), set by the destroy `0x464920` on
/// a pinned object. Arming a non-resting substate pins (`0x5f3b27 call 0x4683e0`, refcount
/// `[obj+0xe8]`), slot 14 unpins at the window end (`0x468410`), and the deferred destroy then runs
/// (`0x46844a call 0x464920`). benilla pins only [`ANIM_DESPAWN`]; an object destroyed
/// mid-transition still goes at once (not built).
///
/// The reference keeps a pinned object addressable by guid while it plays; benilla drops the guid
/// at destroy time, so a respawn reusing it cannot refresh a condemned object. Nothing observable
/// differs: the object still draws, and the server no longer answers for that guid.
#[derive(Component)]
pub(crate) struct PendingDestroy;

/// Caller 5, the despawn-anim opcode: one arm per announcement, as `0x5f8c50(GO, 6)` is one per
/// packet. `Changed`, not `Added`: a re-inserted component is not added, and a boss script can
/// announce the same living object twice.
fn arm_despawn_anim(mut gos: Query<&mut GoAnim, Changed<DespawnAnimAnnounced>>) {
    for mut go in &mut gos {
        go.one_shot = Some(ANIM_DESPAWN);
    }
}

/// Releases a [`PendingDestroy`] object once it is not playing its despawn animation (`0x468410`
/// reaching zero), after [`drive_go_anim`] so the arming frame holds. Release is a fade, not a
/// pop: `0x464920` reaches OnDeactivate `0x6145e0`, which hands the model to the `SWModelFadeout`
/// scheduler `0x672df0`; here that is [`crate::net::tear_down`].
fn release_despawn_pin(
    mut commands: Commands,
    pinned: Query<(Entity, Option<&GoAnim>), With<PendingDestroy>>,
) {
    for (e, go) in &pinned {
        if go.is_some_and(|g| g.transient.is_some_and(|t| t.id == ANIM_DESPAWN)) {
            continue;
        }
        commands
            .entity(e)
            .try_remove::<PendingDestroy>()
            .queue_silenced(crate::net::tear_down);
    }
}

/// Caller 3, the loot release: the client drops the state to READY as it sends
/// `CMSG_LOOT_RELEASE`, with no server round-trip, on any close of the loot source.
fn close_go_lid(
    loot: Res<crate::ui_loot::LootState>,
    index: Res<GuidIndex>,
    mut gos: Query<&mut GoAnim>,
    mut last_source: Local<Option<u64>>,
) {
    let current = loot.source();
    if *last_source == current {
        return;
    }
    if let Some(closed) = *last_source {
        if let Some(&e) = index.0.get(&closed) {
            if let Ok(mut anim) = gos.get_mut(e) {
                anim.state = Some(GO_STATE_READY);
            }
        }
    }
    *last_source = current;
}

/// The substate advance. The completion callback registered at model attach (`0x5f7d43` →
/// `[M2+0x70]`) fires once at the arm's window end, span × replay count, whatever the loop bit
/// (`0x719370`; `0x719503` tests it after), and slot 14 (`0x5f4120`) dispatches on the substate:
/// a motion advances to its rest pose (2 Open → 3 Opened, 4 Close → 1 Closed, 5 Destroy →
/// 6 Destroyed, 7 Rebuild → 1 Closed); Spawn (0) and Custom0..3 (8..11) re-run the machine at the
/// current state (`0x5f4190`); a rest pose (1/3/6) re-arms itself with a fresh roll (`0x5f4167`,
/// calling `0x5f3930` past slot 34's change guard), every window for ever.
///
/// Clearing `shown` makes [`drive_go_anim`], next in the chain, re-resolve the state that frame.
/// Reads never deref-mut, so a quiet object stays out of the `Changed` stream.
fn retire_transient_anim(
    mut gos: Query<(Entity, &mut GoAnim, &AnimationPlayer)>,
    mut dispatch: MessageWriter<GoStateDispatch>,
) {
    for (entity, mut go, player) in &mut gos {
        let done = |n| {
            player
                .animation(n)
                .is_none_or(bevy::animation::ActiveAnimation::is_finished)
        };
        if let Some(t) = go.transient {
            if done(t.node) {
                go.transient = None;
                // Forget the shown pose: the state arm re-resolves it as a silent rest snap.
                go.shown = None;
                // Re-enters `0x5f3cb0` with `old == new`: a GnomeMachine's Custom0 hum ends here.
                if completion_redispatches(t.id) {
                    dispatch.write(GoStateDispatch(entity));
                }
            }
            continue;
        }
        // A rest pose re-arms the same way; `rest_window` clears first so a refused re-arm
        // settles instead of spinning.
        if let Some(node) = go.rest_window {
            if done(node) {
                go.rest_window = None;
                go.shown = None;
            }
        }
    }
}

/// The arm's trace (`WOW_MOVE_TRACE_TAGS=goa`): requested and remapped id, roll, and the sequence
/// slot landed on, which picks the material lanes (keyed by `seq_index`) and shows whether an arm
/// re-rolls every window or stalls (`0x5f39fa`'s skip).
fn trace_arm(
    entity: Entity,
    lane: &str,
    requested: u16,
    want: u16,
    frozen: bool,
    roll: u16,
    clip: &benilla_assets::AnimClip,
) {
    if !benilla_assets::trace::enabled_for("goa") {
        return;
    }
    benilla_assets::trace::line(
        "goa",
        &format!(
            "{lane} e={entity} id={requested}->{want}{} roll={roll} seq={} freq={} dur={:.3}",
            if frozen { " frozen" } else { "" },
            clip.seq_index,
            clip.frequency,
            clip.duration,
        ),
    );
}

/// Plays the sequence for a change of [`GoAnim::state`], first sight silent, as
/// [`crate::sound::gameobject`] does for the audio.
fn drive_go_anim(
    mut gos: Query<
        (
            Entity,
            &mut GoAnim,
            &mut AnimationPlayer,
            &mut AnimationTransitions,
            &ModelAnimations,
        ),
        Changed<GoAnim>,
    >,
    mut dispatch: MessageWriter<GoStateDispatch>,
    // The client's single `_rand` stream (`0x7400e5`); op4's variation roll draws from it.
    mut rng: ResMut<benilla_assets::AnimRng>,
) {
    for (entity, mut go, mut player, mut tr, anims) in &mut gos {
        // ── The state arm ──────────────────────────────────────────────────────────────────────
        if let Some(state) = go.state {
            if go.shown != Some(state) {
                let prev = go.shown;
                go.shown = Some(state);
                // Dispatch (`0x5f3cb0`) before anything below can refuse the arm; a re-resolve
                // from the retire leaves `dispatched` alone and announces nothing.
                if go.dispatched != Some(state) {
                    go.dispatched = Some(state);
                    dispatch.write(GoStateDispatch(entity));
                }
                // A fresh substate replaces the live transient (the reference keeps one,
                // `[handler+0x10]`), so an old motion's completion never fires over it.
                go.transient = None;
                go.rest_window = None;
                if let Some(play) = resolve(prev, state) {
                    // Remap an unowned id, so a lidless model holds a real pose, not bind.
                    let (want, frozen) = remap_missing(anims, play.anim_id());
                    // The already-playing skip (`0x5f39fa: cmp esi,eax; je 0x5f3b32`) holds only
                    // on the remap legs: an owned id (`0x5f396c jne`) and the collapse to Stand
                    // (`0x5f3a54 jmp 0x5f3a0b`) jump past it. So the remapped fishing bobber
                    // settles, and a lava trap that owns its id re-rolls every window.
                    let skip =
                        want != 0 && !anims.owns(play.anim_id()) && go.armed_id == Some(want);
                    if skip {
                        continue;
                    }
                    // The arm rolls a variation (`0x5f3aee: push -1`); the loader seed takes 0
                    // (`0x710189`).
                    let roll = rng.draw();
                    if let Some(clip) = anims.pick_variation(want, roll) {
                        trace_arm(entity, "state", play.anim_id(), want, frozen, roll, clip);
                        go.armed_id = Some(want);
                        // Snap a rest pose; ease a motion over its authored blend.
                        let blend = match play {
                            Play::Rest(_) => 0.0,
                            Play::Motion(_) => clip.blend_time.max(0.0),
                        };
                        let node = clip.node;
                        let active = tr.play(&mut player, node, Duration::from_secs_f32(blend));
                        if frozen {
                            // The reference arms a stand-in motion at rate 0, holding frame 0.
                            active.seek_to(0.0);
                            active.set_speed(0.0);
                            active.set_repeat(RepeatAnimation::Never);
                        } else {
                            // Explicit: `AnimationTransitions::play` keeps a node's speed, so one
                            // a frozen leg parked at rate 0 would stay stuck.
                            active.set_speed(1.0);
                            // One window, rest poses included: the period is `0x7126d8`'s, with
                            // `R = 1` from a `(0,0)` replay pair (`0x7126c3`). Which per-frame
                            // advance list a GameObject's model rides is untraced.
                            active.set_repeat(RepeatAnimation::Never);
                            match play {
                                Play::Motion(_) => {
                                    go.transient = Some(Transient { id: want, node });
                                }
                                Play::Rest(_) => go.rest_window = Some(node),
                            }
                        }
                    }
                    // Else nothing is playable after the remap, and the loader seed's arm holds.
                }
            }
        }
        // ── The one-shot Custom channel (`0x5f8c50`) ────────────────────────────────────────
        // After the state arm, so the bobber's same-frame flip and splash end splash-on-top. Slot
        // 15 needs the model to own the id (no remap); one window, so one `$GC0` splash.
        // `is_some` first: `take()` through the `Mut` would mark the component changed every frame.
        if go.one_shot.is_some() {
            let id = go.one_shot.take().expect("checked is_some");
            if anims.owns(id) {
                // Slot 15 arms through the same `0x5f3930`, so it rolls a variation too.
                let roll = rng.draw();
                if let Some(clip) = anims.pick_variation(id, roll) {
                    trace_arm(entity, "custom", id, id, false, roll, clip);
                    go.armed_id = Some(id);
                    go.rest_window = None;
                    let node = clip.node;
                    let active = tr.play(
                        &mut player,
                        node,
                        Duration::from_secs_f32(clip.blend_time.max(0.0)),
                    );
                    active.set_speed(1.0);
                    active.set_repeat(RepeatAnimation::Never);
                    go.transient = Some(Transient { id, node });
                }
            }
        }
    }
}

/// Gates a door or button's collider on its wire state, the server's passability: solid when
/// READY, disabled when open, since the static hull cannot swing. Reconciles on a state edge or on
/// the collider's arrival, which `entities::attach` inserts only once the M2 loads, frames after
/// the descriptor.
fn drive_go_collision(
    mut commands: Commands,
    gos: Query<(&ObjectStore, Has<ColliderDisabled>), With<Collider>>,
    arrived: Query<Entity, Added<Collider>>,
    mut edges: MessageReader<crate::net::FieldChanged>,
) {
    let state_edges = edges.read().filter(|e| {
        e.kind == benilla_protocol::messages::ObjectType::GameObject
            && e.index == benilla_protocol::field::FIELD_GAMEOBJECT_STATE
    });
    let mut due: bevy::ecs::entity::EntityHashSet = arrived.iter().collect();
    due.extend(state_edges.map(|e| e.entity));
    for entity in due {
        let Ok((store, disabled)) = gos.get(entity) else {
            continue;
        };
        if !collision_follows_state(store.0.gameobject_type_id()) {
            continue;
        }
        let solid = collider_is_solid(store.0.gameobject_state());
        if solid && disabled {
            commands.entity(entity).remove::<ColliderDisabled>();
        } else if !solid && !disabled {
            commands.entity(entity).insert(ColliderDisabled);
        }
    }
}

/// A ghost walks through doors: the mover's mask gains bit `0x8000` for a player body with
/// `PLAYER_FLAGS` ghost bit `0x10` (`0x631658`), and `CGGameObject_C`'s candidacy virtual
/// (`0x5f85f0`, vtable `+0x50`) tests it at `0x5f85f6` to refuse type DOOR. Nothing else about a
/// ghost's collision differs; the flag is the mover's, so a possessed creature never sets it.
fn publish_ghost_door_exclusions(
    mut exclusions: ResMut<benilla_world::collision::MoverTraceExclusions>,
    mover: Query<&ObjectStore, With<crate::net::Embodied>>,
    doors: Query<(Entity, &ObjectStore, &crate::net::NetEntity), With<Collider>>,
) {
    let ghost = mover.single().is_ok_and(|s| s.0.player_is_ghost());
    if !ghost {
        // Clear only when non-empty, so a living player's frames never trip change detection.
        if !exclusions.0.is_empty() {
            exclusions.0.clear();
        }
        return;
    }
    exclusions.0.clear();
    exclusions.0.extend(
        doors
            .iter()
            .filter(|(_, store, net)| ghost_passable(net.kind, store.0.gameobject_type_id()))
            .map(|(entity, _, _)| entity),
    );
}

/// Whether a ghost's mover trace drops this collider: a GameObject of type DOOR. The kind test
/// matters: an absent `GAMEOBJECT_TYPE_ID` reads `0`, DOOR, and a unit has no GameObject block.
fn ghost_passable(kind: benilla_protocol::EntityKind, type_id: i32) -> bool {
    kind == benilla_protocol::EntityKind::GameObject && type_id == GO_TYPE_DOOR
}

/// What the event scanner reads per object: its clock, its park state, and the frame its fired
/// keys resolve in ([`crate::creature_anim::EventFrame`]).
type ScannedGo = (
    Entity,
    &'static ModelAnimations,
    &'static AnimationPlayer,
    Has<benilla_world::rig_anim::AnimParked>,
    &'static GlobalTransform,
    Option<&'static benilla_world::rig_anim::RigPose>,
);

/// Fires the event keys an animated object's clip crossed this frame. The reference registers an
/// event callback per family-A object (`0x5f7d1f` → vtable `+0x30` → `0x5f3e20`), the [`GoAnim`]
/// population; [`crate::sound::gameobject`] routes `$GO0..5`/`$GC0..3` to
/// `GameObjectDisplayInfo.Sound[0..9]`. The bobber's Custom0 `$GC0` splash plays beside the
/// server's object-sound packet, so on vmangos it sounds twice, as in the reference.
fn fire_go_anim_events(
    gos: Query<ScannedGo, With<GoAnim>>,
    globals: Query<&GlobalTransform>,
    mut last: Local<crate::creature_anim::TrackMemory>,
    mut out: MessageWriter<AnimSoundEvent>,
) {
    for (entity, anims, player, parked, world, pose) in &gos {
        // A parked object is not scanned (`MORE_AUDIBLE` is creature-only); its memory drops so
        // a wake re-arms rather than scanning the gap as one crossing.
        if parked {
            last.remove(&entity);
            continue;
        }
        let playing = anims
            .clips
            .iter()
            .filter_map(|c| player.animation(c.node).map(|a| (c, a.seek_time())))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((clip, cur)) = playing {
            if let Some(prev) = advance_track(&mut last, entity, clip.node, cur) {
                let frame = crate::creature_anim::EventFrame {
                    world,
                    rig: pose.and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
                };
                scan_events(clip, entity, prev, cur, &frame, &mut out);
            }
        }
    }
}

/// After the net drain, the state callers write [`GoAnim::state`], the animation and collision
/// consumers act on it, and the event scanner reads what the drive settled.
pub(crate) fn plugin(app: &mut App) {
    app.add_message::<GoLidOpen>()
        .add_message::<GoCustomAnim>()
        .add_message::<GoStateDispatch>()
        .add_systems(
            Update,
            (
                (
                    sync_wire_go_state,
                    open_go_lid,
                    close_go_lid,
                    queue_custom_anim,
                    arm_despawn_anim,
                    // Clears `shown` from last frame's finished flags before the drive, so the
                    // re-arm lands one frame after the window end, as in the reference.
                    retire_transient_anim,
                ),
                (drive_go_anim, drive_go_collision),
                // The pin release reads what the drive just armed.
                release_despawn_pin,
                fire_go_anim_events,
            )
                .chain()
                .in_set(WorldStage::Present),
        );
    // In `Input`, ahead of the mover: `WorldCollision::cast_mover` reads the set inside the
    // player controller, and `Present` would hand it the previous frame's doors.
    app.add_systems(
        Update,
        publish_ghost_door_exclusions.in_set(WorldStage::Input),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_world::model_fade::DespawnFade;

    /// An absent `GAMEOBJECT_TYPE_ID` reads `0`, DOOR, on every unit, so the kind must be checked.
    #[test]
    fn a_ghosts_trace_drops_doors_and_only_doors() {
        use benilla_protocol::EntityKind;

        assert!(
            ghost_passable(EntityKind::GameObject, 0),
            "a DOOR GameObject is what the reference's `0x5f85f0` refuses to make a candidate"
        );
        assert!(
            !ghost_passable(EntityKind::GameObject, 1),
            "a BUTTON is not — it shares the state-driven collider lane, not this one; the \
             reference's predicate is an equality against 0"
        );
        assert!(
            !ghost_passable(EntityKind::Unit, 0),
            "and a UNIT reading type 0 is the absent field, not a door: units keep colliding"
        );
        assert!(
            !ghost_passable(EntityKind::Player, 0),
            "same for a player — the absent-field trap is not unit-specific"
        );
    }

    /// vmangos omits the zero field, so `None` is ACTIVE, open, for every door that spawns open.
    #[test]
    fn an_absent_wire_state_is_open_not_unknown() {
        assert!(!collider_is_solid(None), "absent == ACTIVE(0) == passable");
        assert!(!collider_is_solid(Some(GO_STATE_ACTIVE)));
        assert!(collider_is_solid(Some(GO_STATE_READY)));
        // ALTERNATIVE (destroyed) is passable too; only READY is solid.
        assert!(!collider_is_solid(Some(2)));
    }

    #[test]
    fn rest_poses_match_the_verified_table() {
        assert_eq!(rest_anim(1), Some(0x93)); // READY  → Closed
        assert_eq!(rest_anim(0), Some(0x95)); // ACTIVE → Opened (not the 0x94 Open motion)
        assert_eq!(rest_anim(2), Some(0x97)); // ALT    → Destroyed
        assert_eq!(rest_anim(7), None);
    }

    #[test]
    fn transitions_play_the_motion_then_settle() {
        assert!(matches!(resolve(Some(1), 0), Some(Play::Motion(0x94))));
        assert!(matches!(resolve(Some(0), 1), Some(Play::Motion(0x92))));
        // First sight snaps the rest pose: a door streamed in open must not swing.
        assert!(matches!(resolve(None, 0), Some(Play::Rest(0x95))));
        assert!(matches!(resolve(None, 1), Some(Play::Rest(0x93))));
        assert!(matches!(resolve(Some(2), 0), Some(Play::Rest(0x95))));
        // Destroy swings only from closed (`0x5f3cb0`'s condition is `OLD == 1`).
        assert!(matches!(resolve(Some(1), 2), Some(Play::Motion(0x96))));
        assert!(matches!(resolve(Some(0), 2), Some(Play::Rest(0x97))));
        assert!(matches!(resolve(Some(2), 1), Some(Play::Motion(0x98))));
    }

    /// The custom-anim handler (`0x5f8930`); the bobber's bite, index 0 → 153, is the second
    /// sequence `G_FishingBobber.m2` authors.
    #[test]
    fn custom_anim_maps_the_wire_index_and_rejects_past_3() {
        assert_eq!(custom_anim_id(0), Some(153)); // Custom0, the bobber splash
        assert_eq!(custom_anim_id(1), Some(154));
        assert_eq!(custom_anim_id(2), Some(155));
        assert_eq!(custom_anim_id(3), Some(156));
        assert_eq!(custom_anim_id(4), None);
        assert_eq!(custom_anim_id(u32::MAX), None);
    }

    /// The delta [`step_clock`] uses next frame: with `TimePlugin` live a stalled frame would run
    /// a whole clip out in one step, so the test writes the clock.
    #[derive(Resource, Default)]
    struct NextStep(Option<Duration>);

    fn step_clock(mut time: ResMut<Time>, mut next: ResMut<NextStep>) {
        time.advance_by(next.0.take().unwrap_or(Duration::from_millis(1)));
    }

    /// Run one frame whose delta is exactly `ms`.
    fn advance(app: &mut App, ms: u64) {
        app.world_mut().resource_mut::<NextStep>().0 = Some(Duration::from_millis(ms));
        app.update();
    }

    /// `G_Crate01`'s door family as the asset has it (`benilla-formats/tests/m2_go_crate_lid.rs`):
    /// all four `flags` bit 0 clear, empty replay range; blend zeroed so an arm is a cut.
    const CRATE_FAMILY: [(u16, f32, u16); 4] = [
        (0x94, 0.666, 0),     // 148 Open: the lid sweeps 0° → 75°
        (0x95, 0.100, 0),     // 149 Opened: holds 75°
        (0x92, 0.667, 0),     // 146 Close: sweeps 75° → 0°
        (0x93, 0.167, 29491), // 147 Closed: holds 0° (the chain head, weighted like the traps')
    ];

    /// One GameObject with a crate's animation set, real `AnimationClip`s and graph, so Bevy's
    /// `advance_animations` ticks the completions [`retire_transient_anim`] watches.
    fn crate_app(extra_ids: &[(u16, f32, u16)]) -> (App, Entity) {
        use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
        use bevy::animation::AnimationClip;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins.build().disable::<bevy::time::TimePlugin>(),
            AssetPlugin::default(),
            bevy::animation::AnimationPlugin,
        ));
        app.init_resource::<Time>();
        app.init_resource::<NextStep>();
        app.init_resource::<benilla_assets::AnimRng>();
        // Recorded in place of the audio consumer.
        app.add_message::<GoStateDispatch>();
        app.init_resource::<Dispatched>();
        app.add_systems(bevy::app::First, step_clock);
        app.add_systems(
            Update,
            (
                (arm_despawn_anim, retire_transient_anim),
                drive_go_anim,
                release_despawn_pin,
                record_dispatches,
            )
                .chain(),
        );

        let authored: Vec<(u16, f32, u16)> =
            CRATE_FAMILY.iter().chain(extra_ids).copied().collect();
        let handles: Vec<_> = authored
            .iter()
            .map(|&(_, dur, _)| {
                let mut c = AnimationClip::default();
                c.set_duration(dur);
                app.world_mut()
                    .resource_mut::<Assets<AnimationClip>>()
                    .add(c)
            })
            .collect();
        let (graph, nodes) = AnimationGraph::from_clips(handles);
        let graph = app
            .world_mut()
            .resource_mut::<Assets<AnimationGraph>>()
            .add(graph);

        let mut lookup = vec![0xffffu16; 160];
        for (slot, &(id, _, _)) in authored.iter().enumerate() {
            // `animationLookup` holds a chain's head: only the first slot claiming an id writes,
            // as the exporter bakes it.
            if lookup[id as usize] == 0xffff {
                lookup[id as usize] = slot as u16;
            }
        }
        let anims = ModelAnimations {
            graph: graph.clone(),
            clips: authored
                .iter()
                .zip(&nodes)
                .map(|(&(id, dur, frequency), &node)| benilla_assets::AnimClip {
                    anim_id: id,
                    seq_index: 0,
                    node,
                    // The crate's clips are bit-0-clear bands: the kernel loops them.
                    looping: true,
                    duration: dur,
                    move_speed: 0.0,
                    blend_time: 0.0,
                    bounds_center: Vec3::ZERO,
                    bounds_radius: 0.0,
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::ZERO,
                    events: Vec::new().into(),
                    arm_nodes: None,
                    upper_node: None,
                    frequency,
                    replay: (0, 0),
                    poses_bones: true,
                })
                .collect(),
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: lookup,
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        };
        // First sight is a closed chest, as it streams in.
        let go = app
            .world_mut()
            .spawn((
                anims,
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimationGraphHandle(graph),
                GoAnim {
                    state: Some(GO_STATE_READY),
                    ..Default::default()
                },
            ))
            .id();
        (app, go)
    }

    /// Every [`GoStateDispatch`] the run produced, in order, in place of the audio consumer.
    #[derive(Resource, Default)]
    struct Dispatched(Vec<Entity>);

    fn record_dispatches(mut msgs: MessageReader<GoStateDispatch>, mut out: ResMut<Dispatched>) {
        out.0.extend(msgs.read().map(|d| d.0));
    }

    /// What the object is actually playing: the armed `AnimationData` id and its repeat mode.
    fn armed(app: &App, go: Entity) -> Option<(u16, RepeatAnimation)> {
        let e = app.world().entity(go);
        let node = e.get::<AnimationTransitions>()?.get_main_animation()?;
        let id = e
            .get::<ModelAnimations>()?
            .clips
            .iter()
            .find(|c| c.node == node)?
            .anim_id;
        Some((
            id,
            e.get::<AnimationPlayer>()?.animation(node)?.repeat_mode(),
        ))
    }

    /// The door family is `flags` bit 0 clear, so arming by the loop bit would swing Close for
    /// ever; the reference ends it at the window (slot 14 `0x5f4120`: 4 Close → 1 Closed).
    #[test]
    fn the_looted_crate_settles_shut_instead_of_flapping_for_ever() {
        let (mut app, go) = crate_app(&[]);
        let state = |app: &mut App, s: u32| {
            app.world_mut()
                .entity_mut(go)
                .get_mut::<GoAnim>()
                .unwrap()
                .state = Some(s);
        };

        // Streamed in closed: the rest pose, snapped, one window that `0x5f4167` re-arms at its
        // completion, so what must hold across windows is the armed id.
        app.update();
        assert_eq!(armed(&app, go), Some((0x93, RepeatAnimation::Never)));

        // The open-lock cast lands: the lid swings open, one window.
        state(&mut app, GO_STATE_ACTIVE);
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((0x94, RepeatAnimation::Never)),
            "the Open motion is a transient substate — one window, whatever its loop bit says"
        );

        // At its window end the machine advances 2 Open → 3 Opened.
        advance(&mut app, 700);
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((0x95, RepeatAnimation::Never)),
            "the completion advance settles the swing onto the Opened rest pose"
        );

        // The loot window closes (`CMSG_LOOT_RELEASE`): the lid swings shut, one window.
        state(&mut app, GO_STATE_READY);
        app.update();
        assert_eq!(armed(&app, go), Some((0x92, RepeatAnimation::Never)));

        // 4 Close → 1 Closed, and it stays there through the Closed pose's own re-arms.
        advance(&mut app, 700);
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((0x93, RepeatAnimation::Never)),
            "the lid must settle on Closed"
        );
        for _ in 0..20 {
            advance(&mut app, 100);
            assert_eq!(
                armed(&app, go),
                Some((0x93, RepeatAnimation::Never)),
                "two seconds on — three Close windows — the crate is still shut"
            );
        }
    }

    /// The arm rolls a variation (`0x5f3aee: push -1`), not the id's head; the loader seed takes
    /// 0 (`0x710189`). Onyxia's lava traps in small: a Closed chain weighted 29491 : 3276, rolled
    /// over many objects, whose second-take count must be neither 0 nor all, and near a tenth.
    #[test]
    fn the_arm_rolls_a_weighted_variation_not_the_head() {
        const N: usize = 400;
        let (mut app, go) = crate_app(&[(0x93, 0.866, 3276)]);
        let (anims, player, tr, graph) = {
            let e = app.world().entity(go);
            (
                e.get::<ModelAnimations>().unwrap().clone(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                e.get::<bevy::animation::graph::AnimationGraphHandle>()
                    .unwrap()
                    .clone(),
            )
        };
        let second = anims
            .clips
            .iter()
            .filter(|c| c.anim_id == 0x93)
            .nth(1)
            .expect("the chain's second take")
            .node;
        let mut gos = vec![go];
        for _ in 1..N {
            gos.push(
                app.world_mut()
                    .spawn((
                        anims.clone(),
                        player.clone(),
                        tr.clone(),
                        graph.clone(),
                        GoAnim {
                            state: Some(GO_STATE_READY),
                            ..Default::default()
                        },
                    ))
                    .id(),
            );
        }
        app.update();
        let rolled = gos
            .iter()
            .filter(|&&e| {
                app.world()
                    .entity(e)
                    .get::<AnimationPlayer>()
                    .unwrap()
                    .animation(second)
                    .is_some()
            })
            .count();
        assert!(
            rolled > 0,
            "not one of {N} arms reached the second take — the arm is taking the head, which is \
             the whole Onyxia lava-trap bug"
        );
        assert!(
            (N / 40..N / 4).contains(&rolled),
            "{rolled}/{N} on a 3276/32768 chain — the weighted walk is not weighting"
        );
    }

    /// `0x5f4167` re-arms a rest pose every window, whatever its loop bit, with a fresh roll.
    #[test]
    fn a_rest_pose_re_arms_every_window_and_re_rolls_its_variation() {
        let (mut app, go) = crate_app(&[(0x93, 0.866, 3276)]);
        let nodes: Vec<_> = {
            let e = app.world().entity(go);
            e.get::<ModelAnimations>()
                .unwrap()
                .clips
                .iter()
                .filter(|c| c.anim_id == 0x93)
                .map(|c| c.node)
                .collect()
        };
        assert_eq!(nodes.len(), 2, "the chain is two takes");
        let mut seen = [0u32; 2];
        app.update();
        for _ in 0..300 {
            let player = app.world().entity(go).get::<AnimationPlayer>().unwrap();
            for (i, n) in nodes.iter().enumerate() {
                if player.animation(*n).is_some() {
                    seen[i] += 1;
                }
            }
            // Longer than either take's band, so whichever is armed completes and re-arms.
            advance(&mut app, 900);
        }
        assert!(
            seen[0] > 0 && seen[1] > 0,
            "over 300 windows the pose showed takes {seen:?} — a rest pose that never re-armed \
             would show exactly one, and Onyxia's floor would spurt once and go quiet"
        );
        // The pose itself never wanders off Closed: re-arming is not the substate advance.
        assert!(matches!(armed(&app, go), Some((0x93, _))));
    }

    /// A rest pose's re-arm is no dispatch (a brazier's `$GO2` `TorchLoop` hums on); a real
    /// `SetGoState` and a Custom completion (`0x5f4190`, `old == new`) are.
    #[test]
    fn only_a_state_change_and_a_custom_completion_re_enter_the_dispatch() {
        let (mut app, go) = crate_app(&[(153, 0.667, 0)]); // Custom0, which the crate authors
        app.update();
        let seen = |app: &App| app.world().resource::<Dispatched>().0.len();
        assert_eq!(seen(&app), 1, "the spawn's own SetGoState is a dispatch");

        // Fifty rest windows: the pose re-arms in each, and none may announce a dispatch.
        for _ in 0..50 {
            advance(&mut app, 200);
        }
        assert!(matches!(armed(&app, go), Some((0x93, _))), "still Closed");
        assert_eq!(seen(&app), 1, "a rest pose's own re-arm is not a dispatch");

        // A real SetGoState: the lid opens.
        app.world_mut()
            .entity_mut(go)
            .get_mut::<GoAnim>()
            .unwrap()
            .state = Some(GO_STATE_ACTIVE);
        app.update();
        assert_eq!(seen(&app), 2);
        // The Open motion settling onto Opened is slot 14's `0x5f413d` row, not the dispatch.
        // Two ticks: one runs the clip past its window, the next sees the finished flag.
        advance(&mut app, 700);
        app.update();
        assert_eq!(
            seen(&app),
            2,
            "a transition motion advancing to its rest pose does not re-enter the dispatch"
        );

        // A Custom0 block, whose completion does.
        app.world_mut()
            .entity_mut(go)
            .get_mut::<GoAnim>()
            .unwrap()
            .one_shot = Some(153);
        app.update();
        assert_eq!(seen(&app), 2, "arming a Custom is not itself a dispatch");
        advance(&mut app, 700);
        app.update();
        assert_eq!(
            seen(&app),
            3,
            "its completion re-runs the machine through `0x5f3cb0`"
        );
        assert!(
            app.world()
                .resource::<Dispatched>()
                .0
                .iter()
                .all(|&e| e == go),
            "every dispatch names the object it ran on"
        );
    }

    /// The Custom channel shares the one transient slot (`[handler+0x10]`) with the motions.
    #[test]
    fn a_custom_block_still_runs_one_window_and_hands_back_to_the_state() {
        let (mut app, go) = crate_app(&[(153, 0.667, 0)]); // Custom0, which the crate authors
        app.update();
        assert_eq!(armed(&app, go), Some((0x93, RepeatAnimation::Never)));

        app.world_mut()
            .entity_mut(go)
            .get_mut::<GoAnim>()
            .unwrap()
            .one_shot = Some(153);
        app.update();
        assert_eq!(armed(&app, go), Some((153, RepeatAnimation::Never)));

        advance(&mut app, 700);
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((0x93, RepeatAnimation::Never)),
            "one Custom window, then the state pose — never a churning loop"
        );
    }

    /// UBRS's Rookery Eggs: vmangos sends `SMSG_GAMEOBJECT_DESPAWN_ANIM` then `SMSG_DESTROY_OBJECT`
    /// in one tick, and the egg plays 157 Despawn (2.667 s on `G_DragonEggFreeze`) before it goes.
    #[test]
    fn an_announced_despawn_plays_its_window_before_the_object_pops() {
        let (mut app, go) = crate_app(&[(157, 2.667, 0)]);
        app.update();
        assert_eq!(armed(&app, go), Some((0x93, RepeatAnimation::Never)));

        // The wire pair, in the order vmangos sends it and Commands apply it.
        app.world_mut().entity_mut(go).insert((
            crate::net::Guid(0xF110_0000_0000_0E66),
            DespawnAnimAnnounced,
            PendingDestroy,
        ));
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((157, RepeatAnimation::Never)),
            "the announcement arms 157 Despawn for ONE window"
        );
        assert!(
            app.world().get::<crate::net::Guid>(go).is_some(),
            "the pin holds the OBJECT alive across its own destroy, not just its model"
        );

        advance(&mut app, 2700);
        app.update();
        assert!(
            app.world().get::<DespawnFade>(go).is_some(),
            "the window ended — the pin drops and the deferred destroy hands the model to the fade"
        );
        assert!(
            app.world().get::<crate::net::Guid>(go).is_none(),
            "…and the deferred destroy is the teardown: the object ends as the fade begins"
        );
        assert!(
            app.world().get::<PendingDestroy>(go).is_none(),
            "and the pin comes off with it, so the release stops re-arming the fade"
        );
    }

    /// A model without 157, or an object with no animation machine, never pins and goes straight
    /// to the fade: a looted chest such as the static `DeadmineCargoBoxes.m2`.
    #[test]
    fn an_unownable_despawn_anim_goes_straight_to_the_fade() {
        let (mut app, go) = crate_app(&[]); // the crate family only, no 157
        app.update();

        app.world_mut()
            .entity_mut(go)
            .insert((DespawnAnimAnnounced, PendingDestroy));
        app.update();
        assert!(
            app.world().get::<DespawnFade>(go).is_some(),
            "nothing to play ⇒ the teardown fade, same frame — never a pop"
        );
    }

    /// `SendObjectDeSpawnAnim` is on `WorldObject`, and two vmangos boss scripts send it for
    /// living objects (Sapphiron for himself); only an arriving destroy marks [`PendingDestroy`].
    #[test]
    fn an_announcement_without_a_destroy_never_despawns_anything() {
        let (mut app, go) = crate_app(&[(157, 2.667, 0)]);
        app.update();

        app.world_mut().entity_mut(go).insert(DespawnAnimAnnounced);
        app.update();
        assert_eq!(armed(&app, go), Some((157, RepeatAnimation::Never)));

        advance(&mut app, 2700);
        app.update();
        assert!(app.world().get_entity(go).is_ok(), "no destroy, no despawn");
        assert_eq!(
            armed(&app, go),
            Some((0x93, RepeatAnimation::Never)),
            "and the window hands back to the state pose, like any other one-shot"
        );
    }

    /// A `ModelAnimations` that owns exactly `ids`; only the lookup table matters to the remap.
    fn owning(ids: &[u16]) -> ModelAnimations {
        let hi = ids.iter().copied().max().unwrap_or(0) as usize;
        let mut lookup = vec![0xffffu16; hi + 1];
        for (slot, &id) in ids.iter().enumerate() {
            lookup[id as usize] = slot as u16;
        }
        ModelAnimations {
            graph: Handle::default(),
            clips: Vec::new(),
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: lookup,
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }

    #[test]
    fn an_owned_id_is_never_remapped() {
        let full = owning(&[146, 147, 148, 149]);
        for id in [146, 147, 148, 149] {
            assert_eq!(remap_missing(&full, id), (id, false));
        }
    }

    #[test]
    fn a_missing_rest_pose_freezes_the_neighbouring_motion() {
        // The Ahn'Qiraj gate roots own Stand and Open only: Open frozen at frame 0 is closed.
        let roots = owning(&[0, 148]);
        assert_eq!(remap_missing(&roots, 147), (148, true));
        // The mirror leg: only Close, frozen at frame 0, which is open.
        let no_open = owning(&[146]);
        assert_eq!(remap_missing(&no_open, 149), (146, true));
    }

    #[test]
    fn the_remap_falls_through_the_door_family_in_the_verified_order() {
        assert_eq!(remap_missing(&owning(&[148]), 146), (146, false));
        assert_eq!(remap_missing(&owning(&[147]), 146), (147, false));
        assert_eq!(remap_missing(&owning(&[146]), 147), (147, false));
        assert_eq!(remap_missing(&owning(&[0]), 147), (0, false));
        assert_eq!(remap_missing(&owning(&[146]), 148), (148, false));
        assert_eq!(remap_missing(&owning(&[150]), 148), (150, false));
        assert_eq!(remap_missing(&owning(&[0]), 148), (149, false));
        assert_eq!(remap_missing(&owning(&[0]), 149), (151, false));
    }

    /// Slot 14's table (`0x5f41e4`): rows 0 and 8..11 land at `0x5f4190`, re-running the machine
    /// through `0x5f3cb0`; motion rows advance and rest rows re-arm without it.
    #[test]
    fn only_spawn_and_the_custom_block_re_enter_the_state_dispatch() {
        for id in [145, 153, 154, 155, 156] {
            assert!(completion_redispatches(id), "substate id {id}");
        }
        // Motions, rest poses, and Despawn (157), whose substate 12 is past slot 14's `cmp 0xb`.
        for id in [146, 147, 148, 149, 150, 151, 152, 157] {
            assert!(!completion_redispatches(id), "substate id {id}");
        }
    }

    #[test]
    fn ids_outside_the_door_group_get_no_remap() {
        // `lea eax,[esi-0x92]; cmp eax,3; ja`: only 146..149 index the jump table.
        let none = owning(&[0]);
        for id in [145, 150, 151, 152, 157] {
            assert_eq!(remap_missing(&none, id), (id, false));
        }
    }

    #[test]
    fn ownership_reads_the_lookup_table_not_the_clip_list() {
        // `nAnimationLookup` stops just past a model's highest id; `clips` is empty, so a clip
        // scan would answer "owns nothing".
        let book = owning(&[146, 147, 148, 149]);
        assert!(book.owns(147) && !book.owns(0) && !book.owns(150));
    }

    #[test]
    fn chest_animates_but_keeps_its_collider() {
        assert!(go_animates(3));
        assert!(!collision_follows_state(3));
        assert!(go_animates(0) && collision_follows_state(0));
        assert!(go_animates(1) && collision_follows_state(1));
    }

    #[test]
    fn the_type_gate_is_the_verified_census() {
        // The 20 family-A types (0x1c-byte handler, 36-slot vtable with the real arm).
        for t in [
            0, 1, 2, 3, 6, 8, 9, 10, 12, 16, 17, 18, 19, 23, 24, 26, 27, 28, 29, 30,
        ] {
            assert!(go_animates(t), "type {t} runs the machine");
        }
        // The 11 family-B types, whose vtable slots are the base's empty bodies.
        for t in [4, 5, 7, 11, 13, 14, 15, 20, 21, 22, 25] {
            assert!(!go_animates(t), "type {t} keeps the loader seed");
        }
        // Past the table: the unsigned `ja` default, no handler.
        assert!(!go_animates(31) && !go_animates(99));
        // TEXT (every book) and GOOBER.
        assert!(go_animates(9) && go_animates(10));
        // Type 0 is also what an absent type id reads as, so the default animates.
        assert!(go_animates(0));
    }
}
