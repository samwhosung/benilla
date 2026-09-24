//! A GameObject's idle is animation id 0 through its `playableAnimationLookup` (`0x71019b`), not
//! its first sequence. The book authors the door family in the order Close (146), Closed, Open,
//! Opened, all looping (`flags` bit 0 clear, `0x714585`), and its table sends id 0 to 147 Closed,
//! a still shut book; its first sequence is the 0.333 s Close sweep.

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

    assert_eq!(
        anims.first().map(|a| a.anim_id),
        Some(CLOSE),
        "the model's FIRST sequence is the Close motion, not a rest pose — the whole point"
    );

    assert!(
        anims[0].looping,
        "Close is a looping band — arming it is what made the book cycle for ever"
    );

    let playable = parse_m2_playable_animation_lookup(&bytes).expect("playable lookup");
    let idle_id = playable.first().map_or(0, |p| p.resolved_id);
    assert_eq!(
        idle_id, CLOSED,
        "playableAnimationLookup[0] resolves to Closed — the seed the reference arms"
    );

    // Bone 0 turns about x: x ≈ 0 is shut, x ≈ 0.69 (87°) open. Closed's band (533..633 ms) has no
    // keys of its own, so the window rule makes the identity keys at 500 and 667 one constant.
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

    // Closed tilts its two cover bones 2.5° off bind, which keeps the book on the skinned path.
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

    // The ownership table the missing-sequence remap branches on (`0x711960`): the door family.
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
