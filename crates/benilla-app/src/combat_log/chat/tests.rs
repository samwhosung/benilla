use super::combat;

/// A spell's line with its catalog row, for the tests that need a known spell.
pub(crate) fn spells_with(rows: &[(u32, &str, u32)]) -> crate::ui_action::Spells {
    crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            rows.iter()
                .map(|&(id, name, attributes)| {
                    (
                        id,
                        benilla_formats::SpellDisplay {
                            id,
                            name: name.into(),
                            attributes,
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// No line for a spell the Spell store has no record of, as for one flagged `0x180` or with
/// an empty name: the test each spell formatter makes first (`0x62bac3`-`0x62bb07`).
#[test]
fn a_spell_with_no_record_writes_no_line() {
    let spells = spells_with(&[
        (133, "Fireball", 0),
        (8893, "Malfunction", 0x180),
        (7, "", 0),
    ]);
    let (self_guid, index, reputations) = (
        crate::net::SelfGuid(Some(1)),
        crate::net::GuidIndex::default(),
        crate::net::Reputations::default(),
    );
    let ranges = combat::CombatLogRanges::default();
    let ctx = super::ChatCtx {
        self_guid: &self_guid,
        group: None,
        index: &index,
        factions: None,
        reputations: &reputations,
        spells: Some(&spells),
        names: None,
        skill_lines: None,
        ranges: &ranges,
        periodic: true,
    };
    assert_eq!(ctx.spell_name(133).as_deref(), Some("Fireball"));
    assert_eq!(ctx.spell_name(24_000), None, "a server-only spell");
    assert_eq!(ctx.spell_name(8893), None);
    assert_eq!(ctx.spell_name(7), None);
}

/// A standing packet prints the "increased by" line for its first entry alone (`0x4d57dd`), and
/// not at all when that faction is `HIDDEN`; the factions a kill spills over to update
/// silently. On the real DBC, Booty Bay moving with Gadgetzan and Ratchet.
#[test]
fn a_standing_packet_prints_its_first_entry_only_and_never_a_hidden_one() {
    use crate::net::Reputations;
    use crate::target::ring::Factions;
    use crate::ui_chat::ChatLog;
    use benilla_formats::faction_flags::HIDDEN;

    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
    let slot = |id: u32| u32::try_from(catalog.reputation_faction(id).unwrap().rep_index).unwrap();
    let (booty, gadget, ratchet) = (slot(21), slot(369), slot(470));
    let factions = Factions::from_catalog(catalog);
    let packet = [(booty, 100), (gadget, 50), (ratchet, 50)];
    let kinds = |reps: &Reputations, deltas: &[(u32, i32)]| {
        let mut log = ChatLog::default();
        super::faction_standing(deltas, reps, Some(&factions), &mut log);
        log.pending_kinds()
    };

    let quiet = Reputations::default();
    assert_eq!(
        kinds(&quiet, &packet),
        ["combat"],
        "one line for the packet"
    );

    // Flag 4 on the first entry's slot silences the packet; on a later entry it changes nothing.
    let mut v = vec![(0u8, 0i32); 64];
    v[booty as usize].0 = HIDDEN;
    assert!(kinds(&Reputations::from_slots(v), &packet).is_empty());
    let mut v = vec![(0u8, 0i32); 64];
    v[gadget as usize].0 = HIDDEN;
    assert_eq!(kinds(&Reputations::from_slots(v), &packet), ["combat"]);

    // An unchanged first entry prints nothing, and the entries behind it still stay silent.
    let mut v = vec![(0u8, 0i32); 64];
    v[booty as usize].1 = 100;
    assert!(kinds(&Reputations::from_slots(v), &packet).is_empty());
}

mod emitters {
    use std::collections::HashMap;

    use benilla_protocol::messages::{
        AttackerState, PeriodicAuraLog, PeriodicTick, SpellDamageLog, SpellEnergizeLog,
        SpellHealLog, SpellLogMiss,
    };
    use bevy::prelude::*;

    use crate::net::{GuidIndex, ObjectStore, Reputations, SelfGuid};
    use crate::target::relations::fixture::{creature, factions, player, store_of, HUMAN, ORC};
    use crate::target::{Factions, RelUnit};
    use crate::ui_chat::combat::{CombatLogRanges, LineWord, PendingCombat, Poses, WordText};
    use crate::ui_chat::{ChatEventKind, ChatLog};

    use super::super::ChatCtx;

    const ME: u64 = 1;
    const MOB: u64 = 2;
    const FIREBALL: u32 = 133;
    /// A 15 s heal over time.
    const RENEW: u32 = 139;
    /// A 45 s one.
    const LONG_HOT: u32 = 8936;

    /// Fireball with no duration row, and the two heals with theirs.
    fn spells() -> crate::ui_action::Spells {
        use benilla_formats::{SpellDisplay, SpellDuration};
        let display = |id, name: &str, duration_index| {
            (
                id,
                SpellDisplay {
                    id,
                    name: name.into(),
                    duration_index,
                    ..Default::default()
                },
            )
        };
        let row = |ms| SpellDuration {
            base_ms: ms,
            per_level_ms: 0,
            max_ms: ms,
        };
        crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    display(FIREBALL, "Fireball", 0),
                    display(RENEW, "Renew", 7),
                    display(LONG_HOT, "Regrowth", 8),
                ]
                .into_iter()
                .collect(),
            ),
            durations: benilla_formats::SpellDurationCatalog::from_rows([
                (7, row(15_000)),
                (8, row(45_000)),
            ]),
            ..crate::ui_action::Spells::empty_for_tests()
        }
    }

    /// No pose held: every endpoint is in range.
    struct NoPoses;

    impl Poses for NoPoses {
        fn pose(&self, _: Entity) -> Option<Vec3> {
            None
        }
    }

    /// You, a human player, and an orc creature, each held as a unit, and a known Fireball.
    struct Bench {
        world: World,
        index: GuidIndex,
        stores: HashMap<Entity, ObjectStore>,
        factions: Factions,
        spells: crate::ui_action::Spells,
    }

    impl Bench {
        fn new() -> Self {
            let mut bench = Self {
                world: World::new(),
                index: GuidIndex::default(),
                stores: HashMap::new(),
                factions: factions(),
                spells: spells(),
            };
            bench.hold(&player(ME, HUMAN));
            bench.hold(&creature(MOB, ORC));
            bench
        }

        fn hold(&mut self, u: &RelUnit) {
            let e = self.world.spawn_empty().id();
            self.index.0.insert(u.guid, e);
            self.stores.insert(e, store_of(u));
        }

        /// The lines `emit` parks, with their words.
        fn lines(
            &self,
            emit: impl FnOnce(&ChatCtx, &HashMap<Entity, ObjectStore>, &NoPoses, &mut ChatLog),
        ) -> Vec<PendingCombat> {
            let (self_guid, reputations, ranges) = (
                SelfGuid(Some(ME)),
                Reputations::default(),
                CombatLogRanges::default(),
            );
            let ctx = ChatCtx {
                self_guid: &self_guid,
                group: None,
                index: &self.index,
                factions: Some(&self.factions),
                reputations: &reputations,
                spells: Some(&self.spells),
                names: None,
                skill_lines: None,
                ranges: &ranges,
                periodic: true,
            };
            let mut log = ChatLog::default();
            emit(&ctx, &self.stores, &NoPoses, &mut log);
            log.pending_combat().into_iter().cloned().collect()
        }
    }

    fn swing(attacker: u64, victim: u64, damage: u32, victim_state: u32) -> AttackerState {
        AttackerState {
            attacker,
            victim,
            hit_info: 0,
            damage,
            victim_state,
            absorb: 0,
            resist: 0,
            blocked: 0,
            melee_spell_id: 0,
            school: 0,
        }
    }

    fn melee(s: AttackerState) -> Vec<PendingCombat> {
        Bench::new().lines(|ctx, st, po, log| super::super::attacker_state(s, ctx, st, po, log))
    }

    fn stems(lines: &[PendingCombat]) -> Vec<&'static str> {
        lines.iter().map(|l| l.family.stem).collect()
    }

    fn words(lines: &[PendingCombat]) -> Vec<Option<LineWord>> {
        lines.iter().map(|l| l.word.clone()).collect()
    }

    /// `0x625e20` runs as the record is read (`0x625e0e`), ahead of the log: a swing on you with
    /// no damage and a blocked amount is state 5, a `VSBLOCK` line worded `BLOCK` (`0x62a45b`).
    #[test]
    fn a_full_block_is_a_block_line_and_word() {
        let lines = melee(AttackerState {
            blocked: 30,
            ..swing(MOB, ME, 0, 1)
        });
        assert_eq!(stems(&lines), ["VSBLOCK"]);
        assert_eq!(words(&lines), [Some(LineWord::bare("BLOCK"))]);
    }

    /// The word is the line's arm's (`0x629b60`), and only for a line about you: a missed swing
    /// is `MISS` whatever the state says, and your own swing raises nothing.
    #[test]
    fn a_swing_raises_its_arms_word_only_on_a_line_about_you() {
        let lines = melee(AttackerState {
            hit_info: 0x10,
            ..swing(MOB, ME, 0, 2)
        });
        assert_eq!(stems(&lines), ["MISSED"]);
        assert_eq!(words(&lines), [Some(LineWord::bare("MISS"))]);
        let mine = melee(swing(ME, MOB, 120, 1));
        assert_eq!(stems(&mine), ["COMBATHIT"]);
        assert_eq!(words(&mine), [None]);
    }

    /// `0x625e40` gates the formatter, and with it the word: a Heroic Strike you dodged has
    /// neither.
    #[test]
    fn a_strike_that_did_not_land_raises_no_word() {
        let lines = melee(AttackerState {
            melee_spell_id: 78,
            ..swing(MOB, ME, 0, 2)
        });
        assert!(lines.is_empty());
    }

    fn spell_hit(damage: u32, absorb: u32, resist: i32, blocked: u32) -> SpellDamageLog {
        SpellDamageLog {
            target: ME,
            attacker: MOB,
            spell_id: FIREBALL,
            damage,
            school: 2,
            absorb,
            resist,
            periodic: false,
            blocked,
            hit_info: 0,
        }
    }

    fn spell_damage(s: SpellDamageLog) -> Vec<PendingCombat> {
        Bench::new().lines(|ctx, st, po, log| super::super::spell_damage_log(s, ctx, st, po, log))
    }

    fn named(message_type: &'static str) -> Option<LineWord> {
        Some(LineWord::with(message_type, "Fireball"))
    }

    /// A direct hit with nothing through goes to its reason's formatter, each naming the spell in
    /// its word: absorbed `0x628696`, blocked through the miss formatter (`0x62cec7`), resisted
    /// `0x628806`; nothing at all writes nothing (`0x62ceda`).
    #[test]
    fn a_spell_stopped_whole_names_the_spell_in_its_word() {
        let absorbed = spell_damage(spell_hit(0, 50, 30, 40));
        assert_eq!(stems(&absorbed), ["SPELLLOGABSORB"]);
        assert_eq!(words(&absorbed), [named("SPELL_ABSORBED")]);
        let blocked = spell_damage(spell_hit(0, 0, 30, 40));
        assert_eq!(stems(&blocked), ["SPELLBLOCKED"]);
        assert_eq!(words(&blocked), [named("SPELL_BLOCKED")]);
        let resisted = spell_damage(spell_hit(0, 0, 30, 0));
        assert_eq!(stems(&resisted), ["SPELLRESIST"]);
        assert_eq!(words(&resisted), [named("SPELL_RESISTED")]);
        assert!(spell_damage(spell_hit(0, 0, 0, 0)).is_empty());
        // Damage through: the hit's own word.
        let hit = spell_damage(spell_hit(300, 0, 0, 0));
        assert_eq!(stems(&hit), ["SPELLLOGSCHOOL"]);
        assert_eq!(words(&hit), [Some(LineWord::with("SPELL_DAMAGE", 300))]);
    }

    /// Split damage is `0x62de60`'s, worded with the split amount (`0x62dfd9`); a periodic hit is
    /// `0x628100`'s, in the melee types, and silent while `CombatLogPeriodicSpells` is off.
    #[test]
    fn a_spell_damage_log_words_split_and_periodic_hits_by_their_own_formatters() {
        let split = spell_damage(SpellDamageLog {
            hit_info: 0x8,
            periodic: true,
            ..spell_hit(120, 0, 30, 0)
        });
        assert_eq!(stems(&split), ["SPELLSPLITDAMAGE"]);
        assert_eq!(words(&split), [Some(LineWord::with("SPLIT_DAMAGE", 120))]);
        let periodic = SpellDamageLog {
            periodic: true,
            ..spell_hit(300, 50, 0, 40)
        };
        let lines = spell_damage(periodic);
        assert_eq!(stems(&lines), ["PERIODICAURADAMAGE"]);
        assert_eq!(words(&lines), [Some(LineWord::pair("ABSORB", 300, 50))]);
    }

    fn periodic(ticks: Vec<PeriodicTick>) -> Vec<PendingCombat> {
        periodic_of(FIREBALL, ticks)
    }

    fn periodic_of(spell_id: u32, ticks: Vec<PeriodicTick>) -> Vec<PendingCombat> {
        let s = PeriodicAuraLog {
            target: ME,
            caster: MOB,
            spell_id,
            ticks,
        };
        Bench::new().lines(|ctx, st, po, log| super::super::periodic_aura_log(&s, ctx, st, po, log))
    }

    /// A periodic tick with nothing through is worded as its reason on the periodic types
    /// (`0x6281e5` absorbed, `0x62820e` resisted), typed off the target, or not at all.
    #[test]
    fn a_periodic_tick_with_nothing_through_is_worded_as_its_reason() {
        let tick = |absorb, resist| PeriodicTick::Damage {
            amount: 0,
            school: 2,
            absorb,
            resist,
        };
        let lines = periodic(vec![tick(30, 10), tick(0, 10), tick(0, 0)]);
        assert_eq!(stems(&lines), ["SPELLLOGABSORB", "SPELLRESIST"]);
        assert_eq!(
            words(&lines),
            [named("SPELL_ABSORBED"), named("SPELL_RESISTED")]
        );
        assert!(lines
            .iter()
            .all(|l| l.kind == ChatEventKind::SpellPeriodicSelfDamage));
    }

    /// `0x627520` raises its power word only with the periodic flag clear (`0x6276e2`): a mana
    /// tick on you has a line and no word, a direct gain both.
    #[test]
    fn only_a_direct_power_gain_raises_its_word() {
        let tick = periodic(vec![PeriodicTick::Energize {
            power: 0,
            amount: 40,
        }]);
        assert_eq!(stems(&tick), ["POWERGAIN"]);
        assert_eq!(words(&tick), [None]);
        let s = SpellEnergizeLog {
            target: ME,
            caster: MOB,
            spell_id: FIREBALL,
            power: 0,
            amount: 40,
        };
        let direct = Bench::new()
            .lines(|ctx, st, po, log| super::super::spell_energize_log(s, ctx, st, po, log));
        assert_eq!(words(&direct), [Some(LineWord::with("MANA", 40))]);
    }

    /// A heal on you is worded with the healer's name, the line's own subject, and the amount
    /// (`0x62c8ea`); a periodic heal likewise (`0x627414`).
    #[test]
    fn a_heal_on_you_names_its_healer_in_its_word() {
        let s = SpellHealLog {
            target: ME,
            healer: MOB,
            spell_id: FIREBALL,
            amount: 75,
            critical: true,
        };
        let lines = Bench::new()
            .lines(|ctx, st, po, log| super::super::spell_heal_log(s, ctx, st, po, log));
        let word = lines[0].word.clone().expect("a word");
        assert_eq!(
            (word.message_type, &word.data, word.extra.as_deref()),
            ("HEAL_CRIT", &WordText::SubjectName, Some("75"))
        );
        let mut line = lines[0].clone();
        line.fills.attacker = "Shaman".into();
        assert_eq!(line.word_data(&word).as_deref(), Some("Shaman"));
        let tick = periodic_of(RENEW, vec![PeriodicTick::Heal { amount: 12 }]);
        assert_eq!(words(&tick), [Some(LineWord::named("PERIODIC_HEAL", 12))]);
    }

    /// `0x627240` words a heal over time of 30 s or less (`0x627406`), and one with no duration
    /// row not at all (`0x6273d6`); the line is written either way.
    #[test]
    fn only_a_short_heal_over_time_raises_its_word() {
        let heal = || vec![PeriodicTick::Heal { amount: 12 }];
        let short = periodic_of(RENEW, heal());
        assert_eq!(words(&short), [Some(LineWord::named("PERIODIC_HEAL", 12))]);
        for spell in [LONG_HOT, FIREBALL] {
            let lines = periodic_of(spell, heal());
            assert_eq!(stems(&lines), ["PERIODICAURAHEAL"]);
            assert_eq!(words(&lines), [None], "spell {spell}");
        }
    }

    /// `0x62bab0` returns on code 0 before anything (`0x62babb`); a real code names the spell.
    #[test]
    fn a_spell_miss_of_code_zero_writes_nothing() {
        let miss = |code| {
            let s = SpellLogMiss {
                spell_id: FIREBALL,
                caster: MOB,
                misses: vec![(ME, code)],
            };
            Bench::new()
                .lines(|ctx, st, po, log| super::super::spell_log_miss(&s, ctx, st, po, log))
        };
        assert!(miss(0).is_empty());
        let immune = miss(7);
        assert_eq!(stems(&immune), ["SPELLIMMUNE"]);
        assert_eq!(words(&immune), [named("SPELL_IMMUNE")]);
    }
}
