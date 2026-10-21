//! The `Unit*` global registrations, over the per-token [`UnitState`](super::UnitState) snapshots.
//! Every predicate answers the number 1 or nil through [`super::unit_predicate`] or
//! [`flag`](super::super::binding_abi::flag): no binding in the reference's unit table (`0x850438`)
//! calls `lua_pushboolean` (`0x6f39f0`), though `IsPetAttackActive` (`0x4be0e0`) outside it does.

use mlua::{Lua, Value};

use super::super::binding_abi::flag;
use super::super::calls::{submit, ScriptCall};
use super::super::protected::select_may_cast;
use super::super::Model;
use super::{
    check_unit_token, classification_word, grey_band, level_is_hidden, unit_is_grey,
    unit_predicate, unknownobject, with_unit, PlayerRecord, SelectionRequest,
};

/// The `"player"` fast path of `UnitName`, `UnitRace`, `UnitClass` and `UnitSex`, never
/// `UnitLevel`: the record's answer when the token equals `"player"` (`0x847894`) whole and
/// case-insensitively; any other token, `"playerfoo"` included, goes to the resolver.
fn player_record_arm<T>(
    lua: &Lua,
    token: &Option<String>,
    f: impl FnOnce(&PlayerRecord) -> T,
) -> Option<T> {
    if !token
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("player"))
    {
        return None;
    }
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    Some(f(&model.player_record))
}

/// `SStrCmpI(a, b, 0x7fffffff) == 0` (`0x64a4c0` through `__strnicmp`, `0x414310`): `A` to `Z`
/// fold, every other byte exact, and a NUL in either string ends the compare.
fn c_str_eq_ignore_case(a: &str, b: &str) -> bool {
    fn c_str(s: &str) -> &[u8] {
        let bytes = s.as_bytes();
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        &bytes[..end]
    }
    c_str(a).eq_ignore_ascii_case(c_str(b))
}

/// The class ids `GetComboPoints` (`0x51a190`) accepts: it compares the class, `UNIT_FIELD_BYTES_0`
/// byte 1, with 4 and 0xb.
const CLASS_ROGUE: u32 = 4;
const CLASS_DRUID: u32 = 11;

/// Register the `Unit*` globals.
pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "UnitExists",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.exists)
        })?,
    )?;

    // `UnitIsVisible(unit)` (`0x516030`): whether the client holds the unit's object, nothing else.
    // Not `UnitExists`: an out-of-range party member exists through its roster entry, unseen.
    g.set(
        "UnitIsVisible",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.has_object)
        })?,
    )?;

    // `UnitIsTapped`/`UnitIsTappedByPlayer` (`0x519c90`/`0x519d00`): the object is present and its
    // `UNIT_DYNAMIC_FLAGS` has the mask bit, nothing more. Unlike `UnitIsVisible` they gate with
    // `lua_isstring`: another type raises `Usage:`, and a number raises `Unknown unit name`.
    for (name, usage, by_player) in [
        ("UnitIsTapped", r#"Usage: UnitIsTapped("unit")"#, false),
        (
            "UnitIsTappedByPlayer",
            r#"Usage: UnitIsTappedByPlayer("unit")"#,
            true,
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, token: Value| {
                let token = Some(crate::script::binding_abi::string_arg(lua, token, usage)?);
                let hit = with_unit(lua, &token, false, |u| {
                    if by_player {
                        u.tapped_by_player
                    } else {
                        u.tapped
                    }
                })?;
                Ok(flag(hit))
            })?,
        )?;
    }

    // `UnitIsPartyLeader(unit)` (`0x516210`): a held player's `PLAYER_FLAGS` bit 0x1, for a
    // stranger leading their own party, or a GUID equal to the group leader's, for a member not
    // held. No zero guard (unlike `IsPartyLeader`, `0x4e9130`): GUID 0 matches the zeroed leader,
    // so `UnitIsPartyLeader(nil)` answers 1 while ungrouped. No `Usage:` gate; a bad token or a
    // number raises `Unknown unit name`.
    g.set(
        "UnitIsPartyLeader",
        lua.create_function(|lua, token: Option<String>| {
            check_unit_token(&token)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let by_flag = token
                .as_ref()
                .and_then(|t| model.unit(t))
                .is_some_and(|u| u.group_leader);
            // A token with no snapshot is GUID 0, the reference's `0:0`.
            let guid = token
                .as_ref()
                .and_then(|t| model.unit(t))
                .map_or(0, |u| u.guid);
            Ok(flag(by_flag || guid == model.party.leader_guid))
        })?,
    )?;

    g.set(
        "UnitName",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitName("unit")"#,
            )?);
            // Two values (`0x5170ae`, `0x517289`): the name, and the realm, nil for a same-realm
            // player or a non-player (`0x609210`), so always nil in single-realm benilla. A second
            // argument (stock `UnitPopup.lua:106` passes `true`) is not read. The name is nil only
            // for an empty `"player"` buffer (`0x51708c`, `0x6f3895`) or GUID 0 (`0x5170c0`);
            // otherwise a string, `UNKNOWNOBJECT` until the name cache answers (`0x517220`,
            // `0x609324`), as for a new pet. A seated snapshot answers even where `exists` is
            // false; the `"player"` arm reads the local name buffer (`0xc27d88`), never the unit.
            if let Some(seeded) = player_record_arm(lua, &token, |r| r.name.clone()) {
                let name = if seeded.is_empty() {
                    Value::Nil
                } else {
                    Value::String(lua.create_string(&seeded)?)
                };
                return Ok((name, Value::Nil));
            }
            let name = with_unit(lua, &token, None, |u| {
                Some((u.name.clone(), u.has_object, u.guid))
            })?;
            let name = match name {
                None => Value::Nil,
                Some((Some(n), _, _)) => Value::String(lua.create_string(&n)?),
                Some((None, held, guid)) => {
                    // A unit with no object reads the name cache by guid with a callback
                    // (`0x5171eb`), which asks on the miss: the host sends it.
                    if !held && guid != 0 {
                        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                        model.unit_name_asks.insert(guid);
                    }
                    Value::String(unknownobject(lua)?)
                }
            };
            Ok((name, Value::Nil))
        })?,
    )?;

    g.set(
        "UnitHealth",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitHealth("unit")"#,
            )?);
            with_unit(lua, &token, 0i64, |u| i64::from(u.health))
        })?,
    )?;

    g.set(
        "UnitHealthMax",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitHealthMax("unit")"#,
            )?);
            with_unit(lua, &token, 0i64, |u| i64::from(u.max_health))
        })?,
    )?;

    // UnitLevel (`0x517fc0`): -1 for a world boss or a unit hostile to the player 10 or more levels
    // above it, tested first, so a world boss at level 0 reads -1 ([`level_is_hidden`]); then a
    // level of 0 or less as it is (`0x518092`); else, for a unit the drunk player can attack
    // (`0x518096`-`0x5180b1`), the level its drunkenness lowers (`0x5180b3`-`0x5180ec`).
    g.set(
        "UnitLevel",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitLevel("unit")"#,
            )?);
            check_unit_token(&token)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(u) = token.as_ref().and_then(|t| model.unit(t)) else {
                return Ok(0i64);
            };
            if level_is_hidden(u, model.player_req.level) {
                return Ok(-1);
            }
            if u.level == 0 {
                return Ok(0);
            }
            // The drunk leg needs the unit's object and the active player (`0x518096`).
            let me = model.unit_guids.player;
            let drunk = model.unit("player").map_or(0, |p| p.drunk);
            let attackable = || {
                model
                    .unit_relations
                    .as_deref()
                    .and_then(|r| r.can_attack(me, u.guid))
                    .unwrap_or(false)
            };
            Ok(i64::from(
                if u.has_object && me != 0 && drunk != 0 && attackable() {
                    super::unit_level_drunk_level(u.level, drunk)
                } else {
                    u.level
                },
            ))
        })?,
    )?;

    // UnitIsCorpse (`0x5161c0`): the token names a corpse object, a released player's remains; a
    // dead unit is not one. No feed sets `corpse_object`, so this answers nil.
    g.set(
        "UnitIsCorpse",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.corpse_object)
        })?,
    )?;

    super::relations::register(lua, &["UnitCanAttack", "UnitCanAssist"])?;

    // UnitIsTrivial (`0x519d70`): `0x5f0700` on the token's object (`0x515940`; nil with none,
    // `0x519da8`), false for a player (object type bit `0x10`), else true when the player's level
    // is above the unit's by more than the grey band ([`unit_is_grey`]). The unit's raw level is
    // read, as `UnitLevel`'s raw 0 is.
    g.set(
        "UnitIsTrivial",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsTrivial("unit")"#,
            )?);
            let player_level = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .player_req
                .level;
            unit_predicate(lua, &token, |u| {
                u.has_object && !u.is_player && unit_is_grey(player_level, u.level)
            })
        })?,
    )?;

    // GetQuestGreenRange (`0x4e17d0`): the green-to-grey boundary `QuestLogFrame.lua:593` buckets
    // by, from the table at `0x8076c0`. The reference answers 0 with no player object (`0x4e17f8`);
    // here a level of 0 reads the table's first entry, 4.
    g.set(
        "GetQuestGreenRange",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(grey_band(model.player_req.level)))
        })?,
    )?;

    g.set(
        "UnitIsDead",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsDead("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.dead)
        })?,
    )?;

    // A released ghost has health 1, so `UnitIsDead` is false for it.
    g.set(
        "UnitIsGhost",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsGhost("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.ghost)
        })?,
    )?;
    g.set(
        "UnitIsDeadOrGhost",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsDeadOrGhost("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.dead || u.ghost)
        })?,
    )?;

    super::relations::register(lua, &["UnitReaction", "UnitIsEnemy", "UnitIsFriend"])?;

    // UnitIsCivilian(unit): killing the unit would be dishonorable, the predicate `0x612550`. It
    // shares `is_civilian_kill` with the tooltip's civilian line and `UnitPVPName`, so all agree.
    g.set(
        "UnitIsCivilian",
        lua.create_function(|lua, unit: Value| {
            let unit = Some(crate::script::binding_abi::string_arg(
                lua,
                unit,
                r#"Usage: UnitIsCivilian("unit")"#,
            )?);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let player_level = model.player_req.level;
            Ok(flag(
                unit.and_then(|u| model.unit(&u))
                    .is_some_and(|u| super::is_civilian_kill(u, player_level)),
            ))
        })?,
    )?;
    // UnitPlayerControlled(unit) (`0x516410`): `UNIT_FIELD_FLAGS` bit 0x8, vmangos
    // `UNIT_FLAG_PLAYER_CONTROLLED`, so a player's pet or a charmed creature answers 1 where
    // `UnitIsPlayer` answers nil. An unrecognised token answers nil here; the reference raises.
    g.set(
        "UnitPlayerControlled",
        lua.create_function(|lua, unit: Option<String>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(
                unit.and_then(|u| model.unit(&u))
                    .is_some_and(|u| u.player_controlled),
            ))
        })?,
    )?;
    g.set(
        "UnitIsPlayer",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.is_player)
        })?,
    )?;
    // UnitIsPlusMob(unit) (`0x516d40`): `UNIT_FIELD_FLAGS` bit 0x40, never the creature rank
    // (`0x605620`). The server sets it for a non-pet above normal rank (`Creature.cpp:637`), so a
    // rare answers 1, and an uncached creature still answers from its flags.
    g.set(
        "UnitIsPlusMob",
        lua.create_function(|lua, token: Option<String>| {
            // No `lua_isstring` gate, so nil answers nil; an unrecognised token still raises in
            // the resolver (`0x515940`).
            unit_predicate(lua, &token, |u| u.flags & 0x40 != 0)
        })?,
    )?;

    // UnitIsUnit(a, b) (`0x516070`): two strings equal without regard to case answer 1 before
    // either is resolved (`SStrCmpI`, `0x5160c4`; `0x5160cd`); otherwise both resolve
    // (`0x5160e8`, `0x5160f4`) and a shared nonzero GUID answers 1 (`0x516100`), held or not.
    g.set(
        "UnitIsUnit",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // Both arguments are `lua_isstring`-gated, so either one nil raises.
            let a = crate::script::binding_abi::string_arg(
                lua,
                a,
                r#"Usage: UnitIsUnit("unit", "otherUnit")"#,
            )?;
            let b = crate::script::binding_abi::string_arg(
                lua,
                b,
                r#"Usage: UnitIsUnit("unit", "otherUnit")"#,
            )?;
            if c_str_eq_ignore_case(&a, &b) {
                return Ok(flag(true));
            }
            let (a, b) = (Some(a), Some(b));
            // Both go through the resolver, so either one unrecognised raises.
            check_unit_token(&a)?;
            check_unit_token(&b)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let guid = |t: &Option<String>| {
                t.as_deref()
                    .and_then(|t| model.unit(t))
                    .map_or(0, |u| u.guid)
            };
            let (ga, gb) = (guid(&a), guid(&b));
            Ok(flag(ga != 0 && ga == gb))
        })?,
    )?;

    // UnitAffectingCombat(unit) (`0x517e10`): 1 in combat, else nil. No such unit takes the same
    // arm as a unit out of combat (`0x517e48`, `0x517e5c`), so it cannot probe existence. A missing
    // argument, or one neither string nor number, raises `Usage:` (`0x6f3510`, `0x6f4940`), unlike
    // `UnitInRaid`; a number reaches the resolver, which raises `Unknown unit name` (`0x515c14`).
    g.set(
        "UnitAffectingCombat",
        lua.create_function(|lua, token: Value| {
            let token = super::super::binding_abi::string_arg(
                lua,
                token,
                "Usage: UnitAffectingCombat(\"unit\")",
            )?;
            let hot = with_unit(lua, &Some(token), false, |u| u.exists && u.in_combat)?;
            Ok(flag(hot))
        })?,
    )?;

    // HasFullControl(): the flag `SMSG_CLIENT_CONTROL_UPDATE` writes for the local player
    // (`0xb4b3e4`, read at `0x51a158`), which the stock unit menu greys Trade and Duel on
    // (`UnitPopup.lua:474-477`, `:502-505`).
    g.set(
        "HasFullControl",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.player_control))
        })?,
    )?;

    // UnitPlayerOrPetInParty/InRaid(unit): the unit or its owner (`UnitState::owner`) is in the
    // group. The reference's predicate behind `0x5162f0`/`0x5163b0` is untraced; the owner reading
    // is benilla's.
    for (name, raid) in [
        ("UnitPlayerOrPetInParty", false),
        ("UnitPlayerOrPetInRaid", true),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, token: Option<String>| {
                check_unit_token(&token)?;
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let Some(u) = token.as_ref().and_then(|t| model.unit(t)) else {
                    return Ok(Value::Nil);
                };
                if !u.exists || u.guid == 0 {
                    return Ok(Value::Nil);
                }
                let me = model.unit("player").map(|p| p.guid).unwrap_or(0);
                let in_group = |guid: u64| {
                    guid != 0
                        && if raid {
                            model.party.raid.iter().any(|m| m.guid == guid)
                        } else {
                            guid == me || model.party.members.iter().any(|m| m.guid == guid)
                        }
                };
                let grouped = if raid {
                    !model.party.raid.is_empty()
                } else {
                    !model.party.members.is_empty()
                };
                let hit = grouped && (in_group(u.guid) || in_group(u.owner));
                Ok(flag(hit))
            })?,
        )?;
    }

    // UnitInRaid(unit) (`0x516350`): the constant 1 on a hit (`0x51637e`), never an index. The
    // reference never raises `Usage:`: a missing or uncoercible argument is GUID 0 (`0x6f3690`,
    // `0x515970`), a miss (`0x4baee0`). The roster holds the player, unlike `UnitInParty`'s.
    g.set(
        "UnitInRaid",
        lua.create_function(|lua, token: Option<String>| {
            check_unit_token(&token)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let hit = token
                .as_ref()
                .and_then(|t| model.unit(t))
                .filter(|u| u.exists && u.guid != 0)
                .is_some_and(|u| {
                    model
                        .party
                        .raid
                        .iter()
                        .any(|m| m.guid != 0 && m.guid == u.guid)
                });
            Ok(flag(hit))
        })?,
    )?;

    // UnitInParty(unit) (`0x516290`): the token's guid (`0x515970`) is the player's or one of the
    // four party slots' (`0x4e7f70`, `0x468550` and `0xbc6f48`), the tables the resolver itself
    // reads. No object and no group are asked, so solo `UnitInParty("player")` is 1, and a token
    // naming an out-of-view member is 1 however it spells them; nobody is guid 0, a miss. No
    // `Usage:` gate; an unrecognised token raises in the resolver.
    g.set(
        "UnitInParty",
        lua.create_function(|lua, token: Option<String>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let guids = &model.unit_guids;
            let guid = match token.as_deref() {
                Some(t) => guids.guid_of(t)?.unwrap_or(0),
                None => 0,
            };
            Ok(flag(
                guid != 0 && (guid == guids.player || guids.party.contains(&guid)),
            ))
        })?,
    )?;

    super::relations::register(lua, &["UnitCanCooperate"])?;

    // GetRaidTargetIndex(unit): the mark, 1 to 8, or nil.
    g.set(
        "GetRaidTargetIndex",
        lua.create_function(|lua, token: Value| {
            // Gated (`0x4bb4be`, `0x4bb4cd`); the unquoted `unit` is the reference's spelling.
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                "Usage: GetRaidTargetIndex(unit)",
            )?);
            let idx = with_unit(lua, &token, 0u8, |u| u.raid_target)?;
            Ok(if idx > 0 {
                Value::Integer(i64::from(idx))
            } else {
                Value::Nil
            })
        })?,
    )?;

    // The party frame's status predicates; 1.12 has no unit AFK or DND predicate.
    g.set(
        "UnitIsConnected",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsConnected("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.is_connected)
        })?,
    )?;
    g.set(
        "UnitIsPVP",
        lua.create_function(|lua, token: Value| {
            let token =
                crate::script::binding_abi::string_arg(lua, token, r#"Usage: UnitIsPVP("unit")"#)?;
            super::relations::unit_is_pvp(lua, &token)
        })?,
    )?;
    g.set(
        "UnitIsPVPFreeForAll",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsPVPFreeForAll("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.is_pvp_ffa)
        })?,
    )?;
    // UnitFactionGroup(unit) → FactionGroup.dbc's `InternalName` (field 2), which stock FrameXML
    // builds texture paths from (`PlayerFrame.lua:68`), and its localized `Name0` (field 3); nil,
    // nil for a unit with no side or no snapshot.
    g.set(
        "UnitFactionGroup",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitFactionGroup("unit")"#,
            )?);
            let pair = with_unit(lua, &token, (None, None), |u| {
                (u.faction_group.clone(), u.faction_group_localized.clone())
            })?;
            match pair {
                (Some(english), localized) => {
                    let a = Value::String(lua.create_string(&english)?);
                    let b = match localized {
                        Some(l) if !l.is_empty() => Value::String(lua.create_string(&l)?),
                        _ => a.clone(),
                    };
                    Ok((a, b))
                }
                (None, _) => Ok((Value::Nil, Value::Nil)),
            }
        })?,
    )?;

    // UnitRace(unit) → localized, raceFile; UnitClass(unit) → localized, classFileName; nil, nil
    // for a unit without one.
    g.set(
        "UnitRace",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitRace("unit")"#,
            )?);
            // The `"player"` arm reads the character-select record's race byte (`0x518269`,
            // `0xc27e80`), never the unit resolver.
            if let Some(pair) = player_record_arm(lua, &token, |r| r.race.clone()) {
                return match pair {
                    Some((loc, file)) => Ok((
                        Value::String(lua.create_string(&loc)?),
                        Value::String(lua.create_string(&file)?),
                    )),
                    None => Ok((Value::Nil, Value::Nil)),
                };
            }
            let pair = with_unit(lua, &token, None, |u| {
                u.race.clone().zip(u.race_file.clone())
            })?;
            match pair {
                Some((loc, file)) => Ok((
                    Value::String(lua.create_string(&loc)?),
                    Value::String(lua.create_string(&file)?),
                )),
                None => Ok((Value::Nil, Value::Nil)),
            }
        })?,
    )?;
    g.set(
        "UnitClass",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitClass("unit")"#,
            )?);
            // The `"player"` arm reads the record's class byte (`0x5183b9`, `0xc27e81`); the second
            // return is the uppercase token.
            if let Some(pair) = player_record_arm(lua, &token, |r| r.class.clone()) {
                return match pair {
                    Some((loc, file)) => Ok((
                        Value::String(lua.create_string(&loc)?),
                        Value::String(lua.create_string(&file)?),
                    )),
                    None => Ok((Value::Nil, Value::Nil)),
                };
            }
            let pair = with_unit(lua, &token, None, |u| {
                u.class.clone().zip(u.class_file.clone())
            })?;
            match pair {
                Some((loc, file)) => Ok((
                    Value::String(lua.create_string(&loc)?),
                    Value::String(lua.create_string(&file)?),
                )),
                None => Ok((Value::Nil, Value::Nil)),
            }
        })?,
    )?;
    // UnitHasRelicSlot(unit) (`0x519e50`): a player unit's class has a relic slot, `ChrClasses.dbc`
    // field 16 (`ds:0xc0def4`), which the app resolves; no class id is compared in code. Field 16
    // is in the `patch.MPQ` copy only, set for Paladin, Shaman and Druid. No `"player"` fast path,
    // so any token answers, as stock `InspectPaperDollFrame.lua:123` needs.
    g.set(
        "UnitHasRelicSlot",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitHasRelicSlot("unit")"#,
            )?);
            let has = with_unit(lua, &token, false, |u| u.has_relic_slot)?;
            Ok(flag(has))
        })?,
    )?;

    // GetDamageBonusStat() (`0x48b520`): the active player's `ChrClasses.dbc` field 2 plus one,
    // the 1-based `UnitStat` index its melee damage scales with; 0 with no player or no class row
    // (`0x48b58a`). No stock caller.
    g.set(
        "GetDamageBonusStat",
        lua.create_function(|lua, ()| {
            let stat = with_unit(lua, &Some("player".to_string()), None, |u| {
                u.damage_bonus_stat
            })?;
            Ok(stat.map_or(0, |s| i64::from(s) + 1))
        })?,
    )?;

    // UnitSex(unit): 2 male, 3 female, or 1 neuter, which no feed sends. A unit that does not
    // resolve answers 2, never nil (`0x517f9f`).
    g.set(
        "UnitSex",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitSex("unit")"#,
            )?);
            // The `"player"` arm reads the record's byte (`0x517ef9`, `0xc27e82`) into
            // `{2, 3, 1, 6}` unchecked (`0x808be4`), so an unset record answers 2 there too.
            let sex = match player_record_arm(lua, &token, |r| r.sex) {
                Some(sex) => sex,
                None => with_unit(lua, &token, 0u8, |u| u.sex)?,
            };
            Ok(Value::Integer(i64::from(if sex == 0 { 2 } else { sex })))
        })?,
    )?;

    // UnitCreatureType(unit) (`0x51a280`): one string, nil for a token naming no unit (`0x51a2b8`)
    // and for a type of 0 (`0x51a2c3`). The type is the snapshot's, which the app resolved as the
    // reference's `0x605570` does: the shapeshift form's (`SpellShapeshiftForm.dbc` column 12, only
    // above 0), else the cached creature record's, else the race's (`ChrRaces.dbc` column 9). A
    // snapshot with no descriptor behind it carries none, as the reference's token resolver finds
    // no object for it. A missing argument, or one neither string nor number, raises `Usage:`
    // (`0x6f3510`, `0x6f4940`).
    g.set(
        "UnitCreatureType",
        lua.create_function(|lua, token: Value| {
            let token = match &token {
                Value::String(s) => Some(s.to_str()?.to_string()),
                // A number passes `lua_isstring`; here it answers nil, where the reference's
                // resolver raises `Unknown unit name` for it.
                Value::Number(_) | Value::Integer(_) => Some(String::new()),
                _ => return Err(mlua::Error::runtime("Usage: UnitCreatureType(\"unit\")")),
            };
            let word = with_unit(lua, &token, None, |u| u.creature_type_name.clone())?;
            match word {
                Some(w) => Ok(Value::String(lua.create_string(&w)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // UnitClassification(unit) (`0x516d90`): the rank's word, and "normal" for a token that
    // resolves to no unit, never nil.
    g.set(
        "UnitClassification",
        lua.create_function(|lua, token: Option<String>| {
            let rank = with_unit(lua, &token, 0u32, |u| u.rank)?;
            Ok(classification_word(rank).to_string())
        })?,
    )?;

    // UnitPowerType(unit) (`0x517940`): one number, the power index (0 mana, 1 rage, ...), with no
    // token string. A unit that does not resolve answers 0, never nil: `UnitFrame.lua:122` indexes
    // `ManaBarColor` with it.
    g.set(
        "UnitPowerType",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitPowerType("unit")"#,
            )?);
            let ty = with_unit(lua, &token, 0u8, |u| u.power_type)?;
            Ok(i64::from(ty))
        })?,
    )?;

    // UnitMana/UnitManaMax(unit): the current and maximum of whatever power the unit uses; 1.12
    // has no `UnitPower` and no power-type argument.
    g.set(
        "UnitMana",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitMana("unit")"#,
            )?);
            with_unit(lua, &token, 0i64, |u| i64::from(u.power))
        })?,
    )?;
    g.set(
        "UnitManaMax",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitManaMax("unit")"#,
            )?);
            with_unit(lua, &token, 0i64, |u| i64::from(u.max_power))
        })?,
    )?;

    // UnitXP/UnitXPMax(unit): the token resolved (`0x51738e`), then the active player's private
    // `PLAYER_XP` pair for its object by whatever token names it (`0x5173ad`-`0x5173c9`), 0 for
    // any other unit or none.
    fn names_self(model: &Model, token: &str) -> bool {
        token.eq_ignore_ascii_case("player")
            || (model.unit_guids.player != 0
                && model.unit_guids.resolve(token).ok().flatten() == Some(model.unit_guids.player))
    }
    g.set(
        "UnitXP",
        lua.create_function(|lua, token: Value| {
            let token =
                crate::script::binding_abi::string_arg(lua, token, r#"Usage: UnitXP("unit")"#)?;
            check_unit_token(&Some(token.clone()))?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if names_self(&model, &token) {
                i64::from(model.player_xp)
            } else {
                0
            })
        })?,
    )?;
    g.set(
        "UnitXPMax",
        lua.create_function(|lua, token: Value| {
            let token =
                crate::script::binding_abi::string_arg(lua, token, r#"Usage: UnitXPMax("unit")"#)?;
            check_unit_token(&Some(token.clone()))?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if names_self(&model, &token) {
                i64::from(model.player_next_level_xp)
            } else {
                0
            })
        })?,
    )?;

    // UnitIsCharmed(unit) (`0x516cf0`): `UNIT_FIELD_CHARMEDBY` is nonzero, not a flags bit, so the
    // charmed unit answers 1 and its charmer nil. 1.12 has no `UnitIsPossessed`.
    g.set(
        "UnitIsCharmed",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.charmed)
        })?,
    )?;

    g.set(
        "GetMoney",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.money as i64)
        })?,
    )?;

    // GetComboPoints() (`0x51a190`): `PLAYER_FIELD_BYTES` byte 1, behind two gates stock
    // `ComboFrame.lua` does not repeat: the class is rogue or druid (`0x51a1cc`), so a warrior's
    // dodge point never shows, and `PLAYER_FIELD_COMBO_TARGET` equals the current target
    // (`0x51a205`), so the points read per target. Both are plain equality, GUID 0 included.
    g.set(
        "GetComboPoints",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let class = model.player_req.class_id;
            if class != CLASS_ROGUE && class != CLASS_DRUID {
                return Ok(0_i64);
            }
            let target = model.unit("target").map_or(0, |u| u.guid);
            if model.combo_target != target {
                return Ok(0_i64);
            }
            Ok(i64::from(model.combo_points))
        })?,
    )?;

    // GetRestState() (`0x48d350`) → the Exhaustion.dbc row's ID, name and factor, indexed directly
    // by `PLAYER_BYTES_2` byte 3 (`0xc0dd78`); nil, nil, nil when there is no such row.
    g.set(
        "GetRestState",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match model.exhaustion.get(&model.rest_state) {
                Some((name, factor)) => (
                    Value::Number(f64::from(model.rest_state)),
                    Value::String(lua.create_string(name)?),
                    Value::Number(*factor),
                ),
                None => (Value::Nil, Value::Nil, Value::Nil),
            })
        })?,
    )?;
    // GetXPExhaustion() (`0x48d3f0`): the rested pool times the factor of Exhaustion.dbc row 1
    // whatever the state (2.0 shipped). Nil unless the rest byte is 1 (`0x48d43b`), whatever the
    // pool holds, so a rested byte with an empty pool answers 0.
    g.set(
        "GetXPExhaustion",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if model.rest_state != 1 {
                Value::Nil
            } else {
                let factor = model.exhaustion.get(&1).map_or(2.0, |(_, f)| *f);
                Value::Number(f64::from(model.rest_pool) * factor)
            })
        })?,
    )?;
    // IsResting() (`0x516ea0`): `PLAYER_FLAGS` bit 0x20, in a rest area.
    g.set(
        "IsResting",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.resting))
        })?,
    )?;
    // PartialPlayTime()/NoPlayTime(): the play-time limit flags, `PLAYER_FLAGS` bits 0x1000
    // (`0x48eb70`) and 0x2000 (`0x48ebe0`).
    g.set(
        "PartialPlayTime",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.partial_play_time))
        })?,
    )?;
    g.set(
        "NoPlayTime",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.no_play_time))
        })?,
    )?;
    // GetBillingTimeRested() (`0x48ec50`): always one number, unconverted (`0x48ec5e`); stock
    // `PlayerFrame.lua:246` reads it as minutes. vmangos always sends 0 (`World.cpp:331`).
    g.set(
        "GetBillingTimeRested",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(Value::Number(f64::from(model.billing_time_rested)))
        })?,
    )?;
    // GetTimeToWellRested(): always nil; the reference's binding (`0x48d4b0`) only pushes nil.
    g.set(
        "GetTimeToWellRested",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;

    // TargetUnit(unit): queue the token for the app to resolve and select; a token naming no unit
    // is a no-op, as in the reference. A held, selectable unit is shown as `"target"` at once, as
    // `0x4899d0` commits before it returns, unless the targeting cursor is up: `SelectTarget` binds
    // the unit to the cast then and selects nothing (`0x4935d5`). The gate's tag 2 of that bind
    // (`0x6e5ba5`) is asked here, where the stack that decides it is.
    g.set(
        "TargetUnit",
        lua.create_function(|lua, token: Option<String>| {
            if let Some(token) = token {
                let may_cast = select_may_cast(lua);
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    if !model.selection_call_declined() {
                        if let Some(state) = model.held_unit_view(&token) {
                            model.show_selection(Some(state));
                        }
                    }
                }
                submit(
                    lua,
                    ScriptCall::Select {
                        request: SelectionRequest::Unit(token),
                        may_cast,
                    },
                )?;
            }
            Ok(())
        })?,
    )?;

    // AssistUnit(unit) (`0x489b80`): select the unit's `UNIT_FIELD_TARGET`. The shared tail
    // (`0x489bb2`-`0x489c07`) returns silently on 0 and selects through `0x489a40`, which leaves
    // the selection alone when nothing resolves; no `CanAssist` gate (`0x6066f0`), any unit, and a
    // swing only with `assistAttack` set (default "0", `0x48fc50`). A token that resolves nothing
    // is silent here, where the reference shows `0xb8` ERR_GENERIC_NO_TARGET (`0x489c0e`).
    g.set(
        "AssistUnit",
        lua.create_function(|lua, token: Option<String>| {
            if let Some(token) = token {
                let may_cast = select_may_cast(lua);
                submit(
                    lua,
                    ScriptCall::Select {
                        request: SelectionRequest::Assist(token),
                        may_cast,
                    },
                )?;
            }
            Ok(())
        })?,
    )?;

    // TargetLastEnemy(): re-select the last attackable target (`0x489b45` reads `0xb4e2e8`, which
    // `SetSelection` stamps at `0x49377d`), a no-op once it has streamed out. The app remembers it.
    g.set(
        "TargetLastEnemy",
        lua.create_function(|lua, ()| {
            let may_cast = select_may_cast(lua);
            submit(
                lua,
                ScriptCall::Select {
                    request: SelectionRequest::LastEnemy,
                    may_cast,
                },
            )?;
            Ok(())
        })?,
    )?;

    // TargetByName(name [, exactMatch]) (`0x489d60`, resolver `0x493aa0`, shared with `/target`): a
    // case-insensitive whole-name match wins, else without `exactMatch` the longest common prefix,
    // so "Rag" selects Ragnaros. Any unit (typemask 8), no dead, reaction, range or self gate. A
    // missing name, or one neither string nor number, raises `Usage:` (`0x489d69`, `0x6f4940`); a
    // number is taken as its string. Deviation: among whole-name matches ours picks the nearest,
    // because the reference's first-walked pick is order-dependent and reads as a bug. A miss
    // shows `0x127` ERR_UNIT_NOT_FOUND, or `0xb8` ERR_GENERIC_NO_TARGET for an empty name
    // (`0x489dbd`, `0x489dcf`), from the app's commit. The pick is shown as `"target"` at once,
    // from the app's candidates (`by_name`), so `UnitName("target")` on the next line names it,
    // unless it is not selectable and `SetSelection` returns (`0x4935ee`); the call still commits
    // it.
    g.set(
        "TargetByName",
        lua.create_function(|lua, (name, exact): (Value, Option<Value>)| {
            let name =
                super::super::binding_abi::string_arg(lua, name, "Usage: TargetByName(\"name\")")?;
            // Argument 2 comes from `0x6f1c10` with default 0: it never raises, and 0 is absent.
            let exact = super::super::binding_abi::bool_or_default(exact.as_ref(), false);
            let may_cast = select_may_cast(lua);
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                // With the targeting cursor up the pick is bound, not selected (`0x4935d5`), and a
                // charmed player's select returns first (`0x49359c`).
                let pick = model
                    .named_unit(&name, exact)
                    .filter(|c| c.selectable && !model.selection_call_declined());
                if let Some(state) = pick.map(|c| c.state.clone()) {
                    model.show_selection(Some(state));
                }
            }
            submit(
                lua,
                ScriptCall::TargetByName {
                    name,
                    exact,
                    may_cast,
                },
            )?;
            Ok(())
        })?,
    )?;

    // DropItemOnUnit(unit) (`0x48d960`): drop the cursor's item on a unit, feeding a pet or
    // opening a trade with a player. Only the token is queued; the app runs every gate.
    g.set(
        "DropItemOnUnit",
        lua.create_function(|lua, token: Option<String>| {
            // The pet leg is a cast, `CastFeedPetOnItem 0x6ea1e0` → `TryCastSpell`, behind the
            // gate's tag 2 (`0x6e4bc1`); the app builds that leg alone, for the token `"pet"`.
            if let Some(token) = token.filter(|token| {
                token != "pet"
                    || crate::script::protected::gate(lua, crate::script::protected::Tag::CAST)
            }) {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.drop_item_on_unit.push(token);
            }
            Ok(())
        })?,
    )?;

    // SpellTargetUnit(unit) (`0x6e6d90`): an argument that is not a string or a number raises
    // `Usage:`; while not targeting the call is a silent no-op, before the token is read; an
    // unknown token then raises `Unknown unit name`. The token queues for the host, which ends
    // targeting with "Out of range." when it names no unit and otherwise runs `BindTarget`
    // (`0x6e5b10` → `0x6e5b40`).
    g.set(
        "SpellTargetUnit",
        lua.create_function(|lua, token: Value| {
            let token = crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: SpellTargetUnit("unit")"#,
            )?;
            // The gate's tag 2 follows the argument check (`0x6e6dc1`) and precedes the
            // targeting test (`0x6e6dca`); a refusal is silent.
            if !crate::script::protected::gate(lua, crate::script::protected::Tag::CAST) {
                return Ok(());
            }
            if !lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .spell_targeting
            {
                return Ok(());
            }
            check_unit_token(&Some(token.clone()))?;
            submit(lua, ScriptCall::SpellTargetUnit(token))?;
            Ok(())
        })?,
    )?;

    // ClearTarget() (`0x489ff0`): 1 when it cleared a target, nil when there was none, which
    // `ToggleGameMenu`'s Escape chain needs to fall through to the menu. The deselect is queued
    // either way: it reads the selection as the calls before it leave it (`0x489ff0`-`0x489fff`),
    // so `TargetUnit("player") ClearTarget()` ends with nothing selected, and `"target"` reads
    // nothing for the rest of the chunk.
    g.set(
        "ClearTarget",
        lua.create_function(|lua, ()| {
            let had = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let had = model.unit("target").is_some_and(|u| u.exists);
                // A charmed player's deselect returns first (`0x49359c`).
                if had && !model.unit("player").is_some_and(|u| u.charmed) {
                    model.show_selection(None);
                }
                had
            };
            submit(lua, ScriptCall::ClearTarget)?;
            Ok(flag(had))
        })?,
    )?;

    Ok(())
}
