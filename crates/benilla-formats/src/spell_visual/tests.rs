//! Tests for the spell-visual DBC family — the tables `super` loads, exercised against
//! synthetic WDBCs and against the real 5875 client data (those skip when it is absent).
//! Lifted out of `mod.rs` whole when that file outgrew its budget; two `#[cfg(test)]`
//! modules merged into this one file scope.

use super::*;

/// A minimal synthetic WDBC (20-byte header + fixed-width u32 records, no strings — both
/// tables are all-`u32`) — the same shape `benilla-dbc`'s own tests build (`items.rs`
/// reproduces it too), so this adapter is testable without a real client install.
fn build_wdbc(record_count: u32, field_count: u32, records: &[u8]) -> Vec<u8> {
    let record_size = field_count * 4;
    assert_eq!(records.len(), (record_count * record_size) as usize);
    let mut b = Vec::new();
    b.extend_from_slice(b"WDBC");
    b.extend_from_slice(&record_count.to_le_bytes());
    b.extend_from_slice(&field_count.to_le_bytes());
    b.extend_from_slice(&record_size.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // empty string block
    b.extend_from_slice(records);
    b
}

fn u32le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// One `SpellVisual` row: id, the five stage kits, then the missile block — field 7 (model
/// effect id) and field 9 (dest-attach ordinal) live, the rest zeroed to fill 16 fields.
fn spell_visual_row(
    id: u32,
    precast: u32,
    cast: u32,
    impact: u32,
    state: u32,
    channel: u32,
    missile_model: u32,
    missile_attach: u32,
) -> Vec<u8> {
    let mut rec = Vec::new();
    for v in [id, precast, cast, impact, state, channel] {
        rec.extend(u32le(v));
    }
    for f in 6..SPELL_VISUAL_FIELDS {
        rec.extend(u32le(match f {
            7 => missile_model,
            9 => missile_attach,
            _ => 0,
        }));
    }
    rec
}

/// One `SpellVisualKit` row: id, an unpinned field 1, anim (field 2), nine zeroed emitter
/// slots (3..11), the world-effect slot (field 12), sound (field 13), then zeroed trailing
/// columns to fill 35 fields.
fn spell_visual_kit_row(id: u32, anim: u32, sound: u32, world: u32) -> Vec<u8> {
    let mut rec = Vec::new();
    rec.extend(u32le(id));
    rec.extend(u32le(0)); // field 1, unpinned
    rec.extend(u32le(anim)); // field 2
    for _ in 3..12 {
        rec.extend(u32le(0)); // fields 3..11 (9 emitter slots)
    }
    rec.extend(u32le(world)); // field 12
    rec.extend(u32le(sound)); // field 13
    for _ in 14..SPELL_VISUAL_KIT_FIELDS {
        rec.extend(u32le(0));
    }
    rec
}

#[test]
fn header_shape_matches_the_pinned_layout() {
    // A single-row file of each still has to satisfy the pinned field/record-size shape —
    // exercises the schema's own field-count check against a header lying about it would be
    // a separate (missing) test; this one just guards our row builders stay in lock-step with
    // SPELL_VISUAL_FIELDS/SPELL_VISUAL_KIT_FIELDS.
    let sv = spell_visual_row(1, 0, 0, 0, 0, 0, 0, 0);
    assert_eq!(sv.len(), SPELL_VISUAL_FIELDS * 4, "64B record");
    let svk = spell_visual_kit_row(1, 0, 0, 0);
    assert_eq!(svk.len(), SPELL_VISUAL_KIT_FIELDS * 4, "140B record");
}

#[test]
fn parses_stage_kits_and_zero_means_no_kit_at_that_stage() {
    let mut records = Vec::new();
    records.extend(spell_visual_row(67, 30, 38, 286, 0, 0, 365, 1)); // Fireball's visual row
    records.extend(spell_visual_row(1, 0, 0, 0, 0, 0, 0, 0)); // an all-silent row
    let bytes = build_wdbc(2, SPELL_VISUAL_FIELDS as u32, &records);
    let rs = parse(
        &bytes,
        n_u32_schema("SpellVisual", SPELL_VISUAL_FIELDS),
        "t",
    )
    .unwrap();

    let mut visuals = HashMap::new();
    for r in rs.records() {
        let id = u32_at(r, 0).unwrap();
        let g = |i: usize| u32_at(r, i).unwrap_or(0);
        visuals.insert(
            id,
            VisualStages {
                precast: g(1),
                cast: g(2),
                impact: g(3),
                state: g(4),
                channel: g(5),
                missile_model: g(7),
                missile_attach: g(9),
                missile_sound: u32_at(r, 10).and_then(some_unless_none),
                strike_sound: u32_at(r, 14).and_then(some_unless_none),
                missile_gate: g(6),
                area_gate: g(11),
                area_effect: g(12),
                area_kit: g(13),
            },
        );
    }
    assert_eq!(
        visuals[&67],
        VisualStages {
            precast: 30,
            cast: 38,
            impact: 286,
            state: 0,
            channel: 0,
            missile_model: 365,
            missile_attach: 1,
            missile_sound: None,
            strike_sound: None,
            // The synthetic row builder writes 0 into field 6 and the dest-anchored block
            // (the REAL Fireball row's missile_gate = 1 is pinned in the real-data test).
            ..Default::default()
        }
    );
    assert_eq!(
        visuals[&1],
        VisualStages::default(),
        "an all-zero row is all-silent"
    );
}

#[test]
fn kit_anim_and_sound_fold_both_none_sentinels_to_none() {
    let mut records = Vec::new();
    records.extend(spell_visual_kit_row(38, 53, 1484, 0)); // Fireball's cast kit
    records.extend(spell_visual_kit_row(1, 0, 0, 0)); // plain-zero "none"
    records.extend(spell_visual_kit_row(2, u32::MAX, u32::MAX, u32::MAX)); // the -1 "none" form
    let bytes = build_wdbc(3, SPELL_VISUAL_KIT_FIELDS as u32, &records);
    let rs = parse(
        &bytes,
        n_u32_schema("SpellVisualKit", SPELL_VISUAL_KIT_FIELDS),
        "t",
    )
    .unwrap();

    let mut kits = HashMap::new();
    for r in rs.records() {
        let id = u32_at(r, 0).unwrap();
        let anim_id = u32_at(r, 2).and_then(some_unless_none).map(|a| a as u16);
        let sound = u32_at(r, 13).and_then(some_unless_none);
        kits.insert(
            id,
            VisualKit {
                anim_id,
                sound,
                ..Default::default()
            },
        );
    }
    assert_eq!(
        kits[&38],
        VisualKit {
            anim_id: Some(53),
            sound: Some(1484),
            ..Default::default()
        }
    );
    assert_eq!(kits[&1], VisualKit::default(), "plain 0 = none");
    assert_eq!(kits[&2], VisualKit::default(), "0xFFFFFFFF = none too");
}

/// The emitter-slot surface: kit-field order maps to [`KIT_SLOT_TAGS`], both none-sentinels
/// fold out, and `effects()` yields only the populated pairs.
#[test]
fn kit_effect_slots_pair_with_their_attach_tags() {
    let kit = VisualKit {
        // Fireball's precast shape: LeftHand (slot index 3) + RightHand (slot index 4).
        effect_slots: [
            None,
            None,
            None,
            Some(287),
            Some(287),
            None,
            None,
            None,
            None,
        ],
        ..Default::default()
    };
    assert_eq!(
        kit.effects().collect::<Vec<_>>(),
        vec![(0x15, 287), (0x16, 287)],
        "LeftHand then RightHand, kit-field order"
    );
    assert_eq!(VisualKit::default().effects().count(), 0);
}

/// The tenth slot (field 12) rides `effects()` after the nine, at the interim
/// Base anchor — one iterator, so every kit consumer (the aura-state watcher, kit pushes,
/// cast/impact plays) picks it up without knowing it exists.
#[test]
fn kit_world_effect_joins_effects_at_base() {
    let kit = VisualKit {
        // Frost Nova's state-kit shape: Head sparkle (slot 0) + the feet ice in field 12.
        effect_slots: [Some(54), None, None, None, None, None, None, None, None],
        world_effect: Some(284),
        ..Default::default()
    };
    assert_eq!(
        kit.effects().collect::<Vec<_>>(),
        vec![(0x14, 54), (WORLD_EFFECT_TAG, 284)],
        "the nine first, then the world-plant slot"
    );
}

/// End-to-end on the real build-5875 tables: the byte-verified header shape (2165×16/64B ·
/// 1772×35/140B) and the full Fireball chain (module doc's "Verified chain") — a schema drift
/// or column slip fails loudly. Skips without client data.
#[test]
fn real_spell_visual_chain_resolves_fireball() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load SpellVisual/SpellVisualKit");
    assert_eq!(cat.len(), 2165, "all 5875 SpellVisual rows load");
    assert_eq!(cat.kit_len(), 1772, "all 5875 SpellVisualKit rows load");

    // Fireball (spell 133) → visual 67 (spells.rs's column-115 pin).
    let stages = cat.stages(67).expect("Fireball's SpellVisual row");
    assert_eq!(
        *stages,
        VisualStages {
            precast: 30,
            cast: 38,
            impact: 286,
            state: 0,
            channel: 0,
            missile_model: 365,
            missile_attach: 1,
            // Field 10: the fireball's in-flight loop (SoundEntries 3011 → FireMissileLoop.wav).
            missile_sound: Some(3011),
            strike_sound: None,
            // Field 6 set = a missile owns the arrival: the GO dest one-shot gate is closed;
            // the dest-anchored columns are empty on a projectile nuke.
            missile_gate: 1,
            area_gate: 0,
            area_effect: 0,
            area_kit: 0,
        }
    );
    // The gathering/work strike sounds: Mining's visual 93 carries the pick
    // clang in field 14 (SoundEntries 1143 "Mining Impact" = MiningHitA-E), Herb's 91 the
    // search rustle (1142) - the $TRD anim event's operands.
    assert_eq!(
        cat.stages(93).and_then(|s| s.strike_sound),
        Some(1143),
        "Mining's visual carries the field-14 strike clang"
    );
    assert_eq!(
        cat.stages(91).and_then(|s| s.strike_sound),
        Some(1142),
        "Herb Gathering's visual carries the field-14 rustle"
    );

    // A basic thrown attack borrows the equipped weapon's substitute visual (98 — the thrown
    // dagger's ItemDisplayInfo col 10): no missile MODEL (it flies the weapon itself) but a
    // flight loop in field 10 (SoundEntries 3318 → WeaponLoop.wav) — the whoosh the projectile
    // carries while it travels.
    let thrown = cat
        .stages(98)
        .expect("the thrown-weapon substitute SpellVisual");
    assert_eq!(
        thrown.missile_model, 0,
        "a thrown weapon flies its own model, not an effect"
    );
    assert_eq!(
        thrown.missile_sound,
        Some(3318),
        "the thrown weapon's flight loop"
    );
    // The missile chain (phase 4): field 7 → the projectile's own model, field 9 → the
    // chest attach the missile homes to (ordinal 1 → 0x22).
    assert_eq!(
        cat.effect_path(stages.missile_model),
        Some("Spells\\Fireball_Missile_Low.mdx"),
        "the flying fireball's model"
    );
    assert_eq!(
        MISSILE_ATTACH_TABLE[stages.missile_attach as usize], 0x22,
        "Fireball homes onto the target's chest"
    );

    // The engine-spawned hardcoded set resolves by the client's own baked names — the ding
    // (row 21, byte-verified `0x61f5b0`/`0x8618e0`, decision 0304's fold-back).
    assert_eq!(
        cat.hardcoded_effect("HARDCODED Unit Level Up"),
        Some((21, "Spells\\LevelUp\\LevelUp.mdl")),
        "the level-up pillar resolves by name"
    );
    assert!(
        cat.hardcoded_effect("HARDCODED Loot Art").is_some(),
        "the sibling hardcoded rows ride the same map"
    );

    let cast_kit = cat.kit(stages.cast).expect("cast kit");
    assert_eq!(
        cast_kit.anim_id,
        Some(53),
        "AnimationData id 53 = SpellCastDirected"
    );
    assert_eq!(cast_kit.sound, Some(1484));

    let impact_kit = cat.kit(stages.impact).expect("impact kit");
    assert_eq!(
        impact_kit.anim_id,
        Some(9),
        "AnimationData id 9 = CombatWound, the target's hit reaction"
    );
    assert_eq!(impact_kit.sound, Some(1507));

    // The precast kit's emitter slots (phase 3): effect 287 on both hands, and its
    // SpellVisualEffectName path — the glowing-hands chain end to end.
    let precast_kit = cat.kit(stages.precast).expect("precast kit");
    assert_eq!(
        precast_kit.effects().collect::<Vec<_>>(),
        vec![(0x15, 287), (0x16, 287)],
        "Fire_Precast_Hand on LeftHand + RightHand"
    );
    assert_eq!(
        cat.effect_path(287),
        Some("Spells\\Fire_Precast_Hand.mdx"),
        "SpellVisualEffectName field 2 = the effect model path"
    );

    // The tenth slot (field 12) on the real table — the root/snare state
    // family the nine slots miss. Frost Nova (spell 122 → visual 17): state kit 285's feet
    // ice; Net (spell 6533 → visual 683): state kit 744's net wrap, a kit with NO ordinary
    // slots at all.
    let frost_nova_state = cat
        .kit(cat.stages(17).expect("Frost Nova's visual").state)
        .expect("Frost Nova's state kit");
    assert_eq!(frost_nova_state.world_effect, Some(284));
    assert_eq!(
        cat.effect_path(284),
        Some("Spells\\Frost_Nova_state.mdx"),
        "the frozen-feet model rides field 12"
    );
    let net_state = cat
        .kit(cat.stages(683).expect("Net's visual").state)
        .expect("Net's state kit");
    assert_eq!(
        net_state.effects().collect::<Vec<_>>(),
        vec![(WORLD_EFFECT_TAG, 594)],
        "the net wrap is the kit's ONLY effect — invisible without field 12"
    );
    assert_eq!(cat.effect_path(594), Some("Spells\\Net_State.mdx"));
    // The impact kit's chest burst — the phase-4 arrival hand-off will play this.
    assert_eq!(
        cat.kit(stages.impact)
            .unwrap()
            .effects()
            .collect::<Vec<_>>(),
        vec![(0x22, 321)],
        "MoltenBlast_Impact_Chest on the Chest attach"
    );
}

/// The real 5875 `SpellVisualEffectName`: the boot-time HARDCODED name matcher's one consumed
/// row — `"HARDCODED Loot Art"` is id 14 → `Particles\LootFX.mdl` (values pre-checked against
/// the raw DBC bytes), and the `.m2` it names ships in the chain, so the lootable-corpse
/// sparkle (`0x6005d0`) can actually load. Skips without client data.
#[test]
fn real_effect_name_table_resolves_the_loot_art_row() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load the visual catalog");
    assert_eq!(cat.loot_art_effect(), Some((14, "Particles\\LootFX.mdl")));
    // The model the row names ships in the chain (consumers rewrite .mdl → .m2 to load it).
    assert!(
        chain.read_file("Particles\\LootFX.m2").is_ok(),
        "LootFX.m2 must ship in the chain"
    );
}

/// The dest-anchored chain on the REAL data — 0797's mandatory per-spell data check (the
/// GATE is the binary's, `0x5d57c0`; whether each spell passes it is a table fact). Skips
/// without client data. Every value here corroborates the live vmangos wire capture:
/// Blizzard's dynobj RADIUS was 8.0 (row 14), Flamestrike's 5.0 (row 8).
#[test]
fn ground_aoe_chain_reads_the_dest_anchored_block() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let catalog = crate::load_spell_catalog(&mut chain).expect("spells");
    let visuals = load_spell_visual_catalog(&mut chain).expect("visuals");
    let radii = crate::load_spell_radii(&mut chain).expect("radii");

    // Blizzard 10 → visual 259: both visuals — its own model AND a type-9 shard emitter
    // whose table index decodes to 0 (the same model), rate 5/s, sound 7.
    let d = catalog.get(10).unwrap();
    assert_eq!((d.visual, d.effect_radius_index), (259, [14, 0, 0]));
    assert_eq!(
        radii.get(14).map(|r| r.radius),
        Some(8.0),
        "the wire radius"
    );
    let v = visuals.stages(259).unwrap();
    assert_eq!(
        (v.missile_gate, v.area_gate, v.area_effect, v.area_kit),
        (0, 1, 398, 609)
    );
    assert_eq!(
        visuals.effect_path(398),
        Some("Spells\\Blizzard_Impact_Base.mdx")
    );
    let k = visuals.kit(609).unwrap();
    assert_eq!(k.sound, Some(7));
    let proc = k.char_procs().find(|p| p.ty == 9).unwrap();
    assert_eq!((proc.params[0], proc.params[1]), (0.0, 5.0));

    // Flamestrike 2120 → visual 33: its own model + sound, NO type-9 proc (a burning
    // patch has no falling shards) — the emitter half is data-absent, not code-gated.
    let d = catalog.get(2120).unwrap();
    assert_eq!((d.visual, d.effect_radius_index), (33, [8, 8, 0]));
    assert_eq!(radii.get(8).map(|r| r.radius), Some(5.0), "the wire radius");
    let v = visuals.stages(33).unwrap();
    assert_eq!(
        (v.missile_gate, v.area_gate, v.area_effect, v.area_kit),
        (0, 1, 420, 695)
    );
    assert_eq!(
        visuals.effect_path(420),
        Some("Spells\\Flamestrike_Impact_Base.mdx")
    );
    let k = visuals.kit(695).unwrap();
    assert_eq!(k.sound, Some(3077));
    assert!(k.char_procs().all(|p| p.ty != 9));

    // Rain of Fire 5740 → visual 329: shard-table index 1 (its own model again), rate 5/s.
    let v = visuals.stages(catalog.get(5740).unwrap().visual).unwrap();
    assert_eq!((v.missile_gate, v.area_gate, v.area_effect), (0, 1, 448));
    let proc = visuals
        .kit(v.area_kit)
        .unwrap()
        .char_procs()
        .find(|p| p.ty == 9)
        .unwrap();
    assert_eq!((proc.params[0], proc.params[1]), (1.0, 5.0));

    // All three pass the GO dest one-shot gate (field 6 == 0 ∧ field 12 ≠ 0) — the burst
    // fires for each; Fireball (missile_gate 1, pinned in the loader test above) does not.
}

// ── The chain/beam table ──────────────────────────────────────────────────────

/// The small-int decode, on the exact values the shipped table ships. This is the load-bearing
/// arithmetic — get it wrong and every beam resolves to the wrong texture (or to none) — so it is
/// pinned against hand-computed IEEE bits rather than against itself.
#[test]
fn char_proc_small_int_recovers_the_integer() {
    // 1.0 + 512.0 = 513.0 = 0x44004000; >>14 = 0x11001; &0xff = 1.
    assert_eq!(char_proc_small_int(1.0), 1);
    assert_eq!(char_proc_small_int(0.0), 0);
    for n in 0..=255u32 {
        assert_eq!(char_proc_small_int(n as f32), n, "round-trips every byte");
    }
    // The decode truncates rather than rounds — it reads the integer part out of the mantissa.
    assert_eq!(char_proc_small_int(3.9), 3);
}

/// `SpellChainEffects.dbc` as it actually ships: 18 rows, the gaps at 14/16, and the two rows the
/// director's report turns on — id 1 (Chain Lightning's `Lightning`, the one row whose hops
/// stagger by 300 ms rather than 200) and id 8 (Drain Life's `SoulBeam`, one of the four
/// negative-period drains).
#[test]
fn real_chain_effects_table() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");
    assert_eq!(
        cat.chain_effect_len(),
        18,
        "18 SpellChainEffects rows in 5875"
    );

    let lightning = cat.chain_effect(1).expect("chain effect 1");
    assert_eq!(
        lightning.texture,
        "Textures\\SpellChainEffects\\Lightning.blp"
    );
    assert_eq!(lightning.avg_seg_len, 2.78);
    assert_eq!(lightning.half_width, 0.5);
    assert_eq!(lightning.noise_scale, 0.04);
    assert_eq!(lightning.scroll_period_s, 1.0);
    assert_eq!(lightning.bolt_life_ms, 1000);
    assert_eq!(
        lightning.bolt_stagger_ms, 300,
        "id 1 is the one row at 300 ms — every other live row is 200"
    );

    assert_eq!(
        cat.chain_effect(4).expect("chain effect 4").bolt_stagger_ms,
        200,
        "…and 200 is the table's norm"
    );

    let drain = cat.chain_effect(8).expect("chain effect 8");
    assert_eq!(drain.texture, "Textures\\SpellChainEffects\\SoulBeam.blp");
    assert_eq!(
        drain.scroll_period_s, -0.5,
        "the drains ship a NEGATIVE scroll period — the texture flows back toward the caster"
    );
    assert!(
        drain.noise_scale < lightning.noise_scale,
        "a drain rope is far straighter than a lightning arc"
    );

    // Ids are a lookup, never an index: 14 and 16 are absent from an otherwise 1..=20 run.
    assert!(cat.chain_effect(14).is_none() && cat.chain_effect(16).is_none());
    assert!(cat.chain_effect(0).is_none(), "id 0 is the client's no-op");
    assert!(cat.chain_effect(21).is_none());
}

/// **The regression that IS decision 0955.** The chain `CharProc` reaches its beam on the real
/// table, for both keys — and every live chain slot resolves to a real row.
///
/// The old `char_proc_slot` folded type `0` to "empty", so 34 of the 48 live beams (every channel
/// beam in the game) never existed as far as this crate was concerned. A regression here reads as
/// "Drain Life has no beam again".
#[test]
fn real_chain_procs_resolve_to_their_beams() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");

    // Chain Lightning (spell 421 → visual 36) — cast kit 321, type 12, one beam, flag clear.
    let cl = cat.kit(321).expect("kit 321").chain_proc().expect("a beam");
    assert_eq!(cl.ty, char_proc_type::CHAIN_CAST);
    assert_eq!((cl.effect_id, cl.beams, cl.flag), (1, 1, false));
    assert_eq!(
        cat.chain_effect(cl.effect_id).map(|c| c.texture.as_str()),
        Some("Textures\\SpellChainEffects\\Lightning.blp")
    );

    // Drain Life (spell 689 → visual 177) — channel kit 402, type 0, flag SET. This is the one
    // the `<= 0` sentinel used to swallow whole.
    let dl = cat.kit(402).expect("kit 402").chain_proc().expect("a beam");
    assert_eq!(dl.ty, char_proc_type::CHAIN_CHANNEL);
    assert_eq!((dl.effect_id, dl.beams, dl.flag), (8, 1, true));

    // The named cast-stage others, each predicting its own texture — the corroboration that the
    // decode is the mechanism and not a coincidence that fits Chain Lightning.
    for (kit, effect, texture) in [
        (3169u32, 2u32, "HealBeam.blp"),     // Chain Heal
        (430, 4, "ManaBeam.blp"),            // Drain Mana / Mind Flay (channel)
        (950, 10, "Beam_Purple.blp"),        // Drain Soul (channel)
        (342, 5, "SoulBeam.blp"),            // Health Funnel (channel)
        (2509, 6, "ManaBurnBeam.blp"),       // Shrink Ray
        (6480, 18, "SoulBeam.blp"),          // C'Thun's Eye Beam
        (6567, 7, "ShockLightning.blp"),     // the Feugen/Stalagg chains
        (6397, 3, "DrainManaLightning.blp"), // Chain Burn — the one 3-beam kit
    ] {
        let p = cat
            .kit(kit)
            .unwrap_or_else(|| panic!("kit {kit}"))
            .chain_proc()
            .unwrap_or_else(|| panic!("kit {kit} draws a beam"));
        assert_eq!(p.effect_id, effect, "kit {kit}");
        let got = cat
            .chain_effect(p.effect_id)
            .expect("its row")
            .texture
            .clone();
        assert!(
            got.ends_with(texture),
            "kit {kit}: {got} should end {texture}"
        );
    }
    assert_eq!(
        cat.kit(6397).unwrap().chain_proc().unwrap().beams,
        3,
        "Chain Burn is the only kit asking for more than one beam"
    );

    // The whole-table census — the shape the census instrument prints, asserted so a schema slip
    // or a sentinel regression is a number that moved.
    let (mut slots, mut live, mut padding) = (0, 0, 0);
    for id in cat.kit_ids() {
        let kit = cat.kit(id).expect("kit");
        for proc in kit.char_procs().filter(|p| char_proc_type::is_chain(p.ty)) {
            slots += 1;
            match proc.as_chain() {
                Some(c) => {
                    live += 1;
                    assert!(
                        cat.chain_effect(c.effect_id).is_some(),
                        "kit {id} decodes to chain id {} — every live slot must name a real row",
                        c.effect_id
                    );
                }
                None => padding += 1,
            }
        }
    }
    assert_eq!(
        (slots, live, padding),
        (68, 48, 20),
        "68 chain CharProc slots ship: 48 live beams, 20 zero-param padding"
    );
}

/// **Every live beam's texture actually exists on the patch chain.** The renderer
/// loads these by path, and a path that resolves to nothing draws an invisible beam — a failure no
/// geometry test can see and no gate can catch. Cheapest possible guard against "the whole lane is
/// right and the screen is empty".
#[test]
fn real_chain_effect_textures_resolve_on_the_patch_chain() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");
    // Only the rows a shipped kit can actually reach: an unreachable row's texture is nobody's
    // problem (id 15 is degenerate anyway).
    let mut reached: Vec<u32> = cat
        .kit_ids()
        .filter_map(|id| Some(cat.kit(id)?.chain_proc()?.effect_id))
        .collect();
    reached.sort_unstable();
    reached.dedup();
    assert!(!reached.is_empty(), "some kit must reach a beam");
    for id in reached {
        let path = &cat.chain_effect(id).expect("its row").texture;
        assert!(
            chain.read_file(path).is_ok(),
            "chain effect {id} names {path}, which does not exist on the patch chain"
        );
    }
}

/// The padding case, isolated: a type-0 slot whose `CharParamZero` is `0` is not a beam — the
/// client's own null-row test no-ops it, and so must we. Kit 2089 (Death & Decay, Ritual of Doom)
/// ships four of them.
#[test]
fn real_zero_param_chain_slots_are_padding() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");
    let kit = cat.kit(2089).expect("kit 2089");
    assert_eq!(
        kit.char_procs()
            .filter(|p| p.ty == char_proc_type::CHAIN_CHANNEL)
            .count(),
        4,
        "four type-0 slots — they survive the parser now"
    );
    assert!(kit.chain_proc().is_none(), "…but none of them is a beam");
}

/// [`VisualStages::merged_over_weapon`] — the `60d4d2`–`60d54c` fill, field by field: a populated
/// slot survives, an empty one takes the weapon's, and the two the client pointedly skips
/// (`state`/`channel`) stay empty however loud the weapon's row is.
#[test]
fn the_weapon_merge_fills_only_the_empty_slots() {
    let weapon = VisualStages {
        precast: 7,
        cast: 164,
        impact: 1947,
        state: 999,   // +0x10 — never merged
        channel: 998, // +0x14 — never merged
        missile_gate: 1,
        missile_model: 42,
        missile_attach: 3,
        missile_sound: Some(4222),
        strike_sound: Some(1143),
        area_gate: 1,
        area_effect: 7,
        area_kit: 9,
    };
    // Multi-Shot's real shape: its own impact + missile, both body kits empty.
    let own = VisualStages {
        impact: 658,
        missile_gate: 1,
        missile_model: 528,
        missile_attach: 1,
        ..Default::default()
    };
    let merged = own.merged_over_weapon(&weapon);
    assert_eq!((merged.precast, merged.cast), (7, 164), "empty slots fill");
    assert_eq!(merged.impact, 658, "a populated slot is kept");
    assert_eq!(
        (merged.missile_model, merged.missile_attach),
        (528, 1),
        "its own missile block survives — the gate was already nonzero"
    );
    assert_eq!(
        merged.missile_sound,
        Some(4222),
        "the flight loop fills from the bow (Serpent Sting / Multi-Shot carry none)"
    );
    assert_eq!(
        (merged.state, merged.channel),
        (0, 0),
        "state/channel are NOT in the client's fill list"
    );
    assert_eq!(
        (merged.area_gate, merged.area_effect, merged.area_kit),
        (0, 0, 0),
        "neither is the dest-anchored block"
    );

    // The missile pair's own arm: an own row with NO missile takes the weapon's — and the gate is
    // written as the literal 1 (`60d50b`), not the weapon's value.
    let no_missile = VisualStages::default().merged_over_weapon(&VisualStages {
        missile_gate: 5,
        missile_model: 42,
        ..Default::default()
    });
    assert_eq!((no_missile.missile_gate, no_missile.missile_model), (1, 42));
    // …and a weapon with no missile never plants one.
    let neither = VisualStages::default().merged_over_weapon(&VisualStages::default());
    assert_eq!((neither.missile_gate, neither.missile_model), (0, 0));
}

/// The real 5875 tables, end to end: every live hunter shot leaves both body-kit slots empty, so
/// the bow's own visual 5 is the only source of a draw/release clip — and the merge lands
/// **LoadBow (105) → AttackBow (46)** on each while its impact + missile survive untouched. The
/// pin behind bug B153; a data change that filled these rows would make the merge a no-op and
/// this test would say so.
#[test]
fn real_hunter_shots_take_the_bows_load_and_release_clips() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");
    // `ItemDisplayInfo` col 10 for every bow (measured on the shipped table).
    let bow = *cat.stages(5).expect("the bow's substitute visual 5");
    assert_eq!(
        (bow.precast, bow.cast),
        (7, 164),
        "visual 5 = LoadBow kit 7 → AttackBow kit 164"
    );
    for (spell_visual, own_impact) in [
        (3179, 276),  // Serpent Sting
        (567, 658),   // Multi-Shot
        (3299, 419),  // Arcane Shot
        (3180, 419),  // Aimed / Concussive Shot
        (3181, 2850), // Viper Sting
        (3219, 2893), // Scorpid Sting
        (3300, 419),  // Volley
    ] {
        let own = *cat.stages(spell_visual).expect("a live hunter-shot row");
        assert_eq!(
            (own.precast, own.cast),
            (0, 0),
            "visual {spell_visual} authors no body kit of its own"
        );
        let merged = own.merged_over_weapon(&bow);
        assert_eq!(
            (merged.precast, merged.cast),
            (7, 164),
            "visual {spell_visual} takes the bow's Load/Attack pair"
        );
        assert_eq!(
            merged.impact, own_impact,
            "visual {spell_visual} keeps its own impact kit"
        );
        assert_eq!(
            merged.missile_model, own.missile_model,
            "visual {spell_visual} keeps its own projectile"
        );
    }
    // The kit ids resolve to the AnimationData rows the caster actually plays.
    assert_eq!(cat.kit(7).and_then(|k| k.anim_id), Some(105), "LoadBow");
    assert_eq!(cat.kit(164).and_then(|k| k.anim_id), Some(46), "AttackBow");
}

/// The type-8 arm's decode is **plain truncation** (`_ftol` at `0x40a2b0`), not the small-int
/// idiom every other integer-carrying param uses — a slot decoded the wrong way yields a colour
/// of 0 and a duration of 0, i.e. silently no trail at all.
#[test]
fn a_weapon_trail_proc_decodes_by_truncation() {
    // Kit 324's shipped row: Zero = 16263465 (`0xf82929`), One = 20, Two = 600, Three = 100.
    let proc = CharProc {
        ty: char_proc_type::WEAPON_TRAIL,
        params: [16_263_465.0, 20.0, 600.0, 100.0],
    };
    let trail = proc.as_weapon_trail().expect("a live trail proc");
    assert_eq!(trail.rgb(), [0xf8, 0x29, 0x29], "R248 G41 B41");
    assert_eq!(trail.alpha(), 100);
    assert_eq!(trail.duration_ms, 600);
    assert_eq!(
        trail.packed, 0x64f8_2929,
        "0xAARRGGBB, as `unit+0xd1c` holds it"
    );
    assert_ne!(
        proc.small_int(0),
        u32::from(trail.rgb()[0]),
        "the small-int decode is the WRONG idiom here and must not be reused"
    );
}

/// `0x5fe494 cmp eax,edi ; je` — a zero duration fires nothing, so it is not a trail.
#[test]
fn a_zero_duration_trail_proc_is_no_trail() {
    let proc = CharProc {
        ty: char_proc_type::WEAPON_TRAIL,
        params: [16_263_465.0, 20.0, 0.0, 100.0],
    };
    assert!(proc.as_weapon_trail().is_none());
}

/// Only type 8 reaches the arm — `VisualKit::trail_proc` must not answer for a tint or an alpha
/// proc whose `params[2]` happens to be nonzero.
#[test]
fn only_type_eight_arms_a_trail() {
    for ty in [
        char_proc_type::TINT,
        char_proc_type::ALPHA,
        char_proc_type::ANIM_RATE,
        char_proc_type::CHAIN_CAST,
    ] {
        let proc = CharProc {
            ty,
            params: [16_263_465.0, 20.0, 600.0, 100.0],
        };
        assert!(proc.as_weapon_trail().is_none(), "type {ty}");
    }
}
