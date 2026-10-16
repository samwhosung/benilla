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

/// `Obsidian_Miningnode_01.m2`, one of the five stock models with a two-texture unit: its rock
/// batch binds `ARMORREFLECT4.BLP` as stage 1, with generated coordinates (`0xffff`, `0x70b8bd`)
/// and the version-0x101 combiner `[1, …]` at stage 1 (`0x70b7cb`), a modulate of colour and alpha.
#[test]
fn obsidian_node_rock_binds_its_reflection_as_stage_one() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "World\\SkillActivated\\TradeskillNodes\\Obsidian_Miningnode_01.m2",
    )
    .expect("load Obsidian_Miningnode_01");
    // The combiner array itself: a combine of 1 is also the past-the-array fallback.
    let bytes = chain
        .read_file("World\\SkillActivated\\TradeskillNodes\\Obsidian_Miningnode_01.m2")
        .expect("read the m2");
    assert_eq!(benilla_m2::parse_texture_combiners(&bytes), vec![1; 5]);
    let two: Vec<_> = subs.iter().filter(|s| s.stage1.is_some()).collect();
    assert_eq!(two.len(), 1, "one two-texture unit");
    let rock = two[0];
    assert!(
        rock.texture
            .as_deref()
            .is_some_and(|t| t.to_ascii_lowercase().ends_with("\\obsidian.blp")),
        "stage 0 is the rock, got {:?}",
        rock.texture
    );
    assert!(!rock.env_map, "stage 0 reads its UVs");
    let st = rock.stage1.as_ref().expect("stage 1");
    assert!(
        st.texture
            .as_deref()
            .is_some_and(|t| t.to_ascii_lowercase().ends_with("\\armorreflect4.blp")),
        "stage 1 is the reflection, got {:?}",
        st.texture
    );
    assert!(st.env_map, "stage 1 generates its coordinates");
    assert_eq!(st.combine, 1, "the 0x101 combiner at stage 1");
}
