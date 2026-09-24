//! The world-map bindings (decision 0203 phase 2) — the Era-shaped map-navigation surface the
//! reference `WorldMapFrame.lua` reads, over the questgiver/quest-log seam shape: the app pushes
//! a static **catalog** once (continents → zones, from `WorldMapArea`/`AreaTable`/
//! `WorldMapContinent`) and a small per-frame **feed** (the player's resolved zone, projected map
//! position, and facing — all projection math stays app-side in `map_proj`); the engine owns the
//! **selection** (which map is displayed) so `SetMapZoom` → `GetCurrentMapContinent` reads back
//! synchronously in the same click, exactly like the quest-log selection precedent.
//!
//! `SetMapZoom`/`SetMapToCurrentZone`/`ProcessMapClick` queue `WORLD_MAP_UPDATE` through the
//! model's pending-event queue (fired at the next tick — the reference's synchronous fire is a
//! repaint trigger, and one tick is invisible at frame rate).
//!
//! Selection encoding (the Lua-visible one, the client's internal indices `+1` — `0x4a7ed0`/
//! `0x4a7f00`): continent `0` = the world sheet, `1..` = the catalog's continents; zone `0` = the
//! whole continent, `1..` = that continent's zone list. The catalog's *order* defines those
//! indices everywhere (dropdowns, `SetMapZoom`, the feed) — the app builds it under the
//! verified rules (WorldMapArea file order for continents, `0x4a5d00`; case-insensitive
//! display-name sort for zones, `0x4a6390`).
//!
//! …and a **third** selection state beside that pair: the **direct area**, the map of an
//! instance — a battleground, a dungeon — which is no continent's child and so falls out of both
//! lists (`0x4a67a0`). The reference does not
//! encode it inside `(continent, zone)`: it carries THREE `.data` cells, and `continent == -2`
//! with a `WorldMapArea` row **ID** in the third one IS that state ([`WorldMapState::direct_area`]).
//! Inside Warsong Gulch a client without it answers `GetMapInfo() == nil`, and the stock
//! `Blizzard_BattlefieldMinimap.lua:83-86` returns on the fourth line of its update — an empty
//! battle map and an empty world map, which is the bug this models away.
//!
//! Continent-level hover/click resolve through the pushed **zone grid** — the 128×128 area
//! bitmap (`Interface\WorldMap\<Continent>.zmp`, remapped app-side to 1-based zone indices; the
//! source + cell law read at `0x4a5feb`/`0x4a6ec0`). The cell
//! law itself is transcribed here ([`area_grid_cell`]) rather than in the app's `map_proj`
//! because its only callers are these bindings. `UpdateMapHighlight` answers per level exactly
//! as `0x4a7fa0` does: at continent level the hovered zone's name **and** its
//! highlight quad; at zone level a **name only** — a revealed overlay's sub-area when the cursor
//! is inside its `WorldMapOverlay` hit rect, else the neighbouring zone or city whose grid cell
//! the cursor is in through the displayed zone's rect window, never the displayed zone itself —
//! with a nil fileName, so the frame hides the quad (report B360). The world sheet's continent
//! highlight is not built: hovering a continent there answers the nil/zero tail.
//!
//! Zone maps carry **exploration fog**: the base tiles are the unexplored parchment, and each
//! discovered sub-area's overlay art (`GetNumMapOverlays`/`GetMapOverlayInfo`, filtered by the
//! pushed `PLAYER_EXPLORED_ZONES` bitset) fills it in — the reference's own overlay pool draws
//! the returned pieces. The landmark family (`GetNumMapLandmarks`/`GetMapLandmarkInfo`) answers
//! with the `AreaPOI.dbc` rows the displayed level admits, then the guard-directions marker.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One zone row in a continent's list. Its 1-based position IS the zone index Lua sees.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapZoneView {
    /// The display name (AreaTable's localized string — "Elwynn Forest").
    pub name: String,
    /// The AreaTable id (the feed's player-zone resolution joins on this; phase 3 overlays too).
    pub area_id: u32,
    /// The `Interface\WorldMap\<file>\` art folder (WorldMapArea's internal name — "Elwynn").
    pub map_file: String,
    /// This zone's WorldMapArea loc rect `(left, right, top, bottom)` — the window the continent
    /// highlight sizes/seats against, and the rect the zone-level `ProcessMapClick` un-lerps
    /// through (`0x4a6ec0`: the zone is a rect window onto the one per-continent bitmap).
    pub loc_rect: (f32, f32, f32, f32),
    /// The zone's discovery overlays (WorldMapOverlay rows) — the art that fills the parchment
    /// as sub-areas are explored (`GetNumMapOverlays`/`GetMapOverlayInfo`).
    pub overlays: Vec<WorldMapOverlayView>,
}

/// One **map landmark** — a POI icon on the displayed map (`GetNumMapLandmarks` /
/// `GetMapLandmarkInfo`). The app projects it, so what arrives here is already map UV.
///
/// Two sources feed it, in the reference's own order (its builder `0x4a67a0`):
///
/// - the **`AreaPOI.dbc` rows** that survive the builder's level-flag, exploration and
///   world-state gates — the town and capital icons, the capitals' "Under Attack" markers, and the
///   Eastern Plaguelands towers;
/// - then the **guard-directions marker** (`SMSG_GOSSIP_POI` — the flag a guard drops when you ask
///   where the warrior trainer is), appended last as the one element with `+0x10 == 1` and exempt
///   from every gate above.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapLandmarkView {
    /// The label its tooltip shows ("Stormwind Warrior Trainer").
    pub name: String,
    /// The second tooltip line — a live status string on the AreaPOI rows that carry one (a
    /// battleground node's "In Conflict"); empty for the guard marker.
    pub description: String,
    /// The `Interface\Minimap\POIIcons` cell, 8×8 grid (the guard marker's is `6` — the red flag).
    pub texture_index: u32,
    /// Position on the DISPLAYED map, `[0,1]` UV from its top-left.
    pub uv: (f32, f32),
}

/// One discovery overlay: a sub-area's map art, drawn over the base tiles once any of its
/// explore bits is set (decision 0203 phase 3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapOverlayView {
    /// The full texture path prefix (`Interface\WorldMap\<Zone>\<TexName>`) — the reference Lua
    /// appends the piece number (`..1`, `..2`, …) exactly as the client's C return does.
    pub texture: String,
    /// The overlay's art size in map px (the Lua slices it into 256-tiles with POT file sizes).
    pub width: u32,
    pub height: u32,
    /// Placement within the 1002×668 detail frame, px from its TOPLEFT.
    pub offset_x: u32,
    pub offset_y: u32,
    /// The AreaTable `exploreFlag` bit indices that reveal this overlay (any set bit shows it;
    /// empty = never shown — an overlay whose areas are unknown).
    pub explore_bits: Vec<u32>,
    /// The hover hit rect `(top, left, bottom, right)`, px of the 1002×668 detail frame — the
    /// DBC's own `HitRect*` fields. The zone-level `UpdateMapHighlight` scales it by 1/1002 and
    /// 1/668 and tests the normalized cursor against it, both edges inclusive
    /// (`0x4a7ffc..0x4a80ea`).
    pub hit_rect: (u32, u32, u32, u32),
    /// The name that hover shows inside the rect: the localized AreaTable name of the overlay's
    /// FIRST area slot (`0x4a7fa0` reads `+0x8` only). `None` when that slot resolves to no row —
    /// the client then walks past the overlay exactly as if the cursor were outside it.
    pub area_name: Option<String>,
}

/// One continent. Its 1-based position IS the continent index Lua sees.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapContinentView {
    /// The display name — Map.dbc's localized MapName ("Eastern Kingdoms", "Kalimdor"), the
    /// client's own dropdown-label source (`0x4a65a0`), never the art folder string.
    pub name: String,
    /// The `Interface\WorldMap\<file>\` art folder.
    pub map_file: String,
    /// This continent's rect on the world sheet, normalized UV `(u0, v0, u1, v1)` — world-level
    /// `ProcessMapClick` picks the first continent (catalog order) containing the click, exactly
    /// the client's AABB walk (`0x4a7100`'s world branch over the record trailer floats). The
    /// app computes it with the verified `0x4a5d00` kernel (`map_proj::continent_sheet_rect`,
    /// from the WorldMapContinent tile bounds); the 5875 rects are disjoint.
    pub world_rect: (f32, f32, f32, f32),
    /// The continent's WorldMapArea world rect (loc left/right/top/bottom) — the rect the
    /// `0x4a6ec0` cell law un-lerps hover/click UVs through ([`area_grid_cell`]).
    pub loc_rect: (f32, f32, f32, f32),
    /// The 128×128 area bitmap, remapped app-side to **1-based zone indices** into
    /// [`Self::zones`] (0 = no zone; empty = no bitmap shipped — hover/click inert). Row-major,
    /// indexed by [`area_grid_cell`].
    pub zone_grid: Vec<u16>,
    /// The continent's zones, in Lua index order.
    pub zones: Vec<WorldMapZoneView>,
}

/// The `0x4a6ec0` cell law, verbatim: clamp the UV, un-lerp through the continent's WorldMapArea
/// rect back to world coords, quantize on the half-ADT world grid (2.9296876e-5 = 1/(64·533.33),
/// the binary's 0x806544), range-check, index `col − row·128` (row is truncated from the negated
/// axis, so it's ≤ 0). Verified against the shipped Azeroth.zmp: Goldshire's world position
/// lands on cell 12735 = Elwynn Forest.
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

/// The hovered/clicked child (zone OR city) at the current CONTINENT or ZONE selection: the grid
/// cell's 1-based index into the continent's `zones`, or `None` off-land / off-grid / at world
/// level. The rect that windows the one per-continent 128×128 bitmap is the continent's own loc
/// rect at continent level, the CURRENT zone's loc rect at zone level — there is no per-zone
/// bitmap, a zone/city is just a different rect window (`0x4a6ec0`). This
/// is what lets a click on a city's footprint from its neighbouring zone map drill into the city.
fn grid_area(state: &WorldMapState, u: f32, v: f32) -> Option<u16> {
    // The direct-area state has no bitmap and is refused before the cell law runs: `0x4a6ec0`
    // opens `cmp esi,-2; je 0x4a70ed` — the chain
    // ships exactly three `.zmp` files and none of them is a battleground's. Explicit rather than
    // left to `c == 0` below, because "the world sheet" and "an instance map" are different
    // states that happen to share that cell.
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

/// Power-of-two round-up (≥ n, ≥ 1) — the client's round-up at `0x4a833c`, used for the
/// continent highlight's vertical texcoord crop (`texPercentageY = dim / potdim`).
fn next_pow2(n: i64) -> i64 {
    let mut p = 1i64;
    while p < n {
        p <<= 1;
    }
    p
}

/// The engine-side world-map state: the pushed catalog + feed, and the engine-owned selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapState {
    /// The static catalog (pushed once at startup; empty until then — every binding degrades to
    /// the world level, where `GetMapInfo` returns nil and the reference Lua falls back to
    /// `"World"`).
    pub continents: Vec<WorldMapContinentView>,
    /// The **orphan list** — the third array the reference's catalog builder fills (`0x4a5d00`'s
    /// count/fill passes `0x4a6130`/`0x4a61c9`): every `WorldMapArea` row with `areaID != 0`
    /// whose mapID matches **no** continent record, as `(row ID, the row)`, in DBC **file order,
    /// unsorted** — the three battlegrounds in 5875. Keyed by id, never indexed: nothing outside
    /// [`direct_row`]'s lookup reads a position here, because the selection stores the row's own
    /// ID rather than a place in this list.
    pub direct_areas: Vec<(u32, WorldMapZoneView)>,
    /// The displayed map: `(continent, zone)`, `(0, 0)` = the world sheet. **Not the whole
    /// selection on its own** — [`Self::direct_area`] overrides it.
    pub selection: (u32, u32),
    /// The **direct-area selection**: the `WorldMapArea` row **ID** of an instance map, or `None`
    /// at every continent/zone/world selection.
    ///
    /// The reference's selection is THREE `.data` cells, not two: `[0x84506c]` continent,
    /// `[0x845070]` zone, `[0x845074]` direct. `continent == -2` IS this state, and the third cell
    /// then holds the row's own id — **not** an index into [`Self::direct_areas`], which is only
    /// ever a lookup table.
    /// The setter `0x4a67a0` keeps the two exclusive (`0x4a67c1` stores the id only on the
    /// `ecx == -2` leg and jumps past `0x4a67ea`'s `= -1`; every other leg reaches it), so
    /// `direct_area.is_some()` ⇒ `selection == (0, 0)` — enforced in [`store_selection`], the one
    /// writer.
    pub direct_area: Option<u32>,
    /// Where `SetMapToCurrentZone` lands — the player's current `(continent, zone)` resolved by
    /// the app (AreaTable parent walk × the catalog). `None` = no continent matched → either the
    /// orphan leg below, or the world sheet.
    pub player_zone: Option<(u32, u32)>,
    /// The orphan half of that same answer: the `WorldMapArea` row ID whose mapID is the player's
    /// own map, or `None`. `0x4a6650` runs its continent loop FIRST and reaches the orphan loop
    /// only on a miss (`0x4a66c3`), so at most one of the two is ever set — and inside a
    /// battleground it is this one.
    pub player_direct_area: Option<u32>,
    /// `GetPlayerMapPosition("player")` for the CURRENT selection — projected app-side each frame
    /// (`map_proj`), `None`/(0,0) = off this map (the reference hides the blip).
    pub player_uv: Option<(f32, f32)>,
    /// The world map's arrow frame and the battlefield minimap's — the two per-session
    /// singletons `CreateWorldMapArrowFrame`/`CreateMiniWorldMapArrowFrame` build, by wrapper id
    /// (`worldmap_arrow`, 1980). `None` until the first create.
    pub arrow_world: Option<u32>,
    pub arrow_mini: Option<u32>,
    /// `GetPlayerMapPosition("party1".."party4")` for the CURRENT selection, in `party_slots`
    /// order — projected app-side through the same law as [`Self::player_uv`] (report B320).
    /// A slot is `None` when there is no member there, when we hold no position for them, or when
    /// that position is off the displayed map; all three read as the reference's `(0,0)` hide
    /// sentinel at the binding, which is the only answer its FrameXML consumer knows.
    ///
    /// Shorter than 4 whenever the party is: a missing index is a `None`.
    pub party_uv: Vec<Option<(f32, f32)>>,
    /// `raid1..raid40` in roster order — the stock map's raid arm (1980), fed like `party_uv`.
    pub raid_uv: Vec<Option<(f32, f32)>>,
    /// `GetPlayerFacing()` — wow orientation radians (0 = north, counterclockwise-positive). Our
    /// transcribed frame spins its arrow texture with it (`SetRotation`); the reference rotates
    /// an engine-rendered arrow model instead (0203 flags the stand-in).
    pub player_facing: f32,
    /// `GetCorpseMapPosition()` for the CURRENT selection — the corpse-run marker (decision 0308
    /// §5), projected app-side like [`Self::player_uv`]; `None`/(0,0) = no corpse or off this map
    /// (the reference's WorldMapFrame.lua:443-452 hide law).
    pub corpse_uv: Option<(f32, f32)>,
    /// The discovery bitset (`PLAYER_EXPLORED_ZONES_1` — 64 u32 = 2048 bits, bit n = AreaTable
    /// exploreFlag n), pushed by the app when the descriptor changes. Empty = nothing explored.
    pub explored: Vec<u32>,
    /// The POI icons on the displayed map (`GetNumMapLandmarks`/`GetMapLandmarkInfo`), already
    /// projected into its UV — pushed by the app on change, which repaints an open map.
    pub landmarks: Vec<WorldMapLandmarkView>,
}

/// Is explore-bit `n` set in the pushed bitset?
fn explored_bit(explored: &[u32], n: u32) -> bool {
    explored
        .get((n / 32) as usize)
        .is_some_and(|w| w & (1 << (n % 32)) != 0)
}

/// The reciprocals `0x4a7fa0` scales a `WorldMapOverlay` hit rect by: the rect is authored in
/// pixels of the 1002×668 detail frame, the cursor arrives normalized, and the client brings the
/// rect to the cursor (`fild` → `fmul` a f32 constant → `fstp dword`), so the edges are f32.
const OVERLAY_RECIP_X: f32 = 1.0 / 1002.0;
const OVERLAY_RECIP_Y: f32 = 1.0 / 668.0;

/// The zone-level **overlay pre-search** (`0x4a7ffc..0x4a80ea`): the first
/// REVEALED overlay whose hit rect contains the cursor names its first area. Both edges are
/// inclusive; a rect with a zero-width edge never hits; a NaN cursor never hits; an overlay whose
/// first area resolves to no AreaTable row is walked past, not stopped at. The list is the one
/// `GetNumMapOverlays` counts — the exploration-gated one (`[0xb6e630]`, built by `0x4a6ad9`
/// under the explored-byte read at `0x4a6bfa`), so a fogged sub-area has no name.
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
    /// Continent level, over a zone: its name and the highlight quad — fileName, texPercentageY,
    /// textureX/Y, scrollChildX/Y (texPercentageX is that branch's constant 1.0).
    Highlight {
        name: String,
        file: String,
        tex_pct_y: f64,
        texture: (f64, f64),
        scroll: (f64, f64),
    },
    /// Zone level: a name only — fileName nil, six zeros, the frame hides the quad.
    Name(String),
    /// Off every area: the nil/zero tail.
    Miss,
}

/// The continent-level answer for the hovered zone (the `0x4a81de → 0x4a822a` highlight-compute
/// path, zone-search branch): fileName = its art folder (the frame draws `<file>\<file>Highlight`),
/// the six coords seat/size/crop that texture over the zone's rect within the continent, keeping
/// the client's f32/f64 asymmetric narrowing.
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
    // texPctY = dim/potdim, dim = __ftol((h·128)/w) (rect-form K = 128).
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

/// `0x4a7fa0`'s level dispatch. Continent level: the grid cell's zone lights
/// up. Zone level: the overlay pre-search first, then the same grid re-windowed by the displayed
/// zone's own rect (`0x4a7620`), which returns nothing for the displayed zone itself — and only the
/// name-only tail is reachable from there (`0x4a812e`). World level: the continent highlight is
/// not built, so the tail.
fn hover(wm: &WorldMapState, x: f32, y: f32) -> Hover {
    // An instance map resolves no area under the cursor: the lookup INSIDE `UpdateMapHighlight`
    // (`0x4a7620`) opens `cmp esi,-2; je 0x4a76d7` → `xor eax,eax`, so no highlight and no name.
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

/// The selected direct area's row, resolved by **id** through the orphan list — the reference's
/// `[0xc0d5bc]` id-index lookup (`0x4a6cf0`'s `0x4a6d24` leg), which bound-checks on every read
/// rather than trusting the stored cell. An id no row carries reads back as `None`, which is the
/// binary's own NULL → `lua_pushnil`.
fn direct_row(state: &WorldMapState) -> Option<&WorldMapZoneView> {
    let id = state.direct_area?;
    state
        .direct_areas
        .iter()
        .find(|(row_id, _)| *row_id == id)
        .map(|(_, row)| row)
}

/// The displayed map's `WorldMapArea` row, if one is displayed: the selected zone, or — in the
/// direct-area state — the instance map's own row. `0x4a6cf0` resolves both through the same id
/// lookup, and `0x4a67a0`'s overlay half admits the `-2` state with the displayed key
/// `= [0x845074]`, so a battleground's overlays
/// reveal under the explored-bit gate exactly as a zone's do — no free reveal.
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

/// The displayed zone's REVEALED overlays, in catalog order (the i-th is `GetMapOverlayInfo(i)`).
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
    /// Push the static catalog (startup, once the DBCs are read). Queues `WORLD_MAP_UPDATE` so an
    /// already-open frame repaints (startup normally precedes any frame, harmlessly).
    pub fn set_world_map_catalog(&mut self, continents: Vec<WorldMapContinentView>) {
        let mut model = self.model_mut();
        model.worldmap.continents = continents;
        model
            .pending_events
            .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
    }

    /// Push the **orphan list** — the instance maps, keyed by `WorldMapArea` row id (see
    /// [`WorldMapState::direct_areas`]). Pushed beside the continent catalog at the same edge and
    /// for the same reason; the reference fills both containers in one walk (`0x4a5d00`).
    pub fn set_world_map_direct_areas(&mut self, direct_areas: Vec<(u32, WorldMapZoneView)>) {
        let mut model = self.model_mut();
        model.worldmap.direct_areas = direct_areas;
        model
            .pending_events
            .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
    }

    /// Push the resolver's **orphan leg** for the player — the `WorldMapArea` row id whose map is
    /// the player's own, or `None` (see [`WorldMapState::player_direct_area`]). The continent/zone
    /// leg rides [`Self::set_world_map_feed`]'s `player_zone`; `0x4a6650` computes both in one
    /// pass and reaches this one only when that one missed.
    pub fn set_world_map_player_direct_area(&mut self, direct_area: Option<u32>) {
        self.model_mut().worldmap.player_direct_area = direct_area;
    }

    /// Push the discovery bitset (the app calls this when `PLAYER_EXPLORED_ZONES` changes —
    /// including the first stream-in). Queues `WORLD_MAP_UPDATE` so an open zone map fills in
    /// newly explored art immediately.
    pub fn set_world_map_explored(&mut self, explored: Vec<u32>) {
        let mut model = self.model_mut();
        if model.worldmap.explored != explored {
            model.worldmap.explored = explored;
            model
                .pending_events
                .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
        }
    }

    /// Push the displayed map's landmarks (the app projects them; see [`WorldMapLandmarkView`]).
    /// On change, queues `WORLD_MAP_UPDATE` — the repaint that re-seats the POI icons, exactly
    /// the event the overlay push above uses for the same reason.
    pub fn set_world_map_landmarks(&mut self, landmarks: Vec<WorldMapLandmarkView>) {
        let mut model = self.model_mut();
        if model.worldmap.landmarks != landmarks {
            model.worldmap.landmarks = landmarks;
            model
                .pending_events
                .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
        }
    }

    /// Push the per-frame feed (player zone/projection/facing, the corpse marker, and the party
    /// slots' projections — see [`WorldMapState`]).
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

    /// The engine-owned selection — the app reads it each frame to project the feed for the
    /// displayed map. **All three of the reference's cells**, in its own order: the continent and
    /// zone pair (`0` = whole), then the direct-area row id
    /// ([`WorldMapState::direct_area`]) which overrides both when it is `Some`. A consumer that
    /// reads only the pair sees an instance map as the world sheet — which is precisely the bug
    /// that left the battle map empty — so the third cell travels with them.
    pub fn world_map_selection(&self) -> (u32, u32, Option<u32>) {
        let wm = &self.model_ref().worldmap;
        (wm.selection.0, wm.selection.1, wm.direct_area)
    }

    /// Move the selection **from the engine**, with no Lua in the loop — the reference's
    /// `SetMap` setter `0x4a67a0` reached from the zone updater `0x494780` rather than from a
    /// binding, which is how a freshly-logged-in client already shows the player's own zone.
    ///
    /// The reference has TWO writers of `[0x84506c]`/`[0x845070]`: the Lua verbs
    /// (`SetMapZoom`/`SetMapToCurrentZone`/`ProcessMapClick`), and this one. We only ever had the
    /// first, so until some addon opened the map we sat at the world level — and at the world
    /// level `GetPlayerMapPosition` answers a world-SHEET uv (`0x4a7360` step 2), which every
    /// addon that assumes a zone uv silently mis-scales. See [`crate::script::worldmap`]'s caller
    /// in `ui_world_map::feed_world_map` for the gate.
    ///
    /// It takes the resolver's whole answer — including the orphan leg — because `0x4947ac` calls
    /// `0x4a6650`, the same resolver `SetMapToCurrentZone` does: a player logging straight into a
    /// battleground is on the battleground's own map before any Lua runs, exactly as one logging
    /// into Elwynn is on Elwynn's.
    pub fn sync_world_map_to_player_zone(
        &mut self,
        continent: u32,
        zone: u32,
        direct_area: Option<u32>,
    ) {
        let mut model = self.model_mut();
        select_resolved(&mut model, continent, zone, direct_area);
    }

    /// The **normalized position within the map art** at UI-space `(x, y)`, or `None` when the
    /// point isn't over it. Reproduces the reference's own click normalization verbatim —
    /// `WorldMapFrame.xml`'s `WorldMapButton_OnClick`: `u = (x − left)/width`,
    /// `v = (top − y)/height` over `WorldMapButton`'s rect — so the UV handed back is the same one
    /// [`ProcessMapClick`](install) reads and the same one the blips are placed in. The Lua's
    /// effective-scale division cancels here: point and rect are both already in screen units.
    ///
    /// A pure query (fires nothing, mutates nothing). Its consumer is the app-side dev map-jump,
    /// which needs a click's UV *without* going through the faithful click path — that path drills
    /// into a zone, and the reference's law for it must not grow a second meaning.
    pub fn world_map_uv_at(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let id = self.hit_test(x, y)?;
        let model = self.model_ref();
        let hit = model.id_to_frame.get(&id).copied()?;
        // The map button, or **anything sitting on it**: the stock file makes every POI icon a
        // child of `WorldMapButton` (`WorldMapFrame.lua:178`), and so are the player arrow's clear
        // button and the party/raid blips, so the topmost frame over a town or a teammate is one
        // of those and not the button. Requiring the button itself made the click deaf exactly
        // where the map has something to show — the director's report on the stock map. Walking
        // up rather than widening the name test keeps everything else out: a click on the
        // continent dropdown or the close button is over neither the button nor its subtree.
        let mut fh = hit;
        while model.arena.frame(fh)?.name.as_deref() != Some("WorldMapButton") {
            fh = model.arena.frame(fh)?.parent?;
        }
        let r = model.resolved.get(&fh)?;
        let (w, h) = (r.right - r.left, r.top - r.bottom);
        (w > 0.0 && h > 0.0).then(|| ((x - r.left) / w, (r.top - y) / h))
    }
}

/// **The one writer of the three selection cells** — `0x4a67a0`, whose whole job is that the
/// continent/zone pair and the direct-area id can never both be live: the setter's valid-continent
/// leg and its `-1` leg both fall into `0x4a67ea mov [0x845074],eax` (`= -1`), and only `ecx == -2`
/// jumps past it after storing the id (`0x4a67c1`), having already forced the zone cell to `-1`.
/// Every selection verb in this module goes through here, so neither half can be moved alone.
fn store_selection(model: &mut Model, selection: (u32, u32), direct_area: Option<u32>) {
    model.worldmap.selection = selection;
    model.worldmap.direct_area = direct_area;
    model
        .pending_events
        .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
}

/// Clamp + store a continent/zone selection and queue the repaint event. The shared tail of
/// `SetMapZoom`/`SetMapToCurrentZone`/`ProcessMapClick` — and it **clears the direct area**, which
/// is the setter invariant above.
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

/// Select an instance map directly — `0x4a67a0` reached with `ecx == -2`: the zone cell goes to
/// `-1` (our `0`) and the raw `WorldMapArea` row id lands in `[0x845074]` **verbatim and
/// unchecked**. We keep it unchecked for the same reason the reference can: every consumer
/// bound-checks on read ([`direct_row`]), so an id no row carries degrades to the nil map name
/// instead of corrupting the selection.
fn select_direct(model: &mut Model, wma_id: u32) {
    store_selection(model, (0, 0), Some(wma_id));
}

/// Apply the resolver `0x4a6650`'s answer — the shared tail of `SetMapToCurrentZone` (`0x4a7e20`
/// is literally `call 0x4a6650`) and of the engine's own first-world-enter sync (`0x4947ac`, the
/// same function: there is no second resolver).
///
/// The orphan leg wins when it is set, and it is set only when no continent matched — the
/// reference reaches `0x4a66c3` exclusively through the continent loop's miss, so the two answers
/// are mutually exclusive by construction upstream and the order here is belt-and-braces.
fn select_resolved(model: &mut Model, continent: u32, zone: u32, direct_area: Option<u32>) {
    match direct_area {
        Some(id) => select_direct(model, id),
        None => select(model, i64::from(continent), i64::from(zone)),
    }
}

/// Register the world-map globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetMapContinents() → name1, name2, … (the dropdown list; index = continent number).
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

    // GetMapZones(continent) → name1, name2, … (that continent's zone list; index = zone number).
    // A NEGATIVE continent answers empty — `0x4a7d10`'s `dec edi; cmp edi,[0xb6e664]; jae` is an
    // unsigned bound, so `GetCurrentMapContinent()`'s `-1` on an instance map takes the empty
    // exit. That emptiness is load-bearing: it is what leaves `WorldMapZoneButton_OnClick` with
    // no button to fire, and so what stops a `SetMapZoom(-1, id)` from writing a bogus row id
    // into the direct cell.
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

    // GetCurrentMapContinent() / GetCurrentMapZone() — the selection halves (0 = world / whole
    // continent). The reference reads them right after SetMapZoom in the same click. Each is its
    // cell `+ 1` and nothing else (`0x4a7ed0`, `0x4a7f00`: a straight-line fild/pushnumber), so
    // the direct-area state answers **-1** (from the `-2` sentinel) and **0** (the zone cell it
    // forces to `-1`). The `-1` is a value no stock path tests for, and benignly: it leaves
    // `WorldMapFrame.lua:59`'s zoom-out button enabled, and the zone dropdown it would feed is
    // empty because `GetMapZones(-1)` takes the unsigned-bound exit below.
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

    // SetMapZoom(continent[, zone]) — the dropdowns' and zoom-out button's navigation verb.
    g.set(
        "SetMapZoom",
        lua.create_function(|lua, (c, z): (i64, Option<i64>)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            select(&mut model, c, z.unwrap_or(0));
            Ok(())
        })?,
    )?;

    // SetMapToCurrentZone() — the OnShow verb: jump to the player's own map (app-resolved feed).
    // `0x4a7e20` is `call 0x4a6650; xor eax,eax; ret`, so this is the resolver's whole answer,
    // orphan leg included — which is what puts a player standing in Warsong Gulch on the Warsong
    // Gulch map. The stock battlefield minimap calls it itself on PLAYER_ENTERING_WORLD.
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

    // GetMapInfo() → mapFileName (the Interface\WorldMap\<name>\ folder). World level → nil —
    // the reference Lua's own `"World"` fallback exists because the client returned nil there
    // (`0x4a6cf0`).
    //
    // The name is `WorldMapArea` field[3], the **art-folder identifier** — "WarsongGulch", never
    // the localized "Warsong Gulch": `0x4a6cf0` reads `[row+0xc]` with no locale multiplier, in
    // deliberate contrast to `0x4a8740`'s `[rec + 4*[0xc0e080] + 0x2c]` AreaTable read. That is
    // why FrameXML interpolates it straight into `Interface\WorldMap\<name>\<name>1..12`.
    g.set(
        "GetMapInfo",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let wm = &model.worldmap;
            let file = match (wm.direct_area, wm.selection) {
                // The direct-area state — an instance map, resolved by id through the orphan
                // list. `0x4a6cf0`'s `jl 0x4a6d24` sends BOTH negative continents here and the
                // `-1` world level answers nil only because its third cell is `-1` too.
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
            // THREE values, always: `0x4a7e30` pushes all three unconditionally and returns
            // `mov eax,3` from its single `ret`, so the world-level answer is `nil, 0, 0` rather
            // than one nil. (Its four conditional jumps all sit inside slot 3's power-of-two
            // round and none gates a push. `arity_conf = exact`; decision 1845.)
            //
            // The two zeros are the map art's texture dimensions, which no caller reads: both
            // consumers bind slot 2 to a `textureHeight` local they never read again. They are
            // **0 on every instance map** in the reference too — `0x4a6d50` scans
            // `WorldMapContinent.dbc` for the row's mapID and that table ships only mapIDs
            // {0, 1}, so the scan exhausts.
            let file = match file {
                Some(f) => Value::String(lua.create_string(&f)?),
                None => Value::Nil,
            };
            Ok((file, 0i64, 0i64))
        })?,
    )?;

    // GetCorpseMapPosition() → x, y in [0,1] map UV; (0,0) = no corpse / not on this map — the
    // reference's hide sentinel (WorldMapFrame.lua:445), same shape as the player position below.
    g.set(
        "GetCorpseMapPosition",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (x, y) = model.worldmap.corpse_uv.unwrap_or((0.0, 0.0));
            Ok((f64::from(x), f64::from(y)))
        })?,
    )?;

    // GetPlayerMapPosition(unit) → x, y in [0,1] map UV; (0,0) = not on this map (hide the
    // blip). `"player"`, `"party1".."party4"` (report B320) and `"raid1".."raid40"` (1980, the
    // stock map's raid arm over its `WorldMapRaid1..40` pool) resolve; anything else is (0,0).
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

    // GetPlayerFacing() → wow orientation radians (counterclockwise-positive, 0 = north). Feeds
    // the transcribed frame's arrow SetRotation (the reference's engine arrow reads facing
    // natively; a Lua-visible getter is our seam for the texture stand-in).
    g.set(
        "GetPlayerFacing",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.worldmap.player_facing))
        })?,
    )?;

    // ProcessMapClick(x, y) — normalized click within WorldMapButton (`0x4a7f30`).
    // World level: pick the first continent whose sheet-rect contains the click (the 0x4a7100
    // AABB walk; the 5875 rects are disjoint). Continent OR zone level: identical instructions —
    // the 0x4a6ec0 grid cell through the CURRENT selection's rect (the continent's, or the current
    // zone's) → a continent child index → drill. Because a city is just another child, a click on
    // its footprint — from the continent map, or from a neighbouring zone map when the footprint
    // falls in that zone's window — drills into the city's own map.
    g.set(
        "ProcessMapClick",
        lua.create_function(|lua, (x, y): (f32, f32)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // An instance map swallows the click: the drill INSIDE `ProcessMapClick`
            // (`0x4a7540`) opens `cmp esi,-2; je 0x4a760d`, a bare epilogue. Ahead of the world
            // arm below, because the direct state shares its `continent == 0` cell and must not
            // fall into the world sheet's continent walk.
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
    // scrollChildX, scrollChildY (`0x4a7fa0`) — the per-level law is [`hover`].
    // Eight values always: the frame reads `fileName` to decide whether a quad is drawn at all,
    // so a name-only answer carries a nil there and six zeros behind it.
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

    // GetNumMapLandmarks() — how many POI icons the displayed map carries: the `AreaPOI.dbc` rows
    // that pass the builder's gates, then the guard-directions marker.
    g.set(
        "GetNumMapLandmarks",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.worldmap.landmarks.len() as i64)
        })?,
    )?;

    // GetMapLandmarkInfo(i) → name, description, textureIndex, x, y — the i-th (1-based) landmark
    // (return shape at `0x4a8740`).
    // `textureIndex` indexes `Interface\Minimap\POIIcons`' 8×8 grid, already resolved app-side
    // through the reference's own leg (`0x4a8848`): a row's `Icon`, or the constant 15 at zone
    // level unless it carries `Flags & 0x80`; the guard's marker is its packet `Icon` verbatim.
    // x/y are `[0,1]` UV on the displayed map, the same space `GetPlayerMapPosition` answers in. A
    // landmark with no description answers **nil** there, as the reference does (`0x6f3890` pushes
    // nil on a NULL string) — the guard's marker never carries one, and neither do the ~2/3 of
    // AreaPOI rows with no live status text.
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

    // GetNumMapOverlays() — how many of the displayed map's overlays are REVEALED by the
    // explored bitset (the client's C side filters the same way; the reference Lua draws every
    // returned overlay). 0 at world/continent level; an instance map is admitted under the same
    // gate as a zone (see [`current_zone`]) — which on 5875 data means Alterac Valley's three
    // rows and nothing for the other two battlegrounds.
    g.set(
        "GetNumMapOverlays",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(revealed_overlays(&model.worldmap).len() as i64)
        })?,
    )?;

    // GetMapOverlayInfo(i) → textureName, textureWidth, textureHeight, offsetX, offsetY,
    // mapPointX, mapPointY — the i-th (1-based) revealed overlay. The texture is the full path
    // prefix (the Lua appends the piece number); mapPointX/Y are 0 on every 5875 row.
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
