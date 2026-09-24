//! The `Unit*` global registrations (see the parent module's doc for the seam and the return
//! shapes): every binding reads the per-token [`UnitState`](super::UnitState) snapshot store
//! through the parent's `with_unit`/`pick_unit_token` helpers.
//!
//! **Every predicate here returns through one function** — [`super::unit_predicate`] when it reads
//! a snapshot field, [`flag`](super::super::binding_abi::flag) when it computes its own bool.
//! Neither ever hands mlua a Rust `bool`: all 29 of the reference's unit predicates push the
//! constant double `1.0` (`lua_pushnumber 0x6f3810`) or `nil` (`lua_pushnil 0x6f37f0`), one value
//! at every live `ret`, and **no binding in the 83-entry table at `0x850438` calls
//! `lua_pushboolean 0x6f39f0` at all**. A new predicate that open-codes
//! `Value::Integer(1)`/`Value::Nil`, or returns a `bool`, is the drift those records exist to stop.
//!
//! **The scope of that claim is the unit table, not "the binding surface"** — 2043 said the wider
//! thing and 2048 corrected it. `lua_pushboolean` exists at `0x6f39f0` with seven call sites, and
//! one of them *is* a registered FrameScript binding: `IsPetAttackActive 0x4be0e0` answers a real
//! Lua `true`/`false`, never nil (ours already does — `super::super::pet`).

use mlua::{Lua, Value};

use super::super::binding_abi::flag;
use super::super::Model;
use super::{
    check_unit_token, classification_word, grey_band, level_reads_unknown, pick_unit_token,
    unit_predicate, unknownobject, with_unit, PlayerRecord, SelectionRequest,
};

/// **The `"player"` fast path, shared by the four verbs that take it**.
///
/// `Some(x)` = this token is `"player"` and `x` is the record's answer; `None` = it is any other
/// token and the caller falls through to the unit resolver. That is the reference's own shape —
/// a full-string, case-insensitive compare of the token against `0x847894` (`b"player\0"`) whose
/// **match is the fall-through** — and it is one function rather than four copies for the reason
/// [`super::super::names::gated_rank`](crate)'s sibling comment gives about rank: four readers of
/// one record, each testing the token itself, is how they drift apart. `UnitName`, `UnitRace`,
/// `UnitClass` and `UnitSex` are the whole set; `UnitLevel` is **not** one of them.
///
/// The compare is a full string, not a prefix: `"playerfoo"` is a recognised-but-unresolvable
/// token ([`super::token_recognised`]) and goes to the resolver, not here.
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

/// The two class ids `GetComboPoints 0x51a190` accepts — the literals `4` and `0xb` it compares
/// the class byte `[[player+0x110]+0x79]` against. That byte is `UNIT_FIELD_BYTES_0` byte 1, the
/// same value [`PlayerReqState::class_id`](super::super::PlayerReqState) carries; `UnitClass`'s own
/// binding (`0x518350`) reads it through the identical `[obj+0x110]+0x79` chain, which is what
/// identifies it. The *names* are the conventional vanilla ids — the client's class table is
/// heap-built, so nothing in the file maps 4→Rogue by itself.
const CLASS_ROGUE: u32 = 4;
const CLASS_DRUID: u32 = 11;

/// Register the `Unit*` globals reading the per-token snapshot store (the same style/place the
/// object model and stdlib register their globals — bare globals on `_G`, matching the live API
/// surface).
pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "UnitExists",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.exists)
        })?,
    )?;

    // `UnitIsVisible(unit)` — `0x516030`, and it is **object presence, nothing else**:
    // `ClntObjMgrObjectPtr(resolve(token), TYPEMASK_UNIT) != NULL`. 57 bytes, one branch, no field
    // read and no comparison beyond `test eax,eax`.
    //
    // **Not a synonym for `UnitExists`, and neither implies the other.** An out-of-range party
    // member has a roster entry and no object: `UnitExists` = 1 through its GUID fallback,
    // `UnitIsVisible` = nil. That pair is the branch pfUI takes seven times — most visibly
    // `if not UnitIsVisible(unitstr) or not UnitIsConnected(unitstr)`, which chooses between a 3D
    // portrait and a flat one.
    //
    // **The return is the NUMBER 1, never a boolean** — `0x6f3810` writes tag 3 with the operands
    // of `1.0` on the true leg, `lua_pushnil` on the false one. `UnitExists` above answers a Rust
    // `bool` and so hands Lua `true`/`false`; that is its own pre-existing question, and matching
    // it here would have been the wrong kind of consistency.
    g.set(
        "UnitIsVisible",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.has_object)
        })?,
    )?;

    // `UnitIsTapped(unit)` / `UnitIsTappedByPlayer(unit)` — `0x519c90` / `0x519d00`, a masked-byte
    // pair (108 bytes each; only the mask and the `Usage:` string differ). Each is
    // `object present && (UNIT_DYNAMIC_FLAGS & mask)` and nothing else: no ownership, no GUID
    // compare, no party/raid or health conjunct anywhere in either body.
    //
    // **Shape A, unlike `UnitIsVisible` directly above** — these two carry an `lua_isstring` gate
    // and a `Usage:` `luaL_error`, where their neighbour has none. Two adjacent verbs in one
    // family with opposite argument shapes is exactly why 1717's taxonomy is settled per binding;
    // inheriting the sibling's shape here would have been wrong in the quiet direction.
    //
    // The reference raises TWO different messages — `Usage:` for a non-string/non-number, and the
    // resolver's `"Unknown unit name: %s"` for a bad token (and for any NUMBER, since the
    // `lua_isstring` gate admits tag 3 and hands the resolver `"5"`). `check_unit_token` inside
    // `with_unit` is the second of those; the first is the `?` on the argument type below.
    // The gate is the BINDING's, not `with_unit`'s: `check_unit_token` lets a nil through by
    // design, because that is right for `UnitExists`, `UnitIsVisible` and eleven others. **"and
    // most of this family" is what this comment used to say, and it was the wrong way round** —
    // all 83 entries of the table at `0x850438` gate and raise in 53 cases, with only 13 unit-token
    // bindings quiet. These two were never the exception; they were an early
    // instance of the rule.
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
                // Shape A: a string OR a number passes (`lua_isstring` admits tag 3, and the
                // client hands the resolver `"5"` — which then raises `Unknown unit name: 5`,
                // the family's SECOND message). Everything else — nil, absent, boolean, table,
                // function — is the `Usage:` raise.
                let token = Some(crate::script::binding_abi::string_arg(lua, token, usage)?);
                let hit = with_unit(lua, &token, false, |u| {
                    if by_player {
                        u.tapped_by_player
                    } else {
                        u.tapped
                    }
                })?;
                // One value, and it is the NUMBER 1 or nil — never a boolean, the same shape the
                // rest of this family answers in.
                Ok(flag(hit))
            })?,
        )?;
    }

    // `UnitIsPartyLeader(unit)` — `0x516210`. **Two legs, ORed, covering disjoint failures:**
    //
    //     o = ObjPtr(resolve(token), TYPEMASK_PLAYER)          -- 0x10, not this family's 8
    //     (o != NULL && (o.PLAYER_FLAGS & 0x1)) || resolve(token) == g_groupLeaderGuid
    //
    // The descriptor leg answers for any held player — including a stranger who leads their OWN
    // party, which a comparison against our group's leader can never express. The GUID leg answers
    // for a group member whose object the client does not hold (an out-of-range `party3`, any
    // `raidN`), where there is no descriptor to read. Neither is sufficient alone, which is why
    // this is not derivable from `IsPartyLeader()` + `GetPartyLeaderIndex()` however it is
    // arranged.
    //
    // **No zero guard, and that is deliberate.** `IsPartyLeader 0x4e9130` short-circuits on a
    // `0:0` cached leader; this one does not. An unresolvable-but-non-raising token resolves to
    // `0:0`, which equals the zeroed leader while ungrouped — so `UnitIsPartyLeader(nil)` answers
    // **1 solo**. It reads like a bug and it is the behaviour; answering nil there would be the
    // divergence.
    //
    // Shape C with a shape-A tail: no `lua_isstring` gate and no `Usage:` of its own, but a bad
    // token — and any Lua NUMBER, which the resolver stringifies first — raises
    // `"Unknown unit name: %s"` from `check_unit_token`. One value on both legs, the number 1 or
    // nil, never a boolean.
    g.set(
        "UnitIsPartyLeader",
        lua.create_function(|lua, token: Option<String>| {
            check_unit_token(&token)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let by_flag = token
                .as_ref()
                .and_then(|t| model.unit(t))
                .is_some_and(|u| u.group_leader);
            // The GUID leg. A token with no snapshot resolves to GUID 0 — the reference's `0:0` —
            // and 0 == the zeroed leader is exactly the solo case above.
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
            // TWO values on every live path (`eax = 2` at `0x5170ae` and `0x517289`): the name and
            // the **realm**. Slot 2 is nil for a same-realm player, and *structurally* nil for a
            // pet, creature, game object or item — `0x609210` writes the out-parameter only on its
            // PLAYER branch. benilla is single-realm, so the second is always nil here; the day a
            // cross-realm name arrives it is `UnitState`'s to carry, and `UnitPopup.lua:106`'s
            // `name.."-"..server` join is what will read it.
            //
            // Not to be confused with `showServerName`: that is the parameter of FrameXML's own
            // `GetUnitName(unit, showServerName)` wrapper, which calls this binding with ONE
            // argument. The engine's real second argument is a strict `LUA_TBOOLEAN` and is not
            // modelled — no consumer passes it.
            //
            // Value 1 — and the ONLY two nils a recognised token can produce (`0x517020`): the
            // `"player"` fast path reads the local name buffer and pushes nil when it is empty
            // (`0x51708c` → `0x5abdc0`; the `0x517083` this used to cite is the token's string
            // *compare*, `call 0x64a4c0` — corrected 2261), and a token that resolves to GUID 0
            // pushes nil (`0x5170c0`).
            // The empty-buffer nil is not an explicit push either: `0x517095` is
            // `lua_pushstring`, which falls through on NULL at `0x6f3895` into `lua_pushnil`. EVERY other path ends in a
            // string — the cached name, or `FrameScript_GetText("UNKNOWNOBJECT")`: `0x517220` for
            // a GUID with no object and no cache row, `0x609324` inside `CGUnit_C::GetUnitName`
            // for a unit whose name cache has not answered or is stale (a pet's is
            // `petnamecache.wdb` (`0x554e10`), keyed by `UNIT_FIELD_PETNUMBER` (`0x6092b2`)). A
            // freshly called pet is that case by construction: `UNIT_PET` fires off the
            // descriptor and the name lands a `CMSG_PET_NAME_QUERY` round-trip later, and stock
            // `PetStable.lua:129` concatenates the answer in between.
            //
            // "Resolved to a GUID" is a SEATED SNAPSHOT — not `exists`. The reference's
            // `UnitExists` is a conjunction with `IsSelectable` (the `UnitState::exists` doc's
            // named gap), so a not-selectable unit reads its name there with `UnitExists` nil; the
            // name resolver's own nil is GUID 0 and nothing else, and a feed that seats a token
            // has resolved it.
            //
            // **The `"player"` arm is the BUFFER, not the snapshot**. `0x51708c`
            // reads `0x5abdc0` — the local name buffer at `0xc27d88` — and returns; it never
            // reaches the resolver, so no object, no descriptor and no name-query answer is
            // involved on this path at all. Modelling it off the snapshot (as this did until
            // 2261) put the one name the client always knows behind the one cache that can miss:
            // decision 2260's realm-cache load dropped our own guid, the feed pushed a nameless
            // player over the roster seat, and `UnitName("player")` read nil in the world.
            //
            // The buffer's emptiness is still the reference's own nil — it just means something
            // a snapshot cannot say, and something a client that is in the world never is:
            // "no Enter World has been committed in this process".
            if let Some(seeded) = player_record_arm(lua, &token, |r| r.name.clone()) {
                let name = if seeded.is_empty() {
                    Value::Nil
                } else {
                    Value::String(lua.create_string(&seeded)?)
                };
                return Ok((name, Value::Nil));
            }
            let name = with_unit(lua, &token, None, |u| Some(u.name.clone()))?;
            let name = match name {
                None => Value::Nil,
                Some(Some(n)) => Value::String(lua.create_string(&n)?),
                Some(None) => Value::String(unknownobject(lua)?),
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

    // UnitLevel (`0x517fc0`): the raw UNIT_FIELD_LEVEL — a raw ≤ 0
    // VERBATIM (never −1) — or **−1** iff world-boss rank 3 (unconditional) / hostile
    // (reaction ≤ 1 internal) AND ≥ 10 levels above the player (inclusive). The FrameXML
    // target frame branches its skull on `<= 0` (`TargetFrame_CheckLevel`), so a level-0
    // (unstreamed) unit skulls through the verbatim 0, exactly as the reference does. Not
    // carried: the dormant attackable-decay override (`max(1, raw − round(min(b,100)·0.05))`,
    // `b` INFERRED and 0 in normal play — it can never drive the value ≤ 0 anyway).
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
            Ok(if u.level == 0 {
                0
            } else if level_reads_unknown(u, model.player_req.level) {
                -1
            } else {
                i64::from(u.level)
            })
        })?,
    )?;

    // UnitIsCorpse (`0x5161c0`) → 1/nil: a pure OBJECT-TYPE check —
    // the token resolves to a live TYPEID_CORPSE world object (a released player's remains).
    // NO health test: a dead mob or dead player is NOT a corpse (the ref target frame shows a
    // dead mob's level number, not the skull). Reads [`UnitState::corpse_object`], which no
    // feed sets yet — corpse objects aren't selectable in benilla — so this returns nil today,
    // faithfully.
    g.set(
        "UnitIsCorpse",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.corpse_object)
        })?,
    )?;

    // UnitCanAttack (`0x516c50`) → 1/nil: pure delegation to the
    // `CanAttack 0x606980` predicate, read from the non-player token's app-fed
    // snapshot ([`UnitState::can_attack`]). Directional in the live API; our snapshot carries
    // the player→unit direction, the only order the shipped FrameXML calls
    // (`UnitCanAttack("player", "target")`).
    g.set(
        "UnitCanAttack",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // BOTH positions carry an `lua_isstring` gate — two sites in the body —
            // so either argument being nil raises.
            let a = Some(crate::script::binding_abi::string_arg(
                lua,
                a,
                r#"Usage: UnitCanAttack("unit", "otherUnit")"#,
            )?);
            let b = Some(crate::script::binding_abi::string_arg(
                lua,
                b,
                r#"Usage: UnitCanAttack("unit", "otherUnit")"#,
            )?);
            let token = pick_unit_token(&a, &b);
            unit_predicate(lua, &token, |u| u.can_attack)
        })?,
    )?;

    // GetQuestGreenRange (`0x4e17d0`) — the green→grey boundary the
    // FrameXML `GetDifficultyColor` buckets by (ref QuestLogFrame.lua l.593):
    // `GRAYBAND[min(playerLevel/5, 19)]` off the binary's `0x8076c0` table, byte-identical to
    // the `0x81dda8`/`0x80ae98` twins [`grey_band`] transcribes. No args; 0 with no player.
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

    // The other two of the client's death trio: a released ghost has health 1,
    // so IsDead is false for it and the popup flow branches on all three.
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

    // UnitReaction(unit, other) → the reaction scale, or nil. The live API is directional (unit's
    // reaction toward `other`); our feed only resolves it for the "target" token toward the player,
    // which is the sole caller (`TargetFrame_CheckFaction`), so the `other` arg is accepted and
    // unused.
    //
    // **NOT a 1/nil predicate, and its nil does not mean "reaction 0"** (correcting
    // 2043's aside). `0x5167e0` pushes `0x6061e0(u1, u2)` **plus one** (`0x51683e inc eax`,
    // `0x516842 fild`) — a self-compare answers **5** — so the value is 1-based and **0 is
    // unreachable**. Its nil leg (`0x51685f`) means only that a token failed to resolve to a live
    // UNIT.
    //
    // Ours maps our own `reaction == 0` to nil because that is our sentinel for "not yet fed", and
    // the observable is the same nil an unresolved token gives. The gap is the feed's, not the
    // shape's: a resolved unit whose reaction has not streamed answers nil here where the reference
    // answers a number. The target frame paints its name plate blue on that nil.
    g.set(
        "UnitReaction",
        lua.create_function(|lua, (token, _other): (Value, Value)| {
            // BOTH positions carry an `lua_isstring` gate — two sites in the body —
            // so either argument being nil raises.
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitReaction("unit", "otherUnit")"#,
            )?);
            let _other = Some(crate::script::binding_abi::string_arg(
                lua,
                _other,
                r#"Usage: UnitReaction("unit", "otherUnit")"#,
            )?);
            let r = with_unit(lua, &token, 0u8, |u| u.reaction)?;
            Ok(if r == 0 {
                Value::Nil
            } else {
                Value::Integer(i64::from(r))
            })
        })?,
    )?;

    // UnitIsEnemy / UnitIsFriend — the reaction-thresholded pair (the ref target-select sound
    // branch, TargetFrame_OnShow). v1 derives both from the same snapshot as `UnitReaction`:
    // enemy = reaction ≤ 2 (hated/hostile), friend = reaction ≥ 5 (friendly+); the live API's
    // extra PvP inputs (duels, flagged players, sanctuaries) are deferred with the rest of the
    // PvP wire. The pair is directional in the live API but our snapshot only carries the
    // target↔player reaction, so the binding reads whichever arg isn't "player" (the ref calls
    // both orders: UnitIsEnemy("target","player"), UnitIsFriend("player","target")). Unknown
    // reaction (0) → nil for both, the API's "can't tell". `1`/nil returns, era-style.
    g.set(
        "UnitIsEnemy",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // BOTH positions carry an `lua_isstring` gate — two sites in the body —
            // so either argument being nil raises.
            let a = Some(crate::script::binding_abi::string_arg(
                lua,
                a,
                r#"Usage: UnitIsEnemy("unit", "otherUnit")"#,
            )?);
            let b = Some(crate::script::binding_abi::string_arg(
                lua,
                b,
                r#"Usage: UnitIsEnemy("unit", "otherUnit")"#,
            )?);
            let token = pick_unit_token(&a, &b);
            let r = with_unit(lua, &token, 0u8, |u| u.reaction)?;
            Ok(flag((1..=2).contains(&r)))
        })?,
    )?;
    g.set(
        "UnitIsFriend",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // BOTH positions carry an `lua_isstring` gate — two sites in the body —
            // so either argument being nil raises.
            let a = Some(crate::script::binding_abi::string_arg(
                lua,
                a,
                r#"Usage: UnitIsFriend("unit", "otherUnit")"#,
            )?);
            let b = Some(crate::script::binding_abi::string_arg(
                lua,
                b,
                r#"Usage: UnitIsFriend("unit", "otherUnit")"#,
            )?);
            let token = pick_unit_token(&a, &b);
            let r = with_unit(lua, &token, 0u8, |u| u.reaction)?;
            Ok(flag(r >= 5))
        })?,
    )?;

    // UnitIsPlayer(unit) → 1 if the unit is a player character, else nil. Reads the snapshot's
    // guid-family flag (the same one the unit tooltip's "(Player)" line keys on). The target frame's
    // faction tint branches on it (`TargetFrame_CheckFaction`): a player-controlled unit takes the
    // red/blue player legs, an NPC the reaction swatch. (The live gate is `UnitPlayerControlled`,
    // which also covers pets/charmed creatures; we resolve only the player half of it — the extra
    // reach needs a player-controlled flag we don't carry, and a player's own alt is the case here.)
    // UnitIsCivilian(unit) → 1 if killing this unit would be a DISHONORABLE kill, else nil. This is
    // `0x612550` itself handed to Lua — the same four-term predicate (`pvp` bit AND creature-query
    // civilian flag AND hostile AND the kill would be grey) that the unit tooltip's green CIVILIAN
    // line and `UnitPVPName`'s civilian arm already run through `is_civilian_kill`. That function's
    // doc called itself "ONE home, two callers"; the stock target frame is the third, and it is the
    // one that names the predicate — `TargetFrame_CheckDishonorableKill` (TargetFrame.lua:230) is
    // its only FrameXML caller and the comment inside it reads "Is a dishonorable kill".
    //
    // Sharing the function is the point: a second copy would let the tooltip, the name and the
    // target plate disagree about the same mob.
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
    // UnitPlayerControlled(unit) → 1 if a PLAYER is driving this unit, else nil. `UNIT_FIELD_FLAGS`
    // bit 3 (`UNIT_FLAG_PVP_ATTACKABLE 0x8`), which is wider than `UnitIsPlayer` below: a player's
    // pet and a charmed creature answer 1 here and nil there. Stock `UnitFrame_OnEnter`
    // (UnitFrame.lua:58) is the caller that made this a blocker rather than a nicety — it gates the
    // player-options newbie tip on it, so a missing binding raised on the first unit-frame hover.
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
    // UnitIsPlusMob(unit) → 1 if this unit is a "plus" mob, else nil (`0x516d40`, extent
    // `[0x516d40,0x516d8f)`). One bit of the SAME `UNIT_FIELD_FLAGS` word `UnitPlayerControlled`
    // reads: `shr ecx,6; test cl,1` — bit 6, `UNIT_FLAG_PLUS_MOB 0x40`.
    //
    // **It is not a rank read**, which is the trap the name sets. `UnitClassification` is its
    // table neighbour and answers off the gated creature rank, so the obvious
    // implementation is `rank > 0` — but `0x516d40` never calls the rank getter `0x605620` (whose
    // six callers are enumerated) and never touches the creature cache. The two agree in practice
    // because the SERVER derives the bit from the rank — vmangos `Creature::UpdateEntry` sets it
    // on `!IsPet() && rank > 0` (`Creature.cpp:634`, INFERRED from source) — so **rare** answers 1
    // here alongside elite, rare-elite and world boss, while a player, a pet and a normal mob
    // answer nil. Where they part is the unstreamed unit: a creature whose cache record has not
    // arrived still carries its own flags, so this answers truthfully where a rank read would say
    // "normal". No client code writes the bit (zero bitwise RMWs at `+0xa0` image-wide), so it is
    // the server's word verbatim.
    //
    // No stock FrameXML file calls it, which is why nothing shipped ever raised on its absence;
    // addons do (`FuBar_DakSmak` colours its tooltip with it), and calling the nil global is what
    // B385 reported. Last of the three verbs decision 1834 left loudly absent.
    g.set(
        "UnitIsPlusMob",
        lua.create_function(|lua, token: Option<String>| {
            // One of 1834's quiet thirteen: no `lua_isstring` gate and no `Usage:` arm, so a nil
            // or absent token is a quiet nil. An UNRECOGNISED one still raises — the body reaches
            // the shared resolver through `0x515940`, and that is `unit_predicate`'s own gate.
            unit_predicate(lua, &token, |u| u.flags & 0x40 != 0)
        })?,
    )?;

    // The identity predicates (the unit popup's menu pick + gating). Same-token
    // is trivially the same unit; otherwise both snapshots must carry a real (nonzero) guid.
    g.set(
        "UnitIsUnit",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // BOTH positions carry an `lua_isstring` gate — two sites in the body —
            // so either argument being nil raises.
            let a = Some(crate::script::binding_abi::string_arg(
                lua,
                a,
                r#"Usage: UnitIsUnit("unit", "otherUnit")"#,
            )?);
            let b = Some(crate::script::binding_abi::string_arg(
                lua,
                b,
                r#"Usage: UnitIsUnit("unit", "otherUnit")"#,
            )?);
            // BOTH arguments go through the resolver, so either being unrecognised raises.
            check_unit_token(&a)?;
            check_unit_token(&b)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (Some(a), Some(b)) = (a, b) else {
                return Ok(Value::Nil);
            };
            let (Some(ua), Some(ub)) = (model.unit(&a), model.unit(&b)) else {
                return Ok(Value::Nil);
            };
            if !ua.exists || !ub.exists {
                return Ok(Value::Nil);
            }
            Ok(flag(a == b || (ua.guid != 0 && ua.guid == ub.guid)))
        })?,
    )?;

    // UnitAffectingCombat(unit) → the number 1 in combat, else nil (`0x517e10`).
    //
    // The unusual half is the miss shape: **false and "no such unit" are the SAME arm**
    // (`0x517e48 je 0x517e73` joins `0x517e5c je 0x517e73`), so an unresolvable token is
    // indistinguishable from a peaceful one. Reproduced deliberately — an addon cannot use this
    // binding to probe whether a unit exists, and ours must not let it either.
    //
    // A **bad or missing argument raises**, unlike its `UnitInRaid` neighbour: the guard here is
    // `0x6f3510` (number-or-string, and NULL for an absent argument) with the usage literal at
    // `0x851070` behind `0x6f4940`, which does not return. A *number* is accepted and stringified
    // — `UnitAffectingCombat(5)` resolves the token `"5"`, finds nothing, and answers nil.
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

    // UnitInRaid(unit) → **the constant number 1** on a hit, nil on a miss. NOT an index, and
    // never a raise (`0x516350`).
    //
    // Three things about this binding are the opposite of the obvious guess, and all three are
    // read off its 83 bytes:
    //
    //  * **The hit value is a hard-coded `1.0`** — `0x51637e push 0x3ff00000; push 0` into the
    //    push-number helper. The membership helper underneath (`0x4baee0`) only ever returns
    //    `mov eax,1` / `xor eax,eax`; it never exposes its loop counter, so there is no index to
    //    be 0- or 1-based about. An addon doing `local i = UnitInRaid(u)` and then
    //    `GetRaidRosterInfo(i)` reads member 1 on the real client too.
    //  * **It never raises.** `0x6f3690` hands back NULL for a missing or uncoercible argument
    //    instead of erroring, `0x515970` maps NULL/empty to GUID `0:0`, and `0x4baee0`
    //    short-circuits `0:0` to false at its entry (`or eax,esi; je`). So a missing, wrong-typed
    //    or unresolvable unit answers `nil`, not an error — hence `Option<String>` and no usage
    //    string, unlike its `GetRaidRosterInfo` sibling.
    //  * **The player is in the roster array**, so no `t == "player"` special case is needed here
    //    — unlike `UnitInParty` below, whose roster excludes the recipient.
    // HasFullControl() → 1 | nil: the reference's `[0xb4b3e4]` read at `0x51a158` — the control
    // flag `SMSG_CLIENT_CONTROL_UPDATE` writes for the local player, which the stock unit menu
    // greys its follow/trade rows on (1958).
    g.set(
        "HasFullControl",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.player_control))
        })?,
    )?;

    // UnitPlayerOrPetInParty(unit) / UnitPlayerOrPetInRaid(unit) → 1 | nil: the unit is a
    // member of the group, or a member's pet — its owner (`UNIT_FIELD_SUMMONEDBY`, else the
    // charmer, else the creator: `UnitState::owner`) is. The bindings are registered
    // (`0x5162f0` / `0x5163b0`) and delegate to a C++ predicate not yet identified; the
    // owner reading is this file's, flagged in 1958.
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

    // UnitInParty(unit) → 1 when the unit is the player-in-a-group or one of the party1..4
    // members (a party token directly, or any token whose guid matches the roster's).
    g.set(
        "UnitInParty",
        lua.create_function(|lua, token: Option<String>| {
            check_unit_token(&token)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(t) = token else {
                return Ok(Value::Nil);
            };
            let Some(u) = model.unit(&t) else {
                return Ok(Value::Nil);
            };
            let grouped = !model.party.members.is_empty();
            if !u.exists || !grouped {
                return Ok(Value::Nil);
            }
            let hit = (t.starts_with("party") && !t.starts_with("partypet"))
                || t.eq_ignore_ascii_case("player")
                || (u.guid != 0
                    && (model.unit("player").is_some_and(|p| p.guid == u.guid)
                        || model
                            .party
                            .members
                            .iter()
                            .any(|m| m.guid != 0 && m.guid == u.guid)));
            Ok(flag(hit))
        })?,
    )?;

    // UnitCanCooperate(a, b) → 1 for a friendly PLAYER unit. DEVIATION, stated: the client
    // resolves faction-template cooperation masks; this reads the snapshot's is_player +
    // UnitIsFriend's reaction>=5 — the same verdict for every case the popup gates on (invite/
    // whisper a same-faction player), without the faction machinery the engine doesn't carry.
    g.set(
        "UnitCanCooperate",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            // BOTH positions carry an `lua_isstring` gate — two sites in the body —
            // so either argument being nil raises.
            let a = Some(crate::script::binding_abi::string_arg(
                lua,
                a,
                r#"Usage: UnitCanCooperate("unit", "otherUnit")"#,
            )?);
            let b = Some(crate::script::binding_abi::string_arg(
                lua,
                b,
                r#"Usage: UnitCanCooperate("unit", "otherUnit")"#,
            )?);
            let token = pick_unit_token(&a, &b);
            let ok = with_unit(lua, &token, false, |u| {
                u.exists && u.is_player && u.reaction >= 5
            })?;
            Ok(flag(ok))
        })?,
    )?;

    // GetRaidTargetIndex(unit) → the mark slot 1..8, or nil unmarked (decision 0434 §6's board,
    // fed per token).
    g.set(
        "GetRaidTargetIndex",
        lua.create_function(|lua, token: Value| {
            // Gated (`0x4bb4be` → `0x4bb4cd`), and the usage string names the
            // argument unquoted — the reference's own spelling.
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

    // The party-frame status predicates (decision 0434 §2/§3): connection, AFK/DND, and the two PvP
    // flags. All 1/nil, through the family's one push site (2043).
    //
    // **`UnitIsAFK`/`UnitIsDND` are OURS, not the reference's** — this comment used to claim they
    // "follow the era shape from the start", which named the wrong authority. A raw byte census
    // over the whole image finds ZERO occurrences of `UnitIsAFK`, `UnitIsDND`, `IsAFK` or `IsDND`,
    // as bindings or as strings (positive control: `UnitIsPVP\0` and `UnitIsGhost\0` each return
    // exactly 1). 1.12 surfaces a member's AFK/DND state through `GetGuildRosterInfo` and the
    // `CHAT_FLAG_AFK`/`CHAT_FLAG_DND` GlobalStrings; there is no unit predicate for it. They wear
    // the family's shape because that is the right shape for an invention of ours to wear, not
    // because a binding was read.
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
        "UnitIsAFK",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.is_afk)
        })?,
    )?;
    g.set(
        "UnitIsDND",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.is_dnd)
        })?,
    )?;
    // UnitIsPVP reads the same `pvp` field the unit tooltip's "PvP" line already does (one flag,
    // two callers) — see the field doc.
    g.set(
        "UnitIsPVP",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitIsPVP("unit")"#,
            )?);
            unit_predicate(lua, &token, |u| u.pvp)
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
    // UnitFactionGroup(unit) → (englishGroup, localizedName), the pair the PvP icon law reads:
    // the first names the `UI-PVP-<group>` texture, the second titles the player frame's hit-area
    // tooltip. nil, nil for a unit with no side (Monster/neutral templates) or no snapshot — the
    // ref's icon branches gate on exactly that (decision 0646 §1/§3).
    //
    // **The two are genuinely different strings and this used to push one twice.** They are
    // FactionGroup.dbc's `InternalName` (field 2) and `Name0` (field 3), and only on enUS are they
    // the same word — which is why the duplication survived, and why the old doc on
    // `UnitState::faction_group` said so as if it settled the matter. Every stock consumer of the
    // FIRST return concatenates it into a path: `"…\UI-PVP-"..factionGroup` at `PlayerFrame.lua:68`,
    // `TargetFrame.lua:198` and `PartyMemberFrame.lua:125`, `"…\Battleground-"..` at
    // `BattlefieldFrame.lua:195`, and `HonorFrame.lua:68` compares it to the literal "Alliance".
    // On any localized client the old shape named a texture that does not exist. All five of those
    // files are on our chain.
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
                    // The localized half falls back to the English one rather than to nil: a unit
                    // with a side always has an `InternalName`, and `Name0` is empty for the
                    // Player/Monster rows, so nil here would blank a tooltip the reference fills.
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

    // UnitRace(unit) → (localized, raceFile) or nil, nil; UnitClass(unit) → (localized,
    // classFileName) — the paper doll's "Level %d %s %s" line + the CLASSFILENAME-keyed
    // stat-tooltip lookups. Unknown (feed not landed / a raceless creature)
    // reports nil, nil — the live API's shape for an absent unit.
    g.set(
        "UnitRace",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitRace("unit")"#,
            )?);
            // The `"player"` arm is the RECORD, not the snapshot: `0x518269`
            // reads `0x5abdd0` — `0xc27e80`, the char-enum row's race byte — and resolves it
            // through `ChrRaces`, never reaching the unit resolver. See [`PlayerRecord`].
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
            // The `"player"` arm is the RECORD: `0x5183b9` → `0x5abde0`,
            // `0xc27e81` through `ChrClasses`. Second return is the UPPERCASE token.
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
    // UnitHasRelicSlot(unit) → **the number 1, or nil** (`0x519e50`). Whether this unit's
    // INVSLOT 17 holds a Libram/Totem/Idol instead of a bow.
    //
    // The whole body is: typemask bit 4 (PLAYER) → `UNIT_FIELD_BYTES_0` byte 1 (the class) →
    // bounds against `ds:0xc0def8` → `ds:0xc0def4`[class] → `[row+0x40]`, `ChrClasses.dbc` field
    // 16. There is **no `cmp` against a class id anywhere in the function** — 1.12 is entirely
    // data-driven here, and `benilla_formats::ChrClasses` is that data. The app resolves it and
    // hands us the bit, exactly as it does for `class_file`.
    //
    // **This shipped absent for a long time, on a claim that was simply false** — that the relic
    // slot post-dates 1.12 and `UnitHasRelicSlot` is a later-era verb. The base `dbc.MPQ` copy of
    // `ChrClasses.dbc` really does have no field 16; the `patch.MPQ` copy the client actually
    // reads has it, set for Paladin, Shaman and Druid. Stock `PaperDollFrame.lua`
    // calls this **unconditionally** at l.429 and l.580 — so while it was missing, the character
    // sheet raised `attempt to call global 'UnitHasRelicSlot' (a nil value)` for *every* class,
    // not just the three.
    //
    // No `"player"` fast path (unlike `UnitClass 0x5183b0`, which has one), so this answers for
    // any token — `UnitHasRelicSlot("target")` on an enemy druid is 1, and the stock inspect path
    // at `PaperDollFrame.lua:429` depends on exactly that.
    //
    // The truthy leg is the NUMBER 1, never a boolean; the false leg is nil, never `false`.
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

    // UnitSex(unit) → 2 male, 3 female (1 = neuter — no 1.12 unit feed produces it).
    //
    // **The absent/unstreamed leg is the NUMBER 2, not nil** — `UnitSex 0x517f9f` pushes the
    // constant double 2.0 (`push 0x40000000; push 0`) — the numeric-getter contrast to the
    // predicate family's `1.0`/nil: the numeric getters in this same table answer a number on the
    // unresolved leg, never nil. The shapes table agrees — this row is `(number)`, with no nil
    // alternative anywhere.
    g.set(
        "UnitSex",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitSex("unit")"#,
            )?);
            // The `"player"` arm is the RECORD: `0x517ef9` → `0x5abdf0`,
            // `0xc27e82`. **No bounds check anywhere on that path** — the byte indexes
            // `fild [4*eax+0x808be4]` over `{2,3,1,6}` directly — so an unset record answers `2`
            // ("male") in the reference too, which is what our `0` maps to below.
            let sex = match player_record_arm(lua, &token, |r| r.sex) {
                Some(sex) => sex,
                None => with_unit(lua, &token, 0u8, |u| u.sex)?,
            };
            Ok(Value::Integer(i64::from(if sex == 0 { 2 } else { sex })))
        })?,
    )?;

    // UnitCreatureType(unit) → **1 string, or nil** (`0x51a280`), the sibling of
    // `UnitCreatureFamily 0x51a310`. Adjacent addresses and the same shape — but a DIFFERENT
    // column: this reads `CreatureType.dbc` col `1 + locale`, the family reads col `8 + locale`.
    // "Adjacent so probably identical" was right about the shape and wrong about the map.
    //
    // **A three-stage resolver** (`0x605570`), with no class or typemask gate anywhere — its only
    // three tests are bounds checks:
    //   1. **shapeshift override** — `SpellShapeshiftForm.dbc` col 12, keyed by field 138
    //      `UNIT_FIELD_BYTES_1` byte 2, taken only if signed `> 0` (`0x60559a jg`), so `0` **and
    //      the `-1` sentinel** fall through;
    //   2. **the cached creature record** `[unit+0xb30]+0x18`, returned **unvalidated** — this is
    //      what [`UnitState::creature_type_name`] carries;
    //   3. **race fallback** — `ChrRaces.dbc` col 9, keyed by field 36 `UNIT_FIELD_BYTES_0` byte 0.
    //
    // Stage 3 is why **a PLAYER answers `"Humanoid"`, not nil**: `ChrRaces` col 9 is **7 for all
    // nine shipped race rows** and `CreatureType[7]` is `"Humanoid"`, while `[player+0xb30]` is
    // never populated (a 4-hit writer census: the ctor zeroes it (`0x5fae1a`), two writers
    // early-out on `key==0`, and the third is hard-gated on `OBJECT_FIELD_TYPE == 0x9`
    // (`0x60b131`), an equality a player's `0x19` fails structurally). A creature-record-only
    // reading answers nil there, which would have been wrong for every `UnitCreatureType("player")`
    // and every `("target")` aimed at a player.
    //
    // **Stage 1 is NOT modelled, and it is the one divergence to know about.** A druid in Cat,
    // Bear, Dire Bear, Travel or Aquatic form — and a shaman in Ghost Wolf — answers `"Beast"` on
    // the reference and `"Humanoid"` here. Tree Form, Moonkin, Battle Stance, Shadowform and
    // Spirit of Redemption carry col 12 = `-1` and fall through to race anyway, so those already
    // agree. We hold the active form's spell id and name ([`super::super::ShapeshiftFormView`])
    // but **not the form INDEX the DBC is keyed by**, and matching on a localised form name would
    // be inventing a mechanism the client does not use.
    //
    // The argument gate is real, and differs from the two boolean siblings: arg1 goes through
    // `lua_isstring` (`0x6f3510`) and a miss calls `luaL_error 0x6f4940`, which **longjmps** — so a
    // non-string, including a MISSING argument, abandons the caller's statement. An unresolved
    // *token* is a different thing entirely and answers nil (`0x51a2b8`).
    //
    // Demand: 6 distinct corpus addons over 11 files.
    g.set(
        "UnitCreatureType",
        lua.create_function(|lua, token: Value| {
            let token = match &token {
                Value::String(s) => Some(s.to_str()?.to_string()),
                // `lua_isstring` coerces a number, so a numeric token is accepted and simply
                // resolves to nothing.
                Value::Number(_) | Value::Integer(_) => Some(String::new()),
                _ => return Err(mlua::Error::runtime("Usage: UnitCreatureType(\"unit\")")),
            };
            let word = with_unit(lua, &token, None, |u| {
                u.creature_type_name
                    .clone()
                    // Stage 3, collapsed: every player race maps to CreatureType 7.
                    .or_else(|| u.is_player.then(|| "Humanoid".to_string()))
            })?;
            match word {
                Some(w) => Ok(Value::String(lua.create_string(&w)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // UnitClassification(unit) → "normal" | "elite" | "rareelite" | "worldboss" | "rare" (decision
    // 0782, byte-verified `0x516d90`): a plain table index by the gated rank, and it answers a
    // STRING for every input — never nil. An absent snapshot deliberately reports "normal" rather
    // than nil because the binary does: its unresolved-token path loads table index 0 and pushes
    // that, so a frame reading it gets the plain border art instead of a nil comparison.
    g.set(
        "UnitClassification",
        lua.create_function(|lua, token: Option<String>| {
            let rank = with_unit(lua, &token, 0u32, |u| u.rank)?;
            Ok(classification_word(rank).to_string())
        })?,
    )?;

    // UnitPowerType(unit) → (index, token): (0, "MANA"), (1, "RAGE"), … (the live API also returns
    // alt-power color components; addons that read those handle nil).
    g.set(
        "UnitPowerType",
        lua.create_function(|lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitPowerType("unit")"#,
            )?);
            // ONE value, not the Era pair. `0x517940` has four live `ret`s and `eax = 1` at every
            // one, four `lua_pushnumber` sites and zero `lua_pushstring` — the `(type, "MANA")`
            // tuple does not exist in 5875. `power_token` stays: 1819's per-resource
            // event names are its real caller.
            //
            // The miss default is the NUMBER 0, the same value as Mana and never nil, which is
            // load-bearing rather than tidy: `UnitFrame.lua:122` indexes `ManaBarColor[...]` with it.
            let ty = with_unit(lua, &token, 0u8, |u| u.power_type)?;
            Ok(i64::from(ty))
        })?,
    )?;

    // **`UnitMana`/`UnitManaMax(unit)`, not `UnitPower`** (1188 phase 5, decision 1190's list).
    // These carried Era's names — `UnitPower`/`UnitPowerMax` do not exist in 1.12 at all, which
    // `reference/1.12-globals.tsv` says outright — and an addon feature-detecting `if UnitPower`
    // would have taken a path this client cannot honour.
    //
    // The rename is exact, not approximate: 1.12's verbs take **one argument** and return the
    // current/max of whatever power the unit actually uses (`UnitFrame.lua`: `local currValue =
    // UnitMana(unit)`), which is precisely what the Era pair did when called without a type — and
    // every one of our own call sites called it that way. The type-filtering second argument goes
    // with the name: 1.12 has no such parameter, and asking "how much *mana* does a rage user
    // have" is spelled `UnitPowerType(unit)` first, which we already provide and which IS 1.12.
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

    // UnitXP/UnitXPMax(unit) → the player's XP within the level / the level's requirement. Player-
    // level values (PLAYER_XP is PRIVATE, only our own avatar's), but the live API is unit-tokened:
    // it returns the values only for the "player" token and 0 for any other unit — faithfully, no
    // creature/other player exposes XP. `0` until the app's feed lands (SetMinMaxValues clamps).
    let is_player = |token: &Option<String>| token.as_deref() == Some("player");
    g.set(
        "UnitXP",
        lua.create_function(move |lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitXP("unit")"#,
            )?);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if is_player(&token) {
                i64::from(model.player_xp)
            } else {
                0
            })
        })?,
    )?;
    g.set(
        "UnitXPMax",
        lua.create_function(move |lua, token: Value| {
            let token = Some(crate::script::binding_abi::string_arg(
                lua,
                token,
                r#"Usage: UnitXPMax("unit")"#,
            )?);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if is_player(&token) {
                i64::from(model.player_next_level_xp)
            } else {
                0
            })
        })?,
    )?;

    // UnitIsCharmed(unit) → 1 while somebody is charming this unit, else nil (`0x516cf0`).
    //
    // Three details, all byte-verified, none of them what a reimplementation reaches for:
    //  · the predicate is `UNIT_FIELD_CHARMEDBY != 0` — a plain 64-bit non-zero test on fields
    //    10/11 — **not** a `UNIT_FIELD_FLAGS` bit, which is the obvious guess for a boolean-shaped
    //    unit question;
    //  · a hit is the NUMBER 1 (`lua_pushnumber`, tag 3) and a miss is `nil` (tag 0) — never
    //    `true`/`false`, and never `0`. Exactly one value on both arms;
    //  · it is ASYMMETRIC. The field is "who charms me", so the CHARMER reads nil and the charmed
    //    unit reads 1. `UnitIsPossessed` does not exist in 5875 at all — this is the only
    //    charm verb in the API.
    //
    // KLHThreatMeter is the corpus addon blocked on it (`KTM_My.lua:533`); seven others name it.
    g.set(
        "UnitIsCharmed",
        lua.create_function(|lua, token: Option<String>| {
            unit_predicate(lua, &token, |u| u.charmed)
        })?,
    )?;

    // GetMoney() → the player's purse in copper (a player-level global, not a unit token). The coin
    // display + the merchant window's money line read it; `0` until the app's coinage feed lands.
    g.set(
        "GetMoney",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.money as i64)
        })?,
    )?;

    // GetComboPoints() → the player's banked combo points, 0..5 (a player-level global, not a unit
    // token — the 1.12 binding takes no arguments). `ComboFrame` shows/hides on it, and the
    // combat-text COMBO_POINTS arm reads it.
    //
    // TWO gates before the byte, transcribed from `0x51a190` — the
    // reference `ComboFrame.lua` carries no class check at all, so BOTH of them live here or
    // nowhere:
    //
    //   51a1c3  mov eax,[esi+0x110]; mov al,[eax+0x79]   ; the class byte
    //   51a1cc  cmp al,4 / cmp al,0xb → jne push 0.0     ; ROGUE or DRUID, nothing else
    //   51a1f3  edx=[esi+0xe68]; eax=[edx+0x838] …0x83c  ; PLAYER_FIELD_COMBO_TARGET
    //   51a205  cmp against [0xb4e2d8]/[0xb4e2dc]        ; == the CURRENT target, or push 0.0
    //   51a234  mov al,[ecx+0x1029]                      ; only now, PLAYER_FIELD_BYTES byte 1
    //
    // The class gate is why a *warrior* never sees a dot even though the server really does bank a
    // point for them on a victim's dodge: that byte reaches the usable walk's leg 5 (which has no
    // class test, and is what greys Overpower) but stops here, before any UI can see it.
    // The target gate is why combo points read as "per target": re-targeting empties the dots
    // without the count moving, and selecting the banked unit again refills them.
    //
    // Both comparisons are the binary's own plain equality — no null special case. With nothing
    // banked and no target both GUIDs are 0, which passes the target gate and reads a byte that is
    // 0 anyway; the server writes and clears the pair together.
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

    // The rest-state trio — player-level globals over the app's rest feed
    // ([`UiScript::set_rest_state`]), the MainMenuBar exhaustion tick's and the player frame's
    // whole wire: the surface is Exhaustion.dbc DATA, not client constants — the rows live in the
    // model ([`UiScript::set_exhaustion_rows`]; shipped-table fallback).
    //
    // GetRestState() → (stateID, stateName, multiplier) — `0x48d350`: the raw `PLAYER_BYTES_2`
    // byte 3 indexes Exhaustion.dbc DIRECTLY (the `[0xc0dd78]` ID→row array) and the triple is
    // `(row.ID, row.name[locale], row.factor)`: 1 → (1, "Rested", 2.0), 2 → (2, "Normal", 1.0),
    // and FrameXML's dead 3..5 branches map to the real beta rows (XXXTired 1.0/0.5,
    // XXXExhausted 0.25). Every failure — byte 0 (pre-feed), byte past the table, no row —
    // returns (nil, nil, nil), the binary's own fail path. The multiplier is what
    // `EXHAUST_TOOLTIP1` renders ×100 ("200% of normal experience").
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
    // GetXPExhaustion() → the rested span in BAR-XP units — `0x48d3f0`: the u32 pool × the f32
    // factor of **Exhaustion.dbc row ID 1, hard-coded** (`[[0xc0dd78]+4]`, whatever the current
    // state) — 2.0 in the shipped data, which is the whole "rested XP is double" law: the server
    // drains the pool 1:1 against BASE kill XP while granting +100% (vmangos `GetXPRestBonus`),
    // and the client scales by exactly this row's factor. **nil is decided by the rest-state
    // byte, never by the pool** (`0x48d43b dec/jne`): byte ≠ 1 → nil whatever the pool holds
    // (vmangos's 0 < bonus ≤ 10 hysteresis window sends byte 2 with a nonzero pool — nil there),
    // and a rested byte with pool 0 → the NUMBER 0. The tick parks at `currXP + this`
    // (`ExhaustionTick_Update`).
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
    // IsResting() → 1/nil: inside a rest area (inn/city) right now — `0x516ea0`, byte-VERIFIED:
    // PLAYER_FLAGS `shr 5; test 1` = bit 0x20, pushed as the NUMBER 1.0 or nil (the Lua-vanilla
    // predicate shape, not a boolean). The player frame's flashing zzz reads exactly this.
    g.set(
        "IsResting",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.resting))
        })?,
    )?;
    // PartialPlayTime() / NoPlayTime() → 1/nil: the two anti-addiction play-time regimes, read
    // straight off `PLAYER_FLAGS` — `0x48eb70` is `shr eax,0xc; and al,1` (bit 12) and `0x48ebe0`
    // its bit-13 neighbour (which carved both while establishing that bit 0x1000 is
    // PARTIAL_PLAY_TIME on 5875 rather than the pre-1.6.1 CAN_SELF_RESURRECT). Same Lua-vanilla
    // predicate shape as `IsResting` above: the NUMBER 1 or nil, never a boolean. Stock
    // `PlayerFrame_UpdatePlaytime` (PlayerFrame.lua:244) is the only FrameXML caller of either.
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
    // GetBillingTimeRested() → the account's rested billing MINUTES, as one number, always
    // (binding `0x48ec50`, byte-VERIFIED). The body has no `jcc` at all and ends in
    // an unconditional `mov eax,1`, and both callees are branchless and call-free — so it can never
    // return nil and never return zero values, which is why this is not the `Value::Nil` shape its
    // neighbours use. The engine applies NO conversion: `0x48ec5e` is a bare unsigned u32→double
    // push, so "minutes" is the server's convention, corroborated by stock `PlayerFrame.lua:246`
    // dividing by 60 against `REQUIRED_REST_HOURS = 5`.
    //
    // Against vmangos this reads 0, because the server hardcodes `uint32(0)` for the field
    // (`World.cpp:331`). It is fed from the wire rather than pinned to a constant so a server that
    // populates it is reported honestly. Note the reference only ever reaches this call from
    // INSIDE `if PartialPlayTime() ... elseif NoPlayTime()`, so with those bits clear stock
    // FrameXML never calls it — it has to exist for the file to load, and is off the live path.
    g.set(
        "GetBillingTimeRested",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(Value::Number(f64::from(model.billing_time_rested)))
        })?,
    )?;
    // GetTimeToWellRested() → nil, always — `0x48d4b0`, byte-VERIFIED: the whole binding is 11
    // bytes, `pushnil; return 1`. FrameXML's EXHAUST_TOOLTIP4 countdown branch is dead in 5875.
    g.set(
        "GetTimeToWellRested",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;

    // TargetUnit(unit) — select a unit by token (the reference's `TargetUnit` Lua shim → SetSelection;
    // the caller here is `PlayerFrame_OnClick`'s left-click branch → `TargetUnit("player")`, and the
    // TARGETSELF binding). Queues the raw token; the app resolves it to a streamed entity and commits
    // the selection. A nil/absent unit is ignored, as is any token the app can't resolve — the real
    // client no-ops `TargetUnit` on a unit that doesn't exist.
    g.set(
        "TargetUnit",
        lua.create_function(|lua, token: Option<String>| {
            if let Some(token) = token {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.selection_requests.push(SelectionRequest::Unit(token));
            }
            Ok(())
        })?,
    )?;

    // AssistUnit(unit) — select the *token's own* target (`0x489b80`). The ASSISTTARGET binding's
    // body is `AssistUnit("target")`, and `/assist`'s bare form is the same call.
    //
    // The shared assist tail (`0x489bb2`–`0x489c07`, byte-identical to `AssistByName`'s) is three
    // steps and no more: read `UNIT_FIELD_TARGET` off the basis (`[[obj+0x110]+0x28]`), bail
    // **silently** if it is zero, then hand the guid to the select-if-resolves helper `0x489a40`.
    // Four things it deliberately is not:
    //
    //  * **not gated by `CanAssist`** — a whole-binary census of the 25 `call 0x6066f0` sites puts
    //    none of them on this path (VERIFIED negative), so `AssistUnit("target")` on a hostile
    //    creature assists it, and that is the common case in play.
    //  * **not players-only** — unlike `AssistByName`'s typemask `0x10`, the token resolver takes
    //    whatever the token names.
    //  * **not a deselect on failure** — an unresolvable token, a basis with no target, and a
    //    target that is not streamed all leave the current selection exactly where it was
    //    (`0x489a40`'s arm 3 is a bare `ret`).
    //  * **not an attack** — the tail's swing leg is gated on the `assistAttack` CVar, whose
    //    registered default is `"0"` (VERIFIED at the registration bytes `0x48fc50`). Stock assist
    //    selects and does not swing; the leg becomes reachable only if benilla grows the CVar.
    //
    // A nil/absent unit is ignored here rather than queued, like `TargetUnit`'s: the reference
    // emits game-message `0xb8` for the token it cannot parse, and that id→string table is
    // runtime-populated BSS, not statically recoverable — the same known deviation
    // `TargetByName` already carries, and silence is better than an invented line.
    g.set(
        "AssistUnit",
        lua.create_function(|lua, token: Option<String>| {
            if let Some(token) = token {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .selection_requests
                    .push(SelectionRequest::Assist(token));
            }
            Ok(())
        })?,
    )?;

    // TargetLastEnemy() — re-select the last **attackable** unit that was targeted (`0x489b45`
    // reads the pair `[0xb4e2e8]/[0xb4e2ec]`, which `SetSelection 0x493540` stamps at `0x49377d`
    // alongside the plain last-target pair `TargetLastTarget` reads). The TARGETLASTHOSTILE
    // binding (default `G`) is its only shipped caller.
    //
    // Takes no argument and routes through the same select-if-resolves helper, so a remembered
    // guid whose unit has since streamed out is a silent no-op rather than a deselect. The app
    // owns the memory itself (`crate::target::scan`'s `LastEnemy`) — the VM holds no guids.
    g.set(
        "TargetLastEnemy",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.selection_requests.push(SelectionRequest::LastEnemy);
            Ok(())
        })?,
    )?;

    // TargetNearestFriend([reverse]) — the friendly half of the TAB scan. `0x489aa0` is
    // byte-for-byte `TargetNearestEnemy`'s shim with one changed immediate: both fetch the
    // optional Lua arg #1 as the reverse flag (`0x6f1c10`, default 0) and call the one cycler
    // `0x493f60(ecx = reverse, edx = mode)` — `edx = 1` for enemy, **`2` for friend** (3 and 4 are
    // the party and raid siblings). Everything downstream is the same code; only the
    // per-candidate filter `0x493e40` forks on the mode, and mode 2's arm (`0x493eca`) is
    // `CanAssist(player, cand)` then `UNIT_FIELD_HEALTH > 0`.
    //
    // TARGETNEARESTFRIEND (`CTRL-TAB`) calls it bare; TARGETPREVIOUSFRIEND (`CTRL-SHIFT-TAB`)
    // calls `TargetNearestFriend(1)` — 1.12's own `Bindings.xml` comments the argument
    // `-- 1 (or "true") means reverse!`, so the truthiness test below takes a number OR a boolean.
    // A numeric 0 is forward, matching `0x6f1c10`'s "absent == 0" reading.
    g.set(
        "TargetNearestFriend",
        lua.create_function(|lua, reverse: Option<Value>| {
            let reverse = match reverse {
                None | Some(Value::Nil) | Some(Value::Boolean(false)) => false,
                Some(Value::Integer(n)) => n != 0,
                Some(Value::Number(n)) => n != 0.0,
                Some(_) => true,
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.target_nearest_friend_requests.push(reverse);
            Ok(())
        })?,
    )?;

    // TargetByName(name [, exactMatch]) — select by NAME through the shared resolver `0x493aa0`
    // (`0x489d60`). The app already owns that resolver for `/target` (
    // `crate::target::by_name`); this is the binding half, which 11 corpus addons call and the
    // slash command bypassed.
    //
    // The four things this signature is not:
    //
    //  * **not exact by default** — tier 1 is a whole-string case-insensitive compare, tier 2 is
    //    the longest common PREFIX (anchored at 0, never a substring), and tier 2 is live
    //    whenever Lua arg #2 is absent or 0. `TargetByName("Rag")` selects Ragnaros.
    //  * **not nearest** — a tier-1 hit returns 0 from the walk callback and *terminates the
    //    enumeration*, so among whole-string matches the winner is the first in enumeration
    //    order, not the closest. Our resolver ranks exact-then-longest-prefix-then-nearest
    //    instead; the deviation and its cost are argued at length in `by_name`'s module header,
    //    which is the one place to change if that judgement is ever reversed.
    //  * **not players-only** — typemask 8 is UNIT, i.e. creatures *and* players (contrast
    //    `AssistByName`/`FollowByName`, which pass `0x10`). And with filter mode 0 there is no
    //    dead, reaction, attackability, range, cone or scene-attach gate, and **no
    //    self-exclusion**: `TargetByName(UnitName("player"))` self-targets.
    //  * **not silent on a miss** — the reference emits game-message `0x127` (named, nothing
    //    matched) or `0xb8` (null/empty name) and leaves the current target untouched. We keep
    //    the target untouched and say nothing: the id→string table is runtime-populated BSS that
    //    cannot be statically recovered, and inventing a line is worse than omitting one.
    //    Same known deviation the slash path already carries.
    //
    // A missing or wrong-typed first argument RAISES (`0x489d69 call 0x6f3510` → `0x489de1` →
    // `0x6f4940`, usage literal `0x842698`); a number is accepted and stringified.
    g.set(
        "TargetByName",
        lua.create_function(|lua, (name, exact): (Value, Option<Value>)| {
            let name =
                super::super::binding_abi::string_arg(lua, name, "Usage: TargetByName(\"name\")")?;
            // Arg #2 is fetched with `0x6f1c10(idx=2, default 0)` — it never raises, and a
            // numeric 0 is the same as absent. Matches `FollowByName`'s reading of its own
            // arg #2, which reaches the identical `ctx+0x0c` slot.
            let exact = match exact {
                None | Some(Value::Nil) | Some(Value::Boolean(false)) => false,
                Some(Value::Integer(n)) => n != 0,
                Some(Value::Number(n)) => n != 0.0,
                Some(_) => true,
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.target_by_name_requests.push((name, exact));
            Ok(())
        })?,
    )?;

    // DropItemOnUnit(unit) — drop the cursor's held item onto a unit (`0x48d960`). Two legs in the
    // reference: the PET leg feeds the pet, the PLAYER leg opens a trade. Queues the raw token and
    // nothing else — every gate reads state this VM does not hold, so the app owns all of them
    // (`ui_action::drop_item`).
    //
    // This binding **existed in our shipped `PetFrame_OnClick` before it existed here**: the
    // handler transcribed the reference's three legs faithfully, and the middle one called a nil
    // global, so the whole handler errored out the moment you clicked your pet holding anything.
    // That is B208's "dropping food onto the pet doesn't feed" — the reported bug was a missing
    // registration, not a missing mechanism.
    g.set(
        "DropItemOnUnit",
        lua.create_function(|lua, token: Option<String>| {
            if let Some(token) = token {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.drop_item_on_unit.push(token);
            }
            Ok(())
        })?,
    )?;

    // SpellTargetUnit(unit) — bind a unit to the spell awaiting its click. The other leg of
    // `PetFrame_OnClick`, tested BEFORE `CursorHasItem()` (ref `PetFrame.lua:114-129`), and dead
    // in our VM for the same reason `DropItemOnUnit` was.
    //
    // It is registered as an accepted **no-op**, deliberately, and that is faithful for every word
    // benilla can currently arm: the targeting cursor models the location / item / gameobject
    // seams, and a *unit* cannot satisfy any of them — the reference's
    // `BindTarget 0x6e5b40` would reject it at the same three mask tests our seams ask. The word
    // that would make this do something is the residual unit-word machine that `cast_target`'s
    // header names as still deferred (a unit-target spell never enters targeting mode here at all;
    // it resolves to `CastWireTarget::Unit` or refuses). So: present, silent, and honest — what it
    // must NOT be is absent, which is what took the handler down with it.
    g.set(
        "SpellTargetUnit",
        lua.create_function(|_, _token: Option<String>| Ok(()))?,
    )?;

    // ClearTarget() — drop the current selection (the reference API returns 1 when it cleared,
    // nil when there was nothing to clear; `ToggleGameMenu`'s ESC chain depends on the nil to
    // fall through to opening the menu). Reads the same per-token store `UnitExists("target")`
    // answers from; the app commits the actual deselect (SetSelection guid 0 + the engaged
    // attack-stop) from the drained trigger.
    g.set(
        "ClearTarget",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let had = model.unit("target").is_some_and(|u| u.exists);
            if had {
                model.target_clear = true;
            }
            Ok(flag(had))
        })?,
    )?;

    Ok(())
}
