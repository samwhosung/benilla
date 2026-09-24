//! Drives [`benilla_assets::WorldAssets`] from the client: each system needs a client piece (the
//! light buffer, `MapChange`, the art scope), so it lives here rather than in `benilla-assets`.

use bevy::prelude::*;
use bevy::render::renderer::RenderDevice;

use crate::art_scope::{ArtScope, ArtSlot};
use benilla_assets::{AssetSet, RenderConfig, WorldAssets};
use benilla_formats::open_chain;

/// Opens the patch chain at startup and inserts the shared [`WorldAssets`] and [`RenderConfig`].
pub(crate) struct AssetPlugin;

impl Plugin for AssetPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, open_world_assets.in_set(AssetSet::Open));
        app.add_systems(Update, (evict_world_art, scope_world_art));
    }
}

/// Clears `textures` and `model_materials` on a map change, or they pin every map's art forever.
/// The UI sprite caches stay: they are global, and their negative entries stop per-frame re-walks.
fn evict_world_art(
    mut changes: MessageReader<crate::world_map::MapChange>,
    assets: Option<ResMut<WorldAssets>>,
) {
    if changes.is_empty() {
        return;
    }
    changes.clear();
    if let Some(mut a) = assets {
        a.textures.clear();
        a.model_materials.clear();
    }
}

/// Expires the world-art caches by distance within a map: a cached material pins the decoded BLP it
/// samples, so `textures` is what frees VRAM. The UI sprite caches stay, as on a map change.
fn scope_world_art(mut scope: ArtScope, assets: Option<ResMut<WorldAssets>>) {
    if let Some(mut a) = assets {
        scope.apply(&mut a.model_materials, ArtSlot::ClutterMats);
        scope.apply(&mut a.textures, ArtSlot::Textures);
    }
}

/// Opens the patch chain found by [`benilla_formats::wow_data`] and inserts [`WorldAssets`] and
/// [`RenderConfig`]; with no install, `WorldAssets` is absent and startup falls back to free-fly.
fn open_world_assets(mut commands: Commands, device: Res<RenderDevice>) {
    // Inserted before the install lookup so no early return skips it: even with no install,
    // `particles::model::update_model_particles` takes it as a hard `Res<SharedLightBuffer>`.
    let shared_light = crate::lighting::new_shared_light_buffer(&device);
    let light_buf = shared_light.0.clone();
    commands.insert_resource(shared_light);
    let Some(data) = benilla_formats::wow_data() else {
        warn!(
            "no WoW install found — looked in {:?}; starting with no world",
            benilla_formats::candidates()
        );
        return;
    };
    // Stale tiles released per frame: 1 outpaces even boosted free-fly's stale row a second.
    let unload_budget = std::env::var("WOW_TILE_UNLOAD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    commands.insert_resource(RenderConfig { unload_budget });

    match open_chain(&data) {
        Ok(chain) => commands.insert_resource(WorldAssets::open(chain, light_buf)),
        Err(e) => error!("failed to open client data: {e:#}"),
    }
}
