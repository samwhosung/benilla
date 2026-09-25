//! The Blackrock lava bubble keys all its animation in file sequence slot 1, a 50% variation of
//! animation id 0: both sequences are id 0 at frequencies 16384 and 16383, re-rolled every play
//! window (`0x6951b0`). Slot 0 holds still; slot 1 swells the bubbles to 2.785 and flips the V
//! offset by 0.605.

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

    let peak = |slot: usize| {
        anims[slot]
            .bones
            .iter()
            .flat_map(|b| b.scale.iter().map(|&(_, s)| s[0]))
            .fold(0.0f32, f32::max)
    };
    assert!((peak(0) - 1.0).abs() < 1e-3, "slot 0 is a rest hold");
    assert!(peak(1) > 2.5, "slot 1 is the swell (peak {})", peak(1));

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

    assert!(
        subs.iter().all(|s| s.uv_anim.is_none()),
        "the slot-0 bake yields nothing on any batch — read alone, the sprite freezes"
    );
}
