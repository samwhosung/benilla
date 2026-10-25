//! The frame's window input records, in the order the window delivered them: the reference's ring
//! (`0x42d780`), which the tick drains one record at a time, each whole through its category's
//! handlers before the next (`DrainWindowInput 0x423920` → `0x4239a0` → `IEvtQueueDispatch
//! 0x4245b0`). Bevy's typed input messages each keep only their own order; winit's window-event
//! stream ([`WindowEvent`]) keeps them interleaved, so the records are read from it.
//!
//! Each record carries the modifier mask as its kind handler leaves it (`0x423d06`, `0x423d56`,
//! before the dispatch), and the cursor where it was when the record was queued: a button record
//! carries it in the reference (`0x42dd00` reads `GetCursorPos`); a key record does not
//! (`{key, mods, repeat, time}`), and its position stands in for the move records queued before
//! it, which here are not records of their own.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowEvent};

use crate::textinput::keymap::Mods;

/// What one record is: a key down or up, or a button down or up. Moves are not records of their
/// own: the button records carry the position they were queued at, and the frame's last position
/// is the window's.
#[derive(Clone, Debug)]
pub(crate) enum Record {
    Key(KeyboardInput),
    Button {
        button: MouseButton,
        state: ButtonState,
    },
}

/// One record with the state it was queued in.
#[derive(Clone, Debug)]
pub(crate) struct InputRecord {
    pub(crate) record: Record,
    /// The modifiers held once this record's own key, if it is one, has moved them.
    pub(crate) mods: Mods,
    /// The cursor in window coordinates when the record was queued; `None` outside the window.
    pub(crate) at: Option<Vec2>,
}

/// The frame's records in arrival order, rewritten each frame by [`collect_input_records`].
#[derive(Resource, Default)]
pub(crate) struct InputRecords {
    records: Vec<InputRecord>,
    /// Where the last frame left the cursor, this frame's position before its first move: read in
    /// `Last`, after a look session's end has put the cursor back. `None` before the first frame.
    cursor: Option<Option<Vec2>>,
    /// The modifier keys held when the last frame's records were read.
    modifiers: Vec<KeyCode>,
}

impl InputRecords {
    pub(crate) fn iter(&self) -> impl Iterator<Item = &InputRecord> {
        self.records.iter()
    }
}

/// The eight keys the modifier mask is made of.
const MODIFIER_KEYS: [KeyCode; 8] = [
    KeyCode::ShiftLeft,
    KeyCode::ShiftRight,
    KeyCode::ControlLeft,
    KeyCode::ControlRight,
    KeyCode::AltLeft,
    KeyCode::AltRight,
    KeyCode::SuperLeft,
    KeyCode::SuperRight,
];

fn mods_of(held: &[KeyCode]) -> Mods {
    let down = |a, b| held.contains(&a) || held.contains(&b);
    Mods {
        shift: down(KeyCode::ShiftLeft, KeyCode::ShiftRight),
        ctrl: down(KeyCode::ControlLeft, KeyCode::ControlRight),
        alt: down(KeyCode::AltLeft, KeyCode::AltRight),
        sup: down(KeyCode::SuperLeft, KeyCode::SuperRight),
    }
}

/// Read the frame's window events into [`InputRecords`], at the head of the input records. A
/// modifier key with a record this frame starts from where the last frame left it; one with none
/// is as bevy's plane holds it, which a window deactivate clears without a record. The cursor
/// before the frame's first move is where the last frame left it, and with no move this frame it
/// is the window's.
pub(crate) fn collect_input_records(
    mut events: MessageReader<WindowEvent>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut records: ResMut<InputRecords>,
) {
    let events: Vec<&WindowEvent> = events.read().collect();
    let now = window.single().ok().and_then(Window::cursor_position);
    let moved = events
        .iter()
        .any(|e| matches!(e, WindowEvent::CursorMoved(_) | WindowEvent::CursorLeft(_)));
    let mut at = if moved {
        records.cursor.unwrap_or(now)
    } else {
        now
    };
    let recorded = |k: KeyCode| {
        events
            .iter()
            .any(|e| matches!(e, WindowEvent::KeyboardInput(ev) if ev.key_code == k))
    };
    let mut held: Vec<KeyCode> = MODIFIER_KEYS
        .into_iter()
        .filter(|&k| {
            if recorded(k) {
                records.modifiers.contains(&k)
            } else {
                keys.pressed(k)
            }
        })
        .collect();
    let mut out = Vec::new();
    for event in events {
        match event {
            WindowEvent::CursorMoved(m) => at = Some(m.position),
            WindowEvent::CursorLeft(_) => at = None,
            WindowEvent::KeyboardInput(ev) => {
                if MODIFIER_KEYS.contains(&ev.key_code) {
                    held.retain(|&k| k != ev.key_code);
                    if ev.state == ButtonState::Pressed {
                        held.push(ev.key_code);
                    }
                }
                out.push(InputRecord {
                    record: Record::Key(ev.clone()),
                    mods: mods_of(&held),
                    at,
                });
            }
            WindowEvent::MouseButtonInput(ev) => out.push(InputRecord {
                record: Record::Button {
                    button: ev.button,
                    state: ev.state,
                },
                mods: mods_of(&held),
                at,
            }),
            _ => {}
        }
    }
    records.records = out;
    records.modifiers = MODIFIER_KEYS
        .into_iter()
        .filter(|&k| keys.pressed(k))
        .collect();
}

/// Where the frame leaves the cursor, read after every writer of it: a look session's end puts the
/// pointer back where the look began (`player::camera`), after this frame's records were read.
pub(crate) fn note_frame_end_cursor(
    window: Query<&Window, With<PrimaryWindow>>,
    mut records: ResMut<InputRecords>,
) {
    records.cursor = Some(window.single().ok().and_then(Window::cursor_position));
}

/// For a test that drives the client through bevy's planes and typed messages rather than the
/// window's stream: the frame's records as the window would have queued them, the unread key
/// messages first and then the button plane's edges, each at the window's cursor.
#[cfg(test)]
pub(crate) fn records_from_planes(world: &mut World) {
    let keyboard: Vec<KeyboardInput> = world
        .resource::<Messages<KeyboardInput>>()
        .iter_current_update_messages()
        .cloned()
        .collect();
    let buttons = world.get_resource::<ButtonInput<MouseButton>>();
    let mut edges = Vec::new();
    if let Some(buttons) = buttons {
        for b in buttons.get_just_pressed() {
            edges.push((*b, ButtonState::Pressed));
        }
        for b in buttons.get_just_released() {
            edges.push((*b, ButtonState::Released));
        }
    }
    let at = world
        .query_filtered::<&Window, With<PrimaryWindow>>()
        .single(world)
        .ok()
        .and_then(Window::cursor_position);
    let keys = world.resource::<ButtonInput<KeyCode>>();
    let mods = mods_of(
        &MODIFIER_KEYS
            .into_iter()
            .filter(|&k| keys.pressed(k))
            .collect::<Vec<_>>(),
    );
    let mut records = keyboard
        .into_iter()
        .map(|ev| InputRecord {
            record: Record::Key(ev),
            mods,
            at,
        })
        .collect::<Vec<_>>();
    records.extend(edges.into_iter().map(|(button, state)| InputRecord {
        record: Record::Button { button, state },
        mods,
        at,
    }));
    world.resource_mut::<InputRecords>().records = records;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::SystemId;
    use bevy::input::keyboard::Key;
    use bevy::input::mouse::MouseButtonInput;
    use bevy::window::CursorMoved;

    /// The world and the collector as one system kept across frames, so its reader moves on.
    fn world() -> (World, SystemId) {
        let mut world = World::new();
        world.init_resource::<Messages<WindowEvent>>();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.init_resource::<InputRecords>();
        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(10.0, 10.0)));
        world.spawn((window, PrimaryWindow));
        let collect = world.register_system(collect_input_records);
        (world, collect)
    }

    fn key(code: KeyCode, state: ButtonState) -> WindowEvent {
        WindowEvent::KeyboardInput(KeyboardInput {
            key_code: code,
            logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        })
    }

    fn button(state: ButtonState) -> WindowEvent {
        WindowEvent::MouseButtonInput(MouseButtonInput {
            button: MouseButton::Left,
            state,
            window: Entity::PLACEHOLDER,
        })
    }

    fn moved(x: f32) -> WindowEvent {
        WindowEvent::CursorMoved(CursorMoved {
            window: Entity::PLACEHOLDER,
            position: Vec2::new(x, 10.0),
            delta: None,
        })
    }

    fn collect(
        (world, system): &mut (World, SystemId),
        events: Vec<WindowEvent>,
    ) -> Vec<InputRecord> {
        for e in events {
            world.write_message(e);
        }
        world.run_system(*system).expect("the records collect");
        world
            .run_system_cached(note_frame_end_cursor)
            .expect("the frame's end is noted");
        world.resource::<InputRecords>().iter().cloned().collect()
    }

    /// The records keep the window's interleaving of keys and buttons, each button at the cursor
    /// of its own moment: the frame's start before its first move, then each move's.
    #[test]
    fn the_records_keep_the_windows_order_and_each_ones_cursor() {
        let mut w = world();
        let got = collect(
            &mut w,
            vec![
                button(ButtonState::Pressed),
                key(KeyCode::Digit1, ButtonState::Pressed),
                moved(300.0),
                button(ButtonState::Released),
            ],
        );
        let shape: Vec<String> = got
            .iter()
            .map(|r| match &r.record {
                Record::Key(k) => format!("{:?}@{:?}", k.key_code, r.at.map(|a| a.x)),
                Record::Button { state, .. } => format!("{state:?}@{:?}", r.at.map(|a| a.x)),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "Pressed@Some(10.0)",
                "Digit1@Some(10.0)",
                "Released@Some(300.0)"
            ],
            "the first frame read starts at the window's cursor"
        );
        // The next frame starts where the window's cursor stood at the last read.
        let got = collect(&mut w, vec![moved(500.0), button(ButtonState::Pressed)]);
        assert_eq!(got[0].at, Some(Vec2::new(500.0, 10.0)));
        let got = collect(&mut w, vec![button(ButtonState::Pressed), moved(600.0)]);
        assert_eq!(
            got[0].at,
            Some(Vec2::new(10.0, 10.0)),
            "the window's, read last frame"
        );
        // A frame with no move puts every record at the window's cursor.
        let got = collect(&mut w, vec![button(ButtonState::Released)]);
        assert_eq!(got[0].at, Some(Vec2::new(10.0, 10.0)));
    }

    /// Each record reads the modifiers its own place in the frame left: a Shift that goes down
    /// after a key is not held for it, and is for the key after.
    #[test]
    fn each_record_reads_the_modifiers_its_place_left() {
        let mut w = world();
        w.0.resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
        let got = collect(
            &mut w,
            vec![
                key(KeyCode::Digit1, ButtonState::Pressed),
                key(KeyCode::ShiftLeft, ButtonState::Pressed),
                key(KeyCode::Digit2, ButtonState::Pressed),
            ],
        );
        let shift: Vec<bool> = got.iter().map(|r| r.mods.shift).collect();
        assert_eq!(shift, [false, true, true]);
        // A Shift bevy holds with no record this frame (a direct write) is held for every record.
        w.0.resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ControlLeft);
        let got = collect(&mut w, vec![key(KeyCode::Digit1, ButtonState::Pressed)]);
        assert!(got[0].mods.ctrl && got[0].mods.shift);
    }
}
