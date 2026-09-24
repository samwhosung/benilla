//! The **burst gate**, pinned against a shipped asset whose gate never opens.
//!
//! A burst emitter fires on the rising edge of `enabled != 0 && sampledRate > 0`, both tracks
//! sampled from the same clock in the same frame (`0x718ed2`–`0x718ef6`).
//! `Spells\Strike_Impact_Chest.m2` — the gold flare every warrior ability
//! impact plays (kit 437 → `SpellVisualEffectName` 416, attached at the target's chest `0x22`) —
//! carries two burst emitters, and **only one of them ever emits**:
//!
//! - #0 (`WEAPON\FLARE.BLP`): the enabled track steps `1 → 0` at the exact keyframe the rate track
//!   steps `0 → 50`. The two conditions are never true together, so it fires nothing, ever.
//! - #1 (`FirePlume64.blp`): gate held ON by a single key, rate `0 → 30 → 0`. It fires once.
//!
//! This is here because the *instrument* got it wrong first: the `m2part` dump derived a burst
//! count from `peak_rate()` alone and reported "burst of ~50 particles" for #0, which put a
//! diagnosis of "our impact bursts are too big" 2.7x over on particle count before anyone noticed.
//! An emitter that never fires is a real shipped shape, not a parse failure, and the reader must
//! be able to see the difference.
//!
//! Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{open_chain, parse_m2_particle_emitters};

const STRIKE_IMPACT: &str = "Spells\\Strike_Impact_Chest.m2";

#[test]
fn the_flare_emitter_never_fires_and_the_plume_fires_once() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file(STRIKE_IMPACT)
        .expect("read the impact model");

    let emitters = parse_m2_particle_emitters(&bytes).expect("parse emitters");
    assert_eq!(emitters.len(), 2, "two emitters on the impact flash");
    for e in &emitters {
        assert!(
            e.burst(),
            "both carry the burst flag (shipped flags 0x8429)"
        );
    }

    assert_eq!(
        emitters[0].timing.first_burst(Some(0)),
        None,
        "the flare's gate closes on the same keyframe its rate opens — it emits nothing",
    );
    assert!(
        emitters[0].timing.peak_rate() > 0.0,
        "and its peak rate is nonzero, which is exactly why peak_rate is not a particle count",
    );

    let (t, n) = emitters[1]
        .timing
        .first_burst(Some(0))
        .expect("the plume does fire");
    assert_eq!(n, 30.0, "one burst of 30 particles");
    assert!(
        (0.0..=0.067).contains(&t),
        "fired within the first two frames of the clip, got t={t}",
    );
}
