//! A band row with no keyframes commits the reference's constant, black or `+0.0`: the colour
//! evaluator `0x6d62e0` stores `0xff000000` (guard `0x6d62ef`, store `0x6d62f6`), the scalar one
//! `+0.0f` (`0x6d6489`, `fld [0x7ffd74]`), and both copy on unconditionally. 973 of 7668
//! `LightIntBand` rows and 132 of 2556 `LightFloatBand` rows are keyless.

use benilla_formats::{Chain, LightCatalog, ZERO_KEY_COLOR, ZERO_KEY_SCALAR};

/// Half-minutes; 1440 = noon.
const NOON: u32 = 1440;
/// Blasted Lands' clear `LightParams`; its cloud gradient base, `LightIntBand` 643, has no keys.
const PARAMS_BLASTED_LANDS: u32 = 36;
/// The Azeroth global's clear `LightParams`; its cloud gradient base is keyed, and black.
const PARAMS_ELWYNN: u32 = 12;

fn params(id: u32) -> Option<benilla_formats::Atmosphere> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = Chain::open(&data).expect("open vanilla patch chain");
    let cat = LightCatalog::load(&mut chain).expect("load Light/LightParams/*Band");
    Some(
        cat.sample_params_id(id, NOON)
            .expect("shipped LightParams id"),
    )
}

#[test]
fn the_blasted_lands_cloud_base_is_black_not_a_pale_invention() {
    let Some(atmo) = params(PARAMS_BLASTED_LANDS) else {
        return;
    };

    assert_eq!(
        atmo.cloud_colors[2], ZERO_KEY_COLOR,
        "sub-12 is keyless for LightParams 36 — the cloud gradient base must be black"
    );
    // Its authored neighbours: the row is read, not the palette blanked.
    let rgb = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as i32);
    assert_eq!(
        rgb(atmo.cloud_colors[0]),
        [133, 47, 0],
        "sub-10 sun glow tint"
    );
    assert_eq!(
        rgb(atmo.cloud_colors[1]),
        [0, 34, 88],
        "sub-11 gradient slope"
    );
    assert!(
        (atmo.cloud_density - 0.85).abs() < 1e-6,
        "Blasted Lands authors C = 0.85 (Elwynn's is 0.50): near-total overcast"
    );
}

#[test]
fn an_authored_black_and_a_keyless_row_agree_which_is_why_the_data_reads_as_it_does() {
    let Some(elwynn) = params(PARAMS_ELWYNN) else {
        return;
    };

    // 209 of the 308 keyed cloud-base rows are (0,0,0), and 288 have every channel under 64.
    assert_eq!(
        elwynn.cloud_colors[2], ZERO_KEY_COLOR,
        "LightParams 12 authors its cloud base as black"
    );
    let rgb = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as i32);
    assert_eq!(
        rgb(elwynn.cloud_colors[1]),
        [43, 105, 132],
        "sub-11 slope, authored"
    );
    assert_eq!(rgb(elwynn.sky[0]), [0, 31, 73], "SkyColor0, authored");
    assert_eq!(
        rgb(elwynn.fog_color),
        [77, 120, 143],
        "fog row is authored, unchanged"
    );
}

#[test]
fn a_keyless_scalar_row_is_zero_not_a_thousand_yards() {
    // LightParams 95, a clear-underwater slot on map 269, has keyless fog-end and fog-start rows.
    let Some(atmo) = params(95) else { return };
    assert_eq!(
        atmo.fog_end, ZERO_KEY_SCALAR,
        "keyless fog-end commits +0.0"
    );
    assert_eq!(atmo.fog_start_frac, ZERO_KEY_SCALAR);
    assert_eq!(atmo.fog_color, ZERO_KEY_COLOR, "its fog row is keyless too");
}

#[test]
fn a_params_id_above_the_id_gaps_still_reads_its_own_bands() {
    // `LightParams` ids run 1..499 over 426 records, and a band-row id derives from the params id,
    // `(p-1)*18 + b + 1`, not its row position. 499 is the last id.
    let Some(atmo) = params(499) else { return };
    let rgb = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as i32);
    assert_eq!(rgb(atmo.ambient), [75, 97, 124], "sub-1");
    assert_eq!(rgb(atmo.sun_diffuse), [255, 148, 0], "sub-0");
    assert_eq!(rgb(atmo.fog_color), [107, 114, 136], "sub-7");
    assert_eq!(rgb(atmo.sky[0]), [54, 56, 73], "sub-2");
    assert_eq!(rgb(atmo.sky[4]), [117, 124, 149], "sub-6");
    assert_eq!(atmo.cloud_colors[2], ZERO_KEY_COLOR, "sub-12 keyless");
    assert!(
        (atmo.fog_end - 14000.0 / 36.0).abs() < 1e-3,
        "fsub-0 authored"
    );
    assert!(
        (atmo.fog_start_frac + 0.2).abs() < 1e-6,
        "fsub-1 authored, negative"
    );
}
