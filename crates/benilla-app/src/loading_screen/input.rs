//! While the loading screen covers the frame, no mouse or keyboard input reaches anything: one
//! cut at the source, in `PreUpdate` after `InputSystems`, so no consumer needs to know the cover.
//! The reference does the same: the raise (`0x406800`, `0x4068e0 call 0x4069e0`) registers the
//! stub `0x406a50` (`xor eax,eax; ret`) at priority 8.0 on event categories 1, 8 (key down), 0xa
//! (repeat), 0xb (button down) and 0xc (mouse move), above every `CSimpleTop` handler at 1.0; the
//! dispatcher `0x4245b0` stops on a zero return (`0x4246ad`), and the dismiss `0x407e80`
//! unregisters them.
//! Category 0xc feeds `UpdateMouseFocus` (`0x7660d0`), so the cursor freezes.
//!
//! - Up edges are swallowed too, which matches by outcome: the reference's held key comes back
//!   released on world enter (`0x4908c0 → 0x49093c → 0x5144c0`).
//! - Deviation: the wheel is swallowed, where the reference still zooms the hidden camera, because
//!   nothing should act through the screen.
//! - Deviation: the world pick is cleared, where the reference's `0x481790` keeps picking at the
//!   frozen point, because ours is quieter and looks the same: the reference's pointer cannot
//!   move, so its pick changes nothing visible.
//!
//! Taken: the button planes ([`swallow`]), the raw message queues, the mouse accumulators, and the
//! window's cursor position, blanked so every hit-test takes its pointer-outside-the-window path
//! ([`CoveredPointer`]). Not taken: synthetic probe input, written in `Update` or ordered after
//! [`CoverInput`]. The cursor art (`0x6e4940`, the plain arrow) lives in [`crate::cursor`]. There
//! is no dev exemption; a stuck load is diagnosed by [`super::WAIT_LOG_AFTER`].

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{
    AccumulatedMouseMotion, AccumulatedMouseScroll, MouseButtonInput, MouseMotion, MouseWheel,
};
use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::window::{CursorLeft, CursorMoved, PrimaryWindow};

use super::LoadingScreen;

/// The set [`swallow_input_under_the_cover`] runs in, between `InputSystems` and
/// `UiSystems::Focus`; the capture probes that write input in `PreUpdate` order after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CoverInput;

/// Where the OS pointer is while the cover has the window's cursor position blanked, re-read every
/// covered frame and handed back when the cover drops, so hover works without a mouse move.
///
/// `Window::set_physical_cursor_position` with `Some` is a hardware warp (`changed_windows` calls
/// winit's `set_cursor_position`), so the hand-back writes only a position still true: none when
/// winit already has a fresher one, none when the pointer left or the window lacks focus, and
/// otherwise the stash, where the pointer still is.
#[derive(Resource, Default)]
pub(crate) struct CoveredPointer {
    /// The last position seen while covered, in physical pixels.
    stashed: Option<DVec2>,
    /// Whether the window's cursor position is currently our blank rather than winit's.
    blanked: bool,
    /// Whether winit reported `CursorLeft` since the stash; the window's `None` cannot tell that
    /// from our blank, and restoring over it would warp the pointer back into the window.
    left: bool,
}

/// One frame's swallow of one button plane: a press arriving under the cover is erased, and a
/// button held from before it is released once with a real edge, so a gesture in flight unwinds.
/// Later covered frames find nothing pressed, so the edge comes once per cover.
fn swallow<T>(input: &mut ButtonInput<T>)
where
    T: Copy + Eq + std::hash::Hash + Send + Sync + 'static,
{
    let arrived: Vec<T> = input.get_just_pressed().copied().collect();
    for button in arrived {
        input.reset(button);
    }
    input.release_all();
}

/// `swallow` for a non-`Copy` button type (`ButtonInput<Key>`).
fn swallow_cloned<T>(input: &mut ButtonInput<T>)
where
    T: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
{
    let arrived: Vec<T> = input.get_just_pressed().cloned().collect();
    for button in arrived {
        input.reset(button);
    }
    input.release_all();
}

/// The raw input-message queues, as one [`bevy::ecs::system::SystemParam`] to stay under Bevy's
/// 16-parameter ceiling.
#[derive(bevy::ecs::system::SystemParam)]
struct RawInput<'w> {
    keyboard: ResMut<'w, Messages<KeyboardInput>>,
    buttons: ResMut<'w, Messages<MouseButtonInput>>,
    motion: ResMut<'w, Messages<MouseMotion>>,
    wheel: ResMut<'w, Messages<MouseWheel>>,
    moved: ResMut<'w, Messages<CursorMoved>>,
}

impl RawInput<'_> {
    fn drain(&mut self) {
        self.keyboard.clear();
        self.buttons.clear();
        self.motion.clear();
        self.wheel.clear();
        self.moved.clear();
    }
}

/// The button planes and the accumulators, as one param.
#[derive(bevy::ecs::system::SystemParam)]
struct Buttons<'w> {
    keys: ResMut<'w, ButtonInput<KeyCode>>,
    logical: ResMut<'w, ButtonInput<Key>>,
    mouse: ResMut<'w, ButtonInput<MouseButton>>,
    motion: ResMut<'w, AccumulatedMouseMotion>,
    scroll: ResMut<'w, AccumulatedMouseScroll>,
}

impl Buttons<'_> {
    fn swallow_all(&mut self) {
        swallow(&mut *self.keys);
        swallow_cloned(&mut *self.logical);
        swallow(&mut *self.mouse);
        *self.motion = AccumulatedMouseMotion::default();
        *self.scroll = AccumulatedMouseScroll::default();
    }
}

/// `PreUpdate`, in [`CoverInput`]: take the whole input plane while the cover is up. The cover
/// rises in `Update`, so its raise frame still takes input; that frame is also the first to draw
/// it, so input stops exactly on the frames after the cover was shown.
fn swallow_input_under_the_cover(
    screen: Res<LoadingScreen>,
    mut pointer: ResMut<CoveredPointer>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut departures: MessageReader<CursorLeft>,
    mut buttons: Buttons,
    mut raw: RawInput,
) {
    // Read before anything below writes it: bevy_winit has already applied this frame's cursor
    // events, so `Some` is the live pointer. Departures drain every frame, so none goes stale.
    let departed = departures.read().count() > 0;
    let here = window
        .single()
        .ok()
        .and_then(|w| w.physical_cursor_position());
    if here.is_some() {
        pointer.left = false;
    } else if departed {
        pointer.left = true;
    }

    if !screen.covering() {
        // Hand the pointer back once, only when the stash is still true (see [`CoveredPointer`]).
        if pointer.blanked {
            pointer.blanked = false;
            let stashed = pointer.stashed.take();
            let left = std::mem::take(&mut pointer.left);
            if let Ok(mut window) = window.single_mut() {
                if here.is_none() && !left && window.focused {
                    window.set_physical_cursor_position(stashed);
                }
            }
        }
        return;
    }

    buttons.swallow_all();
    raw.drain();

    // Re-stash before blanking so the stash tracks the real cursor through the load.
    if let Ok(mut window) = window.single_mut() {
        if let Some(seen) = here {
            pointer.stashed = Some(seen.as_dvec2());
            // Writing `None` does not warp. Only when there is something to blank: a spurious
            // `Changed<Window>` reconfigures the surface.
            window.set_physical_cursor_position(None);
        }
        pointer.blanked = true;
    }
}

/// Wire the gate; called by [`super::LoadingScreenPlugin`] so the cover and its input rule are
/// never registered apart.
pub(super) fn build(app: &mut App) {
    app.init_resource::<CoveredPointer>().add_systems(
        PreUpdate,
        swallow_input_under_the_cover
            .in_set(CoverInput)
            // Before `UiSystems::Focus`, Bevy's own `PreUpdate` reader, which hit-tests the cursor
            // into `Interaction`.
            .after(bevy::input::InputSystems)
            .before(bevy::ui::UiSystems::Focus),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;

    #[test]
    fn a_press_under_the_cover_never_happened_and_a_held_one_is_released_once() {
        let mut keys = ButtonInput::<KeyCode>::default();

        // Held from before the cover: `clear` drops its `just_pressed` edge, as a new frame does.
        keys.press(KeyCode::KeyW);
        keys.clear();
        keys.press(KeyCode::Space);

        swallow(&mut keys);

        assert!(!keys.pressed(KeyCode::Space), "the arrival is erased");
        assert!(!keys.just_pressed(KeyCode::Space), "…edge and all");
        assert!(
            !keys.just_released(KeyCode::Space),
            "a press that never happened cannot release either"
        );
        assert!(!keys.pressed(KeyCode::KeyW), "the held key is let go");
        assert!(
            keys.just_released(KeyCode::KeyW),
            "…with a real release edge, so a held MOVEFORWARD binding unwinds"
        );

        keys.clear();
        swallow(&mut keys);
        assert_eq!(
            keys.get_just_released().count(),
            0,
            "the release is delivered exactly once per cover, not every frame"
        );
        assert_eq!(keys.get_pressed().count(), 0);
    }

    #[test]
    fn an_os_release_under_the_cover_is_silent() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::KeyW);
        keys.clear();
        swallow(&mut keys); // frame 1: W released, with its edge

        keys.clear(); // frame 2's head, as bevy's own input systems do it
        keys.release(KeyCode::KeyW); // the player physically lets go
        swallow(&mut keys);
        assert!(
            !keys.just_released(KeyCode::KeyW),
            "no second release edge for the same key"
        );
    }

    /// Against a real `App` and Bevy's `InputSystems`.
    #[test]
    fn a_covered_frame_hands_no_input_to_anyone_and_gives_the_pointer_back() {
        let mut app = App::new();
        app.add_plugins((
            bevy::input::InputPlugin,
            // For `Messages<CursorMoved>` and the window types; the test spawns its own window.
            bevy::window::WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            },
        ))
        .init_resource::<LoadingScreen>()
        .init_resource::<CoveredPointer>()
        .add_systems(
            PreUpdate,
            swallow_input_under_the_cover
                .in_set(CoverInput)
                .after(bevy::input::InputSystems),
        );
        let window = app
            .world_mut()
            .spawn((
                Window {
                    resolution: bevy::window::WindowResolution::new(800, 600),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();

        let seen = |app: &App| {
            app.world()
                .entity(window)
                .get::<Window>()
                .unwrap()
                .physical_cursor_position()
        };
        let put_cursor = |app: &mut App, at: Option<DVec2>| {
            app.world_mut()
                .entity_mut(window)
                .get_mut::<Window>()
                .unwrap()
                .set_physical_cursor_position(at);
        };

        put_cursor(&mut app, Some(DVec2::new(400.0, 300.0)));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);

        app.update();
        assert!(seen(&app).is_some(), "no cover, no blanking");
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW),
            "no cover, no swallow"
        );

        app.world_mut().resource_mut::<LoadingScreen>().active = true;
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Space,
            logical_key: Key::Space,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        });
        app.update();

        assert!(
            seen(&app).is_none(),
            "the pointer is not in the window while the cover is up — the one fact every \
             hit-test in the client already handles"
        );
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        assert_eq!(keys.get_pressed().count(), 0, "no key survives the cover");
        assert!(
            keys.just_released(KeyCode::KeyW),
            "…and the one held across the raise is released with an edge"
        );
        assert!(
            !keys.pressed(KeyCode::Space) && !keys.just_pressed(KeyCode::Space),
            "the press that arrived under the cover never happened"
        );
        assert!(
            app.world().resource::<Messages<KeyboardInput>>().is_empty(),
            "the raw queue is drained too, so a MessageReader sees nothing either"
        );

        app.world_mut().resource_mut::<LoadingScreen>().active = false;
        app.update();
        assert_eq!(
            seen(&app),
            Some(Vec2::new(400.0, 300.0)),
            "the position is handed back on the frame the cover drops"
        );
    }

    /// Writing the cursor position is a hardware warp, so the hand-back writes only a true one.
    #[test]
    fn the_hand_back_never_writes_a_position_it_does_not_know_to_be_true() {
        /// Cover, blank, then drop the cover under `arrange`; returns what the cover wrote back.
        fn round_trip(arrange: impl FnOnce(&mut App, Entity)) -> Option<Vec2> {
            let mut app = App::new();
            app.add_plugins((
                bevy::input::InputPlugin,
                bevy::window::WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                },
            ))
            .init_resource::<LoadingScreen>()
            .init_resource::<CoveredPointer>()
            .add_systems(
                PreUpdate,
                swallow_input_under_the_cover
                    .in_set(CoverInput)
                    .after(bevy::input::InputSystems),
            );
            let window = app
                .world_mut()
                .spawn((
                    Window {
                        resolution: bevy::window::WindowResolution::new(800, 600),
                        ..default()
                    },
                    PrimaryWindow,
                ))
                .id();
            let put = |app: &mut App, at: Option<DVec2>| {
                app.world_mut()
                    .entity_mut(window)
                    .get_mut::<Window>()
                    .unwrap()
                    .set_physical_cursor_position(at);
            };

            put(&mut app, Some(DVec2::new(400.0, 300.0)));
            app.world_mut().resource_mut::<LoadingScreen>().active = true;
            app.update();
            assert!(
                app.world()
                    .entity(window)
                    .get::<Window>()
                    .unwrap()
                    .physical_cursor_position()
                    .is_none(),
                "the cover blanked the position"
            );

            app.world_mut().resource_mut::<LoadingScreen>().active = false;
            arrange(&mut app, window);
            app.update();
            app.world()
                .entity(window)
                .get::<Window>()
                .unwrap()
                .physical_cursor_position()
        }

        assert_eq!(
            round_trip(|_, _| {}),
            Some(Vec2::new(400.0, 300.0)),
            "a pointer that never moved gets its stash back — writing it warps nothing, because \
             that is where the pointer already is"
        );

        // A `Some` on the drop frame is winit's live pointer, the case a moving mouse takes.
        assert_eq!(
            round_trip(|app, window| {
                app.world_mut()
                    .entity_mut(window)
                    .get_mut::<Window>()
                    .unwrap()
                    .set_physical_cursor_position(Some(DVec2::new(120.0, 90.0)));
            }),
            Some(Vec2::new(120.0, 90.0)),
            "winit's fresher answer is left alone, never clobbered with the stash"
        );

        assert_eq!(
            round_trip(|app, window| {
                app.world_mut().write_message(CursorLeft { window });
            }),
            None,
            "a pointer the player took out of the window is not dragged back into it"
        );

        assert_eq!(
            round_trip(|app, window| {
                app.world_mut()
                    .entity_mut(window)
                    .get_mut::<Window>()
                    .unwrap()
                    .focused = false;
            }),
            None,
            "a window without the keyboard does not move the system pointer at all"
        );
    }
}
