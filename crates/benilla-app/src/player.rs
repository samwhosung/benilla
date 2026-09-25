//! Avatar + camera + input. Free-flies until the server reports our position, then takes third-person
//! control of the avatar (WASD walks it, height-following the terrain) and streams our movement to the
//! server as the confirmed mover. Owns the camera entity. The cursor itself is owned by the
//! [`crate::cursor`] subsystem; this module only hides it during mouselook (`CursorOptions.visible`).
//!
//! Mouse control mirrors vanilla's two look modes (grounded in the WoW 1.12 camera CVars / mouselook
//! API): **right-drag turns the character** (movement then
//! follows the camera heading), **left-drag orbits the camera** around a stationary character; either
//! locks + hides the cursor and restores it on release. **Both buttons held together run the character
//! forward** (vanilla's "both-button move"), steering with the mouse like a right-drag. The **scroll
//! wheel** zooms the third-person
//! distance (clamped to the vanilla `cameraDistanceMax` range; the camera *glides* to the new
//! distance). A left-drag orbit offset is reeled back in by the **auto-follow** — vanilla's
//! `cameraSmoothStyle`, a real player setting since decision 1493 ([`camera::FollowStyle`]):
//! Smart (the shipped default) chases while the character moves, Always chases always, Never
//! leaves the camera exactly where the hand put it.
//! The **dev chord + `F`** toggles free-fly (1043); the **dev chord + `G`** lands the avatar where
//! the camera is ([`land`]).
//!
//! Movement is a thin kinematic capsule controller over avian's `MoveAndSlide`.
//!
//! **This file is the map, not the machine.** It holds the plugin and its ordering edges, the
//! shared types the concern modules trade in ([`Player`], [`BodyQuery`], [`TransportQuery`],
//! [`StandStateRequest`]), and the two `UNIT_FIELD_FLAGS` bits with more than one reader. The
//! per-frame system itself is [`controller::control`], and every phase it runs that is a concern
//! of its own has a file beside it: [`input`] decodes the keys, [`wire_in`] applies what the
//! server said, [`ride`] carries us on a deck, [`posture`] holds the body, [`mover`]/[`swim`]
//! move it, [`flags`] builds the move-flag word, [`gait`] + [`body_pose`] render it, [`camera`]
//! seats the eye, [`movement_net`] streams it.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

use crate::creature_anim::{move_flags, wrap_pi, BodyTwist, MovementState};
use crate::net::{ClientCommand, Embodied, NetCommands, TeleportMessage, WorldportMessage};
use crate::ui_script::InspectMode;
use benilla_assets::coords::wow_to_bevy;
use benilla_assets::AssetSet;
use benilla_world::interact::{WorldClick, WorldRightClick, WorldRightPress};
use benilla_world::schedule::WorldStage;

mod arc;
mod net;
// Writing the frame onto the body we drive — pose, MovementState, the counter-twist gap.
mod body_pose;
pub(crate) mod camera;
// The one smoothed-scalar channel the reference instantiates four times (all stepped by
// `0x50f160`) — pitch, pitch-bias, ground tilt and the pivot height, one template.
mod camera_channel;
// The four 1.12 camera option toggles and the mechanisms behind them.
pub(crate) mod camera_dynamics;
// The per-frame controller itself — the one system, split out so the root stays the map.
mod controller;
mod world_focus;
// The remembered camera pose — it lives inside `player/` so it can read the rig's
// own `pub(super)` fields instead of widening them for a module outside.
mod camera_saved;
mod camera_water;
// The five NAMED camera poses the player can jump between — `camera_saved`'s
// complement: that one remembers where you left the camera, this one where you decided it should
// be able to go. Same reason for living inside `player/`: it writes the rig's `pub(super)` fields.
pub(crate) mod camera_view;
mod drunk;
// Which single unit the client embodies — the `Embodied` marker's owner.
mod embody;
// This frame's move-flag word — the one bitset the animation, the wire and the local gates all
// read.
mod flags;
mod follow;

mod gait;
// This frame's decoded input — the netted movement axes and the camera's command word.
// Everything downstream reads the axes, never the keys.
mod input;
// The land-here affordance (free-fly's other half). `pub(crate)` for its `LandHere` message, which
// the debug panel's button writes.
pub(crate) mod land;
mod move_trace;
mod movement_net;
// The stand state + the sheath toggle — how the body HOLDS itself.
mod posture;
// The kinematic mover step. `pub(crate)` because the grounded walk resolve is **not** the local
// player's alone: a remote mover's dead-reckon (`crate::net::motion::remote`) runs its extrapolated
// step through the very same code, the way the reference runs every mover through one controller
// (decision 0059's byte trail).
pub(crate) mod mover;
/// The spyglass zoom — aura 76, a client-local camera override with no wire half at all.
mod scoped_view;
// Riding a transport — the platform-frame carry/attach.
mod ride;
mod server_ride;
mod setup;
mod state;
/// The step-up diagnostic probe — the blocked-frame report behind the `stup` trace tag.
pub(crate) mod step_probe;
mod swim;
/// What the camera orbits, when that is not our own body — the far-sight anchor (and Mind
/// Control's camera half in B211, which rides the same field).
mod view_subject;
/// The walk/run gait toggle (`TOGGLERUN`) — one latched bit and the reference's refusal chain.
mod walk;
mod wire_in;

// `apply_self_model_fade` is `pub(crate)`-visible: it is the LAST writer of a self body part's
// render-alpha field, so the unit-lane material-alpha compose (`entities::apply_unit_mat_alpha`)
// orders itself before it and lets that documented override stand.
pub(crate) use camera::apply_self_model_fade;
// The controller system, named unqualified here so the plugin's ordering edges (and the ones
// sibling modules declare against it) read as they always have.
use controller::control;
// `/follow`: chat asks with the message, `crate::target` resolves the subject into
// the state, and this module owns the motion.
use camera::{
    apply_zoom_scroll, model_pivot_height, run_look_session, FlyCam, LookButton, CAM_DIST_DEFAULT,
};
pub(crate) use camera::{head_height, CameraControl, CameraPivot};
pub(crate) use follow::{FollowRequest, FollowState};
// The shared avatar state + movement constants live in [`state`]; the private re-imports below are
// what lets this module and the concern modules beside it keep naming them `super::X` unchanged.
use state::{
    MoveSpeed, PlayerRide, AIR_NUDGE_SPEED, FALL_FAR_DROP, FALL_FAR_TIME, FOOT_CONE_HEIGHT,
    GROUND_COS, GROUND_PROBE, JUMP_SPEED, LAND_PROBE, MOUSELOOK_PITCH_CLAMP, RUN_BACK_RATIO,
    SKIN_WIDTH, STATIONARY_CHASE_RATE, STEP_SLOPE_RATIO, STEP_SNAP_SLACK, STEP_UP_ADVANCE_PER_YARD,
    TURN_RATE, TURN_RATE_MOVING, WALK_RATIO, WATER_WALK_PITCH_FLOOR, WEDGE_MIN_FALL,
    WEDGE_STALL_RATIO, WEDGE_STILL_FRAMES,
};
// `SETTLE_TIMEOUT` is `pub(crate)`: the settle release lives in the terrain streamer (decision
// 0737 — residency releases the hold, not ground contact), which owns the deadline push while the
// resident world is still the departed map's.
pub(crate) use state::{
    Player, PlayerCapsule, CAPSULE_HEIGHT, CAPSULE_RADIUS, CREATURE_STEP_UP_HEIGHT,
    DEFAULT_COLLISION_HEIGHT, FEATHER_TERMINAL_VELOCITY, GRAVITY, HOVER_CLIMB_RATE, HOVER_HEIGHT,
    SETTLE_TIMEOUT, STEP_UP_HEIGHT, TERMINAL_VELOCITY,
};
/// The swim boundary `0.75·h` — and therefore the **wade ceiling**, since wading is the implicit
/// in-liquid-but-not-swimming state and the two cannot be different numbers. Read by the creature
/// swim marker and the footstep splash slot, which have no `Player` of their own.
pub(crate) use swim::{may_swim, swim_enter_depth};
/// The camera's followed unit when that is not our own body — read by [`crate::camera_shake`] for
/// the shake's body frame and its two suspend gates, which the reference takes off `[cam+0x88]`.
pub(crate) use view_subject::ViewSubject;

/// **`UNIT_FLAG_STUNNED`** — the `UNIT_FIELD_FLAGS` bit that freezes a character's *turning*.
/// Not a movement flag and not an aura: the reference reads it straight off the
/// descriptor block at `[[unit+0x110]+0xa0]` (predicate `0x5145b0` — note the inverted
/// `not/shr/and` form, which a census grepping only `test …,0x40000` misses — consumed at
/// `0x514755`, which skips both the turn and pitch emitters and force-stops either in flight).
///
/// It is the **other half** of a stun. vmangos's `HandleModStun` sets this flag *and* calls
/// `SetRooted(true)`, so `SPELL_AURA_MOD_STUN` grants both: root kills translation, this kills the
/// pivot. A pure root (Frost Nova, Entangling Roots) sets only the first — which is why a rooted
/// player can still turn and a stunned one cannot, the distinction B179 was reporting.
///
/// **It has no second consumer — the "idle/fidget" one was a misread, and this doc outlived the
/// correction.** It used to say the idle/fidget selectors bail on the bit at `0x5eb4f2`/`0x5ec219`
/// and pointed at a `MovementState::stunned` that decision **0889** had already deleted along with
/// the gate: re-read at the bytes, those two addresses are inside **`ToggleSheath 0x5eb480`** and
/// **`CanLootNow 0x5ec110`**, and **no site in the whole `0x40000` census touches animation
/// selection**. What stops the idle twitch under a stun is the animation clock being stopped
/// (0889's proc-11 freeze), not this flag. The claim survived here for a fortnight because a doc
/// comment is not a gate.
///
/// The census's other movement-relevant consumers are real and stay: the two relayed
/// `MSG_MOVE_*` wrappers that refuse outright (`0x602b20` StartTurn, `0x602b80` StartPitch — the
/// only two of twelve carrying a stun test), and the auto-face-target smoothing suppression at
/// `0x600dd7`.
pub(crate) const UNIT_FLAG_STUNNED: u32 = 0x0004_0000;

/// **`IsSelfControlled`** — the reference's `0x5fa550` (`5fa566 a9 04 00 c0 00 test eax, 0xc00004`):
/// is this unit acting under its *own* control? False while `DISABLE_MOVE` (`0x4`), `CONFUSED`
/// (`0x400000`) or `FLEEING` (`0x800000`) — and note what is **absent**: `UNIT_FLAG_STUNNED` is
/// not in the mask (it has its own, separate gate), and neither is the taxi bit.
///
/// Its consumers in the reference are the movement/collision layer and `DoEmote` — and the
/// polarity is the trap: it returns **1 for an ordinary player**, so a gate written on it fires
/// in the *normal* case and is suppressed while feared, not the other way round.
///
/// **One leg is deliberately unmodelled.** `POSSESSED` (`0x1000000`) does not refuse here: the
/// reference redirects at `0x5fa582` to the charmer's GUID and **recurses** into itself, so the
/// answer is the charmer's. Reproducing that needs the charmer's descriptor, and the only
/// consumer that would notice is whether a red toast appears while mind-controlled. Named rather
/// than invented.
pub(crate) fn self_controlled(unit_flags: u32) -> bool {
    /// `DISABLE_MOVE | CONFUSED | FLEEING` — the reference's literal `0xc00004`.
    const NOT_SELF_CONTROLLED: u32 = 0x0000_0004 | 0x0040_0000 | 0x0080_0000;
    unit_flags & NOT_SELF_CONTROLLED == 0
}

/// `UNIT_FLAG_IN_COMBAT` — the same `UNIT_FIELD_FLAGS` word, **bit 19** (vmangos
/// `UnitDefines.h`; the client reads it as `shr reg,0x13; test rl,1`).
///
/// Its readers are deliberately unrelated to each other — the spell-usability walk's leg 8, the
/// probe preflight banner, and `UnitAffectingCombat`'s snapshot field — which is exactly why the
/// constant lives here beside its neighbour rather than being declared a fourth time: three
/// private copies of one bit is how a mask silently drifts.
///
/// **There is no player-specific combat latch to pair it with.** A census of the
/// `shr reg,0x13` + `test rl,1` idiom image-wide (7 hits, 6 on this field+bit) finds the two
/// hardcoded *local-player* readers (`0x482f70`, `0x4d6038`) going through this same flag;
/// `UnitAffectingCombat("player")` takes a GUID fast path and lands on the identical word.
pub(crate) const UNIT_FLAG_IN_COMBAT: u32 = 0x0008_0000;

/// `UNIT_FLAG_TAXI_FLIGHT` — the same `UNIT_FIELD_FLAGS` word, **bit 20** (vmangos
/// `UnitDefines.h:511`; the client reads it as `shr reg,0x14; test rl,1` — 20 independent sites
/// of that idiom with no shared gate).
///
/// Beside its neighbour for the same reason that one is here: readers that have nothing to do with
/// each other — `SetStandState`'s own guard #3, the idle handler's auto-AFK gate and `UnitOnTaxi`
/// — and a bit that is spelled out more than once is a bit that eventually drifts.
pub(crate) const UNIT_FLAG_TAXI_FLIGHT: u32 = 0x0010_0000;

/// Ask for a **stand state** — the client's `SetStandState(newState)` (`0x5ed430`: send
/// `CMSG_STANDSTATECHANGE` + apply locally through `0x6127b0`), as a message so every path that
/// wants a posture funnels through the ONE setter in [`posture`] (the [`crate::creature_anim::
/// SheathRequest`] posture, decision 0080).
///
/// Two senders today: the `X` key reads the toggle inline in [`posture`], and the **posture emotes**
/// (`/sit`, `/sleep`, `/kneel`, `/stand`, `/lay`) send this — `DoEmote`'s `EmoteSpecProc == 1`
/// branch calls the same `0x5ed430` the key does.
/// Routing them here is what makes `/sit` sit at all: the *server*
/// does nothing for a STATE text emote (vmangos `HandleTextEmoteOpcode` breaks out of the switch
/// for SIT/SLEEP/KNEEL), so the posture is the client's own to set.
///
/// The state values are `UnitStandStateType`: 0 STAND · 1 SIT · 3 SLEEP · 8 KNEEL.
#[derive(bevy::ecs::message::Message, Clone, Copy, Debug)]
pub(crate) struct StandStateRequest {
    pub(crate) state: u8,
}

/// The server's own stand state for OUR body (`SMSG_STANDSTATE_UPDATE` — the eat/drink sit, the
/// stand on damage). Applied by [`posture`] through the same local setter the
/// volunteered change uses, with no refusal gate and no packet back — the reference's `0x603e50`
/// → `0x6127b0`. Written by [`net`], read in `control`.
#[derive(bevy::ecs::message::Message, Clone, Copy, Debug)]
pub(crate) struct ServerStandState {
    pub(crate) state: u8,
}

/// **The body in our hands** — normally our own streamed avatar, and a possessed creature while we
/// hold its reins. The controller reads its server pose to take control, then
/// drives its transform (feet position + facing) and feeds its movement to the animation selector
/// via `MovementState`. Its model is attached by the entity renderer through the same path as any
/// other unit, which is exactly why a creature needs nothing special: everything downstream
/// reads the body's own pivot, speeds, scale and descriptor off the entity.
///
/// An alias because three modules take it — [`control`], [`posture`] (read-only, for the sheath
/// toggle's guard chain) and [`body_pose`] (the frame's one writer) — and one eleven-column tuple
/// written out three times is how a query silently drifts from its readers.
pub(super) type BodyQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Transform,
        Option<&'static mut MovementState>,
        Option<&'static CameraPivot>,
        Option<&'static crate::creature_anim::AnimDriver>,
        Option<&'static crate::net::ObjectStore>,
        Has<crate::creature_anim::Engaged>,
        Option<&'static crate::net::UnitSpeeds>,
        Option<&'static mut BodyTwist>,
        // What is worn in each hand and in the ranged slot — the Z toggle's cycle reads it
        // (the ref's three `GetWeapon(0/1/2)` calls before the state walk).
        Option<&'static crate::creature_anim::Wielded>,
        // The **raw** `OBJECT_FIELD_SCALE_X`, for the camera pivot's target height — not the
        // transform's, which is the 2 s-eased render scale (see [`camera::head_height`]).
        Option<&'static crate::net::NetEntity>,
    ),
    (With<Embodied>, Without<FlyCam>),
>;

/// The armed transports — the platform-frame carry/attach ([`ride`], decision 0438 phase 2). The
/// `Without`s only disjoint the borrows against the body and the camera.
///
/// `ColliderAabb` rides along for the **ride trace only**, and it is the column that separates
/// "the deck moved" from "the deck's collider moved": avian refreshes it in `PhysicsSchedule`
/// (i.e. `FixedPostUpdate`, *before* `Update`), while `tick_transports` writes the deck's pose *in*
/// `Update` — so the box the broad phase prunes against is always one frame behind the deck it
/// belongs to, and the down-probe's candidate set is whatever that stale box still overlaps.
pub(super) type TransportQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static crate::net::Guid,
        Option<&'static avian3d::prelude::ColliderAabb>,
    ),
    (
        With<crate::transport::Transport>,
        Without<Embodied>,
        Without<FlyCam>,
    ),
>;

/// The player controller's **ordering handle**. Anything that must write the aim or the camera rig
/// before the controller reads them orders against this — the two scripted probe drivers do
/// (`capture::probe_look` / `capture::probe_cam`). A set rather than the `control`
/// symbol itself: an instrument may name the gameplay system it runs against, but exporting
/// `control` would drag its private parameter types (`MoveSpeed`, `PlayerCapsule`, `PressGesture`)
/// out with it, which is exactly the internals-publishing 1173 rejected a crate wall to avoid.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PlayerControlSet;

/// The player/camera subsystem: spawns the camera + move/avatar resources at startup, drives the
/// third-person/free-fly controller each frame. (The cursor is the [`crate::cursor`] subsystem.)
pub(crate) struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        follow::plugin(app);
        camera_saved::plugin(app);
        camera_view::plugin(app);
        // 1160's wire (a), both directions (see `world_focus`): the game answers the world's
        // "where do I stream from" before the stream stage, and reads the residency the world
        // publishes after it to end its own post-snap hold.
        //
        // **Both ordering edges are load-bearing** (B263 round 3). `.before(Stream)`
        // alone left the publish free to run before `control`'s teleport snap (conflicting access,
        // no edge — the executor picks either order), and on the snap frame the streamer then
        // streamed around the DEPARTURE position: residency read "world resident", the settle hold
        // released on frame 0, and the body free-fell at the destination under the loading screen —
        // every same-map `.tele` on the probe machine. `.after(Input)` pins the publish to the
        // post-snap position; the schedule contract (snap → focus → stream → release → present)
        // is only a contract if every arrow is an explicit edge.
        app.add_systems(
            Update,
            (world_focus::publish_viewer, world_focus::publish_view_focus)
                .after(benilla_world::schedule::WorldStage::Input)
                .before(benilla_world::schedule::WorldStage::Stream),
        )
        .add_systems(
            Update,
            world_focus::release_post_snap_hold.after(benilla_world::schedule::WorldStage::Stream),
        );
        app.add_observer(camera::on_cvar);
        app.add_observer(camera_dynamics::on_cvar);
        app.init_resource::<camera::LookConfig>();
        app.init_resource::<camera::ZoomLimit>();
        app.init_resource::<camera::FollowConfig>();
        app.init_resource::<camera_dynamics::CameraOptions>();
        // Far sight: resolve `PLAYER_FARSIGHT` into a pose before `control` reads it to seat the
        // camera. A separate system rather than another query on `control` for a hard reason —
        // `control` already holds the self entity's `Transform` mutably, so it cannot also read an
        // arbitrary unit's.
        app.init_resource::<view_subject::ViewSubject>()
            .init_resource::<scoped_view::ScopedView>()
            .add_systems(
                Update,
                (
                    view_subject::publish_view_subject,
                    // The spyglass zoom (aura 76) — a different mechanism entirely, sharing only
                    // the family name. Also before `control`, which reads the scope to hold the rig
                    // in first person: the reference's camera lock.
                    scoped_view::apply_scoped_view,
                )
                    .in_set(WorldStage::Input)
                    .before(control)
                    .in_set(crate::char_select::InWorldGated),
            );
        app.add_systems(
            Startup,
            // AFTER the config fold, not merely after the assets: the camera reads `gxMultisample`
            // once, at spawn (1629), so a player's setting has to be in the resource by now.
            setup::setup_player
                .after(AssetSet::Open)
                .after(crate::cvars::CvarLoad),
        )
        // The world camera renders only when the world can be seen: in world,
        // or under the opaque loading screen (whose covered render is what compiles the
        // world's pipelines before the first visible frame). At the glue screens the fully
        // streamed world otherwise burns real GPU time behind an opaque fullscreen scene.
        .add_systems(Update, setup::gate_world_camera)
        // In capture mode the harness ([`crate::capture`]) pins the camera (and thus the stream
        // focus), so `control` must not also drive it — gate it off when capturing. In-world
        // only: at the character-select glue screen the controller must not
        // grab the cursor, fly the camera, or queue movement sends behind the overlay.
        .add_systems(
            Update,
            control
                .in_set(PlayerControlSet)
                .in_set(WorldStage::Input)
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>))
                .in_set(crate::char_select::InWorldGated),
        )
        // The posture setter's queue (the `/sit` family — decision 0881; `control` is the sole
        // executor, like the sheath queue).
        .add_message::<StandStateRequest>()
        // The server's own stand state for our body, written by [`net`].
        .add_message::<ServerStandState>()
        // Land-here ([`land`]): the ask, and the re-attach when the server's teleport lands.
        // Before `control` so the frame that applies the teleport is the frame that takes
        // third-person control back — `control` reads `detached` after this has cleared it.
        .add_message::<land::LandHere>()
        .add_systems(
            Update,
            land::land_here
                .in_set(WorldStage::Input)
                .before(control)
                .in_set(crate::char_select::InWorldGated),
        )
        // (The two scripted probe drivers that used to sit here — `WOW_PROBE_LOOK`'s
        // mouse-turn and `WOW_PROBE_CAM`'s camera park — are the harness's now, and register
        // themselves against `control` from there: decision 1174 moved every instrument out of
        // this module so a player build carries none of them.)
        // A server-authored spline (Charge/knockback/taxi) driving our own player is mirrored into
        // `Player` here, *before* `control` reads `pos` to seat the camera and skip input. Same
        // gates as `control` (not while capturing; in-world only).
        .add_systems(
            Update,
            server_ride::drive_self_ride
                .in_set(WorldStage::Input)
                .before(control)
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>))
                .in_set(crate::char_select::InWorldGated),
        )
        // A session END releases the avatar — a confirmed `/logout`, or a lost session:
        // the streamed entity is despawned by the net drain either way, and
        // dropping `active` re-arms the take-control latch for the next login (possibly a
        // different character). Ungated — the message lands as the state flips.
        .add_systems(
            Update,
            wire_in::release_on_session_end.in_set(WorldStage::Input),
        )
        // Which body the client drives at all. Strictly before everything that
        // reads the marker — the controller, and the collision-height mirror below.
        .add_systems(
            Update,
            embody::maintain_embodiment
                .in_set(WorldStage::Input)
                .before(control)
                .before(mirror_mover_collision_height),
        )
        // Mirror the driven body's collision height onto `Player` for the swim arm (decision
        // 0645). A *continuous* sync rather than a one-shot at take-control, for the reason the
        // take-control branch itself records: a cross-map worldport re-streams the entity, so
        // anything latched on that edge is lost on transfer — and a possession swaps it for a
        // body of an entirely different size. Before `control`, which is where the swim depth
        // lines are evaluated.
        .add_systems(
            Update,
            mirror_mover_collision_height
                .in_set(WorldStage::Input)
                .before(control),
        )
        // The camera shake lands on the camera AFTER `control` has
        // seated it: the applier adds its offset to the pose `seat_camera` just wrote, so the
        // eye it measures the distance falloff against is the un-shaken one. `control` is at
        // Bevy's 16-param ceiling, so the offset cannot be threaded into it as a resource —
        // and running after is the better shape anyway.
        .add_systems(
            Update,
            crate::camera_shake::apply_camera_shake
                .in_set(WorldStage::Input)
                .after(control)
                // Gated off in capture mode alongside `control`, and for the same reason a
                // capture keeps the doodad rail static: a pinned camera that a passing kodo
                // could nudge is not a regression baseline any more.
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>)),
        )
        // **Which mouse buttons the world owns** (ledger B364) — decoded once, ahead of all
        // three readers: `/follow`'s both-button cancel below, the look session, and the camera's
        // input command word. `control` cannot own it, because `steer_follow` runs before
        // `control` and would then read it a frame late.
        .add_systems(
            Update,
            camera::latch_world_mouse
                .in_set(WorldStage::Input)
                .before(control)
                .before(follow::steer_follow)
                .in_set(crate::char_select::InWorldGated),
        )
        // `/follow`: steer the facing and decide this tick's synthesized forward
        // input immediately BEFORE the controller, which folds the flag into its forward axis.
        // The player's own turn input therefore runs after us and wins, which is exactly what
        // makes the turn-away cancel reachable.
        .add_systems(
            Update,
            follow::steer_follow
                .in_set(WorldStage::Input)
                .before(control)
                .in_set(crate::char_select::InWorldGated),
        )
        // The self-avatar zoom-in fade rides the same `MeshTag`/material channel as the interior
        // classifier + the appear/despawn fades, so it must run *after* both to win the frame while
        // fading (and yield to them otherwise). It also writes `Visibility` (the first-person
        // hide), so it must run after the model-`Visibility` authority
        // (`debug_panel::apply_model_visibility`) too — otherwise whichever system Bevy's
        // arbitrary sort ran last would win, and the authority could re-show the body in
        // first-person. First-person correctness outranks the dev creature-toggle for these few
        // submeshes. Gated off in capture mode alongside `control` (whose per-frame
        // `self_fade_alpha` it consumes), so a pinned capture never hides the avatar.
        .add_systems(
            Update,
            apply_self_model_fade
                .after(benilla_world::interior::classify_entity_interior)
                .after(benilla_world::model_fade::apply_render_fade)
                .after(benilla_world::model_render::ModelVisSet)
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>)),
        );
    }
}

/// Mirror the driven body's [`crate::entities::CollisionHeight`] onto [`Player`] — its swim depth
/// lines are fractions of it, and the swim arm runs off the resource, not the
/// entity. One entity, one copy: no work until a body streams in, and it re-syncs itself after a
/// worldport re-streams that entity under a new one — or after a possession puts a different-sized
/// body in our hands.
fn mirror_mover_collision_height(
    mut player: ResMut<Player>,
    mover: Query<&crate::entities::CollisionHeight, With<Embodied>>,
) {
    if let Ok(&h) = mover.single() {
        if player.collision_height != h {
            player.collision_height = h;
        }
    }
}
