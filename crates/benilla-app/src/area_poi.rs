//! The shared `AreaPOI.dbc` catalog — the world's named points of interest, and the **one** copy
//! of that table in the process.
//!
//! It used to live inside `MinimapAssets`, because the minimap's nearest-3 landmark blips
//! (decision 0203 phase 3) were its only reader. The world map is the second (the
//! `0x4a67a0` landmark pass — city icons, the "Under Attack" indicators, the Eastern Plaguelands
//! towers), and two owners of one DBC is how the two surfaces drift apart on what a POI *is*. So
//! it is hoisted here, beside [`crate::area::AreaTableRes`], on 0287's precedent — the same move,
//! for the same reason, on the sibling table.
//!
//! Loaded once at Startup; absent if the DBC failed to read, which every consumer takes as
//! `Option` and goes quiet about (no landmark blips, no map icons, everything else intact).

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::AreaPoiCatalog;

/// The shared `AreaPOI.dbc` catalog, in file order (see [`AreaPoiCatalog`]).
#[derive(Resource)]
pub(crate) struct AreaPoiRes(pub(crate) AreaPoiCatalog);

/// Startup: load the shared POI catalog off the patch chain.
fn load_area_poi(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_area_poi_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("area_poi: {} rows in the shared AreaPOI catalog", cat.len());
            commands.insert_resource(AreaPoiRes(cat));
        }
        Err(e) => warn!("area_poi: AreaPOI.dbc failed — no map/minimap landmarks: {e:#}"),
    }
}

pub(crate) struct AreaPoiPlugin;

impl Plugin for AreaPoiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_area_poi.after(AssetSet::Open));
    }
}
