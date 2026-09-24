//! Difftest the **billboard-card lit-face** shape against real content: which
//! billboard batches are authored back-to-front against the law's `+X`-at-the-viewer, and — the
//! load-bearing half — which must NOT be touched.
//!
//! `RenderSubmesh::billboard_card_faces_away` decides whether a consumer turns a card's normals
//! round so it is lit off the face it presents. Get it wrong in the permissive direction and 3-D
//! billboard geometry loses its shading; get it wrong in the strict direction and the cards go on
//! swinging warm/cool with the camera. Both sides are pinned here on the shipped assets. Skips
//! when the client isn't present.

use benilla_formats::{load_m2_mesh, open_chain, BillboardKind};

/// The hanging shop sign — the report this shape came from. Its two "chains" are not chain geometry
/// at all but a pair of 4-vert lock-Z billboard cards on a tiled chain texture, each authored with
/// its whole plane's normal on the −X side, i.e. facing away from the viewer the law points them at.
/// The sign body is ordinary geometry and must stay untouched.
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

/// The negative that matters: the questgiver `?` marker is a lock-Z billboard too, but its geometry
/// is 3-D (hundreds of verts, normals pointing every way round it). A rule that flipped every −X
/// normal on a billboard batch would gut its shading, so the shape demands ONE plane.
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

/// The shape's remaining arms, on synthetic normals (no assets needed): an EDGE-ON card has no
/// facing to correct, the already-correct majority is untouched, degenerate normals never flip,
/// and a non-billboard batch is out of scope however it is normalled.
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
    // Soft/averaged authoring: unnormalised and a hair off plane — still one card.
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
