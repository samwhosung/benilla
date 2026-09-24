//! **A band row with no keyframes commits the reference's constant — black, or `+0.0` — never an
//! invented daytime value.**
//!
//! 973 of the 7668 shipped `LightIntBand` rows and 132 of the 2556 `LightFloatBand` rows carry
//! `numKeys == 0`. The colour evaluator `0x6d62e0` early-outs before touching the key arrays
//! (count load `0x6d62ec`, guard `0x6d62ef`/`0x6d62f4`, store `0x6d62f6 mov [edi], 0xff000000`) and
//! the scalar one does the same at `0x6d6489`/`0x6d648e` with `fld [0x7ffd74]` = `+0.0f`; both
//! stores are immediates, and the copy into the colour table is unconditional — so the answer is
//! not stale, not skipped, and not a static initialiser (decision 1465).
//!
//! benilla used to answer every one of those rows out of `Atmosphere::DEFAULT`, a hand-written
//! "neutral daytime" palette meant for *no lighting data at all*. Report **B90** is what that cost:
//! `LightParams` 36 (Blasted Lands / the Tainted Scar) leaves the cloud gradient base unkeyed, so
//! the cloud layer took a pale grey `[0.75, 0.78, 0.82]` where the data says black — and the zone
//! authors cloud density 0.85, so that layer *is* the sky there. The reporter saw pure white.

use benilla_formats::{Chain, LightCatalog, ZERO_KEY_COLOR, ZERO_KEY_SCALAR};

/// Half-minutes; 1440 = noon.
const NOON: u32 = 1440;
/// Blasted Lands' clear-weather `LightParams` — the B90 record. Its `LightIntBand` sub-12
/// (cloud gradient base) is id `(36-1)*18 + 13 = 643`, `numKeys = 0`.
const PARAMS_BLASTED_LANDS: u32 = 36;
/// Elwynn / the Azeroth global — the control. Its sub-12 is authored, and authored *black*.
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

    // The defect: this used to be `[0.75, 0.78, 0.82]`.
    assert_eq!(
        atmo.cloud_colors[2], ZERO_KEY_COLOR,
        "sub-12 is keyless for LightParams 36 — the cloud gradient base must be black"
    );
    // Its two neighbours ARE authored, and must be untouched by the change: this is what proves the
    // fix reads the row rather than blanking the palette.
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
    // And the density that makes this zone show the defect at all.
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

    // Elwynn's sub-12 has keys — and they are (0,0,0). 209 of the 308 keyed gbase rows are exactly
    // that, 288 have max channel < 64, and NOT ONE is pale. A pale fallback was never plausible.
    assert_eq!(
        elwynn.cloud_colors[2], ZERO_KEY_COLOR,
        "LightParams 12 authors its cloud base as black"
    );
    // The control that must not move: Elwynn's authored slope/sun.
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
    // LightParams 95 is a clear-underwater slot on map 269 whose fog-end AND fog-start rows are
    // both keyless. The old `.filter(|v| v > 1.0)` turned that into 1000 yd of daylight fog.
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
    // `LightParams` ids run 1..499 across 426 records — 73 gaps. The band-row id is derived from the
    // params **ID** (`(p-1)*18 + b + 1`), so keying that lookup by row *position* would hand every
    // group past the first gap another zone's bands, and the zero-key arm would then be firing on
    // the wrong record entirely. 499 is the very last id; these are its own authored rows.
    let Some(atmo) = params(499) else { return };
    let rgb = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as i32);
    assert_eq!(rgb(atmo.ambient), [75, 97, 124], "sub-1");
    assert_eq!(rgb(atmo.sun_diffuse), [255, 148, 0], "sub-0");
    assert_eq!(rgb(atmo.fog_color), [107, 114, 136], "sub-7");
    assert_eq!(rgb(atmo.sky[0]), [54, 56, 73], "sub-2");
    assert_eq!(rgb(atmo.sky[4]), [117, 124, 149], "sub-6");
    // ...and its own keyless rows still answer black, on a record reached through the gaps.
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
