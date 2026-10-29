//! The per-frame player controller, [`control`]: this frame's input to the avatar's pose, its
//! animation state and the packets owed the server. The phases live in sibling modules; their order
//! here is the reference's frame order and is load-bearing (the swim latch before the mover, the
//! posture commit before the pose, the ack after the launch).

use super::*;

#[cfg(test)]
mod tests;

/// What the frame's mouse input handed the controller: [`sample_look`] writes it among the input
/// records, ahead of the packets, and [`control`] reads it in the frame time.
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct LookSample {
    /// The mouse's hand-off of the camera's facing and pitch to the body (`0x5103e0`) ran this
    /// frame: a motion while the mouse turns the body, or the press that completed both buttons.
    pub(super) hands_off: bool,
    /// The mover's facing as the input found it, before the look session turned it: the word the
    /// motion handler's caller reads (`0x514446`, ahead of the mover lookup).
    pub(super) facing: f32,
}

/// The gates the driven body's descriptor sets on the input. Stunned is `UNIT_FIELD_FLAGS &
/// 0x40000` (`0x5145b0`), which skips the turn and pitch emitters (`0x514755`). Dead fails the
/// shared precondition `0x5144e0` (health above zero), so a corpse is stunned to the input tick; a
/// ghost has health 1. The stand state gates the mouse alone: `0x5145e0` wants the predicted
/// `GetStandState() == 0` (`0x5ed570`; [`super::StandPending`] is `[player+0x1d68]`).
pub(super) struct BodyGates {
    pub(super) stunned: bool,
    pub(super) stand_state: u8,
    pub(super) mover: state::MoverInput,
}

/// [`BodyGates`] off the driven body's descriptor; `self_driven` when the body is the active
/// player's, whose stand state is its prediction ([`posture`]), where a possessed body reads its own
/// byte.
pub(super) fn body_gates(
    store: Option<&crate::net::ObjectStore>,
    self_driven: bool,
    pending: &super::StandPending,
    lock: &super::MoverLock,
    view_subject: &view_subject::ViewSubject,
) -> BodyGates {
    let (stunned, dead, stand_byte) = store
        .map(|s| {
            (
                s.0.unit_flags() & UNIT_FLAG_STUNNED != 0,
                s.0.unit_is_dead(),
                s.0.unit_stand_state(),
            )
        })
        .unwrap_or((false, false, 0));
    let stand_state = if self_driven {
        pending.stand_state(stand_byte)
    } else {
        stand_byte
    };
    // The shared precondition `0x5144e0`, once per tick. Its far-sight conjunct,
    // `!(IsActivePlayer(mover) && [mover+0x1c70] & 1)`, reads the resolved subject (`0x5ee290`
    // latches only once resolved) and spares a possessed mover, which sets the same field.
    let mover = state::MoverInput {
        dead,
        view_is_out: state::view_is_out(
            lock.foreign_mover.is_none(),
            view_subject.remote.is_some(),
        ),
    };
    BodyGates {
        stunned,
        stand_state,
        mover,
    }
}

/// The camera dynamics' inputs, last frame's word as `0x50fee0` reads it: its caller `0x514446`
/// precedes the mover lookup. `body` is the driven body's descriptor and speeds; `facing` the
/// mover's facing as the frame's input found it.
#[allow(clippy::too_many_arguments)]
pub(super) fn dynamics_input(
    options: &camera_dynamics::CameraOptions,
    nearclip: f32,
    follow: &camera::FollowConfig,
    player: &Player,
    lock: &super::MoverLock,
    body: Option<(
        Option<&crate::net::ObjectStore>,
        Option<&crate::net::UnitSpeeds>,
    )>,
    follow_command: u32,
    scoped: bool,
    facing: f32,
) -> camera_dynamics::DynamicsInput {
    let (store, speeds) = body.unwrap_or((None, None));
    camera_dynamics::DynamicsInput {
        options: *options,
        nearclip,
        // Last frame's cached surface, the reference's own lag.
        surface_y: player.liquid_surface,
        smooth_style: follow.style,
        tracking_style: follow.tracking_style,
        subject: camera_dynamics::SubjectState {
            move_flags: player.move_flags,
            // The driven body's descriptor: a possessed creature's taxi state, not ours.
            taxi: store.is_some_and(|s| s.0.unit_flags() & 0x0010_0000 != 0),
            track: player.server_riding,
            fear: lock.control_lost,
            facing,
            mounted: store.is_some_and(|s| s.0.unit_mount_display_id() > 0),
            // `GetCurrentSpeed` (`0x7c4c90`) on the mover's speeds and last frame's word.
            speed: speeds.map_or(0.0, |s| crate::net::current_speed(&s.0, player.move_flags)),
            command: follow_command,
            scoped,
        },
    }
}

/// The mouse's input records: the look session ([`run_look_session`]), its turn of the camera and
/// its hand-off to the body, and the travel of the armed click ([`crate::target::ClickArm`]). The
/// reference's mouse handlers run in the frame's window input (`0x423920` at `0x420d23`): the
/// press `0x51491f`, the motion `0x514400` and the release `0x514b70`, ahead of the packets, so
/// this reads the body as the last frame left it. [`control`], the frame time's step, reads
/// what it handed on ([`LookSample`]).
#[allow(clippy::type_complexity)]
pub(super) fn sample_look(
    pointer: (
        Res<AccumulatedMouseMotion>,
        Res<camera::LookConfig>,
        Res<camera::FollowConfig>,
        Res<camera_dynamics::CameraOptions>,
        // Only `nearclip` is read here, for the dynamics.
        Res<benilla_world::view::ViewDistance>,
    ),
    gates: (
        Res<InspectMode>,
        Res<crate::bindings::BindingsState>,
        Res<view_subject::ViewSubject>,
        Res<scoped_view::ScopedView>,
        // The loading cover blanks input at the source; a gesture in flight is unwound below.
        Res<crate::loading_screen::LoadingScreen>,
        Res<super::StandPending>,
        Query<Has<Embodied>, With<crate::net::SelfPlayer>>,
    ),
    mut player: ResMut<Player>,
    lock: Res<super::MoverLock>,
    mut rig: ResMut<CameraControl>,
    mut cameras: Query<&mut FlyCam, With<Camera>>,
    body: Query<
        (
            Option<&crate::net::ObjectStore>,
            Option<&crate::net::UnitSpeeds>,
        ),
        (With<Embodied>, Without<FlyCam>),
    >,
    window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    // The click the mouse commands armed, whose travel the look's motion adds to.
    mut arm: ResMut<crate::target::ClickArm>,
    mut sample: ResMut<LookSample>,
) {
    let Ok(mut cam) = cameras.single_mut() else {
        return;
    };
    let (mut window, mut cursor_opts) = window.into_inner();
    let (inspect, binds, view_subject, scoped) = (&gates.0, &gates.1, &gates.2, &gates.3);
    let input::LookInput {
        both_buttons,
        follow_command,
    } = input::look_input(binds, &player, &lock, &rig);
    let body = body.single().ok();
    let dynamics = dynamics_input(
        &pointer.3,
        pointer.4.nearclip,
        &pointer.2,
        &player,
        &lock,
        body,
        follow_command,
        scoped.active(),
        player.face_yaw,
    );
    let BodyGates {
        stunned,
        stand_state,
        mover,
    } = body_gates(
        body.and_then(|(store, _)| store),
        gates.6.single().unwrap_or(false),
        &gates.5,
        &lock,
        view_subject,
    );
    // `0x5145e0`: `0x5145b0` and standing; a turn key stands a seated body up instead.
    let mouse_turns_body = mover.mouse_may_turn_body(stunned, stand_state);

    // A shadow copy, written back only on change: a `Mut` each frame marks `CursorOptions` changed,
    // and bevy_winit then re-applies the cursor to the OS every frame, which can stall.
    let mut opts_shadow = cursor_opts.bypass_change_detection().clone();
    // The facing before the look session, restored below when the mouse may not turn the body.
    let yaw_before_look = player.face_yaw;
    // A loading cover cancels a gesture in flight: its release must not click the world we just
    // left.
    if gates.4.covering() {
        arm.disarm();
    }
    let focused = window.focused;
    let looked = run_look_session(
        &pointer.0,
        both_buttons,
        &mut rig,
        &mut cam,
        &mut player.face_yaw,
        &mut window,
        &mut opts_shadow,
        inspect.enabled,
        &mut arm,
        *pointer.1,
        &dynamics,
        focused,
    );
    // The mouse turns the view, but the body hand-off (`0x514474`) skips a body that is stunned,
    // dead or seated (`0x5145e0`) or not ours to drive (`control_lost`, `reseat`), while the
    // camera rotate (`0x514444`) still runs. The reference never writes the facing; writing and
    // restoring is the same downstream. Standing up hands the camera's yaw back at the next
    // motion sample, a snap, as `0x5103e0` commits the camera's facing rather than a delta.
    if !mouse_turns_body || lock.control_lost || lock.reseat {
        player.face_yaw = yaw_before_look;
    }
    // The hand-off runs on each motion and on the press that completes both mouse bits, motion or
    // none (`0x514924`-`0x514966`), behind the same body gate `0x5145e0`.
    *sample = LookSample {
        hands_off: (looked || input::both_engaged(binds, &rig, both_buttons))
            && mouse_turns_body
            && !lock.control_lost
            && !lock.reseat,
        facing: yaw_before_look,
    };
    {
        let cur = cursor_opts.bypass_change_detection();
        if cur.visible != opts_shadow.visible
            || cur.grab_mode != opts_shadow.grab_mode
            || cur.hit_test != opts_shadow.hit_test
        {
            *cursor_opts = opts_shadow;
        }
    }
}

/// Free-flies until the server places us, then drives the avatar and streams its movement as the
/// mover; the dev chord with `F` toggles free fly.
#[allow(clippy::type_complexity)]
pub(super) fn control(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    // Tupled parameters keep this system under Bevy's limit of 16.
    pointer: (
        Res<AccumulatedMouseMotion>,
        Res<camera::LookConfig>,
        Res<camera_zoom::ZoomLimit>,
        Res<camera::FollowConfig>,
        Res<camera_dynamics::CameraOptions>,
        // Only `nearclip` is read here, for the self-avatar fade.
        Res<benilla_world::view::ViewDistance>,
        // Whether the live view is customised, the auto-return's view gate (`0x50fc30`).
        Option<Res<camera_view::CameraViews>>,
    ),
    mut net: (
        Res<NetCommands>,
        MessageReader<TeleportMessage>,
        MessageReader<WorldportMessage>,
        MessageWriter<crate::creature_anim::SheathRequest>,
        MessageReader<crate::net::SpeedChangeMessage>,
        // Root, water walk, feather fall and hover on our mover, acked with the live pose.
        MessageReader<crate::net::MoveModeMessage>,
        // The hard-landing report (`0x602d00`: the wound vocal and the dust).
        MessageWriter<crate::creature_anim::HardLanding>,
        // The mounted Space flourish plays locally at send; the net drain drops its echo.
        MessageWriter<crate::creature_anim::MountFlourish>,
        // A `MSG_MOVE_*` for our own mover: a pose the server wrote, no ack owed.
        MessageReader<crate::net::SelfMoveMessage>,
        MessageReader<StandStateRequest>,
        // Control of a unit granted or revoked: the mover claim and parting pose are ours to give.
        MessageReader<crate::net::ClientControlMessage>,
        // A knockback on our mover: latched by `wire_in`, flown below, acked after the launch.
        MessageReader<crate::net::KnockBackMessage>,
        crate::tutorial::InputHooks,
        // The loot window's move-start close (`0x60e990`).
        ResMut<crate::ui_loot::LootMoveStart>,
        // `SMSG_STANDSTATE_UPDATE`, applied locally and ungated, with nothing sent back.
        MessageReader<super::ServerStandState>,
    ),
    mut speed_capsule: (
        Res<MoveSpeed>,
        Res<PlayerCapsule>,
        // What the frame's mouse input handed on ([`sample_look`]).
        Res<LookSample>,
        Res<crate::ui_script::UiKeyboardCapture>,
        // Every rebindable input reads here; raw `keys` serve only free fly and its chord.
        Res<crate::bindings::BindingsState>,
        // The far-sight subject, resolved earlier since this system holds our `Transform` mutably.
        Res<view_subject::ViewSubject>,
        // Tells a revoked body from a granted creature in the control handoff.
        Res<crate::net::SelfGuid>,
        Res<scoped_view::ScopedView>,
        // Our own `[0xb4b3e4]`, which the control packet's handler writes.
        Res<super::OwnControlLost>,
        // The active player, whom the posture verbs read and act on whichever body is driven.
        SelfPosture,
        // `ChrClasses.dbc`, whose relic flag the sheath toggle's ranged candidate reads.
        Option<Res<crate::chr_classes::ChrClassTable>>,
        // The predicted stand state `[player+0x1d68]`, which the posture verbs write.
        ResMut<super::StandPending>,
    ),
    mut commands: Commands,
    mut player: ResMut<Player>,
    mut rig: ResMut<CameraControl>,
    collide: benilla_world::collision::WorldCollision,
    mut cameras: Query<(&mut Transform, &mut FlyCam), With<Camera>>,
    // The body we drive, a possessed creature's included ([`BodyQuery`]).
    mut body: BodyQuery,
    // The liquid query, the transports, the parent chain from a deck collider to its transport,
    // and the active mover `0xc4da98`, as whether it is our own character.
    mut world_q: (
        benilla_world::world_point::WorldPoint,
        TransportQuery,
        Query<&ChildOf>,
        Query<Has<crate::net::SelfPlayer>, With<crate::net::ActiveMover>>,
        // Our create's transport block and the transport it names, placed or not yet.
        (
            Query<&crate::transport::TransportRider, With<Embodied>>,
            Res<crate::net::GuidIndex>,
            Query<&Transform, (Without<Embodied>, Without<FlyCam>, Without<Camera>)>,
            Query<&crate::net::ObjectStore>,
        ),
        // The active mover global `SetActiveMover` (`0x6006e0`) compares a claim with.
        ResMut<crate::net::MoverClaim>,
        // Who we drive and whether we may.
        ResMut<super::MoverLock>,
    ),
    // The auto-move a root ack's tick stops, its release, and where its `AUTOFOLLOW_END` goes.
    mut auto_move: (
        Option<ResMut<super::FollowState>>,
        Option<ResMut<super::Approach>>,
        Option<ResMut<super::press::CallAutorun>>,
        Option<ResMut<crate::field_watch::NoticeLane>>,
    ),
) {
    let (world, transports, child_of) = (&world_q.0, &world_q.1, &world_q.2);
    let Ok((mut cam_t, mut cam)) = cameras.single_mut() else {
        return;
    };
    let mouse_motion = &pointer.0;
    let look_cfg = *pointer.1;
    let zoom_max = pointer.2.max;
    let (move_speed, capsule, sample, ui_capture) = (
        &speed_capsule.0,
        &speed_capsule.1 .0,
        &speed_capsule.2,
        &speed_capsule.3,
    );
    let binds = &speed_capsule.4;
    let view_subject = &speed_capsule.5;
    let self_guid = speed_capsule.6 .0;
    let scoped = &speed_capsule.7;
    // Far sight turns the auto-follow off: our facing is not "behind" for a body we only watch.
    let follow_cfg = camera::FollowConfig {
        style: if view_subject.remote.is_some() {
            camera::FollowStyle::Never
        } else {
            pointer.3.style
        },
        tracking_style: if view_subject.remote.is_some() {
            camera::FollowStyle::Never
        } else {
            pointer.3.tracking_style
        },
        view_customised: pointer.6.as_ref().is_some_and(|v| v.current_customised()),
        ..*pointer.3
    };
    let dt = time.delta_secs();
    // Free fly reads no keys while an EditBox is `typing` (not the per-key `consumed`, a binding's
    // concern); `binds` applied that gate when it latched, and the dev chords ignore it.
    let typing = ui_capture.typing;

    // Every press below comes from `camera::latch_world_mouse`, so one the UI ate reaches none of
    // them; the raw buttons only hold and release a gesture the world already owns. The
    // not-driving path seats its camera from this word and returns early.
    let input::LookInput {
        both_buttons,
        follow_command,
    } = input::look_input(binds, &player, &world_q.6, &rig);

    // The word as `0x50fee0` read it in the frame's input, with the facing the input found.
    let dynamics = dynamics_input(
        &pointer.4,
        pointer.5.nearclip,
        &pointer.3,
        &player,
        &world_q.6,
        body.single()
            .ok()
            .map(|(_, _, _, _, _, store, speeds, ..)| (store, speeds)),
        follow_command,
        scoped.active(),
        sample.facing,
    );
    let BodyGates {
        stunned,
        stand_state,
        mover,
    } = body_gates(
        body.single().ok().and_then(|(.., store, _, _, _, _)| store),
        speed_capsule
            .9
            .single()
            .is_some_and(|(.., embodied)| embodied),
        &speed_capsule.11,
        &world_q.6,
        view_subject,
    );
    // `0x5145b0` now, for the key turn; its translate sibling waits for this frame's root edge.
    let may_turn = mover.may_turn(stunned);
    // The drunk wobble, for the veer and the swim porpoise; zero when turning is refused, as the
    // reference's sits behind `0x5145b0` (`0x60aa47`).
    let drunk_wobble = {
        let f = body
            .single()
            .ok()
            .and_then(|(.., store, _, _, _, _)| store.and_then(|s| s.0.player_drunk_byte()))
            .map_or(0.0, drunk::fraction);
        if !may_turn {
            0.0
        } else {
            drunk::wobble(time.elapsed().as_millis() as u32, f)
        }
    };
    // The mouse's hand-off to the body, as the frame's input decided it, or for a
    // `MouselookStart` a packet's handler made after it that completed both mouse bits, which the
    // reference's press handler hands off inside the call (`0x514924`-`0x514966`): the camera's
    // facing here, its pitch below (`0x5103e0`).
    let late_hand_off = rig.world_mouse.late_rise()
        && both_buttons
        && mover.mouse_may_turn_body(stunned, stand_state)
        && !world_q.6.control_lost
        && !world_q.6.reseat;
    if late_hand_off {
        player.face_yaw = cam.yaw;
    }
    let hands_off = sample.hands_off || late_hand_off;

    // A rebound zoom key steps 1.0 per press, the stock `CameraZoomIn(1.0)` (`Bindings.xml:707`).
    let zoom = binds.amount(crate::bindings::Input::CameraZoomIn)
        - binds.amount(crate::bindings::Input::CameraZoomOut);
    apply_zoom_scroll(zoom, dt, &mut rig, zoom_max, pointer.2.move_speed);
    // The `MoveView*` axes, the camera update's `0x5112d0`, after this frame's calls.
    rig.view_axes.apply(binds.view_calls());
    camera_axes::tick(
        &mut rig,
        &mut cam,
        dt,
        look_cfg,
        pointer.2.move_speed,
        zoom_max,
    );

    // Free fly is a dev tool on the dev chord: a bare `F` is the player's to bind.
    if crate::run_mode::dev_chord(&keys, KeyCode::KeyF) {
        player.detached = !player.detached;
    }

    // The server's movement edges and their acks. Speed changes come back unacked while we drive,
    // for the movement stream to ack with the live pose.
    let server_moves = wire_in::apply_server_moves(
        &time,
        &mut commands,
        &mut player,
        &mut world_q.6,
        &mut cam,
        &net.0,
        &mut net.1,
        &mut net.2,
        &mut net.4,
        &mut net.5,
        &mut net.11,
        &mut net.8,
        &mut net.10,
        &mut world_q.5,
        self_guid,
        transports,
        body.single()
            .ok()
            .map(|(_, t, ..)| (t.translation, server_ride::yaw_of(t.rotation))),
        self_seat(&world_q.4),
    );
    let speed_acks = server_moves.speed_acks;
    // The root and unroot acks' input tick (`0x60e347`): a mover that may now neither translate
    // nor turn takes its teardown leg (`0x5146d6`), so a stun before its root stops the auto-move
    // here, at the root's ack. The steer ran ahead of this, so the release applies at once.
    if server_moves.input_ticked && mover.torn_down(player.modes.rooted, stunned) {
        let (follow, approach, autorun, lane) = &mut auto_move;
        super::end_auto_move(
            follow.as_deref_mut(),
            approach.as_deref_mut(),
            autorun.as_deref_mut(),
            lane.as_deref_mut(),
        );
        if let Some(follow) = follow.as_deref_mut() {
            super::follow::release_owed(follow, &mut player, autorun.as_deref());
        }
    }
    // A command press's message that ended the precast ([`super::press`]), with the acks'.
    let press_precast = std::mem::take(&mut player.press_ended_precast);
    if let (true, Ok((e, ..))) = (server_moves.precast_ended || press_precast, body.single()) {
        crate::creature_anim::end_precast(&mut commands, e, crate::creature_anim::PrecastTail::Run);
    }

    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();

    // On a transport, recompose the rider from the deck's pose this frame, before input integrates.
    ride::carry(&mut player, &mut cam, transports, &world_q.4 .1);

    if player.active && !player.detached {
        // The autorun toggle and the cast bar's self-cancel ran at the press, among the input
        // records ([`super::press`]).
        let active_mover = world_q.3.iter().next();
        // Not driving, while the camera stays on the body and input, physics and the stream yield:
        // - `server_riding`: a server spline (Charge, knockback, taxi, a flee path) owns the body;
        //   `drive_self_ride` has mirrored it into `Player`, reporting FORWARD on purpose.
        // - `control_lost`: someone else, or fear, drives our body. The server neither roots nor
        //   validates the victim, so this gate is the immobility.
        // - `reseat`: the mover guid is changing and the claimed unit has not streamed in; moves
        //   carry no guid, so driving would write one body's pose onto another.
        // Possession is none of these: it runs the ordinary path below, on the creature.
        if player.server_riding || world_q.6.control_lost || world_q.6.reseat {
            // The latch outlives the yield: lost control and an unresolved reseat leave the tick
            // no unit (`0x51466e`), but a ride keeps the mover and fails `0x5144e0`, so the tick
            // the ride's monster move runs (`0x602053`), and each press's, clears the bit
            // (`0x514748`).
            if player.server_riding && active_mover.is_some() {
                player.autorun = false;
            }
            // The moving body's transform is the truth; without this the camera stays behind
            // during a fear. Not while reseating, when `Player` still holds the body let go.
            if world_q.6.control_lost && !world_q.6.reseat {
                if let Ok((_, t, ..)) = body.single() {
                    let yaw = server_ride::yaw_of(t.rotation);
                    player.pos = t.translation;
                    player.face_yaw = yaw;
                    player.model_yaw = yaw;
                }
            }
            let head = player.pos + Vec3::Y * (CAPSULE_HEIGHT - CAPSULE_RADIUS);
            // Far sight outlives all three (Sentry Totem has no interrupt flags), and the
            // auto-follow runs through the reference's `Track` and `Fear` states.
            camera::seat_on_subject(
                dt,
                0.0,
                player.pos,
                head,
                body.single().ok().and_then(|(_, _, _, pivot, .., net)| {
                    // The pivot preset follows the body's own SWIMMING (`0x50f880`), here the
                    // last streamed word, which `wire_in` merges from the server's poses.
                    body_pose::pivot_target(
                        pivot,
                        net,
                        player.move_flags() & crate::creature_anim::move_flags::SWIMMING != 0,
                    )
                }),
                view_subject,
                &mut rig,
                &mut cam,
                &mut cam_t,
                &collide,
                &camera::FollowInput {
                    cfg: follow_cfg,
                    face_yaw: player.face_yaw,
                    command: follow_command,
                },
                &dynamics,
            );
            // Flush a stale run once, but never under a ride, whose FORWARD is deliberate.
            if !player.server_riding {
                movement_net::park_mover(&net.0 .0, &mut player);
            }
            // After the park, so a fear's ack carries the stopped word.
            movement_net::ack_speeds_undriven(&net.0 .0, &player, &speed_acks);
            return;
        }
        // `0x514560`, after `apply_server_moves`, so this frame's root edge is already in `modes`.
        let may_translate = mover.may_translate(player.modes.rooted);
        // Name an extra mouse button the bindings do not know, rather than fail silently.
        for b in buttons.get_just_pressed() {
            if let MouseButton::Other(n) = b {
                info!("mouse: unmapped button Other({n}) — bindings know BUTTON4/BUTTON5 as winit Forward/Back");
            }
        }
        let axes = input::move_axes(
            binds,
            &mut player,
            &rig,
            both_buttons,
            may_translate,
            may_turn,
        );
        let input::MoveAxes {
            fwd: fwd_axis,
            side: side_axis,
            mouselook,
            turning,
            translating,
            turn_left,
            turn_right,
            ..
        } = axes;
        if fwd_axis != 0 || side_axis != 0 {
            net.12.moved();
        }
        if mouselook {
            net.12.mouselooked();
        }
        // The driven unit's own speeds: the reference's input path never reads ours for another
        // mover, so a possessed creature moves and turns at its own.
        let mover_speeds = body.single().ok().and_then(|q| q.6).map(|s| s.0);
        // Zero is the ctor state, not a rate: a unit whose create block has not landed falls back.
        let turn_rate = mover_speeds
            .map(|s| s.turn_rate)
            .filter(|r| *r > 0.0)
            .unwrap_or(TURN_RATE);
        // This frame's own turn; `seat_camera` carries the camera by it, the two turning as one.
        let mut turn_delta = 0.0;
        if turning {
            let mut turn = 0.0;
            if turn_left {
                turn += 1.0;
            }
            if turn_right {
                turn -= 1.0;
            }
            // 0.75× while translating or falling (`flags & 0x200f`, `0x7c5c73`).
            let slowed = translating || player.airborne_since.is_some();
            let rate = turn_rate * if slowed { TURN_RATE_MOVING } else { 1.0 };
            turn_delta = turn * rate * dt;
            player.face_yaw += turn_delta;
        }
        // The drunk veer: while moving, the wobble adds to the facing every frame and commits
        // through the facing setter, so it streams like a turn (`0x60aa70`-`0x60aab7`,
        // `0x60de30`); a held keyboard turn skips it (`0x60aa5a`). Both yaw conventions grow
        // leftward, and it joins `turn_delta`, so the camera follows the meander.
        if drunk_wobble != 0.0 && translating && !turning {
            player.face_yaw += drunk_wobble;
            turn_delta = drunk_wobble;
        }
        let face_rot = Quat::from_rotation_y(player.face_yaw);
        let move_fwd = flat(face_rot * Vec3::NEG_Z);
        let move_right = flat(face_rot * Vec3::X);
        let mut dir = Vec3::ZERO;
        // One step in the net axis's sign, as the emitter issues one START in `sign(axis)`.
        match fwd_axis.signum() {
            1 => dir += move_fwd,
            -1 => dir -= move_fwd,
            _ => {}
        }
        // Strafe likewise; a cancelled pair is no strafe.
        match side_axis.signum() {
            1 => dir += move_right,
            -1 => dir -= move_right,
            _ => {}
        }
        // Translation dies here, but a root leaves turning live: the reference's allow-list permits
        // the turn, pitch, run/walk and SetFacing commands (`0x615c71`, `0x618054`). A stun, which
        // vmangos applies with a root (`SpellAuras.cpp:3502`), or death stops the turn as well.
        if !may_translate {
            dir = Vec3::ZERO;
        }
        let moving = dir != Vec3::ZERO;
        // A keyboard turn or the veer stands a seated body; a mouse turn does not, since the
        // facing commit refuses a seated player (`0x51460c`, `0x51520a`) and the turn emitter
        // skips its stand arm while the right button is held (`0x514f6d`). A right-click stands
        // you instead (`0x514ae0`, `camera::PressGesture::is_click`; `crate::target::click`).
        let turned = turn_delta != 0.0;
        // `TOGGLERUN` before the speed select, which reads the bit this frame's press left: the
        // mover reads `CMovement+0x40` after the input phase.
        walk::update(&mut player, &body, binds);
        // The stand state and the sheath toggle, which interlock; returns the committed stand.
        let reads = posture::PlayerReads {
            control_lost: speed_capsule.8 .0,
        };
        let stand_now = posture::update(
            &mut player,
            &mut speed_capsule.11,
            &body,
            &speed_capsule.9,
            binds,
            &net.0,
            &mut commands,
            &mut net.3,
            &mut net.13,
            &mut net.9,
            &mut net.14,
            reads,
            speed_capsule.10.as_deref().map(|t| &t.0),
            moving,
            turned,
        );
        // The backward flag selects the backward speed over strafe; a backward jump lands shorter.
        let net_backward = fwd_axis < 0;
        // The server's speeds (the LIVING block, then `SMSG_FORCE_*_SPEED_CHANGE`), or before the
        // create and under the `WOW_MOVE_SPEED` override, a set at the 2.5/4.5/7.0 ratios.
        let speeds = match mover_speeds {
            Some(s) if !move_speed.env_override => s,
            _ => benilla_protocol::MoveSpeeds {
                walk: move_speed.value * WALK_RATIO,
                run: move_speed.value,
                run_back: move_speed.value * RUN_BACK_RATIO,
                ..Default::default()
            },
        };
        // `GetCurrentSpeed` (`0x7c4c90`), shared with the remote extrapolator: the walk arm comes
        // before the backward min, so walking backwards is walk speed. It takes this frame's gait
        // intent, since the wire word is built after the mover.
        let speed = crate::net::current_speed(
            &speeds,
            if net_backward {
                move_flags::BACKWARD
            } else {
                move_flags::FORWARD
            } | if player.walking {
                move_flags::WALK_MODE
            } else {
                0
            },
        );
        // `Jump` (`0x513bd0`) inlines `0x5144e0` and `0x514560`, which is `may_translate` term for
        // term: health, root and stand state 7. Hover's refusal is the movement handler's
        // (`0x7c623a`), which keeps the mounted flourish reachable while hovering.
        let mut want_jump = binds.fired(crate::bindings::Input::Jump) && may_translate;

        // Swim or walk, latched with hysteresis at the `0x6030c0` boundary against flicker.
        let surface_y = swim::surface_over_feet(world, player.pos);
        // For the camera's water corridor, which reads it a frame late, as `0x670630` does.
        player.liquid_surface = surface_y;
        let swimming = swim::update_swimming(&mut player, surface_y, time.elapsed_secs());
        if let Some(surface) = surface_y {
            move_trace::swim(player.pos.y, surface, swimming, player.collision_height.0);
        }
        // Space while swimming is the Jump command (`0x7c6230`), once per press: it breaches at
        // the surface and hops about 1.6 yd below it, re-latching swim once the launch halves
        // (`0x7c5de0`). It clears SWIMMING before the mover, so this frame's mover, flags and wire
        // see a jump. Hover refuses it (`0x7c623a`, ahead of the take-off select at `0x7c6261`),
        // and hover does not stop swim entry (`0x6030c0` tests only LEVITATING).
        //
        // The `Jump(force = 0)` a hover grant owes: `force` 0 skips the hover refusal (`0x7c6236`),
        // so it launches a body already hovering; root and falling still refuse it (`0x7c625c`).
        let wire_jump = player.take_wire_jump();
        // The knockback: horizontal `(cos, sin) · xy_speed` in absolute world XY, vertical
        // `−zspeed`, since the wire's take-off speed is down-positive, as in the jump tail.
        let knockback = player.take_knockback();
        let knock_launch = knockback.map(|k| {
            let (c, sn, xy) = (k.launch.cos_angle, k.launch.sin_angle, k.launch.xy_speed);
            wow_to_bevy([c * xy, sn * xy, -k.launch.zspeed])
        });

        // A knockback sets FALLING, which excludes SWIMMING; it brings its own launch, so the land
        // mover flies it, not `breach_step`.
        let breach = swimming && (want_jump && !player.modes.hover || wire_jump);
        let knock_breach = swimming && knock_launch.is_some();
        if breach || knock_breach {
            player.swimming = false;
        }
        let swimming = swimming && !breach && !knock_breach;

        // Read by both the swim mover and the flag build, so the two cannot disagree.
        let (swim_fwd, swim_side) = if swimming {
            swim::translate_amounts(&axes, !may_translate)
        } else {
            (0.0, 0.0)
        };

        // The pitch keys: the input tick's pitch emitter (`0x515010`, behind `0x514755` and the
        // swim test `0x514766`), then the mover's pitch at the pitch rate (`0x7c4f80`), before
        // the swim mover reads it.
        swim::pitch_emitter(
            &mut player,
            (
                binds.pressed(crate::bindings::Input::PitchUp),
                binds.pressed(crate::bindings::Input::PitchDown),
            ),
            follow_command & camera::follow_cmd::RIGHT_MOUSE != 0,
            may_turn,
            swimming,
        );
        // The integration runs in any mode the bits survive into (`0x7c53b9`), at three quarters
        // under a direction bit or FALLING (`0x200f`, `0x7c5cb3`).
        let pitch_integrating = if swimming {
            swim_fwd != 0.0 || swim_side != 0.0
        } else {
            (may_translate && (fwd_axis != 0 || side_axis != 0)) || player.airborne_since.is_some()
        };
        swim::integrate_pitch_keys(&mut player, turn_rate, pitch_integrating, dt);

        // The mounted Space flourish, the jump-key handler `0x60dea0`: mounted, still and grounded
        // plays MountSpecial (94) locally, then sends `CMSG_MOUNTSPECIAL_ANIM`; moving jumps;
        // turning in place eats the press. Airborne, the press falls through and the mover drops
        // it (the reference's ground-clearance test `0x605650`; the airborne arc stands in).
        if want_jump && !moving && !swimming && player.airborne_since.is_none() {
            if let Ok((e, .., store, _, _, _, _)) = body.single() {
                if store.is_some_and(|s| s.0.unit_mount_display_id() != 0) {
                    want_jump = false;
                    if !turning {
                        let _ = net.0 .0.send(ClientCommand::MountSpecial);
                        net.7.write(crate::creature_anim::MountFlourish { unit: e });
                    }
                }
            }
        }

        // The mover pitch (`CMovement+0x20`), in every mode: held when unsteered (`0x7c4f80`) and
        // set by mouse look to the aim ([`swim::hand_off_pitch`]), whose write precedes its
        // SWIMMING test (`0x7c6f91`). A left-drag orbit steers nothing. It is written per mouse
        // motion (event `0x400500cb`), not per frame, so other writers survive a still mouse. The
        // aim includes the pivot bias, as `0x5103e0` passes `[cam+0x104] + [cam+0xf4]`.
        if hands_off {
            swim::hand_off_pitch(&mut player, cam.pitch + rig.smart_pivot.bias());
        }
        let mut swim_pitch = 0.0_f32;
        // Pre-step feet Y, the true take-off height: the step already rises one jump tick.
        let launch_y = player.pos.y;
        let mover::Outcome {
            held,
            grounded,
            jumped,
            knocked,
            air_nudged,
            ground,
            gated,
        } = if breach {
            // Jump while swimming (`0x7c6230`): falls unconditionally, seeded about 14% over a land
            // jump, and streams as a normal jump.
            swim::breach_step(&mut player, &time, &collide, capsule)
        } else if swimming {
            // One swimming frame; it also picks the presented pitch, levelled at the surface cap.
            let frame = swim::drive_step(
                &mut player,
                &time,
                &collide,
                capsule,
                world,
                surface_y,
                (move_fwd, move_right),
                (swim_fwd, swim_side),
                mover_speeds,
                move_speed,
                if translating { drunk_wobble } else { 0.0 },
            );
            swim_pitch = frame.pitch;
            frame.outcome
        } else {
            // Water walking hands the mover the liquid surface as ground ([`mover::water_floor`]),
            // where the reference ORs the liquid layers into the walk trace's class mask
            // (`0x63162e`); liquid is queried here rather than swept.
            let water_floor = mover::water_floor(
                player.modes.water_walking,
                swimming,
                player.mover_pitch,
                surface_y,
            );
            mover::step(
                &mut player,
                &time,
                &collide,
                capsule,
                moving,
                dir,
                speed,
                want_jump,
                wire_jump,
                knock_launch,
                water_floor,
                // The air nudge speed is `min(walk, run)`, live (`0x7c4c90(1)`, `0x7c4d19`), with
                // the default walk speed until the server sends speeds.
                mover_speeds.map_or(super::AIR_NUDGE_SPEED, |s| s.walk.min(s.run)),
            )
        };

        // Traced here, the first point that knows both the latch and the mover's verdict.
        if let Some(k) = knockback {
            move_trace::knockback(knocked, k.launch);
        }

        let now = time.elapsed_secs();
        if gated {
            player.skip_arc_time(dt);
        }
        if gated != player.gated {
            player.gated = gated;
            move_trace::gate(gated, player.pos);
        }
        // Our FALLING: never while swimming, and a root ends it outright (`SetRoot` `0x7c7340`
        // calls `StopFalling` `0x7c6290`), leaving the body hanging; `grounded` keeps true contact.
        let airborne = !swimming && !held && !player.modes.rooted && (!grounded || jumped);
        // Airborne keeps the attachment, so a jump above a deck lands where it took off.
        ride::update_attachment(
            &mut player,
            transports,
            &world_q.4 .1,
            child_of,
            ground,
            grounded,
            swimming,
        );
        // The live flag word, the take-off-frozen one the animation reads, and the fall clock.
        let flags::FrameFlags {
            wire: move_flags_now,
            pose: pose_flags,
            landed,
            fall_time: wire_fall_time,
        } = flags::this_frame(
            &mut player,
            &axes,
            swimming.then_some((swim_fwd, swim_side)),
            airborne,
            jumped,
            held,
            air_nudged,
            may_translate,
            may_turn,
            now,
            launch_y,
        );
        let anim_flags = gait::drive_body_heading(
            &mut player,
            pose_flags,
            dt,
            swimming,
            moving,
            airborne,
            turning || mouselook,
            turn_rate,
        );
        let cam_pivot_target = body_pose::drive(
            &player,
            &mut body,
            &mut net.6,
            swimming,
            swim_pitch,
            move_flags_now,
            anim_flags,
            landed,
            stand_now,
        );

        // The camera sweep's root: the capsule's top hemisphere centre, not the framing pivot.
        let head = player.pos + Vec3::Y * (CAPSULE_HEIGHT - CAPSULE_RADIUS);
        // The spyglass lock (aura 76): re-parking each frame stands in for the reference's camera
        // flag `0x8`, which makes `SetCameraView` return early.
        if scoped.active() {
            rig.park_distance(0.0);
        }
        // Far sight moves only the picture: `seat_on_subject` orbits the object, all else runs.

        // `WOW_CAM_DUMP`: this frame's input beside `seat_camera`'s realized `[cam]` line.
        if crate::player::camera::cam_dump_enabled() {
            eprintln!(
                "[turn] t={:.6} dt={:.6} dx={:.3} dy={:.3} look={} face={:.6} model={:.6} \
                 pos [{:.4},{:.4},{:.4}] pivot={:.4}->{:.4}",
                time.elapsed_secs_f64(),
                dt,
                mouse_motion.delta.x,
                mouse_motion.delta.y,
                match rig.look {
                    Some(LookButton::Right) => "R",
                    Some(LookButton::Left) => "L",
                    None => "-",
                },
                player.face_yaw,
                player.model_yaw,
                player.pos.x,
                player.pos.y,
                player.pos.z,
                // The pivot channel's live height and its target.
                rig.pivot.probe().0,
                rig.pivot.probe().1,
            );
        }
        // `turn_delta` is our own turn; the deck's yaw reached `cam.yaw` in the ride carry.
        let follow = camera::FollowInput {
            cfg: follow_cfg,
            face_yaw: player.face_yaw,
            command: follow_command,
        };
        camera::seat_on_subject(
            dt,
            turn_delta,
            player.pos,
            head,
            cam_pivot_target,
            view_subject,
            &mut rig,
            &mut cam,
            &mut cam_t,
            &collide,
            &follow,
            &dynamics,
        );

        let steering_ourselves = world_q.6.foreign_mover.is_none();
        // The loot window's walk-away, the movement-start guard `0x60e990`: forward, back, strafe,
        // keyboard turn, pitch and jump close it, mouse-look facing does not, and only for our
        // own character.
        if steering_ourselves && (loot_walk_away(player.move_flags, move_flags_now) || jumped) {
            net.13 .0 = true;
        }

        // Stream the movement: a `MSG_MOVE_*` per axis transition, the jump and fall edges and a
        // 500 ms heartbeat, which vmangos relays to nearby players.
        let wire_transport = movement_net::wire_transport(&player);
        // A held frame is simulation time skipped; the mover named is the one we drive.
        let skip = movement_net::SkipClock {
            dt,
            held: player.settling,
            gated,
            mover: world_q.6.foreign_mover.or(self_guid),
        };
        let precast_ended = movement_net::stream_self_movement(
            &net.0 .0,
            &mut player,
            move_flags_now,
            swim_pitch,
            movement_net::ArcEdges {
                jumped,
                // Wire launches send no `MSG_MOVE_JUMP`: the hover grant's jump sends nothing
                // (`0x61a620`) and a knockback sends its ack instead.
                wire_launch: knocked || wire_jump,
                air_nudged,
                landed,
                fall_time: wire_fall_time,
            },
            now,
            &speed_acks,
            // Acked only if flown: under root the reference drops the record unacked (`0x615c71`).
            knockback.filter(|_| knocked),
            wire_transport,
            skip,
        );
        if let (true, Ok((e, ..))) = (precast_ended, body.single()) {
            crate::creature_anim::end_precast(
                &mut commands,
                e,
                crate::creature_anim::PrecastTail::Run,
            );
        }
    } else {
        // Free fly. Park the mover first, so a detach mid-move leaves no stale flags for observers
        // to extrapolate; a no-op once stopped.
        movement_net::park_mover(&net.0 .0, &mut player);
        camera::fly_free(dt, &keys, typing, &mut rig, &mut cam, &mut cam_t);
    }
}

/// Whether a new direction or keyboard-turn bit, `prev` to `now`, closes the loot window: the
/// movement-start guard `0x60e990` the local movement commands call first.
pub(super) fn loot_walk_away(prev: u32, now: u32) -> bool {
    now & (move_flags::ANY_MOVE | move_flags::TURN_LEFT | move_flags::TURN_RIGHT) & !prev != 0
}

/// Our create's transport block, the [`crate::transport::TransportRider`] tail on the body we
/// steer, and the streamed transport it names, as [`wire_in::apply_server_moves`] seats a login
/// aboard it.
#[allow(clippy::type_complexity)]
fn self_seat(
    q: &(
        Query<&crate::transport::TransportRider, With<Embodied>>,
        Res<crate::net::GuidIndex>,
        Query<&Transform, (Without<Embodied>, Without<FlyCam>, Without<Camera>)>,
        Query<&crate::net::ObjectStore>,
    ),
) -> Option<wire_in::SelfSeat> {
    let rider = q.0.single().ok()?;
    let entity = *q.1 .0.get(&rider.transport_guid)?;
    // 0x630a80: the lookup `0x468460` with the game object mask 0x20, then the transport bit 0x8
    // of its flags.
    let fields = &q.3.get(entity).ok()?.0;
    if fields.object_type_mask().unwrap_or(0) & 0x20 == 0 || fields.gameobject_flags() & 0x8 == 0 {
        return None;
    }
    Some(wire_in::SelfSeat {
        entity,
        guid: rider.transport_guid,
        local_pos: rider.local_pos,
        local_orientation: rider.local_orientation,
        deck: *q.2.get(entity).ok()?,
    })
}
