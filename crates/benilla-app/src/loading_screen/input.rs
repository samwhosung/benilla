//! **The cover takes the input plane.** While the loading screen covers the frame, benilla accepts
//! no mouse and no keyboard: no hover, no cursor classification, no click, no keybinding, no
//! camera, no movement. One system, at the source, so *nothing under the cover has to know the
//! cover exists*.
//!
//! ## The reference does this too, and it does it at the source
//!
//! This file was written on the assumption that the reference simply blocks inside its world load,
//! so the question never arises there. The disassembly behind decision 1990 says otherwise, and
//! the answer is better than the assumption: **the reference has an explicit input veto, registered
//! by the screen's own raise.**
//!
//! `0x406800` (the raise) ends `0x4068e0 call 0x4069e0`, which registers the three-byte stub
//! `0x406a50` — `xor eax,eax; ret` — on event-bus categories **1, 8 (key down), 0xa (auto-repeat),
//! 0xb (button down) and 0xc (mouse move)** at priority **8.0f**; `0x407e80` (the dismiss)
//! unregisters all five. The dispatcher `0x4245b0` stops its walk on a zero return (`0x4246ad`),
//! and every `CSimpleTop` input handler sits at 1.0f — so those five categories never reach the
//! interface at all while the screen is up. Category 0xc's registrant is `UpdateMouseFocus`
//! (`0x7660d0`) itself, so the veto stops the pointer resolver outright: **the cursor freezes.**
//! Key-up, button-up, the wheel, category 0x10 and `WM_ACTIVATE` are deliberately let through.
//!
//! So the shape below is the reference's shape — one veto at the source, not a condition on each
//! consumer — arrived at independently and then confirmed. Two deliberate differences, named
//! rather than drifted into:
//!
//! - **We swallow the wheel and the up edges too.** The up edges are equivalent by outcome: the
//!   reference's held key comes back **released** anyway (`0x4908c0 → 0x49093c → 0x5144c0` zeroes
//!   the movement mask on world-enter, and a still-held key cannot re-arm it — it reclassifies to
//!   the vetoed auto-repeat category), which is exactly what the release edge below produces. The
//!   wheel is a real deviation: in the reference a notch under the loading screen still zooms the
//!   camera you cannot see. Swallowing it is the director's "nothing through the screen", and it
//!   costs a zoom nobody asked for.
//! - **We clear the world pick; the reference leaves it running at the frozen point.** `0x481790`
//!   hangs off the layer walk rather than the bus, so it keeps picking — and in the two windows
//!   where a world is populated (before `SMSG_NEW_WORLD`, and again once the new player object
//!   exists) it *can* set a context cursor under the screen. It is invisible there only because the
//!   pointer cannot move. Ours is strictly quieter and observably identical.
//!
//! ## Why this is a source cut and not a run condition
//!
//! The obvious way to build it is a run condition per input system. That is the way it regresses:
//! the census behind this module counted **fifty-one** places raw input enters the app, more than
//! half of them with no state gate of any kind — the whole `target::TargetUpdate` pick chain
//! included, which is the one the director caught. Every one of them would have had to remember,
//! and every future one would have to remember too. The director's report was exactly a thing
//! nobody remembered: the world pick ran under the cover, found a unit, and the *hardware cursor
//! changed to the sword* over art the player was only waiting on.
//!
//! So the cut is at the source, in `PreUpdate`, after `bevy::input::InputSystems` has built this
//! frame's input and before anything reads it. A consumer written a year from now is covered
//! without knowing this file exists — which is the only property that survives a multi-year
//! codebase.
//!
//! ## The four things it takes, and what makes the pointer half work
//!
//! 1. **The button planes** — `ButtonInput<KeyCode>`/`<Key>`/`<MouseButton>`. See [`swallow`] for
//!    the exact rule: a press that arrives under the cover never happened, and anything held from
//!    *before* the cover is released once, with a real release edge, so a gesture already in
//!    flight (a mouselook session, a held `MOVEFORWARD` binding, an armed UI drag) unwinds through
//!    the path it already has for a released button instead of being stranded down forever.
//! 2. **The raw message queues** — `KeyboardInput`, `MouseButtonInput`, `MouseMotion`,
//!    `MouseWheel`, `CursorMoved` — drained, because a `MessageReader` reads the queue, not the
//!    button state (`feed_ui_input`'s character/EditBox feed and the glue screens all read raw).
//! 3. **The accumulators** — `AccumulatedMouseMotion`/`AccumulatedMouseScroll`, which
//!    `InputSystems` builds from those same messages and which the camera and the wheel bindings
//!    poll rather than read.
//! 4. **The pointer position itself**, which is the half a message drain cannot reach: a hit-test
//!    does not read an event, it reads `Window::cursor_position()`, a *field*. So the cover blanks
//!    it and restores it on the way out ([`CoveredPointer`]).
//!
//! That fourth one is the load-bearing trick, and it is not a hack: **"the pointer is not in the
//! window" is a state every pointer consumer in the client already implements correctly**, because
//! it is the ordinary alt-tabbed-away case. `update_hover`/`update_hovered_object` clear
//! [`crate::target::Hovered`] and return; `update_pick_occlusion` leaves the ray at infinity;
//! `classify_cursor` then reads an empty pick and settles on Point; `feed_ui_input` takes its
//! `pointer_left_window()` arm, which leaves the hovered frame (one `OnLeave`) and disarms any
//! press/drag; bevy's own `ui_focus_system` clears every `Interaction`, so `glue_clicks` goes
//! quiet with it. Not one of those had to be told about loading screens.
//!
//! ## What it deliberately does not take
//!
//! **Synthetic input.** The capture probes press into the same `ButtonInput` resources — but they
//! do it in `Update`, after this system, and the pointer probes drive
//! `ui_script::SyntheticPointer` rather than the window. An instrument driving the client is the
//! operator, not the player. The two probe writers that *do* run in `PreUpdate` order themselves
//! [`after`](CoverInput) this set so the relationship is written down rather than left ambiguous.
//!
//! **The cursor's own art.** Parking it at the plain arrow and dropping any item/spell overlay is
//! the reference's `0x6e4940` (cursor index 1, set *before* the screen goes up), and it lives with
//! the cursor in [`crate::cursor`] rather than here — it is an output, not a channel.
//!
//! **Nothing else.** There is no dev exemption: the debug panel, the perf pill, the inspector and
//! their chords go quiet under the cover along with everything else. That is a real cost — a stuck
//! load is exactly when you want the instruments — and it is deliberate, because the alternative
//! is a keycode allowlist, and a keycode handed to the dev plane is also a keycode handed to the
//! keybinding table. The diagnosis path for a stuck load is the wait instrument
//! ([`super::WAIT_LOG_AFTER`]), which names the blocking term without anyone touching a key.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{
    AccumulatedMouseMotion, AccumulatedMouseScroll, MouseButtonInput, MouseMotion, MouseWheel,
};
use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::window::{CursorLeft, CursorMoved, PrimaryWindow};

use super::LoadingScreen;

/// The set [`swallow_input_under_the_cover`] runs in — `PreUpdate`, after
/// `bevy::input::InputSystems` and before `bevy::ui::UiSystems::Focus`.
///
/// Exported so the handful of `PreUpdate` systems that legitimately write input — the capture
/// probes — can order themselves after it instead of racing it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CoverInput;

/// Where the OS pointer really is while the cover has the window's cursor position blanked.
///
/// Re-read every covered frame, not stashed once: `bevy_winit` keeps writing the window's position
/// on every physical move regardless of whether anyone reads the `CursorMoved` message we drain,
/// so this tracks the pointer through the whole load and hands it back on the way out. Without the
/// hand-back the position would stay `None` until the player's next mouse move — hover would come
/// back dead after a teleport, and the first click would land nowhere, which is the same defect in
/// the other direction.
///
/// # `set_physical_cursor_position` MOVES THE MOUSE (decision 2090)
///
/// The load-bearing fact, because the name does not say it and getting it wrong shipped a bug for
/// months: **`Window::set_physical_cursor_position` is not "tell bevy where the pointer is", it is
/// "put the pointer there".** It writes `Window.internal.physical_cursor_position`, and
/// `bevy_winit::system::changed_windows` turns *any* change of that field to a `Some` into
/// `winit_window.set_cursor_position(…)` — `CGWarpMouseCursorPosition` on macOS, a real hardware
/// warp. There is no bookkeeping-only setter; this field is bevy's cursor-warp API, and winit is
/// its only other writer.
///
/// So the hand-back cannot simply write what it stashed. It used to, and the pointer was yanked
/// across the screen on every cover drop: measured on 2026-09-08 across a live `/logout`, the
/// restore wrote `(811.9, 1242.6)` physical and the hardware cursor teleported from `(308, 588)`
/// to `(406, 621)` points in the same millisecond. The stash goes stale over exactly the frames a
/// cover is up for — a world teardown is one ~283 ms frame during which no events are pumped at
/// all — so the pointer landed wherever it had been when the world went away, which after a click
/// on the centred game menu's Logout is the middle of the window.
///
/// **The law, therefore: never write a position we do not know to be true right now.** Which is
/// [`swallow_input_under_the_cover`]'s three-way hand-back:
///
/// - Winit already has a position (the field reads `Some`) → it is fresher than anything stashed,
///   so leave it alone. This is the case a moving mouse always takes: the events queued during a
///   hitch are delivered *before* the frame that drops the cover.
/// - The pointer has left the window ([`left`](Self::left)), or the window does not have the
///   keyboard → write nothing. We have no business moving a pointer that is over somebody else's
///   window.
/// - Otherwise nothing has moved since we stashed it, so the stash *is* where the pointer is and
///   writing it back moves nothing.
#[derive(Resource, Default)]
pub(crate) struct CoveredPointer {
    /// The last position seen while covered, in physical pixels.
    stashed: Option<DVec2>,
    /// Is the window's cursor position currently ours (blanked) rather than winit's?
    blanked: bool,
    /// Has winit said the pointer left the window since [`stashed`](Self::stashed) was taken?
    ///
    /// The one thing the window's own field cannot tell us: a `None` there is either winit's
    /// `CursorLeft` or our blank, and the two must not be confused — restoring the stash over the
    /// first would warp the pointer back *into* the window from wherever the player took it.
    left: bool,
}

/// One frame's swallow of one button plane.
///
/// The whole rule, and it needs no memory of its own:
///
/// - **A press that arrives under the cover never happened** — `just_pressed` is exactly this
///   frame's arrivals, and [`ButtonInput::reset`] takes each out of all three sets.
/// - **Whatever is still held came from before the cover, and is released once, with an edge** —
///   after the step above, `pressed` can only contain buttons held on an earlier frame, so
///   [`ButtonInput::release_all`] moves precisely those into `just_released`. On every *later*
///   covered frame `pressed` is already empty and this is a no-op, so the release edge is
///   delivered exactly once per cover, with no flag to keep.
///
/// A real OS release arriving mid-cover is likewise silent: bevy's own release only records an
/// edge for a button it still has pressed, and we took it out on the first covered frame.
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

/// `swallow` for a plane whose button type is not `Copy` (`ButtonInput<Key>`'s logical keys carry
/// a `SmolStr`).
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

/// The raw input-message queues, as one [`bevy::ecs::system::SystemParam`] — bevy's 16-element
/// system-param ceiling, the same squeeze `drive_loading_screen` next door already pays.
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

/// The button planes + the accumulators, as one param (see [`RawInput`] on the ceiling).
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

/// `PreUpdate`, in [`CoverInput`]: take the whole input plane for as long as the cover is up.
///
/// **The one-frame edge, stated so nobody has to rediscover it.** The cover is raised in `Update`
/// (`drive_loading_screen`, in `WorldStage::Present`), and this runs in `PreUpdate`, so the raise
/// frame itself still takes input — its `Input` stage ran before the raise did. That is the
/// correct boundary rather than a miss: the raise frame's *render* is the first one that draws the
/// cover, so input dies on exactly the frames whose previous present showed it, and the player
/// never sees the cover on a frame that also acted on their mouse.
fn swallow_input_under_the_cover(
    screen: Res<LoadingScreen>,
    mut pointer: ResMut<CoveredPointer>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut departures: MessageReader<CursorLeft>,
    mut buttons: Buttons,
    mut raw: RawInput,
) {
    // **Winit's last word on the pointer, read before anything below can overwrite it.** The
    // window's position field already reflects the LAST cursor event of this frame (bevy_winit
    // writes it as it dispatches, in order, before the update runs), so `Some` here is the live
    // truth and outranks the stash. A `None` is ambiguous — winit's `CursorLeft` or our own blank —
    // which is the whole reason [`CoveredPointer::left`] exists. Drained every frame, covered or
    // not, so the reader never hands a covered frame a departure from two frames ago.
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
        // The way out: hand the window its pointer back, once, on the frame the cover drops — and
        // only when that hand-back is TRUE, because the write is a hardware warp (see
        // [`CoveredPointer`]). Winit's own fresher answer wins; a pointer that has left the window,
        // or a window that does not have the keyboard, gets nothing.
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

    // The pointer half. Re-stash before blanking so the position tracks the real cursor through
    // the load (see [`CoveredPointer`]); `physical_cursor_position()` already answers `None` for a
    // pointer outside the window, which is the same nothing we are about to write.
    if let Ok(mut window) = window.single_mut() {
        if let Some(seen) = here {
            pointer.stashed = Some(seen.as_dvec2());
            // Blanking is safe where restoring is not: `changed_windows` guards its warp on the
            // *clamped* getter answering `Some`, so writing `None` re-caches without touching the
            // hardware pointer. Only when there is something to blank — an unconditional write
            // would deref-mut `Window` every covered frame, and a spurious `Changed<Window>` is a
            // surface reconfigure (`video::apply_present_mode`'s note).
            window.set_physical_cursor_position(None);
        }
        pointer.blanked = true;
    }
}

/// Wire the gate. Called by [`super::LoadingScreenPlugin`] — this is not a plugin of its own,
/// because the cover and its input rule are one system and must never be registerable apart.
pub(super) fn build(app: &mut App) {
    app.init_resource::<CoveredPointer>().add_systems(
        PreUpdate,
        swallow_input_under_the_cover
            .in_set(CoverInput)
            // After the input is built, before anything reads it. `UiSystems::Focus` is bevy's own
            // `PreUpdate` reader (it hit-tests the cursor into `Interaction`, which is what
            // `glue::glue_clicks` rides), so it has to land on the other side of us.
            .after(bevy::input::InputSystems)
            .before(bevy::ui::UiSystems::Focus),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;

    /// The [`swallow`] rule, both halves, in the order they matter: a press that arrives under the
    /// cover is erased, and a button held from before it is released **once**, with a real edge.
    #[test]
    fn a_press_under_the_cover_never_happened_and_a_held_one_is_released_once() {
        let mut keys = ButtonInput::<KeyCode>::default();

        // Held from before the cover: pressed on an earlier frame, so its `just_pressed` edge is
        // gone by now (bevy clears the edge sets at the head of every frame).
        keys.press(KeyCode::KeyW);
        keys.clear();
        // …and this frame, under the cover, the player also presses SPACE.
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

        // The next covered frame: bevy clears the edge sets, and there is nothing left to release.
        keys.clear();
        swallow(&mut keys);
        assert_eq!(
            keys.get_just_released().count(),
            0,
            "the release is delivered exactly once per cover, not every frame"
        );
        assert_eq!(keys.get_pressed().count(), 0);
    }

    /// A real OS release arriving mid-cover produces no second edge — bevy's own `release` only
    /// records one for a button it still holds pressed, and the first covered frame took it out.
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

    /// The gate against a real `App` and bevy's own `InputSystems`: with the cover up, a frame of
    /// input reaches nobody — button planes, raw queues and the window's cursor position all read
    /// empty — and the pointer comes back on the frame the cover drops, with no mouse move needed.
    #[test]
    fn a_covered_frame_hands_no_input_to_anyone_and_gives_the_pointer_back() {
        let mut app = App::new();
        app.add_plugins((
            bevy::input::InputPlugin,
            // For `Messages<CursorMoved>` — a window message, not an input one — and for the
            // window types themselves. `primary_window: None`: the test spawns its own.
            bevy::window::WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            },
        ))
        .init_resource::<LoadingScreen>()
        .init_resource::<CoveredPointer>()
        // `UiSystems::Focus` is bevy's, and pulling `UiPlugin` into a unit test to state an
        // ordering the real wiring already states would test bevy, not us.
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

        // A pointer in the middle of the window, and a key held from before the cover.
        put_cursor(&mut app, Some(DVec2::new(400.0, 300.0)));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);

        // --- Uncovered: an ordinary frame. Nothing is touched.
        app.update();
        assert!(seen(&app).is_some(), "no cover, no blanking");
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW),
            "no cover, no swallow"
        );

        // --- The cover goes up, and the player presses SPACE under it.
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

        // --- The cover clears: the pointer comes straight back.
        app.world_mut().resource_mut::<LoadingScreen>().active = false;
        app.update();
        assert_eq!(
            seen(&app),
            Some(Vec2::new(400.0, 300.0)),
            "the position is handed back on the frame the cover drops"
        );
    }

    /// **The hand-back never invents a position** (decision 2090). Writing the window's cursor
    /// position is a hardware warp, so the cover may only write one it knows is still true — which
    /// is the difference between handing the pointer back and dragging it across the player's
    /// screen. Three cases, one harness: winit's fresher answer wins; a departed pointer and an
    /// unfocused window get nothing; a still pointer gets its stash.
    #[test]
    fn the_hand_back_never_writes_a_position_it_does_not_know_to_be_true() {
        /// Cover, blank, then drop the cover under `arrange` — answering: what does the cover write
        /// into the window on the way out?
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

            // A pointer in the window, then the cover: one covered frame stashes and blanks it.
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

        // Winit spoke first: bevy_winit writes the window's field as it dispatches the frame's
        // events, so a `Some` on the drop frame is the live pointer. Overwriting it with the stash
        // is exactly the warp — this is the case a moving mouse always takes, and the one that
        // dragged the cursor to the middle of the window on every `/logout` before 2090.
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
