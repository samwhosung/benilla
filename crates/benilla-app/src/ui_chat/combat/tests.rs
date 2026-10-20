//! The combat log's tests: the slot orders against the shipped `GlobalStrings.lua`, the msgType
//! matrices against the reference's selectors, and single lines.

use super::*;

/// A template's `%s`/`%d` conversions by argument, a numbered hole (`%2$d`, deDE's) at its number;
/// two templates with the same one take the same arguments.
fn signature(template: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut cursor = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' || chars.next_if_eq(&'%').is_some() {
            continue;
        }
        let mut number = String::new();
        while let Some(d) = chars.next_if(char::is_ascii_digit) {
            number.push(d);
        }
        let index = if number.is_empty() {
            cursor += 1;
            cursor - 1
        } else {
            chars.next_if_eq(&'$');
            number
                .parse::<usize>()
                .map_or(usize::MAX, |n| n.saturating_sub(1))
        };
        let Some(k) = chars.next() else { break };
        if index >= out.len() {
            out.resize(index.saturating_add(1).min(64), '?');
        }
        if let Some(slot) = out.get_mut(index) {
            *slot = k;
        }
    }
    out
}

/// deDE numbers its holes in the argument order enUS fills in sequence, so both read one list; a
/// hole past the list, a wrong type or an unused argument stays a mismatch.
#[test]
fn numbered_holes_fill_from_the_same_argument_list() {
    let hit = [Arg::Str("Ragnaros"), Arg::Num(1000)];
    assert_eq!(
        fill("You hit %s for %d.", &hit).as_deref(),
        Some("You hit Ragnaros for 1000.")
    );
    assert_eq!(
        fill("Ihr trefft %1$s. Schaden: %2$d.", &hit).as_deref(),
        Some("Ihr trefft Ragnaros. Schaden: 1000.")
    );
    let reflect = [Arg::Num(50), Arg::Str("Feuer"), Arg::Str("Ragnaros")];
    assert_eq!(
        fill("Ihr reflektiert %1$d %2$sschaden auf %3$s.", &reflect).as_deref(),
        Some("Ihr reflektiert 50 Feuerschaden auf Ragnaros.")
    );
    assert_eq!(
        fill("%2$s von %1$s", &[Arg::Str("A"), Arg::Str("B")]).as_deref(),
        Some("B von A")
    );
    assert_eq!(
        fill("100%% %1$s", &[Arg::Str("x")]).as_deref(),
        Some("100% x")
    );
    assert_eq!(fill("%3$s", &[Arg::Str("a")]), None);
    assert_eq!(fill("%1$d", &[Arg::Str("a")]), None);
    assert_eq!(fill("%1$s", &[Arg::Str("a"), Arg::Num(1)]), None);
    assert_eq!(fill("%0$s", &[Arg::Str("a")]), None);
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

// ── the unit classifier, `0x5efea0` ─────────────────────────────────────────────────────────

impl Stores for std::collections::HashMap<Entity, ObjectStore> {
    fn store(&self, entity: Entity) -> Option<&ObjectStore> {
        self.get(&entity)
    }
}

/// Descriptors held by guid, each at an entity of its own, for [`classify`].
struct Held {
    world: World,
    index: GuidIndex,
    stores: std::collections::HashMap<Entity, ObjectStore>,
}

impl Held {
    fn new() -> Self {
        Self {
            world: World::new(),
            index: GuidIndex::default(),
            stores: Default::default(),
        }
    }

    fn hold(&mut self, guid: u64, store: ObjectStore) {
        let e = self.world.spawn_empty().id();
        self.index.0.insert(guid, e);
        self.stores.insert(e, store);
    }

    fn unit(&mut self, u: &RelUnit) {
        self.hold(u.guid, crate::target::relations::fixture::store_of(u));
    }

    /// The class of `guid` to the active player `me`, with `party` in the party slots.
    fn class(&self, me: u64, party: &[u64], guid: u64) -> UnitClass {
        let group = GroupState {
            in_group: !party.is_empty(),
            members: party
                .iter()
                .map(|&guid| benilla_protocol::messages::GroupMemberEntry {
                    name: String::new(),
                    guid,
                    status: 1,
                    flags: 0,
                })
                .collect(),
            ..Default::default()
        };
        let factions = crate::target::relations::fixture::factions();
        classify(
            guid,
            &SelfGuid(Some(me)),
            Some(&group),
            &self.index,
            &self.stores,
            Some(&factions),
            &Reputations::default(),
            None,
        )
    }
}

/// `0x5eff85`-`0x5effad`: a player is hostile only when each can attack the other. An enemy who is
/// PvP-flagged while you are not is one you can attack and who cannot attack you: a friendly
/// player's lines, 4, until you flag too.
#[test]
fn a_player_is_hostile_only_when_each_can_attack_the_other() {
    use crate::target::relations::fixture::{player, with_flags, HUMAN, ORC, PVP};
    let me = player(1, HUMAN);
    let flagged_orc = with_flags(player(2, ORC), PVP);
    let mut held = Held::new();
    held.unit(&me);
    held.unit(&flagged_orc);
    assert_eq!(held.class(1, &[], 2), UnitClass::FriendlyPlayer);

    let mut held = Held::new();
    held.unit(&with_flags(me, PVP));
    held.unit(&flagged_orc);
    assert_eq!(held.class(1, &[], 2), UnitClass::HostilePlayer);
}

/// `0x5effcd` comes after the mutual `CanAttack`: a party member you are duelling is a hostile
/// player, 6, and the same member out of the duel is a party member, 2.
#[test]
fn a_party_member_in_a_duel_with_you_is_a_hostile_player() {
    use crate::target::relations::fixture::{duelling, player, HUMAN};
    let mut held = Held::new();
    held.unit(&duelling(player(1, HUMAN), 99, 1));
    held.unit(&duelling(player(2, HUMAN), 99, 2));
    assert_eq!(held.class(1, &[2], 2), UnitClass::HostilePlayer);

    let mut held = Held::new();
    held.unit(&player(1, HUMAN));
    held.unit(&player(2, HUMAN));
    assert_eq!(held.class(1, &[2], 2), UnitClass::Party);
}

/// `0x5f0006` is reached only for a unit that is not a player: a player you mind-control is judged
/// as a player, never as your pet; a creature you charm is your pet, 1.
#[test]
fn a_charmed_player_is_never_classified_by_its_controller() {
    use crate::target::relations::fixture::{creature, player, HUMAN, ORC};
    let me = player(1, HUMAN);
    let charmed = RelUnit {
        charmed_by: 1,
        ..player(2, ORC)
    };
    let charmed_mob = RelUnit {
        charmed_by: 1,
        ..creature(3, ORC)
    };
    let mut held = Held::new();
    held.unit(&me);
    held.unit(&charmed);
    held.unit(&charmed_mob);
    assert_eq!(held.class(1, &[], 2), UnitClass::FriendlyPlayer);
    assert_eq!(held.class(1, &[], 3), UnitClass::MyPet);
}

/// `0x5f00e3`-`0x5f00fd`: another player's pet is hostile, 7, when the pet and you can each
/// attack the other, else friendly, 5; a pet whose owner is not held is a creature, 8.
#[test]
fn another_players_pet_takes_the_mutual_test_on_itself() {
    use crate::target::relations::fixture::{pet_of, player, with_flags, HUMAN, ORC, PVP};
    let me = with_flags(player(1, HUMAN), PVP);
    let owner = with_flags(player(2, ORC), PVP);
    let mut held = Held::new();
    held.unit(&me);
    held.unit(&owner);
    held.unit(&pet_of(3, &owner));
    assert_eq!(held.class(1, &[], 3), UnitClass::HostilePet);

    let mut held = Held::new();
    held.unit(&player(1, HUMAN));
    held.unit(&owner);
    held.unit(&pet_of(3, &owner));
    assert_eq!(held.class(1, &[], 3), UnitClass::FriendlyPet);

    let mut held = Held::new();
    held.unit(&me);
    held.unit(&pet_of(3, &owner));
    assert_eq!(held.class(1, &[], 3), UnitClass::Creature);
}

/// `0x5eff1e`-`0x5eff45`: a game object stands for its creator, one class up on the player arms:
/// your own is 1, a party member's 3; a creature's object takes the creature's 8.
#[test]
fn a_game_object_classifies_as_its_creator_one_class_up() {
    use crate::target::relations::fixture::{creature, player, HUMAN};
    let object = |creator: u64| {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x21),
            (6, creator as u32),
            (7, (creator >> 32) as u32),
        ]))
    };
    let mut held = Held::new();
    held.unit(&player(1, HUMAN));
    held.unit(&player(2, HUMAN));
    held.unit(&creature(3, HUMAN));
    held.hold(10, object(1));
    held.hold(11, object(2));
    held.hold(12, object(3));
    held.hold(13, object(77));
    assert_eq!(held.class(1, &[2], 10), UnitClass::MyPet);
    assert_eq!(held.class(1, &[2], 11), UnitClass::PartyPet);
    assert_eq!(held.class(1, &[], 11), UnitClass::FriendlyPet);
    assert_eq!(held.class(1, &[], 12), UnitClass::Creature);
    assert_eq!(
        held.class(1, &[], 13),
        UnitClass::Unknown,
        "no creator held"
    );
}

/// `0x6264e0` never waits: a unit no cache names prints its miss at once, and only an item the
/// item cache has not answered holds the line (`0x55ba30`).
#[test]
fn a_unit_name_never_holds_a_line_and_an_item_name_does() {
    let mut line = queue(
        ChatEventKind::CombatCreatureVsSelfHits,
        COMBATHIT,
        (7, UnitClass::Creature),
        (1, UnitClass::Me),
        Fills::default(),
        Named::Ready,
    )
    .unwrap();
    let named = |guid: u64| if guid == 1 { "Pat" } else { "Unknown" }.to_string();
    assert!(line.fill_names(named, |_| ItemName::Waiting));
    assert_eq!(
        (line.fills.attacker.as_str(), line.fills.victim.as_str()),
        ("Unknown", "Pat")
    );

    let mut pet = PendingCombat {
        named: Named::Unit(9),
        ..line.clone()
    };
    assert!(pet.fill_names(|_| "Unknown".into(), |_| ItemName::Waiting));
    assert_eq!(pet.fills.named, "Unknown");

    let mut made = PendingCombat {
        named: Named::Item(2589),
        ..line.clone()
    };
    assert!(!made.fill_names(named, |_| ItemName::Waiting));
    assert!(made.fill_names(named, |_| ItemName::Named("Linen Cloth".into())));
    assert_eq!(made.fills.named, "Linen Cloth");
}

/// A name nothing holds prints the miss of the getter `0x6264e0` went through: a held unit's
/// `GetUnitName` (`0x626531`) falls to "Unknown Being" (`0x60933c`), anything else to
/// `UKNOWNOBJECT` (`0x62660c`), each only once `UNKNOWNOBJECT` is missing.
#[test]
fn an_unnamed_held_unit_prints_the_unit_getters_miss() {
    let script = benilla_ui::script::UiScript::new().expect("VM");
    let names = NameCache::default();
    let store = |kind: u32| ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, kind)]));
    let name = |store: Option<&ObjectStore>| logged_name(7, store, &names, Ask::Read, &script);
    assert_eq!(name(Some(&store(0x19))), "Unknown Being", "a held player");
    assert_eq!(name(Some(&store(0x9))), "Unknown Being", "a held creature");
    assert_eq!(name(Some(&store(0x21))), "UKNOWNOBJECT", "a game object");
    assert_eq!(name(None), "UKNOWNOBJECT", "nothing held");

    script
        .lua()
        .load("UNKNOWNOBJECT = \"Unknown\"")
        .exec()
        .expect("set");
    assert_eq!(name(Some(&store(0x19))), "Unknown");
    assert_eq!(name(None), "Unknown");
}

/// A school with no name writes no line, even where the template names none: each formatter that
/// takes a school returns on its name first (`0x62cd99`, `0x628117`), before the line and its word.
#[test]
fn a_school_with_no_name_writes_no_line() {
    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(r#"SPELLLOGABSORBOTHERSELF = "You absorb %s's %s."; SPELL_SCHOOL2_CAP = "Fire""#)
        .expect("strings");
    let fills = |school| Fills {
        attacker: "Imp".into(),
        spell: "Firebolt".into(),
        school: Some(school),
        ..Default::default()
    };
    assert_eq!(
        compose_line(&script, SPELLLOGABSORB, Variant::OtherSelf, &fills(2)).as_deref(),
        Some("You absorb Imp's Firebolt.")
    );
    assert_eq!(
        compose_line(&script, SPELLLOGABSORB, Variant::OtherSelf, &fills(9)),
        None
    );
}
