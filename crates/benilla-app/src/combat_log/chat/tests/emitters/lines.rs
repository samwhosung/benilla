//! The chat type of the split-damage and extra-attack lines, and the open-lock line.

use benilla_protocol::field::FIELD_OBJECT_ENTRY;
use benilla_protocol::messages::{ExecuteLog, SpellDamageLog, SpellLogExecute};

use super::{
    execute, row, spell_hit, stems, words, Bench, CLEANSE, FIREBALL, HIDDEN, HOSTILE, ME, MOB, ORC,
    PICK_LOCK, UNDISPLAYED,
};
use crate::net::ObjectStore;
use crate::target::relations::fixture::creature;
use crate::ui_chat::combat::PendingCombat;
use crate::ui_chat::ChatEventKind as K;

fn kinds(lines: &[PendingCombat]) -> Vec<K> {
    lines.iter().map(|l| l.kind).collect()
}

/// `0x62de60` picks its chat type by the line's form (`0x62e000`): you as the source of another
/// unit's damage (`SELFOTHER`) goes through the buff table `0x627820` (`0x62df92`), whose row for
/// a source of class 0 is `SPELL_SELF_BUFF`; the other two forms go through the damage table
/// `0x626be0` (`0x62df69`, `0x62dfc4`).
#[test]
fn split_damage_you_cause_is_typed_as_a_buff() {
    const OTHER: u64 = 3;
    let split = |attacker, target| {
        let mut bench = Bench::new();
        bench.hold(&creature(OTHER, ORC));
        let s = SpellDamageLog {
            attacker,
            target,
            hit_info: 0x8,
            ..spell_hit(120, 0, 0, 0)
        };
        bench.lines(|ctx, st, po, log| super::super::super::spell_damage_log(s, ctx, st, po, log))
    };
    let mine = split(ME, MOB);
    assert_eq!(stems(&mine), ["SPELLSPLITDAMAGE"]);
    assert_eq!(kinds(&mine), [K::SpellSelfBuff]);
    assert_eq!(kinds(&split(MOB, ME)), [K::SpellCreatureVsSelfDamage]);
    assert_eq!(
        kinds(&split(MOB, OTHER)),
        [K::SpellCreatureVsCreatureDamage]
    );
}

/// `0x62d9f0` types its line with `0x62b190` (`0x62db78`): the damage table for a spell
/// `GetSpellHostility` (`0x6ea280`) calls hostile, else the buff table, so a self-targeted
/// extra-attack proc is `SPELL_SELF_BUFF`.
#[test]
fn extra_attacks_are_typed_by_the_spells_hostility() {
    let extra = |spell_id, unit| {
        execute(
            spell_id,
            unit,
            vec![(
                19,
                vec![ExecuteLog::ExtraAttacks {
                    target: unit,
                    count: 1,
                }],
            )],
        )
    };
    assert_eq!(kinds(&extra(FIREBALL, ME)), [K::SpellSelfBuff]);
    assert_eq!(kinds(&extra(HOSTILE, ME)), [K::SpellSelfDamage]);
    assert_eq!(
        kinds(&extra(FIREBALL, MOB)),
        [K::SpellCreatureVsCreatureBuff]
    );
    assert_eq!(
        kinds(&extra(HOSTILE, MOB)),
        [K::SpellCreatureVsCreatureDamage]
    );
}

const CHEST: u64 = 0xf110_0000_0000_0007;
const JUNKBOX: u64 = 0x4000_0000_0000_0042;
const CHEST_ENTRY: u32 = 1731;
const JUNKBOX_ENTRY: u32 = 2589;
/// `Effect` 59, `OPEN_LOCK_ITEM`.
const OPEN_LOCK_ITEM: u32 = 59;

/// A bench holding a chest, a game object whose template is cached, and a junkbox, an item
/// whose template is.
fn lock_bench() -> Bench {
    let mut bench = Bench::new();
    for (guid, mask, entry) in [(CHEST, 0x21, CHEST_ENTRY), (JUNKBOX, 0x3, JUNKBOX_ENTRY)] {
        let e = bench.world.spawn_empty().id();
        bench.index.0.insert(guid, e);
        bench.stores.insert(
            e,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                // `OBJECT_FIELD_TYPE`.
                (2, mask),
                (FIELD_OBJECT_ENTRY, entry),
            ])),
        );
    }
    bench
        .gameobjects
        .insert(CHEST_ENTRY, 3, "Copper Vein".into(), &[0; 24]);
    bench
        .items
        .insert_template(JUNKBOX_ENTRY, Some(crate::items::test_template("Junkbox")));
    bench
}

fn open(bench: &Bench, spell_id: u32, caster: u64, effect: u32, target: u64) -> Vec<PendingCombat> {
    let s = SpellLogExecute {
        caster,
        spell_id,
        effects: vec![(effect, vec![row(target)])],
    };
    bench.lines(|ctx, st, po, log| super::super::super::spell_log_execute(&s, ctx, st, po, log))
}

/// `SMSG_SPELLLOGEXECUTE`'s effects 33 and 59 go to `LogOpenLock` (`0x62dc10`, from `0x5e82f7`),
/// which names the opened game object from the object cache (`0x62ddac`) or item from the item
/// cache (`0x62dce2`) and writes `OPEN_LOCK_SELF` or `OPEN_LOCK_OTHER`, typed by the caster against
/// itself through the spell's hostility (`0x62dd36`).
#[test]
fn opening_a_lock_names_the_opened_object_or_item() {
    let bench = lock_bench();
    let chest = open(&bench, PICK_LOCK, ME, 33, CHEST);
    assert_eq!(stems(&chest), ["OPEN_LOCK_"]);
    assert_eq!(chest[0].fills.spell, "Pick Lock");
    assert_eq!(chest[0].fills.named, "Copper Vein");
    assert_eq!(kinds(&chest), [K::SpellSelfBuff]);
    assert_eq!(words(&chest), [None], "no word follows the line");

    let junkbox = open(&bench, PICK_LOCK, ME, OPEN_LOCK_ITEM, JUNKBOX);
    assert_eq!(stems(&junkbox), ["OPEN_LOCK_"]);
    assert_eq!(junkbox[0].fills.named, "Junkbox");

    // Another unit opening it: its own type, its name in the sentence.
    let theirs = open(&bench, PICK_LOCK, MOB, 33, CHEST);
    assert_eq!(kinds(&theirs), [K::SpellCreatureVsCreatureBuff]);
    assert_eq!(theirs[0].subject, MOB);
}

/// `LogOpenLock` writes nothing for a target the client holds no record of (`0x62dce7`,
/// `0x62ddb1`) or holds as neither an item nor a game object (`0x62dd90`), for a caster the
/// client holds as no unit (`0x628010`), or for a spell with no record, flagged `0x180` or
/// nameless (`0x62dc35`-`0x62dc78`).
#[test]
fn opening_a_lock_needs_its_target_its_record_and_a_unit_caster() {
    let mut bench = lock_bench();
    assert_eq!(open(&bench, CLEANSE, ME, 33, CHEST).len(), 1, "any spell");
    assert!(open(&bench, PICK_LOCK, ME, 33, 0xf110_0000_0000_0099).is_empty());
    assert!(open(&bench, PICK_LOCK, ME, 33, MOB).is_empty(), "a unit");
    assert!(
        open(&bench, PICK_LOCK, 99, 33, CHEST).is_empty(),
        "not held"
    );
    assert!(
        open(&bench, HIDDEN, ME, 33, CHEST).is_empty(),
        "Attributes & 0x100"
    );
    assert!(
        open(&bench, UNDISPLAYED, ME, 33, CHEST).is_empty(),
        "Attributes & 0x80"
    );
    assert!(open(&bench, 24_000, ME, 33, CHEST).is_empty(), "no record");
    bench.gameobjects = crate::go_templates::GameObjectTemplates::default();
    bench.items = crate::items::Items::default();
    assert!(
        open(&bench, PICK_LOCK, ME, 33, CHEST).is_empty(),
        "no template"
    );
    assert!(open(&bench, PICK_LOCK, ME, OPEN_LOCK_ITEM, JUNKBOX).is_empty());
}
