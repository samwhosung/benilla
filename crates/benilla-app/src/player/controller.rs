//! The per-frame player controller, [`control`]: this frame's input to the avatar's pose, its
//! animation state and the packets owed the server. The phases live in sibling modules; their order
//! here is the reference's frame order and is load-bearing (the swim latch before the mover, the
//! posture commit before the pose, the ack after the launch).

use super::*;

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
        Res<camera::ZoomLimit>,
        Res<camera::FollowConfig>,
        Res<camera_dynamics::CameraOptions>,
        // Only `nearclip` is read here, for the self-avatar fade.
        Res<benilla_world::view::ViewDistance>,
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
        // The move edges the reference hands `AbortCast` (`0x6e4940`), for the cast bar.
        ResMut<crate::spell::LocalMoveStart>,
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
    speed_capsule: (
        Res<MoveSpeed>,
        Res<PlayerCapsule>,
        Res<InspectMode>,
        Res<crate::ui_script::UiKeyboardCapture>,
        Res<crate::ui_script::PlayerUiClickConsumed>,
        // Every rebindable input reads here; raw `keys` serve only free fly and its chord.
        Res<crate::bindings::BindingsState>,
        // The far-sight subject, resolved earlier since this system holds our `Transform` mutably.
        Res<view_subject::ViewSubject>,
        // Tells a revoked body from a granted creature in the control handoff.
        Res<crate::net::SelfGuid>,
        Res<scoped_view::ScopedView>,
        // The loading cover blanks input at the source; a gesture in flight is unwound below.
        Res<crate::loading_screen::LoadingScreen>,
    ),
    mut commands: Commands,
    mut player: ResMut<Player>,
    mut rig: ResMut<CameraControl>,
    collide: benilla_world::collision::WorldCollision,
    mut cameras: Query<(&mut Transform, &mut FlyCam), With<Camera>>,
    // The body we drive, a possessed creature's included ([`BodyQuery`]).
    mut body: BodyQuery,
    window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    // A press starts a look and arms a click test, which the release settles on time and travel.
    mut world_clicks: (
        MessageWriter<WorldClick>,
        MessageWriter<WorldRightClick>,
        MessageWriter<WorldRightPress>,
    ),
    mut click_test: (
        Local<Option<camera::PressGesture>>,
        Local<Option<camera::PressGesture>>,
    ),
    // The liquid query, the transports, and the parent chain from a deck collider to its transport.
    world_q: (
        benilla_world::world_point::WorldPoint,
        TransportQuery,
        Query<&ChildOf>,
    ),
) {
    let (world, transports, child_of) = (&world_q.0, &world_q.1, &world_q.2);
    let (left_click, right_click) = (&mut *click_test.0, &mut *click_test.1);
    let Ok((mut cam_t, mut cam)) = cameras.single_mut() else {
        return;
    };
    let (mut window, mut cursor_opts) = window.into_inner();
    let mouse_motion = &pointer.0;
    let look_cfg = *pointer.1;
    let zoom_max = pointer.2.max;
    let (move_speed, capsule, inspect, ui_capture, click_consumed) = (
        &speed_capsule.0,
        &speed_capsule.1 .0,
        &speed_capsule.2,
        &speed_capsule.3,
        &speed_capsule.4,
    );
    let binds = &speed_capsule.5;
    let view_subject = &speed_capsule.6;
    let self_guid = speed_capsule.7 .0;
    let scoped = &speed_capsule.8;
    let covered = speed_capsule.9.covering();
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
        ..*pointer.3
    };
    let dt = time.delta_secs();
    // Free fly reads no keys while an EditBox is `typing` (not the per-key `consumed`, a binding's
    // concern); `binds` applied that gate when it latched, and the dev chords ignore it.
    let typing = ui_capture.typing;

    // Every press below comes from `camera::latch_world_mouse`, so one the UI ate reaches none of
    // them; the raw buttons only hold and release a gesture the world already owns. Read before
    // the look session: the not-driving path seats its camera from this word and returns early.
    let input::LookInput {
        both_buttons,
        follow_command,
    } = input::look_input(binds, &player, &rig);

    // Last frame's word, as `0x50fee0` reads it: its caller `0x514446` precedes the mover lookup.
    let dynamics = camera_dynamics::DynamicsInput {
        options: *pointer.4,
        nearclip: pointer.5.nearclip,
        // Last frame's cached surface, the reference's own lag.
        surface_y: player.liquid_surface,
        smooth_style: pointer.3.style,
        tracking_style: pointer.3.tracking_style,
        subject: camera_dynamics::SubjectState {
            move_flags: player.move_flags,
            // The driven body's descriptor: a possessed creature's taxi state, not ours.
            taxi: body
                .single()
                .ok()
                .and_then(|(_, _, _, _, _, store, ..)| store)
                .is_some_and(|s| s.0.unit_flags() & 0x0010_0000 != 0),
            track: player.server_riding,
            fear: player.control_lost,
            facing: player.face_yaw,
            mounted: body
                .single()
                .ok()
                .and_then(|(_, _, _, _, _, store, ..)| store)
                .is_some_and(|s| s.0.unit_mount_display_id() > 0),
            // `GetCurrentSpeed` (`0x7c4c90`) on the mover's speeds and last frame's word.
            speed: body
                .single()
                .ok()
                .and_then(|(_, _, _, _, _, _, _, speeds, ..)| speeds)
                .map_or(0.0, |s| crate::net::current_speed(&s.0, player.move_flags)),
            command: follow_command,
            scoped: scoped.active(),
        },
    };

    // A shadow copy, written back only on change: a `Mut` each frame marks `CursorOptions` changed,
    // and bevy_winit then re-applies the cursor to the OS every frame, which can stall.
    let mut opts_shadow = cursor_opts.bypass_change_detection().clone();
    // The facing before the look session, restored below when the mouse may not turn the body.
    let yaw_before_look = player.face_yaw;
    // Off the driven body's descriptor. Stunned is `UNIT_FIELD_FLAGS & 0x40000` (`0x5145b0`),
    // which skips the turn and pitch emitters (`0x514755`). Dead fails the shared precondition
    // `0x5144e0` (health above zero), so a corpse is stunned to the input tick; a ghost has
    // health 1. The stand state gates the mouse alone: `0x5145e0` wants the predicted
    // `GetStandState() == 0` (`0x5ed570`; `stand_pending` is `[player+0x1d68]`).
    let (stunned, dead, stand_byte) = body
        .single()
        .ok()
        .and_then(|(.., store, _, _, _, _, _)| {
            store.map(|s| {
                (
                    s.0.unit_flags() & UNIT_FLAG_STUNNED != 0,
                    s.0.unit_is_dead(),
                    s.0.unit_stand_state(),
                )
            })
        })
        .unwrap_or((false, false, 0));
    let stand_state = player.stand_pending.unwrap_or(stand_byte);
    // The shared precondition `0x5144e0`, once per tick. Its far-sight conjunct,
    // `!(IsActivePlayer(mover) && [mover+0x1c70] & 1)`, reads the resolved subject (`0x5ee290`
    // latches only once resolved) and spares a possessed mover, which sets the same field.
    let mover = state::MoverInput {
        dead,
        view_is_out: state::view_is_out(
            player.foreign_mover.is_none(),
            view_subject.remote.is_some(),
        ),
    };
    // `0x5145b0` now, for the mouse turn; its translate sibling waits for this frame's root edge.
    let may_turn = mover.may_turn(stunned);
    // `0x5145e0`: `may_turn` and standing; a turn key stands a seated body up instead.
    let mouse_turns_body = mover.mouse_may_turn_body(stunned, stand_state);
    // The drunk wobble, for the veer and the swim porpoise; zero when turning is refused, as the
    // reference's sits behind `0x5145b0` (`0x60aa47`).
    let drunk_wobble = {
        let f = body
            .single()
            .ok()
            .and_then(|(.., store, _, _, _, _, _)| store.and_then(|s| s.0.player_drunk_byte()))
            .map_or(0.0, drunk::fraction);
        if !may_turn {
            0.0
        } else {
            drunk::wobble(time.elapsed().as_millis() as u32, f)
        }
    };
    // A loading cover cancels a gesture in flight: ending the look session would settle its click
    // test and click the world we just left, so the tests are dropped first.
    if covered {
        *left_click = None;
        *right_click = None;
    }
    run_look_session(
        &buttons,
        mouse_motion,
        both_buttons,
        &mut rig,
        &mut cam,
        &mut player.face_yaw,
        &mut window,
        &mut opts_shadow,
        inspect.enabled,
        click_consumed.0,
        &mut world_clicks.0,
        &mut world_clicks.1,
        &mut world_clicks.2,
        left_click,
        right_click,
        look_cfg,
        &dynamics,
        time.elapsed_secs(),
    );
    // The mouse turns the view, but the body hand-off (`0x514474`) skips a body that is stunned,
    // dead or seated (`0x5145e0`) or not ours to drive (`control_lost`, `reseat`), while the
    // camera rotate (`0x514444`) still runs. The reference never writes the facing; writing and
    // restoring is the same downstream. Standing up hands the camera's yaw back at the next
    // motion sample, a snap, as `0x5103e0` commits the camera's facing rather than a delta.
    if !mouse_turns_body || player.control_lost || player.reseat {
        player.face_yaw = yaw_before_look;
    }
    {
        let cur = cursor_opts.bypass_change_detection();
        if cur.visible != opts_shadow.visible
            || cur.grab_mode != opts_shadow.grab_mode
            || cur.hit_test != opts_shadow.hit_test
        {
            *cursor_opts = opts_shadow;
        }
    }

    // A rebound zoom key steps 1.0 per press, the stock `CameraZoomIn(1.0)` (`Bindings.xml:707`).
    let zoom = binds.amount(crate::bindings::cmd::CAMERA_ZOOM_IN)
        - binds.amount(crate::bindings::cmd::CAMERA_ZOOM_OUT);
    apply_zoom_scroll(zoom, dt, &mut rig, zoom_max);

    // Free fly is a dev tool on the dev chord: a bare `F` is the player's to bind.
    if crate::run_mode::dev_chord(&keys, KeyCode::KeyF) {
        player.detached = !player.detached;
    }

    // The server's movement edges and their acks. Speed changes come back unacked while we drive,
    // for the movement stream to ack with the live pose.
    let speed_acks = wire_in::apply_server_moves(
        &time,
        &mut commands,
        &mut player,
        &mut cam,
        &net.0,
        &mut net.1,
        &mut net.2,
        &mut net.4,
        &mut net.5,
        &mut net.12,
        &mut net.9,
        &mut net.11,
        self_guid,
        transports,
        body.single()
            .ok()
            .map(|(_, t, ..)| (t.translation, server_ride::yaw_of(t.rotation))),
    );

    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();

    // On a transport, recompose the rider from the deck's pose this frame, before input integrates.
    ride::carry(&mut player, &mut cam, transports);

    if player.active && !player.detached {
        // Not driving, while the camera stays on the body and input, physics and the stream yield:
        // - `server_riding`: a server spline (Charge, knockback, taxi, a flee path) owns the body;
        //   `drive_self_ride` has mirrored it into `Player`, reporting FORWARD on purpose.
        // - `control_lost`: someone else, or fear, drives our body. The server neither roots nor
        //   validates the victim, so this gate is the immobility.
        // - `reseat`: the mover guid is changing and the claimed unit has not streamed in; moves
        //   carry no guid, so driving would write one body's pose onto another.
        // Possession is none of these: it runs the ordinary path below, on the creature.
        if player.server_riding || player.control_lost || player.reseat {
            // The moving body's transform is the truth; without this the camera stays behind
            // during a fear. Not while reseating, when `Player` still holds the body let go.
            if player.control_lost && !player.reseat {
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
        let axes = input::move_axes(
            binds,
            &buttons,
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
            autorun_armed,
            turn_left,
            turn_right,
            ..
        } = axes;
        if fwd_axis != 0 || side_axis != 0 {
            net.13.moved();
        }
        if mouselook {
            net.13.mouselooked();
        }
        // The driven unit's own speeds: the reference's input path never reads ours for another
        // mover, so a possessed creature moves and turns at its own.
        let mover_speeds = body.single().ok().and_then(|q| q.7).map(|s| s.0);
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
        let stand_now = posture::update(
            &mut player,
            &body,
            binds,
            &net.0,
            &mut net.3,
            &mut net.10,
            &mut net.15,
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
        let mut want_jump = binds.fired(crate::bindings::cmd::JUMP) && may_translate;

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

        // The mounted Space flourish, the jump-key handler `0x60dea0`: mounted, still and grounded
        // plays MountSpecial (94) locally, then sends `CMSG_MOUNTSPECIAL_ANIM`; moving jumps;
        // turning in place eats the press. Airborne, the press falls through and the mover drops
        // it (the reference's ground-clearance test `0x605650`; the airborne arc stands in).
        if want_jump && !moving && !swimming && player.airborne_since.is_none() {
            if let Ok((e, .., store, _, _, _, _, _)) = body.single() {
                if store.is_some_and(|s| s.0.unit_mount_display_id() != 0) {
                    want_jump = false;
                    if !turning {
                        let _ = net.0 .0.send(ClientCommand::MountSpecial);
                        net.8.write(crate::creature_anim::MountFlourish { unit: e });
                    }
                }
            }
        }

        // The mover pitch (`CMovement+0x20`), in every mode: held when unsteered (`0x7c4f80`) and
        // set by mouse look to the aim through `SetPitch` (`0x7c6f70`), an unconditional store
        // clamped ±89° (the ±π/2 clamp is the pitch keys'), whose write precedes its SWIMMING test
        // (`0x7c6f91`). A left-drag orbit steers nothing. It is written per mouse motion (event
        // `0x400500cb`), not per frame, so other writers survive a still mouse. The aim includes
        // the pivot bias, as `0x5103e0` passes `[cam+0x104] + [cam+0xf4]`.
        let aim_pitch = cam.pitch + rig.smart_pivot.bias();
        if mouselook && aim_pitch != player.aim_pitch_seen {
            player.aim_pitch_seen = aim_pitch;
            player.mover_pitch = aim_pitch.clamp(-MOUSELOOK_PITCH_CLAMP, MOUSELOOK_PITCH_CLAMP);
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
        // Our FALLING: never while swimming, and a root ends it outright (`SetRoot` `0x7c7340`
        // calls `StopFalling` `0x7c6290`), leaving the body hanging; `grounded` keeps true contact.
        let airborne = !swimming && !held && !player.modes.rooted && (!grounded || jumped);
        // Airborne keeps the attachment, so a jump above a deck lands where it took off.
        ride::update_attachment(
            &mut player,
            transports,
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

        // The cast bar's self-cancel: a new directional start, a jump, or autorun's on edge (the
        // interrupt mask `0x10f0` at `0x5150ce` is forward, back, strafe and autorun, and a clear
        // edge returns early at `0x5150c8`); turning and pitch never cancel. Only our own
        // character's moves count: vmangos breaks a channel on the caster's own movement
        // (`Spell::update`), so a possessed creature's steps must not end Mind Control.
        let steering_ourselves = player.foreign_mover.is_none();
        if steering_ourselves
            && (move_flags_now & move_flags::ANY_MOVE & !player.move_flags != 0
                || jumped
                || autorun_armed)
        {
            net.7 .0 = true;
        }
        // The loot window's walk-away, the movement-start guard `0x60e990`: forward, back, strafe,
        // keyboard turn, pitch and jump close it, mouse-look facing does not, and only for our
        // own character.
        if steering_ourselves
            && (move_flags_now
                & (move_flags::ANY_MOVE | move_flags::TURN_LEFT | move_flags::TURN_RIGHT)
                & !player.move_flags
                != 0
                || jumped)
        {
            net.14 .0 = true;
        }

        // Stream the movement: a `MSG_MOVE_*` per axis transition, the jump and fall edges and a
        // 500 ms heartbeat, which vmangos relays to nearby players.
        let wire_transport = movement_net::wire_transport(&player);
        // A held frame is simulation time skipped; the mover named is the one we drive.
        let skip = movement_net::SkipClock {
            dt,
            held: player.settling,
            mover: player.foreign_mover.or(self_guid),
        };
        movement_net::stream_self_movement(
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
    } else {
        // Free fly. Park the mover first, so a detach mid-move leaves no stale flags for observers
        // to extrapolate; a no-op once stopped.
        movement_net::park_mover(&net.0 .0, &mut player);
        camera::fly_free(dt, &keys, typing, &mut rig, &mut cam, &mut cam_t);
    }
}
