//! The world click among the frame's input records, over the whole client: the mouse commands arm
//! it and pick at the press (`0x514810` → `0x481f00` → `0x481190`), and the last primary's release
//! dispatches it (`0x514b70` → `0x481f60`), inside the record whose binding ran them, so a UI click
//! and a world click in one frame resolve in the order they came (`0x423920` → `0x4245b0`).

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use bevy::camera::primitives::Aabb;
use bevy::camera::RenderTargetInfo;
use bevy::input::mouse::MouseButtonInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::{CursorMoved, WindowEvent};

use benilla_ui::script::UiScript;
use benilla_world::interact::CreaturePickPart;
use benilla_world::view::WorldCamera;

use super::Selection;
use crate::net::{GuidIndex, SelfGuid};

const ME: u64 = 0x10;
const MOB: u64 = 0xF130_0000_0031_0001;

/// Over the left half's button, which targets us on its `OnClick`.
const ON_BUTTON: Vec2 = Vec2::new(100.0, 400.0);
/// Over the world, where the mob stands.
const ON_MOB: Vec2 = Vec2::new(900.0, 600.0);
/// Over the world, beside the mob.
const BESIDE_MOB: Vec2 = Vec2::new(900.0, 100.0);

/// A create block of `kind` at full health, with its `OBJECT_FIELD_TYPE` mask.
fn created(guid: u64, kind: EntityKind, as_type: ObjectType, mask: u32) -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id: None,
        position: [3.0, 0.0, 0.0],
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        fields: ObjectFields::from_pairs(&[
            (2, mask),
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
        ])
        .into_created(as_type),
    }
}

#[derive(Resource)]
struct PrimaryId(Entity);

/// The whole client in the world with us and a mob streamed, the mob's pick box on the camera's
/// ray through [`ON_MOB`], `BUTTON1` bound to `bindings` (the stock `CAMERAORSELECTORMOVE`, or a
/// rebind), and a button over the screen's left half whose `OnClick` runs `TargetUnit("player")`.
fn client(binding: &str) -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let mut script = UiScript::new().expect("a VM");
    script.set_default_bindings(benilla_ui::script::keybind::parse_bindings_wtf(&format!(
        "bind BUTTON1 {binding}\r\n"
    )));
    script.load_binding_set(1);
    script.register_bindings(
        &benilla_ui::bindings_xml::parse(
            r#"<Bindings>
                <Binding name="CAMERAORSELECTORMOVE" runOnUp="true">
                    if ( keystate == "down" ) then
                        CameraOrSelectOrMoveStart();
                    else
                        CameraOrSelectOrMoveStop();
                    end
                </Binding>
                <Binding name="PROBEBUTTON" runOnUp="true">LOG = LOG .. "b1 " .. keystate .. ";"</Binding>
            </Bindings>"#,
        )
        .expect("well-formed"),
    );
    script
        .run(
            r#"LOG = ""
            CreateFrame("WorldFrame", "WorldFrame")
            local b = CreateFrame("Button", "TargetMeButton")
            b:SetPoint("BOTTOMLEFT", 0, 0) b:SetWidth(512) b:SetHeight(768)
            b:EnableMouse(true)
            b:SetScript("OnClick", function() TargetUnit("player") LOG = LOG .. "click;" end)"#,
        )
        .expect("the button builds");
    script.set_screen_size(1024.0, 768.0);
    script.resolve();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
    let mut window = Window {
        resolution: UVec2::new(1024, 768).into(),
        ..default()
    };
    window.set_cursor_position(Some(BESIDE_MOB));
    let id = app
        .world_mut()
        .spawn((window, bevy::window::PrimaryWindow))
        .id();
    app.world_mut().insert_resource(PrimaryId(id));
    for ev in crate::net::wire([
        created(ME, EntityKind::Player, ObjectType::Player, 0x19),
        created(MOB, EntityKind::Unit, ObjectType::Unit, 0x9),
        SessionEvent::ObjectUpdateEnd,
    ]) {
        inbound.send(ev).unwrap();
    }
    app.update();
    app.update();
    app
}

/// Give the world camera a projection the headless client has no render to compute, and put the
/// mob's pick box on its ray through [`ON_MOB`], five yards out, as the camera stands now.
fn aim(app: &mut App) {
    let world = app.world_mut();
    let mut q = world.query_filtered::<(&mut Camera, &GlobalTransform), With<WorldCamera>>();
    let (mut camera, cam_tf) = q.single_mut(world).expect("the world camera");
    camera.computed.clip_from_view = Mat4::perspective_infinite_reverse_rh(1.0, 4.0 / 3.0, 0.1);
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(1024, 768),
        scale_factor: 1.0,
    });
    let ray = camera
        .viewport_to_world(cam_tf, ON_MOB)
        .expect("the camera projects");
    let at = ray.origin + *ray.direction * 5.0;
    let mob = world.resource::<GuidIndex>().0[&MOB];
    world.entity_mut(mob).insert(InheritedVisibility::VISIBLE);
    world.spawn((
        ChildOf(mob),
        CreaturePickPart,
        Aabb::from_min_max(Vec3::splat(-0.5), Vec3::splat(0.5)),
        GlobalTransform::from_translation(at),
        InheritedVisibility::VISIBLE,
    ));
}

/// One window message as winit hands it over: its typed message and the same message on the
/// window-event stream.
fn send(app: &mut App, event: WindowEvent) {
    let world = app.world_mut();
    match event.clone() {
        WindowEvent::MouseButtonInput(e) => {
            world.write_message(e);
        }
        WindowEvent::CursorMoved(e) => {
            world.write_message(e);
        }
        _ => unreachable!("the fixture sends buttons and moves"),
    }
    world.write_message(event);
}

fn move_to(app: &mut App, at: Vec2) {
    let window = app.world().resource::<PrimaryId>().0;
    send(
        app,
        WindowEvent::CursorMoved(CursorMoved {
            window,
            position: at,
            delta: None,
        }),
    );
    app.world_mut()
        .query::<&mut Window>()
        .single_mut(app.world_mut())
        .expect("the window")
        .set_cursor_position(Some(at));
}

fn button(app: &mut App, state: ButtonState) {
    let window = app.world().resource::<PrimaryId>().0;
    send(
        app,
        WindowEvent::MouseButtonInput(MouseButtonInput {
            button: MouseButton::Left,
            state,
            window,
        }),
    );
}

/// A left click at `at`: the move there, the press and the release.
fn click_at(app: &mut App, at: Vec2) {
    move_to(app, at);
    button(app, ButtonState::Pressed);
    button(app, ButtonState::Released);
}

fn target(app: &App) -> Option<u64> {
    app.world().resource::<Selection>().guid
}

/// A click on a button whose `OnClick` targets us, then a click on the mob, in one frame: the
/// target ends on the mob. Reversed, it ends on us: the world click resolved at its own record,
/// ahead of the button's.
#[test]
fn a_ui_click_and_a_world_click_in_one_frame_resolve_in_arrival_order() {
    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    click_at(&mut app, ON_BUTTON);
    click_at(&mut app, ON_MOB);
    app.update();
    assert_eq!(target(&app), Some(MOB), "the world click came last");

    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    click_at(&mut app, ON_MOB);
    click_at(&mut app, ON_BUTTON);
    app.update();
    assert_eq!(target(&app), Some(ME), "the button's click came last");
}

/// A press at a new position picks what is under the press: the frame's hover stood beside the
/// mob, and the press came after a move onto it.
#[test]
fn a_press_after_a_move_picks_what_is_under_the_press() {
    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    app.update();
    assert_eq!(target(&app), None);
    click_at(&mut app, ON_MOB);
    app.update();
    assert_eq!(target(&app), Some(MOB));
}

/// `BUTTON1` bound away from `CameraOrSelectOrMoveStart` arms nothing, so its click selects
/// nothing in the world, as in 1.12.1; its binding still runs.
#[test]
fn a_button_rebound_away_from_the_mouse_command_clicks_nothing() {
    let mut app = client("PROBEBUTTON");
    aim(&mut app);
    click_at(&mut app, ON_MOB);
    app.update();
    assert_eq!(target(&app), None);
    let log: String = app
        .world()
        .non_send_resource::<UiScript>()
        .eval("return LOG")
        .unwrap();
    assert_eq!(log, "b1 down;b1 up;");
}
