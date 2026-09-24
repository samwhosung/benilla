//! A map with no `Light.dbc` row falls back to the record whose id is 1. For Deeprun Tram (369)
//! the per-map filter of `0x6d6170` matches nothing (`0x6d61a9`), the tail writes `idMap[1]` into
//! slot 0 (`0x6d62b2`-`0x6d62c9`), `0x6d2d00` has nothing to blend, and that record, the Azeroth
//! global with `LightParams` 12 (int bands 199..216, float bands 67..72), commits whole.

use benilla_formats::{Chain, LightCatalog, Submersion};

const MAP_DEEPRUN_TRAM: u32 = 369;
/// Half-minutes; 1440 = noon.
const NOON: u32 = 1440;

#[test]
fn deeprun_tram_takes_light_record_1_not_an_invented_default() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = Chain::open(&data).expect("open vanilla patch chain");
    let cat = LightCatalog::load(&mut chain).expect("load Light/LightParams/*Band");

    // The position is irrelevant: no row covers any point of the map.
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

    // Every map with no row lands on the same record, whatever its id or position.
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

/// Seven maps have positioned rows but no `(0,0,0)` global: the reference fills slots 1.. and
/// leaves slot 0 unwritten, an effect not traced, so they stay out of the record-1 fallback.
#[test]
fn maps_with_positioned_rows_but_no_global_are_left_alone() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = Chain::open(&data).expect("open vanilla patch chain");
    let cat = LightCatalog::load(&mut chain).expect("load Light/LightParams/*Band");

    // Map 169 (Emerald Dream): 4 rows, none a global, sampled far from all of them.
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
