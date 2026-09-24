//! **The residency window** — how far around the view focus terrain is kept resident, derived
//! from `farclip` exactly as the reference derives it (decision 1513).
//!
//! The reference has one view-distance setting, the `farclip` CVar (the Video Options "Terrain
//! Distance" slider, 177..777), and streams *from* it: `SetFarClip 0x6725d0` computes
//! `r = 1 + trunc(farclip / 33.333)` **chunks** once, and the per-frame world tick (`0x672730`)
//! centres two nested square windows on the viewer's chunk — the **inner** `idx ± r` (terrain that
//! must be resident: the farclip disc rounded out to whole chunks) and the **outer**
//! `idx ± max(r + 2, 8)` (requested ahead, built under a 5 ms/frame budget; the 8-chunk floor is
//! 266⅔ yd). Eviction is pure membership in the outer window, at tile granularity (`[area+0x88]`
//! vs `bounds >> 4`).
//!
//! Two deliberate differences from the reference, both named in 1513:
//! - **Whole tiles.** benilla loads, spawns and releases ADT tiles, not chunks; a tile is wanted iff
//!   any of its chunks lies in the outer window. The reference builds chunk-by-chunk inside the
//!   same windows, so the edge of its resident world is a chunk line and ours is a tile line.
//! - **A one-chunk keep band** ([`KEEP_BAND_CHUNKS`]). The reference releases by membership alone
//!   because its reload is a cheap async read; ours re-decodes and re-spawns a tile (B181's
//!   frame spike), so a tile is wanted inside `outer` and released only past `outer + 1` —
//!   hysteresis against a body pacing across the one chunk line that toggles a far tile.

use benilla_formats::{world_to_chunk, CHUNK_SIZE, TILE_SIZE};

/// Chunks per tile edge; `chunk >> 4` is the tile.
const CHUNKS_PER_TILE: i32 = 16;
/// Chunk indices run `0..=1023` (64 tiles × 16), matching the reference's `[0, 0x3ff]` clamp.
const CHUNK_MAX: i32 = 64 * CHUNKS_PER_TILE - 1;
/// The outer window's floor in chunks — `max(r + 2, 8)` (`0x672966`–`0x67297b`).
const OUTER_FLOOR_CHUNKS: i32 = 8;
/// Chunks the outer window grows by for the *release* test only (see the module docs).
pub const KEEP_BAND_CHUNKS: i32 = 1;

/// The two nested windows around one focus chunk, in chunk units on the tile-grid axes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamWindow {
    /// The focus's chunk index `(chunk_x, chunk_y)` — the same axes as the tile grid.
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

    /// The inclusive tile range `((min_x, min_y), (max_x, max_y))` touched by the outer window
    /// grown by `band` chunks, clamped to the 64×64 tile grid.
    fn tile_range(&self, band: i32) -> ((i32, i32), (i32, i32)) {
        let half = self.outer + band;
        let lo = |c: i32| (c - half).clamp(0, CHUNK_MAX) / CHUNKS_PER_TILE;
        let hi = |c: i32| (c + half).clamp(0, CHUNK_MAX) / CHUNKS_PER_TILE;
        (
            (lo(self.focus.0), lo(self.focus.1)),
            (hi(self.focus.0), hi(self.focus.1)),
        )
    }

    /// Every tile the outer window touches — the set the streamer wants resident, in grid order.
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

    /// Does the window still *keep* this tile — is it inside the outer window plus the keep band?
    /// A resident tile that is not kept is stale and releases.
    pub fn keeps(&self, tile: (i32, i32)) -> bool {
        let ((x0, y0), (x1, y1)) = self.tile_range(KEEP_BAND_CHUNKS);
        (x0..=x1).contains(&tile.0) && (y0..=y1).contains(&tile.1)
    }
}

/// The inner half-width in chunks for a view distance — `1 + trunc(farclip / 33.333)`
/// (`0x6725e9 fmul −0.03 · __ftol · 1 − eax`). 777 ⇒ 24, 350 ⇒ 11, 177 ⇒ 6.
pub fn inner_radius(farclip: f32) -> i32 {
    1 + (farclip / CHUNK_SIZE).trunc() as i32
}

/// The outer half-width in chunks for an inner one — `max(inner + 2, 8)`.
pub fn outer_radius(inner: i32) -> i32 {
    (inner + 2).max(OUTER_FLOOR_CHUNKS)
}

/// How far, in yards, the corner of any tile this window can hold resident may lie from the
/// focus — the bound a within-map art sweep must clear so it never evicts what the streamer is
/// still holding (`art_scope::radius_floor`). A kept tile's far edge lies at most
/// `outer + band + 16` chunks out per axis (the window's edge chunk plus the rest of that whole
/// tile), on both axes at once at the corner.
pub fn max_resident_reach_yd(farclip: f32) -> f32 {
    let half = outer_radius(inner_radius(farclip)) + KEEP_BAND_CHUNKS + CHUNKS_PER_TILE;
    half as f32 * CHUNK_SIZE * std::f32::consts::SQRT_2 + TILE_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::tile_to_world;

    /// The radii are the bytes': 777 ⇒ 24/26, the registrar default 350 ⇒ 11/13, and at the
    /// 177 floor the outer window sits on its 8-chunk floor, not at inner + 2.
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

    /// A WoW-space point in chunk (5, 9) of tile (32, 44): the chunk index is that chunk.
    fn at_chunk(farclip: f32, tile: (u32, u32), chunk: (i32, i32)) -> StreamWindow {
        let (ox, oy) = tile_to_world(tile.0, tile.1);
        let wy = oy - (chunk.0 as f32 + 0.5) * CHUNK_SIZE;
        let wx = ox - (chunk.1 as f32 + 0.5) * CHUNK_SIZE;
        StreamWindow::at(farclip, wx, wy)
    }

    /// At 777 the outer window reaches 26 chunks — 1 tile + 10 chunks — so from a chunk in the
    /// tile's middle band (5..=9) it touches two tiles each way (5×5), and from a chunk near an
    /// edge only one tile on that side (4×4 at the corner chunk). The old fixed 5×5 was the
    /// worst case of this, every time.
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

    /// At the registrar default 350 the outer window is 13 chunks (433 yd): a 2×2 to 3×3 block —
    /// the working set the slider is for.
    #[test]
    fn a_shorter_view_distance_wants_fewer_tiles() {
        assert_eq!(at_chunk(350.0, (32, 44), (7, 7)).wanted_tiles().len(), 9);
        assert_eq!(at_chunk(350.0, (32, 44), (15, 15)).wanted_tiles().len(), 4);
    }

    /// The keep band is one chunk: a tile the outer window's edge just left is still kept, and
    /// one a chunk further out is not.
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

    /// The windows clamp at the map's edge instead of naming tiles that do not exist.
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

    /// The art-scope floor clears the far corner of the widest block 777 can hold, and grows
    /// with the view distance.
    #[test]
    fn the_resident_reach_bounds_the_block() {
        let reach = max_resident_reach_yd(777.0);
        // (26 + 1 + 16) chunks = 43 × 33⅓ = 1433 yd per axis; the corner is √2 of that, plus a tile.
        assert!((reach - (43.0 * CHUNK_SIZE * std::f32::consts::SQRT_2 + TILE_SIZE)).abs() < 0.01);
        assert!(max_resident_reach_yd(350.0) < reach);
        // A wanted tile's far corner is inside the reach (worst case: the focus at a chunk's
        // near edge, the tile at the window's far edge).
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
