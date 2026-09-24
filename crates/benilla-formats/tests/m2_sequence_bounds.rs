//! Pins the per-sequence bounds-sphere parse (`M2Sequence` CAaBox @+0x24/+0x30, radius @+0x3c) on
//! real build-5875 unit models. This sphere is the mouse-pick **broad phase** for the model's
//! current animation (`0x7089c0`): if it parsed as
//! zero or a sliver, the faithful pick would reject clicks the reference accepts. Skips when the
//! gitignored client data isn't present.

use benilla_formats::{open_chain, parse_m2_animations};

#[test]
fn stand_sequence_bounds_sphere_is_body_scale() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");

    // (model, Stand bounds-sphere radius read off the real files). Note the chicken: its sphere
    // (0.30) is *smaller* than its ring footprint (0.57) — the ring's nested-sqrt inflates small
    // extents — so no ring-relative bound holds in general; the parsed values themselves are the pin.
    for (path, radius) in [
        ("Creature\\Chicken\\Chicken.mdx", 0.304_f32),
        ("Creature\\Horse\\Horse.mdx", 2.019),
        ("Creature\\Wolf\\Wolf.mdx", 1.963),
        ("Creature\\Kobold\\Kobold.mdx", 0.970),
        ("Character\\Human\\Male\\HumanMale.mdx", 1.118),
    ] {
        // The chain stores `.m2`; the DBC-style `.mdx` spelling normalizes (as the loaders do).
        let bytes = chain
            .read(&path.to_ascii_lowercase().replace(".mdx", ".m2"))
            .expect("read m2");
        let anims = parse_m2_animations(&bytes);
        let stand = anims
            .iter()
            .find(|a| a.anim_id == 0)
            .expect("Stand sequence");
        assert!(
            (stand.bounds_radius - radius).abs() < 5e-3,
            "{path}: Stand bounds sphere {:.3}, expected {radius:.3}",
            stand.bounds_radius
        );
        // The centre sits above the feet (positive raw-WoW Z), inside the sphere — a zeroed/garbage
        // box would fail one of these.
        assert!(stand.bounds_center[2] > 0.0 && stand.bounds_center[2] < stand.bounds_radius * 2.0);
    }
}
