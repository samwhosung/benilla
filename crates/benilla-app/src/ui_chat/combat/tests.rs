//! The combat log's own tests. Two of them are the ones that matter: the slot orders checked
//! against the *shipped* `GlobalStrings.lua`, and the three msgType matrices checked against the
//! byte-read table they transcribe. Everything else is a spot check on a line that was reported.

use super::*;

/// The `%s`/`%d` conversion sequence of a template, in order — its "type signature". Two format
/// strings with the same signature accept the same argument list, which is exactly the property a
/// declared slot order has to have.
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

/// The signature our declared slot list produces for one variant — `Attacker`/`Victim` present only
/// where the variant spells that endpoint out.
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

/// The variants a family can actually be asked for. A [`Keying::Quad`] family answers all four;
/// a `Duo` one collapses to two distinct keys (both `…SELF*` variants give the `me` word), and a
/// `Single` family has exactly one. Sweeping the quad list over a Duo family would check the same
/// key twice and prove nothing extra — worse, it would let `every_family_has_the_three_core_keys`
/// demand keys that do not exist.
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

/// **The gate on the one thing this module authors.** Every family × every variant: if the install
/// defines the key, our declared slot order must produce exactly the template's own `%s`/`%d`
/// sequence.
///
/// This is what makes the slot lists safe to hand-declare. A transposed pair of different types
/// (`Amount` before `School`, `Spell` after `Victim` where the string puts it first) changes the
/// signature and fails here. A transposition of two *same-type* slots does not — that residue is
/// covered by [`the_reported_sentences_read_correctly`], which pins whole sentences for the shapes
/// a player actually sees, and by each family's doc comment quoting its line.
///
/// A key the install does **not** define is not a failure: several families genuinely have no
/// `…SELFSELF` (`COMBATHIT`, `MISSED`, every `VS*`, `DAMAGESHIELD`, `SPELLPOWERLEECH`), and the
/// reference's own selector returns NULL for exactly those. [`every_family_has_the_three_core_keys`]
/// is what stops "absent" from silently covering a typo'd stem.
///
/// Skips without client data.
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
    // A stem typo'd across the board would make every lookup miss and leave this at zero, which
    // would otherwise pass silently.
    assert!(
        checked >= ALL_FAMILIES.len(),
        "only {checked} keys resolved"
    );
}

/// Every family defines the variants that are not `…SELFSELF` — for a `Quad` family the other
/// three, for a `Duo` one both of its keys, for a `Single` one its single key. Those are the ones
/// the reference's selectors always return a key for, so an absent one is a wrong stem, not a data
/// condition — the failure mode [`every_family_matches_the_shipped_template`] cannot see because
/// it skips what it cannot find. Skips without client data.
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

/// The school words and power words the `…SCHOOL…` and `POWERGAIN` slots take actually resolve.
/// Skips without client data.
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
    // `0x6278f0`'s table is FIVE entries — `cmp ecx,5; jae` — and happiness is the fifth. This
    // used to stop at 3 and assert that happiness had no word "deliberately"; the shipped file
    // says otherwise, and reading it here is what settles it.
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

/// The sentences a player actually reads, end to end, on the real strings — the residue
/// [`every_family_matches_the_shipped_template`]'s type check cannot cover (two adjacent `%s`
/// slots), pinned as whole sentences rather than as an argument order.
/// Skips without client data.
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

    // The ledger's own example line, and the one a damage meter parses — both directions.
    assert_eq!(
        line(COMBATHIT, Variant::SelfOther),
        "You hit Victim for 120."
    );
    assert_eq!(
        line(COMBATHIT, Variant::OtherSelf),
        "Attacker hits you for 120."
    );
    // The spell name comes BEFORE the victim in this family and AFTER the amount in the periodic
    // one — the two orders that would look identical to a type check if both were `s,s,d`.
    assert_eq!(
        line(SPELLLOG, Variant::SelfOther),
        "Your Fireball hits Victim for 120."
    );
    assert_eq!(
        line(PERIODICAURADAMAGE, Variant::SelfOther),
        // **Capitalized** — the reference words this cell out of `Resistances.dbc`, not out of
        // `SPELL_SCHOOL<n>_NAME`. Lowercase is what MikScrollingBattleText's
        // school compare misses.
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

/// The `vsnprintf` subset, including the two ways it is allowed to refuse.
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

/// The melee family dispatcher, arm for arm against `0x629b60`'s order — including the two overlaps
/// that make the order load-bearing: a blocked swing that also carries damage is `VSBLOCK` (the
/// VictimState test runs before the damage test), and a fully-absorbed swing is `VSABSORB` and not
/// `MISSED` (the MISS bit is not set for an absorb).
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
    // BLOCKS wins over the damage test — a partial block still lands damage.
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

/// `0x62a710` declines: the VictimStates whose flag-table byte is `0` produce **no line at all**.
///
/// The table `0x8628f8` is `[0,0,1,1,0,1,1,1,1,0]`, so 0, 1, 4 and 9 are silent — and 1 is the
/// interesting one, because a VictimState of 1 with damage through is the commonest line in the
/// whole log. It is arm 5 that words that, on `damage != 0`; strip the damage and the same state
/// falls to this arm and says nothing. Answering `MISSED` here (which is what we did) invents a
/// sentence the reference never prints.
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
    // Index 5 of the table is a `1`, and it is unreachable — a block is claimed by arm 2.
    assert_eq!(melee_family(0, 5, 0, 0).map(|f| f.stem), Some("VSBLOCK"));
}

/// The melee msgType matrix, against `0x62a0d0`/`0x62a2e0` as decompiled and against the 94-entry
/// type table `.rdata 0x804710` (whose 1-based index is one more than the selector's return).
#[test]
fn the_melee_matrix_is_the_reference_selector() {
    use ChatEventKind as K;
    use UnitClass as C;
    let hits = |a, v| combat_kind(a, v, false).unwrap();
    let misses = |a, v| combat_kind(a, v, true).unwrap();

    // src 0 → 0x1b, src 1 → 0x1d — no victim dependence.
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
    // ...a party or friendly player attacking me or mine reads as HOSTILE — the duel case.
    for victim in [C::Me, C::MyPet, C::Party, C::PartyPet] {
        assert_eq!(hits(C::Party, victim), K::CombatHostilePlayerHits);
        assert_eq!(hits(C::FriendlyPlayer, victim), K::CombatHostilePlayerHits);
    }
    assert_eq!(hits(C::HostilePlayer, C::Me), K::CombatHostilePlayerHits);
    assert_eq!(hits(C::HostilePet, C::Me), K::CombatHostilePlayerHits);
    // src 8/9 split on the VICTIM, into three buckets.
    assert_eq!(hits(C::Creature, C::Me), K::CombatCreatureVsSelfHits);
    assert_eq!(hits(C::Creature, C::MyPet), K::CombatCreatureVsSelfHits);
    assert_eq!(hits(C::Creature, C::Party), K::CombatCreatureVsPartyHits);
    assert_eq!(hits(C::Creature, C::PartyPet), K::CombatCreatureVsPartyHits);
    for victim in [C::FriendlyPlayer, C::HostilePlayer, C::Creature] {
        assert_eq!(hits(C::Creature, victim), K::CombatCreatureVsCreatureHits);
    }
}

/// The spell matrix is the melee one's shape with a different base row (`0x627820`), and the
/// periodic matrix is a genuinely different, smaller one: no PET row, no `CREATURE_VS_*` split, and
/// **the victim is not consulted at all**.
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

/// The variant picker: only class 0 is `SELF`. Your own pet is an `OTHER` for string selection even
/// though the msgType matrix gives it its own row — the two classifications are independent, and
/// conflating them is the obvious way to get "Your pet hits X" wrong.
#[test]
fn only_the_player_is_self_for_string_selection() {
    use UnitClass as C;
    assert_eq!(Variant::of(C::Me, C::Creature), Variant::SelfOther);
    assert_eq!(Variant::of(C::Creature, C::Me), Variant::OtherSelf);
    assert_eq!(Variant::of(C::MyPet, C::Creature), Variant::OtherOther);
    assert_eq!(Variant::of(C::Creature, C::MyPet), Variant::OtherOther);
    assert_eq!(Variant::of(C::Me, C::Me), Variant::SelfSelf);
}

/// **One `Unknown` endpoint does not drop the line; two do.** 1571 dropped on either, which was a
/// consequence of reading the range gate as an AND — class 9's range of `0.0` means it can never
/// satisfy its own half, but the other endpoint's half still carries the line.
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

/// The miss-code table, against the reference's own jump table (`0x62bb50`, table `0x62bde8`).
///
/// The default arm is the interesting one: `lea eax,[edi-2]; cmp eax,9; ja default` puts **0, 1,
/// 10 and everything out of range** on `SPELLMISS*`. 1571 had 10 (ABSORB) on `SPELLLOGABSORB` —
/// a family this switch never reaches — and dropped 0/1/out-of-range entirely.
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

/// The range table, read out of `WoW.exe` at `0x8629e0` — the evidence the class indices are what
/// they are named. Pinned here so a later reshuffle of the enum has to face it.
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

/// **The eight range CVars reach the gate** — the table is live, not compiled in.
///
/// The reference registers all eight in one place (`0x626d00`) and looks each up by name at every
/// use; we resolve them once into [`CombatLogRanges`]. What this pins is the property that made
/// them worth registering: a `SetCVar` moves the number the gate actually compares against, for
/// each of the seven classes independently and for the death line separately.
///
/// The control is class 0: it has NO CVar in the reference's table, so no name may reach it.
#[test]
fn a_set_cvar_moves_the_range_the_gate_compares_against() {
    use super::{CombatLogRanges, UnitClass as C};

    let mut r = CombatLogRanges::default();
    // Seeded from the reference's own `{cvarName, defaultValue}` pairs.
    assert_eq!(r.class(C::Party), 50.0);
    assert_eq!(r.class(C::Creature), 30.0);
    assert_eq!(r.class(C::Me), 100_000.0);
    assert_eq!(r.death(), 60.0);

    // Each of the seven moves its own class and nothing else — BigWigs' slider, which wrote
    // nothing at all before these rows were registered.
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

    // Zero is a real value, not "unset": the reference's `dist² < 0` is never true, so the class
    // goes silent. Nothing may clamp it up to a floor.
    assert!(r.set("CombatLogRangePartyPet", 0.0));
    assert_eq!(r.class(C::PartyPet), 0.0);

    // The control: classes 0 and 1 have a NULL name in the reference's table, and an unrelated
    // name is refused rather than silently swallowed.
    assert!(!r.set("CombatLogRangeMe", 10.0));
    assert!(!r.set("mousespeed", 10.0));
    assert_eq!(r.class(C::Me), 100_000.0);
}
