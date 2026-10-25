//! One frame of the whole client in its schedule's order: the window input, then the packets
//! applied and their events, then the `OnUpdate` walk, as the reference's tick runs them
//! (`0x423920` at `0x420d23`, category 6 at `0x420d55`, then the frame pass `0x442350` at
//! `0x420d6a`, whose top runs the walk, `0x764330` → `0x765650` → `0x76b2c0`).

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::{extract, input};
use crate::net::SelfGuid;
use crate::test_support::runs_before;

/// Our player's guid in the fixture.
const ME: u64 = 0x7;

/// A step of the frame the fixture runs, each the client's own system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The UI's input pass.
    Input,
    /// The net drain, which applies the frame's packets to the world.
    Net,
    /// The lanes' drain, which fires the packets' events in arrival order.
    Packets,
    /// The tick, which runs the `OnUpdate` walk.
    Update,
}

const STEPS: [Step; 4] = [Step::Input, Step::Net, Step::Packets, Step::Update];

/// Binds `$s` to the client's system for `$step` and evaluates `$body` with it.
macro_rules! with_system {
    ($step:expr, |$s:ident| $body:expr) => {
        match $step {
            Step::Input => {
                let $s = input::feed_ui_input;
                $body
            }
            Step::Net => {
                let $s = crate::net::apply_net_updates;
                $body
            }
            Step::Packets => {
                let $s = crate::lanes::fire_in_order;
                $body
            }
            Step::Update => {
                let $s = extract::tick_script;
                $body
            }
        }
    };
}

/// Whether `Update` declares `a` before `b`.
fn declared_before(app: &mut App, a: Step, b: Step) -> bool {
    with_system!(a, |x| with_system!(b, |y| runs_before(app, x, y)))
}

/// The steps in the order `Update` declares them; panics on a pair it leaves unordered.
fn in_frame_order(app: &mut App) -> Vec<Step> {
    let mut table = Vec::new();
    for (i, a) in STEPS.iter().enumerate() {
        for b in &STEPS[i + 1..] {
            let (ab, ba) = (declared_before(app, *a, *b), declared_before(app, *b, *a));
            assert!(
                ab != ba,
                "{a:?} and {b:?} run in an order the executor picks"
            );
            table.push(if ab { (*a, *b) } else { (*b, *a) });
        }
    }
    let mut steps = STEPS.to_vec();
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

/// Runs one frame's steps in `order`.
fn run_frame(world: &mut World, order: &[Step]) {
    for step in order {
        match step {
            Step::Input => super::feed_input_once(world),
            Step::Net => crate::net::apply_net_updates(world),
            Step::Packets => crate::lanes::fire_in_order(world),
            Step::Update => world
                .run_system_once(extract::tick_script)
                .expect("the tick runs"),
        }
    }
}

/// The whole client in the world, our player streamed at full health, and a full-screen button
/// `b` that `chunk` scripts to log into `LOG`, with the cursor over it.
struct Client {
    app: App,
    /// The order the schedule runs the steps in.
    order: Vec<Step>,
    inbound: crossbeam_channel::Sender<SessionEvent>,
    _sent: crossbeam_channel::Receiver<crate::net::ClientCommand>,
}

fn client(chunk: &str) -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let order = in_frame_order(&mut app);
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run(&format!(
            r#"LOG = ""
            local b = CreateFrame("Button", "TickOrderButton")
            b:SetPoint("BOTTOMLEFT", 0, 0) b:SetWidth(1024) b:SetHeight(768)
            b:EnableMouse(true)
            {chunk}"#
        ))
        .expect("the button builds");
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
    for ev in crate::net::wire([
        SessionEvent::ObjectCreate {
            guid: ME,
            kind: EntityKind::Player,
            display_id: None,
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields: ObjectFields::from_pairs(&[
                (FIELD_UNIT_HEALTH, 100),
                (FIELD_UNIT_MAXHEALTH, 100),
            ])
            .into_created(ObjectType::Player),
        },
        SessionEvent::ObjectUpdateEnd,
    ]) {
        inbound.send(ev).unwrap();
    }
    app.update();
    let world = app.world_mut();
    {
        let mut script = world.non_send_resource_mut::<UiScript>();
        script.set_screen_size(1024.0, 768.0);
        script.resolve();
        script.run(r#"LOG = """#).expect("the log clears");
    }
    let mut window = Window {
        resolution: UVec2::new(1024, 768).into(),
        ..default()
    };
    window.set_cursor_position(Some(Vec2::new(100.0, 100.0)));
    world.spawn((window, bevy::window::PrimaryWindow));
    Client {
        app,
        order,
        inbound,
        _sent: sent,
    }
}

/// The fixture's log.
fn log(world: &World) -> String {
    world
        .non_send_resource::<UiScript>()
        .eval::<String>("return LOG")
        .expect("the log reads")
}

/// One frame over the whole client, with a full-screen button that logs its `OnMouseDown`, its
/// `CHAT_MSG_SYSTEM` and its `OnUpdate` (with whether `IsShiftKeyDown()` answers), while Shift is
/// held, the left button goes down over it and a system message has arrived. Returns the log.
fn one_frame() -> String {
    let mut c = client(
        r#"b:RegisterEvent("CHAT_MSG_SYSTEM")
        b:SetScript("OnMouseDown", function() LOG = LOG .. "down;" end)
        b:SetScript("OnEvent", function() LOG = LOG .. "event;" end)
        b:SetScript("OnUpdate", function()
            LOG = LOG .. (IsShiftKeyDown() and "update+shift;" or "update;")
        end)"#,
    );
    let world = c.app.world_mut();
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
    run_frame(world, &c.order);
    log(world)
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

/// The frame's input runs ahead of its packets: the input pass, then the net drain that applies
/// them, then their events, then the `OnUpdate` walk.
#[test]
fn the_input_runs_before_the_packets_apply() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    assert_eq!(
        in_frame_order(&mut app),
        [Step::Input, Step::Net, Step::Packets, Step::Update]
    );
}

/// An `OnClick` reads a unit as the last frame left it, though this frame's packet changes it:
/// the click's handler runs in the window input, ahead of the packets, and their event and the
/// `OnUpdate` after them read the new value.
#[test]
fn an_on_click_reads_the_unit_before_this_frames_packet() {
    let mut c = client(
        r#"b:RegisterEvent("UNIT_HEALTH")
        b:SetScript("OnClick", function() LOG = LOG .. "click " .. UnitHealth("player") .. ";" end)
        b:SetScript("OnEvent", function()
            if arg1 == "player" then LOG = LOG .. "event " .. UnitHealth("player") .. ";" end
        end)
        b:SetScript("OnUpdate", function()
            if LOG ~= "" then LOG = LOG .. "update " .. UnitHealth("player") .. ";" end
        end)"#,
    );
    let world = c.app.world_mut();
    // The press, a frame of its own.
    world
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    run_frame(world, &c.order);
    assert_eq!(log(world), "", "the press clicks nothing");
    // The release, in the frame whose packet takes our health to 60.
    let mut buttons = world.resource_mut::<ButtonInput<MouseButton>>();
    buttons.clear();
    buttons.release(MouseButton::Left);
    for ev in crate::net::wire([SessionEvent::ObjectValues {
        guid: ME,
        fields: ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 60)]),
    }]) {
        c.inbound.send(ev).unwrap();
    }
    run_frame(world, &c.order);
    assert_eq!(log(world), "click 100;event 60;update 60;");
}

/// A click's handler reads the targeting cursor as the frame began, though an engine writer after
/// the last frame's feed raised it (a cast drain, a walk's bind): the frame's input runs against
/// the cursor's own word, as `SpellIsTargeting` (`0x6e6cd0`) reads it, not the VM's view of a frame
/// before. The frames run `Update` whole.
#[test]
fn a_click_reads_the_cursor_an_engine_writer_raised_after_the_last_feed() {
    let mut c = client(
        r#"b:SetScript("OnClick", function()
            LOG = LOG .. "click " .. tostring(SpellIsTargeting() and true or false) .. ";"
        end)"#,
    );
    let world = c.app.world_mut();
    world
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    left_record(world, bevy::input::ButtonState::Pressed);
    world.run_schedule(Update);
    assert_eq!(log(world), "", "the press clicks nothing");
    // After the frame's feed: the cursor goes up outside any script call.
    world.resource_mut::<crate::spell::SpellTargeting>().enter(
        133,
        crate::spell::CastCommit::Spell,
        0x0002,
    );
    let mut buttons = world.resource_mut::<ButtonInput<MouseButton>>();
    buttons.clear();
    buttons.release(MouseButton::Left);
    left_record(world, bevy::input::ButtonState::Released);
    world.run_schedule(Update);
    assert_eq!(log(world), "click true;");
}

/// The left button's window record, which the input records read; the plane is the test's own.
fn left_record(world: &mut World, state: bevy::input::ButtonState) {
    world.write_message(bevy::window::WindowEvent::MouseButtonInput(
        bevy::input::mouse::MouseButtonInput {
            button: MouseButton::Left,
            state,
            window: Entity::PLACEHOLDER,
        },
    ));
}

/// The cursor moves onto a button and the right button goes down in the same frame: the press is
/// the interface's alone, and the world's mouse takes none of it, as the reference hands a
/// mouse-down to the one frame under the cursor (`0x7662c0`, capture at `0x7663e9`). The pointer
/// arbiter publishes the frame's hover before the world's mouse reads it.
#[test]
fn a_right_press_as_the_cursor_reaches_a_button_is_the_interfaces_alone() {
    let mut c = client("");
    assert!(runs_before(
        &mut c.app,
        input::feed_ui_input,
        super::arbitrate_pointer_over_ui
    ));
    assert!(runs_before(
        &mut c.app,
        super::arbitrate_pointer_over_ui,
        crate::player::camera::latch_world_mouse
    ));
    let world = c.app.world_mut();
    assert!(
        !world.resource::<super::PointerOverUi>().0,
        "the last frame's cursor was over nothing"
    );
    world
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Right);
    super::feed_input_once(world);
    world
        .run_system_once(super::arbitrate_pointer_over_ui)
        .expect("the arbiter runs");
    world
        .run_system_once(crate::player::camera::latch_world_mouse)
        .expect("the world's mouse latches");
    assert!(
        !world
            .resource::<crate::player::CameraControl>()
            .world_mouse
            .holds_right(),
        "the world took the button's press"
    );
}
