//! The ammo crate the director looted was **stuck open/closing** — the lid springing to 75° and
//! swinging shut, ~1.5 times a second, for ever. The model is why the symptom is
//! that loud, and why "hold the motion's last frame" was never enough on its own.
//!
//! `G_Crate01.m2` is the door family, textbook: **bone 8** is the lid, and the four sequences are
//! Open(148) 0° → 75°, Opened(149) holding 75°, Close(146) 75° → 0°, Closed(147) holding 0°. Every
//! one of them carries `flags` bit 0 **clear**, i.e. the kernel wraps its band for ever
//! (`0x714585`) — so the loop bit says nothing about how long a
//! transition lasts, and a consumer that reads it as "should this clip repeat?" arms the Close
//! sweep on an endless loop the moment the loot window closes.
//!
//! What ends a swing in the real client is the **object layer**: the completion callback fires once
//! at the arm's baked window (span × replay `R`, the loop bit ignored) and slot 14 `0x5f4120`
//! advances substate 4 Close → 1 Closed. This file pins the asset facts that law rests on —
//! the loop bits, `R = 1`, and that Close's last frame *is* the Closed pose — so the fix can't be
//! undone by "the clip says it loops".
//!
//! The `G_BookOpenMediumBrown` test next door pins the seed law on the same family; this one pins
//! the **transition** law. Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{open_chain, parse_m2_animation_lookup, parse_m2_animations};

/// The quest ammo crate the report came off — GameObject type 3 CHEST, `world/goober/g_crate01.m2`.
const CRATE: &str = "World\\Goober\\G_Crate01.m2";
/// `AnimationData.dbc`: 146 Close (motion), 147 Closed (rest), 148 Open (motion), 149 Opened (rest).
const CLOSE: u16 = 146;
const CLOSED: u16 = 147;
const OPEN: u16 = 148;
const OPENED: u16 = 149;
/// The crate's **lid** bone — the only bone the door family moves (bones 0/1 are the static body,
/// 9.. the destruction shards). Read off `benilla-extract m2bones`.
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

    // 1. The whole door family is authored, so no remap leg (`0x5f3972`) is reachable: the arm
    //    plays the LUT id directly and the test below is about the *transition*, nothing else.
    let lookup = parse_m2_animation_lookup(&bytes).expect("animation lookup");
    for id in [CLOSE, CLOSED, OPEN, OPENED] {
        assert!(
            lookup.get(id as usize).is_some_and(|&s| s != 0xffff),
            "the crate owns animation id {id}"
        );
    }

    // 2. **Every** one of the four has `flags` bit 0 clear — the kernel loops them all, rest poses
    //    and motions alike. So "does this clip loop?" cannot be the question a transition asks:
    //    reading the bit here arms the Close sweep for ever, which IS the report.
    for id in [CLOSE, CLOSED, OPEN, OPENED] {
        assert!(
            seq(id).looping,
            "id {id} is a bit-0-clear band — the kernel wraps it, so only the object layer's \
             completion advance can end a transition"
        );
    }

    // 3. …and the window that advance fires at is exactly ONE pass: `R = max(1, min + roll)` with
    //    an empty replay range rolls to 1 (`0x712692..0x7126cd`). One window is the
    //    whole swing — there is no second pass to model.
    for id in [CLOSE, CLOSED, OPEN, OPENED] {
        let s = seq(id);
        assert_eq!(
            (s.min_replay, s.max_replay),
            (0, 0),
            "id {id} rolls R = 1, so the completion lands at one band length"
        );
    }

    // 4. The poses themselves, as the viewer sees them. The lid's rotation is a pure −Y quaternion:
    //    |y| ≈ 0 is shut, |y| ≈ 0.61 (75°) is fully open.
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

    // Close sweeps open → shut. Looping THAT is a lid that springs to 75° and slams, 1.5×/s.
    let (lo, hi) = span(&lid(CLOSE));
    assert!(
        lo < 1e-3 && hi > 0.5,
        "Close must sweep from open (|y| {hi}) to shut (|y| {lo}) — that sweep, looped, is the \
         reported bug"
    );
    // Open is the mirror sweep — the other half of the cycle the loot window straddles.
    let (lo, hi) = span(&lid(OPEN));
    assert!(
        lo < 1e-3 && hi > 0.5,
        "Open must sweep from shut (|y| {lo}) to open (|y| {hi})"
    );
    // The rest poses genuinely rest, and each is the pose its motion lands on — which is why
    // holding a finished motion's last frame reads correctly, and why the `0x5f4120` advance onto
    // the rest sequence is invisible rather than a pop.
    for q in lid(CLOSED) {
        assert!(q[1].abs() < 1e-3, "Closed holds the SHUT lid, got {}", q[1]);
    }
    for q in lid(OPENED) {
        assert!(q[1].abs() > 0.5, "Opened holds the OPEN lid, got {}", q[1]);
    }
}
