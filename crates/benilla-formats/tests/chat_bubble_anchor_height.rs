//! Pins the **chat bubble's** anchor height against real build-5875 assets — the sibling of
//! `selection_ring_radius`, reading the same Stand-animation box on the other axis.
//!
//! The reference anchors the bubble at
//!
//! ```text
//! worldZ = unit.z + (StandBox.max.z − StandBox.min.z) × modelScale + 0.7
//! ```
//!
//! where the extent comes from `0x4b0e38 call 0x711a20`, traced into the model layer: it reads
//! the **MD20 header image** — file bytes, no bone matrix anywhere in the call tree — returning
//! that CAaBox's Z. The scaled product
//! is latched at `bubble+0x354` behind a parity guard, so it is queried once per chat line.
//!
//! Benilla shipped the bubble on the **posed PlayerName attachment** instead (the `0x608640`
//! chain), on a recorded INFERRED claim that the two calls were equivalent — "both the head-region
//! attachment height, model-scaled". They are not: they differ precisely on animated-vs-static.
//! This test exists so that claim cannot come back by accident, and so the two heights stay
//! distinguishable — they are close enough (2.01 vs 2.21 on a human male) that an eye cannot tell
//! them apart on a human (2.01 vs 2.21) — which is exactly how the wrong one shipped. On a chicken
//! they are not close at all (0.44 vs 0.81): Blizzard floats the *name* anchor well clear of a small
//! model's head, while the bubble hugs the model. That spread is why the pair is pinned per model
//! rather than as a single fudge factor. Decision 1406.
//!
//! Skips when the gitignored client data isn't present.

use benilla_formats::{load_m2_bounds, open_chain};

#[test]
fn stand_box_z_is_the_bubble_anchor_height_and_not_the_attachment() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    // (model, the Stand box's Z extent, the PlayerName attachment-18 Z benilla used to anchor on).
    // Both read straight out of the shipped M2s; the pair is the point — a test that only pinned
    // the first would pass just as happily on the wrong source.
    // All four numbers are read out of the shipped M2s, not estimated: the Stand box from the
    // `M2Sequence` CAaBox at `animationLookup[0]` (record 0 for the human, record **2** for the
    // chicken, whose record 0 is a flap), the attachment from the attachment table's id 18.
    let cases = [
        (
            "Character\\Human\\Male\\HumanMale.mdx",
            2.0128_f32,
            2.2120_f32,
        ),
        ("Creature\\Chicken\\Chicken.mdx", 0.4435, 0.8090),
    ];

    for (path, stand_z, attach_z) in cases {
        let b = load_m2_bounds(&mut chain, path).unwrap_or_else(|e| panic!("bounds {path}: {e}"));
        assert!(
            (b.stand_box_z - stand_z).abs() < 0.001,
            "{path}: stand_box_z {:.4} should be the Stand CAaBox Z extent {stand_z}",
            b.stand_box_z
        );
        // The two sources are genuinely different values, so a regression to the attachment cannot
        // slip through the bound above.
        assert!(
            (b.stand_box_z - attach_z).abs() > 0.02,
            "{path}: stand_box_z {:.4} must not be the posed-attachment height {attach_z} — those \
             are the two mechanisms 1406 separated",
            b.stand_box_z
        );
        // It is a height, and it is not the all-animation vertex box (which on a character runs far
        // over the head — the reason that box is wrong for anything head-shaped).
        assert!(
            b.stand_box_z > 0.0,
            "{path}: a Stand box has positive height"
        );
        let header_z = b.bbox_max[2] - b.bbox_min[2];
        assert!(
            b.stand_box_z < header_z,
            "{path}: the Stand box {:.4} sits inside the all-animation box {header_z:.4}",
            b.stand_box_z
        );
    }
}
