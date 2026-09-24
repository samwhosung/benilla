//! The camera's swim framing pivot. Per frame `0x50f880` picks one of three presets: `cam+0x124`
//! while swimming (movement flag `0x200000`), else `cam+0x11c` or `cam+0x120` split on
//! `cam+0x198 < 1.8315`, a pair benilla serves with one standing height. `0x50ca90` builds all
//! three from `attach17.z + 0.0972` and lowers only `+0x124`, by `S * (Stand.max.z - Swim.max.z)`
//! over the `0x711a20` boxes of sequences 0 and 42 (`0x50cccf`, `0x50ccde`, `0x50ccf6`). For the
//! Human Male at scale 1 the binary gives `+0x11c = +0x120 = 1.9002692`, `+0x124 = 1.5120120`.

use benilla_formats::{load_m2_bounds, open_chain};

/// The presets' base above attachment 17: `0.0972222` in the reference (`[0x808ab0]`), `0.0972`
/// in `entities::display::build_parts`. The 0.02 mm gap is why the presets get a 1e-4 window.
const PIVOT_BASE: f32 = 0.0972;

#[test]
fn the_swim_preset_is_the_stand_minus_swim_sequence_box_delta() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    let b = load_m2_bounds(&mut chain, "Character\\Human\\Male\\HumanMale.mdx")
        .expect("bounds HumanMale");
    let standing = b.pivot_z.expect("HumanMale authors attachment 17") + PIVOT_BASE;

    assert!(
        (b.swim_pivot_drop - 0.3882572).abs() < 1e-5,
        "HumanMale swim_pivot_drop {:.7} should be the reference's 1.9002692 − 1.5120120 = 0.3882572",
        b.swim_pivot_drop
    );
    assert!(
        (standing - 1.9002692).abs() < 1e-4,
        "HumanMale standing preset {standing:.7} should be the reference's cam+0x11c 1.9002692"
    );
    assert!(
        (standing - b.swim_pivot_drop - 1.512_012).abs() < 1e-4,
        "HumanMale swim preset {:.7} should be the reference's cam+0x124 1.5120120",
        standing - b.swim_pivot_drop
    );
}

#[test]
fn the_drop_is_per_model_and_zero_for_a_body_with_no_swim_sequence() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    // (model, the drop measured off the shipped M2); how far the swim pose sinks is per skeleton.
    let cases = [
        ("Character\\Human\\Male\\HumanMale.mdx", 0.3882571_f32),
        ("Character\\Tauren\\Male\\TaurenMale.mdx", 0.4236124),
        ("Character\\NightElf\\Female\\NightElfFemale.mdx", 0.1288066),
        // No Swim sequence: the reference's both-present guard (`0x711960` on 0 and 0x2a at
        // `0x50cc43`/`0x50cc4f`) keeps the standing preset.
        ("Creature\\Chicken\\Chicken.mdx", 0.0),
    ];

    for (path, drop) in cases {
        let b = load_m2_bounds(&mut chain, path).unwrap_or_else(|e| panic!("bounds {path}: {e}"));
        assert!(
            (b.swim_pivot_drop - drop).abs() < 1e-5,
            "{path}: swim_pivot_drop {:.7} should be {drop:.7}",
            b.swim_pivot_drop
        );
        let standing = b.pivot_z.map(|z| z + PIVOT_BASE).unwrap_or(0.0);
        assert!(
            b.swim_pivot_drop >= 0.0 && b.swim_pivot_drop < standing,
            "{path}: a swim drop of {:.7} against a standing preset of {standing:.7} is not a dip",
            b.swim_pivot_drop
        );
    }
}
