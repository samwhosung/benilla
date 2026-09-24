//! `RenderSubmesh::section`: two batches naming one M2 skin section draw the same triangles, so
//! they are coplanar and `terrain_stream::spawn::assemble` keeps them on one vertex-transform lane.
//! The Ballista's bolt head and shields each add an `ARMORREFLECT3` shine that is additive,
//! env-mapped and render flag `0x10`, each a consolidator exclusion.

use benilla_formats::{load_m2_mesh, open_chain, RenderSubmesh};

/// What the consolidators exclude: `static_gx::divert` refuses env-mapped and depth-flagged
/// batches, the merge lane additive ones.
fn refusable(s: &RenderSubmesh) -> bool {
    s.env_map || s.no_depth_write || s.no_depth_test || s.additive
}

#[test]
fn the_ballista_authors_a_coplanar_shine_over_its_bolt_head_and_shields() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Ballista\\Ballista.m2",
    )
    .expect("load Ballista");

    assert!(
        subs.iter().all(|s| s.section.is_some()),
        "every M2 batch carries its skin-section index"
    );

    let mut shared = 0;
    for section in [7u16, 10] {
        let batches: Vec<&RenderSubmesh> =
            subs.iter().filter(|s| s.section == Some(section)).collect();
        assert_eq!(
            batches.len(),
            2,
            "Ballista section {section} is drawn by a base batch and a shine batch"
        );
        assert_eq!(
            batches.iter().filter(|s| refusable(s)).count(),
            1,
            "exactly one of section {section}'s two batches is a consolidator exclusion — that \
             asymmetry is the defect the assemble gate closes"
        );
        let shine = batches.iter().find(|s| refusable(s)).unwrap();
        assert!(shine.env_map, "the shine batch generates its texcoords");
        assert!(shine.no_depth_write, "…carries render flag 0x10");
        assert!(shine.additive, "…and blends additively (blend mode 4)");
        shared += 1;
    }
    assert_eq!(shared, 2);
}

#[test]
fn a_plain_doodad_draws_every_section_once() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "World\\Generic\\Buildings\\HumanTentMedium\\HumanTentMedium.m2",
    )
    .expect("load HumanTentMedium");

    let mut seen: Vec<Option<u16>> = subs.iter().map(|s| s.section).collect();
    let before = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen.len(),
        before,
        "the tent draws each of its sections exactly once — nothing here is coplanar with itself"
    );
}
