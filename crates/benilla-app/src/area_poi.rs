//! The one shared `AreaPOI.dbc` catalog, read by the minimap's landmark blips and the world map's
//! landmark pass (`0x4a67a0`). Absent when the DBC fails to load; every reader then shows no POIs.

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::AreaPoiCatalog;

/// The `AreaPOI.dbc` rows, in file order.
#[derive(Resource)]
pub(crate) struct AreaPoiRes(pub(crate) AreaPoiCatalog);

/// Startup: load the catalog off the patch chain.
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
