//! The six relation verbs, each two units in with the first as `this`: `UnitReaction 0x5167e0`,
//! `UnitIsEnemy 0x516890`, `UnitIsFriend 0x516930`, `UnitCanCooperate 0x516a70`,
//! `UnitCanAssist 0x516bb0` and `UnitCanAttack 0x516c50`. Each resolves both tokens to the units
//! they name and asks the host's relation functions ([`UnitRelations`]) of the pair, so the answer
//! is the engine's for those two units in that order, never a stand-in toward the player.

use std::rc::Rc;

use mlua::{Lua, Value};

use super::super::binding_abi::{flag, string_arg};
use super::super::Model;
use super::{check_unit_token, UnitGuids};

/// The engine's relation functions between two units, by guid, `a` as `this` and `b` its
/// argument: what the six verbs ask. A method answers `None` when the host does not hold either
/// unit.
pub trait UnitRelations {
    /// Whether the object manager holds `guid` as a unit (`0x468460`, typemask 8): what
    /// `GetUnitFromToken 0x515940` finds for a token.
    fn holds(&self, guid: u64) -> bool;
    /// `UnitReaction 0x6061e0(a, b)` on the internal scale, 0 hated to 7 exalted.
    fn reaction(&self, a: u64, b: u64) -> Option<u8>;
    /// `CanAttack 0x606980(a, b)`.
    fn can_attack(&self, a: u64, b: u64) -> Option<bool>;
    /// `CanAssist 0x6066f0(a, b)`.
    fn can_assist(&self, a: u64, b: u64) -> Option<bool>;
    /// `CanCooperate 0x606ba0(a, b)`.
    fn can_cooperate(&self, a: u64, b: u64) -> Option<bool>;
    /// `IsPvP 0x605ff0(unit)`: the charmer's or creator's PvP flag, none under bit 8, else the
    /// unit's own.
    fn is_pvp(&self, guid: u64) -> Option<bool>;
}

impl super::super::UiScript {
    /// Push the relation functions the six verbs ask. `None`, the state before the first push,
    /// holds no unit, so every verb answers as for two units not held.
    pub fn set_unit_relations(&mut self, relations: Option<Rc<dyn UnitRelations>>) {
        self.model_mut().unit_relations = relations;
    }

    /// Whether `guid` is the active player's (`0x468550`), as the resolver's inputs hold it.
    pub fn is_active_player(&self, guid: u64) -> bool {
        guid != 0 && guid == self.model_ref().unit_guids.player
    }

    /// The drunk watcher's test on the active player (`0x5e2afb`-`0x5e2b2a`): the target global
    /// `[0xb4e2d8]` names a unit the host holds (`0x468460`, typemask 8) and
    /// `CanAttack 0x606980(player, target)` holds.
    pub fn player_can_attack_target(&self) -> bool {
        let model = self.model_ref();
        let guids = &model.unit_guids;
        model.unit_relations.as_deref().is_some_and(|rel| {
            guids.target != 0
                && rel.holds(guids.target)
                && rel.can_attack(guids.player, guids.target) == Some(true)
        })
    }
}

/// `GetGUIDFromName 0x515970` on one token: the guid of the unit it names, 0 for nobody. An
/// unknown token raises `Unknown unit name` (`0x515c14`).
fn guid_of(model: &Model, token: &str) -> mlua::Result<u64> {
    check_unit_token(&Some(token.to_string()))?;
    Ok(model.unit(token).map_or(0, |u| u.guid))
}

/// The guid when the host holds it as a unit: `None` is the null object a verb answers nil for.
fn held(model: &Model, guid: u64) -> Option<u64> {
    let rel = model.unit_relations.as_deref()?;
    (guid != 0 && rel.holds(guid)).then_some(guid)
}

/// `0x491900`: a nonzero guid that is the player, his pet, a party member or a party member's pet
/// (`0x4e80a0`), or a raid member or a raid member's pet (`0x4bafd0`): the player or his pet, else
/// a group row ([`UnitGuids::roster_token`]). It reads the group tables by guid, so a member out of
/// view counts.
fn member_or_pet(guids: &UnitGuids, guid: u64) -> bool {
    guid != 0 && (guid == guids.player || guid == guids.pet || guids.roster_token(guid).is_some())
}

/// The two string arguments every relation verb takes, both `lua_isstring`-gated (`0x6f3510`), so a
/// nil or a table raises the verb's `Usage:` before either token is read.
fn two_tokens(
    lua: &Lua,
    a: Value,
    b: Value,
    usage: &'static str,
) -> mlua::Result<(String, String)> {
    Ok((string_arg(lua, a, usage)?, string_arg(lua, b, usage)?))
}

/// A verb over the two units `GetUnitFromToken 0x515940` finds, the first token resolved first:
/// `ask` of their guids, `None` (nil) when either names no unit the host holds.
fn ask_pair<T>(
    lua: &Lua,
    a: &str,
    b: &str,
    ask: impl FnOnce(&dyn UnitRelations, u64, u64) -> Option<T>,
) -> mlua::Result<Option<T>> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let (ga, gb) = (guid_of(&model, a)?, guid_of(&model, b)?);
    let (Some(ga), Some(gb)) = (held(&model, ga), held(&model, gb)) else {
        return Ok(None);
    };
    Ok(model
        .unit_relations
        .as_deref()
        .and_then(|rel| ask(rel, ga, gb)))
}

/// `UnitIsFriend` and `UnitCanCooperate`'s shape (`0x516961`-`0x516a1d`, `0x516aa1`-`0x516b5c`):
/// both guids resolved first (`0x515970`), the predicate over the two units when both are held,
/// else `"player"` (the whole token in any case, `0x847894`) on one side and the other's guid a
/// group member or pet ([`member_or_pet`]), held or not, either way round.
fn predicate_or_group(
    lua: &Lua,
    a: &str,
    b: &str,
    predicate: impl FnOnce(&dyn UnitRelations, u64, u64) -> Option<bool>,
) -> mlua::Result<bool> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let (ga, gb) = (guid_of(&model, a)?, guid_of(&model, b)?);
    let hit = held(&model, ga)
        .zip(held(&model, gb))
        .zip(model.unit_relations.as_deref())
        .and_then(|((ga, gb), rel)| predicate(rel, ga, gb))
        .unwrap_or(false);
    let player = |t: &str| t.eq_ignore_ascii_case("player");
    let guids = &model.unit_guids;
    Ok(hit || player(a) && member_or_pet(guids, gb) || player(b) && member_or_pet(guids, ga))
}

/// `UnitIsPVP(unit)` (`0x516460`): a unit the host holds answers `IsPvP` (`0x5164d1`), so a pet
/// or a charmed unit answers with its owner's flag, and none under bit 8; a guid with no object,
/// a party member out of view, the roster's PvP bit (`0x5164f9`), which its snapshot carries. An
/// unknown token raises (`0x515c14`).
pub(super) fn unit_is_pvp(lua: &Lua, token: &str) -> mlua::Result<Value> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let guid = guid_of(&model, token)?;
    let answer = match held(&model, guid) {
        Some(guid) => model
            .unit_relations
            .as_deref()
            .and_then(|rel| rel.is_pvp(guid))
            .unwrap_or(false),
        None => model.unit(token).is_some_and(|u| u.pvp),
    };
    Ok(flag(answer))
}

/// One of the six verbs as a Lua function.
fn verb(lua: &Lua, name: &str) -> mlua::Result<mlua::Function> {
    match name {
        // UnitReaction(unit, other) (`0x5167e0`): `0x6061e0(unit, other)` plus one (`0x51683e`),
        // so 1 hated to 8 exalted and 5 for a unit toward itself; nil (`0x51685f`) when a token
        // names no unit the host holds.
        "UnitReaction" => lua.create_function(|lua, (a, b): (Value, Value)| {
            let usage = r#"Usage: UnitReaction("unit", "otherUnit")"#;
            let (a, b) = two_tokens(lua, a, b, usage)?;
            let r = ask_pair(lua, &a, &b, |rel, a, b| rel.reaction(a, b))?;
            Ok(r.map_or(Value::Nil, |r| Value::Integer(i64::from(r) + 1)))
        }),
        // UnitIsEnemy(unit, other) (`0x516890`): `0x6061e0(unit, other)` hated or hostile, 1 or
        // below on the internal scale (`0x5168ea`).
        "UnitIsEnemy" => lua.create_function(|lua, (a, b): (Value, Value)| {
            let usage = r#"Usage: UnitIsEnemy("unit", "otherUnit")"#;
            let (a, b) = two_tokens(lua, a, b, usage)?;
            let r = ask_pair(lua, &a, &b, |rel, a, b| rel.reaction(a, b))?;
            Ok(flag(r.is_some_and(|r| r <= 1)))
        }),
        // UnitIsFriend(unit, other) (`0x516930`): `0x6061e0(unit, other)` friendly or better
        // (`0x5169d7`), else the player and a member of his group, either way round.
        "UnitIsFriend" => lua.create_function(|lua, (a, b): (Value, Value)| {
            let usage = r#"Usage: UnitIsFriend("unit", "otherUnit")"#;
            let (a, b) = two_tokens(lua, a, b, usage)?;
            let hit =
                predicate_or_group(lua, &a, &b, |rel, a, b| rel.reaction(a, b).map(|r| r >= 4))?;
            Ok(flag(hit))
        }),
        // UnitCanCooperate(unit, other) (`0x516a70`): `CanCooperate 0x606ba0(unit, other)`
        // (`0x516b12`), else the player and a member of his group, either way round.
        "UnitCanCooperate" => lua.create_function(|lua, (a, b): (Value, Value)| {
            let usage = r#"Usage: UnitCanCooperate("unit", "otherUnit")"#;
            let (a, b) = two_tokens(lua, a, b, usage)?;
            let hit = predicate_or_group(lua, &a, &b, |rel, a, b| rel.can_cooperate(a, b))?;
            Ok(flag(hit))
        }),
        // UnitCanAssist(unit, other) (`0x516bb0`): `CanAssist 0x6066f0(unit, other)` (`0x516c05`).
        "UnitCanAssist" => lua.create_function(|lua, (a, b): (Value, Value)| {
            let usage = r#"Usage: UnitCanAssist("unit", "otherUnit")"#;
            let (a, b) = two_tokens(lua, a, b, usage)?;
            let hit = ask_pair(lua, &a, &b, |rel, a, b| rel.can_assist(a, b))?;
            Ok(flag(hit == Some(true)))
        }),
        // UnitCanAttack(unit, other) (`0x516c50`): `CanAttack 0x606980(unit, other)` (`0x516ca5`).
        "UnitCanAttack" => lua.create_function(|lua, (a, b): (Value, Value)| {
            let usage = r#"Usage: UnitCanAttack("unit", "otherUnit")"#;
            let (a, b) = two_tokens(lua, a, b, usage)?;
            let hit = ask_pair(lua, &a, &b, |rel, a, b| rel.can_attack(a, b))?;
            Ok(flag(hit == Some(true)))
        }),
        _ => unreachable!("{name} is no relation verb"),
    }
}

/// Register these of the six verbs, in this order: the bindings set each where the global table
/// has always been built with it, whose layout `pairs(_G)` walks.
pub(super) fn register(lua: &Lua, names: &[&str]) -> mlua::Result<()> {
    let g = lua.globals();
    for &name in names {
        g.set(name, verb(lua, name)?)?;
    }
    Ok(())
}

/// Verdicts by guid pair, the host a test stands up.
#[cfg(test)]
pub(crate) mod fixture {
    use std::collections::{HashMap, HashSet};

    use super::UnitRelations;

    /// The held units and each ordered pair's verdicts; a held pair with no reaction entry is
    /// neutral, 3.
    #[derive(Default)]
    pub(crate) struct Verdicts {
        pub(crate) held: HashSet<u64>,
        pub(crate) reaction: HashMap<(u64, u64), u8>,
        pub(crate) attack: HashSet<(u64, u64)>,
        pub(crate) assist: HashSet<(u64, u64)>,
        pub(crate) cooperate: HashSet<(u64, u64)>,
        pub(crate) pvp: HashSet<u64>,
    }

    impl Verdicts {
        fn pair(&self, a: u64, b: u64) -> Option<(u64, u64)> {
            (self.held.contains(&a) && self.held.contains(&b)).then_some((a, b))
        }
    }

    impl UnitRelations for Verdicts {
        fn holds(&self, guid: u64) -> bool {
            self.held.contains(&guid)
        }
        fn reaction(&self, a: u64, b: u64) -> Option<u8> {
            let pair = self.pair(a, b)?;
            Some(self.reaction.get(&pair).copied().unwrap_or(3))
        }
        fn can_attack(&self, a: u64, b: u64) -> Option<bool> {
            Some(self.attack.contains(&self.pair(a, b)?))
        }
        fn can_assist(&self, a: u64, b: u64) -> Option<bool> {
            Some(self.assist.contains(&self.pair(a, b)?))
        }
        fn can_cooperate(&self, a: u64, b: u64) -> Option<bool> {
            Some(self.cooperate.contains(&self.pair(a, b)?))
        }
        fn is_pvp(&self, guid: u64) -> Option<bool> {
            self.holds(guid).then(|| self.pvp.contains(&guid))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::fixture::Verdicts;
    use crate::script::{UiScript, UnitGuids, UnitState};

    const ME: u64 = 0x10;
    const FOE: u64 = 0x20;
    const NPC: u64 = 0x30;
    const MEMBER: u64 = 0x40;

    fn unit(guid: u64) -> UnitState {
        UnitState {
            exists: true,
            guid,
            ..Default::default()
        }
    }

    /// The player, `target` and `npc` bound to their guids, the relation functions over `v`.
    fn script(v: Verdicts) -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit("player", Some(unit(ME)));
        s.set_unit("target", Some(unit(FOE)));
        s.set_unit("npc", Some(unit(NPC)));
        s.set_unit_relations(Some(Rc::new(v)));
        s
    }

    fn held(guids: &[u64]) -> Verdicts {
        Verdicts {
            held: guids.iter().copied().collect(),
            ..Default::default()
        }
    }

    fn answer(s: &UiScript, call: &str) -> Option<i64> {
        s.eval::<Option<i64>>(&format!("return {call}")).unwrap()
    }

    /// `UnitReaction(a, b)` is `0x6061e0(a, b)` plus one, the first unit as `this`: a reaction that
    /// differs by direction answers each direction's own.
    #[test]
    fn unit_reaction_is_the_first_units_toward_the_second_plus_one() {
        let mut v = held(&[ME, FOE]);
        v.reaction.insert((FOE, ME), 1);
        v.reaction.insert((ME, FOE), 4);
        let s = script(v);
        assert_eq!(answer(&s, r#"UnitReaction("target", "player")"#), Some(2));
        assert_eq!(answer(&s, r#"UnitReaction("player", "target")"#), Some(5));
        assert_eq!(answer(&s, r#"UnitReaction("PLAYER", "Target")"#), Some(5));
        // Exalted, 7 on the internal scale, is the eighth answer.
        let mut v = held(&[ME, FOE]);
        v.reaction.insert((ME, FOE), 7);
        assert_eq!(
            answer(&script(v), r#"UnitReaction("player", "target")"#),
            Some(8)
        );
    }

    /// `UnitIsEnemy` (`0x5168ea`) is the reaction at 1 or below, `UnitIsFriend` (`0x5169d7`) at 4
    /// or above, each in the order the tokens come.
    #[test]
    fn enemy_and_friend_threshold_the_ordered_reaction() {
        let mut v = held(&[ME, FOE]);
        v.reaction.insert((FOE, ME), 1);
        v.reaction.insert((ME, FOE), 4);
        let s = script(v);
        assert_eq!(answer(&s, r#"UnitIsEnemy("target", "player")"#), Some(1));
        assert_eq!(answer(&s, r#"UnitIsEnemy("player", "target")"#), None);
        assert_eq!(answer(&s, r#"UnitIsFriend("player", "target")"#), Some(1));
        assert_eq!(answer(&s, r#"UnitIsFriend("target", "player")"#), None);
        for (rank, enemy, friend) in [(0, true, false), (2, false, false), (3, false, false)] {
            let mut v = held(&[ME, FOE]);
            v.reaction.insert((FOE, ME), rank);
            let s = script(v);
            assert_eq!(
                answer(&s, r#"UnitIsEnemy("target", "player")"#).is_some(),
                enemy,
                "rank {rank}"
            );
            assert_eq!(
                answer(&s, r#"UnitIsFriend("target", "player")"#).is_some(),
                friend,
                "rank {rank}"
            );
        }
    }

    /// `UnitCanAttack(a, b)` and `UnitCanAssist(a, b)` ask `a` of `b`: a flagged enemy the player
    /// can attack while he cannot attack back answers per direction, and two units neither of
    /// which is the player are asked as any other pair.
    #[test]
    fn attack_and_assist_take_the_first_unit_as_the_actor() {
        let mut v = held(&[ME, FOE, NPC]);
        v.attack.insert((ME, FOE));
        v.assist.insert((NPC, ME));
        v.attack.insert((FOE, NPC));
        let s = script(v);
        assert_eq!(answer(&s, r#"UnitCanAttack("player", "target")"#), Some(1));
        assert_eq!(answer(&s, r#"UnitCanAttack("target", "player")"#), None);
        assert_eq!(answer(&s, r#"UnitCanAssist("npc", "player")"#), Some(1));
        assert_eq!(answer(&s, r#"UnitCanAssist("player", "npc")"#), None);
        assert_eq!(answer(&s, r#"UnitCanAttack("target", "npc")"#), Some(1));
        assert_eq!(answer(&s, r#"UnitCanAttack("npc", "target")"#), None);
    }

    /// Every verb answers nil when either token names a unit the host does not hold
    /// (`0x515940` null), or names nobody, and the number 1, never a boolean, on a hit.
    #[test]
    fn an_unheld_unit_answers_nil_and_a_hit_the_number_one() {
        let mut v = held(&[ME]);
        v.attack.insert((ME, FOE));
        v.reaction.insert((ME, FOE), 4);
        let mut s = script(v);
        s.set_unit("mouseover", None);
        for call in [
            r#"UnitReaction("player", "target")"#,
            r#"UnitIsEnemy("player", "target")"#,
            r#"UnitIsFriend("player", "target")"#,
            r#"UnitCanCooperate("player", "target")"#,
            r#"UnitCanAssist("player", "target")"#,
            r#"UnitCanAttack("player", "target")"#,
            r#"UnitCanAttack("player", "mouseover")"#,
        ] {
            assert_eq!(s.arity(call).unwrap(), 1, "{call}");
            assert!(
                s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
                "{call}"
            );
        }
        let mut v = held(&[ME, FOE]);
        v.attack.insert((ME, FOE));
        let s = script(v);
        let call = r#"UnitCanAttack("player", "target")"#;
        assert_eq!(
            s.eval::<String>(&format!("return type({call})")).unwrap(),
            "number"
        );
        // No relation functions pushed: nothing is held.
        let mut s = UiScript::new().unwrap();
        s.set_unit("player", Some(unit(ME)));
        assert!(s
            .eval::<bool>(r#"return UnitReaction("player", "player") == nil"#)
            .unwrap());
    }

    /// Both arguments are `lua_isstring`-gated, so a missing one raises the verb's `Usage:`; both
    /// tokens are resolved, the first first, so an unknown one raises `Unknown unit name`.
    #[test]
    fn both_tokens_are_gated_and_resolved() {
        let s = script(held(&[ME, FOE]));
        let usage = r#"Usage: UnitCanAssist("unit", "otherUnit")"#;
        for call in [
            "UnitCanAssist()",
            r#"UnitCanAssist("player")"#,
            r#"UnitCanAssist(nil, "target")"#,
            r#"UnitCanAssist("player", {})"#,
            r#"UnitCanAssist(true, "target")"#,
        ] {
            let err = s.run(call).expect_err(call).to_string();
            assert!(err.contains(usage), "{call}: {err}");
        }
        // A number is a string to `lua_isstring`; the resolver then refuses it.
        let err = s
            .run(r#"UnitCanAssist(5, "target")"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unknown unit name: 5"), "{err}");
        let err = s
            .run(r#"UnitIsFriend("bogus", "nope")"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unknown unit name: bogus"), "{err}");
        let err = s
            .run(r#"UnitReaction("player", "nope")"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unknown unit name: nope"), "{err}");
    }

    /// `UnitIsFriend` and `UnitCanCooperate` fall back to the group (`0x491900`) when the predicate
    /// fails or a unit is not held: the whole token `"player"`, in any case, against a party member
    /// or a member's pet out of view, either way round. Two other units take no fallback, nor does
    /// `"playertarget"`, a token that only begins with it.
    #[test]
    fn friend_and_cooperate_count_the_players_group_out_of_view() {
        let mut s = script(held(&[ME, FOE]));
        s.set_unit("party1", Some(unit(MEMBER)));
        s.set_unit_guids(&UnitGuids {
            player: ME,
            target: FOE,
            party: [MEMBER, 0, 0, 0],
            party_pets: [MEMBER + 1, 0, 0, 0],
            ..Default::default()
        });
        s.set_unit("partypet1", Some(unit(MEMBER + 1)));
        for verb in ["UnitIsFriend", "UnitCanCooperate"] {
            for (a, b, want) in [
                ("player", "party1", Some(1)),
                ("party1", "PLAYER", Some(1)),
                ("player", "partypet1", Some(1)),
                ("target", "party1", None),
                ("playertarget", "party1", None),
                ("party1", "playertarget", None),
                ("player", "target", None),
            ] {
                let call = format!(r#"{verb}("{a}", "{b}")"#);
                assert_eq!(answer(&s, &call), want, "{call}");
            }
        }
        // The predicate itself, both held.
        let mut v = held(&[ME, FOE]);
        v.cooperate.insert((FOE, ME));
        let s = script(v);
        assert_eq!(
            answer(&s, r#"UnitCanCooperate("target", "player")"#),
            Some(1)
        );
        assert_eq!(answer(&s, r#"UnitCanCooperate("player", "target")"#), None);
    }

    /// `UnitIsPVP` asks `IsPvP` of a unit the host holds (`0x5164d1`), whatever its snapshot's own
    /// flag, and reads the snapshot only for one it does not hold, a party member out of view
    /// (`0x5164f9`).
    #[test]
    fn unit_is_pvp_asks_is_pvp_of_a_held_unit() {
        let flagged = |guid| UnitState {
            pvp: true,
            ..unit(guid)
        };
        let mut v = held(&[ME, FOE, NPC]);
        v.pvp.insert(NPC);
        let mut s = script(v);
        s.set_unit("target", Some(flagged(FOE)));
        s.set_unit("party1", Some(flagged(MEMBER)));
        assert_eq!(answer(&s, r#"UnitIsPVP("target")"#), None, "held, IsPvP no");
        assert_eq!(
            answer(&s, r#"UnitIsPVP("npc")"#),
            Some(1),
            "held, IsPvP yes"
        );
        assert_eq!(answer(&s, r#"UnitIsPVP("party1")"#), Some(1), "out of view");
        let err = s.run(r#"UnitIsPVP("nope")"#).unwrap_err().to_string();
        assert!(err.contains("Unknown unit name: nope"), "{err}");
    }
}
