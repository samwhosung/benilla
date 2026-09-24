//! The area blend applies spheres farthest-first and carries the water swatch. `0x6d2d00` heaps
//! every `Light.dbc` row within its outer radius by distance and drains the farthest first, so the
//! nearest merges last and dominates. `0x6d30e0` merges all 18 colour slots per light, the water
//! swatch among them (IntBand rows 14-17, its step-9 loop `+0x34..+0x40`): no single-sphere pick.

use benilla_formats::{Chain, LightCatalog, Submersion};

const MAP_EK: u32 = 0;
/// Half-minutes; 1440 = noon.
const NOON: u32 = 1440;

fn rgb(c: [f32; 3]) -> [i32; 3] {
    c.map(|v| (v * 255.0).round() as i32)
}

fn catalog() -> Option<LightCatalog> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = Chain::open(&data).expect("open vanilla patch chain");
    Some(LightCatalog::load(&mut chain).expect("load Light/LightParams/*Band"))
}

fn blend(cat: &LightCatalog, pos: [f32; 3]) -> benilla_formats::Atmosphere {
    cat.sample_blended(MAP_EK, pos, NOON, false, Submersion::Dry, false)
}

/// Two points 16 yd apart across the Tirisfal-Silverpine border. Light 4 (falloff 985-1437 yd,
/// `LightParams` 40) enters between them: at the second the eye is 1434 yd out, weight 0.006.
#[test]
fn water_swatch_does_not_snap_at_the_tirisfal_silverpine_border() {
    let Some(cat) = catalog() else { return };
    let a = blend(&cat, [1391.07, 641.39, 35.37]);
    let b = blend(&cat, [1401.03, 628.90, 35.42]);

    for (label, x, y) in [
        ("river shallow", a.water_river[0], b.water_river[0]),
        ("river deep", a.water_river[1], b.water_river[1]),
        ("ocean shallow", a.water_ocean[0], b.water_ocean[0]),
        ("ocean deep", a.water_ocean[1], b.water_ocean[1]),
    ] {
        let (x, y) = (rgb(x), rgb(y));
        let step = (0..3).map(|i| (x[i] - y[i]).abs()).max().unwrap_or(0);
        assert!(
            step <= 2,
            "{label} must cross the border continuously: {x:?} -> {y:?} (max channel step {step})"
        );
    }

    assert_eq!(
        rgb(a.water_river[0]),
        [82, 93, 46],
        "river shallow at pin A"
    );
    assert_eq!(rgb(a.water_river[1]), [60, 88, 89], "river deep at pin A");
    assert_ne!(
        rgb(b.water_river[1]),
        [35, 28, 37],
        "pin B must not commit LightParams 40's raw deep row through a single-sphere pick"
    );
}

/// Inside the nearest sphere's inner radius it merges last at weight 1, a full replace, though two
/// wider spheres also reach the point (Light 31, `LightParams` 46, at weight 0.195).
#[test]
fn the_nearest_sphere_dominates_where_it_is_at_full_weight() {
    let Some(cat) = catalog() else { return };
    let pos = [2250.0, -750.0, 50.0];
    let blended = blend(&cat, pos);
    let picked = cat.sample(MAP_EK, pos, NOON, false);

    assert_eq!(rgb(blended.ambient), rgb(picked.ambient), "ambient");
    assert_eq!(rgb(blended.sun_diffuse), rgb(picked.sun_diffuse), "diffuse");
    assert_eq!(rgb(blended.fog_color), rgb(picked.fog_color), "fog");
    assert_eq!(
        rgb(blended.water_river[0]),
        rgb(picked.water_river[0]),
        "river shallow"
    );
    // LightParams 40's own rows, off the shipped bands.
    assert_eq!(rgb(blended.ambient), [80, 63, 79]);
    assert_eq!(rgb(blended.water_river[0]), [82, 64, 49]);
    assert_eq!(rgb(blended.water_river[1]), [35, 28, 37]);
}

/// A reference frame capture reads Stranglethorn's river swatch as `LightParams` 26 exactly, alpha
/// 216/255: Light 9 covers the river at full weight, so the blend commits it whole.
#[test]
fn the_stranglethorn_river_swatch_is_lightparams_26() {
    let Some(cat) = catalog() else { return };
    let stv = blend(&cat, [-13333.3, 0.0, 50.0]);

    assert_eq!(rgb(stv.water_river[0]), [90, 140, 140], "LP 26 IntBand 16");
    assert_eq!(rgb(stv.water_river[1]), [28, 55, 64], "LP 26 IntBand 17");
    assert_eq!(rgb(stv.water_ocean[0]), [90, 171, 140], "LP 26 IntBand 14");
    assert_eq!(rgb(stv.water_ocean[1]), [28, 44, 44], "LP 26 IntBand 15");
    assert!(
        (stv.water_river_alpha[0] - 216.0 / 255.0).abs() < 0.01,
        "the trace's shallow swatch alpha, got {}",
        stv.water_river_alpha[0]
    );
}
