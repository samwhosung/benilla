//! The world-map projection laws (decision 0203 phase 2) — world position ↔ normalized map UV,
//! transcribed from the byte-verified primitives for `0x4a7100` / `0x4a72b0` / `0x4a7360` /
//! `0x4a6ec0`.
//!
//! Frames: **world** is wow coords (wx +north, wy +west, yards); **UV** is [0,1]² over the map
//! art, u rightward (east), v downward (south) — exactly what `GetPlayerMapPosition` returns and
//! what the reference FrameXML multiplies by the detail-frame size (`x·w`, `−y·h` from TOPLEFT).
//!
//! The world map has three levels but only two laws:
//! - **World level** (both continents on one 62.625×41.75-tile sheet): the WorldMapContinent.dbc
//!   constants ([`WorldProj`]) drive `0x4a7360`'s world-mode branch ([`world_uv`]).
//! - **Continent and zone levels**: a guarded lerp over the WorldMapArea.dbc world rect
//!   ([`zone_uv`]) — a continent is just a big zone (its areaId=0 row carries a whole-continent
//!   rect, aspect 1.5 like every other row in the 5875 data).
//!
//! `0x4a72b0` (the arrow-frame variant) is the same world-mode math with a final `1 − v` flip —
//! it feeds the y-up arrow-frame anchor path (`0x4a8d20`), not the Lua position API, so it is not
//! transcribed here; our player blip goes through [`world_uv`]/[`zone_uv`] + FrameXML `SetPoint`
//! exactly like the reference's `GetPlayerMapPosition` consumers.
//!
//! NOT here: the 128×128 area-bitmap cell law (`0x4a6ec0`) — its consumers (`ProcessMapClick`/
//! `UpdateMapHighlight`) execute inside the engine's Lua bindings, so the transcription lives
//! with them in benilla-ui's `script/worldmap.rs` (the grid data is pushed in the catalog; the
//! source file is `Interface\WorldMap\<Continent>.zmp`).

/// Yards per ADT tile (the binary's 0x80654c).
const TILE_YARDS: f32 = 533.333_3;
/// The world sheet's span in tiles: u axis (0x806548) and v axis (0x806550). Ratio 1.5 — the
/// 1002×668 map aspect.
const WORLD_SPAN_U_TILES: f32 = 62.625;
const WORLD_SPAN_V_TILES: f32 = 41.75;
/// The binary's precomputed per-axis products (0x806554/0x806558): `1/(span · tile)` — world
/// yards → sheet fraction.
const K_U: f32 = 1.0 / (WORLD_SPAN_U_TILES * TILE_YARDS);
const K_V: f32 = 1.0 / (WORLD_SPAN_V_TILES * TILE_YARDS);

/// One continent's world-level projection constants — WorldMapContinent.dbc fields 6/7/8, which
/// the binary reads at raw-record offsets +0x18/+0x1c/+0x20 (`0x4a72b0` / `0x4a7360` world-mode).
/// f6/f7 place the continent on the world sheet (tile units, u/v axes); f8 scales world yards
/// onto it. The 5875 rows: EK `{14.5, −7.0, 0.75}`, Kalimdor `{−19.0, −0.32249799, 0.75}`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldProj {
    pub offset_u: f32,
    pub offset_v: f32,
    pub scale: f32,
}

/// A map sheet's world-coordinate rect — a WorldMapArea.dbc row's loc fields. `left` = max wy
/// (west edge), `top` = max wx (north edge); left > right and top > bottom numerically.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneRect {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

/// World → UV on the world sheet (`0x4a7360` world-mode, verbatim). The axis swap is the law:
/// u from wy (east-west), v from wx (north-south). No range check — the caller decides what
/// off-sheet means (the reference's own world-mode consumers don't clamp here either).
pub fn world_uv(proj: WorldProj, wx: f32, wy: f32) -> (f32, f32) {
    let u = (proj.offset_u * (1.0 / WORLD_SPAN_U_TILES) + 0.5) - wy * K_U * proj.scale;
    let v = (proj.offset_v * (1.0 / WORLD_SPAN_V_TILES) + 0.5) - wx * K_V * proj.scale;
    (u, v)
}

/// World → UV inside a zone/continent rect (`0x4a7360` zone-mode, verbatim, quirks included).
/// Each axis is `1 − (in − low)/span` with the binary's guard shape: *both* the span and the
/// numerator short-circuit to 0 (an input exactly on the right/bottom edge yields 0, not 1).
/// Then the combined range check zeroes both outputs if either leaves [0,1] — the `(0,0)` the
/// reference FrameXML reads as "not on this map, hide the blip".
///
/// **That range check was contested and is now settled** (2026-08-25): the clamp lives at
/// `[0x4a74d6, 0x4a7533)` — four `fcomp`s against `0.0` (`0x7ffd74`) and `1.0` (`0x7ff9d8`), any
/// failure zeroing *both* outputs. This function was already right, and it is
/// what makes the world map's landmark pass show a zone only its own POIs rather than scattering
/// the continent's off-art.
///
/// Two residual differences from the bytes, both named rather than chased:
///
/// - the compares are **inclusive** — exactly `0.0` and exactly `1.0` pass, which is what
///   `(0.0..=1.0)` does — but an **unordered** (NaN) compare passes all four in the reference and
///   is zeroed here. No DBC-sourced or wire-sourced position can produce one.
/// - the reference compares its two axes at **different precisions**: the first against its `f32`
///   store, the second as the live 80-bit `st0` the `fsubr` left. An `f32` reimplementation cannot
///   have that asymmetry, and it can only matter within an ulp of an edge.
pub fn zone_uv(rect: ZoneRect, wx: f32, wy: f32) -> (f32, f32) {
    fn axis(input: f32, low: f32, span: f32) -> f32 {
        if input - low != 0.0 && span != 0.0 {
            1.0 - (input - low) / span
        } else {
            0.0
        }
    }
    let u = axis(wy, rect.right, rect.left - rect.right);
    let v = axis(wx, rect.bottom, rect.top - rect.bottom);
    if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
        (u, v)
    } else {
        (0.0, 0.0)
    }
}

/// UV → world on a zone/continent rect (`0x4a7100` zone-mode, verbatim — including the binary's
/// quirk that BOTH axes lerp by the same `t` (its v input goes unused; the real callers consume
/// one axis per call). Returns (wx, wy).
///
/// Its first consumer is the dev map-jump ([`crate::ui_world_map`]), which calls it exactly that
/// way — once for `wy` with the click's `u`, once for `wx` with its `v`. Used per-axis, this IS
/// the exact inverse of [`zone_uv`] (unlike the world-mode pair below).
pub fn zone_world(rect: ZoneRect, t: f32) -> (f32, f32) {
    let wy = rect.left - (rect.left - rect.right) * t;
    let wx = rect.top - (rect.top - rect.bottom) * t;
    (wx, wy)
}

/// UV → world at the world level (`0x4a7100` world-mode, verbatim). Returns (wx, wy). This IS
/// the client's world-level click→world law.
///
/// ANOMALY, CONFIRMED REAL (2026-07-07): NOT the algebraic inverse of
/// [`world_uv`] when scale ≠ 1 — the true inverse needs `offset·533.33/scale` but this uses the
/// unscaled offset; with the 5875 scale of 0.75 that's a real ~1.33× offset discrepancy. The two
/// functions genuinely disagree at the world level; each is reproduced as-is, and a round-trip
/// does not close. Phase 2's click path only consumes the continent pick, not the world output.
#[allow(dead_code)] // transcribed law, held for its first world-output consumer
pub(crate) fn world_click_world(proj: WorldProj, u: f32, v: f32) -> (f32, f32) {
    let wx = (proj.offset_v - ((v - 0.5) / proj.scale) * WORLD_SPAN_V_TILES) * TILE_YARDS;
    let wy = (proj.offset_u - ((u - 0.5) / proj.scale) * WORLD_SPAN_U_TILES) * TILE_YARDS;
    (wx, wy)
}

/// The continent's rect on the world sheet, normalized UV — the `0x4a5d00` builder kernel,
/// verbatim (X edges `(f2·f8 + xoff)/62.625` and `((f3+1)·f8 + xoff)/62.625`
/// with `xoff = (31.3125 − f8·32) + f6`; Y likewise over 41.75 with `yoff = (20.875 − f8·32) +
/// f7`), from the WorldMapContinent ADT tile bounds f2..f5. This rect is the world-level click's
/// AABB test (`0x4a7100`'s containment walk). Returns (u0, v0, u1, v1). The 5875 rects are
/// disjoint (Kalimdor u 0.089..0.400, EK u 0.624..0.923) — unlike the WorldMapArea art rects,
/// which overlap mid-ocean.
pub fn continent_sheet_rect(bounds: (u32, u32, u32, u32), proj: WorldProj) -> (f32, f32, f32, f32) {
    let (left, right, top, bottom) = bounds;
    let xoff = (31.3125 - proj.scale * 32.0) + proj.offset_u;
    let yoff = (20.875 - proj.scale * 32.0) + proj.offset_v;
    let u0 = (left as f32 * proj.scale + xoff) * (1.0 / WORLD_SPAN_U_TILES);
    let u1 = ((right + 1) as f32 * proj.scale + xoff) * (1.0 / WORLD_SPAN_U_TILES);
    let v0 = (top as f32 * proj.scale + yoff) * (1.0 / WORLD_SPAN_V_TILES);
    let v1 = ((bottom + 1) as f32 * proj.scale + yoff) * (1.0 / WORLD_SPAN_V_TILES);
    (u0, v0, u1, v1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 WorldMapContinent rows (dumped from the chain's dbc this session).
    const EK: WorldProj = WorldProj {
        offset_u: 14.5,
        offset_v: -7.0,
        scale: 0.75,
    };
    const KALIMDOR: WorldProj = WorldProj {
        offset_u: -19.0,
        offset_v: -0.322_498,
        scale: 0.75,
    };
    /// The real 5875 Elwynn row (WorldMapArea id 30).
    const ELWYNN: ZoneRect = ZoneRect {
        left: 1535.4166,
        right: -1935.4166,
        top: -7939.583,
        bottom: -10254.166,
    };

    fn close(a: (f32, f32), b: (f32, f32)) {
        assert!(
            (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4,
            "{a:?} vs {b:?}"
        );
    }

    /// Stormwind on the world sheet: right-hand continent (EK), southern half — the values are
    /// the formula evaluated at the real constants; the geography is the sanity.
    #[test]
    fn world_uv_stormwind() {
        close(world_uv(EK, -8842.0, 626.0), (0.71748, 0.63016));
    }

    /// Orgrimmar on the world sheet: left-hand continent (Kalimdor), upper-middle.
    #[test]
    fn world_uv_orgrimmar() {
        close(world_uv(KALIMDOR, 1629.0, -4373.0), (0.29480, 0.43740));
    }

    /// Goldshire inside the Elwynn rect: center-west, lower-middle.
    #[test]
    fn zone_uv_goldshire() {
        close(zone_uv(ELWYNN, -9450.0, 60.0), (0.42509, 0.65256));
    }

    /// The rect edges: west edge → u=0; the *east* edge hits the binary's numerator guard and
    /// yields 0 too (not 1) — transcribed quirk.
    #[test]
    fn zone_uv_edges() {
        let (u, _) = zone_uv(ELWYNN, -9450.0, ELWYNN.left);
        assert_eq!(u, 0.0);
        let (u, v) = zone_uv(ELWYNN, -9450.0, ELWYNN.right);
        assert_eq!(u, 0.0);
        assert!(v > 0.0, "v stays a real projection: {v}");
    }

    /// Off the rect on either axis → the (0,0) "hide the blip" sentinel.
    #[test]
    fn zone_uv_off_map() {
        assert_eq!(zone_uv(ELWYNN, -9450.0, ELWYNN.left + 10.0), (0.0, 0.0));
        assert_eq!(zone_uv(ELWYNN, ELWYNN.top + 10.0, 60.0), (0.0, 0.0));
    }

    /// Zone-mode UV→world: t=0 is the rect's top-left (north-west) corner, t=1 bottom-right —
    /// and both axes ride the same t (the transcribed 0x4a7100 quirk).
    #[test]
    fn zone_world_corners() {
        close(zone_world(ELWYNN, 0.0), (ELWYNN.top, ELWYNN.left));
        close(zone_world(ELWYNN, 1.0), (ELWYNN.bottom, ELWYNN.right));
    }

    /// Used ONE AXIS PER CALL — the way the reference's callers consume it, and the way the dev
    /// map-jump inverts a click — `zone_world` is the exact inverse of [`zone_uv`]. This is the
    /// claim the jump's landing accuracy rests on, so it is pinned rather than reasoned about:
    /// a world point projects to UV, and that UV comes back to the same point.
    #[test]
    fn zone_world_per_axis_inverts_zone_uv() {
        for (wx, wy) in [
            (ELWYNN.top - 1.0, ELWYNN.left - 1.0),
            (-9000.0, 100.0),
            (-9450.0, 500.0),
            (
                (ELWYNN.top + ELWYNN.bottom) * 0.5,
                (ELWYNN.left + ELWYNN.right) * 0.5,
            ),
        ] {
            let (u, v) = zone_uv(ELWYNN, wx, wy);
            assert!(u > 0.0 && v > 0.0, "fixture must be on the rect: {u} {v}");
            // wy rides the u axis, wx the v axis — the axis swap zone_uv makes.
            let (_, back_wy) = zone_world(ELWYNN, u);
            let (back_wx, _) = zone_world(ELWYNN, v);
            // Yards, not `close`'s 1e-4: these coordinates are ~10^4, so f32's ~7 digits leave
            // millimetre-scale slop. A hundredth of a yard is inverse enough to teleport onto.
            assert!(
                (back_wx - wx).abs() < 0.01 && (back_wy - wy).abs() < 0.01,
                "({back_wx}, {back_wy}) vs ({wx}, {wy})"
            );
        }
    }

    /// With scale = 1 the world-level pair IS inverse (the anomaly is scale-dependent — see
    /// `world_click_world`'s doc); pin the transcription shape through a roundtrip.
    #[test]
    fn world_click_roundtrip_at_unit_scale() {
        let p = WorldProj {
            offset_u: 14.5,
            offset_v: -7.0,
            scale: 1.0,
        };
        let (u, v) = world_uv(p, -8842.0, 626.0);
        let (wx, wy) = world_click_world(p, u, v);
        assert!(
            (wx - -8842.0).abs() < 0.5 && (wy - 626.0).abs() < 0.5,
            "{wx} {wy}"
        );
    }

    /// The 0x4a5d00 rect kernel at the real 5875 rows: EK tiles (23,47,15,61), Kalimdor
    /// (23,48,9,52) — expected values hand-evaluated from the recorded formula, and the two
    /// rects are disjoint on the u axis (the world-level click's whole disambiguation).
    #[test]
    fn continent_sheet_rects_disjoint() {
        let ek = continent_sheet_rect((23, 47, 15, 61), EK);
        let kal = continent_sheet_rect((23, 48, 9, 52), KALIMDOR);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(
            near(ek.0, 0.62375)
                && near(ek.1, 0.02695)
                && near(ek.2, 0.92315)
                && near(ek.3, 0.87126),
            "{ek:?}"
        );
        assert!(
            near(kal.0, 0.08882)
                && near(kal.1, 0.07910)
                && near(kal.2, 0.40020)
                && near(kal.3, 0.86952),
            "{kal:?}"
        );
        assert!(kal.2 < ek.0, "the sheet rects are disjoint on u");
    }
}
