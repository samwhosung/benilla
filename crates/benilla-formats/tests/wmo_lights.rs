//! WMO MOLT lights, `0x30` bytes a record with a BGRA colour; the chunk walk steps past the
//! zero-size `MOVV` and `MOVB` that precede MOLT.

use benilla_formats::{parse_wmo_lights, Chain};

#[test]
fn goldshire_blacksmith_has_three_warm_omni_lights() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let bytes = reader
        .read("World\\wmo\\Azeroth\\Buildings\\GoldshireBlacksmith\\GoldshireBlacksmith.wmo")
        .expect("read GoldshireBlacksmith.wmo");

    let lights = parse_wmo_lights(&bytes);
    assert_eq!(lights.len(), 3, "blacksmith should have 3 MOLT lights");
    for l in &lights {
        assert!(
            l.is_omni(),
            "forge MOLT lights are type 0 (omni), got {}",
            l.light_type
        );
        // RGB (255, 140, 37).
        let c = l.color;
        assert!(
            c[0] > 0.9 && c[1] > 0.4 && c[1] < 0.7 && c[2] < 0.25,
            "forge light should be warm-orange, got {c:?}"
        );
    }
}

#[test]
fn no_molt_yields_empty() {
    assert!(parse_wmo_lights(&[0u8; 8]).is_empty());
}
