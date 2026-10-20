//! Saved variables: the Lua settings that survive a restart. The reference saves FrameXML's
//! `RegisterForSave` globals to one flat file it reads back line by line, never as Lua
//! (`0x4913b0`, strings and numbers only: [`parse_flat`]), and an addon's `## SavedVariables` to its
//! own file, executed as a chunk before `ADDON_LOADED` (`AddOn_Load` `0x51f240`). The app owns the
//! files (`ui_saved`).
//!
//! The written grammar is the reference's: `NAME = value`, `nil` written out, bracketed keys, tab
//! indent, trailing comma. Deviations, so a file diffs cleanly and reloads exactly: LF line ends
//! with no leading blank line, the shortest round-tripping float where the reference's `%.16g` can
//! lose a digit, entries sorted rather than in `pairs` order, `\r` escaped where the reference
//! writes it raw and cannot read it back, and a cycle dropped where the reference writes a sentinel
//! into the caller's table.

use std::collections::HashSet;
use std::ffi::c_void;

use mlua::{Lua, Table, Value};

use super::{binding_abi, Model};

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

    /// Whether an addon's saved-variables file at `path` ran cleanly this session.
    pub fn saved_file_loaded(&self, path: &std::path::Path) -> bool {
        self.model_ref()
            .loaded_saved_files
            .iter()
            .any(|p| p == path)
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
        let model = self.lua().app_data_ref::<Model>().expect("model");
        for name in names {
            let value: Value = match self.lua().globals().get(name.as_str()) {
                Ok(v) => v,
                Err(e) => {
                    unwritable.push(format!("{name} ({e})"));
                    continue;
                }
            };
            let mut seen = HashSet::new();
            match serialize(&value, 1, &mut seen, &model) {
                Some(text) => {
                    out.extend_from_slice(name.as_bytes());
                    out.extend_from_slice(b" = ");
                    out.extend_from_slice(&text);
                    out.push(b'\n');
                }
                None => unwritable.push(name.clone()),
            }
        }
        drop(model);
        if !unwritable.is_empty() {
            self.model_mut().record_warning(format!(
                "saved variables: not serializable, skipped: {}",
                unwritable.join(", ")
            ));
        }
        out
    }

    /// Read the flat `RegisterForSave` file into the globals as `0x4913b0` does: [`parse_flat`]'s
    /// strings through `0x704300` and its numbers through `0x704250`, and no Lua runs. Answers
    /// [`FlatFile::lost`].
    pub fn load_flat_saved_variables(&self, bytes: &[u8]) -> usize {
        let lua = self.lua();
        let globals = lua.globals();
        let flat = parse_flat(bytes);
        for (name, value) in flat.assignments {
            let value = match value {
                Some(FlatValue::Str(s)) => lua.create_string(&s).map(Value::String),
                Some(FlatValue::Num(n)) => Ok(Value::Number(f64::from(n))),
                None => Ok(Value::Nil),
            };
            if let (Ok(name), Ok(value)) = (lua.create_string(&name), value) {
                let _ = globals.set(name, value);
            }
        }
        flat.lost
    }
}

/// A value the flat reader sets: a non-empty string, or a number already rounded to `f32`.
#[derive(Debug, PartialEq)]
pub(crate) enum FlatValue {
    Str(Vec<u8>),
    Num(f32),
}

/// What [`parse_flat`] reads: the assignments in file order (`None` sets nil), and how many lines
/// set nothing though they were not blank, a comment or `NAME = nil`, or were cut at the limit.
#[derive(Debug, Default)]
pub(crate) struct FlatFile {
    pub(crate) assignments: Vec<(Vec<u8>, Option<FlatValue>)>,
    pub(crate) lost: usize,
}

/// The longest line the reader keeps: `0x64ae50` copies at most 1,023 bytes into its 1,024-byte
/// buffer (`0x491407`, `0x64af4b`) and drops the rest up to the next separator.
const FLAT_LINE_MAX: usize = 0x3ff;

/// Parse the flat file as `0x4913b0` does. The text ends at its first NUL, and `0x64ae50` splits it
/// at CR and LF, skipping empty lines. Per line: leading spaces skipped, not tabs (`0x49141c`); the
/// name runs to a space or `=` (`0x491445`-`0x49144d`); spaces, then `=` or the line is ignored
/// (`0x49146b`); spaces, then the value is the rest of the line (`0x491489`); an empty name or value
/// is ignored (`0x4914ab`, `0x4914b9`). A value opening `"` is a string ([`flat_string`]), one
/// opening with a digit, or `-` or `.` then a digit (`0x4914fa`-`0x491526`), a number
/// ([`flat_number`]); anything else (`nil`, `true`, a table, a call) is ignored.
pub(crate) fn parse_flat(bytes: &[u8]) -> FlatFile {
    let bytes = bytes.split(|&b| b == 0).next().unwrap_or_default();
    let mut out = FlatFile::default();
    for raw in bytes.split(|&b| b == b'\r' || b == b'\n') {
        if raw.is_empty() {
            continue;
        }
        let cut = raw.len() > FLAT_LINE_MAX;
        let line = &raw[..raw.len().min(FLAT_LINE_MAX)];
        let set = flat_line(line);
        let benign = || {
            let rest = skip_spaces(line);
            rest.iter().all(|&b| b == b' ' || b == b'\t')
                || rest.starts_with(b"--")
                || flat_split(line).is_some_and(|(_, v)| v == b"nil" || v.starts_with(b"nil "))
        };
        if cut || (set.is_none() && !benign()) {
            out.lost += 1;
        }
        out.assignments.extend(set);
    }
    out
}

fn skip_spaces(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| b != b' ').unwrap_or(s.len());
    &s[start..]
}

/// `(name, value text)` of a `name = value` line, `None` where `0x4913b0` ignores the line.
fn flat_split(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = skip_spaces(line);
    let end = rest
        .iter()
        .position(|&b| b == b' ' || b == b'=')
        .unwrap_or(rest.len());
    let (name, rest) = rest.split_at(end);
    let value = skip_spaces(skip_spaces(rest).strip_prefix(b"=")?);
    (!name.is_empty() && !value.is_empty()).then_some((name, value))
}

/// One line's assignment, `None` when the reader ignores the line.
fn flat_line(line: &[u8]) -> Option<(Vec<u8>, Option<FlatValue>)> {
    let (name, value) = flat_split(line)?;
    let value = match value {
        [b'"', ..] => flat_string(value).map(FlatValue::Str),
        [d, ..] if d.is_ascii_digit() => Some(FlatValue::Num(flat_number(value))),
        [b'-' | b'.', d, ..] if d.is_ascii_digit() => Some(FlatValue::Num(flat_number(value))),
        _ => return None,
    };
    Some((name.to_vec(), value))
}

/// A quoted value cut at its last `"` above index 1 (`0x4914c3`-`0x4914e0`), so `""` and an
/// unterminated `"abc` are empty, which `0x704300` sets as nil (`0x70430f`).
/// Deviation: the writer's escapes are undone ([`unescape`]), where the reference sets the bytes
/// between the quotes raw, so a string holding a quote or a backslash would grow every session.
fn flat_string(value: &[u8]) -> Option<Vec<u8>> {
    let close = (2..value.len())
        .rev()
        .find(|&i| value[i] == b'"')
        .unwrap_or(1);
    let raw = value.get(1..close).unwrap_or_default();
    (!raw.is_empty()).then(|| unescape(raw))
}

/// Undo [`quote`]'s escapes (`\\`, `\"`, `\n`, `\r`, `\000`); any other backslash stays as it is.
fn unescape(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let (byte, len) = match (raw[i], raw.get(i + 1)) {
            (b'\\', Some(b'\\')) => (b'\\', 2),
            (b'\\', Some(b'"')) => (b'"', 2),
            (b'\\', Some(b'n')) => (b'\n', 2),
            (b'\\', Some(b'r')) => (b'\r', 2),
            (b'\\', Some(b'0')) if raw.get(i + 2..i + 4) == Some(b"00") => (0, 4),
            (b, _) => (b, 1),
        };
        out.push(byte);
        i += len;
    }
    out
}

/// `SStrToFloat` (`0x64aaa0`) rounded to `f32` (`fstp dword` at `0x491534`): an optional `-`,
/// digits, an optional `.` and digits, then `e` or `E`, an optional `+` (`0x64ac0a`-`0x64ac1d`) and
/// `SStrToInt`'s optional `-` and digits (`0x64ac60`); whatever follows is ignored, so `5abc` is 5
/// and `0x10` is 0.
fn flat_number(value: &[u8]) -> f32 {
    fn digits(s: &[u8]) -> (&[u8], &[u8]) {
        s.split_at(s.iter().take_while(|b| b.is_ascii_digit()).count())
    }
    fn sign(s: &[u8]) -> (bool, &[u8]) {
        s.strip_prefix(b"-").map_or((false, s), |rest| (true, rest))
    }
    let (neg, rest) = sign(value);
    let (int, rest) = digits(rest);
    let (frac, rest) = rest.strip_prefix(b".").map_or((&b""[..], rest), digits);
    let mut exp: i64 = 0;
    if let Some(rest) = rest.strip_prefix(b"e").or_else(|| rest.strip_prefix(b"E")) {
        let (eneg, rest) = sign(rest.strip_prefix(b"+").unwrap_or(rest));
        for &d in digits(rest).0 {
            exp = (exp * 10 + i64::from(d - b'0')).min(100_000);
        }
        if eneg {
            exp = -exp;
        }
    }
    let text = |d: &[u8]| {
        if d.is_empty() {
            "0".to_owned()
        } else {
            String::from_utf8_lossy(d).into_owned()
        }
    };
    let text = format!(
        "{}{}.{}e{exp}",
        if neg { "-" } else { "" },
        text(int),
        text(frac)
    );
    text.parse::<f64>().unwrap_or(0.0) as f32
}

/// One Lua value as Lua expression text, or `None` when it has none. `depth` is the indent of a
/// table's contents here (1 at top level); `seen` holds the tables on the current path, so a repeat
/// is a cycle and drops, while a shared subtable is written each time it appears.
fn serialize(
    v: &Value,
    depth: usize,
    seen: &mut HashSet<*const c_void>,
    model: &Model,
) -> Option<Vec<u8>> {
    match v {
        Value::Nil => Some(b"nil".to_vec()),
        Value::Boolean(b) => Some(b.to_string().into_bytes()),
        Value::Integer(i) => Some(i.to_string().into_bytes()),
        Value::Number(n) => number(*n).map(String::into_bytes),
        Value::String(s) => Some(quote(&s.as_bytes())),
        // A widget writes as its name instead of its fields (`0x70463b`-`0x704666`): the reference
        // reads `[0]` first (`0x704619`) and, for a handle, writes the object's vtable `+4` name
        // bare through `%s`, so the chunk reloads the live frame by its global, or
        // `nil --[[ unnamed object ]]` (`0x872e0c`) when it has none. The name comes off the
        // model, never through a Lua `GetName` an addon can replace.
        Value::Table(t) => match binding_abi::handle_name(model, t) {
            Some(name) => Some(name.filter(|n| !n.is_empty()).map_or_else(
                || b"nil --[[ unnamed object ]]".to_vec(),
                String::into_bytes,
            )),
            None => table(t, depth, seen, model),
        },
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
fn table(
    t: &Table,
    depth: usize,
    seen: &mut HashSet<*const c_void>,
    model: &Model,
) -> Option<Vec<u8>> {
    if depth > MAX_DEPTH || !seen.insert(t.to_pointer()) {
        return None;
    }
    let mut ints: Vec<(i64, Vec<u8>)> = Vec::new();
    let mut strs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for pair in t.pairs::<Value, Value>() {
        let Ok((k, v)) = pair else { continue };
        let Some(value) = serialize(&v, depth + 1, seen, model) else {
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

/// Register `RegisterForSave` (`0x4884e0`): context 0 only (`[0xceeac0]`, [`super::taint::current`]),
/// then a string or number argument (`0x6f3510`), each refusal raising the reference's text.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "RegisterForSave",
        lua.create_function(|lua, name: Value| {
            if super::taint::current(lua) != super::taint::Context::Client {
                return Err(mlua::Error::RuntimeError(
                    "RegisterForSave() is only available to Blizzard scripts".into(),
                ));
            }
            let name =
                super::binding_abi::string_arg(lua, name, r#"Usage: RegisterForSave("variable")"#)?;
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

    /// A frame kept in a saved variable (Decursive keeps `DEFAULT_CHAT_FRAME` as its output
    /// window) writes as its name and reloads as the same frame, not as a table of its fields
    /// without its methods; an unnamed one writes nil.
    #[test]
    fn a_frame_writes_its_name_and_reloads_as_itself() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            local named = CreateFrame("Frame", "SavedTestOutputFrame")
            named.channelList = { "General" }
            SAVED_WINDOWS = { output = named, scratch = CreateFrame("Frame") }
        "#,
        )
        .unwrap();
        let text = s.saved_variables_bytes_for(&["SAVED_WINDOWS".to_string()]);
        assert_eq!(
            String::from_utf8_lossy(&text),
            "SAVED_WINDOWS = {\n\t[\"output\"] = SavedTestOutputFrame,\n\t[\"scratch\"] = nil --[[ unnamed object ]],\n}\n"
        );
        s.run("SAVED_WINDOWS = nil").unwrap();
        s.run_chunk(&text).unwrap();
        assert!(s
            .eval::<bool>(
                "return SAVED_WINDOWS.output == SavedTestOutputFrame \
                 and SAVED_WINDOWS.output:GetName() == 'SavedTestOutputFrame' \
                 and SAVED_WINDOWS.scratch == nil"
            )
            .unwrap());
    }

    /// A Font object writes as its name, as a frame does: the writer's `lua_touserdata` on `[0]`
    /// (`0x704623`) finds its handle and prints its vtable `+4` name (`0x70463b`).
    #[test]
    fn a_font_object_writes_its_name() {
        let s = UiScript::new().unwrap();
        s.run(r#"SAVED_FONTS = { CreateFont("SavedTestFont") }"#)
            .unwrap();
        let text = s.saved_variables_bytes_for(&["SAVED_FONTS".to_string()]);
        assert_eq!(
            String::from_utf8_lossy(&text),
            "SAVED_FONTS = {\n\t[1] = SavedTestFont,\n}\n"
        );
    }

    /// The name is the object's own (vtable `+4`), so a Lua `GetName` an addon replaced changes
    /// nothing, and a plain table whose `[0]` is a number is no object and writes its fields.
    #[test]
    fn a_frame_writes_its_own_name_and_a_numbered_zero_key_stays_a_table() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            local f = CreateFrame("Frame", "SavedTestRenamedFrame")
            f.GetName = function() return "SomethingElse" end
            SAVED_MIXED = { frame = f, list = { [0] = 5, [1] = 6 } }
        "#,
        )
        .unwrap();
        let text = s.saved_variables_bytes_for(&["SAVED_MIXED".to_string()]);
        assert_eq!(
            String::from_utf8_lossy(&text),
            "SAVED_MIXED = {\n\t[\"frame\"] = SavedTestRenamedFrame,\n\t[\"list\"] = {\n\t\t[0] = 5,\n\t\t[1] = 6,\n\t},\n}\n"
        );
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

    /// One flat file read into a fresh VM whose `N` and `S` default to `"default"`.
    fn read_flat(file: &[u8]) -> UiScript {
        let s = UiScript::new().unwrap();
        s.run("N = 'default' S = 'default'").unwrap();
        s.load_flat_saved_variables(file);
        s
    }

    /// `type(N)` and `tostring(N)` for the global `name` after reading `file`.
    fn flat_global(file: &[u8], name: &str) -> String {
        read_flat(file)
            .eval::<String>(&format!(
                "local v = getglobal({name:?}) return type(v) .. ':' .. tostring(v)"
            ))
            .unwrap()
    }

    /// The number goes through `fstp dword` (`0x491534`): 0.1 comes back as the f32 nearest it.
    #[test]
    fn a_flat_number_is_rounded_to_single_precision() {
        let s = read_flat(b"PARTYBACKGROUND_OPACITY = 0.1\n");
        assert_eq!(
            s.eval::<f64>("return PARTYBACKGROUND_OPACITY").unwrap(),
            f64::from(0.1f32)
        );
    }

    /// The flat file is never run (`0x4913b0` has no call into the compiler): a call is not made,
    /// and a table, a boolean or `nil` leave the global as it was.
    #[test]
    fn a_flat_file_runs_no_lua_and_sets_only_strings_and_numbers() {
        let s = UiScript::new().unwrap();
        s.run(
            "MARKED = 0 function MARK() MARKED = MARKED + 1 return 1 end \
             CALLED = 'default' T = 'default' B = 'default' NIL = 'default'",
        )
        .unwrap();
        s.load_flat_saved_variables(b"CALLED = MARK()\nT = {1}\nB = true\nNIL = nil\n");
        assert_eq!(s.eval::<i64>("return MARKED").unwrap(), 0, "MARK never ran");
        for name in ["CALLED", "T", "B", "NIL"] {
            assert_eq!(
                s.eval::<String>(&format!("return {name}")).unwrap(),
                "default",
                "{name} keeps its default"
            );
        }
    }

    /// The line grammar of `0x4913b0`, case by case.
    #[test]
    fn the_flat_line_grammar_is_the_references() {
        let long = format!("S = \"{}\"", "a".repeat(1100));
        let cases: &[(&[u8], &str, &str)] = &[
            // Strings: cut at the last quote above index 1, empty is nil (`0x70430f`).
            (b"S = \"abc\"", "S", "string:abc"),
            (b"S = \"\"", "S", "nil:nil"),
            (b"S = \"abc", "S", "nil:nil"),
            (b"S = \"", "S", "nil:nil"),
            (b"S = \"a\"b\"", "S", "string:a\"b"),
            (b"S = \"abc\" -- note", "S", "string:abc"),
            // The writer's escapes come back as the bytes they stand for.
            (b"S = \"a\\\"b\\\\c\\nd\\re\"", "S", "string:a\"b\\c\nd\re"),
            (b"S = \"a\\tb\"", "S", "string:a\\tb"),
            // Numbers: a digit, or `-`/`.` then a digit, through `SStrToFloat`.
            (b"N = 7", "N", "number:7"),
            (b"N = -7", "N", "number:-7"),
            (b"N = .5", "N", "number:0.5"),
            (b"N = -.5", "N", "string:default"),
            (b"N = 5abc", "N", "number:5"),
            (b"N = 1e3", "N", "number:1000"),
            (b"N = 2E+2", "N", "number:200"),
            (b"N = 5e-1", "N", "number:0.5"),
            (b"N = 0x10", "N", "number:0"),
            (b"N = 3 -- note", "N", "number:3"),
            // The name and the `=`: leading spaces skipped, tabs kept, spaces optional.
            (b"   N=8", "N", "number:8"),
            (b"\tX = 1", "\tX", "number:1"),
            (b"N x = 1", "N", "string:default"),
            (b"N = = 3", "N", "string:default"),
            (b"N 3", "N", "string:default"),
            // A line past 1,023 bytes is cut, so its closing quote is lost.
            (long.as_bytes(), "S", "nil:nil"),
        ];
        for (file, name, want) in cases {
            assert_eq!(
                flat_global(file, name),
                *want,
                "{:?}",
                String::from_utf8_lossy(file)
            );
        }
        // CR LF, LF and CR alone all end a line; a NUL ends the file.
        for file in [
            &b"N = 1\r\nS = \"x\"\r\n"[..],
            b"N = 1\nS = \"x\"",
            b"N = 1\rS = \"x\"\r",
            b"\r\n\r\nN = 1\n\n\nS = \"x\"\n",
        ] {
            assert_eq!(flat_global(file, "N"), "number:1");
            assert_eq!(flat_global(file, "S"), "string:x");
        }
        assert_eq!(flat_global(b"N = 1\0\nS = \"x\"\n", "S"), "string:default");
    }

    /// What the writer emits for a string reads back as the same bytes, a NUL included.
    #[test]
    fn a_written_string_reads_back_unchanged() {
        let s = UiScript::new().unwrap();
        s.run("V = 'say \"hi\"\\\\ now\\n' .. string.char(0, 200) RegisterForSave('V')")
            .unwrap();
        let fresh = UiScript::new().unwrap();
        fresh.load_flat_saved_variables(&s.saved_variables_bytes());
        assert!(fresh
            .eval::<bool>("return V == 'say \"hi\"\\\\ now\\n' .. string.char(0, 200)")
            .unwrap());
    }

    /// `parse_flat`'s lost count: a line that set nothing and was not blank, a comment or `nil`.
    #[test]
    fn the_lost_count_names_only_lines_with_a_value_dropped() {
        let flat = super::parse_flat(
            b"-- header\n   \n\t\nA = nil\nB = 1\nC = {\n\t[1] = 1,\n}\nD = true\n",
        );
        // `C = {`, `}` and `D = true`; `\t[1] = 1,` sets the global "\t[1]", as the reference does.
        assert_eq!(flat.lost, 3);
        assert_eq!(flat.assignments.len(), 2);
    }

    /// `RegisterForSave` (`0x4884e0`) raises outside context 0, and for an argument that is not a
    /// string or number (`0x488525`).
    #[test]
    fn register_for_save_is_the_clients_alone_and_takes_a_name() {
        let s = UiScript::new().unwrap();
        s.run("RegisterForSave('MINE') RegisterForSave(5)").unwrap();
        assert_eq!(s.saved_variable_names(), vec!["MINE", "5"]);
        let err = s.run("RegisterForSave({})").unwrap_err().to_string();
        assert!(
            err.contains("Usage: RegisterForSave(\"variable\")"),
            "{err}"
        );
        let err = s.run("RegisterForSave()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: RegisterForSave(\"variable\")"),
            "{err}"
        );
        // `RunScript` carries the macro context and hands its error to the handler.
        s.run("RunScript(\"RegisterForSave('MACRO')\")").unwrap();
        let errors = s.errors().join("\n");
        assert!(
            errors.contains("RegisterForSave() is only available to Blizzard scripts"),
            "{errors}"
        );
        assert_eq!(s.saved_variable_names(), vec!["MINE", "5"]);
    }
}
