//! Where the liquid is and whether a subject is in it: every spawned surface publishes its grid in
//! world WoW space ([`WaterChunkInfo`]), and swimming, wading, foam, the ambient loops and the
//! camera's submersion verdict ask it through [`liquid_at`] and its siblings.

use bevy::prelude::*;

use crate::view::WorldCamera;
use crate::wmo_portal::{
    CameraInteriorClaim, PlayerWmoRoom, UnitWmoRoom, WmoPortalInstance, WmoRoom,
};
use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_formats::{LiquidKind, LiquidMesh};

/// The placements a claim reads its room's whole-group override from, baked onto
/// [`WmoPortalInstance`] at spawn, so the answer leaves with the placement.
pub type RoomPlacements<'w, 's> = Query<'w, 's, &'static WmoPortalInstance>;

/// A room's whole-group submersion override, if its group carries one and its placement is loaded.
fn flood_of(room: WmoRoom, placements: &RoomPlacements) -> Option<LiquidKind> {
    placements
        .get(room.instance)
        .ok()
        .and_then(|i| i.flooded.get(room.group as usize).copied().flatten())
}

/// The camera eye's claim: the environment probe `0x6809c0` samples the containing map-object's
/// MLIQ when `[0xc7b748]` names one, else the ADT liquid.
pub fn camera_claim(claim: &CameraInteriorClaim, placements: &RoomPlacements) -> LiquidClaim {
    match claim.0 {
        Some(c) => LiquidClaim::Inside {
            room: c.room,
            flooded: flood_of(c.room, placements),
        },
        None => LiquidClaim::Outdoors,
    }
}

/// The player's claim, from `wmo_portal`'s per-frame interior down-ray.
pub fn player_claim(room: &PlayerWmoRoom, placements: &RoomPlacements) -> LiquidClaim {
    match room.0 {
        Some(room) => LiquidClaim::Inside {
            room,
            flooded: flood_of(room, placements),
        },
        None => LiquidClaim::Outdoors,
    }
}

/// A remote unit's claim, from its own room; [`LiquidClaim::Unknown`] only on its first frame,
/// before `wmo_portal::track_unit_interiors` reaches it.
pub fn unit_claim(room: Option<&UnitWmoRoom>, placements: &RoomPlacements) -> LiquidClaim {
    match room.map(UnitWmoRoom::room) {
        Some(Some(room)) => LiquidClaim::Inside {
            room,
            flooded: flood_of(room, placements),
        },
        Some(None) => LiquidClaim::Outdoors,
        None => LiquidClaim::Unknown,
    }
}

/// Which liquid the camera eye is in, set by [`detect_submersion`]. Lighting selects the whole
/// submerged atmosphere from it, with no overlay quad: water and ocean take the zone's underwater
/// `LightParams` slot, magma and slime fixed global rows (`0x6d2371`).
#[derive(Resource, Default)]
pub struct Underwater(pub(crate) benilla_formats::Submersion);

/// Where a surface came from and, for WMO liquid, whose room it is: the scope key for
/// [`liquid_at`]. The reference's terrain query `0x69b6d0` tries the map-object leg `0x69b520`
/// first, which moves the point into each map-object's own space before sampling MLIQ, then the
/// ADT; the camera probe `0x6809c0` samples the current group's MLIQ (`0x6b9f10`) when
/// `[0xc7b748]` names a containing map-object, else the ADT query (`0x6723d0` → `0x69b6d0`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LiquidSource {
    /// An ADT map-chunk surface (MCLQ).
    AdtChunk,
    /// A WMO group's surface (MLIQ), with the room that owns it and that room's floor.
    WmoGroup(WmoPool),
}

/// A WMO pool's scope: `owner` bounds its footprint to one placement, `floor` to one storey.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WmoPool {
    /// The room this pool belongs to; `None` for a placement that spawned no `WmoPortalInstance`
    /// (no portals, no `WMOAreaTable` identity), which no interior claim can then reach.
    pub(crate) owner: Option<WmoRoom>,
    /// World WoW Z of the owning group's bounding-box floor: the pool claims no subject below it.
    /// The reference's per-group box test (`0x6a4e00`) never picks an interior group: it skips
    /// every group whose flags match its mask, and its one caller passes `0x2000`, the interior
    /// flag (`0x69b570` → `0x69b575`). This floor bounds interior and exterior pools alike, in Z
    /// only, since wet cells reach up to 25 yd outside their group's box in XY (Ahn'Qiraj,
    /// Stratholme). `NEG_INFINITY` for unknown bounds, so a missing box fails open.
    pub(crate) floor: f32,
}

impl WmoPool {
    /// The pool's scope under a placement `transform`: the floor is the lowest of the box's eight
    /// transformed corners, since a rotated box's lowest corner is not its `bbox_min`.
    pub(crate) fn new(
        owner: Option<WmoRoom>,
        transform: &Transform,
        bounds: Option<&benilla_formats::WmoGroupInfo>,
    ) -> Self {
        let Some(g) = bounds else {
            return Self {
                owner,
                floor: f32::NEG_INFINITY,
            };
        };
        let mut floor = f32::INFINITY;
        for x in [g.bbox_min[0], g.bbox_max[0]] {
            for y in [g.bbox_min[1], g.bbox_max[1]] {
                for z in [g.bbox_min[2], g.bbox_max[2]] {
                    // Bevy's +Y is WoW's +Z, so a transformed corner's `y` is its height.
                    floor = floor.min(transform.transform_point(wow_to_bevy([x, y, z])).y);
                }
            }
        }
        Self { owner, floor }
    }
}

/// Whose liquid answers for a subject: the player's room ([`PlayerWmoRoom`]), the camera eye's
/// ([`CameraInteriorClaim`], the reference's `[0xc7b748]`) or a remote unit's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LiquidClaim {
    /// In the open world: only the ADT's MCLQ answers.
    Outdoors,
    /// Inside this placed building: only that placement's MLIQ answers.
    Inside {
        room: WmoRoom,
        /// The room's whole-group submersion override: its MOGP `groupLiquid` when not the `0xf`
        /// sentinel. The group probe `0x6b9f10` reads it first and, when set, hits unconditionally:
        /// the raw value as the kind, `FLT_MAX` as the height, no Z compare and no MLIQ test. None
        /// of the 13 groups that set it has an MLIQ; it floods the Deeprun Tram's submerged
        /// sections, the Prison Oubliette and two MD caves.
        flooded: Option<LiquidKind>,
    },
    /// No claim yet (a subject's first frame, before its tracker runs): both sources answer.
    Unknown,
}

impl LiquidClaim {
    /// A claim on a room with no whole-group override, for callers with no placement to read.
    #[cfg(test)]
    pub(crate) fn inside(room: WmoRoom) -> Self {
        Self::Inside {
            room,
            flooded: None,
        }
    }
}

/// One liquid surface of any kind as the queries see it: its grid in world WoW space, placement
/// baked in, with its XY bounds, source and kind. Wet-or-dry and the surface height both come from
/// the cell containing the XY; the box is only a cheap reject.
#[derive(Component)]
pub struct WaterChunkInfo {
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
    source: LiquidSource,
    kind: LiquidKind,
    grid: LiquidGrid,
}

/// A surface's vertex grid in world WoW space and its lattice basis. A MODF placement is affine, so
/// the XY positions stay `origin + i·u + j·v` (to 0.0005 yd on Blackrock's rotated 55×82 grid) and
/// a world XY inverts to a cell with one 2×2 solve.
struct LiquidGrid {
    cols: usize,
    rows: usize,
    /// Vertex positions, world WoW, row-major `j·cols + i`.
    positions: Vec<[f32; 3]>,
    /// Per-cell liquid coverage, row-major over `(cols−1) × (rows−1)`.
    wet: Vec<bool>,
    /// Grid vertex `(0, 0)`, XY.
    origin: [f32; 2],
    /// World XY step per `+1` in `i` / in `j`, over the full span (`(last − first)/n`): one
    /// adjacent pair's f32 error at world magnitude drifts 0.02 yd across Blackrock's 54 cells.
    u: [f32; 2],
    v: [f32; 2],
    /// `1/det` of the `[u v]` basis; `None` when degenerate in XY, where queries use the bounds.
    inv_det: Option<f32>,
    /// The highest wet vertex: the degenerate fallback only, never the surface of a sampled grid.
    fallback_z: f32,
}

/// How far outside the grid, in cells, a query still snaps in, so a lake's rim is wet on a tie.
const GRID_EDGE_TOLERANCE: f32 = 1e-3;

impl LiquidGrid {
    /// The wet cell containing this world XY and the in-cell fractions `(i, j, fx, fy)`. A grid is
    /// sparse (MLIQ tile nibble `0xf` is a hole, MCLQ likewise), so its box spans dry ground: one
    /// Stormwind MLIQ box, 95 × 80 yd, covers a canal and the dry tunnel beside it.
    fn wet_cell_at(&self, x: f32, y: f32) -> Option<(usize, usize, f32, f32)> {
        let (cells_x, cells_y) = (self.cols.checked_sub(1)?, self.rows.checked_sub(1)?);
        let inv_det = self.inv_det?;
        // Invert the lattice basis: p − origin = a·u + b·v, solved in cell units.
        let (dx, dy) = (x - self.origin[0], y - self.origin[1]);
        let a = (dx * self.v[1] - dy * self.v[0]) * inv_det;
        let b = (self.u[0] * dy - self.u[1] * dx) * inv_det;
        let snap = |t: f32, cells: usize| -> Option<(usize, f32)> {
            if t < -GRID_EDGE_TOLERANCE || t > cells as f32 + GRID_EDGE_TOLERANCE {
                return None;
            }
            // The last cell owns its far edge, so `t == cells` lands in cell `cells−1` at f = 1.
            let idx = (t.floor().max(0.0) as usize).min(cells - 1);
            Some((idx, (t - idx as f32).clamp(0.0, 1.0)))
        };
        let (i, fx) = snap(a, cells_x)?;
        let (j, fy) = snap(b, cells_y)?;
        self.wet.get(j * cells_x + i)?.then_some((i, j, fx, fy))
    }

    /// The surface height (WoW Z) in a cell: the bilinear over its four corners, as `0x6b7500`
    /// lerps along one axis and then the other.
    fn height_in_cell(&self, i: usize, j: usize, fx: f32, fy: f32) -> f32 {
        let z = |i: usize, j: usize| self.positions[j * self.cols + i][2];
        let t1 = z(i, j) + (z(i + 1, j) - z(i, j)) * fx;
        let t2 = z(i, j + 1) + (z(i + 1, j + 1) - z(i, j + 1)) * fx;
        t1 + (t2 - t1) * fy
    }

    /// The `(lowest, highest)` wet vertex, for the `/liquid` instrument.
    fn wet_z_range(&self) -> (f32, f32) {
        let Some(cells_x) = self.cols.checked_sub(1) else {
            return (self.fallback_z, self.fallback_z);
        };
        let mut lo = f32::MAX;
        for cell in (0..self.wet.len()).filter(|&c| self.wet[c]) {
            let (i, j) = (cell % cells_x, cell / cells_x);
            for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                lo = lo.min(self.positions[(j + dj) * self.cols + i + di][2]);
            }
        }
        (lo.min(self.fallback_z), self.fallback_z)
    }
}

impl WaterChunkInfo {
    /// The grid's highest wet vertex, which `super::real_data` shows is not the surface.
    #[cfg(test)]
    pub(super) fn chunk_max_z(&self) -> f32 {
        self.grid.fallback_z
    }

    /// A footprint from a world-space grid of `cols × rows` positions and one wet flag per cell;
    /// bounds and fallback height come from the wet cells' corners only.
    pub fn new(
        source: LiquidSource,
        kind: LiquidKind,
        grid: [usize; 2],
        positions: Vec<[f32; 3]>,
        wet: Vec<bool>,
    ) -> Self {
        // Mismatched dimensions make an empty grid that claims nothing and walks nothing.
        let [cols, rows] = grid;
        let sane = cols >= 2
            && rows >= 2
            && positions.len() == cols * rows
            && wet.len() == (cols - 1) * (rows - 1);
        if !sane {
            return WaterChunkInfo {
                min_x: f32::MAX,
                max_x: f32::MIN,
                min_y: f32::MAX,
                max_y: f32::MIN,
                source,
                kind,
                grid: LiquidGrid {
                    cols: 0,
                    rows: 0,
                    positions: Vec::new(),
                    wet: Vec::new(),
                    origin: [0.0; 2],
                    u: [0.0; 2],
                    v: [0.0; 2],
                    inv_det: None,
                    fallback_z: f32::MIN,
                },
            };
        }
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        let (mut min_y, mut max_y) = (f32::MAX, f32::MIN);
        let mut fallback_z = f32::MIN;
        for cell in (0..wet.len()).filter(|&c| wet[c]) {
            let (i, j) = (cell % (cols - 1), cell / (cols - 1));
            for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = positions[(j + dj) * cols + i + di];
                min_x = min_x.min(p[0]);
                max_x = max_x.max(p[0]);
                min_y = min_y.min(p[1]);
                max_y = max_y.max(p[1]);
                fallback_z = fallback_z.max(p[2]);
            }
        }
        // The span-derived basis; a plane on edge has no XY area and leaves `inv_det` `None`.
        let origin = [positions[0][0], positions[0][1]];
        let step = |far: [f32; 3], n: usize| {
            [
                (far[0] - origin[0]) / n as f32,
                (far[1] - origin[1]) / n as f32,
            ]
        };
        let u = step(positions[cols - 1], cols - 1);
        let v = step(positions[(rows - 1) * cols], rows - 1);
        let det = u[0] * v[1] - u[1] * v[0];
        WaterChunkInfo {
            min_x,
            max_x,
            min_y,
            max_y,
            source,
            kind,
            grid: LiquidGrid {
                cols,
                rows,
                positions,
                wet,
                origin,
                u,
                v,
                inv_det: (det.abs() > 1e-9).then(|| 1.0 / det),
                fallback_z,
            },
        }
    }

    /// The surface height (WoW Z) at a WoW-space XY, `None` where dry: the one wet-or-dry question,
    /// answered by the containing cell's bilinear, never by the grid's highest vertex.
    pub(crate) fn surface_z_at(&self, x: f32, y: f32) -> Option<f32> {
        if !self.contains(x, y) {
            return None; // the bounding box is the cheap reject
        }
        match self.grid.wet_cell_at(x, y) {
            Some((i, j, fx, fy)) => Some(self.grid.height_in_cell(i, j, fx, fy)),
            // A grid that cannot be inverted answers its whole box at the highest wet vertex: a
            // wrong "dry" drops a player through a lake.
            None if self.grid.inv_det.is_none() => Some(self.grid.fallback_z),
            None => None,
        }
    }

    /// Is this WoW-space XY inside the wet footprint's box?
    pub(crate) fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }

    /// The wet footprint's XY box, `None` for an empty grid: what [`super::spatial::WaterIndex`]
    /// buckets by.
    pub(super) fn xy_bounds(&self) -> Option<[[f32; 2]; 2]> {
        (self.min_x <= self.max_x && self.min_y <= self.max_y)
            .then_some([[self.min_x, self.min_y], [self.max_x, self.max_y]])
    }

    /// Does this surface answer for a subject holding `claim` at WoW height `z`? The pool floor
    /// ([`WmoPool::floor`]) holds under every claim, `Unknown` included.
    fn answers(&self, claim: LiquidClaim, z: f32) -> bool {
        match (claim, self.source) {
            (_, LiquidSource::WmoGroup(pool)) if z < pool.floor => false,
            (LiquidClaim::Unknown, _) => true,
            (LiquidClaim::Outdoors, LiquidSource::AdtChunk) => true,
            (LiquidClaim::Outdoors, LiquidSource::WmoGroup(_)) => false,
            (LiquidClaim::Inside { .. }, LiquidSource::AdtChunk) => false,
            (LiquidClaim::Inside { room, .. }, LiquidSource::WmoGroup(pool)) => {
                pool.owner.is_some_and(|o| o.instance == room.instance)
            }
        }
    }

    /// The room this surface belongs to, for the instruments; `None` for ADT and unowned liquid.
    fn owner(&self) -> Option<WmoRoom> {
        match self.source {
            LiquidSource::AdtChunk => None,
            LiquidSource::WmoGroup(pool) => pool.owner,
        }
    }

    /// Does this WoW-space XY box overlap the wet footprint's box?
    pub(crate) fn overlaps(&self, lo_x: f32, hi_x: f32, lo_y: f32, hi_y: f32) -> bool {
        hi_x >= self.min_x && lo_x <= self.max_x && hi_y >= self.min_y && lo_y <= self.max_y
    }

    /// Call `f` with every wet cell's four world WoW corners `[tl, tr, bl, br]`, for the foam clip.
    pub(crate) fn for_each_wet_cell(&self, mut f: impl FnMut([[f32; 3]; 4])) {
        let g = &self.grid;
        let Some(cells_x) = g.cols.checked_sub(1) else {
            return;
        };
        for cell in (0..g.wet.len()).filter(|&c| g.wet[c]) {
            let (i, j) = (cell % cells_x, cell / cells_x);
            let p = |di: usize, dj: usize| g.positions[(j + dj) * g.cols + i + di];
            f([p(0, 0), p(1, 0), p(0, 1), p(1, 1)]);
        }
    }

    /// The ambient loop's emitter target: the XY clamped into the footprint's box, at the surface's
    /// height there, or the highest wet vertex over a hole. The reference uses the nearest liquid
    /// cell; this clamp approximates it.
    pub(crate) fn nearest_point_wow(&self, x: f32, y: f32) -> [f32; 3] {
        let cx = x.clamp(self.min_x, self.max_x);
        let cy = y.clamp(self.min_y, self.max_y);
        [
            cx,
            cy,
            self.surface_z_at(cx, cy).unwrap_or(self.grid.fallback_z),
        ]
    }
}

/// A surface's [`WaterChunkInfo`], its grid carried into world WoW space by `transform`:
/// `IDENTITY` for MCLQ, whose positions are already absolute (the axis round trip is exact), the
/// MODF placement for model-local WMO liquid.
pub(super) fn wet_footprint(
    lq: &LiquidMesh,
    transform: &Transform,
    source: LiquidSource,
) -> WaterChunkInfo {
    let positions: Vec<[f32; 3]> = lq
        .positions
        .iter()
        .map(|&p| world_wow(transform, p))
        .collect();
    WaterChunkInfo::new(
        source,
        lq.kind,
        [lq.grid[0] as usize, lq.grid[1] as usize],
        positions,
        lq.wet.clone(),
    )
}

fn world_wow(transform: &Transform, local: [f32; 3]) -> [f32; 3] {
    bevy_to_wow(transform.transform_point(wow_to_bevy(local)))
}

/// Marks a water surface that grows foam; the cells it clips against are on [`WaterChunkInfo`].
#[derive(Component)]
pub struct FoamPatch;

/// The liquid a query landed in: its surface height (WoW Z) and kind.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LiquidHit {
    pub surface_z: f32,
    pub kind: LiquidKind,
}

/// The liquid over a WoW-space position under `claim`: the shared query of swimming, submersion,
/// wading and the enter-water sounds. `Inside` reads only that placement's MLIQ and `Outdoors`
/// only MCLQ. The reference's terrain query `0x69b6d0` reads the map-object leg first (`0x69b6ec`
/// → `0x69b520` → `0x6a4e00`, exterior groups only), then the ADT, so an outdoor subject in an
/// exterior group's pool reads that pool, which `Outdoors` here never does. Where surfaces stack,
/// the lowest at this XY wins, the one a subject between them is in.
pub fn liquid_at<'a>(
    liquids: impl Iterator<Item = &'a WaterChunkInfo>,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> Option<LiquidHit> {
    // The whole-group override answers first and alone, as `0x6b9f10` reads it before any grid;
    // as one more candidate, `min_by` would hand the room to any lower sibling pool.
    if let LiquidClaim::Inside {
        flooded: Some(kind),
        ..
    } = claim
    {
        return Some(LiquidHit {
            surface_z: f32::MAX,
            kind,
        });
    }
    liquids
        .filter(|w| w.answers(claim, wow[2]))
        .filter_map(|w| {
            w.surface_z_at(wow[0], wow[1]).map(|surface_z| LiquidHit {
                surface_z,
                kind: w.kind,
            })
        })
        .min_by(|a, b| a.surface_z.total_cmp(&b.surface_z))
}

/// Every loaded footprint containing this WoW XY, one line each: the `/liquid` chat instrument.
/// Each line names the surface's owner and floor, whether the claim admits it, the cell and height
/// sampled and the surface's Z range, so a wrong claim and a wrong height read apart.
pub fn describe_at<'a>(
    liquids: impl Iterator<Item = &'a WaterChunkInfo>,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> Vec<String> {
    // A flooded room has no footprint to list, so the override gets a line of its own.
    let mut lines = Vec::new();
    if let LiquidClaim::Inside {
        room,
        flooded: Some(kind),
    } = claim
    {
        lines.push(format!(
            "WHOLE-GROUP OVERRIDE g{} ◀ YOURS {kind:?} — the ROOM is submerged (MOGP groupLiquid, \
             no MLIQ grid): surface FLT_MAX, no Z bound, nothing below to sample",
            room.group,
        ));
    }
    let mut out: Vec<(f32, String)> = liquids
        .filter(|w| w.contains(wow[0], wow[1]))
        .map(|w| {
            let z = w.surface_z_at(wow[0], wow[1]);
            let (lo, hi) = w.grid.wet_z_range();
            let here = match (z, w.grid.wet_cell_at(wow[0], wow[1])) {
                (Some(z), Some((i, j, fx, fy))) => format!(
                    "WET-CELL surface z {z:.2} ({:+.2} over feet)  cell [{i},{j}] +({fx:.2},{fy:.2})",
                    z - wow[2]
                ),
                (Some(z), None) => format!(
                    "no-grid (bounds fallback) surface z {z:.2} ({:+.2} over feet)",
                    z - wow[2]
                ),
                (None, _) => "box-only (dry here)".to_string(),
            };
            // The floor too, or a pool rejected for its storey reads like another building's.
            let owner = match w.source {
                LiquidSource::AdtChunk => "AdtChunk".to_string(),
                LiquidSource::WmoGroup(pool) => match pool.owner {
                    Some(o) => format!(
                        "WmoGroup {:?} g{} floor {:.2}",
                        o.instance, o.group, pool.floor
                    ),
                    None => "WmoGroup (unowned)".to_string(),
                },
            };
            (
                z.unwrap_or(hi),
                format!(
                    "{owner} {} {:?} {here}  grid z [{lo:.2}..{hi:.2}]  xy [{:.0}..{:.0}, {:.0}..{:.0}]",
                    if w.answers(claim, wow[2]) {
                        "◀ YOURS"
                    } else {
                        "· other room"
                    },
                    w.kind,
                    w.min_x,
                    w.max_x,
                    w.min_y,
                    w.max_y,
                ),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines.extend(out.into_iter().map(|(_, line)| line));
    lines
}

/// Every surface height over `wow`'s XY that the claim admits, above or below the subject, for the
/// effect lane's water-side test (`particles::sim::far_side_of_water_at`).
pub fn surfaces_at<'a>(
    liquids: impl Iterator<Item = &'a WaterChunkInfo> + 'a,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> impl Iterator<Item = f32> + 'a {
    liquids
        .filter(move |w| w.answers(claim, wow[2]))
        .filter_map(move |w| w.surface_z_at(wow[0], wow[1]))
}

/// [`liquid_at`] over the water kinds only, for the wade splash, footstep depth and spline depth;
/// swimming and the submerged atmosphere take every kind.
pub fn water_surface_at<'a>(
    water: impl Iterator<Item = &'a WaterChunkInfo>,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> Option<f32> {
    liquid_at(water.filter(|w| !w.kind.is_fullbright()), wow, claim).map(|h| h.surface_z)
}

// No wade constant: wading is the complement of swimming (`player::swim_enter_depth`).

/// The eye's accept margin on the water kinds, `z < surface + 0.01` strict (`0x69b6d0`, compared
/// at `0x69ba23` against the f32 `0x3c23d70a` at `0x8029d0`); the WMO magma/slime compare has none.
const SUBMERSION_EPS: f32 = 0.01;

/// The submerged atmosphere a liquid kind selects.
fn submersion_of(kind: LiquidKind) -> benilla_formats::Submersion {
    use benilla_formats::Submersion;
    match kind {
        LiquidKind::Still | LiquidKind::Rapids => Submersion::Water,
        // Ocean alone runs the depth ramp; only ADT MCLQ authors it (`(b & 0x0f) & 3 == 1`).
        LiquidKind::Ocean => Submersion::Ocean,
        LiquidKind::Magma => Submersion::Magma,
        LiquidKind::Slime => Submersion::Slime,
    }
}

/// The verdict alone, for tests: [`submersion_claim_at`] without the height.
#[cfg(test)]
pub(super) fn submersion_at<'a>(
    liquids: impl Iterator<Item = &'a WaterChunkInfo>,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> benilla_formats::Submersion {
    submersion_claim_at(liquids, wow, claim).map_or_else(Default::default, |(s, _)| s)
}

/// The liquid a WoW position is submerged in and that surface's height, one pass for both, as the
/// reference's probe writes the pair (`0x680a87`/`0x680ab0`). Every admitted kind counts; the
/// position must be under a wet cell's bilinear surface, as `0x69b6d0` samples its 9×9 grid, and
/// where surfaces stack the lowest one over the position wins.
pub(super) fn submersion_claim_at<'a>(
    liquids: impl Iterator<Item = &'a WaterChunkInfo>,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> Option<(benilla_formats::Submersion, f32)> {
    liquids
        .filter(|w| w.answers(claim, wow[2]))
        .filter_map(|w| {
            let z = w.surface_z_at(wow[0], wow[1])?;
            let eps = if w.kind.is_fullbright() {
                0.0
            } else {
                SUBMERSION_EPS
            };
            (wow[2] < z + eps).then_some((z, submersion_of(w.kind)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(z, s)| (s, z))
}

/// The camera eye's liquid state as one system parameter: the verdict and its two scalars, written
/// together in [`super::SubmersionVerdict`] and so read together.
#[derive(bevy::ecs::system::SystemParam)]
pub struct EyeLiquid<'w> {
    verdict: Option<Res<'w, Underwater>>,
    eye: Option<Res<'w, SubmergedEye>>,
}

impl EyeLiquid<'_> {
    /// What the eye is in; `Dry` before the probe first runs.
    pub fn submersion(&self) -> benilla_formats::Submersion {
        self.verdict.as_ref().map_or_else(Default::default, |v| v.0)
    }

    /// The eye's absolute world Z, the ocean ramp's input.
    pub fn eye_z(&self) -> f32 {
        self.eye.as_ref().map_or(0.0, |e| e.eye_z)
    }

    /// How far the probe sits below the liquid surface, in yards; `0.0` when dry.
    pub fn depth(&self) -> f32 {
        self.eye.as_ref().map_or(0.0, |e| e.depth)
    }
}

/// The submerged camera's two scalars, published beside [`Underwater`].
#[derive(bevy::prelude::Resource, Default, Clone, Copy)]
pub struct SubmergedEye {
    /// `liquidSurfaceHeight − probeZ` in yards, positive when submerged and `0.0` when dry, as the
    /// reference writes it at `0x680a87`/`0x680ab0`; drives the glare's 10 yd fade.
    pub depth: f32,
    /// The eye's absolute world Z. The ocean ramp clamps it raw, `clamp(eye.z, −30, 0)`, since
    /// ocean surfaces are pinned to z = 0 by a branch (`0x69ba85`), not by data.
    pub eye_z: f32,
}

/// How far the near rectangle's lowest corner sits below the eye, `≤ 0` yd: `eye_z + drop` is the
/// reference's probe height `min(eye.z, corner[0..3].z)`, its corners built from NDC z = −1
/// (`0x5c43b0`).
pub(super) fn lowest_near_corner_drop(rotation: Quat, fov: f32, aspect: f32, near: f32) -> f32 {
    let half_h = (fov * 0.5).tan() * near;
    let half_w = half_h * aspect;
    let mut drop: f32 = 0.0;
    for sx in [-1.0, 1.0] {
        for sy in [-1.0, 1.0] {
            drop = drop.min((rotation * Vec3::new(sx * half_w, sy * half_h, -near)).y);
        }
    }
    drop
}

// The reference's camera probe spans `[0x6809c0, 0x680b90)`: it reads the render eye
// `[0xc7cf20/24/28]`, samples one source, the current group's MLIQ (`0x6b9f10`) when `[0xc7b748]`
// names a containing map-object, else the ADT liquid (`0x6723d0` → `0x69b6d0`), and writes
// `[0xc7f288]`. `CameraInteriorClaim` comes off the down-ray that writes `[0xc7b748]`.
pub(super) fn detect_submersion(
    mut underwater: ResMut<Underwater>,
    camera: Query<(&Transform, &Projection), With<WorldCamera>>,
    water: Query<&WaterChunkInfo>,
    eye_claim: Res<crate::wmo_portal::CameraInteriorClaim>,
    placements: RoomPlacements,
    time: Res<Time>,
    mut submerged_eye: ResMut<SubmergedEye>,
    mut last_dump: Local<Option<u32>>,
) {
    let Ok((cam, proj)) = camera.single() else {
        return;
    };
    let claim = camera_claim(&eye_claim, &placements);
    let eye = bevy_to_wow(cam.translation);
    // `0x6809c0` tests the eye's XY at the near rectangle's lowest point, `min(eye.z,
    // corner[0..3].z)`, so the frame turns submerged as the view's leading corner reaches the
    // surface and no under-surface view renders dry, with no camera constraint needed.
    let probe_z = match proj {
        Projection::Perspective(p) => {
            eye[2] + lowest_near_corner_drop(cam.rotation, p.fov, p.aspect_ratio, p.near)
        }
        _ => eye[2],
    };
    let verdict = submersion_claim_at(water.iter(), [eye[0], eye[1], probe_z], claim);
    underwater.0 = verdict.map(|(s, _)| s).unwrap_or_default();
    // Depth from the probe, not the eye, so the fade agrees with the verdict.
    submerged_eye.depth = verdict.map_or(0.0, |(_, z)| (z - probe_z).max(0.0));
    submerged_eye.eye_z = eye[2];
    // `WOW_FOG_DUMP`: once a second, the eye, the verdict and every surface over the eye's XY.
    static FOG_DUMP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *FOG_DUMP.get_or_init(|| std::env::var_os("WOW_FOG_DUMP").is_some()) {
        let sec = time.elapsed_secs() as u32;
        if last_dump.replace(sec) != Some(sec) {
            // Rejected candidates too, marked, since a wrong verdict needs them seen.
            let mut cands: Vec<String> = water
                .iter()
                .filter_map(|w| {
                    w.surface_z_at(eye[0], eye[1]).map(|z| {
                        // The owner's group tells another storey's pool from the room's own.
                        format!(
                            "{:?} z {z:.2} {}{}",
                            w.kind,
                            match w.owner() {
                                Some(o) => format!("g{}", o.group),
                                None => "adt".into(),
                            },
                            if w.answers(claim, probe_z) {
                                ""
                            } else {
                                " (not yours)"
                            }
                        )
                    })
                })
                .collect();
            cands.sort();
            eprintln!(
                "[submerged] {:?} claim {claim:?} eye [{:.1} {:.1} {:.2}] probe-z {probe_z:.2} over-xy {}",
                underwater.0,
                eye[0],
                eye[1],
                eye[2],
                if cands.is_empty() {
                    "(no surface covers the eye's XY)".to_string()
                } else {
                    cands.join(", ")
                }
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Level reaches half the near rectangle's height down, straight down the full near distance,
    /// straight up nothing, and yaw never enters.
    #[test]
    fn near_corner_drop_is_the_rectangles_lowest_point() {
        let (fov, aspect, near) = (std::f32::consts::FRAC_PI_4, 16.0 / 9.0, 1.0 / 9.0);
        let half_h = (fov * 0.5).tan() * near;
        let at = |yaw: f32, pitch: f32| {
            lowest_near_corner_drop(
                Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0),
                fov,
                aspect,
                near,
            )
        };
        assert!((at(0.0, 0.0) + half_h).abs() < 1e-6);
        assert!((at(0.0, -std::f32::consts::FRAC_PI_2) + near).abs() < 1e-6);
        assert_eq!(at(0.0, std::f32::consts::FRAC_PI_2), 0.0);
        assert!((at(1.23, -0.4) - at(0.0, -0.4)).abs() < 1e-6);
    }

    /// A flat one-cell 10 yd wet quad at WoW z = `z`.
    fn flat_quad(z: f32) -> LiquidMesh {
        LiquidMesh {
            grid: [2, 2],
            wet: vec![true],
            shared: vec![false],
            positions: vec![
                [0.0, 0.0, z],
                [10.0, 0.0, z],
                [0.0, 10.0, z],
                [10.0, 10.0, z],
            ],
            uvs: vec![[0.0, 0.0]; 4],
            depths: vec![1.0; 4],
            indices: vec![0, 1, 2, 1, 3, 2],
            sound_nibble: 0,
            material_id: None,
            kind: LiquidKind::Still,
        }
    }

    /// A `cols × rows` grid of `step`-yard cells from the origin, with heights `z(i, j)`.
    fn grid_info(
        source: LiquidSource,
        kind: LiquidKind,
        cols: usize,
        rows: usize,
        step: f32,
        wet: Vec<bool>,
        z: impl Fn(usize, usize) -> f32,
    ) -> WaterChunkInfo {
        let mut positions = Vec::with_capacity(cols * rows);
        for j in 0..rows {
            for i in 0..cols {
                positions.push([i as f32 * step, j as f32 * step, z(i, j)]);
            }
        }
        WaterChunkInfo::new(source, kind, [cols, rows], positions, wet)
    }

    /// A flat one-cell surface at `z`, 10 yd square.
    fn flat_info(source: LiquidSource, kind: LiquidKind, z: f32) -> WaterChunkInfo {
        grid_info(source, kind, 2, 2, 10.0, vec![true], move |_, _| z)
    }

    fn placement(n: u32) -> Entity {
        Entity::from_raw_u32(n).expect("valid entity id")
    }

    /// A WMO pool owned by placement `owner`, group 0, with no floor.
    fn wmo_info(owner: u32, kind: LiquidKind, z: f32) -> WaterChunkInfo {
        flat_info(
            LiquidSource::WmoGroup(wmo_pool(owner, f32::NEG_INFINITY)),
            kind,
            z,
        )
    }

    fn wmo_pool(owner: u32, floor: f32) -> WmoPool {
        WmoPool {
            owner: Some(WmoRoom {
                instance: placement(owner),
                group: 0,
            }),
            floor,
        }
    }

    /// An unowned, unfloored pool: a portal-less placement's.
    fn orphan_pool() -> WmoPool {
        WmoPool {
            owner: None,
            floor: f32::NEG_INFINITY,
        }
    }

    fn inside(owner: u32) -> LiquidClaim {
        LiquidClaim::inside(WmoRoom {
            instance: placement(owner),
            group: 0,
        })
    }

    /// The claim of a subject in placement `owner`'s group 0, flooded whole with `kind`.
    fn inside_flooded(owner: u32, kind: LiquidKind) -> LiquidClaim {
        LiquidClaim::Inside {
            room: WmoRoom {
                instance: placement(owner),
                group: 0,
            },
            flooded: Some(kind),
        }
    }

    /// A flooded room has no MLIQ at all, and `0x6b9f10`'s override leg runs before any Z compare.
    #[test]
    fn a_flooded_room_is_submerged_at_every_z_with_no_surface_in_the_world() {
        let claim = inside_flooded(1, LiquidKind::Still);
        for z in [-9000.0_f32, -125.4, 0.0, 4000.0] {
            let hit = liquid_at(std::iter::empty(), [10.0, 20.0, z], claim)
                .expect("a flooded room answers with no surfaces loaded at all");
            assert_eq!(hit.kind, LiquidKind::Still);
            assert_eq!(
                hit.surface_z,
                f32::MAX,
                "the override's height is FLT_MAX (0x7f7fffff), not +inf"
            );
            assert!(
                hit.surface_z - z > 0.0,
                "so every consumer that measures depth as surface−feet reads deep, at any z"
            );
        }
    }

    /// `0x6b9f10` reads `groupLiquid` before the grid, so a sibling pool never gets a vote, where
    /// as a `min_by` candidate `FLT_MAX` would lose to it.
    #[test]
    fn the_override_outranks_a_sibling_pool_rather_than_losing_the_min() {
        let sibling = wet_footprint(
            &flat_quad(-100.0),
            &Transform::IDENTITY,
            LiquidSource::WmoGroup(wmo_pool(1, f32::NEG_INFINITY)),
        );
        let feet = [5.0, 5.0, -50.0];

        let plain = liquid_at(std::iter::once(&sibling), feet, inside(1)).expect("the pool");
        assert_eq!(plain.surface_z, -100.0);

        let flooded = liquid_at(
            std::iter::once(&sibling),
            feet,
            inside_flooded(1, LiquidKind::Still),
        )
        .expect("the room");
        assert_eq!(flooded.surface_z, f32::MAX);
    }

    /// A group carrying the `0xf` sentinel takes the plain query: the override only adds water.
    #[test]
    fn a_room_without_the_override_is_the_old_query_unchanged() {
        assert!(liquid_at(std::iter::empty(), [0.0, 0.0, 0.0], inside(1)).is_none());
        assert!(liquid_at(std::iter::empty(), [0.0, 0.0, 0.0], LiquidClaim::Outdoors).is_none());
    }

    /// `/liquid` in a flooded room names the override, which has no footprint to list.
    #[test]
    fn the_instrument_names_the_override_it_has_no_footprint_for() {
        let lines = describe_at(
            std::iter::empty(),
            [0.0, 0.0, -125.0],
            inside_flooded(1, LiquidKind::Still),
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("WHOLE-GROUP OVERRIDE g0"), "{}", lines[0]);
        assert!(describe_at(std::iter::empty(), [0.0, 0.0, 0.0], inside(1)).is_empty());
    }

    /// Under `IDENTITY` the axis round trip is a pure permutation, so MCLQ bounds are bit-exact.
    #[test]
    fn identity_footprint_is_the_raw_bounds() {
        let info = wet_footprint(
            &flat_quad(5.0),
            &Transform::IDENTITY,
            LiquidSource::AdtChunk,
        );
        assert_eq!((info.min_x, info.max_x), (0.0, 10.0));
        assert_eq!((info.min_y, info.max_y), (0.0, 10.0));
        assert_eq!(info.surface_z_at(5.0, 5.0), Some(5.0));
    }

    /// Under any yaw plus a lift the surface stays level at the local height plus the lift, and the
    /// cell lookup still finds the spun quad's centre.
    #[test]
    fn yaw_placement_keeps_the_surface_level() {
        let lift = 3.0_f32;
        for deg in [0.0_f32, 30.0, 90.0, 200.0, 355.0] {
            let transform = Transform {
                translation: Vec3::new(100.0, lift, -50.0), // Bevy +Y = WoW +Z lift
                rotation: Quat::from_rotation_y(deg.to_radians()), // yaw about vertical
                scale: Vec3::ONE,
            };
            let info = wet_footprint(
                &flat_quad(5.0),
                &transform,
                LiquidSource::WmoGroup(orphan_pool()),
            );
            let centre = bevy_to_wow(transform.transform_point(wow_to_bevy([5.0, 5.0, 5.0])));
            let z = info
                .surface_z_at(centre[0], centre[1])
                .unwrap_or_else(|| panic!("yaw {deg}°: centre {centre:?} off the grid"));
            assert!(
                (z - (5.0 + lift)).abs() < 1e-3,
                "yaw {deg}°: surface not level (got {z})"
            );
        }
    }

    /// The surface at an XY is the bilinear of its cell's corners, never the grid's highest vertex.
    #[test]
    fn the_surface_is_the_bilinear_of_its_cell_not_the_chunk_maximum() {
        // Corners 0 / 4 (+x) / 2 (+y) / 8 (+x+y): a twist no plane through three corners fits.
        let info = grid_info(
            LiquidSource::AdtChunk,
            LiquidKind::Still,
            2,
            2,
            10.0,
            vec![true],
            |i, j| match (i, j) {
                (0, 0) => 0.0,
                (1, 0) => 4.0,
                (0, 1) => 2.0,
                _ => 8.0,
            },
        );
        for (x, y, want) in [
            (0.0, 0.0, 0.0),  // the corners are exact
            (10.0, 0.0, 4.0), // …including the far edges, which the last cell owns
            (0.0, 10.0, 2.0),
            (10.0, 10.0, 8.0),
            (5.0, 0.0, 2.0), // midway along the near edge
            (5.0, 5.0, 3.5), // the centre: (0 + 4 + 2 + 8)/4
            // An interior sample: lerp(lerp(0,4,.25), lerp(2,8,.25), .75) = lerp(1.0, 3.5, .75).
            (2.5, 7.5, 2.875),
        ] {
            let got = info
                .surface_z_at(x, y)
                .expect("wet everywhere on this cell");
            assert!(
                (got - want).abs() < 1e-4,
                "bilinear at ({x}, {y}): got {got}, want {want}"
            );
        }
        // The grid's highest wet vertex would answer 8.0 at every one.
        assert!(info.surface_z_at(0.0, 0.0).unwrap() < 8.0);
    }

    /// Inside a WMO only its liquid answers and outdoors only the ADT's, so a tunnel under a lake,
    /// inside the lake's floorless footprint, is dry.
    #[test]
    fn indoors_and_outdoors_see_different_liquid() {
        let lake = flat_info(LiquidSource::AdtChunk, LiquidKind::Still, 50.0);
        let canal = wmo_info(1, LiquidKind::Still, 8.0);
        let all = [&lake, &canal];
        let deep_under = [5.0, 5.0, 0.0];

        // In the tunnel: the lake 50 yd overhead does not answer.
        let hit = liquid_at(all.into_iter(), deep_under, inside(1)).unwrap();
        assert_eq!(hit.surface_z, 8.0, "indoors must read the WMO's own liquid");
        let outside = liquid_at(all.into_iter(), deep_under, LiquidClaim::Outdoors).unwrap();
        assert_eq!(outside.surface_z, 50.0);
        // Unclassified (a unit's first frame): both sources answer.
        assert!(liquid_at(all.into_iter(), deep_under, LiquidClaim::Unknown).is_some());
        assert!(liquid_at(all.into_iter(), [99.0, 99.0, 0.0], LiquidClaim::Outdoors).is_none());
    }

    /// Inside one building, another building's pool never answers, at any height.
    #[test]
    fn another_buildings_pool_never_claims_you() {
        let mine = wmo_info(1, LiquidKind::Still, 8.0);
        let theirs = wmo_info(2, LiquidKind::Still, 190.0);
        let all = [&mine, &theirs];
        let feet = [5.0, 5.0, 0.0];

        assert_eq!(
            liquid_at(all.into_iter(), feet, inside(1))
                .unwrap()
                .surface_z,
            8.0,
            "in building 1: only building 1's pool"
        );
        // In building 2 the tall pool is yours: scoping attributes a pool, it does not cap height.
        assert_eq!(
            liquid_at(all.into_iter(), feet, inside(2))
                .unwrap()
                .surface_z,
            190.0
        );
        // A building with no pool of its own, under another's (Uldaman under a cave's pool).
        assert!(
            liquid_at([&theirs].into_iter(), feet, inside(1)).is_none(),
            "building 1 has no liquid; building 2's must not stand in for it"
        );
    }

    /// A pool in another storey of the same placement never claims the room below, and the floor
    /// never costs a room its own pool.
    #[test]
    fn a_pool_upstairs_does_not_claim_the_room_below() {
        // Undercity's shape, in miniature: one placement, two rooms stacked 115 yd apart.
        let upstairs = flat_info(
            LiquidSource::WmoGroup(wmo_pool(1, 48.0)), // the upper channels' room floor
            LiquidKind::Slime,
            51.98,
        );
        let downstairs = flat_info(
            LiquidSource::WmoGroup(wmo_pool(1, -70.0)), // the Rogues'-Quarter-level room
            LiquidKind::Slime,
            -64.48,
        );
        let all = [&upstairs, &downstairs];

        // The eye in the lower room, above its own slime: the upstairs pool is no candidate at all.
        assert!(
            !upstairs.answers(inside(1), -63.59),
            "a pool 115 yd overhead, in another storey of the same building, must not submerge you"
        );
        assert!(
            downstairs.answers(inside(1), -63.59),
            "…while the eye's OWN room's pool stays a candidate (it is simply below the eye)"
        );
        // Down in the lower room's own slime, its pool still answers.
        assert_eq!(
            liquid_at(all.into_iter(), [5.0, 5.0, -66.0], inside(1))
                .unwrap()
                .surface_z,
            -64.48
        );
        assert!(upstairs.answers(inside(1), 50.0));
    }

    /// The floor belongs to the pool, not the claim, so it holds for an `Unknown` claim too.
    #[test]
    fn the_floor_holds_even_for_an_unclassified_subject() {
        let upstairs = flat_info(
            LiquidSource::WmoGroup(wmo_pool(1, 48.0)),
            LiquidKind::Slime,
            51.98,
        );
        assert!(liquid_at(
            [&upstairs].into_iter(),
            [5.0, 5.0, -63.59],
            LiquidClaim::Unknown
        )
        .is_none());
        assert!(liquid_at(
            [&upstairs].into_iter(),
            [5.0, 5.0, 50.0],
            LiquidClaim::Unknown
        )
        .is_some());
    }

    /// A group with no bounds has no floor: a missing box fails open.
    #[test]
    fn a_pool_with_no_bounds_keeps_its_pre_floor_reach() {
        let unbounded = WmoPool::new(
            Some(WmoRoom {
                instance: placement(1),
                group: 0,
            }),
            &Transform::IDENTITY,
            None,
        );
        assert_eq!(unbounded.floor, f32::NEG_INFINITY);
        let pool = flat_info(LiquidSource::WmoGroup(unbounded), LiquidKind::Still, 8.0);
        assert!(liquid_at([&pool].into_iter(), [5.0, 5.0, -9999.0], inside(1)).is_some());
    }

    /// The floor is the lowest of the eight placed corners, under a lift and under a roll.
    #[test]
    fn the_floor_follows_the_placement_transform() {
        let bounds = benilla_formats::WmoGroupInfo {
            interior: true,
            show_skybox: false,
            bbox_min: [-10.0, -10.0, 0.0],
            bbox_max: [10.0, 10.0, 4.0],
        };
        // A pure world lift: WoW z-floor 0 + 100 = 100 (Bevy +Y is the WoW z lift).
        let lifted = WmoPool::new(
            None,
            &Transform::from_translation(Vec3::new(0.0, 100.0, 0.0)),
            Some(&bounds),
        );
        assert!((lifted.floor - 100.0).abs() < 1e-3, "got {}", lifted.floor);
        // Rolled 90° about Bevy Z (a WoW-X roll): a ±10 horizontal half-width now reaches down.
        let rolled = WmoPool::new(
            None,
            &Transform {
                translation: Vec3::ZERO,
                rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                scale: Vec3::ONE,
            },
            Some(&bounds),
        );
        assert!(
            rolled.floor < -9.0,
            "a rolled placement's floor must follow its corners, got {}",
            rolled.floor
        );
    }

    /// A pool on a placement with no instance entity answers no claim but `Unknown`.
    #[test]
    fn an_unowned_pool_answers_no_one() {
        let orphan = flat_info(
            LiquidSource::WmoGroup(orphan_pool()),
            LiquidKind::Still,
            8.0,
        );
        let feet = [5.0, 5.0, 0.0];
        assert!(liquid_at([&orphan].into_iter(), feet, inside(1)).is_none());
        assert!(liquid_at([&orphan].into_iter(), feet, LiquidClaim::Outdoors).is_none());
        assert!(liquid_at([&orphan].into_iter(), feet, LiquidClaim::Unknown).is_some());
    }

    /// An indoor claim excludes the ADT water overhead for the eye as for the feet.
    #[test]
    fn an_indoor_eye_does_not_see_the_adt_water_overhead() {
        let tirisfal = flat_info(LiquidSource::AdtChunk, LiquidKind::Still, 32.93);
        let eye = [5.0, 5.0, -62.26];
        assert!(
            liquid_at([&tirisfal].into_iter(), eye, LiquidClaim::Outdoors).is_some(),
            "outdoors under the same water: still submerged (the control)"
        );
        assert!(
            liquid_at([&tirisfal].into_iter(), eye, inside(1)).is_none(),
            "inside a building, the ADT water overhead is not the eye's liquid"
        );
    }

    /// Stacked surfaces resolve to the lowest, in any iteration order.
    #[test]
    fn stacked_surfaces_take_the_lowest() {
        let upper = wmo_info(1, LiquidKind::Still, 40.0);
        let lower = wmo_info(1, LiquidKind::Still, 4.0);
        for order in [[&upper, &lower], [&lower, &upper]] {
            let hit = liquid_at(order.into_iter(), [5.0, 5.0, 0.0], inside(1)).unwrap();
            assert_eq!(hit.surface_z, 4.0);
        }
    }

    /// Lava and slime are swimmable but never water to the wade splash and depth consumers.
    #[test]
    fn fullbright_kinds_swim_but_are_not_water() {
        let lava = wmo_info(1, LiquidKind::Magma, 6.0);
        let here = [5.0, 5.0, 0.0];
        let hit = liquid_at([&lava].into_iter(), here, inside(1)).expect("lava is a swim volume");
        assert_eq!(hit.kind, LiquidKind::Magma);
        assert!(
            water_surface_at([&lava].into_iter(), here, inside(1)).is_none(),
            "magma must not read as water"
        );
    }

    /// Containment tests the cells, not the box, which spans dry ground.
    #[test]
    fn a_dry_spot_inside_the_bounding_box_is_not_liquid() {
        // Three cells in a row, the middle one a hole: a canal on either side of a tunnel.
        let info = grid_info(
            LiquidSource::WmoGroup(wmo_pool(1, f32::NEG_INFINITY)),
            LiquidKind::Still,
            4,
            2,
            10.0,
            vec![true, false, true],
            |_, _| 5.0,
        );
        assert!(info.contains(15.0, 5.0), "the box does span the dry middle");
        assert!(
            info.surface_z_at(5.0, 5.0).is_some() && info.surface_z_at(25.0, 5.0).is_some(),
            "over the wet cells — must be liquid"
        );
        assert!(
            info.surface_z_at(15.0, 5.0).is_none(),
            "inside the box but over the HOLE — must NOT be liquid (the canal tunnel)"
        );
        assert!(liquid_at([&info].into_iter(), [5.0, 5.0, 0.0], inside(1)).is_some());
        assert!(liquid_at([&info].into_iter(), [15.0, 5.0, 0.0], inside(1)).is_none());
    }

    /// A grid that cannot be inverted (a plane on edge) falls back to its bounds and its highest
    /// wet vertex rather than reading dry.
    #[test]
    fn a_degenerate_grid_falls_back_to_the_bounds() {
        let info = WaterChunkInfo::new(
            LiquidSource::AdtChunk,
            LiquidKind::Still,
            [2, 2],
            vec![
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 10.0], // zero XY extent along i ⇒ the basis has no area
                [0.0, 10.0, 0.0],
                [0.0, 10.0, 10.0],
            ],
            vec![true],
        );
        assert_eq!(info.surface_z_at(0.0, 5.0), Some(10.0));
        assert_eq!(info.surface_z_at(50.0, 5.0), None, "still bounded in XY");
    }

    /// A grid whose dimensions don't match its arrays claims nothing and walks nothing.
    #[test]
    fn a_malformed_grid_claims_nothing_and_walks_nothing() {
        let info = WaterChunkInfo::new(
            LiquidSource::AdtChunk,
            LiquidKind::Still,
            [9, 9],
            vec![[0.0, 0.0, 5.0]; 4], // 4 positions for an 81-vertex grid
            vec![true; 64],
        );
        assert_eq!(info.surface_z_at(0.0, 0.0), None);
        let mut cells = 0;
        info.for_each_wet_cell(|_| cells += 1);
        assert_eq!(cells, 0, "nothing to walk, and no panic walking it");
        assert!(
            describe_at([&info].into_iter(), [0.0, 0.0, 0.0], LiquidClaim::Outdoors).is_empty(),
            "and `/liquid` lists no candidate for it"
        );
    }
}
