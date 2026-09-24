//! `RenderSubmesh::ground_quad`: the flat quads the ground-fx lane re-renders as projected decals.
//! Battle Shout's cast base authors six 4-vertex quads at z = 0, each fully weighted to one bone.

use benilla_formats::{open_chain, parse_m2_render_submeshes};

#[test]
fn battle_shout_crescents_detect_as_ground_quads() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    let bytes = chain
        .read_file("Spells\\BattleShout_Cast_Base.m2")
        .expect("Battle Shout cast-base model");
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("submeshes");
    assert_eq!(subs.len(), 6, "six crescent batches");
    let mut bones = Vec::new();
    for sub in &subs {
        let quad = sub
            .ground_quad()
            .expect("every crescent batch is a ground quad");
        bones.push(quad.bone);
        // The authored rect, read off the raw M2 vertex table.
        assert!((quad.corners[0][0] + 0.776).abs() < 1e-3, "min x");
        assert!((quad.corners[3][0] - 0.212).abs() < 1e-3, "max x");
        assert!((quad.corners[0][1] + 0.494).abs() < 1e-3, "min y");
        assert!((quad.corners[3][1] - 0.494).abs() < 1e-3, "max y");
        assert!(quad.corners.iter().all(|c| c[2] == 0.0), "authored z = 0");
    }
    bones.sort_unstable();
    assert_eq!(bones, [1, 2, 3, 5, 6, 7], "one quad per slide bone");
}

/// A disc authored just above the ground, under `GROUND_HOVER_MAX`, detects with its hover kept:
/// Consecration's burn disc at z = 0.097, its glow at 0.207. Of Flamestrike's batches only the two
/// flat discs detect.
#[test]
fn hovering_discs_detect_as_ground_quads() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    let bytes = chain
        .read_file("spells\\consecration_impact_base.m2")
        .expect("Consecration impact-base model");
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("submeshes");
    assert_eq!(subs.len(), 2, "glow + burn disc");
    let mut hovers: Vec<f32> = subs
        .iter()
        .map(|s| {
            let quad = s.ground_quad().expect("both Consecration discs detect");
            let z = quad.corners[0][2];
            assert!(
                quad.corners.iter().all(|c| c[2] == z),
                "uniform authored plane"
            );
            z
        })
        .collect();
    hovers.sort_by(f32::total_cmp);
    assert!((hovers[0] - 0.097).abs() < 1e-3, "burn disc hover");
    assert!((hovers[1] - 0.207).abs() < 1e-3, "center glow hover");

    let bytes = chain
        .read_file("Spells\\Flamestrike_Impact_Base.m2")
        .expect("Flamestrike impact-base model");
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("submeshes");
    let quads = subs.iter().filter(|s| s.ground_quad().is_some()).count();
    assert_eq!(
        quads, 2,
        "exactly the burn disc + center glow — flames/ribbons/billboard stay meshes"
    );
}

#[test]
fn character_model_detects_no_ground_quads() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");
    let bytes = chain
        .read_file("Character\\Human\\Male\\HumanMale.m2")
        .expect("human male model");
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("submeshes");
    assert!(
        subs.iter().all(|s| s.ground_quad().is_none()),
        "no body batch reads as a ground quad"
    );
}

/// `GroundQuad::tint` carries a static M2Color, which a decal has no vertex buffer for: the Flare's
/// two 13.89-yd washes on neutral `GENERICGLOW*` radials are all `(0.992, 0.467, 0.0)`. An animated
/// colour rides `rgb_anim` and the tint stays white, so the two never double-apply.
#[test]
fn ground_quads_carry_their_static_m2color_tint() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    let bytes = chain
        .read_file("SPELLS\\Flare_State_Base.m2")
        .expect("Flare state-base model");
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("submeshes");
    let quads: Vec<_> = subs.iter().filter_map(|s| s.ground_quad()).collect();
    assert_eq!(quads.len(), 2, "both washes are ground quads");
    for q in &quads {
        assert!((q.tint[0] - 0.992).abs() < 1e-3, "warm red: {:?}", q.tint);
        assert!((q.tint[1] - 0.467).abs() < 1e-3, "warm green: {:?}", q.tint);
        assert!(q.tint[2] < 1e-3, "no blue at all: {:?}", q.tint);
        let span = q.corners[3][0] - q.corners[0][0];
        assert!((span - 13.89).abs() < 0.02, "wash span {span}");
    }

    let bytes = chain
        .read_file("Spells\\BattleShout_Cast_Base.m2")
        .expect("Battle Shout cast-base model");
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("submeshes");
    for sub in &subs {
        let q = sub.ground_quad().expect("crescent");
        assert!(sub.rgb_anim.is_some(), "the crescent's colour is a loop");
        assert_eq!(q.tint, [1.0; 3], "…so the static tint stays white");
    }
}
