//! M2 cameras: `0x7c` bytes a record (built at `0x70f270`); the unit-frame portrait renders
//! through `cameraLookup[0]` (`0x713540`).

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
        // Models face +X, so a portrait camera sits +X of its subject, looking back.
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
    // HumanMale's authored rig: fov π/4, the eye at head height, in front and to the model's
    // right (so the portrait faces viewer-left), the target on the head's centre.
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
    assert!(parse_m2_portrait_camera(&[0u8; 16]).is_none());
}

/// A `<PlayerModel>` pane renders through raw `cameras[1]`: the chooser `0x505890`, from
/// `0x505b30`, takes index 1, not `cameraLookup`. Every record's clips are 8/36 and 1000/36.
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

    // The standoff is authored per model, never normalized: a gnome's ~2.2 yd, a boar's ~4.9.
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
