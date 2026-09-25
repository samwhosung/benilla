//! The world-map data feed behind the stock `WorldMapFrame.xml` and benilla-ui's map verbs:
//!
//! - [`seed_world_map_catalog`], at world entry: the static catalog from `WorldMapArea`,
//!   `AreaTable`, `WorldMapContinent`, `Map` and the `.zmp` bitmaps, built as the reference's
//!   `0x4a5d00` builds it, with the instance maps no continent owns beside it.
//! - [`feed_world_map`], every frame: the player's selection (`0x4a6650`), the blips projected
//!   onto the displayed map, and the landmark list (`0x4a67a0`), rebuilt when its inputs change.
//! - [`dev_map_jump`]: Alt+click the map to go there.

use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::{
    load_world_map_area_catalog, load_world_map_continent_catalog, load_world_map_overlay_catalog,
    load_zone_map, WorldMapArea,
};
use benilla_ui::script::{
    UiScript, WorldMapContinentView, WorldMapLandmarkView, WorldMapOverlayView, WorldMapZoneView,
};

use crate::net::{ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_script::UiInput;
use benilla_assets::MapCatalogRes;
use benilla_assets::{LockRecover, WorldAssets};
use benilla_world::map_proj::{self, WorldProj, ZoneRect};
use benilla_world::world_map::CurrentMap;

/// The app's copy of the pushed catalog's projection data (rects, world-sheet constants), in the
/// engine's order: the indices must agree.
#[derive(Resource)]
pub(crate) struct WorldMapUiData {
    continents: Vec<ContinentEntry>,
    /// The instance maps no continent owns, `0x4a5d00`'s third array in `WorldMapArea.dbc` file
    /// order, unsorted: what the `-2` direct-area selection resolves against.
    direct: Vec<DirectAreaEntry>,
}

pub(crate) struct ContinentEntry {
    map_id: u32,
    proj: Option<WorldProj>,
    rect: ZoneRect,
    zones: Vec<ZoneEntry>,
}

struct ZoneEntry {
    area_id: u32,
    rect: ZoneRect,
}

/// One instance map, selected directly rather than through a continent and zone; 1.12 ships
/// three, the battlegrounds.
struct DirectAreaEntry {
    /// The `WorldMapArea` row id, which the third selection cell `[0x845074]` holds (`0x4a6717`);
    /// never a list index.
    id: u32,
    /// Matched against the player's map by the resolver (`0x4a66f6`); the projection admits only a
    /// body on this map (`0x4a7437`).
    map_id: u32,
    /// The row's loc rect, which `0x4a7360`'s direct-area leg projects through as a zone's.
    rect: ZoneRect,
    /// What the engine is handed for this row (`GetMapInfo`'s art folder, the overlays).
    view: WorldMapZoneView,
}

fn zone_rect(a: &WorldMapArea) -> ZoneRect {
    ZoneRect {
        left: a.loc_left,
        right: a.loc_right,
        top: a.loc_top,
        bottom: a.loc_bottom,
    }
}

/// One `WorldMapArea` row as the engine sees it. Zones, cities and instance maps are alike: one
/// `0x4a6cf0` lookup answers `GetMapInfo` for all three.
fn map_row_view(
    wma_id: u32,
    a: &WorldMapArea,
    name: String,
    areas: &benilla_formats::AreaTableCatalog,
    wmo: &benilla_formats::WorldMapOverlayCatalog,
) -> WorldMapZoneView {
    WorldMapZoneView {
        name,
        area_id: a.area_id,
        map_file: a.name.clone(),
        loc_rect: (a.loc_left, a.loc_right, a.loc_top, a.loc_bottom),
        // WorldMapOverlay rows by WMA id, each revealed by its covered areas' `exploreFlag` bits.
        overlays: wmo
            .for_area(wma_id)
            .iter()
            .map(|o| WorldMapOverlayView {
                texture: format!("Interface\\WorldMap\\{}\\{}", a.name, o.texture_name),
                width: o.texture_width,
                height: o.texture_height,
                offset_x: o.offset_x,
                offset_y: o.offset_y,
                explore_bits: o
                    .area_id
                    .iter()
                    .filter(|&&aid| aid != 0)
                    .filter_map(|&aid| areas.get(aid).map(|r| r.explore_flag))
                    .collect(),
                hit_rect: (
                    o.hit_rect_top,
                    o.hit_rect_left,
                    o.hit_rect_bottom,
                    o.hit_rect_right,
                ),
                // The hover label is the first area slot's name (`0x4a7fa0` reads `+0x8` only);
                // with no row the overlay is hidden from the hover, not from the draw.
                area_name: areas.name(o.area_id[0]).map(str::to_string),
            })
            .collect(),
    }
}

/// The catalog off the patch chain, the pure half of [`seed_world_map_catalog`]; `None` when a
/// DBC fails to load, which disables the map window.
pub(crate) fn build_catalog(
    chain: &mut benilla_formats::Chain,
    areas: &benilla_formats::AreaTableCatalog,
    maps: &MapCatalogRes,
) -> Option<(Vec<WorldMapContinentView>, WorldMapUiData)> {
    let loaded = load_world_map_area_catalog(&mut *chain).and_then(|wma| {
        let wmc = load_world_map_continent_catalog(&mut *chain)?;
        let wmo = load_world_map_overlay_catalog(&mut *chain)?;
        Ok((wma, wmc, wmo))
    });
    let (wma, wmc, wmo) = match loaded {
        Ok(t) => t,
        Err(e) => {
            error!("world map: DBC load failed, map window disabled: {e:#}");
            return None;
        }
    };

    // Continents: the areaId 0 rows in WorldMapArea file order, the Lua continent index
    // (`0x4a5d00`'s walk).
    let cont_rows: Vec<(u32, &WorldMapArea)> = wma.iter().filter(|(_, a)| a.area_id == 0).collect();

    let mut entries = Vec::with_capacity(cont_rows.len());
    let mut views = Vec::with_capacity(cont_rows.len());
    for (_, cont) in cont_rows {
        // Zones: this continent's other rows, sorted case-insensitively by AreaTable name, the
        // Lua zone index (`0x4a6390`'s `SStrCmpI`).
        let mut zones: Vec<(u32, &WorldMapArea, String)> = wma
            .iter()
            .filter(|(_, a)| a.map_id == cont.map_id && a.area_id != 0)
            .map(|(id, a)| {
                let name = areas.name(a.area_id).unwrap_or(a.name.as_str()).to_string();
                (id, a, name)
            })
            .collect();
        zones.sort_by_key(|(_, _, name)| name.to_lowercase());

        let proj = wmc.get(cont.map_id).map(|c| WorldProj {
            offset_u: c.offset_x,
            offset_v: c.offset_y,
            scale: c.scale,
        });
        // The world-sheet rect, `0x4a5d00` over the WorldMapContinent tile bounds: the world-level
        // click box, disjoint per continent unlike the art rects.
        let world_rect = wmc
            .get(cont.map_id)
            .zip(proj)
            .map(|(c, p)| {
                map_proj::continent_sheet_rect(
                    (
                        c.left_boundary,
                        c.right_boundary,
                        c.top_boundary,
                        c.bottom_boundary,
                    ),
                    p,
                )
            })
            .unwrap_or((0.0, 0.0, 0.0, 0.0));
        let rect = zone_rect(cont);

        // The area bitmap as 1-based zone indices: the load-time remap `0x4a6070`, a one-hop
        // parent rollup, then the (mapId, areaId) match.
        let zone_grid: Vec<u16> = match load_zone_map(&mut *chain, &cont.name) {
            Ok(grid) => grid
                .iter()
                .map(|&raw| {
                    let mut area_id = raw;
                    if let Some(row) = areas.get(area_id) {
                        if row.zone_id != 0 {
                            area_id = row.zone_id; // one hop, not a full walk
                        }
                    }
                    zones
                        .iter()
                        .position(|(_, a, _)| a.area_id == area_id)
                        .map(|i| i as u16 + 1)
                        .unwrap_or(0)
                })
                .collect(),
            Err(e) => {
                // The client tolerates a missing bitmap: hover and click are inert.
                warn!("world map: no zone bitmap for {}: {e:#}", cont.name);
                Vec::new()
            }
        };

        views.push(WorldMapContinentView {
            // Map.dbc's localized name, never the art folder (`0x4a65a0`).
            name: maps
                .0
                .name(cont.map_id)
                .unwrap_or(cont.name.as_str())
                .to_string(),
            map_file: cont.name.clone(),
            world_rect,
            loc_rect: (cont.loc_left, cont.loc_right, cont.loc_top, cont.loc_bottom),
            zone_grid,
            zones: zones
                .iter()
                .map(|(wma_id, a, name)| map_row_view(*wma_id, a, name.clone(), areas, &wmo))
                .collect(),
        });
        entries.push(ContinentEntry {
            map_id: cont.map_id,
            proj,
            rect,
            zones: zones
                .iter()
                .map(|(_, a, _)| ZoneEntry {
                    area_id: a.area_id,
                    rect: zone_rect(a),
                })
                .collect(),
        });
    }

    // The instance maps: rows with `areaID != 0` on no continent's map, in file order, unsorted
    // (`0x4a5d00`'s passes `0x4a6130` and `0x4a61c9`); the `areaID` test is the reference's,
    // though inert on the shipped data (`0x4a6170`).
    let continent_maps: Vec<u32> = entries.iter().map(|c| c.map_id).collect();
    let direct: Vec<DirectAreaEntry> = wma
        .iter()
        .filter(|(_, a)| a.area_id != 0 && !continent_maps.contains(&a.map_id))
        .map(|(id, a)| DirectAreaEntry {
            id,
            map_id: a.map_id,
            rect: zone_rect(a),
            view: map_row_view(
                id,
                a,
                areas.name(a.area_id).unwrap_or(a.name.as_str()).to_string(),
                areas,
                &wmo,
            ),
        })
        .collect();

    Some((
        views,
        WorldMapUiData {
            continents: entries,
            direct,
        },
    ))
}

/// The built continent catalog, kept for the process: the walk cannot change, so a later login
/// re-seeds its VM from here. The instance maps ride [`WorldMapUiData`].
#[derive(Resource)]
pub(crate) struct WorldMapCatalog(pub(crate) Vec<WorldMapContinentView>);

/// Push the map catalog into the VM before any interface file runs, from
/// [`crate::ui_script::load_ingame_ui_on_world_entry`]: addons read `GetMapContinents` and
/// `GetMapZones` at file scope (Astrolabe builds its zone table there), and the reference holds
/// this DBC data from load.
pub(crate) fn seed_world_map_catalog(world: &mut World, script: &mut UiScript) {
    if !world.contains_resource::<WorldMapCatalog>() {
        let Some((views, data)) = build_catalog_from_world(world) else {
            return;
        };
        info!(
            "world map: catalog — {} continents, {} zones, {} instance maps",
            views.len(),
            views.iter().map(|c| c.zones.len()).sum::<usize>(),
            data.direct.len()
        );
        world.insert_resource(data);
        world.insert_resource(WorldMapCatalog(views));
    }
    let Some(catalog) = world.get_resource::<WorldMapCatalog>() else {
        return;
    };
    script.set_world_map_catalog(catalog.0.clone());
    // The instance maps, at the same edge: the reference fills both containers in one walk.
    if let Some(data) = world.get_resource::<WorldMapUiData>() {
        script.set_world_map_direct_areas(
            data.direct
                .iter()
                .map(|d| (d.id, d.view.clone()))
                .collect::<Vec<_>>(),
        );
    }
}

/// [`build_catalog`] over the app's `Startup` resources; only a bare test world gets `None`.
fn build_catalog_from_world(world: &World) -> Option<(Vec<WorldMapContinentView>, WorldMapUiData)> {
    let assets = world.get_resource::<WorldAssets>()?;
    let maps = world.get_resource::<MapCatalogRes>()?;
    let areas = world.get_resource::<crate::area::AreaTableRes>()?;
    let mut chain = assets.chain.lock_recover();
    build_catalog(&mut chain, &areas.0, maps)
}

/// The displayed level the landmark gates branch on, from the selection cells `[0x84506c]`,
/// `[0x845070]` and `[0x845074]`. An instance map is `Zone`: both gates treat it as zone level
/// (`0x4a79de`; `0x4a8856`/`0x4a885f` fall through to `0x4a8868`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MapLevel {
    /// Both continents on one sheet (the reference's `continent == -1`).
    World,
    /// One continent, no zone selected (`continent >= 0, zone == -1`).
    Continent,
    /// One zone (`continent >= 0, zone >= 0`), or an instance map (`continent == -2`).
    Zone,
}

impl MapLevel {
    /// From the engine's selection, the 1-based `(continent, zone)` with 0 for whole plus the
    /// direct area, which is read first because it shares the world sheet's `(0, 0)`.
    fn of((continent, zone, direct): (u32, u32, Option<u32>)) -> Self {
        match (continent, zone, direct) {
            (_, _, Some(_)) => Self::Zone,
            (0, _, None) => Self::World,
            (_, 0, None) => Self::Continent,
            _ => Self::Zone,
        }
    }
}

/// The inputs the landmark list is a pure function of. `0x4a67a0` rebuilds only on these: a
/// selection change, a world-state push (`0x48fa0d`), a `PLAYER_EXPLORED_ZONES` change
/// (`0x4a6477`, an UpdateFields callback on byte `0xe6c`) and the gossip marker's set and clear.
#[derive(PartialEq, Eq)]
struct LandmarkKey {
    /// All three selection cells, the direct area's included.
    selection: (u32, u32, Option<u32>),
    map: u32,
    states: u64,
    explored: Vec<u32>,
    /// The guard-directions marker `(continent, pos, icon)`, the position as raw `f32` bits: it is
    /// copied from the packet, so a bit compare is exact.
    marker: Option<(u32, [u32; 3], u32)>,
}

impl LandmarkKey {
    fn matches(
        &self,
        selection: (u32, u32, Option<u32>),
        map: u32,
        states: u64,
        explored: &[u32],
        marker: Option<(u32, [u32; 3], u32)>,
    ) -> bool {
        self.selection == selection
            && self.map == map
            && self.states == states
            && self.marker == marker
            && self.explored == explored
    }
}

/// The builder's near-zero skip (`0x4a6868`/`0x4a687a`, epsilon `2.384e-7`): a UV of 0 on both
/// axes is off the displayed map, which is how POIs are filtered by continent; a POI on a rect's
/// edge has one non-zero axis and stays.
fn is_degenerate(uv: (f32, f32)) -> bool {
    const EPS: f32 = 2.384e-7;
    uv.0.abs() < EPS && uv.1.abs() < EPS
}

/// `0x4a67a0`'s AreaPOI gates, in the reference's order:
///
/// 1. Level flag (`0x4a79b0`): `0x04` at zone level, `0x08` at continent, and both `0x10` and
///    `0x08` at world level, since the chain falls through rather than switching.
/// 2. Exploration (`0x4a6890`-`0x4a68f3`): a signed `AreaID > 0` (`-1` is continent-wide) whose
///    `AreaTable` row has `ExplorationLevel >= 0` must be discovered.
/// 3. World state (`0x4a6903`): a row with a `WorldStateID` shows only while that state is
///    non-zero; the Eastern Plaguelands towers are one row per tower, owner and state.
///
/// The loop reads no `ContinentID`, `Importance`, `FactionID` or `Icon`.
fn landmark_gates_pass(
    poi: &benilla_formats::AreaPoi,
    level: MapLevel,
    areas: &benilla_formats::AreaTableCatalog,
    explored: &[u32],
    world_states: &crate::world_state::WorldStates,
) -> bool {
    let level_ok = match level {
        MapLevel::Zone => poi.flags & 0x04 != 0,
        MapLevel::Continent => poi.flags & 0x08 != 0,
        MapLevel::World => poi.flags & 0x10 != 0 && poi.flags & 0x08 != 0,
    };
    if !level_ok {
        return false;
    }
    // A row the table does not carry passes: the bounds check on `[0xc0e048]`/`[0xc0e04c]`
    // guards the read, not the landmark.
    if poi.area_id as i32 > 0 {
        if let Some(area) = areas.get(poi.area_id) {
            if area.exploration_level >= 0 && !explored_bit(explored, area.explore_flag) {
                return false;
            }
        }
    }
    poi.world_state_id == 0 || world_states.get(poi.world_state_id) != 0
}

/// `PLAYER_EXPLORED_ZONES` bit `bit`, which `0x4a9a40` reads bytewise; the little-endian dword
/// slots hold the same bits.
fn explored_bit(explored: &[u32], bit: u32) -> bool {
    let (slot, within) = (bit as usize / 32, bit % 32);
    explored.get(slot).is_some_and(|w| w & (1 << within) != 0)
}

/// `GetMapLandmarkInfo`'s `textureIndex` (`0x4a8848`-`0x4a8877`): the row's `Icon`, but cell 15
/// at zone level for a row without `Flags & 0x80`, which the shipped data sets on exactly the
/// world-state rows.
fn landmark_texture_index(poi: &benilla_formats::AreaPoi, level: MapLevel) -> u32 {
    /// The generic zone-level POI cell of `Interface\Minimap\POIIcons`.
    const ZONE_SUBSTITUTE: u32 = 15;
    match poi.flags & 0x80 != 0 || level != MapLevel::Zone {
        true => poi.icon,
        false => ZONE_SUBSTITUTE,
    }
}

/// Which map the player's position selects (`0x4a6650`), in the Lua-visible encoding; at most one
/// leg is set, since the instance-map loop runs only on a continent miss (`0x4a66c3`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct PlayerSelection {
    /// The 1-based `(continent, zone)`, zone 0 for the continent map: `SetMap(continent, -1)`,
    /// which `GetCurrentMapZone` (`0x4a7f00`) reads back as 0.
    zone: Option<(u32, u32)>,
    /// The `WorldMapArea` row id of the instance map the player is on.
    direct: Option<u32>,
}

/// `SetMapToCurrentZone`'s resolver `0x4a6650` (`0x4a7e20`), which the engine's world-enter sync
/// `0x4947ac` also calls: the continent carrying the player's map, then its zone, where a zone
/// miss leaves the continent map; only on a continent miss, the first instance map on the
/// player's map (`SetMap(-2, id)`, `0x4a6717`); else the world view.
fn resolve_player_selection(
    data: &WorldMapUiData,
    map_id: u32,
    top_zone: Option<u32>,
) -> PlayerSelection {
    if let Some(ci) = data.continents.iter().position(|c| c.map_id == map_id) {
        let zone = top_zone
            .and_then(|top| {
                data.continents[ci]
                    .zones
                    .iter()
                    .position(|z| z.area_id == top)
            })
            .map_or(0, |zi| zi as u32 + 1);
        return PlayerSelection {
            zone: Some((ci as u32 + 1, zone)),
            direct: None,
        };
    }
    PlayerSelection {
        zone: None,
        direct: data
            .direct
            .iter()
            .find(|d| d.map_id == map_id)
            .map(|d| d.id),
    }
}

/// The engine's world-enter sync, `0x494780`'s `old == 0` leg into `0x4a6650`, which fires once
/// per session: it waits for a known zone (`0x67e510` bails on 0), then applies whatever the
/// resolver answers, its failure legs' world view (`0x4a667e`/`0x4a670b`) and an instance map
/// included.
fn world_enter_selection(
    synced: bool,
    top_zone: Option<u32>,
    player: PlayerSelection,
) -> Option<PlayerSelection> {
    (!synced && top_zone.is_some()).then_some(player)
}

/// A position's UV on the displayed map (`0x4a7360`), `None` off it, which Lua sees as the
/// `(0, 0)` hide: the world sheet projects through the position's own continent constants, a
/// continent or zone through its rect, an instance map through its row's rect (`0x4a73e0`). The
/// last two require the position's own map (`0x4a7437`; `0x4a7870` passes the unit's).
pub(crate) fn project_on_displayed(
    data: &WorldMapUiData,
    selection: (u32, u32, Option<u32>),
    pos_map: u32,
    px: f32,
    py: f32,
) -> Option<(f32, f32)> {
    match selection {
        // The direct cell first: it shares the `(0, 0)` pair with the world sheet.
        (_, _, Some(id)) => data
            .direct
            .iter()
            .find(|d| d.id == id && d.map_id == pos_map)
            .map(|d| map_proj::zone_uv(d.rect, px, py)),
        (0, _, None) => data
            .continents
            .iter()
            .find(|cont| cont.map_id == pos_map)
            .and_then(|cont| cont.proj)
            .map(|p| map_proj::world_uv(p, px, py)),
        (c, z, None) => data
            .continents
            .get(c as usize - 1)
            .filter(|cont| cont.map_id == pos_map)
            .and_then(|cont| match z {
                0 => Some(cont.rect),
                z => cont.zones.get(z as usize - 1).map(|zone| zone.rect),
            })
            .map(|rect| map_proj::zone_uv(rect, px, py)),
    }
}

/// [`feed_world_map`]'s memos under one [`crate::ui_script::VmMemo`], so a new VM resets them all.
#[derive(Default)]
struct FeedMemos {
    explored: Option<Vec<u32>>,
    landmarks: Option<LandmarkKey>,
    /// The world-enter sync has run: the reference's `old == 0` gate.
    map_synced: bool,
}

fn feed_world_map(
    script: Option<NonSendMut<UiScript>>,
    data: Option<Res<WorldMapUiData>>,
    player: Res<Player>,
    map: Option<Res<CurrentMap>>,
    world: benilla_world::world_point::WorldPoint,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    areas: Option<Res<crate::area::AreaTableRes>>,
    death_net: Res<crate::death::DeathNet>,
    poi_marker: Res<crate::poi_marker::PoiMarker>,
    group: Res<crate::ui_party::GroupState>,
    guids: Res<crate::net::GuidIndex>,
    unit_pos: Query<&GlobalTransform, With<crate::net::NetEntity>>,
    pois: Option<Res<crate::area_poi::AreaPoiRes>>,
    world_states: Res<crate::world_state::WorldStates>,
    mut memo: Local<crate::ui_script::VmMemo<FeedMemos>>,
) {
    let (Some(mut script), Some(data), Some(map), Some(areas)) = (script, data, map, areas) else {
        return;
    };
    // PLAYER_EXPLORED_ZONES, pushed on change; the setter queues WORLD_MAP_UPDATE for newly
    // explored art.
    let explored: Vec<u32> = self_q
        .iter()
        .next()
        .map(|store| {
            (0..benilla_protocol::messages::PLAYER_EXPLORED_ZONES_SLOTS)
                .map(|i| store.0.player_explored_zone_slot(i))
                .collect()
        })
        .unwrap_or_default();
    {
        let memo = memo.get(&script);
        if !explored.is_empty() && memo.explored.as_ref() != Some(&explored) {
            memo.explored = Some(explored.clone());
            script.set_world_map_explored(explored.clone());
        }
    }
    let wow = bevy_to_wow(player.pos);
    let (wx, wy) = (wow[0], wow[1]);

    // CurrentArea is the MCNK leaf; its top zone is the zone-level area the reference matches.
    let top_zone = world.area().and_then(|aid| areas.0.top_zone(aid));
    let player_sel = resolve_player_selection(&data, map.0, top_zone);

    // First world enter: the zone updater `0x494780` (`crate::area`), finding the cached zone id
    // 0, selects the player's zone itself through `0x4a6650` and the setter `0x4a67a0` before any
    // Lua asks. Once per VM, each login being a first enter.
    {
        let memo = memo.get(&script);
        if let Some(sel) = world_enter_selection(memo.map_synced, top_zone, player_sel) {
            memo.map_synced = true;
            let (c, z) = sel.zone.unwrap_or((0, 0));
            script.sync_world_map_to_player_zone(c, z, sel.direct);
        }
    }

    let selection = script.world_map_selection();
    let project =
        |pos_map: u32, px: f32, py: f32| project_on_displayed(&data, selection, pos_map, px, py);
    let uv = project(map.0, wx, wy);
    // The corpse at the query answer's display position, a dungeon corpse at its entrance.
    let corpse_uv = death_net.corpse.and_then(|cp| {
        project(
            u32::try_from(cp.display_map).unwrap_or(u32::MAX),
            cp.position[0],
            cp.position[1],
        )
    });

    // The landmarks (`GetNumMapLandmarks`), rebuilt only when a `LandmarkKey` input moves.
    let marker = poi_marker
        .on_map(map.0)
        .map(|m| (m.continent_id, m.pos.map(f32::to_bits), m.icon));
    let states_gen = world_states.generation();
    let unchanged = memo
        .get(&script)
        .landmarks
        .as_ref()
        .is_some_and(|k| k.matches(selection, map.0, states_gen, &explored, marker));
    if !unchanged {
        let level = MapLevel::of(selection);
        let mut landmarks = Vec::new();
        // The DBC rows first, in file order, the Lua landmark index (`0x4a6819`'s walk of
        // `[0xc0e054]`).
        if let Some(pois) = pois.as_ref() {
            for (_, poi) in pois.0.rows() {
                if !landmark_gates_pass(poi, level, &areas.0, &explored, &world_states) {
                    continue;
                }
                let Some(uv) = project(poi.continent_id, poi.pos[0], poi.pos[1]) else {
                    continue;
                };
                if is_degenerate(uv) {
                    continue;
                }
                landmarks.push(WorldMapLandmarkView {
                    name: poi.name.clone(),
                    description: poi.description.clone(),
                    texture_index: landmark_texture_index(poi, level),
                    uv,
                });
            }
        }
        // Then the guard-directions marker, last (`0x4a69c7`): no gate and no icon substitution,
        // only the degenerate-projection test.
        if let Some(poi) = poi_marker.on_map(map.0) {
            if let Some(uv) = project(poi.continent_id, poi.pos[0], poi.pos[1]) {
                if !is_degenerate(uv) {
                    landmarks.push(WorldMapLandmarkView {
                        name: poi.name.clone(),
                        description: poi.description.clone(),
                        texture_index: poi.icon,
                        uv,
                    });
                }
            }
        }
        memo.get(&script).landmarks = Some(LandmarkKey {
            selection,
            map: map.0,
            states: states_gen,
            explored: explored.clone(),
            marker,
        });
        script.set_world_map_landmarks(landmarks);
    }

    // The `party1..4` blips: a streamed member's transform, else the `i16` position from
    // `SMSG_PARTY_MEMBER_STATS`. That packet carries a zone, not a map, so the map is the
    // continent owning that zone, else ours.
    let member_uv = |m: &benilla_protocol::messages::GroupMemberEntry| {
        let (px, py) = crate::minimap::party_member_pos(m, &group, &guids, &unit_pos)?;
        let member_map = group
            .stats
            .get(&m.guid)
            .and_then(|st| st.zone)
            .and_then(|zone| {
                data.continents
                    .iter()
                    .find(|cont| cont.zones.iter().any(|z| z.area_id == u32::from(zone)))
                    .map(|cont| cont.map_id)
            })
            .unwrap_or(map.0);
        project(member_map, px, py).filter(|uv| *uv != (0.0, 0.0))
    };
    let party_uv: Vec<Option<(f32, f32)>> = group.party_slots().map(member_uv).collect();

    // The `raid1..40` blips in the raid roster's order, ourselves first so `raidN` matches; the
    // stock map skips our own slot by `UnitIsUnit` (`WorldMapFrame.lua:381`).
    let raid_uv: Vec<Option<(f32, f32)>> = if group.group_type == crate::ui_party::GROUPTYPE_RAID {
        std::iter::once(uv)
            .chain(group.members.iter().map(member_uv))
            .collect()
    } else {
        Vec::new()
    };

    script.set_world_map_feed(
        player_sel.zone,
        uv,
        player.facing(),
        corpse_uv,
        party_uv,
        raid_uv,
    );
    // The instance-map leg for `SetMapToCurrentZone`, which the battlefield minimap calls on
    // `PLAYER_ENTERING_WORLD` (`Blizzard_BattlefieldMinimap.lua:58-62`).
    script.set_world_map_player_direct_area(player_sel.direct);
}

/// Alt+click the world map to go there, a dev jump: the click's UV back through the displayed
/// rect to a world `(x, y)`, sent as vmangos's `.go xy x y <mapid>` so the server finds the ground.
/// It declines on the world sheet, where the client's click law `0x4a7100` does not invert its
/// projection (`map_proj::world_click_world`). The click is not consumed: the stock
/// `WorldMapButton_OnClick` still zooms into the clicked zone.
fn dev_map_jump(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    ui_scale: Res<crate::ui_script::UiScaleCvar>,
    script: Option<NonSendMut<UiScript>>,
    data: Option<Res<WorldMapUiData>>,
    net: Res<crate::net::NetCommands>,
) {
    // A dev affordance compiled into every build, so gated at run time.
    if !crate::run_mode::dev_affordances() {
        return;
    }
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    if !alt || !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let (Some(script), Some(data), Ok(window)) = (script, data, windows.single()) else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    // Window px (y-down) to the VM's y-up 768-unit space, as the pointer feed converts them.
    let s = crate::ui_script::seam_scale(window.height(), ui_scale.0);
    let Some((u, v)) = script.world_map_uv_at(cursor.x / s, (window.height() - cursor.y) / s)
    else {
        return;
    };
    let (c, z, direct) = script.world_map_selection();
    // An instance map inverts like a zone, through `0x4a7100`'s zone mode.
    let (rect, map_id) = match direct {
        Some(id) => match data.direct.iter().find(|d| d.id == id) {
            Some(d) => (d.rect, d.map_id),
            None => return,
        },
        None => {
            let Some(cont) = c
                .checked_sub(1)
                .and_then(|i| data.continents.get(i as usize))
            else {
                info!(
                    "map-jump: no exact click→world law at the world level — zoom into a \
                     continent first"
                );
                return;
            };
            let rect = match z.checked_sub(1) {
                None => cont.rect,
                Some(i) => match cont.zones.get(i as usize) {
                    Some(zone) => zone.rect,
                    None => return,
                },
            };
            (rect, cont.map_id)
        }
    };
    // `zone_world` lerps both axes by one `t`, the reference's shape, so it runs once per axis.
    let (_, wy) = map_proj::zone_world(rect, u);
    let (wx, _) = map_proj::zone_world(rect, v);
    let text = format!(".go xy {wx:.2} {wy:.2} {map_id}");
    info!("map-jump: {text}");
    let _ = net.0.send(crate::net::ClientCommand::Chat {
        kind: crate::net::ChatKind::Say,
        target: None,
        text,
    });
}

/// The world-map data feed.
pub(crate) struct WorldMapUiPlugin;

impl Plugin for WorldMapUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // After the script tick: a selection changed this tick projects on the next.
                feed_world_map.after(UiInput),
                // After the script tick, so the jump hit-tests this tick's frame rects.
                dev_map_jump.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_state::WorldStates;
    use benilla_formats::AreaPoi;

    /// A bare row; each test sets only the columns its gate reads.
    fn poi(flags: u32, area_id: u32, world_state_id: u32) -> AreaPoi {
        AreaPoi {
            importance: 0,
            icon: 4,
            faction_id: 0,
            pos: [0.0, 0.0, 0.0],
            continent_id: 0,
            flags,
            area_id,
            name: String::new(),
            description: String::new(),
            world_state_id,
        }
    }

    fn empty_areas() -> benilla_formats::AreaTableCatalog {
        benilla_formats::AreaTableCatalog::from_rows(Vec::new())
    }

    #[test]
    fn the_level_flag_chain_falls_through_at_world_level() {
        let (areas, states, explored) = (empty_areas(), WorldStates::default(), Vec::new());
        let pass = |flags, level| {
            landmark_gates_pass(&poi(flags, 0, 0), level, &areas, &explored, &states)
        };

        // A town (0x0d = 0x01|0x04|0x08): zone and continent, not the world sheet.
        assert!(pass(0x0d, MapLevel::Zone));
        assert!(pass(0x0d, MapLevel::Continent));
        assert!(!pass(0x0d, MapLevel::World));

        // A capital (0x1d = 0x0d|0x10): all three.
        assert!(pass(0x1d, MapLevel::Zone));
        assert!(pass(0x1d, MapLevel::Continent));
        assert!(pass(0x1d, MapLevel::World));

        // An Eastern Plaguelands tower (0x87 = 0x01|0x02|0x04|0x80): zone only.
        assert!(pass(0x87, MapLevel::Zone));
        assert!(!pass(0x87, MapLevel::Continent));
        assert!(!pass(0x87, MapLevel::World));

        // `0x10` alone, which 1.12 does not ship: world level still wants `0x08`.
        assert!(!pass(0x10, MapLevel::World));
        assert!(!pass(0x10, MapLevel::Continent));
        assert!(!pass(0x10, MapLevel::Zone));
    }

    #[test]
    fn a_world_state_row_appears_only_while_its_key_is_set() {
        let (areas, explored) = (empty_areas(), Vec::new());
        let mut states = WorldStates::default();
        // 2372 / 2373 are the real pair for Northpass Tower: Alliance-held / Horde-held.
        let alliance = poi(0x87, 0, 2372);
        let horde = poi(0x87, 0, 2373);
        let plain = poi(0x87, 0, 0);
        let pass = |p: &AreaPoi, st: &WorldStates| {
            landmark_gates_pass(p, MapLevel::Zone, &areas, &explored, st)
        };

        assert!(
            pass(&plain, &states),
            "an ungated row is always a candidate"
        );
        assert!(!pass(&alliance, &states), "nothing received yet");
        assert!(!pass(&horde, &states));

        states.write(&[(2372, 1)]);
        assert!(pass(&alliance, &states));
        assert!(!pass(&horde, &states));

        states.write(&[(2372, 0), (2373, 1)]);
        assert!(!pass(&alliance, &states));
        assert!(pass(&horde, &states));
    }

    #[test]
    fn the_exploration_gate_reads_the_right_bit_and_exempts_the_right_rows() {
        use benilla_formats::AreaTableRow;
        let row = |explore_flag, exploration_level| AreaTableRow {
            map_id: 0,
            zone_id: 0,
            explore_flag,
            flags: 0,
            faction_group_mask: 0,
            exploration_level,
            name: String::new(),
        };
        let areas = benilla_formats::AreaTableCatalog::from_rows(vec![
            (139, row(40, 0)),  // gated
            (200, row(41, -1)), // exempt by ExplorationLevel
        ]);
        let states = WorldStates::default();
        // Bit 40 → slot 1, bit 8 within it.
        let unexplored = vec![0u32; 4];
        let explored = {
            let mut v = vec![0u32; 4];
            v[1] = 1 << 8;
            v
        };
        let pass =
            |p: &AreaPoi, ex: &[u32]| landmark_gates_pass(p, MapLevel::Zone, &areas, ex, &states);

        assert!(!pass(&poi(0x04, 139, 0), &unexplored), "not discovered yet");
        assert!(pass(&poi(0x04, 139, 0), &explored));
        assert!(
            pass(&poi(0x04, 200, 0), &unexplored),
            "ExplorationLevel -1 exempts the row"
        );
        assert!(
            pass(&poi(0x04, u32::MAX, 0), &unexplored),
            "AreaID -1 (continent-wide) is not > 0, so no gate"
        );
        assert!(pass(&poi(0x04, 0, 0), &unexplored), "AreaID 0 likewise");
        assert!(
            pass(&poi(0x04, 999, 0), &unexplored),
            "an AreaID the table does not carry cannot be tested"
        );
    }

    #[test]
    fn the_zone_level_icon_substitution_spares_the_world_state_rows() {
        let town = poi(0x0d, 0, 0);
        let tower = poi(0x87, 0, 2372);
        assert_eq!(landmark_texture_index(&town, MapLevel::Zone), 15);
        assert_eq!(
            landmark_texture_index(&town, MapLevel::Continent),
            town.icon
        );
        assert_eq!(landmark_texture_index(&town, MapLevel::World), town.icon);
        for level in [MapLevel::Zone, MapLevel::Continent, MapLevel::World] {
            assert_eq!(
                landmark_texture_index(&tower, level),
                tower.icon,
                "Flags & 0x80 escapes the substitution at every level"
            );
        }
    }

    #[test]
    fn only_a_both_axes_zero_projection_is_dropped() {
        assert!(is_degenerate((0.0, 0.0)));
        assert!(is_degenerate((1e-8, -1e-8)));
        assert!(!is_degenerate((0.0, 0.5)));
        assert!(!is_degenerate((0.5, 0.0)));
        assert!(!is_degenerate((1e-6, 0.0)));
    }

    #[test]
    fn the_selection_pair_maps_onto_the_reference_levels() {
        assert_eq!(MapLevel::of((0, 0, None)), MapLevel::World);
        assert_eq!(
            MapLevel::of((0, 3, None)),
            MapLevel::World,
            "continent 0 wins"
        );
        assert_eq!(MapLevel::of((2, 0, None)), MapLevel::Continent);
        assert_eq!(MapLevel::of((2, 7, None)), MapLevel::Zone);
        assert_eq!(MapLevel::of((0, 0, Some(443))), MapLevel::Zone);
    }

    #[test]
    fn the_real_table_gates_the_epl_towers_and_the_city_icons() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let pois = benilla_formats::load_area_poi_catalog(&mut chain).expect("AreaPOI");
        let areas = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable");
        let nothing_explored = vec![0u32; 64];

        // The towers: Northpass Tower's Alliance-held row.
        let (_, tower) = pois
            .rows()
            .find(|(id, _)| *id == 1768)
            .expect("AreaPOI 1768");
        assert_eq!(tower.name, "Northpass Tower");
        assert_eq!(tower.world_state_id, 2372);
        assert_eq!(
            tower.flags & 0x80,
            0x80,
            "escapes the level-15 substitution"
        );
        assert_eq!(tower.area_id, 139, "Eastern Plaguelands");

        let mut states = WorldStates::default();
        let explored_epl = {
            let bit = areas.get(139).expect("EPL area row").explore_flag;
            let mut v = vec![0u32; 64];
            v[bit as usize / 32] |= 1 << (bit % 32);
            v
        };
        let tower_shows = |st: &WorldStates, ex: &[u32]| {
            landmark_gates_pass(tower, MapLevel::Zone, &areas, ex, st)
        };
        assert!(
            !tower_shows(&states, &explored_epl),
            "no world states received — no tower icon (this IS the reported bug)"
        );
        states.write(&[(2372, 1)]);
        assert!(tower_shows(&states, &explored_epl), "Alliance holds it");
        assert!(
            !tower_shows(&states, &nothing_explored),
            "and it still needs the zone discovered"
        );
        assert!(
            !landmark_gates_pass(tower, MapLevel::Continent, &areas, &explored_epl, &states),
            "a tower is a zone-level row (Flags 0x87 carries no 0x08)"
        );

        // The capitals: continent level and exploration-exempt.
        let states = WorldStates::default();
        for (id, name) in [
            (16u32, "Stormwind"),
            (8, "Ironforge"),
            (18, "The Undercity"),
        ] {
            let (_, city) = pois.rows().find(|(r, _)| *r == id).expect("city row");
            assert_eq!(city.name, name);
            assert_eq!(
                city.area_id,
                u32::MAX,
                "continent-wide, so no exploration gate"
            );
            for level in [MapLevel::World, MapLevel::Continent, MapLevel::Zone] {
                assert!(
                    landmark_gates_pass(city, level, &areas, &nothing_explored, &states),
                    "{name} shows at every level on a fresh character"
                );
            }
            assert_eq!(
                landmark_texture_index(city, MapLevel::Continent),
                city.icon,
                "the city icon is its own at continent level"
            );
            assert_eq!(
                landmark_texture_index(city, MapLevel::Zone),
                15,
                "and the generic cell inside a zone map"
            );
        }
    }

    #[test]
    fn the_catalog_preserves_dbc_file_order() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let a: Vec<u32> = benilla_formats::load_area_poi_catalog(&mut chain)
            .expect("AreaPOI")
            .rows()
            .map(|(id, _)| id)
            .collect();
        let b: Vec<u32> = benilla_formats::load_area_poi_catalog(&mut chain)
            .expect("AreaPOI")
            .rows()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            a, b,
            "two loads agree — the order is the file's, not a hash's"
        );
        assert!(a.len() > 300);
        assert_ne!(
            a,
            {
                let mut sorted = a.clone();
                sorted.sort_unstable();
                sorted
            },
            "and it is genuinely file order, not id order"
        );
    }
}

#[cfg(test)]
mod player_zone_tests {
    use super::*;

    fn rect() -> ZoneRect {
        ZoneRect {
            left: 1.0,
            right: -1.0,
            top: 1.0,
            bottom: -1.0,
        }
    }

    /// Kalimdor (map 1), Azeroth (map 0) with Elwynn at index 9 (Lua zone 10), and Warsong
    /// Gulch's real row (`WorldMapArea` 443, map 489) as the one instance map.
    fn data() -> WorldMapUiData {
        WorldMapUiData {
            continents: vec![
                ContinentEntry {
                    map_id: 1,
                    proj: None,
                    rect: rect(),
                    zones: Vec::new(),
                },
                ContinentEntry {
                    map_id: 0,
                    proj: None,
                    rect: rect(),
                    zones: (0..10)
                        .map(|i| ZoneEntry {
                            // index 9 is Elwynn's areaId 12; the rest are filler ids.
                            area_id: if i == 9 { 12 } else { 900 + i },
                            rect: rect(),
                        })
                        .collect(),
                },
            ],
            direct: vec![DirectAreaEntry {
                id: 443,
                map_id: 489,
                rect: rect(),
                view: WorldMapZoneView {
                    name: "Warsong Gulch".into(),
                    area_id: 3277,
                    map_file: "WarsongGulch".into(),
                    loc_rect: (1.0, -1.0, 1.0, -1.0),
                    overlays: Vec::new(),
                },
            }],
        }
    }

    #[test]
    fn an_unresolved_zone_falls_back_to_the_continent_not_the_world() {
        let d = data();
        let on_continent = |c, z| PlayerSelection {
            zone: Some((c, z)),
            direct: None,
        };

        // Elwynn is continent 2, zone 10.
        assert_eq!(
            resolve_player_selection(&d, 0, Some(12)),
            on_continent(2, 10)
        );

        // An area the catalog has no row for.
        assert_eq!(
            resolve_player_selection(&d, 0, Some(4242)),
            on_continent(2, 0),
            "SetMap(continent, -1); GetCurrentMapZone reads that back as 0"
        );

        // The area feed has not answered yet.
        assert_eq!(
            resolve_player_selection(&d, 0, None),
            on_continent(2, 0),
            "a zone we do not know YET is still this continent"
        );

        // Map 389, Ragefire Chasm, has no `WorldMapArea` row: the world view, `SetMap(-1, -1)`.
        assert_eq!(
            resolve_player_selection(&d, 389, Some(12)),
            PlayerSelection::default()
        );
    }

    #[test]
    fn a_battleground_takes_the_orphan_leg_and_projects_on_its_own_rect() {
        let d = data();

        assert_eq!(
            resolve_player_selection(&d, 489, Some(3277)),
            PlayerSelection {
                zone: None,
                direct: Some(443),
            },
            "the orphan leg selects the WorldMapArea row id"
        );
        assert_eq!(
            resolve_player_selection(&d, 0, Some(12)).direct,
            None,
            "a continent match short-circuits before the orphan loop"
        );

        // The fixture rect is [-1, 1] on both axes, so the origin is its centre.
        let sel = (0, 0, Some(443));
        assert_eq!(
            project_on_displayed(&d, sel, 489, 0.0, 0.0),
            Some((0.5, 0.5)),
            "a body in the battleground lands on the battleground map"
        );
        assert_eq!(
            project_on_displayed(&d, sel, 0, 0.0, 0.0),
            None,
            "`0x4a7437`: a body on another map is refused, which is the (0,0) hide sentinel"
        );
        assert_eq!(
            project_on_displayed(&d, (0, 0, Some(9999)), 489, 0.0, 0.0),
            None,
            "an id no row carries resolves to nothing rather than indexing anything"
        );
        assert_eq!(
            project_on_displayed(&d, (0, 0, None), 489, 0.0, 0.0),
            None,
            "the world sheet has no continent for map 489 at all"
        );
        assert_eq!(MapLevel::of(sel), MapLevel::Zone);
        assert_eq!(MapLevel::of((0, 0, None)), MapLevel::World);
    }

    /// `0x494780` calls `0x4a6650` at `0x4947ac` when the cached zone id `[0xb4e314]` was 0.
    #[test]
    fn the_engine_selects_the_players_zone_on_first_world_enter() {
        let on_continent = |c, z| PlayerSelection {
            zone: Some((c, z)),
            direct: None,
        };
        // No area yet: `0x67e510` bails before `0x494780` on a zero zone id.
        assert_eq!(world_enter_selection(false, None, on_continent(2, 0)), None);

        assert_eq!(
            world_enter_selection(false, Some(12), on_continent(2, 10)),
            Some(on_continent(2, 10))
        );

        // Never again: only the world-session teardown `0x491180` re-zeroes `[0xb4e314]`.
        assert_eq!(
            world_enter_selection(true, Some(40), on_continent(2, 22)),
            None
        );

        // An unresolvable player still syncs, to the world view: every exit of `0x4a6650` writes.
        assert_eq!(
            world_enter_selection(false, Some(12), PlayerSelection::default()),
            Some(PlayerSelection::default())
        );

        // Logging straight into a battleground: the one sync carries the instance map.
        let bg = PlayerSelection {
            zone: None,
            direct: Some(443),
        };
        assert_eq!(world_enter_selection(false, Some(3277), bg), Some(bg));
    }
}

#[cfg(test)]
mod direct_area_tests {
    use super::*;

    /// The real catalog off the player's own chain, or a skip.
    fn real_catalog() -> Option<(Vec<WorldMapContinentView>, WorldMapUiData)> {
        let data = benilla_formats::wow_data_or_skip!(None);
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let areas = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable");
        let maps = benilla_assets::MapCatalogRes(
            benilla_formats::load_map_catalog(&mut chain).expect("Map"),
        );
        Some(build_catalog(&mut chain, &areas, &maps).expect("the real catalog"))
    }

    #[test]
    fn the_real_dbc_carries_exactly_the_three_battleground_orphans() {
        let Some((views, data)) = real_catalog() else {
            return;
        };

        assert_eq!(
            data.direct
                .iter()
                .map(|d| (d.id, d.map_id, d.view.map_file.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (401, 30, "AlteracValley"),
                (443, 489, "WarsongGulch"),
                (461, 529, "ArathiBasin"),
            ],
            "file order, no sort — the reference's fill pass appends as it walks"
        );

        for d in &data.direct {
            assert!(
                !views
                    .iter()
                    .any(|c| c.zones.iter().any(|z| z.map_file == d.view.map_file)),
                "{} is no continent's child",
                d.view.map_file
            );
        }
        assert!(
            data.continents
                .iter()
                .all(|c| c.map_id == 0 || c.map_id == 1),
            "and the continents are still only Azeroth and Kalimdor"
        );

        // Only Alterac Valley has overlay art, 3 rows on WMA 401.
        let overlays = |folder: &str| {
            data.direct
                .iter()
                .find(|d| d.view.map_file == folder)
                .map(|d| d.view.overlays.len())
        };
        assert_eq!(overlays("AlteracValley"), Some(3));
        assert_eq!(overlays("WarsongGulch"), Some(0));
        assert_eq!(overlays("ArathiBasin"), Some(0));
    }

    /// `GetMapInfo()` must answer `"WarsongGulch"` for `Blizzard_BattlefieldMinimap.lua:83-86` to
    /// draw; the continent reads back -1 (`-2 + 1`) and the zone 0.
    #[test]
    fn a_body_in_warsong_gulch_lands_on_the_warsong_gulch_map() {
        let Some((views, data)) = real_catalog() else {
            return;
        };

        let player = resolve_player_selection(&data, 489, Some(3277));
        assert_eq!(
            player,
            PlayerSelection {
                zone: None,
                direct: Some(443),
            }
        );

        let mut script = UiScript::new().expect("a bare engine");
        script.set_world_map_catalog(views);
        script.set_world_map_direct_areas(
            data.direct
                .iter()
                .map(|d| (d.id, d.view.clone()))
                .collect::<Vec<_>>(),
        );
        // The engine's world-enter sync (`0x4947ac`).
        let (c, z) = player.zone.unwrap_or((0, 0));
        script.sync_world_map_to_player_zone(c, z, player.direct);

        assert_eq!(
            script
                .eval::<(String, i64, i64)>(
                    "return GetMapInfo(), GetCurrentMapContinent(), GetCurrentMapZone()"
                )
                .expect("the getters answer"),
            ("WarsongGulch".into(), -1, 0)
        );

        // The blip: the rect centre is the middle of the map, and a body on Azeroth hides.
        let wsg = data
            .direct
            .iter()
            .find(|d| d.id == 443)
            .expect("the Warsong Gulch row");
        let (wx, wy) = (
            (wsg.rect.top + wsg.rect.bottom) / 2.0,
            (wsg.rect.left + wsg.rect.right) / 2.0,
        );
        let selection = script.world_map_selection();
        assert_eq!(selection, (0, 0, Some(443)));
        let uv = project_on_displayed(&data, selection, 489, wx, wy).expect("inside the rect");
        assert!(
            (uv.0 - 0.5).abs() < 1e-5 && (uv.1 - 0.5).abs() < 1e-5,
            "the rect centre is the middle of the battle map, got {uv:?}"
        );
        assert_eq!(project_on_displayed(&data, selection, 0, wx, wy), None);

        // A world-state push or an exploration update re-selects, keeping the instance map: the
        // reference re-passes `direct != -1 ? direct : zone` at all five in-place refresh sites.
        script.set_world_map_explored(vec![u32::MAX; 64]);
        script.set_world_map_player_direct_area(player.direct);
        script
            .run("SetMapToCurrentZone()")
            .expect("the OnShow verb");
        assert_eq!(
            script
                .eval::<(String, i64)>("return GetMapInfo(), GetCurrentMapContinent()")
                .expect("the getters answer"),
            ("WarsongGulch".into(), -1),
            "the instance selection survives every refresh path"
        );

        // A continent selection clears it: `0x4a67a0`'s other legs store -1 there.
        script.run("SetMapZoom(1)").expect("SetMapZoom");
        assert_eq!(
            script.world_map_selection(),
            (1, 0, None),
            "selecting a continent clears the direct cell"
        );
    }
}
