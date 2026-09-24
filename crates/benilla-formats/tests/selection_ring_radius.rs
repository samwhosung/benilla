//! Pins the selection-ring radius against the **pixel-measured reference** on real build-5875 assets.
//!
//! The reference client sizes a **living unit's** ground selection ring (draw `0x608e00`, sizer
//! `0x60aee0`) as:
//!
//! ```text
//! ring_radius = OBJECT_FIELD_SCALE_X × sqrt( 0.5 · sqrt(dx² + dy²) )
//! ```
//!
//! where `dx,dy` are the horizontal (X,Y) extents of the unit's **Stand** animation bounding box (the M2
//! `M2Sequence` CAaBox), Stand being animation **id 0** via `animationLookup[0]`. This formula
//! reproduces the reference apitrace's measured ring radii to ~1 mm. It is NOT the render
//! bounding sphere
//! (`0xCC`) — that (`0x5d6fe0`, `[unit+0x2b0]`) is the *corpse* decal; the render sphere over-sized tall
//! humans and under-sized the squat chicken because it folds in height, whereas the nested-sqrt footprint
//! compresses range.
//!
//! [`M2Bounds::ring_footprint`] is the model-local (pre-scale) part; the world radius is it ×
//! `OBJECT_FIELD_SCALE_X`. The four creatures below were the ones captured in the reference trace, drawn
//! life-size (scale ≈ 1.0), so `ring_footprint` ≈ the measured world radius. Skips when the gitignored
//! client data isn't present.

use benilla_formats::{load_m2_bounds, open_chain};

#[test]
fn ring_footprint_matches_reference_pixels() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    // (model, ring radius measured from the reference apitrace at scale 1.0). The Stand-box footprint
    // must reproduce these; a header-render-box or render-sphere source does not.
    let cases = [
        ("Creature\\Chicken\\Chicken.mdx", 0.572_f32),
        ("Character\\Human\\Female\\HumanFemale.mdx", 0.731),
        ("Character\\Human\\Male\\HumanMale.mdx", 0.841),
        ("Creature\\Horse\\Horse.mdx", 1.295),
    ];

    for (path, measured) in cases {
        let b = load_m2_bounds(&mut chain, path).unwrap_or_else(|e| panic!("bounds {path}: {e}"));
        assert!(
            (b.ring_footprint - measured).abs() < 0.01,
            "{path}: ring_footprint {:.4} should reproduce the reference-measured {measured} (Stand-box \
             sqrt(0.5·sqrt(dx²+dy²)))",
            b.ring_footprint
        );
        // Guard against a regression to the (wrong) corpse-path render-sphere source: for all four it is
        // far off (0.5×sphere = 0.33 / 1.19 / 1.39 / 1.96), so it can't sneak past the bound above.
        assert!(
            (b.ring_footprint - 0.5 * b.sphere_radius).abs() > 0.05,
            "{path}: ring_footprint must not coincide with the corpse-path 0.5×renderSphere"
        );
    }
}

/// The sizer's **other** branch: a box whose X and Y extents are both exactly zero takes the
/// literal 1.2 (`0x60af4f..0x60af67`, `0x3f99999a`) and never reaches the formula.
///
/// This branch reads as one that "never fires for real creatures" — true of the four
/// life-size units measured, false of the whole trigger-creature family. `InvisibleStalker`
/// authors **all 135** of its sequence boxes at zero, so 1.2 is its ring, and the Naxxramas weapon
/// mobs (an `InvisibleStalker` body holding a visible axe, display 15294 at scale 2.25 ⇒ a 2.7 yd
/// ring) are exactly where a player meets it. We computed `sqrt(0.5·sqrt(0))` = 0 and drew a ring
/// the width of a coin — the director's A/B against the reference is what caught it.
#[test]
fn a_degenerate_stand_box_rings_at_the_reference_constant() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    let path = "Creature\\InvisibleStalker\\InvisibleStalker.mdx";
    let b = load_m2_bounds(&mut chain, path).unwrap_or_else(|e| panic!("bounds {path}: {e}"));
    // The premise: this model really is the degenerate case, on the header box and the Stand box
    // alike (0 vertices ⇒ nothing to bound). If Blizzard had authored a box here the constant
    // below would be the wrong answer, so pin the input, not just the output.
    assert_eq!(
        (b.bbox_min, b.bbox_max, b.sphere_radius),
        ([0.0; 3], [0.0; 3], 0.0),
        "{path}: expected an entirely unauthored bound"
    );
    assert_eq!(
        b.ring_footprint,
        benilla_formats::DEGENERATE_RING_FOOTPRINT,
        "{path}: a zero-extent box takes the writer's 1.2, not the formula's 0"
    );
}
