//! `MouselookStart`/`MouselookStop` against the look session: the latch and the session run as the
//! frame runs them, with the VM's calls in between.

use benilla_ui::script::UiScript;
use bevy::ecs::system::RunSystemOnce;
use bevy::math::DVec2;

use super::super::camera_dynamics::{CameraOptions, SubjectState};
use super::*;

/// What the session's caller keeps between frames: the facing and the two click tests.
#[derive(Resource, Default)]
struct Hand {
    face_yaw: f32,
    left: Option<PressGesture>,
    right: Option<PressGesture>,
}

/// `control`'s call of [`run_look_session`], with the both-button run read off the latch.
fn look_frame(
    motion: Res<AccumulatedMouseMotion>,
    mut rig: ResMut<CameraControl>,
    cam: Single<&mut FlyCam>,
    window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut clicks: (MessageWriter<WorldClick>, MessageWriter<WorldRightClick>),
    mut hand: ResMut<Hand>,
) {
    let (mut window, mut opts) = window.into_inner();
    let focused = window.focused;
    let mut cam = cam.into_inner();
    let hand = &mut *hand;
    let both = rig.world_mouse.both();
    let dynamics = DynamicsInput {
        options: CameraOptions::default(),
        smooth_style: FollowStyle::default(),
        tracking_style: FollowStyle::default(),
        subject: SubjectState::default(),
        nearclip: 0.2,
        surface_y: None,
    };
    run_look_session(
        &motion,
        both,
        &mut rig,
        &mut cam,
        &mut hand.face_yaw,
        &mut window,
        &mut opts,
        false,
        false,
        &mut clicks.0,
        &mut clicks.1,
        &mut hand.left,
        &mut hand.right,
        LookConfig::default(),
        &dynamics,
        0.0,
        focused,
    );
}

/// A world with the camera, the window, a VM and nothing held; `over_ui` says where the cursor is.
fn world(over_ui: bool) -> World {
    let mut world = World::new();
    world.init_resource::<ButtonInput<MouseButton>>();
    world.init_resource::<AccumulatedMouseMotion>();
    world.insert_resource(crate::ui_script::PointerOverUi(over_ui));
    world.init_resource::<CameraControl>();
    world.init_resource::<Hand>();
    world.init_resource::<Messages<WorldClick>>();
    world.init_resource::<Messages<WorldRightClick>>();
    world.spawn((
        Camera::default(),
        FlyCam {
            yaw: 0.0,
            pitch: 0.0,
            speed: 0.0,
        },
    ));
    let mut window = Window::default();
    window.set_physical_cursor_position(Some(DVec2::new(100.0, 100.0)));
    world.spawn((window, CursorOptions::default(), PrimaryWindow));
    world.insert_non_send_resource(UiScript::new().unwrap());
    world
}

/// One frame with the mouse moved by `dx` pixels: the latch, then the session, then the input
/// planes age as bevy's do.
fn frame(world: &mut World, dx: f32) {
    world.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::new(dx, 0.0);
    world.run_system_once(latch_world_mouse).unwrap();
    world.run_system_once(look_frame).unwrap();
    world.resource_mut::<ButtonInput<MouseButton>>().clear();
}

/// The OS gives the window focus or takes it.
fn focus(world: &mut World, focused: bool) {
    world
        .query::<&mut Window>()
        .single_mut(world)
        .unwrap()
        .focused = focused;
}

/// Where the OS pointer is, in logical window pixels.
fn pointer(world: &mut World) -> Option<Vec2> {
    world
        .query::<&Window>()
        .single(world)
        .unwrap()
        .cursor_position()
}

fn lua(world: &mut World, chunk: &str) {
    world.non_send_resource::<UiScript>().run(chunk).unwrap();
}

fn is_mouselooking(world: &World) -> bool {
    world
        .non_send_resource::<UiScript>()
        .eval::<Option<i64>>("return IsMouselooking()")
        .unwrap()
        == Some(1)
}

fn yaw(world: &mut World) -> f32 {
    world.query::<&FlyCam>().single(world).unwrap().yaw
}

fn cursor(world: &mut World) -> CursorOptions {
    world
        .query::<&CursorOptions>()
        .single(world)
        .unwrap()
        .clone()
}

fn press(world: &mut World, b: MouseButton) {
    world.resource_mut::<ButtonInput<MouseButton>>().press(b);
}

fn release(world: &mut World, b: MouseButton) {
    world.resource_mut::<ButtonInput<MouseButton>>().release(b);
}

/// The right button's session, entered with no button: the mouse turns the camera and the body
/// together, the cursor is locked and hidden, and `IsMouselooking` answers 1 (`0x514210`).
#[test]
fn mouselook_start_turns_the_camera_and_the_character_with_no_button_held() {
    let mut w = world(false);
    frame(&mut w, 0.0);
    assert!(!is_mouselooking(&w));

    lua(&mut w, "MouselookStart()");
    frame(&mut w, 40.0);
    let rig = w.resource::<CameraControl>();
    assert_eq!(
        rig.look,
        Some(LookButton::Right),
        "the right button's session"
    );
    assert!(rig.freelook);
    let turned = yaw(&mut w);
    assert!(turned < 0.0, "the mouse turns the camera: {turned}");
    assert_eq!(
        w.resource::<Hand>().face_yaw,
        turned,
        "and the character with it"
    );
    let c = cursor(&mut w);
    assert_eq!(c.grab_mode, CursorGrabMode::Locked);
    assert!(!c.visible);
    assert!(is_mouselooking(&w));

    // It holds with no button while the mouse keeps moving.
    frame(&mut w, 40.0);
    assert!(yaw(&mut w) < turned);
    assert!(w.resource::<CameraControl>().is_looking());
}

/// `MouselookStop` (`0x514240`) ends the session: the cursor comes back and the mouse turns
/// nothing, and `IsMouselooking` answers nil.
#[test]
fn mouselook_stop_ends_the_session() {
    let mut w = world(false);
    lua(&mut w, "MouselookStart()");
    frame(&mut w, 40.0);
    let turned = yaw(&mut w);

    lua(&mut w, "MouselookStop()");
    frame(&mut w, 40.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    assert_eq!(yaw(&mut w), turned, "the mouse turns nothing");
    let c = cursor(&mut w);
    assert_eq!(c.grab_mode, CursorGrabMode::None);
    assert!(c.visible);
    assert!(!is_mouselooking(&w));
}

/// CustomNameplates' steer: a right press a plate took calls `MouselookStart`, and the button's
/// release ends it (`0x492b50` routes it to `TurnOrActionStop`) without a world click.
#[test]
fn the_right_release_ends_a_mouselook_its_press_started_on_the_ui() {
    let mut w = world(true);
    press(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    assert_eq!(
        w.resource::<CameraControl>().look,
        None,
        "the plate's press alone starts no look"
    );

    lua(&mut w, "MouselookStart()");
    frame(&mut w, 40.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Right));
    assert!(is_mouselooking(&w));

    release(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    assert!(!is_mouselooking(&w));
    assert!(
        w.resource::<Messages<WorldRightClick>>().is_empty(),
        "no click was armed"
    );
}

/// The loading cover's world enter clears the whole word (`0x5144c0`).
#[test]
fn the_cover_ends_a_scripted_mouselook() {
    let mut w = world(false);
    lua(&mut w, "MouselookStart()");
    frame(&mut w, 0.0);
    w.insert_resource(crate::loading_screen::LoadingScreen::test_covering());
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    assert!(!is_mouselooking(&w));
}

/// A session across a focus loss: the look state stays as the deactivate leaves it (`0x514490`
/// keeps the mouse bits), the OS cursor is free and shown meanwhile and the mouse turns nothing,
/// and the focus takes it back locked and hidden, with no jolt on the frame it returns.
fn a_session_survives_focus_loss(start: impl Fn(&mut World)) {
    let mut w = world(false);
    start(&mut w);
    frame(&mut w, 40.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Right));
    let before = yaw(&mut w);

    focus(&mut w, false);
    frame(&mut w, 40.0);
    assert_eq!(
        w.resource::<CameraControl>().look,
        Some(LookButton::Right),
        "the session outlives the focus"
    );
    assert!(is_mouselooking(&w));
    let c = cursor(&mut w);
    assert_eq!(c.grab_mode, CursorGrabMode::None, "the OS pointer is free");
    assert!(c.visible);
    assert!(!w.resource::<CameraControl>().holds_cursor());
    assert_eq!(
        yaw(&mut w),
        before,
        "the pointer in another app turns nothing"
    );

    focus(&mut w, true);
    frame(&mut w, 40.0);
    let c = cursor(&mut w);
    assert_eq!(c.grab_mode, CursorGrabMode::Locked);
    assert!(!c.visible);
    assert!(w.resource::<CameraControl>().holds_cursor());
    assert_eq!(
        yaw(&mut w),
        before,
        "the returning frame's travel is not a turn"
    );
    frame(&mut w, 40.0);
    assert!(yaw(&mut w) < before, "and the mouse turns the view again");
    assert!(is_mouselooking(&w));
}

#[test]
fn a_scripted_mouselook_survives_focus_loss() {
    a_session_survives_focus_loss(|w| lua(w, "MouselookStart()"));
}

#[test]
fn a_right_drag_survives_focus_loss() {
    a_session_survives_focus_loss(|w| press(w, MouseButton::Right));
}

/// `MouselookStop` while unfocused ends the session; the pointer stays where the player left it
/// in the other app, and the focus coming back takes nothing.
#[test]
fn mouselook_stop_while_unfocused_ends_it_and_the_focus_takes_nothing_back() {
    let mut w = world(false);
    lua(&mut w, "MouselookStart()");
    frame(&mut w, 0.0);
    focus(&mut w, false);
    frame(&mut w, 0.0);
    let away = Vec2::new(300.0, 250.0);
    w.query::<&mut Window>()
        .single_mut(&mut w)
        .unwrap()
        .set_cursor_position(Some(away));

    lua(&mut w, "MouselookStop()");
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    assert!(!is_mouselooking(&w));
    assert_eq!(pointer(&mut w), Some(away), "no warp back to the stash");

    focus(&mut w, true);
    frame(&mut w, 0.0);
    let c = cursor(&mut w);
    assert_eq!(c.grab_mode, CursorGrabMode::None);
    assert!(c.visible);
    assert!(!w.resource::<CameraControl>().is_looking());
}

/// One bit, whoever set it: `MouselookStop` ends a right drag in flight and disarms its click, so
/// the release after it selects nothing (`0x514810(0)`, then `0x515090(1, 0, …)`).
#[test]
fn mouselook_stop_ends_a_right_drag_and_its_release_clicks_nothing() {
    let mut w = world(false);
    press(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    assert!(
        w.resource::<Hand>().right.is_some(),
        "the press armed a click"
    );

    lua(&mut w, "MouselookStop()");
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    assert!(!is_mouselooking(&w));
    release(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    assert!(w.resource::<Messages<WorldRightClick>>().is_empty());
    assert!(!is_mouselooking(&w));

    // The control: the same press and release with no stop is a click.
    let mut w = world(false);
    press(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    release(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<Messages<WorldRightClick>>().len(), 1);
}

/// `MouselookStart` under a left drag sets the other primary channel: the both-button run, its
/// rise, and the left click disarmed; the left release hands the session to the mouselook.
#[test]
fn mouselook_start_under_a_left_drag_is_the_both_button_run() {
    let mut w = world(false);
    press(&mut w, MouseButton::Left);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Left));
    assert!(w.resource::<Hand>().left.is_some());

    lua(&mut w, "MouselookStart()");
    w.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::new(40.0, 0.0);
    w.run_system_once(latch_world_mouse).unwrap();
    {
        let wm = &w.resource::<CameraControl>().world_mouse;
        assert!(wm.both(), "both channels held");
        assert!(wm.rose(), "the transition into the run (0x514a73)");
    }
    w.run_system_once(look_frame).unwrap();
    w.resource_mut::<ButtonInput<MouseButton>>().clear();
    assert!(
        w.resource::<Hand>().left.is_none(),
        "the left click disarmed"
    );
    let cam_yaw = yaw(&mut w);
    assert!(cam_yaw < 0.0);
    assert_eq!(
        w.resource::<Hand>().face_yaw,
        cam_yaw,
        "the body turns with the camera"
    );

    release(&mut w, MouseButton::Left);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Right));
    assert!(w.resource::<Messages<WorldClick>>().is_empty());
}
