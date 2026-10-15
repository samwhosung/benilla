//! The looking-for-group result globals of `PartyFrame.cpp` (`0x4e9760`..`0x4e9bd1`; the table
//! `0x84c170`) that the 1.12 interface never calls. `SetLookingForGroup` and `GetLookingForGroup`
//! are [`super::social`]'s.
//!
//! The client keeps its result list (50 records, `0xbc6fd8`) from opcode `0x1ff`, which benilla
//! does not handle, so that list is empty. The dungeon lists are built: `LFGDungeons.dbc` grouped
//! by type (`0x4e7c70`, pushed by the app as [`LfgDungeon`]), counted for the player's level and
//! side (`0x4e89c0`) and sorted so the rows that fit the level come first (`0x4e8b20`); the quest
//! list (type 3) is the quest log's ids, cached titles first (`0x4e8be0`, `0x4e8c80`). A type's
//! entries are the first `count` of its sorted list, so a row of the wrong side can show where a
//! right one is cut, as in the reference.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{coerced_number, ftol, number_arg, tuple};
use super::social::SocialRequest;
use super::Model;

/// The five literal type names `GetLFGTypes` pushes from `0x84c158` (types 1 to 5).
const LFG_TYPES: [&str; 5] = ["Dungeon", "Raid", "Quest", "Zone", "Battleground"];

/// One `LFGDungeons.dbc` row, in file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfgDungeon {
    pub name: String,
    pub min_level: u32,
    pub max_level: u32,
    /// The list it joins, 0 to 5.
    pub kind: u32,
    /// 0 Horde, 1 Alliance, negative either.
    pub side: i32,
}

impl super::UiScript {
    /// Push the dungeon table; without it every list is empty.
    pub fn set_lfg_dungeons(&mut self, rows: Vec<LfgDungeon>) {
        self.model_mut().lfg_dungeons = rows;
    }
}

/// A type's displayed entries and their count (`0xbc6f18[type]`): the first `count` names of the
/// sorted list, which are all there is for the quest list, whose count is its length.
fn type_list(model: &Model, kind: usize) -> (usize, Vec<String>) {
    if kind == 3 {
        // `0x4e8be0`: every nonzero quest id of the log, cached titles first, by title (`0x4e8c80`).
        let mut titles: Vec<&str> = model
            .quest_log
            .entries
            .iter()
            .filter(|e| !e.is_header && e.quest_id != 0)
            .map(|e| e.title.as_str())
            .collect();
        let count = titles.len();
        titles.sort_by(|a, b| match (a.is_empty(), b.is_empty()) {
            (false, false) => icase(a, b),
            (a_null, b_null) => a_null.cmp(&b_null),
        });
        return (count, titles.into_iter().map(str::to_string).collect());
    }
    // The recount needs the active player (`0x4e89cc`).
    let Some(player) = model.unit("player").filter(|p| p.level > 0) else {
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
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if kind < 6 && (entry as usize) < type_list(&model, kind as usize).0 {
                model.social_requests.push(SocialRequest::LfgQuery {
                    criterion: kind.wrapping_shl(24) & entry,
                    class,
                });
            }
            Ok(())
        })?,
    )?;

    // GetNumLFGResults() (`0x4e9870`): the stored count and the server's total, both 0.
    g.set(
        "GetNumLFGResults",
        lua.create_function(|_, ()| Ok(tuple(&[Some(0), Some(0)])))?,
    )?;

    // GetLFGResults(index) (`0x4e98c0`): a number or the usage error; past the list (always) the
    // seven-value miss `nil, 0, nil, nil, nil, nil, nil` (`0x4e9a33`).
    g.set(
        "GetLFGResults",
        lua.create_function(|lua, index: Value| {
            number_arg(lua, index, "Usage: GetLFGResults(index)")?;
            Ok(tuple(&[None, Some(0), None, None, None, None, None]))
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
            let (count, names) = type_list(&model, kind as usize);
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
        use crate::script::{QuestLogEntryView, QuestLogState};
        let mut s = UiScript::new().unwrap();
        let q = |id, title: &str| QuestLogEntryView {
            quest_id: id,
            title: title.into(),
            ..Default::default()
        };
        let header = QuestLogEntryView {
            is_header: true,
            title: "Elwynn Forest".into(),
            ..Default::default()
        };
        s.set_quest_log(QuestLogState {
            entries: vec![header, q(10, "wolves"), q(11, ""), q(12, "Apples")],
            num_quests: 3,
            ..Default::default()
        });
        assert_eq!(names(&s, 3), ["Apples", "wolves", ""]);
        // The quest list needs no player.
        assert_eq!(s.arity("GetLFGTypeEntries(3)").unwrap(), 3);
    }

    /// `LFGQuery(type, entry)` sends only for a type below 6 and an entry below the type's count,
    /// with `(type << 24) & entry` and the class number (0 if not a number).
    #[test]
    fn lfg_query_sends_inside_the_counts_only() {
        use crate::script::SocialRequest::LfgQuery;
        let mut s = UiScript::new().unwrap();
        s.set_lfg_dungeons(table());
        s.set_unit("player", Some(player(20, Some("Alliance"))));
        // Type 1 has 2 entries at level 20 (Alliance).
        s.run("LFGQuery(1, 0) LFGQuery(1, 1, 7) LFGQuery(1, 2) LFGQuery(1) LFGQuery(6, 0)")
            .unwrap();
        s.run("LFGQuery(1, 1, 'x') LFGQuery(2, 0) LFGQuery(-1, 0) LFGQuery(1, -1)")
            .unwrap();
        // `(1 << 24) & entry` is 0 for any real entry (the reference's `and`, not an `or`). The
        // bare `LFGQuery(1)` is entry 0, and a class that is no number is 0.
        let sent = s.take_social_requests();
        let class = |c| LfgQuery {
            criterion: 0,
            class: c,
        };
        assert_eq!(sent, [class(0), class(7), class(0), class(0)]);
        // A quest entry below the log's count.
        s.set_quest_log(crate::script::QuestLogState {
            entries: vec![crate::script::QuestLogEntryView {
                quest_id: 5,
                title: "A".into(),
                ..Default::default()
            }],
            num_quests: 1,
            ..Default::default()
        });
        s.run("LFGQuery(3, 0) LFGQuery(3, 1)").unwrap();
        assert_eq!(
            s.take_social_requests(),
            [LfgQuery {
                criterion: 0,
                class: 0
            }]
        );
    }
}
