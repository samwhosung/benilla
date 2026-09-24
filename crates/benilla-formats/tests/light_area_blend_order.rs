//! **The water swatch rides the area blend, and the blend applies spheres farthest-first.**
//!
//! Two halves of one law (`0x6d2d00`):
//!
//! 1. `dn_light_select 0x6d2d00` pushes every `Light.dbc` row with `dist ≤ outer` into a **max-heap
//!    keyed on DISTANCE** and drains it root-first — so the farthest light merges first and the
//!    **nearest lands last and dominates**. A point inside the tightest sphere's inner radius
//!    therefore resolves that sphere's `LightParams` **exactly**, whatever else overlaps it.
//! 2. `0x6d30e0` merges **all 18 colour slots** of the gather record per light —
//!    its step-9 loop `+0x34..+0x40` is precisely IntBand rows 14–17, the ocean/river swatch. The
//!    water tint is an ordinary band; the client has no single-sphere pick anywhere in that path.
//!
//! Both were wrong before decision 1104, and the two errors hid each other. Ordering by blend
//! *weight* let a wide distant sphere land last and dilute a zone that fully contained the camera —
//! which made the blended water read muddy, which is what the `pick_light` water split was
//! introduced to dodge. The split then made the tint **discontinuous**: `pick_light` switches at a
//! tighter sphere's *outer* radius, exactly where that sphere's own weight is still zero, so the
//! Tirisfal→Silverpine border snapped green water to near-black brown in a single step (the
//! director's report) while ambient, sun and fog crossed it without a flicker.

use benilla_formats::{Chain, LightCatalog, Submersion};

/// Eastern Kingdoms.
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

/// The director's two `.go` pins, 16 yd apart across the Tirisfal → Silverpine border. Light 4
/// (falloff 985→1437 yd, `LightParams` 40) *enters* between them — at pin B the eye is 1434 yd out,
/// three yards inside its outer radius, where its blend weight is 0.006. The area blend must
/// therefore barely move; the old `pick_light` swatch instead switched wholesale to LP 40's rows.
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

    // The exact swatch on the Tirisfal side, so a future re-derivation moves this on purpose.
    assert_eq!(
        rgb(a.water_river[0]),
        [82, 93, 46],
        "river shallow at pin A"
    );
    assert_eq!(rgb(a.water_river[1]), [60, 88, 89], "river deep at pin A");
    // The `pick_light` answers the old code committed — the snap, in both its endpoints.
    assert_ne!(
        rgb(b.water_river[1]),
        [35, 28, 37],
        "pin B must not commit LightParams 40's raw deep row through a single-sphere pick"
    );
}

/// Inside the tightest sphere's inner radius the nearest light merges **last** at weight 1, which is
/// a full replace — so the blend equals that sphere's `LightParams` outright, even with two wider
/// spheres also covering the point (here Light 31 / `LightParams` 46 reaches it at weight 0.195).
/// Sorting the merge by weight instead put that 0.195 last and smeared 20% of a distant zone's
/// palette over a zone the camera stands in the middle of.
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
    // LightParams 40's own rows, off the shipped bands — the identity, not just the agreement.
    assert_eq!(rgb(blended.ambient), [80, 63, 79]);
    assert_eq!(rgb(blended.water_river[0]), [82, 64, 49]);
    assert_eq!(rgb(blended.water_river[1]), [35, 28, 37]);
}

/// The Stranglethorn anchor the `pick_light` split was built on (apitrace WoW.21: the river swatch
/// reads `LightParams` 26 exactly, alpha 216/255). It never needed a pick — Light 9 covers the river
/// at full weight, so the faithful nearest-last blend commits LP 26 whole.
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
