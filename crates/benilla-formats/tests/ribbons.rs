//! The M2 ribbon-emitter parser against shipped trail-carrying models.

use benilla_formats::{open_chain, parse_m2_ribbon_emitters};

#[test]
fn ribbon_records_match_real_bytes() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    let wisp =
        parse_m2_ribbon_emitters(&chain.read_file("Creature\\WISP\\WispRed.m2").unwrap()).unwrap();
    assert_eq!(wisp.len(), 3, "WispRed authors three ribbons");
    for (i, r) in wisp.iter().enumerate() {
        assert!(
            r.edges_per_second > 0.0 && r.edges_per_second < 1000.0,
            "ribbon {i} sane edge rate, got {}",
            r.edges_per_second
        );
        assert!(
            r.edge_lifetime >= 0.25,
            "ribbon {i} lifetime respects the reference's 0.25 s load clamp"
        );
        assert!(
            r.height_above.peak() + r.height_below.peak() > 0.0,
            "ribbon {i} has a nonzero peak cross-section"
        );
        assert!(
            r.texture.is_some(),
            "ribbon {i} resolves a trail texture from the M2 textures table"
        );
        for &(_, a) in &r.alpha.keys {
            assert!(
                (0.0..=1.0).contains(&a),
                "ribbon {i} fixed16 alpha decodes into 0..1, got {a}"
            );
        }
        for &(_, c3) in &r.color.keys {
            for c in c3 {
                assert!((0.0..=1.5).contains(&c), "ribbon {i} color channel {c}");
            }
        }
    }

    // A weapon whose enchant trail rides the item root.
    let blade = parse_m2_ribbon_emitters(
        &chain
            .read_file("ITEM\\ObjectComponents\\WEAPON\\Sword_1H_Thunderblade_A_01.m2")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(blade.len(), 3, "Thunderblade authors three ribbons");
    assert!(blade.iter().all(|r| r.texture.is_some()));

    let torch = parse_m2_ribbon_emitters(
        &chain
            .read_file("World\\Generic\\PassiveDoodads\\Lights\\Torch.m2")
            .unwrap(),
    )
    .unwrap();
    assert!(torch.is_empty(), "the torch has no ribbons");
}
