//! Generated (environment-map) texcoords: `texCoordSet` (`+0x12`) indexes `texture_unit_lookup`
//! (`0x9c`), and the reference takes a value above 2 as generated (`0x70b8bd`). The Deeprun Tram's
//! glass authors `[-1]` and leaves all 330 vertices' UVs at (0, 0).

use benilla_formats::{load_m2_mesh, open_chain};

#[test]
fn tram_glass_batches_generate_their_texcoords() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "World\\Generic\\Gnome\\Passive Doodads\\GnomeMachine\\GnomeSubwayGlass.m2",
    )
    .expect("load GnomeSubwayGlass");

    assert!(!subs.is_empty(), "the glass tube has render batches");
    assert!(
        subs.iter().all(|s| s.env_map),
        "every GnomeSubwayGlass batch authors texture_unit_lookup = -1 (generated texcoords)"
    );
    for s in &subs {
        assert!(
            s.uvs.iter().all(|uv| uv[0] == 0.0 && uv[1] == 0.0),
            "an env-mapped batch's authored UVs are degenerate — the runtime supplies them"
        );
    }
}

#[test]
fn weapon_rack_splits_env_from_uv_batches() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "World\\Generic\\Human\\Passive Doodads\\WeaponRacks\\GeneralWeaponrack01.m2",
    )
    .expect("load GeneralWeaponrack01");

    // `texture_unit_lookup = [0, -1]`: the body reads UV channel 0, the sheen layers generate.
    let env: Vec<&str> = subs
        .iter()
        .filter(|s| s.env_map)
        .filter_map(|s| s.texture.as_deref())
        .collect();
    assert!(
        !env.is_empty()
            && env
                .iter()
                .all(|t| t.to_ascii_lowercase().contains("armorreflect")),
        "only the ARMORREFLECT sheen layers env-map on the rack, got {env:?}"
    );
    assert!(
        subs.iter().any(|s| !s.env_map),
        "the rack's body and blade batches read their authored UVs"
    );
}
