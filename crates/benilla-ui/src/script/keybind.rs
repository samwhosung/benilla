//! The key-binding table behind `GetBinding`, `SetBinding`, `LoadBindings` and their kin: per
//! command, a category header and its chords, spelled `[ALT-][CTRL-][SHIFT-]<TOKEN>` and matched by
//! string equality as the reference does. Host commands carry their 1.12 defaults; an addon's
//! `Bindings.xml` adds ordinary rows that also carry a Lua body.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use crate::bindings_xml::AddonBinding;

use super::Model;

/// One host-registered command: the 1.12 name, its category header's global string
/// (`BINDING_HEADER_MOVEMENT`), whether it also runs on release (`runOnUp`, read only by
/// `RunCommand` at `0x4b7bf1`, never a bindability rule), and the 1.12 default chords.
#[derive(Clone, Copy, Debug)]
pub struct KeybindCommand {
    pub name: &'static str,
    pub category: &'static str,
    pub run_on_up: bool,
    pub default1: Option<&'static str>,
    pub default2: Option<&'static str>,
}

/// An addon binding's runnable half: what the app's dispatch needs to fire it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddonBindingBody {
    /// The command name as registered, the key of a `keybind_snapshot` row.
    pub name: String,
    /// Run the body a second time on the release, with `keystate = "up"` (`runOnUp="true"`).
    pub run_on_up: bool,
    /// The `<Binding>` element's Lua chunk, verbatim.
    pub body: String,
}

/// A host request queued by Lua. `Save(set)` persists set 1 or 2 under `benilla-config/bindings/`;
/// `Save(1)` also deletes the character set's file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeybindRequest {
    Save(u32),
    /// `RunBinding(command)`: the app runs the command's action as if its chord were pressed.
    Run(String),
}

/// `SetBinding`'s only refusal: the alias step of `CBindings::SetBinding` (`0x4b7490`), then
/// `IsValidBindingKeyString` (`0x4b7890`); no command is consulted. `Some` is the key to store,
/// upper-cased because the table's hash and compare fold case (`0x64b3f0`, `0x64a4c0`). The alias
/// step ignores case but the validator does not (`0x64a480`), so `"mousewheelup"` is refused.
pub fn normalize_binding_key(key: &str) -> Option<String> {
    // Eleven punctuation names become their character on a whole-string match only, so
    // `SHIFT-LEFTBRACKET` is refused where `SHIFT-[` passes.
    const ALIASES: [(&str, &str); 11] = [
        ("LEFTBRACKET", "["),
        ("RIGHTBRACKET", "]"),
        ("SLASH", "/"),
        ("BACKSLASH", "\\"),
        ("SEMICOLON", ";"),
        ("APOSTROPHE", "'"),
        ("COMMA", ","),
        ("PERIOD", "."),
        ("TILDE", "`"),
        ("PLUS", "="),
        ("MINUS", "-"),
    ];
    let key = ALIASES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map_or(key, |(_, literal)| literal);
    is_valid_binding_key(key).then(|| key.to_ascii_uppercase())
}

/// `IsValidBindingKeyString 0x4b7890`, arm for arm.
fn is_valid_binding_key(key: &str) -> bool {
    // 1. Modifier prefixes, in any order and any number of times (`0x4b78a0`-`0x4b78d0`).
    let mut rest = key;
    while let Some(next) = ["SHIFT-", "CTRL-", "ALT-"]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
    {
        rest = next;
    }
    // 2. One character or none passes (`0x4b78e6`): `SHIFT-` alone is a legal key.
    if rest.chars().count() <= 1 {
        return true;
    }
    // 3. `F`, `NUMPAD` or `BUTTON` (`0x846c04`) followed by digits only.
    for prefix in ["F", "NUMPAD", "BUTTON"] {
        if let Some(digits) = rest.strip_prefix(prefix) {
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                return true;
            }
        }
    }
    // 4. The 26 names at `0x846c20`-`0x846c84`: no lone modifier, `SCROLLLOCK` or `PAUSE`.
    const NAMES: [&str; 26] = [
        "SPACE",
        "NUMPADPLUS",
        "NUMPADMINUS",
        "NUMPADMULTIPLY",
        "NUMPADDIVIDE",
        "NUMPADDECIMAL",
        "ESCAPE",
        "ENTER",
        "BACKSPACE",
        "TAB",
        "LEFT",
        "UP",
        "RIGHT",
        "DOWN",
        "INSERT",
        "DELETE",
        "HOME",
        "END",
        "PAGEUP",
        "PAGEDOWN",
        "NUMLOCK",
        "CAPSLOCK",
        "PRINTSCREEN",
        "NUMPADEQUALS",
        "MOUSEWHEELDOWN",
        "MOUSEWHEELUP",
    ];
    NAMES.contains(&rest)
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    category: String,
    run_on_up: bool,
    defaults: [Option<String>; 2],
    /// The live chords in bind order; `GetBindingKey` and the window show the first two.
    keys: Vec<String>,
    /// An addon's `hidden="true"`: a full binding, left out of `GetNumBindings`/`GetBinding`.
    hidden: bool,
    /// An addon binding's Lua chunk; `None` for a host command, whose action is engine-side.
    body: Option<String>,
}

/// The table, the stored sets and the queued host requests; lives in [`Model`].
#[derive(Default)]
pub(crate) struct KeybindState {
    entries: Vec<Entry>,
    by_name: HashMap<String, usize>,
    /// Stored sets in entry order: `[0]` account (set 1), `[1]` character (set 2, if any).
    stored: [Option<Vec<Vec<String>>>; 2],
    /// The same sets by uppercased name, for addon commands registered after the seed.
    stored_by_name: [HashMap<String, Vec<String>>; 2],
    /// The set the live table edits (`GetCurrentBindingSet`): 1 account, 2 character.
    current_set: u32,
    /// Bumped by every change to the live table: the app's signal to re-derive dispatch.
    generation: u64,
    requests: Vec<KeybindRequest>,
    /// Set by `BenillaBindCapture`: the host swallows raw input and hands the page the chord.
    capture_armed: bool,
}

impl KeybindState {
    /// The rows `GetNumBindings`/`GetBinding` enumerate: all but `hidden="true"`, since the stock
    /// window skips only `HEADER` rows (`Blizzard_BindingUI.lua:87`).
    fn visible(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.hidden)
    }

    /// `SetBinding(key[, command])`: unbind `key` everywhere, then append it to the command's
    /// list. The reference refuses only an invalid key (`0x4b762c`) and never reads the command
    /// (`0x4b7490`), so the wheel binds to a `runOnUp` command despite the stock window's
    /// MOUSEWHEEL_ERROR branch.
    fn set_binding(&mut self, key: &str, command: Option<&str>) -> bool {
        let Some(key) = normalize_binding_key(key) else {
            return false;
        };
        // Deviation: an unregistered command answers nil where the reference stores the string
        // and answers 1, because this table only holds key lists of registered commands.
        let cmd_idx = match command {
            Some(name) => match self.by_name.get(&name.to_ascii_uppercase()) {
                Some(&i) => Some(i),
                None => return false,
            },
            None => None,
        };
        for e in &mut self.entries {
            e.keys.retain(|k| k != &key);
        }
        if let Some(i) = cmd_idx {
            self.entries[i].keys.push(key);
        }
        self.generation += 1;
        true
    }

    /// The live key lists in entry order, as a stored set holds them.
    fn snapshot(&self) -> Vec<Vec<String>> {
        self.entries.iter().map(|e| e.keys.clone()).collect()
    }

    /// Replace the live key lists from a snapshot; an entry past the snapshot's end keeps its keys.
    fn apply(&mut self, snap: &[Vec<String>]) {
        for (i, e) in self.entries.iter_mut().enumerate() {
            if let Some(keys) = snap.get(i) {
                e.keys = keys.clone();
            }
        }
        self.generation += 1;
    }

    fn defaults_snapshot(&self) -> Vec<Vec<String>> {
        self.entries
            .iter()
            .map(|e| e.defaults.iter().flatten().cloned().collect())
            .collect()
    }

    /// `LoadBindings(set)`: 0 defaults, 1 account, 2 character, which falls back to the account
    /// set (the reference's starts as a copy). Loading defaults keeps the current set, as the
    /// window's Reset To Default does.
    fn load(&mut self, set: u32) {
        match set {
            0 => {
                let d = self.defaults_snapshot();
                self.apply(&d);
            }
            1 => {
                let s = self.stored[0]
                    .clone()
                    .unwrap_or_else(|| self.defaults_snapshot());
                self.apply(&s);
                self.current_set = 1;
            }
            2 => {
                let s = self.stored[1]
                    .clone()
                    .or_else(|| self.stored[0].clone())
                    .unwrap_or_else(|| self.defaults_snapshot());
                self.apply(&s);
                self.current_set = 2;
            }
            _ => {}
        }
    }

    /// `SaveBindings(which)`: store the live table as set `which` and make it current. Saving the
    /// account set drops the character set, the window's confirmed delete.
    fn save(&mut self, which: u32) {
        if which != 1 && which != 2 {
            return;
        }
        self.stored[(which - 1) as usize] = Some(self.snapshot());
        if which == 1 {
            self.stored[1] = None;
        }
        self.current_set = which;
        self.requests.push(KeybindRequest::Save(which));
    }

    /// Append one addon's parsed `Bindings.xml`, skipping a name already registered, so a host
    /// command keeps it and no handed-out index moves. A `header` opens a section
    /// (`BINDING_HEADER_<header>`) until the next one, as in the reference's flat list.
    /// Deviation: rows before a file's first header take the addon's name as their section, not
    /// the previous file's, because under a foreign header the player could not find them.
    fn register_addon(&mut self, addon: &str, bindings: &[AddonBinding]) {
        let mut section = addon.to_string();
        for b in bindings {
            // A foreign `platform` skips the whole node, header included (`0x4b70c3`-`0x4b70e5`).
            if b.platform.as_deref().is_some_and(|p| p != THIS_PLATFORM) {
                continue;
            }
            // Before the duplicate skip: a duplicate's header still opens its section.
            if let Some(h) = &b.header {
                section = format!("BINDING_HEADER_{h}");
            }
            let key = b.name.to_ascii_uppercase();
            if self.by_name.contains_key(&key) {
                continue;
            }
            let idx = self.entries.len();
            // The stored chord: the current set first, then the account set, as `load` falls back.
            let stored = {
                let cur = if self.current_set == 2 { 1 } else { 0 };
                self.stored_by_name[cur]
                    .get(&key)
                    .or_else(|| self.stored_by_name[0].get(&key))
                    .cloned()
            };
            self.by_name.insert(key, idx);
            self.entries.push(Entry {
                name: b.name.clone(),
                category: section.clone(),
                run_on_up: b.run_on_up,
                // 1.12's `<Binding>` carries no default chord.
                defaults: [None, None],
                keys: stored.unwrap_or_default(),
                hidden: b.hidden,
                body: Some(b.body.clone()),
            });
        }
        self.generation += 1;
    }
}

/// The `platform="…"` value that registers here: the Windows client compares against `"windows"`
/// (`0x846f98`), the Mac client against `"mac"` (`0x4e28f8`); every other OS takes `"windows"`.
const THIS_PLATFORM: &str = if cfg!(target_os = "macos") {
    "mac"
} else {
    "windows"
};

impl super::UiScript {
    /// Register the host's commands at boot, in 1.12 `Bindings.xml` order, keys from the defaults.
    pub fn register_bindings(&mut self, commands: &[KeybindCommand]) {
        let mut model = self.model_mut();
        for c in commands {
            let key = c.name.to_ascii_uppercase();
            if model.keybinds.by_name.contains_key(&key) {
                continue;
            }
            let defaults = [c.default1.map(str::to_owned), c.default2.map(str::to_owned)];
            let idx = model.keybinds.entries.len();
            model.keybinds.by_name.insert(key, idx);
            model.keybinds.entries.push(Entry {
                name: c.name.to_owned(),
                category: c.category.to_owned(),
                run_on_up: c.run_on_up,
                keys: defaults.iter().flatten().cloned().collect(),
                defaults,
                hidden: false,
                body: None,
            });
        }
        model.keybinds.current_set = 1;
        model.keybinds.generation += 1;
    }

    /// Register one addon's parsed `Bindings.xml`, at the reference's point in the addon load:
    /// after its `.toc` files, before its saved variables (`0x51f400`).
    pub fn register_addon_bindings(&mut self, addon: &str, bindings: &[AddonBinding]) {
        self.model_mut().keybinds.register_addon(addon, bindings);
    }

    /// Every addon binding's runnable half, in registration order; host commands have none.
    pub fn addon_binding_bodies(&self) -> Vec<AddonBindingBody> {
        self.model_mut()
            .keybinds
            .entries
            .iter()
            .filter_map(|e| {
                e.body.as_ref().map(|body| AddonBindingBody {
                    name: e.name.clone(),
                    run_on_up: e.run_on_up,
                    body: body.clone(),
                })
            })
            .collect()
    }

    /// Seed stored set 1 (account) or 2 (character) from `benilla-config/bindings/`; `None` clears
    /// it. The live table is untouched until [`Self::load_binding_set`].
    pub fn seed_binding_set(&mut self, set: u32, keys: Option<Vec<(String, Vec<String>)>>) {
        let mut model = self.model_mut();
        let kb = &mut model.keybinds;
        // By name as well: an addon command registered later has no position yet.
        let mut by_name_owned: HashMap<String, Vec<String>> = HashMap::new();
        let snap = keys.map(|pairs| {
            let by_name: HashMap<_, _> = pairs.into_iter().collect();
            by_name_owned = by_name
                .iter()
                .map(|(n, k)| (n.to_ascii_uppercase(), k.clone()))
                .collect();
            kb.entries
                .iter()
                .map(|e| {
                    by_name
                        .get(&e.name)
                        .cloned()
                        .unwrap_or_else(|| e.keys.clone())
                })
                .collect()
        });
        match set {
            1 => {
                kb.stored[0] = snap;
                kb.stored_by_name[0] = by_name_owned;
            }
            2 => {
                kb.stored[1] = snap;
                kb.stored_by_name[1] = by_name_owned;
            }
            _ => {}
        }
    }

    /// Host-side `LoadBindings` (world entry: character set if it exists, else account).
    pub fn load_binding_set(&mut self, set: u32) {
        self.model_mut().keybinds.load(set);
    }

    /// The live table as `(command, chords)` in registration order, for dispatch and the save.
    pub fn keybind_snapshot(&self) -> Vec<(String, Vec<String>)> {
        self.model_mut()
            .keybinds
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.keys.clone()))
            .collect()
    }

    /// Bumped by every table change; re-derive dispatch when it moves.
    pub fn keybinds_generation(&self) -> u64 {
        self.model_mut().keybinds.generation
    }

    /// Drain the queued host requests.
    pub fn take_keybind_requests(&mut self) -> Vec<KeybindRequest> {
        std::mem::take(&mut self.model_mut().keybinds.requests)
    }

    /// Which set the live table edits: 1 account, 2 character.
    pub fn current_binding_set(&self) -> u32 {
        self.model_mut().keybinds.current_set
    }

    /// Whether a character-specific stored set exists (the window's checkbox).
    pub fn character_bindings_exist(&self) -> bool {
        self.model_mut().keybinds.stored[1].is_some()
    }

    /// While true, the host swallows raw input and calls `KeyBindings_OnHostKey("<chord>")`.
    pub fn bind_capture_armed(&self) -> bool {
        self.model_mut().keybinds.capture_armed
    }
}

/// Register one addon's `Bindings.xml` from a bare `&Lua`: `LoadAddOn` runs inside a Lua binding
/// with no `UiScript` to reach.
pub(crate) fn register_addon_bindings(lua: &Lua, addon: &str, bindings: &[AddonBinding]) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .keybinds
        .register_addon(addon, bindings);
}

/// The binding globals. `GetBinding` returns command, category, key1, key2, which is not 1.12's
/// shape: 1.12 returns command, key1, key2, with each header a `HEADER_*` row
/// (`Blizzard_BindingUI.lua:84`). Only `GetNumBindings` and `GetBinding` skip hidden rows.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "GetNumBindings",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(model.keybinds.visible().count())
        })?,
    )?;
    lua.globals().set(
        "GetBinding",
        lua.create_function(|lua, i: usize| {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            let Some(e) = i
                .checked_sub(1)
                .and_then(|i| model.keybinds.visible().nth(i))
            else {
                return Ok(MultiValue::new());
            };
            let mut out = vec![
                Value::String(lua.create_string(&e.name)?),
                Value::String(lua.create_string(&e.category)?),
            ];
            for k in e.keys.iter().take(2) {
                out.push(Value::String(lua.create_string(k)?));
            }
            Ok(MultiValue::from_iter(out))
        })?,
    )?;
    lua.globals().set(
        "SetBinding",
        lua.create_function(|lua, (key, command): (String, Option<String>)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let ok = model.keybinds.set_binding(&key, command.as_deref());
            Ok(if ok { Some(1) } else { None })
        })?,
    )?;
    // `RunBinding(command)`, as `CinematicFrame.xml:42` passes SCREENSHOT through: a host
    // command's action is engine-side, so this queues it and the app runs it in the same frame.
    lua.globals().set(
        "RunBinding",
        lua.create_function(|lua, command: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let name = command.to_ascii_uppercase();
            if model.keybinds.by_name.contains_key(&name) {
                model.keybinds.requests.push(KeybindRequest::Run(name));
            }
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "GetBindingKey",
        lua.create_function(|lua, command: String| {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            let kb = &model.keybinds;
            let mut out = Vec::new();
            if let Some(&i) = kb.by_name.get(&command.to_ascii_uppercase()) {
                for k in kb.entries[i].keys.iter().take(2) {
                    out.push(Value::String(lua.create_string(k)?));
                }
            }
            Ok(MultiValue::from_iter(out))
        })?,
    )?;
    lua.globals().set(
        "GetBindingAction",
        lua.create_function(|lua, key: String| {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            let key = key.to_ascii_uppercase();
            let name = model
                .keybinds
                .entries
                .iter()
                .find(|e| e.keys.iter().any(|k| k == &key))
                .map(|e| e.name.as_str())
                .unwrap_or("");
            lua.create_string(name)
        })?,
    )?;
    lua.globals().set(
        "LoadBindings",
        lua.create_function(|lua, set: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.keybinds.load(set);
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "SaveBindings",
        lua.create_function(|lua, which: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.keybinds.save(which);
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "GetCurrentBindingSet",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(model.keybinds.current_set)
        })?,
    )?;
    lua.globals().set(
        "BenillaCharacterBindingsExist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(model.keybinds.stored[1].is_some())
        })?,
    )?;
    lua.globals().set(
        "BenillaBindCapture",
        lua.create_function(|lua, armed: Option<bool>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.keybinds.capture_armed = armed.unwrap_or(false);
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::KeybindCommand;
    use crate::script::keybind::KeybindRequest;
    use crate::script::UiScript;

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.register_bindings(&[
            KeybindCommand {
                name: "MOVEFORWARD",
                category: "BINDING_HEADER_MOVEMENT",
                run_on_up: true,
                default1: Some("W"),
                default2: Some("UP"),
            },
            KeybindCommand {
                name: "JUMP",
                category: "BINDING_HEADER_MOVEMENT",
                run_on_up: false,
                default1: Some("SPACE"),
                default2: Some("NUMPAD0"),
            },
            KeybindCommand {
                name: "CAMERAZOOMIN",
                category: "BINDING_HEADER_CAMERA",
                run_on_up: false,
                default1: Some("MOUSEWHEELUP"),
                default2: None,
            },
        ]);
        s
    }

    #[test]
    fn the_table_reads_like_the_reference() {
        let s = script();
        assert_eq!(s.eval::<usize>("return GetNumBindings()").unwrap(), 3);
        // Command, category, key1, key2 (not 1.12's three values), the defaults seeded live.
        assert!(s
            .eval::<bool>(
                r#"local c, cat, k1, k2 = GetBinding(1)
                   return c == "MOVEFORWARD" and cat == "BINDING_HEADER_MOVEMENT"
                      and k1 == "W" and k2 == "UP""#
            )
            .unwrap());
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("NUMPAD0")"#)
                .unwrap(),
            "JUMP"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("F")"#).unwrap(),
            "",
            "an unbound key reads as the empty command, like the client"
        );
    }

    #[test]
    fn set_binding_steals_and_refuses_only_a_key_string_that_is_not_a_key() {
        let s = script();
        let g0 = s.keybinds_generation();
        // W leaves MOVEFORWARD (UP slides up) and joins the end of JUMP's list.
        assert!(s
            .eval::<bool>(r#"return SetBinding("W", "JUMP") == 1"#)
            .unwrap());
        assert!(s
            .eval::<bool>(
                r#"local k1, k2 = GetBindingKey("MOVEFORWARD"); return k1 == "UP" and k2 == nil"#
            )
            .unwrap());
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("W")"#).unwrap(),
            "JUMP"
        );
        s.run(r#"SetBinding("SPACE")"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("SPACE")"#)
                .unwrap(),
            ""
        );
        // The wheel binds to a `runOnUp` command: `0x4b7490` never reads the command.
        assert!(s
            .eval::<bool>(r#"return SetBinding("SHIFT-MOUSEWHEELUP", "MOVEFORWARD") == 1"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return SetBinding("MOUSEWHEELDOWN", "CAMERAZOOMIN") == 1"#)
            .unwrap());
        // Refused by `IsValidBindingKeyString` (`0x4b7890`), on the pure unbind too.
        for bad in ["SHIFT", "ALT", "UNKNOWN", "SCROLLLOCK", "SHIFT-LEFTBRACKET"] {
            assert!(
                s.eval::<bool>(&format!(r#"return SetBinding("{bad}", "JUMP") == nil"#))
                    .unwrap(),
                "{bad} is not a bindable key string"
            );
            assert!(
                s.eval::<bool>(&format!(r#"return SetBinding("{bad}") == nil"#))
                    .unwrap(),
                "{bad} fails the same validator on the pure unbind"
            );
        }
        for good in [
            "SHIFT-CTRL-ALT-K",
            "CTRL-SHIFT-K",
            "-",
            "BUTTON5",
            "NUMPAD7",
            "F11",
            "NUMPADEQUALS",
            "PRINTSCREEN",
        ] {
            assert!(
                s.eval::<bool>(&format!(r#"return SetBinding("{good}", "JUMP") == 1"#))
                    .unwrap(),
                "{good} is a bindable key string"
            );
        }
        assert!(s
            .eval::<bool>(r#"return SetBinding("LEFTBRACKET", "JUMP") == 1"#)
            .unwrap());
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("[")"#).unwrap(),
            "JUMP",
            "the alias is normalized to its literal character before it is stored"
        );
        // The validator is case-sensitive: a lower-case key string is refused.
        assert!(s
            .eval::<bool>(r#"return SetBinding("shift-w", "JUMP") == nil"#)
            .unwrap());
        assert!(
            s.keybinds_generation() > g0,
            "mutations bump the generation"
        );
    }

    #[test]
    fn the_three_set_model_loads_saves_and_deletes_like_the_reference() {
        let mut s = script();
        assert_eq!(s.current_binding_set(), 1);
        assert!(!s.character_bindings_exist());
        s.run(r#"SetBinding("F", "JUMP")"#).unwrap();
        s.run("SaveBindings(2)").unwrap();
        assert_eq!(s.current_binding_set(), 2);
        assert!(s.character_bindings_exist());
        assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(2)]);
        // Reset To Default keeps the character set current.
        s.run("LoadBindings(0)").unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("F")"#).unwrap(),
            ""
        );
        assert_eq!(s.current_binding_set(), 2);
        // Cancel reloads the saved character set.
        s.run("LoadBindings(2)").unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("F")"#).unwrap(),
            "JUMP"
        );
        // Saving the account set drops the character set, the confirmed delete.
        s.run("SaveBindings(1)").unwrap();
        assert_eq!(s.current_binding_set(), 1);
        assert!(!s.character_bindings_exist());
        assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    }

    #[test]
    fn an_addons_bindings_xml_registers_as_ordinary_rows() {
        let mut s = script();
        let parsed = crate::bindings_xml::parse(
            r#"<Bindings>
                <Binding name="PROBEHOLD" runOnUp="true" header="PROBE">
                    if ( keystate == "down" ) then Down(); else Up(); end
                </Binding>
                <Binding name="PROBEEDGE">Edge();</Binding>
                <Binding name="PROBEHIDDEN" hidden="true">Hidden();</Binding>
                <Binding name="MOVEFORWARD">Hijack();</Binding>
            </Bindings>"#,
        )
        .expect("well-formed");
        let g0 = s.keybinds_generation();
        s.register_addon_bindings("ProbeAddon", &parsed);
        assert!(
            s.keybinds_generation() > g0,
            "registration must move the generation — it is the app's re-derive-dispatch signal"
        );

        // Three host rows and two addon rows: the addon's `MOVEFORWARD` is skipped and
        // `PROBEHIDDEN` is not listed.
        assert_eq!(s.eval::<usize>("return GetNumBindings()").unwrap(), 5);
        assert!(s
            .eval::<bool>(
                r#"local c, cat, k1 = GetBinding(4)
                   return c == "PROBEHOLD" and cat == "BINDING_HEADER_PROBE" and k1 == nil"#
            )
            .unwrap());
        assert!(
            s.eval::<bool>(
                r#"local c, cat = GetBinding(5)
                   return c == "PROBEEDGE" and cat == "BINDING_HEADER_PROBE""#
            )
            .unwrap(),
            "a row with no header of its own belongs to the section the last header opened"
        );
        assert!(s
            .eval::<bool>(
                r#"local k1, k2 = GetBindingKey("MOVEFORWARD"); return k1 == "W" and k2 == "UP""#
            )
            .unwrap());

        // The hidden row is not enumerated, but binds and is found by key.
        assert!(s
            .eval::<bool>(
                r#"for i = 1, GetNumBindings() do
                       if GetBinding(i) == "PROBEHIDDEN" then return false end
                   end
                   return true"#
            )
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return SetBinding("H", "PROBEHIDDEN") == 1"#)
            .unwrap());
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("H")"#).unwrap(),
            "PROBEHIDDEN"
        );
        // `runOnUp` never refuses a wheel chord.
        assert!(s
            .eval::<bool>(r#"return SetBinding("MOUSEWHEELUP", "PROBEHOLD") == 1"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return SetBinding("MOUSEWHEELDOWN", "PROBEEDGE") == 1"#)
            .unwrap());

        // A file with no header: its rows' section is the addon's name.
        let parsed = crate::bindings_xml::parse(
            r#"<Bindings><Binding name="LIBKEY">Lib();</Binding></Bindings>"#,
        )
        .expect("well-formed");
        s.register_addon_bindings("ProbeLib", &parsed);
        assert!(s
            .eval::<bool>(
                r#"local c, cat = GetBinding(6); return c == "LIBKEY" and cat == "ProbeLib""#
            )
            .unwrap());

        // Addon rows only, in registration order, hidden included.
        let bodies = s.addon_binding_bodies();
        let names: Vec<&str> = bodies.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["PROBEHOLD", "PROBEEDGE", "PROBEHIDDEN", "LIBKEY"]);
        assert!(bodies[0].run_on_up && !bodies[1].run_on_up);
        assert!(bodies[0].body.contains(r#"keystate == "down""#));
    }

    #[test]
    fn a_binding_for_another_platform_does_not_register() {
        let mut s = script();
        let parsed = crate::bindings_xml::parse(
            r#"<Bindings>
                <Binding name="PROBEHERE" platform="THIS">Here();</Binding>
                <Binding name="PROBEELSEWHERE" platform="elsewhere">Elsewhere();</Binding>
                <Binding name="PROBEANYWHERE">Anywhere();</Binding>
            </Bindings>"#
                .replace("THIS", super::THIS_PLATFORM)
                .as_str(),
        )
        .unwrap();
        s.register_addon_bindings("Probe", &parsed);

        let names: Vec<String> = s.keybind_snapshot().into_iter().map(|(n, _)| n).collect();
        assert!(names.iter().any(|n| n == "PROBEHERE"), "{names:?}");
        assert!(names.iter().any(|n| n == "PROBEANYWHERE"), "{names:?}");
        assert!(
            !names.iter().any(|n| n == "PROBEELSEWHERE"),
            "a foreign-platform row registered: {names:?}"
        );
        assert!(!s
            .addon_binding_bodies()
            .iter()
            .any(|b| b.name == "PROBEELSEWHERE"));
    }

    #[test]
    fn host_seeding_feeds_load_and_the_capture_arm_reads_back() {
        let mut s = script();
        s.seed_binding_set(1, Some(vec![("JUMP".into(), vec!["F".into()])]));
        s.load_binding_set(1);
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("F")"#).unwrap(),
            "JUMP"
        );
        // A command the seed lacks keeps its live keys.
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("W")"#).unwrap(),
            "MOVEFORWARD"
        );
        assert!(!s.bind_capture_armed());
        s.run("BenillaBindCapture(true)").unwrap();
        assert!(s.bind_capture_armed());
        s.run("BenillaBindCapture(false)").unwrap();
        assert!(!s.bind_capture_armed());
        let snap = s.keybind_snapshot();
        assert_eq!(snap[1].0, "JUMP");
        assert_eq!(snap[1].1, vec!["F".to_string()]);
    }
}

#[cfg(test)]
mod addon_persistence_tests {
    use crate::bindings_xml::AddonBinding;
    use crate::script::keybind::KeybindCommand;
    use crate::script::UiScript;

    fn binding(name: &str) -> AddonBinding {
        AddonBinding {
            name: name.into(),
            header: None,
            run_on_up: false,
            hidden: false,
            platform: None,
            body: "AddonBindingRan = true".into(),
        }
    }

    /// The stored set is seeded at boot, before the addon's `Bindings.xml` registers at world
    /// entry, so its row has no position to land in.
    #[test]
    fn an_addon_binding_restores_its_stored_chord_registered_after_the_seed() {
        let mut s = UiScript::new().unwrap();
        s.register_bindings(&[KeybindCommand {
            name: "JUMP",
            category: "BINDING_HEADER_MOVEMENT",
            run_on_up: false,
            default1: Some("SPACE"),
            default2: None,
        }]);
        s.seed_binding_set(
            1,
            Some(vec![
                ("JUMP".into(), vec!["SPACE".into()]),
                ("MYADDONTOGGLE".into(), vec!["CTRL-X".into()]),
            ]),
        );
        s.load_binding_set(1);

        s.register_addon_bindings("MyAddon", &[binding("MYADDONTOGGLE")]);

        let bound = s
            .keybind_snapshot()
            .into_iter()
            .find(|(n, _)| n == "MYADDONTOGGLE")
            .expect("the addon's command is in the table");
        assert_eq!(
            bound.1,
            vec!["CTRL-X".to_string()],
            "the stored chord came back — this is the assertion 1192 §4 could not make"
        );
    }

    #[test]
    fn an_addon_binding_with_no_stored_chord_registers_unbound() {
        let mut s = UiScript::new().unwrap();
        s.seed_binding_set(1, Some(vec![("SOMETHINGELSE".into(), vec!["Q".into()])]));
        s.load_binding_set(1);
        s.register_addon_bindings("MyAddon", &[binding("MYADDONTOGGLE")]);
        let bound = s
            .keybind_snapshot()
            .into_iter()
            .find(|(n, _)| n == "MYADDONTOGGLE")
            .expect("registered");
        assert!(bound.1.is_empty());
    }

    #[test]
    fn the_character_set_wins_for_an_addon_command() {
        let mut s = UiScript::new().unwrap();
        s.seed_binding_set(
            1,
            Some(vec![("MYADDONTOGGLE".into(), vec!["CTRL-X".into()])]),
        );
        s.seed_binding_set(
            2,
            Some(vec![("MYADDONTOGGLE".into(), vec!["ALT-Z".into()])]),
        );
        s.load_binding_set(2);
        s.register_addon_bindings("MyAddon", &[binding("MYADDONTOGGLE")]);
        let bound = s
            .keybind_snapshot()
            .into_iter()
            .find(|(n, _)| n == "MYADDONTOGGLE")
            .expect("registered");
        assert_eq!(bound.1, vec!["ALT-Z".to_string()]);
    }
}
