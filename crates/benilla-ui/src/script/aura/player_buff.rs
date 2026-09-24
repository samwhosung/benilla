//! The 1.12 aura verbs: `GetPlayerBuff`, which returns a cache position, and the five that take
//! one. A position is a 0-based index into the pushed `"player"` list, which is the reference's
//! insertion-ordered cache (`0xbc6040`).

use mlua::{Lua, MultiValue, Value};

use super::{cancel_authorized, AuraState, Model};

/// `GetPlayerBuff`'s filter mask, not `UnitAura`'s parser: no filter argument means
/// `HELPFUL|HARMFUL` (`0x4e4618`), and a filter string starts the mask at zero (`0x4e4639`), so a
/// bare `"CANCELABLE"` matches nothing.
#[derive(Clone, Copy)]
struct PlayerBuffFilter(u32);

impl PlayerBuffFilter {
    /// Case-insensitive match against the four tokens (`0x4e4661`-`0x4e46cf`); anything else,
    /// `"PASSIVE"` included, sets no bit. Space and pipe both delimit, runs collapsed (`0x84bc3c`).
    fn parse(spec: Option<&str>) -> Self {
        const HELPFUL: u32 = 0x1;
        const HARMFUL: u32 = 0x2;
        const CANCELABLE: u32 = 0x10;
        const NOT_CANCELABLE: u32 = 0x20;

        let Some(spec) = spec else {
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

    /// The enumerator's per-record test (`0x4e43c9`-`0x4e43f7`): the record's sign bit, then any
    /// named `CANCELABLE`/`NOT_CANCELABLE` against `record+0xa & 1`.
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

/// The family's index argument, as every verb opens (`0x4e45d6`, `0x4e4748`, `0x4e4808`,
/// `0x4e48bc`, `0x4e493e`, `0x4e49a8`): a number or numeric string, truncated; anything else raises
/// the reference's usage line through `luaL_error` (`0x6f4940`).
fn buff_index_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i64> {
    match lua.coerce_number(v)? {
        // `__ftol` truncates toward zero, as `as i64` does.
        Some(n) => Ok(n as i64),
        None => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

/// The record at cache position `pos`, or `None` past the packed prefix, where the reference has
/// NULL outside `0..0x30` (`0x4e4430`) and cleared records its siblings treat as absent. Deviation:
/// `GetPlayerBuffApplications` answers 1 for a cleared record where the reference reads its stale
/// `+0x9`, the previous occupant's count, because no guarded caller reaches it.
fn player_buff_record(lua: &Lua, pos: i64) -> Option<AuraState> {
    let pos = usize::try_from(pos).ok()?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model.auras.get("player")?.get(pos).cloned()
}

/// The enumerator (`0x4e43b0`): the `index`-th record passing `filter`, by ascending position,
/// with that position. A negative `index` never matches the counter, which counts up from 0.
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

/// Register the six 1.12 globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetPlayerBuff(index [, "filter"]) (`0x4e45d0`) returns two numbers on both paths
    // (`0x4e471e`, `0x4e4733`): the position, -1 past the end and never nil, as addons loop while
    // it is `>= 0`; and untilCancelled as the number 0 or 1, which stock `BuffFrame.lua:124`
    // compares with `1`. The index is 0-based, used without `UnitBuff`'s `dec` (`0x4e460a`,
    // `0x519579`). Unfiltered, every record passes, so `GetPlayerBuff(i)` is `i`, which addons use
    // to cancel by the counter.
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

    // GetPlayerBuffTexture(buffIndex) (`0x4e4740`): the icon path, or nil for an absent position or
    // a missing spell or icon row; addons loop until the nil. A surplus argument is ignored, as the
    // reference reads only the first.
    g.set(
        "GetPlayerBuffTexture",
        lua.create_function(|lua, index: Value| {
            let pos = buff_index_arg(lua, index, "Usage: GetPlayerBuffTexture(buffIndex)")?;
            Ok(player_buff_record(lua, pos).and_then(|a| a.icon))
        })?,
    )?;

    // GetPlayerBuffDispelType(buffIndex) (`0x4e4800`): the `SpellDispelType.dbc` name of the
    // spell's `Dispel`, or nil for dispel 0 or a missing row (`0x4e485e`); never `""`, which Lua
    // takes as true. Stock `BuffFrame.lua:83` passes it `GetPlayerBuff`'s result unguarded, -1 too.
    g.set(
        "GetPlayerBuffDispelType",
        lua.create_function(|lua, index: Value| {
            let pos = buff_index_arg(lua, index, "Usage: GetPlayerBuffDispelType(buffIndex)")?;
            Ok(player_buff_record(lua, pos).and_then(|a| a.debuff_type))
        })?,
    )?;

    // GetPlayerBuffApplications(buffIndex) (`0x4e48b0`): the stack count, and 1 for an absent
    // position (`0x4e4917`), never nil, as addons test `count > 1` unguarded. Deviation: the
    // reference's usage line names `GetPlayerBuffTimeLeft` (`0x84bcc0`, shared with `0x4e4930`);
    // ours names this verb, because nothing can depend on the wrong name.
    g.set(
        "GetPlayerBuffApplications",
        lua.create_function(|lua, index: Value| {
            let pos = buff_index_arg(lua, index, "Usage: GetPlayerBuffApplications(buffIndex)")?;
            Ok(player_buff_record(lua, pos).map_or(1, |a| i64::from(a.count)))
        })?,
    )?;

    // GetPlayerBuffTimeLeft(buffIndex) (`0x4e4930`, reader `0x4e4450`): seconds, the reader's ms
    // times the 0.001 at `0x801608` (`0x4e4986`), as `max(0, expiry - now)`, and 0 for an absent,
    // cleared or permanent aura, never nil, as addons do arithmetic on it unguarded. It is computed
    // per call against `GetTime()`, as stock `BuffFrame.lua:130` re-reads it every frame.
    g.set(
        "GetPlayerBuffTimeLeft",
        lua.create_function(|lua, index: Value| {
            let pos = buff_index_arg(lua, index, "Usage: GetPlayerBuffTimeLeft(buffIndex)")?;
            let now: f64 = lua.globals().get("__benilla_now").unwrap_or(0.0);
            // `max(0, …)` covers both zero cases: an expired aura, and a permanent one, expiry 0.
            Ok(player_buff_record(lua, pos).map_or(0.0, |a| (a.expiration_time - now).max(0.0)))
        })?,
    )?;

    // CancelPlayerBuff(buffIndex) (`0x4e49a0`): no return values on any path. It queues the spell
    // id, the one `u32` of `CMSG_CANCEL_AURA` (0x136, `Spell_C::CancelAura` `0x6e7040`), on the
    // queue `CancelUnitBuff` uses; a refused or absent aura is a silent no-op.
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

    /// The player's cache as the app pushes it: buffs and debuffs in one insertion-ordered list.
    fn player_cache() -> Vec<AuraState> {
        let mut mark = aura(1126, "Mark of the Wild", true, true);
        mark.count = 1;
        mark.expiration_time = 100.0;
        let mut stance = aura(2457, "Battle Stance", true, false);
        stance.until_cancelled = true; // permanent
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

    /// The arity is the point: a nil miss passes each single read but breaks every `>= 0` loop.
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

        // 0-based: position 0 is the first aura.
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(0)").unwrap(),
            (0, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(3)").unwrap(),
            (-1, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(99)").unwrap(),
            (-1, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(-1)").unwrap(),
            (-1, 0)
        );
        // untilCancelled is the number 1, not `true`: callers compare it with `1`.
        assert!(s
            .eval::<bool>("local _, uc = GetPlayerBuff(1) return uc == 1")
            .unwrap());
        assert!(s
            .eval::<bool>("local _, uc = GetPlayerBuff(0) return uc == 0")
            .unwrap());
        s.set_auras("player", None);
        assert_eq!(
            s.eval::<(i64, i64)>("return GetPlayerBuff(0)").unwrap(),
            (-1, 0)
        );
    }

    /// The corpus's loop (`_LazyPig/LazyPig.lua:1174`) visits every aura, debuffs included, and
    /// ends: a hang there freezes the client inside a `PLAYER_AURAS_CHANGED` handler.
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

    /// Under a `HELPFUL` default an unfiltered walk for `"Poison"` would find nothing, silently;
    /// and a position is the same number under every filter, which callers compare.
    #[test]
    fn get_player_buff_defaults_to_both_halves_and_indexes_absolutely() {
        let s = with_player_cache();

        assert_eq!(
            s.eval::<i64>("local n = 0 while GetPlayerBuff(n) >= 0 do n = n + 1 end return n")
                .unwrap(),
            3
        );
        assert_eq!(
            s.eval::<String>("return GetPlayerBuffDispelType(GetPlayerBuff(2))")
                .unwrap(),
            "Magic"
        );
        // "HARMFUL" finds the debuff at ordinal 0 and returns position 2, the unfiltered number.
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
        assert_eq!(
            s.eval::<(i64, i64)>(
                r#"return (GetPlayerBuff(0, "HELPFUL")), (GetPlayerBuff(1, "HELPFUL"))"#
            )
            .unwrap(),
            (0, 1)
        );
        // Pipe- and space-delimited.
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
        // A filter string with no sign token matches nothing: the mask starts at 0.
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(0, "CANCELABLE"))"#)
                .unwrap(),
            -1
        );
        // "PASSIVE" is not a token.
        assert_eq!(
            s.eval::<i64>(r#"return (GetPlayerBuff(0, "PASSIVE"))"#)
                .unwrap(),
            -1
        );
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

    #[test]
    fn the_player_buff_accessors_answer_an_absent_position_without_nil_arithmetic() {
        let mut s = with_player_cache();
        s.tick(0.0); // GetTime() = 0

        // One value, hit or miss.
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
        for arg in ["0", "-1", "99"] {
            assert_eq!(s.arity(&format!("CancelPlayerBuff({arg})")).unwrap(), 0);
        }
        s.take_cancel_aura_requests();

        // Texture: nil past the end, a loop condition in the corpus.
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

        // DispelType: nil, never "".
        assert!(s
            .eval::<bool>("return GetPlayerBuffDispelType(0) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetPlayerBuffDispelType(99) == nil")
            .unwrap());
        // Stock BuffFrame.lua:83 nests the calls unguarded.
        assert!(s
            .eval::<bool>(r#"return GetPlayerBuffDispelType(GetPlayerBuff(9, "HARMFUL")) == nil"#)
            .unwrap());

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
        // The corpus's unguarded arithmetic.
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

        // It counts down against GetTime, per call, and floors at 0.
        s.tick(5.0);
        assert_eq!(
            s.eval::<f64>("return GetPlayerBuffTimeLeft(2)").unwrap(),
            13.0
        );
        s.tick(20.0);
        assert_eq!(
            s.eval::<f64>("return GetPlayerBuffTimeLeft(2)").unwrap(),
            0.0
        );

        // A surplus argument is ignored.
        assert_eq!(
            s.eval::<String>(r#"return GetPlayerBuffTexture(0, "HELPFUL|HARMFUL")"#)
                .unwrap(),
            "Interface\\Icons\\Spell_1126"
        );

        // A non-number argument raises the reference's usage line.
        let err = s
            .eval::<()>("GetPlayerBuff({})")
            .expect_err("a table index must raise");
        assert!(
            format!("{err}").contains(r#"Usage: GetPlayerBuff(index [, "filter"])"#),
            "expected the reference usage message, got: {err}"
        );
    }

    #[test]
    fn cancel_player_buff_shares_the_gate_and_the_queue_with_cancel_unit_buff() {
        let mut s = with_player_cache();
        assert!(s.take_cancel_aura_requests().is_empty());

        // Position 0 is cancelable: its spell id queues.
        s.eval::<()>("CancelPlayerBuff(0)").unwrap();
        assert_eq!(s.take_cancel_aura_requests(), vec![1126]);

        // Not cancelable, a plain debuff, absent: all silent no-ops.
        for arg in ["1", "2", "99", "-1"] {
            s.eval::<()>(&format!("CancelPlayerBuff({arg})")).unwrap();
        }
        assert!(s.take_cancel_aura_requests().is_empty());

        // Both names reach one queue: the first helpful aura is cache position 0.
        s.eval::<()>(r#"CancelUnitBuff("player", 1)"#).unwrap();
        s.eval::<()>("CancelPlayerBuff(0)").unwrap();
        assert_eq!(s.take_cancel_aura_requests(), vec![1126, 1126]);

        // The channeled arm (`AttributesEx & 0x4`, `0x4e4a10`) cancels a negative aura.
        let mut channeled = aura(689, "Drain Life", false, false);
        channeled.channeled = true;
        s.set_auras("player", Some(vec![channeled]));
        s.eval::<()>("CancelPlayerBuff(0)").unwrap();
        assert_eq!(s.take_cancel_aura_requests(), vec![689]);
    }
}
