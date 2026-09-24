//! The unit blob shadow sizes from the Stand sequence's `M2Sequence` box (`0x711a20`), used raw
//! (clamped to ±5, then scaled), so `ModelAnimation::bounds_min/max` must round-trip the record.

use benilla_formats::{open_chain, parse_m2_animations};

#[test]
fn stand_box_extents_match_reference() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    // (model, Stand box dx, dy), measured off the real sequence records.
    let cases = [
        ("Creature\\Chicken\\Chicken.m2", 0.532_f32, 0.382_f32),
        ("Character\\Human\\Male\\HumanMale.m2", 0.913, 1.080),
    ];
    for (path, dx, dy) in cases {
        let bytes = chain.read_file(path).expect(path);
        let anims = parse_m2_animations(&bytes);
        // Stand's first variation: the box the shadow always projects.
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
        assert!(
            bmax[2] > bmin[2],
            "{path}: Stand box has no vertical extent"
        );
        for i in 0..3 {
            assert!(
                ((bmin[i] + bmax[i]) * 0.5 - stand.bounds_center[i]).abs() < 1e-4,
                "{path}: bounds_center[{i}] must be the box midpoint"
            );
        }
    }
}
