//! Swipe's cast model (`SpellVisual` 189, kit 182, `SpellVisualEffectName` 215) shows only through
//! its texture transform: its strips' UVs sit at u 0.945..1.944 on a CLAMP sheet with a transparent
//! border, so they draw nothing until the 1.5 s U scroll drags the claw across them.

use benilla_formats::{open_chain, parse_m2_render_submeshes, uv_transform};

const SWIPE: &str = "Spells\\SwipeCaster.m2";

/// The u extent of the UVs offset by `t` under `uv' = R·S·((uv + t) − p) + p` (`0x714260`).
fn u_span_at(uvs: &[[f32; 2]], offset: [f32; 2]) -> (f32, f32) {
    uvs.iter()
        .map(|&uv| uv_transform(uv, offset, [0.0, 0.0, 0.0, 1.0], [1.0, 1.0])[0])
        .fold((f32::MAX, f32::MIN), |(lo, hi), u| (lo.min(u), hi.max(u)))
}

#[test]
fn the_swipe_claw_trail_is_nothing_but_its_uv_scroll() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain.read_file(SWIPE).expect("SwipeCaster is in the chain");
    let subs = parse_m2_render_submeshes(&bytes, "Spells", &[]).expect("parse");

    assert_eq!(subs.len(), 2, "the two claw-strip batches");
    for (i, s) in subs.iter().enumerate() {
        let uv = s
            .uv_anim
            .as_ref()
            .unwrap_or_else(|| panic!("batch {i} carries a UV loop"));
        assert!(
            s.uv_seq.is_none(),
            "batch {i}: one sequence, so no per-slot set"
        );

        let (first, last) = (
            uv.keys.first().expect("keys").1,
            uv.keys.last().expect("keys").1,
        );
        assert!(
            first[0].abs() < 0.05 && (last[0] + 0.97).abs() < 0.05,
            "batch {i}: U sweeps ~0 → ~−0.97 ({first:?} → {last:?})"
        );
        assert!(
            (uv.period - 1.5).abs() < 0.01,
            "batch {i}: over the 1.5 s clip (period {})",
            uv.period
        );

        assert!(!s.wrap_x, "batch {i}: U is authored CLAMP");

        let frozen = u_span_at(&s.uvs, first);
        assert!(
            frozen.0 > 0.94,
            "batch {i}: at t=0 the whole strip is at/past the right edge (u {frozen:?})"
        );
        let scrolled = u_span_at(&s.uvs, last);
        assert!(
            scrolled.0 < 0.01 && scrolled.1 > 0.95,
            "batch {i}: at the last key it covers the sheet (u {scrolled:?})"
        );
    }

    // The claw sheet: a 16×16 blob with a fully transparent frame.
    let blp = chain
        .read_file("Spells\\BloodSpurtSmall01.blp")
        .expect("the claw sheet is in the chain");
    let tex = benilla_formats::blp_bytes_to_mip_chain(&blp).expect("decode");
    let (w, h) = (tex.width as usize, tex.height as usize);
    let px = &tex.mips[0];
    let alpha = |x: usize, y: usize| px[(y * w + x) * 4 + 3];
    assert!(
        (0..h).all(|y| alpha(w - 1, y) == 0 && alpha(0, y) == 0),
        "both U border columns are fully transparent"
    );
    assert!(
        (0..h).any(|y| (0..w).any(|x| alpha(x, y) > 128)),
        "…while the middle of the sheet is the claw itself"
    );
}
