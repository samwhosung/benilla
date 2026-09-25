//! The water corridor: `cameraWaterCollision`'s floor/cap block, which re-bases the framing pivot
//! against the waterline before the boom's origin is built from it (`0x50e786`).
//!
//! The one CVar read (`0x50e5ec`) has two consumers: its `0xf0000` nibble rides the trace mask of
//! all three `0x50e570` queries, so the boom hits the waterline ([`benilla_world::collision`]), and
//! `0x50e629 test esi,0xf0000` gates this block. They ship together: without the floor, a surface
//! swimmer's pivot sits 11 mm under the plane the trace makes solid, and the boom straddles it.
//!
//! Not built: the reference's catch-up pair `0x50eeb0` (pivot height) and `0x50ee5d` (distance),
//! which hard-store a live channel more than 1/9 above the solved `H` to `H + 1/9 + 2⁻²⁰` and
//! ease it for 2.0 s toward the untouched target. They are one mechanism (the shared duration is
//! loaded once, at `0x50ee63`, in the distance block), neither is gated on this CVar, and both
//! run after the eye is stored (`0x50ede5`), so here only the corridor's upper edge would move.
//!
//! Untraced: whether the followed unit's z is swept or pinned entering the submerge band
//! (`CMovement::UpdateLiquid` `0x6a7650`, the buoyancy equilibrium `0x610520`); ours is pinned.

/// `[0x8089cc]`: the resting floor, and the pivot's depth under the surface in the submerge band.
const REST: f32 = 5.0 / 6.0;
/// `[0x8089d0]`: the surface band's width, and the pivot's lift above the waterline inside it.
const SURFACE_BAND: f32 = 2.0 / 9.0;
/// `[0x8089d4]`: the submerge band's outer edge, read only by the classifier (`0x511b83`); arm B's
/// constant is [`REST`], not this.
const SUBMERGE_EDGE: f32 = 5.0 / 9.0;
/// `[0x808a04]`: the least head-room the pivot keeps above the corridor floor.
const MIN_HEADROOM: f32 = 1.0 / 9.0;

/// The camera target's liquid band, the two bits `0x511ad0` writes into `[cam+0x90]`. Both are
/// cleared before any test (`0x511ae3`), so every frame re-derives the band: no hysteresis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum WaterBand {
    /// Neither bit: no liquid, or deeper than [`SUBMERGE_EDGE`] past the pivot target.
    Clear,
    /// `0x100000`: at or near the surface; the floor lifts the pivot to `surface + 2/9`.
    Surface,
    /// `0x200000`: a 1/3 yd window below the surface band; the pivot sits at `surface − 5/6`.
    Submerge,
}

/// `0x511ad0`: the band, and `d = liquidSurfaceZ − z` of the followed unit's own origin (vtable
/// slot `+0x14`), the origin the pivot base also comes from (`0x50e9ce`). The surface is a cached
/// field (`0x670630`); with no valid liquid it returns `+0.0` with both bits clear. The band is
/// judged against the pivot target (`[cam+0x1c8]`), not the live value (`[cam+0xfc]`) the cap
/// seeds from; banding on the eased live value would chatter.
pub(super) fn classify(surface_y: Option<f32>, feet_y: f32, target: f32) -> (WaterBand, f32) {
    let Some(surface_y) = surface_y else {
        return (WaterBand::Clear, 0.0);
    };
    let d = surface_y - feet_y;
    let excess = d - target;
    // A tie at 2/9 lands in Submerge and one at 5/9 in Clear: the reference's own tie-breaks.
    let band = if excess < SURFACE_BAND {
        WaterBand::Surface
    } else if excess < SUBMERGE_EDGE {
        WaterBand::Submerge
    } else {
        WaterBand::Clear
    };
    (band, d)
}

/// The floor/cap pair the corridor block leaves in `[ebp-0x4]` / `[ebp-0xc]`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Corridor {
    /// `[ebp-0x4]`, seeded [`REST`] at `0x50e604`.
    pub(super) floor: f32,
    /// `[ebp-0xc]`, seeded from the live pivot height at `0x50e615`.
    pub(super) cap: f32,
}

/// `0x50e629`–`0x50e685`: the floor/cap block, gated on the CVar's own nibble.
///
/// | arm | predicate | floor | cap |
/// |---|---|---|---|
/// | gate off | `0x50e629 test esi,0xf0000` → `je 0x50e687` | `5/6` | `live` |
/// | A surface | `0x50e637 test eax,0x100000` | `d + 2/9` | `max(live, d + 2/9)` |
/// | B submerge | `0x50e65f test eax,0x200000` | `5/6` (untouched) | `max(d − 5/6, 5/6)` |
/// | C clear | both bits clear | `5/6` | `live` |
///
/// From A to B the pivot drops `2/9 + 5/6 = 19/18` yd in one frame, as in the reference, where
/// nothing smooths it. Swimming never grazes that edge: `d − target` is piecewise constant while
/// swimming (a surface-swimming human male sits `0.011238` into band A, `0.211` short of it), so
/// a dive crosses it once.
pub(super) fn corridor(band: WaterBand, d: f32, live: f32) -> Corridor {
    match band {
        WaterBand::Surface => {
            let floor = d + SURFACE_BAND;
            Corridor {
                floor,
                cap: live.max(floor),
            }
        }
        WaterBand::Submerge => Corridor {
            floor: REST,
            cap: (d - REST).max(REST),
        },
        WaterBand::Clear => Corridor {
            floor: REST,
            cap: live,
        },
    }
}

/// The CVar off: `0x50e629`'s `je` to `0x50e687` leaves both seeds, the same corridor as dry land.
pub(super) fn corridor_off(live: f32) -> Corridor {
    Corridor {
        floor: REST,
        cap: live,
    }
}

/// `0x50e756` then `0x50e767`: the pivot height the boom's origin is built from (`0x50e786`),
/// `floor + max(max(target, live) − floor, 1/9) × headroom`, then capped. `headroom` is the
/// head-room probe's hit fraction, 1.0 when clear. The cap applies last, so it wins on a crossed
/// corridor (arm A with the live pivot under the waterline), landing the pivot at `surface + 2/9`.
/// On NaN the reference's compares leave the seeds standing, as Rust's `max`/`min` (returning the
/// other operand) do, so a NaN `d` yields the resting corridor.
pub(super) fn pivot_height(c: &Corridor, target: f32, live: f32, headroom: f32) -> f32 {
    let reach = (target.max(live) - c.floor).max(MIN_HEADROOM) * headroom;
    (c.floor + reach).min(c.cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::camera_channel::assert_bounded_step;

    /// A human male's swim framing pivot (`cam+0x124`, off the model's Stand and Swim sequence
    /// boxes) and surface swim depth (`0.75 · CollisionHeight`, `h = 2.0309999`).
    const SWIM_PIVOT: f32 = 1.512_012;
    const SURFACE_DEPTH: f32 = 0.75 * 2.031;

    /// The pivot's height over the water plane, feet at the origin so `d` is the surface.
    fn pivot_over_surface(d: f32, target: f32, live: f32) -> f32 {
        let (band, dd) = classify(Some(d), 0.0, target);
        let c = corridor(band, dd, live);
        pivot_height(&c, target, live, 1.0) - d
    }

    #[test]
    fn the_surface_band_lifts_the_pivot_clear_of_the_water() {
        let over = pivot_over_surface(SURFACE_DEPTH, SWIM_PIVOT, SWIM_PIVOT);
        assert!(
            (over - SURFACE_BAND).abs() < 1e-5,
            "a surface swimmer's pivot must sit 2/9 ABOVE the plane, got {over:+}"
        );
        assert!(
            over > 0.0,
            "and above it at all — this is the whole difference between 2170 and a working camera"
        );
    }

    #[test]
    fn a_surface_swimmer_sits_far_inside_the_band_and_never_grazes_its_edge() {
        let excess = SURFACE_DEPTH - SWIM_PIVOT;
        assert!(
            (excess - 0.011_238).abs() < 1e-4,
            "the pinned excess is 0.011238, got {excess}"
        );
        assert_eq!(
            classify(Some(SURFACE_DEPTH), 0.0, SWIM_PIVOT).0,
            WaterBand::Surface
        );
        assert!(
            SURFACE_BAND - excess > 0.2,
            "and it clears the edge by 0.211 yd, which is what makes the step below unreachable \
             while swimming"
        );
    }

    #[test]
    fn crossing_into_the_submerge_band_drops_the_pivot_by_nineteen_eighteenths() {
        let edge = SWIM_PIVOT + SURFACE_BAND;
        let above = pivot_over_surface(edge - 1e-4, SWIM_PIVOT, SWIM_PIVOT);
        let below = pivot_over_surface(edge + 1e-4, SWIM_PIVOT, SWIM_PIVOT);
        assert!(
            (above - SURFACE_BAND).abs() < 1e-3,
            "just inside: +2/9, got {above:+}"
        );
        assert!(
            (below + REST).abs() < 1e-3,
            "just past: −5/6, got {below:+}"
        );
        let step = below - above;
        assert!(
            (step + 19.0 / 18.0).abs() < 1e-3,
            "the step is −19/18 = −1.0556, got {step:+}"
        );
    }

    /// Leaving the submerge band steps `+5/18` here; the reference's catch-up blocks (`0x50eeb0`,
    /// `0x50ee5d`), not built here, hold it to `+1/9` at most, negative on a fast ascent.
    #[test]
    fn leaving_the_submerge_band_with_the_live_pivot_held_is_the_closed_forms_five_eighteenths() {
        let edge = SWIM_PIVOT + SUBMERGE_EDGE;
        let inside = pivot_over_surface(edge - 1e-4, SWIM_PIVOT, SWIM_PIVOT);
        let outside = pivot_over_surface(edge + 1e-4, SWIM_PIVOT, SWIM_PIVOT);
        let step = outside - inside;
        assert!(
            (step - 5.0 / 18.0).abs() < 1e-3,
            "the held-live step is +5/18 = +0.2778, got {step:+}"
        );
    }

    #[test]
    fn the_corridor_is_continuous_inside_every_band() {
        let t = SWIM_PIVOT;
        // Band A's interior, from dry land to short of the edge.
        assert_bounded_step((t - 1.5, t + SURFACE_BAND - 0.01), 0.001, 0.002, |d| {
            pivot_over_surface(d, t, t)
        });
        // Band B's interior.
        assert_bounded_step(
            (t + SURFACE_BAND + 0.01, t + SUBMERGE_EDGE - 0.01),
            0.001,
            0.002,
            |d| pivot_over_surface(d, t, t),
        );
        // Past the outer edge, down to a proper dive.
        assert_bounded_step((t + SUBMERGE_EDGE + 0.01, t + 4.0), 0.001, 0.002, |d| {
            pivot_over_surface(d, t, t)
        });
    }

    #[test]
    fn the_only_jump_in_the_whole_range_is_the_dive() {
        let t = SWIM_PIVOT;
        assert_bounded_step((t - 1.5, t + 4.0), 0.0005, 19.0 / 18.0 + 1e-3, |d| {
            pivot_over_surface(d, t, t)
        });
    }

    #[test]
    fn the_option_off_is_indistinguishable_from_dry_land() {
        let off = corridor_off(SWIM_PIVOT);
        let dry = corridor(WaterBand::Clear, 3.0, SWIM_PIVOT);
        assert_eq!(off.floor, dry.floor);
        assert_eq!(off.cap, dry.cap);
        assert_eq!(off.floor, REST);
    }

    /// No liquid is `0x511ad0`'s null return, `+0.0` with both bits clear.
    #[test]
    fn a_missing_surface_is_clear_and_a_nan_depth_does_not_poison_the_pivot() {
        assert_eq!(classify(None, 0.0, SWIM_PIVOT), (WaterBand::Clear, 0.0));
        let (band, d) = classify(Some(f32::NAN), 0.0, SWIM_PIVOT);
        assert_eq!(band, WaterBand::Clear, "NaN clears both bits");
        let h = pivot_height(&corridor(band, d, SWIM_PIVOT), SWIM_PIVOT, SWIM_PIVOT, 1.0);
        assert!(h.is_finite(), "the pivot stays finite, got {h}");
    }
}
