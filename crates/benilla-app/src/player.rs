//! The avatar, the camera and their input: free-fly until the server places us, then third-person
//! control of our body, a kinematic capsule over avian's `MoveAndSlide`, streamed to the server as
//! the active mover. The cursor is [`crate::cursor`]'s; this module only hides it for mouselook.
//!
//! As in 1.12, right-drag turns the character, left-drag orbits the camera, both buttons run
//! forward steering like a right-drag, and the wheel zooms within `cameraDistanceMax`; the
//! auto-follow (`cameraSmoothStyle`, [`camera::FollowStyle`]) reels a left-drag orbit back in. The
//! dev chord with `F` toggles free-fly, and with `G` lands the avatar at the camera ([`land`]).
//!
//! This file holds the plugin and its ordering edges, the types the modules beside it share
//! ([`Player`], [`BodyQuery`], [`TransportQuery`], [`StandStateRequest`]) and the
//! `UNIT_FIELD_FLAGS` bits with several readers; the per-frame system is [`controller::control`].

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
// Writes the frame onto the body we drive: pose, `MovementState`, the counter-twist gap.
mod body_pose;
pub(crate) mod camera;
mod camera_channel;
pub(crate) mod camera_dynamics;
mod controller;
mod world_focus;
// The remembered camera pose, inside `player/` to read the rig's `pub(super)` fields.
mod camera_saved;
mod camera_water;
// The five named camera views, inside `player/` to write the rig's `pub(super)` fields.
pub(crate) mod camera_view;
mod drunk;
mod embody;
mod flags;
mod follow;

mod gait;
mod input;
// `pub(crate)` for its `LandHere` message, which the debug panel writes.
pub(crate) mod land;
mod move_trace;
mod movement_net;
mod posture;
// `pub(crate)`: a remote mover's dead-reckon (`crate::net::motion::remote`) steps through the same
// walk resolve, as the reference runs every mover through one controller.
pub(crate) mod mover;
mod scoped_view;
// Riding a transport: the deck carry and attach.
mod ride;
mod server_ride;
mod setup;
mod state;
pub(crate) mod step_probe;
mod swim;
mod view_subject;
mod walk;
mod wire_in;

// The last writer of a self body part's render alpha: `entities::apply_unit_mat_alpha` orders
// itself before it.
pub(crate) use camera::apply_self_model_fade;
use controller::control;
// The private camera and `state` items are re-imported for the modules beside this one, which
// name them `super::X`.
use camera::{
    apply_zoom_scroll, model_pivot_height, run_look_session, FlyCam, LookButton, CAM_DIST_DEFAULT,
};
pub(crate) use camera::{head_height, CameraControl, CameraPivot};
// `/follow`: chat sends the request, `crate::target` resolves the subject into the state, and
// `follow` owns the motion.
pub(crate) use follow::{FollowRequest, FollowState};
use state::{
    MoveSpeed, PlayerRide, AIR_NUDGE_SPEED, FALL_FAR_DROP, FALL_FAR_TIME, FOOT_CONE_HEIGHT,
    GROUND_COS, GROUND_PROBE, JUMP_SPEED, LAND_PROBE, MOUSELOOK_PITCH_CLAMP, RUN_BACK_RATIO,
    SKIN_WIDTH, STATIONARY_CHASE_RATE, STEP_SLOPE_RATIO, STEP_SNAP_SLACK, STEP_UP_ADVANCE_PER_YARD,
    TURN_RATE, TURN_RATE_MOVING, WALK_RATIO, WATER_WALK_PITCH_FLOOR, WEDGE_MIN_FALL,
    WEDGE_STALL_RATIO, WEDGE_STILL_FRAMES,
};
pub(crate) use state::{
    Player, PlayerCapsule, CAPSULE_HEIGHT, CAPSULE_RADIUS, CREATURE_STEP_UP_HEIGHT,
    DEFAULT_COLLISION_HEIGHT, FEATHER_TERMINAL_VELOCITY, GRAVITY, HOVER_CLIMB_RATE, HOVER_HEIGHT,
    SETTLE_TIMEOUT, STEP_UP_HEIGHT, TERMINAL_VELOCITY,
};
/// The swim boundary `0.75·h`, also the wade ceiling, for the creature swim marker and the
/// footstep splash, which have no `Player`.
pub(crate) use swim::{may_swim, swim_enter_depth};
/// The camera's followed unit when not our body; [`crate::camera_shake`] takes its body frame and
/// two suspend gates from it, as the reference does from `[cam+0x88]`.
pub(crate) use view_subject::ViewSubject;

/// `UNIT_FLAG_STUNNED`, the `UNIT_FIELD_FLAGS` bit that stops turning. The reference reads it off
/// the descriptor (`[[unit+0x110]+0xa0]`, predicate `0x5145b0`, an inverted `not/shr/and` test) at
/// `0x514755`, which skips the turn and pitch emitters and stops either in flight. StartTurn
/// (`0x602b20`) and StartPitch (`0x602b80`), the only two of the twelve relayed `MSG_MOVE_*`
/// wrappers with a stun test, refuse on it, and it suppresses the auto-face smoothing
/// (`0x600dd7`). No reader touches animation: its sites at `0x5eb4f2` and `0x5ec219` are in
/// `ToggleSheath` (`0x5eb480`) and `CanLootNow` (`0x5ec110`).
///
/// vmangos's stun aura sets it (`SpellAuras.cpp:3523`) and roots (`SpellAuras.cpp:3542`), where a
/// root aura only roots (`SpellAuras.cpp:3772`): a rooted player can still turn, a stunned one
/// cannot.
pub(crate) const UNIT_FLAG_STUNNED: u32 = 0x0004_0000;

/// `IsSelfControlled` (`0x5fa550`, `test eax, 0xc00004` at `0x5fa566`): false while
/// `DISABLE_MOVE` (`0x4`), `CONFUSED` (`0x400000`) or `FLEEING` (`0x800000`) is set; the stun and
/// taxi bits are not in the mask. It is true for an ordinary player, so a gate on it fires in the
/// normal case and is suppressed while feared. Its readers are the movement and collision layer
/// and `DoEmote`.
///
/// `POSSESSED` (`0x1000000`) is not handled: the reference recurses on the charmer's guid
/// (`0x5fa582`) and answers with the charmer's flags, which needs the charmer's descriptor.
pub(crate) fn self_controlled(unit_flags: u32) -> bool {
    /// `DISABLE_MOVE | CONFUSED | FLEEING`, the reference's literal `0xc00004`.
    const NOT_SELF_CONTROLLED: u32 = 0x0000_0004 | 0x0040_0000 | 0x0080_0000;
    unit_flags & NOT_SELF_CONTROLLED == 0
}

/// `UNIT_FLAG_IN_COMBAT`, bit 19 of `UNIT_FIELD_FLAGS` (vmangos `UnitDefines.h:564`), read by the
/// client as `shr reg,0x13; test rl,1`. The local player has no separate combat latch: its two
/// hardcoded readers (`0x482f70`, `0x4d6038`) and `UnitAffectingCombat("player")` read this bit.
pub(crate) const UNIT_FLAG_IN_COMBAT: u32 = 0x0008_0000;

/// `UNIT_FLAG_TAXI_FLIGHT`, bit 20 of `UNIT_FIELD_FLAGS` (vmangos `UnitDefines.h:565`), read by the
/// client as `shr reg,0x14; test rl,1` at 20 sites with no shared gate, among them
/// `SetStandState`'s third guard, the idle auto-AFK gate and `UnitOnTaxi`.
pub(crate) const UNIT_FLAG_TAXI_FLIGHT: u32 = 0x0010_0000;

/// A stand-state request for [`posture`]'s one setter, the client's `SetStandState` (`0x5ed430`:
/// send `CMSG_STANDSTATECHANGE` and apply locally through `0x6127b0`). The posture emotes (`/sit`,
/// `/sleep`, `/kneel`, `/stand`, `/lay`) send it, as `DoEmote`'s `EmoteSpecProc == 1` branch calls
/// the setter the `X` key does; the server sets no posture for a state text emote
/// (`ChatHandler.cpp:729-733`). States are `UnitStandStateType`: 0 stand, 1 sit, 3 sleep, 8 kneel.
#[derive(bevy::ecs::message::Message, Clone, Copy, Debug)]
pub(crate) struct StandStateRequest {
    pub(crate) state: u8,
}

/// `SMSG_STANDSTATE_UPDATE` for our body (the eat and drink sit, the stand on damage), applied by
/// [`posture`] through the same local setter with no refusal and no packet back (`0x603e50` →
/// `0x6127b0`).
#[derive(bevy::ecs::message::Message, Clone, Copy, Debug)]
pub(crate) struct ServerStandState {
    pub(crate) state: u8,
}

/// The body we drive: our own avatar, or a possessed creature. Everything reads its pivot, speeds,
/// scale and descriptor off the entity, so a creature needs nothing special.
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
        // Both hands and the ranged slot, for the `Z` toggle's cycle (the reference's
        // `GetWeapon(0/1/2)`).
        Option<&'static crate::creature_anim::Wielded>,
        // The raw `OBJECT_FIELD_SCALE_X` for the pivot height, not the transform's 2 s-eased scale.
        Option<&'static crate::net::NetEntity>,
    ),
    (With<Embodied>, Without<FlyCam>),
>;

/// The armed transports, for the deck carry ([`ride`]). `ColliderAabb` is for the ride trace:
/// avian refreshes it in `FixedPostUpdate`, before `tick_transports` moves the deck in `Update`, so
/// the broad-phase box lags its deck by a frame.
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

/// The controller's ordering handle, for anything that writes the aim or the camera rig before
/// `control` reads them (`capture::probe_look`, `capture::probe_cam`).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PlayerControlSet;

/// The player and camera: spawns the camera and avatar resources, and runs the controller.
pub(crate) struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        follow::plugin(app);
        camera_saved::plugin(app);
        camera_view::plugin(app);
        // The stream focus is published after `control`'s teleport snap and before the stream
        // stage, and the post-snap hold released after it. Both edges are needed: without
        // `.after(Input)` the focus can precede the snap, the world streams around the departure,
        // and the hold releases at once.
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
        // Far sight resolves `PLAYER_FARSIGHT` to a pose before `control` seats the camera: a
        // separate system, as `control` holds our `Transform` mutably and cannot read another's.
        app.init_resource::<view_subject::ViewSubject>()
            .init_resource::<scoped_view::ScopedView>()
            .add_systems(
                Update,
                (
                    view_subject::publish_view_subject,
                    // The spyglass zoom (aura 76), before `control`, which reads it to hold the
                    // rig in first person.
                    scoped_view::apply_scoped_view,
                )
                    .in_set(WorldStage::Input)
                    .before(control)
                    .in_set(crate::char_select::InWorldGated),
            );
        app.add_systems(
            Startup,
            // After the config load: the camera reads `gxMultisample` once, at spawn.
            setup::setup_player
                .after(AssetSet::Open)
                .after(crate::cvars::CvarLoad),
        )
        // The world camera renders only in world or under the loading screen, whose covered
        // render compiles the world's pipelines before the first visible frame.
        .add_systems(Update, setup::gate_world_camera)
        // Off in capture mode, where the harness pins the camera, and in world only, so the glue
        // screens keep the cursor and send no movement.
        .add_systems(
            Update,
            control
                .in_set(PlayerControlSet)
                .in_set(WorldStage::Input)
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>))
                .in_set(crate::char_select::InWorldGated),
        )
        // The posture queue (the `/sit` family), which `control` alone executes.
        .add_message::<StandStateRequest>()
        .add_message::<ServerStandState>()
        // Land-here, before `control`, so the frame that applies the teleport takes control back.
        .add_message::<land::LandHere>()
        .add_systems(
            Update,
            land::land_here
                .in_set(WorldStage::Input)
                .before(control)
                .in_set(crate::char_select::InWorldGated),
        )
        // A server spline driving us (Charge, knockback, taxi) is mirrored into `Player` before
        // `control` reads `pos`; gated as `control` is.
        .add_systems(
            Update,
            server_ride::drive_self_ride
                .in_set(WorldStage::Input)
                .before(control)
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>))
                .in_set(crate::char_select::InWorldGated),
        )
        // A session end (a `/logout` or a lost session) drops `active`, re-arming the take-control
        // latch for the next login; ungated, as the message lands while the state flips.
        .add_systems(
            Update,
            wire_in::release_on_session_end.in_set(WorldStage::Input),
        )
        // Embodiment, before every reader of the marker.
        .add_systems(
            Update,
            embody::maintain_embodiment
                .in_set(WorldStage::Input)
                .before(control)
                .before(mirror_mover_collision_height),
        )
        // Before `control`, which evaluates the swim depth lines.
        .add_systems(
            Update,
            mirror_mover_collision_height
                .in_set(WorldStage::Input)
                .before(control),
        )
        // The shake adds its offset after `control` seats the camera, so its distance falloff
        // measures the unshaken eye.
        .add_systems(
            Update,
            crate::camera_shake::apply_camera_shake
                .in_set(WorldStage::Input)
                .after(control)
                // Off in capture mode, where the camera is pinned.
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>)),
        )
        // Which mouse buttons the world owns, decoded ahead of its three readers: `/follow`'s
        // both-button cancel, the look session and the camera's command word. Not in `control`:
        // `steer_follow` runs before it and would read the latch a frame late.
        .add_systems(
            Update,
            camera::latch_world_mouse
                .in_set(WorldStage::Input)
                .before(control)
                .before(follow::steer_follow)
                .in_set(crate::char_select::InWorldGated),
        )
        // `/follow` steers before `control`, so the player's own turn input lands after it and
        // wins over the follow's steering.
        .add_systems(
            Update,
            follow::steer_follow
                .in_set(WorldStage::Input)
                .before(control)
                .in_set(crate::char_select::InWorldGated),
        )
        // The self-avatar fade shares the `MeshTag` and material channel with the interior
        // classifier and the appear and despawn fades, and writes `Visibility` for the
        // first-person hide, so it runs after them and the model-visibility set; off in capture
        // mode, like the `control` whose alpha it reads.
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

/// Mirror the driven body's [`crate::entities::CollisionHeight`] onto [`Player`], whose swim depth
/// lines are fractions of it; continuous, since a worldport re-streams the body and a possession
/// swaps it.
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
