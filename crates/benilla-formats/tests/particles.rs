//! Difftest the vanilla M2 particle-emitter parser against the real `ElwynnCampfire.m2` (the
//! Goldshire-area campfire). Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{
    open_chain, parse_m2_particle_emitters, CellRamp, OverLife, ParticleBlend, ParticleShape,
};

#[test]
fn campfire_emitters_match_real_bytes() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.m2")
        .expect("read ElwynnCampfire.m2");

    let emitters = parse_m2_particle_emitters(&bytes).expect("parse emitters");

    // The campfire has exactly two additive plane emitters: a wide slow glow/smoke plume and a fast
    // narrow flame with a 4×4 cell flicker. These values are read straight off the real file.
    assert_eq!(emitters.len(), 2, "campfire has two emitters");

    for e in &emitters {
        assert_eq!(e.shape, ParticleShape::Plane);
        assert_eq!(e.blend, ParticleBlend::Add);
        let now = e.params.sample(None, 0.0, 0.0);
        assert!(now.lifespan > 0.0 && now.lifespan.is_finite());
        let rate = e
            .timing
            .constant_rate()
            .expect("ambient prop rates are constant tracks");
        assert!(rate > 0.0 && rate.is_finite());
        assert!(now.horizontal_range > 6.0, "campfire emits in a full ring");
        // Texture resolves to a real .blp via the M2 textures table.
        assert!(
            e.texture.as_deref().is_some_and(|t| !t.is_empty()),
            "emitter texture resolves, got {:?}",
            e.texture
        );
    }

    // Glow/smoke plume: wide 20° cone, long life, low rate, single cell.
    let glow = &emitters[0];
    let glow_now = glow.params.sample(None, 0.0, 0.0);
    assert!(
        (glow_now.lifespan - 4.0).abs() < 1e-3,
        "glow lifespan ~4.0, got {}",
        glow_now.lifespan
    );
    assert!(
        (glow.timing.constant_rate().unwrap() - 6.0).abs() < 1e-3,
        "glow rate ~6, got {:?}",
        glow.timing.constant_rate()
    );
    assert_eq!((glow.tile_rows, glow.tile_cols), (1, 1));
    assert!(glow_now.vertical_range > 0.3, "glow has a wide cone");

    // Flame: short life, high rate, narrow cone, 4×4 cell animation.
    let flame = &emitters[1];
    let flame_now = flame.params.sample(None, 0.0, 0.0);
    assert!(
        (flame_now.lifespan - 1.5).abs() < 1e-3,
        "flame lifespan ~1.5, got {}",
        flame_now.lifespan
    );
    assert!(
        (flame.timing.constant_rate().unwrap() - 20.0).abs() < 1e-3,
        "flame rate ~20, got {:?}",
        flame.timing.constant_rate()
    );
    assert_eq!(
        (flame.tile_rows, flame.tile_cols),
        (4, 4),
        "flame has a 4×4 flicker atlas"
    );
    assert!(
        flame_now.vertical_range < 0.2,
        "flame is a tight upward jet"
    );

    // Drag (file +0x194): the velocity-decay term the verified integrator applies as
    // `vel −= min(dt·drag, 1)·vel`. The campfire's smoke/glow plume carries a gentle 0.5 (contained
    // column); its short-lived flame carries 0.0 (a free upward jet). Read straight off the real
    // bytes — the candelabra props instead author a strong 10.0 and rely on it to stay a flicker.
    assert!(
        (glow.drag - 0.5).abs() < 1e-3,
        "campfire glow drag ~0.5, got {}",
        glow.drag
    );
    assert!(
        flame.drag == 0.0,
        "campfire flame drag 0, got {}",
        flame.drag
    );

    // Over-life ramps (verified tail). Dump the sampled color/size/cell across life, and assert the
    // believability invariants: a fading additive weight (A) and a sensible size in yards.
    eprintln!("campfire emitters OK:");
    for (i, e) in emitters.iter().enumerate() {
        let ol = &e.over_life;
        eprintln!(
            "  [{i}] {:?} {:?} tex={:?} life={} rate={:?} cone={:.3} tiles={}x{}",
            e.shape,
            e.blend,
            e.texture,
            e.params.sample(None, 0.0, 0.0).lifespan,
            e.timing.constant_rate(),
            e.params.sample(None, 0.0, 0.0).vertical_range,
            e.tile_rows,
            e.tile_cols
        );
        eprintln!(
            "      mid={:.2} color={:?} scale={:?} head{:?} tail{:?} repeat{:?}",
            ol.mid, ol.color, ol.scale, ol.head_cells, ol.tail_cells, ol.repeat
        );
        for u in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let s = ol.sample(u);
            eprintln!(
                "      u={u:.2}: rgba={:?} size={:.3} cell head={} tail={}",
                s.color, s.size, s.head_cell, s.tail_cell
            );
        }

        // Sizes are finite, non-negative, and small (campfire props are ~sub-yard to a couple yards).
        for s in ol.scale {
            assert!(
                s.is_finite() && (0.0..8.0).contains(&s),
                "emitter {i} scale {s} sane"
            );
        }
        // Color/alpha keys are valid 0..1.
        for k in ol.color {
            for ch in k {
                assert!(
                    (0.0..=1.0).contains(&ch),
                    "emitter {i} color channel {ch} in 0..1"
                );
            }
        }
        assert!(
            (0.0..=1.0).contains(&ol.mid),
            "emitter {i} midPoint in 0..1"
        );
    }

    // The glow/smoke plume fades out: its additive weight (alpha) at end-of-life is below its peak.
    let glow_ol = &emitters[0].over_life;
    let a_start = glow_ol.sample(0.0).color[3];
    let a_end = glow_ol.sample(1.0).color[3];
    assert!(
        a_end <= a_start,
        "glow alpha should not rise over life ({a_start} -> {a_end})"
    );

    // The flame's cell index advances across its 4×4 atlas over life (the flicker animation).
    let flame_ol = &emitters[1].over_life;
    let cell_start = flame_ol.sample(0.0).head_cell;
    let cell_end = flame_ol.sample(1.0).head_cell;
    assert!(
        cell_end >= cell_start,
        "flame cell advances ({cell_start} -> {cell_end})"
    );
}

/// The flipbook **cell ramp**, against the reference's own emulated output (`0x7b9da0` builds the
/// record, `0x7b9b10` samples it). Two properties, and the second is the one that crashed us:
///
/// 1. **The endpoint law** — `cell(0) == begin` and `cell(1) == end`, EXACTLY, in both directions.
///    That is what the `±1` in the build arms and the evaluator's `0.99·t + 0.005` inset exist to
///    buy; either one alone is off by one at an endpoint.
/// 2. **A DECREASING pair plays the flipbook backwards** — it is not swapped, not clamped, not
///    sign-normalized. Shipped data authors it, so a `clamp(begin, end)` here is both wrong and
///    (in Rust) a panic.
#[test]
fn cell_ramp_matches_the_reference_including_backwards() {
    let at = |begin: u16, end: u16| {
        let r = CellRamp::new(begin, end);
        // The sample points, through the same inset the evaluator
        // applies (u = 0, ¼, ½, ¾, 1 of the segment).
        [0.0_f32, 0.25, 0.5, 0.75, 1.0]
            .map(|t| r.sample(t * 0.99 + 0.005))
            .to_vec()
    };

    // Forward, and the three inverted pairs the shipped corpus actually authors.
    assert_eq!(at(0, 15), vec![0, 4, 8, 11, 15], "ascending 0..15");
    assert_eq!(at(6, 5), vec![6, 6, 6, 5, 5], "DwarvenBrazier01 (6,5)");
    assert_eq!(
        at(31, 16),
        vec![31, 27, 24, 20, 16],
        "ShadowWordSilence (31,16)"
    );
    assert_eq!(at(15, 0), vec![15, 11, 8, 4, 0], "ShadowWordSilence (15,0)");

    // The endpoint law over every direction and the degenerate equal pair.
    for (b, e) in [(0, 15), (8, 16), (31, 16), (15, 0), (6, 5), (7, 7), (0, 63)] {
        let r = CellRamp::new(b, e);
        assert_eq!(r.sample(0.005), b, "cell(u=0) == begin for ({b},{e})");
        assert_eq!(r.sample(0.995), e, "cell(u=1) == end for ({b},{e})");
    }

    // …and END-TO-END through `OverLife::sample`, which is where the inset is actually applied.
    // Asserting the law on `CellRamp` alone passes even with the inset deleted (it feeds one in
    // itself) — a mutation check caught exactly that hole. The reference has ONE normalized time
    // and every consumer reloads it, so the endpoints must land on a whole-life sample too.
    let ol = OverLife {
        mid: 0.5,
        color: [[1.0; 4]; 3],
        scale: [1.0; 3],
        head_cells: [CellRamp::new(0, 7), CellRamp::new(31, 16)],
        tail_cells: [CellRamp::new(3, 3), CellRamp::new(9, 4)],
        repeat: [1.0; 2],
    };
    assert_eq!(ol.sample(0.0).head_cell, 0, "u=0 sits on segment A's begin");
    assert_eq!(ol.sample(1.0).head_cell, 16, "u=1 sits on segment B's end");
    assert_eq!(ol.sample(0.0).tail_cell, 3, "the tail ramp is sampled too");
    assert_eq!(ol.sample(1.0).tail_cell, 4, "…and backwards, independently");
    // The split is inclusive toward A (`age > lifespan·mid` is the only way into B).
    assert_eq!(
        ol.sample(0.5).head_cell,
        7,
        "u==mid is still segment A, at its end"
    );
}

/// Colour and size ride the **same inset** as the cells — `0x7b9b10` computes `t·0.99 + 0.005`
/// once, into its own `age` slot, and all four colour channels and the size reload that slot.
/// So a particle never sits exactly on an authored key:
/// it starts 0.5 % into the ramp and ends 0.5 % short. Lerping on a raw `t` is wrong at both ends
/// of both segments — a small error, but a systematic one, and free to get right.
#[test]
fn colour_and_size_ride_the_same_inset() {
    let ol = OverLife {
        mid: 0.5,
        color: [[0.0; 4], [1.0; 4], [1.0; 4]],
        scale: [0.0, 100.0, 100.0],
        head_cells: [CellRamp::new(0, 0); 2],
        tail_cells: [CellRamp::new(0, 0); 2],
        repeat: [1.0; 2],
    };
    // Segment A runs 0 → 100 over u ∈ [0, 0.5]; at u=0 the inset puts us 0.5 % along it.
    let start = ol.sample(0.0);
    assert!(
        (start.size - 0.5).abs() < 1e-4,
        "u=0 is 0.5 % into the ramp, not on the key: got {}",
        start.size
    );
    assert!(
        (start.color[0] - 0.005).abs() < 1e-4,
        "colour takes the same inset: got {}",
        start.color[0]
    );
    // …and 99.5 % of the way at the segment's end, not 100 %.
    let end = ol.sample(0.5);
    assert!(
        (end.size - 99.5).abs() < 1e-3,
        "u=mid is 99.5 % along, not 100 %: got {}",
        end.size
    );
}

/// The crash reported from Winterspring: `DwarvenBrazier01`'s settling flame authors the
/// inverted segment-B pair `(6,5)`, which the old `idx.clamp(begin, end)` met with a panic —
/// `clamp` requires `min <= max`. Sampling the real record across life must simply work.
#[test]
fn inverted_ramp_on_real_data_does_not_panic() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("World\\Generic\\Dwarf\\Passive Doodads\\Braziers\\DwarvenBrazier01.m2")
        .expect("read DwarvenBrazier01.m2");
    let emitters = parse_m2_particle_emitters(&bytes).expect("parse emitters");
    let ol = &emitters[1].over_life;
    assert_eq!(
        (ol.head_cells[0].begin, ol.head_cells[0].end),
        (0, 5),
        "segment A runs forward"
    );
    assert_eq!(
        (ol.head_cells[1].begin, ol.head_cells[1].end),
        (6, 5),
        "segment B is the INVERTED pair — the whole point of this test"
    );
    // Every point of the particle's life, at the granularity a 60 Hz frame would visit.
    for i in 0..=1000 {
        let s = ol.sample(i as f32 / 1000.0);
        assert!(s.head_cell <= 6, "cell stays inside the authored pair");
    }
}

/// The per-segment flipbook **repeat count** (file +0x16c/+0x172): `fmod(t·repeat, 1.0)` cycles the
/// cell ramp that many times across the segment. 18 shipped emitters author one — all of them the
/// druid Insect Swarm visuals, whose insects flap by cycling a 5- or 8-pass flipbook. A reader that
/// ignores the field runs the sheet exactly once and the swarm never flaps.
#[test]
fn repeat_count_cycles_the_flipbook() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("SPELLS\\InsectSwarm_State_Chest.m2")
        .expect("read InsectSwarm_State_Chest.m2");
    let emitters = parse_m2_particle_emitters(&bytes).expect("parse emitters");
    let ol = &emitters[0].over_life;
    assert_eq!(ol.repeat, [5.0, 5.0], "the swarm authors a 5× cycle");

    // Five passes across segment A means the cell index resets five times: count the drops.
    let mid = ol.mid;
    let mut drops = 0;
    let mut prev = ol.sample(0.0).head_cell;
    for i in 1..=2000 {
        let c = ol.sample(mid * (i as f32 / 2000.0)).head_cell;
        if c < prev {
            drops += 1;
        }
        prev = c;
    }
    assert_eq!(
        drops, 4,
        "5 passes over the segment == 4 wraps, got {drops}"
    );
}

/// The record-tail **twinkle** fields (`0x7b2a50`): file +0x188/+0x18c are twinkleScale
/// **{min, max}** — a GATED per-frame size flicker, skipped
/// when the range is degenerate — NOT a spawn-time size multiplier. The discriminating real-data
/// case is the kobold candle: it authors `{0, 0}` and burns in the reference client, which the old
/// `base + rand·variation` reading collapsed to size zero (the director's "candles not burning").
#[test]
fn twinkle_fields_gate_not_scale() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // Kobold candle: twinkle {0,0} — degenerate range, the multiplier must be identity.
    let kobold = parse_m2_particle_emitters(
        &chain
            .read_file("Creature\\Kobold\\Kobold.m2")
            .expect("read Kobold.m2"),
    )
    .expect("parse kobold emitters");
    assert_eq!(kobold.len(), 1, "kobold has one candle emitter");
    let candle = &kobold[0];
    assert_eq!((candle.twinkle_min, candle.twinkle_max), (0.0, 0.0));
    assert_eq!(
        candle.twinkle(0.7),
        1.0,
        "degenerate {{0,0}} twinkle is identity — the candle burns at ramp size"
    );
    // Its base size is the over-life ramp alone — nonzero at mid-life.
    assert!(
        candle.over_life.sample(0.5).size > 0.0,
        "the candle flame's over-life size ramp is nonzero"
    );

    // Campfire glow plume: twinkle {0, 1} — an active flicker range; samples lerp min..max.
    let campfire = parse_m2_particle_emitters(
        &chain
            .read_file("World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.m2")
            .expect("read ElwynnCampfire.m2"),
    )
    .expect("parse campfire emitters");
    let glow = &campfire[0];
    assert_eq!((glow.twinkle_min, glow.twinkle_max), (0.0, 1.0));
    assert_eq!(glow.twinkle(0.25), 0.25, "active range lerps min..max");
    // A degenerate NON-ZERO range is also identity ({1,1} torches burn steady, not 1–2× inflated).
    assert!(glow.twinkle_percent.is_finite() && glow.twinkle_speed.is_finite());
}

/// The file→runtime flag remap (`0x70faf8`–`0x70fc44`): the space switch is FILE bit 0x10
/// (→ rt 0x100), the size-by-scale
/// enable FILE 0x20 (→ rt 0x200) — pinned on real content whose behavior the reference shows:
/// the kobold candle (0x01) is carried with no trail and un-flagged for both; the swinging
/// chandelier's candle flames (0x11/0x15) are model-space (they rigidly ride the swing); the
/// campfire (0x21/0x29) scales its flame size with the placement.
#[test]
fn flag_remap_reads_the_file_bits() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    let kobold =
        parse_m2_particle_emitters(&chain.read_file("Creature\\Kobold\\Kobold.m2").unwrap())
            .unwrap();
    assert_eq!(kobold[0].flags, 0x01);
    assert!(!kobold[0].model_space() && !kobold[0].scale_size_by_instance());

    let chandelier = parse_m2_particle_emitters(
        &chain
            .read_file("World\\Dungeon\\GoldshireInn\\InnChandelier\\InnChandelier.m2")
            .unwrap(),
    )
    .unwrap();
    assert!(
        chandelier.iter().take(6).all(|e| e.model_space()),
        "the swinging candle flames are model-space (file bit 0x10)"
    );

    let campfire = parse_m2_particle_emitters(
        &chain
            .read_file("World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.m2")
            .unwrap(),
    )
    .unwrap();
    assert!(
        campfire
            .iter()
            .all(|e| e.scale_size_by_instance() && !e.model_space()),
        "campfire: size-by-scale (0x20) set, model-space (0x10) clear"
    );
}

/// **The B27 pin, at the real bytes** — `G_BarrelExplode.m2` (GameObject 20737, "Keenly
/// Disguised Barrel"): all 7 emitters author their explosion inside the one-shot clips
/// (slot 0 = anim 157, slot 3 = anim 150 Destroy) and an OFF window in both idle sequences
/// (slot 1 = Stand, slot 2 = anim 147 Closed). The reference shows a quiet barrel at rest; the
/// old seq-0-only rebase parked the clamped explode clock at its end value and the barrel
/// burned permanently at 748 live particles.
#[test]
fn barrel_explode_emitters_are_off_at_rest_and_fire_in_their_clips() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let emitters = parse_m2_particle_emitters(
        &chain
            .read_file("World\\Goober\\G_BarrelExplode.m2")
            .expect("read G_BarrelExplode.m2"),
    )
    .expect("parse emitters");
    assert_eq!(emitters.len(), 7, "the barrel authors seven emitters");
    for (i, e) in emitters.iter().enumerate() {
        assert!(e.timing.peak_rate() > 0.0, "emitter {i} can emit");
        // OFF through both idle windows — Stand (slot 1) and Closed (slot 2), at any time.
        for slot in [1usize, 2] {
            for t in [0.0f32, 0.1, 0.3, 5.0] {
                assert!(
                    !e.timing.emitting(Some(slot), t, 0.0),
                    "emitter {i} must be OFF at rest (slot {slot}, t {t})"
                );
            }
        }
        // The one-shot explode clip (slot 0, anim 157, 1 s clamped): the choreography fires
        // somewhere inside it…
        let fires = (0..100).any(|k| e.timing.emitting(Some(0), k as f32 * 0.01, 0.0));
        assert!(fires, "emitter {i} fires inside the explode clip");
        // …and the clamped clock PARKS OFF at/after the clip end — the exact end state the old
        // rebase got wrong (it clamped a later clip's ON key onto the band end).
        assert!(
            !e.timing.emitting(Some(0), 1.0, 0.0),
            "emitter {i} must be off at the clip end"
        );
        assert!(
            !e.timing.emitting(Some(0), 30.0, 0.0),
            "emitter {i} must stay off parked past the clip"
        );
    }
    // The Destroy clip (slot 3, anim 150) fires the explosion too — the quest's actual payoff.
    let destroy_fires = emitters
        .iter()
        .any(|e| (0..100).any(|k| e.timing.emitting(Some(3), k as f32 * 0.01, 0.0)));
    assert!(destroy_fires, "the Destroy clip plays the explosion");
}

/// The debris family's blend: file `blendingType` **1 is AlphaKey**, not Opaque — the fold that
/// painted the Lesser Rock Elemental's rock chips as flat pale squares.
///
/// Pinned on the elemental because it is the one CREATURE in the corpus that authors mode 1 (a
/// `partcensus` sweep finds 26 such emitters in 8 models, the other 7 all `Spells\`). The
/// distinction is load-bearing: mode 1 draws with blending OFF but under `glAlphaFunc(GEQUAL,
/// 224/255)`, which carves `PARTROCK.BLP`'s silhouette out of the quad; mode 0 has no alpha test
/// at all, and nothing in 1.12.1 authors it.
#[test]
fn earth_elemental_debris_is_alphakey_not_opaque() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("Creature\\ElementalEarth\\ElementalEarth.m2")
        .expect("read ElementalEarth.m2");

    let emitters = parse_m2_particle_emitters(&bytes).expect("parse emitters");
    assert_eq!(emitters.len(), 13, "the elemental authors 13 emitters");

    // Emitters 3 and 12 are the rock chips; 11 is the third PARTROCK emitter, authored mode 2.
    let rock = |e: &benilla_formats::ParticleEmitterDef| {
        e.texture
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("CREATURE\\ELEMENTALEARTH\\PARTROCK.BLP"))
    };
    assert!(rock(&emitters[3]) && rock(&emitters[11]) && rock(&emitters[12]));
    assert_eq!(emitters[3].blend, ParticleBlend::AlphaKey);
    assert_eq!(emitters[12].blend, ParticleBlend::AlphaKey);
    assert_eq!(emitters[11].blend, ParticleBlend::Alpha);

    // Nothing on this model — nor anywhere in the shipped corpus — is mode 0.
    assert!(
        !emitters.iter().any(|e| e.blend == ParticleBlend::Opaque),
        "no emitter here folds to Opaque"
    );
}
