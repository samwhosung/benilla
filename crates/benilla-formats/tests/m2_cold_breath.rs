//! The cold-breath puff, pinned against the shipped assets.
//!
//! B233 was reported as a **player character in Dun Morogh** with no visible breath, and the fix
//! rests on three asset facts that a mechanism note alone cannot establish — the trap decision
//! 0705 names, where a verified mechanism turns out not to *apply* to the thing in front of you:
//!
//! 1. the trigger exists on the reported model — `HumanMale.m2` authors a **`$BTH` event on
//!    animation id 0 (Stand)**, so an idle player fires one every loop with no timer of ours;
//! 2. the attach tag the client passes, **`0x11` = AttachmentID 17**, resolves on that model to a
//!    bone at the **mouth**, not the base — the difference between vapour at the face and a cloud
//!    at the feet;
//! 3. the asset the DBC names is a **one-shot emitter**, so the client's end-of-clip terminator is
//!    the right lifetime for it (`FxStage::OneShot`, as the mount poof uses).
//!
//! Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{open_chain, parse_m2_animations, parse_m2_attachments};

const HUMAN_MALE: &str = "Character\\Human\\Male\\HumanMale.m2";
/// `SpellVisualEffectName` row 107 (`"HARDCODED Breath Cold"`) names `Particles\ColdBreath.mdl`;
/// the loader's own extension swap makes that the shipped `.m2`.
const COLD_BREATH: &str = "Particles\\ColdBreath.m2";
/// The `$BTH` family's attach tag — `DAT_0080c968[3]`, a raw M2 `AttachmentID`.
const BREATH_ATTACH: u16 = 0x11;
/// `AnimationData.dbc` id 0 = Stand.
const STAND: u16 = 0;

#[test]
fn the_idle_player_keys_bth_and_attaches_it_at_the_mouth() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file(HUMAN_MALE)
        .expect("HumanMale is in the chain");

    // (1) The trigger, on the animation an idle player is actually in.
    let anims = parse_m2_animations(&bytes);
    let stand = anims
        .iter()
        .find(|a| a.anim_id == STAND)
        .expect("HumanMale authors Stand");
    let bth: Vec<f32> = stand
        .events
        .iter()
        .filter(|e| e.ident == *b"$BTH")
        .map(|e| e.time)
        .collect();
    assert_eq!(
        bth.len(),
        1,
        "Stand keys exactly one $BTH; got {bth:?} over a {:.3}s clip",
        stand.duration
    );
    // 0.667 s into a 2.667 s loop: the clip is comfortably longer than ColdBreath's own 1.5 s, so
    // consecutive puffs never overlap on an idling player and the overlap guard never bites.
    assert!(
        (bth[0] - 0.667).abs() < 0.01,
        "the $BTH key sits at 0.667s, got {:.3}s",
        bth[0]
    );
    assert!(
        stand.duration > 1.5,
        "Stand ({:.3}s) outlasts the 1.5s puff",
        stand.duration
    );

    // (2) The attach point: tag 0x11 through the model's OWN AttachLookup (the `0x710310`
    // indirection — the tag is an id, never an array index), landing at the mouth.
    let attachments = parse_m2_attachments(&bytes).expect("parse attachments");
    let breath = attachments
        .iter()
        .find(|a| a.id == BREATH_ATTACH)
        .expect("HumanMale authors attachment 0x11");
    // The head attach (id 11) is the neighbour that makes "mouth" mean something: the breath point
    // sits just below it and forward of it, on the face — not at the unit base (z ≈ 0).
    let head = attachments
        .iter()
        .find(|a| a.id == 11)
        .expect("HumanMale authors the head attachment");
    assert!(
        breath.position[2] > 1.5 && breath.position[2] < head.position[2],
        "breath z {:.3} sits on the face, below the head attach z {:.3}",
        breath.position[2],
        head.position[2]
    );
    assert!(
        breath.position[0] > head.position[0],
        "breath x {:.3} is forward of the head attach x {:.3} — out of the mouth",
        breath.position[0],
        head.position[0]
    );
    assert_ne!(
        breath.bone, 0,
        "the breath rides a real bone, not the model root"
    );

    // (3) The puff itself: a single non-looping sequence, so the end-of-clip terminator is the
    // right lifetime. (`Particles\Bubbles.m2`, the underwater sibling, LOOPS — which is why it
    // needs the client's replace-on-respawn dedup and this one does not.)
    let breath_bytes = chain
        .read_file(COLD_BREATH)
        .expect("ColdBreath.m2 is in the chain");
    let clips = parse_m2_animations(&breath_bytes);
    assert_eq!(clips.len(), 1, "ColdBreath authors one sequence");
    assert!(
        !clips[0].looping,
        "ColdBreath's sequence is one-shot (flags bit 0 set) — it ENDS, which is what lets the \
         client's end-of-clip terminator own its lifetime"
    );
    assert!(
        (clips[0].duration - 1.5).abs() < 0.01,
        "ColdBreath runs 1.5s, got {:.3}s",
        clips[0].duration
    );
}
