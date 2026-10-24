//! One frame of the whole client in its schedule's order: the window input, then the packets'
//! events, then the `OnUpdate` walk, as the reference's tick runs them (`0x423920` at `0x420d23`,
//! category 6 at `0x420d55`, then the frame pass `0x442350` at `0x420d6a`, whose top runs the
//! walk, `0x764330` → `0x765650` → `0x76b2c0`).

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::{extract, input};
use crate::test_support::runs_before;

/// A step of the frame the fixture runs, each the client's own system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The UI's input pass.
    Input,
    /// The lanes' drain, which fires the packets' events in arrival order.
    Packets,
    /// The tick, which runs the `OnUpdate` walk.
    Update,
}

/// The steps in the order `Update` declares them; panics on a pair it leaves unordered.
fn in_frame_order(app: &mut App) -> Vec<Step> {
    use Step::*;
    let mut table = Vec::new();
    for (a, b, ab, ba) in [
        (
            Input,
            Packets,
            runs_before(app, input::feed_ui_input, crate::lanes::fire_in_order),
            runs_before(app, crate::lanes::fire_in_order, input::feed_ui_input),
        ),
        (
            Packets,
            Update,
            runs_before(app, crate::lanes::fire_in_order, extract::tick_script),
            runs_before(app, extract::tick_script, crate::lanes::fire_in_order),
        ),
        (
            Input,
            Update,
            runs_before(app, input::feed_ui_input, extract::tick_script),
            runs_before(app, extract::tick_script, input::feed_ui_input),
        ),
    ] {
        assert!(
            ab != ba,
            "{a:?} and {b:?} run in an order the executor picks"
        );
        table.push(if ab { (a, b) } else { (b, a) });
    }
    let mut steps = vec![Input, Packets, Update];
    steps.sort_by(|x, y| {
        if x == y {
            std::cmp::Ordering::Equal
        } else if table.contains(&(*x, *y)) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    steps
}

/// One frame over the whole client, with a full-screen button that logs its `OnMouseDown`, its
/// `CHAT_MSG_SYSTEM` and its `OnUpdate` (with whether `IsShiftKeyDown()` answers), while Shift is
/// held, the left button goes down over it and a system message has arrived. Returns the log.
fn one_frame() -> String {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let order = in_frame_order(&mut app);
    let world = app.world_mut();
    let mut script = UiScript::new().expect("a VM");
    script.set_screen_size(1024.0, 768.0);
    script
        .run(
            r#"LOG = ""
            local b = CreateFrame("Button", "TickOrderButton")
            b:SetPoint("BOTTOMLEFT", 0, 0) b:SetWidth(1024) b:SetHeight(768)
            b:EnableMouse(true)
            b:RegisterEvent("CHAT_MSG_SYSTEM")
            b:SetScript("OnMouseDown", function() LOG = LOG .. "down;" end)
            b:SetScript("OnEvent", function() LOG = LOG .. "event;" end)
            b:SetScript("OnUpdate", function()
                LOG = LOG .. (IsShiftKeyDown() and "update+shift;" or "update;")
            end)"#,
        )
        .expect("the button builds");
    script.resolve();
    world.insert_non_send_resource(script);
    let mut window = Window {
        resolution: UVec2::new(1024, 768).into(),
        ..default()
    };
    window.set_cursor_position(Some(Vec2::new(100.0, 100.0)));
    world.spawn((window, bevy::window::PrimaryWindow));
    world
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);
    world
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    world.resource_mut::<crate::ui_chat::ChatLog>().push_event(
        crate::ui_chat::ChatEvent::text_only(
            crate::ui_chat::ChatEventKind::System,
            "Welcome".into(),
        ),
    );
    for step in order {
        match step {
            Step::Input => world
                .run_system_once(input::feed_ui_input)
                .expect("the input pass runs"),
            Step::Packets => crate::lanes::fire_in_order(world),
            Step::Update => world
                .run_system_once(extract::tick_script)
                .expect("the tick runs"),
        }
    }
    world
        .non_send_resource::<UiScript>()
        .eval::<String>("return LOG")
        .expect("the log reads")
}

/// An `OnUpdate` reads the modifiers of its own frame: the window input reaches the interface
/// before the frame pass runs the walk.
#[test]
fn an_on_update_reads_this_frames_shift() {
    let log = one_frame();
    assert!(log.contains("update+shift;"), "{log}");
}

/// A press's `OnMouseDown` runs before the same frame's `OnUpdate`.
#[test]
fn a_mouse_down_runs_before_the_frames_on_update() {
    let log = one_frame();
    let (down, update) = (log.find("down;"), log.find("update"));
    assert!(down.is_some() && down < update, "{log}");
}

/// A packet's event of this frame fires after the frame's input handlers and before its
/// `OnUpdate`.
#[test]
fn a_packet_event_fires_between_the_input_and_the_on_update() {
    let log = one_frame();
    assert_eq!(log, "down;event;update+shift;");
}
