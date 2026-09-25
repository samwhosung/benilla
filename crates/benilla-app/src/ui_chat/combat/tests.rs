//! The combat log's tests: the slot orders against the shipped `GlobalStrings.lua`, the msgType
//! matrices against the reference's selectors, and single lines.

use super::*;

/// A template's `%s`/`%d` conversions in order; two templates with the same one take the same
/// arguments.
fn signature(template: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            continue;
        }
        match chars.next() {
            Some('%') => {}
            Some(k) => out.push(k),
            None => {}
        }
    }
    out
}

/// The signature a family's declared slots give for one variant; `Attacker` and `Victim` count
/// only where the variant names them.
fn declared_signature(family: Family, variant: Variant) -> Vec<char> {
    family
        .slots
        .iter()
        .filter_map(|slot| match slot {
            Slot::Attacker => family.names_subject(variant).then_some('s'),
            Slot::Victim => family.names_object(variant).then_some('s'),
            Slot::Spell | Slot::School | Slot::Power | Slot::Power2 | Slot::Named => Some('s'),
            Slot::Amount | Slot::Amount2 => Some('d'),
        })
        .collect()
}

/// The variants with distinct keys: four for `Quad`, two for `Duo` (both `…SELF*` variants give
/// the `me` word), one for `Single`.
fn variants_of(family: Family) -> &'static [Variant] {
    match family.keying {
        Keying::Quad => &[
            Variant::SelfOther,
            Variant::OtherSelf,
            Variant::OtherOther,
            Variant::SelfSelf,
        ],
        Keying::Duo { .. } => &[Variant::SelfOther, Variant::OtherOther],
        Keying::Single => &[Variant::OtherOther],
    }
}

/// Every family and variant whose key the install defines: the declared slots give exactly the
/// template's `%s`/`%d` sequence. A swap of two same-type slots passes here and is caught by
/// [`the_reported_sentences_read_correctly`]. An undefined key is skipped: several families have
/// no `…SELFSELF`, for which the reference's selector returns NULL.
#[test]
fn every_family_matches_the_shipped_template() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(&String::from_utf8_lossy(&src))
        .expect("GlobalStrings runs clean");

    let mut checked = 0usize;
    for &family in ALL_FAMILIES {
        for &variant in variants_of(family) {
            let key = family.key(variant);
            let Some(template) = global_string(&script, &key) else {
                continue;
            };
            assert_eq!(
                declared_signature(family, variant),
                signature(&template),
                "{key}: our slots {:?} do not match the shipped {template:?}",
                family.slots,
            );
            checked += 1;
        }
    }
    // A stem typo'd everywhere would miss every lookup and pass silently.
    assert!(
        checked >= ALL_FAMILIES.len(),
        "only {checked} keys resolved"
    );
}

/// Every key but `…SELFSELF` exists, as the reference's selectors always return one: an absent key
/// is a wrong stem.
#[test]
fn every_family_has_the_three_core_keys() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(&String::from_utf8_lossy(&src))
        .expect("GlobalStrings runs clean");

    for &family in ALL_FAMILIES {
        let core: Vec<Variant> = match family.keying {
            Keying::Quad => vec![Variant::SelfOther, Variant::OtherSelf, Variant::OtherOther],
            _ => variants_of(family).to_vec(),
        };
        for variant in core {
            let key = family.key(variant);
            assert!(
                global_string(&script, &key).is_some(),
                "{key} is not a GlobalString — wrong stem?"
            );
        }
    }
}

#[test]
fn the_school_and_power_words_resolve() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(&String::from_utf8_lossy(&src))
        .expect("GlobalStrings runs clean");

    // The seven 1.12 schools (`SpellSchools`, 0 physical … 6 arcane).
    for school in 0..=6u8 {
        assert!(
            school_word(&script, school).is_some(),
            "SPELL_SCHOOL{school}_CAP missing"
        );
    }
    // `0x6278f0`'s power table has five entries (`cmp ecx,5; jae`); happiness is the fifth.
    for power in 0..=4u32 {
        assert!(
            power_word(&script, power).is_some(),
            "power {power} missing"
        );
    }
    assert_eq!(power_word(&script, 4).as_deref(), Some("Happiness"));
    // Past the table's five entries the reference returns NULL and the line is dropped.
    assert!(power_word(&script, 5).is_none());
}

/// Whole sentences on the real strings, which catch the same-type slot swaps the signature check
/// cannot.
#[test]
fn the_reported_sentences_read_correctly() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(&String::from_utf8_lossy(&src))
        .expect("GlobalStrings runs clean");

    let fills = Fills {
        attacker: "Attacker".into(),
        victim: "Victim".into(),
        spell: "Fireball".into(),
        // Resolved through the real strings by `compose_line`: school 2 is fire, power 0 is mana.
        school: Some(2),
        power: Some(0),
        amount: 120,
        amount2: 40,
        power2: Some(0),
        named: "Copper Bar".into(),
        trailers: None,
    };
    let line = |family, variant| compose_line(&script, family, variant, &fills).expect("composes");

    assert_eq!(
        line(COMBATHIT, Variant::SelfOther),
        "You hit Victim for 120."
    );
    assert_eq!(
        line(COMBATHIT, Variant::OtherSelf),
        "Attacker hits you for 120."
    );
    // The spell comes before the victim here and after the amount in the periodic family.
    assert_eq!(
        line(SPELLLOG, Variant::SelfOther),
        "Your Fireball hits Victim for 120."
    );
    assert_eq!(
        line(PERIODICAURADAMAGE, Variant::SelfOther),
        // Capitalized: the reference takes this word from `Resistances.dbc`, not from
        // `SPELL_SCHOOL<n>_NAME`.
        "Victim suffers 120 Fire damage from your Fireball."
    );
    assert_eq!(
        line(POWERGAIN, Variant::SelfSelf),
        "You gain 120 Mana from Fireball."
    );
    assert_eq!(
        line(HEALED, Variant::OtherOther),
        "Attacker's Fireball heals Victim for 120."
    );
    assert_eq!(
        line(DAMAGESHIELD, Variant::SelfOther),
        "You reflect 120 Fire damage to Victim."
    );
    // The double-subject family: the drainer is named twice and the second gain has its own pair.
    assert_eq!(
        line(SPELLPOWERLEECH, Variant::OtherOther),
        "Attacker's Fireball drains 120 Mana from Victim. Attacker gains 40 Mana."
    );
    assert_eq!(line(MISSED, Variant::SelfOther), "You miss Victim.");
    assert_eq!(
        line(VSDODGE, Variant::OtherSelf),
        "Attacker attacks. You dodge."
    );
}

#[test]
fn the_fill_is_vsnprintf_and_refuses_a_mismatch() {
    fn s(v: &str) -> Arg<'_> {
        Arg::Str(v)
    }
    assert_eq!(
        fill("%s hits %s for %d.", &[s("A"), s("B"), Arg::Num(7)]).as_deref(),
        Some("A hits B for 7.")
    );
    assert_eq!(fill("100%% sure", &[]).as_deref(), Some("100% sure"));
    // A `%d` where a string is queued, and vice versa.
    assert_eq!(fill("%d", &[s("A")]), None);
    assert_eq!(fill("%s", &[Arg::Num(1)]), None);
    // Too few, and too many.
    assert_eq!(fill("%s %s", &[s("A")]), None);
    assert_eq!(fill("%s", &[s("A"), s("B")]), None);
    // A conversion outside the vocabulary is a mismatch, not a passthrough.
    assert_eq!(fill("%f", &[Arg::Num(1)]), None);
}

/// `0x629b60`'s arm order: a blocked swing with damage is `VSBLOCK`, and a fully absorbed one
/// `VSABSORB`, not `MISSED`.
#[test]
fn the_melee_dispatcher_follows_the_reference_order() {
    let f = |h, v, d, s| melee_family(h, v, d, s).expect("a family").stem;
    // A plain landed swing, and its crit and school variants.
    assert_eq!(f(0, 1, 120, 0), "COMBATHIT");
    assert_eq!(f(0x80, 1, 120, 0), "COMBATHITCRIT");
    assert_eq!(f(0, 1, 120, 2), "COMBATHITSCHOOL");
    assert_eq!(f(0x80, 1, 120, 2), "COMBATHITCRITSCHOOL");
    // MISS wins over everything, including a VictimState that would say otherwise.
    assert_eq!(f(0x10, 2, 0, 0), "MISSED");
    // BLOCKS wins over the damage test: a partial block still lands damage.
    assert_eq!(f(0, 5, 90, 0), "VSBLOCK");
    // Zero damage + the absorb/resist bits.
    assert_eq!(f(0x20, 1, 0, 0), "VSABSORB");
    assert_eq!(f(0x40, 1, 0, 0), "VSRESIST");
    // ...but the same bits with damage through are a landed hit, not a full absorb.
    assert_eq!(f(0x20, 1, 90, 0), "COMBATHIT");
    // The VictimState words.
    assert_eq!(f(0, 2, 0, 0), "VSDODGE");
    assert_eq!(f(0, 3, 0, 0), "VSPARRY");
    assert_eq!(f(0, 6, 0, 0), "VSEVADE");
    assert_eq!(f(0, 7, 0, 0), "VSIMMUNE");
    assert_eq!(f(0, 8, 0, 0), "VSDEFLECT");
}

/// `0x62a710` prints nothing for a VictimState whose byte in `0x8628f8`, `[0,0,1,1,0,1,1,1,1,0]`,
/// is 0. State 1 words a hit only with damage through, in arm 5.
#[test]
fn the_silent_victim_states_emit_no_melee_line() {
    for state in [0, 1, 4, 9] {
        assert!(
            melee_family(0, state, 0, 0).is_none(),
            "VictimState {state} must emit no line"
        );
    }
    // The same states still word normally when an earlier arm claims them: the MISS bit, and
    // state 1 with damage through.
    assert!(melee_family(0x10, 0, 0, 0).is_some(), "the MISS bit wins");
    assert!(melee_family(0, 1, 120, 0).is_some(), "a landed hit wins");
    // Index 5 of the table is a 1, and unreachable: arm 2 claims a block.
    assert_eq!(melee_family(0, 5, 0, 0).map(|f| f.stem), Some("VSBLOCK"));
}

/// The melee msgType matrix of `0x62a0d0`/`0x62a2e0`, through the 94-entry type table at
/// `0x804710`, whose 1-based index is the selector's return plus one.
#[test]
fn the_melee_matrix_is_the_reference_selector() {
    use ChatEventKind as K;
    use UnitClass as C;
    let hits = |a, v| combat_kind(a, v, false).unwrap();
    let misses = |a, v| combat_kind(a, v, true).unwrap();

    // src 0 → 0x1b, src 1 → 0x1d, whatever the victim.
    assert_eq!(hits(C::Me, C::Creature), K::CombatSelfHits);
    assert_eq!(misses(C::Me, C::Creature), K::CombatSelfMisses);
    assert_eq!(hits(C::MyPet, C::Creature), K::CombatPetHits);
    // src 3 and 5 are unconditional; src 2 and 4 are the two reclassifying arms.
    assert_eq!(hits(C::PartyPet, C::Me), K::CombatPartyHits);
    assert_eq!(hits(C::FriendlyPet, C::Me), K::CombatFriendlyPlayerHits);
    assert_eq!(hits(C::Party, C::Creature), K::CombatPartyHits);
    assert_eq!(
        hits(C::FriendlyPlayer, C::Creature),
        K::CombatFriendlyPlayerHits
    );
    // A party or friendly player attacking me or mine reads as hostile: the duel case.
    for victim in [C::Me, C::MyPet, C::Party, C::PartyPet] {
        assert_eq!(hits(C::Party, victim), K::CombatHostilePlayerHits);
        assert_eq!(hits(C::FriendlyPlayer, victim), K::CombatHostilePlayerHits);
    }
    assert_eq!(hits(C::HostilePlayer, C::Me), K::CombatHostilePlayerHits);
    assert_eq!(hits(C::HostilePet, C::Me), K::CombatHostilePlayerHits);
    // src 8 and 9 split three ways on the victim.
    assert_eq!(hits(C::Creature, C::Me), K::CombatCreatureVsSelfHits);
    assert_eq!(hits(C::Creature, C::MyPet), K::CombatCreatureVsSelfHits);
    assert_eq!(hits(C::Creature, C::Party), K::CombatCreatureVsPartyHits);
    assert_eq!(hits(C::Creature, C::PartyPet), K::CombatCreatureVsPartyHits);
    for victim in [C::FriendlyPlayer, C::HostilePlayer, C::Creature] {
        assert_eq!(hits(C::Creature, victim), K::CombatCreatureVsCreatureHits);
    }
}

/// The spell matrix is the melee one from another base row (`0x627820`); the periodic one has no
/// pet row and no `CREATURE_VS_*` split, and ignores the victim.
#[test]
fn the_spell_and_periodic_matrices() {
    use ChatEventKind as K;
    use UnitClass as C;
    assert_eq!(
        spell_kind(C::Me, C::Creature, false).unwrap(),
        K::SpellSelfDamage
    );
    assert_eq!(
        spell_kind(C::Me, C::Creature, true).unwrap(),
        K::SpellSelfBuff
    );
    assert_eq!(
        spell_kind(C::FriendlyPlayer, C::Me, false).unwrap(),
        K::SpellHostilePlayerDamage
    );
    assert_eq!(
        spell_kind(C::Creature, C::Party, true).unwrap(),
        K::SpellCreatureVsPartyBuff
    );

    // A pet folds into its owner's periodic row, and every victim gives the same answer.
    for victim in [C::Me, C::Party, C::HostilePlayer, C::Creature] {
        assert_eq!(
            periodic_kind(C::MyPet, false).unwrap(),
            K::SpellPeriodicSelfDamage,
            "victim {victim:?} must not matter"
        );
        assert_eq!(
            periodic_kind(C::Creature, false).unwrap(),
            K::SpellPeriodicCreatureDamage
        );
    }
    assert_eq!(
        periodic_kind(C::HostilePet, true).unwrap(),
        K::SpellPeriodicHostilePlayerBuffs
    );
}

/// Only class 0 is `SELF`: your pet is `OTHER` for the string though the msgType matrix gives it
/// its own row.
#[test]
fn only_the_player_is_self_for_string_selection() {
    use UnitClass as C;
    assert_eq!(Variant::of(C::Me, C::Creature), Variant::SelfOther);
    assert_eq!(Variant::of(C::Creature, C::Me), Variant::OtherSelf);
    assert_eq!(Variant::of(C::MyPet, C::Creature), Variant::OtherOther);
    assert_eq!(Variant::of(C::Creature, C::MyPet), Variant::OtherOther);
    assert_eq!(Variant::of(C::Me, C::Me), Variant::SelfSelf);
}

/// Class 9's range is 0.0, so it never passes its own half of the range gate, but the other
/// endpoint's half still carries the line.
#[test]
fn an_unknown_endpoint_alone_does_not_drop_the_line() {
    use UnitClass as C;
    let f = Fills::default();
    let q = |a, b| {
        queue(
            ChatEventKind::CombatSelfHits,
            COMBATHIT,
            (1, a),
            (2, b),
            f.clone(),
            Named::Ready,
        )
    };
    assert!(
        q(C::Me, C::Unknown).is_some(),
        "a resolvable attacker carries it"
    );
    assert!(
        q(C::Unknown, C::Creature).is_some(),
        "so does a resolvable victim"
    );
    assert!(q(C::Me, C::Creature).is_some());
    assert!(
        q(C::Unknown, C::Unknown).is_none(),
        "neither end resolvable — nothing to say"
    );
}

/// The reference's miss-code switch (`0x62bb50`, table `0x62bde8`): `lea eax,[edi-2]; cmp eax,9;
/// ja default` sends 0, 1, 10 and anything out of range to `SPELLMISS*`.
#[test]
fn the_miss_codes_map_to_their_families() {
    assert_eq!(miss_family(2).stem, "SPELLRESIST");
    assert_eq!(miss_family(3).stem, "SPELLDODGED");
    assert_eq!(miss_family(4).stem, "SPELLPARRIED");
    assert_eq!(miss_family(5).stem, "SPELLBLOCKED");
    assert_eq!(miss_family(6).stem, "SPELLEVADED");
    // Both IMMUNE spellings are one outcome.
    assert_eq!(miss_family(7).stem, "SPELLIMMUNE");
    assert_eq!(miss_family(8).stem, "SPELLIMMUNE");
    assert_eq!(miss_family(9).stem, "SPELLDEFLECTED");
    assert_eq!(miss_family(11).stem, "SPELLREFLECT");
    // The default arm, all four ways into it.
    for code in [0u8, 1, 10, 12, 255] {
        assert_eq!(
            miss_family(code).stem,
            "SPELLMISS",
            "{code} belongs on the default arm"
        );
    }
}

/// The class range table at `0x8629e0`, the evidence the class indices are what they are named.
#[test]
fn the_class_range_table_is_the_binarys() {
    use UnitClass as C;
    assert_eq!(C::Me.range_cvar(), None);
    assert_eq!(C::MyPet.range_cvar(), None);
    assert_eq!(C::Party.range_cvar(), Some("CombatLogRangeParty"));
    assert_eq!(C::PartyPet.range_cvar(), Some("CombatLogRangePartyPet"));
    assert_eq!(
        C::FriendlyPlayer.range_cvar(),
        Some("CombatLogRangeFriendlyPlayers")
    );
    assert_eq!(
        C::FriendlyPet.range_cvar(),
        Some("CombatLogRangeFriendlyPlayersPets")
    );
    assert_eq!(
        C::HostilePlayer.range_cvar(),
        Some("CombatLogRangeHostilePlayers")
    );
    assert_eq!(
        C::HostilePet.range_cvar(),
        Some("CombatLogRangeHostilePlayersPets")
    );
    assert_eq!(C::Creature.range_cvar(), Some("CombatLogRangeCreature"));
    assert_eq!(C::Unknown.range_cvar(), None);

    assert_eq!(C::Me.default_range(), 100_000.0);
    assert_eq!(C::Party.default_range(), 50.0);
    assert_eq!(C::Creature.default_range(), 30.0);
    assert_eq!(C::Unknown.default_range(), 0.0);
}

/// The reference registers the eight range CVars at `0x626d00` and reads each by name at every
/// use; [`CombatLogRanges`] holds them, and a `SetCVar` moves each class, and the death range,
/// on its own. Class 0 has no CVar.
#[test]
fn a_set_cvar_moves_the_range_the_gate_compares_against() {
    use super::{CombatLogRanges, UnitClass as C};

    let mut r = CombatLogRanges::default();
    // Seeded from the reference's own `{cvarName, defaultValue}` pairs.
    assert_eq!(r.class(C::Party), 50.0);
    assert_eq!(r.class(C::Creature), 30.0);
    assert_eq!(r.class(C::Me), 100_000.0);
    assert_eq!(r.death(), 60.0);

    // Each of the seven moves its own class and nothing else.
    assert!(r.set("CombatLogRangeParty", 200.0));
    assert_eq!(r.class(C::Party), 200.0);
    assert_eq!(r.class(C::PartyPet), 50.0, "a sibling class must not move");
    assert!(r.set("CombatLogRangeCreature", 15.0));
    assert_eq!(r.class(C::Creature), 15.0);

    // The name match is case-insensitive, like the reference's own `SStrCmpI` lookup.
    assert!(r.set("combatlograngehostileplayers", 80.0));
    assert_eq!(r.class(C::HostilePlayer), 80.0);

    // The death range is its own store, and moving it leaves every class alone.
    assert!(r.set(super::DEATH_LOG_RANGE_CVAR, 5.0));
    assert_eq!(r.death(), 5.0);
    assert_eq!(r.class(C::Party), 200.0);

    // Zero is a real value: the reference's `dist² < 0` is never true, so the class goes silent.
    assert!(r.set("CombatLogRangePartyPet", 0.0));
    assert_eq!(r.class(C::PartyPet), 0.0);

    // Classes 0 and 1 have a NULL name in the reference's table; an unrelated name is refused.
    assert!(!r.set("CombatLogRangeMe", 10.0));
    assert!(!r.set("mousespeed", 10.0));
    assert_eq!(r.class(C::Me), 100_000.0);
}
