//! The M2 light block: `0xd4` bytes a record, the diffuse-colour track at `0x48`.

use benilla_formats::{parse_m2_lights, Chain};

#[test]
fn elwynn_campfire_has_one_warm_point_light() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let bytes = reader
        .read("World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.m2")
        .expect("read ElwynnCampfire.m2");

    let lights = parse_m2_lights(&bytes);
    assert_eq!(lights.len(), 1, "campfire has exactly one M2 light");

    let l = lights[0];
    assert!(
        l.is_point(),
        "campfire light is type 1 (point), got type {}",
        l.light_type
    );
    let d = l.diffuse_color;
    assert!(
        (d[0] - 0.71).abs() < 0.03 && (d[1] - 0.20).abs() < 0.03 && d[2] < 0.05,
        "campfire diffuse ≈ (0.71, 0.20, 0.00), got {d:?}"
    );
}

/// The torch an NPC carries (`ItemDisplayInfo` 12236, item 1906 "Monster - Torch"): a point light
/// on bone 9 up the shaft, at intensity 3, whose visibility track's first key is nonzero: it casts.
#[test]
fn held_torch_casts_one_warm_point_light_up_the_shaft() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let bytes = reader
        .read("Item\\ObjectComponents\\Weapon\\Club_1H_Torch_A_01.m2")
        .expect("read Club_1H_Torch_A_01.m2");

    let lights = parse_m2_lights(&bytes);
    assert_eq!(lights.len(), 1, "the torch has exactly one M2 light");
    let l = lights[0];
    assert!(l.casts(), "point + visibility-on: the torch really glows");
    assert_eq!(l.bone, 9, "the light hangs on the torch's own bone");
    assert!(
        (l.position[0] - 0.5765).abs() < 0.01 && l.position[1].abs() < 0.01,
        "0.58 yd up the shaft (the head), got {:?}",
        l.position
    );
    let d = l.diffuse_color.map(|c| c * l.diffuse_intensity);
    assert!(
        (d[0] - 1.40).abs() < 0.01 && (d[1] - 0.871).abs() < 0.01 && (d[2] - 0.40).abs() < 0.01,
        "diffuse × intensity ≈ (1.40, 0.87, 0.40) — flame orange, over-driven, got {d:?}"
    );
}

/// A point light whose visibility track is a static `0` key never casts (`0x716413`): 11 of the
/// corpus's 85, mostly spell impacts.
#[test]
fn a_static_zero_visibility_key_reads_as_dark() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let bytes = reader
        .read("Spells\\FlameStrike_ImpactDD_Med_Base.m2")
        .expect("read FlameStrike_ImpactDD_Med_Base.m2");

    let dark: Vec<_> = parse_m2_lights(&bytes)
        .into_iter()
        .filter(|l| l.is_point())
        .collect();
    assert!(
        !dark.is_empty(),
        "the impact model does author point lights"
    );
    assert!(
        dark.iter().all(|l| l.visibility_off && !l.casts()),
        "every one is held dark by its visibility track"
    );
}

#[test]
fn too_short_yields_no_lights() {
    assert!(parse_m2_lights(&[0u8; 16]).is_empty());
}
