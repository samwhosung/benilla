//! The druid's Swipe leaves no claw trail — and, like the lava bubbles next door,
//! the asset says why in one number: **this model's whole visible existence is its texture
//! transform.**
//!
//! `Spells\SwipeCaster.m2` — Swipe's `cast` kit (`SpellVisual` 189 → kit 182 →
//! `SpellVisualEffectName` 215) — is two 51-vertex strips, three per batch, each rigidly skinned to
//! one bone that sweeps it through the arc. Their UVs are authored `u ∈ [0.945, 1.944]` against a
//! sheet the file addresses **CLAMP**, whose border texels are fully transparent. At the
//! translation track's first key the strips therefore sample the sheet's transparent right edge
//! along their whole length and draw *nothing at all*; the 1.5 s U-scroll is what drags the claw
//! across them.
//!
//! So a consumer that renders this model's geometry, rig, alpha loops and particles but not its
//! texture transform renders a correct-looking blood spray and no trail — which is exactly what
//! benilla's spell-effect lane did, on 0271's recorded premise that "no effect model in the
//! current corpus needs it". This file pins the three facts that make that premise false, so the
//! effect lane's UV channel can never be quietly dropped again.
//!
//! Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{open_chain, parse_m2_render_submeshes, uv_transform};

const SWIPE: &str = "Spells\\SwipeCaster.m2";

/// The u extent the batch's authored UVs cover once the loop's offset at `t` is folded in, by the
/// verified law (`0x714260`: `uv' = R·S·((uv + t) − p) + p`, and with no rotation
/// or scaling authored here that is a pure `uv + t`).
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
        // 1. Every batch animates its texture transform, on the SHARED spelling (one sequence, so
        //    the slots cannot disagree and 1408's per-slot set is rightly absent).
        let uv = s
            .uv_anim
            .as_ref()
            .unwrap_or_else(|| panic!("batch {i} carries a UV loop"));
        assert!(
            s.uv_seq.is_none(),
            "batch {i}: one sequence, so no per-slot set"
        );

        // 2. The scroll is a near-whole-sheet sweep along U, and it runs the length of the clip.
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

        // 3. The sheet is CLAMP-addressed on U — which is what turns the scroll from a nicety into
        //    the whole visual. A repeat-addressed strip would show *something* at every offset.
        assert!(!s.wrap_x, "batch {i}: U is authored CLAMP");

        // 4. …and so, frozen at the first key, the strip sits entirely at or past the sheet's right
        //    edge: clamped, every one of its texels is the border. Scrolled to the last key it
        //    covers the sheet instead. This pair IS the bug and its fix, in the asset's own numbers.
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

    // 5. The border really is empty, so "clamped to the edge" means "invisible" and not "a stripe
    //    of colour". `BloodSpurtSmall01` is a 16×16 blob with a fully transparent frame.
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
