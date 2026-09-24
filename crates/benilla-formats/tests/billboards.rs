//! Regression: an M2 batch that packs several billboard glow cards on different bones must be split
//! into one submesh per billboard bone, each centred on its own bone pivot — so the renderer rotates
//! each card about its own candle (faces the camera in place) instead of swinging the whole cluster
//! about a single pivot. See. Skips (passes) when the client isn't present.

use benilla_formats::{load_m2_mesh, open_chain, RenderSubmesh};

/// The geometric centre of a submesh's vertices (model space).
fn geom_center(s: &RenderSubmesh) -> [f32; 3] {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &s.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    [
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    ]
}

#[test]
fn candelabra_glow_cards_split_per_bone() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    // CandelabraTallWall01 has five candles; all five glow cards share one additive glow-texture batch,
    // each quad skinned to its own per-candle billboard bone.
    let subs = load_m2_mesh(
        &mut chain,
        "World\\Generic\\PassiveDoodads\\Lights\\CandelabraTallWall01.m2",
    )
    .expect("load CandelabraTallWall01.m2");

    let cards: Vec<&RenderSubmesh> = subs.iter().filter(|s| s.billboard.is_some()).collect();
    assert_eq!(
        cards.len(),
        5,
        "the one glow batch must split into five per-candle billboard cards, got {}",
        cards.len()
    );

    let mut pivots: Vec<[f32; 3]> = Vec::new();
    for c in &cards {
        let bb = c.billboard.as_ref().unwrap();
        // Each split card is a single quad (4 verts / 2 tris) …
        assert_eq!(c.positions.len(), 4, "a glow card is one quad (4 verts)");
        assert_eq!(c.indices.len(), 6, "a glow card is one quad (2 tris)");
        // … centred on its OWN bone pivot, so it faces the camera and pulses in place (no swing/slide).
        let ctr = geom_center(c);
        let d = [
            ctr[0] - bb.pivot[0],
            ctr[1] - bb.pivot[1],
            ctr[2] - bb.pivot[2],
        ];
        let off = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(
            off < 0.05,
            "card geometry must sit on its pivot (off={off:.3} yd, pivot={:?})",
            bb.pivot
        );
        pivots.push(bb.pivot);
    }

    // The five pivots are five distinct candle positions (no two cards collapsed onto one bone).
    for (i, a) in pivots.iter().enumerate() {
        for b in &pivots[i + 1..] {
            let spread = (a[1] - b[1]).abs() + (a[2] - b[2]).abs();
            assert!(
                spread > 0.05,
                "two cards collapsed onto the same pivot: {a:?} ~ {b:?}"
            );
        }
    }
}
