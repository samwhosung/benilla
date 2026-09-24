//! The **entity** corpus's material animation (decision 2295) — the four shapes the unit /
//! GameObject / held-item lane has to serve, each pinned on the asset that makes it unavoidable.
//!
//! Decision 0130 phase 3 built the texture transform for placed doodads and left every other lane
//! off it; 2282 put the spell effects on; this file is the entity lane's turn, and it exists for
//! the same reason 2282's does — *the premise that a lane needs no channel is a measurement, and an
//! unmeasured one is how a channel gets quietly dropped.* `benilla-extract entityuvscan` is the
//! census; these are the four rows of it a reader has to be able to check by hand.
//!
//! Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{open_chain, parse_m2_render_submeshes};

/// The whole entity corpus's widest case: **a creature, on a global sequence.** Every wind
/// elemental in the game is this model, and 26 of its 32 batches slide a full sheet across
/// themselves on a free-running clock — which is the one case where a shared material uniform is
/// not a compromise but exactly what the reference does (0136 choice 1: the reference clocks a
/// global-sequence loop on one per-scene ms cursor).
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
    // A full sheet per loop: the batches slide u by 1.0 over their period, which is what makes the
    // shell read as moving air rather than as a painted band.
    for a in &live {
        let (lo, hi) = a
            .keys
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), (_, v)| {
                (lo.min(v[0]), hi.max(v[0]))
            });
        // A whole sheet, to the key quantization the exporter left behind (one batch ends at
        // 1.0017): what matters is that a full repeat passes under the geometry each loop.
        assert!(
            (hi - lo - 1.0).abs() < 5e-3,
            "a whole sheet of U per loop: [{lo}, {hi}]"
        );
    }
    // Nothing here is per-sequence, so these batches keep the shared, deduped material.
    assert!(subs.iter().all(|s| s.uv_seq.is_none()));
}

/// **A batch that rotates and never translates.** Both of the corpus's two live on GameObjects,
/// and both answer `None` on the channel a reader checks first — so a lane whose predicate is
/// `uv_anim.is_some()`, or whose row carries only a translation, drops them without a trace. This
/// is `UvLoops::animates`' whole reason for existing, and 2019's affine row's.
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

/// **The slots disagree, so no shared material can be right.** `BloodOfHeroes` authors its five
/// bubble sheets twice over one animation id — file slot 0 holds them still, slot 1 sweeps them —
/// at `freq` 16384/16383, so its 114 placements bubble independently of each other. That is
/// 1408's population reached from the entity side: the instance needs a material of its own, and
/// the row has to read whichever slot the instance is playing.
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
    // The pool itself is a sixth batch with no transform at all — so what was missing was never
    // "the blood pool", it was the bubbling.
    assert_eq!(
        subs.iter().filter(|s| s.uv_seq.is_none()).count(),
        1,
        "the pool draws with no transform, and always did"
    );
}

/// **And slot 1 is REACHABLE — the other half of the same asset's story.** A per-sequence UV loop
/// only ever runs if the arm picks that sequence, so "slot 1 sweeps the sheet" is half a fact: the
/// bubbling is live exactly as often as the weighted variation walk lands on take 1.
///
/// The pool's two takes are one `Stand` (`AnimationData.dbc` id 0) chain — and a GameObject whose
/// model owns *none* of the door-family ids collapses its substate to Stand and re-arms it every
/// window, with a fresh `variationIdx = -1` roll each time (`0x5f3a52`, `0x5f4167`;
/// `crate::go_anim`'s rest arm). So these two frequencies are the duty cycle of every
/// blood pool in the Plaguelands: `roll < 16384` takes the still sheet, `16384..=32766` takes the
/// bubbling one, and the single leftover draw (`32767`) exhausts the chain back to the head — the
/// authored convention, frequencies summing to 32767 against 32768 outcomes.
///
/// It is pinned here rather than left to arithmetic because the failure it guards is silent: an
/// asset re-read that dropped `frequency` to 0, or a walk that took the head, would leave this
/// file's neighbour above passing unchanged while every pool in the game went still.
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
    // The walk is `roll < freq` (strict, unsigned) node by node, else `roll -= freq` — so the
    // counts below ARE the duty cycle, not an approximation of it.
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

/// **The same shape on the TINT channel**, and worse: four of the corpus's five animated entity
/// tints bake *nothing* in file slot 0, so `rgb_anim` is `None` for them and a shared material can
/// only ever seed white however faithfully it is ticked. `G_FreezingTrap` is the one with play
/// frequency — the hunter's trap, whose glow card is meant to pulse blue-white as it arms, on the
/// `Custom0` clip the server rings.
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
