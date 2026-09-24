//! **The 1.12 dialect** of the aura API — `GetPlayerBuff` and the five verbs that consume the cache
//! position it hands back, plus `CancelPlayerBuff`.
//!
//! The index law, the byte-verified signature table, and the traps each shape hides are in the
//! parent module's header ([`super`]); this file is the implementation. Everything here reads the
//! *same* `model.auras["player"]` list the Era verbs read — the player's list **is** the reference's
//! `0xbc6040` insertion-ordered cache — so a "cache position" needs no structure of its own.
//!
//! The one thing that is genuinely not shared with [`super`] is the **filter**: see
//! [`PlayerBuffFilter`] for why reusing the Era parser would be a silent bug rather than a tidy-up.

use mlua::{Lua, MultiValue, Value};

use super::{cancel_authorized, AuraState, Model};

/// `GetPlayerBuff`'s filter, as a **bitmask** — deliberately *not* [`Filter`].
///
/// The two parsers are not the same function and sharing one would be the silent-shape bug this
/// module's header is about. `UnitAura`'s filter defaults its *sign* to helpful and treats a bare
/// `"CANCELABLE"` as `HELPFUL|CANCELABLE`; `GetPlayerBuff`'s mask starts at **zero** the moment a
/// filter string is supplied (`xor esi,esi`, `0x4e4639`), so a bare `"CANCELABLE"` sets no sign bit
/// and matches nothing at all. And with **no** filter argument the mask is `HELPFUL|HARMFUL`
/// (`mov esi,0x3`, `0x4e4618`), where `UnitAura`'s default is helpful only.
#[derive(Clone, Copy)]
struct PlayerBuffFilter(u32);

impl PlayerBuffFilter {
    /// `stricmp` against the four `.rdata` tokens (`0x4e4661`-`0x4e46cf`). Anything else — including
    /// `"PASSIVE"` — contributes no bit, exactly as the reference's `strtok`/`stricmp` chain falls
    /// through. Delimiters are `" |"` (`0x84bc3c`): space or pipe, runs collapsed like `strtok`'s.
    fn parse(spec: Option<&str>) -> Self {
        const HELPFUL: u32 = 0x1;
        const HARMFUL: u32 = 0x2;
        const CANCELABLE: u32 = 0x10;
        const NOT_CANCELABLE: u32 = 0x20;

        let Some(spec) = spec else {
            // No filter argument at all: the preloaded `HELPFUL|HARMFUL`, both halves.
            return Self(HELPFUL | HARMFUL);
        };
        let mut mask = 0;
        for token in spec.split([' ', '|']).filter(|t| !t.is_empty()) {
            for (name, bit) in [
                ("HELPFUL", HELPFUL),
                ("HARMFUL", HARMFUL),
                ("CANCELABLE", CANCELABLE),
                ("NOT_CANCELABLE", NOT_CANCELABLE),
            ] {
                if token.eq_ignore_ascii_case(name) {
                    mask |= bit;
                }
            }
        }
        Self(mask)
    }

    /// The enumerator's per-record test (`0x4e43c9`-`0x4e43f7`): the sign bit for this record's half
    /// must be set, then `CANCELABLE`/`NOT_CANCELABLE` (if named) must agree with `record+0xa & 1`.
    fn matches(self, a: &AuraState) -> bool {
        const HELPFUL: u32 = 0x1;
        const HARMFUL: u32 = 0x2;
        const CANCELABLE: u32 = 0x10;
        const NOT_CANCELABLE: u32 = 0x20;

        let sign = if a.helpful { HELPFUL } else { HARMFUL };
        self.0 & sign != 0
            && !(self.0 & CANCELABLE != 0 && !a.cancelable)
            && !(self.0 & NOT_CANCELABLE != 0 && a.cancelable)
    }
}

/// `lua_isnumber`-then-`lua_tonumber`-then-`__ftol`, with the reference's own usage message.
///
/// Every verb in the family opens with exactly this (`0x4e45d6`, `0x4e4748`, `0x4e4808`, `0x4e48bc`,
/// `0x4e493e`, `0x4e49a8`): a non-number argument raises through `luaL_error` (`0x6f4940` =
/// `luaL_where` + `lua_pushvfstring` + `lua_concat` + `lua_error`) rather than returning nil, so an
/// addon sees the same "Usage:" line the reference prints. Lua 5.1's `lua_isnumber` accepts a numeric
/// *string*, and [`Lua::coerce_number`] is that same coercion.
fn buff_index_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i64> {
    match lua.coerce_number(v)? {
        // `__ftol` truncates toward zero; `as i64` on f64 does the same (and saturates).
        Some(n) => Ok(n as i64),
        None => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

/// The player's display cache — the reference's `0xbc6040`, which for us **is** the pushed
/// `"player"` list (`benilla::ui_aura` maintains the insertion order across frames).
/// `pos` is a physical cache position, 0-based.
///
/// Out of range is `None`, covering both of the reference's miss shapes: `0x4e4430` hands back NULL
/// for `pos < 0` and `pos >= 0x30`, and a position inside the array but past the packed prefix
/// resolves to a **cleared** record (`slot = -1`, `spellId = 0`), which every sibling then treats as
/// absent anyway — `GetPlayerBuffTexture` fails its `Spell.dbc` lookup on id 0 and pushes nil,
/// `…TimeLeft` returns 0 on `slot < 0`, `CancelPlayerBuff` finds no spell and no-ops. (One reference
/// quirk is deliberately *not* reproduced: `GetPlayerBuffApplications` on a cleared record reads
/// `+0x9`, which `BuildBuffRecord` never rewrites when it clears — so the real client hands back the
/// previous occupant's stack count. It is unreachable from any guarded call site, and stale memory is
/// not a mechanism worth mirroring; we answer with the absent-record default, `1`.)
fn player_buff_record(lua: &Lua, pos: i64) -> Option<AuraState> {
    let pos = usize::try_from(pos).ok()?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model.auras.get("player")?.get(pos).cloned()
}

/// The enumerator, `0x4e43b0`: walk the cache by ascending physical position, counting only records
/// that pass `filter`, and stop when that count reaches `index`. Returns the **position** and the
/// record — the position is what every sibling verb consumes.
///
/// A negative `index` can never match: the reference's counter starts at 0 and only increments, and
/// the hit test is an equality (`cmp ebx,[ebp-4]; je`), so it walks the whole cache and falls out
/// with `*outPos` still at its pre-seeded `-1`.
fn enumerate_player_buff(
    lua: &Lua,
    index: i64,
    filter: PlayerBuffFilter,
) -> Option<(usize, AuraState)> {
    if index < 0 {
        return None;
    }
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model
        .auras
        .get("player")?
        .iter()
        .enumerate()
        .filter(|(_, a)| filter.matches(a))
        .nth(usize::try_from(index).ok()?)
        .map(|(pos, a)| (pos, a.clone()))
}

/// Register the six 1.12 globals. Signatures are byte-verified — see [`super`]'s header table for
/// the addresses and the corpus/FrameXML site that pins each shape.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetPlayerBuff(index [, "filter"]) — 0x4e45d0. ALWAYS two numbers: the physical cache position
    // (-1 past the end) and untilCancelled (0/1). Never nil, never a different arity: both the hit
    // path (0x4e471e) and the miss path (0x4e4733) end `mov eax,0x2`.
    //
    // With the default HELPFUL|HARMFUL mask every live record passes the sign test, so the match
    // counter advances in lockstep with the position and `GetPlayerBuff(i)` returns exactly `i` over
    // the packed prefix — which `_LazyPig/LazyPig.lua:1174-1181` relies on, enumerating with
    // `counter` and then cancelling `counter` rather than the returned index.
    g.set(
        "GetPlayerBuff",
        lua.create_function(|lua, (index, filter): (Value, Option<String>)| {
            let index = buff_index_arg(lua, index, r#"Usage: GetPlayerBuff(index [, "filter"])"#)?;
            let hit = enumerate_player_buff(lua, index, PlayerBuffFilter::parse(filter.as_deref()));
            let (pos, until_cancelled) = match hit {
                Some((pos, a)) => (pos as i64, i64::from(a.until_cancelled)),
                None => (-1, 0),
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(pos),
                Value::Integer(until_cancelled),
            ]))
        })?,
    )?;

    // GetPlayerBuffTexture(buffIndex) — 0x4e4740. One value: the icon path, or nil when the position
    // is absent or the spell/icon row is missing (three separate `lua_pushnil; mov eax,0x1` exits).
    // Nil is load-bearing: `SnaFu/SnaFu.lua:317` uses this call *as its loop condition*
    // (`while GetPlayerBuffTexture(i) do`), so an empty string past the end would spin forever.
    //
    // The extra argument `CT_BuffMod/CT_BuffFrame.lua:148` passes
    // (`GetPlayerBuffTexture(buffIndex, "HELPFUL|HARMFUL")`) is simply ignored — the reference reads
    // argument 1 and nothing else, and mlua drops the surplus for the same reason.
    g.set(
        "GetPlayerBuffTexture",
        lua.create_function(|lua, index: Value| {
            let pos = buff_index_arg(lua, index, "Usage: GetPlayerBuffTexture(buffIndex)")?;
            Ok(player_buff_record(lua, pos).and_then(|a| a.icon))
        })?,
    )?;

    // GetPlayerBuffDispelType(buffIndex) — 0x4e4800. One value: SpellDispelType.dbc's name for the
    // spell's `Dispel` column, or nil (dispel id 0, or any missing row — gated at 0x4e485e before
    // the DBC is touched). The vocabulary is the DBC's, shared with `UnitDebuff`'s third return.
    //
    // Nil, never `""` and never `"none"`: the corpus indexes `DebuffTypeColor[debuffType]` behind an
    // `if ( debuffType )` and falls back to the `"none"` key itself
    // (`ElkBuffBar/ElkBuffBar.lua:324-329`, `CT_BuffMod/CT_BuffFrame.lua:137-142`). An empty string
    // is truthy in Lua, so it would take the wrong branch and then index a nil colour.
    //
    // It must also tolerate `-1` directly: `ref-BuffFrame.lua:83` calls
    // `GetPlayerBuffDispelType(GetPlayerBuff(this:GetID(), "HARMFUL"))` with no guard between them.
    g.set(
        "GetPlayerBuffDispelType",
        lua.create_function(|lua, index: Value| {
            let pos = buff_index_arg(lua, index, "Usage: GetPlayerBuffDispelType(buffIndex)")?;
            Ok(player_buff_record(lua, pos).and_then(|a| a.debuff_type))
        })?,
    )?;

    // GetPlayerBuffApplications(buffIndex) — 0x4e48b0. One value, always a number: the record's
    // stack count, and **1** for an absent position (`push 0x3ff00000; push 0x0` = the double 1.0,
    // 0x4e4917). Never nil — `CT_BuffMod/CT_BuffFrame.lua:216` and `ElkBuffBar/ElkBuffBar.lua:278`
    // both do `if ( count > 1 )` with no guard, which nil turns into a hard error.
    //
    // (The reference's usage string here is `0x84bcc0`, "Usage: GetPlayerBuffTimeLeft(buffIndex)" —
    // the same constant 0x4e4930 pushes. A copy-paste in the shipped binary; we name the verb the
    // caller actually called, since nothing can depend on the wrong name and a debugging addon
    // author would be misled by it.)
    g.set(
        "GetPlayerBuffApplications",
        lua.create_function(|lua, index: Value| {
            let pos = buff_index_arg(lua, index, "Usage: GetPlayerBuffApplications(buffIndex)")?;
            Ok(player_buff_record(lua, pos).map_or(1, |a| i64::from(a.count)))
        })?,
    )?;

    // GetPlayerBuffTimeLeft(buffIndex) — 0x4e4930 over the reader 0x4e4450. One value, always a
    // number in **seconds** (the reader returns ms and 0x4e4986 multiplies by the 0.001 at
    // 0x801608): `max(0, expiry - now)`, and **0** for an absent position, a cleared record, or an
    // aura with no expiry at all.
    //
    // Never nil: `oRA2/Participant/Buff.lua:286` computes `floor(GetPlayerBuffTimeLeft(index) + .5)`
    // with no guard whatsoever, and `PowerAuras/PowerAuras.lua:593` feeds the result of a *possibly
    // -1* `GetPlayerBuff` straight in and then compares `> 0`.
    //
    // The clock is the VM's own `GetTime()` session clock, the same one `expiration_time` is
    // expressed on — so this is a live subtraction per call, exactly as
    // `ref-BuffFrame.lua:130` re-reads it every frame from `OnUpdate` rather than caching it on the
    // event.
    g.set(
        "GetPlayerBuffTimeLeft",
        lua.create_function(|lua, index: Value| {
            let pos = buff_index_arg(lua, index, "Usage: GetPlayerBuffTimeLeft(buffIndex)")?;
            let now: f64 = lua.globals().get("__benilla_now").unwrap_or(0.0);
            // The `max(0, …)` covers both of the reference's zero cases at once: an expired aura,
            // and a permanent one, whose expiry array slot holds 0 so that `now - 0` is positive.
            Ok(player_buff_record(lua, pos).map_or(0.0, |a| (a.expiration_time - now).max(0.0)))
        })?,
    )?;

    // CancelPlayerBuff(buffIndex) — 0x4e49a0. **Zero** return values on every path, including the
    // successful one (`xor eax,eax; ret`). Resolves the position to a record, applies
    // [`cancel_authorized`], and queues the SPELL id onto the same drain `CancelUnitBuff` uses —
    // there is one queue because there is one packet: `Spell_C::CancelAura 0x6e7040` sends
    // `CMSG_CANCEL_AURA` (0x136) with a single u32 spell id (decision 0257 B8). A refused or absent
    // aura is a silent no-op, as in the reference.
    g.set(
        "CancelPlayerBuff",
        lua.create_function(|lua, index: Value| {
            let pos = buff_index_arg(lua, index, "Usage: CancelPlayerBuff(buffIndex)")?;
            if let Some(a) = player_buff_record(lua, pos).filter(cancel_authorized) {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.cancel_aura_requests.push(a.spell_id);
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{AuraState, UiScript};

    /// A plain aura as the app's feed would push it. Local to this module's fixtures: the 1.12
    /// verbs read fields (`until_cancelled`, `channeled`) the Era tests have no use for.
    fn aura(spell_id: u32, name: &str, helpful: bool, cancelable: bool) -> AuraState {
        AuraState {
            spell_id,
            name: Some(name.into()),
            icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
            count: 1,
            debuff_type: None,
            duration: 0.0,
            expiration_time: 0.0,
            helpful,
            cancelable,
            until_cancelled: false,
            channeled: false,
        }
    }

    /// The player's cache as the app pushes it: buffs and debuffs in ONE insertion-ordered list,
    /// which is what makes a cache position an absolute handle across filters.
    fn player_cache() -> Vec<AuraState> {
        let mut mark = aura(1126, "Mark of the Wild", true, true);
        mark.count = 1;
        mark.expiration_time = 100.0;
        let mut stance = aura(2457, "Battle Stance", true, false);
        stance.until_cancelled = true; // permanent: no SpellDuration row to count down
        let mut pain = aura(589, "Shadow Word: Pain", false, false);
        pain.count = 3;
        pain.debuff_type = Some("Magic".into());
        pain.expiration_time = 18.0;
        // position: 0 = Mark (buff), 1 = Stance (buff), 2 = Pain (debuff)
        vec![mark, stance, pain]
    }

    fn with_player_cache() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_auras("player", Some(player_cache()));
        s
    }

    /// **The sentinel, and the arity that carries it.** `GetPlayerBuff` returns two numbers on every
    /// path — `(-1, 0)` past the end, never nil and never a different count.
    ///
    /// The arity assertions are the half that matters, exactly as in
    /// [`super::super::item_stats`]'s `get_item_info_tests`: an implementation that returned `nil`
    /// for a miss still "works" for every individual read, and only the arity notices. Here it is
    /// worse than cosmetic — the corpus's termination test is `>= 0` on the *first* value, so nil is
    /// `attempt to compare nil with number` and zero returns is the same crash.
    #[test]
    fn get_player_buff_returns_two_numbers_and_minus_one_past_the_end() {
        let mut s = with_player_cache();

        assert_eq!(
            s.arity("GetPlayerBuff(0)").unwrap(),
            2,
            "1.12 pushes exactly two values (`mov eax,0x2` at 0x4e471e) — a hit must not be a tuple \
             of aura fields, which is the modern UnitAura shape"
        );
        assert_eq!(
            s.arity("GetPlayerBuff(99)").unwrap(),
            2,
            "and a MISS pushes two as well (0x4e4733), not zero and not nil"
        );

        // 0-based: position 0 is the first aura, not the second.
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(0)").unwrap(),
            (0, 0)
        );
        // Past the end: the -1 sentinel, and untilCancelled 0 beside it.
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(3)").unwrap(),
            (-1, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(99)").unwrap(),
            (-1, 0)
        );
        // A negative index can never match the counter, so it is a miss, not an error.
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(-1)").unwrap(),
            (-1, 0)
        );
        // untilCancelled is the NUMBER 1 — five corpus sites compare it to `1`, so a boolean
        // (`true ~= 1`) would silently invert every one of them.
        assert!(s
            .eval::<bool>("local _, uc = GetPlayerBuff(1) return uc == 1")
            .unwrap());
        assert!(s
            .eval::<bool>("local _, uc = GetPlayerBuff(0) return uc == 0")
            .unwrap());
        // An empty cache is the same miss shape as an over-long index (not an error, not nil).
        s.set_auras("player", None);
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(0)").unwrap(),
            (-1, 0)
        );
    }

    /// **The corpus's own loop.** `while GetPlayerBuff(counter) >= 0 do … end` over a real cache
    /// must visit every aura once and then *stop* — `_LazyPig/LazyPig.lua:1174`,
    /// `Zorlen/Zorlen.lua:2797`. Not terminating is the failure mode that matters: it is an infinite
    /// loop inside a `PLAYER_AURAS_CHANGED` handler, i.e. a frozen client, and no gate but this one
    /// would see it.
    ///
    /// The loop also pins LazyPig's assumption that `counter` and the returned index are
    /// interchangeable under the default filter, and that the walk covers **debuffs too** (the
    /// `HELPFUL|HARMFUL` default).
    #[test]
    fn the_corpus_while_ge_zero_loop_terminates_and_visits_every_aura() {
        let mut s = with_player_cache();

        let (visited, names) = s
            .eval::<(i64, String)>(
                r#"
                local counter = 0
                local visited = 0
                local names = ""
                while GetPlayerBuff(counter) >= 0 do
                    local index, untilCancelled = GetPlayerBuff(counter)
                    -- LazyPig's assumption: the ordinal IS the returned position, unfiltered.
                    assert(index == counter, "unfiltered enumeration must be position-identical")
                    names = names .. GetPlayerBuffTexture(index) .. ";"
                    visited = visited + 1
                    counter = counter + 1
                    assert(counter < 100, "GetPlayerBuff loop did not terminate")
                end
                return visited, names
            "#,
            )
            .unwrap();

        assert_eq!(visited, 3, "all three auras, buffs AND the debuff");
        assert_eq!(
            names,
            "Interface\\Icons\\Spell_1126;Interface\\Icons\\Spell_2457;Interface\\Icons\\Spell_589;"
        );

        // The same loop over an EMPTY cache terminates immediately rather than spinning.
        s.set_auras("player", Some(vec![]));
        assert_eq!(
            s.eval::<i64>(
                r#"local c = 0
                   while GetPlayerBuff(c) >= 0 do c = c + 1 assert(c < 100, "did not terminate") end
                   return c"#
            )
            .unwrap(),
            0
        );
    }

    /// **The default filter is `HELPFUL|HARMFUL`.** The trap that fails silently: under a `HELPFUL`
    /// default (which is what `UnitAura` uses, one function away in this file)
    /// `Zorlen/Zorlen.lua:2797-2801` — an unfiltered walk feeding `GetPlayerBuffDispelType` and
    /// matching `"Poison"`/`"Disease"` — simply never finds anything, and nothing errors.
    ///
    /// Also pins the absolute index space: a `"HARMFUL"` enumeration hands back the same position an
    /// unfiltered one does, which `CT_BuffMod/CT_BuffFrame.lua:73-79` compares for equality across
    /// the two filters.
    #[test]
    fn get_player_buff_defaults_to_both_halves_and_indexes_absolutely() {
        let s = with_player_cache();

        // No filter: three matches — the debuff included.
        assert_eq!(
            s.eval::<i64>("local n = 0 while GetPlayerBuff(n) >= 0 do n = n + 1 end return n")
                .unwrap(),
            3
        );
        // The debuff is reachable unfiltered, and its dispel class comes back.
        assert_eq!(
            s.eval::<String>("return GetPlayerBuffDispelType(GetPlayerBuff(2))")
                .unwrap(),
            "Magic"
        );
        // "HARMFUL" alone finds it at ordinal 0 — and hands back position 2, the SAME number the
        // unfiltered walk used.
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(0, "HARMFUL"))"#)
                .unwrap(),
            2
        );
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(1, "HARMFUL"))"#)
                .unwrap(),
            -1
        );
        // "HELPFUL" alone: two, at positions 0 and 1.
        assert_eq!(
            s.eval::<(i64, i64)>(
                r#"return (GetPlayerBuff(0, "HELPFUL")), (GetPlayerBuff(1, "HELPFUL"))"#
            )
            .unwrap(),
            (0, 1)
        );
        // CT_BuffMod's own literal, and the space-delimited spelling the reference's strtok accepts.
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(2, "HELPFUL|HARMFUL"))"#)
                .unwrap(),
            2
        );
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(2, "HELPFUL HARMFUL"))"#)
                .unwrap(),
            2
        );
        // The zero-mask rule: a filter STRING with no sign token matches nothing at all, because the
        // mask starts at 0 (`xor esi,esi`) rather than defaulting the sign the way UnitAura's does.
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(0, "CANCELABLE"))"#)
                .unwrap(),
            -1
        );
        // "PASSIVE" is not a token in this parser: it contributes no bit, so it is a zero mask too.
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(0, "PASSIVE"))"#)
                .unwrap(),
            -1
        );
        // CANCELABLE / NOT_CANCELABLE partition a named sign.
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(0, "HELPFUL|CANCELABLE"))"#)
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(0, "HELPFUL|NOT_CANCELABLE"))"#)
                .unwrap(),
            1
        );
    }

    /// **The accessors' empty-slot answers**, each of which is a different shape, and each of which
    /// a corpus site would crash on if it were nil.
    #[test]
    fn the_player_buff_accessors_answer_an_absent_position_without_nil_arithmetic() {
        let mut s = with_player_cache();
        s.tick(0.0); // GetTime() = 0, the clock GetPlayerBuffTimeLeft counts against

        // Every accessor returns exactly ONE value, hit or miss — the reference's `mov eax,0x1`.
        for verb in [
            "GetPlayerBuffTexture",
            "GetPlayerBuffDispelType",
            "GetPlayerBuffApplications",
            "GetPlayerBuffTimeLeft",
        ] {
            for arg in ["0", "-1", "99"] {
                assert_eq!(
                    s.arity(&format!("{verb}({arg})")).unwrap(),
                    1,
                    "{verb}({arg}) must push one value, not zero and not a tuple"
                );
            }
        }
        // CancelPlayerBuff pushes NONE (`xor eax,eax; ret`), on every path.
        for arg in ["0", "-1", "99"] {
            assert_eq!(s.arity(&format!("CancelPlayerBuff({arg})")).unwrap(), 0);
        }
        s.take_cancel_aura_requests();

        // Texture: nil past the end — SnaFu's `while GetPlayerBuffTexture(i) do` loop condition.
        assert!(s
            .eval::<bool>("return GetPlayerBuffTexture(3) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetPlayerBuffTexture(-1) == nil")
            .unwrap());
        assert_eq!(
            s.eval::<i64>(
                r#"local i = 0
                   while GetPlayerBuffTexture(i) do i = i + 1 assert(i < 100, "no nil terminator") end
                   return i"#
            )
            .unwrap(),
            3
        );

        // DispelType: nil, never "" and never "none" — the corpus branches on truthiness and then
        // indexes DebuffTypeColor with the value, so "" would take the wrong branch.
        assert!(s
            .eval::<bool>("return GetPlayerBuffDispelType(0) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetPlayerBuffDispelType(99) == nil")
            .unwrap());
        // ref-BuffFrame.lua:83 nests the calls with no guard, so -1 must pass straight through.
        assert!(s
            .eval::<bool>(r#"return GetPlayerBuffDispelType(GetPlayerBuff(9, "HARMFUL")) == nil"#)
            .unwrap());

        // Applications: a NUMBER always, and 1 (not 0, not nil) for an absent position — the
        // reference's `push 0x3ff00000`. `count > 1` is unguarded in CT_BuffMod and ElkBuffBar.
        assert_eq!(
            s.eval::<i64>("return GetPlayerBuffApplications(2)")
                .unwrap(),
            3
        );
        assert_eq!(
            s.eval::<i64>("return GetPlayerBuffApplications(99)")
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>("return GetPlayerBuffApplications(-1)")
                .unwrap(),
            1
        );
        assert!(s
            .eval::<bool>("return GetPlayerBuffApplications(-1) > 1 == false")
            .unwrap());

        // TimeLeft: a NUMBER always, in seconds, 0 for absent/permanent — oRA2 does
        // `floor(GetPlayerBuffTimeLeft(index) + .5)` with no guard at all.
        assert_eq!(
            s.eval::<f64>("return GetPlayerBuffTimeLeft(2)").unwrap(),
            18.0
        );
        assert_eq!(
            s.eval::<f64>("return GetPlayerBuffTimeLeft(1)").unwrap(),
            0.0,
            "a permanent aura has no expiry stamp: max(0, 0 - now) = 0"
        );
        assert_eq!(
            s.eval::<f64>("return GetPlayerBuffTimeLeft(99)").unwrap(),
            0.0
        );
        assert_eq!(
            s.eval::<f64>("return GetPlayerBuffTimeLeft(-1)").unwrap(),
            0.0
        );
        // The unguarded corpus arithmetic, run for real.
        assert_eq!(
            s.eval::<f64>("return math.floor(GetPlayerBuffTimeLeft(-1) + .5)")
                .unwrap(),
            0.0
        );
        assert_eq!(
            s.eval::<f64>(r#"return GetPlayerBuffTimeLeft(GetPlayerBuff(9, "HARMFUL"))"#)
                .unwrap(),
            0.0
        );

        // It counts down against the VM's GetTime clock, live, per call.
        s.tick(5.0);
        assert_eq!(
            s.eval::<f64>("return GetPlayerBuffTimeLeft(2)").unwrap(),
            13.0
        );
        // And floors at 0 rather than going negative once the aura is past its expiry.
        s.tick(20.0);
        assert_eq!(
            s.eval::<f64>("return GetPlayerBuffTimeLeft(2)").unwrap(),
            0.0
        );

        // A surplus argument is ignored, not an error — CT_BuffMod/CT_BuffFrame.lua:148 passes a
        // filter string to GetPlayerBuffTexture, which takes none.
        assert_eq!(
            s.eval::<String>(r#"return GetPlayerBuffTexture(0, "HELPFUL|HARMFUL")"#)
                .unwrap(),
            "Interface\\Icons\\Spell_1126"
        );

        // A non-number argument raises with the reference's own usage line, rather than answering.
        let err = s
            .eval::<()>("GetPlayerBuff({})")
            .expect_err("a table index must raise");
        assert!(
            format!("{err}").contains(r#"Usage: GetPlayerBuff(index [, "filter"])"#),
            "expected the reference usage message, got: {err}"
        );
    }

    /// `CancelPlayerBuff` is `CancelUnitBuff` under its 1.12 name: the same gate, the same queue,
    /// the same `CMSG_CANCEL_AURA` spell id — never a second mechanism. It differs only in how the
    /// aura is addressed (a cache position, not a token plus a filtered ordinal).
    #[test]
    fn cancel_player_buff_shares_the_gate_and_the_queue_with_cancel_unit_buff() {
        let mut s = with_player_cache();
        assert!(s.take_cancel_aura_requests().is_empty());

        // Position 0 is a cancelable buff: its SPELL id is queued, not its index.
        s.eval::<()>("CancelPlayerBuff(0)").unwrap();
        assert_eq!(s.take_cancel_aura_requests(), vec![1126]);

        // Position 1 is a buff without AFLAG_CANCELABLE, position 2 a plain debuff, and 99/-1 are
        // absent — all silent no-ops, exactly as the reference's gate falls through.
        for arg in ["1", "2", "99", "-1"] {
            s.eval::<()>(&format!("CancelPlayerBuff({arg})")).unwrap();
        }
        assert!(s.take_cancel_aura_requests().is_empty());

        // Both names reach the same queue for the same aura: CancelUnitBuff addresses it as the
        // 1st helpful aura of "player", CancelPlayerBuff as cache position 0.
        s.eval::<()>(r#"CancelUnitBuff("player", 1)"#).unwrap();
        s.eval::<()>("CancelPlayerBuff(0)").unwrap();
        assert_eq!(s.take_cancel_aura_requests(), vec![1126, 1126]);

        // The channeled arm — the only way a NEGATIVE aura is cancelable (DBC AttributesEx & 0x4,
        // 0x4e4a10). Without it the debuff below is refused, which is the shape CancelUnitBuff
        // shipped with.
        let mut channeled = aura(689, "Drain Life", false, false);
        channeled.channeled = true;
        s.set_auras("player", Some(vec![channeled]));
        s.eval::<()>("CancelPlayerBuff(0)").unwrap();
        assert_eq!(s.take_cancel_aura_requests(), vec![689]);
    }
}
