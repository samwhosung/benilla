//! The residency window, derived from `farclip` (177..777) as the reference derives it:
//! `SetFarClip` (`0x6725d0`) sets `r = 1 + trunc(farclip / 33.333)` chunks, and the world tick
//! (`0x672730`) centres two windows on the viewer's chunk, the inner `idx ± r` (must be resident)
//! and the outer `idx ± max(r + 2, 8)` (requested ahead, built under 5 ms a frame). Eviction is
//! membership in the outer window, by tile (`[area+0x88]` vs `bounds >> 4`).
//!
//! - Deviation: whole tiles, because benilla loads, spawns and releases ADT tiles, not chunks: a
//!   tile is wanted when any of its chunks lies in the outer window, so the resident edge is a
//!   tile line where the reference's is a chunk line.
//! - Deviation: a one-chunk keep band ([`KEEP_BAND_CHUNKS`]), because a tile reload re-decodes and
//!   re-spawns where the reference's is a cheap read: a tile releases only past `outer + 1`.

use benilla_formats::{world_to_chunk, CHUNK_SIZE, TILE_SIZE};

/// Chunks per tile edge; `chunk >> 4` is the tile.
const CHUNKS_PER_TILE: i32 = 16;
/// Chunk indices run `0..=1023` (64 tiles × 16), matching the reference's `[0, 0x3ff]` clamp.
const CHUNK_MAX: i32 = 64 * CHUNKS_PER_TILE - 1;
/// The outer window's floor in chunks: `max(r + 2, 8)` (`0x672966`–`0x67297b`).
const OUTER_FLOOR_CHUNKS: i32 = 8;
/// Chunks the outer window grows by for the release test only.
pub const KEEP_BAND_CHUNKS: i32 = 1;

/// The two nested windows around one focus chunk, in chunk units on the tile-grid axes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamWindow {
    /// The focus's chunk index `(chunk_x, chunk_y)`, on the tile grid's axes.
    pub focus: (i32, i32),
    /// Inner half-width in chunks: `1 + trunc(farclip / CHUNK_SIZE)`.
    pub inner: i32,
    /// Outer half-width in chunks: `max(inner + 2, 8)`.
    pub outer: i32,
}

impl StreamWindow {
    /// The windows `farclip` yards of view distance put around the WoW-space focus `(x, y)`.
    pub fn at(farclip: f32, wow_x: f32, wow_y: f32) -> Self {
        let (cx, cy) = world_to_chunk(wow_x, wow_y);
        let inner = inner_radius(farclip);
        Self {
            focus: (cx as i32, cy as i32),
            inner,
            outer: outer_radius(inner),
        }
    }

    /// The tile the focus stands on.
    pub fn focus_tile(&self) -> (i32, i32) {
        (
            self.focus.0 / CHUNKS_PER_TILE,
            self.focus.1 / CHUNKS_PER_TILE,
        )
    }

    /// The tile range the outer window grown by `band` chunks touches, clamped to the grid.
    fn tile_range(&self, band: i32) -> ((i32, i32), (i32, i32)) {
        let half = self.outer + band;
        let lo = |c: i32| (c - half).clamp(0, CHUNK_MAX) / CHUNKS_PER_TILE;
        let hi = |c: i32| (c + half).clamp(0, CHUNK_MAX) / CHUNKS_PER_TILE;
        (
            (lo(self.focus.0), lo(self.focus.1)),
            (hi(self.focus.0), hi(self.focus.1)),
        )
    }

    /// Every tile the outer window touches, in grid order: the set the streamer wants resident.
    pub fn wanted_tiles(&self) -> Vec<(i32, i32)> {
        let ((x0, y0), (x1, y1)) = self.tile_range(0);
        let mut out = Vec::with_capacity(((x1 - x0 + 1) * (y1 - y0 + 1)) as usize);
        for tx in x0..=x1 {
            for ty in y0..=y1 {
                out.push((tx, ty));
            }
        }
        out
    }

    /// Whether a tile is inside the outer window plus the keep band.
    pub fn keeps(&self, tile: (i32, i32)) -> bool {
        let ((x0, y0), (x1, y1)) = self.tile_range(KEEP_BAND_CHUNKS);
        (x0..=x1).contains(&tile.0) && (y0..=y1).contains(&tile.1)
    }
}

/// The inner half-width in chunks, `1 + trunc(farclip / 33.333)` (`0x6725e9`: `fmul −0.03`,
/// `__ftol`, `1 − eax`): 777 ⇒ 24, 350 ⇒ 11, 177 ⇒ 6.
pub fn inner_radius(farclip: f32) -> i32 {
    1 + (farclip / CHUNK_SIZE).trunc() as i32
}

/// The outer half-width in chunks: `max(inner + 2, 8)`.
pub fn outer_radius(inner: i32) -> i32 {
    (inner + 2).max(OUTER_FLOOR_CHUNKS)
}

/// The farthest, in yards, a resident tile's corner can lie from the focus: the floor an art sweep
/// must clear (`art_scope::radius_floor`) so it never evicts what the streamer still holds.
pub fn max_resident_reach_yd(farclip: f32) -> f32 {
    let half = outer_radius(inner_radius(farclip)) + KEEP_BAND_CHUNKS + CHUNKS_PER_TILE;
    half as f32 * CHUNK_SIZE * std::f32::consts::SQRT_2 + TILE_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::tile_to_world;

    /// 350 is the registered default; at 177 the outer window sits on its 8-chunk floor.
    #[test]
    fn the_radii_are_the_references() {
        assert_eq!(inner_radius(777.0), 24);
        assert_eq!(outer_radius(24), 26);
        assert_eq!(inner_radius(350.0), 11);
        assert_eq!(outer_radius(11), 13);
        assert_eq!(inner_radius(177.0), 6);
        assert_eq!(outer_radius(6), 8);
        // Truncation, not rounding: a hair under a chunk multiple stays on the lower ring.
        assert_eq!(inner_radius(CHUNK_SIZE * 3.0 - 0.01), 3);
        assert_eq!(inner_radius(CHUNK_SIZE * 3.0 + 0.01), 4);
    }

    /// The window at the centre of `chunk` in `tile`.
    fn at_chunk(farclip: f32, tile: (u32, u32), chunk: (i32, i32)) -> StreamWindow {
        let (ox, oy) = tile_to_world(tile.0, tile.1);
        let wy = oy - (chunk.0 as f32 + 0.5) * CHUNK_SIZE;
        let wx = ox - (chunk.1 as f32 + 0.5) * CHUNK_SIZE;
        StreamWindow::at(farclip, wx, wy)
    }

    /// At 777 the outer window reaches 26 chunks: two tiles each way from the middle band (5×5);
    /// from a corner chunk, two behind and one ahead (4×4).
    #[test]
    fn the_wanted_block_follows_the_focus_chunk() {
        let w = at_chunk(777.0, (32, 44), (7, 7));
        assert_eq!(w.focus, (32 * 16 + 7, 44 * 16 + 7));
        assert_eq!(w.focus_tile(), (32, 44));
        let tiles = w.wanted_tiles();
        assert_eq!(tiles.len(), 25);
        assert!(tiles.contains(&(30, 42)) && tiles.contains(&(34, 46)));

        let w = at_chunk(777.0, (32, 44), (0, 0));
        let tiles = w.wanted_tiles();
        // Back: chunk 0 − 26 reaches 26 chunks into the previous tiles ⇒ two tiles. Forward:
        // chunk 0 + 26 = chunk 10 of the next tile ⇒ one tile.
        assert_eq!(tiles.len(), 16);
        assert!(tiles.contains(&(30, 42)) && tiles.contains(&(33, 45)));
        assert!(!tiles.contains(&(34, 44)));
    }

    /// At 350 the outer window is 13 chunks (433 yd): a 2×2 to 3×3 block.
    #[test]
    fn a_shorter_view_distance_wants_fewer_tiles() {
        assert_eq!(at_chunk(350.0, (32, 44), (7, 7)).wanted_tiles().len(), 9);
        assert_eq!(at_chunk(350.0, (32, 44), (15, 15)).wanted_tiles().len(), 4);
    }

    #[test]
    fn the_keep_band_is_one_chunk_of_hysteresis() {
        // From chunk 11 the outer window (26) reaches forward to chunk 37 = tile +2, chunk 5.
        let w = at_chunk(777.0, (32, 44), (11, 11));
        assert!(w.wanted_tiles().contains(&(34, 46)));
        // From chunk 5 it reaches chunk 31 = tile +1's last chunk: tile +2 is not wanted…
        let w = at_chunk(777.0, (32, 44), (5, 5));
        assert!(!w.wanted_tiles().contains(&(34, 46)));
        // …but the band (chunk 32 = tile +2, chunk 0) still keeps it.
        assert!(w.keeps((34, 46)));
        // From chunk 4 even the band falls short: the tile releases.
        let w = at_chunk(777.0, (32, 44), (4, 4));
        assert!(!w.keeps((34, 46)));
        // What is wanted is always kept.
        for t in w.wanted_tiles() {
            assert!(w.keeps(t));
        }
    }

    #[test]
    fn the_window_clamps_to_the_grid() {
        let w = at_chunk(777.0, (0, 63), (0, 15));
        let tiles = w.wanted_tiles();
        assert!(tiles
            .iter()
            .all(|&(x, y)| (0..64).contains(&x) && (0..64).contains(&y)));
        assert!(tiles.contains(&(0, 63)) && tiles.contains(&(1, 62)));
        assert!(w.keeps((0, 63)) && !w.keeps((-1, 63)) && !w.keeps((0, 64)));
    }

    #[test]
    fn the_resident_reach_bounds_the_block() {
        let reach = max_resident_reach_yd(777.0);
        // (26 + 1 + 16) chunks = 43 × 33⅓ = 1433 yd per axis; the corner is √2 of that, plus a tile.
        assert!((reach - (43.0 * CHUNK_SIZE * std::f32::consts::SQRT_2 + TILE_SIZE)).abs() < 0.01);
        assert!(max_resident_reach_yd(350.0) < reach);
        // A wanted tile's far corner lies inside the reach.
        let w = at_chunk(777.0, (32, 44), (9, 9));
        let (fx, fy) = (w.focus.0 as f32, w.focus.1 as f32);
        let far = w
            .wanted_tiles()
            .into_iter()
            .map(|(tx, ty)| {
                let cx = ((tx + 1) * CHUNKS_PER_TILE) as f32 - fx;
                let cy = ((ty + 1) * CHUNKS_PER_TILE) as f32 - fy;
                (cx * cx + cy * cy).sqrt() * CHUNK_SIZE
            })
            .fold(0.0f32, f32::max);
        assert!(
            far < reach,
            "far corner {far} must sit inside the reach {reach}"
        );
    }
}
