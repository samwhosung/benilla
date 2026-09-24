//! The world-map bindings `WorldMapFrame.lua` reads, over a catalog and a per-frame feed the app
//! pushes (projection stays in `map_proj`); the engine owns the selection, so `SetMapZoom` reads
//! back in the same click.
//!
//! The Lua selection is the client's indices plus one (`0x4a7ed0`, `0x4a7f00`): continent 0 is the
//! world sheet, zone 0 the whole continent; continents in `WorldMapArea` file order (`0x4a5d00`),
//! zones by case-insensitive name (`0x4a6390`). An instance map is a third state (`0x4a67a0`).
//!
//! Deviation: `WORLD_MAP_UPDATE` fires at the next tick, where the reference fires it
//! synchronously, because it only triggers a repaint and one tick is invisible.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One zone of a continent; its 1-based position is the Lua zone index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapZoneView {
    /// `AreaTable.dbc`'s localized name, "Elwynn Forest".
    pub name: String,
    /// The `AreaTable.dbc` id the feed's player-zone resolution joins on.
    pub area_id: u32,
    /// The `Interface\WorldMap\<file>\` art folder, `WorldMapArea`'s internal name ("Elwynn").
    pub map_file: String,
    /// The `WorldMapArea` loc rect `(left, right, top, bottom)`; at zone level, the grid's window.
    pub loc_rect: (f32, f32, f32, f32),
    pub overlays: Vec<WorldMapOverlayView>,
}

/// A POI icon on the displayed map, projected to map UV. The reference's builder (`0x4a67a0`)
/// lists the `AreaPOI.dbc` rows past its level, exploration and world-state gates, then the
/// guard-directions marker (`SMSG_GOSSIP_POI`, `+0x10 == 1`), ungated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapLandmarkView {
    pub name: String,
    /// The second tooltip line, a live status such as "In Conflict"; empty for the guard marker.
    pub description: String,
    /// The `Interface\Minimap\POIIcons` cell in its 8×8 grid.
    pub texture_index: u32,
    /// Position on the displayed map, UV from its top-left.
    pub uv: (f32, f32),
}

/// A sub-area's map art, drawn over the base tiles once any of its explore bits is set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapOverlayView {
    /// The path prefix `Interface\WorldMap\<Zone>\<TexName>`; the Lua appends the piece number.
    pub texture: String,
    pub width: u32,
    pub height: u32,
    /// Placement in the 1002×668 detail frame, pixels from its top-left.
    pub offset_x: u32,
    pub offset_y: u32,
    /// The `AreaTable.dbc` exploreFlag bits that reveal it, any one sufficing; empty never shows.
    pub explore_bits: Vec<u32>,
    /// The DBC's `HitRect*` `(top, left, bottom, right)` in detail-frame pixels, which the
    /// zone-level hover tests with both edges inclusive (`0x4a7ffc..0x4a80ea`).
    pub hit_rect: (u32, u32, u32, u32),
    /// The hover name: the `AreaTable.dbc` name of the first area slot only (`0x4a7fa0` reads
    /// `+0x8`); `None` walks past the overlay as if the cursor were outside.
    pub area_name: Option<String>,
}

/// One continent; its 1-based position is the Lua continent index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapContinentView {
    /// `Map.dbc`'s localized MapName, the dropdown label (`0x4a65a0`), not the art folder.
    pub name: String,
    pub map_file: String,
    /// The rect on the world sheet in UV `(u0, v0, u1, v1)`, from the `WorldMapContinent` tile
    /// bounds (`0x4a5d00`); a world-level click takes the first that holds it (`0x4a7100`).
    pub world_rect: (f32, f32, f32, f32),
    /// The `WorldMapArea` loc rect the cell law un-lerps a hover or click through.
    pub loc_rect: (f32, f32, f32, f32),
    /// The 128×128 area bitmap (`<Continent>.zmp`, read at `0x4a5feb`) as 1-based indices into
    /// [`Self::zones`], 0 for none; empty when no bitmap ships, leaving hover and click inert.
    pub zone_grid: Vec<u16>,
    pub zones: Vec<WorldMapZoneView>,
}

/// The `0x4a6ec0` cell law: clamp the UV, un-lerp through the loc rect to world coordinates,
/// scale by `0x806544` (1/(64·533.33)), range-check, and index `col - row·128`, `row` being
/// truncated from the negated axis and so `<= 0`.
fn area_grid_cell(loc: (f32, f32, f32, f32), u: f32, v: f32) -> Option<usize> {
    const K: f32 = 2.929_687_6e-5;
    let (left, right, top, bottom) = loc;
    let clamp01 = |p: f32| {
        if p >= 1.0 {
            1.0
        } else if p >= 0.0 {
            p
        } else {
            0.0
        }
    };
    let (u, v) = (clamp01(u), clamp01(v));
    let span_u = left - right;
    let span_v = top - bottom;
    if span_u == 0.0 || span_v == 0.0 {
        return None;
    }
    let f1 = 0.5 - ((1.0 - u) * span_u + right) * K;
    let f2 = 0.5 - ((1.0 - v) * span_v + bottom) * K;
    if !(0.0..=1.0).contains(&f1) || !(0.0..=1.0).contains(&f2) {
        return None;
    }
    let col = (f1 * 128.0) as i32; // __ftol: truncate toward zero
    let row = (f2 * -128.0) as i32;
    usize::try_from(col - row * 128)
        .ok()
        .filter(|&i| i < 128 * 128)
}

/// The 1-based zone or city under a hover or click: the continent's one grid, windowed by its own
/// loc rect at continent level and by the displayed zone's at zone level (`0x4a6ec0`).
fn grid_area(state: &WorldMapState, u: f32, v: f32) -> Option<u16> {
    // An instance map has no grid (`0x4a6ec0` opens `cmp esi,-2; je 0x4a70ed`); tested apart from
    // `c == 0`, which the world sheet shares.
    if state.direct_area.is_some() {
        return None;
    }
    let (c, z) = state.selection;
    if c == 0 {
        return None;
    }
    let cont = state.continents.get(c as usize - 1)?;
    if cont.zone_grid.is_empty() {
        return None;
    }
    let rect = if z == 0 {
        cont.loc_rect
    } else {
        cont.zones.get(z as usize - 1)?.loc_rect
    };
    let cell = area_grid_cell(rect, u, v)?;
    match cont.zone_grid.get(cell).copied().unwrap_or(0) {
        0 => None,
        zi => Some(zi),
    }
}

/// The client's power-of-two round-up (`0x4a833c`).
fn next_pow2(n: i64) -> i64 {
    let mut p = 1i64;
    while p < n {
        p <<= 1;
    }
    p
}

/// The engine-side world-map state: the pushed catalog and feed, and the selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapState {
    /// The catalog, pushed once at startup; until then every binding answers as at world level.
    pub continents: Vec<WorldMapContinentView>,
    /// The orphan list the reference's catalog builder also fills (`0x4a6130`, `0x4a61c9`): each
    /// `WorldMapArea` row with `areaID != 0` on no continent's map, the three battlegrounds.
    pub direct_areas: Vec<(u32, WorldMapZoneView)>,
    /// The displayed `(continent, zone)`, `(0, 0)` the world sheet, unless [`Self::direct_area`].
    pub selection: (u32, u32),
    /// An instance map's `WorldMapArea` row id: the reference's third selection cell `[0x845074]`
    /// beside continent `[0x84506c]` (then -2) and zone `[0x845070]`. The setter `0x4a67a0` keeps
    /// it exclusive with the pair (`0x4a67c1`, `0x4a67ea`); [`store_selection`] is the one writer.
    pub direct_area: Option<u32>,
    /// The player's `(continent, zone)`, where `SetMapToCurrentZone` lands.
    pub player_zone: Option<(u32, u32)>,
    /// The resolver's orphan leg, the `WorldMapArea` row id on the player's map. `0x4a6650`
    /// reaches it only when its continent loop misses (`0x4a66c3`), so at most one is set.
    pub player_direct_area: Option<u32>,
    /// `GetPlayerMapPosition("player")` on the displayed map; `None` is off it.
    pub player_uv: Option<(f32, f32)>,
    /// The arrow frames' wrapper ids, built once per session; `None` until created.
    pub arrow_world: Option<u32>,
    pub arrow_mini: Option<u32>,
    /// `party1`..`party4` in `party_slots` order; `None` or missing answers `(0, 0)`.
    pub party_uv: Vec<Option<(f32, f32)>>,
    /// `raid1`..`raid40` in roster order, fed like [`Self::party_uv`].
    pub raid_uv: Vec<Option<(f32, f32)>>,
    /// Radians, 0 north, counterclockwise-positive; `UpdateWorldMapArrowFrames` turns the arrows.
    pub player_facing: f32,
    /// `GetCorpseMapPosition()` on the displayed map; `None` is no corpse or off the map.
    pub corpse_uv: Option<(f32, f32)>,
    /// `PLAYER_EXPLORED_ZONES_1`, 64 words, bit n being `AreaTable.dbc` exploreFlag n.
    pub explored: Vec<u32>,
    pub landmarks: Vec<WorldMapLandmarkView>,
}

fn explored_bit(explored: &[u32], n: u32) -> bool {
    explored
        .get((n / 32) as usize)
        .is_some_and(|w| w & (1 << (n % 32)) != 0)
}

/// The f32 reciprocals `0x4a7fa0` scales a hit rect's detail-frame pixels by (`fild`, `fmul`,
/// `fstp dword`) to meet the normalized cursor, so the edges are f32.
const OVERLAY_RECIP_X: f32 = 1.0 / 1002.0;
const OVERLAY_RECIP_Y: f32 = 1.0 / 668.0;

/// The zone-level overlay pre-search (`0x4a7ffc..0x4a80ea`): the first revealed overlay whose hit
/// rect holds the cursor, edges inclusive, names its first area; a zero-width rect or a NaN cursor
/// never hits, and a nameless overlay is walked past. The list is the revealed one (`[0xb6e630]`,
/// built by `0x4a6ad9`, gated at `0x4a6bfa`), so a fogged sub-area has no name.
fn overlay_hover(state: &WorldMapState, x: f32, y: f32) -> Option<String> {
    revealed_overlays(state).into_iter().find_map(|o| {
        let (top, left, bottom, right) = o.hit_rect;
        let (xlo, xhi) = (
            left as f32 * OVERLAY_RECIP_X,
            right as f32 * OVERLAY_RECIP_X,
        );
        let (ylo, yhi) = (
            top as f32 * OVERLAY_RECIP_Y,
            bottom as f32 * OVERLAY_RECIP_Y,
        );
        let inside = xhi - xlo != 0.0
            && yhi - ylo != 0.0
            && (xlo..=xhi).contains(&x)
            && (ylo..=yhi).contains(&y);
        if inside {
            o.area_name.clone()
        } else {
            None
        }
    })
}

/// What `UpdateMapHighlight` answers for a cursor at map UV `(x, y)` under the current selection.
enum Hover {
    Highlight {
        name: String,
        file: String,
        tex_pct_y: f64,
        texture: (f64, f64),
        scroll: (f64, f64),
    },
    Name(String),
    Miss,
}

/// The continent-level highlight (`0x4a81de` → `0x4a822a`): the zone's art folder, drawn as
/// `<file>\<file>Highlight`, and the coords seating it over the zone's rect in the client's mixed
/// f32 and f64.
fn continent_hover(cont: &WorldMapContinentView, zone: &WorldMapZoneView) -> Option<Hover> {
    let (cl, cr, ct, cb) = cont.loc_rect;
    let (zl, zr, zt, zb) = zone.loc_rect;
    let (w, h) = (zl - zr, zt - zb);
    let (cont_w, cont_h) = (cl - cr, ct - cb);
    if w == 0.0 || h == 0.0 || cont_w == 0.0 || cont_h == 0.0 {
        return None;
    }
    let recip_x = 1.0f32 / cont_w;
    let recip_y_f32 = 1.0f32 / cont_h;
    let texture_x = f64::from(recip_x * w);
    let texture_y = (1.0f64 / f64::from(cont_h)) * f64::from(h);
    let scroll_x = f64::from(recip_x * (cl - zl));
    let scroll_y = f64::from(recip_y_f32 * (ct - zt));
    // texPctY = dim / potdim, dim = __ftol(h·128 / w).
    let dim = ((h * 128.0) / w) as i64;
    let potdim = next_pow2(dim);
    let tex_pct_y = if potdim > 0 {
        dim as f64 / potdim as f64
    } else {
        0.0
    };
    Some(Hover::Highlight {
        name: zone.name.clone(),
        file: zone.map_file.clone(),
        tex_pct_y,
        texture: (texture_x, texture_y),
        scroll: (scroll_x, scroll_y),
    })
}

/// `0x4a7fa0`'s level dispatch. Continent level: the grid's zone highlights. Zone level: the
/// overlay pre-search, then the grid through the displayed zone's rect (`0x4a7620`), naming a
/// neighbour but never the displayed zone, name only (`0x4a812e`). World level: the tail, as the
/// continent highlight the reference draws there is not built here.
fn hover(wm: &WorldMapState, x: f32, y: f32) -> Hover {
    // An instance map resolves no area (`0x4a7620` opens `cmp esi,-2; je 0x4a76d7`).
    if wm.direct_area.is_some() {
        return Hover::Miss;
    }
    let (c, z) = wm.selection;
    let Some(cont) = c.checked_sub(1).and_then(|i| wm.continents.get(i as usize)) else {
        return Hover::Miss;
    };
    if z != 0 {
        if let Some(name) = overlay_hover(wm, x, y) {
            return Hover::Name(name);
        }
        return match grid_area(wm, x, y) {
            Some(zi) if u32::from(zi) != z => cont
                .zones
                .get(zi as usize - 1)
                .map_or(Hover::Miss, |zone| Hover::Name(zone.name.clone())),
            _ => Hover::Miss,
        };
    }
    grid_area(wm, x, y)
        .and_then(|zi| cont.zones.get(zi as usize - 1))
        .and_then(|zone| continent_hover(cont, zone))
        .unwrap_or(Hover::Miss)
}

/// The direct area's row by id, bound-checked as the reference's `[0xc0d5bc]` lookup is
/// (`0x4a6d24`); an id no row carries is `None`, its NULL.
fn direct_row(state: &WorldMapState) -> Option<&WorldMapZoneView> {
    let id = state.direct_area?;
    state
        .direct_areas
        .iter()
        .find(|(row_id, _)| *row_id == id)
        .map(|(_, row)| row)
}

/// The displayed zone's or instance map's row; `0x4a67a0` keys the -2 state's overlays by
/// `[0x845074]`, so an instance map's pass the same explored-bit gate.
fn current_zone(state: &WorldMapState) -> Option<&WorldMapZoneView> {
    if state.direct_area.is_some() {
        return direct_row(state);
    }
    let (c, z) = state.selection;
    if c == 0 || z == 0 {
        return None;
    }
    state
        .continents
        .get(c as usize - 1)?
        .zones
        .get(z as usize - 1)
}

/// The displayed zone's revealed overlays in catalog order, the i-th `GetMapOverlayInfo(i)`.
fn revealed_overlays(state: &WorldMapState) -> Vec<&WorldMapOverlayView> {
    current_zone(state)
        .map(|zone| {
            zone.overlays
                .iter()
                .filter(|o| {
                    o.explore_bits
                        .iter()
                        .any(|&b| explored_bit(&state.explored, b))
                })
                .collect()
        })
        .unwrap_or_default()
}

impl super::UiScript {
    /// Pushes the catalog at startup, queueing `WORLD_MAP_UPDATE` so an open map repaints.
    pub fn set_world_map_catalog(&mut self, continents: Vec<WorldMapContinentView>) {
        let mut model = self.model_mut();
        model.worldmap.continents = continents;
        model
            .pending_events
            .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
    }

    /// Pushes the orphan list beside the catalog, both filled in one walk (`0x4a5d00`).
    pub fn set_world_map_direct_areas(&mut self, direct_areas: Vec<(u32, WorldMapZoneView)>) {
        let mut model = self.model_mut();
        model.worldmap.direct_areas = direct_areas;
        model
            .pending_events
            .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
    }

    /// Pushes the resolver's orphan leg; the continent leg rides [`Self::set_world_map_feed`].
    pub fn set_world_map_player_direct_area(&mut self, direct_area: Option<u32>) {
        self.model_mut().worldmap.player_direct_area = direct_area;
    }

    /// Pushes the discovery bitset; a change queues `WORLD_MAP_UPDATE` so an open map fills in.
    pub fn set_world_map_explored(&mut self, explored: Vec<u32>) {
        let mut model = self.model_mut();
        if model.worldmap.explored != explored {
            model.worldmap.explored = explored;
            model
                .pending_events
                .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
        }
    }

    /// Pushes the displayed map's landmarks; a change queues `WORLD_MAP_UPDATE` to re-seat them.
    pub fn set_world_map_landmarks(&mut self, landmarks: Vec<WorldMapLandmarkView>) {
        let mut model = self.model_mut();
        if model.worldmap.landmarks != landmarks {
            model.worldmap.landmarks = landmarks;
            model
                .pending_events
                .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
        }
    }

    /// Pushes the per-frame feed: the player's zone, the projected positions and the facing.
    pub fn set_world_map_feed(
        &mut self,
        player_zone: Option<(u32, u32)>,
        player_uv: Option<(f32, f32)>,
        player_facing: f32,
        corpse_uv: Option<(f32, f32)>,
        party_uv: Vec<Option<(f32, f32)>>,
        raid_uv: Vec<Option<(f32, f32)>>,
    ) {
        let mut model = self.model_mut();
        model.worldmap.player_zone = player_zone;
        model.worldmap.player_uv = player_uv;
        model.worldmap.player_facing = player_facing;
        model.worldmap.corpse_uv = corpse_uv;
        model.worldmap.party_uv = party_uv;
        model.worldmap.raid_uv = raid_uv;
    }

    /// The selection's three cells, which the app reads each frame to project the feed: continent,
    /// zone, and the direct-area id that overrides them when `Some`. The pair alone reads an
    /// instance map as the world sheet.
    pub fn world_map_selection(&self) -> (u32, u32, Option<u32>) {
        let wm = &self.model_ref().worldmap;
        (wm.selection.0, wm.selection.1, wm.direct_area)
    }

    /// Moves the selection from the engine, as the zone updater `0x494780` calls the setter
    /// `0x4a67a0`: a fresh login shows the player's map before any Lua runs, not the world sheet,
    /// whose `GetPlayerMapPosition` is sheet UV (`0x4a7360`). It takes the resolver's whole
    /// answer, orphan leg included, as `0x4947ac` calls `0x4a6650`.
    pub fn sync_world_map_to_player_zone(
        &mut self,
        continent: u32,
        zone: u32,
        direct_area: Option<u32>,
    ) {
        let mut model = self.model_mut();
        select_resolved(&mut model, continent, zone, direct_area);
    }

    /// The map UV at UI point `(x, y)`, or `None` off the art, normalized over `WorldMapButton`
    /// as `WorldMapButton_OnClick` does (WorldMapFrame.lua:287-291). A pure query for the app's
    /// dev map-jump, which must not drill in as the click path does.
    pub fn world_map_uv_at(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let id = self.hit_test(x, y)?;
        let model = self.model_ref();
        let hit = model.id_to_frame.get(&id).copied()?;
        // The button or anything on it: POI icons (WorldMapFrame.lua:178), the player arrow's
        // button and the blips are its children and sit on top.
        let mut fh = hit;
        while model.arena.frame(fh)?.name.as_deref() != Some("WorldMapButton") {
            fh = model.arena.frame(fh)?.parent?;
        }
        let r = model.resolved.get(&fh)?;
        let (w, h) = (r.right - r.left, r.top - r.bottom);
        (w > 0.0 && h > 0.0).then(|| ((x - r.left) / w, (r.top - y) / h))
    }
}

/// The one writer of the three selection cells, as the setter `0x4a67a0` is: every leg but
/// `ecx == -2` clears the direct cell (`0x4a67ea`), and that one stores the id (`0x4a67c1`).
fn store_selection(model: &mut Model, selection: (u32, u32), direct_area: Option<u32>) {
    model.worldmap.selection = selection;
    model.worldmap.direct_area = direct_area;
    model
        .pending_events
        .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
}

/// Clamps and stores a continent and zone, clearing the direct area: the tail of `SetMapZoom`,
/// `SetMapToCurrentZone` and `ProcessMapClick`.
fn select(model: &mut Model, continent: i64, zone: i64) {
    let c = continent.clamp(0, model.worldmap.continents.len() as i64) as u32;
    let z = if c == 0 {
        0
    } else {
        let n = model.worldmap.continents[c as usize - 1].zones.len() as i64;
        zone.clamp(0, n) as u32
    };
    store_selection(model, (c, z), None);
}

/// Selects an instance map, `0x4a67a0` with `ecx == -2`: the zone cell goes to -1 (our 0) and the
/// row id is stored unchecked, as every reader bound-checks it ([`direct_row`]).
fn select_direct(model: &mut Model, wma_id: u32) {
    store_selection(model, (0, 0), Some(wma_id));
}

/// Applies the resolver `0x4a6650`'s answer, the tail of `SetMapToCurrentZone` and of the login
/// sync (`0x4947ac`).
fn select_resolved(model: &mut Model, continent: u32, zone: u32, direct_area: Option<u32>) {
    match direct_area {
        Some(id) => select_direct(model, id),
        None => select(model, i64::from(continent), i64::from(zone)),
    }
}

/// Register the world-map globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetMapContinents() → the continent names, in index order.
    g.set(
        "GetMapContinents",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let names: Vec<Value> = model
                .worldmap
                .continents
                .iter()
                .map(|c| Ok(Value::String(lua.create_string(&c.name)?)))
                .collect::<mlua::Result<_>>()?;
            Ok(MultiValue::from_vec(names))
        })?,
    )?;

    // GetMapZones(continent) → the zone names. A negative continent answers none, as `0x4a7d10`'s
    // bound is unsigned (`dec edi; cmp edi,[0xb6e664]; jae`): an instance map's empty zone list is
    // what keeps `WorldMapZoneButton_OnClick` from calling `SetMapZoom(-1, id)`.
    g.set(
        "GetMapZones",
        lua.create_function(|lua, c: i64| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let zones = usize::try_from(c)
                .ok()
                .and_then(|c| c.checked_sub(1))
                .and_then(|c| model.worldmap.continents.get(c))
                .map(|cont| cont.zones.as_slice())
                .unwrap_or(&[]);
            let names: Vec<Value> = zones
                .iter()
                .map(|z| Ok(Value::String(lua.create_string(&z.name)?)))
                .collect::<mlua::Result<_>>()?;
            Ok(MultiValue::from_vec(names))
        })?,
    )?;

    // GetCurrentMapContinent() / GetCurrentMapZone(): each cell plus one (`0x4a7ed0`, `0x4a7f00`),
    // so an instance map answers -1 and 0.
    g.set(
        "GetCurrentMapContinent",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match model.worldmap.direct_area {
                Some(_) => -1i64,
                None => i64::from(model.worldmap.selection.0),
            })
        })?,
    )?;
    g.set(
        "GetCurrentMapZone",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.worldmap.selection.1))
        })?,
    )?;

    g.set(
        "SetMapZoom",
        lua.create_function(|lua, (c, z): (i64, Option<i64>)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            select(&mut model, c, z.unwrap_or(0));
            Ok(())
        })?,
    )?;

    // SetMapToCurrentZone(): the resolver's whole answer, orphan leg included (`0x4a7e20` is
    // `call 0x4a6650; xor eax,eax; ret`).
    g.set(
        "SetMapToCurrentZone",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let (c, z) = model.worldmap.player_zone.unwrap_or((0, 0));
            let direct = model.worldmap.player_direct_area;
            select_resolved(&mut model, c, z, direct);
            Ok(())
        })?,
    )?;

    // GetMapInfo() → mapFileName and two texture dimensions (`0x4a7e30`, `mov eax,3`): nil, 0, 0
    // at world level. The name is the unlocalized art folder (`0x4a6cf0` reads `[row+0xc]`).
    g.set(
        "GetMapInfo",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let wm = &model.worldmap;
            let file = match (wm.direct_area, wm.selection) {
                // An instance map, by id through the orphan list: `0x4a6cf0`'s `jl 0x4a6d24`
                // sends both negative continents there, and world level's third cell is -1.
                (Some(_), _) => direct_row(wm).map(|row| row.map_file.clone()),
                (None, (0, _)) => None,
                (None, (c, z)) => wm.continents.get(c as usize - 1).map(|cont| match z {
                    0 => cont.map_file.clone(),
                    z => cont
                        .zones
                        .get(z as usize - 1)
                        .map(|zone| zone.map_file.clone())
                        .unwrap_or_else(|| cont.map_file.clone()),
                }),
            };
            // No caller reads the dimensions, 0 here on every map; the reference computes them
            // from `WorldMapContinent.dbc` (`0x4a6d50`), which lacks an instance map's mapID.
            let file = match file {
                Some(f) => Value::String(lua.create_string(&f)?),
                None => Value::Nil,
            };
            Ok((file, 0i64, 0i64))
        })?,
    )?;

    // GetCorpseMapPosition() → map UV, or (0, 0), which WorldMapFrame.lua:445 hides.
    g.set(
        "GetCorpseMapPosition",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (x, y) = model.worldmap.corpse_uv.unwrap_or((0.0, 0.0));
            Ok((f64::from(x), f64::from(y)))
        })?,
    )?;

    // GetPlayerMapPosition(unit) → map UV, or (0, 0) off the map; `player`, `party1`..`party4`
    // and `raid1`..`raid40` resolve, anything else is (0, 0).
    g.set(
        "GetPlayerMapPosition",
        lua.create_function(|lua, unit: String| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let slot = |prefix: &str, list: &[Option<(f32, f32)>]| {
                unit.strip_prefix(prefix)
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|n| *n >= 1)
                    .and_then(|n| list.get(n - 1).copied().flatten())
            };
            let (x, y) = match unit.as_str() {
                "player" => model.worldmap.player_uv,
                u if u.starts_with("party") => slot("party", &model.worldmap.party_uv),
                u if u.starts_with("raid") => slot("raid", &model.worldmap.raid_uv),
                _ => None,
            }
            .unwrap_or((0.0, 0.0));
            Ok((f64::from(x), f64::from(y)))
        })?,
    )?;

    // GetPlayerFacing() → the facing in radians. Not a 1.12 global.
    g.set(
        "GetPlayerFacing",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.worldmap.player_facing))
        })?,
    )?;

    // ProcessMapClick(x, y) (`0x4a7f30`): at world level the first continent whose sheet rect
    // holds the click (`0x4a7100`); otherwise the zone or city under it through the displayed
    // rect (`0x4a6ec0`), which it drills into.
    g.set(
        "ProcessMapClick",
        lua.create_function(|lua, (x, y): (f32, f32)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // An instance map swallows the click (`0x4a7540` opens `cmp esi,-2; je 0x4a760d`),
            // tested first as it shares the world sheet's `continent == 0`.
            if model.worldmap.direct_area.is_some() {
                return Ok(());
            }
            if model.worldmap.selection.0 == 0 {
                let hit = model.worldmap.continents.iter().position(|cont| {
                    let (u0, v0, u1, v1) = cont.world_rect;
                    (u0..=u1).contains(&x) && (v0..=v1).contains(&y)
                });
                if let Some(i) = hit {
                    select(&mut model, i as i64 + 1, 0);
                }
            } else if let Some(zi) = grid_area(&model.worldmap, x, y) {
                let c = i64::from(model.worldmap.selection.0);
                select(&mut model, c, i64::from(zi));
            }
            Ok(())
        })?,
    )?;

    // UpdateMapHighlight(x, y) → name, fileName, texPctX, texPctY, textureX, textureY,
    // scrollChildX, scrollChildY (`0x4a7fa0`): eight values always, a nil fileName hiding the quad.
    g.set(
        "UpdateMapHighlight",
        lua.create_function(|lua, (x, y): (f32, f32)| {
            let hit = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                hover(&model.worldmap, x, y)
            };
            let string = |s: &str| Ok::<_, mlua::Error>(Value::String(lua.create_string(s)?));
            Ok(match hit {
                Hover::Highlight {
                    name,
                    file,
                    tex_pct_y,
                    texture,
                    scroll,
                } => (
                    string(&name)?,
                    string(&file)?,
                    1.0f64,
                    tex_pct_y,
                    texture.0,
                    texture.1,
                    scroll.0,
                    scroll.1,
                ),
                Hover::Name(name) => (string(&name)?, Value::Nil, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
                Hover::Miss => (Value::Nil, Value::Nil, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            })
        })?,
    )?;

    g.set(
        "GetNumMapLandmarks",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.worldmap.landmarks.len() as i64)
        })?,
    )?;

    // GetMapLandmarkInfo(i) → name, description, textureIndex, x, y (`0x4a8740`). textureIndex is
    // a row's Icon, or 15 at zone level without `Flags & 0x80` (`0x4a8848`), and the guard
    // marker's packet Icon; an empty description is nil, as `0x6f3890` pushes for a NULL string.
    g.set(
        "GetMapLandmarkInfo",
        lua.create_function(|lua, i: i64| {
            let landmark = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                usize::try_from(i)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| model.worldmap.landmarks.get(i).cloned())
            };
            let Some(l) = landmark else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let description = match l.description.is_empty() {
                true => Value::Nil,
                false => Value::String(lua.create_string(&l.description)?),
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&l.name)?),
                description,
                Value::Integer(i64::from(l.texture_index)),
                Value::Number(f64::from(l.uv.0)),
                Value::Number(f64::from(l.uv.1)),
            ]))
        })?,
    )?;

    // GetNumMapOverlays(): the displayed map's revealed overlays; 0 at world and continent level.
    g.set(
        "GetNumMapOverlays",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(revealed_overlays(&model.worldmap).len() as i64)
        })?,
    )?;

    // GetMapOverlayInfo(i) → textureName, textureWidth, textureHeight, offsetX, offsetY,
    // mapPointX, mapPointY; mapPointX and mapPointY are 0 on every 5875 row.
    g.set(
        "GetMapOverlayInfo",
        lua.create_function(|lua, i: i64| {
            let overlay = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                usize::try_from(i)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| revealed_overlays(&model.worldmap).get(i).cloned().cloned())
            };
            let Some(o) = overlay else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&o.texture)?),
                Value::Integer(i64::from(o.width)),
                Value::Integer(i64::from(o.height)),
                Value::Integer(i64::from(o.offset_x)),
                Value::Integer(i64::from(o.offset_y)),
                Value::Integer(0),
                Value::Integer(0),
            ]))
        })?,
    )?;

    Ok(())
}
