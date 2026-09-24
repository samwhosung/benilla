//! The M2 particle-emitter parser against shipped models.

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

    // Two additive plane emitters, a slow glow plume and a fast 4×4-flipbook flame, off the file.
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
        assert!(
            e.texture.as_deref().is_some_and(|t| !t.is_empty()),
            "emitter texture resolves, got {:?}",
            e.texture
        );
    }

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

    // Drag, file `+0x194`, decays velocity as `vel -= min(dt·drag, 1)·vel`.
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

        for s in ol.scale {
            assert!(
                s.is_finite() && (0.0..8.0).contains(&s),
                "emitter {i} scale {s} sane"
            );
        }
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

    let glow_ol = &emitters[0].over_life;
    let a_start = glow_ol.sample(0.0).color[3];
    let a_end = glow_ol.sample(1.0).color[3];
    assert!(
        a_end <= a_start,
        "glow alpha should not rise over life ({a_start} -> {a_end})"
    );

    let flame_ol = &emitters[1].over_life;
    let cell_start = flame_ol.sample(0.0).head_cell;
    let cell_end = flame_ol.sample(1.0).head_cell;
    assert!(
        cell_end >= cell_start,
        "flame cell advances ({cell_start} -> {cell_end})"
    );
}

/// The flipbook cell ramp against the reference's output (`0x7b9da0` builds it, `0x7b9b10` samples
/// it): `cell(0) == begin` and `cell(1) == end` exactly, by the build's `±1` and the
/// `0.99·t + 0.005` inset together, and a decreasing pair plays backwards.
#[test]
fn cell_ramp_matches_the_reference_including_backwards() {
    let at = |begin: u16, end: u16| {
        let r = CellRamp::new(begin, end);
        // u = 0, ¼, ½, ¾ and 1, through the evaluator's inset.
        [0.0_f32, 0.25, 0.5, 0.75, 1.0]
            .map(|t| r.sample(t * 0.99 + 0.005))
            .to_vec()
    };

    // Forward, and the three inverted pairs the shipped corpus authors.
    assert_eq!(at(0, 15), vec![0, 4, 8, 11, 15], "ascending 0..15");
    assert_eq!(at(6, 5), vec![6, 6, 6, 5, 5], "DwarvenBrazier01 (6,5)");
    assert_eq!(
        at(31, 16),
        vec![31, 27, 24, 20, 16],
        "ShadowWordSilence (31,16)"
    );
    assert_eq!(at(15, 0), vec![15, 11, 8, 4, 0], "ShadowWordSilence (15,0)");

    for (b, e) in [(0, 15), (8, 16), (31, 16), (15, 0), (6, 5), (7, 7), (0, 63)] {
        let r = CellRamp::new(b, e);
        assert_eq!(r.sample(0.005), b, "cell(u=0) == begin for ({b},{e})");
        assert_eq!(r.sample(0.995), e, "cell(u=1) == end for ({b},{e})");
    }

    // Through `OverLife::sample`, where the inset is applied.
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

/// Colour and size ride the cells' inset: `0x7b9b10` stores `t·0.99 + 0.005` once and every
/// channel reloads it, so a particle starts 0.5% into its ramp and ends 0.5% short.
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
    // Segment A runs 0 to 100 over u in [0, 0.5].
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
    let end = ol.sample(0.5);
    assert!(
        (end.size - 99.5).abs() < 1e-3,
        "u=mid is 99.5 % along, not 100 %: got {}",
        end.size
    );
}

/// `DwarvenBrazier01`'s settling flame authors the inverted segment-B pair `(6, 5)`.
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

/// The per-segment flipbook repeat count (file `+0x16c`, `+0x172`) cycles the cell ramp by
/// `fmod(t·repeat, 1.0)`; the 18 shipped emitters that author one are the Insect Swarm visuals.
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

/// File `+0x188` and `+0x18c` are twinkleScale `{min, max}`, a per-frame size flicker skipped when
/// the range is degenerate (`0x7b2a50`), not a spawn-time multiplier: the kobold candle authors
/// `{0, 0}` and burns in the reference.
#[test]
fn twinkle_fields_gate_not_scale() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

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
    assert!(
        candle.over_life.sample(0.5).size > 0.0,
        "the candle flame's over-life size ramp is nonzero"
    );

    let campfire = parse_m2_particle_emitters(
        &chain
            .read_file("World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.m2")
            .expect("read ElwynnCampfire.m2"),
    )
    .expect("parse campfire emitters");
    let glow = &campfire[0];
    assert_eq!((glow.twinkle_min, glow.twinkle_max), (0.0, 1.0));
    assert_eq!(glow.twinkle(0.25), 0.25, "active range lerps min..max");
    assert!(glow.twinkle_percent.is_finite() && glow.twinkle_speed.is_finite());
}

/// The file-to-runtime flag remap (`0x70faf8`-`0x70fc44`): file bit 0x10 (runtime 0x100) is model
/// space, file 0x20 (runtime 0x200) scales size by the placement. The chandelier's flames (0x11,
/// 0x15) ride its swing; the campfire (0x21, 0x29) scales.
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

/// `G_BarrelExplode.m2` (GameObject 20737): all 7 emitters fire inside the one-shot clips (slot 0,
/// anim 157; slot 3, anim 150 Destroy) and are off in both idles (slot 1 Stand, slot 2 Closed).
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
        for slot in [1usize, 2] {
            for t in [0.0f32, 0.1, 0.3, 5.0] {
                assert!(
                    !e.timing.emitting(Some(slot), t, 0.0),
                    "emitter {i} must be OFF at rest (slot {slot}, t {t})"
                );
            }
        }
        // The explode clip, slot 0, runs 1 s and clamps.
        let fires = (0..100).any(|k| e.timing.emitting(Some(0), k as f32 * 0.01, 0.0));
        assert!(fires, "emitter {i} fires inside the explode clip");
        assert!(
            !e.timing.emitting(Some(0), 1.0, 0.0),
            "emitter {i} must be off at the clip end"
        );
        assert!(
            !e.timing.emitting(Some(0), 30.0, 0.0),
            "emitter {i} must stay off parked past the clip"
        );
    }
    let destroy_fires = emitters
        .iter()
        .any(|e| (0..100).any(|k| e.timing.emitting(Some(3), k as f32 * 0.01, 0.0)));
    assert!(destroy_fires, "the Destroy clip plays the explosion");
}

/// Particle `blendingType` 1 is AlphaKey, not Opaque: blending off under `glAlphaFunc(GEQUAL,
/// 224/255)`, which cuts `PARTROCK.BLP`'s silhouette. The earth elemental is the one creature that
/// authors it, and nothing in 1.12.1 authors mode 0.
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

    assert!(
        !emitters.iter().any(|e| e.blend == ParticleBlend::Opaque),
        "no emitter here folds to Opaque"
    );
}
