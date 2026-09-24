//! Pins the camera's **swim** framing-pivot preset against real build-5875 assets — the third
//! sibling of `selection_ring_radius` and `chat_bubble_anchor_height`, reading the same
//! `M2Sequence` `CAaBox` those two read, on a second animation.
//!
//! The reference keeps three framing-pivot presets and picks one per frame at `0x50f880`:
//!
//! ```text
//! swimming (MOVEFLAG 0x200000) -> cam+0x124        ; the swim preset
//! else cam+0x198 < 1.8315      -> cam+0x11c        ; zoomed in    -- NOT BUILT
//! else                            cam+0x120        ; zoomed out   -- NOT BUILT
//! ```
//!
//! All three are rebuilt from the model by `0x50ca90` off one shared base — the
//! `attach17.z + 0.0972` neck height — and only `+0x124` is then pulled down:
//!
//! ```text
//! 50cccf  call 0x711a20(model, seq=0)     ; Stand
//! 50ccde  call 0x711a20(model, seq=0x2a)  ; Swim (42)
//! 50ccf6  fsubr [esi+0x124]               ; +0x124 -= S * (Stand.max.z - Swim.max.z)
//! ```
//!
//! Measured off the shipped Human Male at scale 1: `+0x11c = +0x120 = 1.9002692` and
//! `+0x124 = 1.5120120`.
//! That pair — and only that pair — is a number read off the binary; everything else below is
//! measured off the shipped M2s, pinned so the id-42 lookup cannot quietly stop resolving.
//!
//! The Human Male standing side comes out 1.900247 here rather than 1.9002692 because benilla
//! ships the base constant as `0.0972` while the reference's `[0x808ab0]` is `0.0972222` — 0.02 mm,
//! and *not* part of this feature, which is why the presets get a 1e-4 window and the drop itself
//! (a pure box difference, no constant in it) gets 1e-5.
//!
//! Skips when the gitignored client data isn't present.

use benilla_formats::{load_m2_bounds, open_chain};

/// The base the reference adds to all three presets — `attach17.z + 0.0972222 [0x808ab0]`, as
/// benilla's own display builder rounds it (`entities::display::build_parts`).
const PIVOT_BASE: f32 = 0.0972;

#[test]
fn the_swim_preset_is_the_stand_minus_swim_sequence_box_delta() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    let b = load_m2_bounds(&mut chain, "Character\\Human\\Male\\HumanMale.mdx")
        .expect("bounds HumanMale");
    let standing = b.pivot_z.expect("HumanMale authors attachment 17") + PIVOT_BASE;

    // The one number read off the binary: 1.9002692 − 1.5120120.
    assert!(
        (b.swim_pivot_drop - 0.3882572).abs() < 1e-5,
        "HumanMale swim_pivot_drop {:.7} should be the reference's 1.9002692 − 1.5120120 = 0.3882572",
        b.swim_pivot_drop
    );
    assert!(
        (standing - 1.9002692).abs() < 1e-4,
        "HumanMale standing preset {standing:.7} should be the reference's cam+0x11c 1.9002692"
    );
    assert!(
        (standing - b.swim_pivot_drop - 1.512_012).abs() < 1e-4,
        "HumanMale swim preset {:.7} should be the reference's cam+0x124 1.5120120",
        standing - b.swim_pivot_drop
    );
}

#[test]
fn the_drop_is_per_model_and_zero_for_a_body_with_no_swim_sequence() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");

    // (model, the drop measured off the shipped M2). Character models only for the non-zero rows:
    // the drop is `Stand.max.z − Swim.max.z`, and how far a race's swim pose sinks below its stand
    // pose is authored per skeleton — a night elf barely dips, a tauren drops nearly half a yard.
    // A single-model pin would pass just as happily on a lookup that always resolved record 0.
    let cases = [
        ("Character\\Human\\Male\\HumanMale.mdx", 0.3882571_f32),
        ("Character\\Tauren\\Male\\TaurenMale.mdx", 0.4236124),
        ("Character\\NightElf\\Female\\NightElfFemale.mdx", 0.1288066),
        // No Swim sequence at all: the chicken's animation lookup is 17 entries long, so id 42 is
        // past its end and no record carries that id. This is the reference's own both-present
        // guard (`0x711960` on 0 and 0x2a at `0x50cc43`/`0x50cc4f`) — a body that cannot swim keeps
        // the standing preset in every state.
        ("Creature\\Chicken\\Chicken.mdx", 0.0),
    ];

    for (path, drop) in cases {
        let b = load_m2_bounds(&mut chain, path).unwrap_or_else(|e| panic!("bounds {path}: {e}"));
        assert!(
            (b.swim_pivot_drop - drop).abs() < 1e-5,
            "{path}: swim_pivot_drop {:.7} should be {drop:.7}",
            b.swim_pivot_drop
        );
        // A drop is a drop: never negative, and never so large it would put the swim preset under
        // the model's feet. Both hold for every shipped body; either failing means the id-42
        // lookup landed on the wrong record.
        let standing = b.pivot_z.map(|z| z + PIVOT_BASE).unwrap_or(0.0);
        assert!(
            b.swim_pivot_drop >= 0.0 && b.swim_pivot_drop < standing,
            "{path}: a swim drop of {:.7} against a standing preset of {standing:.7} is not a dip",
            b.swim_pivot_drop
        );
    }
}
