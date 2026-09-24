//! Every book in the world opened and closed constantly — because the loader-idle
//! seed armed the model's **file-order-first** sequence, and on a book that sequence is the *Close
//! motion*, not the closed pose.
//!
//! `G_BookOpenMediumBrown.m2` is the model that makes this unmissable. Its four sequences are the
//! whole door family — Close(146) / Closed(147) / Open(148) / Opened(149) — authored in **that**
//! order, so file-order-0 is a 0.333 s motion that sweeps bone 0 from the open angle to the closed
//! one. Every one of the four carries `flags` bit 0 clear, i.e. the kernel loops it forever
//! (`0x714585`). Looping the Close motion is therefore a book that snaps
//! open and swings shut three times a second, for ever — exactly what was reported.
//!
//! The reference arms **animation id 0 (Stand)** resolved through the model's own
//! `playableAnimationLookup` (`0x71019b`), and this book's table sends id 0 to
//! **147 Closed** — a two-key band whose keys are both identity, i.e. a still, shut book.
//!
//! The DuelingFlag test next door pins the same seed law on a model whose first sequence is a
//! *Spawn*; this one pins it where the wrong pick is a **motion**, which is the visually loudest
//! failure and the one a whole class of GameObjects (every `World\Goober\` door-family prop) shares.
//! Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{
    open_chain, parse_m2_animation_lookup, parse_m2_animations, parse_m2_playable_animation_lookup,
};

const BOOK: &str = "World\\Goober\\G_BookOpenMediumBrown.m2";
/// `AnimationData.dbc`: 146 Close (motion), 147 Closed (rest), 148 Open (motion), 149 Opened (rest).
const CLOSE: u16 = 146;
const CLOSED: u16 = 147;
const OPEN: u16 = 148;

#[test]
fn the_book_idle_resolves_to_closed_not_the_close_motion() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain.read_file(BOOK).expect("the book M2 is in the chain");
    let anims = parse_m2_animations(&bytes);

    // 1. File order really does lead with the Close MOTION — without this the test proves nothing.
    assert_eq!(
        anims.first().map(|a| a.anim_id),
        Some(CLOSE),
        "the model's FIRST sequence is the Close motion, not a rest pose — the whole point"
    );

    // 2. That first sequence loops (bit 0 clear), so arming it is a permanent flap, not a one-shot.
    assert!(
        anims[0].looping,
        "Close is a looping band — arming it is what made the book cycle for ever"
    );

    // 3. The loader's seed resolves to Closed through the model's own table.
    let playable = parse_m2_playable_animation_lookup(&bytes).expect("playable lookup");
    let idle_id = playable.first().map_or(0, |p| p.resolved_id);
    assert_eq!(
        idle_id, CLOSED,
        "playableAnimationLookup[0] resolves to Closed — the seed the reference arms"
    );

    // 4. The pose each band actually holds, as the viewer would see it. This is the assertion that
    //    catches the bug coming back even if the id plumbing is rewritten. Bone 0's rotation is a
    //    pure x-axis quaternion: x ≈ 0 is the shut book, x ≈ 0.69 (≈ 87°) the open one.
    //
    //    Closed's band (533..633 ms) has NO keys of its own — the authored keys sit at 500 and 667,
    //    both identity — so the window rule (decision 0643) resolves it to a single constant key.
    //    One key here is the *correct* answer, not a missing one: the band genuinely doesn't move.
    let root_rot = |id: u16| -> Vec<[f32; 4]> {
        anims
            .iter()
            .find(|a| a.anim_id == id)
            .and_then(|a| a.bones.iter().find(|b| b.bone == 0))
            .map(|b| b.rotation.iter().map(|(_, q)| *q).collect())
            .unwrap_or_default()
    };
    let closed = root_rot(CLOSED);
    assert!(!closed.is_empty(), "Closed must resolve to a pose");
    for q in &closed {
        assert!(
            q[0].abs() < 1e-3,
            "Closed must hold the SHUT pose (bone 0 x ≈ 0), got {}",
            q[0]
        );
    }

    //    Close, by contrast, sweeps: it starts at the open angle and lands shut. Looping *that* is
    //    the flapping book — so both halves of the claim are pinned, not just the one we fixed.
    let close_motion = root_rot(CLOSE);
    let (lo, hi) = close_motion
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), q| {
            (lo.min(q[0]), hi.max(q[0]))
        });
    assert!(
        hi - lo > 0.5 && lo.abs() < 1e-3 && hi > 0.5,
        "Close must swing from open ({hi}) to shut ({lo}) — that sweep, looped, IS the reported bug"
    );

    // 5. Closed is a non-BIND pose, which is what keeps the book on the skinned path at all: its
    //    two cover bones sit at ±2.5° while bind has them square. If the idle ever stopped counting
    //    as "differs from bind" the book would drop to the static mesh — silently, because the
    //    difference is small enough to pass an eyeball and large enough to be the wrong book.
    let closed_seq = anims
        .iter()
        .find(|a| a.anim_id == CLOSED)
        .expect("the book authors Closed");
    let tilted = closed_seq
        .bones
        .iter()
        .filter(|b| {
            b.rotation
                .iter()
                .any(|(_, q)| (q[3].abs() - 1.0).abs() > 1e-4)
        })
        .count();
    assert!(
        tilted >= 2,
        "Closed must pose bones off bind (the covers), got {tilted}"
    );

    // 6. The ownership table the GameObject arm's missing-sequence remap branches on (the
    //    reference's `0x711960`): this model owns the whole door family and nothing beyond it, so
    //    no remap leg is reachable here and the arm plays the LUT id directly.
    let lookup = parse_m2_animation_lookup(&bytes).expect("animation lookup");
    let owns = |id: u16| lookup.get(id as usize).is_some_and(|&s| s != 0xffff);
    for id in [CLOSE, CLOSED, OPEN, 149] {
        assert!(owns(id), "the book authors animation id {id}");
    }
    assert!(
        !owns(0),
        "the book authors NO Stand — the seed reaches Closed only through the playable table, \
         which is exactly why a naive `find(Stand)` would arm nothing"
    );
    assert!(
        !owns(150),
        "ids past the table's end read as the out-of-bounds sentinel, i.e. not owned"
    );
}
