//! The material animation shapes the unit, GameObject and held-item lane serves, each on the asset
//! that needs it.

use benilla_formats::{open_chain, parse_m2_render_submeshes};

/// Every wind elemental is this model, and 26 of its 32 batches slide a full sheet on a global
/// sequence. The reference clocks a global sequence on one per-scene ms cursor, so a shared
/// material is exact.
#[test]
fn a_creature_scrolls_on_a_global_sequence() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("Creature\\AirElemental\\AirElemental.m2")
        .expect("AirElemental is in the chain");
    let subs = parse_m2_render_submeshes(&bytes, "Creature\\AirElemental", &[]).expect("parse");

    let live: Vec<_> = subs
        .iter()
        .filter_map(|s| s.uv_anim.as_ref())
        .filter(|a| a.period > 0.0)
        .collect();
    assert_eq!(live.len(), 26, "the banded shell's animating batches");
    assert!(
        live.iter().all(|a| a.gseq),
        "every one rides a GLOBAL sequence — no host, no play head, a shared clock is faithful"
    );
    for a in &live {
        let (lo, hi) = a
            .keys
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), (_, v)| {
                (lo.min(v[0]), hi.max(v[0]))
            });
        // To the keys' quantization: one batch ends at 1.0017.
        assert!(
            (hi - lo - 1.0).abs() < 5e-3,
            "a whole sheet of U per loop: [{lo}, {hi}]"
        );
    }
    // Nothing is per-sequence, so the shared material serves.
    assert!(subs.iter().all(|s| s.uv_seq.is_none()));
}

/// The corpus's two batches that rotate and never translate, both on GameObjects: `uv_anim` alone
/// misses them.
#[test]
fn a_gameobject_batch_rotates_with_no_translation_at_all() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    for (path, batches) in [
        ("World\\Goober\\G_ScryingBowl.m2", 1usize),
        ("World\\Goober\\G_ScourgeRuneCircleCrystal.m2", 1),
    ] {
        let bytes = chain.read_file(path).expect("in the chain");
        let dir = path.rsplit_once('\\').expect("a directory").0;
        let subs = parse_m2_render_submeshes(&bytes, dir, &[]).expect("parse");
        let turning: Vec<_> = subs.iter().filter(|s| s.uv_rot_seq.is_some()).collect();
        assert_eq!(turning.len(), batches, "{path}: the rotating batches");
        for s in &turning {
            assert!(
                s.uv_anim.is_none() && s.uv_seq.is_none(),
                "{path}: no translation channel at all — the obvious test answers None"
            );
            assert!(s.uv_scale_seq.is_none(), "{path}: and no scaling either");
            let rot = s.uv_rot_seq.as_ref().expect("checked");
            let l = rot.seq(None).expect("slot 0 animates");
            assert!(l.period > 0.0 && l.keys.len() > 1, "{path}: a live turn");
        }
    }
}

/// `BloodOfHeroes` authors its five bubble sheets in two slots of one animation id, slot 0 still
/// and slot 1 sweeping, so each instance needs its own material reading the slot it plays.
#[test]
fn a_gameobjects_slots_bake_different_loops() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let path = "World\\Lordaeron\\Plagueland\\PassiveDoodads\\BloodOfHeroes\\BloodOfHeroes.m2";
    let bytes = chain.read_file(path).expect("in the chain");
    let dir = path.rsplit_once('\\').expect("a directory").0;
    let subs = parse_m2_render_submeshes(&bytes, dir, &[]).expect("parse");

    let split: Vec<_> = subs.iter().filter(|s| s.uv_seq.is_some()).collect();
    assert_eq!(split.len(), 5, "the bubble sheets");
    for s in &split {
        assert!(
            s.uv_anim.is_none(),
            "no single loop serves both slots — which is why the shared lane registers nothing \
             here and the bubbles never moved"
        );
        let seqs = s.uv_seq.as_ref().expect("checked");
        let slots = seqs.slots();
        assert_eq!(slots.len(), 2, "two file sequence slots");
        assert!(slots[0].is_none(), "slot 0 holds the sheet still");
        let live = slots[1].as_ref().expect("slot 1 sweeps it");
        let hi = live.keys.iter().fold(f32::MIN, |hi, (_, v)| hi.max(v[1]));
        assert!(
            (live.period - 3.334).abs() < 1e-2 && (hi - 0.605).abs() < 1e-2,
            "slot 1 sweeps v to {hi} over {}s",
            live.period
        );
    }
    assert_eq!(
        subs.iter().filter(|s| s.uv_seq.is_none()).count(),
        1,
        "the pool draws with no transform, and always did"
    );
}

/// Slot 1 plays as often as the variation walk lands on it. A GameObject owning none of the
/// door-family ids re-arms Stand every window with a fresh roll (`0x5f3a52`, `0x5f4167`), so the
/// two frequencies are the duty cycle: `roll < 16384` is still, `16384..=32766` bubbles, and the
/// leftover 32767 exhausts the chain back to the head.
#[test]
fn the_bubbling_take_is_half_the_pools_duty_cycle() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("World\\Lordaeron\\Plagueland\\PassiveDoodads\\BloodOfHeroes\\BloodOfHeroes.m2")
        .expect("in the chain");
    let seqs = benilla_formats::parse_m2_animations(&bytes);

    assert_eq!(
        seqs.len(),
        2,
        "one two-take variation chain and nothing else"
    );
    assert!(
        seqs.iter().all(|s| s.anim_id == 0),
        "both takes are Stand — which is what the collapse-to-0 leg arms"
    );
    let freqs: Vec<u16> = seqs.iter().map(|s| s.frequency).collect();
    assert_eq!(
        freqs,
        vec![16_384, 16_383],
        "the still sheet and the bubbling one, at even odds"
    );
    // The walk: `roll < freq`, strict and unsigned, node by node, else `roll -= freq`.
    let bubbling = (0u32..32_768)
        .filter(|&roll| {
            let mut roll = roll;
            freqs.iter().position(|&f| {
                let win = roll < u32::from(f);
                roll = roll.saturating_sub(u32::from(f));
                win
            }) == Some(1)
        })
        .count();
    assert_eq!(
        bubbling, 16_383,
        "the bubbling take wins 16383 of 32768 draws — 49.997 %, re-rolled every 3.3 s window"
    );
}

/// Four of the corpus's five animated entity tints key nothing in slot 0, so `rgb_anim` is `None`:
/// the hunter's freezing trap pulses its glow card only on the `Custom0` clip the server plays.
#[test]
fn a_gameobjects_tint_is_keyed_in_a_later_slot_alone() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("World\\Goober\\G_FreezingTrap.m2")
        .expect("in the chain");
    let subs = parse_m2_render_submeshes(&bytes, "World\\Goober", &[]).expect("parse");

    let tinted: Vec<_> = subs.iter().filter(|s| s.rgb_seq.is_some()).collect();
    assert!(!tinted.is_empty(), "the glow card's tint is per-sequence");
    for s in &tinted {
        assert!(
            s.rgb_anim.is_none(),
            "slot 0 bakes nothing — the shared lane's seed is white, for ever"
        );
        let seqs = s.rgb_seq.as_ref().expect("checked");
        let slots = seqs.slots();
        assert!(slots[0].is_none(), "…which is exactly what slot 0 holds");
        let live = slots
            .iter()
            .enumerate()
            .find_map(|(i, l)| l.as_ref().map(|l| (i, l)));
        let (slot, l) = live.expect("some later slot keys the pulse");
        assert!(
            slot > 0 && l.period > 0.0 && l.keys.len() > 1,
            "slot {slot}"
        );
    }
}
