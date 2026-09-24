//! The column index: which of a face set's triangles can own a vertical (x, y) column, for the
//! down-cast position queries against a WMO (the camera's room, a unit's room, the floor MOCV that
//! lights an entity). It never changes a verdict: the candidates are a superset of the triangles
//! whose XY box holds the column, in ascending index order, the order a linear scan visits them,
//! so the first-wins and later-wins tie-breaks downstream resolve the same.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// A triangle touching more cells than this goes to the next level down, and past the coarse level
/// to [`ColumnGrid::spanning`], so a big floor slab is not copied into hundreds of cells.
const MAX_SPAN_CELLS: u32 = 32;

/// Below this many oversized faces they stay an always-tested list instead of a second grid.
const MIN_COARSE_TRIS: usize = 24;

/// The finest cell, yd, so a face set stacked in one spot cannot ask for an unbounded grid.
const MIN_CELL: f32 = 0.25;

/// The most cells one level holds, whatever the face count.
const MAX_CELLS: usize = 1 << 16;

/// One uniform XY grid in CSR form (`starts`, `items`).
#[derive(Debug, Clone)]
struct Level {
    /// Grid origin (the indexed set's XY minimum).
    min: [f32; 2],
    /// 1 / cell size, in cells per yard.
    inv_cell: f32,
    nx: u32,
    ny: u32,
    /// CSR row offsets into [`Self::items`], length `nx * ny + 1`.
    starts: Vec<u32>,
    /// Global triangle indices, ascending within each cell.
    items: Vec<u32>,
}

/// A face set's column index: a fine grid at about one triangle a cell, a coarse grid over the
/// faces too large for it (dungeon floor slabs), and a residue too large for both.
#[derive(Debug, Clone)]
pub struct ColumnGrid {
    /// The ~1-triangle-per-cell grid over the ordinary faces.
    fine: Level,
    /// The grid over the faces `fine` set aside; `None` when too few, all in [`Self::spanning`].
    coarse: Option<Level>,
    /// Faces too large for either level, ascending: candidates for every query.
    spanning: Vec<u32>,
}

/// `WOW_COLUMN_COST=1`: triangles tested per column query by source (fine cell, coarse cell,
/// residue). The residue's share should read near zero; a climb means a slab too big for the
/// coarse level.
static COLUMN_QUERIES: AtomicU64 = AtomicU64::new(0);
static COLUMN_BINNED: AtomicU64 = AtomicU64::new(0);
static COLUMN_COARSE: AtomicU64 = AtomicU64::new(0);
static COLUMN_SPANNING: AtomicU64 = AtomicU64::new(0);

/// Whether the column-query meter is armed (`WOW_COLUMN_COST`), read once.
pub fn column_cost_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_COLUMN_COST").is_some())
}

/// Take and zero the counters, `(queries, binned, coarse, spanning)`; one caller a frame keeps them
/// per frame.
pub fn take_column_query_stats() -> (u64, u64, u64, u64) {
    (
        COLUMN_QUERIES.swap(0, Ordering::Relaxed),
        COLUMN_BINNED.swap(0, Ordering::Relaxed),
        COLUMN_COARSE.swap(0, Ordering::Relaxed),
        COLUMN_SPANNING.swap(0, Ordering::Relaxed),
    )
}

impl Level {
    /// Index ascending `ids` into one grid, also returning, ascending, the ids that span more than
    /// [`MAX_SPAN_CELLS`] cells, for the next level; `None` when the set has no finite extent.
    fn build(
        ids: &[u32],
        xy_aabb: &impl Fn(usize) -> ([f32; 2], [f32; 2]),
    ) -> Option<(Self, Vec<u32>)> {
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for &i in ids {
            let (lo, hi) = xy_aabb(i as usize);
            for a in 0..2 {
                min[a] = min[a].min(lo[a]);
                max[a] = max[a].max(hi[a]);
            }
        }
        let (w, h) = ((max[0] - min[0]).max(0.0), (max[1] - min[1]).max(0.0));
        if !w.is_finite() || !h.is_finite() || (w <= 0.0 && h <= 0.0) {
            return None;
        }
        // About one triangle a cell, clamped by MIN_CELL and MAX_CELLS. Sizing from this set's own
        // count and extent is what makes the coarse level coarse enough for a slab to bin.
        let target_cells = ids.len().clamp(1, MAX_CELLS) as f32;
        let area = (w * h).max(f32::MIN_POSITIVE);
        let mut cell = (area / target_cells).sqrt().max(MIN_CELL);
        let (mut nx, mut ny) = dims(w, h, cell);
        while nx as usize * ny as usize > MAX_CELLS {
            cell *= 2.0;
            (nx, ny) = dims(w, h, cell);
        }
        let inv_cell = 1.0 / cell;
        let cells = nx as usize * ny as usize;

        // Pass 1: count per cell, and set the oversized triangles aside.
        let mut counts = vec![0u32; cells + 1];
        let mut overflow = Vec::new();
        let span_of = |i: u32| -> Option<(u32, u32, u32, u32)> {
            let (lo, hi) = xy_aabb(i as usize);
            let x0 = cell_of(lo[0], min[0], inv_cell, nx);
            let x1 = cell_of(hi[0], min[0], inv_cell, nx);
            let y0 = cell_of(lo[1], min[1], inv_cell, ny);
            let y1 = cell_of(hi[1], min[1], inv_cell, ny);
            let touched = (x1 - x0 + 1) * (y1 - y0 + 1);
            (touched <= MAX_SPAN_CELLS).then_some((x0, x1, y0, y1))
        };
        for &i in ids {
            match span_of(i) {
                Some((x0, x1, y0, y1)) => {
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            counts[(y * nx + x) as usize] += 1;
                        }
                    }
                }
                None => overflow.push(i),
            }
        }
        // Prefix sum → CSR starts.
        let mut starts = vec![0u32; cells + 1];
        let mut acc = 0u32;
        for c in 0..cells {
            starts[c] = acc;
            acc += counts[c];
        }
        starts[cells] = acc;

        // Pass 2: fill. `ids` is ascending, so each cell is too, which the tie-breaks rest on.
        let mut items = vec![0u32; acc as usize];
        let mut cursor = starts.clone();
        for &i in ids {
            if let Some((x0, x1, y0, y1)) = span_of(i) {
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let c = (y * nx + x) as usize;
                        items[cursor[c] as usize] = i;
                        cursor[c] += 1;
                    }
                }
            }
        }
        Some((
            Self {
                min,
                inv_cell,
                nx,
                ny,
                starts,
                items,
            },
            overflow,
        ))
    }

    /// This level's cell list for the column at `(x, y)`, ascending.
    fn cell(&self, x: f32, y: f32) -> &[u32] {
        let cx = cell_of(x, self.min[0], self.inv_cell, self.nx);
        let cy = cell_of(y, self.min[1], self.inv_cell, self.ny);
        let c = (cy * self.nx + cx) as usize;
        &self.items[self.starts[c] as usize..self.starts[c + 1] as usize]
    }
}

impl ColumnGrid {
    /// Index `count` triangles by their XY AABBs; `None` for a set small enough that the caller's
    /// linear scan is cheaper.
    pub fn build(count: usize, xy_aabb: impl Fn(usize) -> ([f32; 2], [f32; 2])) -> Option<Self> {
        const MIN_TRIS: usize = 64;
        if count < MIN_TRIS {
            return None;
        }
        let all: Vec<u32> = (0..count as u32).collect();
        let (fine, oversized) = Level::build(&all, &xy_aabb)?;

        let (coarse, spanning) = if oversized.len() >= MIN_COARSE_TRIS {
            match Level::build(&oversized, &xy_aabb) {
                Some((level, residue)) => (Some(level), residue),
                None => (None, oversized),
            }
        } else {
            (None, oversized)
        };

        Some(Self {
            fine,
            coarse,
            spanning,
        })
    }

    /// The triangles that can own the column at `(x, y)`: a superset, ascending.
    pub fn candidates(&self, x: f32, y: f32) -> ColumnCandidates<'_> {
        let binned = self.fine.cell(x, y);
        let coarse = self.coarse.as_ref().map_or(&[][..], |c| c.cell(x, y));
        if column_cost_enabled() {
            COLUMN_QUERIES.fetch_add(1, Ordering::Relaxed);
            COLUMN_BINNED.fetch_add(binned.len() as u64, Ordering::Relaxed);
            COLUMN_COARSE.fetch_add(coarse.len() as u64, Ordering::Relaxed);
            COLUMN_SPANNING.fetch_add(self.spanning.len() as u64, Ordering::Relaxed);
        }
        ColumnCandidates {
            binned,
            coarse,
            spanning: &self.spanning,
        }
    }

    /// Index size in triangle slots, for the load-time log.
    pub fn slots(&self) -> usize {
        self.fine.items.len()
            + self.coarse.as_ref().map_or(0, |c| c.items.len())
            + self.spanning.len()
    }
}

/// Ascending merge of the fine cell, the coarse cell and the residue. A face is in exactly one of
/// them, so the merge never duplicates, and its order is a linear scan's.
pub struct ColumnCandidates<'a> {
    binned: &'a [u32],
    coarse: &'a [u32],
    spanning: &'a [u32],
}

impl Iterator for ColumnCandidates<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        // Pick the smallest head across the three streams; `None` sorts last.
        let heads = [
            self.binned.first().copied(),
            self.coarse.first().copied(),
            self.spanning.first().copied(),
        ];
        let pick = heads
            .iter()
            .enumerate()
            .filter_map(|(n, h)| h.map(|v| (n, v)))
            .min_by_key(|&(_, v)| v)?
            .0;
        let src = match pick {
            0 => &mut self.binned,
            1 => &mut self.coarse,
            _ => &mut self.spanning,
        };
        let (first, rest) = src.split_first()?;
        *src = rest;
        Some(*first as usize)
    }
}

/// Grid dimensions for an extent at a cell size (at least one cell each way).
fn dims(w: f32, h: f32, cell: f32) -> (u32, u32) {
    let n = |extent: f32| ((extent / cell).ceil() as u32).max(1);
    (n(w), n(h))
}

/// A value's cell, clamped into the grid so a column just outside the bounds (float error at the
/// edge) still lands in the edge cell.
fn cell_of(v: f32, min: f32, inv_cell: f32, n: u32) -> u32 {
    let i = ((v - min) * inv_cell).floor();
    if i < 0.0 {
        0
    } else {
        (i as u32).min(n - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb(tri: [[f32; 3]; 3]) -> ([f32; 2], [f32; 2]) {
        let xs = [tri[0][0], tri[1][0], tri[2][0]];
        let ys = [tri[0][1], tri[1][1], tri[2][1]];
        (
            [
                xs.iter().copied().fold(f32::MAX, f32::min),
                ys.iter().copied().fold(f32::MAX, f32::min),
            ],
            [
                xs.iter().copied().fold(f32::MIN, f32::max),
                ys.iter().copied().fold(f32::MIN, f32::max),
            ],
        )
    }

    /// Small pseudo-random triangles with a slab every 97th: the dungeon shape.
    fn field(n: usize) -> Vec<[[f32; 3]; 3]> {
        let mut out = Vec::with_capacity(n);
        let mut s = 12345u32;
        let mut rnd = || {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            (s >> 8) as f32 / (1 << 24) as f32
        };
        for i in 0..n {
            let (x, y) = (rnd() * 200.0 - 100.0, rnd() * 200.0 - 100.0);
            if i % 97 == 0 {
                // A slab: the MAX_SPAN_CELLS path.
                out.push([[x, y, 0.0], [x + 80.0, y, 0.0], [x, y + 80.0, 0.0]]);
            } else {
                let (dx, dy) = (rnd() * 3.0, rnd() * 3.0);
                out.push([[x, y, 0.0], [x + dx, y, 0.0], [x, y + dy, 0.0]]);
            }
        }
        out
    }

    #[test]
    fn candidates_are_a_superset_of_the_linear_scan() {
        let tris = field(4000);
        let grid = ColumnGrid::build(tris.len(), |i| aabb(tris[i])).expect("field indexes");
        let inside = |t: &[[f32; 3]; 3], x: f32, y: f32| {
            let (lo, hi) = aabb(*t);
            x >= lo[0] && x <= hi[0] && y >= lo[1] && y <= hi[1]
        };
        let mut probes = 0;
        for gx in -12..=12 {
            for gy in -12..=12 {
                let (x, y) = (gx as f32 * 9.7, gy as f32 * 9.3);
                let got: Vec<usize> = grid.candidates(x, y).collect();
                for (i, t) in tris.iter().enumerate() {
                    if inside(t, x, y) {
                        assert!(
                            got.contains(&i),
                            "column ({x}, {y}) missed triangle {i} — the index is not a superset"
                        );
                    }
                }
                probes += 1;
            }
        }
        assert!(probes > 500, "the sweep must actually probe");
    }

    #[test]
    fn candidates_are_ascending() {
        let tris = field(2000);
        let grid = ColumnGrid::build(tris.len(), |i| aabb(tris[i])).expect("field indexes");
        for gx in -8..=8 {
            for gy in -8..=8 {
                let got: Vec<usize> = grid
                    .candidates(gx as f32 * 12.0, gy as f32 * 12.0)
                    .collect();
                assert!(
                    got.windows(2).all(|w| w[0] < w[1]),
                    "candidates must be strictly ascending, got {got:?}"
                );
            }
        }
    }

    #[test]
    fn columns_outside_the_bounds_are_safe() {
        let tris = field(200);
        let grid = ColumnGrid::build(tris.len(), |i| aabb(tris[i])).expect("field indexes");
        for (x, y) in [(-1e6, -1e6), (1e6, 1e6), (0.0, 1e6), (f32::MIN, f32::MAX)] {
            let _: Vec<usize> = grid.candidates(x, y).collect();
        }
    }

    /// The tests above would still pass with no coarse level at all; this one would not.
    #[test]
    fn the_coarse_level_takes_the_slabs() {
        let tris = field(4000);
        let grid = ColumnGrid::build(tris.len(), |i| aabb(tris[i])).expect("field indexes");

        // Every 97th triangle is a slab, too wide for the fine level.
        let slabs = tris.len().div_ceil(97);
        let coarse = grid
            .coarse
            .as_ref()
            .expect("the oversized set must get its own grid");
        assert!(
            coarse.items.len() >= slabs / 2,
            "the coarse level binned {} of ~{slabs} slabs — it is not taking them",
            coarse.items.len()
        );
        assert!(
            grid.spanning.len() * 4 < slabs,
            "residue {} is not small against ~{slabs} slabs — the coarse cells are too fine",
            grid.spanning.len()
        );
    }

    #[test]
    fn no_triangle_is_in_two_sources() {
        let tris = field(3000);
        let grid = ColumnGrid::build(tris.len(), |i| aabb(tris[i])).expect("field indexes");
        for gx in -10..=10 {
            for gy in -10..=10 {
                let (x, y) = (gx as f32 * 11.3, gy as f32 * 10.7);
                let got: Vec<usize> = grid.candidates(x, y).collect();
                let mut sorted = got.clone();
                sorted.sort_unstable();
                sorted.dedup();
                assert_eq!(
                    sorted.len(),
                    got.len(),
                    "column ({x}, {y}) yielded a duplicate"
                );
            }
        }
    }

    #[test]
    fn tiny_face_sets_are_not_indexed() {
        let tris = field(8);
        assert!(ColumnGrid::build(tris.len(), |i| aabb(tris[i])).is_none());
    }
}
