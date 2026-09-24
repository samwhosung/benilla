//! `RenderSubmesh::billboard_card_faces_away`: a flat billboard card whose normals sit on the −X
//! side, away from the viewer its bone turns `+X` to, is lit off the face it presents; 3-D
//! billboard geometry keeps its normals.

use benilla_formats::{load_m2_mesh, open_chain, BillboardKind};

/// The shop sign's two chains are 4-vertex lock-Z billboard cards on a tiled chain texture, each
/// plane's normal on the −X side; the sign body is ordinary geometry.
#[test]
fn the_shop_signs_chain_cards_face_away_and_its_body_is_untouched() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "World\\Generic\\Human\\Passive Doodads\\Signs\\CheeseShop01.m2",
    )
    .expect("load CheeseShop01");

    let cards: Vec<_> = subs.iter().filter(|s| s.billboard.is_some()).collect();
    assert_eq!(cards.len(), 2, "the sign hangs on two billboard cards");
    for c in &cards {
        let bb = c.billboard.as_ref().unwrap();
        assert_eq!(
            bb.kind,
            BillboardKind::LockZ,
            "a hanging chain card spins about model up (bone flag 0x40)"
        );
        assert_eq!(c.positions.len(), 4, "a card is one quad");
        assert!(
            c.two_sided,
            "the card is two-sided (material 0x04), so the reference draws the back we see"
        );
        assert!(
            c.billboard_card_faces_away(),
            "the card's plane normal is authored on the −X side: lit off the face it presents"
        );
    }
    assert!(
        subs.iter()
            .any(|s| s.billboard.is_none() && !s.billboard_card_faces_away()),
        "the sign body is not a billboard and is never re-normalled"
    );
}

/// The questgiver `?` marker is a lock-Z billboard too, but 3-D, its normals pointing every way:
/// only a single plane counts as a card.
#[test]
fn the_questgiver_marker_is_3d_billboard_geometry_not_a_card() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(&mut chain, "Interface\\Buttons\\TalkToMeQuestionMark.m2")
        .expect("load TalkToMeQuestionMark");

    let billboards: Vec<_> = subs.iter().filter(|s| s.billboard.is_some()).collect();
    assert!(
        !billboards.is_empty(),
        "the marker rides a billboard bone (it faces the camera)"
    );
    for b in &billboards {
        assert!(
            b.positions.len() > 4,
            "3-D geometry, not a quad — got {} verts",
            b.positions.len()
        );
        assert!(
            !b.billboard_card_faces_away(),
            "3-D billboard geometry is not a flat card: its normals must be left alone"
        );
    }
}

/// The rule's other arms, on synthetic normals.
#[test]
fn the_shape_ignores_edge_on_correct_and_degenerate_batches() {
    use benilla_formats::{Billboard, ModelBlend, RenderSubmesh};

    let card = |normals: Vec<[f32; 3]>, billboard: bool| RenderSubmesh {
        positions: vec![[0.0; 3]; normals.len()],
        normals,
        uvs: vec![[0.0; 2]; 0],
        indices: vec![],
        texture: None,
        skin_slot: None,
        geoset_id: 0,
        char_slot: None,
        blend: ModelBlend::Opaque,
        wrap_x: true,
        wrap_y: true,
        two_sided: true,
        joints: vec![],
        weights: vec![],
        vertex_colors: vec![],
        interior: false,
        emissive: false,
        icon_slot: false,
        uv_rot_seq: None,
        uv_scale_seq: None,
        sidn: None,
        window: false,
        additive: false,
        no_depth_write: false,
        no_depth_test: false,
        fog_policy: benilla_formats::FogPolicy::Scene,
        billboard: billboard.then(|| Billboard {
            bone: 0,
            pivot: [0.0; 3],
            kind: BillboardKind::LockZ,
            scale_anim: None,
            seq_translations: vec![],
        }),
        welded_billboard: false,
        alpha_anim: None,
        uv_anim: None,
        uv_seq: None,
        rgb_anim: None,
        rgb_seq: None,
        wmo_batch: None,
        env_map: false,
        section: None,
    };

    assert!(
        card(vec![[-1.0, 0.0, 0.0]; 4], true).billboard_card_faces_away(),
        "the away-facing card (the control) flips"
    );
    assert!(
        card(vec![[-0.98, 0.01, -0.02], [-1.02, -0.01, 0.01]], true).billboard_card_faces_away(),
        "a soft-normal card is still one plane"
    );
    assert!(
        !card(vec![[1.0, 0.0, 0.0]; 4], true).billboard_card_faces_away(),
        "the majority already presents its lit face — untouched"
    );
    assert!(
        !card(vec![[0.0, 0.0, -1.0]; 4], true).billboard_card_faces_away(),
        "edge-on to the camera axis: no facing to correct"
    );
    assert!(
        !card(vec![[0.0; 3]; 2], true).billboard_card_faces_away(),
        "degenerate zero normals never flip"
    );
    assert!(
        !card(vec![], true).billboard_card_faces_away(),
        "no normals, no rule"
    );
    assert!(
        !card(vec![[-1.0, 0.0, 0.0]; 4], false).billboard_card_faces_away(),
        "an ordinary batch is out of scope however it is normalled"
    );
}
