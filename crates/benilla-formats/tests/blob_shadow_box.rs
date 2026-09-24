//! Pins the unit blob shadow's box source against real build-5875 assets: the shadow sizes from
//! the **current animation's** `M2Sequence` CAaBox (`0x711a20` — the
//! same sequence-record box the pick volume and the ring's Stand-footprint read), so
//! `ModelAnimation::bounds_min/max` must round-trip the raw record. The Stand-box horizontal
//! extents of the four reference-traced creatures are already pinned by
//! `selection_ring_radius.rs` *through* the nested-sqrt footprint; this asserts the **box itself**
//! (the shadow consumes it raw: clamp ±5, scale, AA-bound — no compression). Skips when the
//! gitignored client data isn't present.

use benilla_formats::{open_chain, parse_m2_animations};

#[test]
fn stand_box_extents_match_reference() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    // (model, Stand-box horizontal extents (dx, dy) — measured straight off the real sequence
    // records).
    let cases = [
        ("Creature\\Chicken\\Chicken.m2", 0.532_f32, 0.382_f32),
        ("Character\\Human\\Male\\HumanMale.m2", 0.913, 1.080),
    ];
    for (path, dx, dy) in cases {
        let bytes = chain.read_file(path).expect(path);
        let anims = parse_m2_animations(&bytes);
        // Stand (anim id 0)'s head variation — the box the idle shadow projects.
        let stand = anims
            .iter()
            .find(|a| a.anim_id == 0)
            .unwrap_or_else(|| panic!("{path}: no Stand sequence"));
        let (bmin, bmax) = (stand.bounds_min, stand.bounds_max);
        assert!(
            (bmax[0] - bmin[0] - dx).abs() < 0.01,
            "{path}: Stand box dx {:.3} != reference {dx}",
            bmax[0] - bmin[0]
        );
        assert!(
            (bmax[1] - bmin[1] - dy).abs() < 0.01,
            "{path}: Stand box dy {:.3} != reference {dy}",
            bmax[1] - bmin[1]
        );
        // The box is a real volume: a degenerate Z extent would zero the shadow's vertical reach.
        assert!(
            bmax[2] > bmin[2],
            "{path}: Stand box has no vertical extent"
        );
        // And the centre derives from the same corners (the sphere fields already shipped).
        for i in 0..3 {
            assert!(
                ((bmin[i] + bmax[i]) * 0.5 - stand.bounds_center[i]).abs() < 1e-4,
                "{path}: bounds_center[{i}] must be the box midpoint"
            );
        }
    }
}
