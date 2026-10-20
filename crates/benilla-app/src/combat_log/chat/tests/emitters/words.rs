//! The `COMBAT_TEXT_UPDATE` words the remaining formatters raise after their lines, each under the
//! formatter's own you-only test, and the environmental log's `UNIT_COMBAT`.

use benilla_protocol::messages::{
    DamageShield, DispelFailed, EnvironmentalDamageLog, ExecuteLog, PeriodicTick, SpellDispelLog,
    SpellOutcomeLog,
};

use super::{execute, periodic, stems, words, Bench, CLEANSE, FIREBALL, ME, MOB, RENEW};
use crate::ui_chat::combat::{LineWord, PendingCombat};

fn shield(victim: u64, attacker: u64, damage: u32) -> Vec<PendingCombat> {
    let s = DamageShield {
        victim,
        attacker,
        damage,
        school: 3,
    };
    Bench::new().lines(|ctx, st, po, log| super::super::super::damage_shield(s, ctx, st, po, log))
}

/// `0x62ca20` raises `DAMAGE_SHIELD` with the damage after its line when the unit that struck the
/// shield, its second unit, is you (`0x62cb5d`-`0x62cb6b`), and writes neither for no damage
/// (`0x62ca50`).
#[test]
fn a_damage_shields_hit_on_you_raises_damage_shield() {
    let on_me = shield(MOB, ME, 12);
    assert_eq!(stems(&on_me), ["DAMAGESHIELD"]);
    assert_eq!(words(&on_me), [Some(LineWord::with("DAMAGE_SHIELD", 12))]);
    assert_eq!(words(&shield(ME, MOB, 12)), [None], "your shield's hit");
    assert!(shield(MOB, ME, 0).is_empty(), "no damage");
}

/// `0x626a10` raises `INTERRUPT` with the interrupted spell's name when the interrupted unit is you
/// (`0x626b53`-`0x626b69`); `0x62d9f0` raises `EXTRA_ATTACKS` with the count when the unit that gains
/// them is you (`0x62db7d`-`0x62dba9`), and writes neither for a count of 0 (`0x62da2a`).
#[test]
fn an_interrupt_and_extra_attacks_on_you_raise_their_words() {
    let interrupt = |target| {
        execute(
            CLEANSE,
            MOB,
            vec![(
                68,
                vec![ExecuteLog::InterruptCast {
                    target,
                    spell_id: FIREBALL,
                }],
            )],
        )
    };
    let on_me = interrupt(ME);
    assert_eq!(stems(&on_me), ["SPELLINTERRUPT"]);
    assert_eq!(
        words(&on_me),
        [Some(LineWord::with("INTERRUPT", "Fireball"))]
    );
    assert_eq!(words(&interrupt(MOB)), [None]);

    let extra = |target, count| {
        execute(
            FIREBALL,
            target,
            vec![(19, vec![ExecuteLog::ExtraAttacks { target, count }])],
        )
    };
    let mine = extra(ME, 2);
    assert_eq!(stems(&mine), ["SPELLEXTRAATTACKS"]);
    assert_eq!(words(&mine), [Some(LineWord::with("EXTRA_ATTACKS", 2))]);
    assert_eq!(words(&extra(MOB, 2)), [None]);
    assert!(extra(ME, 0).is_empty(), "no attacks: no line, no word");
}

fn outcome(immune: bool, caster: u64, target: u64, log_format: u8) -> Vec<PendingCombat> {
    let s = SpellOutcomeLog {
        caster,
        target,
        spell_id: FIREBALL,
        log_format,
    };
    Bench::new().lines(|ctx, st, po, log| {
        super::super::super::spell_outcome_log(s, immune, ctx, st, po, log)
    })
}

/// `SMSG_PROCRESIST` raises `PROC_RESISTED` (`0x628b8a`-`0x628b96`) and `SMSG_SPELLORDAMAGE_IMMUNE`
/// `SPELL_IMMUNE` (`0x62d3ab`-`0x62d3b9`), each with the spell's name, when the second unit is you.
#[test]
fn a_resisted_proc_and_an_immunity_on_you_raise_their_words() {
    let named = |message_type| Some(LineWord::with(message_type, "Fireball"));
    assert_eq!(words(&outcome(false, MOB, ME, 0)), [named("PROC_RESISTED")]);
    assert_eq!(words(&outcome(false, ME, MOB, 0)), [None]);
    assert_eq!(words(&outcome(true, MOB, ME, 0)), [named("SPELL_IMMUNE")]);
    assert_eq!(words(&outcome(true, ME, MOB, 0)), [None]);
}

/// `SMSG_DISPEL_FAILED` raises `DISPEL_FAILED` after each spell's line when its second unit is you
/// (`0x628daa`-`0x628db8`); `SMSG_SPELLDISPELLOG` raises `SPELL_DISPELLED` after each
/// `AURADISPELSELF` line (`0x62d5ce`-`0x62d5e8`).
#[test]
fn a_failed_dispel_and_a_dispelled_aura_on_you_raise_their_words() {
    let failed = |caster, victim| {
        let s = DispelFailed {
            caster,
            victim,
            spell_ids: vec![FIREBALL, RENEW],
        };
        Bench::new()
            .lines(|ctx, st, po, log| super::super::super::dispel_failed(&s, ctx, st, po, log))
    };
    assert_eq!(
        words(&failed(MOB, ME)),
        [
            Some(LineWord::with("DISPEL_FAILED", "Fireball")),
            Some(LineWord::with("DISPEL_FAILED", "Renew"))
        ]
    );
    assert_eq!(words(&failed(ME, MOB)), [None, None]);

    let dispelled = |victim, caster| {
        let s = SpellDispelLog {
            victim,
            caster,
            spell_ids: vec![FIREBALL],
        };
        Bench::new()
            .lines(|ctx, st, po, log| super::super::super::spell_dispel_log(&s, ctx, st, po, log))
    };
    assert_eq!(
        words(&dispelled(ME, MOB)),
        [Some(LineWord::with("SPELL_DISPELLED", "Fireball"))]
    );
    assert_eq!(words(&dispelled(MOB, ME)), [None]);
}

fn environment(
    victim: u64,
    damage_type: u8,
    damage: u32,
    absorb: u32,
    resist: i32,
) -> EnvironmentalDamageLog {
    EnvironmentalDamageLog {
        victim,
        damage_type,
        damage,
        absorb,
        resist,
    }
}

fn env_lines(e: EnvironmentalDamageLog) -> Vec<PendingCombat> {
    Bench::new().lines(|ctx, st, po, log| {
        super::super::super::environmental_damage_log(e, ctx, st, po, log)
    })
}

/// `0x62aac0` raises its word when the victim is you (`0x62ac74`): `RESIST` with the damage and a
/// resisted amount above 0 (`0x62ac7e`), else `ABSORB` with an absorbed one (`0x62ac99`), else
/// `DAMAGE` (`0x62acad`).
#[test]
fn environmental_damage_on_you_raises_its_word() {
    let word = |e| words(&env_lines(e));
    assert_eq!(
        word(environment(ME, 2, 245, 0, 0)),
        [Some(LineWord::with("DAMAGE", 245))]
    );
    assert_eq!(
        word(environment(ME, 3, 50, 20, 10)),
        [Some(LineWord::pair("RESIST", 50, 10))]
    );
    assert_eq!(
        word(environment(ME, 3, 50, 20, -10)),
        [Some(LineWord::pair("ABSORB", 50, 20))],
        "a negative resist is no resist"
    );
    assert_eq!(word(environment(MOB, 2, 245, 0, 0)), [None]);
}

/// `0x624f30` writes the line (`0x624f7c`), then for the active player (`0x624f83`) signals
/// `UNIT_COMBAT` `WOUND` (`0x624fc4`) in the type's school from `0x809274` (0, 0, 0, 2, 3, 2), marked
/// `0x20` for an absorb and `0x40` for a resist; a hit of all zeros writes and signals nothing
/// (`0x624f4c`-`0x624f60`).
#[test]
fn environmental_damage_on_you_flashes_wound_after_its_line() {
    let run = |e: EnvironmentalDamageLog| {
        Bench::new().run(|ctx, st, po, log| {
            super::super::super::environmental_damage_log(e, ctx, st, po, log)
        })
    };
    let lava = run(environment(ME, 3, 50, 0, 0));
    assert_eq!(
        lava.pending_combat_order(),
        ["VSENVIRONMENTALDAMAGE_LAVA_", "UNIT_COMBAT WOUND"]
    );
    let flash = |e| {
        run(e)
            .pending_unit_combat()
            .into_iter()
            .map(|f| (f.guid, f.action, f.flags, f.amount, f.school))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        flash(environment(ME, 3, 50, 0, 0)),
        [(ME, "WOUND", "", 50, 2)]
    );
    assert_eq!(
        flash(environment(ME, 4, 50, 0, 0)),
        [(ME, "WOUND", "", 50, 3)]
    );
    assert_eq!(
        flash(environment(ME, 5, 50, 0, 0)),
        [(ME, "WOUND", "", 50, 2)]
    );
    assert_eq!(
        flash(environment(ME, 2, 0, 30, 0)),
        [(ME, "WOUND", "ABSORB", 0, 0)]
    );
    assert_eq!(
        flash(environment(ME, 1, 0, 0, 30)),
        [(ME, "WOUND", "RESIST", 0, 0)]
    );
    assert_eq!(
        flash(environment(ME, 0, 0, 30, 30)),
        [(ME, "WOUND", "ABSORB", 0, 0)]
    );
    assert!(flash(environment(MOB, 3, 50, 0, 0)).is_empty(), "not you");
    let nothing = run(environment(ME, 2, 0, 0, 0));
    assert!(nothing.pending_combat_order().is_empty(), "all zeros");
}

/// `0x627930` raises the drained power's word with minus the amount drained when the target is you
/// (`0x627b94`-`0x627c5c`), from the execute log's drain and the periodic leech alike.
#[test]
fn power_drained_from_you_raises_its_power_word() {
    let drain = |target, power, amount| {
        execute(
            FIREBALL,
            MOB,
            vec![(
                8,
                vec![ExecuteLog::PowerDrain {
                    target,
                    amount,
                    power,
                    multiplier: 0.0,
                }],
            )],
        )
    };
    let mana = drain(ME, 0, 45);
    assert_eq!(stems(&mana), ["SPELLPOWERDRAIN"]);
    assert_eq!(words(&mana), [Some(LineWord::with("MANA", -45))]);
    assert_eq!(
        words(&drain(ME, 1, 100)),
        [Some(LineWord::with("RAGE", -10))]
    );
    assert_eq!(words(&drain(MOB, 0, 45)), [None]);
    let leech = periodic(vec![PeriodicTick::ManaLeech {
        power: 0,
        amount: 30,
        multiplier: 1.0,
    }]);
    assert_eq!(stems(&leech), ["SPELLPOWERLEECH"]);
    assert_eq!(words(&leech), [Some(LineWord::with("MANA", -30))]);
}

/// `0x62c5f0` raises `FACTION` with the faction's name and the signed change after its line, with
/// no class test (`0x62c661`-`0x62c667`). On the real DBC, Booty Bay.
#[test]
fn a_standing_change_raises_faction() {
    use crate::net::Reputations;
    use crate::target::ring::Factions;
    use crate::ui_chat::ChatLog;

    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
    let booty = u32::try_from(catalog.reputation_faction(21).unwrap().rep_index).unwrap();
    let name = catalog.faction_name(21).unwrap().to_string();
    let factions = Factions::from_catalog(catalog);
    let word = |standing| {
        let mut log = ChatLog::default();
        crate::combat_log::chat::faction_standing(
            &[(booty, standing)],
            &Reputations::default(),
            Some(&factions),
            &mut log,
        );
        words(
            &log.pending_combat()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        word(25),
        [Some(LineWord::text_number("FACTION", &name, 25))]
    );
    assert_eq!(
        word(-40),
        [Some(LineWord::text_number("FACTION", &name, -40))]
    );
}
