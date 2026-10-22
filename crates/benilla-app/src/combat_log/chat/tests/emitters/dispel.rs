//! The dispel line's gate: the bearer alone (`0x62d480`).

use std::collections::HashMap;

use benilla_protocol::messages::SpellDispelLog;
use bevy::prelude::*;

use super::{At, Bench, FIREBALL, ME, MOB};

/// A guid the client holds nothing for.
const STRANGER: u64 = 0x99;

/// `SMSG_SPELLDISPELLOG` of Fireball off `victim` by `caster`, with the creature `distance` yards
/// from you: the lines' keys.
fn dispelled(victim: u64, caster: u64, distance: f32) -> Vec<String> {
    let bench = Bench::new();
    let at = At(HashMap::from([
        (bench.index.0[&ME], Vec3::ZERO),
        (bench.index.0[&MOB], Vec3::new(distance, 0.0, 0.0)),
    ]));
    let s = SpellDispelLog {
        victim,
        caster,
        spell_ids: vec![FIREBALL],
    };
    bench
        .run_posed(&at, |ctx, st, po, log| {
            super::super::super::spell_dispel_log(&s, ctx, st, po, log)
        })
        .pending_combat()
        .into_iter()
        .map(|l| l.family.key(l.variant))
        .collect()
}

/// `ReadSpellDispelLog` hands the line writer the bearer, the packet's first guid, and the
/// dispeller (`0x5e8be4`); `0x62d480` never reads the dispeller. It returns unless the bearer is a
/// held unit (`0x62d4fb`-`0x62d504`), then tests you against the bearer's class range alone
/// (`0x62d516`-`0x62d58e`), so your own dispel of a creature beyond its 30 yards writes nothing.
#[test]
fn a_dispel_line_gates_on_its_bearers_range_alone() {
    assert_eq!(dispelled(MOB, ME, 20.0), ["AURADISPELOTHER"]);
    assert!(
        dispelled(MOB, ME, 40.0).is_empty(),
        "beyond the bearer's range"
    );
    assert!(
        dispelled(STRANGER, ME, 20.0).is_empty(),
        "a bearer the client does not hold"
    );
    assert_eq!(
        dispelled(ME, MOB, 40.0),
        ["AURADISPELSELF"],
        "a far dispeller of your aura"
    );
}
