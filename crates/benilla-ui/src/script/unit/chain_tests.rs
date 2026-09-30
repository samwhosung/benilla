//! The getters and `SpellCanTargetUnit` over a `target` chain: the token resolves to a guid
//! through [`UnitGuids`] (`0x515970`), and the guid's snapshot answers.

use std::collections::HashMap;

use crate::script::{UiScript, UnitGuids, UnitState};

const ME: u64 = 0x10;
const PET: u64 = 0xF140_0000_0000_0077;
/// Our target.
const MOB: u64 = 0xF130_0000_0000_0001;
/// party1 and raid2, who targets `BOSS`.
const P1: u64 = 0x21;
/// party2, held by no one.
const FAR: u64 = 0x22;
/// raid3, who targets `ADD`.
const R3: u64 = 0x33;
/// What party1 targets, and the unit `MOB`'s hop lands on party1 from.
const BOSS: u64 = 0xF130_0000_0000_00B0;
/// What raid3 targets: held, and never named by a base token.
const ADD: u64 = 0xF130_0000_0000_00AD;
/// What our pet targets.
const PET_FOE: u64 = 0xF130_0000_0000_00F0;
/// What `BOSS` targets, and nobody holds.
const GONE: u64 = 0xF130_0000_0000_0099;
/// Held, targets nobody, and no snapshot is pushed for it.
const SILENT: u64 = 0xF130_0000_0000_0055;
/// raid4, who targets `SILENT`.
const R4: u64 = 0x44;

fn state(name: &str, guid: u64, health: u32) -> UnitState {
    UnitState {
        exists: true,
        has_object: true,
        guid,
        name: Some(name.into()),
        health,
        max_health: 1000,
        level: 60,
        ..Default::default()
    }
}

/// We target the mob, which targets party1, who targets the boss, which targets a unit nobody
/// holds. raid3 targets the add, raid4 a held unit with no snapshot, our pet its foe, and party2
/// is out of range. Pushed by name: the bases. Pushed by guid: the units a chain ends on.
fn world() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_unit_guids(&UnitGuids {
        player: ME,
        pet: PET,
        target: MOB,
        party: [P1, FAR, 0, 0],
        raid: vec![ME, P1, R3, R4],
        held: HashMap::from([
            (ME, MOB),
            (PET, PET_FOE),
            (MOB, P1),
            (P1, BOSS),
            (BOSS, GONE),
            (R3, ADD),
            (R4, SILENT),
            (ADD, 0),
            (PET_FOE, 0),
            (SILENT, 0),
        ]),
        ..Default::default()
    });
    for (token, unit) in [
        ("player", state("Me", ME, 900)),
        ("target", state("Mob", MOB, 500)),
        ("party1", state("Pia", P1, 800)),
        ("pet", state("Pet", PET, 300)),
    ] {
        s.set_unit(token, Some(unit));
    }
    for unit in [
        state("Boss", BOSS, 4000),
        state("Add", ADD, 250),
        state("Foe", PET_FOE, 120),
        // The hops that land on a unit a base names.
        state("Mob", MOB, 500),
        state("Pia", P1, 800),
    ] {
        s.set_unit_by_guid(unit.guid, Some(unit));
    }
    s
}

fn eval<T: mlua::FromLuaMulti>(s: &UiScript, chunk: &str) -> T {
    s.eval(chunk).unwrap_or_else(|e| panic!("{chunk}: {e}"))
}

/// The getters read the snapshot of the unit the chain ends on, whatever the base is, and however
/// many hops the token takes.
#[test]
fn a_chain_answers_the_snapshot_of_the_unit_it_ends_on() {
    let s = world();
    for (chunk, want) in [
        (r#"return UnitName("party1target")"#, "Boss"),
        (r#"return UnitName("raid2target")"#, "Boss"),
        (r#"return UnitName("raid3target")"#, "Add"),
        (r#"return UnitName("pettarget")"#, "Foe"),
        (r#"return UnitName("playertarget")"#, "Mob"),
        (r#"return UnitName("targettarget")"#, "Pia"),
        (r#"return UnitName("targettargettarget")"#, "Boss"),
        (r#"return UnitName("playertargettargettarget")"#, "Boss"),
        // The compares fold case, the hops' too.
        (r#"return UnitName("PARTY1TARGET")"#, "Boss"),
        (r#"return UnitName("Raid3Target")"#, "Add"),
    ] {
        assert_eq!(eval::<String>(&s, chunk), want, "{chunk}");
    }
    assert_eq!(eval::<i64>(&s, r#"return UnitHealth("raid3target")"#), 250);
    assert_eq!(
        eval::<i64>(&s, r#"return UnitHealth("party1target")"#),
        4000
    );
    assert_eq!(
        eval::<i64>(&s, r#"return UnitHealthMax("party1target")"#),
        1000
    );
    assert_eq!(eval::<i64>(&s, r#"return UnitLevel("pettarget")"#), 60);
    assert!(eval::<bool>(&s, r#"return UnitExists("pettarget") == 1"#));
    assert!(eval::<bool>(
        &s,
        r#"return UnitExists("party1target") == 1"#
    ));
    assert!(eval::<bool>(
        &s,
        r#"return UnitIsUnit("targettarget", "party1") == 1"#
    ));
}

/// A chain that names nobody answers what the getter answers for no unit: nil, 0 and nil. A chain
/// to a guid nothing holds reads 0 and nil too, but its name is the reference's name-cache leg
/// (`0x5171eb`, else `UNKNOWNOBJECT` at `0x517216`), which is not built, so it is not asserted.
#[test]
fn a_chain_to_a_unit_nobody_holds_answers_nothing() {
    let s = world();
    // `BOSS` targets `GONE`, which is not held.
    for (lua, want) in [
        (r#"UnitHealth("party1targettarget")"#, "0"),
        (r#"UnitExists("party1targettarget")"#, "nil"),
    ] {
        assert_eq!(
            eval::<String>(&s, &format!("return tostring({lua})")),
            want,
            "{lua}"
        );
    }
    for token in [
        // `party2` is not held, so a hop off it names nobody.
        "party2target",
        // `SILENT` is held, and nothing pushed its snapshot.
        "raid4target",
        // A hop off a unit with no target, and a base past the roster.
        "raid3targettarget",
        "raid9target",
        "partypet1target",
    ] {
        assert!(
            eval::<bool>(&s, &format!(r#"return UnitName("{token}") == nil"#)),
            "{token}: name"
        );
        assert_eq!(
            eval::<i64>(&s, &format!(r#"return UnitHealth("{token}")"#)),
            0,
            "{token}: health"
        );
        assert!(
            eval::<bool>(&s, &format!(r#"return UnitExists("{token}") == nil"#)),
            "{token}: exists"
        );
    }
}

/// The plain tokens read the snapshot the app pushed under them, never a guid's: a base with no
/// push answers nothing even where its guid has an entry, and `"target"` is not `"party1target"`.
#[test]
fn a_token_without_a_hop_still_reads_its_own_push() {
    let mut s = world();
    for (chunk, want) in [
        (r#"return UnitName("player")"#, "Me"),
        (r#"return UnitName("target")"#, "Mob"),
        (r#"return UnitName("party1")"#, "Pia"),
        (r#"return UnitName("pet")"#, "Pet"),
        (r#"return UnitName("TARGET")"#, "Mob"),
    ] {
        assert_eq!(eval::<String>(&s, chunk), want, "{chunk}");
    }
    assert_eq!(eval::<i64>(&s, r#"return UnitHealth("target")"#), 500);

    // A guid entry for the unit a plain token resolves to is not that token's snapshot: the
    // app pushes each base by name.
    s.set_unit_by_guid(R3, Some(state("Raider", R3, 700)));
    assert!(eval::<bool>(&s, r#"return UnitName("raid3") == nil"#));
    assert_eq!(eval::<i64>(&s, r#"return UnitHealth("raid3")"#), 0);
    // A push under the token wins over a guid entry, which is what `targettarget` is.
    s.set_unit("targettarget", Some(state("Tot", P1, 1)));
    assert_eq!(
        eval::<String>(&s, r#"return UnitName("targettarget")"#),
        "Tot"
    );
    // Cleared, it falls to the chain.
    s.set_unit("targettarget", None);
    assert_eq!(
        eval::<String>(&s, r#"return UnitName("targettarget")"#),
        "Pia"
    );
    // And the entry is dropped with `None`.
    s.set_unit_by_guid(P1, None);
    assert!(eval::<bool>(
        &s,
        r#"return UnitName("targettarget") == nil"#
    ));
}

/// The junk after a hop and an unknown token behave as the resolver does: the first names
/// nobody, the second raises.
#[test]
fn a_malformed_chain_is_nobody_and_an_unknown_token_raises() {
    let s = world();
    for token in [
        "party1targetfoo",
        "party1 target",
        "party1targettarge",
        "targetfoo",
    ] {
        assert!(
            eval::<bool>(&s, &format!(r#"return UnitName("{token}") == nil"#)),
            "{token}"
        );
    }
    for chunk in [
        r#"return UnitName("npctarget")"#,
        r#"return UnitHealth("bogustarget")"#,
    ] {
        let err = s.eval::<mlua::Value>(chunk).expect_err(chunk).to_string();
        assert!(err.contains("Unknown unit name"), "{chunk}: {err}");
    }
}

/// `UnitInParty` judges a chain by the unit it ends on, not by the `party` in its text.
#[test]
fn unit_in_party_judges_the_unit_a_party_chain_ends_on() {
    let mut s = world();
    s.set_party(crate::script::PartyState {
        members: vec![crate::script::PartyMemberInfo {
            guid: P1,
            ..Default::default()
        }],
        ..Default::default()
    });
    assert!(eval::<bool>(&s, r#"return UnitInParty("party1") == 1"#));
    // The boss is no member.
    assert!(eval::<bool>(
        &s,
        r#"return UnitInParty("party1target") == nil"#
    ));
    // `MOB` targets party1, so the chain ends on a member.
    assert!(eval::<bool>(
        &s,
        r#"return UnitInParty("targettarget") == 1"#
    ));
}

/// `SpellCanTargetUnit` resolves its token to a guid and asks the verdicts by guid: a chain reads
/// the unit it ends on, and two tokens naming one unit answer alike.
#[test]
fn spell_can_target_unit_answers_the_verdict_of_the_unit_the_token_names() {
    let mut s = world();
    s.set_spell_targetable_units([BOSS, P1]);
    let can = |s: &UiScript, token: &str| {
        s.eval::<bool>(&format!("return SpellCanTargetUnit({token:?}) == true"))
            .unwrap()
    };
    let refused = |s: &UiScript, token: &str| {
        s.eval::<bool>(&format!("return SpellCanTargetUnit({token:?}) == nil"))
            .unwrap()
    };
    assert!(can(&s, "party1target"), "the boss");
    assert!(can(&s, "raid2target"), "the boss by another base");
    assert!(can(&s, "PARTY1TARGET"), "case folds");
    assert!(can(&s, "targettarget"), "party1, by a hop off the target");
    assert!(can(&s, "party1"), "a base, by its guid");
    assert!(can(&s, "raid2"), "another token, one unit");
    assert!(refused(&s, "raid3target"), "the add is not cleared");
    assert!(refused(&s, "pettarget"));
    assert!(refused(&s, "target"), "the mob is not cleared");
    assert!(refused(&s, "party1targettarget"), "a unit nobody holds");
    assert!(refused(&s, "party2target"), "a hop off a unit not held");
    assert!(refused(&s, "party1targetfoo"), "junk after a hop");
    assert!(refused(&s, ""), "no token names nobody");

    // The set moves with the push, and clearing it clears every token.
    s.set_spell_targetable_units([ADD]);
    assert!(can(&s, "raid3target"));
    assert!(refused(&s, "party1target"));
    s.set_spell_targetable_units([]);
    assert!(refused(&s, "raid3target"));
}

/// The reference gates the argument (`0x6e6d0e`) and resolves through `0x515970`, which raises for
/// a token it does not know.
#[test]
fn spell_can_target_unit_raises_as_the_resolver_and_the_gate_do() {
    let s = world();
    let err = |chunk: &str| s.eval::<mlua::Value>(chunk).expect_err(chunk).to_string();
    for chunk in [
        "return SpellCanTargetUnit()",
        "return SpellCanTargetUnit(nil)",
        "return SpellCanTargetUnit({})",
    ] {
        assert!(
            err(chunk).contains(r#"Usage: SpellCanTargetUnit("unit")"#),
            "{chunk}"
        );
    }
    for chunk in [
        r#"return SpellCanTargetUnit("bogus")"#,
        r#"return SpellCanTargetUnit("npctarget")"#,
        "return SpellCanTargetUnit(5)",
    ] {
        assert!(err(chunk).contains("Unknown unit name"), "{chunk}");
    }
}
