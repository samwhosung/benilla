//! The asset foundation's **plugin shell** — the three systems that drive
//! [`benilla_assets::WorldAssets`] from inside the client.
//!
//! The store itself went down to `benilla-assets`; what could not follow it is
//! exactly this: opening the chain needs the shared light buffer (`lighting`), evicting the world
//! art needs the cross-map message (`world_map`), and the residency sweep needs the art-scope
//! instrument. Three upward reaches, all of them here, none of them in the data core.

use bevy::prelude::*;
use bevy::render::renderer::RenderDevice;

use crate::art_scope::{ArtScope, ArtSlot};
use benilla_assets::{AssetSet, RenderConfig, WorldAssets};
use benilla_formats::open_chain;

/// The asset foundation plugin: opens the **one** patch chain at startup and inserts the shared
/// [`WorldAssets`] + [`RenderConfig`] that the other subsystems build on.
pub(crate) struct AssetPlugin;

impl Plugin for AssetPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, open_world_assets.in_set(AssetSet::Open));
        app.add_systems(Update, (evict_world_art, scope_world_art));
    }
}

/// Drop the world-art dedup on a cross-map transition (`world_map::MapChange` — see its doc for
/// why a clear is always safe): `textures` + `model_materials` pin every map's world art forever
/// otherwise (the teleport leak). The UI sprite caches (`sprites`/`tiled_sprites`/
/// `portraits`/`masks`) stay — they are game-global UI scope, and their negative entries exist
/// precisely to stop per-frame re-walks of the chain.
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

/// Expire the world-art dedup by **distance** — the within-map half of the eviction
/// above. `textures` is the one that matters for VRAM: a decoded BLP is pinned by the material that
/// samples it, and a material by this cache, so nothing here dropping is why `images` never fell on a
/// same-map traverse. The UI sprite caches stay unswept for the same reason they survive a map change
/// (game-global scope, and their negative entries exist to stop per-frame chain re-walks).
fn scope_world_art(mut scope: ArtScope, assets: Option<ResMut<WorldAssets>>) {
    if let Some(mut a) = assets {
        scope.apply(&mut a.model_materials, ArtSlot::ClutterMats);
        scope.apply(&mut a.textures, ArtSlot::Textures);
    }
}

/// Open the vanilla patch chain from wherever the install is ([`benilla_formats::wow_data`] —
/// `$WOW_DATA`, the project folder on a dev build, else beside the binary) and
/// insert the shared [`WorldAssets`] (chain + dedup caches) + [`RenderConfig`]. If the client data
/// can't be found or opened, `WorldAssets` is simply absent and downstream startup falls back to
/// an empty free-fly scene.
fn open_world_assets(mut commands: Commands, device: Res<RenderDevice>) {
    // The one shared global-light buffer, created here (RenderDevice is live by Startup) so it exists
    // before any material is built. Inserted FIRST — ahead of the install lookup, so no early return
    // below can skip it: it is cloned into `WorldAssets` (for model materials) and read as the
    // `SharedLightBuffer` resource by the terrain streamer, the model/particle lanes and the
    // render-world upload. Always present — even with no client data — so the render upload has a
    // target; harmless if unused. It used to be created *after* the lookup, so a client that found no
    // install had no buffer at all and `particles::model::update_model_particles` — a hard
    // `Res<SharedLightBuffer>` — could not validate.
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
    // How much terrain is resident is NOT a knob here: the streamer derives its window from the
    // live `farclip` (`view::ViewDistance`, the player's Terrain Distance setting) the way the
    // reference does — `terrain_stream::window`. `$WOW_TILE_RADIUS` is retired.
    // See the field doc: the tile-unload budget. Default 1 — even the fastest focus
    // (boosted free-fly, ~1 stale row/s) produces stale tiles far slower than 60/s drains them.
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
