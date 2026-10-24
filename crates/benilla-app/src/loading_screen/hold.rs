//! A long same-map teleport freezes the frame until the destination is drawn, and the count of
//! frames the window presents, which the instruments read.
//!
//! The reference's near-teleport snap (`0x617e90`, from the drain arm `0x6164db`) calls
//! `0x630bd0` when it moved the mover more than 30 yards (`d² > 900`), and when the mover is the
//! unit the world view follows, that reloads the destination in place: `0x66fc80` recentres the
//! streaming windows and `0x694920` runs one full map update (`0x698390`: tiles, doodads, map
//! objects) with reads made synchronous (`0x86a2d0`) and waits for every one (`0x443d30`). The
//! ack's apply runs in the frame-time step (category 5), an apply-now `MSG_MOVE_TELEPORT` in the
//! packet step (category 6), both ahead of the frame pass that presents (0x11), so no frame is
//! presented until the destination is in, the next one shows it whole, and no loading
//! screen is raised. Window messages arriving meanwhile wait in the queue and are delivered, in
//! order, by the next tick's input step (`0x42c9f0`).
//!
//! Here the destination streams in over frames under the settle hold, so the frame is frozen
//! instead: while [`PresentHold`] is up the render world draws no camera and withholds the
//! present, so the window keeps the last frame it presented, and the raw input messages are kept
//! back and handed in, in order, on the first frame after. The pointer the pick and hover read
//! holds where it was on the last frame shown, so no `OnEnter`, `OnLeave` or tooltip fires under
//! the frozen frame. The hold ends with the settle hold, on the destination's residency
//! (`WorldLoadProgress::presentable`) or its stall budget.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bevy::ecs::message::{Message, MessageCursor};
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{KeyboardFocusLost, KeyboardInput};
use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::render::camera::SortedCameras;
use bevy::render::extract_resource::{extract_resource, ExtractResource};
use bevy::render::render_resource::SurfaceTexture;
use bevy::render::renderer::render_system;
use bevy::render::view::ExtractedWindows;
use bevy::render::{ExtractSchedule, Render, RenderApp, RenderSystems};
use bevy::window::{CursorMoved, PrimaryWindow};

/// Whether this frame is withheld from the window: decided in `PostUpdate` from the settle hold of
/// a snap that reloads the world ([`crate::player::Player::holds_the_frame`]), and extracted with
/// the frame it decides.
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
pub(crate) struct PresentHold {
    held: bool,
}

impl PresentHold {
    /// This frame renders but does not present.
    pub(crate) fn held(&self) -> bool {
        self.held
    }

    /// A held frame, for tests.
    #[cfg(test)]
    pub(crate) fn holding() -> Self {
        Self { held: true }
    }
}

/// `PostUpdate`: hold the frame while a reloading snap's settle hold is up.
pub(crate) fn decide_present_hold(
    player: Option<Res<crate::player::Player>>,
    mut hold: ResMut<PresentHold>,
    mut frames: Local<u32>,
) {
    let held = player.is_some_and(|p| p.holds_the_frame());
    if held {
        *frames += 1;
    }
    if hold.held != held {
        hold.held = held;
        if held {
            info!("frame: held — a same-map teleport reloads the destination");
        } else {
            info!("frame: resumed after {} held frames", *frames);
            *frames = 0;
        }
    }
}

/// Frames presented so far: one per render that presented at least one window.
#[derive(Resource, Clone, Default)]
pub(crate) struct Presents(Arc<AtomicU64>);

impl Presents {
    /// The count so far. Under pipelined rendering a frame's present lands while the main world
    /// runs the next frame, so a reader in the main world sees it a frame or two late.
    pub(crate) fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Render world: the windows holding a swap-chain texture going into this frame's render.
#[derive(Resource, Default)]
struct Acquired(Vec<Entity>);

/// Render world: the swap-chain textures a held frame keeps from `render_system`'s present.
#[derive(Resource, Default)]
struct Withheld(Vec<(Entity, SurfaceTexture)>);

/// Render world, before `render_system`: note which windows hold a texture to present.
fn note_acquired(windows: Res<ExtractedWindows>, mut acquired: ResMut<Acquired>) {
    acquired.0.clear();
    acquired.0.extend(
        windows
            .iter()
            .filter(|(_, w)| w.swap_chain_texture.is_some())
            .map(|(&e, _)| e),
    );
}

/// Render world, before `render_system`: a held frame runs no camera's graph, as the reference's
/// blocked tick runs no frame pass, and presents nothing: each window's texture is taken so the
/// present finds none, and the window keeps the last frame it presented.
fn withhold_present(
    hold: Option<Res<PresentHold>>,
    mut sorted: ResMut<SortedCameras>,
    mut windows: ResMut<ExtractedWindows>,
    mut withheld: ResMut<Withheld>,
) {
    if !hold.is_some_and(|h| h.held) {
        return;
    }
    sorted.0.clear();
    for (&entity, window) in windows.windows.iter_mut() {
        if let Some(texture) = window.swap_chain_texture.take() {
            withheld.0.push((entity, texture));
        }
    }
}

/// Render world, after `render_system`: a window that held a texture and holds none presented it;
/// one withheld did not.
fn count_presents(
    windows: Res<ExtractedWindows>,
    acquired: Res<Acquired>,
    withheld: Res<Withheld>,
    presents: Res<Presents>,
) {
    let presented = acquired
        .0
        .iter()
        .filter(|&&e| !withheld.0.iter().any(|&(w, _)| w == e))
        .any(|e| {
            windows
                .get(e)
                .is_some_and(|w| w.swap_chain_texture.is_none())
        });
    if presented {
        presents.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Render world, after `render_system`: hand the textures back, so each window keeps its texture
/// into the next frame (`prepare_windows` acquires none), which draws over it. Never dropped
/// unpresented: Vulkan's discard is a no-op that leaves the image acquired.
fn restore_withheld(mut windows: ResMut<ExtractedWindows>, mut withheld: ResMut<Withheld>) {
    for (entity, texture) in withheld.0.drain(..) {
        if let Some(window) = windows.windows.get_mut(&entity) {
            window.swap_chain_texture = Some(texture);
        }
    }
}

/// One raw input queue's messages from held frames, read past and hidden from every reader.
struct Deferred<M: Message> {
    cursor: MessageCursor<M>,
    held: Vec<M>,
}

impl<M: Message> Default for Deferred<M> {
    fn default() -> Self {
        Self {
            cursor: MessageCursor::default(),
            held: Vec::new(),
        }
    }
}

impl<M: Message + Clone> Deferred<M> {
    /// A held frame: keep what arrived since the last look, and leave the queue empty.
    fn hold(&mut self, queue: &mut Messages<M>) {
        self.held.extend(self.cursor.read(queue).cloned());
        queue.clear();
    }

    /// A shown frame: hand back what was kept, ahead of what arrived since, in arrival order.
    fn release(&mut self, queue: &mut Messages<M>) {
        if !self.held.is_empty() {
            let fresh: Vec<M> = self.cursor.read(queue).cloned().collect();
            queue.clear();
            queue.write_batch(self.held.drain(..).chain(fresh));
        }
        self.cursor.clear(queue);
    }

    fn step(&mut self, held: bool, queue: &mut Messages<M>) {
        if held {
            self.hold(queue);
        } else {
            self.release(queue);
        }
    }
}

/// The pointer a held frame's systems read: winit writes the window's position straight in, and
/// the pick and hover read it there, so a held frame reads [`Self::shown`] in its place.
#[derive(Resource, Default)]
struct HeldPointer {
    /// Where the pointer was on the last frame shown, in physical pixels.
    shown: Option<Vec2>,
    /// winit's position, set aside for the held frame and handed back at its end. Read and written
    /// at `f32`, the precision `changed_windows` compares at, so the hand-back reads unchanged.
    live: Option<Option<Vec2>>,
}

/// The keys and the window's focus losses, deferred in one order. Bevy's keyboard pass applies a
/// frame's keys and then releases everything on a focus loss (`keyboard_input_system`), so a loss
/// is handed back in a frame of its own with the keys that came before it, and the keys after it
/// follow on the next frame.
#[derive(Default)]
struct DeferredKeys {
    keys: Deferred<KeyboardInput>,
    losses: Deferred<KeyboardFocusLost>,
    /// How many of the held keys came before the last focus loss under the hold.
    lost_after: Option<usize>,
}

impl DeferredKeys {
    fn step(
        &mut self,
        held: bool,
        keys: &mut Messages<KeyboardInput>,
        losses: &mut Messages<KeyboardFocusLost>,
    ) {
        if held {
            self.keys.hold(keys);
            let lost = self.losses.cursor.read(losses).count() > 0;
            losses.clear();
            if lost {
                self.lost_after = Some(self.keys.held.len());
            }
            return;
        }
        let Some(cut) = self.lost_after.take() else {
            self.keys.release(keys);
            self.losses.release(losses);
            return;
        };
        // The keys up to the loss, then the loss; the rest, and whatever arrived since, wait a
        // frame behind it.
        let fresh: Vec<KeyboardInput> = self.keys.cursor.read(keys).cloned().collect();
        keys.clear();
        let after = self.keys.held.split_off(cut);
        keys.write_batch(self.keys.held.drain(..));
        self.keys.held = after.into_iter().chain(fresh).collect();
        self.keys.cursor.clear(keys);
        losses.write(KeyboardFocusLost);
        self.losses.cursor.clear(losses);
    }
}

/// The raw queues the input plane is built from, as the cover's input rule takes them.
#[derive(Resource, Default)]
struct HeldInput {
    keyboard: DeferredKeys,
    buttons: Deferred<MouseButtonInput>,
    motion: Deferred<MouseMotion>,
    wheel: Deferred<MouseWheel>,
    moved: Deferred<CursorMoved>,
}

#[derive(SystemParam)]
struct RawQueues<'w> {
    keyboard: ResMut<'w, Messages<KeyboardInput>>,
    focus_lost: ResMut<'w, Messages<KeyboardFocusLost>>,
    buttons: ResMut<'w, Messages<MouseButtonInput>>,
    motion: ResMut<'w, Messages<MouseMotion>>,
    wheel: ResMut<'w, Messages<MouseWheel>>,
    moved: ResMut<'w, Messages<CursorMoved>>,
}

/// `PreUpdate`, ahead of `InputSystems`: while the frame is held keep the raw input back, and on
/// the first frame shown deliver it. The held key tables are left as they stand, as the
/// reference's are until its pump runs.
fn defer_input_through_the_hold(
    hold: Res<PresentHold>,
    mut deferred: ResMut<HeldInput>,
    mut queues: RawQueues,
    mut pointer: ResMut<HeldPointer>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
) {
    let held = hold.held;
    if let (true, Ok(mut window)) = (held, window.single_mut()) {
        pointer.live = Some(window.physical_cursor_position());
        // Unchanged, so `changed_windows` does not read it as a warp; handed back in `PostUpdate`.
        window
            .bypass_change_detection()
            .set_physical_cursor_position(pointer.shown.map(|p| p.as_dvec2()));
    }
    deferred
        .keyboard
        .step(held, &mut queues.keyboard, &mut queues.focus_lost);
    deferred.buttons.step(held, &mut queues.buttons);
    deferred.motion.step(held, &mut queues.motion);
    deferred.wheel.step(held, &mut queues.wheel);
    deferred.moved.step(held, &mut queues.moved);
}

/// `PostUpdate`, after every reader: a held frame hands winit's position back, so the window
/// leaves the frame as winit left it; a frame shown notes where the pointer was.
fn hand_back_the_pointer(
    mut pointer: ResMut<HeldPointer>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
) {
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    match pointer.live.take() {
        Some(live) => window
            .bypass_change_detection()
            .set_physical_cursor_position(live.map(|p| p.as_dvec2())),
        None => pointer.shown = window.physical_cursor_position(),
    }
}

pub(super) fn build(app: &mut App) {
    let presents = Presents::default();
    app.insert_resource(presents.clone())
        .init_resource::<PresentHold>()
        .init_resource::<HeldInput>()
        .init_resource::<HeldPointer>()
        .add_systems(PostUpdate, (hand_back_the_pointer, decide_present_hold))
        .add_systems(
            PreUpdate,
            defer_input_through_the_hold.before(bevy::input::InputSystems),
        );
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(presents)
        .init_resource::<Acquired>()
        .init_resource::<Withheld>()
        .add_systems(ExtractSchedule, extract_resource::<PresentHold>)
        .add_systems(
            Render,
            (
                (note_acquired, withhold_present)
                    .chain()
                    .before(render_system),
                (count_presents, restore_withheld)
                    .chain()
                    .after(render_system),
            )
                .in_set(RenderSystems::Render),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::keyboard::Key;
    use bevy::input::ButtonState;

    fn key(state: ButtonState) -> KeyboardInput {
        KeyboardInput {
            key_code: KeyCode::KeyW,
            logical_key: Key::Character("w".into()),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    #[derive(Resource, Default)]
    struct Seen(Vec<ButtonState>);

    fn see(mut keys: MessageReader<KeyboardInput>, mut seen: ResMut<Seen>) {
        seen.0.extend(keys.read().map(|k| k.state));
    }

    fn take_seen(app: &mut App) -> Vec<ButtonState> {
        std::mem::take(&mut app.world_mut().resource_mut::<Seen>().0)
    }

    /// A key pressed under the hold and then an alt-tab: the focus loss arrives after the press,
    /// in order, so the key ends released, as the window's own order releases it.
    #[test]
    fn a_press_then_a_focus_loss_under_the_hold_ends_released() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin))
            .add_message::<CursorMoved>()
            .init_resource::<PresentHold>()
            .init_resource::<HeldInput>()
            .init_resource::<HeldPointer>()
            .add_systems(
                PreUpdate,
                defer_input_through_the_hold.before(bevy::input::InputSystems),
            );
        let pressed = |app: &App| {
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        };

        app.world_mut().resource_mut::<PresentHold>().held = true;
        app.world_mut().write_message(key(ButtonState::Pressed));
        app.update();
        app.world_mut().write_message(KeyboardFocusLost);
        app.update();
        app.world_mut().resource_mut::<PresentHold>().held = false;
        for _ in 0..3 {
            app.update();
        }
        assert!(
            !pressed(&app),
            "the press is released by the focus loss after it"
        );
    }

    /// The reference pumps no window message while its reload blocks (`0x42c9f0` runs only from
    /// the input step), and the next tick delivers all of them in order.
    #[test]
    fn input_through_a_held_frame_waits_and_arrives_in_order() {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<KeyboardFocusLost>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseMotion>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<PresentHold>()
            .init_resource::<HeldInput>()
            .init_resource::<HeldPointer>()
            .init_resource::<Seen>()
            .add_systems(PreUpdate, defer_input_through_the_hold)
            .add_systems(Update, see);

        // A key pressed before the hold arrives at once.
        app.world_mut().write_message(key(ButtonState::Pressed));
        app.update();
        assert_eq!(take_seen(&mut app), [ButtonState::Pressed]);

        app.world_mut().resource_mut::<PresentHold>().held = true;
        app.world_mut().write_message(key(ButtonState::Released));
        app.update();
        app.world_mut().write_message(key(ButtonState::Pressed));
        app.update();
        assert!(
            take_seen(&mut app).is_empty(),
            "nothing reaches a reader while the frame is held"
        );

        // The first shown frame delivers what waited, ahead of what arrived with it.
        app.world_mut().resource_mut::<PresentHold>().held = false;
        app.world_mut().write_message(key(ButtonState::Released));
        app.update();
        assert_eq!(
            take_seen(&mut app),
            [
                ButtonState::Released,
                ButtonState::Pressed,
                ButtonState::Released
            ]
        );
        app.update();
        assert!(take_seen(&mut app).is_empty(), "and delivers it once");
    }
    /// The pick and hover read the window's pointer; while the frame is held they read where it
    /// was on the last frame shown, as the reference's pick waits for the queued moves, and the
    /// first frame shown reads where it is now. The window keeps winit's position outside the
    /// frame's systems, so the pointer is never warped.
    #[test]
    fn the_pointer_the_pick_reads_holds_still_through_a_held_frame() {
        #[derive(Resource, Default)]
        struct Picked(Vec<Option<Vec2>>);
        fn pick(window: Single<&Window>, mut picked: ResMut<Picked>) {
            picked.0.push(window.physical_cursor_position());
        }
        let at = |x: f64| Some(Vec2::new(x as f32, 100.0));
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<KeyboardFocusLost>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseMotion>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<PresentHold>()
            .init_resource::<HeldInput>()
            .init_resource::<HeldPointer>()
            .init_resource::<Picked>()
            .add_systems(PreUpdate, defer_input_through_the_hold)
            .add_systems(Update, pick)
            .add_systems(PostUpdate, hand_back_the_pointer);
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        // What winit does between frames: write the pointer straight into the window.
        let winit_moves_to = |app: &mut App, x: f64| {
            app.world_mut()
                .get_mut::<Window>(window)
                .unwrap()
                .set_physical_cursor_position(at(x).map(|p| p.as_dvec2()));
        };

        winit_moves_to(&mut app, 10.0);
        app.update();
        app.world_mut().resource_mut::<PresentHold>().held = true;
        winit_moves_to(&mut app, 20.0);
        app.update();
        assert_eq!(
            app.world()
                .get::<Window>(window)
                .unwrap()
                .physical_cursor_position(),
            at(20.0),
            "the window ends the frame on winit's position, so nothing warps"
        );
        winit_moves_to(&mut app, 30.0);
        app.update();
        app.world_mut().resource_mut::<PresentHold>().held = false;
        app.update();
        assert_eq!(
            app.world().resource::<Picked>().0,
            [at(10.0), at(10.0), at(10.0), at(30.0)],
            "no hover change until the frame is shown again"
        );
    }
}
