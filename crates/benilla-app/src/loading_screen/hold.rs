//! A long same-map teleport freezes the frame until the destination is drawn, and the count of
//! frames the window presents, which the instruments read.
//!
//! The reference's near-teleport snap (`0x617e90`, from the drain arm `0x6164db`) calls
//! `0x630bd0` when it moved the mover more than 30 yards (`d² > 900`), and when the mover is the
//! unit the world view follows, that reloads the destination in place: `0x66fc80` recentres the
//! streaming windows and `0x694920` runs one full map update (`0x698390`: tiles, doodads, map
//! objects) with reads made synchronous (`0x86a2d0`) and waits for every one (`0x443d30`). It runs
//! in the frame-time dispatch (category 5), ahead of the frame pass that presents (0x11), so no
//! frame is presented until the destination is in, the next one shows it whole, and no loading
//! screen is raised. Window messages arriving meanwhile wait in the queue and are delivered, in
//! order, by the next tick's input step (`0x42c9f0`).
//!
//! Here the destination streams in over frames under the settle hold, so the frame is frozen
//! instead: while [`PresentHold`] is up the render world renders as usual but withholds the
//! present, so the window keeps the last frame it presented, and the raw input messages are kept
//! back and handed in, in order, on the first frame after. The hold ends with the settle hold, on
//! the destination's residency (`WorldLoadProgress::presentable`) or its stall budget.
//!
//! Deviation: the window's cursor position, which winit writes directly, stays live through the
//! hold, so hover can move under the frozen frame where the reference's pick waits for the queued
//! moves; none of it shows until the frame resumes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bevy::ecs::message::{Message, MessageCursor};
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::render::extract_resource::{extract_resource, ExtractResource};
use bevy::render::render_resource::SurfaceTexture;
use bevy::render::renderer::render_system;
use bevy::render::view::ExtractedWindows;
use bevy::render::{ExtractSchedule, Render, RenderApp, RenderSystems};
use bevy::window::CursorMoved;

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
}

/// `PostUpdate`: hold the frame while a reloading snap's settle hold is up.
pub(super) fn decide_present_hold(
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

/// Render world, before `render_system`: on a held frame take each window's texture, so the
/// present finds nothing to present. The views keep theirs, so the frame still renders into it.
fn withhold_present(
    hold: Option<Res<PresentHold>>,
    mut windows: ResMut<ExtractedWindows>,
    mut withheld: ResMut<Withheld>,
) {
    if !hold.is_some_and(|h| h.held) {
        return;
    }
    for (&entity, window) in windows.windows.iter_mut() {
        if let Some(texture) = window.swap_chain_texture.take() {
            withheld.0.push((entity, texture));
        }
    }
}

/// Render world, after `render_system`: hand the textures back. A window that still holds one
/// keeps it into the next frame (`prepare_windows` acquires none), which renders over it.
fn restore_withheld(mut windows: ResMut<ExtractedWindows>, mut withheld: ResMut<Withheld>) {
    for (entity, texture) in withheld.0.drain(..) {
        if let Some(window) = windows.windows.get_mut(&entity) {
            window.swap_chain_texture = Some(texture);
        }
    }
}

/// Render world, after `render_system`: a window that held a texture and holds none presented it.
fn count_presents(
    windows: Res<ExtractedWindows>,
    acquired: Res<Acquired>,
    presents: Res<Presents>,
) {
    let presented = acquired.0.iter().any(|e| {
        windows
            .get(e)
            .is_some_and(|w| w.swap_chain_texture.is_none())
    });
    if presented {
        presents.0.fetch_add(1, Ordering::Relaxed);
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

    fn step(&mut self, _held: bool, queue: &mut Messages<M>) {
        self.release(queue);
    }
}

/// The raw queues the input plane is built from, as the cover's input rule takes them.
#[derive(Resource, Default)]
struct HeldInput {
    keyboard: Deferred<KeyboardInput>,
    buttons: Deferred<MouseButtonInput>,
    motion: Deferred<MouseMotion>,
    wheel: Deferred<MouseWheel>,
    moved: Deferred<CursorMoved>,
}

#[derive(SystemParam)]
struct RawQueues<'w> {
    keyboard: ResMut<'w, Messages<KeyboardInput>>,
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
) {
    let held = hold.held;
    deferred.keyboard.step(held, &mut queues.keyboard);
    deferred.buttons.step(held, &mut queues.buttons);
    deferred.motion.step(held, &mut queues.motion);
    deferred.wheel.step(held, &mut queues.wheel);
    deferred.moved.step(held, &mut queues.moved);
}

pub(super) fn build(app: &mut App) {
    let presents = Presents::default();
    app.insert_resource(presents.clone())
        .init_resource::<PresentHold>()
        .init_resource::<HeldInput>()
        .add_systems(PostUpdate, decide_present_hold)
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
                (restore_withheld, count_presents)
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

    /// The reference pumps no window message while its reload blocks (`0x42c9f0` runs only from
    /// the input step), and the next tick delivers all of them in order.
    #[test]
    fn input_through_a_held_frame_waits_and_arrives_in_order() {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseMotion>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>()
            .init_resource::<PresentHold>()
            .init_resource::<HeldInput>()
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
}
