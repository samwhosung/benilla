//! A living unit's selection ring (draw `0x608e00`, sizer `0x60aee0`) has radius
//! `OBJECT_FIELD_SCALE_X * sqrt(0.5 * sqrt(dx² + dy²))` over the Stand sequence box's horizontal
//! extents (`animationLookup[0]`). The render sphere (`0xCC`) sizes the corpse decal instead
//! (`0x5d6fe0`, `[unit+0x2b0]`). `M2Bounds::ring_footprint` is the pre-scale part.

use benilla_formats::{load_m2_bounds, open_chain};

#[test]
fn ring_footprint_matches_reference_pixels() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    // (model, ring radius measured off a reference frame capture at scale 1).
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
        assert!(
            (b.ring_footprint - 0.5 * b.sphere_radius).abs() > 0.05,
            "{path}: ring_footprint must not coincide with the corpse-path 0.5×renderSphere"
        );
    }
}

/// A box whose X and Y extents are both zero takes the literal 1.2 (`0x60af4f`..`0x60af67`,
/// `0x3f99999a`): `InvisibleStalker` authors all 135 sequence boxes at zero, so the Naxxramas
/// weapon mobs on its body (display 15294, scale 2.25) ring at 2.7 yd.
#[test]
fn a_degenerate_stand_box_rings_at_the_reference_constant() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    let path = "Creature\\InvisibleStalker\\InvisibleStalker.mdx";
    let b = load_m2_bounds(&mut chain, path).unwrap_or_else(|e| panic!("bounds {path}: {e}"));
    // No vertices, so nothing is bounded, the header box and the Stand box alike.
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
