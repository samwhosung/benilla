//! The network↔ECS bridge: a typed-event channel into real ECS entities (decision 0006).
//!
//! A background **read** thread owns the blocking [`WorldSession`] (logon → world handshake → enter
//! world) and streams the world as a flat list of [`SessionEvent`]s over a channel; a sibling
//! **write** thread owns the writer and drains [`ClientCommand`]s. One ECS system,
//! [`apply_net_updates`], drains the event channel each frame and mutates **real Bevy entities** —
//! spawn on create, transform-write on move, despawn on remove — keyed by a [`Guid`] component with a
//! [`GuidIndex`] for O(1) lookup. There is no shadow world: a unit is one entity, full stop.
//!
//! Movement paths become a [`Spline`] component sampled by [`sample_splines`]; the server clock
//! becomes the [`ServerTime`] resource; one-shot directives (teleport, worldport) become Bevy
//! [`Message`]s the player/terrain consume. Outbound movement + chat go through [`NetCommands`].
//!
//! The bridge runs in [`WorldStage::Net`], before `Input`, so a server teleport snaps → streams →
//! covers in one frame. Coordinates cross from raw WoW into Bevy space here (`wow_to_bevy`).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use benilla_protocol::{
    messages::WhoRequest, EntityKind, JumpInfo, MoveMode, MoveSpeeds, ObjectFields, SessionEvent,
    SpeedKind, TransportPose,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use benilla_world::schedule::WorldStage;

mod apply;
pub(crate) mod handlers;
pub(crate) mod io;
mod motion;
mod objects;
pub(crate) use objects::tear_down;
mod session;
mod world;

pub(crate) use apply::apply_net_updates;
use apply::tag_self_player;
pub(crate) use handlers::NetHandlerApp;

// The per-frame motion model lives in [`motion`]: `RemoteMotion`/`Spline` are re-exported for the
// crate (the animation selector reads them); the integration systems + pose helpers stay `pub(super)`
// and are pulled in here for the plugin + the event bridge.
pub(crate) use io::LoginRequest;
use motion::{
    drain_pending_moves, extrapolate_remote_units, ground_clamp_creatures, mark_swimming_creatures,
    sample_splines,
};
// The client-local facing turn. `pub(crate)` because the shuffle latch it produces is read in
// `creature_anim`, and the cross-seam test that pins the pair runs both systems in one app.
pub(crate) use motion::drive_display_facing;
pub(crate) use motion::{
    ground_derived, grounded_y, jump_seed, CreatureSwimming, FacingStep, RemoteMotion, Spline,
    SplineStopped, UnitMoveModes,
};
// `GroundClamped`'s only consumer outside `net::motion::spline` is the ground-census probe, and a
// probe is an instrument — a build with the instruments compiled out contains nothing that names
// this re-export, and warned about it every time (decision 1451). The `allow` is "unused in *that*
// build", not dead code; a `cfg` here is not the alternative, because seam knowledge has exactly
// three addresses and this file is not one of them (1179, and its test says so out loud).
#[allow(unused_imports)]
pub(crate) use motion::GroundClamped;

/// The net subsystem: spawns the background IO threads and drives the per-frame event drain.
pub(crate) struct NetPlugin {
    /// Connect to the server. `false` in capture mode ([`crate::capture`]): the channel resources
    /// still exist (so the player/chat command sends stay harmless no-ops), but no IO thread runs, so
    /// captures are deterministic regardless of whether a server happens to be up.
    pub(crate) connect: bool,
}

/// Marker for "this process has **no** IO thread" — inserted only when [`NetPlugin::connect`] is
/// false. Read by `crate::preflight`'s startup notice, which says so out loud, so a run that cannot
/// exercise the wire path is never mistaken for one that did (decision 0728).
///
/// A resource of its own rather than a bool on [`NetStatus`], because it answers a different
/// question: `connected` is *runtime* state that a refused or dropped connection also clears, while
/// this one says the attempt was never made and no packet can ever arrive.
#[derive(Resource)]
pub(crate) struct NetOffline;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        // The one release-on-enter for the ask-once caches this plugin owns (decision 2288;
        // the story of the latch that emptied the mail send tab's stationery for a session is
        // on `query_cache::register`).
        crate::query_cache::register::<crate::names::NameCache>(app);
        crate::query_cache::register::<crate::go_templates::GameObjectTemplates>(app);
        crate::query_cache::register::<crate::items::Items>(app);
        let handles = io::spawn_net(io::NetConfig::from_env(), self.connect);
        if !self.connect {
            app.insert_resource(NetOffline);
        }
        // In the wire-drain stage, because that is what it is the product of. The lighting
        // resolve is ordered after that stage so it always reads THIS frame's clock rather than
        // whichever way the executor happened to break the tie.
        app.add_systems(
            Update,
            publish_world_time.in_set(benilla_world::schedule::WorldStage::Net),
        );
        // The bridge's own handlers (decision 2325): the world feed it forwards, the name cache it
        // initialises.
        world::register(app);
        session::register(app);
        objects::register(app);
        crate::names::net::register(app);
        app.insert_resource(NetEvents(handles.events))
            .insert_resource(NetCommands(handles.commands))
            .insert_resource(CharPick(handles.pick))
            .insert_resource(RealmChoice(handles.realm))
            .insert_resource(LoginSubmit(handles.login))
            .insert_resource(LoginAbandon(handles.login_abandon))
            .insert_resource(PingShared(handles.ping))
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<AddonInfoReply>()
            .init_resource::<PendingTransfer>()
            .init_resource::<NetStatus>()
            .init_resource::<DroppedOpcodes>()
            .init_resource::<ServerTime>()
            .init_resource::<ServerWallClock>()
            .init_resource::<Reputations>()
            .init_resource::<HomeBind>()
            .init_resource::<PlayedTimeAnswer>()
            .init_resource::<Proficiencies>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::go_templates::GameObjectTemplates>()
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::world_state::WorldStates>()
            .add_message::<TeleportMessage>()
            .add_message::<FieldChanged>()
            .add_message::<SelfMoveMessage>()
            .add_message::<SpeedChangeMessage>()
            .add_message::<ClientControlMessage>()
            .add_message::<MoveModeMessage>()
            .add_message::<KnockBackMessage>()
            .add_message::<ServerSoundMessage>()
            .add_message::<EmoteMessage>()
            .add_message::<AiReactionMessage>()
            .add_message::<PetTalkMessage>()
            .add_message::<PetDismissSoundMessage>()
            .add_message::<WorldportMessage>()
            .add_message::<RealmListMessage>()
            .add_message::<CharListMessage>()
            .add_message::<CharActionResultMessage>()
            .add_message::<CharacterLoginFailedMessage>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<CinematicTriggeredMessage>()
            .add_message::<ServerSaidMessage>()
            .add_message::<LoggedOutMessage>()
            .add_message::<LoginStageMessage>()
            .add_message::<LoginQueuedMessage>()
            .add_message::<LoginFailedMessage>()
            .add_message::<DisconnectedMessage>()
            .add_systems(
                Update,
                (
                    apply_net_updates,
                    tag_self_player,
                    sample_splines,
                    // Derive each creature's swim state from the water over its feet (the wire never
                    // carries it for creatures) — before the clamp, so a swimmer is exempt same-frame.
                    mark_swimming_creatures,
                    // After `sample_splines` writes the raw spline Z: re-ground walking units onto our
                    // terrain (the client discards a ground spline's Z — decision 0059); a swimming
                    // creature keeps its wire Z (its path runs through the water volume).
                    ground_clamp_creatures,
                    // Fire due scheduled relays (decision 0601) before the extrapolator advances
                    // the freshly-applied state and reconcile-lerps toward the next queued head.
                    drain_pending_moves,
                    extrapolate_remote_units,
                    // The client-local facing turn runs here — after the movers (spline / remote)
                    // have set their poses, so a turn reads the goal's fresh position this frame.
                    // A stationary unit squares up on its target, and the NPC whose interaction
                    // window is open turns to face us (decision 1467).
                    drive_display_facing,
                )
                    .chain()
                    .in_set(WorldStage::Net),
                // `InteractNpc`'s writer (`ui_session::feed_interact_npc`) seats itself INSIDE
                // this chain — after `apply_net_updates`, before `drive_display_facing` — and
                // declares both edges itself. It used to sit ahead of the whole chain, which
                // read the window state one frame late; harmless for an ~8-frame facing ease,
                // and exactly the frame the `"npc"` unit token was stale on when a window's
                // own `MERCHANT_SHOW` handler read it (decision 2022).
            )
            // Not part of the movement chain above: one send on the world-enter message.
            .add_systems(Update, send_query_time.in_set(WorldStage::Net))
            .add_systems(Update, population_pulse.in_set(WorldStage::Net));
    }
}

/// The population pulse (`WOW_MOVE_TRACE_TAGS=pop`): once a second, one trace line with the count
/// of net entities resident in the object index. The "did the world arrive" instrument, born as
/// decision 1340's retest: a teleport's visibility refresh (the release-time position report)
/// reads as the count climbing within a couple of seconds of the `sett` release, while the ~20 s
/// lazy-relocation pop-in the report guards against reads as the count sitting still until the
/// server's own timer fires. Free when the tag is off.
fn population_pulse(index: Res<GuidIndex>, time: Res<Time>, mut last: Local<f32>) {
    if !benilla_assets::trace::enabled_for("pop") {
        return;
    }
    let now = time.elapsed_secs();
    if now - *last < 1.0 {
        return;
    }
    *last = now;
    benilla_assets::trace::line("pop", &format!("net entities={}", index.0.len()));
}

// ── ECS state: components ────────────────────────────────────────────────────────────────────────

/// The server guid of a streamed entity. The one stable identity for a unit/player/GameObject.
#[derive(Component, Clone, Copy)]
pub(crate) struct Guid(pub(crate) u64);

/// A streamed entity's network-authoritative identity: its coarse kind + model display id. The
/// renderer ([`crate::entities`]) attaches a visual from these; the entity's [`Transform`] is its pose.
#[derive(Component)]
pub(crate) struct NetEntity {
    pub(crate) kind: EntityKind,
    pub(crate) display_id: Option<u32>,
    /// `OBJECT_FIELD_SCALE_X` — the unit/object's *complete* render scale: the server already folds
    /// the DBC scale (`CreatureModelData.modelScale × CreatureDisplayInfo.scale`, or a per-spawn
    /// override) into it, so the renderer ([`crate::entities`]) bakes this onto the transform *alone*,
    /// never times its own DBC scale (that would double-apply). `1.0` is the default (no rescale).
    /// A *live* SCALE_X change (a values delta on an existing unit) updates this and eases the
    /// render scale over 2 s with a cosine smoothstep — the reference's own transition
    /// (byte-verified, `0x614bbf`; the code once misread as a selection fade-in) — through
    /// `entities::live_display` (decision 0695), which likewise applies a live
    /// `UNIT_FIELD_DISPLAYID` change (druid forms, GM morphs — the old F04 deferral).
    pub(crate) scale: f32,
}

/// A unit's movement speeds (yd/s), decoded from its `LIVING` movement block. The creature animation
/// selector ([`crate::creature_anim`]) keys its walk-vs-run boundary on `walk` (run above 2× walk —
/// `0x5fd224`); [`extrapolate_remote_units`] integrates a remote mover between packets at `run` /
/// `run_back` / `swim` and turns it in place at `turn_rate`. Present only on units (GameObjects don't
/// move under their own power).
#[derive(Component, Clone, Copy)]
pub(crate) struct UnitSpeeds(pub(crate) MoveSpeeds);

/// **`CMovement::GetCurrentSpeed 0x7c4c90`** — the one place a mover's move-flags become a
/// yards/second, for our own body and for every remote one. It is a *cascade*, and its ORDER is
/// the whole of it:
///
/// 1. `0x7c4c99 test dl,0xf` — **no direction bit at all → 0.0** (`[0x7ffd74]`), never a stale
///    speed. A standing mover is standing however its speed fields read.
/// 2. `0x7c4cd9 test edx,0x200000` — **SWIMMING first**, and its arm returns from inside: there
///    is no "swim-walk", so the walk bit below cannot reach a swimmer. Backward → the `min`.
/// 3. `0x7c4d11 test dh,0x1` — **WALK mode (`0x100`) next**, and this is the ordering that is easy
///    to get backwards: the walk arm `0x7c4d4d fld [ecx+0x88]; fld [ecx+0x8c]; fcompp` is taken
///    **before** the run arm's backward `min`, so a **walking backpedal is walk speed, not
///    run-back speed**. The arm is `min(MOVE_WALK, MOVE_RUN)` — not a plain select — so a
///    walk-speed buff above run speed is clamped exactly as the reference clamps it.
/// 4. `0x7c4d1d` — the run arm: backward → `min(MOVE_RUN_BACK, MOVE_RUN)`, else `MOVE_RUN`.
///
/// Both backward arms are a **min**, not a select: x87 `fcompp` + `test ah,0x41` + `jp` returns
/// the smaller (ties/NaN → the forward field). At vanilla values the min is always the back field,
/// so the clamp is only visible under a server that force-sets a back speed above its forward one.
///
/// **There is no zero-`walk` fallback here, and that is a settled question rather than an
/// oversight** (decision 1759). The ctor does zero all six speed fields (`0x7c48e8`–`0x7c4906`),
/// and 1752 guarded against a mover reaching this with `walk == 0` by falling back to the run arm.
/// The local player's create does **not** skip the speed-block apply, so the zero is not live
/// after a world port: the guard `0x5ff070` has two callers, and the post-create `0x5fad50` passes
/// 0, so the block is applied for our own unit exactly as for a remote one (`0x5fad98 xor edi,edi`,
/// unwritten to the push at `0x5fb131`). The self-skip belongs to the *redundant*-create path
/// `0x466350`, reached only when the object-manager lookup HIT. The ctor's zero is a transient
/// inside one call and is never observable, so this is the plain minimum the bytes are.
pub(crate) fn current_speed(s: &MoveSpeeds, flags: u32) -> f32 {
    use crate::creature_anim::move_flags as f;
    if flags & f::ANY_MOVE == 0 {
        return 0.0;
    }
    if flags & f::SWIMMING != 0 {
        return if flags & f::BACKWARD != 0 {
            s.swim_back.min(s.swim)
        } else {
            s.swim
        };
    }
    if flags & f::WALK_MODE != 0 {
        return s.walk.min(s.run);
    }
    if flags & f::BACKWARD != 0 {
        return s.run_back.min(s.run);
    }
    s.run
}

/// A streamed object's descriptor field set (`UpdateFields`) — the ECS's mirror of the object's
/// server-side values array (decision 0061). Seeded from the create mask and [`ObjectFields::merge`]d
/// with each `Values` delta, so it accumulates the object's full descriptor state over its life.
/// Consumers read named accessors on the inner [`ObjectFields`]: `unit_health`/`unit_max_health` for the
/// Death selector + the inspector, `unit_race`/`unit_gender` + the `player_*` customization for the
/// character compositor. Speeds and pose are **not** here — they're movement-block data ([`UnitSpeeds`]
/// + the `Transform`), not descriptor fields.
#[derive(Component, Clone, Default)]
pub(crate) struct ObjectStore(pub(crate) ObjectFields);

/// **One descriptor dword moved on a streamed object** — the reference's `CMirrorHandler` edge
/// (decision 2297). The real client keeps a shadow copy of every object's field array, and the
/// values-apply notifier (`0x465330`) memcmps live against shadow over each registered span and
/// calls the watcher with the OLD value (`0x465570` hands the mirror pointer as `param3`); a
/// first CREATE passes a notify-suppress flag and fires nothing, an in-place re-create of a live
/// guid goes down the values path and fires like any delta.
///
/// benilla emits the same edge once, from the drain's merge, which is the one place that holds
/// both sides: every consumer that used to poll `Changed<ObjectStore>` and re-derive "did THIS
/// field move" against its own `Local<EntityHashMap>` shadow reads these instead, with no
/// first-sight rule and no per-frame `retain` of its own (the create is never merged, so the
/// suppression is structural). Unwatched fields cost one `Vec` push each; the consumers filter
/// by `(kind, index)` through [`benilla_protocol::field`]'s named constants — the same constants
/// the store's accessors read through, so a watch and its accessor cannot disagree.
///
/// `kind` is the store's own class off its create block: a raw index means different things per
/// class (`36` is a unit's `BYTES_0` and a corpse's `DYNAMIC_FLAGS`), and a unit-field watcher
/// must accept both `Unit` and `Player` ([`Self::unit_field`]).
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FieldChanged {
    pub entity: Entity,
    pub guid: u64,
    pub kind: benilla_protocol::messages::ObjectType,
    pub index: u16,
    pub old: u32,
    pub new: u32,
}

impl FieldChanged {
    /// A unit-block edge — units and players share the UNIT block, so a watch registered for
    /// the reference's TYPEID 3 sees both here.
    pub fn is_unit(&self) -> bool {
        use benilla_protocol::messages::ObjectType;
        matches!(self.kind, ObjectType::Unit | ObjectType::Player)
    }
    /// The edge on one named unit field.
    pub fn unit_field(&self, index: u16) -> bool {
        self.is_unit() && self.index == index
    }
    /// The edge's slot inside a unit-block array starting at `base` with `len` dwords — the aura
    /// arrays' shape (`UNIT_FIELD_AURA[48]`, `AURAAPPLICATIONS[12]`).
    pub fn unit_array_slot(&self, base: u16, len: u16) -> Option<u16> {
        (self.is_unit() && (base..base + len).contains(&self.index)).then(|| self.index - base)
    }
}

/// The field edges one feed saw this run, keyed the way the unit feeds ask — "did THIS guid's
/// field move" per token — so a per-token event rides the reference's own trigger (its notifier
/// fans a moved field out to every token naming the unit, `0x515e50`) instead of a snapshot diff
/// guarded by `prev.is_some()` (decision 2297). Collected once per run, off the reader.
#[derive(Default)]
pub(crate) struct FieldEdges(HashSet<(u64, u16)>);

impl FieldEdges {
    pub fn collect(reader: &mut MessageReader<FieldChanged>) -> Self {
        Self(reader.read().map(|e| (e.guid, e.index)).collect())
    }
    /// Did `guid`'s dword at `index` move this run?
    pub fn moved(&self, guid: u64, index: u16) -> bool {
        self.0.contains(&(guid, index))
    }
    #[cfg(test)]
    pub fn of(edges: &[(u64, u16)]) -> Self {
        Self(edges.iter().copied().collect())
    }
}

/// Fold a descriptor update onto a live store and report its field edges — the drain's merge
/// body, shared with the test harness so a fixture drives a watcher through exactly the path
/// the wire does. A store that is not there yet is seeded silently (the first create).
pub(crate) fn merge_store_fields(
    store: &mut ObjectFields,
    delta: ObjectFields,
    entity: Entity,
    guid: u64,
    mut emit: impl FnMut(FieldChanged),
) {
    let Some(kind) = store.created_as() else {
        // No create seen for this store (a defensively seeded bare delta): nothing the reference
        // would have an object to notify on. The fields still land.
        store.merge(delta);
        return;
    };
    store.merge_diff(delta, |index, old, new| {
        emit(FieldChanged {
            entity,
            guid,
            kind,
            index,
            old,
            new,
        });
    });
}

/// Apply a descriptor update to `entity` the way the drain does — seed on first sight (no
/// edges), merge and emit [`FieldChanged`] after — so a watcher test speaks the wire's language
/// instead of poking the component. The fixture must be [`ObjectFields::into_created`] on its
/// first application, like every real create block.
#[cfg(test)]
pub(crate) fn apply_fields_for_test(world: &mut World, entity: Entity, delta: ObjectFields) {
    let guid = world.get::<Guid>(entity).map_or(0, |g| g.0);
    let mut edges = Vec::new();
    match world.get_mut::<ObjectStore>(entity) {
        Some(mut store) => merge_store_fields(&mut store.0, delta, entity, guid, |e| edges.push(e)),
        None => {
            assert!(
                delta.created_as().is_some(),
                "a fixture's first descriptor is a create block: chain `.into_created(..)`"
            );
            world.entity_mut(entity).insert(ObjectStore(delta));
        }
    }
    world
        .resource_mut::<Messages<FieldChanged>>()
        .write_batch(edges);
}

/// Marks our own player's streamed entity (guid == [`SelfGuid`]). This is **identity** — "my
/// character", the thing whose bags, auras, quest log and paper doll are mine. It is deliberately
/// *not* "the thing I steer": that is [`Embodied`], which normally sits on the same entity and
/// leaves it while you drive something else.
#[derive(Component)]
pub(crate) struct SelfPlayer;

/// **The body this client is attached to** — where the camera is, whose feet make the footsteps,
/// whose height the water lines are measured against, whose server-authored spline is ours to ride.
/// The reference's **camera anchor** (`camera+0x88`), which has exactly three writers — the
/// constructor and `SetTarget 0x50d0f0`'s two legs — and is *not* touched by a control update.
///
/// Normally our own body, so it rides alongside [`SelfPlayer`]. While we hold somebody else's reins
/// — mind-controlling a creature, an Eye of Kilrogg — it moves to *that* entity and our own body
/// keeps none of it, exactly as `GetCamera().SetView(target)` moves the anchor at possession. That
/// is decision 0092's separation carried one step further: possession moves what you *inhabit*
/// without moving who you *are*.
///
/// **Attached is not the same as allowed to move**, and the second is [`ActiveMover`]'s job, not
/// this marker's (decision 1281). A feared player is still attached to their own body — the
/// reference keeps the anchor on it and keeps following it, merely smoothed — while the server, not
/// their input, says where it goes.
///
/// Exactly one entity carries it, and possibly none — a claimed mover whose object has not streamed
/// in yet is nobody, never a silent fallback to our own body. That distinction is load-bearing:
/// outbound `MSG_MOVE_*` carry no guid, so driving our body under a claimed creature's mover writes
/// our pose onto the creature. [`crate::player`] owns the placement.
#[derive(Component)]
pub(crate) struct Embodied;

/// **The one unit this client authors motion for** — the reference's active-mover global
/// (`ds:0xc4da98`/`0xc4da9c`, written only by `SetActiveMover 0x6006e0`), as a marker on the entity
/// that global names.
///
/// [`Embodied`] narrowed to the frames the body is ours to move. The narrowing is the reference's
/// own: `0x5fa600` **zeroes the mover globals** when a control update forbids the unit they name,
/// and with them zero the input applier (`0x514640` skips the whole tick when the mover does not
/// resolve) and every plain movement report — `0x600860`'s mover check kills the lot, heartbeat
/// included. There is no separate "may not move" flag in the movement path; the zeroed global *is*
/// the immobility.
///
/// Here it also answers the ECS's version of that question — *may the ordinary server-replay path
/// move this unit?* A body we are attached to but may not move must answer **yes**, which is how a
/// mind-controlled player sees their own character walk where their captor drives it. A body we are
/// driving must answer **no**, or the server's echo of our own movement fights the controller. So
/// the replay lanes filter `Without<ActiveMover>`, not `Without<Embodied>`: the two differ only
/// while control is lost, which is precisely the window where the difference is the whole point.
#[derive(Component)]
pub(crate) struct ActiveMover;

// ── ECS state: resources ─────────────────────────────────────────────────────────────────────────

/// The inbound event channel — drained each frame by [`apply_net_updates`].
#[derive(Resource)]
pub(crate) struct NetEvents(Receiver<SessionEvent>);

/// The outbound command channel — cloned by the player/chat systems to send movement + chat.
#[derive(Resource)]
pub(crate) struct NetCommands(pub(crate) Sender<ClientCommand>);

/// The character-management channel: the app's answer to each [`CharListMessage`] while the IO
/// thread is parked at select. [`CharRequest::Enter`] picks a character to `CMSG_PLAYER_LOGIN` as
/// (decision 0193); [`CharRequest::Create`]/[`CharRequest::Delete`] are serviced *in place* by the
/// parked loop, which stays parked (re-enum, re-emit the roster, emit a [`SessionEvent::CharActionResult`])
/// until an `Enter` moves the session into the world (decision 0423). Sent by [`crate::char_select`]'s
/// pick policy and the char-create screen; the parked IO read thread blocks on the other end.
#[derive(Resource)]
pub(crate) struct CharPick(pub(crate) Sender<CharRequest>);

/// One request to the parked IO thread over the [`CharPick`] channel (decision 0423). `Delete`'s
/// wire + service path is live (proven by `WOW_PROBE_CHARCREATE`); its UI affordance is a later
/// slice (decision 0423's deferrals).
pub(crate) enum CharRequest {
    /// Log in as this character (`CMSG_PLAYER_LOGIN`) — the session leaves select for the world.
    Enter(u64),
    /// Create a character (`CMSG_CHAR_CREATE`), then stay parked at select with the fresh roster.
    Create(benilla_protocol::CharCreateReq),
    /// Delete a character by guid (`CMSG_CHAR_DELETE`), then stay parked at select.
    Delete(u64),
    /// Select's Back (decision 0539): drop the parked session and return the IO thread to the
    /// pre-logon park — the app is heading to the login screen.
    Abandon,
}

/// The **realm channel**: the app's answer to each [`RealmListMessage`], whichever park is asking.
///
/// **Two parks listen on it**, because the reference's realm list is a dialog rather than a screen
/// (`RealmList.xml` is `frameStrata="DIALOG"` and `GlueParent.lua`'s `GlueScreenInfo` has no
/// `realmlist` entry): the *login-side* realm park, between the logon and the first world dial,
/// and the *character* park, where Change Realm raises the same list over the select screen. The
/// app sends the same three requests either way and never has to know which one is listening —
/// which is the point, since the answer to Cancel ("hide the dialog") is the same in both.
///
/// Sent by [`crate::realm_select`]'s policy (a remembered realm, `WOW_REALM`, or the player's
/// click); the parked read thread blocks on the other end.
#[derive(Resource)]
pub(crate) struct RealmChoice(pub(crate) Sender<RealmRequest>);

/// One request to the IO thread parked at the realm list.
#[derive(Debug)]
pub(crate) enum RealmRequest {
    /// Enter this realm — dial its world server. Carries the realm's **name**, not its index: the
    /// list is re-requested every few seconds while the screen is up, and a server that adds or
    /// drops a realm between the draw and the click would otherwise silently move the row out from
    /// under the player's finger.
    Enter(String),
    /// Re-request the realm list on the still-open realmd connection (the reference's
    /// `RequestRealmList`, fired by `RealmList_OnUpdate` every 5 s while the window is open).
    Refresh,
    /// The realm list's Cancel — `RealmList_OnCancel`, which only hides the frame. What that means
    /// depends on which park hears it, and in both cases it means "leave the screen underneath
    /// alone": at the login-side park there is no session yet, so the thread returns to the
    /// pre-logon park and the login screen is what the player is left looking at; at the character
    /// park the parked world session is untouched and the select screen simply reappears.
    Abandon,
}

/// The credentials channel (decision 0539): the app's answer to the IO thread's **pre-logon park**.
/// Sent by [`crate::login`]'s policy (the screen's submit, the env fast path, the reconnect
/// resubmit); the parked read thread blocks on the other end.
#[derive(Resource)]
pub(crate) struct LoginSubmit(pub(crate) Sender<io::LoginRequest>);

/// The login abandon generation (decision 0539): Cancel bumps it; each [`io::LoginRequest`] carries
/// the value read at submit, and the IO thread discards an attempt whose value has been passed.
#[derive(Resource)]
pub(crate) struct LoginAbandon(pub(crate) std::sync::Arc<std::sync::atomic::AtomicU64>);

/// guid → spawned ECS entity, for O(1) lookup on move/remove — every kind, items and containers
/// included since decision 2334 (the reference's one `ClntObjMgr` index). Maintained solely by
/// the object layer's handlers (`net::objects`); read-only to everyone else, through [`Objects`]
/// or directly.
#[derive(Resource, Default)]
pub(crate) struct GuidIndex(pub(crate) HashMap<u64, Entity>);

/// **The object manager's guid lookup** — `ClntObjMgrObjectPtr 0x468460`'s shape: a guid to its
/// descriptor store, whatever the kind. The one read-only parameter a helper that resolves guids
/// takes (the inventory walkers resolve item guids off the player's slot arrays through it,
/// decision 2334), and an item's countdown cells beside its fields (2340); the handlers write
/// both through their own mutable queries.
#[derive(SystemParam)]
pub(crate) struct Objects<'w, 's> {
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    countdowns: Query<'w, 's, &'static crate::items::Countdowns>,
}

impl Objects<'_, '_> {
    /// The entity behind a guid, if streamed.
    pub(crate) fn entity(&self, guid: u64) -> Option<Entity> {
        self.index.0.get(&guid).copied()
    }

    /// A streamed object's merged descriptor fields.
    pub(crate) fn object(&self, guid: u64) -> Option<&ObjectFields> {
        self.index
            .0
            .get(&guid)
            .and_then(|&e| self.stores.get(e).ok())
            .map(|s| &s.0)
    }

    /// An item object's countdown cells — `None` for a guid that is not a held item.
    pub(crate) fn countdowns(&self, guid: u64) -> Option<&crate::items::Countdowns> {
        self.index
            .0
            .get(&guid)
            .and_then(|&e| self.countdowns.get(e).ok())
    }
}

/// Our own player's guid, once the IO thread reports we're in the world. Used to tag
/// [`SelfPlayer`], and read by the combat-text emitters' source-ownership classifier
/// (`crate::combat_text::melee_impact_text` — the "mine" test against Summoned/CreatedBy).
#[derive(Resource, Default)]
pub(crate) struct SelfGuid(pub(crate) Option<u64>);

/// Connection status for the rest of the app (decision 0065): `connected` flips on
/// `Connected`/`Disconnected`; `last_reason` keeps the most recent failure so it can be surfaced
/// (debug panel, future UI). The streamed-world teardown itself happens in [`apply_net_updates`].
#[derive(Resource, Default)]
pub(crate) struct NetStatus {
    pub(crate) connected: bool,
    pub(crate) last_reason: Option<String>,
}

/// The connection's ping clock and RTT history ([`io::PingClock`]), shared with both net threads:
/// the write thread stamps each `CMSG_PING` on it, and the **read thread** measures the
/// `SMSG_PONG` echo against it the moment the packet lands. The app only ever *reads* it — for
/// `GetNetStats` ([`crate::ui_net`]) and the debug panel — and clears it when a connection dies.
/// It does not measure: doing that from the drain is what B346 was.
#[derive(Resource)]
pub(crate) struct PingShared(pub(crate) std::sync::Arc<std::sync::Mutex<io::PingClock>>);

/// The dropped-packet tally — the wire-coverage instrument (decision 0022). Every server packet the
/// codec dropped on the floor lands here, keyed by opcode: `unknown` counts opcodes with **no parse
/// arm at all** (`ServerPacket::Other`), `unparseable` counts packets whose parser errored (skipped
/// to keep the stream aligned). Fed by [`apply_net_updates`] from `SessionEvent::PacketDropped`;
/// read by the debug panel's Net section. Never cleared on reconnect — the tally is about coverage,
/// not session state.
#[derive(Resource, Default)]
pub(crate) struct DroppedOpcodes(pub(crate) HashMap<u16, DropTally>);

/// Per-opcode drop counts (see [`DroppedOpcodes`]).
#[derive(Default, Clone, Copy)]
pub(crate) struct DropTally {
    pub(crate) unknown: u64,
    pub(crate) unparseable: u64,
}

/// The server in-game clock sample (`SMSG_LOGIN_SETTIMESPEED`), advanced by its timescale. `None`
/// until the first time packet. Read by the lighting subsystem to drive time-of-day.
#[derive(Resource, Default)]
pub(crate) struct ServerTime(pub(crate) Option<GameTime>);

/// Publish the session clock into the engine's [`benilla_world::lighting::WorldTime`] input, once a
/// frame. The renderer wants three scalars, not a wire sample with an `Instant` in it — and with
/// no server at all it wants noon, which is `WorldTime`'s own default rather than a branch here.
pub(crate) fn publish_world_time(
    server: Res<ServerTime>,
    mut world_time: ResMut<benilla_world::lighting::WorldTime>,
) {
    *world_time = match server.0 {
        Some(gt) => benilla_world::lighting::WorldTime {
            minute: gt.minute_of_day(),
            minute_f: gt.minute_of_day_f32(),
            day: gt.day_continuous(),
            live: true,
        },
        None => benilla_world::lighting::WorldTime::default(),
    };
}

/// The server's **wall clock** (`SMSG_QUERY_TIME_RESPONSE`, asked for on entering the world),
/// advanced monotonically from the sample. `None` until the first answer lands.
///
/// Not [`ServerTime`] just above: that is the *in-game* day/night clock the lighting reads, a
/// different quantity in a different unit which says nothing about the epoch here. This one is
/// unix-epoch seconds, and it exists because the server writes **absolute** stamps in that epoch
/// into descriptor fields — a timed quest's deadline is `time(nullptr) + limitTime` (vmangos
/// `Player::AddQuest`) and no packet ever restates it as a duration. So a countdown is
/// `deadline − now_unix()`, and the local machine's own wall clock is deliberately never consulted:
/// a player whose clock is a minute off would see every countdown a minute wrong, silently and only
/// on their screen (decision 1150).
#[derive(Resource, Default)]
pub(crate) struct ServerWallClock(pub(crate) Option<WallClockSample>);

/// One `SMSG_QUERY_TIME_RESPONSE` sample plus the monotonic instant it arrived — the [`GameTime`]
/// pattern, for the same reason: the clock advances between packets without re-reading a local wall
/// clock that an NTP step or a manual change can move under us.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WallClockSample {
    /// The server's `time(nullptr)` at `received`.
    base_unix: u32,
    /// When the answer arrived (monotonic).
    received: Instant,
}

impl ServerWallClock {
    /// Take a fresh sample (the `SMSG_QUERY_TIME_RESPONSE` handler).
    ///
    /// The reply is one round trip old, so the clock reads ~RTT/2 behind the server's; against a
    /// countdown displayed to the second that is below the noise floor, which is why there is no
    /// correction term here.
    pub(crate) fn sample(&mut self, unix_time: u32) {
        self.0 = Some(WallClockSample {
            base_unix: unix_time,
            received: Instant::now(),
        });
    }

    /// Server unix seconds now (fractional), or `None` before the first sample.
    pub(crate) fn now_unix(&self) -> Option<f64> {
        self.0
            .map(|s| f64::from(s.base_unix) + s.received.elapsed().as_secs_f64())
    }

    /// Whether the sample is old enough to re-ask (or absent entirely).
    fn stale(&self) -> bool {
        self.0.is_none_or(|s| s.received.elapsed() >= RESYNC_AFTER)
    }
}

/// How long a wall-clock sample stands before the client asks again — the reference's own hour
/// (the resync site `0x4de836` is gated on `now > [0xbb749c]`, armed `= now + 0xe10` at
/// `0x4def11`; decision 1154).
const RESYNC_AFTER: Duration = Duration::from_secs(3600);

/// Ask for the server's wall clock (decision 1150) — on entering the world (login, worldport and
/// instance transfer alike, the same cascade the mail arc's `QueryNextMailTime` hangs off), and
/// hourly thereafter.
///
/// The hourly leg is the reference's, and it is not about our own drift: a monotonic base does not
/// wander against a wall clock at any rate a countdown could show. What it tracks is the **server**
/// being re-clocked under us — the one direction no local reasoning can see — for the price of four
/// bytes an hour (decision 1154). It also self-heals a session that entered the world before the
/// socket could answer.
fn send_query_time(
    mut entered: MessageReader<EnteredWorldMessage>,
    commands: Res<NetCommands>,
    clock: Res<ServerWallClock>,
    status: Res<NetStatus>,
    mut asked_at: Local<Option<Instant>>,
) {
    let entering = entered.read().next().is_some();
    // Only chase the cadence while a session exists — a command with no live writer evaporates
    // with a warn, and the world-enter send covers the reconnect.
    let due =
        status.connected && clock.stale() && asked_at.is_none_or(|t| t.elapsed() >= RESYNC_AFTER);
    if entering || due {
        *asked_at = Some(Instant::now());
        let _ = commands.0.send(ClientCommand::QueryTime);
    }
}

/// Our player's reputation store: `(flags, standing)` per reputation-list slot, indexed by
/// `Faction.dbc`'s `reputationIndex`. The standing excludes the DBC race/class base — consumers add
/// it before ranking. Empty until `SMSG_INITIALIZE_FACTIONS` lands at login; kept current after that
/// by `SMSG_SET_FACTION_STANDING` (which also auto-reveals) and `SMSG_SET_FACTION_VISIBLE`.
///
/// Two consumers, and they read different halves. The **reaction decode** (targeting) reads the
/// standing: a reputation faction's NPCs colour by our rank with them, before any faction-template
/// comparison. The **reputation pane** ([`crate::ui_reputation`]) reads the flag byte as well — the
/// visible bit decides which rows exist at all, and bit `0x08` marks the pane's headers.
#[derive(Resource, Default)]
pub(crate) struct Reputations(pub(crate) Vec<(u8, i32)>);

/// The player's hearthstone bind point (`SMSG_BINDPOINTUPDATE`, at login + on re-bind): the
/// AreaTable id the hearthstone tooltip's `$z` token names ("Returns you to Goldshire.").
/// `None` until the packet lands.
#[derive(Resource, Default)]
pub(crate) struct HomeBind(pub(crate) Option<u32>);

/// The `/played` answer awaiting delivery to the VM (`SMSG_PLAYED_TIME`): total seconds played and
/// seconds since the last level-up.
///
/// A one-slot mailbox rather than a queue: the reply answers a request, the server sends one per
/// request, and a second landing before the first is drained would mean the first is already stale.
/// `None` whenever there is nothing undelivered.
#[derive(Resource, Default)]
pub(crate) struct PlayedTimeAnswer(pub(crate) Option<(u32, u32)>);

/// The player's equip proficiencies (`SMSG_SET_PROFICIENCY`, at login + on train): item class
/// (2 weapons / 4 armor) → allowed-subclass bitmask — the client's `0xc4d4a0[class]` store.
/// The item tooltip's slot-line red compares against it. Empty until the packets land.
#[derive(Resource, Default)]
pub(crate) struct Proficiencies(pub(crate) std::collections::HashMap<u32, u32>);

/// The server's in-game clock, from `SMSG_LOGIN_SETTIMESPEED`. WoW packs the server's `localtime()`
/// into the `DateTime`, so the in-game hour:minute follows the server's wall clock; `timescale` is how
/// many game-minutes pass per real second (vmangos ≈ `0.01667` → real-time). We keep the sample plus
/// the [`Instant`] it arrived so the clock advances between packets.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GameTime {
    /// Minute of the game day (`0..1440`) at `received`.
    base_minute: u32,
    /// Monotonic day serial at `received` (`year·372 + month·31 + day` of the packed server date —
    /// the packed convention's fixed 31-day months). Only differences matter: it feeds the
    /// celestial moon-phase precession (`(dayCounter + todPhase) mod 1.7`, decision 0485).
    base_day: u32,
    /// Game-minutes elapsed per real second.
    timescale: f32,
    /// When this sample arrived (monotonic; used to advance the clock).
    received: Instant,
}

impl GameTime {
    /// Build from a decoded `SMSG_LOGIN_SETTIMESPEED` (server `hours`/`minutes`/`day_serial` +
    /// `timescale`).
    pub(crate) fn new(hours: u8, minutes: u8, day_serial: u32, timescale: f32) -> Self {
        Self {
            base_minute: hours as u32 * 60 + minutes as u32,
            base_day: day_serial,
            timescale,
            received: Instant::now(),
        }
    }

    /// Current minute of the game day (`0..1440`), advanced from the sample by `timescale`.
    pub(crate) fn minute_of_day(&self) -> u32 {
        self.minute_of_day_f32() as u32
    }

    /// Current **fractional** minute of the game day (`0.0..1440.0`), advanced from the sample by
    /// `timescale` — same value as [`Self::minute_of_day`] but without truncation. Drives the celestial
    /// body *positions* continuously (sampling at the truncated integer minute steps them once per
    /// game-minute, a visible jump at a fast timescale).
    pub(crate) fn minute_of_day_f32(&self) -> f32 {
        let elapsed = self.received.elapsed().as_secs_f32() * self.timescale;
        (self.base_minute as f32 + elapsed).rem_euclid(1440.0)
    }

    /// Continuous **days + day-fraction** since the day-serial epoch, advanced by `timescale` —
    /// UNWRAPPED (it crosses midnight by growing past the next integer), in `f64` (the serial
    /// reaches ~12k and the consumer needs sub-minute resolution on top). Feeds the celestial
    /// moon-phase `(dayCounter + todPhase) mod 1.7` (`0x6d41b9`; decision 0485).
    pub(crate) fn day_continuous(&self) -> f64 {
        let elapsed = self.received.elapsed().as_secs_f64() * self.timescale as f64;
        self.base_day as f64 + (self.base_minute as f64 + elapsed) / 1440.0
    }
}

// ── Outbound commands + one-shot inbound events ──────────────────────────────────────────────────

/// One self-movement event the controller streams to the server. Each maps to a `MSG_MOVE_*` opcode
/// ([`Self::opcode`]); the writer thread attaches the current `MovementInfo`. This mirrors the real
/// client: it announces each movement-*axis* transition (start/stop forward-back, strafe, turn), a
/// jump, a facing change (mouse-turn while standing), and a periodic heartbeat while moving — so the
/// server relays a faithful movement stream to nearby players.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoveKind {
    StartForward,
    StartBackward,
    Stop,
    StartStrafeLeft,
    StartStrafeRight,
    StopStrafe,
    StartTurnLeft,
    StartTurnRight,
    StopTurn,
    Jump,
    FallLand,
    StartSwim,
    StopSwim,
    /// The walk/run gait flipped — the `TOGGLERUN` keybind's own packet. The reference sends it
    /// from the toggle handler itself (`ToggleRun 0x513d50` → `0x60e080` → `0x617de0`, which
    /// picks move-event kind `0xf`/`0xe` off the CURRENT walk bit and enqueues it), not from the
    /// move-state broadcaster — so it goes out **standing still**, where the broadcaster's
    /// locomotion-nibble gate (`0x61a99d test al,0xf`) would have swallowed it.
    SetWalkMode,
    SetRunMode,
    SetFacing,
    Heartbeat,
}

impl MoveKind {
    /// The wire opcode this event sends.
    pub(crate) fn opcode(self) -> u16 {
        use benilla_protocol::messages::opcode as op;
        match self {
            MoveKind::StartForward => op::MSG_MOVE_START_FORWARD,
            MoveKind::StartBackward => op::MSG_MOVE_START_BACKWARD,
            MoveKind::Stop => op::MSG_MOVE_STOP,
            MoveKind::StartStrafeLeft => op::MSG_MOVE_START_STRAFE_LEFT,
            MoveKind::StartStrafeRight => op::MSG_MOVE_START_STRAFE_RIGHT,
            MoveKind::StopStrafe => op::MSG_MOVE_STOP_STRAFE,
            MoveKind::StartTurnLeft => op::MSG_MOVE_START_TURN_LEFT,
            MoveKind::StartTurnRight => op::MSG_MOVE_START_TURN_RIGHT,
            MoveKind::StopTurn => op::MSG_MOVE_STOP_TURN,
            MoveKind::Jump => op::MSG_MOVE_JUMP,
            MoveKind::FallLand => op::MSG_MOVE_FALL_LAND,
            MoveKind::StartSwim => op::MSG_MOVE_START_SWIM,
            MoveKind::StopSwim => op::MSG_MOVE_STOP_SWIM,
            MoveKind::SetWalkMode => op::MSG_MOVE_SET_WALK_MODE,
            MoveKind::SetRunMode => op::MSG_MOVE_SET_RUN_MODE,
            MoveKind::SetFacing => op::MSG_MOVE_SET_FACING,
            MoveKind::Heartbeat => op::MSG_MOVE_HEARTBEAT,
        }
    }
}

/// The wire `ChatMsg` type byte an addon broadcast rides (decision 1235) — the client's own
/// four-lane whitelist at `0x49fa3f`-`0x49fa4e`.
///
/// A **total** map from a closed enum, and a named function rather than an inline `match` in
/// [`io`]'s dispatch so that it is assertable: this is the one place a distribution becomes a wire
/// byte, and getting it wrong sends an addon's payload down a lane nobody is listening on. There
/// is no fallback arm and none can be added — that is the whole reason the distribution crosses
/// the crate boundary as an enum instead of a token string.
pub(crate) fn addon_wire_chat_type(distribution: benilla_ui::script::AddonDistribution) -> u32 {
    use benilla_protocol::messages as m;
    use benilla_ui::script::AddonDistribution as D;
    match distribution {
        D::Party => m::CHAT_TYPE_PARTY,
        D::Raid => m::CHAT_TYPE_RAID,
        D::Guild => m::CHAT_TYPE_GUILD,
        D::Battleground => m::CHAT_TYPE_BATTLEGROUND,
    }
}

/// The wire `ChatMsg` type a chat-bar line sends as — the FULL sendable set (decision 0288 P5;
/// vmangos `HandleChatMessageOpcode`'s switch). Each maps to its `WorldWriter` sender in [`io`]'s
/// dispatch; `Whisper` carries its target and `Channel` its channel name through
/// [`ClientCommand::Chat`]'s `target` field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ChatKind {
    Say,
    Yell,
    Emote,
    Whisper,
    Party,
    Raid,
    RaidLeader,
    RaidWarning,
    Guild,
    Officer,
    Battleground,
    BattlegroundLeader,
    Afk,
    Dnd,
    Channel,
}

/// Diagnostic gate (`WOW_CAST_TRACE=1`): log our own cast lifecycle — every self `SMSG_SPELL_START`
/// / `GO` / `CAST_RESULT` / `SPELL_FAILED_OTHER` / `SPELL_DELAYED` — **and** every outbound movement
/// packet, at info. A live "the cast bar vanished when I got hit" repro then shows exactly which
/// packet ended the cast (a pushback that should extend vs an interrupt that cancels) and whether a
/// movement packet preceded it — vmangos interrupts a cast on any movement report whose position
/// differs from the server's stored one (`Player::SetPosition`'s exact-float `positionChanged`). A
/// reusable instrument, off by default (decision 0022 — own the instruments).
pub(crate) static CAST_TRACE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var("WOW_CAST_TRACE").is_ok());

/// A message the ECS sends to the server through the write thread. Carries the player's pose so the
/// writer thread needs no shared state.
#[derive(Debug)]
pub(crate) enum ClientCommand {
    /// A self-movement packet: the [`MoveKind`] opcode + the live CMovement `flags` + our pose, plus the
    /// airborne clock (`fall_time`, ms) and ballistic launch tail (`jump`) while jumping/falling. The
    /// `flags` set the base directional/turn/walk bits, `JUMPING` while airborne, and `ON_TRANSPORT`
    /// while standing on a boat/zepp (with its `transport` local-frame tail — decision 0438 phase 2);
    /// the writer serializes each tail iff its flag is set (a flag without its tail desyncs the
    /// server's parse).
    Move {
        kind: MoveKind,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        /// Swim pitch (radians, +up) — written iff `SWIMMING` is in `flags`; `0` otherwise.
        pitch: f32,
        fall_time: u32,
        jump: Option<JumpInfo>,
        /// The rider's pose in the transport's local frame — written iff `ON_TRANSPORT` is in `flags`.
        transport: Option<TransportPose>,
    },
    /// Acknowledge a finished server-authored spline that drove our own player (Charge, and later
    /// knockback/taxi): `CMSG_MOVE_SPLINE_DONE` with our pose at the ride's endpoint and the
    /// `spline_id` we were driven by. The server holds a player mover as spline-pending until this
    /// arrives, so the controller sends it the instant the ride ends, then resumes its own stream.
    MoveSplineDone {
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        spline_id: u32,
    },
    /// Answer a `SMSG_FORCE_*_SPEED_CHANGE` (`CMSG_FORCE_*_SPEED_CHANGE_ACK`): echo the mover
    /// `guid`/`counter`/exact `speed`, carrying the same live wire payload a [`Self::Move`] would
    /// (the server relocates us to it and its anticheat runs the same position tests). Mandatory —
    /// unacked, the server force-resolves after ~4 s and cheat-flags the session. Sent by the
    /// controller (which owns the live pose) off a [`SpeedChangeMessage`].
    ForceSpeedAck {
        kind: SpeedKind,
        guid: u64,
        counter: u32,
        speed: f32,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        pitch: f32,
        fall_time: u32,
        jump: Option<JumpInfo>,
        transport: Option<TransportPose>,
    },
    /// Report that our mover's simulation advanced through `lag_ms` without integrating
    /// (`CMSG_MOVE_TIME_SKIPPED`) — decision 1935. `guid` is the mover's own, the reference's
    /// `0x600be0` gate being that it equals the active-mover globals. The server folds the number
    /// into its copy of that mover's movement clock (`stime`/`ctime`), and — while it thinks we
    /// have just boarded a transport — answers by re-sending the transport's create update.
    MoveTimeSkipped {
        guid: u64,
        lag_ms: u32,
    },
    /// Claim `guid` as our mover (`CMSG_SET_ACTIVE_MOVER`). Login sends it for our own body; a
    /// possession handoff re-sends it for the unit we were handed, because the server drops every
    /// `MSG_MOVE_*` for a mover it has not confirmed.
    SetActiveMover {
        guid: u64,
    },
    /// Release `guid` as our mover (`CMSG_MOVE_NOT_ACTIVE_MOVER`) at the pose it is parting on —
    /// the server re-broadcasts a stop under that guid from this payload, so observers do not keep
    /// the unit sliding.
    NotActiveMover {
        guid: u64,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        fall_time: u32,
    },
    /// Vote on the far-sight view (`CMSG_FAR_SIGHT`): `true` as it attaches, `false` as it
    /// releases. The reference sends both; neither names an object.
    FarSight {
        engage: bool,
    },
    /// Echo a same-map teleport ack (`MSG_MOVE_TELEPORT_ACK_Client`) — without it the server freezes
    /// our movement until relog.
    TeleportAck {
        guid: u64,
        counter: u32,
    },
    /// Echo a cross-map worldport ack (`MSG_MOVE_WORLDPORT_ACK`) — unblocks the new map's stream.
    WorldportAck,
    /// Set (or clear, with `guid == 0`) our current target on the server (`CMSG_SET_SELECTION`).
    SetSelection {
        guid: u64,
    },
    /// Send a chat line as the given [`ChatKind`] (`CMSG_MESSAGECHAT`). `target` carries the
    /// whisper recipient's name — meaningful only when `kind == ChatKind::Whisper` — and is
    /// `None` otherwise. Plain lines with no leading `/` (including `.` dot commands, which
    /// vmangos parses as GM commands on the way in) ride as `ChatKind::Say`, unchanged from
    /// before slash commands existed. Built by [`crate::ui_chat`]'s command parser.
    Chat {
        kind: ChatKind,
        target: Option<String>,
        text: String,
    },
    /// **An addon broadcast** (`SendAddonMessage`, decision 1235) — a `CMSG_MESSAGECHAT` carrying
    /// `LANG_ADDON` in the language field instead of the speaker's tongue, which is the entire
    /// difference between addon data and speech (1.12.1 has no addon opcode).
    ///
    /// Deliberately **not** a flag on [`Self::Chat`]: that command's whole family speaks
    /// `WorldWriter::chat_language`, and a language field bolted onto it would be a `None` that
    /// fourteen arms have to remember to honour. This one carries no target (1.12's
    /// `SendAddonMessage` has no fourth argument, so there is no whispered addon message) and no
    /// language dial (there is exactly one legal value).
    ///
    /// `distribution` is the client's own four-value whitelist, already resolved and already
    /// downgraded for the outside-a-raid case by the binding
    /// ([`benilla_ui::script::AddonDistribution`]); `text` is the composed `prefix` TAB `message`.
    AddonMessage {
        distribution: benilla_ui::script::AddonDistribution,
        text: String,
    },
    /// Ask a player character's name (`CMSG_NAME_QUERY`); answered by a `PlayerName` event into the
    /// [`crate::names::NameCache`]. Sent by the cache's ask-once resolve, never directly.
    NameQuery {
        guid: u64,
    },
    /// Ask a creature template's name (`CMSG_CREATURE_QUERY`, entry from the guid's bits 24–47);
    /// answered by a `CreatureName` event into the cache.
    CreatureQuery {
        entry: u32,
        guid: u64,
    },
    /// Ask a pet's name (`CMSG_PET_NAME_QUERY`, pet number from the guid's bits 24–47 — where a
    /// creature keeps its template entry); answered by a `PetName` event into the cache. A pet
    /// cannot be named by [`Self::CreatureQuery`]; see [`benilla_protocol::guid::pet_number`].
    PetNameQuery {
        pet_number: u32,
        guid: u64,
    },
    /// Ask an item template (`CMSG_ITEM_QUERY_SINGLE`; `guid` = the concrete item when the ask is
    /// about one, `0` for template-only); answered by an `ItemTemplate` event into the
    /// [`crate::items::Items`] cache. Sent by the cache's ask-once resolve, never directly.
    ItemQuery {
        entry: u32,
        guid: u64,
    },
    /// Use an item by wire bag position (`CMSG_USE_ITEM` — bag 255 + absolute slot for anything
    /// in the player's own field array, i.e. an equipment slot, the backpack or the keyring; a
    /// bag's own player-array slot 19–22 + inner slot otherwise). The container drain maps the
    /// Lua `(bagID, slot)` space before sending. `spell_index` is the template block ordinal the
    /// server should cast (`ItemInfo::use_spell_index`) — 0 for every item whose on-use spell is
    /// its first block, which is nearly all of them.
    ///
    /// `target` is the cast-targets block, bound by the one cast ladder exactly as a
    /// [`Self::CastSpell`]'s is — the real client's `SendCast 0x6e54f0` writes one block for both
    /// opcodes. `Object` is the key-in-a-lock arm (decision 0769); `Unit` a bandage/soulstone;
    /// `SelfImplicit` every ordinary consumable.
    UseItem {
        bag_index: u8,
        slot: u8,
        spell_index: u8,
        target: benilla_protocol::messages::UseItemTarget,
    },
    /// Open a bag item (`CMSG_OPEN_ITEM`, same bag addressing) — the drain's fork for an
    /// *openable* click (`ItemInfo::openable`): a clam, an unlocked lockbox, a wrapped gift. The
    /// server answers `SMSG_LOOT_RESPONSE` on the **item's own guid**, so the ordinary loot feed
    /// opens a window over a thing in the bag; a wrapped gift instead swaps entry in place and
    /// sends no window. Refusals arrive as `InventoryFailure` (still locked, dead, flying).
    OpenItem {
        bag_index: u8,
        slot: u8,
    },
    /// Wrap the item at `(item_bag, item_slot)` in the paper at `(gift_bag, gift_slot)`
    /// (`CMSG_WRAP_ITEM`, the same bag addressing) — the completion of the local wrap cursor a
    /// `ITEM_FLAG_WRAPPER` item's right-click armed. The **paper leads**, as the wire does.
    ///
    /// The client applies no eligibility filter of its own (decision 1934): an equipped item, a
    /// bag, a soulbound or stackable or unique item, or an already-wrapped one all send and are
    /// refused server-side with one of the six `ERR_CANT_WRAP_*` reasons, arriving here as an
    /// `InventoryFailure` on the UI error line. Success is silent — field updates on the target
    /// and one paper destroyed.
    WrapItem {
        gift_bag: u8,
        gift_slot: u8,
        item_bag: u8,
        item_slot: u8,
    },
    /// Equip a bag item (`CMSG_AUTOEQUIP_ITEM`, same bag addressing) — the drain's fork for an
    /// *equippable* click, mirroring the real client's equip-vs-use decision. Refusals come back
    /// as `InventoryFailure` events onto the UI error line.
    AutoEquipItem {
        bag_index: u8,
        slot: u8,
    },
    /// Load ammo into the ammo slot (`CMSG_SET_AMMO`) — the equip drains' fork when the clicked/
    /// dropped item is ammo-class (INVTYPE_AMMO), mirroring the real client's own auto-equip fork
    /// (`0x5e1480`). Addressed by item `entry`, not a bag slot: the stack
    /// stays in the bag and `PLAYER_AMMO_ID` starts referencing it. A wrong/absent ranged weapon
    /// refuses via `InventoryFailure`. Decision 0526.
    SetAmmo {
        entry: u32,
    },
    /// Swap two of the player's own inventory slots (`CMSG_SWAP_INV_ITEM`) — the wire for a
    /// backpack-internal pick/place/swap, both slots on the player array (`INVENTORY_SLOT_ITEM_START`
    /// onward). The container-move drain maps the Lua backpack `(bag 0, slot)` space onto these; an
    /// empty destination is a move. Refusals surface as `InventoryFailure` events.
    SwapInvItem {
        src_slot: u8,
        dst_slot: u8,
    },
    /// The general bag↔bag move (`CMSG_SWAP_ITEM`, decision 0216 §6 slice 2) — either endpoint may
    /// be an equipped bag (unlike [`ClientCommand::SwapInvItem`], the player-array-only wire). The
    /// container-move drain sends this instead of `SwapInvItem` whenever either end's wire bag
    /// isn't [`benilla_protocol::messages::BAG_PLAYER_INVENTORY`] — VERIFIED vmangos
    /// `Packets/Item.cpp:30-36`: body order is dstbag, dstslot, srcbag, srcslot (opcode 0x10C).
    /// Refusals surface as `InventoryFailure` events.
    SwapItem {
        dst_bag: u8,
        dst_slot: u8,
        src_bag: u8,
        src_slot: u8,
    },
    /// Auto-store an item into a bag (`CMSG_AUTOSTORE_BAG_ITEM`): take `(src_bag, src_slot)` and
    /// put it anywhere inside `dst_bag` — **the client names no destination slot** (`0x5e12e0`:
    /// AUTOSTORE carries none). Sent by `PutItemInBag`'s third leg (an
    /// ordinary item dropped on an occupied bag button) and by all of `PutItemInBackpack`.
    /// Refusals surface as `InventoryFailure` events.
    AutoStoreBagItem {
        src_bag: u8,
        src_slot: u8,
        dst_bag: u8,
    },
    /// Carry a partial stack from one bag position to another (`CMSG_SPLIT_ITEM`, decision 0216
    /// §6) — either endpoint may be an equipped bag, unlike [`ClientCommand::SwapInvItem`]. The
    /// container-move drain sends this instead of a swap when the queued move carries a split
    /// count. Refusals surface as `InventoryFailure` events.
    SplitItem {
        src_bag: u8,
        src_slot: u8,
        dst_bag: u8,
        dst_slot: u8,
        count: u8,
    },
    /// Destroy a bag item (`CMSG_DESTROYITEM`, decision 0216 §3): `count` 0 = the whole stack.
    /// Sent by the delete-confirm popup's accept (`DeleteCursorItem`), mapped from the engine's
    /// queued `(bag, slot, count)` destroy through the same wire-position map as every other
    /// container drain.
    DestroyItem {
        bag_index: u8,
        slot: u8,
        count: u8,
    },
    /// Perform a chat emote (`CMSG_TEXT_EMOTE`); the server echoes it back to us (and everyone
    /// in range) as `SMSG_TEXT_EMOTE`, so the local sound/anim ride the receive path.
    TextEmote {
        text_id: u32,
        target: u64,
    },
    /// Cast a spell (`CMSG_CAST_SPELL`): `target: None` = self/implicit-target cast, `Some(guid)`
    /// = explicit unit target. Answered by `SMSG_CAST_RESULT` (a `CastResult` event).
    CastSpell {
        spell_id: u32,
        target: Option<u64>,
    },
    /// Cast a spell at a **ground point** (`CMSG_CAST_SPELL` with `TARGET_FLAG_DEST_LOCATION`,
    /// decision 0792): the targeting-cursor commit for a ground-targeted AOE. `dest` is the
    /// clicked world point in **WoW coords** (`bevy_to_wow` at the send site — the wire never
    /// sees Bevy space). Answered by `SMSG_CAST_RESULT`.
    CastSpellAtDest {
        spell_id: u32,
        dest: [f32; 3],
    },
    /// Cast a spell at a **source point** (`CMSG_CAST_SPELL` with `TARGET_FLAG_SOURCE_LOCATION`,
    /// decision 2218): the same terrain click, one bit over — `BindLocation 0x6e60f0` binds bit 5
    /// to `SPELLCAST+0x30` where it binds bit 6 to `+0x3c`. `src` is the clicked world point in
    /// **WoW coords**. Answered by `SMSG_CAST_RESULT`.
    CastSpellAtSource {
        spell_id: u32,
        src: [f32; 3],
    },
    /// Cancel one of our own auras (`CMSG_CANCEL_AURA`, decision 0257): the right-click-a-buff wire,
    /// carrying the **spell id** (the server cancels by spell, not slot). No answer packet — the
    /// removal comes back as a `UNIT_FIELD_AURA` delta. Sent by the aura feed's cancel drain.
    CancelAura {
        spell_id: u32,
    },
    /// Set (or clear, `packed == 0`) one action-bar slot (`CMSG_SET_ACTION_BUTTON`, decision
    /// 0216 §7/0218 §4): `button` is the 0-based wire slot (lua action id − 1), `packed` the
    /// engine's own `kind<<24 | action` word. Sent by the action drain on every queued
    /// `PickupAction`/`PlaceAction` mutation — client-authoritative, no answer packet.
    SetActionButton {
        button: u8,
        packed: u32,
    },
    /// Post the four extra bars' visibility byte (`CMSG_SET_ACTIONBAR_TOGGLES`, `PLAYER_FIELD_BYTES`
    /// byte 2 — read at `0x4e768c`, sent by `0x4e76e0`). Sent by the toggle drain, one
    /// per `SetActionBarToggles` call: the binding gates nothing, so two calls in a frame are two
    /// packets.
    ///
    /// **Nothing answers it, and the sender is not allowed to answer itself.** Unlike
    /// [`Self::SetActionButton`], whose state is genuinely ours, this byte is *server-owned*: the
    /// real client has no instruction that writes the cell (the one `+0x102a` access image-wide is
    /// a read), so the value only becomes true when the server's `SMSG_UPDATE_OBJECT` echoes it
    /// into the descriptor — and no field-change callback fires when it does. The owner therefore
    /// sees its own change come back the long way round, and the UI's optimism lives in Lua.
    ///
    /// A send while disconnected is a **silent no-op**, matching the reference's three unreported
    /// drops (`0x5ab637`, `0x5379ab`, `0x5379b6`) behind a binding that returns zero Lua values.
    SetActionBarToggles {
        toggles: u8,
    },
    /// Press one pet bar slot (`CMSG_PET_ACTION`, decisions 0982/0988). `packed` is the slot's OWN
    /// word as the server last sent it — command, reaction and spell all ride this one command,
    /// because the server dispatches on the type byte inside the word. `target_guid` is the
    /// player's current selection, which is what the client always sends (`0x4bd212`).
    ///
    /// **Nothing answers it.** Unlike [`Self::SetActionButton`], whose silence is because the
    /// state is ours, this one is silent because the *server* simply does not reply — so the
    /// caller has already applied the visible change locally before queueing it.
    PetAction {
        pet_guid: u64,
        packed: u32,
        target_guid: u64,
    },
    /// Write pet bar slots (`CMSG_PET_SET_ACTION`, decision 0988) — `(0-based position, the whole
    /// new word)` pairs, one or two.
    ///
    /// **This is how the autocast toggle travels**, with one entry: the client flips bit 30 in the
    /// slot's word and posts the result, and the server reads the direction back out of the type
    /// byte it arrives in. `CMSG_PET_SPELL_AUTOCAST` (0x2F3) is a *different* binding's opcode —
    /// the pet spellbook's `ToggleSpellAutocast`, which indexes the spellbook rather than the bar
    /// and which we do not ship. The same send carries the drag when that lands.
    PetSetAction {
        pet_guid: u64,
        entries: Vec<(u32, u32)>,
    },
    /// Call the pet off its target (`CMSG_PET_STOP_ATTACK`) — the Attack button's second press.
    PetStopAttack {
        pet_guid: u64,
    },
    /// Cancel one of the **pet's** auras (`CMSG_PET_CANCEL_AURA`, decision 1007) — the pet bar's
    /// press-again-to-cancel, and the pet-shaped twin of [`Self::CancelAura`].
    ///
    /// It is a separate opcode rather than `CancelAura` with a guid because the server needs to
    /// know *whose* aura to drop, and `CMSG_CANCEL_AURA`'s body is a bare spell id. Like its
    /// player twin: no answer packet — the removal arrives as a `UNIT_FIELD_AURA` delta on the
    /// pet, which is also what puts the slot's icon back.
    PetCancelAura {
        pet_guid: u64,
        spell_id: u32,
    },
    /// Flip one pet **spellbook** entry's autocast (`CMSG_PET_SPELL_AUTOCAST` 0x2F3, decision
    /// 1032) — `ToggleSpellAutocast`'s send, and **not** the pet bar's autocast verb.
    ///
    /// The bar's right click is [`Self::PetSetAction`]: it rewrites a *slot's word*, so its body
    /// names a position. This one names a **spell id**, because the book has no positions —
    /// `0x4b4240` looks the id up in `0xb6f098` and hands `0x4bccb0` the id, never an index. Both
    /// end up flipping the same bit on the same word; only one of them can say which word by slot.
    /// No reply packet either way.
    PetSpellAutocast {
        pet_guid: u64,
        spell_id: u32,
        enabled: bool,
    },
    /// Give the pet up permanently (`CMSG_PET_ABANDON`, decision 1066) — the right-click menu's
    /// **Abandon**, and only that row.
    ///
    /// **Its Dismiss is not this**, however alike they read: `PetDismiss 0x4be4d0` opens no packet
    /// and goes down the pet bar's ordinary [`Self::PetAction`] path with the word `0x07000003`.
    /// Both would have worked against vmangos, which is why it is written here.
    ///
    /// No reply: the answer is `SMSG_PET_SPELLS` with a zero guid, and the pet object leaving.
    PetAbandon {
        pet_guid: u64,
    },
    /// Rename the pet (`CMSG_PET_RENAME`, decision 1066) — the `PETRENAMECONFIRM` popup's accept.
    ///
    /// The server may refuse the name outright, so nothing is applied locally; success arrives as a
    /// bumped `UNIT_FIELD_PET_NAME_TIMESTAMP` on the pet, which is what re-asks the name cache.
    PetRename {
        pet_guid: u64,
        name: String,
    },
    /// Start melee auto-attack on `guid` (`CMSG_ATTACKSWING`); echoed as `SMSG_ATTACKSTART`.
    AttackSwing {
        guid: u64,
    },
    /// Stop melee auto-attack (`CMSG_ATTACKSTOP`); echoed as `SMSG_ATTACKSTOP`, whose receive path
    /// drops the attacker's [`crate::creature_anim::Engaged`] stance. Sent when the attack target
    /// is lost — Esc / click-off / target death (ref-observed: losing the target stops the swing
    /// and leaves the attack stance; the weapons stay drawn).
    AttackStop,
    /// Stop our ranged auto-repeat (`CMSG_CANCEL_AUTO_REPEAT_SPELL`, empty body) — the ack every
    /// local cancel sends (the client's sole send site `0x6ea0c6`, inside the cancel routine
    /// `0x6ea080`). vmangos interrupts the held auto-repeat spell; idempotent when the server
    /// already cancelled it first (the cast-result path).
    CancelAutoRepeat,
    /// Cancel a named in-flight cast (`CMSG_CANCEL_CAST`, one `u32` spell id). Sent by the
    /// wand-only auto-repeat handoff before its local cancel (`0x6095b8`) and by the cast bar's
    /// local self-cancel (movement/Esc mid-cast — `ui_cast`'s mirror of the client's
    /// `AbortCast 0x6e4940` send leg).
    CancelCast {
        spell_id: u32,
    },
    /// End our own running channel (`CMSG_CANCEL_CHANNELLING`, one `u32` spell id the server
    /// reads and ignores — the real client still writes it). The channel half of the local
    /// self-cancel (`ui_cast`).
    CancelChannelling {
        spell_id: u32,
    },
    /// Volunteer our sheath state (`CMSG_SETSHEATHED`: 0 stowed · 1 melee · 2 ranged). The real
    /// client auto-sends `1` when initiating melee with weapons stowed (decision 0073, verified
    /// `0x5ecb70` → `0x611cf0`); it lands in our `UNIT_FIELD_BYTES_2`, which drives everyone's
    /// weapon placement — including our own, via the descriptor echo.
    SetSheathed {
        state: u32,
    },
    /// Volunteer our stand state (`CMSG_STANDSTATECHANGE`: 0 stand · 1 sit · 3 sleep · 8 kneel).
    /// The echo into `UNIT_FIELD_BYTES_1` byte 0 drives everyone's sit/stand pose — including our
    /// own, via the descriptor (decision 0080c: the same pattern as sheath).
    StandStateChange {
        state: u32,
    },
    /// The mounted space-bar flourish (`CMSG_MOUNTSPECIAL_ANIM`, empty body). The sender
    /// plays MountSpecial(94) on its own mount locally at send time; the broadcast echo —
    /// present or not by server config — is self-suppressed on receive (decision 0441 P2).
    MountSpecial,
    /// Open a gossip dialog on an NPC (`CMSG_GOSSIP_HELLO`, decision 0081 phase 2): the universal
    /// interact opener — the server's `CanInteractWithNPC` passes `UNIT_NPC_FLAG_NONE`, so it
    /// works on any interactable creature. Sent by the right-click interact route
    /// ([`crate::target`]); answered by `SMSG_GOSSIP_MESSAGE` (a `GossipMenu` event).
    GossipHello {
        guid: u64,
    },
    /// Choose a gossip menu option (`CMSG_GOSSIP_SELECT_OPTION`): `option` is the chosen line's
    /// echoed `index`. v1 never sends a password — coded options are greyed and never selected
    /// (decision 0081), so the writer always omits the trailing code. The server answers a fresh
    /// menu (`SMSG_GOSSIP_MESSAGE`) or closes it (`SMSG_GOSSIP_COMPLETE`).
    GossipSelectOption {
        guid: u64,
        option: u32,
    },
    /// Fetch a gossip menu's greeting text (`CMSG_NPC_TEXT_QUERY`) — auto-sent on menu receipt for
    /// its `text_id`; answered by `SMSG_NPC_TEXT_UPDATE` (an `NpcGreeting` event). Ask-once cached
    /// per `text_id`, like an item template (decision 0081 phase 3).
    NpcTextQuery {
        text_id: u32,
        guid: u64,
    },
    /// Open a vendor's stock (`CMSG_LIST_INVENTORY`, decision 0081 phase 2): the direct opener a
    /// right-click on a vendor-only NPC uses. Answered by `SMSG_LIST_INVENTORY` (a
    /// `VendorInventory` event; the merchant window is phase 4).
    ListInventory {
        guid: u64,
    },
    /// Buy from a vendor (`CMSG_BUY_ITEM`, decision 0081 phase 4): `entry` is the item **template**
    /// id (buy is by entry, not the vendor row's `muid`), `count` the number of stacks. Auto-places
    /// into the first free bag slot; success answers `SMSG_BUY_ITEM` + the item-create path, refusal
    /// `SMSG_BUY_FAILED` (a `VendorBuyFailed` event → the merchant error line).
    BuyItem {
        vendor: u64,
        entry: u32,
        count: u8,
    },
    /// Buy into a **named** container slot (`CMSG_BUY_ITEM_IN_SLOT`, decision 1797) — what a
    /// vendor row dragged off the merchant window and dropped into a bag or an equipment slot
    /// sends, as against [`ClientCommand::BuyItem`]'s auto-place from a plain row click.
    /// `bag_guid` is the container object's guid, or the player's own for the backpack and the
    /// equipment slots. Same refusal shape as `BuyItem` — `SMSG_BUY_FAILED`.
    BuyItemInSlot {
        vendor: u64,
        entry: u32,
        bag_guid: u64,
        bag_slot: u8,
        count: u8,
    },
    /// Sell an item to a vendor (`CMSG_SELL_ITEM`, decision 0081 phase 4's sell affordance):
    /// `item_guid` is the concrete bag item, `count` 0 = the whole stack. Success is silent (the item
    /// vanishes + coinage rises via `UPDATE_OBJECT`); refusal answers `SMSG_SELL_ITEM`'s error shape
    /// (a `VendorSellFailed` event → the merchant error line).
    SellItem {
        vendor: u64,
        item_guid: u64,
        count: u8,
    },
    /// Buy a sold item back (`CMSG_BUYBACK_ITEM`): `slot` is the **absolute** player-array buyback
    /// slot 69–80 (the app maps the clicked 1-based, timestamp-sorted list index to it). Success is
    /// the item re-creating + coinage falling via `UPDATE_OBJECT`; refusal answers
    /// `SMSG_BUY_FAILED`.
    BuybackItem {
        vendor: u64,
        slot: u32,
    },
    /// Repair at a repair-capable vendor (`CMSG_REPAIR_ITEM`): `item_guid` 0 = repair everything,
    /// else the one item. Success is durability rising + coinage falling via `UPDATE_OBJECT`
    /// (no dedicated answer packet).
    RepairItem {
        vendor: u64,
        item_guid: u64,
    },
    /// Accept an innkeeper's bind offer (`CMSG_BINDER_ACTIVATE`, decision 1331): the
    /// `CONFIRM_BINDER` dialog's Accept, carrying the guid `SMSG_BINDER_CONFIRM` asked with. This
    /// is the ONLY packet in the flow that binds anything — selecting the gossip line just raises
    /// the question. Answered by `SMSG_BINDPOINTUPDATE` + `SMSG_PLAYERBOUND` once the innkeeper's
    /// Bind cast lands; declining sends nothing.
    /// File a GM trouble ticket (`CMSG_GMTICKET_CREATE`, decision 1673): the Help window's Submit.
    /// `category` is a `GMTicketCategory.dbc` id (1..10); `map`/`pos` are where the player stands,
    /// which is what a GM's `.ticket go` uses. Answered by `SMSG_GMTICKET_CREATE` — **or by
    /// nothing**, on vmangos's several silent refusals (queue off, under `GMTickets.MinLevel`,
    /// category >= 11). Never retried: the window asks once per click.
    GmTicketCreate {
        category: u8,
        map: u32,
        pos: [f32; 3],
        text: String,
    },
    /// Edit the open ticket's text (`CMSG_GMTICKET_UPDATETEXT`): the window's "Save Changes".
    /// Carries the category byte the real client carries even though vmangos discards it.
    /// **Rate-limited server-side to 2 per world tick, sanction = kick** — one send per click.
    GmTicketUpdate {
        category: u8,
        text: String,
    },
    /// Ask for the open ticket (`CMSG_GMTICKET_GETTICKET`, empty body): `GetGMTicket()`. The
    /// window's own poll — once on world entry, then every 10 minutes while the toast is up.
    GmTicketGet,
    /// Abandon the open ticket (`CMSG_GMTICKET_DELETETICKET`, empty body): the
    /// `HELP_TICKET_ABANDON_CONFIRM` dialog's Yes.
    GmTicketDelete,
    /// Ask whether the petition queue is taking tickets (`CMSG_GMTICKET_SYSTEMSTATUS`, empty
    /// body): `GetGMStatus()`, called from the Help window's OnShow.
    GmTicketSystemStatus,
    BinderActivate {
        binder: u64,
    },
    /// Accept a summon (`CMSG_SUMMON_RESPONSE`, decision 1747): the `CONFIRM_SUMMON` dialog's
    /// Accept, carrying the guid the question arrived with. The **only** packet in the summon
    /// flow — there is no decline opcode, so a refused summon expires on the server's own timer
    /// and this client says nothing.
    SummonResponse {
        summoner: u64,
    },
    /// Unlearn every talent (`MSG_TALENT_WIPE_CONFIRM` outbound, decision 1580): the
    /// `CONFIRM_TALENT_WIPE` dialog's Accept, carrying the guid the trainer's question asked with.
    /// This is the ONLY packet in the flow that resets anything — selecting the gossip line just
    /// raises the question. Answered by the un-learn of every rank spell plus the refreshed
    /// `PLAYER_CHARACTER_POINTS1`; declining sends nothing.
    TalentWipeConfirm {
        trainer: u64,
    },
    // ── The dialog engine's verbs (decision 1963) ──
    /// `ForceLogout()` — `CMSG_PLAYER_LOGOUT` (`0x4A`, empty), the forced flavour of the logout
    /// dispatcher; sent only with a live world session.
    ForceLogout,
    /// `AcceptAreaSpiritHeal()` — `CMSG_AREA_SPIRIT_HEALER_QUEUE` with the cached healer.
    AreaSpiritHealerQueue {
        healer: u64,
    },
    /// The client ADOPTED a new area spirit healer — `CMSG_AREA_SPIRIT_HEALER_QUERY`, asking for
    /// its wave clock. No Lua verb sends this: its only writer is the reference's own
    /// "set current area spirit healer" routine `0x4921c0`, reached from the per-frame proximity
    /// poll and from the spirit-guide click arm ([`crate::ui_dialog_verbs::AreaSpiritHealer`]).
    AreaSpiritHealerQuery {
        healer: u64,
    },
    /// `AcceptBattlefieldPort(index, accept)` — `CMSG_BATTLEFIELD_PORT`: the slot's map id and
    /// the answer as one byte.
    BattlefieldPort {
        map_id: u32,
        accept: bool,
    },
    /// `RequestBattlefieldScoreData()` — `MSG_PVP_LOG_DATA`, empty (decision 1972).
    RequestBattlefieldScoreData,
    /// `LeaveBattlefield()` — `CMSG_LEAVE_BATTLEFIELD`: the active slot's map (decision 1972).
    LeaveBattlefield {
        map_id: u32,
    },
    /// A right-click on a `GAMEOBJECT_TYPE_MEETINGSTONE` that passed the type's four client-side
    /// refusals — `CMSG 0x292`, `u64 goGuid` (decision 2283).
    MeetingStoneJoin {
        go_guid: u64,
    },
    /// `CancelMeetingStoneRequest()` — `CMSG 0x293`, empty.
    MeetingStoneLeave,
    /// The enter-world meeting-stone status query — `CMSG 0x296`, empty (decision 1974).
    MeetingStoneStatusQuery,
    // ── The tutorial system (decision 1976) ──
    /// `FlagTutorial` / an auto-acknowledge site — `CMSG_TUTORIAL_FLAG`, the 0-based id.
    TutorialFlag {
        id: u32,
    },
    /// `ClearTutorials()` — `CMSG_TUTORIAL_CLEAR`, empty.
    TutorialClear,
    /// `ResetTutorials()` — `CMSG_TUTORIAL_RESET`, empty.
    TutorialReset,
    // ── The battleground list window (decision 1974) ──
    /// `ShowBattlefieldList(index)` — `CMSG_BATTLEFIELD_LIST`: the queued slot's map.
    BattlefieldList {
        map_id: u32,
    },
    /// `JoinBattlefield` when the list came from a battlemaster — `CMSG_BATTLEMASTER_JOIN`.
    BattlemasterJoin {
        battlemaster: u64,
        map_id: u32,
        instance_id: u32,
        as_group: bool,
    },
    /// `RequestBattlefieldPositions()` — `MSG_BATTLEGROUND_PLAYER_POSITIONS` outbound (empty;
    /// decision 1980), throttled app-side to the reference's 5000 ms.
    RequestBattlefieldPositions,
    // ── The tabard designer (decision 1977) ──
    /// The NPC-click ladder's TABARDDESIGNER arm — `MSG_TABARDVENDOR_ACTIVATE` out.
    TabardVendorActivate {
        npc: u64,
    },
    /// `TabardModel:Save()` past its pre-flight checks — `MSG_SAVE_GUILD_EMBLEM` out.
    SaveGuildEmblem {
        vendor: u64,
        design: [u32; 5],
    },
    /// The ladder's BATTLEMASTER arm — `CMSG_BATTLEMASTER_HELLO`; the list window answers.
    BattlemasterHello {
        npc: u64,
    },
    /// `JoinBattlefield` when the list arrived without one — `CMSG_BATTLEFIELD_JOIN`.
    BattlefieldJoin {
        map_id: u32,
        instance_id: u32,
        as_group: bool,
    },
    /// The world-enter status request — `CMSG_BATTLEFIELD_STATUS`, empty.
    BattlefieldStatusRequest,
    /// `ConfirmPetUnlearn()` — `CMSG_PET_UNLEARN` with the latched trainer guid.
    PetUnlearn {
        trainer: u64,
    },
    /// Open the bank (`CMSG_BANKER_ACTIVATE`, decision 0604): the direct opener a right-click on
    /// a pure banker (bit 8 the lowest service bit) uses — a gossip-flagged banker routes through
    /// the gossip menu instead, whose bank option makes the server volunteer the same answer.
    /// Answered by `SMSG_SHOW_BANK` (a `ShowBank` event → the bank window opens off local state).
    BankerActivate {
        guid: u64,
    },
    /// Buy the next bank-bag slot (`CMSG_BUY_BANK_SLOT`, decision 0604): the purchase popup's
    /// accept. **No packet on success** — the descriptor's `PLAYER_BYTES_2` bank-bag count + the
    /// falling coinage are the confirmation; refusal answers `SMSG_BUY_BANK_SLOT_RESULT` (a
    /// `BuyBankSlotResult` event → the red error line).
    BuyBankSlot {
        guid: u64,
    },
    /// Deposit: auto-place a bag/doll item into the bank (`CMSG_AUTOBANK_ITEM`, decision 0604) —
    /// the right-click auto-move while the bank window is open. Wire `(bag, slot)`; refusal
    /// answers `SMSG_INVENTORY_CHANGE_FAILURE` (the red line), success moves the item via
    /// `UPDATE_OBJECT` field deltas.
    AutoBankItem {
        bag: u8,
        slot: u8,
    },
    /// Withdraw: auto-place a bank item into the bags (`CMSG_AUTOSTORE_BANK_ITEM`, decision 0604)
    /// — the right-click auto-move on a bank slot. Same wire shape and answers as
    /// [`Self::AutoBankItem`] (vmangos routes by whether the source is a bank position).
    AutoStoreBankItem {
        bag: u8,
        slot: u8,
    },
    /// Ask (or re-ask) a trainer's service list (`CMSG_TRAINER_LIST`, decision 0237): one trainer
    /// guid. The window first opens off the gossip trainer option's `SMSG_TRAINER_LIST`; this is the
    /// *refresh* verb, re-requested after a purchase to repaint the bought row green→gray (the server
    /// does not auto-resend on a buy). Answered by `SMSG_TRAINER_LIST` (a `TrainerList` event).
    TrainerList {
        trainer: u64,
    },
    /// Buy (learn) a trainer service (`CMSG_TRAINER_BUY_SPELL`, decision 0237): the trainer guid + the
    /// service's `spell_id`. Sent by the Train button ([`crate::ui_trainer`]'s buy drain). Success
    /// answers `SMSG_TRAINER_BUY_SUCCEEDED` and delivers the spell via `SMSG_LEARNED_SPELL`; refusal
    /// answers `SMSG_TRAINER_BUY_FAILED` (a `TrainerBuyFailed` event → the window's error line).
    TrainerBuySpell {
        trainer: u64,
        spell_id: u32,
    },
    /// Ask (or re-ask) a stable master's pet list (`MSG_LIST_STABLED_PETS`, decision 1676): one
    /// NPC guid. The window first opens off the gossip stable option's own inbound send of the
    /// same opcode; this is the *refresh* verb, re-requested after every successful mutation
    /// because none of them is answered with a fresh list.
    ListStabledPets {
        npc: u64,
    },
    /// Put the current pet away (`CMSG_STABLE_PET`, decision 1676). **No destination slot** — the
    /// server takes the first free bought one, so there is none to carry.
    StablePet {
        npc: u64,
    },
    /// Summon a stabled pet (`CMSG_UNSTABLE_PET`, decision 1676): the NPC guid + the pet's own
    /// number. Valid only with no current pet; with one out the verb is [`Self::StableSwapPet`].
    UnstablePet {
        npc: u64,
        pet_number: u32,
    },
    /// Trade the current pet for a stabled one in one step (`CMSG_STABLE_SWAP_PET`, decision 1676).
    StableSwapPet {
        npc: u64,
        pet_number: u32,
    },
    /// Buy the next stable slot (`CMSG_BUY_STABLE_SLOT`, decision 1676). The *which* is implicit,
    /// as with the bank's bag slots; a refusal answers `SMSG_STABLE_RESULT`'s `ERR_MONEY`.
    BuyStableSlot {
        npc: u64,
    },
    /// Spend talent points (`CMSG_LEARN_TALENT`, decision 0304): a `Talent.dbc` row id + the
    /// requested rank (0-based, learn-up-to — the click sends the current rank count). Sent by
    /// the talent window's click-to-learn. No dedicated reply: success arrives as the rank
    /// spell's learn effects + the refreshed `PLAYER_CHARACTER_POINTS1`.
    LearnTalent {
        talent_id: u32,
        rank: u32,
    },
    /// Abandon a whole skill line (`CMSG_UNLEARN_SKILL`): the skills pane's unlearn button →
    /// the `UNLEARN_SKILL` popup's accept ([`crate::ui_char`]'s abandon drain). No ack — the
    /// server's `SetSkill(id, 0, 0)` returns as a `PLAYER_SKILL_INFO` field update, which the
    /// skills feed re-pushes (the engine never removes the line locally).
    UnlearnSkill {
        skill_id: u32,
    },
    /// Declare or withdraw war on a faction (`CMSG_SET_FACTION_ATWAR`): the reputation pane's
    /// crossed-swords box. Addressed by reputation-list slot. No ack — the engine already flipped
    /// its own flag copy ([`crate::ui_reputation`]), and vmangos DROPS the request outright while
    /// the player is in combat.
    SetFactionAtWar {
        rep_list_id: u32,
        at_war: bool,
    },
    /// Move a faction into or out of the pane's inactive bucket (`CMSG_SET_FACTION_INACTIVE`).
    /// Same slot addressing and same no-ack rule as [`Self::SetFactionAtWar`].
    SetFactionInactive {
        rep_list_id: u32,
        inactive: bool,
    },
    /// Watch a faction's bar on the main menu bar (`CMSG_SET_WATCHED_FACTION`). **Signed**, and
    /// `-1` — not `0` — is "watch nothing": slot 0 is the Bloodsail Buccaneers. The answer returns
    /// as a `PLAYER_FIELD_WATCHED_FACTION_INDEX` descriptor update.
    SetWatchedFaction {
        rep_list_id: i32,
    },
    /// Use a world GameObject (`CMSG_GAMEOBJ_USE`, decision 0236): a full guid naming the
    /// chest/door/quest-object/lever under the cursor. Sent by the right-click route
    /// ([`crate::target`]) when the nearest hovered CGObject is a usable GameObject. The server fans
    /// it out by GO type — a chest answers with `SMSG_LOOT_RESPONSE` (the loot window), a questgiver
    /// GO with the gossip/quest packets, a door with a `GAMEOBJECT_STATE` flip — or refuses silently.
    GameObjUse {
        guid: u64,
    },
    /// Report walking into an `AreaTrigger.dbc` volume (`CMSG_AREATRIGGER`): the trigger's id.
    /// Sent by [`crate::area_trigger`]'s per-frame containment check — the client's whole part in
    /// the system. The server answers a teleport trigger with the ordinary
    /// `SMSG_TRANSFER_PENDING`/`SMSG_NEW_WORLD` pair (or a same-map `MSG_MOVE_TELEPORT_ACK`), a
    /// refused one with `SMSG_AREA_TRIGGER_MESSAGE`, and most with nothing at all.
    AreaTrigger {
        trigger_id: u32,
    },
    /// Ask a GameObject's template (`CMSG_GAMEOBJECT_QUERY`, decision 0239): `entry` + the asking
    /// `guid`. Sent ask-once when a GameObject streams in ([`crate::go_templates`]); the answer's
    /// `lockId` decides whether a right-click uses it or casts an OPEN_LOCK spell.
    GameObjectQuery {
        entry: u32,
        guid: u64,
    },
    /// Ask for one page of a book (`CMSG_PAGE_TEXT_QUERY`, decision 1105): the `PageText` id + the
    /// asking object's `guid` (an item's or a TEXT GameObject's — the server discards it). Sent
    /// ask-once by [`crate::ui_item_text::PageTexts`] when a reader opens on a page it hasn't got;
    /// vmangos answers with the whole forward chain, one `SMSG_PAGE_TEXT_QUERY_RESPONSE` per page.
    PageTextQuery {
        page_id: u32,
        guid: u64,
    },
    /// Cast an OPEN_LOCK spell at a **GameObject** (`CMSG_CAST_SPELL`, decision 0239): the right-click
    /// on a locked chest / mining vein / herb node. The server runs `EffectOpenLock` → a chest opens
    /// its loot (`SMSG_LOOT_RESPONSE`); the profession/skill gate is the server's.
    CastSpellGameObject {
        spell_id: u32,
        go_guid: u64,
    },
    /// An item-targeted cast (`TARGET_FLAG_ITEM` + packed guid) — the CraftFrame enchant pick
    /// (decision 0437 phase 3).
    CastSpellItem {
        spell_id: u32,
        item_guid: u64,
    },
    /// Open a corpse/creature's loot (`CMSG_LOOT`, decision 0084): a full guid naming the lootable
    /// unit. Sent by the right-click loot route ([`crate::target`]) on the loot classification
    /// (a dead unit carrying `UNIT_DYNFLAG_LOOTABLE`). Answered by `SMSG_LOOT_RESPONSE` (a
    /// `LootResponse` event on success, `LootError` on refusal). Not usable on a GameObject guid —
    /// the server rejects those; a GameObject loots via `GameObjUse` above.
    Loot {
        guid: u64,
    },
    /// Take one loot row into the bags (`CMSG_AUTOSTORE_LOOT_ITEM`): `slot` is the **wire** loot slot
    /// (0-based, from `SMSG_LOOT_RESPONSE`), which the loot drain maps from the clicked 1-based
    /// display row. The server auto-places into the first free bag slot; success answers
    /// `SMSG_LOOT_REMOVED` + `SMSG_ITEM_PUSH_RESULT` + the item-create path.
    AutostoreLootItem {
        slot: u8,
    },
    /// Take the loot's coin pile (`CMSG_LOOT_MONEY`, empty body): the intent a click on the
    /// synthesized coin row queues. Answered by `SMSG_LOOT_CLEAR_MONEY` (+ the coinage rising via
    /// `UPDATE_OBJECT`; solo looting gets no `SMSG_LOOT_MONEY_NOTIFY` on this server).
    LootMoney,
    /// Close the loot window (`CMSG_LOOT_RELEASE`, decision 0084): the server ignores `guid` and
    /// releases whatever loot it has stored for us. Sent on `CloseLoot` (the window's `OnHide`).
    /// Answered by `SMSG_LOOT_RELEASE_RESPONSE` (a `LootReleaseResponse` event).
    LootRelease {
        guid: u64,
    },
    /// Cast a group-loot vote (`CMSG_LOOT_ROLL`, decision 0591): the Need/Greed/Pass click on a
    /// `GroupLootFrame`. The roll is addressed by the `(looted_target, item_slot)` pair the server
    /// opened it with — the client-internal `rollID` the Lua side uses never reaches the wire.
    /// Answered by an `SMSG_LOOT_ROLL` echo of our vote, then the roll's resolution.
    LootRoll {
        looted_target: u64,
        item_slot: u32,
        roll_type: u8,
    },
    /// Hand a loot row to a group member (`CMSG_LOOT_MASTER_GIVE`, decision 1675): the master
    /// looter's dropdown pick. `guid` is the open loot source, `slot` the row's **wire** slot, and
    /// `target` the recipient's guid. Success arrives as an ordinary `SMSG_LOOT_REMOVED` (the item
    /// traffic goes to the recipient, not to us); a refusal as a `MASTER_*` loot error.
    LootMasterGive {
        guid: u64,
        slot: u8,
        target: u64,
    },
    /// Look at an available quest (`CMSG_QUESTGIVER_QUERY_QUEST`, decision 0088): a greeting/gossip
    /// quest row click. Answered by `SMSG_QUESTGIVER_QUEST_DETAILS` (a `QuestDetail` event → the
    /// accept panel).
    QuestgiverQuery {
        npc: u64,
        quest: u32,
    },
    /// Accept a quest (`CMSG_QUESTGIVER_ACCEPT_QUEST`): the detail panel's Accept button. Adds it to
    /// the log; the server closes the gossip window (`SMSG_GOSSIP_COMPLETE`).
    QuestgiverAccept {
        npc: u64,
        quest: u32,
    },
    /// Ask a quest's turn-in progress panel (`CMSG_QUESTGIVER_COMPLETE_QUEST`): an active greeting/
    /// gossip quest row click. Answered by `SMSG_QUESTGIVER_REQUEST_ITEMS` (a `QuestProgress` event),
    /// or OFFER_REWARD when there are no required items.
    QuestgiverComplete {
        npc: u64,
        quest: u32,
    },
    /// Advance from the progress panel to the reward panel (`CMSG_QUESTGIVER_REQUEST_REWARD`): the
    /// progress panel's Continue button. Answered by `SMSG_QUESTGIVER_OFFER_REWARD` (a `QuestOffer`
    /// event).
    QuestgiverRequestReward {
        npc: u64,
        quest: u32,
    },
    /// Choose a reward and finish the quest (`CMSG_QUESTGIVER_CHOOSE_REWARD`, `choice` = the 0-based
    /// choice index): the reward panel's Complete button. Answered by `SMSG_QUESTGIVER_QUEST_COMPLETE`
    /// (a `QuestComplete` event) + the XP/money/item grants via `UPDATE_OBJECT`.
    QuestgiverChooseReward {
        npc: u64,
        quest: u32,
        choice: u32,
    },
    /// Ask a quest's full template (`CMSG_QUEST_QUERY`, the quest-log slice, decision 0088's
    /// deferred second half): the log window's ask-once detail source, distinct from
    /// `QuestgiverQuery` (which needs an NPC guid, not just the quest id). Answered by
    /// `SMSG_QUEST_QUERY_RESPONSE` (a `QuestTemplate` event) into the [`crate::ui_quest_log::QuestLog`]
    /// cache.
    QuestQuery {
        quest: u32,
    },
    /// Ask an NPC's questgiver dialog status (`CMSG_QUESTGIVER_STATUS_QUERY`) — the overhead
    /// `!`/`?` marker's value, answered by `SMSG_QUESTGIVER_STATUS`.
    QuestgiverStatusQuery {
        npc: u64,
    },
    /// Greet a questgiver (`CMSG_QUESTGIVER_HELLO`) — the quest session's END on a non-gossip
    /// NPC, which is what re-opens its quest list after a decline (decision 1738). The gossip
    /// twin of this send is [`Self::GossipHello`].
    QuestgiverHello {
        npc: u64,
    },
    /// Abandon a quest-log slot (`CMSG_QUESTLOG_REMOVE_QUEST`): the log window's confirmed abandon
    /// (`AbandonQuest()`'s two-step). No ack SMSG — the server clears the `PLAYER_QUEST_LOG` slot
    /// fields directly, which the next feed pass reads as the slot going empty.
    QuestlogRemove {
        slot: u8,
    },
    // ── The party quest-share (decision 1733; bodies in benilla-protocol `messages/quest/share.rs`). ──
    /// Share the selected quest with the party (`CMSG_PUSHQUESTTOPARTY`) — the quest log's *Share
    /// Quest* button. The server walks the group itself; the answers come back as one
    /// `MSG_QUEST_PUSH_RESULT` per member (usually two: the opener, then the outcome).
    PushQuestToParty {
        quest: u32,
    },
    /// Answer the escort confirm with Yes (`CMSG_QUEST_CONFIRM_ACCEPT`) — a
    /// `QUEST_FLAGS_PARTY_ACCEPT` quest a party member started. There is no No command: the
    /// reference sends nothing at all when the popup is dismissed.
    QuestConfirmAccept {
        quest: u32,
    },
    /// Report our verdict on a quest a party member shared with us (`MSG_QUEST_PUSH_RESULT`) —
    /// today only the decline; every other verdict on the share is the server's own. `sharer` is
    /// the giver guid the shared `SMSG_QUESTGIVER_QUEST_DETAILS` arrived under.
    QuestPushResult {
        sharer: u64,
        msg: benilla_protocol::messages::QuestShareMsg,
    },
    // ── The mail arc (decision 0544; writer bodies in benilla-protocol `world/writer/mail.rs`). ──
    /// Ask the mailbox's inbox page (`CMSG_GET_MAIL_LIST`) — the window's open verb and its
    /// refresh. Answered by `SMSG_MAIL_LIST_RESULT` (a `MailList` event).
    GetMailList {
        mailbox: u64,
    },
    /// Send a mail (`CMSG_SEND_MAIL`): recipient/subject/body, an optional item attachment
    /// (`item_guid == 0` = none), money, and COD. Answered by `SMSG_SEND_MAIL_RESULT`.
    SendMail {
        mailbox: u64,
        receiver: String,
        subject: String,
        body: String,
        /// The selected `Stationery.dbc` id (`CMSG_SEND_MAIL`'s sixth field, 1970).
        stationery: u32,
        item_guid: u64,
        money: u32,
        cod: u32,
    },
    /// Take a mail's attached money (`CMSG_MAIL_TAKE_MONEY`).
    MailTakeMoney {
        mailbox: u64,
        mail_id: u32,
    },
    /// Take a mail's attached item (`CMSG_MAIL_TAKE_ITEM`).
    MailTakeItem {
        mailbox: u64,
        mail_id: u32,
    },
    /// Mark a mail read (`CMSG_MAIL_MARK_AS_READ`) — sent when a letter opens. No response packet.
    MailMarkAsRead {
        mailbox: u64,
        mail_id: u32,
    },
    /// Return a mail to its sender (`CMSG_MAIL_RETURN_TO_SENDER`).
    MailReturnToSender {
        mailbox: u64,
        mail_id: u32,
    },
    /// Delete a mail (`CMSG_MAIL_DELETE`) — the taken-letter close and the explicit Delete button
    /// alike.
    MailDelete {
        mailbox: u64,
        mail_id: u32,
    },
    /// Make a permanent copy of a letter's body (`CMSG_MAIL_CREATE_TEXT_ITEM`) — the open letter's
    /// attachment-row letter button. Answered by `SMSG_SEND_MAIL_RESULT` (action MADE_PERMANENT).
    MailCreateTextItem {
        mailbox: u64,
        mail_id: u32,
    },
    /// Fetch a letter's body text (`CMSG_ITEM_TEXT_QUERY`) — sent once per mail whose
    /// `item_text_id != 0`. Answered by `SMSG_ITEM_TEXT_QUERY_RESPONSE` (a `MailItemText` event).
    ItemTextQuery {
        text_id: u32,
        mail_id: u32,
    },
    /// Ask whether unread mail is waiting (`MSG_QUERY_NEXT_MAIL_TIME`, empty body). Sent once at
    /// login to seed `HasNewMail()`/the minimap letter icon (decision 0544 P3, sent by
    /// `crate::ui_mail`'s world-enter one-shot).
    QueryNextMailTime,
    // ── The auction house arc (decision 1511; writer bodies in benilla-protocol
    //    `world/writer/auction.rs`). Every verb carries the auctioneer guid — the server
    //    re-checks the 5 yd interact distance on each one, so there is no session token, and
    //    several refusals (a zero bid/duration, an unaffordable bid, a cancel whose cut can't be
    //    paid, a list request while one is in flight) come back as NO packet at all. ──────────
    //
    //    Every one of these is constructed by `crate::ui_auction`. 1511 P0 landed the wire
    //    first, under a per-variant `#[allow(dead_code)]` that was to come off the moment its
    //    caller landed; the window came and the allows stayed, so rustc could no longer judge
    //    them. A stale allow is a lie the compiler cannot catch — none here, by design.
    /// Greet an auctioneer (`MSG_AUCTION_HELLO`, one guid) — the two-way opcode whose *reply*
    /// (an `AuctionHello` event, carrying the `AuctionHouse.dbc` house id) opens the window.
    AuctionHello {
        auctioneer: u64,
    },
    /// Ask a Browse page (`CMSG_AUCTION_LIST_ITEMS`): the search filters exactly as the server
    /// reads them, with `benilla_protocol::messages::auction_filter`'s sentinels for the unset
    /// ones. **No sort rides the wire** — sorting the page is ours. `list_from` pages by 50.
    /// Answered by `SMSG_AUCTION_LIST_RESULT` (an `AuctionListResult` event).
    AuctionListItems {
        auctioneer: u64,
        list_from: u32,
        searched_name: String,
        level_min: u8,
        level_max: u8,
        slot_id: u32,
        main_category: u32,
        sub_category: u32,
        quality: u32,
        usable: u8,
    },
    /// Ask the Auctions tab page (`CMSG_AUCTION_LIST_OWNER_ITEMS`) — our own listings.
    AuctionListOwnerItems {
        auctioneer: u64,
        list_from: u32,
    },
    /// Ask the Bid tab page (`CMSG_AUCTION_LIST_BIDDER_ITEMS`). `auction_ids` is a **refresh
    /// set**, not a filter: those rows are emitted first, then every auction we currently hold the
    /// bid on. Empty for a plain page.
    AuctionListBidderItems {
        auctioneer: u64,
        list_from: u32,
        auction_ids: Vec<u32>,
    },
    /// List an item for auction (`CMSG_AUCTION_SELL_ITEM`) — the Create Auction pane.
    /// `etime_minutes` must be 120, 480 or 1440; the deposit leaves the purse immediately.
    AuctionSellItem {
        auctioneer: u64,
        item_guid: u64,
        bid: u32,
        buyout: u32,
        etime_minutes: u32,
    },
    /// Bid on — or buy out — an auction (`CMSG_AUCTION_PLACE_BID`). One verb for both: a `price`
    /// at or above a nonzero buyout *is* the buyout, inferred server-side.
    AuctionPlaceBid {
        auctioneer: u64,
        auction_id: u32,
        price: u32,
    },
    /// Cancel one of our own auctions (`CMSG_AUCTION_REMOVE_ITEM`). The deposit is forfeit, and a
    /// cancel on an auction that already has a bid costs the 5% cut out of pocket.
    AuctionRemoveItem {
        auctioneer: u64,
        auction_id: u32,
    },
    /// Ask for the server's wall clock (`CMSG_QUERY_TIME`, empty body) — sent on every world
    /// entry, answered by `SMSG_QUERY_TIME_RESPONSE` into [`ServerWallClock`]. That clock is the
    /// only way to read the absolute deadlines the server writes into descriptor fields, which is
    /// how the timed-quest countdown gets its number (decision 1150).
    QueryTime,
    /// Ask to inspect a player (`CMSG_INSPECT`, `u64 target`) — the UnitPopup INSPECT row
    /// (decision 0631). Fire-and-forget: the reply echoes the guid and nothing else, and the window
    /// paints from the already-streamed PUBLIC `PLAYER_VISIBLE_ITEM_*` fields. Sent anyway because
    /// server-side it also sets our selection (`MiscHandler.cpp:945`), as the real client's does.
    Inspect {
        target: u64,
    },
    /// Ask for a player's honor stats (`MSG_INSPECT_HONOR_STATS`, `u64 target`) — the inspect
    /// window's Honor tab (decision 1512). Unlike [`Self::Inspect`] this one is a **real** round
    /// trip: the reply rides the same opcode and is the only source of another player's honor
    /// numbers, since every field of the honor block is PRIVATE. A refusal is silence — the server
    /// applies the same three gates as `CMSG_INSPECT` and simply does not answer.
    InspectHonorStats {
        target: u64,
    },
    // ── The player-trade arc (decision 0592; writer bodies in benilla-protocol
    //    `world/writer/trade.rs`). ─────────────────────────────────────────────────────────────
    /// Offer to trade with a player (`CMSG_INITIATE_TRADE`, `u64 target`) — the UnitPopup TRADE row.
    /// The server answers the initiator on any refusal (`SMSG_TRADE_STATUS`) and, on success, sends
    /// the *target* `BEGIN_TRADE`.
    InitiateTrade {
        target: u64,
    },
    /// **Accept** an incoming trade request (`CMSG_BEGIN_TRADE`, empty) — makes the server emit
    /// `OPEN_WINDOW` to both sides. Sent by [`crate::ui_trade::answer_trade_request`], never by the
    /// net drain: *whether* to accept is a policy with five inputs and (decision 1764) the player's
    /// own answer, so the arm that decodes `BEGIN_TRADE` only records the request.
    BeginTrade,
    /// Decline an incoming trade request as **ignored** (`CMSG_IGNORE_TRADE`, `0x119`, empty) —
    /// leg 2 of the reference's ladder (`0x4bf759` → `0x5d41c0`, its one call site image-wide),
    /// and the only leg that answers on a different opcode. vmangos turns it into
    /// `TRADE_STATUS_IGNORE_YOU`, so the initiator reads "… is ignoring you" rather than "busy".
    IgnoreTrade,
    /// **Decline** an incoming trade request as busy (`CMSG_BUSY_TRADE`, `0x118`, empty) — the
    /// reference's own refusal sender (`0x5d4150`), which vmangos turns into `TRADE_STATUS_BUSY`
    /// to *both* sides. Five of the ladder's eight refusal legs go out on this opcode (decision
    /// 1764); the ignore leg uses [`Self::IgnoreTrade`], two legs send nothing at all, and the
    /// **player's** own No is [`Self::CancelTrade`], not this.
    BusyTrade,
    /// Press the Trade button (`CMSG_ACCEPT_TRADE`) — the accept half of the two-sided confirm.
    AcceptTrade,
    /// Drop your own accept but stay in the trade (`CMSG_UNACCEPT_TRADE`).
    UnacceptTrade,
    /// Cancel/close the trade (`CMSG_CANCEL_TRADE`, empty) — the window's close/cancel verb.
    CancelTrade,
    /// Offer this many copper on our side (`CMSG_SET_TRADE_GOLD`, `u32`) — the money input's
    /// value-changed callback (decision 0592 P2). Clears both accepts + re-arms the 200 ms scam delay
    /// server-side; the server echoes our new gold back as `SMSG_TRADE_STATUS_EXTENDED`.
    SetTradeGold {
        copper: u32,
    },
    /// Place the item at inventory (`bag`, `slot`) into our trade slot `trade_slot` (0-based, 0..=6;
    /// 6 = the non-traded/enchant slot) — `CMSG_SET_TRADE_ITEM`, a bag item dropped onto the slot
    /// (decision 0592 P2). The server echoes the filled slot back as `SMSG_TRADE_STATUS_EXTENDED`.
    SetTradeItem {
        trade_slot: u8,
        bag: u8,
        slot: u8,
    },
    /// Clear our trade slot `trade_slot` (0-based) — `CMSG_CLEAR_TRADE_ITEM`, an empty-cursor click on
    /// a filled slot (decision 0592 P2).
    ClearTradeItem {
        trade_slot: u8,
    },
    /// Leave the world back to character select (`CMSG_LOGOUT_REQUEST`, the `/logout` command —
    /// decision 0193). The server answers `SMSG_LOGOUT_COMPLETE` (instant for a resting/GM
    /// character), which the IO thread turns into a [`LoggedOutMessage`] + an immediate
    /// reconnect whose fresh roster is the select screen.
    Logout,
    /// Call off a pending logout (`CMSG_LOGOUT_CANCEL`) — the CAMP/QUIT dialog's Cancel, decision
    /// 0674. Only the non-instant logout has anything to cancel; the server acks with
    /// `SMSG_LOGOUT_CANCEL_ACK`, which becomes the UI's `LOGOUT_CANCEL` event.
    LogoutCancel,
    /// Join a chat channel (`CMSG_JOIN_CHANNEL` — `/join`, and the 0288 P6 zone auto-join).
    JoinChannel {
        name: String,
        password: String,
    },
    /// Leave a chat channel (`CMSG_LEAVE_CHANNEL` — `/leave`).
    LeaveChannel {
        name: String,
    },
    /// Ask a channel's roster (`CMSG_CHANNEL_LIST` — `/chatlist <name>`).
    ChannelList {
        name: String,
    },
    /// `/random [min] [max]` (`MSG_RANDOM_ROLL`).
    RandomRoll {
        min: u32,
        max: u32,
    },
    /// `DisplayChannelOwner` — `CMSG_CHANNEL_OWNER`.
    ChannelOwner {
        name: String,
    },
    /// `SetChannelOwner` — `CMSG_CHANNEL_SET_OWNER`.
    ChannelSetOwner {
        name: String,
        player: String,
    },
    /// `SetChannelPassword` — `CMSG_CHANNEL_PASSWORD` (an empty password clears it).
    ChannelPassword {
        name: String,
        password: String,
    },
    ChannelModerator {
        name: String,
        player: String,
    },
    ChannelUnmoderator {
        name: String,
        player: String,
    },
    ChannelMute {
        name: String,
        player: String,
    },
    ChannelUnmute {
        name: String,
        player: String,
    },
    ChannelInvite {
        name: String,
        player: String,
    },
    ChannelKick {
        name: String,
        player: String,
    },
    ChannelBan {
        name: String,
        player: String,
    },
    ChannelUnban {
        name: String,
        player: String,
    },
    /// `ChannelToggleAnnouncements` — `CMSG_CHANNEL_ANNOUNCEMENTS`.
    ChannelAnnouncements {
        name: String,
    },
    /// `ChannelModerate` — `CMSG_CHANNEL_MODERATE`.
    ChannelModerate {
        name: String,
    },
    /// `/played` (`CMSG_PLAYED_TIME`).
    PlayedTime,
    /// Acknowledge a triggered cinematic as finished (`CMSG_COMPLETE_CINEMATIC`) — sent by
    /// [`crate::cinematic`] when playback ends or the player ESCs out of it, and immediately for a
    /// trigger it cannot resolve to a shot. Without the ack, vmangos anchors object visibility to
    /// the flying cinematic camera (a first login's race intro) and every NPC around the body
    /// despawns until relog (decision 0196).
    CompleteCinematic,
    /// Announce the shot now being armed (`CMSG_NEXT_CINEMATIC_CAMERA`) — see
    /// [`benilla_protocol::WorldWriter::next_cinematic_camera`]. Sent for **every** shot, the
    /// first included: the reference's send is inside the shot arm (`0x48ef11`), not the shot
    /// advance, so a shipped single-camera race intro sends exactly one. See
    /// [`crate::cinematic`]'s module doc.
    NextCinematicCamera,
    /// **Acknowledge a granted mover mode** — root, water-walk, feather-fall or hover (decisions
    /// 0308, 0866): the echoed `counter` + our live pose, on the opcode
    /// [`MoveMode::ack_opcode`] picks. Sent by the controller the frame the
    /// [`MoveModeMessage`] lands; un-acked, the server never applies the change and observers never
    /// see it.
    ///
    /// `flags` must already carry the applied mode bit — the controller ORs it in before sending
    /// (vmangos KICKS a root-apply ack whose `MovementInfo` lacks `MOVEFLAG_ROOT`, and reads the
    /// word as the mover's new flags for the other three).
    MoveModeAck {
        guid: u64,
        counter: u32,
        mode: MoveMode,
        apply: bool,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
    },
    /// **Acknowledge the knockback we just flew** (`CMSG_MOVE_KNOCK_BACK_ACK`, decision 1702): the
    /// echoed `counter`, our live pose/flags, and `launch` — the server's own quad, echoed back as
    /// the `MovementInfo` jump tail.
    ///
    /// Sent by the controller on the frame the mover actually takes off, never on receipt, and never
    /// if the launch was refused. That order is the reference's: the knockback is queued as a
    /// due-timed record, and the frame update drains it **apply-then-send inside one call**
    /// (`0x61624d call 0x6179c0` → `0x616261 push 0xf0`), so the `MovementInfo` on the wire is
    /// always the post-launch one.
    KnockBackAck {
        guid: u64,
        counter: u32,
        launch: JumpInfo,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        transport: Option<TransportPose>,
    },
    /// Release the spirit (`CMSG_REPOP_REQUEST`, empty body) — the DEATH popup's Release Spirit
    /// (and its timeout's server-forced twin is server-side; decision 0308 slice 1).
    RepopRequest,
    /// Ask where our corpse is (`MSG_CORPSE_QUERY`, empty request) — on becoming a ghost and on
    /// login-while-dead; answered into [`crate::death::DeathNet`] (decision 0308 §5).
    CorpseQuery,
    /// Reclaim our corpse (`CMSG_RECLAIM_CORPSE` — RECOVER_CORPSE's Accept). Server-gated to
    /// ghost + delay elapsed + 39 yd; success returns as ordinary descriptor deltas.
    ReclaimCorpse {
        corpse: u64,
    },
    /// Self-resurrect (`CMSG_SELF_RES`, empty body — the DEATH popup's soulstone/Reincarnation
    /// button, decision 1746). The server casts whatever `PLAYER_SELF_RES_SPELL` holds; success
    /// returns as ordinary descriptor deltas, no answer packet.
    SelfRes,
    /// Take the spirit healer's resurrection (`CMSG_SPIRIT_HEALER_ACTIVATE` — the XP_LOSS
    /// confirm's final Accept): 50% res, 25% durability, sickness at level ≥ 11.
    SpiritHealerActivate {
        npc: u64,
    },
    /// Answer a resurrection offer (`CMSG_RESURRECT_RESPONSE` — the RESURRECT popups).
    ResurrectResponse {
        caster: u64,
        accept: bool,
    },
    // ── The group/party family (decision 0434; writer bodies in benilla-protocol
    //    `world/writer/group.rs`) ──────────────────────────────────────────────────────────────
    /// Invite a player by name (`CMSG_GROUP_INVITE` — `/invite`, the unit menus, the invite ack
    /// comes back as `SMSG_PARTY_COMMAND_RESULT`).
    GroupInvite {
        name: String,
    },
    /// Accept the pending group invite (`CMSG_GROUP_ACCEPT` — the PARTY_INVITE popup's Accept).
    GroupAccept,
    /// Decline the pending group invite (`CMSG_GROUP_DECLINE` — Decline/timeout/escape).
    GroupDecline,
    /// Kick a member by name (`CMSG_GROUP_UNINVITE` — `/uninvite`, leader only).
    GroupUninvite {
        name: String,
    },
    /// Hand leadership to a member (`CMSG_GROUP_SET_LEADER` — `/promote`, leader only).
    GroupSetLeader {
        guid: u64,
    },
    /// Leave the group (`CMSG_GROUP_DISBAND` — the wire's leave verb, despite the name).
    GroupLeave,
    /// Convert the party into a raid (`CMSG_GROUP_RAID_CONVERT`, leader only; echoes back as a
    /// `SMSG_GROUP_LIST` with `group_type` raid).
    GroupRaidConvert,
    /// Ask the server for one party/raid member's whole stat block
    /// (`CMSG_REQUEST_PARTY_MEMBER_STATS`; answered by `SMSG_PARTY_MEMBER_STATS_FULL`).
    ///
    /// **Sent from exactly two edges, both the reference's** (decision 1640, `0x4e8646` and
    /// `0x4e83f1`): the moment a member's object leaves the object manager
    /// (`ui_party::net::member_deactivated`), and the GROUP_LIST seat of a member new to the roster
    /// whose object we do not hold (`seat_new_records`). There is no timer and no Lua binding — a
    /// periodic poll would be ours, not the client's.
    RequestPartyMemberStats {
        guid: u64,
    },
    /// Set the loot rules (`CMSG_LOOT_METHOD`): `method` 0..4, `master` guid (master loot only),
    /// `threshold` quality 2..4. Leader only; echoes back as a fresh `SMSG_GROUP_LIST`.
    LootMethod {
        method: u32,
        master: u64,
        threshold: u32,
    },
    /// Mark a unit with a raid-target icon (`MSG_RAID_TARGET_UPDATE` outbound — the popup's
    /// submenu, decision 0434 §5): wire `icon` 0..7, `guid` 0 clears that icon's slot. Leader/
    /// assistant only server-side; echoes back as the delta form.
    SetRaidTarget {
        icon: u8,
        guid: u64,
    },
    /// `MSG_MINIMAP_PING` (a minimap click, decision 1596): raw world `(x, y)`. The server relays
    /// them verbatim to the rest of the group and does nothing at all when we are solo — which is
    /// why the marker is drawn locally at click time rather than awaited off the wire.
    MinimapPing {
        x: f32,
        y: f32,
    },
    // ── The raid-management family (decision 1549's Raid tab) ─────────────────────────────────
    /// Move a raid member to another subgroup (`CMSG_GROUP_CHANGE_SUB_GROUP`): the member's NAME
    /// and the **0-based** subgroup the wire wants. Leader/assistant only; echoes as a fresh
    /// `SMSG_GROUP_LIST`.
    GroupChangeSubGroup {
        name: String,
        group: u8,
    },
    /// Trade two raid members' subgroups (`CMSG_GROUP_SWAP_SUB_GROUP`, two names).
    GroupSwapSubGroup {
        name: String,
        other: String,
    },
    /// Grant or revoke the raid-assistant flag (`CMSG_GROUP_ASSISTANT_LEADER`, leader only).
    GroupAssistantLeader {
        guid: u64,
        grant: bool,
    },
    /// Start a ready check (`MSG_RAID_READY_CHECK`, empty body — leader only).
    ReadyCheckStart,
    /// Answer a ready check (`MSG_RAID_READY_CHECK`, one byte).
    ReadyCheckAnswer {
        ready: bool,
    },
    /// Ask for our saved raid lockouts (`CMSG_REQUEST_RAID_INFO`) — the Raid Info panel.
    RequestRaidInfo,
    /// Reset every dungeon we own (`CMSG_RESET_INSTANCES`, empty body) — the SELF menu's "Reset
    /// all instances" (decision 1748). Answered per instance: an `SMSG_INSTANCE_RESET` for each
    /// map that reset, an `SMSG_INSTANCE_RESET_FAILED` for each that would not.
    ResetInstances,
    // ── The duel family (decision 0633; writer bodies in benilla-protocol
    //    `world/writer/duel.rs`). Challenging is a `CastSpell` of the duel spell, not a verb here.
    /// Accept a duel challenge (`CMSG_DUEL_ACCEPTED`) — the popup's Accept, and the challenger's
    /// own auto-accept the instant its request echoes back.
    DuelAccepted {
        arbiter: u64,
    },
    /// Decline / cancel / forfeit a duel (`CMSG_DUEL_CANCELLED`) — one opcode for all three; the
    /// server reads the intent from the duel's state.
    DuelCancelled {
        arbiter: u64,
    },
    // ── The social family (decision 0668; writer bodies in benilla-protocol
    //    `world/writer/social.rs`). Note the wire's own asymmetry: add by NAME, remove by GUID.
    /// Refresh the friend list (`CMSG_FRIEND_LIST`) — the FrameXML's `ShowFriends()`. The list
    /// also arrives unasked at login, so this is never the only path.
    FriendListRequest,
    /// Befriend a character by name (`CMSG_ADD_FRIEND`).
    AddFriend {
        name: String,
    },
    /// The client's LFG slots and comment (`CMSG_SET_LOOKING_FOR_GROUP`), sent by
    /// `SetLookingForGroup` only when the commit changed something (1961).
    SetLookingForGroup {
        slots: [u32; 3],
        comment: String,
    },
    /// Drop a friend by guid (`CMSG_DEL_FRIEND`) — the caller resolves the name first.
    DelFriend {
        guid: u64,
    },
    /// Ignore a character by name (`CMSG_ADD_IGNORE`).
    AddIgnore {
        name: String,
    },
    /// Stop ignoring, by guid (`CMSG_DEL_IGNORE`).
    DelIgnore {
        guid: u64,
    },
    /// Tell the server we dropped an ignored player's whisper (`CMSG_CHAT_IGNORED`) so it can
    /// answer them "X is ignoring you". The drop itself is entirely client-side — the server
    /// keeps delivering an ignored player's chat, which is why the client has to filter *and*
    /// report.
    ChatIgnored {
        guid: u64,
    },
    /// Run a `/who` (`CMSG_WHO`) — the filter string is already parsed into wire fields
    /// (`ui_social::who_query`, which needs the DBCs the parse resolves names against).
    Who {
        request: Box<WhoRequest>,
    },
    /// Ask to flip our own PvP flag (`CMSG_TOGGLE_PVP`, empty body — decision 0646): `/pvp` and
    /// the unit popup's PvP row, both through the VM's intent queue. Nothing local changes; the
    /// answer is the descriptor's PvP bit (and flagging *off* waits out the server's 300 s timer).
    TogglePvp,
    /// Ask to flip our own show-helm preference (`CMSG_TOGGLE_HELM`, empty body — decision 1472):
    /// the Options window's *Show Helm* row, through the VM's intent queue. Nothing local changes;
    /// the answer is `PLAYER_FLAGS`' `HIDE_HELM` bit, which is what dresses the body — ours and
    /// every other player's, since the field is public.
    ToggleHelm,
    /// The cloak half of [`Self::ToggleHelm`] (`CMSG_TOGGLE_CLOAK`, empty body).
    ToggleCloak,
    // ── The guild family (writer bodies in benilla-protocol `world/writer/guild.rs`) ──────────
    //
    //    Three things shape every caller of this band. **Members are addressed by NAME**, not by
    //    guid as the friend list is. **Rank ids are 0-based with 0 = guild master**, and authority
    //    falls as the id rises (promote = `rank - 1`). And **almost nothing is acked
    //    individually**: the mutating verbs answer with a whole fresh `SMSG_GUILD_ROSTER` (plus an
    //    `SMSG_GUILD_QUERY_RESPONSE` for the rank verbs), so the model updates when that snapshot
    //    lands, never optimistically at the send; a refusal arrives separately as
    //    `SMSG_GUILD_COMMAND_RESULT`.
    //
    //    Every one of these is sent by `crate::ui_guild` (decision 1257) except `GuildCreate`,
    //    which keeps its `#[allow(dead_code)]`: *founding* a guild is the charter/petition flow,
    //    deliberately out of the membership slice, and vmangos registers the opcode `STATUS_NEVER`
    //    so nothing would answer it anyway.
    /// Ask a guild's public identity by id (`CMSG_GUILD_QUERY`) — the ask-once cache fill behind
    /// every "which guild is that?" (a roster row, a `/who` hit, a guild chat line).
    GuildQuery {
        guild_id: u32,
    },
    /// Found a guild by name (`CMSG_GUILD_CREATE`). vmangos registers the opcode `STATUS_NEVER` —
    /// at 1.12 founding runs through the charter flow — so this draws no reply.
    #[allow(dead_code)]
    GuildCreate {
        name: String,
    },
    /// Invite a character into our guild (`CMSG_GUILD_INVITE`).
    GuildInvite {
        name: String,
    },
    /// Accept the guild invitation we are holding (`CMSG_GUILD_ACCEPT`, empty body) — which one is
    /// the server's pending state, not a field.
    GuildAccept,
    /// Turn it down (`CMSG_GUILD_DECLINE`, empty body); the inviter hears about it, we don't.
    GuildDecline,
    /// Ask our guild's founding date + member/account counts (`CMSG_GUILD_INFO`, empty body).
    GuildInfoRequest,
    /// Refresh the guild roster (`CMSG_GUILD_ROSTER`, empty body) — the guild pane's opener. The
    /// server also pushes the roster unasked after every change, so this is never the only path.
    GuildRosterRequest,
    /// Promote a member one rank (`CMSG_GUILD_PROMOTE`) — towards guild master.
    GuildPromote {
        name: String,
    },
    /// Demote a member one rank (`CMSG_GUILD_DEMOTE`).
    GuildDemote {
        name: String,
    },
    /// Leave our guild (`CMSG_GUILD_LEAVE`, empty body). Refused while we are the guild master and
    /// anyone else remains.
    GuildLeave,
    /// Kick a member by name (`CMSG_GUILD_REMOVE`).
    GuildRemove {
        name: String,
    },
    /// Disband the guild (`CMSG_GUILD_DISBAND`, empty body) — guild master only, irreversible.
    GuildDisband,
    /// Hand the guild to another member (`CMSG_GUILD_LEADER`).
    GuildLeader {
        name: String,
    },
    /// Set the message of the day (`CMSG_GUILD_MOTD`); `""` clears it.
    GuildMotd {
        motd: String,
    },
    /// Rewrite one rank's name **and** rights together (`CMSG_GUILD_RANK`) — there is no partial
    /// form, so a caller changing one must resend the other's current value. Rank 0's rights are
    /// ignored and forced to "all"; an over-long name gets the session kicked, so cap it at
    /// `messages::GUILD_RANK_MAX_LENGTH`.
    GuildRank {
        rank_id: u32,
        rights: u32,
        name: String,
    },
    /// Append a rank at the bottom of the ladder (`CMSG_GUILD_ADD_RANK`).
    GuildAddRank {
        name: String,
    },
    /// Delete the **lowest** rank (`CMSG_GUILD_DEL_RANK`, empty body) — there is no rank id on the
    /// wire, it is always the last one.
    GuildDelRank,
    /// Set a member's public note (`CMSG_GUILD_SET_PUBLIC_NOTE`).
    GuildSetPublicNote {
        name: String,
        note: String,
    },
    /// Set a member's officer note (`CMSG_GUILD_SET_OFFICER_NOTE`). Editing and *viewing* officer
    /// notes are separate rights — an all-empty officer column may mean we cannot see them.
    GuildSetOfficerNote {
        name: String,
        note: String,
    },
    /// Set the guild information text (`CMSG_GUILD_INFO_TEXT`) — the long free-text pane, which
    /// rides back as the roster's `info` field.
    GuildInfoText {
        text: String,
    },
    // ── The petition family (writer bodies in benilla-protocol `world/writer/petition.rs`,
    //    decision 1672) — FOUNDING a guild, which at 1.12 shares nothing with the band above but
    //    its error channel: a refusal on any of these comes back as `SMSG_GUILD_COMMAND_RESULT`.
    //
    //    **A charter is addressed by its ITEM guid**, not by a petition id, everywhere except the
    //    record query. And almost none of these is acked positively: buying answers with the new
    //    item alone, offering answers the TARGET, renaming echoes only if it took. Nothing here may
    //    be treated as applied at the send.
    /// Ask a petitioner NPC for its charter list (`CMSG_PETITION_SHOWLIST`). The gossip row pushes
    /// the same answer unasked, so this is the direct re-open, not the only way in.
    PetitionShowList {
        npc: u64,
    },
    /// Buy a guild charter under a name (`CMSG_PETITION_BUY`). Answered by nothing at all on
    /// success — only the new item arrives.
    PetitionBuy {
        npc: u64,
        name: String,
    },
    /// Open a charter and list its signatures (`CMSG_PETITION_SHOW_SIGNATURES`). Sent by the
    /// item-use fork when a charter is clicked, and again to refresh after a signature lands.
    PetitionShowSignatures {
        item: u64,
    },
    /// Sign somebody's charter (`CMSG_PETITION_SIGN`). `byte` is the optional Lua argument's wire
    /// byte, whose client-side default is **1**; the server reads and discards it.
    PetitionSign {
        item: u64,
        byte: i8,
    },
    /// Show our charter to a player so they can sign it (`CMSG_OFFER_PETITION`) — the success path
    /// answers *them*, not us.
    OfferPetition {
        item: u64,
        player: u64,
    },
    /// Turn a completed charter in (`CMSG_TURN_IN_PETITION`). Owner only; a name collision answers
    /// with a guild command result and **no** turn-in result at all.
    TurnInPetition {
        item: u64,
    },
    /// Ask for a petition's record by id (`CMSG_PETITION_QUERY`) — the only packet carrying the
    /// proposed guild's name and its signature requirement.
    PetitionQuery {
        petition_id: u32,
        item: u64,
    },
    /// Rename a charter (`MSG_PETITION_RENAME`); echoed back only on success.
    PetitionRename {
        item: u64,
        name: String,
    },
    /// Decline a charter offered to us (`MSG_PETITION_DECLINE`) — we send the item guid, the owner
    /// receives ours.
    #[allow(dead_code)]
    PetitionDecline {
        item: u64,
    },
    // ── The taxi/flight-master family (decision 0484 phase 1; writer bodies in
    //    benilla-protocol `messages::taxi`) ──────────────────────────────────────────────────
    /// Ask a nearby flight master's known status (`CMSG_TAXINODE_STATUS_QUERY`): the guid of the
    /// flight master, not ours (vmangos's own comment on the wire field). Answered by
    /// `SMSG_TAXINODE_STATUS` (a `TaxiNodeStatus` event). No send site yet — phase 2's cursor/
    /// tooltip work; wired end-to-end now so that phase only adds a call site.
    #[allow(dead_code)]
    TaxiNodeStatusQuery {
        guid: u64,
    },
    /// Open a flight master's taxi map (`CMSG_TAXIQUERYAVAILABLENODES`, decision 0496 I4:
    /// CONFIRMED as built): the direct opener the taxi-cursor interact sends — the interact ladder
    /// is first-match-wins low→high over `UNIT_NPC_FLAGS`, so a gossip+taxi NPC pre-empts to
    /// gossip and only a pure flightmaster reaches this send. A known node answers
    /// `SMSG_SHOWTAXINODES` (a `TaxiNodesShown` event); a never-visited node instead answers the
    /// first-visit learn pair (`NewTaxiPath` + `TaxiNodeStatus`) and opens nothing on this click.
    TaxiQueryNodes {
        guid: u64,
    },
    /// Fly a single hop (`CMSG_ACTIVATETAXI`): the flight-master guid, the source node, the
    /// destination node. Answered by `SMSG_ACTIVATETAXIREPLY`; success continues into the mount +
    /// `SMSG_MONSTER_MOVE` flight (the existing self-spline rails, decision 0260). Sent by the
    /// taxi-map window's `TakeTaxiNode` drain on a one-hop route pick (`crate::ui_taxi`, phase 2).
    ActivateTaxi {
        guid: u64,
        source_node: u32,
        dest_node: u32,
    },
    /// Fly a multi-hop chain in one send (`CMSG_ACTIVATETAXIEXPRESS`, decision 0496 §TU-3): sent
    /// when no direct `TaxiPath` edge exists current→target — the byte-verified discriminator,
    /// not hop count. The flight-master guid, the route's combined fare, and the full node chain
    /// in order. Answered by `SMSG_ACTIVATETAXIREPLY`, same as [`Self::ActivateTaxi`]. Sent by the
    /// taxi-map window's `TakeTaxiNode` drain on an edge-less route pick (`crate::ui_taxi`, phase 2).
    ActivateTaxiExpress {
        guid: u64,
        total_cost: u32,
        nodes: Vec<u32>,
    },
}

/// The realms this account may enter (`CMD_REALM_LIST`, bridged from the Net drain): the IO thread
/// is parked between the logon and the world dial, waiting for the app to name one. Consumed by
/// [`crate::realm_select`]'s policy — auto-answer (the remembered realm / `WOW_REALM`) or show the
/// realm list and wait for the director.
#[derive(Message)]
pub(crate) struct RealmListMessage {
    pub(crate) realms: Vec<benilla_protocol::RealmInfo>,
}

/// The account's character roster (`SMSG_CHAR_ENUM`, bridged from the Net drain): the IO thread is
/// parked at character select waiting for the app's pick. Consumed by [`crate::char_select`]'s
/// policy — auto-answer (pending pick / `WOW_CHAR`) or show the roster and wait for the director.
/// `realm` is the auth realm-list entry this session connected to (name + type for the screen's
/// realm banner, decision 0465).
#[derive(Message)]
pub(crate) struct CharListMessage {
    pub(crate) characters: Vec<benilla_protocol::Character>,
    pub(crate) realm: Option<benilla_protocol::RealmInfo>,
}

/// The result of an in-place char create/delete the parked IO thread serviced (decision 0423),
/// bridged from the Net drain. A refreshed [`CharListMessage`] precedes a successful one. Consumed by
/// the char-create screen (`crate::char_create`) to show the outcome; `code` is the raw
/// `WorldResult` byte the screen maps to its 1.12 status text.
#[derive(Message, Clone, Copy)]
pub(crate) struct CharActionResultMessage {
    pub(crate) action: benilla_protocol::CharAction,
    pub(crate) code: u8,
}

/// The server **refused** the character we picked (`SMSG_CHARACTER_LOGIN_FAILED`), bridged from the
/// Net drain. The entry announced a moment earlier is void: `crate::char_select` takes the screen
/// back and raises the refusal dialog, and the loading cover comes down with it.
///
/// `result` is the server's raw reason index — [`crate::char_select::char_login_refusal_text`] is
/// the one place that holds the reference's table for it.
#[derive(Message, Clone, Copy)]
pub(crate) struct CharacterLoginFailedMessage {
    pub(crate) result: u8,
}

/// We entered the world (the IO thread's `Connected`, bridged from the Net drain): flips
/// [`crate::char_select::ClientState`] to `InWorld`.
#[derive(Message)]
pub(crate) struct EnteredWorldMessage {
    /// The account's accumulated **rested billing minutes**, from the `SMSG_AUTH_RESPONSE` that
    /// admitted the session — pushed into the script here because this is the only moment it ever
    /// arrives (decision 1820).
    pub(crate) billing_time_rested: u32,
    /// The tutorial bank, if `SMSG_TUTORIAL_FLAGS` landed during the login handshake (1976);
    /// otherwise it arrives in the world stream.
    pub(crate) tutorial_flags: Option<Vec<u8>>,
}

/// **`SMSG_ADDON_INFO`'s verdict for the live session** (decision 2175) — the addons the server
/// hid from the Lua index space, or `None` when it never answered our addon block.
///
/// A resource rather than a field on [`EnteredWorldMessage`], because of *when* it is needed: the
/// index space has to exist before the first addon's file-scope code runs, and the world-entry UI
/// load ([`crate::ui_script::lifecycle::load_ingame_ui_on_world_entry`]) is inside that same edge.
/// A message read a frame later by some other feed would seat it after every addon had already
/// asked. Rewritten on every login, so a second server's silence cannot inherit the first's answer.
#[derive(Resource, Default)]
pub(crate) struct AddonInfoReply(pub(crate) Option<Vec<String>>);

/// The server asked us to play a cinematic (`SMSG_TRIGGER_CINEMATIC`) — a `CinematicSequences.dbc`
/// id. Read by [`crate::cinematic`], which owns the playback *and* the ack (decision 0196: the ack
/// is not optional, only late).
#[derive(Message)]
pub(crate) struct CinematicTriggeredMessage {
    pub(crate) cinematic_id: u32,
}

/// One `CHAT_MSG_SYSTEM` line — the server's own answer to a GM dot-command ("You set god mode to
/// on for …", "There is no such command", "Player not found!"). It has always been *logged*
/// (`net: server says — …`, decision 0651); this makes it *readable*, which is what lets a sender
/// know whether its command landed. That matters most for server state the descriptor does not
/// carry: vmangos's god mode is a runtime-only `Unit::m_invincibilityHpThreshold` with no field and
/// no flag on the wire, so this line is the only ground truth there is (decision 0677).
#[derive(Message, Clone)]
pub(crate) struct ServerSaidMessage {
    pub(crate) text: String,
}

/// The server confirmed our logout (`SMSG_LOGOUT_COMPLETE`): back to character select. The Net
/// drain does the self-teardown (the part 0065's disconnect teardown deliberately keeps); this
/// message flips [`crate::char_select::ClientState`] and clears the pick policy's pending state.
#[derive(Message)]
pub(crate) struct LoggedOutMessage;

/// A login attempt progressed (the IO thread's `LoginStage`, bridged from the Net drain): the
/// login screen's connecting dialog quotes the matching `LOGIN_STATE_*` string (decision 0539).
#[derive(Message, Clone, Copy)]
pub(crate) struct LoginStageMessage {
    pub(crate) stage: benilla_protocol::LoginStage,
}

/// We are **queued** for a full realm (decision 1681) — one per `AUTH_WAIT_QUEUE` packet, while
/// the world handshake is still parked. Not a failure: the attempt is live and the queue ends
/// either with a roster or with a real failure.
#[derive(Message, Clone)]
pub(crate) struct LoginQueuedMessage {
    pub(crate) position: Option<u32>,
    pub(crate) realm: Option<String>,
}

/// The session ended (socket death, logout's teardown edge) — bridged from the Net drain's
/// `Disconnected` arm. [`crate::login`]'s policy reads it as "the IO thread is back at its
/// pre-logon park"; whether anything re-authenticates is `Self::ends_the_session`'s answer.
#[derive(Message, Clone)]
pub(crate) struct DisconnectedMessage {
    pub(crate) reason: String,
    /// How it ended — the fact the whole post-session behaviour turns on (decision 1262).
    pub(crate) end: benilla_protocol::SessionEnd,
    /// **Is this the reference's `DISCONNECTED_FROM_SERVER`** — the session over, the client back
    /// at the account screen behind a "Disconnected from server" dialog, and nothing retried?
    ///
    /// Decided **once**, by [`Self::new`], at the one edge where the event enters the app
    /// (decision 1262). Four readers act on it — the world teardown, the screen flip, the black
    /// cover, the credential policy — and they may not disagree: a teardown that keeps the avatar
    /// for a reconnect that never comes is exactly the avatar-less free camera the report
    /// described. Carrying the verdict rather than re-deriving it four times is what makes that
    /// structural instead of conventional.
    pub(crate) session_over: bool,
}

impl DisconnectedMessage {
    /// The session-end event as the app sees it, with [`Self::session_over`] settled here and
    /// nowhere else.
    ///
    /// It is `false` twice over. A [`SessionEnd::LoggedOut`](benilla_protocol::SessionEnd::LoggedOut)
    /// teardown is the relist *inside* one session — the roster that follows IS the character
    /// select the player asked for. And a run that has **declared itself unattended**
    /// ([`crate::run_mode::unattended`]) keeps 0065's seamless reconnect: a probe has nobody to
    /// press Okay.
    ///
    /// That second clause used to read the *environment* instead
    /// ([`crate::run_mode::env_login`] — "any of `$WOW_USER`/`$WOW_PASS`/`$WOW_CHAR`"), which is
    /// true of the director's own launch line, so 1262's fix was inert in every session a person
    /// was actually in: the kick came, and the client reconnected and took the account back off
    /// whoever had displaced it. Decision 1769 — the doubt now falls the other way, and only a run
    /// that says it is driverless gets to act instead of a person.
    pub(crate) fn new(reason: String, end: benilla_protocol::SessionEnd) -> Self {
        let session_over =
            end == benilla_protocol::SessionEnd::Lost && !crate::run_mode::unattended();
        Self {
            reason,
            end,
            session_over,
        }
    }
}

/// A login attempt failed before the roster (decision 0539): `code` is the server's auth result
/// byte on a refusal (mapped to its `AUTH_*` glue string), `None` on a transport failure. The IO
/// thread is back at its pre-logon park; [`crate::login`]'s policy decides what happens next.
/// `terminal` marks a codeless failure that retrying cannot fix (the server is unusable by this
/// client — e.g. it requires Warden): show `reason` and stop, never resubmit.
#[derive(Message, Clone)]
pub(crate) struct LoginFailedMessage {
    /// Which server refused us and with which byte — see [`benilla_protocol::LoginRefusal`] for
    /// why this is not a bare `Option<u8>`.
    pub(crate) refusal: Option<benilla_protocol::LoginRefusal>,
    pub(crate) reason: String,
    pub(crate) terminal: bool,
    /// The dial that never opened a socket, when that is what failed — see
    /// [`benilla_protocol::DialFailure`]. What lets the screen say *which* of "that name does not
    /// exist" and "nothing is answering there" happened, instead of one "Unable to connect" that
    /// sends a player off editing a correct address.
    pub(crate) dial: Option<benilla_protocol::DialFailure>,
}

/// A same-map teleport for our player: snap to the pose, then echo the ack. Written by
/// [`apply_net_updates`] (only for our guid), read by the player controller in `WorldStage::Input`.
#[derive(Message, Clone, Copy)]
pub(crate) struct TeleportMessage {
    pub(crate) guid: u64,
    pub(crate) counter: u32,
    pub(crate) position: [f32; 3],
    pub(crate) orientation: f32,
}

/// A **server-authored move for our own mover** — an inbound `MSG_MOVE_*` whose guid is ours. It is
/// never an echo of our own reporting: vmangos stamps every one `MovementInfo::SetAsServerSide`
/// (`ctime = 0`, "not a client packet"), and the senders are all server-side edges with no
/// handshake and no ack owed — `.go forward`/`up`/`relative` (`Unit::NearLandTo` →
/// `MSG_MOVE_FALL_LAND`), `.cheat fly`/`fixedz` and the movement anticheat's snap-back
/// (`SendHeartBeat(true)` → `MSG_MOVE_HEARTBEAT`).
///
/// **The real client applies these** — its inbound move path has no mover-guid gate at all, and the
/// local player sits in the object manager under its own guid, so a self-addressed packet resolves
/// and applies exactly like a remote's (`0x603bb0`; decision 0725, which corrects the drop this
/// used to take). Our avatar's motion source is the controller rather than the `RemoteMotion`
/// lane, so the pose crosses as this message and lands in `player::wire_in`.
#[derive(Message, Clone, Copy)]
pub(crate) struct SelfMoveMessage {
    pub(crate) position: [f32; 3],
    pub(crate) orientation: f32,
    /// The wire's `MOVEMENTFLAGS`. Merged into ours under a mask, never assigned — see
    /// [`crate::creature_anim::move_flags::SERVER_AUTHORED`].
    pub(crate) flags: u32,
    pub(crate) pitch: f32,
    pub(crate) fall_time: u32,
    pub(crate) jump: Option<benilla_protocol::JumpInfo>,
    /// The `ON_TRANSPORT` tail — **the server's own deck-local pose for us**, when it sent one.
    ///
    /// This used to be dropped here, and the consumer re-derived the local pose as
    /// `world − boat.translation` against whatever the client's transport tick had last written.
    /// That is only equal to the server's answer while the two agree about where the boat *is* —
    /// and the one moment they provably do not is a cross-map seam, where our path clock has not
    /// crossed yet and [`crate::transport::tick_transports`] is holding the transform frozen at the
    /// far continent's pose. Re-deriving there yields a local offset the size of the gap between
    /// two continents, which the next honest boat pose then multiplies into a fling off the deck.
    /// The authoritative answer was on the wire the whole time (decision 2026).
    pub(crate) transport: Option<benilla_protocol::TransportPose>,
}

/// The server granted or revoked control of a unit (`SMSG_CLIENT_CONTROL_UPDATE`). Written by
/// [`apply_net_updates`] verbatim — the *decision* is the controller's, because only it knows the
/// live pose it would have to park, so the drain deliberately interprets nothing.
///
/// **`mover` is the unit spoken about, not a new subject.** The server revokes by naming *us*
/// (`mover == self guid`, `allow_move = false`), which is what a mind-controlled player receives
/// about themselves, and grants by naming somebody else. Reading it as "the new mover" gets the
/// victim's case exactly backwards — it would hand us control of ourselves at the moment we lost it.
#[derive(Message, Clone, Copy)]
pub(crate) struct ClientControlMessage {
    pub(crate) mover: u64,
    pub(crate) allow_move: bool,
}

/// The server changed one of our own mover's speeds (`SMSG_FORCE_*_SPEED_CHANGE` — aura, mount, GM
/// `.modify speed`). Written by [`apply_net_updates`] (which also applies the new value to the
/// entity's [`UnitSpeeds`], and only forwards changes addressed to our guid); read by the player
/// controller, which answers with [`ClientCommand::ForceSpeedAck`] carrying its live wire state —
/// the same pattern as [`TeleportMessage`], because only the controller owns the honest pose.
#[derive(Message, Clone, Copy)]
pub(crate) struct SpeedChangeMessage {
    pub(crate) guid: u64,
    pub(crate) kind: SpeedKind,
    pub(crate) counter: u32,
    pub(crate) speed: f32,
}

/// **The server granted or revoked a mover mode** on our own mover — root, water-walk, feather-fall
/// or hover (the ack'd movement-mode family; decision 0866). Sibling of [`SpeedChangeMessage`] in
/// every respect: written by [`apply_net_updates`] for our guid only, read by the player controller,
/// which is the one place that owns both the mode state and the honest pose the ack must carry.
///
/// This used to be two death-arc-shaped messages (`MoveRootMessage`/`WaterWalkMessage` in
/// `crate::death`), which is why the other two modes were never noticed as missing: root arrived at
/// death, water-walk arrived in ghost form, and nothing framed them as one family. They are one —
/// the server's own `IsFlagAckOpcode` set.
#[derive(Message, Clone, Copy)]
pub(crate) struct MoveModeMessage {
    /// Our mover's guid (the bridge only emits ours) — echoed in the ack body.
    pub(crate) guid: u64,
    pub(crate) counter: u32,
    pub(crate) mode: MoveMode,
    pub(crate) apply: bool,
}

/// **The server aimed a knockback at our mover** (`SMSG_MOVE_KNOCK_BACK`, decision 1702). Written by
/// [`apply_net_updates`] for our own guid only, read by the player controller — the one place that
/// owns both the mover the launch acts on and the honest post-launch pose the ack must carry.
#[derive(Message, Clone, Copy)]
pub(crate) struct KnockBackMessage {
    /// Our mover's guid (the bridge only emits ours) — echoed in the ack as a **full** u64.
    pub(crate) guid: u64,
    pub(crate) counter: u32,
    /// The launch quad, in the jump tail's own convention (`zspeed` down-positive).
    pub(crate) launch: JumpInfo,
}

/// A cross-map worldport (`.tele Orgrimmar`, initial-login map, a boat crossing the sea): snap
/// the avatar, swap the map's ADTs, and ack if required. Written by [`apply_net_updates`], read
/// by the player controller.
#[derive(Message, Clone, Copy)]
pub(crate) struct WorldportMessage {
    pub(crate) map_id: u32,
    /// **Boat-local** when `transport_entry` is set (vmangos `SendNewWorld` sends the rider's
    /// `GetTransportPos()` — decision 0455); world otherwise.
    pub(crate) position: [f32; 3],
    pub(crate) orientation: f32,
    pub(crate) needs_ack: bool,
    /// The `gameobject_template` entry of the transport carrying us through this transfer, from
    /// the preceding `SMSG_TRANSFER_PENDING` (the [`PendingTransfer`] latch). `Some` ⇒ the pose
    /// above is boat-local and the ride survives the port; `None` ⇒ world pose, and any ride is
    /// stale (the server detached us — e.g. a GM `.tele` off a deck).
    pub(crate) transport_entry: Option<u32>,
}

/// The `SMSG_TRANSFER_PENDING` latch (decision 0455): a far teleport was announced; the value
/// tells the coming worldport whether we ride a transport through it (and which). Set by the
/// apply layer's TransferPending arm, consumed by its worldport arm, cleared on abort and on
/// disconnect.
#[derive(Resource, Default)]
pub(crate) struct PendingTransfer(pub(crate) Option<PendingTransferInfo>);

#[derive(Clone, Copy)]
pub(crate) struct PendingTransferInfo {
    pub(crate) map_id: u32,
    pub(crate) transport_entry: Option<u32>,
}

/// A server-pushed sound (`SMSG_PLAY_SOUND`/`PLAY_MUSIC`/`PLAY_OBJECT_SOUND`): a SoundEntries kit
/// to play — 2D, on the music channel, or 3D at `source` (guid already resolved → entity by
/// [`apply_net_updates`]; `None` when the source isn't streamed to us — consumers play it 2D
/// then, audible beats dropped). Read by the sound subsystem.
#[derive(Message, Clone, Copy)]
pub(crate) struct ServerSoundMessage {
    pub(crate) kind: ServerSoundKind,
    pub(crate) sound_id: u32,
    pub(crate) source: Option<Entity>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerSoundKind {
    Sound2d,
    Music,
    ObjectSound,
}

/// A nearby unit's emote (`SMSG_TEXT_EMOTE` / `SMSG_EMOTE`, bridged from the Net drain), guid
/// already resolved to its entity. `None` source = the performer isn't streamed (drop — an
/// emote needs its performer for voice race/sex and position).
#[derive(Message, Clone, Copy)]
pub(crate) struct EmoteMessage {
    pub(crate) source: Option<Entity>,
    pub(crate) kind: EmoteKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmoteKind {
    /// A chat emote — the `EmotesText.dbc` id (voice by performer race/sex).
    Text(u32),
    /// An anim emote — the `Emotes.dbc` id (its `EventSoundID`).
    Anim(u32),
}

/// A creature flared at someone (`SMSG_AI_REACTION`, bridged from the Net drain; decision 0280):
/// `hostile` = reaction 2 HOSTILE (sent on every creature melee-attack start) vs reaction 0, the
/// stealth pre-aggro ALERT. Pure audio in the client (`0x6056e0` — no animation, nameplate, or
/// UI); consumer: `sound::creature`.
#[derive(Message, Clone, Copy)]
pub(crate) struct AiReactionMessage {
    pub(crate) unit: Entity,
    pub(crate) hostile: bool,
}

/// The pet spoke (`SMSG_PET_ACTION_SOUND`, bridged from the Net drain; decision 2039): `talk` is
/// the wire's own selector (`PET_TALK_ORDER` / `PET_TALK_ATTACK`), not a `SoundEntries` id — the
/// kit comes off the pet's own `CreatureSoundData` row. Pure audio in the client (`0x6040c0`
/// resolves the guid and calls the bark dispatcher, nothing else); consumer: `sound::creature`.
#[derive(Message, Clone, Copy)]
pub(crate) struct PetTalkMessage {
    pub(crate) unit: Entity,
    pub(crate) talk: u32,
}

/// A dismissed pet's parting sound (`SMSG_PET_DISMISS_SOUND`, decision 2039): a
/// `CreatureModelData` id and the point, already in Bevy space. **No entity** — the packet names
/// no guid and the pet is gone by the time it arrives, which is the whole reason it carries a
/// position at all. Consumer: `sound::creature`.
#[derive(Message, Clone, Copy)]
pub(crate) struct PetDismissSoundMessage {
    pub(crate) model_id: u32,
    pub(crate) pos: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The field-edge contract at the one write that emits it (decision 2297), through the same
    /// body the drain's merge calls: a first create seeds silently (the reference's create-time
    /// notify-suppress), a values delta reports each moved dword with its old value and the
    /// store's class, a value re-sent unchanged reports nothing, and an in-place re-create of a
    /// live object notifies like any delta — including a field the fresh snapshot dropped.
    #[test]
    fn the_merge_emits_one_edge_per_moved_dword_and_none_for_a_first_create() {
        use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL};
        use benilla_protocol::messages::ObjectType;

        let mut world = World::new();
        world.init_resource::<Messages<FieldChanged>>();
        let unit = world.spawn(Guid(0xF130_0000_0000_0042)).id();
        let drain = |world: &mut World| -> Vec<FieldChanged> {
            world
                .resource_mut::<Messages<FieldChanged>>()
                .drain()
                .collect()
        };

        // The create block: seeds, says nothing.
        apply_fields_for_test(
            &mut world,
            unit,
            ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_LEVEL, 9)])
                .into_created(ObjectType::Unit),
        );
        assert!(drain(&mut world).is_empty(), "a create is not an edge");

        // A values delta: the ding's dword and the health's, the level re-sent unchanged.
        apply_fields_for_test(
            &mut world,
            unit,
            ObjectFields::from_pairs(&[(FIELD_UNIT_LEVEL, 9), (FIELD_UNIT_HEALTH, 0)]),
        );
        assert_eq!(
            drain(&mut world),
            vec![FieldChanged {
                entity: unit,
                guid: 0xF130_0000_0000_0042,
                kind: ObjectType::Unit,
                index: FIELD_UNIT_HEALTH,
                old: 100,
                new: 0,
            }],
            "one edge for the dword that moved, carrying the old value; the resend is silent"
        );

        // A re-create of the live guid: the reference's in-place refresh notifies, and the
        // replace makes the dropped health an explicit `0 → 0`... no: it was 0 already, so the
        // level's rise is the only edge.
        apply_fields_for_test(
            &mut world,
            unit,
            ObjectFields::from_pairs(&[(FIELD_UNIT_LEVEL, 10)]).into_created(ObjectType::Unit),
        );
        let edges = drain(&mut world);
        assert_eq!(edges.len(), 1);
        assert!(edges[0].unit_field(FIELD_UNIT_LEVEL) && edges[0].old == 9 && edges[0].new == 10);
        assert_eq!(
            world.get::<ObjectStore>(unit).unwrap().0.unit_level(),
            Some(10)
        );
    }

    /// **The kick sends the player back to the login screen** — decision 1262's whole promise,
    /// pinned at the one place that decides it, in the environment the director actually plays in.
    ///
    /// 1262 was built and verified in August and then quietly stopped working, with none of its
    /// code touched: the verdict asked the *environment* whether anybody was here, and on
    /// 2026-08-29 the director's launch line became `WOW_USER=… WOW_PASS=… cargo play` — the
    /// example line in our own `.cargo/config.toml`. From that moment every session they played
    /// was "a probe", so a kick meant reconnect, and the account ping-ponged exactly as B252
    /// reported. Nothing about the fix was wrong; the fact under it was. 1769 made the claim a
    /// declaration, and this test is the tripwire: a run may only act instead of a person if it
    /// said so.
    #[test]
    fn a_kick_ends_the_session_unless_the_run_declared_itself_driverless() {
        use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let lost = || {
            DisconnectedMessage::new("kicked".into(), benilla_protocol::SessionEnd::Lost)
                .session_over
        };

        // The director's launch line, verbatim. Credentials in the environment, nobody declared
        // absent — this is a person playing, and a lost session is over.
        let _user = EnvGuard::set("WOW_USER", "player");
        let _pass = EnvGuard::set("WOW_PASS", "secret");
        let _char = EnvGuard::set("WOW_CHAR", "One");
        let _decl = EnvGuard::unset("WOW_UNATTENDED");
        let _cap = EnvGuard::unset("WOW_CAPTURE");
        let _rig = EnvGuard::unset("WOW_RIG");
        assert!(lost(), "the reported regression: this must be `true`");

        // A clean `/logout` is never "over" — the roster that follows IS the screen they asked for.
        assert!(
            !DisconnectedMessage::new("bye".into(), benilla_protocol::SessionEnd::LoggedOut)
                .session_over
        );

        // 0065's seamless reconnect survives exactly where the run says nobody is there to press
        // Okay — a declaration, or the two runs that cannot be a person.
        {
            let _d = EnvGuard::set("WOW_UNATTENDED", "1");
            assert!(!lost(), "a declared probe still reconnects");
        }
        {
            let _r = EnvGuard::set("WOW_RIG", "at:229,0,0,0");
            assert!(!lost(), "a rig drives the body; it cannot be a person");
        }
        {
            let _c = EnvGuard::set("WOW_CAPTURE", "some-scenario");
            assert!(
                !lost(),
                "a capture authors the camera; it cannot be a person"
            );
        }
        assert!(lost(), "and the guards put every one of those back");
    }

    /// The vanilla player set, and the one every number below is read against:
    /// walk 2.5 · run 7.0 · runBack 4.5 · swim 4.722 · swimBack 2.5 (vmangos `baseMoveSpeed`).
    fn vanilla() -> MoveSpeeds {
        MoveSpeeds {
            walk: 2.5,
            run: 7.0,
            run_back: 4.5,
            swim: 4.722,
            swim_back: 2.5,
            turn_rate: std::f32::consts::PI,
        }
    }

    /// **The cascade's ORDER is the whole feature, and one rung of it is counter-intuitive.**
    /// `0x7c4c90` takes the walk arm at `0x7c4d11` *before* the run arm's backward min at
    /// `0x7c4d1d`, so a walking backpedal is walk speed — not run-back. Our remote extrapolator
    /// tested the backpedal first and got 4.5 here for five hundred commits (decision 1752); this
    /// is the assertion that pins the fix, in the one place both callers now go through.
    #[test]
    fn the_walk_arm_outranks_the_backward_min() {
        use crate::creature_anim::move_flags as f;
        let s = vanilla();
        assert_eq!(current_speed(&s, f::FORWARD), 7.0, "plain run");
        assert_eq!(current_speed(&s, f::BACKWARD), 4.5, "the backward min");
        assert_eq!(current_speed(&s, f::FORWARD | f::WALK_MODE), 2.5, "walk");
        assert_eq!(
            current_speed(&s, f::BACKWARD | f::WALK_MODE),
            2.5,
            "walking backwards is a WALK, not a run-back: the walk arm is taken first"
        );
        // …and a strafing walker likewise — the walk arm is ahead of everything on land.
        assert_eq!(current_speed(&s, f::STRAFE_LEFT | f::WALK_MODE), 2.5);
    }

    /// SWIMMING pre-empts the walk bit entirely (`0x7c4cd9` precedes `0x7c4d11`): there is no
    /// "swim-walk" in this client, so a walker who wades in strokes at full swim speed.
    #[test]
    fn swimming_is_tested_first_so_there_is_no_swim_walk() {
        use crate::creature_anim::move_flags as f;
        let s = vanilla();
        assert_eq!(
            current_speed(&s, f::FORWARD | f::SWIMMING | f::WALK_MODE),
            s.swim
        );
        assert_eq!(
            current_speed(&s, f::BACKWARD | f::SWIMMING | f::WALK_MODE),
            2.5,
            "the swim arm's own backward min — reached, and unaffected by the walk bit"
        );
    }

    /// Both backward arms are a **min**, not a select (x87 `fcompp` + `test ah,0x41` + `jp`), and
    /// so is the walk arm. Only visible under a server that force-sets a slower field above its
    /// faster one — which is exactly why it is a test and not a comment.
    #[test]
    fn every_arm_that_looks_like_a_select_is_really_a_min() {
        use crate::creature_anim::move_flags as f;
        let s = MoveSpeeds {
            walk: 12.0, // a walk-speed buff above run
            run: 7.0,
            run_back: 9.0, // a run-back above run
            swim: 4.0,
            swim_back: 9.0, // a swim-back above swim
            turn_rate: std::f32::consts::PI,
        };
        assert_eq!(current_speed(&s, f::FORWARD | f::WALK_MODE), 7.0);
        assert_eq!(current_speed(&s, f::BACKWARD), 7.0);
        assert_eq!(current_speed(&s, f::FORWARD | f::SWIMMING), 4.0);
        assert_eq!(current_speed(&s, f::BACKWARD | f::SWIMMING), 4.0);
    }

    /// The guard at the TOP of the cascade: no direction bit → 0.0 (`0x7c4c99 test dl,0xf`), never
    /// a stale speed. A mover standing still is standing however its fields read, and the
    /// turn/pitch/mode bits are not translation — walk mode least of all, being precisely a mode
    /// and not a motion.
    ///
    /// The bottom of the cascade has **no** guard, and decision 1759 is why: 1752 shipped a
    /// zero-`walk` fallback to the run arm, on the reading that a mover could reach here before
    /// its speed block landed. It cannot — the local player's create applies the speed block like
    /// anyone else's (`0x5fad50`) — so the fallback is gone. The zero is asserted rather than
    /// merely no longer asserted against, so that re-adding the guard is a deliberate act with a
    /// record to argue against, not a plausible-looking tidy-up.
    #[test]
    fn a_mode_is_not_a_motion_and_a_zero_walk_speed_is_a_zero() {
        use crate::creature_anim::move_flags as f;
        let s = vanilla();
        assert_eq!(current_speed(&s, 0), 0.0);
        assert_eq!(
            current_speed(&s, f::WALK_MODE),
            0.0,
            "a mode is not a motion"
        );
        assert_eq!(current_speed(&s, f::TURN_LEFT | f::ROOT | f::SWIMMING), 0.0);

        let unfilled = MoveSpeeds {
            run: 7.0,
            ..Default::default()
        };
        assert_eq!(
            current_speed(&unfilled, f::FORWARD | f::WALK_MODE),
            0.0,
            "no fallback: `min(walk, run)` with walk 0 is 0 (decision 1759)"
        );
        assert_eq!(
            current_speed(&unfilled, f::FORWARD),
            7.0,
            "…and the run arm is untouched by it"
        );
    }

    /// **The addon lane's four wire bytes** (decision 1235) — the client's own whitelist at
    /// `0x49fa3f`-`0x49fa4e`, and the last link in the chain between an addon's Lua call and the
    /// bytes `benilla-protocol`'s `addon_message_bodies_golden` pins. Wrong here and the payload
    /// goes down a lane nobody listens on, which is silent at both ends.
    ///
    /// `RAID_LEADER`/`RAID_WARNING`/`OFFICER`/`BATTLEGROUND_LEADER`/`CHANNEL` are asserted absent
    /// from the map's *image*: vmangos would accept `LANG_ADDON` on all five, and the client sends
    /// on none of them, so a future widening has to fail a test rather than pass unnoticed.
    #[test]
    fn addon_distributions_map_to_the_clients_own_four_wire_types() {
        use benilla_protocol::messages as m;
        use benilla_ui::script::AddonDistribution as D;

        assert_eq!(addon_wire_chat_type(D::Party), 0x01, "CHAT_MSG_PARTY");
        assert_eq!(addon_wire_chat_type(D::Raid), 0x02, "CHAT_MSG_RAID");
        assert_eq!(addon_wire_chat_type(D::Guild), 0x03, "CHAT_MSG_GUILD");
        assert_eq!(
            addon_wire_chat_type(D::Battleground),
            0x5C,
            "CHAT_MSG_BATTLEGROUND"
        );

        // The image is exactly those four — no distribution reaches a lane the reference refuses.
        let image: Vec<u32> = [D::Party, D::Raid, D::Guild, D::Battleground]
            .into_iter()
            .map(addon_wire_chat_type)
            .collect();
        for refused in [
            m::CHAT_TYPE_SAY,
            m::CHAT_TYPE_YELL,
            m::CHAT_TYPE_WHISPER,
            m::CHAT_TYPE_EMOTE,
            m::CHAT_TYPE_OFFICER,
            m::CHAT_TYPE_CHANNEL,
            m::CHAT_TYPE_RAID_LEADER,
            m::CHAT_TYPE_RAID_WARNING,
            m::CHAT_TYPE_BATTLEGROUND_LEADER,
        ] {
            assert!(
                !image.contains(&refused),
                "{refused:#04x} is a lane the client never sends addon data on"
            );
        }
    }
}
