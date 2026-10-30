//! `MouselookStart`/`MouselookStop` against the look session: the latch and the session run as the
//! frame runs them, with the VM's calls in between.

use benilla_ui::script::{BindingInput, HeldInput, UiScript};
use bevy::ecs::system::RunSystemOnce;
use bevy::math::DVec2;
use bevy::window::PrimaryWindow;

use crate::bindings::{BindingsState, Input};

use super::super::camera_dynamics::{CameraOptions, SubjectState};
use super::*;

/// What the session's caller keeps between frames: the facing and the armed click.
#[derive(Resource, Default)]
struct Hand {
    face_yaw: f32,
    arm: crate::target::ClickArm,
    /// The session's answer: a motion reached the body's hand-off.
    handed: bool,
}

/// `control`'s call of [`run_look_session`], with the both-button run read off the latch.
fn look_frame(
    motion: Res<AccumulatedMouseMotion>,
    mut rig: ResMut<CameraControl>,
    cam: Single<&mut FlyCam>,
    window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut hand: ResMut<Hand>,
    binds: Option<Res<BindingsState>>,
) {
    let (mut window, mut opts) = window.into_inner();
    let focused = window.focused;
    let mut cam = cam.into_inner();
    let hand = &mut *hand;
    let primaries = binds.as_deref().map(Primaries::of).unwrap_or_default();
    let dynamics = DynamicsInput {
        options: CameraOptions::default(),
        smooth_style: FollowStyle::default(),
        tracking_style: FollowStyle::default(),
        subject: SubjectState::default(),
        nearclip: 0.2,
        surface_y: None,
    };
    hand.handed = run_look_session(
        &motion,
        &mut rig,
        &mut cam,
        &mut hand.face_yaw,
        &mut window,
        &mut opts,
        false,
        &mut hand.arm,
        LookConfig::default(),
        &dynamics,
        focused,
        primaries,
        binds
            .as_deref()
            .is_some_and(|b| super::super::input::command_word(b) != 0),
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
    // The interface: the `WorldFrame` under everything and, `over_ui`, a frame under the cursor.
    let mut script = UiScript::new().unwrap();
    script.set_screen_size(1024.0, 768.0);
    script
        .run(r#"WorldFrame = CreateFrame("WorldFrame", "WorldFrame") WorldFrame:SetAllPoints()"#)
        .unwrap();
    if over_ui {
        script
            .run(
                r#"F = CreateFrame("Frame", "F")
                F:SetPoint("BOTTOMLEFT", 50, 50) F:SetWidth(100) F:SetHeight(100) F:EnableMouse(true)"#,
            )
            .unwrap();
    }
    script.resolve();
    script.mouse_move(AT.0, AT.1);
    world.insert_non_send_resource(script);
    world.init_resource::<BindingsState>();
    world
}

/// The cursor in the interface's units.
const AT: (f32, f32) = (100.0, 100.0);

/// One frame with the mouse moved by `dx` pixels: the latch, then the session, then the input
/// planes age as bevy's do.
fn frame(world: &mut World, dx: f32) {
    world.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::new(dx, 0.0);
    world.run_system_once(latch_world_mouse).unwrap();
    world.run_system_once(look_frame).unwrap();
    world.run_system_once(focus_on_look_edges).unwrap();
    world.resource_mut::<ButtonInput<MouseButton>>().clear();
    world.resource_mut::<BindingsState>().next_frame_for_test();
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

/// A binding body, as `ExecuteBinding` runs one: the movement gate passes.
fn binding(world: &mut World, chunk: &str) {
    world
        .non_send_resource::<UiScript>()
        .run_binding(chunk)
        .unwrap();
}

fn looking(world: &World) -> bool {
    world.resource::<CameraControl>().is_looking()
}

/// `f` on the VM with the look's live reads lent, as the host lends them: the bindings' bit and
/// the camera's freelook.
fn with_look_reads<R>(world: &mut World, f: impl FnOnce(&mut UiScript) -> R) -> R {
    struct Lent<'a, 'w>(&'a mut super::LookLive<'w>);
    impl benilla_ui::script::LiveReader for Lent<'_, '_> {
        fn read_live(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
            crate::script_calls::live::LiveFamily::answer(self.0, ask)
        }
    }
    let mut script = world.remove_non_send_resource::<UiScript>().expect("a VM");
    let mut state = bevy::ecs::system::SystemState::<super::LookLive>::new(world);
    let mut live = state.get(world);
    let r = script.reading(&mut Lent(&mut live), f);
    world.insert_non_send_resource(script);
    r
}

/// `IsMouselooking()` as a script reads it: the bindings' bit.
fn is_mouselooking(world: &mut World) -> bool {
    with_look_reads(world, |s| {
        s.eval::<Option<i64>>("return IsMouselooking()").unwrap()
    }) == Some(1)
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

/// The stock binding of a button run as the world frame's slot runs it, with no host lent: `BUTTON1`
/// runs `CAMERAORSELECTORMOVE`, `BUTTON2` `TURNORACTION`, and the bodies' calls reach the bindings'
/// state as the dispatch's tail hands them over.
fn bound(world: &mut World, b: MouseButton, down: bool) {
    let body = match (b, down) {
        (MouseButton::Left, true) => "CameraOrSelectOrMoveStart()",
        (MouseButton::Left, false) => "CameraOrSelectOrMoveStop()",
        (_, true) => "TurnOrActionStart()",
        (_, false) => "TurnOrActionStop()",
    };
    let inputs = {
        let mut script = world.non_send_resource_mut::<UiScript>();
        script.run_binding(body).unwrap();
        script.take_binding_input()
    };
    let mut binds = world.resource_mut::<BindingsState>();
    for input in inputs {
        binds.apply_unhosted(input);
    }
}

/// The interface's walk of a button record at the cursor, as the input records' feed runs it:
/// where it went ([`benilla_ui::script::PointerTarget`]).
fn walk(world: &mut World, b: MouseButton, down: bool) -> benilla_ui::script::PointerTarget {
    let name = match b {
        MouseButton::Left => "LeftButton",
        _ => "RightButton",
    };
    with_look_reads(world, |s| s.mouse_button_to(AT.0, AT.1, name, down))
}

/// The buttons whose press ran the stock binding, as the dispatch latches them.
#[derive(Resource, Default)]
struct Latched(Vec<MouseButton>);

/// A button goes down, the interface walks it, and the world frame runs the stock binding with no
/// host lent when the press went to it: the press's target, the frame holding a button else the
/// hover (`0x76637f`), or in freelook the world frame (`0x492bd9`). A press the feed consumed
/// ahead of the walk (`PlayerUiPressConsumed`) runs nothing.
fn press(world: &mut World, b: MouseButton) {
    use benilla_ui::script::PointerTarget;
    world.resource_mut::<ButtonInput<MouseButton>>().press(b);
    let consumed = world
        .get_resource::<crate::ui_script::PlayerUiPressConsumed>()
        .is_some_and(|c| c.0);
    let target = walk(world, b, true);
    if !consumed && target == PointerTarget::World {
        bound(world, b, true);
        world.get_resource_or_init::<Latched>().0.push(b);
    }
}

/// A button comes up, the interface walks it, and the stock binding's up-half runs as the
/// dispatch runs it: for a press it latched unless a frame took the release, and for none when
/// the release went to the world frame, in freelook through the hook (`0x492be5`).
fn release(world: &mut World, b: MouseButton) {
    use benilla_ui::script::PointerTarget;
    world.resource_mut::<ButtonInput<MouseButton>>().release(b);
    let target = walk(world, b, false);
    let latched = {
        let mut latched = world.get_resource_or_init::<Latched>();
        let was = latched.0.contains(&b);
        latched.0.retain(|&l| l != b);
        was
    };
    if target != PointerTarget::Ui && (latched || target == PointerTarget::World) {
        bound(world, b, false);
    }
}

/// The right button's session, entered with no button: the mouse turns the camera and the body
/// together, the cursor is locked and hidden, and `IsMouselooking` answers 1 (`0x514210`).
#[test]
fn mouselook_start_turns_the_camera_and_the_character_with_no_button_held() {
    let mut w = world(false);
    frame(&mut w, 0.0);
    assert!(!is_mouselooking(&mut w));

    lua(&mut w, "MouselookStart()");
    frame(&mut w, 40.0);
    let rig = w.resource::<CameraControl>();
    assert_eq!(
        rig.look,
        Some(LookButton::Right),
        "the right button's session"
    );
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
    assert!(is_mouselooking(&mut w));

    // It holds with no button while the mouse keeps moving.
    frame(&mut w, 40.0);
    assert!(yaw(&mut w) < turned);
    assert!(w.resource::<CameraControl>().is_looking());
}

/// A freelook the session enters outside any call, a `MouselookStart` made with no host lent,
/// moves the mouse focus to the world frame right after it (`0x493085`), and its leave has the
/// hover picked again (`0x4930a0`), so the VM's next update gives it back.
#[test]
fn a_freelook_entered_at_the_frame_moves_the_mouse_focus_and_its_leave_gives_it_back() {
    let mut w = world(true);
    let focus = |w: &mut World| {
        w.non_send_resource::<UiScript>()
            .eval::<String>("return GetMouseFocus():GetName()")
            .unwrap()
    };
    assert_eq!(focus(&mut w), "F");
    lua(&mut w, "MouselookStart()");
    assert_eq!(
        focus(&mut w),
        "F",
        "the call made no entry with no host lent"
    );
    frame(&mut w, 0.0);
    assert!(looking(&w));
    assert_eq!(focus(&mut w), "WorldFrame");
    lua(&mut w, "MouselookStop()");
    frame(&mut w, 0.0);
    assert!(!looking(&w));
    w.non_send_resource_mut::<UiScript>().tick(0.0);
    assert_eq!(focus(&mut w), "F");
}

/// A right press over bare world that the UI consumed, as a layout-mode Alt press is
/// (`0x766353`, before any frame sees it), turns nothing; the same press unconsumed turns.
#[test]
fn a_press_the_ui_consumed_over_the_world_turns_nothing() {
    let mut w = world(false);
    w.insert_resource(crate::ui_script::PlayerUiPressConsumed(true));
    press(&mut w, MouseButton::Right);
    frame(&mut w, 40.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    assert_eq!(yaw(&mut w), 0.0, "the camera did not turn");

    let mut w = world(false);
    w.insert_resource(crate::ui_script::PlayerUiPressConsumed(false));
    press(&mut w, MouseButton::Right);
    frame(&mut w, 40.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Right));
    assert!(yaw(&mut w) < 0.0, "an unconsumed press turns it");
}

/// The motion handler hands the facing and pitch to the body on every motion while the mouse
/// turns it (`0x514400` → `0x51446f` → `0x5103e0`), a yaw-only drag included, and not for a still
/// mouse or a left-drag orbit.
#[test]
fn every_mouselook_motion_reaches_the_hand_off() {
    let mut w = world(false);
    lua(&mut w, "MouselookStart()");
    frame(&mut w, 40.0);
    assert!(w.resource::<Hand>().handed, "a yaw-only motion");
    frame(&mut w, 0.0);
    assert!(!w.resource::<Hand>().handed, "a still mouse");
    let mut w = world(false);
    press(&mut w, MouseButton::Left);
    frame(&mut w, 40.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Left));
    assert!(!w.resource::<Hand>().handed, "an orbit turns no body");
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
    assert!(!is_mouselooking(&mut w));
}

/// A freelook a call enters and leaves inside it ([`LookSteps::call`]) hands the OS cursor at the
/// frames after: the session locks and hides it, then frees it and puts it back where the look
/// began.
#[test]
fn a_freelook_left_inside_its_call_gives_the_cursor_back_at_the_frame() {
    use benilla_ui::script::LookCall;
    let call = |w: &mut World, c: LookCall| {
        w.run_system_once(move |mut steps: LookSteps| steps.call(c))
            .unwrap()
    };
    let mut w = world(false);
    let began = pointer(&mut w);
    assert!(call(&mut w, LookCall::Mouselook(true)).entered);
    frame(&mut w, 0.0);
    let c = cursor(&mut w);
    assert_eq!(c.grab_mode, CursorGrabMode::Locked);
    assert!(!c.visible);
    w.query::<&mut Window>()
        .single_mut(&mut w)
        .unwrap()
        .set_cursor_position(Some(Vec2::new(10.0, 10.0)));

    call(&mut w, LookCall::Mouselook(false));
    assert!(!looking(&w), "left inside the call");
    frame(&mut w, 0.0);
    let c = cursor(&mut w);
    assert_eq!(c.grab_mode, CursorGrabMode::None);
    assert!(c.visible);
    assert_eq!(pointer(&mut w), began, "back where the look began");
}

/// A key bound to `TurnOrAction` holds the bit with no button down: the freelook its press entered
/// inside its call ([`LookSteps::bound`]) lasts through the frame's session while the binding holds
/// the bit (`[InputControl+4] & 1`), and ends with its release.
#[test]
fn a_freelook_a_bound_key_holds_lasts_with_no_button_down() {
    let mut w = world(false);
    w.insert_resource(BindingsState::after_calls(
        vec![],
        vec![BindingInput::Start(HeldInput::TurnOrAction)],
    ));
    let entered = w
        .run_system_once(|mut steps: LookSteps| {
            let start = BindingInput::Start(HeldInput::TurnOrAction);
            steps.bound(start, PrimaryBits::default())
        })
        .unwrap()
        .entered;
    assert!(entered);
    // The session takes the cursor, then the mouse turns the camera.
    frame(&mut w, 0.0);
    frame(&mut w, 40.0);
    assert!(looking(&w), "the bound bit holds the session");
    assert_ne!(yaw(&mut w), 0.0, "and the mouse turns the camera");

    w.insert_resource(BindingsState::after_calls(
        vec![BindingInput::Start(HeldInput::TurnOrAction)],
        vec![BindingInput::Stop(HeldInput::TurnOrAction)],
    ));
    frame(&mut w, 0.0);
    assert!(!looking(&w), "its release ends it");
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
    assert!(is_mouselooking(&mut w));

    release(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    assert!(!is_mouselooking(&mut w));
    assert!(
        left_click_arms(&mut w),
        "the release freed the command's bit"
    );
}

/// The interface under the cursor changes: `chunk` runs, and the hover walks the cursor again.
fn reshape(world: &mut World, chunk: &str) {
    let mut script = world.non_send_resource_mut::<UiScript>();
    script.run(chunk).unwrap();
    script.resolve();
    script.mouse_move(AT.0, AT.1);
}

/// A right drag's bit outlives a frame appearing under the locked cursor, and the chord's second
/// button joins the drag there: the world frame holds the press, so the left press goes to it
/// (`0x76637f`), not to the frame under the cursor, and runs its binding, the both-button run.
#[test]
fn a_frame_appearing_under_a_drag_neither_ends_it_nor_takes_the_chord() {
    let mut w = world(false);
    press(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Right));
    reshape(
        &mut w,
        r#"PRESSED = 0
        F = CreateFrame("Frame", "F")
        F:SetPoint("BOTTOMLEFT", 50, 50) F:SetWidth(100) F:SetHeight(100) F:EnableMouse(true)
        F:SetScript("OnMouseDown", function() PRESSED = PRESSED + 1 end)"#,
    );
    frame(&mut w, 40.0);
    frame(&mut w, 40.0);
    assert!(looking(&w), "the drag outlives the frame");
    assert!(w.resource::<BindingsState>().pressed(Input::TurnOrAction));
    press(&mut w, MouseButton::Left);
    frame(&mut w, 0.0);
    assert!(
        w.resource::<BindingsState>().steering(),
        "the chord joined the drag"
    );
    let pressed = w
        .non_send_resource::<UiScript>()
        .eval::<i64>("return PRESSED")
        .unwrap();
    assert_eq!(pressed, 0, "the frame under the cursor saw no press");
}

/// A press while a frame holds a button goes to that frame (`0x76637f`), wherever the cursor is:
/// a right press over the world after a left press went to a frame runs no binding.
#[test]
fn a_press_while_a_frame_holds_a_button_goes_to_that_frame() {
    let mut w = world(true);
    press(&mut w, MouseButton::Left);
    reshape(
        &mut w,
        "F:ClearAllPoints() F:SetPoint(\"BOTTOMLEFT\", 500, 500)",
    );
    press(&mut w, MouseButton::Right);
    frame(&mut w, 40.0);
    assert!(!looking(&w));
    assert!(!w.resource::<BindingsState>().pressed(Input::TurnOrAction));
}

/// Whether a left press's `CameraOrSelectOrMoveStart` arms a world click, which it does only with
/// neither primary bit held (`0x51481a`).
fn left_click_arms(w: &mut World) -> bool {
    use benilla_ui::script::{ClickCall, ClickMode};
    let mut script = w.non_send_resource_mut::<UiScript>();
    script.take_click_calls();
    script
        .run_binding("CameraOrSelectOrMoveStart() CameraOrSelectOrMoveStop()")
        .unwrap();
    script.take_click_calls().first() == Some(&ClickCall::Arm(ClickMode::Select))
}

/// The loading cover's world enter clears the whole word (`0x5144c0`): the look session and the
/// commands' bits, so a left click after it clicks.
#[test]
fn the_cover_ends_a_scripted_mouselook() {
    let mut w = world(false);
    lua(&mut w, "MouselookStart()");
    frame(&mut w, 0.0);
    assert!(!left_click_arms(&mut w), "the mouselook holds the bit");
    w.insert_resource(crate::loading_screen::LoadingScreen::test_covering());
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    assert!(!is_mouselooking(&mut w));
    assert!(left_click_arms(&mut w), "the cover cleared the bit");
}

/// A right button held across a zone: the cover clears its command's bit, so a left click after
/// the zone clicks though the right release never came.
#[test]
fn the_cover_frees_the_clicks_of_a_right_held_across_a_zone() {
    let mut w = world(false);
    press(&mut w, MouseButton::Right);
    binding(&mut w, "TurnOrActionStart()");
    frame(&mut w, 0.0);
    assert!(!left_click_arms(&mut w), "the right command holds its bit");
    w.insert_resource(crate::loading_screen::LoadingScreen::test_covering());
    frame(&mut w, 0.0);
    w.remove_resource::<crate::loading_screen::LoadingScreen>();
    frame(&mut w, 0.0);
    assert!(left_click_arms(&mut w), "the cover cleared the bit");
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
    assert!(is_mouselooking(&mut w));
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
    assert!(is_mouselooking(&mut w));
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
    assert!(!is_mouselooking(&mut w));
    assert_eq!(pointer(&mut w), Some(away), "no warp back to the stash");

    focus(&mut w, true);
    frame(&mut w, 0.0);
    let c = cursor(&mut w);
    assert_eq!(c.grab_mode, CursorGrabMode::None);
    assert!(c.visible);
    assert!(!w.resource::<CameraControl>().is_looking());
}

/// One bit, whoever set it: `MouselookStop` ends a right drag in flight (`0x515090(1, 0, …)`); its
/// disarm of the click is the VM's (`0x514810(0)`).
#[test]
fn mouselook_stop_ends_a_right_drag() {
    let mut w = world(false);
    press(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Right));

    lua(&mut w, "MouselookStop()");
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    assert!(!is_mouselooking(&mut w));
    release(&mut w, MouseButton::Right);
    frame(&mut w, 0.0);
    assert!(!is_mouselooking(&mut w));
}

/// The right drag's bit is the one `MouselookStop` and `ToggleMouseMove` act on, inside their
/// calls, with `BUTTON2` bound to `TURNORACTION`: the stop clears it and leaves (`0x514240` →
/// `0x514b70` → `0x514c48`), and a toggle leaves with the bit still held. Neither is entered again
/// while the button stays down: only the first primary press enters (`0x51490a`).
#[test]
fn a_right_drag_left_inside_a_call_is_not_entered_again_while_held() {
    use benilla_ui::script::LookCall;
    let call = |w: &mut World, c: LookCall| {
        w.run_system_once(move |mut steps: LookSteps| steps.call(c))
            .unwrap()
    };
    for leave in [LookCall::Mouselook(false), LookCall::ToggleMouseMove] {
        let mut w = world(false);
        press(&mut w, MouseButton::Right);
        frame(&mut w, 0.0);
        assert!(looking(&w), "{leave:?}: the drag entered");

        assert!(call(&mut w, leave).left, "{leave:?}: left inside the call");
        frame(&mut w, 40.0);
        frame(&mut w, 40.0);
        assert!(!looking(&w), "{leave:?}: the held button enters nothing");
        release(&mut w, MouseButton::Right);
        frame(&mut w, 0.0);
        press(&mut w, MouseButton::Right);
        frame(&mut w, 0.0);
        assert!(looking(&w), "{leave:?}: the next press enters");
    }
}

/// `MouselookStart` under a left drag sets the other primary channel: the both-button run and its
/// rise; the left release hands the session to the mouselook. The left click's disarm is the
/// VM's (`0x514ac1`).
#[test]
fn mouselook_start_under_a_left_drag_is_the_both_button_run() {
    let mut w = world(false);
    press(&mut w, MouseButton::Left);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Left));

    lua(&mut w, "MouselookStart()");
    w.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::new(40.0, 0.0);
    w.run_system_once(latch_world_mouse).unwrap();
    {
        let binds = w.resource::<BindingsState>();
        assert!(binds.steering(), "both primary bits held");
        assert!(
            binds.steering_began(),
            "the transition into the run (0x514a73)"
        );
    }
    w.run_system_once(look_frame).unwrap();
    w.resource_mut::<ButtonInput<MouseButton>>().clear();
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
}

/// `ToggleMouseMove` with no button enters mouselook (`0x50f910` → `0x50f930`): the cursor locks,
/// and `IsMouselooking` still answers nil. With nothing held the command word is empty, and the
/// motion handler turns nothing (`0x51440c`); a key held, W here, gives the word a bit, and the
/// mouse turns the camera and not the body, since the hand-off wants the TurnOrAction bit
/// (`0x5145e0`). It lasts with no button until the next call leaves it (`0x50f940(0)`).
#[test]
fn toggle_mouse_move_turns_the_camera_alone_under_a_held_command_until_toggled_back() {
    let mut w = world(false);
    lua(&mut w, "ToggleMouseMove()");
    frame(&mut w, 0.0);
    assert!(looking(&w));
    assert_eq!(cursor(&mut w).grab_mode, CursorGrabMode::Locked);
    frame(&mut w, 100.0);
    assert_eq!(yaw(&mut w), 0.0, "an empty command word turns nothing");
    let held = {
        let mut script = w.non_send_resource_mut::<UiScript>();
        script.run_binding("MoveForwardStart()").unwrap();
        script.take_binding_input()
    };
    for input in held {
        w.resource_mut::<BindingsState>().apply_unhosted(input);
    }
    frame(&mut w, 100.0);
    assert!(yaw(&mut w) != 0.0, "W held, the camera turns");
    assert_eq!(w.resource::<Hand>().face_yaw, 0.0, "the body does not");
    assert!(!is_mouselooking(&mut w));
    frame(&mut w, 0.0);
    assert!(looking(&w), "no button is needed to stay");
    lua(&mut w, "ToggleMouseMove()");
    frame(&mut w, 0.0);
    assert!(!looking(&w));
    assert_eq!(cursor(&mut w).grab_mode, CursorGrabMode::None);
}

/// A press under a toggled mouselook sets its bit, and that bit's release is the last mouse
/// release, which leaves mouselook (`0x514b70` → `0x50f940`).
#[test]
fn a_button_released_under_a_toggled_mouselook_ends_it() {
    let mut w = world(false);
    lua(&mut w, "ToggleMouseMove()");
    frame(&mut w, 0.0);
    assert!(looking(&w));
    press(&mut w, MouseButton::Left);
    frame(&mut w, 0.0);
    assert!(looking(&w));
    release(&mut w, MouseButton::Left);
    frame(&mut w, 0.0);
    assert!(!looking(&w));
}

/// `CameraOrSelectOrMoveStop(1)` at the last mouse release sets the camera's sticky bit
/// (`0x514c48` → `0x50f958`); a plain release clears it (`0x50f95d`), and so does a toggle's
/// leave (`0x50f91b`).
#[test]
fn a_sticky_release_sets_the_cameras_bit_and_a_plain_one_clears_it() {
    let mut w = world(false);
    let sticky = |w: &World| w.resource::<CameraControl>().follow.sticky;
    let drag = |w: &mut World, body: &str| {
        press(w, MouseButton::Left);
        frame(w, 0.0);
        release(w, MouseButton::Left);
        binding(w, body);
        frame(w, 0.0);
    };
    drag(&mut w, "CameraOrSelectOrMoveStop(1)");
    assert!(sticky(&w));
    drag(&mut w, "CameraOrSelectOrMoveStop()");
    assert!(!sticky(&w));
    drag(&mut w, "CameraOrSelectOrMoveStop(1)");
    lua(&mut w, "ToggleMouseMove()");
    frame(&mut w, 0.0);
    assert!(sticky(&w), "entering leaves the bit");
    lua(&mut w, "ToggleMouseMove()");
    frame(&mut w, 0.0);
    assert!(!sticky(&w));
}

/// The look calls a packet's handler makes land after the frame's input latch and before the
/// frame time's controller, and those of the frame pass's `OnUpdate` after it: the reference's
/// press handler moves the input control inside the call (`0x514210` → `0x515090` → `0x514840`),
/// ahead of the input tick in category 5 when a packet's handler makes it, and after it when the
/// frame pass does.
#[test]
fn the_late_look_calls_land_between_the_packets_and_the_controller() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    use crate::test_support::runs_before;
    assert!(runs_before(
        &mut app,
        crate::lanes::fire_in_order,
        apply_late_look_calls
    ));
    assert!(crate::ui_script::tick_runs_after(
        &mut app,
        apply_late_look_calls
    ));
    assert!(runs_before(
        &mut app,
        apply_late_look_calls,
        super::super::controller::control
    ));
}

/// A `MouselookStart` a packet's handler makes, after the frame's input latch, holds the
/// TurnOrAction channel for the frame's controller and raises it, and the next frame's input
/// engages its session, the first motion after the call.
#[test]
fn a_mouselook_start_after_the_latch_reaches_the_frames_controller() {
    let mut w = world(false);
    frame(&mut w, 0.0);
    w.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::ZERO;
    w.run_system_once(latch_world_mouse).unwrap();
    w.run_system_once(look_frame).unwrap();
    lua(&mut w, "MouselookStart()");
    w.run_system_once(apply_late_look_calls).unwrap();
    let binds = w.resource::<BindingsState>();
    assert!(
        binds.just_pressed(Input::TurnOrAction)
            && w.resource::<CameraControl>().world_mouse.late_rise(),
        "the controller reads the bit risen after its session"
    );
    w.resource_mut::<ButtonInput<MouseButton>>().clear();
    frame(&mut w, 40.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Right));
    assert!(yaw(&mut w) < 0.0, "the next motion turns");
}

/// A `ToggleMouseMove` made after the frame's look session reaches the next one: what a call
/// hands the session waits for it, not for the frame's end.
#[test]
fn a_toggle_after_the_session_reaches_the_next_one() {
    let mut w = world(false);
    frame(&mut w, 0.0);
    w.run_system_once(latch_world_mouse).unwrap();
    w.run_system_once(look_frame).unwrap();
    lua(&mut w, "ToggleMouseMove()");
    w.run_system_once(apply_late_look_calls).unwrap();
    w.resource_mut::<ButtonInput<MouseButton>>().clear();
    frame(&mut w, 0.0);
    assert!(looking(&w), "the toggle entered mouselook");
}

/// A left orbit is freelook as a right drag is (`[cam+0x90] & 1`, `0x50fe41`): its entry hands the
/// ground's lean into the pitch (`0x50fe72`, `0x50fe79`) and its leave takes it back (`0x50fde6`,
/// `0x50fded`), so the view holds still across both.
#[test]
fn a_left_orbit_hands_the_ground_lean_into_the_pitch_and_back() {
    let mut w = world(false);
    frame(&mut w, 0.0);
    let options = CameraOptions {
        terrain_tilt: true,
        ..CameraOptions::default()
    };
    let moving = SubjectState {
        move_flags: crate::creature_anim::move_flags::FORWARD,
        ..SubjectState::default()
    };
    {
        let mut rig = w.resource_mut::<CameraControl>();
        for _ in 0..900 {
            rig.terrain_tilt.advance(
                || 0.5,
                true,
                &moving,
                FollowStyle::Smart,
                &options,
                1.0 / 60.0,
            );
        }
    }
    let lean = w.resource::<CameraControl>().terrain_tilt.pitch();
    assert!(lean > 0.0, "uphill leans the view: {lean}");
    let pitch = |w: &mut World| w.query::<&FlyCam>().single(w).unwrap().pitch;
    let hand_off = |w: &mut World| {
        w.resource_scope(|w, mut rig: Mut<CameraControl>| {
            let mut cam = w.query::<&mut FlyCam>().single_mut(w).unwrap();
            rig.hand_off_ground_pitch(&mut cam);
        });
    };
    hand_off(&mut w);
    let level = pitch(&mut w);

    press(&mut w, MouseButton::Left);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, Some(LookButton::Left));
    hand_off(&mut w);
    assert!(
        (pitch(&mut w) - (level + lean)).abs() < 1.0e-6,
        "the entry pushes the lean into the pitch"
    );
    assert_eq!(w.resource::<CameraControl>().terrain_tilt.pitch(), 0.0);

    release(&mut w, MouseButton::Left);
    frame(&mut w, 0.0);
    assert_eq!(w.resource::<CameraControl>().look, None);
    hand_off(&mut w);
    assert!(
        (pitch(&mut w) - level).abs() < 1.0e-6,
        "the leave pops it back out"
    );
    assert_eq!(w.resource::<CameraControl>().terrain_tilt.pitch(), lean);
}
