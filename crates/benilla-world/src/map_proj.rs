//! The world-map projections between world position and normalized map UV (reference `0x4a7100`,
//! `0x4a7360`). World is wow coordinates (wx north, wy west, yards); UV is [0,1]² over the map
//! art, u east and v south, as `GetPlayerMapPosition` returns it and FrameXML multiplies it by the
//! detail frame's size (`x·w`, `−y·h` from TOPLEFT).
//!
//! The world level (both continents on one 62.625×41.75-tile sheet) projects by the
//! WorldMapContinent.dbc constants ([`world_uv`]). A continent or zone is a guarded lerp over its
//! WorldMapArea.dbc rect ([`zone_uv`]); a continent's rect is its areaId 0 row.

/// Yards per ADT tile (reference `0x80654c`).
const TILE_YARDS: f32 = 533.333_3;
/// The world sheet's span in tiles, u (`0x806548`) and v (`0x806550`), the 1002×668 art's aspect.
const WORLD_SPAN_U_TILES: f32 = 62.625;
const WORLD_SPAN_V_TILES: f32 = 41.75;
/// Yards to sheet fraction, `1/(span · tile)`, as the reference precomputes it (`0x806554`,
/// `0x806558`).
const K_U: f32 = 1.0 / (WORLD_SPAN_U_TILES * TILE_YARDS);
const K_V: f32 = 1.0 / (WORLD_SPAN_V_TILES * TILE_YARDS);

/// A continent's world-level projection: WorldMapContinent.dbc fields 6 and 7 (its sheet offset
/// in tiles, u and v) and 8 (the yard scale), read at record +0x18/+0x1c/+0x20 (`0x4a72b0`,
/// `0x4a7360`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldProj {
    pub offset_u: f32,
    pub offset_v: f32,
    pub scale: f32,
}

/// A WorldMapArea.dbc row's world rect: `left` is the west edge (max wy), `top` the north edge
/// (max wx), so left > right and top > bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneRect {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

/// World to UV on the world sheet (`0x4a7360` world mode): u from wy, v from wx, unclamped as in
/// the reference. `0x4a72b0`, the arrow frame's variant, is the same with a final `1 − v`.
pub fn world_uv(proj: WorldProj, wx: f32, wy: f32) -> (f32, f32) {
    let u = (proj.offset_u * (1.0 / WORLD_SPAN_U_TILES) + 0.5) - wy * K_U * proj.scale;
    let v = (proj.offset_v * (1.0 / WORLD_SPAN_V_TILES) + 0.5) - wx * K_V * proj.scale;
    (u, v)
}

/// World to UV inside a zone or continent rect (`0x4a7360` zone mode, quirks included). Each axis
/// is `1 − (in − low)/span`, 0 when the span or the numerator is 0, so the east and south edges
/// yield 0, not 1. If either leaves [0,1] both are zeroed, the `(0,0)` FrameXML reads as "not on
/// this map" (`[0x4a74d6, 0x4a7533)`: inclusive compares against 0.0 at `0x7ffd74` and 1.0 at
/// `0x7ff9d8`).
///
/// Two residual differences from the reference: a NaN passes its compares and is zeroed here (no
/// DBC or wire position makes one), and it compares its second axis at x87 precision, which `f32`
/// cannot copy and which matters only within an ulp of an edge.
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

/// UV to world `(wx, wy)` on a zone or continent rect (`0x4a7100` zone mode): both axes lerp by
/// the same `t`, as in the reference, whose callers take one axis per call; used so, it inverts
/// [`zone_uv`].
pub fn zone_world(rect: ZoneRect, t: f32) -> (f32, f32) {
    let wy = rect.left - (rect.left - rect.right) * t;
    let wx = rect.top - (rect.top - rect.bottom) * t;
    (wx, wy)
}

/// UV to world `(wx, wy)` at the world level (`0x4a7100` world mode), the reference's click law.
/// Not the inverse of [`world_uv`] when the scale is not 1: like the reference, it uses the
/// unscaled offset, so at the 0.75 scale a round trip misses.
#[allow(dead_code)] // transcribed law, held for its first world-output consumer
pub(crate) fn world_click_world(proj: WorldProj, u: f32, v: f32) -> (f32, f32) {
    let wx = (proj.offset_v - ((v - 0.5) / proj.scale) * WORLD_SPAN_V_TILES) * TILE_YARDS;
    let wy = (proj.offset_u - ((u - 0.5) / proj.scale) * WORLD_SPAN_U_TILES) * TILE_YARDS;
    (wx, wy)
}

/// A continent's `(u0, v0, u1, v1)` on the world sheet from its WorldMapContinent tile bounds
/// (fields 2 to 5), the `0x4a5d00` builder: the world-level click's hit test (`0x4a7100`). The two
/// rects are disjoint, unlike the WorldMapArea art rects, which overlap mid-ocean.
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

    /// The 5875 WorldMapContinent rows.
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
    /// The 5875 Elwynn row (WorldMapArea id 30).
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

    /// Stormwind, in EK's southern half; the expected values are the formula at the real constants.
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

    /// The west edge gives u = 0, and so does the east, through the reference's numerator guard.
    #[test]
    fn zone_uv_edges() {
        let (u, _) = zone_uv(ELWYNN, -9450.0, ELWYNN.left);
        assert_eq!(u, 0.0);
        let (u, v) = zone_uv(ELWYNN, -9450.0, ELWYNN.right);
        assert_eq!(u, 0.0);
        assert!(v > 0.0, "v stays a real projection: {v}");
    }

    /// Off the rect on either axis: the (0,0) "hide the blip" sentinel.
    #[test]
    fn zone_uv_off_map() {
        assert_eq!(zone_uv(ELWYNN, -9450.0, ELWYNN.left + 10.0), (0.0, 0.0));
        assert_eq!(zone_uv(ELWYNN, ELWYNN.top + 10.0, 60.0), (0.0, 0.0));
    }

    /// t = 0 is the north-west corner, t = 1 the south-east; both axes ride one t (`0x4a7100`).
    #[test]
    fn zone_world_corners() {
        close(zone_world(ELWYNN, 0.0), (ELWYNN.top, ELWYNN.left));
        close(zone_world(ELWYNN, 1.0), (ELWYNN.bottom, ELWYNN.right));
    }

    /// Used one axis per call, as the reference's callers and the dev map-jump use it,
    /// `zone_world` inverts [`zone_uv`]; the jump's landing rests on it.
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
            // wy rides the u axis, wx the v axis: `zone_uv`'s axis swap.
            let (_, back_wy) = zone_world(ELWYNN, u);
            let (back_wx, _) = zone_world(ELWYNN, v);
            // Yards, not `close`'s 1e-4: at ~10^4, f32 leaves millimetre slop.
            assert!(
                (back_wx - wx).abs() < 0.01 && (back_wy - wy).abs() < 0.01,
                "({back_wx}, {back_wy}) vs ({wx}, {wy})"
            );
        }
    }

    /// At scale 1 the world-level pair is inverse; the mismatch is the scale's.
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

    /// `0x4a5d00` at the 5875 rows, EK tiles (23,47,15,61) and Kalimdor (23,48,9,52), evaluated by
    /// hand; disjoint on u, which is how the world-level click tells them apart.
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
