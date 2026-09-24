//! The cold-breath puff: `HumanMale.m2` keys a `$BTH` event on Stand, so an idle player fires one
//! every loop; its attach tag `0x11` (AttachmentID 17) is a bone at the mouth; and the puff model
//! is one-shot, so the end-of-clip terminator is its lifetime.

use benilla_formats::{open_chain, parse_m2_animations, parse_m2_attachments};

const HUMAN_MALE: &str = "Character\\Human\\Male\\HumanMale.m2";
/// `SpellVisualEffectName` 107, "HARDCODED Breath Cold", names `Particles\ColdBreath.mdl`, which
/// ships as `.m2`.
const COLD_BREATH: &str = "Particles\\ColdBreath.m2";
/// The `$BTH` family's attach tag, `DAT_0080c968[3]`: a raw M2 `AttachmentID`.
const BREATH_ATTACH: u16 = 0x11;
const STAND: u16 = 0;

#[test]
fn the_idle_player_keys_bth_and_attaches_it_at_the_mouth() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file(HUMAN_MALE)
        .expect("HumanMale is in the chain");

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
    // 0.667 s into a 2.667 s loop, longer than the 1.5 s puff, so puffs never overlap.
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

    // The tag is an attachment id, never an index, resolved through the AttachLookup (`0x710310`).
    let attachments = parse_m2_attachments(&bytes).expect("parse attachments");
    let breath = attachments
        .iter()
        .find(|a| a.id == BREATH_ATTACH)
        .expect("HumanMale authors attachment 0x11");
    // The mouth sits just below and forward of the head attachment, id 11.
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
