//! Texture unit flag 0x08 is `no_hit_test`: the scene ray skips such a unit (`0x70902f`). Arathi
//! Basin's neutral banner aura glow (units `0x18`) carries it on every batch, so the pick reaches
//! the banner inside it; the banner itself (units `0x10`) stays hit-tested.

use benilla_formats::{load_m2_mesh, open_chain};

#[test]
fn the_banner_aura_glow_is_out_of_the_hit_test() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let aura = load_m2_mesh(
        &mut chain,
        "World\\Generic\\PassiveDoodads\\ParticleEmitters\\AuraYellowVeryTall.m2",
    )
    .expect("load AuraYellowVeryTall");
    assert!(!aura.is_empty());
    assert!(
        aura.iter().all(|s| s.no_hit_test),
        "every batch of the aura glow carries unit flag 0x08"
    );

    let banner = load_m2_mesh(
        &mut chain,
        "World\\Generic\\PVP\\BattlefieldBanners\\BattlefieldBannerHordeLarge.m2",
    )
    .expect("load BattlefieldBannerHordeLarge");
    assert!(!banner.is_empty());
    assert!(
        banner.iter().all(|s| !s.no_hit_test),
        "the banner's batches stay in the hit test"
    );
}
