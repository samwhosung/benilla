//! **The per-frame player controller** — [`control`], the one system that turns this frame's input
//! into the avatar's pose, its animation state and the packets we owe the server.
//!
//! It reads as a spine of named phases, and every phase that is a *concern of its own* lives in a
//! module beside this one: [`super::input`] decodes the keys, [`super::wire_in`] applies what the
//! server said, [`super::ride`] carries us on a deck, [`super::posture`] holds the body,
//! [`super::mover`]/[`super::swim`] move it, [`super::flags`] builds the word, [`super::gait`] and
//! [`super::body_pose`] render it, [`super::camera`] seats the eye, and [`super::movement_net`]
//! streams it. What is left here is the *order* — which is itself load-bearing, because the
//! reference's own frame runs in this order and several of the gates below only work where they
//! stand (the swim latch before the mover, the posture commit before the pose, the ack after the
//! launch).
//!
//! Split out of the module root by the same rule the rest of `player/` follows: the root is the
//! map (the plugin, the shared types, the ordering edges), the concerns are the files.

use super::*;

/// Camera + avatar controller. Free-flies until the server reports our position; then takes
/// third-person control (WASD walks the avatar; right-drag turns it, left-drag orbits the camera,
/// wheel zooms) and streams our movement to the server as the confirmed mover. The dev chord + `F`
/// toggles free-fly.
#[allow(clippy::type_complexity)]
pub(super) fn control(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    // Nested into one param to stay within Bevy's 16-element system-param tuple limit. (The
    // scroll wheel left this tuple with 0997: zoom reads the CAMERAZOOM bindings now.)
    // The pointer's motion this frame + the camera knobs that scale what it does with it — the
    // two look/zoom knobs and the auto-follow style. Bundled because a Bevy system
    // takes at most 16 parameters and this one is at the ceiling — the grouping is the existing
    // `mouse` tuple, widened rather than a seventeenth argument.
    pointer: (
        Res<AccumulatedMouseMotion>,
        Res<camera::LookConfig>,
        Res<camera::ZoomLimit>,
        Res<camera::FollowConfig>,
        // The four 1.12 camera option toggles and their numeric siblings.
        Res<camera_dynamics::CameraOptions>,
        // `nearclip`/`farclip` (2163) — the camera's own clip pair. Only the near half is read
        // here, for the self-avatar fade's reference plane; the far half is the wall's, read by
        // the world.
        Res<benilla_world::view::ViewDistance>,
    ),
    // The net bridge, bundled into one param (16-param limit): the outbound command channel + the
    // inbound teleport/worldport messages `apply_net_updates` wrote earlier this frame
    // (WorldStage::Net), + the sheath-setter queue (the Z toggle's request).
    mut net: (
        Res<NetCommands>,
        MessageReader<TeleportMessage>,
        MessageReader<WorldportMessage>,
        MessageWriter<crate::creature_anim::SheathRequest>,
        MessageReader<crate::net::SpeedChangeMessage>,
        // The ack'd movement-mode family: root / water-walk / feather-fall /
        // hover, granted on our mover — applied to `player.modes` and acked here with the live pose.
        MessageReader<crate::net::MoveModeMessage>,
        // The landing report for the client-side hard-landing predictor (`0x602d00` — wound
        // vocal + dust; the consumers gate on the threshold, `creature_anim::env_damage`).
        MessageWriter<crate::creature_anim::HardLanding>,
        // The cast bar's local self-cancel trigger (decision 0256 open item 2): the controller
        // reports the move edges the real client's movement machine hands `AbortCast 0x6e4940`.
        ResMut<crate::spell::LocalMoveStart>,
        // The mounted space-bar flourish (decision 0441 P2): our own MountSpecial(94) plays
        // locally at send time; the net drain self-suppresses any broadcast echo.
        MessageWriter<crate::creature_anim::MountFlourish>,
        // A `MSG_MOVE_*` the server addressed to our OWN mover — a pose it wrote,
        // with no handshake and no ack owed. `wire_in` snaps to it.
        MessageReader<crate::net::SelfMoveMessage>,
        // The posture queue: `/sit` and its family ask here; the X key is read
        // inline in [`super::posture`]. One setter, either way.
        MessageReader<StandStateRequest>,
        // The possession handoff: control of a unit granted or revoked. Lands here rather
        // than at the net drain because both answers it needs — the mover claim and the parting
        // pose — are the controller's to give.
        MessageReader<crate::net::ClientControlMessage>,
        // A knockback the server aimed at our mover — `wire_in` latches it, the
        // take-off site below flies it, and the movement stream acks it with the post-launch pose.
        MessageReader<crate::net::KnockBackMessage>,
        // The tutorial system's world-input sites (1976): Movement acknowledged + the 10 s
        // Targeting popup on a movement input, Cameras acknowledged on a mouse-look.
        crate::tutorial::InputHooks,
        // The loot window's move-start close: the reference's movement-START
        // guard `0x60e990`, reported here beside the cast bar's edge and consumed by `ui_loot`.
        ResMut<crate::ui_loot::LootMoveStart>,
        // The server's own stand state for our body: `SMSG_STANDSTATE_UPDATE`,
        // applied through the posture setter's local half — ungated, nothing sent back.
        MessageReader<super::ServerStandState>,
    ),
    // Nested into one param to stay within Bevy's 16-element system-param tuple limit (see `mouse`).
    speed_capsule: (
        Res<MoveSpeed>,
        Res<PlayerCapsule>,
        Res<InspectMode>,
        Res<crate::ui_script::UiKeyboardCapture>,
        Res<crate::ui_script::PlayerUiClickConsumed>,
        // The binding dispatch: every rebindable input below reads command
        // state from here — raw `keys` remain only for the dev chord's free-fly toggle
        // and the look-session mouse.
        Res<crate::bindings::BindingsState>,
        // What the camera orbits, when that is not our body ([`view_subject`]). Resolved by
        // a system ordered just before us, because `control` holds the self `Transform` mutably
        // and so cannot also read the far-sight object's.
        Res<view_subject::ViewSubject>,
        // Our own guid — the control handoff is a statement about *some* unit, and telling
        // "the server revoked my body" from "the server handed me a creature" is exactly this test.
        Res<crate::net::SelfGuid>,
        // The spyglass scope ([`scoped_view`]): while held, the rig is pinned to first person and
        // the wheel cannot leave it.
        Res<scoped_view::ScopedView>,
        // Is the loading cover up? The cover takes the whole input plane at the source
        // (`loading_screen::input`), so nothing new arrives here — but a mouse gesture already in
        // flight is retained state, and only its owner can unwind it. See the cancel below.
        Res<crate::loading_screen::LoadingScreen>,
    ),
    mut commands: Commands,
    mut player: ResMut<Player>,
    mut rig: ResMut<CameraControl>,
    // Avian's kinematic move-and-slide: sweeps the capsule against the streamed colliders.
    collide: benilla_world::collision::WorldCollision,
    mut cameras: Query<(&mut Transform, &mut FlyCam), With<Camera>>,
    // **The body in our hands** — see [`BodyQuery`] for what rides on it and why a possessed
    // creature needs nothing special here.
    mut body: BodyQuery,
    window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    // Clicks go out here — left for the target picker, right for the context action (attack) — and
    // the third is the right button's raw DOWN edge (targeting's cancel). A press
    // engages its camera look *and* arms a click test; the release settles that test on the
    // reference's time/travel predicate, so one gesture can orbit and select both.
    // The locals hold each button's pending [`camera::PressGesture`] (`None` = no press pending).
    mut world_clicks: (
        MessageWriter<WorldClick>,
        MessageWriter<WorldRightClick>,
        MessageWriter<WorldRightPress>,
    ),
    mut click_test: (
        Local<Option<camera::PressGesture>>,
        Local<Option<camera::PressGesture>>,
    ),
    // World context for the mover, bundled into one param (16-param limit): the world query the
    // swim mode + buoyant float ask the liquid through (see [`swim`]), the armed transports (the
    // platform-frame carry/attach — decision 0438 phase 2; `Without`s only disjoint the borrows),
    // and the parent chain (the attach walk resolves a deck prop's collider child to the
    // transport that owns it — solid cargo).
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
    // The auto-follow knobs, with far sight's one exception folded in here so
    // both camera seats below agree: while the rig orbits somebody ELSE's body (Mind Vision, Sentry
    // Totem), our own facing is not what "behind" means, so the return is forced off rather than
    // reeling that camera toward a heading with nothing to do with the view.
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
    // While a focused UI EditBox (the chat input, a mail field) owns the keyboard, keyboard reads see
    // "no keys held" — so the avatar isn't also driven while typing (a `.tele` command). Mouse still
    // works. The gate is `UiKeyboardCapture::typing`, which the focused chat EditBox drives —
    // deliberately not its per-key `consumed` twin (2196): that one says a keyboard FRAME ate one
    // key, whose business is that key's binding, not whether a dev free-cam may read WASD. The free-fly
    // chord below is deliberately outside it, like every dev chord ([`modkeys::dev_chord`]).
    let typing = ui_capture.typing;
    // The rebindable inputs all read `binds`: the dispatch already enforced the
    // typing gate and 0585's exact-modifier law when it latched, so this module carries neither
    // anymore. Nothing here reads a bare key any more — the free-fly toggle is on
    // the dev chord, and the Ctrl run boost is gone.

    // Which mouse buttons the world owns this frame is [`camera::latch_world_mouse`]'s, decided
    // in a system ahead of this one (ledger B364). Every *press* below is taken from that latch —
    // the look session's engage, the camera's command word, the both-button run — so a press the
    // UI ate reaches none of them. The raw buttons are still read below, but only for the hold and
    // release of a gesture the world already owns.
    // The both-button state and the camera's input command word ([`input::look_input`]) — read
    // here, before the look session, because the not-driving path below seats its camera from the
    // same word and returns without ever reaching the movement axes.
    let input::LookInput {
        both_buttons,
        follow_command,
    } = input::look_input(binds, &player, &rig);

    // The camera-option inputs. `translating` is **last frame's** wire word, and
    // deliberately: the reference's `0x50fee0` runs from the input handler `0x514446`, which
    // precedes the mover lookup, so it reads the movement flags the previous update left. It is
    // also all this path can read — `flags::this_frame` runs several hundred lines below, after
    // the mover.
    let dynamics = camera_dynamics::DynamicsInput {
        options: *pointer.4,
        nearclip: pointer.5.nearclip,
        // Last frame's cached surface — the reference's own lag (`Player::liquid_surface`).
        surface_y: player.liquid_surface,
        smooth_style: pointer.3.style,
        tracking_style: pointer.3.tracking_style,
        subject: camera_dynamics::SubjectState {
            move_flags: player.move_flags,
            // Off the DRIVEN body's descriptor (1277) — a possessed creature's taxi state is what
            // the camera follows, not ours.
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
            // `GetCurrentSpeed 0x7c4c90` on the mover's own speed set, against LAST frame's flag
            // word — the same pairing the reference's bob kernel makes, and the only speed either
            // camera seat below can ask for (the driving path's own is computed hundreds of lines
            // later, and the stand-down path never computes one at all).
            speed: body
                .single()
                .ok()
                .and_then(|(_, _, _, _, _, _, _, speeds, ..)| speeds)
                .map_or(0.0, |s| crate::net::current_speed(&s.0, player.move_flags)),
            command: follow_command,
            scoped: scoped.active(),
        },
    };

    // The look session gets a SHADOW copy of `CursorOptions`, written back only on a real change:
    // handing it the component's `Mut` directly reborrowed mutably every frame, which marks it
    // Changed regardless of writes — and bevy_winit's `changed_cursor_options` then re-applied
    // cursor state to AppKit per frame, an OS call that intermittently stalls the main thread for
    // milliseconds (the 0366 frame-tail hunt's second-biggest line).
    let mut opts_shadow = cursor_opts.bypass_change_detection().clone();
    // Snapshot for the seated-turn stand-up ([`super::posture`]): a right-drag (or both-button) look session
    // writes `face_yaw` directly — any change is a real mouse TURN of the character (a left-drag
    // orbits the camera only and never touches it).
    let yaw_before_look = player.face_yaw;
    // **Stunned** (`UNIT_FIELD_FLAGS & 0x40000`): read once here, because the very
    // first thing a stun suppresses is the mouse turn below. This is a descriptor bit, NOT a
    // movement flag and NOT an aura: the reference's `0x5145b0` computes `!STUNNED` straight off
    // `[[unit+0x110]+0xa0]` (`not eax; shr eax,0x12; and eax,1`) and `0x514755` consumes it to skip
    // the turn and pitch emitters outright.
    // **Dead** is read in the same pass and for the same reason: the reference's two movement-input
    // predicates share a precondition (`0x5144e0`) whose second term is `[[mover+0x110]+0x40] > 0`
    // — UNIT_FIELD_HEALTH, the `jle` at `0x5144f8` — so a body at zero health answers *no* to both
    // "may I translate?" (`0x514560`) and "am I not stunned?" (`0x5145b0`). **A corpse is stunned**
    // as far as the input tick is concerned, and that is the whole reason the reference will not
    // let you spin your own body on the ground. Off the MOVER's descriptor, not
    // ours — `esi` in `0x5144e0` is whatever we are driving (1277) — and a ghost is not dead by it,
    // because the server puts a released player's health at 1.
    // **The stand state** is read in the same pass for the mouse alone: the
    // reference's camera→body hand-off predicate `0x5145e0` ends on `GetStandState() == 0`
    // (`0x51460c`, the CGPlayer override `0x5ed570` = the client-predicted cache), so a seated
    // body — sitting, in a chair, asleep — is never re-faced by a right-drag, and never stood up
    // by one either. Predicted, not merely echoed: `stand_pending` is our `[player+0x1d68]`.
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
    // The shared precondition `0x5144e0`, assembled once for this tick the way the reference
    // evaluates it once for the mover — health above, and the far-sight conjunct here.
    //
    // **Conjunct 5** is `!(IsActivePlayer(mover) && [mover+0x1c70] & 1)`: while your view is out on
    // a far-sight object you may not drive your own body. Both halves matter. The latch half is the
    // **resolved** subject and not the raw `PLAYER_FARSIGHT` field, because `0x5ee290` sets the
    // latch only on its post-resolve ENGAGE leg; the active-player half is `foreign_mover.is_none()`,
    // and it is what keeps Mind Control working — possession sets the very same field, so without
    // it the victim would be frozen.
    let mover = state::MoverInput {
        dead,
        view_is_out: state::view_is_out(
            player.foreign_mover.is_none(),
            view_subject.remote.is_some(),
        ),
    };
    // `0x5145b0`, evaluated here because the first thing it suppresses is the mouse turn below. Its
    // translate sibling waits until after `apply_server_moves`, where the root edge it reads lands.
    let may_turn = mover.may_turn(stunned);
    // `0x5145e0`, the mouse's own hand-off gate — `may_turn` plus the stand-state conjunct the
    // keyboard path does not have (it stands you up instead; the mouse is refused).
    let mouse_turns_body = mover.mouse_may_turn_body(stunned, stand_state);
    // Drunkenness: this frame's wobble angle, computed once — the facing veer and the
    // swim-pitch porpoise ([`swim::drive_step`]) both read it. Zero while sober (`wobble` early-outs on a 0.0
    // fraction), and zero whenever the turn predicate is down — the reference's wobble sits behind
    // the same input-allowed chain as the turn emitters (`0x60aa47` → `0x5145b0`), so a stun stops
    // it and, through the precondition that predicate shares, so does death.
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
    // **A cover cancels a gesture; it never completes one.** The cover emptied the button planes
    // in `PreUpdate`, so the look session below ends on its own this frame — and ending it also
    // *settles* the click test the press armed, against `PressGesture::is_click`'s free window. A
    // right-press 0.1 s before a portal would therefore have dispatched a `WorldRightClick` under
    // the loading screen, acting on the pick latched in the world we just left. Dropping both
    // tests first leaves the session to end with nothing to fire. This is the one place the cover
    // has to be named by hand: taking the input away cannot rewind a state machine that is already
    // mid-gesture, and this system owns the only such state the world side has (the UI's own armed
    // press/drag is unwound by `UiScript::pointer_left_window`, which the blanked cursor already
    // triggers).
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
    // A stun freezes the BODY, not the view. The look session has already moved `cam.yaw` (and, on
    // a right-drag, coupled `face_yaw = cam.yaw`); putting the aim back leaves the camera orbiting
    // a body that does not turn — which is what a stunned character looks like, and what the
    // reference produces by never running the turn emitter at all. Restoring rather than gating
    // inside the session keeps the approved camera path (0050/0366's right-drag coupling) untouched.
    // (Nothing downstream can tell: the mouse turn no longer stands a seated player either — 1766.)
    // Losing the reins has the same shape, and the binary says so explicitly: with the mover global
    // zeroed, `0x514640` skips the whole tick at `51466c` — input is still *sampled*, the body turn
    // is skipped at `514474` — while the camera rotate at `514444` happens BEFORE the mover lookup
    // and so keeps working. A mind-controlled player can still look around; they just cannot turn
    // or move their body. Same restore, for the same reason — and `reseat` is the same condition
    // once more, the frames where the mover global would not resolve at all.
    //
    // Holding *somebody else's* reins is emphatically not in this set: the turn belongs to whatever
    // we are driving, and once the marker has caught up that is the creature, whose `Transform` is
    // the one this `face_yaw` writes.
    //
    // **Death enters through the same door** — but by its own byte trail, not the
    // keyboard's, and the two are worth keeping apart. The reference's right-drag lives in
    // `0x514400`, off the mouse-MOVE handler `0x492c00`, and its body hand-off
    // `0x51447b call 0x5103e0` is gated at `0x514474` by a THIRD predicate, `0x5145e0`, whose first
    // act is to call the stun test `0x5145b0` — which fails the precondition `0x5144e0` it shares
    // with the translate gate at zero health, and short-circuits the rest. The two commit gates
    // further down, `0x5151b0` (yaw) and `0x515250` (pitch), carry their own health tests as well,
    // and a closed census of all nine call sites of the facing setters `0x60de30`/`0x60de70` finds
    // every route health-gated. So the mouse and the keys do NOT share `0x514755`; they share
    // `0x5144e0`, and `may_turn` is where that term lives on our side.
    //
    // That is the line this file was missing: benilla modelled the server's root on death,
    // and a root deliberately leaves turning live, so a right-drag went on spinning the body
    // on the ground. Note the reference has **nothing to restore** — it never writes the facing at
    // all, because `0x50fee0` (the camera rotate at `0x514446`) runs before any object lookup and
    // the hand-off simply does not run. Ours writes and puts back, which is indistinguishable
    // downstream and keeps the approved camera path intact — a shape difference, not a copy.
    //
    // **A seated body enters through the same door too**. `0x5145e0`'s last
    // conjunct is `GetStandState() == 0` — the right-drag orbits the camera round a body that keeps
    // facing its chair, sends nothing, and does not stand it up (1766 had already taken the stand
    // off this gesture; this takes the turn off it). Stand up — X, a turn key, a flick — and the
    // next motion sample hands the camera's yaw to the body again, which is the snap the reference
    // shows too: `0x5103e0` commits the camera's own facing, not a delta.
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

    // Camera zoom rides the CAMERAZOOMIN/OUT bindings (defaults = the wheel pair). The
    // wheel-over-UI routing lives in the dispatch now — a wheel the quest log consumed never
    // reaches these commands — and a rebound zoom KEY steps 1.0 per press, 1.12's own
    // `CameraZoomIn(1.0)` argument.
    let zoom = binds.amount(crate::bindings::cmd::CAMERA_ZOOM_IN)
        - binds.amount(crate::bindings::cmd::CAMERA_ZOOM_OUT);
    apply_zoom_scroll(zoom, dt, &mut rig, zoom_max);

    // Free-fly is a dev instrument, so it sits on the dev chord, not a bare `F`.
    // A bare `F` is a key the reference lets a player bind — our own store test binds it to JUMP —
    // and a dev doesn't get to squat on the game's namespace (the same rule that moved the
    // perf HUD off bare `P`). Ungated on `typing` like every chord: it can't be mistaken for text.
    if crate::run_mode::dev_chord(&keys, KeyCode::KeyF) {
        player.detached = !player.detached;
    }

    // Server-authored movement edges + their mandatory acks (worldport/teleport snaps, root,
    // water-walk, the take-control edge — [`wire_in`]). The returned forced-speed changes were
    // already acked pre-control/detached; controlled, the movement stream below acks them with
    // its live per-frame payload.
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

    // The platform carry ([`ride::carry`], decision 0438 phase 2): while attached to a transport,
    // recompose the whole rider — feet, aim, rendered body and camera — from the boat's THIS-frame
    // pose, before any input integrates.
    ride::carry(&mut player, &mut cam, transports);

    if player.active && !player.detached {
        // Server ride guard: a server-authored spline (Charge/knockback/taxi) owns the avatar this
        // frame. `drive_self_ride` (ordered just before us) already synced `player.pos` + facing from
        // the `sample_splines` transform and set the run animation; here we only carry the
        // follow-camera onto the moving avatar. Input, physics, and the outbound movement stream all
        // yield until the ride ends (where `drive_self_ride` acks `CMSG_MOVE_SPLINE_DONE` and resumes).
        // **The three ways of not driving**, and they share every line of their answer: the camera
        // keeps seating on the body, input and physics and the outbound stream all yield.
        //
        // - `server_riding` — a server-authored spline (Charge/knockback/taxi/a flee path) owns the
        //   avatar. `drive_self_ride`, ordered just before us, has already mirrored the sampled
        //   transform into `Player`, so there is nothing to sync and nothing to park: it is
        //   reporting FORWARD on the wire on purpose.
        // - `control_lost` — somebody else is driving our body, or the body in our hands has
        //   been feared out of our control. It is NOT the free-fly branch below, which would fly
        //   the camera off the body; and it is not root, which leaves turning live. Nothing else
        //   will stop us: the server neither roots the victim nor validates their movement, so this
        //   gate IS the immobility (see `Player::control_lost`).
        // - [`Player::reseat`] — the window between mover guids where what we intend to drive and
        //   what carries `Embodied` have not yet met: the frame a grant lands, and every frame
        //   after it while the claimed unit has not streamed in. Driving during that window writes
        //   one body's pose onto another, because outbound moves carry no guid of their own.
        //   `apply_server_moves` above closes it the moment a pose is there to adopt, so this is
        //   normally a single frame.
        //
        // Note the middle one is deliberately NOT "we are possessing": once the marker has caught
        // up, possession runs the *ordinary* controlled path below, on the creature.
        if player.server_riding || player.control_lost || player.reseat {
            // Whoever is moving the body, its transform is the truth and `Player` follows it.
            // Skipping this is what stranded the camera during a fear: the body
            // ran off on its spline while the orbit stayed at the pose the controller last wrote,
            // which reads as the view detaching into free flight (director, 2026-08-13). A
            // `reseat` window is excluded — there the resource still describes the body we are
            // letting go of, and `apply_server_moves` owns the adoption.
            //
            if player.control_lost && !player.reseat {
                if let Ok((_, t, ..)) = body.single() {
                    let yaw = server_ride::yaw_of(t.rotation);
                    player.pos = t.translation;
                    player.face_yaw = yaw;
                    player.model_yaw = yaw;
                }
            }
            let head = player.pos + Vec3::Y * (CAPSULE_HEIGHT - CAPSULE_RADIUS);
            // Far sight outlives all three, so it has to be honoured here too — Sentry Totem
            // carries no interrupt flags at all, which means you can board a taxi with your view
            // still on the totem. [`camera::seat_on_subject`] is the shared substitution, which is
            // exactly why it is shared: skipping it here would read as far sight mysteriously
            // dropping the moment a spline takes the body.
            //
            // The auto-follow still runs on this path — a taxi, a Charge, a knockback or a fear
            // all translate the avatar while the controller stands down, and the reference has
            // states for exactly those (`Track`, `Fear`: a 0.4 s delay and a lazy 18 °/s return
            // under Smart). The word carries both flags, so the edge into and out of one of them
            // is what arms it.
            camera::seat_on_subject(
                dt,
                0.0,
                player.pos,
                head,
                body.single().ok().and_then(|(_, _, _, pivot, .., net)| {
                    // Even while a spline/taxi/fear owns the body, the pivot preset follows
                    // that body's own MOVEFLAG_SWIMMING (`0x50f880` reads the camera target's
                    // CMovement word, not ours). On this path the controller builds no live
                    // flag word, so the last-streamed one — which `wire_in` merges from the
                    // server's own poses — is the body's state.
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
            // Flush a stale run once, so observers stop extrapolating it — but never under a ride,
            // whose FORWARD report is deliberate and would be cancelled every frame.
            if !player.server_riding {
                movement_net::park_mover(&net.0 .0, &mut player);
            }
            // After the park, so a fear's ack carries the stopped word the park just reported.
            movement_net::ack_speeds_undriven(&net.0 .0, &player, &speed_acks);
            return;
        }
        // This frame's netted movement axes, the mouselook/turn modes they imply, and the autorun
        // latch with its verified cancel set — all of it decoded once in [`input::move_axes`].
        // `0x514560`, evaluated now — after `apply_server_moves`, so a root edge that landed this
        // frame is already in `player.modes` and not read a frame late.
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
        // The world-input tutorial sites (1976): a movement input, a mouse-look.
        if fwd_axis != 0 || side_axis != 0 {
            net.13.moved();
        }
        if mouselook {
            net.13.mouselooked();
        }
        // **The mover's own** six speeds, read once here for the whole frame — the turn below and
        // the run/backpedal selection further down. All six live on the driven unit's `CMovement`
        // and nothing in the reference's applied-input path ever reads *our* speeds when the mover
        // is a different object (VERIFIED), so a possessed creature moves and turns
        // at its own numbers for free: the component was on its entity all along.
        let mover_speeds = body.single().ok().and_then(|q| q.7).map(|s| s.0);
        // The 6th speed. Zero is the ctor state, not a rate — the client keeps no default of its
        // own, so a unit whose create block has not landed falls back rather than freezing solid.
        let turn_rate = mover_speeds
            .map(|s| s.turn_rate)
            .filter(|r| *r > 0.0)
            .unwrap_or(TURN_RATE);
        // This frame's keyboard-turn rotation — `seat_camera` carries the camera by it rigidly
        // (char and camera turn as one on the reference; director's call, closing 0050's open
        // "camera follow on turn" feel item).
        let mut turn_delta = 0.0;
        if turning {
            let mut turn = 0.0;
            if turn_left {
                turn += 1.0;
            }
            if turn_right {
                turn -= 1.0;
            }
            // 0.75× while translating **or falling** — the verified `flags & 0x200f` case, whose
            // `0x2000` is FALLING (`0x7c5c73`). A jump mid-turn keeps the reduced rate.
            let slowed = translating || player.airborne_since.is_some();
            let rate = turn_rate * if slowed { TURN_RATE_MOVING } else { 1.0 };
            turn_delta = turn * rate * dt;
            player.face_yaw += turn_delta;
        }
        // The drunk veer: while moving, the facing increments by the wobble angle every
        // frame (`0x60aa70–0x60aab7`: `facing + wobble`, 2π-wrapped, committed via the normal
        // facing pipeline `0x60de30` — so it streams on the wire like any turn). The slow sign
        // oscillation of the pulse is what makes the walk meander. Skipped while a keyboard turn
        // is held — the reference's `flags & 0x30` guard (`0x60aa5a`) — so deliberate turning
        // stays crisp; both yaw conventions increase turning left, so the add maps sign-for-sign.
        // The veer joins `turn_delta` so `seat_camera` carries the camera with it exactly like a
        // keyboard turn (char and camera turn as one) — the reference's camera follows the drunk
        // meander too (director's ref observation); without the carry the character
        // staggers out from under a fixed camera.
        if drunk_wobble != 0.0 && translating && !turning {
            player.face_yaw += drunk_wobble;
            turn_delta = drunk_wobble;
        }
        let face_rot = Quat::from_rotation_y(player.face_yaw);
        let move_fwd = flat(face_rot * Vec3::NEG_Z);
        let move_right = flat(face_rot * Vec3::X);
        let mut dir = Vec3::ZERO;
        // Forward/back comes from the net axis (W, S, both-button and autorun already summed) — one
        // step in its sign, never a doubled push, exactly as the emitter issues one START in
        // `sign(axis)`. (`mover::step` normalizes anyway, but the axis is the honest shape.)
        match fwd_axis.signum() {
            1 => dir += move_fwd,
            -1 => dir -= move_fwd,
            _ => {}
        }
        // Strafe slides without turning, one step in the netted sign — never a doubled push, and a
        // cancelled pair is genuinely no strafe (the same shape as the forward/back axis).
        match side_axis.signum() {
            1 => dir += move_right,
            -1 => dir -= move_right,
            _ => {}
        }
        // **The translate predicate down: translation intent dies here — and under a plain root,
        // turning above stays live.** Confirmed at the bytes and it is
        // *authored*, not accidental: the reference's input
        // tick consults an allow-list (`0x615c71` → the byte table at `0x618054`) which blocks the
        // translation command ids and **explicitly permits** the turn ids 8/9/0xa, pitch, run/walk
        // and SetFacing. A character who cannot even pivot is STUNNED, a separate `UNIT_FIELD_FLAGS`
        // gate handled above — and vmangos's `HandleModStun` grants both at once, which is why Ice
        // Block freezes completely while Frost Nova lets you turn. **Death is the third way this
        // predicate goes down** (1753), and unlike the root it takes the pivot with it.
        if !may_translate {
            dir = Vec3::ZERO;
        }
        let moving = dir != Vec3::ZERO;
        // The character's own KEYBOARD turn this frame (or the drunk veer, which rides `turn_delta`
        // the same way) — one of the movement inputs that stands a seated avatar back up.
        //
        // **A mouse turn is deliberately not in this set**. It was, on the strength
        // of a director observation that a right-drag stands you. The observation is right and the
        // attribution wrong: the body-facing commit is refused for a
        // seated player by two independent gates (`0x5145e0` @`0x51460c` on the prediction cache,
        // `0x5151b0` @`0x51520a` on the raw descriptor byte), and `0x514f50` skips its stand arm
        // outright while the RMB bit is held (`0x514f6d test al,1; jne`). What stands you is the
        // **release**: a press-to-release under 200 ms with under 2.25° of yaw is dispatched as a
        // right-CLICK (`0x514ae0`, which is [`camera::PressGesture::is_click`] here), and the
        // click's INTERACT reaches `SetStandState(0)`. So the stand belongs to the click, and it
        // lives in [`crate::target::click`] now — a deliberate turn-drag leaves you seated.
        let turned = turn_delta != 0.0;
        // The gait toggle (`TOGGLERUN`) — the walk/run latch, run here so the speed select
        // below reads the bit this frame's press left, which is the reference's own order
        // (`ToggleRun` is an input-phase command; the mover reads `CMovement+0x40` after it).
        // [`super::walk`] owns the latch and the refusal chain.
        walk::update(&mut player, &body, binds);
        // Posture ([`posture`]) — the stand state (`X` and the `/sit` family) and the sheath
        // toggle (`Z`), which interlock: the stow rider fires off the stand state this commits,
        // and the toggle's guard chain refuses on it. Returns the **committed** state, which the
        // body pose below streams to the animation selector.
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
        // Backpedaling is slower: the backward move-flag selects the backward speed, dominating
        // strafe (binary-VERIFIED — see RUN_BACK_RATIO). Net-backward = the S key held without a
        // forward override (W or both-button run). The resulting (slower) speed also feeds jump
        // takeoff, so a backward jump lands shorter for free.
        let net_backward = fwd_axis < 0;
        // The mover's speed SET this frame: server-authoritative (`UnitSpeeds` — seeded by our
        // create's LIVING block, moved live by SMSG_FORCE_*_SPEED_CHANGE, so `.modify speed`,
        // mounts and slows actually move us at the server's number), or the `$WOW_MOVE_SPEED` dev
        // override's synthetic set, which keeps the vanilla 2.5/4.5/7.0 ratios so that walking and
        // backpedaling stay themselves under it. Pre-create frames take the same fallback.
        let speeds = match mover_speeds {
            Some(s) if !move_speed.env_override => s,
            _ => benilla_protocol::MoveSpeeds {
                walk: move_speed.value * WALK_RATIO,
                run: move_speed.value,
                run_back: move_speed.value * RUN_BACK_RATIO,
                ..Default::default()
            },
        };
        // …turned into a yards/second by the ONE statement of the reference's
        // `GetCurrentSpeed 0x7c4c90` ([`crate::net::current_speed`]), the same call the remote
        // extrapolator makes — so our own body and every body we watch agree about the cascade,
        // including the part that is easy to get backwards: **the walk arm is taken before the
        // backward min**, so walking backwards is walk speed (2.5), not run-back (4.5).
        //
        // The flag word handed over is this frame's *gait intent*, not the wire word — that one is
        // built later, after the mover has run ([`super::flags::this_frame`]). Only the three bits
        // the ground cascade reads are needed and all three are known here; swimming never reaches
        // this arm ([`super::swim`] owns its own leg of the same getter).
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
        // Root is refused HERE (the reference refuses it twice upstream of the handler: the Lua
        // gate's `test ch,0x12` and the replay allow-list `0x615c71`/`0x618030`, where command
        // id 7 is blocked). HOVER's refusal is NOT here — it belongs to the movement handler
        // itself (`0x7c623a`, the breach term below and [`mover::step`]'s grounded arm), which
        // is what keeps the mounted flourish reachable while hovering, as the reference has it.
        //
        // **And death is refused here too** — `may_translate` and not `!rooted`, which corrects
        // what 1753 first shipped on the reading that `0x513cee`'s `test ch,0x12` was the whole
        // gate. `Jump 0x513bd0` inlines *both* `0x5144e0` and `0x514560`: a health test at
        // `0x513cbc` (`jle 0x513d43`), a second at `0x513cde`, the root mask, and stand state
        // `!= 7` at `0x513cf3` — which is `may_translate`, term for term.
        let mut want_jump = binds.fired(crate::bindings::cmd::JUMP) && may_translate;

        // Swim vs walk: the water over our feet decides. Hysteresis-latched (`update_swimming`,
        // the verified `0x6030c0` boundary — B7 resolved) so wading the line
        // doesn't flicker between the two physics regimes.
        let surface_y = swim::surface_over_feet(world, player.pos);
        // Cache it for the camera's water corridor, which reads it a frame later exactly as the
        // reference's `0x670630` accessor does (see `Player::liquid_surface`).
        player.liquid_surface = surface_y;
        let swimming = swim::update_swimming(&mut player, surface_y, time.elapsed_secs());
        if let Some(surface) = surface_y {
            move_trace::swim(player.pos.y, surface, swimming, player.collision_height.0);
        }
        // Space while swimming = the ref's Jump routing (superseding 0479),
        // fired on the PRESS EDGE only — one hop per press, a held key does not re-fire
        // (director-verified on the ref; 0487's held-chaining was our
        // over-extension of the swim Jump routing, and near the surface its re-latch→re-fire loop
        // bounced the avatar under the waterline — the "invisible wall"). The routing
        // (`0x7c6230`) has no depth gate and no swim re-route — at the surface the press
        // breaches out; submerged it's the ~1.6-yd dolphin-hop, re-latching into swim once the
        // launch velocity halves (`0x7c5de0`). The smooth way UP is aiming up in mouselook and
        // swimming forward (the 0492 pitch law). The breach exits the water mode INSIDE this
        // frame — the byte handler runs before the mover, clearing SWIMMING unconditionally —
        // so the latch drops now and this frame's mover, flags, and wire all see the leap as a
        // jump.
        // HOVER refuses the breach too: `0x7c623a`'s test sits AHEAD of the SWIMMING take-off
        // select (`0x7c6261` only picks the seed velocity, it gates nothing), and hover does not
        // suppress swim entry — `0x6030c0` tests only LEVITATING (`0x400`) — so a hovering
        // swimmer is a real state and their Space does nothing at all. The land leg's refusal
        // lives in [`mover::step`],
        // the same handler's grounded arm.
        // **The wire's jump** — the `Jump(force = 0)` a `SetHover(true)` owes
        // ([`Player::hover_launch`]). It differs from Space in exactly one gate and
        // that gate is the point: `0x7c6236 test eax,eax; je 0x7c6243` skips the hover refusal when
        // `force` is 0, so this leg jumps a body that is *already* hovering — which is every body
        // that just got granted hover. The two refusals it keeps are ROOT and FALLING
        // (`0x7c625c test ah,0x30`); the seed select at `0x7c6261` is shared, so the swim/land
        // choice is made below by the same two take-off sites Space uses.
        let wire_jump = player.take_wire_jump();
        // **The knockback the server aimed at us** — taken at the same site as the
        // other two take-offs so all three enter the mover through one door. Resolved here from the
        // wire quad into one Bevy launch velocity: the horizontal is `(cos, sin)·xy_speed` in
        // **absolute world XY** (nothing to do with our facing or our keys), and the vertical is
        // `−zspeed`, because the wire's take-off speed is DOWN-positive — the same convention the
        // jump tail this quad is about to be echoed back as already uses.
        let knockback = player.take_knockback();
        let knock_launch = knockback.map(|k| {
            let (c, sn, xy) = (k.launch.cos_angle, k.launch.sin_angle, k.launch.xy_speed);
            wow_to_bevy([c * xy, sn * xy, -k.launch.zspeed])
        });

        // A knockback lifts a swimmer clear of the water like any other take-off — it sets FALLING,
        // and FALLING and SWIMMING are exclusive. It is NOT routed through `breach_step`, though:
        // that arm exists to seed `SWIM_JUMP_SPEED`, and a knockback brings its own seed on both
        // axes, so it leaves the water here and is flown by the ordinary land mover below.
        let breach = swimming && (want_jump && !player.modes.hover || wire_jump);
        let knock_breach = swimming && knock_launch.is_some();
        if breach || knock_breach {
            player.swimming = false;
        }
        let swimming = swimming && !breach && !knock_breach;

        // The netted swim translation amounts ([`swim::translate_amounts`]) — read by the swim
        // mover arm AND the flag build, so the two can never disagree.
        let (swim_fwd, swim_side) = if swimming {
            swim::translate_amounts(&axes, !may_translate)
        } else {
            (0.0, 0.0)
        };

        // The mounted space-bar flourish (decision 0441 P2). The gate is the
        // client's jump-key handler `0x60dea0`: mounted +
        // no translational move + not turning + grounded → play MountSpecial(94) locally FIRST,
        // then send `CMSG_MOUNTSPECIAL_ANIM` (the receive side self-suppresses the echo, see
        // `net/apply.rs`); translational move → a real jump, the unmounted path; **turn-only
        // (the `0x30` turn flags) → a silent no-op** — the press is consumed, nothing plays;
        // airborne → silent no-op (the client's geometric ground-clearance test `0x605650`;
        // our airborne arc stands in — an airborne press falls through and the mover ignores
        // it, the same net silence). Swim disposition is INFERRED-moot (you can't be mounted
        // while swimming in 1.12); a swimming Space is the jump-exit above — and only that
        // (Space is the Jump command `0x513bd0`; it is NOT a pitch or ascend input) — and never
        // reaches this walk-side gate.
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

        // This frame's PRESENTED swim pitch — the persistent [`Player::mover_pitch`] while swimming
        // (held even idle, the client's `CMovement+0x20`), except leveled by the 0499 surface
        // redirect when the rest-line cap bites. Feeds the body pose and the wire pitch tail (one
        // source — the pose and the stream can't disagree); the tail only serializes with the
        // SWIMMING flag, so the walking value is inert.
        // **The mover pitch, set — in every mode, not just the swim one** ([`Player::mover_pitch`]
        // = `CMovement+0x20`). HELD when unsteered (`0x7c4f80` — an idle floater keeps its
        // pitch, never auto-levels), and steered by mouselook as a DIRECT set of the camera aim
        // (closing 0488's INTERIM): the ref's mouse-move
        // chain ends in `SetPitch 0x7c6f70`, an unconditional store — no integrator, no rate limit
        // — clamped ±89° ([`MOUSELOOK_PITCH_CLAMP`], the byte constant; the ±π/2 clamp belongs to
        // the unbound pitch-KEY integrator), with the velocity basis rebuilt in-call: the aim
        // re-points travel the same frame, zero lag. (The ref's `fchs` negate is its own camera
        // sign convention; ours maps aim-up to pitch-up already.) A left-drag camera orbit steers
        // NOTHING — it moves the camera without turning the character (the walk rule at `move_fwd`
        // above), so it must not bend the swim either (director-reported, 2026-07-18).
        //
        // It lived inside the swimming branch until decision 1616. Nothing on the ref's
        // write path is swim-gated — not the mouse handler `0x514400`, not the applier `0x5103e0`,
        // not the relay `0x515330`, not the enqueuer `0x6198a0`, and not `SetPitch`'s own store at
        // `0x7c6f91`, which precedes the `test [esi+0x40],0x200000` that splits the two arms.
        // Swimming gates only the *readers* — the travel basis, the body pose, the wire tail — and
        // on land the field has two more, both water walking's: the trace-mask arm's third gate
        // below, and `SetPitch`'s own dive-through complement.
        //
        // The push is **per mouse-move, not per frame** ([`Player::aim_pitch_seen`]): the ref's
        // enqueue hangs off the mouse-MOTION event `0x400500cb`, so a still mouse pushes nothing
        // and the other writers of the field — the wobble, StopSwim's levelling — survive.
        //
        // **The aim is the COMPOSITE, not the camera's own pitch**: the reference's
        // camera→body hand-off `0x5103e0` clamps `[cam+0x104] + [cam+0xf4]` to ±89° before passing
        // the pitch on, so a smart-pivot frame — where the arm's pitch does not move and the view's
        // does — still aims the body. The bias is zero except on those frames, so this reads as
        // `cam.pitch` everywhere else.
        let aim_pitch = cam.pitch + rig.smart_pivot.bias();
        if mouselook && aim_pitch != player.aim_pitch_seen {
            player.aim_pitch_seen = aim_pitch;
            player.mover_pitch = aim_pitch.clamp(-MOUSELOOK_PITCH_CLAMP, MOUSELOOK_PITCH_CLAMP);
        }
        let mut swim_pitch = 0.0_f32;
        // The ground height the mover starts this frame at (pre-step feet Y). For a jump this is the
        // true takeoff height — the mover integrates one jump-tick upward *within* the step, so the
        // post-step `pos.y` is already ~0.13 yd (60 fps) above the ground and must not be used as
        // the launch height (see [`Player::advance_airborne_arc`]).
        let launch_y = player.pos.y;
        let mover::Outcome {
            held,
            grounded,
            jumped,
            knocked,
            air_nudged,
            ground,
        } = if breach {
            // Jump while swimming (`0x7c6230`): clears SWIMMING and enters the FALLING lifecycle
            // *unconditionally* —
            // no swim re-route, no surface-proximity gate — seeding a take-off ~14% over a land
            // jump. At the surface this is the jump-out hop (the leap clears the water and can
            // carry onto a low bank); deep, it's the ~1.6-yd dolphin-hop — swim re-latches once
            // the upward velocity halves (`update_swimming`'s verified `0x7c5de0` gate). The wire
            // streams it as a normal JUMP: fall clock 0, the seeded zspeed in the tail —
            // `advance_airborne_arc` snapshots it like any land jump ([`super::flags`]).
            swim::breach_step(&mut player, &time, &collide, capsule)
        } else if swimming {
            // One swimming frame ([`swim::drive_step`]): the drunk porpoise, the pitched travel
            // basis, the directional speed select, the stroke rate, and the float physics. It also
            // decides this frame's PRESENTED pitch — the raw aim, except leveled by the 0499
            // surface redirect when the rest-line cap bites.
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
            // The kinematic mover step — walk/fall physics + the step-down snap (decisions
            // 0009/0182/0190); the mechanism lives in [`mover`].
            // **Water walking**: hand the mover the liquid surface, which
            // it treats as ordinary ground — the classify sees it, the grounded arm runs, and the
            // clamp finalises Y ([`mover::step`], where the *why* of both halves lives). In the
            // reference this is not a floor that gets handed anywhere: `MOVEFLAG_WATERWALKING` ORs
            // the ADT liquid layers into the walk trace's class mask (`0x63162e`), so the surface
            // simply *is* geometry. Passing it down is our stand-in for that, because liquid is
            // queried rather than swept here.
            //
            // All three of the arm's gates live in [`mover::water_floor`], where each one's byte
            // site and its consequence are written out — including the pitch gate that 1611 could
            // only *name*, because the pitch was steered inside the swim branch until 1616's
            // hoist put it above this call.
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
                // **The air-control nudge speed is `min(MOVE_WALK, MOVE_RUN)`, read live**
                // — the walk override inside the reference's `0x7c4c90(1)`
                // (`0x7c4d19`/`0x7c4d1b`), not a constant. `AIR_NUDGE_SPEED`'s 2.5 is the default
                // `MOVE_WALK` and stays as the fallback for before the server has sent us speeds;
                // once it has, a walk aura, a Slow or a daze moves this with them, and the min is
                // why a *slowed* run still cannot out-steer a walk.
                mover_speeds.map_or(super::AIR_NUDGE_SPEED, |s| s.walk.min(s.run)),
            )
        };

        // The knockback's own trace line — flown or refused, with the quad. Emitted
        // here because this is the first point where both halves are known: the latch we took, and
        // the mover's verdict on it.
        if let Some(k) = knockback {
            move_trace::knockback(knocked, k.launch);
        }

        let now = time.elapsed_secs();
        // Airborne is a walk-only concept — swimming never falls, so the body-heading / anim-flags
        // logic below reads this hoisted value (false while swimming) instead of the walk branch's.
        //
        // **A root ends the arc outright**: `airborne` is our `MOVEFLAG_FALLING`,
        // and `SetRoot 0x7c7340`'s second act is `StopFalling 0x7c6290`, which clears FALLING and
        // FALLINGFAR together. So a root or a stun taken mid-air is not a body that lands — it is a
        // body that is no longer falling, hanging where it was ([`mover::step`]'s anchor holds the
        // position). Ground contact is left honest in `grounded` (the transport attach and the trace
        // both want the truth); it is the *arc* that the root ends.
        let airborne = !swimming && !held && !player.modes.rooted && (!grounded || jumped);
        // Transport attach/detach + the deck-local re-snapshot, and the `ride` trace ([`ride`],
        // decision 0438 phase 2 / 0470's solid cargo). Airborne keeps the current attachment, so
        // a jump above the deck is deck-frame ballistics and lands where it took off.
        ride::update_attachment(
            &mut player,
            transports,
            child_of,
            ground,
            grounded,
            swimming,
        );
        // This frame's two move-flag words + the wire's fall clock ([`flags::this_frame`]): the
        // live word the wire and the local gates read, and the take-off-frozen one the animation
        // does. The arc bookkeeping (snapshot / FALLINGFAR / the landing edge) runs inside.
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
        // The rendered body heading + the animation's view of the flags — the display-facing law
        // lives in [`gait::drive_body_heading`] (strafe offset ease / moving snap / the standing
        // FROZEN chase whose body-step latches the turn-in-place shuffle).
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
        // Write the frame onto the driven body — pose, `MovementState`, the counter-twist gap and
        // the landing report ([`body_pose::drive`]) — and take back the camera-pivot target it
        // read off that body.
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

        // The camera-collision sweep is rooted at the *head* (capsule top hemisphere centre), not the
        // framing pivot — see `seat_camera`'s doc for why. Computed here (not in `camera`) because it
        // depends on the avatar's own capsule constants, which are a movement concern.
        let head = player.pos + Vec3::Y * (CAPSULE_HEIGHT - CAPSULE_RADIUS);
        // The spyglass lock (aura 76): pin the rig to first person for as long as the scope is
        // held. Parking BOTH the live distance and the wheel target is what makes it a lock rather
        // than a nudge — the wheel writes `target_distance`, and re-parking here every frame is our
        // equivalent of the reference's camera flag `0x8` making `SetCameraView` early-return.
        if scoped.active() {
            rig.park_distance(0.0);
        }
        // Far sight: while `PLAYER_FARSIGHT` names an object, the rig orbits IT instead of
        // the body — the substitution is [`camera::seat_on_subject`]'s, and it is the entire
        // feature. Everything else in this system runs untouched, which is the point: Mind Vision
        // leaves you walking, streaming, hearing and sending movement as yourself while only the
        // picture moves.

        // `WOW_CAM_DUMP`: the per-frame INPUT signal beside `seat_camera`'s realized-pose `[cam]`
        // line — wall clock, frame dt, this frame's accumulated mouse delta, the active look mode,
        // and the yaw/pos the frame produced. A turn-feel question ("keyboard turn smooth, mouse
        // turn jittery") needs the input cadence and the output cadence on the same timeline: a
        // bursty `dx` under a steady `dt` convicts event delivery; a steady `dx` with an uneven
        // realized pose convicts everything downstream.
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
                // The pivot channel's live height and where it is heading — the two columns that
                // answer "does the camera snap?" numerically (timing is measured).
                rig.pivot.probe().0,
                rig.pivot.probe().1,
            );
        }
        // `turn_delta` is the character's own turn this frame (keyboard turn, or the drunk veer)
        // — the deck's yaw delta was already applied to `cam.yaw` at the ride block (frame motion
        // carries the camera unconditionally; only input turns respect `seat_camera`'s
        // look-session gate).
        // The auto-follow's own three facts: the knobs, where "behind" is,
        // and the input word whose edges arm a return.
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

        // The cast bar's local self-cancel trigger (`spell::local_self_cancel`): a fresh
        // *directional* start (the same wire-axis edge the stream below turns into a
        // MSG_MOVE_START_*; diffed against the pre-stream `player.move_flags`) or a jump launch.
        // Turn-in-place and pitch deliberately absent: the client's interrupt mask `0x10f0`
        // (`0x5150ce`) is {fwd, back, strafe L/R, autorun};
        // turn/pitch flags sit outside it and never cancel.
        // `autorun_armed` is 0445's dormant fifth mask member waking up — the `0x1000` bit IS in the
        // verified `0x10f0` interrupt mask, but **only on the ON edge**: `ToggleAutoRun` computes its
        // `setBool` as the new state, and the dispatcher short-circuits the whole interrupt block on a
        // clear edge (`0x5150c8`) *before* the mask is tested. So arming autorun kills a cast;
        // disarming it does not. It needs its own term because the flag-delta test above can't see it —
        // toggling autorun on with W already held raises no new direction bit (VERIFIED wire-silence),
        // yet the reference still cancels. (0445's row says "YES" unqualified; only the ON edge
        // cancels.)
        //
        // **And it is a fact about our own character, not about whatever we are steering**.
        // The whole point of Mind Control is walking the victim around while the
        // channel holds, and the interrupt this feeds is the *caster's*: vmangos breaks a channel on
        // `m_caster`'s own position moving (`Spell::update`), and a possessed creature's steps never
        // touch it. Without this gate the first movement key after a Mind Control shipped
        // `CMSG_CANCEL_CHANNELLING` for our own channel 23 ms later, ending the possession — and
        // since the reins then came home mid-keypress, the still-held key ran our own character,
        // which reads exactly like "moving my character cancelled the spell" (director, 2026-08-13;
        // reproduced live, `WOW_CAST_TRACE`).
        let steering_ourselves = player.foreign_mover.is_none();
        if steering_ourselves
            && (move_flags_now & move_flags::ANY_MOVE & !player.move_flags != 0
                || jumped
                || autorun_armed)
        {
            net.7 .0 = true;
        }
        // The loot window's own walk-away — the reference's movement-START guard
        // `0x60e990`, called first by every player-initiated START emitter: forward/back, strafe,
        // keyboard-turn and pitch START, pitch STOP, `SetPitch` and Jump pass `arg2 = 0` and close
        // an open loot at distance zero; mouse-look `SetFacing` passes `arg2 = 1` and does not.
        // A different mask from the cast's `0x10f0` above — TURN is in this one — and the same
        // `IsActivePlayer(this)` gate: a possessed mover's steps close nothing of ours.
        if steering_ourselves
            && (move_flags_now
                & (move_flags::ANY_MOVE | move_flags::TURN_LEFT | move_flags::TURN_RIGHT)
                & !player.move_flags
                != 0
                || jumped)
        {
            net.14 .0 = true;
        }

        // Stream this frame's movement to the server — a `MSG_MOVE_*` per movement-axis transition, the
        // jump/fall lifecycle, and a ~500 ms heartbeat, each carrying the live `MovementInfo` (decisions
        // 0052 + 0053). vmangos relays it to nearby players, who extrapolate from the flags. See the
        // [`movement_net`] module (the outbound mirror of `net::motion`'s remote integration).
        // The rider's local pose for the wire's ON_TRANSPORT tail.
        let wire_transport = movement_net::wire_transport(&player);
        // The skipped-time clock: a held frame is a frame of movement simulation
        // we advanced through without integrating, and the mover we name is the one we are
        // actually driving — a possessed unit while we hold its reins. Read out before the call
        // takes `player` mutably.
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
                // **The wire's own take-offs announce themselves, and not with `MSG_MOVE_JUMP`**.
                // `wire_jump` covers the hover grant's `CMovement::Jump(force = 0)`,
                // whose handler `0x61a620` sends nothing at all — and it cannot be racing a keyboard
                // jump, because the body it launches was granted HOVER in the same breath and hover
                // is the first refusal Space takes (`0x7c623a`). `knocked` covers the knockback,
                // which pushes its ack instead. This is the correction 1620 needed: benilla streamed
                // a JUMP for the hover launch, and the reference streams nothing.
                wire_launch: knocked || wire_jump,
                air_nudged,
                landed,
                fall_time: wire_fall_time,
            },
            now,
            &speed_acks,
            // Owed only if the mover actually flew it: a launch the settle hold or the root anchor
            // refused is dropped in silence, exactly as the reference discards an already-popped
            // knockback record under `MOVEFLAG_ROOT` (`0x615c71 test ah,0x10` → the kind-28 table
            // byte is 0 → `0x615c80 je 0x616539`: no apply, and no ack).
            knockback.filter(|_| knocked),
            wire_transport,
            skip,
        );
    } else {
        // Free fly (pre-connect or detached): aim from the look angles, move the camera directly
        // ([`camera::fly_free`]). If we just detached mid-move, the controlled branch above (which
        // owns the per-frame movement stream) has stopped running with our last move-flags still
        // live on the wire — park the mover so the server clears them, else observers extrapolate a
        // phantom walk/spin until we re-attach. No-op pre-connect / once already stopped (decision
        // 0056). The avatar stays frozen at `player.pos`.
        movement_net::park_mover(&net.0 .0, &mut player);
        camera::fly_free(dt, &keys, typing, &mut rig, &mut cam, &mut cam_t);
    }
}
