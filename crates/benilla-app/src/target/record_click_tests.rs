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
/// A spell whose cursor wants a ground point.
const BLIZZARD: u32 = 10;
const MOB: u64 = 0xF130_0000_0031_0001;
/// A second unit, off the camera's ray, whose plate a test lays over the mob.
const PLATED: u64 = 0xF130_0000_0032_0002;

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

/// The client's sends.
#[derive(Resource)]
struct Sent(crossbeam_channel::Receiver<crate::net::ClientCommand>);

/// [`client_bound`] with `BUTTON1` bound to `binding` (the stock `CAMERAORSELECTORMOVE`, or a
/// rebind) and `BUTTON2` to the stock `TURNORACTION`.
fn client(binding: &str) -> App {
    client_bound(&format!(
        "bind BUTTON1 {binding}\r\nbind BUTTON2 TURNORACTION\r\n"
    ))
}

/// The whole client in the world with us and a mob streamed, the mob's pick box on the camera's
/// ray through [`ON_MOB`], the buttons bound as `wtf` binds them, and a button over the screen's
/// left half whose `OnClick` runs `TargetUnit("player")`.
fn client_bound(wtf: &str) -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.insert_resource(Sent(sent));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let mut script = UiScript::new().expect("a VM");
    script.set_default_bindings(benilla_ui::script::keybind::parse_bindings_wtf(wtf));
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
                <Binding name="TURNORACTION" runOnUp="true">
                    if ( keystate == "down" ) then
                        TurnOrActionStart();
                    else
                        TurnOrActionStop();
                    end
                </Binding>
                <Binding name="PROBEBUTTON" runOnUp="true">LOG = LOG .. "b1 " .. keystate .. ";"</Binding>
                <Binding name="PROBESELECT" runOnUp="true">
                    if ( keystate == "down" ) then
                        CameraOrSelectOrMoveStart();
                    else
                        CameraOrSelectOrMoveStop();
                        LOG = LOG .. tostring(UnitName("target")) .. ";"
                    end
                </Binding>
                <Binding name="PROBEKEYCLICK">
                    CameraOrSelectOrMoveStart();
                    CameraOrSelectOrMoveStop();
                </Binding>
                <Binding name="PROBESTICKY" runOnUp="true">
                    if ( keystate == "down" ) then
                        CameraOrSelectOrMoveStart();
                    else
                        if ARMED then SetCVar("deselectOnClick", "0") end
                        CameraOrSelectOrMoveStop();
                        LOG = LOG .. tostring(UnitName("target")) .. ";"
                    end
                </Binding>
                <Binding name="PROBEKEYACTION">
                    TurnOrActionStart();
                    TurnOrActionStop();
                </Binding>
                <Binding name="PROBEKEYSELL">
                    TurnOrActionStart();
                    TurnOrActionStop();
                    LOG = LOG .. "after;"
                </Binding>
                <Binding name="PROBEKEYREAD">
                    CameraOrSelectOrMoveStart();
                    CameraOrSelectOrMoveStop();
                    LOG = LOG .. tostring(UnitName("target")) .. ";"
                </Binding>
                <Binding name="PROBETHENBUTTON" runOnUp="true">
                    if ( keystate == "down" ) then
                        CameraOrSelectOrMoveStart();
                    else
                        CameraOrSelectOrMoveStop();
                        TargetMeButton:Click();
                    end
                </Binding>
                <Binding name="PROBERIGHT">
                    LOG = LOG .. "targeting " .. tostring(SpellIsTargeting())
                        .. ", repair " .. tostring(InRepairMode()) .. ";"
                </Binding>
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
        created(PLATED, EntityKind::Unit, ObjectType::Unit, 0x9),
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
    press_button(app, MouseButton::Left, state);
}

fn press_button(app: &mut App, button: MouseButton, state: ButtonState) {
    let window = app.world().resource::<PrimaryId>().0;
    send(
        app,
        WindowEvent::MouseButtonInput(MouseButtonInput {
            button,
            state,
            window,
        }),
    );
}

/// A right click at `at`: the move there, the press and the release.
fn right_click_at(app: &mut App, at: Vec2) {
    move_to(app, at);
    press_button(app, MouseButton::Right, ButtonState::Pressed);
    press_button(app, MouseButton::Right, ButtonState::Released);
}

/// The targeting cursor up for a spell whose word wants a ground point (`TARGET_FLAG_DEST_LOCATION`
/// `0x40`), as the VM reads it.
fn arm_ground_cast(app: &mut App) {
    app.world_mut()
        .resource_mut::<crate::spell::SpellTargeting>()
        .enter(BLIZZARD, crate::spell::CastCommit::Spell, 0x0040);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_spell_targeting(true);
}

fn targeting(app: &App) -> bool {
    app.world()
        .resource::<crate::spell::SpellTargeting>()
        .active()
}

fn log(app: &App) -> String {
    app.world()
        .non_send_resource::<UiScript>()
        .eval("return LOG")
        .unwrap()
}

/// Repair mode armed, as `ShowRepairCursor` arms it at a repair vendor.
fn in_repair_mode(app: &mut App) {
    let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
    script.set_merchant(Some(benilla_ui::script::MerchantState {
        can_repair: true,
        ..Default::default()
    }));
    script.run("ShowRepairCursor()").unwrap();
    assert!(script.repair_mode());
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

/// The release's click runs inside `CameraOrSelectOrMoveStop` (`0x514b70` → `0x481f60`), so the
/// binding body's next line reads the target the click selected.
#[test]
fn the_binding_line_after_the_release_reads_the_clicks_target() {
    let mut app = client("PROBESELECT");
    aim(&mut app);
    click_at(&mut app, ON_MOB);
    app.update();
    assert_eq!(target(&app), Some(MOB));
    let name: String = app
        .world()
        .non_send_resource::<UiScript>()
        .eval(r#"return tostring(UnitName("target"))"#)
        .unwrap();
    assert_ne!(name, "nil");
    assert_eq!(
        log(&app),
        format!("{name};"),
        "the line after the stop read the mob"
    );
}

/// A hook around `CameraOrSelectOrMoveStop` reads the click's target inside the hook, after the
/// command it wraps returns.
#[test]
fn a_hook_around_the_release_reads_the_clicks_target() {
    let mut app = client("CAMERAORSELECTORMOVE");
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"local stop = CameraOrSelectOrMoveStop
            CameraOrSelectOrMoveStop = function(sticky)
                stop(sticky)
                LOG = LOG .. tostring(UnitName("target")) .. ";"
            end"#,
        )
        .unwrap();
    aim(&mut app);
    click_at(&mut app, ON_MOB);
    app.update();
    assert_eq!(target(&app), Some(MOB));
    let name: String = app
        .world()
        .non_send_resource::<UiScript>()
        .eval(r#"return tostring(UnitName("target"))"#)
        .unwrap();
    assert_ne!(name, "nil");
    assert_eq!(log(&app), format!("{name};"), "the hook read the mob");
}

/// A body that releases the world click and then clicks a button whose `OnClick` targets us ends on
/// us: the world click selected inside the release, before the button's `TargetUnit`.
#[test]
fn a_ui_click_after_the_release_in_one_body_lands_after_the_world_click() {
    let mut app = client("PROBETHENBUTTON");
    aim(&mut app);
    click_at(&mut app, ON_MOB);
    app.update();
    assert_eq!(log(&app), "click;");
    assert_eq!(target(&app), Some(ME), "the button's target came last");
}

/// A press of the F key, in its window message and its typed one.
fn press_f(app: &mut App) {
    let ev = bevy::input::keyboard::KeyboardInput {
        key_code: KeyCode::KeyF,
        logical_key: bevy::input::keyboard::Key::Character("f".into()),
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window: app.world().resource::<PrimaryId>().0,
    };
    app.world_mut().write_message(ev.clone());
    app.world_mut()
        .write_message(WindowEvent::KeyboardInput(ev));
}

/// The client with F bound to `command`.
fn client_with_f(command: &str) -> App {
    client_bound(&format!(
        "bind BUTTON1 CAMERAORSELECTORMOVE\r\nbind BUTTON2 TURNORACTION\r\nbind F {command}\r\n"
    ))
}

/// The name `UnitName("target")` answers now, which is the mob's.
fn target_name(app: &App) -> String {
    let name: String = app
        .world()
        .non_send_resource::<UiScript>()
        .eval(r#"return tostring(UnitName("target"))"#)
        .unwrap();
    assert_ne!(name, "nil");
    name
}

/// A key whose binding arms and releases the click in one body, with no press of the mouse's,
/// clicks what is under the cursor.
#[test]
fn a_key_bound_click_selects_under_the_cursor() {
    let mut app = client_with_f("PROBEKEYCLICK");
    aim(&mut app);
    move_to(&mut app, ON_MOB);
    app.update();
    press_f(&mut app);
    app.update();
    assert_eq!(target(&app), Some(MOB), "the key's click selected the mob");
}

/// A key's binding body picks inside `CameraOrSelectOrMoveStart` (`0x5141ae` → `ArmClick
/// 0x514810` → `0x481f00`), at the cursor the last mouse event left (`0x481f1d`), and the stop
/// dispatches inside its call: the body's next line reads the mob.
#[test]
fn a_key_bound_click_reads_its_target_on_the_next_line() {
    let mut app = client_with_f("PROBEKEYREAD");
    aim(&mut app);
    move_to(&mut app, ON_MOB);
    app.update();
    press_f(&mut app);
    app.update();
    assert_eq!(target(&app), Some(MOB));
    let name = target_name(&app);
    assert_eq!(
        log(&app),
        format!("{name};"),
        "the line after the stop read the mob"
    );
}

/// A release whose body turns Sticky Targeting on, then dispatches its click on the empty world:
/// `CVar::Set` (`0x63df50`) runs `deselectOnClick`'s callback inside the write (`0x63df7d`), so the
/// click's deselect gate reads 0 and the body's next line still reads the mob. Main ran the
/// callback in the frame's sync, after the input pass, and deselected.
#[test]
fn a_set_cvar_moves_its_knob_before_the_chunks_next_line() {
    let mut app = client("PROBESTICKY");
    aim(&mut app);
    click_at(&mut app, ON_MOB);
    app.update();
    assert_eq!(target(&app), Some(MOB));
    let name = target_name(&app);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run("LOG = '' ARMED = true")
        .unwrap();
    click_at(&mut app, BESIDE_MOB);
    app.update();
    assert_eq!(target(&app), Some(MOB), "the click kept the target");
    assert_eq!(
        log(&app),
        format!("{name};"),
        "the line after the click read it"
    );
}

/// A chest, beside us.
const CHEST: u64 = 0xF110_0000_0000_00D4;
/// Over the world, where the chest's pick box stands.
const ON_CHEST: Vec2 = Vec2::new(700.0, 600.0);

/// A chest (type 3) where we stand, its drawn pick box on the camera's ray through [`ON_CHEST`],
/// five yards out, then `frames` frames run, so its part's stream edge is behind the click.
fn chest(app: &mut App, frames: usize) -> Entity {
    let world = app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    let (tf, gt) = (
        *world.get::<Transform>(me).expect("our body"),
        *world.get::<GlobalTransform>(me).expect("our body"),
    );
    let store = crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (2, 0x21),
        (21, 3),
    ]));
    let chest = world
        .spawn((
            crate::net::Guid(CHEST),
            crate::net::NetEntity {
                kind: EntityKind::GameObject,
                display_id: None,
                scale: 1.0,
            },
            tf,
            gt,
            store,
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(CHEST, chest);
    let mut q = world.query_filtered::<(&Camera, &GlobalTransform), With<WorldCamera>>();
    let (camera, cam_tf) = q.single(world).expect("the world camera");
    let ray = camera
        .viewport_to_world(cam_tf, ON_CHEST)
        .expect("the camera projects");
    let at = ray.origin + *ray.direction * 5.0;
    world.spawn((
        ChildOf(chest),
        benilla_world::interact::GoPickPart,
        benilla_world::interact::PickBox,
        Aabb::from_min_max(Vec3::splat(-0.5), Vec3::splat(0.5)),
        GlobalTransform::from_translation(at),
        ViewVisibility::HIDDEN,
    ));
    for _ in 0..frames {
        app.update();
    }
    // Drawn this frame: the box's visibility, which the frame's end resets headless.
    let world = app.world_mut();
    let mut parts =
        world.query_filtered::<&mut ViewVisibility, With<benilla_world::interact::GoPickPart>>();
    use bevy::camera::visibility::SetViewVisibility;
    for mut vis in parts.iter_mut(world) {
        vis.set_visible();
    }
    chest
}

/// Whether the client sent the chest's use.
fn used_chest(app: &App) -> bool {
    app.world()
        .resource::<Sent>()
        .0
        .try_iter()
        .any(|c| matches!(c, crate::net::ClientCommand::GameObjUse { guid: CHEST }))
}

/// A right click on a chest whose model streamed in frames ago uses it: the press's pick reads
/// the GameObject pick set every frame keeps, not one only a pick's own run rebuilds.
#[test]
fn a_right_click_uses_a_chest_streamed_in_frames_ago() {
    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    chest(&mut app, 3);
    right_click_at(&mut app, ON_CHEST);
    app.update();
    assert!(used_chest(&app), "the click used the chest");
}

/// A key's `TurnOrActionStart`/`Stop` over a chest whose model streamed in frames ago uses it,
/// the arm's in-call pick reading the same set.
#[test]
fn a_key_bound_action_uses_a_chest_streamed_in_frames_ago() {
    let mut app = client_with_f("PROBEKEYACTION");
    aim(&mut app);
    move_to(&mut app, ON_CHEST);
    app.update();
    chest(&mut app, 3);
    press_f(&mut app);
    app.update();
    assert!(used_chest(&app), "the key's click used the chest");
}

/// [`PLATED`]'s plate shown over [`ON_MOB`] and hovered there, as the walk and the plate hover's
/// seed after the tick leave a still cursor on it.
fn plate_over_mob(app: &mut App) {
    use benilla_ui::script::{PlateGeometry, PlateState};
    let state = PlateState {
        key: PLATED,
        // The plate hangs below its top centre; [`ON_MOB`] is (900, 168) in the root's y-up units.
        top_centre: (900.0, 180.0),
        health: 100.0,
        max_health: 100.0,
        bar_colour: [1.0, 0.0, 0.0],
        name: "Plated".into(),
        level: Some(1),
        skull: false,
        level_colour: [1.0, 1.0, 0.0],
        raid_icon: None,
        alpha: 255,
        lit: false,
        hovered: false,
    };
    let geometry = PlateGeometry {
        width: 128.0,
        height: 32.0,
        bar_off_x: 4.0,
        bar_off_y: 4.0,
        bar_width: 103.0,
        bar_height: 9.0,
        level_off_x: 11.8,
        level_off_y: 9.1,
        skull_size: 12.8,
        raid_size: 25.6,
        name_height: 12.8,
        level_height: 11.0,
        shadow_offset: 1.0,
    };
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script
            .run(r#"NAMEPLATE_FONT = "Fonts\FRIZQT__.TTF" WorldFrame:SetAllPoints()"#)
            .unwrap();
        script.resolve();
        script.sync_nameplates(geometry, &[state]);
        script.resolve();
        script.mouse_move(900.0, 168.0);
        assert_eq!(
            script.hovered_nameplate(),
            Some(PLATED),
            "the plate holds the hover"
        );
    }
    use bevy::ecs::system::RunSystemOnce;
    app.world_mut()
        .run_system_once(crate::vplates::seed_plate_hover)
        .unwrap();
}

/// A key's click picks the world under the cursor, never a plate over it: `PickAtCursor`
/// (`0x481190`) traces the terrain and the world frame's drawn objects (`0x480df0`, `0x480a50`)
/// and asks no interface frame. A plate takes only its own press (`0x7cb910`).
#[test]
fn a_key_bound_click_under_a_plate_selects_the_world_under_it() {
    let mut app = client_with_f("PROBEKEYCLICK");
    aim(&mut app);
    move_to(&mut app, ON_MOB);
    app.update();
    plate_over_mob(&mut app);
    press_f(&mut app);
    app.update();
    assert_eq!(target(&app), Some(MOB), "the key's click selected the mob");
}

/// A button over the mob whose `OnClick` runs `body`, and a recorder of `PLAYER_TARGET_CHANGED`.
fn button_over_mob(app: &mut App, body: &str) {
    let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
    script
        .run(&format!(
            r#"local c = CreateFrame("Button", "OverMobButton")
            c:SetPoint("BOTTOMLEFT", 850, 118) c:SetWidth(100) c:SetHeight(100)
            c:EnableMouse(true)
            c:SetScript("OnClick", function() {body} end)
            local e = CreateFrame("Frame")
            e:RegisterEvent("PLAYER_TARGET_CHANGED")
            e:SetScript("OnEvent", function() LOG = LOG .. "changed;" end)"#
        ))
        .expect("the button builds");
    script.resolve();
}

/// A button's `OnClick` that arms and releases the click picks at the cursor its click left, the
/// world under the button, inside the arm's call: the handler's next line reads the mob.
#[test]
fn a_buttons_click_handler_reads_its_world_clicks_target() {
    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    button_over_mob(
        &mut app,
        r#"CameraOrSelectOrMoveStart() CameraOrSelectOrMoveStop()
        LOG = LOG .. tostring(UnitName("target")) .. ";""#,
    );
    click_at(&mut app, ON_MOB);
    app.update();
    assert_eq!(target(&app), Some(MOB));
    let name = target_name(&app);
    assert_eq!(
        log(&app),
        format!("changed;{name};"),
        "the handler read the mob"
    );
}

/// The select inside the handler's `CameraOrSelectOrMoveStop` signals `PLAYER_TARGET_CHANGED` in
/// place (`0x514b70` → `0x481f60` → `SetSelection`), before the handler's next line.
#[test]
fn a_buttons_click_handler_sees_the_target_change_inside_it() {
    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    button_over_mob(
        &mut app,
        r#"CameraOrSelectOrMoveStart() CameraOrSelectOrMoveStop() LOG = LOG .. "after;""#,
    );
    click_at(&mut app, ON_MOB);
    app.update();
    assert_eq!(target(&app), Some(MOB));
    assert_eq!(
        log(&app),
        "changed;after;",
        "the change fired inside the handler"
    );
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
    assert_eq!(log(&app), "b1 down;b1 up;");
}

/// The world frame's right-press hook runs ahead of the binding (`0x483c62`, then `0x483c93`): the
/// binding reads the pending target the hook cancelled, and the repair mode it ended.
#[test]
fn a_right_press_hooks_ahead_of_its_binding() {
    let mut app = client_bound("bind BUTTON2 PROBERIGHT\r\n");
    arm_ground_cast(&mut app);
    right_click_at(&mut app, BESIDE_MOB);
    app.update();
    assert!(!targeting(&app), "the hook cancelled the pending target");
    assert_eq!(
        log(&app),
        "targeting nil, repair nil;",
        "the binding read the cancel"
    );

    let mut app = client_bound("bind BUTTON2 PROBERIGHT\r\n");
    in_repair_mode(&mut app);
    right_click_at(&mut app, BESIDE_MOB);
    app.update();
    assert_eq!(
        log(&app),
        "targeting nil, repair nil;",
        "the binding read repair mode ended"
    );
}

/// A right click on the world with a pending target cancels it and sends nothing; the click's
/// action is the router's, on the pick the press traced after the cancel.
#[test]
fn a_right_click_cancels_the_pending_target() {
    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    arm_ground_cast(&mut app);
    right_click_at(&mut app, BESIDE_MOB);
    app.update();
    assert!(!targeting(&app));
    let sent: Vec<_> = app.world().resource::<Sent>().0.try_iter().collect();
    assert!(
        !sent
            .iter()
            .any(|c| matches!(c, crate::net::ClientCommand::CastSpellAtDest { .. })),
        "a cancel casts nothing: {sent:?}"
    );
}

/// The right click's payload leg clears the cursor and signals in place (`0x492c90`, `0x492d30`
/// → `0x495190`), so its listeners run before the router's leg goes on: nothing the click writes
/// after the payload leg follows them ([`crate::script_calls::census::writes_after`]).
#[test]
fn a_right_clicks_payload_clear_runs_ahead_of_the_rest_of_its_leg() {
    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    arm_ground_cast(&mut app);
    let _watch = benilla_ui::script::census::watch_writes();
    benilla_ui::script::census::take_writes();
    right_click_at(&mut app, BESIDE_MOB);
    app.update();
    assert_eq!(
        crate::script_calls::census::writes_after(),
        Vec::<String>::new()
    );
}

/// Over the world's ground, away from the mob and the button.
const ON_GROUND: Vec2 = Vec2::new(800.0, 700.0);

/// A ground plane under [`ON_GROUND`], ten yards out along its ray: a static doodad's opaque batch,
/// which the press's trace hits.
fn ground(app: &mut App) {
    let world = app.world_mut();
    let mut q = world.query_filtered::<(&Camera, &GlobalTransform), With<WorldCamera>>();
    let (camera, cam_tf) = q.single(world).expect("the world camera");
    let ray = camera
        .viewport_to_world(cam_tf, ON_GROUND)
        .expect("the camera projects");
    let at = ray.origin + *ray.direction * 10.0;
    // A wide triangle in the model's horizontal plane (WoW z = 0, Bevy y = 0), under `at`.
    let plane = benilla_formats::RenderSubmesh {
        positions: vec![[50.0, 50.0, 0.0], [50.0, -50.0, 0.0], [-50.0, 0.0, 0.0]],
        indices: vec![0, 1, 2],
        ..Default::default()
    };
    let plane = world
        .spawn((
            benilla_world::interact::PickMesh(std::sync::Arc::new(plane)),
            benilla_world::interact::PickOccluderPart,
            GlobalTransform::from_translation(at),
            ViewVisibility::HIDDEN,
        ))
        .id();
    use bevy::camera::visibility::SetViewVisibility;
    world
        .get_mut::<ViewVisibility>(plane)
        .expect("spawned")
        .set_visible();
}

/// A left click on the ground with a ground spell's cursor up binds the press's point and sends
/// the cast there (`0x492c90` → `0x492580` → `BindLocation 0x6e60f0`), ending the cursor.
#[test]
fn a_ground_click_casts_at_the_press_point() {
    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    ground(&mut app);
    arm_ground_cast(&mut app);
    click_at(&mut app, ON_GROUND);
    app.update();
    let sent: Vec<_> = app.world().resource::<Sent>().0.try_iter().collect();
    assert!(
        sent.iter()
            .any(|c| matches!(c, crate::net::ClientCommand::CastSpellAtDest { .. })),
        "the click casts at the ground: {sent:?}"
    );
    assert!(!targeting(&app), "the cast ended the cursor");
}

/// A press held past the free window whose drag's last motion came in the release's own frame is
/// a drag, not a click (`0x514ae0`): the release's test counts the frame's travel, as the motion
/// reaches `0x514400` ahead of it.
#[test]
fn a_drag_whose_last_motion_is_in_the_release_frame_clicks_nothing() {
    let mut app = client("CAMERAORSELECTORMOVE");
    aim(&mut app);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_millis(300),
    ));
    move_to(&mut app, ON_MOB);
    button(&mut app, ButtonState::Pressed);
    app.update();
    app.world_mut()
        .write_message(bevy::input::mouse::MouseMotion {
            delta: Vec2::new(200.0, 0.0),
        });
    button(&mut app, ButtonState::Released);
    app.update();
    assert_eq!(target(&app), None, "the drag selected nothing");
}

/// An item on the cursor, picked from the backpack's first slot.
fn hold_an_item(app: &mut App) {
    use benilla_ui::script::{CursorItem, CursorPayload, HeldCursor};
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .seed_held_cursor(HeldCursor::holding(CursorPayload::Item(CursorItem {
            guid: 0x4000_0000_0000_0001,
            bag: 0,
            slot: 1,
            item_id: 25,
            item_class: None,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
            bar_placeable: false,
            consumable: false,
        })));
}

/// A right press with an item on the cursor is the world input hook's (`0x492b50`), in a look
/// session or not: it clears the cursor (`ClearCursor(1, 1)` at `0x492bb5`) and eats the press,
/// so the world frame's hook (`0x492c20`) never runs and the pending target stands.
#[test]
fn a_right_press_that_drops_the_cursor_keeps_the_pending_target() {
    for freelook in [false, true] {
        let mut app = client("CAMERAORSELECTORMOVE");
        aim(&mut app);
        if freelook {
            app.world()
                .non_send_resource::<UiScript>()
                .run_binding("MouselookStart()")
                .unwrap();
            app.update();
            assert!(
                app.world()
                    .resource::<crate::player::CameraControl>()
                    .is_looking(),
                "the mouselook holds the cursor"
            );
        }
        arm_ground_cast(&mut app);
        hold_an_item(&mut app);
        right_click_at(&mut app, BESIDE_MOB);
        app.update();
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .cursor_payload()
                .is_none(),
            "freelook {freelook}: the hook cleared the cursor"
        );
        assert!(targeting(&app), "freelook {freelook}: the target stands");
    }
}

/// The held item's guid, in our backpack's first slot.
const HELD_ITEM: u64 = 0x4000_0000_0000_0001;

/// The vendor arm's sale from the cursor (`0x5df5d0`): it sends the sale (`0x5df5f0`), then its
/// `ClearCursor(0, 1)` (`0x5df600` → `0x494b60` → `0x495190`) signals `CURSOR_UPDATE` inside the
/// click, so the line after the binding's `TurnOrActionStop` runs after it.
#[test]
fn a_vendor_sale_signals_the_cursor_inside_the_click() {
    use benilla_protocol::field::FIELD_UNIT_NPC_FLAGS;
    /// `PLAYER_FIELD_PACK_SLOT_1`, the backpack's first guid.
    const PACK_SLOT_1: u16 = 532;
    let mut app = client_with_f("PROBEKEYSELL");
    aim(&mut app);
    let world = app.world_mut();
    let (me, mob) = {
        let index = &world.resource::<GuidIndex>().0;
        (index[&ME], index[&MOB])
    };
    world.entity_mut(mob).insert(crate::net::ObjectStore(
        ObjectFields::from_pairs(&[
            (2, 0x9),
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (FIELD_UNIT_NPC_FLAGS, 0x4),
        ])
        .into_created(ObjectType::Unit),
    ));
    world.entity_mut(me).insert(crate::net::ObjectStore(
        ObjectFields::from_pairs(&[
            (2, 0x19),
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (PACK_SLOT_1, HELD_ITEM as u32),
            (PACK_SLOT_1 + 1, (HELD_ITEM >> 32) as u32),
        ])
        .into_created(ObjectType::Player),
    ));
    hold_an_item(&mut app);
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"local f = CreateFrame("Frame")
            f:RegisterEvent("CURSOR_UPDATE")
            f:SetScript("OnEvent", function() LOG = LOG .. "cursor;" end)"#,
        )
        .unwrap();
    move_to(&mut app, ON_MOB);
    app.update();
    app.world()
        .non_send_resource::<UiScript>()
        .run("LOG = ''")
        .unwrap();
    press_f(&mut app);
    app.update();
    assert!(
        app.world()
            .resource::<Sent>()
            .0
            .try_iter()
            .any(|c| matches!(
                c,
                crate::net::ClientCommand::SellItem {
                    vendor: MOB,
                    item_guid: HELD_ITEM,
                    ..
                }
            )),
        "control: the click sold the held item"
    );
    assert_eq!(
        log(&app),
        "cursor;after;",
        "the clear signalled inside the click"
    );
}
