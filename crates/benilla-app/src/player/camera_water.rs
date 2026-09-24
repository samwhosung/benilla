//! **The water corridor** — `cameraWaterCollision`'s *other* consumer, and the reason the option is
//! atomic.
//!
//! One CVar read at `0x50e5ec` produces one register, and that register has **two** consumers:
//!
//! * `esi`'s `0xf0000` nibble rides the trace mask to all three of `0x50e570`'s queries, so the
//!   camera boom hits a bare waterline ([`benilla_world::collision`]); and
//! * `0x50e629 test esi,0xf0000` admits the **floor/cap block** transcribed here, which re-bases the
//!   framing pivot against the waterline *before* the boom's origin is built from it (`0x50e786`).
//!
//! **Shipping either half alone is a visible defect, and this client has now shipped each once.**
//! 2149 built the corridor without the trace and put a discontinuity on screen; 2170 built the trace
//! without the corridor and pinned the camera to the waterline, because a surface swimmer's framing
//! pivot sits 11 mm *under* the water plane and the boom then straddles it every frame (2173 §1).
//! The floor is what lifts the sweep origin clear — to `surface + 2/9` — so the boom never starts
//! near the plane. Neither half is optional; see [`corridor`].
//!
//! Transcribed from the reference. Constants: `[0x8089cc] = 5/6`, `[0x8089d0] = 2/9`,
//! `[0x8089d4] = 5/9`, `[0x808a04] = 1/9`.
//!
//! **The two catch-up blocks, named and NOT built.**
//!
//! `0x50eeb0` (pivot height) and `0x50ee5d` (distance) are a matched pair that pull a live channel
//! down when it has drifted more than `1/9` above what the solver just produced:
//! `live > H + 1/9` strictly (equality and NaN skip), then a **hard store** of `H + 1/9 + 2⁻²⁰`
//! into the live field, plus an ease start at that value with duration **`2.0` s** — and the
//! channel's *target* deliberately left alone, so the ease walks back to where the camera was
//! always headed. Both can only ever decrease their field: `0x50e8bf` caps the arm at
//! `[cam+0xec]`, so `live − H ≥ 0` always, and no mirror block exists.
//!
//! **They are absent together, on purpose.** Three reasons, in order of weight:
//!
//! 1. **They are one mechanism, not two.** `ecx = 2.0f` — the duration for *both* — is loaded once
//!    at `0x50ee63`, inside the DISTANCE block's predicate. Lifting `0x50eeb0` alone stores
//!    garbage for its duration. This module's entire history is shipping one half of a pair
//!    (2149, 2170, 2173); a spec that opens by warning against it gets taken at its word.
//! 2. **Neither is gated on `cameraWaterCollision`** — control reaches them on two edges with no
//!    band bit, CVar or mode flag read anywhere in `0x50eeb0`–`0x50eef8`. They are general camera
//!    behaviour that this option merely *exposes*, and the distance twin changes what zoom does
//!    after any collision, on land as much as in water. That is its own change to a hot path,
//!    with its own gate to earn.
//! 3. Their only effect here is on the corridor's **upper** edge, and it makes an already-small
//!    artifact smaller. The lower edge — the one you can actually see — is unaffected: the eye is
//!    stored at `0x50ede5` before either block runs.
//!
//! Also underived, and flagged rather than guessed: whether the followed object's `z` is swept or
//! pinned entering the submerge band. `CMovement::UpdateLiquid` (`0x6a7650`) hands the physics step
//! a plane at exactly `z = liquidSurfaceZ`, which rules out a fixed offset but does not settle it,
//! and it sits in unresolved tension with the `0x610520` buoyancy equilibrium `|depth − 0.75h| <
//! 0.001`. This client builds the pinned reading.

/// `[0x8089cc]` — the corridor's resting floor, and the depth the pivot is pinned to while the
/// camera target is in the SUBMERGE band. Also the cap's own floor there.
const REST: f32 = 5.0 / 6.0;
/// `[0x8089d0]` — the SURFACE band's width, and the height above the waterline the floor lifts the
/// pivot to inside it. **This is the number that makes the trace survivable**: 2/9 yd of clearance
/// between the sweep origin and the plane it would otherwise start on top of.
const SURFACE_BAND: f32 = 2.0 / 9.0;
/// `[0x8089d4]` — the SUBMERGE band's outer edge. Its **only** reference image-wide is `0x511b83`,
/// in the classifier (three independent censuses) — it is emphatically *not* the constant in the
/// corridor's arm B, which is [`REST`]. 2149 shipped `5/9` there and stepped `7/9` where the
/// reference steps `19/18`; that single substitution is most of why its numbers came out at
/// 1.036/0.293 instead of 1.0556/0.2778.
const SUBMERGE_EDGE: f32 = 5.0 / 9.0;
/// `[0x808a04]` — the minimum head-room the pivot keeps above the corridor floor, and the offset
/// the catch-up blocks `0x50eeb0`/`0x50ee5d` would pin their live field to.
const MIN_HEADROOM: f32 = 1.0 / 9.0;

/// **Which liquid band the camera target is in** — the two bits `0x511ad0` writes into `[cam+0x90]`
/// (`0x100000` surface, `0x200000` submerge) and the only thing image-wide that writes either.
///
/// `0x511ae3` clears **both** unconditionally before any test, which is why there is no hysteresis
/// anywhere in this mechanism: every frame re-derives the band from scratch. That matters for what
/// can and cannot hide a step — see [`corridor`]'s note on the lower edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum WaterBand {
    /// Neither bit — no liquid, or deeper than [`SUBMERGE_EDGE`] past the pivot target.
    Clear,
    /// `0x100000` — the target is at or near the surface. The floor lifts the pivot to
    /// `surface + 2/9`.
    Surface,
    /// `0x200000` — a 1/3 yd window below the surface band. The pivot is pinned to
    /// `surface − 5/6`.
    Submerge,
}

/// **`0x511ad0`** — classify the camera target's depth, and return `d`.
///
/// `d = liquidSurfaceZ − targetObject.position.z`: the **followed unit's own origin** (feet), read
/// through vtable slot `+0x14`, not the pivot and not `*heightOut`. `[ebp+0x8]` (the pivot base)
/// comes from the same call at `0x50e9ce`, so both are measured from one origin — which is what
/// lets `d` be compared against a pivot height at all.
///
/// **The surface height is cached, never queried here.** `0x670630` is a field accessor:
/// `0x670637 test byte ptr [ecx+0x90],0x20` gates it and `0x670640 mov eax,[ecx+0x98]` returns it.
/// An invalid or null liquid object makes `0x511ad0` return `+0.0` with both bits cleared — hence
/// `surface_y: None` mapping to [`WaterBand::Clear`] and `d = 0.0`, not to a panic or a probe.
///
/// **The comparison is against the pivot TARGET, not its live value** — an asymmetry that's easy
/// to invert, but byte-exact: `[cam+0x1c8]` is the target and `[cam+0xfc]` the live value (the
/// only directional store is `0x50f3e7 call 0x5b7bb0 →
/// 0x50f3ec fstp [esi+0xfc]`; `0x50f3ff`'s `[0x1c8] := [0xfc]` is a settle guarded by
/// `|Δ| < 2⁻²²`). The classifier reads the target; the corridor seeds its cap from the live value.
/// Getting this backwards bands against a continuously-eased quantity and chatters on its own,
/// independently of any of the steps below.
pub(super) fn classify(surface_y: Option<f32>, feet_y: f32, target: f32) -> (WaterBand, f32) {
    let Some(surface_y) = surface_y else {
        return (WaterBand::Clear, 0.0);
    };
    let d = surface_y - feet_y;
    let excess = d - target;
    // Equality at `2/9` lands in SUBMERGE (the low side) and equality at `5/9` in `Clear` (the high
    // side) — the two edges' tie-breaks go opposite ways, which is the reference's own shape and
    // not a symmetry worth tidying.
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
    /// `[ebp-0xc]`, seeded from the **live** pivot height at `0x50e615`.
    pub(super) cap: f32,
}

/// **`0x50e629`–`0x50e685`** — the floor/cap block, gated on the CVar's own nibble.
///
/// | arm | predicate | floor | cap |
/// |---|---|---|---|
/// | gate off | `0x50e629 test esi,0xf0000` → `je 0x50e687` | `5/6` | `live` |
/// | **A** surface | `0x50e637 test eax,0x100000` | `d + 2/9` | `max(live, d + 2/9)` |
/// | **B** submerge | `0x50e65f test eax,0x200000` | `5/6` (untouched) | `max(d − 5/6, 5/6)` |
/// | **C** clear | both bits clear | `5/6` | `live` |
///
/// Arm A is the whole point: it pins the framing pivot at `surface + 2/9` while the target is at the
/// waterline, which is what puts 2/9 yd between the boom's origin and the water plane the same CVar
/// just made solid.
///
/// **The lower edge steps `−19/18` and that is the reference's own behaviour, not a defect.** At
/// `excess = 2/9` the pivot leaves `surface + 2/9` for `surface − 5/6` in one frame: `2/9 + 5/6 =
/// 19/18 ≈ 1.0556 yd`. Every candidate hiding mechanism was checked and refuted — no hysteresis
/// (`0x511ae3` clears both bits before any test), `d` is not quantised, and band B is a reachable
/// 1/3 yd window, and the reference's own catch-up cannot absorb it: `0x50eeb0` runs AFTER the
/// eye is stored (`0x50ede5`), and the whole function has zero backward branches.
///
/// **What keeps it off screen in normal play is that it is never approached continuously.** `d` is
/// the movement tick's pinned swim depth and the pivot target steps on the same swim flag, so
/// `d − target` is piecewise constant while swimming: a surface-swimming human male sits `0.011238`
/// into band A, `0.211` short of the edge. The corridor is crossed by **diving**, once — which is
/// exactly the event the reference wants to move the camera for. This is why 2173 §2's rule is
/// "compute where the camera lands", not "forbid every step": the step is real, intended, and
/// [`assert_bounded_step`](super::camera_channel::assert_bounded_step) is where its size is written
/// down rather than eyeballed.
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

/// The corridor the CVar's `off` state leaves behind — `0x50e629`'s `je` straight to `0x50e687`,
/// with both seeds untouched. Identical to [`WaterBand::Clear`]'s arm, which is why turning the
/// option off is indistinguishable from standing on dry land and not a second code path.
pub(super) fn corridor_off(live: f32) -> Corridor {
    Corridor {
        floor: REST,
        cap: live,
    }
}

/// **`0x50e756` then `0x50e767`** — the clamped pivot height the boom's origin is built from at
/// `0x50e786`.
///
/// `FLOOR + max(max(target, live) − FLOOR, 1/9)`, then capped. The head-room sweep scales the
/// bracketed term by its hit fraction, so `headroom` is `1.0` on a clear probe and the hit's `t`
/// otherwise — a low ceiling pulls the pivot down toward the floor rather than through it.
///
/// **Floor first, cap second, so the cap wins on a crossed corridor** (`FLOOR + 1/9 > CAP`, which is
/// arm A's normal state whenever the live pivot is below the waterline). That ordering is what
/// actually delivers `surface + 2/9`: the floor sets the corridor and the cap collapses onto it.
///
/// Neither clamp fires on NaN — both `je`s need C0 and C3 clear, so a NaN takes the assigning
/// fall-through and the seeds stand. Rust's `max`/`min` on `f32` return the non-NaN operand, which
/// is the same outcome by a different route; a NaN `d` therefore yields the resting corridor rather
/// than poisoning the pivot, and there is a test for it.
pub(super) fn pivot_height(c: &Corridor, target: f32, live: f32, headroom: f32) -> f32 {
    let reach = (target.max(live) - c.floor).max(MIN_HEADROOM) * headroom;
    (c.floor + reach).min(c.cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::camera_channel::assert_bounded_step;

    /// A human male's two numbers, both already shipping elsewhere in this tree: the swim framing
    /// pivot (`cam+0x124`, off the model's Stand/Swim sequence boxes) and the depth the movement
    /// tick pins a surface swimmer's feet to (`0.75 · CollisionHeight`, `h = 2.0309999`).
    const SWIM_PIVOT: f32 = 1.512_012;
    const SURFACE_DEPTH: f32 = 0.75 * 2.031;

    /// The pivot's height **relative to the water plane** — what the player actually sees, and the
    /// quantity every claim in this module is about. Feet at the origin, so `d` is the surface.
    fn pivot_over_surface(d: f32, target: f32, live: f32) -> f32 {
        let (band, dd) = classify(Some(d), 0.0, target);
        let c = corridor(band, dd, live);
        pivot_height(&c, target, live, 1.0) - d
    }

    /// **The defect 2170 shipped, stated as a test.** In the surface band the pivot is lifted to
    /// `2/9` above the waterline — so the boom's origin starts clear of the plane the same CVar
    /// makes solid, instead of 11 mm underneath it.
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

    /// **Why the reference never shows the lower step in normal swimming.** `d` is the movement
    /// tick's pinned depth and the pivot target steps on the same swim flag, so `d − target` is
    /// piecewise constant: a human male sits 0.0112 into a band 0.2222 wide. The corridor is
    /// crossed by diving, deliberately, once — not grazed at the surface.
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

    /// **The dive step, written down.** `2/9 + 5/6 = 19/18`. Not smoothed, not hidden — the
    /// reference's own deliberate "the pivot drops under the water with you" event, and the number
    /// 2149 got wrong (it shipped `5/9` in arm B and stepped 7/9).
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

    /// The upper edge — `+5/18`. **This is also what this client ships**, because the reference's
    /// two catch-up blocks (`0x50eeb0`/`0x50ee5d`) are absent: with them, leaving the submerge band
    /// steps `+1/9` at most and can go negative on a fast ascent. `+0.278` yd where the reference
    /// gives at most `+0.111`, once, on surfacing — smaller than the dive step above it, which is
    /// intended and four times larger.
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

    /// **The sweep 2165 §2 exists for, and the gate this feature ships behind.** Walk the water
    /// depth across BOTH edges and bound the step. Inside every band the pivot is continuous to
    /// well under a millimetre per 1 mm of depth; the only steps in the whole range are the two
    /// pinned above.
    #[test]
    fn the_corridor_is_continuous_inside_every_band() {
        let t = SWIM_PIVOT;
        // Band A's interior, approached from dry land and stopping short of the edge.
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

    /// And the whole range in one walk, with the largest intended jump as the bound — the form
    /// `assert_bounded_step`'s doc asks for, so a NEW cliff anywhere cannot hide behind the known
    /// one being larger.
    #[test]
    fn the_only_jump_in_the_whole_range_is_the_dive() {
        let t = SWIM_PIVOT;
        assert_bounded_step((t - 1.5, t + 4.0), 0.0005, 19.0 / 18.0 + 1e-3, |d| {
            pivot_over_surface(d, t, t)
        });
    }

    /// Turning the option off is standing on dry land — the same arm, not a second path. This is
    /// the assertion that keeps the two halves atomic: with the CVar clear there is no corridor
    /// AND (at the call site) no liquid in the trace mask.
    #[test]
    fn the_option_off_is_indistinguishable_from_dry_land() {
        let off = corridor_off(SWIM_PIVOT);
        let dry = corridor(WaterBand::Clear, 3.0, SWIM_PIVOT);
        assert_eq!(off.floor, dry.floor);
        assert_eq!(off.cap, dry.cap);
        assert_eq!(off.floor, REST);
    }

    /// No liquid object is `+0.0` with both bits clear (`0x511ad0`'s null return), and a NaN depth
    /// takes the assigning fall-through rather than poisoning the pivot.
    #[test]
    fn a_missing_surface_is_clear_and_a_nan_depth_does_not_poison_the_pivot() {
        assert_eq!(classify(None, 0.0, SWIM_PIVOT), (WaterBand::Clear, 0.0));
        let (band, d) = classify(Some(f32::NAN), 0.0, SWIM_PIVOT);
        assert_eq!(band, WaterBand::Clear, "NaN clears both bits");
        let h = pivot_height(&corridor(band, d, SWIM_PIVOT), SWIM_PIVOT, SWIM_PIVOT, 1.0);
        assert!(h.is_finite(), "the pivot stays finite, got {h}");
    }
}
