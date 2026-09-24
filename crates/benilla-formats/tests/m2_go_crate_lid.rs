//! A door-family transition ends on the object layer, not the loop bit: the completion callback
//! fires once at the arm's window (span × replay `R`), and slot 14 `0x5f4120` advances Close to
//! Closed. `G_Crate01.m2`'s lid runs Open 0° to 75°, Opened, Close 75° to 0°, Closed, and all four
//! loop (`flags` bit 0 clear, `0x714585`).

use benilla_formats::{open_chain, parse_m2_animation_lookup, parse_m2_animations};

/// A quest ammo crate, GameObject type 3 (CHEST).
const CRATE: &str = "World\\Goober\\G_Crate01.m2";
/// `AnimationData.dbc`: 146 Close (motion), 147 Closed (rest), 148 Open (motion), 149 Opened (rest).
const CLOSE: u16 = 146;
const CLOSED: u16 = 147;
const OPEN: u16 = 148;
const OPENED: u16 = 149;
/// The lid, the only bone the door family moves (0 and 1 are the body, 9.. the shards).
const LID_BONE: u16 = 8;

#[test]
fn the_crate_lid_transition_is_bounded_by_the_window_not_the_loop_bit() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file(CRATE)
        .expect("the crate M2 is in the chain");
    let anims = parse_m2_animations(&bytes);
    let seq = |id: u16| {
        anims
            .iter()
            .find(|a| a.anim_id == id)
            .unwrap_or_else(|| panic!("the crate authors animation id {id}"))
    };

    // The whole family is authored, so the remap leg (`0x5f3972`) never runs.
    let lookup = parse_m2_animation_lookup(&bytes).expect("animation lookup");
    for id in [CLOSE, CLOSED, OPEN, OPENED] {
        assert!(
            lookup.get(id as usize).is_some_and(|&s| s != 0xffff),
            "the crate owns animation id {id}"
        );
    }

    for id in [CLOSE, CLOSED, OPEN, OPENED] {
        assert!(
            seq(id).looping,
            "id {id} is a bit-0-clear band — the kernel wraps it, so only the object layer's \
             completion advance can end a transition"
        );
    }

    // `R = max(1, min + roll)`, and an empty replay range rolls 1 (`0x712692`..`0x7126cd`).
    for id in [CLOSE, CLOSED, OPEN, OPENED] {
        let s = seq(id);
        assert_eq!(
            (s.min_replay, s.max_replay),
            (0, 0),
            "id {id} rolls R = 1, so the completion lands at one band length"
        );
    }

    // The lid turns about −Y: |y| ≈ 0 is shut, |y| ≈ 0.61 (75°) open.
    let lid = |id: u16| -> Vec<[f32; 4]> {
        seq(id)
            .bones
            .iter()
            .find(|b| b.bone == LID_BONE)
            .map(|b| b.rotation.iter().map(|(_, q)| *q).collect())
            .unwrap_or_default()
    };
    let span = |q: &[[f32; 4]]| {
        q.iter().fold((f32::MAX, f32::MIN), |(lo, hi), q| {
            (lo.min(q[1].abs()), hi.max(q[1].abs()))
        })
    };

    let (lo, hi) = span(&lid(CLOSE));
    assert!(
        lo < 1e-3 && hi > 0.5,
        "Close must sweep from open (|y| {hi}) to shut (|y| {lo}) — that sweep, looped, is the \
         reported bug"
    );
    let (lo, hi) = span(&lid(OPEN));
    assert!(
        lo < 1e-3 && hi > 0.5,
        "Open must sweep from shut (|y| {lo}) to open (|y| {hi})"
    );
    // Each rest pose is where its motion lands, so the `0x5f4120` advance onto it is seamless.
    for q in lid(CLOSED) {
        assert!(q[1].abs() < 1e-3, "Closed holds the SHUT lid, got {}", q[1]);
    }
    for q in lid(OPENED) {
        assert!(q[1].abs() > 0.5, "Opened holds the OPEN lid, got {}", q[1]);
    }
}
