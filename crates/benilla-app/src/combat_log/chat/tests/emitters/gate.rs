//! The target half of the two-sided gate (`0x626630`).

use benilla_protocol::messages::{ExecuteLog, PartyKillLog, SpellDamageLog, SpellLogExecute};

use super::{melee, spell_damage, spell_hit, stems, swing, Bench, NoPoses, FIREBALL, ME, MOB};
use crate::net::ObjectStore;
use crate::ui_chat::combat::PendingCombat;

/// A guid the client holds nothing for.
const GONE: u64 = 0x99;

/// `0x626630` returns false when the lookup of its target finds nothing (`0x626690`-`0x6266a6`),
/// ahead of any range: your own swing or spell, whose class reaches 100,000 yards, writes no line
/// on a unit already destroyed for you.
#[test]
fn a_line_needs_its_target_held() {
    assert_eq!(stems(&melee(swing(ME, MOB, 120, 1))), ["COMBATHIT"]);
    assert!(melee(swing(ME, GONE, 120, 1)).is_empty(), "your swing");
    let spell = |target| {
        spell_damage(SpellDamageLog {
            target,
            attacker: ME,
            ..spell_hit(300, 0, 0, 0)
        })
    };
    assert_eq!(stems(&spell(MOB)), ["SPELLLOGSCHOOL"]);
    assert!(spell(GONE).is_empty(), "your spell");
}

/// A source not held is no refusal: `0x62674d` measures it from the target, so the line stands on
/// the target's range alone.
#[test]
fn a_line_stands_without_its_source() {
    assert_eq!(melee(swing(GONE, ME, 30, 1)).len(), 1);
}

/// The target's lookup has type mask 1 (`0x626689`): any live object answers it, a game object
/// included, though its class reaches no range of its own.
#[test]
fn a_target_held_as_any_object_passes() {
    const TRAP: u64 = 7;
    let mut bench = Bench::new();
    let e = bench.world.spawn_empty().id();
    bench.index.0.insert(TRAP, e);
    bench.stores.insert(
        e,
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x21)])),
    );
    let s = swing(ME, TRAP, 120, 1);
    let lines =
        bench.lines(|ctx, st, po, log| super::super::super::attacker_state(s, ctx, st, po, log));
    assert_eq!(stems(&lines), ["COMBATHIT"]);
}

fn party_kill(killer: u64, victim: u64) -> Vec<PendingCombat> {
    let s = PartyKillLog { killer, victim };
    Bench::new().lines(|ctx, st, po, log| super::super::super::party_kill_log(s, ctx, st, po, log))
}

/// `0x628890`'s gate takes the victim as its target (`0x6288e1`-`0x6288f8`), though the sentence
/// names it first: your kill of a unit already gone writes nothing.
#[test]
fn a_kill_line_needs_its_victim_held() {
    assert_eq!(stems(&party_kill(ME, MOB)), ["SELFKILLOTHER"]);
    assert!(party_kill(ME, GONE).is_empty());
}

/// The durability writer runs the gate again at its item's record (`0x629941`, target its second
/// unit): a target gone by then writes nothing, though it was held at the packet.
#[test]
fn a_waiting_durability_line_needs_its_target_held_at_its_record() {
    use crate::items::RecordCallback::CombatLog;
    use crate::ui_chat::combat::ItemLog;
    const BLADE: u32 = 4_243;
    let worn = SpellLogExecute {
        caster: ME,
        spell_id: FIREBALL,
        effects: vec![(
            111,
            vec![ExecuteLog::DurabilityDamage {
                target: MOB,
                item_entry: BLADE as i32,
                slot: 0,
            }],
        )],
    };
    let answered = |gone: bool| {
        let mut bench = Bench::new();
        let log = bench.run(|ctx, st, po, log| {
            super::super::super::spell_log_execute(&worn, ctx, st, po, log)
        });
        assert!(log.pending_combat().is_empty(), "waiting on its item");
        assert!(bench.items.waiting(BLADE, CombatLog(ItemLog::Durability)));
        bench
            .items
            .insert_template(BLADE, Some(crate::items::test_template("Blade")));
        if gone {
            bench.index.0.remove(&MOB);
        }
        bench
            .run_posed_on(log, &NoPoses, |ctx, st, po, log| {
                super::super::super::item_answered(
                    ItemLog::Durability,
                    BLADE,
                    true,
                    ctx,
                    st,
                    po,
                    log,
                )
            })
            .pending_combat()
            .len()
    };
    assert_eq!(answered(false), 1);
    assert_eq!(answered(true), 0);
}
