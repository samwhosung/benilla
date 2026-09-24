//! The chat bubble anchors at `unit.z + (Stand.max.z - Stand.min.z) * modelScale + 0.7`
//! (`0x4b0c30`): the Stand sequence box read from the file image (`0x4b0e38 call 0x711a20`),
//! latched once per chat line at `bubble+0x354`, not the posed PlayerName attachment (`0x608640`).

use benilla_formats::{load_m2_bounds, open_chain};

#[test]
fn stand_box_z_is_the_bubble_anchor_height_and_not_the_attachment() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    // (model, Stand box Z extent, attachment 18's Z), read off the shipped M2s. The Stand box is
    // `animationLookup[0]`'s record: 0 for the human, 2 for the chicken, whose record 0 is a flap.
    let cases = [
        (
            "Character\\Human\\Male\\HumanMale.mdx",
            2.0128_f32,
            2.2120_f32,
        ),
        ("Creature\\Chicken\\Chicken.mdx", 0.4435, 0.8090),
    ];

    for (path, stand_z, attach_z) in cases {
        let b = load_m2_bounds(&mut chain, path).unwrap_or_else(|e| panic!("bounds {path}: {e}"));
        assert!(
            (b.stand_box_z - stand_z).abs() < 0.001,
            "{path}: stand_box_z {:.4} should be the Stand CAaBox Z extent {stand_z}",
            b.stand_box_z
        );
        assert!(
            (b.stand_box_z - attach_z).abs() > 0.02,
            "{path}: stand_box_z {:.4} must not be the posed-attachment height {attach_z} — those \
             are the two mechanisms 1406 separated",
            b.stand_box_z
        );
        assert!(
            b.stand_box_z > 0.0,
            "{path}: a Stand box has positive height"
        );
        let header_z = b.bbox_max[2] - b.bbox_min[2];
        assert!(
            b.stand_box_z < header_z,
            "{path}: the Stand box {:.4} sits inside the all-animation box {header_z:.4}",
            b.stand_box_z
        );
    }
}
