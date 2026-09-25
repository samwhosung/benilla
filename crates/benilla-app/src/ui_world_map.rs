//! The world-map data feed (decision 0203 phase 2) — the app half behind
//! stock `Interface\FrameXML\WorldMapFrame.xml` and benilla-ui's `script/worldmap.rs` bindings.
//!
//! A seed and a feed, the quest-log seam shape:
//! - [`seed_world_map_catalog`] (at the world-entry edge, before any interface file runs — 2240):
//!   builds the static **catalog** from `WorldMapArea` × `AreaTable` × `WorldMapContinent` ×
//!   `Map` × the `.zmp` bitmaps and pushes it into the engine. Every ordering/naming rule is the
//!   reference's own: continents in WorldMapArea **file
//!   order** (Kalimdor, then EK — the `0x4a5d00` builder's walk), displayed under their
//!   `Map.dbc` localized names ("Eastern Kingdoms", not the art folder's "Azeroth"); zones
//!   sorted case-insensitively by AreaTable localized name (the `0x4a6390` comparator's
//!   `SStrCmpI`); the zone grids remapped from raw AreaTable ids by the client's one-hop parent
//!   rollup + (mapId, areaId) match — here straight to 1-based zone indices. Beside that it
//!   builds the **orphan list** ([`DirectAreaEntry`]) — the instance maps, which belong to no
//!   continent and so appear in neither list above; the reference fills the same second container
//!   in the same walk (`0x4a5d00`), and it is what the `−2` direct-area selection resolves
//!   against.
//! - [`feed_world_map`] (every frame): reads the engine-owned selection back, projects the
//!   player's world position onto the displayed map via [`benilla_world::map_proj`] (world sheet:
//!   WorldMapContinent constants; continent/zone: the WorldMapArea rect lerp; an instance map:
//!   that row's own rect), resolves where the player's own position puts the selection
//!   ([`resolve_player_selection`], the reference's `0x4a6650` — continent × zone, then the
//!   orphan list) from `CurrentArea` through the AreaTable parent chain, and pushes the trio +
//!   facing. (The client matches its zone-level area global directly; our MCNK `CurrentArea` is
//!   the leaf sub-area, so the parent walk lands on the same zone.) It also runs the **landmark
//!   pass** — the reference's `0x4a67a0` builder
//!   ([`landmark_gates_pass`]): the `AreaPOI.dbc` rows the displayed level admits,
//!   then the guard-directions marker. That pass is keyed rather than per-frame ([`LandmarkKey`]),
//!   because the reference rebuilds it on events, not on a clock.
//!
//! …and one dev affordance beside them, [`dev_map_jump`]: **Alt+click the map to go there.**

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

/// The app-side mirror of the pushed catalog — the projection data (rects + world-sheet
/// constants) per continent/zone, in the SAME order as the engine's copy (indices must agree).
/// The AreaTable itself is the shared [`crate::area::AreaTableRes`].
#[derive(Resource)]
pub(crate) struct WorldMapUiData {
    continents: Vec<ContinentEntry>,
    /// The **orphan list** — the instance maps, which are no continent's child and so appear in
    /// neither list above. `0x4a5d00`'s third array, in `WorldMapArea.dbc` file order with no
    /// sort; the rows the `−2` direct-area selection resolves against (see [`DirectAreaEntry`]).
    /// It also carries what the engine is pushed, so the two copies cannot drift apart.
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

/// One orphan row: an instance map (a battleground, a dungeon) selected **directly** rather than
/// through the continent/zone pair. In 5875 there are exactly three, all battlegrounds.
struct DirectAreaEntry {
    /// The `WorldMapArea` row **ID** — the value the reference's third selection cell
    /// `[0x845074]` holds verbatim (`0x4a6717 mov edx,[esi]`). Not a position in this list:
    /// nothing indexes it, the resolver looks rows up by id.
    id: u32,
    /// The row's `Map.dbc` id. The resolver matches the player's own map against this
    /// (`0x4a66f6`), and the projection admits a body only when its map is this one
    /// (`0x4a7437`) — which is what makes a teammate elsewhere answer the (0,0) hide sentinel.
    map_id: u32,
    /// The row's own loc rect — the window `0x4a7360`'s direct-area leg projects through, the
    /// same tail the zone case uses.
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

/// One `WorldMapArea` row as the engine sees it — a zone, a city, or an instance map: the client
/// makes no structural distinction (the same `0x4a6cf0` id lookup answers `GetMapInfo` for all
/// three, and the same overlay gate reveals their art), so neither do we.
///
/// `name` is passed in because the zone list sorts on it before the views are built.
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
        // The row's discovery overlays: WorldMapOverlay rows joined by its WMA id, their reveal
        // bits = each covered area's AreaTable exploreFlag.
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
                // The zone-level hover's label inside that rect: the FIRST area slot's AreaTable
                // name — `0x4a7fa0` reads `+0x8` only; a slot that resolves
                // to no row makes the overlay invisible to the hover, never to the draw.
                area_name: areas.name(o.area_id[0]).map(str::to_string),
            })
            .collect(),
    }
}

/// Build the catalog off the patch chain — the pure half of [`seed_world_map_catalog`], split out
/// so a test can drive the REAL `WorldMapArea` × `AreaTable` × `WorldMapContinent` × `Map` ×
/// `.zmp` build rather than a hand-written stand-in. A fixture catalog can only ever prove the
/// engine's arithmetic; what the player hovers is this, and the two were never joined before
/// ([`super::ui_script::world_map_tests`]'s real-data hover).
///
/// `None` when a DBC fails to load — the caller disables the map window and says so.
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

    // Continents: the areaId==0 rows in WorldMapArea FILE order — the 0x4a5d00 builder's walk,
    // which defines the Lua continent index (Kalimdor, then EK in 5875).
    let cont_rows: Vec<(u32, &WorldMapArea)> = wma.iter().filter(|(_, a)| a.area_id == 0).collect();

    let mut entries = Vec::with_capacity(cont_rows.len());
    let mut views = Vec::with_capacity(cont_rows.len());
    for (_, cont) in cont_rows {
        // Zones: this continent's areaId!=0 rows, display-named from AreaTable, sorted
        // case-insensitively by that name (the 0x4a6390 comparator's SStrCmpI — this order IS
        // the Lua zone index). Instance rows (BGs) live on other map ids and drop out naturally.
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
        // The continent's rect on the world sheet: the 0x4a5d00 kernel over the WorldMapContinent
        // tile bounds (the world-level click's AABB — disjoint per continent, unlike the art
        // rects).
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

        // The continent's area bitmap, remapped from raw AreaTable ids to 1-based zone indices —
        // the client's load-time remap (one-hop parent rollup, then the (mapId, areaId) match;
        // `0x4a6070`), fused with its zone-index resolution since our engine consumes indices.
        let zone_grid: Vec<u16> = match load_zone_map(&mut *chain, &cont.name) {
            Ok(grid) => grid
                .iter()
                .map(|&raw| {
                    let mut area_id = raw;
                    if let Some(row) = areas.get(area_id) {
                        if row.zone_id != 0 {
                            area_id = row.zone_id; // one-hop rollup, verbatim (not a full walk)
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
                // The client tolerates a missing bitmap (grid stays zero → hover/click inert).
                warn!("world map: no zone bitmap for {}: {e:#}", cont.name);
                Vec::new()
            }
        };

        views.push(WorldMapContinentView {
            // Display name = Map.dbc's localized MapName ("Eastern Kingdoms"), never the art
            // folder (`0x4a65a0`); the folder stays the map_file.
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

    // ── The THIRD list: the orphans — the instance maps. `0x4a5d00` fills a container
    // beside the continents with every row that has `areaID != 0` and whose mapID matches **no**
    // continent record's mapID, in `WorldMapArea.dbc` **file order with no sort** — the passes at
    // `0x4a6130` (count) and `0x4a61c9` (fill), whose predicate is the exact complement of the
    // child rule above. In 5875 that is Alterac Valley (WMA 401, map 30), Warsong Gulch (443,
    // 489) and Arathi Basin (461, 529), and nothing else.
    //
    // The `areaID != 0` gate is carried although it is **provably inert on the shipped data**:
    // every `areaID == 0` row becomes a continent record and so matches its own mapID at
    // `0x4a6170`. It is the reference's predicate, not a filter we inferred from the data.
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

/// The built catalog, kept for the life of the process. A static `WorldMapArea` × `AreaTable` ×
/// `WorldMapContinent` × `Map` × `.zmp` walk whose answer cannot change, so the second login
/// re-seeds its VM from here rather than reading the chain again.
///
/// `pub(crate)` because the seam it feeds is what
/// [`crate::ui_script::world_entry_tests::an_addon_reads_the_map_catalog_at_file_scope`] asserts:
/// the test plants a catalog and drives the entry edge, which is the ordering question without
/// the DBC walk (the walk itself is covered against the real chain by
/// `world_map_tests::the_real_feralas_catalog_names_dire_maul_under_the_cursor`).
///
/// The **continent** half only: the orphan half of the same build rides [`WorldMapUiData`], where
/// each instance map's row already sits beside its projection rect.
#[derive(Resource)]
pub(crate) struct WorldMapCatalog(pub(crate) Vec<WorldMapContinentView>);

/// **The map catalog goes into the VM before a single interface file runs**.
///
/// Called from [`crate::ui_script::load_ingame_ui_on_world_entry`], beside the CVar table, the
/// realm name and the addon-info array, and for exactly their reason: that edge mints a fresh VM
/// and runs FrameXML and every addon inside ONE call, so anything pushed from an `Update` system
/// lands after the whole interface has already asked its questions. This one was pushed there —
/// measured live, the addon file scope read `conts=0 zones(1)=0 zones(2)=0` and the catalog
/// arrived 210 ms later, on both logins of a round trip.
///
/// `GetMapContinents`/`GetMapZones` are answered off this catalog, and they are **file-scope**
/// reads in the corpus: Astrolabe — Questie's and Cartographer's positioning library — builds its
/// whole continent → zone table inside `AceLibrary:Register`'s synchronous `activate`, from those
/// two calls. Built from nothing, that table has no numeric zone entries at all, and every icon
/// placement afterwards indexes a nil zone (`attempt to index local 'zoneData'`).
///
/// In the reference the question has no timing: the catalog is DBC data the client has held since
/// load, and the getters read it whenever they are asked.
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
    // The orphan half of the same build, pushed at the same edge and for the same reason (the
    // reference fills both containers in one walk). It rides [`WorldMapUiData`] rather than
    // [`WorldMapCatalog`] because that is where the rows and their projection rects already live
    // together — one list, one order, nothing to drift.
    if let Some(data) = world.get_resource::<WorldMapUiData>() {
        script.set_world_map_direct_areas(
            data.direct
                .iter()
                .map(|d| (d.id, d.view.clone()))
                .collect::<Vec<_>>(),
        );
    }
}

/// [`build_catalog`] over the resources the app holds — `None` when the patch chain or either DBC
/// catalog is missing, which in a real run cannot happen at this edge: all three are `Startup`
/// systems and the initial state transition is after `PostStartup`. A bare test
/// world takes the `None`.
fn build_catalog_from_world(world: &World) -> Option<(Vec<WorldMapContinentView>, WorldMapUiData)> {
    let assets = world.get_resource::<WorldAssets>()?;
    let maps = world.get_resource::<MapCatalogRes>()?;
    let areas = world.get_resource::<crate::area::AreaTableRes>()?;
    let mut chain = assets.chain.lock_recover();
    build_catalog(&mut chain, &areas.0, maps)
}

/// Which of the map's levels is displayed — the reference's three selection cells
/// `[0x84506c]`/`[0x845070]`/`[0x845074]` reduced to the thing every landmark gate actually
/// branches on.
///
/// **The direct-area state answers [`MapLevel::Zone`], and that is the reference's own law rather
/// than an approximation**. Both gates that read
/// this enum test "zone **or** orphan": the AreaPOI level flag falls into the zone-level
/// `test [rec+0x20],0x4` at `-2` (`0x4a79de cmp eax,-2; jne`), and `GetMapLandmarkInfo`'s
/// `textureIndex` substitution is reached when the zone cell is `-1` **and** the continent is `-2`
/// (`0x4a8856`/`0x4a885f` both fall through to `0x4a8868`). A fourth variant would be two arms
/// that copy `Zone` exactly — so the fact is recorded here instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MapLevel {
    /// Both continents on one sheet (the reference's `continent == -1`).
    World,
    /// One continent, no zone selected (`continent >= 0, zone == -1`).
    Continent,
    /// One zone (`continent >= 0, zone >= 0`) — or an instance map (`continent == -2`).
    Zone,
}

impl MapLevel {
    /// The engine's selection ([`UiScript::world_map_selection`]) — our 1-based
    /// `(continent, zone)` with `0` for "whole", plus the direct-area row id — mapped onto the
    /// reference's levels. The direct cell is read FIRST: it shares the `(0, 0)` pair with the
    /// world sheet and overrides it.
    fn of((continent, zone, direct): (u32, u32, Option<u32>)) -> Self {
        match (continent, zone, direct) {
            (_, _, Some(_)) => Self::Zone,
            (0, _, None) => Self::World,
            (_, 0, None) => Self::Continent,
            _ => Self::Zone,
        }
    }
}

/// The inputs the landmark list is a pure function of — the rebuild edge.
///
/// The reference does not rebuild per frame: `0x4a67a0` runs from thirteen callers, and they are
/// exactly this set — a map selection change, a world-state push (`0x48fa0d`), a
/// `PLAYER_EXPLORED_ZONES` descriptor change (`0x4a6477`, registered as an UpdateFields callback
/// on byte offset `0xe6c`), and the three gossip-marker set/clear sites. Keying on them keeps our
/// pass event-driven the same way, which is what stops a 339-row walk and ~40 string clones from
/// running 60 times a second with nothing changed.
///
/// `explored` is held whole rather than hashed: it is 64 dwords, it is compared far more often
/// than it is stored, and a hash would trade an exact answer for a collision risk on the one
/// input that silently hides map icons when it is wrong.
#[derive(PartialEq, Eq)]
struct LandmarkKey {
    /// All three selection cells — the direct-area one included, or an instance map's landmark
    /// list would never rebuild when the selection moved onto it.
    selection: (u32, u32, Option<u32>),
    map: u32,
    states: u64,
    /// Owned rather than borrowed, and compared before it is ever cloned — see
    /// [`LandmarkKey::matches`]. Cloning it every frame to build a throwaway key would have cost
    /// the allocation this whole mechanism exists to avoid.
    explored: Vec<u32>,
    /// The guard-directions marker's identity — `(continent, pos, icon)`. Its position is held as
    /// raw `f32` bits: "the same marker" here means the same wire packet, and the coordinates are
    /// copied from it rather than computed, so a bit compare is the exact question.
    marker: Option<(u32, [u32; 3], u32)>,
}

impl LandmarkKey {
    /// Is this key still the current one? Takes `explored` by slice so the common case — nothing
    /// changed — compares without allocating.
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

/// The builder's near-zero skip (`0x4a6868`/`0x4a687a`): a landmark whose projected UV is `0` on
/// **both** axes is dropped.
///
/// That is the level/continent filter, not a paranoia check. [`map_proj::zone_uv`] answers
/// `(0, 0)` for a position outside the displayed rect or on the wrong continent, and `world_uv`'s
/// caller answers `None` for a map with no `WorldMapContinent` row — so "did it project to zero"
/// IS "does this POI belong on the map that is showing". Both axes, never either: a POI genuinely
/// on the top-left edge of a rect (`u = 0`, `v = 0.5`) survives, as it does in the reference.
///
/// The epsilon is the binary's own `2.384e-7`.
fn is_degenerate(uv: (f32, f32)) -> bool {
    const EPS: f32 = 2.384e-7;
    uv.0.abs() < EPS && uv.1.abs() < EPS
}

/// The three gates `0x4a67a0`'s AreaPOI walk applies, in the reference's order.
///
/// 1. **The level flag** (`0x4a79b0`'s fall-through chain, `0x4a79cb`–`0x4a7a05`): a row must
///    carry `0x04` to show at zone level, `0x08` at continent level, and **both `0x10` and
///    `0x08`** at world level — the chain is not a switch, so the world leg falls through the
///    continent leg. The shipped data agrees and could have disagreed: of the eleven `Flags`
///    values in `AreaPOI.dbc`, the only two carrying `0x10` (`0x1d`, `0x98`) also carry `0x08`.
/// 2. **Exploration** (`0x4a6890`–`0x4a68f3`): a row whose `AreaID` is `> 0` (signed — `-1`
///    arrives here as `u32::MAX` and means continent-wide) must resolve in `AreaTable`, and if
///    that row's `ExplorationLevel` is `>= 0` the player must have discovered it. This is why
///    Stormwind's icon is on the map from the first login (its row is continent-wide) while a
///    tower in a zone you have never walked is not.
/// 3. **World state** (`0x4a6903`): a row carrying a `WorldStateID` shows only while
///    [`crate::world_state::WorldStates`] reads that key non-zero. This is the whole Eastern
///    Plaguelands tower mechanism — rows 1749–1777 are gated on states 2352–2379, one row per
///    (tower, owner, contested/progressing) combination, so the server flipping a state swaps
///    which icon exists rather than editing one.
///
/// **Not tested here, and deliberately:** `ContinentID`, `Importance`, `FactionID`, `Icon`. The
/// reference's loop reads none of them — the continent filter is the projection (see
/// [`is_degenerate`]), and `Importance` gates nothing on the world map at all.
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
    // `AreaID > 0`, signed. A row the table does not carry cannot be tested for exploration, so
    // it passes — the reference's own bounds check on `[0xc0e048]`/`[0xc0e04c]` guards the read,
    // not the landmark. (Moot on 5875 data: all 171 gated rows resolve.)
    if poi.area_id as i32 > 0 {
        if let Some(area) = areas.get(poi.area_id) {
            if area.exploration_level >= 0 && !explored_bit(explored, area.explore_flag) {
                return false;
            }
        }
    }
    poi.world_state_id == 0 || world_states.get(poi.world_state_id) != 0
}

/// `PLAYER_EXPLORED_ZONES` bit `bit` — the reference's `0x4a9a40`, which indexes the descriptor
/// bitfield bytewise (`[base + bit/8] & (1 << (bit & 7))`). Our slots are the same bits as
/// little-endian dwords, so the dword form below is the same test.
fn explored_bit(explored: &[u32], bit: u32) -> bool {
    let (slot, within) = (bit as usize / 32, bit % 32);
    explored.get(slot).is_some_and(|w| w & (1 << within) != 0)
}

/// `GetMapLandmarkInfo`'s `textureIndex` leg (`0x4a8848`–`0x4a8877`): the row's own `Icon`, except
/// that at **zone** level a row without `Flags & 0x80` is substituted with the constant **15**.
///
/// The substitution and the world-state gate are one designed mechanism, and the shipped data
/// proves it: across all 339 rows, `Flags & 0x80` is set exactly when `WorldStateID != 0`. A
/// battleground node or an Eastern Plaguelands tower whose icon *means* something — Alliance vs
/// Horde vs contested — has to escape a substitution that would flatten all of them to one cell.
fn landmark_texture_index(poi: &benilla_formats::AreaPoi, level: MapLevel) -> u32 {
    /// The generic zone-level POI cell of `Interface\Minimap\POIIcons`.
    const ZONE_SUBSTITUTE: u32 = 15;
    match poi.flags & 0x80 != 0 || level != MapLevel::Zone {
        true => poi.icon,
        false => ZONE_SUBSTITUTE,
    }
}

/// `0x4a6650`'s answer — which map the player's own position selects — in the Lua-visible
/// encoding. Its two legs are mutually exclusive by construction: the resolver runs the continent
/// loop first and reaches the orphan loop only on a miss (`0x4a66c3`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct PlayerSelection {
    /// The continent/zone leg: the 1-based pair, with **zone 0 meaning the continent map** (the
    /// reference's `SetMap(continent, −1)` as `GetCurrentMapZone` reads it back). `None` = no
    /// continent record carried the player's map.
    zone: Option<(u32, u32)>,
    /// The orphan leg: the `WorldMapArea` row **ID** whose mapID is the player's own map.
    direct: Option<u32>,
}

/// `SetMapToCurrentZone`'s resolver (`0x4a7e20` is `call 0x4a6650`, and the engine's own
/// world-enter sync `0x4947ac` calls the same function — there is no second resolver).
///
/// Three exits, in the reference's own precedence:
///
/// 1. **A continent whose mapID is the player's**, and then its child zones — whose two halves
///    fail SEPARATELY (see the call site in [`feed_world_map`] for the byte citation).
/// 2. **Only on a continent miss**, the orphan list, matched on `row.mapID == playerMapId`, first
///    match wins: the direct-area selection `SetMap(-2, row.id)` (`0x4a6717`–`0x4a671e`). This is
///    the leg a battleground takes — map 489 is no continent's, so nothing above can match.
/// 3. Neither: the world view (both cells `None`), which is also what an empty orphan list gives.
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

/// The engine-side world-enter sync's gate and answer — `0x494780`'s `old == 0` leg into the
/// resolver `0x4a6650`. `None` = do not sync this frame; `Some(sel)` = apply `sel` and never
/// sync again this session.
///
/// Two things it has to get right, both from the bytes:
///
/// - **The trigger is the zone id becoming known, not the frame count.** `0x67e510` bails before
///   `0x494780` whenever the resolved zone id is 0, so the reference's sync cannot fire before
///   the player's area is real. `top_zone.is_some()` is that same precondition; without it we
///   would sync on frame 1 to the bare continent and — since the reference never re-syncs — stay
///   there.
/// - **Every exit of `0x4a6650` writes the selection**, including its two failure legs
///   (`0x4a667e`/`0x4a670b` ⇒ `SetMap(−1, −1)`). So an unresolvable player — a map in neither
///   list — still counts as synced, at the world view, rather than re-arming the gate every
///   frame. That is the all-`None` [`PlayerSelection`]: it IS the reference's `(−1, −1)`, which
///   `GetCurrentMapContinent` reads back to Lua as `0`.
///
/// A player logging straight into a battleground takes the *direct* leg here, which is why this
/// hands the whole answer on rather than the pair: the sync happens once per session, so dropping
/// the third cell would strand such a session on the world sheet for its whole life.
fn world_enter_selection(
    synced: bool,
    top_zone: Option<u32>,
    player: PlayerSelection,
) -> Option<PlayerSelection> {
    (!synced && top_zone.is_some()).then_some(player)
}

/// Per frame: selection read-back → projection → feed push (see the module doc).
/// One projection law for every blip on the DISPLAYED map: world-sheet mode (selection
/// `(0, _)`) projects through the POSITION's own map's continent constants; continent and zone
/// mode through the selected rect, gated to that continent's map; **direct-area mode through the
/// selected row's own rect**, gated to that row's map. Off-map — the wrong continent, a body in
/// another instance, outside the rect — is `None`, the reference's `(0, 0)` hide sentinel once it
/// reaches Lua.
///
/// The direct arm is `0x4a7360`'s own (`0x4a73e0 jl 0x4a7414`): it reads `[0x845074]`, resolves
/// the row, requires `[row+0x4] == mapId` (`0x4a7437`) and then runs the **same tail the zone case
/// uses** — it never touches the continent record array. The gate is what makes it right for
/// every caller and not just the player: `0x4a7870` passes the *unit's own* map id, so a
/// teammate standing in the battleground projects and one who is elsewhere answers `(0, 0)`.
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

/// [`feed_world_map`]'s memos, bundled behind ONE [`crate::ui_script::VmMemo`] because the feed
/// sits at Bevy's system-parameter ceiling. Keeping the memo on the outside rather than on each
/// field is the point: a login is a new VM, and the whole bundle resets with it in one place —
/// there is no field that can be added later and quietly outlive the session it is memory about.
#[derive(Default)]
struct FeedMemos {
    /// The last pushed `PLAYER_EXPLORED_ZONES` bitset.
    explored: Option<Vec<u32>>,
    /// The inputs the last landmark rebuild was keyed on.
    landmarks: Option<LandmarkKey>,
    /// The reference's `old == 0` gate on the engine-side map sync — see the call site.
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
    // The party blips' two position sources: the roster + the wire's per-member stats, and
    // the streamed-entity index that beats them when a member is actually in the world with us.
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
    // The discovery bitset (PLAYER_EXPLORED_ZONES, PRIVATE — only our own avatar carries it):
    // push on change (including the first stream-in); the engine setter queues the
    // WORLD_MAP_UPDATE that fills newly explored art into an open zone map. It is also the
    // landmark pass's exploration gate below, so it is resolved before either use.
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

    // The player's (continent, zone) — CurrentArea's MCNK areaId walked up the AreaTable parent
    // chain to its top-level zone, matched against the displayed catalog (1-based indices).
    //
    // **The two halves fail SEPARATELY, and that is the reference's law, not a convenience.**
    // `SetMapToCurrentZone 0x4a7e20`'s resolver `0x4a6650` matches the player's map id against
    // each continent record's `+0x00`, then walks that record's sorted child-zone array for the
    // zone whose WMA areaID equals the player's current-area global — and **"no zone match ⇒
    // `SetMap(continent, −1)`"**, i.e. the CONTINENT map, with only "no continent match" falling
    // through to the orphan list and then `SetMap(−1, −1)` = the world view
    // (`GetCurrentMapZone 0x4a7f00` is `DAT_00845070 + 1`, so that −1 reads back to Lua as **0**).
    //
    // We used to resolve both or neither, so a zone the catalog has no row for — and, before the
    // area feed answers, a zone we simply do not know yet — slammed the selection to the WORLD
    // view instead of the player's continent. That matters beyond the picture: at the world level
    // `GetPlayerMapPosition` answers a world-SHEET uv (`0x4a7360`'s step-2 continent formula), and
    // every consumer that assumes a zone uv — Astrolabe, and so every addon built on it — silently
    // mis-scales it.
    //
    // And when NO continent matched, the resolver falls through to its third leg: the orphan
    // list, i.e. the instance maps (`0x4a66c3`). Inside a battleground that is the only leg that
    // can answer — map 489 is no continent's child — and it selects the battleground's own
    // `WorldMapArea` row directly, `SetMap(-2, row.id)`. Both halves travel to the engine
    // together; with only the pair, `GetMapInfo()` answers nil in there and the stock
    // `Blizzard_BattlefieldMinimap.lua:83-86` returns on the fourth line of its update, which is
    // a battle map and a world map that draw nothing at all.
    let top_zone = world.area().and_then(|aid| areas.0.top_zone(aid));
    let player_sel = resolve_player_selection(&data, map.0, top_zone);

    // ── First world-enter: the ENGINE selects the player's zone, with no Lua in the loop.
    //
    // `0x494780` — the zone updater we already transcribe verbatim in `crate::area` for its
    // ZONE_CHANGED event election — carries one more effect that election note called "an
    // unrelated side call": when the cached zone id was **0** (the zeroed BSS, i.e. the first
    // world-enter of the session) it also calls `0x4a6650`, the same player→(continent, zone)
    // resolver `SetMapToCurrentZone` uses, and hands it to the `SetMap` setter `0x4a67a0`. So a
    // freshly-logged-in reference client is ALREADY on the player's own zone map before a single
    // line of FrameXML or addon Lua has asked for it.
    //
    // We modelled the election and dropped the side call, so our selection stayed at the world
    // level until something opened the map. That is not cosmetic: at the world level
    // `GetPlayerMapPosition` answers a world-SHEET uv (`0x4a7360` step 2) instead of a zone uv,
    // and every addon built on Astrolabe scales it with the *zone's* yard dimensions — Questie's
    // quest arrow pointed ~100 yards off the turn-in, on a client where the reference is exact.
    // Astrolabe's own rescue cannot save it: that is gated on `(x <= 0 and y <= 0)`, and a
    // world-sheet uv is a perfectly non-zero pair.
    //
    // **The gate is the zone id becoming known, not the frame count.** The reference reaches
    // `0x494780` only with a nonzero resolved zone (`0x67e510` bails on 0), so its sync cannot
    // fire before the player's area is real; ours must not either, or `resolve_player_zone`
    // answers the bare continent and nothing re-asks. `top_zone.is_some()` is that same
    // precondition. Per **VM**, not per process: a new login is a new session's first enter.
    {
        let memo = memo.get(&script);
        if let Some(sel) = world_enter_selection(memo.map_synced, top_zone, player_sel) {
            memo.map_synced = true;
            let (c, z) = sel.zone.unwrap_or((0, 0));
            script.sync_world_map_to_player_zone(c, z, sel.direct);
        }
    }

    // The player's UV on the DISPLAYED map. Off-map (wrong continent, another instance, outside
    // the rect) resolves to None/(0,0) — the reference's hide-the-blip sentinel.
    let selection = script.world_map_selection();
    // One projection law for every blip on the displayed map (the player now, the corpse below,
    // the battleground teammates in `ui_battlefield_positions`): [`project_on_displayed`].
    let project =
        |pos_map: u32, px: f32, py: f32| project_on_displayed(&data, selection, pos_map, px, py);
    let uv = project(map.0, wx, wy);
    // The corpse marker: the query answer's DISPLAY position/map (a dungeon
    // corpse projects at its entrance — the server rewrote it). `zone_uv`'s outside-the-rect
    // (0,0) and the None here both land on the reference's hide sentinel.
    let corpse_uv = death_net.corpse.and_then(|cp| {
        project(
            u32::try_from(cp.display_map).unwrap_or(u32::MAX),
            cp.position[0],
            cp.position[1],
        )
    });

    // ── The map's POI icons (`GetNumMapLandmarks`) — the reference's landmark-list builder
    // `0x4a67a0`, rebuilt only when one of its inputs moves (see [`LandmarkKey`]).
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
        // The DBC rows first, in file order, each through the builder's gate chain — that order
        // IS the Lua landmark index (`0x4a6819`'s walk over `[0xc0e054]`).
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
        // Then the guard-directions marker (`crate::poi_marker`), appended last exactly as
        // `0x4a69c7` does and — like it — exempt from every gate above: no level flag, no
        // exploration bit, no world state, its `Icon` verbatim with no level-15 substitution
        // (the `+0x10 == 1` element). Only the same
        // non-degenerate projection test applies.
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

    // The party slots' blips (report B320) — `party1..4` in the same order the frames and the
    // unit tokens use (`GroupState::party_slots`).
    //
    // Two position sources, the minimap's own law (`minimap::blips::party_member_pos`): a member
    // who is STREAMED has a real transform, and anyone else has the `(x, y)` their
    // `SMSG_PARTY_MEMBER_STATS` carried — an `i16` pair, so a far member's blip is yard-accurate
    // and no better, which is all the reference has for them either.
    //
    // Which MAP that position belongs to is the part the stats packet does not say: it carries a
    // zone, not a map id. So the member's zone is looked up in the catalog and the continent that
    // owns it supplies the map; a zone we cannot place (an instance, a zone missing from the
    // catalog) falls back to ours, which is right for the overwhelmingly common case of a party
    // spread across one continent and merely projects off-rect — the (0,0) hide — when it is not.
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

    // The raid roster's blips (1980, the stock map's `raid1..raid40` arm over `WorldMapRaid1..40`):
    // the order the RaidFrame's own roster uses — ourselves first, then the members as listed
    // (`ui_party::feed::raid_roster`) — so `raidN` here is `raidN` there. The reference skips
    // `UnitIsUnit(unit, "player")` in Lua, so our own slot carries the player's UV rather than a
    // hole that would shift every index after it.
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
    // The orphan leg of the same resolver answer, beside the pair: `SetMapToCurrentZone` reads
    // both back, and the stock battlefield minimap calls it itself on `PLAYER_ENTERING_WORLD`
    // while it is shown — the call that lands a player zoning into a battleground on the
    // battleground's own map.
    script.set_world_map_player_direct_area(player_sel.direct);
}

/// **Alt+click the world map to go there** — the dev jump.
///
/// The inverse of the blip projection above: the click's UV inside the map art
/// ([`UiScript::world_map_uv_at`], the reference's own normalization) run back through the
/// displayed rect to a world `(x, y)`, sent as vmangos's `.go xy x y <mapid>` — the **no-Z** form,
/// so the server resolves the ground (`GetWaterOrGroundLevel`) instead of us guessing a height off
/// a 2-D sheet. A jump to another continent's sheet is a cross-map worldport, handled like any
/// other ([`crate::player::wire_in`]).
///
/// **Zone and continent sheets only.** At the *world* level (both continents on one sheet) the
/// client's own UV→world law `0x4a7100` is not the inverse of its world→UV law — a confirmed,
/// reproduced anomaly (see [`map_proj::world_click_world`]) — so a click there
/// would land somewhere real but wrong by ~1.33× the sheet offset. Rather than invent a corrected
/// inverse the reference doesn't have, the jump declines and says to zoom in first.
///
/// The click is NOT consumed: the faithful path (`WorldMapButton_OnClick` → `ProcessMapClick`)
/// also runs and drills into the clicked zone, which is what you want anyway — you arrive, and the
/// map is showing where you arrived. Adding a modifier fork to the reference's click law to
/// suppress that would be a dev affordance rewriting a faithful one.
fn dev_map_jump(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    ui_scale: Res<crate::ui_script::UiScaleCvar>,
    script: Option<NonSendMut<UiScript>>,
    data: Option<Res<WorldMapUiData>>,
    net: Res<crate::net::NetCommands>,
) {
    // A dev affordance living in a gameplay module — it names no dev root, so nothing about it
    // fails to compile in a player build, and it shipped in 1174's. Alt-click is
    // free-fly's closest sibling: it moves the player's body across the continent.
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
    // Window px (y-down) → the VM's y-up 768-virtual units, exactly as the pointer feed converts
    // them (`ui_script::input`) — `world_map_uv_at` hit-tests in that space.
    let s = crate::ui_script::seam_scale(window.height(), ui_scale.0);
    let Some((u, v)) = script.world_map_uv_at(cursor.x / s, (window.height() - cursor.y) / s)
    else {
        return;
    };
    let (c, z, direct) = script.world_map_selection();
    // An instance map is a rect like any other — the same `0x4a7100` zone-mode inverse — so the
    // jump works off a battleground map too; only the world sheet has no exact inverse.
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
    // `zone_world` lerps BOTH axes by its single `t` (the binary's own shape) — so it is called
    // once per axis, which is how the reference's own callers consume it.
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

/// The world-map data feed (decision 0203 phase 2) — see the module doc.
pub(crate) struct WorldMapUiPlugin;

impl Plugin for WorldMapUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // After the script tick (UiInput), like the minimap's zone feed: the projection
                // for a selection changed THIS tick lands next tick — invisible at frame rate.
                feed_world_map.after(UiInput),
                // Same slot, and for the same reason from the other side: the jump hit-tests
                // against the frame rects THIS tick's resolve produced.
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

    /// The level-flag chain, in the shape `0x4a79b0` has: `0x04` for zone, `0x08` for continent,
    /// and world demanding **both** `0x10` and `0x08` because the tests fall through rather than
    /// switch. A row with `0x10` alone is invisible everywhere — the case the shipped data can't
    /// exhibit (no 5875 row sets `0x10` without `0x08`) and the one a switch would get wrong.
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

        // `0x10` alone: the fall-through means world level still wants `0x08`, so nowhere.
        assert!(!pass(0x10, MapLevel::World));
        assert!(!pass(0x10, MapLevel::Continent));
        assert!(!pass(0x10, MapLevel::Zone));
    }

    /// The world-state gate — the Eastern Plaguelands tower mechanism. A row with no
    /// `WorldStateID` is ungated; a row with one appears exactly while that key reads non-zero,
    /// so flipping the state swaps which of the sibling rows exists.
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

        // The server flips the tower: the Alliance row's state goes to 0, the Horde row's to 1.
        states.write(&[(2372, 0), (2373, 1)]);
        assert!(!pass(&alliance, &states));
        assert!(pass(&horde, &states));
    }

    /// The exploration gate and its two exemptions: a continent-wide row (`AreaID` `-1`, which
    /// arrives as `u32::MAX`) and a row whose area has `ExplorationLevel < 0` are never gated;
    /// everything else waits for the bit.
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

    /// `GetMapLandmarkInfo`'s texture leg: the level-15 substitution bites only at zone level and
    /// only on a row without `Flags & 0x80` — which is exactly the rows with no live world state.
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

    /// The near-zero skip is on **both** axes, not either — a POI on the top edge of a rect
    /// (`v == 0`) stays on the map.
    #[test]
    fn only_a_both_axes_zero_projection_is_dropped() {
        assert!(is_degenerate((0.0, 0.0)));
        assert!(is_degenerate((1e-8, -1e-8)));
        assert!(!is_degenerate((0.0, 0.5)));
        assert!(!is_degenerate((0.5, 0.0)));
        assert!(!is_degenerate((1e-6, 0.0)));
    }

    /// The engine's three selection cells — our `(continent, zone)` pair with `0` = whole, plus
    /// the direct-area id — onto the reference's levels.
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
        // An instance map takes the ZONE arms of both gates it feeds — `0x4a79de`'s level flag
        // and `0x4a8856`'s forced textureIndex 15 — even though it shares the world sheet's pair.
        assert_eq!(MapLevel::of((0, 0, Some(443))), MapLevel::Zone);
    }

    /// The REAL `AreaPOI.dbc`, run through the whole gate chain the way the feed does — the
    /// report B190 asks about, answered off the shipped table rather than a fixture.
    ///
    /// Two claims: the Eastern Plaguelands towers exist as world-state-gated zone-level rows and
    /// change with the state; and the capitals are continent-level rows that need no exploration
    /// bit (which is why a fresh character sees Stormwind on the Eastern Kingdoms map).
    /// Skips without client data.
    #[test]
    fn the_real_table_gates_the_epl_towers_and_the_city_icons() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let pois = benilla_formats::load_area_poi_catalog(&mut chain).expect("AreaPOI");
        let areas = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable");
        let nothing_explored = vec![0u32; 64];

        // ── The towers. Northpass Tower's Alliance-held row (1768, state 2372).
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
            "no world states received — no tower icon (this IS report B190)"
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

        // ── The capitals: continent-level, and exploration-exempt.
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

    /// File order is the landmark index: the catalog must hand rows back in the DBC's own record
    /// order, not a hash permutation (which would also make the feed's change-diff fire forever).
    /// Skips without client data.
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

    /// Azeroth (map 0) with Elwynn at catalog index 9 (→ Lua zone 10), plus Kalimdor (map 1) —
    /// and one orphan, Warsong Gulch's real row (`WorldMapArea` 443 on map 489), which is in
    /// neither continent's list because no continent record carries its map.
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

    /// **The two halves fail separately** — `SetMapToCurrentZone`'s resolver `0x4a6650`:
    /// "no zone match ⇒ `SetMap(continent, −1)`", and only "no continent match" reaches the world
    /// view. Resolving both-or-neither sent a player
    /// whose zone the catalog cannot name — and, for the frames before the area feed answers,
    /// EVERY player — to the world level, where `GetPlayerMapPosition` answers a world-SHEET uv
    /// that every zone-uv consumer silently mis-scales.
    #[test]
    fn an_unresolved_zone_falls_back_to_the_continent_not_the_world() {
        let d = data();
        let on_continent = |c, z| PlayerSelection {
            zone: Some((c, z)),
            direct: None,
        };

        // Both halves resolve: Elwynn is continent 2, zone 10.
        assert_eq!(
            resolve_player_selection(&d, 0, Some(12)),
            on_continent(2, 10)
        );

        // The continent resolves, the zone does not — an area the catalog has no row for.
        assert_eq!(
            resolve_player_selection(&d, 0, Some(4242)),
            on_continent(2, 0),
            "SetMap(continent, -1); GetCurrentMapZone reads that back as 0"
        );

        // The area feed has not answered yet — same leg, not the world view.
        assert_eq!(
            resolve_player_selection(&d, 0, None),
            on_continent(2, 0),
            "a zone we do not know YET is still this continent"
        );

        // No continent match and no orphan either (map 389 is Ragefire Chasm — a dungeon with no
        // `WorldMapArea` row at all): both cells empty, which IS the reference's `SetMap(-1, -1)`.
        assert_eq!(
            resolve_player_selection(&d, 389, Some(12)),
            PlayerSelection::default()
        );
    }

    /// **The third leg — a battleground.** No continent record carries map 489, so `0x4a6650`'s
    /// continent loop misses and only then does the orphan loop run, matching on the row's own
    /// mapID and selecting `SetMap(-2, 443)` — the row's **ID**, not its place in the list.
    /// Everything that draws the battle map hangs off that: `GetMapInfo()` answers nil without it,
    /// and `Blizzard_BattlefieldMinimap.lua:83-86` returns before it draws a single tile.
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
        // The continent loop runs FIRST: a map that IS a continent's never reaches the orphan
        // list, even were a row to shadow it.
        assert_eq!(
            resolve_player_selection(&d, 0, Some(12)).direct,
            None,
            "a continent match short-circuits before the orphan loop"
        );

        // The projection: the direct state goes through the selected row's OWN rect (the same
        // tail the zone case uses), gated to that row's map. The fixture rect is [-1, 1] on both
        // axes, so the origin is its centre.
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
        // And the direct cell WINS over the pair it shares its `(0, 0)` with — read the pair
        // alone and an instance map projects as the world sheet.
        assert_eq!(
            project_on_displayed(&d, (0, 0, None), 489, 0.0, 0.0),
            None,
            "the world sheet has no continent for map 489 at all"
        );
        assert_eq!(MapLevel::of(sel), MapLevel::Zone);
        assert_eq!(MapLevel::of((0, 0, None)), MapLevel::World);
    }

    /// **The engine selects the player's zone itself, on the first world-enter** — the side call
    /// `0x494780` makes to `0x4a6650` when the cached zone id was 0. We transcribed that
    /// function's event election (`crate::area`) and dropped this half (`0x4947ac`, guarded by
    /// `sete al` on `[0xb4e314] == 0`, with the new zone committed *before* the call).
    ///
    /// Dropping it is why Questie's quest arrow pointed ~100 yards off the turn-in while the
    /// reference was exact: with no zone selected, `GetPlayerMapPosition` answers a world-SHEET
    /// uv, and Astrolabe scales it with the *zone's* yard dimensions. Astrolabe's own rescue
    /// cannot catch it — that is gated on `(x <= 0 and y <= 0)`, and a world-sheet uv is a
    /// perfectly non-zero pair.
    #[test]
    fn the_engine_selects_the_players_zone_on_first_world_enter() {
        let on_continent = |c, z| PlayerSelection {
            zone: Some((c, z)),
            direct: None,
        };
        // Frame 1, no area yet: the reference cannot be here at all (`0x67e510` bails before
        // `0x494780` on a zero zone id), so neither are we — syncing now would take the
        // continent leg and, since the engine never re-syncs, strand us there.
        assert_eq!(world_enter_selection(false, None, on_continent(2, 0)), None);

        // The area answers: sync, once, to the player's own zone.
        assert_eq!(
            world_enter_selection(false, Some(12), on_continent(2, 10)),
            Some(on_continent(2, 10))
        );

        // And never again. A later zone change — even cross-continent — leaves the selection
        // alone: the guard is `old == 0`, and only the world-session teardown `0x491180`
        // re-zeroes `[0xb4e314]`.
        assert_eq!(
            world_enter_selection(true, Some(40), on_continent(2, 22)),
            None
        );

        // An unresolvable player still counts as synced rather than re-arming the gate every
        // frame: EVERY exit of `0x4a6650` writes the selection, and its two failure legs write
        // `SetMap(-1, -1)` — which `GetCurrentMapContinent` reads back to Lua as `0`.
        assert_eq!(
            world_enter_selection(false, Some(12), PlayerSelection::default()),
            Some(PlayerSelection::default())
        );

        // Logging STRAIGHT INTO a battleground: the once-per-session sync carries the direct cell
        // too. Dropping it here would strand that whole session on the world sheet, because
        // nothing re-arms the gate.
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

    /// **The whole population of the direct-area state in 1.12.1**, off the shipped
    /// `WorldMapArea.dbc` (51 rows): the three battlegrounds, in the file order `0x4a5d00`'s fill
    /// pass appends in — Alterac Valley, Warsong Gulch, Arathi Basin — and nothing else. The
    /// complement rule is what produces them: `areaID != 0` (so not a continent row) and a mapID
    /// no continent record carries (so not a zone either). Skips without client data.
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

        // They are in NEITHER continent list: that is the whole reason a third state exists.
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

        // Alterac Valley is the only battleground with overlay art (3 rows keyed on WMA 401);
        // the other two have none, so a fully explored character still sees bare detail tiles.
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

    /// **The bug, end to end, on the real data**: a body standing in Warsong Gulch (map 489)
    /// resolves to the direct area, the engine selects it with no Lua in the loop, and
    /// `GetMapInfo()` answers `"WarsongGulch"` — the art-folder identifier the stock
    /// `Blizzard_BattlefieldMinimap.lua:83-86` needs before it will draw a single tile, and the
    /// name that closes `Interface\WorldMap\WarsongGulch\WarsongGulch1..12` (all twelve ship).
    /// The continent reads back `-1` (`-2 + 1`) and the zone `0`.
    ///
    /// The player's own blip comes with it: the rect centre projects to the middle of the map.
    /// Skips without client data.
    #[test]
    fn a_body_in_warsong_gulch_lands_on_the_warsong_gulch_map() {
        let Some((views, data)) = real_catalog() else {
            return;
        };

        // The resolver: map 489 is no continent's, so the orphan loop answers with the row id.
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
        // The engine-side first-world-enter sync — `0x4947ac` into the same resolver, no Lua.
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

        // The blip. The WSG row's rect centre in world coords → the middle of the map art; a body
        // on Azeroth is refused outright (the reference's `(0, 0)` hide sentinel).
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

        // THE REFRESH TRAP: a world-state push or an exploration update re-selects, and a
        // re-selection built from the continent/zone pair alone would silently drop the battle
        // map for the world view (the reference re-passes `direct != -1 ? direct : zone` at all
        // five of its in-place refresh sites).
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

        // And a continent selection clears it — `0x4a67a0`'s other legs all store `-1` there.
        script.run("SetMapZoom(1)").expect("SetMapZoom");
        assert_eq!(
            script.world_map_selection(),
            (1, 0, None),
            "selecting a continent clears the direct cell"
        );
    }
}
