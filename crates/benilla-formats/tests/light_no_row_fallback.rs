//! **A map with no `Light.dbc` row of its own falls back to record ID 1, not to an invention.**
//!
//! `Light.dbc` carries rows for 26 map ids; **Deeprun Tram (369) has none** — not even the
//! falloff-0 `(0,0,0)` global that maps 0/1 carry. The client handles that in
//! `dn_light_array_build 0x6d6170`: the per-map filter (`0x6d61a9 cmp [row+4], mapId`) matches
//! nothing, and the tail (`0x6d62b2`–`0x6d62c9`) writes `idMap[1]` — the record whose **ID column**
//! is 1, not row-index 1 — into slot 0. `dn_light_select 0x6d2d00` then no-ops (count ≤ 1 ⇒ empty
//! blend heap) and the colour table commits that record whole.
//!
//! Row 1 is the Azeroth global → LightParams **12**, whose bands are an ordinary six-key day curve.
//! Before this was wired, benilla substituted a hardcoded bright noon (fog `[140,183,234]` at
//! 1000 yd) and the Tram's undersea stretch rendered as a daylight scene against the reference's
//! deep blue. The numbers below are read straight off the shipped DBC — LightParams 12 → int bands
//! 199..216 (sub-0 diffuse, sub-1 ambient, sub-7 fog) and float bands 67..72 (sub-0 fog end in
//! inches, sub-1 start fraction) — so they pin the *identity of the record*, which is the part that
//! was wrong.

use benilla_formats::{Chain, LightCatalog, Submersion};

/// `Map.dbc` id of Deeprun Tram — the WMO-only map with no `Light.dbc` row.
const MAP_DEEPRUN_TRAM: u32 = 369;
/// Half-minutes; 1440 = noon.
const NOON: u32 = 1440;

#[test]
fn deeprun_tram_takes_light_record_1_not_an_invented_default() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = Chain::open(&data).expect("open vanilla patch chain");
    let cat = LightCatalog::load(&mut chain).expect("load Light/LightParams/*Band");

    // The Tram's own position is irrelevant — nothing covers it, which is the whole point.
    let tram = cat.sample_blended(
        MAP_DEEPRUN_TRAM,
        [25.0, -1256.0, -117.0],
        NOON,
        false,
        Submersion::Dry,
        false,
    );

    // LightParams 12 at noon, off the shipped bands.
    let rgb = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as i32);
    assert_eq!(
        rgb(tram.fog_color),
        [77, 120, 143],
        "fog must be LightParams 12's own band, not the old [140,183,234] invention"
    );
    assert_eq!(rgb(tram.ambient), [104, 130, 154], "ambient = band sub-1");
    assert_eq!(rgb(tram.sun_diffuse), [255, 136, 0], "diffuse = band sub-0");
    assert!(
        (tram.fog_end - 500.0).abs() < 1.0,
        "fog end {} should be 18000 inches / 36 = 500 yd (was 1000)",
        tram.fog_end
    );
    assert!(
        (tram.fog_start_frac - 0.25).abs() < 1e-3,
        "start fraction {} should be 0.25 (was 0.40)",
        tram.fog_start_frac
    );

    // The fallback is a RECORD LOOKUP, not a per-map constant: every map with no row of its own
    // lands on the same record, regardless of id or sample position. (Deliberately not compared
    // against map 0 sampled at its origin — local Azeroth spheres cover that point and blend over
    // the global, so it resolves [62,96,102], not the bare record.)
    let other_rowless = cat.sample_blended(
        4242,
        [9000.0, -1000.0, 60.0],
        NOON,
        false,
        Submersion::Dry,
        false,
    );
    assert_eq!(
        rgb(other_rowless.fog_color),
        rgb(tram.fog_color),
        "any map with no Light.dbc row takes the same record 1"
    );

    // And it is a real day curve, not a frozen constant: midnight differs from noon.
    let midnight = cat.sample_blended(
        MAP_DEEPRUN_TRAM,
        [25.0, -1256.0, -117.0],
        0,
        false,
        Submersion::Dry,
        false,
    );
    assert_ne!(
        rgb(midnight.fog_color),
        rgb(tram.fog_color),
        "the fallback runs the ordinary day/night pipeline, so the hours must differ"
    );
}

/// The scoping guard for the fix: the seven shipped maps that carry positioned rows but **no**
/// `(0,0,0)` global are a *different* case, and one the byte finding does not settle (the reference
/// seeds the array count at 1 and fills slots 1.. with the positioned rows, leaving slot 0
/// unwritten). They must not be swept into the record-1 fallback on the strength of a guess.
#[test]
fn maps_with_positioned_rows_but_no_global_are_left_alone() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = Chain::open(&data).expect("open vanilla patch chain");
    let cat = LightCatalog::load(&mut chain).expect("load Light/LightParams/*Band");

    // Map 169 (Emerald Dream) carries 4 rows, none of them a global. Sampled far from all of them,
    // it must NOT come back as record 1's atmosphere.
    let far = cat.sample_blended(169, [0.0, 0.0, 0.0], NOON, false, Submersion::Dry, false);
    let tram = cat.sample_blended(
        MAP_DEEPRUN_TRAM,
        [25.0, -1256.0, -117.0],
        NOON,
        false,
        Submersion::Dry,
        false,
    );
    let rgb = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as i32);
    assert_ne!(
        rgb(far.fog_color),
        rgb(tram.fog_color),
        "a map WITH rows must not inherit the zero-match fallback"
    );
}
