//! M2 portrait-camera parse — byte-level check against real character/creature models. Pins the
//! vanilla camera record stride (`0x7c`, built at `0x70f270`) + the `cameraLookup[0]` selection
//! (`0x713540`): the unit-frame portrait renders through exactly this authored camera. Skips when
//! the client isn't present.

use benilla_formats::{parse_m2_portrait_camera, Chain};

#[test]
fn character_and_creature_portrait_cameras_parse_sane() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    for path in [
        "Character\\Human\\Male\\HumanMale.m2",
        "Creature\\Wolf\\Wolf.m2",
        "Creature\\Rabbit\\Rabbit.m2",
    ] {
        let bytes = reader.read(path).expect("read model");
        let cam = parse_m2_portrait_camera(&bytes)
            .unwrap_or_else(|| panic!("{path}: no portrait camera"));
        eprintln!("{path}: {cam:?}");
        // Structural sanity that a garbled stride/offset could not land on: a usable perspective
        // (fov in a plausible authored range, near < far), the camera off the target, and the rig
        // in front of the model (WoW models author facing +X; a portrait camera sits +X of its
        // subject looking back).
        assert!(
            cam.fov > 0.1 && cam.fov < 1.6,
            "{path}: fov {} outside plausible authored range",
            cam.fov
        );
        assert!(
            cam.near_clip > 0.0 && cam.near_clip < cam.far_clip,
            "{path}: bad clip planes {} .. {}",
            cam.near_clip,
            cam.far_clip
        );
        let dx = cam.position[0] - cam.target[0];
        assert!(
            dx > 0.1,
            "{path}: camera not in front of the model (Δx {dx})"
        );
    }
    // Numeric regression pin — HumanMale's authored rig (fov exactly π/4; eye head-height, in
    // front and off to the model's right — why the ref portrait faces viewer-left; target on the
    // head center). A wrong stride or a swapped base/track offset cannot land on all nine.
    let bytes = reader
        .read("Character\\Human\\Male\\HumanMale.m2")
        .expect("read HumanMale.m2");
    let cam = parse_m2_portrait_camera(&bytes).expect("HumanMale portrait camera");
    let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
    assert!(
        close(cam.fov, std::f32::consts::FRAC_PI_4),
        "fov {}",
        cam.fov
    );
    for (got, want) in cam
        .position
        .iter()
        .zip([0.6335, -0.3879, 1.8867])
        .chain(cam.target.iter().zip([0.0627, 0.0343, 1.8636]))
    {
        assert!(close(*got, want), "eye/target drifted: {got} vs {want}");
    }
    assert!(close(cam.roll, 0.0), "roll {}", cam.roll);
}

#[test]
fn too_short_yields_no_camera() {
    // No MD20 camera array header → None, no panic.
    assert!(parse_m2_portrait_camera(&[0u8; 16]).is_none());
}

/// **The model-frame pane camera** — raw `cameras[1]`, the rig a 1.12 `<PlayerModel>` widget renders
/// through (`0x505b30` → the chooser `0x505890` takes a literal index 1, NOT `cameraLookup`;
/// decision 1089).
///
/// Byte-level regression pin against the shipped records, read here independently through our
/// own parser — matching the reference's own read is the cross-check. The universal clips
/// (`near = 8/36`, `far = 1000/36`) come along because a wrong stride would land on neither.
#[test]
fn pane_cameras_match_the_authored_records() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
    for (path, eye, target, fov) in [
        (
            "Character\\Human\\Male\\HumanMale.m2",
            [3.6585_f32, 0.0338, 0.9227],
            [-0.3644_f32, 0.0291, 0.9873],
            0.97991_f32,
        ),
        (
            "Character\\Tauren\\Male\\TaurenMale.m2",
            [4.4317, -0.0213, 1.0861],
            [0.2520, -0.0210, 1.0086],
            0.87991,
        ),
        (
            "Creature\\Boar\\Boar.m2",
            [4.8611, 0.0, 1.8056],
            [-0.1389, 0.0, 0.9722],
            0.76101,
        ),
    ] {
        let bytes = reader.read(path).expect("read model");
        let cam = benilla_formats::parse_m2_camera(&bytes, 1)
            .unwrap_or_else(|| panic!("{path}: no cameras[1]"));
        eprintln!("{path}: {cam:?}");
        assert!(close(cam.fov, fov), "{path}: fov {} vs {fov}", cam.fov);
        assert!(
            close(cam.near_clip, 8.0 / 36.0),
            "{path}: near {}",
            cam.near_clip
        );
        assert!(
            close(cam.far_clip, 1000.0 / 36.0),
            "{path}: far {}",
            cam.far_clip
        );
        for (got, want) in cam
            .position
            .iter()
            .zip(eye)
            .chain(cam.target.iter().zip(target))
        {
            assert!(close(*got, want), "{path}: eye/target {got} vs {want}");
        }
        assert!(close(cam.roll, 0.0), "{path}: roll {}", cam.roll);
    }

    // The standoff is AUTHORED per model, which is the whole point: nothing normalizes a pane, so a
    // gnome's camera sits ~2.2yd out and a boar's ~4.9. If these ever converged, some engine-side
    // fit would have crept back in.
    let x = |path: &str| {
        let bytes = reader.read(path).expect("read model");
        benilla_formats::parse_m2_camera(&bytes, 1)
            .unwrap_or_else(|| panic!("{path}: no cameras[1]"))
            .position[0]
    };
    let gnome = x("Character\\Gnome\\Female\\GnomeFemale.m2");
    let tauren = x("Character\\Tauren\\Male\\TaurenMale.m2");
    assert!(
        gnome < 2.5 && tauren > 4.0,
        "authored standoffs collapsed: gnome {gnome}, tauren {tauren}"
    );
}
