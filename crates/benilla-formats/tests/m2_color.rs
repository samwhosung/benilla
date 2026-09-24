//! Regression: an M2 batch that references an M2Color (`texUnit.colorIndex != 0xffff`) must carry that
//! colour's RGB as a per-vertex tint, so the model shader multiplies it into the texture. Load-bearing
//! for additive glow cards drawn with a neutral glow texture — the Orgrimmar bonfire's base glow
//! (`GenericGlow_Alpha_128`, a white-cored radial) gets its warmth *only* from this M2Color; without it
//! the additive draw washes the bright core to white. See. Skips when the client is absent.

use benilla_formats::{load_m2_mesh, open_chain};

#[test]
fn bonfire_glow_carries_warm_m2color_tint() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "World\\Kalimdor\\Orgrimmar\\PassiveDoodads\\OrgrimmarBonfire\\OrgrimmarBonfire01.m2",
    )
    .expect("load OrgrimmarBonfire01.m2");

    // The additive billboard glow card (GenericGlow) references M2Color[0] = warm fire-orange.
    let glow = subs
        .iter()
        .find(|s| s.additive && s.billboard.is_some())
        .expect("bonfire has an additive billboard glow card");
    assert_eq!(
        glow.vertex_colors.len(),
        glow.positions.len(),
        "the glow card's M2Color must be baked as a per-vertex tint"
    );
    let c = glow.vertex_colors[0];
    // Authored M2Color RGB ≈ (0.925, 0.663, 0.098): warm (R ≫ B), not the neutral/white it'd be without
    // the tint. Asserting the shape (warm) rather than exact bytes keeps this robust.
    assert!(
        (c[0] - 0.925).abs() < 0.02 && (c[1] - 0.663).abs() < 0.02 && (c[2] - 0.098).abs() < 0.02,
        "glow tint should be the authored warm fire-orange, got {c:?}"
    );
    assert!(
        c[0] > c[1] && c[1] > c[2] && c[0] - c[2] > 0.5,
        "glow tint must be distinctly warm (R≫G≫B), got {c:?}"
    );

    // The wood/rock batches don't reference an M2Color (colorIndex 0xffff) → no baked tint.
    let untinted = subs
        .iter()
        .filter(|s| !s.additive && s.vertex_colors.is_empty())
        .count();
    assert!(
        untinted >= 2,
        "the two opaque batches carry no M2Color tint (empty vertex colours), found {untinted}"
    );
}
