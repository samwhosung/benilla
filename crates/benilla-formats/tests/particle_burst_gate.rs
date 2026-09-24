//! A burst emitter fires on the rising edge of `enabled != 0 && sampledRate > 0`, both sampled on
//! one clock in one frame (`0x718ed2`-`0x718ef6`). Of the warrior impact flare's two (kit 437),
//! #0's enabled track steps to 0 on the key its rate steps to 50, so only #1 ever fires.

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
