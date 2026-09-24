//! The BRM lava bubbles' animation is frozen (bug B98) — and the asset says why in
//! one number: **every one of this model's animated channels is keyed inside file sequence slot 1,
//! and slot 1 is a 50 %-weighted variation of animation id 0.**
//!
//! `BlackrockStatueLavaBubble.m2` is placed 15 times inside `Blackrock.wmo`. It authors two
//! sequences, both animation id 0 with frequencies 16384/16383 — so the reference re-rolls between
//! them every play-window (`0x6951b0`) and two placements are
//! on different slots at the same instant. Slot 0 is a dead hold: its bone-scale window is two
//! identity keys and its texture-transform window is two zero keys. Slot 1 is the whole animation:
//! the bubbles swell 1.0 → 2.785 while the sprite's V offset flips by 0.605.
//!
//! benilla baked the UV loop against slot 0 and only slot 0, through a registry keyed by MATERIAL —
//! shared by every instance of the batch, so there is no sequence to key on. On this model that
//! bakes to `None`: no registration, no table row, no scroll, on every placement, for ever. This
//! file pins both halves of that — the dead slot 0 and the live slot 1 — so the per-sequence set
//! can never silently collapse back to one loop.
//!
//! Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{open_chain, parse_m2_animations, parse_m2_render_submeshes};

const BUBBLE: &str =
    "World\\KhazModan\\Blackrock\\PassiveDoodads\\BlackrockLavaBubbles\\BlackrockStatueLavaBubble.m2";

#[test]
fn the_lava_bubble_keys_its_whole_uv_flipbook_in_variation_one() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file(BUBBLE)
        .expect("the bubble M2 is in the chain");

    // 1. Two sequences, both animation id 0 — a variation chain, not a state machine. The roll is
    //    per placement and per window, so no single slot can be "the" one to bake.
    let anims = parse_m2_animations(&bytes);
    assert_eq!(anims.len(), 2, "two sequences");
    assert!(
        anims.iter().all(|a| a.anim_id == 0),
        "both are animation id 0 — one variation chain"
    );
    assert!(
        anims.iter().all(|a| a.frequency > 15_000),
        "…and both are genuinely rollable (≈50/50), so placements DIVERGE: {:?}",
        anims.iter().map(|a| a.frequency).collect::<Vec<_>>()
    );

    // 2. The bones say it first: slot 0 holds bind pose, slot 1 swells past 2.7×.
    let peak = |slot: usize| {
        anims[slot]
            .bones
            .iter()
            .flat_map(|b| b.scale.iter().map(|&(_, s)| s[0]))
            .fold(0.0f32, f32::max)
    };
    assert!((peak(0) - 1.0).abs() < 1e-3, "slot 0 is a rest hold");
    assert!(peak(1) > 2.5, "slot 1 is the swell (peak {})", peak(1));

    // 3. …and the UV channel agrees, which is the half benilla renders. Every batch carries a
    //    per-sequence set — the bake's own "the slots disagree, this batch cannot take the shared
    //    lane" verdict — whose slot 0 is empty and whose slot 1 moves.
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
    assert_eq!(subs.len(), 5, "five bubbles, five batches");
    for (i, sub) in subs.iter().enumerate() {
        let set = sub
            .uv_seq
            .as_ref()
            .unwrap_or_else(|| panic!("batch {i} carries a per-sequence UV set"));
        assert!(
            set.uniform().is_none(),
            "batch {i}: the slots disagree — that IS the verdict"
        );
        assert!(
            set.slots()[0].is_none(),
            "batch {i}: slot 0's window never moves the UVs — the dead bake"
        );
        let live = set.slots()[1]
            .as_ref()
            .unwrap_or_else(|| panic!("batch {i}: slot 1 carries the flipbook"));
        let v_span = live
            .keys
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &(_, v)| {
                (lo.min(v[1]), hi.max(v[1]))
            });
        assert!(
            v_span.1 - v_span.0 > 0.5,
            "batch {i}: the V offset flips a whole sprite row (span {:?})",
            v_span
        );
    }

    // 4. And the pre-1408 read — the shared lane's slot-0 bake — is `None` on every batch. This is
    //    the defect stated as an assertion: nothing to register, so nothing ever scrolled.
    assert!(
        subs.iter().all(|s| s.uv_anim.is_none()),
        "the slot-0 bake yields nothing on any batch — the frozen sprite B98 reported"
    );
}
