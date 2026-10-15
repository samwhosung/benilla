//! The looking-for-group result globals of `PartyFrame.cpp` (`0x4e9760`..`0x4e9bd1`; the table
//! `0x84c170`) that the 1.12 interface never calls. `SetLookingForGroup` and `GetLookingForGroup`
//! are [`super::social`]'s.
//!
//! The client keeps its result list (50 records, `0xbc6fd8`) from opcode `0x1ff`, and the dungeon
//! lists (`0xbc6f90`, counted per player level at `0xbc6f18`) from `LfgDungeons.dbc` and the quest
//! log. Benilla handles no `0x1ff` and builds none of those lists, so each function answers what
//! the client answers with both empty: no results, no entries, and `LFGQuery`'s bounds test
//! (`entry < count`) failing before anything is sent.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{number_arg, tuple};

/// The five literal type names `GetLFGTypes` pushes from `0x84c158` (types 1 to 5).
const LFG_TYPES: [&str; 5] = ["Dungeon", "Raid", "Quest", "Zone", "Battleground"];

/// Register the looking-for-group result globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // LFGQuery(type [, entry, class]) (`0x4e9760`): `is-number` on the first argument or the
    // usage error. With every type's count 0 the bounds test (`0x4e97be`) fails, so no `0x1ff`.
    g.set(
        "LFGQuery",
        lua.create_function(|lua, args: mlua::Variadic<Value>| {
            number_arg(
                lua,
                args.first().cloned().unwrap_or(Value::Nil),
                "Usage: LFGQuery(param1 [, class])",
            )?;
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

    // GetLFGTypeEntries(type) (`0x4e9aa0`): a number or the usage error; every list is empty, so
    // `xor eax,eax` after the bounds test: no value.
    g.set(
        "GetLFGTypeEntries",
        lua.create_function(|lua, kind: Value| {
            number_arg(lua, kind, "Usage: GetLFGTypeEntries(type)")?;
            Ok(())
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
}
