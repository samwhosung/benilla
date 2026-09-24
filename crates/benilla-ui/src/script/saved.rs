//! Saved variables: the Lua settings that survive a restart. The reference saves FrameXML's
//! `RegisterForSave` globals to one file read back by a line parser (`0x4913b0`, strings and
//! numbers only), and an addon's `## SavedVariables` to its own file, executed as a chunk before
//! `ADDON_LOADED` (`AddOn_Load` `0x51f240`). Deviation: the `RegisterForSave` file is executed as a
//! chunk too, because the line parser cannot carry a table. The app owns the files (`ui_saved`).
//!
//! The grammar is the reference's: `NAME = value`, `nil` written out, bracketed keys, tab indent,
//! trailing comma. Deviations, so a file diffs cleanly and reloads exactly: LF line ends with no
//! leading blank line, the shortest round-tripping float where the reference's `%.16g` can lose a
//! digit, entries sorted rather than in `pairs` order, `\r` escaped where the reference writes it
//! raw and cannot read it back, and a cycle dropped where the reference writes a sentinel into the
//! caller's table.

use std::collections::HashSet;
use std::ffi::c_void;

use mlua::{Lua, Table, Value};

use super::Model;

/// Deviation: nesting stops at 32 where the reference recurses uncapped (its indent saturates at
/// `0x80`), so a hostile structure cannot exhaust the stack.
const MAX_DEPTH: usize = 32;

impl super::UiScript {
    /// Hold a saved-variables file that failed to load, so the shutdown write leaves it alone.
    /// Deviation: the reference rewrites it from the live defaults (`0x51f865`–`0x51f970`), which
    /// would cost the player the whole file for one typo in a hand edit.
    pub fn hold_saved_file(&self, path: &std::path::Path) {
        let mut model = self.model_mut();
        if !model.held_saved_files.iter().any(|p| p == path) {
            model.held_saved_files.push(path.to_path_buf());
        }
    }

    /// Whether this session's load failed on `path`.
    pub fn saved_file_held(&self, path: &std::path::Path) -> bool {
        self.model_ref().held_saved_files.iter().any(|p| p == path)
    }

    /// The registered names in registration order, the reference's own write order.
    pub fn saved_variable_names(&self) -> Vec<String> {
        self.model_mut().saved_names.clone()
    }

    /// Serialize every registered global as `NAME = value` lines, in bytes, since a Lua string is
    /// bytes and the reference writes them raw. A value with no literal (a function, userdata,
    /// `inf`, `NaN`) is skipped with a warning where the reference drops it silently; `nil` is
    /// written, as the reference records an untouched toggle.
    pub fn saved_variables_bytes(&self) -> Vec<u8> {
        self.saved_variables_bytes_for(&self.saved_variable_names())
    }

    /// [`UiScript::saved_variables_bytes`] over an explicit name list, an addon's own
    /// `## SavedVariables` set.
    pub fn saved_variables_bytes_for(&self, names: &[String]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut unwritable = Vec::new();
        for name in names {
            let value: Value = match self.lua().globals().get(name.as_str()) {
                Ok(v) => v,
                Err(e) => {
                    unwritable.push(format!("{name} ({e})"));
                    continue;
                }
            };
            let mut seen = HashSet::new();
            match serialize(&value, 1, &mut seen) {
                Some(text) => {
                    out.extend_from_slice(name.as_bytes());
                    out.extend_from_slice(b" = ");
                    out.extend_from_slice(&text);
                    out.push(b'\n');
                }
                None => unwritable.push(name.clone()),
            }
        }
        if !unwritable.is_empty() {
            self.model_mut().record_warning(format!(
                "saved variables: not serializable, skipped: {}",
                unwritable.join(", ")
            ));
        }
        out
    }
}

/// One Lua value as Lua expression text, or `None` when it has none. `depth` is the indent of a
/// table's contents here (1 at top level); `seen` holds the tables on the current path, so a repeat
/// is a cycle and drops, while a shared subtable is written each time it appears.
fn serialize(v: &Value, depth: usize, seen: &mut HashSet<*const c_void>) -> Option<Vec<u8>> {
    match v {
        Value::Nil => Some(b"nil".to_vec()),
        Value::Boolean(b) => Some(b.to_string().into_bytes()),
        Value::Integer(i) => Some(i.to_string().into_bytes()),
        Value::Number(n) => number(*n).map(String::into_bytes),
        Value::String(s) => Some(quote(&s.as_bytes())),
        Value::Table(t) => table(t, depth, seen),
        // Functions, threads and userdata have no literal; a frame's `[0]` handle is a
        // lightuserdata (`0x701bd0`).
        _ => None,
    }
}

/// A Lua number literal: integral values bare, as the reference's `%.16g` prints them, others in
/// the shortest form that round-trips; `inf` and `NaN` have none.
fn number(n: f64) -> Option<String> {
    if !n.is_finite() {
        return None;
    }
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        return Some(format!("{}", n as i64));
    }
    Some(format!("{n}"))
}

/// A quoted Lua string: the reference's four escapes (`\000`, `\n`, `\"`, `\\`) plus `\r`, every
/// other byte raw.
fn quote(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'"');
    for &b in s {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'"' => out.extend_from_slice(b"\\\""),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0 => out.extend_from_slice(b"\\000"),
            b => out.push(b),
        }
    }
    out.push(b'"');
    out
}

/// A table constructor, `{\n<tabs>[key] = value,\n<tabs-1>}`: every key bracketed, as the
/// reference writes for every table shape (`[0x704607, 0x704989)`), integer keys ascending, then
/// strings. It reloads a list as a list through the 1.12 parser: `recfield` gives a `[k] = v` field
/// no size hint, so dense integer keys land in the array part, which `next` walks in order.
fn table(t: &Table, depth: usize, seen: &mut HashSet<*const c_void>) -> Option<Vec<u8>> {
    if depth > MAX_DEPTH || !seen.insert(t.to_pointer()) {
        return None;
    }
    let mut ints: Vec<(i64, Vec<u8>)> = Vec::new();
    let mut strs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for pair in t.pairs::<Value, Value>() {
        let Ok((k, v)) = pair else { continue };
        let Some(value) = serialize(&v, depth + 1, seen) else {
            continue; // an unserializable entry drops, like the reference's
        };
        match k {
            Value::Integer(i) => ints.push((i, value)),
            // A float key that is integral is the same slot as the integer in Lua 5.1.
            Value::Number(n) if n.fract() == 0.0 => ints.push((n as i64, value)),
            Value::String(s) => strs.push((s.as_bytes().to_vec(), value)),
            _ => continue, // a table/bool/function key cannot be written as a literal
        }
    }
    seen.remove(&t.to_pointer());
    ints.sort_by_key(|(k, _)| *k);
    // Byte order, the same as `str` order for UTF-8 keys.
    strs.sort_by(|a, b| a.0.cmp(&b.0));

    let indent = "\t".repeat(depth);
    let close = "\t".repeat(depth.saturating_sub(1));
    let mut out = b"{\n".to_vec();
    let mut entry = |key: &[u8], value: &[u8]| {
        out.extend_from_slice(indent.as_bytes());
        out.push(b'[');
        out.extend_from_slice(key);
        out.extend_from_slice(b"] = ");
        out.extend_from_slice(value);
        out.extend_from_slice(b",\n");
    };
    for (k, v) in ints {
        entry(k.to_string().as_bytes(), &v);
    }
    for (k, v) in strs {
        entry(&quote(&k), &v);
    }
    out.extend_from_slice(close.as_bytes());
    out.push(b'}');
    Some(out)
}

/// Register `RegisterForSave`, with no taint gate: the reference refuses it outside Blizzard code
/// (`0x4884e0`), and benilla has no taint model.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "RegisterForSave",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if !model.saved_names.contains(&name) {
                model.saved_names.push(name);
            }
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// Bagnon's shape: a saved bag list sorted with the keyring (-2) last, laid out by `pairs`.
    #[test]
    fn a_saved_list_still_walks_in_index_order_after_a_restart() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            BAGS = { -2, 0, 1, 2, 3, 4 }
            table.sort(BAGS, function(a, b)
                if a == -2 then return false
                elseif b == -2 then return true
                else return a < b end
            end)
            RegisterForSave("BAGS")
        "#,
        )
        .unwrap();
        let walk = |vm: &UiScript| {
            vm.eval::<String>(
                "local out = '' for _, v in pairs(BAGS) do out = out .. v .. ',' end return out",
            )
            .unwrap()
        };
        assert_eq!(walk(&s), "0,1,2,3,4,-2,", "the live table, before any save");

        let text = s.saved_variables_bytes();
        let fresh = UiScript::new().unwrap();
        fresh.run_chunk(&text).unwrap();
        assert_eq!(
            walk(&fresh),
            "0,1,2,3,4,-2,",
            "the restart must walk the same order the live session did; wrote:\n{}",
            String::from_utf8_lossy(&text)
        );
    }

    #[test]
    fn the_declared_globals_round_trip_through_a_fresh_vm() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            TRAINER_FILTER_AVAILABLE = 1
            TRAINER_FILTER_UNAVAILABLE = 0
            NEVER_TOUCHED = nil
            CHAT_LABEL = "say \"hi\"\nnow"
            OPACITY = 0.5
            SPELLBOOK_PAGENUMBERS = { [2] = 3, [1] = 1, ["odd key"] = { [1] = true } }
            RegisterForSave("TRAINER_FILTER_AVAILABLE")
            RegisterForSave("TRAINER_FILTER_UNAVAILABLE")
            RegisterForSave("NEVER_TOUCHED")
            RegisterForSave("CHAT_LABEL")
            RegisterForSave("OPACITY")
            RegisterForSave("SPELLBOOK_PAGENUMBERS")
            RegisterForSave("TRAINER_FILTER_AVAILABLE")
        "#,
        )
        .unwrap();
        assert_eq!(
            s.saved_variable_names(),
            vec![
                "TRAINER_FILTER_AVAILABLE",
                "TRAINER_FILTER_UNAVAILABLE",
                "NEVER_TOUCHED",
                "CHAT_LABEL",
                "OPACITY",
                "SPELLBOOK_PAGENUMBERS",
            ],
            "registration order, and a re-register is not a second entry"
        );
        let bytes = s.saved_variables_bytes();
        let text = String::from_utf8(bytes).expect("every value here is UTF-8");
        assert_eq!(
            text,
            "TRAINER_FILTER_AVAILABLE = 1\n\
             TRAINER_FILTER_UNAVAILABLE = 0\n\
             NEVER_TOUCHED = nil\n\
             CHAT_LABEL = \"say \\\"hi\\\"\\nnow\"\n\
             OPACITY = 0.5\n\
             SPELLBOOK_PAGENUMBERS = {\n\
             \t[1] = 1,\n\
             \t[2] = 3,\n\
             \t[\"odd key\"] = {\n\
             \t\t[1] = true,\n\
             \t},\n\
             }\n",
            "got:\n{text}"
        );
        assert!(s.take_warnings().is_empty(), "nothing was unserializable");

        let fresh = UiScript::new().unwrap();
        fresh.run(&text).unwrap();
        assert_eq!(
            fresh
                .eval::<i64>("return TRAINER_FILTER_AVAILABLE + TRAINER_FILTER_UNAVAILABLE")
                .unwrap(),
            1
        );
        assert!(fresh.eval::<bool>("return NEVER_TOUCHED == nil").unwrap());
        assert_eq!(
            fresh.eval::<String>("return CHAT_LABEL").unwrap(),
            "say \"hi\"\nnow"
        );
        assert_eq!(fresh.eval::<f64>("return OPACITY").unwrap(), 0.5);
        assert_eq!(
            fresh
                .eval::<i64>("return SPELLBOOK_PAGENUMBERS[2]")
                .unwrap(),
            3
        );
        assert!(fresh
            .eval::<bool>("return SPELLBOOK_PAGENUMBERS['odd key'][1]")
            .unwrap());
    }

    #[test]
    fn unserializable_values_are_skipped_with_one_warning() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            A_FUNCTION = function() end
            NOT_A_NUMBER = 1/0
            KEPT = 7
            CYCLE = {}
            CYCLE.self = CYCLE
            RegisterForSave("A_FUNCTION")
            RegisterForSave("NOT_A_NUMBER")
            RegisterForSave("KEPT")
            RegisterForSave("CYCLE")
        "#,
        )
        .unwrap();
        let text = s.saved_variables_bytes();
        // The cycle's own entry drops; the table itself still writes (empty here).
        assert_eq!(text, b"KEPT = 7\nCYCLE = {\n}\n");
        let warns = s.take_warnings();
        assert_eq!(warns.len(), 1, "one line, not one per name: {warns:?}");
        assert!(warns[0].contains("A_FUNCTION") && warns[0].contains("NOT_A_NUMBER"));
        UiScript::new().unwrap().run_chunk(&text).unwrap();
    }

    #[test]
    fn a_byte_string_that_is_not_utf8_round_trips_unchanged() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            PACKED = string.char(65, 200, 255, 0, 66)
            KEYS = { [string.char(200)] = 1, [string.char(201)] = 2 }
            RegisterForSave("PACKED")
            RegisterForSave("KEYS")
        "#,
        )
        .unwrap();
        let fresh = UiScript::new().unwrap();
        fresh.run_chunk(&s.saved_variables_bytes()).unwrap();
        assert!(fresh
            .eval::<bool>(
                "return string.len(PACKED) == 5 and string.byte(PACKED, 2) == 200 \
                 and string.byte(PACKED, 3) == 255 and string.byte(PACKED, 4) == 0"
            )
            .unwrap());
        assert!(fresh
            .eval::<bool>("return KEYS[string.char(200)] == 1 and KEYS[string.char(201)] == 2")
            .unwrap());
    }
}
