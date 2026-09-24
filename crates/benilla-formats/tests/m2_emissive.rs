//! The M2 material flag UNLIT (0x01) is `emissive`: GeneralLantern03's glass (flags 0x05) is
//! self-lit, its body (0x00) is not.

use benilla_formats::{load_m2_mesh, open_chain};

#[test]
fn lantern_glass_batch_is_emissive() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "World\\Generic\\Human\\Passive Doodads\\Lanterns\\GeneralLantern03.m2",
    )
    .expect("load GeneralLantern03");

    assert!(
        subs.iter().any(|s| s.emissive),
        "the lantern has a self-lit (UNLIT 0x01) glass batch"
    );
    assert!(
        subs.iter().any(|s| !s.emissive),
        "the lantern body batch is lit normally (not emissive)"
    );
}
