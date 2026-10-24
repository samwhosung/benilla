//! The frames the window presents, counted where they are presented: the render world's
//! `present_frames` (inside `render_system`) takes each window's swap-chain texture as it presents
//! it, so a window that held one before the render and holds none after has presented.
//! [`Presents`] is the count, shared with the main world for the instruments.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::renderer::render_system;
use bevy::render::view::ExtractedWindows;
use bevy::render::{Render, RenderApp, RenderSystems};

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

pub(super) fn build(app: &mut App) {
    let presents = Presents::default();
    app.insert_resource(presents.clone());
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(presents)
        .init_resource::<Acquired>()
        .add_systems(
            Render,
            (
                note_acquired.before(render_system),
                count_presents.after(render_system),
            )
                .in_set(RenderSystems::Render),
        );
}
