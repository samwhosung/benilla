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
        enchants: None,
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
        AttackerState, ExecuteLog, PeriodicAuraLog, PeriodicTick, SpellDamageLog, SpellEnergizeLog,
        SpellHealLog, SpellLogExecute, SpellLogMiss,
    };
    use bevy::prelude::*;

    use crate::net::{GuidIndex, ObjectStore, Reputations, SelfGuid};
    use crate::target::relations::fixture::{creature, factions, player, store_of, HUMAN, ORC};
    use crate::target::{Factions, RelUnit};
    use crate::ui_chat::combat::{CombatLogRanges, LineWord, PendingCombat, Poses, WordText};
    use crate::ui_chat::{ChatEventKind, ChatLog};

    use super::super::ChatCtx;

    mod words;

    const ME: u64 = 1;
    const MOB: u64 = 2;
    const FIREBALL: u32 = 133;
    /// A 15 s heal over time.
    const RENEW: u32 = 139;
    /// A 45 s one.
    const LONG_HOT: u32 = 8936;

    /// A spell with no effect that matters, cast by a lone `SMSG_SPELLLOGEXECUTE` row.
    const CLEANSE: u32 = 4987;
    /// An ability (`Attributes & 0x10`).
    const VANISH: u32 = 1856;
    /// Three the cast line skips: a learn-spell effect, `AttributesEx4 & 1`, an open-lock effect.
    const LEARN: u32 = 483;
    const UNLOGGED: u32 = 20_000;
    const PICK_LOCK: u32 = 1804;
    /// `Attributes & 0x100`: no line, no `UNIT_COMBAT`, no float.
    const HIDDEN: u32 = 20_001;
    /// `Attributes & 0x80` alone: no line, but its `UNIT_COMBAT` fires.
    const UNDISPLAYED: u32 = 20_002;

    /// Fireball with no duration row, the two heals with theirs, and the casts.
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
        let cast = |id, name: &str, attributes, attributes_ex4, effect| {
            (
                id,
                SpellDisplay {
                    id,
                    name: name.into(),
                    attributes,
                    attributes_ex4,
                    effects: [effect, 0, 0],
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
                    cast(CLEANSE, "Cleanse", 0, 0, 38),
                    cast(VANISH, "Vanish", 0x10, 0, 3),
                    cast(LEARN, "Learning", 0, 0, 36),
                    cast(UNLOGGED, "Unlogged", 0, 1, 38),
                    cast(PICK_LOCK, "Pick Lock", 0, 0, 33),
                    cast(HIDDEN, "Hidden", 0x100, 0, 0),
                    cast(UNDISPLAYED, "Undisplayed", 0x80, 0, 0),
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
        enchants: benilla_formats::EnchantCatalog,
    }

    impl Bench {
        fn new() -> Self {
            let mut bench = Self {
                world: World::new(),
                index: GuidIndex::default(),
                stores: HashMap::new(),
                factions: factions(),
                spells: spells(),
                enchants: benilla_formats::EnchantCatalog::from_rows(
                    HashMap::new(),
                    [(CRUSADER, "Crusader".to_string())].into(),
                    [(CRUSADER, 0), (ROCKBITER, 0x2)].into(),
                ),
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

        /// The lines and `UNIT_COMBAT`s `emit` parks, in queue order
        /// ([`ChatLog::pending_combat_order`]).
        fn order(
            &self,
            emit: impl FnOnce(&ChatCtx, &HashMap<Entity, ObjectStore>, &NoPoses, &mut ChatLog),
        ) -> Vec<String> {
            self.run(emit).pending_combat_order()
        }

        /// The `UNIT_COMBAT`s `emit` parks, as `(guid, action, descriptor, amount)`.
        fn flashes(
            &self,
            emit: impl FnOnce(&ChatCtx, &HashMap<Entity, ObjectStore>, &NoPoses, &mut ChatLog),
        ) -> Vec<(u64, &'static str, &'static str, u32)> {
            self.run(emit)
                .pending_unit_combat()
                .into_iter()
                .map(|f| (f.guid, f.action, f.flags, f.amount))
                .collect()
        }

        /// The lines `emit` parks, with their words.
        fn lines(
            &self,
            emit: impl FnOnce(&ChatCtx, &HashMap<Entity, ObjectStore>, &NoPoses, &mut ChatLog),
        ) -> Vec<PendingCombat> {
            self.run(emit)
                .pending_combat()
                .into_iter()
                .cloned()
                .collect()
        }

        /// The log `emit` leaves.
        fn run(
            &self,
            emit: impl FnOnce(&ChatCtx, &HashMap<Entity, ObjectStore>, &NoPoses, &mut ChatLog),
        ) -> ChatLog {
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
                enchants: Some(&self.enchants),
                ranges: &ranges,
                periodic: true,
            };
            let mut log = ChatLog::default();
            emit(&ctx, &self.stores, &NoPoses, &mut log);
            log
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

    /// `0x625e20` returns for a victim the client does not hold as a unit (`0x625e22`): the swing
    /// stays state 1 with no damage, which writes no line, where a held victim's is `VSBLOCK`.
    #[test]
    fn a_full_block_on_a_victim_held_as_no_unit_stays_as_sent() {
        let blocked = |victim| AttackerState {
            blocked: 30,
            ..swing(MOB, victim, 0, 1)
        };
        assert_eq!(stems(&melee(blocked(ME))), ["VSBLOCK"]);
        assert!(melee(blocked(99)).is_empty(), "not held");
        const TRAP: u64 = 7;
        let mut bench = Bench::new();
        let e = bench.world.spawn_empty().id();
        bench.index.0.insert(TRAP, e);
        bench.stores.insert(
            e,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x21)])),
        );
        let s = blocked(TRAP);
        let lines =
            bench.lines(|ctx, st, po, log| super::super::attacker_state(s, ctx, st, po, log));
        assert!(lines.is_empty(), "a game object");
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
                extended: false,
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

    /// The GO's reflect leg logs the reflected spell's outcome on its caster, the caster on both
    /// sides (`0x6e7dfd`-`0x6e7e14`), after the entry's own line; a reflect with no outcome has
    /// its line alone (`0x6e7dfb`).
    #[test]
    fn a_go_reflect_logs_the_outcome_on_the_caster() {
        use benilla_protocol::messages::SpellGoMiss;
        let entry = |reflect_result| SpellGoMiss {
            target: MOB,
            info: 11,
            reflect_result,
        };
        let misses = [entry(2), entry(0)];
        let lines = Bench::new().lines(|ctx, st, po, log| {
            super::super::spell_go_misses(ME, ME, FIREBALL, &misses, ctx, st, po, log)
        });
        assert_eq!(
            stems(&lines),
            ["SPELLREFLECT", "SPELLRESIST", "SPELLREFLECT"]
        );
        let ends: Vec<_> = lines.iter().map(|l| (l.subject, l.object)).collect();
        assert_eq!(ends, [(ME, MOB), (ME, ME), (ME, MOB)]);
        assert_eq!(words(&lines), [None, named("SPELL_RESISTED"), None]);
    }

    fn execute(
        spell_id: u32,
        caster: u64,
        effects: Vec<(u32, Vec<ExecuteLog>)>,
    ) -> Vec<PendingCombat> {
        let s = SpellLogExecute {
            caster,
            spell_id,
            effects,
        };
        Bench::new().lines(|ctx, st, po, log| super::super::spell_log_execute(&s, ctx, st, po, log))
    }

    fn row(target: u64) -> ExecuteLog {
        ExecuteLog::Target { target }
    }

    /// `0x62ace0` words a cast with a target as `SIMPLECAST`, raising `SPELL_CAST` with the
    /// spell's name when the target is you (`0x62aef7`), and an ability's as `SIMPLEPERFORM`.
    #[test]
    fn a_targeted_cast_line_words_you_as_its_target() {
        let on_me = execute(CLEANSE, MOB, vec![(38, vec![row(ME)])]);
        assert_eq!(stems(&on_me), ["SIMPLECAST"]);
        assert_eq!(
            words(&on_me),
            [Some(LineWord::with("SPELL_CAST", "Cleanse"))]
        );
        let mine = execute(CLEANSE, ME, vec![(38, vec![row(MOB)])]);
        assert_eq!(stems(&mine), ["SIMPLECAST"]);
        assert_eq!(words(&mine), [None]);
        let ability = execute(VANISH, ME, vec![(38, vec![row(MOB)])]);
        assert_eq!(stems(&ability), ["SIMPLEPERFORM"]);
    }

    /// An effect with no arm takes the cast line once per row with no target (`0x5e83e3`): the
    /// terse line on its caster, worded when that is you (`0x62b12f`); so does a lone row-less
    /// effect 3 (`0x5e8004`), and nothing else row-less.
    #[test]
    fn an_effect_with_no_arm_takes_the_terse_cast_line_per_row() {
        let summon = execute(VANISH, ME, vec![(28, vec![row(MOB), row(MOB)])]);
        assert_eq!(stems(&summon), ["SPELLTERSEPERFORM", "SPELLTERSEPERFORM"]);
        assert_eq!(
            words(&summon),
            [
                Some(LineWord::with("SPELL_CAST", "Vanish")),
                Some(LineWord::with("SPELL_CAST", "Vanish"))
            ]
        );
        let theirs = execute(CLEANSE, MOB, vec![(28, vec![row(ME)])]);
        assert_eq!(stems(&theirs), ["SPELLTERSE"]);
        assert_eq!(words(&theirs), [None]);
        assert_eq!(
            stems(&execute(VANISH, ME, vec![(3, vec![])])),
            ["SPELLTERSEPERFORM"]
        );
        assert!(execute(VANISH, ME, vec![(3, vec![]), (3, vec![])]).is_empty());
        assert!(execute(VANISH, ME, vec![(28, vec![])]).is_empty());
    }

    /// The cast line skips a spell with a learn-spell effect, `AttributesEx4 & 1` or an open-lock
    /// effect (`0x62ad24`-`0x62ad3c`).
    #[test]
    fn the_cast_line_skips_learning_unlogged_and_lock_spells() {
        for spell in [LEARN, UNLOGGED, PICK_LOCK] {
            assert!(
                execute(spell, MOB, vec![(38, vec![row(ME)])]).is_empty(),
                "{spell}"
            );
        }
    }

    /// An instakill row on its own caster writes nothing (`0x5e81a2`-`0x5e81b1`).
    #[test]
    fn a_self_inflicted_instakill_writes_no_cast_line() {
        assert!(execute(CLEANSE, MOB, vec![(1, vec![row(MOB)])]).is_empty());
        assert_eq!(
            stems(&execute(CLEANSE, MOB, vec![(1, vec![row(ME)])])),
            ["SIMPLECAST"]
        );
    }

    /// The terse line needs a caster the client holds as a unit (`0x62b030`-`0x62b039`): your
    /// own trap, a game object, classifies as yours and still writes none.
    #[test]
    fn a_terse_cast_line_needs_a_unit_caster() {
        const TRAP: u64 = 7;
        let mut bench = Bench::new();
        let e = bench.world.spawn_empty().id();
        bench.index.0.insert(TRAP, e);
        bench.stores.insert(
            e,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x21),
                (6, ME as u32),
                (7, (ME >> 32) as u32),
            ])),
        );
        let s = SpellLogExecute {
            caster: TRAP,
            spell_id: CLEANSE,
            effects: vec![(28, vec![row(ME)])],
        };
        let lines =
            bench.lines(|ctx, st, po, log| super::super::spell_log_execute(&s, ctx, st, po, log));
        assert!(lines.is_empty());
        assert!(execute(CLEANSE, 99, vec![(28, vec![row(ME)])]).is_empty());
    }

    /// `0x5e7f90` tests no record itself: only the formatters handed the packet's spell
    /// (`[ebp-0x14]`) return for one with none. Create item (`0x62d8d0`), feed pet (`0x62d800`) and
    /// dismiss pet (`0x62e360`) take no spell, and interrupt (`0x626a10`) takes the row's
    /// (`0x5e8268`); power drain, extra attacks and durability damage write nothing.
    #[test]
    fn an_unknown_spell_drops_only_the_arms_that_name_it() {
        const UNKNOWN: u32 = 424_242;
        let lines = execute(
            UNKNOWN,
            ME,
            vec![
                (24, vec![ExecuteLog::CreateItem { item_entry: 2589 }]),
                (101, vec![ExecuteLog::FeedPet { item_entry: 2589 }]),
                (102, vec![row(MOB)]),
                (
                    68,
                    vec![ExecuteLog::InterruptCast {
                        target: MOB,
                        spell_id: CLEANSE,
                    }],
                ),
                (
                    8,
                    vec![ExecuteLog::PowerDrain {
                        target: MOB,
                        amount: 40,
                        power: 0,
                        multiplier: 1.0,
                    }],
                ),
                (
                    19,
                    vec![ExecuteLog::ExtraAttacks {
                        target: ME,
                        count: 1,
                    }],
                ),
                (
                    111,
                    vec![ExecuteLog::DurabilityDamage {
                        target: MOB,
                        item_entry: -1,
                        slot: -1,
                    }],
                ),
                (28, vec![row(MOB)]),
            ],
        );
        assert_eq!(
            stems(&lines),
            [
                "TRADESKILL_LOG",
                "FEEDPET_LOG",
                "SPELLDISMISSPET",
                "SPELLINTERRUPT"
            ]
        );
    }

    /// `SpellItemEnchantment` rows: one named, one with `Flags & 0x2`.
    const CRUSADER: u32 = 1900;
    const ROCKBITER: u32 = 29;

    fn enchant(caster: u64, owner: u64, enchant_id: u32) -> Vec<PendingCombat> {
        let s = benilla_protocol::messages::EnchantmentLog {
            caster,
            owner,
            item_entry: 2589,
            enchant_id,
            show_affiliation: false,
        };
        Bench::new().lines(|ctx, st, po, log| super::super::enchantment_log(s, ctx, st, po, log))
    }

    /// The packet's id names a `SpellItemEnchantment` row through `0xc0d7d8` (`0x62922d`,
    /// `0x6290d3`), never a spell; a row with `Flags & 0x2` writes nothing (`0x62923e`,
    /// `0x6290e4`). The word carries the enchantment's name when the item is yours (`0x62937a`,
    /// `0x6291b6`).
    #[test]
    fn an_enchantment_line_names_its_enchantment_row() {
        let added = enchant(ME, ME, CRUSADER);
        assert_eq!(stems(&added), ["ITEMENCHANTMENTADD"]);
        assert_eq!(added[0].fills.spell, "Crusader");
        assert_eq!(
            words(&added),
            [Some(LineWord::with("ENCHANTMENT_ADDED", "Crusader"))]
        );
        let faded = enchant(0, ME, CRUSADER);
        assert_eq!(stems(&faded), ["ITEMENCHANTMENTREMOVE"]);
        assert_eq!(
            words(&faded),
            [Some(LineWord::with("ENCHANTMENT_REMOVED", "Crusader"))]
        );
        assert_eq!(words(&enchant(ME, MOB, CRUSADER)), [None], "not your item");
        assert!(enchant(ME, ME, ROCKBITER).is_empty(), "Flags & 0x2");
        assert!(enchant(0, ME, ROCKBITER).is_empty(), "Flags & 0x2");
        assert!(enchant(ME, ME, FIREBALL).is_empty(), "a spell id, no row");
        // A fade's owner must be held as a unit (`0x62902c`): your own trap, a game object the
        // classifier counts as yours, writes nothing.
        const TRAP: u64 = 7;
        let mut bench = Bench::new();
        let e = bench.world.spawn_empty().id();
        bench.index.0.insert(TRAP, e);
        bench.stores.insert(
            e,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x21),
                (6, ME as u32),
                (7, (ME >> 32) as u32),
            ])),
        );
        let fade = benilla_protocol::messages::EnchantmentLog {
            caster: 0,
            owner: TRAP,
            item_entry: 2589,
            enchant_id: CRUSADER,
            show_affiliation: false,
        };
        let lines =
            bench.lines(|ctx, st, po, log| super::super::enchantment_log(fade, ctx, st, po, log));
        assert!(lines.is_empty(), "a fade's owner held as no unit");
    }

    // ── `UNIT_COMBAT`: its gates and flags, and its place against the line ──

    fn hit_by(
        spell_id: u32,
        damage: u32,
        absorb: u32,
        resist: i32,
        blocked: u32,
    ) -> SpellDamageLog {
        SpellDamageLog {
            target: ME,
            attacker: MOB,
            spell_id,
            damage,
            school: 2,
            absorb,
            resist,
            periodic: false,
            blocked,
            hit_info: 0,
        }
    }

    fn damage(s: SpellDamageLog) -> Vec<(u64, &'static str, &'static str, u32)> {
        Bench::new().flashes(|ctx, st, po, log| super::super::spell_damage_log(s, ctx, st, po, log))
    }

    /// `0x5e85e0` writes the line (`0x5e87de`), then `UNIT_COMBAT` (`0x5e883f`).
    #[test]
    fn a_spell_hit_writes_its_line_then_its_unit_combat() {
        let s = hit_by(FIREBALL, 300, 0, 0, 0);
        let order = Bench::new()
            .order(|ctx, st, po, log| super::super::spell_damage_log(s, ctx, st, po, log));
        assert_eq!(order, ["SPELLLOGSCHOOL", "UNIT_COMBAT WOUND"]);
    }

    /// The flash is `WOUND`, its descriptor the first of `ABSORB`, `BLOCK` and `RESIST` when
    /// nothing got through (`0x49463b`-`0x494656`), the crit when something did; split damage
    /// flashes with no flags (`0x5e872d`-`0x5e876a`). It needs a record without
    /// `Attributes & 0x100` (`0x5e87e3`-`0x5e87f4`), whatever the line's own gate says.
    #[test]
    fn a_spell_hit_flashes_its_reason_for_a_spell_without_0x100() {
        assert_eq!(
            damage(hit_by(FIREBALL, 0, 0, 30, 40)),
            [(ME, "WOUND", "BLOCK", 0)]
        );
        assert_eq!(
            damage(hit_by(FIREBALL, 0, 50, 30, 40)),
            [(ME, "WOUND", "ABSORB", 0)]
        );
        assert_eq!(damage(hit_by(FIREBALL, 0, 0, 0, 0)), [(ME, "WOUND", "", 0)]);
        let crit = SpellDamageLog {
            hit_info: 0x2,
            ..hit_by(FIREBALL, 300, 50, 0, 0)
        };
        assert_eq!(damage(crit), [(ME, "WOUND", "CRITICAL", 300)]);
        let split = SpellDamageLog {
            hit_info: 0x8 | 0x2,
            ..hit_by(FIREBALL, 120, 50, 0, 0)
        };
        assert_eq!(damage(split), [(ME, "WOUND", "", 120)]);
        assert!(damage(hit_by(HIDDEN, 300, 0, 0, 0)).is_empty());
        assert!(damage(hit_by(24_000, 300, 0, 0, 0)).is_empty());
        assert_eq!(
            damage(hit_by(UNDISPLAYED, 300, 0, 0, 0)),
            [(ME, "WOUND", "", 300)]
        );
    }

    /// `0x5e84e0` writes the shield's line (`0x5e8552`), then `UNIT_COMBAT` on the striker
    /// (`0x5e858c`).
    #[test]
    fn a_damage_shield_writes_its_line_then_its_unit_combat() {
        let s = benilla_protocol::messages::DamageShield {
            victim: MOB,
            attacker: ME,
            damage: 12,
            school: 3,
        };
        let order =
            Bench::new().order(|ctx, st, po, log| super::super::damage_shield(s, ctx, st, po, log));
        assert_eq!(order, ["DAMAGESHIELD", "UNIT_COMBAT WOUND"]);
    }

    /// `0x5e7e00` flashes each target first (`0x5e7f23`), then writes its line (`0x5e7f3e`), and
    /// does neither for a spell with `Attributes & 0x100` (`0x5e7f15`). Code 0 flashes `NONE`
    /// (table `0x86582c`) and writes no line (`0x62babb`).
    #[test]
    fn a_spell_log_miss_flashes_each_target_then_writes_its_line() {
        let miss = |spell_id, misses| SpellLogMiss {
            spell_id,
            caster: MOB,
            extended: false,
            misses,
        };
        let s = miss(FIREBALL, vec![(ME, 3), (ME, 4)]);
        let order = Bench::new()
            .order(|ctx, st, po, log| super::super::spell_log_miss(&s, ctx, st, po, log));
        assert_eq!(
            order,
            [
                "UNIT_COMBAT DODGE",
                "SPELLDODGED",
                "UNIT_COMBAT PARRY",
                "SPELLPARRIED"
            ]
        );
        let s = miss(FIREBALL, vec![(ME, 0)]);
        let order = Bench::new()
            .order(|ctx, st, po, log| super::super::spell_log_miss(&s, ctx, st, po, log));
        assert_eq!(order, ["UNIT_COMBAT NONE"]);
        let s = miss(HIDDEN, vec![(ME, 3)]);
        assert!(Bench::new()
            .order(|ctx, st, po, log| super::super::spell_log_miss(&s, ctx, st, po, log))
            .is_empty());
    }

    /// `0x6e7a70` writes each entry's line (`0x6e7d95`), then flashes it (`0x6e7de5`); a reflect
    /// with an outcome then writes the outcome's line on the packet's second guid (`0x6e7dfd`,
    /// `0x6e7e14`) and flashes it (`0x6e7e5f`). A spell with `Attributes & 0x100` keeps its lines
    /// and loses its flashes (`0x6e7d9a`, `0x6e7e19`).
    #[test]
    fn a_go_miss_writes_its_line_then_its_unit_combat() {
        let entry = |target, info, reflect_result| benilla_protocol::messages::SpellGoMiss {
            target,
            info,
            reflect_result,
        };
        let misses = [entry(ME, 3, 0), entry(ME, 11, 2)];
        let bench = Bench::new();
        let order = bench.order(|ctx, st, po, log| {
            super::super::spell_go_misses(MOB, MOB, FIREBALL, &misses, ctx, st, po, log)
        });
        assert_eq!(
            order,
            [
                "SPELLDODGED",
                "UNIT_COMBAT DODGE",
                "SPELLREFLECT",
                "UNIT_COMBAT REFLECT",
                "SPELLRESIST",
                "UNIT_COMBAT RESIST"
            ]
        );
        let flashes = bench.flashes(|ctx, st, po, log| {
            super::super::spell_go_misses(MOB, MOB, FIREBALL, &misses, ctx, st, po, log)
        });
        let on: Vec<_> = flashes.iter().map(|f| (f.0, f.1)).collect();
        assert_eq!(on, [(ME, "DODGE"), (ME, "REFLECT"), (MOB, "RESIST")]);
        assert!(bench
            .flashes(|ctx, st, po, log| {
                super::super::spell_go_misses(MOB, MOB, HIDDEN, &misses, ctx, st, po, log)
            })
            .is_empty());
    }

    /// `0x5e89c0` flashes `HEAL` first, with no spell gate (`0x5e8a48`), then writes the line
    /// (`0x5e8a71`).
    #[test]
    fn a_heal_flashes_then_writes_its_line() {
        let heal = |spell_id, critical| SpellHealLog {
            target: ME,
            healer: MOB,
            spell_id,
            amount: 90,
            critical,
        };
        let s = heal(FIREBALL, true);
        let order = Bench::new()
            .order(|ctx, st, po, log| super::super::spell_heal_log(s, ctx, st, po, log));
        assert_eq!(order, ["UNIT_COMBAT HEAL", "HEALEDCRIT"]);
        let s = heal(HIDDEN, false);
        let flashes = Bench::new()
            .flashes(|ctx, st, po, log| super::super::spell_heal_log(s, ctx, st, po, log));
        assert_eq!(flashes, [(ME, "HEAL", "", 90)]);
    }

    /// `0x626dd0` flashes each tick before its line: a damage tick `WOUND` with its absorbed and
    /// resisted flags alone (`0x626fb4`, before `0x626ff4`), a heal tick `HEAL` (`0x627033`,
    /// before `0x62705e`); an energize tick only writes its line.
    #[test]
    fn a_periodic_tick_flashes_then_writes_its_line() {
        let s = PeriodicAuraLog {
            target: ME,
            caster: MOB,
            spell_id: RENEW,
            ticks: vec![
                PeriodicTick::Damage {
                    amount: 0,
                    school: 2,
                    absorb: 0,
                    resist: 12,
                },
                PeriodicTick::Heal { amount: 9 },
                PeriodicTick::Energize {
                    power: 0,
                    amount: 30,
                },
            ],
        };
        let bench = Bench::new();
        let order =
            bench.order(|ctx, st, po, log| super::super::periodic_aura_log(&s, ctx, st, po, log));
        assert_eq!(
            order,
            [
                "UNIT_COMBAT WOUND",
                "SPELLRESIST",
                "UNIT_COMBAT HEAL",
                "PERIODICAURAHEAL",
                "POWERGAIN"
            ]
        );
        let flashes =
            bench.flashes(|ctx, st, po, log| super::super::periodic_aura_log(&s, ctx, st, po, log));
        assert_eq!(flashes, [(ME, "WOUND", "RESIST", 0), (ME, "HEAL", "", 9)]);
        let hidden = PeriodicAuraLog {
            spell_id: HIDDEN,
            ..s
        };
        assert!(bench
            .order(|ctx, st, po, log| super::super::periodic_aura_log(&hidden, ctx, st, po, log))
            .is_empty());
    }
}
