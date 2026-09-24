//! The Lamppost's glow card rides a spherical billboard bone (flag 0x08); its post does not.

use benilla_formats::{load_m2_mesh, open_chain, BillboardKind};

#[test]
fn lamppost_glow_is_spherical_billboard() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Lamppost\\Lamppost.m2",
    )
    .expect("load Lamppost");

    let spherical = subs
        .iter()
        .filter_map(|s| s.billboard.as_ref())
        .find(|b| b.kind == BillboardKind::Spherical);
    let bb = spherical.expect("lamppost has a spherical-billboard batch (the glow card)");
    assert!(
        bb.pivot.iter().all(|c| c.is_finite()),
        "billboard pivot is finite, got {:?}",
        bb.pivot
    );
    assert!(
        subs.iter().any(|s| s.billboard.is_none()),
        "the lamppost post batch is not billboarded"
    );
    eprintln!("lamppost billboard pivot = {:?}", bb.pivot);

    // The glow's pulse: bone 4's scale track on global sequence 0, 5 keys, 0.86..1.04.
    let anim = bb
        .scale_anim
        .as_ref()
        .expect("the lamppost glow card's billboard bone has a global-sequence scale pulse");
    assert_eq!(anim.duration_ms, 1333, "global sequence loop length (ms)");
    assert_eq!(anim.keys.len(), 5, "scale keyframe count");
    assert!(anim.interp, "the scale track interpolates (interp != 0)");
    let samples: Vec<f32> = (0..anim.duration_ms)
        .step_by(33)
        .map(|t| anim.sample(t)[0])
        .collect();
    let (lo, hi) = samples
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    assert!(
        (0.8..0.95).contains(&lo) && (1.0..1.1).contains(&hi),
        "scale pulse breathes within ~0.86..1.04, got {lo}..{hi}"
    );
    assert!(
        hi - lo > 0.1,
        "the pulse has visible amplitude, got {lo}..{hi}"
    );
    let s = anim.sample(700);
    assert!(
        (s[0] - s[1]).abs() < 1e-6 && (s[1] - s[2]).abs() < 1e-6,
        "scale is uniform, got {s:?}"
    );
    eprintln!(
        "lamppost glow pulse: {lo:.3}..{hi:.3} over {} ms",
        anim.duration_ms
    );
}
