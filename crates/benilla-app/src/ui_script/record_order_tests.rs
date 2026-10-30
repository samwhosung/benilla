//! The frame's input records dispatched whole and in arrival order, over the whole client: each
//! record's interface handlers and the world frame's binding, and the calls they make, run before
//! the next record (`DrainWindowInput 0x423920` → `0x4239a0` → `IEvtQueueDispatch 0x4245b0`), as the
//! window's own message order gives them.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{MouseButtonInput, MouseScrollUnit, MouseWheel};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::{CursorMoved, WindowEvent};

use benilla_ui::script::UiScript;

/// The whole client in the world with a VM that binds `1` to a body logging `key;` and the wheel up
/// to one logging `zoom;`, and a button over the screen's left half logging its `OnClick` as
/// `click;` and its `OnMouseWheel` as `wheel;`. The cursor starts over it. A small frame on the
/// right half logs its `OnEnter` and `OnLeave`.
fn client() -> App {
    client_with(UVec2::new(1024, 768), true)
}

/// [`client`] on a `size` window, `widescreen` as given.
fn client_with(size: UVec2, widescreen: bool) -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(crate::video::Widescreen(widescreen));
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (_inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let mut script = UiScript::new().expect("a VM");
    script.set_default_bindings(benilla_ui::script::keybind::parse_bindings_wtf(
        "bind 1 PROBEKEY\r\nbind SHIFT-1 PROBESHIFT\r\nbind ENTER PROBEBOX\r\n\
         bind W PROBEW\r\nbind BUTTON1 PROBEBUTTON\r\nbind MOUSEWHEELUP PROBEWHEEL\r\n",
    ));
    script.load_binding_set(1);
    script.register_bindings(
        &benilla_ui::bindings_xml::parse(
            r#"<Bindings>
                <Binding name="PROBEKEY">LOG = LOG .. "key;"</Binding>
                <Binding name="PROBESHIFT">LOG = LOG .. "shift;"</Binding>
                <Binding name="PROBEBOX">ProbeBox:Show() ProbeBox:SetFocus()</Binding>
                <Binding name="PROBEW">LOG = LOG .. "w;"</Binding>
                <Binding name="PROBEWHEEL">LOG = LOG .. "zoom;"</Binding>
                <Binding name="PROBEBUTTON" runOnUp="true">LOG = LOG .. "b1 " .. keystate .. ";"</Binding>
            </Bindings>"#,
        )
        .expect("well-formed"),
    );
    script
        .run(
            r#"LOG = ""
            CreateFrame("WorldFrame", "WorldFrame")
            local b = CreateFrame("Button", "RecordOrderButton")
            b:SetPoint("BOTTOMLEFT", 0, 0) b:SetWidth(512) b:SetHeight(768)
            b:EnableMouse(true)
            b:SetScript("OnClick", function() LOG = LOG .. "click;" end)
            b:EnableMouseWheel(true)
            b:SetScript("OnMouseWheel", function() LOG = LOG .. "wheel;" end)
            local f = CreateFrame("Frame", "CrossedFrame")
            f:SetPoint("BOTTOMLEFT", 700, 618) f:SetWidth(100) f:SetHeight(100)
            f:EnableMouse(true)
            f:SetScript("OnEnter", function() LOG = LOG .. "enter;" end)
            f:SetScript("OnLeave", function() LOG = LOG .. "leave;" end)
            local e = CreateFrame("EditBox", "ProbeBox")
            e:SetPoint("TOPLEFT", 600, -10) e:SetWidth(200) e:SetHeight(20)
            e:SetAutoFocus(false) e:Hide()"#,
        )
        .expect("the button builds");
    script.set_screen_size(1024.0, 768.0);
    script.resolve();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    let mut window = Window {
        resolution: size.into(),
        ..default()
    };
    window.set_cursor_position(Some(Vec2::new(100.0, 100.0)));
    let id = app
        .world_mut()
        .spawn((window, bevy::window::PrimaryWindow))
        .id();
    app.world_mut().insert_resource(PrimaryId(id));
    app.update();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"LOG = """#)
        .expect("the log clears");
    app
}

#[derive(Resource)]
struct PrimaryId(Entity);

/// One window message as winit hands it over: its typed message and, in arrival order, the same
/// message on the window-event stream.
fn send(app: &mut App, event: WindowEvent) {
    let world = app.world_mut();
    match event.clone() {
        WindowEvent::KeyboardInput(e) => {
            world.write_message(e);
        }
        WindowEvent::MouseButtonInput(e) => {
            world.write_message(e);
        }
        WindowEvent::CursorMoved(e) => {
            world.write_message(e);
        }
        WindowEvent::MouseWheel(e) => {
            world.write_message(e);
        }
        _ => unreachable!("the fixture sends keys, buttons, moves and wheel turns"),
    }
    world.write_message(event);
}

fn key(app: &mut App, code: KeyCode, state: ButtonState) {
    typed(app, code, state, None);
}

/// A key with the character it types.
fn typed(app: &mut App, code: KeyCode, state: ButtonState, text: Option<&str>) {
    let window = app.world().resource::<PrimaryId>().0;
    send(
        app,
        WindowEvent::KeyboardInput(KeyboardInput {
            key_code: code,
            logical_key: Key::Character(text.unwrap_or("1").into()),
            state,
            text: text.map(Into::into),
            repeat: false,
            window,
        }),
    );
}

/// A cursor move to `at`, the window's cursor left there.
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

/// One wheel notch up.
fn wheel_up(app: &mut App) {
    let window = app.world().resource::<PrimaryId>().0;
    send(
        app,
        WindowEvent::MouseWheel(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window,
        }),
    );
}

fn log(app: &App) -> String {
    app.world()
        .non_send_resource::<UiScript>()
        .eval::<String>("return LOG")
        .expect("the log reads")
}

/// A bound key, then a click on a button, in one frame: the key's binding runs first.
#[test]
fn a_key_then_a_click_in_one_frame_run_the_binding_first() {
    let mut app = client();
    key(&mut app, KeyCode::Digit1, ButtonState::Pressed);
    button(&mut app, ButtonState::Pressed);
    button(&mut app, ButtonState::Released);
    app.update();
    assert_eq!(log(&app), "key;click;");
}

/// A click on a button, then a bound key, in one frame: the click's handler runs first.
#[test]
fn a_click_then_a_key_in_one_frame_run_the_handler_first() {
    let mut app = client();
    button(&mut app, ButtonState::Pressed);
    button(&mut app, ButtonState::Released);
    key(&mut app, KeyCode::Digit1, ButtonState::Pressed);
    app.update();
    assert_eq!(log(&app), "click;key;");
}

/// A press where the cursor was when the button went down, though it moved off the button later
/// in the same frame: the press and its release reach the button (`0x7662c0` targets the frame
/// the last move hovered).
#[test]
fn a_click_lands_where_the_cursor_was_at_the_press() {
    let mut app = client();
    let window = app.world().resource::<PrimaryId>().0;
    button(&mut app, ButtonState::Pressed);
    button(&mut app, ButtonState::Released);
    let off = Vec2::new(900.0, 100.0);
    send(
        &mut app,
        WindowEvent::CursorMoved(CursorMoved {
            window,
            position: off,
            delta: None,
        }),
    );
    app.world_mut()
        .query::<&mut Window>()
        .single_mut(app.world_mut())
        .expect("the window")
        .set_cursor_position(Some(off));
    app.update();
    assert_eq!(log(&app), "click;");
}

/// Shift goes down, `1`, and Shift comes up, all in one frame: the `1` reads the Shift its place
/// left (`0x423d06`), so `SHIFT-1` runs, though the frame ends with Shift up.
#[test]
fn a_key_reads_the_modifiers_of_its_place_in_the_frame() {
    let mut app = client();
    key(&mut app, KeyCode::ShiftLeft, ButtonState::Pressed);
    key(&mut app, KeyCode::Digit1, ButtonState::Pressed);
    key(&mut app, KeyCode::ShiftLeft, ButtonState::Released);
    app.update();
    assert_eq!(log(&app), "shift;");
}

/// Enter's binding focuses a box, then `W` in the same frame: the box holds the keyboard by the
/// time `W` comes, so `W` types into it and its binding does not run.
#[test]
fn a_box_a_key_focused_takes_the_next_key_of_the_frame() {
    let mut app = client();
    key(&mut app, KeyCode::Enter, ButtonState::Pressed);
    typed(&mut app, KeyCode::KeyW, ButtonState::Pressed, Some("w"));
    app.update();
    let s = app.world().non_send_resource::<UiScript>();
    assert_eq!(s.eval::<String>("return ProbeBox:GetText()").unwrap(), "w");
    assert_eq!(log(&app), "", "the box took W");
}

/// A press over the world whose cursor reaches the button before the frame ends: the press is the
/// world frame's, so its binding runs; one over the button whose cursor leaves it is neither's.
#[test]
fn a_press_and_its_binding_agree_when_the_cursor_crosses_a_frame_in_the_frame() {
    let mut app = client();
    move_to(&mut app, Vec2::new(900.0, 100.0));
    app.update();
    button(&mut app, ButtonState::Pressed);
    move_to(&mut app, Vec2::new(100.0, 100.0));
    app.update();
    assert_eq!(log(&app), "b1 down;", "the world frame's binding ran");

    let mut app = client();
    button(&mut app, ButtonState::Pressed);
    move_to(&mut app, Vec2::new(900.0, 100.0));
    app.update();
    assert_eq!(log(&app), "", "the press was the button's");
}

/// With `widescreen` 0 the interface is 4:3 (`0x4901e0` answers 4/3) and drawn stretched across a
/// 16:9 window: a point in root units taken out to the window and clicked there reaches the frame
/// under it, and `GetCursorPosition` (`0x48b820`) answers the same point back.
#[test]
fn widescreen_off_takes_a_click_through_the_stretch_and_back() {
    use super::{ui_to_window, UiScreen};
    let mut app = client_with(UVec2::new(1920, 1080), false);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"local b = CreateFrame("Button", "StretchProbe")
            b:SetPoint("BOTTOMLEFT", 900, 0) b:SetWidth(124) b:SetHeight(768)
            b:EnableMouse(true)
            b:SetScript("OnClick", function()
                local x, y = GetCursorPosition()
                LOG = LOG .. format("%.0f %.0f %.0f;", x, y, GetScreenWidth())
            end)"#,
        )
        .expect("the probe builds");
    app.update();
    let at = ui_to_window(
        UiScreen::of(Vec2::new(1920.0, 1080.0), false),
        Vec2::new(962.0, 384.0),
    );
    // The 1024-wide root spans the 1920 window: 962 units in is 1803.75 px.
    assert!((at - Vec2::new(1803.75, 540.0)).length() < 1e-3, "{at}");
    move_to(&mut app, at);
    button(&mut app, ButtonState::Pressed);
    button(&mut app, ButtonState::Released);
    app.update();
    assert_eq!(log(&app), "962 384 1024;");
}

/// Two moves in one frame, onto a frame and off it again: each move is a record with its own hover
/// walk (`0x7660d0`), so the frame the cursor crossed gets its `OnEnter` and its `OnLeave`.
#[test]
fn a_frame_crossed_within_one_frame_is_entered_and_left() {
    let mut app = client();
    move_to(&mut app, Vec2::new(750.0, 100.0));
    move_to(&mut app, Vec2::new(950.0, 100.0));
    app.update();
    assert_eq!(log(&app), "enter;leave;");
}

/// A wheel turn and a click in one frame run in the window's order: the wheel's record dispatches
/// at its place (kind 0xb, category 0x10, `0x7664f0`), ahead of a click that follows it and behind
/// one that came first.
#[test]
fn a_wheel_turn_runs_at_its_place_among_the_clicks() {
    let mut app = client();
    wheel_up(&mut app);
    button(&mut app, ButtonState::Pressed);
    button(&mut app, ButtonState::Released);
    app.update();
    assert_eq!(log(&app), "wheel;click;");

    let mut app = client();
    button(&mut app, ButtonState::Pressed);
    button(&mut app, ButtonState::Released);
    wheel_up(&mut app);
    app.update();
    assert_eq!(log(&app), "click;wheel;");
}

/// A notch over the world runs the world frame's wheel binding (`0x483d20`) inside its own record,
/// so a key bound after it runs after it.
#[test]
fn a_notch_over_the_world_binds_at_its_place() {
    let mut app = client();
    // The world frame over the right half, where the wheel walk meets it.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"WorldFrame:SetPoint("BOTTOMLEFT", 512, 0)
            WorldFrame:SetWidth(512) WorldFrame:SetHeight(768)"#,
        )
        .expect("the world frame places");
    app.update();
    move_to(&mut app, Vec2::new(900.0, 700.0));
    wheel_up(&mut app);
    key(&mut app, KeyCode::Digit1, ButtonState::Pressed);
    app.update();
    assert_eq!(log(&app), "zoom;key;");
}

/// While the egui dev overlay holds the pointer, a notch over the world runs no binding: the
/// world's wheel is not the overlay's to give.
#[test]
fn a_notch_runs_no_binding_while_the_dev_overlay_holds_the_pointer() {
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"WorldFrame:SetPoint("BOTTOMLEFT", 512, 0)
            WorldFrame:SetWidth(512) WorldFrame:SetHeight(768)"#,
        )
        .expect("the world frame places");
    app.update();
    app.insert_resource(super::EguiPointerOver(true));
    move_to(&mut app, Vec2::new(900.0, 700.0));
    wheel_up(&mut app);
    app.update();
    assert_eq!(log(&app), "");
    app.insert_resource(super::EguiPointerOver(false));
    wheel_up(&mut app);
    app.update();
    assert_eq!(log(&app), "zoom;", "and does once the overlay lets go");
}
