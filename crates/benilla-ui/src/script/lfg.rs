//! The looking-for-group result globals of `PartyFrame.cpp` (`0x4e9760`..`0x4e9bd1`; the table
//! `0x84c170`) that the 1.12 interface never calls. `SetLookingForGroup` and `GetLookingForGroup`
//! are [`super::social`]'s.
//!
//! The client keeps its result list (50 records, `0xbc6fd8`) from opcode `0x1ff`
//! ([`super::UiScript::lfg_results`]). The dungeon lists are built: `LFGDungeons.dbc` grouped
//! by type (`0x4e7c70`, pushed by the app as [`LfgDungeon`]), counted for the player's level and
//! side (`0x4e89c0`) and sorted so the rows that fit the level come first (`0x4e8b20`); the quest
//! list (type 3) is the quest log's ids, cached titles first (`0x4e8be0`, `0x4e8c80`). A type's
//! entries are the first `count` of its sorted list, so a row of the wrong side can show where a
//! right one is cut, as in the reference. Each recount and rebuild signals `UPDATE_LFG_TYPES`
//! (`0x4e8b0e`, `0x4e8c70`): the app fires it at a level change and at each quest slot that moved
//! (`field_watch` in benilla-app), and the world-enter cascade twice
//! ([`super::UiScript::fire_world_enter_rebuilds`]).

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{coerced_number, ftol, number_arg, tuple};
use super::social::SocialCall;
use super::Model;

/// The five literal type names `GetLFGTypes` pushes from `0x84c158` (types 1 to 5).
const LFG_TYPES: [&str; 5] = ["Dungeon", "Raid", "Quest", "Zone", "Battleground"];

/// One `LFGDungeons.dbc` row, in file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfgDungeon {
    /// The row id the store is indexed by (`0xc0db34`).
    pub id: u32,
    pub name: String,
    pub min_level: u32,
    pub max_level: u32,
    /// The list it joins, 0 to 5.
    pub kind: u32,
    /// 0 Horde, 1 Alliance, negative either.
    pub side: i32,
}

/// The records the list holds (`0x4e7cdf`, `0x4e8e60`).
const LFG_KEPT: u32 = 50;

/// One stored result record (`0xc4` bytes at `0xbc6fd8`), the class resolved by the app. A record
/// no reply has written reads as zeroes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LfgResult {
    name: String,
    level: u32,
    /// The `ChrClasses` name, empty for none (`0x4e9977`).
    class: String,
    /// The slot words: the type in the top byte, the entry in the low 24 bits.
    slots: [u32; 3],
    comment: String,
}

/// A reply's record as `0x4e8dc0` writes it over the stored one: a `None` its read fell short
/// of, which leaves the stored value (`0x418e44`). The strings always write, `""` when short.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgResultRead {
    pub name: String,
    pub level: Option<u32>,
    /// The `ChrClasses` name of the id read, empty for an id with no row (`0x4e9977`).
    pub class: Option<String>,
    /// The slot words: the type in the top byte, the entry in the low 24 bits.
    pub slots: [Option<u32>; 3],
    pub comment: String,
}

/// `MSG_LOOKING_FOR_GROUP`'s reply as its handler reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgReply {
    /// The count read, at most 50; `None` when the reply was short of it, which keeps the stored
    /// count (`0x418eb0` writes nothing).
    pub count: Option<u32>,
    /// The server's total; `None` keeps the stored one.
    pub total: Option<u32>,
    /// The records read. Each record past them, up to the count, reads a poisoned store: name and
    /// comment `""`, its numbers and slots unwritten.
    pub records: Vec<LfgResultRead>,
}

/// The result list and the quest-title asks it waits for (`0xbc6fd8`, `0xbc7618`, `0xbc761c`,
/// `0xbc7620`): engine state that only the reply's handler writes and `InitializeGame`'s party
/// init (`0x4e7c70`, from `0x48f59a`) re-makes, which a `/reload` does not run, so a reload's VM
/// takes it over ([`super::UiScript::take_lfg_store`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgStore {
    /// `0xbc7618`, at most 50.
    count: u32,
    /// `0xbc761c`.
    total: u32,
    /// The records, as many as a reply has reached.
    records: Vec<LfgResult>,
    /// `0xbc7620`: the title queries outstanding.
    pending: u32,
    /// The callbacks (`0x4e8f60`) kept per quest, one per slot that missed it.
    waits: HashMap<u32, u32>,
    /// The quests the last reply missed, asked inside its call.
    asks: Vec<u32>,
}

impl super::UiScript {
    /// Push the dungeon table; without it every list is empty.
    pub fn set_lfg_dungeons(&mut self, rows: Vec<LfgDungeon>) {
        self.model_mut().lfg_dungeons = rows;
    }

    /// `MSG_LOOKING_FOR_GROUP`'s reply (`0x4e8dc0`). With titles outstanding the old records'
    /// callbacks go (`0x563280`, which drops every `0x4e8f60` on the quest) and the counter is
    /// zeroed (`0x4e8e3a`). The reply is read over the stored list, a short read leaving what it
    /// missed ([`LfgReply`]). Each type 3 slot, as it then stands, asks the quest cache for its
    /// title with the callback `0x4e8f60` (`0x4e8eef`) and counts a miss (`0x4e8efb`); a zero
    /// entry is a miss the cache never answers (`0x562a52`), which holds the signal until the next
    /// reply. With none outstanding `UPDATE_LFG_LIST` fires (`0x4e8f37`-`0x4e8f45`).
    pub fn lfg_results(&mut self, reply: LfgReply) {
        let pending = {
            let mut model = self.model_mut();
            let model = &mut *model;
            let quests = &model.quest_store.records;
            let LfgStore {
                count,
                total,
                records,
                pending,
                waits,
                asks,
            } = &mut model.lfg_store;
            waits.clear();
            *pending = 0;
            if let Some(read) = reply.count {
                *count = read.min(LFG_KEPT);
            }
            if let Some(read) = reply.total {
                *total = read;
            }
            let kept = *count as usize;
            if records.len() < kept {
                records.resize_with(kept, LfgResult::default);
            }
            let poisoned = LfgResultRead::default();
            for (i, record) in records.iter_mut().take(kept).enumerate() {
                let read = reply.records.get(i).unwrap_or(&poisoned);
                record.name.clone_from(&read.name);
                if let Some(level) = read.level {
                    record.level = level;
                }
                if let Some(class) = &read.class {
                    record.class.clone_from(class);
                }
                for (slot, word) in record.slots.iter_mut().zip(read.slots) {
                    if let Some(word) = word {
                        *slot = word;
                    }
                    // `0x4e8ec1`: the byte compare on the slot as it stands.
                    let id = *slot & 0xff_ffff;
                    if *slot >> 24 == 3 && !quests.contains_key(&id) {
                        *pending += 1;
                        if id != 0 {
                            *waits.entry(id).or_default() += 1;
                            asks.push(id);
                        }
                    }
                }
                record.comment.clone_from(&read.comment);
            }
            *pending
        };
        ask_titles(&self.lua);
        if pending == 0 {
            self.fire_event("UPDATE_LFG_LIST", Vec::new());
        }
    }

    /// The quests a reply missed, for a harness with no host to ask.
    pub fn take_lfg_title_asks(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().lfg_store.asks)
    }

    /// The list out of a dying VM, for a `/reload`'s ([`Self::seed_lfg_store`]).
    pub fn take_lfg_store(&mut self) -> LfgStore {
        std::mem::take(&mut self.model_mut().lfg_store)
    }

    /// Seat a new VM's list before its load runs a file.
    pub fn seed_lfg_store(&mut self, store: LfgStore) {
        self.model_mut().lfg_store = store;
    }

    /// Quest records landed or refused: each kept callback (`0x4e8f60` into `0x4e8d30`) counts one
    /// title down, status unread, and the last signals `UPDATE_LFG_LIST`.
    pub(super) fn lfg_titles_landed(&mut self, ids: &[u32]) {
        let calls: u32 = {
            let mut model = self.model_mut();
            ids.iter()
                .map(|id| model.lfg_store.waits.remove(id).unwrap_or(0))
                .sum()
        };
        for _ in 0..calls {
            let signal = {
                let mut model = self.model_mut();
                let lfg = &mut model.lfg_store;
                // `0x4e8d30`: a counter at 0 signals too (`0x4e8d37`).
                if lfg.pending != 0 {
                    lfg.pending -= 1;
                }
                lfg.pending == 0
            };
            if signal {
                self.fire_event("UPDATE_LFG_LIST", Vec::new());
            }
        }
    }
}

/// The titles a reply missed, asked inside its call with the host lent (`0x562a40`).
fn ask_titles(lua: &Lua) {
    if !super::host::open(lua) {
        return;
    }
    let asks = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        std::mem::take(&mut model.lfg_store.asks)
    };
    for id in asks {
        let _ = super::calls::ask(lua, super::RecordAsk::QuestRecord(id));
    }
}

/// A slot word as `GetLFGResults` pushes it (`0x4e9990`-`0x4e9a03`): nil for type 0, the cached
/// quest title for type 3, else the dungeon row's name; a miss is `""`.
fn slot_text(model: &Model, word: u32) -> Option<String> {
    let kind = (word >> 24) as u8 as i8;
    if kind == 0 {
        return None;
    }
    let id = word & 0xff_ffff;
    Some(if kind == 3 {
        model
            .quest_store
            .records
            .get(&id)
            .map(|r| r.title.clone())
            .unwrap_or_default()
    } else {
        model
            .lfg_dungeons
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.name.clone())
            .unwrap_or_default()
    })
}

/// A type's displayed entries and their count (`0xbc6f18[type]`): the first `count` names of the
/// sorted list, which are all there is for the quest list, whose count is its length.
fn type_list(lua: &Lua, model: &Model, kind: usize) -> (usize, Vec<String>) {
    if kind == 3 {
        // Every nonzero quest id of the log's 20 slots (`0x4e8c1e`-`0x4e8c32`), cached titles
        // first, by title (`0x4e8c80`), the count stored at `0x4e8c5b`. The reference's list is a
        // snapshot `0x4e8be0` rebuilds when the slot watcher runs (`0x5dde98`); this reads the
        // slots off the host and the records at the call, so a record landing since re-sorts it.
        let store = &model.quest_store;
        let slots = super::quest_live::slots(lua, model);
        let mut titles: Vec<&str> = slots
            .iter()
            .filter(|s| s.quest_id != 0)
            .map(|s| {
                store
                    .records
                    .get(&s.quest_id)
                    .map_or("", |r| r.title.as_str())
            })
            .collect();
        let count = titles.len();
        titles.sort_by(|a, b| match (a.is_empty(), b.is_empty()) {
            (false, false) => icase(a, b),
            (a_null, b_null) => a_null.cmp(&b_null),
        });
        return (count, titles.into_iter().map(str::to_string).collect());
    }
    // The recount needs the active player (`0x4e89cc`), read as the call runs.
    let Some(player) = super::unit::view(lua, model, "player").filter(|p| p.level > 0) else {
        return (0, Vec::new());
    };
    let level = player.level;
    // The player's side from its faction template's group mask: Horde 0, Alliance 1, none -1.
    let side = match player.faction_group.as_deref() {
        Some("Horde") => 0,
        Some("Alliance") => 1,
        _ => -1,
    };
    let in_level = |d: &LfgDungeon| level >= d.min_level && level <= d.max_level;
    let mut rows: Vec<&LfgDungeon> = model
        .lfg_dungeons
        .iter()
        .filter(|d| d.kind as usize == kind)
        .collect();
    let count = rows
        .iter()
        .filter(|d| in_level(d) && (d.side < 0 || d.side == side))
        .count();
    // `0x4e8b20`: rows that fit the level first, then by name.
    rows.sort_by(|a, b| {
        in_level(b)
            .cmp(&in_level(a))
            .then_with(|| icase(&a.name, &b.name))
    });
    (count, rows.into_iter().map(|d| d.name.clone()).collect())
}

/// `0x64a4c0`, the case-folding compare.
fn icase(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
}

/// Register the looking-for-group result globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // LFGQuery(type [, entry, class]) (`0x4e9760`): `is-number` on the first argument or the
    // usage error. A type below 6 and an entry below that type's count (both unsigned, the entry
    // read by `lua_tonumber` with no gate) send `MSG_LOOKING_FOR_GROUP` with `(type << 24) & entry`
    // and the class (0 unless a number).
    g.set(
        "LFGQuery",
        lua.create_function(|lua, args: mlua::Variadic<Value>| {
            let kind = number_arg(
                lua,
                args.first().cloned().unwrap_or(Value::Nil),
                "Usage: LFGQuery(param1 [, class])",
            )? as u32;
            let entry = ftol(coerced_number(lua, args.get(1).cloned())) as u32;
            let class = match args.get(2).cloned() {
                Some(v) => number_arg(lua, v, "").unwrap_or(0) as u32,
                None => 0,
            };
            let inside = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                kind < 6 && (entry as usize) < type_list(lua, &model, kind as usize).0
            };
            if !inside {
                return Ok(());
            }
            let criterion = kind.wrapping_shl(24) & entry;
            super::calls::submit(
                lua,
                super::ScriptCall::Social(SocialCall::LfgQuery { criterion, class }),
            )
        })?,
    )?;

    // GetNumLFGResults() (`0x4e9870`): the stored count and the server's total, both unsigned
    // (`fild qword` over a zero high dword, `0x4e9886`, `0x4e98a6`).
    g.set(
        "GetNumLFGResults",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let lfg = &model.lfg_store;
            Ok(tuple(&[
                Some(i64::from(lfg.count)),
                Some(i64::from(lfg.total)),
            ]))
        })?,
    )?;

    // GetLFGResults(index) (`0x4e98c0`): a number or the usage error; the record's name, level,
    // class name, three slots and comment (`0x4e9936`-`0x4e9a25`); an index past the count (the
    // compare is unsigned, `0x4e991e`) the seven-value miss `nil, 0, nil, nil, nil, nil, nil`
    // (`0x4e9a33`).
    g.set(
        "GetLFGResults",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetLFGResults(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let lfg = &model.lfg_store;
            let slot = (index as u32).wrapping_sub(1);
            let Some(record) = (slot < lfg.count)
                .then(|| lfg.records.get(slot as usize))
                .flatten()
            else {
                return Ok(tuple(&[None, Some(0), None, None, None, None, None]));
            };
            let mut out = vec![
                Value::String(lua.create_string(&record.name)?),
                Value::Integer(i64::from(record.level as i32)),
                Value::String(lua.create_string(&record.class)?),
            ];
            for word in record.slots {
                out.push(match slot_text(&model, word) {
                    Some(text) => Value::String(lua.create_string(&text)?),
                    None => Value::Nil,
                });
            }
            out.push(Value::String(lua.create_string(&record.comment)?));
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetLFGTypes() (`0x4e9a70`): the five names.
    g.set(
        "GetLFGTypes",
        lua.create_function(|lua, ()| {
            let names = LFG_TYPES
                .iter()
                .map(|n| lua.create_string(n).map(Value::String))
                .collect::<mlua::Result<Vec<_>>>()?;
            Ok(MultiValue::from_vec(names))
        })?,
    )?;

    // GetLFGTypeEntries(type) (`0x4e9aa0`): a number or the usage error; a type of 6 or more or an
    // empty list returns nothing, else one name per entry (`0x4e9b50`..`0x4e9bc6`) and their count.
    g.set(
        "GetLFGTypeEntries",
        lua.create_function(|lua, kind: Value| {
            let kind = number_arg(lua, kind, "Usage: GetLFGTypeEntries(type)")? as u32;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            if kind >= 6 {
                return Ok(MultiValue::new());
            }
            let (count, names) = type_list(lua, &model, kind as usize);
            let out = names
                .iter()
                .take(count)
                .map(|n| lua.create_string(n).map(Value::String))
                .collect::<mlua::Result<Vec<_>>>()?;
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    fn raises(s: &UiScript, call: &str, text: &str) {
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(text), "{call}: {err}");
    }

    #[test]
    fn the_empty_results_answer_the_references_shapes() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetNumLFGResults()").unwrap(), 2);
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumLFGResults()").unwrap(),
            (0, 0)
        );
        // Seven values, the second the number 0, the rest nil, trailing ones kept.
        assert_eq!(s.arity("GetLFGResults(1)").unwrap(), 7);
        assert_eq!(
            s.eval::<(Option<String>, i64, Option<String>)>(
                "local a, b, c = GetLFGResults(1) return a, b, c"
            )
            .unwrap(),
            (None, 0, None)
        );
        assert_eq!(s.arity("GetLFGResults('9')").unwrap(), 7);
        // No entries in any list, whatever the type, in range or not.
        for kind in ["0", "1", "3", "5", "6", "-1", "99"] {
            assert_eq!(
                s.arity(&format!("GetLFGTypeEntries({kind})")).unwrap(),
                0,
                "{kind}"
            );
        }
        // The query sends nothing and returns nothing.
        for call in [
            "LFGQuery(1, 0)",
            "LFGQuery(3)",
            "LFGQuery(9, 9, 9)",
            "LFGQuery('2', 1, 1)",
        ] {
            assert_eq!(s.arity(call).unwrap(), 0, "{call}");
        }
    }

    fn listen(s: &UiScript) {
        s.run(
            r#"LIST = 0
            local f = CreateFrame("Frame")
            f:RegisterEvent("UPDATE_LFG_LIST")
            f:SetScript("OnEvent", function() LIST = LIST + 1 end)"#,
        )
        .unwrap();
    }

    fn result(name: &str, slots: [u32; 3]) -> crate::script::LfgResultRead {
        crate::script::LfgResultRead {
            name: name.into(),
            level: Some(60),
            class: Some("Mage".into()),
            slots: slots.map(Some),
            comment: "LF Strat".into(),
        }
    }

    fn reply(total: u32, records: Vec<crate::script::LfgResultRead>) -> crate::script::LfgReply {
        crate::script::LfgReply {
            count: Some(records.len() as u32),
            total: Some(total),
            records,
        }
    }

    fn quest_record(title: &str) -> crate::script::QuestRecordView {
        crate::script::QuestRecordView {
            title: title.into(),
            ..Default::default()
        }
    }

    fn listed(s: &UiScript) -> i64 {
        s.eval::<i64>("return LIST").unwrap()
    }

    /// `0x4e8dc0`: a reply with no title outstanding signals at once, a count of 0 included, and
    /// the getters read the stored records (`0x4e9870`, `0x4e98c0`): the dungeon slot by its row
    /// id, a type 0 slot as nil, an unknown row as `""`, the total unsigned.
    #[test]
    fn a_reply_with_no_quest_title_outstanding_signals_and_the_getters_read_it() {
        let mut s = UiScript::new().unwrap();
        listen(&s);
        let mut row = dungeon("Wailing Caverns", 15, 28, 1, -1);
        row.id = 2;
        s.set_lfg_dungeons(vec![row]);
        s.lfg_results(reply(0, Vec::new()));
        assert_eq!(listed(&s), 1, "a count of 0");
        s.lfg_results(reply(
            0xffff_fffe,
            vec![result("Aria", [0x0100_0002, 0x0100_0004, 0])],
        ));
        assert_eq!(listed(&s), 2);
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumLFGResults()").unwrap(),
            (1, 0xffff_fffe)
        );
        assert_eq!(
            s.eval::<String>(
                "local a, b, c, d, e, f, g = GetLFGResults(1) \
                 return a .. b .. c .. d .. '|' .. e .. '|' .. tostring(f) .. '|' .. g"
            )
            .unwrap(),
            "Aria60MageWailing Caverns||nil|LF Strat"
        );
        assert_eq!(s.arity("GetLFGResults(1)").unwrap(), 7);
        // Past the list, and index 0 (unsigned), give the miss.
        for index in [2, 0] {
            assert_eq!(
                s.eval::<(Option<String>, i64)>(&format!("return GetLFGResults({index})"))
                    .unwrap(),
                (None, 0)
            );
        }
    }

    /// A type 3 slot whose quest is uncached asks the cache and holds the signal until its record
    /// lands (`0x4e8efb`, `0x4e8d30`); a new reply drops the old callbacks (`0x563280`).
    #[test]
    fn a_missed_quest_title_holds_the_signal_until_it_lands() {
        let mut s = UiScript::new().unwrap();
        listen(&s);
        let quest = |id: u32| 0x0300_0000 | id;
        s.lfg_results(reply(
            2,
            vec![result("A", [quest(11), quest(12), quest(11)])],
        ));
        assert_eq!(listed(&s), 0, "held");
        assert_eq!(s.take_lfg_title_asks(), vec![11, 12, 11]);
        s.land_quest_records(vec![(11, quest_record("Wolves"))]);
        assert_eq!(listed(&s), 0, "one title still out");
        s.refuse_quest_record(12);
        assert_eq!(listed(&s), 1, "the last answer, a refusal, counts too");
        assert_eq!(
            s.eval::<String>("local _, _, _, a, b = GetLFGResults(1) return a .. '/' .. b")
                .unwrap(),
            "Wolves/"
        );
        // A cached title waits on nothing; a newer reply forgets the callbacks of the old.
        s.lfg_results(reply(1, vec![result("B", [quest(13), 0, 0])]));
        s.lfg_results(reply(1, vec![result("C", [quest(11), 0, 0])]));
        assert_eq!(listed(&s), 2, "11 is cached");
        s.land_quest_records(vec![(13, quest_record("Late"))]);
        assert_eq!(listed(&s), 2, "13's was dropped");
    }

    /// A type 3 slot with entry 0 counts a title the quest cache never asks for, a zero key
    /// returning at once (`0x562a52`), so the list goes unsignalled until the next reply.
    #[test]
    fn a_zero_quest_entry_holds_the_signal_until_the_next_reply() {
        let mut s = UiScript::new().unwrap();
        listen(&s);
        s.lfg_results(reply(1, vec![result("A", [0x0300_0000, 0, 0])]));
        assert_eq!(listed(&s), 0);
        assert!(s.take_lfg_title_asks().is_empty(), "nothing asked");
        s.lfg_results(reply(0, Vec::new()));
        assert_eq!(listed(&s), 1);
    }

    /// The reply is read over the stored records (`0x418eb0`, `0x418e30`, `0x4191b0`): with no
    /// header the count and total stand, and every record under the count reads a poisoned
    /// store, its name and comment emptied and its numbers and slots kept; a cut record keeps the
    /// dwords it missed. Either way the list signals.
    #[test]
    fn a_short_reply_writes_over_the_stored_list() {
        let mut s = UiScript::new().unwrap();
        listen(&s);
        let mut row = dungeon("Wailing Caverns", 15, 28, 1, -1);
        row.id = 2;
        s.set_lfg_dungeons(vec![row]);
        let row_text = "local a, b, c, d, _, _, g = GetLFGResults(1) \
                        return a .. '|' .. b .. '|' .. c .. '|' .. d .. '|' .. g";
        s.lfg_results(reply(
            7,
            vec![result("Aria", [0x0100_0002, 0, 0]), result("Bo", [0; 3])],
        ));
        s.lfg_results(crate::script::LfgReply::default());
        assert_eq!(listed(&s), 2, "a short reply signals");
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumLFGResults()").unwrap(),
            (2, 7)
        );
        assert_eq!(
            s.eval::<String>(row_text).unwrap(),
            "|60|Mage|Wailing Caverns|"
        );
        // A record cut after its level: the rest is the old record's.
        s.lfg_results(crate::script::LfgReply {
            count: Some(1),
            total: Some(1),
            records: vec![crate::script::LfgResultRead {
                name: "Cid".into(),
                level: Some(10),
                ..Default::default()
            }],
        });
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumLFGResults()").unwrap(),
            (1, 1)
        );
        assert_eq!(
            s.eval::<String>(row_text).unwrap(),
            "Cid|10|Mage|Wailing Caverns|"
        );
    }

    /// The list is engine state a `/reload` keeps: a VM seated with the dying one's store reads
    /// its records and hears the title it waited on (`0x4e8d30`).
    #[test]
    fn a_seated_vm_reads_the_list_and_its_waits() {
        let mut old = UiScript::new().unwrap();
        old.lfg_results(reply(5, vec![result("A", [0x0300_000b, 0, 0])]));
        let store = old.take_lfg_store();
        let mut s = UiScript::new().unwrap();
        s.seed_lfg_store(store);
        listen(&s);
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumLFGResults()").unwrap(),
            (1, 5)
        );
        s.land_quest_records(vec![(11, quest_record("Wolves"))]);
        assert_eq!(listed(&s), 1);
        assert_eq!(
            s.eval::<String>("local _, _, _, a = GetLFGResults(1) return a")
                .unwrap(),
            "Wolves"
        );
    }

    #[test]
    fn the_five_type_names_are_the_exes_literals() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetLFGTypes()").unwrap(), 5);
        let names: Vec<String> = s
            .eval("return { GetLFGTypes() }")
            .map(|t: std::collections::HashMap<i64, String>| {
                let mut v: Vec<_> = t.into_iter().collect();
                v.sort();
                v.into_iter().map(|(_, n)| n).collect()
            })
            .unwrap();
        assert_eq!(names, ["Dungeon", "Raid", "Quest", "Zone", "Battleground"]);
    }

    #[test]
    fn a_missing_or_non_number_argument_raises_the_usage_text() {
        let s = UiScript::new().unwrap();
        for call in [
            "LFGQuery()",
            "LFGQuery(nil, 1)",
            "LFGQuery('x')",
            "LFGQuery({})",
        ] {
            raises(&s, call, "Usage: LFGQuery(param1 [, class])");
        }
        for call in [
            "GetLFGResults()",
            "GetLFGResults(nil)",
            "GetLFGResults('x')",
        ] {
            raises(&s, call, "Usage: GetLFGResults(index)");
        }
        for call in [
            "GetLFGTypeEntries()",
            "GetLFGTypeEntries(nil)",
            "GetLFGTypeEntries({})",
        ] {
            raises(&s, call, "Usage: GetLFGTypeEntries(type)");
        }
    }

    fn dungeon(name: &str, min: u32, max: u32, kind: u32, side: i32) -> crate::script::LfgDungeon {
        crate::script::LfgDungeon {
            id: 0,
            name: name.into(),
            min_level: min,
            max_level: max,
            kind,
            side,
        }
    }

    fn table() -> Vec<crate::script::LfgDungeon> {
        vec![
            dungeon("Wailing Caverns", 15, 28, 1, -1),
            dungeon("Ragefire Chasm", 13, 22, 1, 0),
            dungeon("Deadmines", 15, 28, 1, -1),
            dungeon("Stormwind Stockades", 22, 34, 1, 1),
            dungeon("Molten Core", 60, 60, 2, -1),
            dungeon("Elwynn Forest", 1, 14, 4, 1),
        ]
    }

    fn player(level: u32, side: Option<&str>) -> crate::script::UnitState {
        crate::script::UnitState {
            exists: true,
            level,
            faction_group: side.map(str::to_string),
            ..Default::default()
        }
    }

    fn names(s: &UiScript, kind: i64) -> Vec<String> {
        s.eval::<mlua::Variadic<String>>(&format!("return GetLFGTypeEntries({kind})"))
            .unwrap()
            .to_vec()
    }

    /// `0x4e89c0` counts the rows that fit the player's level and side, `0x4e8b20` sorts the fitting
    /// rows first, by name, and `GetLFGTypeEntries` returns the first `count` of them.
    #[test]
    fn a_types_entries_are_the_first_count_of_the_level_sorted_list() {
        let mut s = UiScript::new().unwrap();
        s.set_lfg_dungeons(table());
        // No player yet: the counts stay 0.
        assert!(names(&s, 1).is_empty());
        // A level 20 human (Alliance): Ragefire (Horde) is out of the count, though its level fits,
        // so the cut keeps Deadmines and Wailing Caverns and leaves the sorted list's third row,
        // Ragefire Chasm, off the end.
        s.set_unit("player", Some(player(20, Some("Alliance"))));
        assert_eq!(names(&s, 1), ["Deadmines", "Ragefire Chasm"]);
        // The same level for a Horde player counts Ragefire in, Deadmines and the caverns too.
        s.set_unit("player", Some(player(20, Some("Horde"))));
        assert_eq!(
            names(&s, 1),
            ["Deadmines", "Ragefire Chasm", "Wailing Caverns"]
        );
        // A template with no side counts only the rows for either side.
        s.set_unit("player", Some(player(20, None)));
        assert_eq!(names(&s, 1), ["Deadmines", "Ragefire Chasm"]);
        // Level 30, Alliance: only the Stockades fit (22 to 34), so the list is cut to one.
        s.set_unit("player", Some(player(30, Some("Alliance"))));
        assert_eq!(names(&s, 1), ["Stormwind Stockades"]);
        // The other types, an empty one, and a type of 6 or more.
        assert!(names(&s, 2).is_empty(), "no raid fits level 30");
        s.set_unit("player", Some(player(60, Some("Alliance"))));
        assert_eq!(names(&s, 2), ["Molten Core"]);
        for kind in [0, 3, 5, 6, 99, -1] {
            assert!(names(&s, kind).is_empty(), "{kind}");
        }
    }

    /// The quest list (type 3) is the log's quests, cached titles first, by title.
    #[test]
    fn the_quest_list_is_the_logs_quests_cached_titles_first() {
        use crate::script::quest_log::fixture::{record, slot};
        use crate::script::{QuestSlotView, QUEST_LOG_SLOTS};
        let mut s = UiScript::new().unwrap();
        // Quest 11's record is not held: it lists with no title.
        let mut slots = [QuestSlotView::default(); QUEST_LOG_SLOTS];
        for (i, id) in [10, 11, 12].into_iter().enumerate() {
            slots[i] = slot(id, 0, 0);
        }
        s.seed_quest_record(10, record("wolves", 1, 12, "Elwynn Forest"));
        s.seed_quest_record(12, record("Apples", 1, 12, "Elwynn Forest"));
        s.set_quest_slots(slots);
        assert_eq!(names(&s, 3), ["Apples", "wolves", ""]);
        // The quest list needs no player.
        assert_eq!(s.arity("GetLFGTypeEntries(3)").unwrap(), 3);
    }

    /// `LFGQuery(type, entry)` sends only for a type below 6 and an entry below the type's count,
    /// with `(type << 24) & entry` and the class number (0 if not a number).
    #[test]
    fn lfg_query_sends_inside_the_counts_only() {
        use crate::script::SocialCall::LfgQuery;
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_lfg_dungeons(table());
        s.set_unit("player", Some(player(20, Some("Alliance"))));
        // Type 1 has 2 entries at level 20 (Alliance).
        s.run("LFGQuery(1, 0) LFGQuery(1, 1, 7) LFGQuery(1, 2) LFGQuery(1) LFGQuery(6, 0)")
            .unwrap();
        s.run("LFGQuery(1, 1, 'x') LFGQuery(2, 0) LFGQuery(-1, 0) LFGQuery(1, -1)")
            .unwrap();
        // `(1 << 24) & entry` is 0 for any real entry (the reference's `and`, not an `or`). The
        // bare `LFGQuery(1)` is entry 0, and a class that is no number is 0.
        let sent = s.take_social_calls();
        let class = |c| LfgQuery {
            criterion: 0,
            class: c,
        };
        assert_eq!(sent, [class(0), class(7), class(0), class(0)]);
        // A quest entry below the log's count.
        let mut slots = [crate::script::QuestSlotView::default(); crate::script::QUEST_LOG_SLOTS];
        slots[0] = crate::script::quest_log::fixture::slot(5, 0, 0);
        s.set_quest_slots(slots);
        s.run("LFGQuery(3, 0) LFGQuery(3, 1)").unwrap();
        assert_eq!(
            s.take_social_calls(),
            [LfgQuery {
                criterion: 0,
                class: 0
            }]
        );
    }
}
