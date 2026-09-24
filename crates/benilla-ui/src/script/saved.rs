//! **Saved variables** — the Lua-level settings that survive a restart.
//!
//! The reference client has *two* mechanisms for this, byte-verified:
//!
//! 1. **`RegisterForSave("NAME")`** — the Lua API FrameXML uses for its **38** globals
//!    (`LOCK_ACTIONBAR`, `CHAT_LOCKED`, the eleven `COMBAT_TEXT_*`, …), written to the flat
//!    `WTF/Account/<ACC>/SavedVariables.lua` and read back by a hand-rolled line parser
//!    (`0x4913b0`, strings and numbers only) before `VARIABLES_LOADED` fires.
//! 2. **`## SavedVariables` in a `.toc`** — one file per addon, *executed as a Lua chunk*
//!    (`AddOn_Load 0x51f240` step 4, before `ADDON_LOADED` at step 6). Exactly one stock addon
//!    uses it: `Blizzard_TrainerUI`, for the trainer window's three filter globals.
//!
//! benilla's ported UI is **one FrameXML tree, not addons** ([`crate::script`]'s manifest), so the
//! two collapse into one here: the *declaration* API is (1) — the one our XML can call — and the
//! *load* is (2)'s, a real chunk, because we already have a Lua VM and executing beats a line
//! parser that cannot carry a table. One file, install-scoped (the `benilla/` folder is already
//! per-install, so it IS the reference's account scope). The host side — path,
//! load seam, write triggers — is the app's `ui_saved` module's.
//!
//! **Deliberate divergences from the reference's serializer**, all recorded in 1128: LF and no
//! leading blank line (its `"\r\n"`-per-variable prefix means every file it writes *starts* with a
//! blank line); Rust's shortest round-tripping float formatting instead of `%.16g` (which loses a
//! double needing 17 digits); table entries emitted in a **sorted** order instead of raw `pairs`
//! order, so a settings file diffs cleanly; `\r` escaped rather than written raw (raw CR inside a
//! quoted string is what the reference emits and it does not re-parse); and a cycle is dropped
//! rather than cut by writing a lightuserdata sentinel *into the caller's table*. What is faithful
//! is the grammar — `NAME = value`, `nil` written out, bracketed keys, TAB indent, trailing comma —
//! so a file is readable by, and swappable with, the reference's own.

use std::collections::HashSet;
use std::ffi::c_void;

use mlua::{Lua, Table, Value};

use super::Model;

/// How deep a table may nest before the serializer gives up. The reference recurses uncapped (only
/// its indent saturates at `0x80`); a settings file is not a place where 32 levels is anything but
/// a bug, and a bound is what keeps a hostile or accidental structure from eating the stack.
const MAX_DEPTH: usize = 32;

impl super::UiScript {
    /// **Hold a saved-variables file that did not load** — the shutdown write must leave it alone.
    ///
    /// A file that fails as a chunk (a hand edit with a typo; a database past Lua's 262,143
    /// constants per chunk) leaves its globals at the defaults the code assigned, and the write at
    /// logout would then replace the player's whole file with those defaults. The reference does
    /// exactly that — it discards an unparseable file and writes whole from live values
    /// (`0x51f865`–`0x51f970`) — and both load sites here already promised otherwise ("left on
    /// disk untouched"); this is what makes the promise true. The cost is this
    /// session's changes to that one file, which is the trade the promise named.
    pub fn hold_saved_file(&self, path: &std::path::Path) {
        let mut model = self.model_mut();
        if !model.held_saved_files.iter().any(|p| p == path) {
            model.held_saved_files.push(path.to_path_buf());
        }
    }

    /// Did this session's load fail on `path`? ([`Self::hold_saved_file`].)
    pub fn saved_file_held(&self, path: &std::path::Path) -> bool {
        self.model_ref().held_saved_files.iter().any(|p| p == path)
    }

    /// The registered names, in registration order — what the host writes out (the reference's
    /// own emission order, and stable across runs because the load order is).
    pub fn saved_variable_names(&self) -> Vec<String> {
        self.model_mut().saved_names.clone()
    }

    /// Serialize every registered global into the settings file's body, `NAME = value` per line.
    ///
    /// A name whose value cannot round-trip through a Lua chunk (a function, a widget reference,
    /// any userdata — and `inf`/`NaN`, which are not Lua literals) is **skipped with a warning**
    /// rather than written as something that would fail to load; the reference loses the same set,
    /// silently. `nil` IS written (`NAME = nil`), which is how the reference records a toggle that
    /// has never been touched.
    ///
    /// **Bytes, not text**: a Lua string is a byte string, and the file is executed as a chunk
    /// that reads bytes (1193), so the writer is the one place that could change a value — and it
    /// did. Values and keys went through `to_string_lossy`, so a string an addon cut mid-codepoint
    /// or packed with high bytes (a compression library's output) came back from the next login as
    /// U+FFFD, and two such keys could fold into one entry. The reference writes the bytes raw.
    pub fn saved_variables_bytes(&self) -> Vec<u8> {
        self.saved_variables_bytes_for(&self.saved_variable_names())
    }

    /// [`UiScript::saved_variables_bytes`] over an explicit name list — an addon's own
    /// `## SavedVariables` set (1188 phase 3), which is declared in its manifest rather than
    /// through `RegisterForSave`. Same grammar, same skip rules; only the source of the names
    /// differs, which is exactly the difference between the reference's two mechanisms.
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

/// One Lua value as the text of a Lua expression, or `None` when it cannot be one.
///
/// `depth` is the indent level of the *contents* of a table at this position (so a top-level value
/// starts at 1); `seen` carries the table identities on the current path — a repeat is a cycle and
/// drops that entry. Note it is the *path*, not every table ever visited: a shared subtable is
/// legitimately written twice (as it must be, since the file has no way to express aliasing).
fn serialize(v: &Value, depth: usize, seen: &mut HashSet<*const c_void>) -> Option<Vec<u8>> {
    match v {
        Value::Nil => Some(b"nil".to_vec()),
        Value::Boolean(b) => Some(b.to_string().into_bytes()),
        Value::Integer(i) => Some(i.to_string().into_bytes()),
        Value::Number(n) => number(*n).map(String::into_bytes),
        Value::String(s) => Some(quote(&s.as_bytes())),
        Value::Table(t) => table(t, depth, seen),
        // Functions, threads, userdata (every widget reference is one — a frame's Lua value is a
        // table whose `[0]` is a lightuserdata handle, written at `0x701bd0`) cannot be written as
        // a literal.
        _ => None,
    }
}

/// A Lua number literal. Integral values print without a fractional part (`1`, not `1.0` — the
/// reference's `%.16g` does the same, and our XML compares these against integers); everything
/// else takes Rust's shortest form that round-trips exactly. `inf`/`NaN` have no literal at all.
fn number(n: f64) -> Option<String> {
    if !n.is_finite() {
        return None;
    }
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        return Some(format!("{}", n as i64));
    }
    Some(format!("{n}"))
}

/// A quoted Lua string. The reference escapes exactly four characters (`\000`, `\n`, `\"`, `\\`)
/// and writes CR and every high byte raw; we add `\r` (raw CR in a quoted string is not something
/// its own loader could read back) and keep every other byte raw, so localized text stays legible
/// and a byte string that is not UTF-8 round-trips unchanged.
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

/// A table constructor: `{\n<tabs>[key] = value,\n<tabs-1>}`. Keys are always bracketed (the
/// reference's shape — its writer emits `[key] = value` for **every** table shape and never a bare
/// positional entry, byte-verified (no branch in `[0x704607, 0x704989)` writes a value without a
/// bracketed key, for any table shape — and it sidesteps every reserved-word and non-identifier
/// question); entries are **sorted**
/// — integer keys ascending, then strings alphabetically — so the file is stable across runs
/// instead of following Lua's hash order.
///
/// **A list written this way still reloads as a list**, and that is a property of the *parser*, not
/// of this emitter: 1.12's `recfield` gives a `[expr] = v` field neither size hint, so the table is
/// born on the dummy node, the first store rehashes, and the dense integer keys land in an array
/// part that `next` walks ascending. Our vendored parser was restored to that placement in decision
/// 2111 — before it, this exact file shape came back keyed 1..n in the *hash* part and Bagnon's
/// keyring drew first.
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
    // Byte order — the same order `str`'s `Ord` gave every UTF-8 key before.
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

/// Register the `RegisterForSave` global.
///
/// Divergence, disclosed: the reference's binding is **taint-gated to Blizzard code**
/// (`0x4884e0`), because a third-party addon writing into the shared account file would be a
/// security surface. benilla has no taint model yet and loads no third-party addons, so the gate
/// has nothing to gate; when addon loading lands, this is one of the bindings that needs it (and
/// an addon's own `## SavedVariables` file is the mechanism it should use instead — the `.toc`
/// directive [`crate::toc::Toc::list`] already parses).
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

    /// **A saved LIST comes back a list** — Bagnon's inventory grid, at 1:1 scale, and the
    /// round-trip half of decision 2111 (the dialect half is `lua50`'s own tests).
    ///
    /// `Bagnon_Core` keeps its frame's bag order in a saved table, sorts it so the keyring
    /// (`KEYRING_CONTAINER` = -2) lands last, and then lays the grid out by walking it with
    /// **`pairs`** (`BagnonFrame_Generate`'s `for _, bagID in pairs(BagnonSets[frameName].bags)`).
    /// Session one has no file: the table is the literal `{-2,0,1,2,3,4}`, an array table, and
    /// `pairs` walks it 1..6 — keyring last, as the reference shows it. Session two reads the
    /// serializer's `[k] = v` form back, and before 2111 that came back keyed 1..6 in the **hash**
    /// part, walked in slot order, with key 6 — the keyring — **first**, ahead of the backpack.
    /// That is the director's report; the writer is unchanged and the parser is what was wrong.
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

    /// The round trip that is the whole point: what the writer emits, the loader's own chunk
    /// restores — through a fresh VM, exactly as a restart does.
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
        // The grammar: one statement per line, `nil` written out, integral numbers bare, keys
        // bracketed and SORTED (integers ascending, then strings), tab-indented, trailing comma.
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

        // A fresh VM (the restart), the file's chunk, and every value is back.
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

    /// A value with no Lua literal is skipped and named, never written as something that would
    /// break the next load — the one place we improve on the reference, which loses these silently.
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
        // And what it wrote is loadable.
        UiScript::new().unwrap().run_chunk(&text).unwrap();
    }

    /// **A byte string comes back as the same bytes.** A Lua string is bytes; an addon that cuts
    /// a UTF-8 name mid-codepoint with `string.sub`, or packs data with `string.char(≥128)` (a
    /// compression library's output), stores something that is not UTF-8. The writer used to
    /// decode lossily, so the next login read U+FFFD — and two such keys folded into one entry.
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
