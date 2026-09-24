//! **FixColorVertexAlpha** — the bright-doorway portal fade, pinned to the reference's own live
//! capture and to the building the fade is load-bearing for (Dire Maul's entrance corridors).
//!
//! The capture is an unusually exact oracle. It read the abbey's uploaded MOCV vertex buffers back
//! off the D3D stream and diffed them against the file: **678/678 vertices byte-identical in group 1,
//! 496/506 in group 3** — the fade is the whole difference, and it is exactly 10 vertices, all at the
//! one portal whose far side is an EXTERIOR group, all hard-set to `(255,255,255,255)`. Interior↔
//! interior portals whiten nothing, "including vertices AT portal corners".
//!
//! That single pair of counts pins every part of the mechanism at once — the exterior-neighbour gate,
//! the containment half of the distance kernel (an infinite-plane test whitens 12 here, not 10), and
//! the absence of the MOPY alpha pre-pass (which would rewrite 372 of group 3's alphas and blow the
//! "byte-identical" finding apart). Skips when the client isn't present.

use benilla_formats::{parse_wmo_root, wmo_group_fixed_colors, Chain};

/// How many of a group's MOCV slots the fade rewrites, and the mean luminance before → after.
fn fade_census(reader: &Chain, stem: &str, gi: u32) -> (usize, usize, f32, f32) {
    let root_bytes = reader.read(&format!("{stem}.wmo")).expect("read root");
    let root = parse_wmo_root(&root_bytes).expect("parse root");
    let gbytes = reader
        .read(&format!("{stem}_{gi:03}.wmo"))
        .expect("read group");
    let raw = benilla_formats::wmo_group_raw_colors(&gbytes).expect("group carries MOCV");
    let fixed = wmo_group_fixed_colors(&gbytes, &root).expect("group carries MOCV");
    let lum =
        |c: [u8; 4]| 0.299 * f32::from(c[2]) + 0.587 * f32::from(c[1]) + 0.114 * f32::from(c[0]);
    let n = raw.len().max(1) as f32;
    let changed = raw.iter().zip(&fixed).filter(|(a, b)| a != b).count();
    (
        changed,
        raw.len(),
        raw.iter().map(|&c| lum(c)).sum::<f32>() / n,
        fixed.iter().map(|&c| lum(c)).sum::<f32>() / n,
    )
}

/// The abbey, against the D3D capture: group 1 untouched, group 3 rewritten in exactly 10 slots.
#[test]
fn abbey_matches_the_reference_capture() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let stem = "World\\wmo\\Azeroth\\Buildings\\NSabbey\\NSabbey";

    let (changed, total, _, _) = fade_census(&reader, stem, 1);
    assert_eq!(total, 678, "capture's group 1 upload was 678 vertices");
    assert_eq!(
        changed, 0,
        "group 1's portals all lead to interior neighbours — the capture read it byte-exact"
    );

    let (changed, total, before, after) = fade_census(&reader, stem, 3);
    assert_eq!(total, 506, "capture's group 3 upload was 506 vertices");
    assert_eq!(
        changed, 10,
        "the capture read 496/506 byte-exact — the fade's whole footprint is 10 slots \
         (an infinite-plane distance test rewrites 12; the MOPY pre-pass would rewrite 372)"
    );
    // Those 10 were already near-white in the file (225..254) — the fade is a seam touch-up here,
    // not a lighting change. Anything that moves this group's mean has over-fired.
    assert!(
        (before - after).abs() < 0.5,
        "abbey g003 mean luminance moved {before} → {after}"
    );
}

/// Dire Maul's entrance corridors — why the fade is not optional. Each of the five short transition
/// passages floors its walkway at MOCV `(10,10,40, α=0)`, a near-black navy the interior TRANS law
/// renders literally: a black floor between lit walls (the director's report, 2026-08-04). The floor
/// quad's corners sit *in* the doorway portals, so the fade takes them white.
#[test]
fn dire_maul_entrance_corridor_floors_are_lit() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let stem = "World\\wmo\\Dungeon\\KL_Diremaul\\KL_Diremaul";

    for gi in [12, 14, 17, 27, 31] {
        let root_bytes = reader.read(&format!("{stem}.wmo")).expect("read root");
        let root = parse_wmo_root(&root_bytes).expect("parse root");
        let gbytes = reader
            .read(&format!("{stem}_{gi:03}.wmo"))
            .expect("read group");
        let raw = benilla_formats::wmo_group_raw_colors(&gbytes).expect("MOCV");
        let fixed = wmo_group_fixed_colors(&gbytes, &root).expect("MOCV");

        // The authored walkway: the near-black navy, alpha 0 — the "unlit, the fade lights me" bake.
        let dark = raw.iter().filter(|c| **c == [40, 10, 10, 0]).count();
        assert!(
            dark >= 4,
            "g{gi:03} should author its walkway quad at BGRA (40,10,10,0); found {dark} such slots"
        );
        // After the fade every one of them reads white: `tex × white` at full lit weight. Not always
        // *exactly* 255 — a corner a couple of centimetres off the portal plane takes the partial
        // lerp at `t ≈ 0.997` and lands on 254 — so the assertion is "no longer dark", which is the
        // claim that matters. 13 → ~255 luminance is the whole bug.
        for (i, (r, f)) in raw.iter().zip(&fixed).enumerate() {
            if *r == [40, 10, 10, 0] {
                assert!(
                    f.iter().all(|&c| c >= 250),
                    "g{gi:03} v{i}: the corridor floor must whiten, or it renders black — got {f:?}"
                );
            }
        }
    }
}
