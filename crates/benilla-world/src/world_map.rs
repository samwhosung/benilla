//! World-map state: the `Map.dbc` catalog and the current map id, for every subsystem that keys
//! off the map (the terrain and WDL streamers, the loading screen, time-of-day lighting).

use benilla_assets::MapCatalogRes;
use benilla_formats::load_map_catalog;
use bevy::prelude::*;

use benilla_assets::LockRecover;
use benilla_assets::{AssetSet, WorldAssets};

/// The map before the server names one: Eastern Kingdoms (`Map.dbc` id 0).
pub(crate) const DEFAULT_MAP_ID: u32 = 0;

/// `$WOW_MAP`: the `Map.dbc` id a server-less run starts on (`0` Azeroth, `1` Kalimdor), unset
/// giving [`DEFAULT_MAP_ID`]. A value that does not parse panics rather than falling back.
fn map_id_from_env() -> u32 {
    match std::env::var("WOW_MAP") {
        Err(_) => DEFAULT_MAP_ID,
        Ok(v) => v.trim().parse().unwrap_or_else(|_| {
            panic!("WOW_MAP={v:?} is not a Map.dbc id — it takes a NUMBER (0 = Azeroth, 1 = Kalimdor), not a name")
        }),
    }
}

/// The map the player is on, rewritten by the game on every worldport, login's included.
#[derive(Resource, Clone, Copy)]
pub struct CurrentMap(pub u32);

/// A cross-map transition, the world-scope teardown signal: every map-scoped strong-handle cache
/// clears on it, since one that kept its handles would pin every map visited. A clear is safe
/// mid-session, as live users hold their own handles.
#[derive(Message, Clone, Copy)]
pub struct MapChange;

/// The startup set the map catalog loads in, which the streamers' setup runs after.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WorldMapLoad;

/// Loads `Map.dbc` and seeds [`CurrentMap`] at startup, after the patch chain opens.
pub(crate) struct WorldMapPlugin;

impl Plugin for WorldMapPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<MapChange>().add_systems(
            Startup,
            load_world_map.after(AssetSet::Open).in_set(WorldMapLoad),
        );
        app.add_systems(Update, announce_map_change);
    }
}

/// Announce every [`CurrentMap`] flip as a [`MapChange`]; the startup seed and a same-map
/// worldport announce nothing.
fn announce_map_change(
    map: Option<Res<CurrentMap>>,
    mut last: Local<Option<u32>>,
    mut changes: MessageWriter<MapChange>,
) {
    let Some(map) = map else { return };
    if let Some(prev) = last.replace(map.0) {
        if prev != map.0 {
            info!(
                "map change: {prev} → {} — evicting map-scoped caches",
                map.0
            );
            changes.write(MapChange);
        }
    }
}

/// Read `Map.dbc` off the patch chain into [`MapCatalogRes`] and seed [`CurrentMap`].
fn load_world_map(mut commands: Commands, world_assets: Option<Res<WorldAssets>>) {
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match load_map_catalog(&mut chain) {
        Ok(c) => {
            info!("Map.dbc: {} maps catalogued", c.len());
            commands.insert_resource(MapCatalogRes(c));
            // `$WOW_MAP` picks a server-less run's map (a named capture scenario sets it before
            // `Startup`); a live session's `player::wire_in` overwrites it from the server.
            let map = map_id_from_env();
            commands.insert_resource(CurrentMap(map));
        }
        Err(e) => error!("failed to load Map.dbc, cross-map teleport disabled: {e:#}"),
    }
}
