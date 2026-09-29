//! The key-binding table behind `GetBinding`, `SetBinding`, `LoadBindings` and their kin, in the
//! reference's two halves. The commands are what `Bindings.xml` declared (the core's off the
//! player's chain, then each addon's, through the one loader `0x4b6f70`): a name, its Lua body,
//! `runOnUp`, and the one flat list `GetBinding` walks, whose section headers are rows of their
//! own. The keys are a separate table of chord to command name, spelled
//! `[ALT-][CTRL-][SHIFT-]<TOKEN>` and matched by string equality, which `SetBinding` writes without
//! reading the command (`0x4b7490`): the live set the dispatcher probes, the stored account and
//! character sets, and the defaults `WTF\DefaultBindings.wtf` fills (`0x4b62b0`).

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use crate::bindings_xml::Binding;

use super::Model;

/// A host request queued by Lua. `Save(set)` persists set 1 or 2 under `benilla-config/bindings/`;
/// `Save(1)` also deletes the character set's file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeybindRequest {
    Save(u32),
}

/// One key of a set: the chord and the command name it runs, as `SetBinding` stored it.
pub type KeyBinding = (String, String);

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

/// One command a `Bindings.xml` declared.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    /// `runOnUp="true"`: the release runs the body again (`0x4b7bf1`).
    run_on_up: bool,
    /// `hidden="true"`: a full binding, left out of `GetNumBindings`/`GetBinding`.
    hidden: bool,
    /// The `<Binding>` element's Lua chunk, verbatim.
    body: String,
}

/// One row of the list `GetNumBindings`/`GetBinding` walk, in load order.
#[derive(Clone, Debug)]
enum Row {
    /// A section header, `HEADER_<header>`: the reference stores `sprintf("HEADER_%s", header)`
    /// (`0x846f58`) as a row of its own with a display ordinal and no script (`0x4b72f5`-`0x4b72fa`).
    Header(String),
    /// A command, an index into [`KeybindState::entries`].
    Command(usize),
}

/// The commands, the key sets and the queued host requests; lives in [`Model`].
#[derive(Default)]
pub(crate) struct KeybindState {
    entries: Vec<Entry>,
    /// The display list, headers and commands in load order.
    rows: Vec<Row>,
    by_name: HashMap<String, usize>,
    /// The live keys the dispatcher probes, in bind order: one command per chord, and a command's
    /// keys in the order `GetBindingKey` answers them.
    live: Vec<KeyBinding>,
    /// Set 0, the defaults `LoadBindings(0)` restores, in file order.
    defaults: Vec<KeyBinding>,
    /// Stored sets: `[0]` account (set 1), `[1]` character (set 2, if any).
    stored: [Option<Vec<KeyBinding>>; 2],
    /// The set the live table edits (`GetCurrentBindingSet`): 1 account, 2 character.
    current_set: u32,
    /// Bumped by every change to the live table or the commands: the app's signal to re-derive
    /// dispatch.
    generation: u64,
    requests: Vec<KeybindRequest>,
}

/// `SetBinding(set, key, command)` on one key list: unbind `key` everywhere, then bind it to the
/// command unless that is empty (`0x4b7490`, where an empty action is the unbind). The key is one
/// [`normalize_binding_key`] accepted.
fn bind_key(list: &mut Vec<KeyBinding>, key: String, command: Option<&str>) {
    list.retain(|(k, _)| k != &key);
    if let Some(command) = command.filter(|c| !c.is_empty()) {
        list.push((key, command.to_owned()));
    }
}

/// A list of `(key, command)` pairs applied in order through [`bind_key`], a key the validator
/// refuses dropped: how the `.wtf` loader (`0x4b6140`) fills a set, line by line.
fn key_list(pairs: impl IntoIterator<Item = KeyBinding>) -> Vec<KeyBinding> {
    let mut list = Vec::new();
    for (key, command) in pairs {
        if let Some(key) = normalize_binding_key(&key) {
            bind_key(&mut list, key, Some(&command));
        }
    }
    list
}

/// A `.wtf` bindings file as the reference's reader (`0x4b6140`) takes it: lines split on CR and
/// LF, a line counted only when it opens with `bind ` in any case (`0x4b61ea`), then the key up to
/// the next space and the rest of the line as the command (`0x4b620d`), handed to `SetBinding`.
pub fn parse_bindings_wtf(text: &str) -> Vec<KeyBinding> {
    text.split(['\r', '\n'])
        .filter_map(|line| {
            let rest = line
                .get(..5)
                .filter(|p| p.eq_ignore_ascii_case("bind "))
                .map(|_| &line[5..])?;
            let (key, command) = rest.split_once(' ').unwrap_or((rest, ""));
            Some((key.to_owned(), command.to_owned()))
        })
        .collect()
}

impl KeybindState {
    /// The rows `GetNumBindings`/`GetBinding` enumerate: every header and every command but a
    /// `hidden="true"` one, which the reference numbers from a second counter, negated
    /// (`0x4b73da`-`0x4b73e5`), so `GetBinding`'s ordinal walk (`0x4b7c80`) never reaches it and
    /// `GetNumBindings` (`0x4b7f40`) counts only the first.
    fn visible(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| match r {
            Row::Header(_) => true,
            Row::Command(i) => !self.entries[*i].hidden,
        })
    }

    /// The live keys bound to `command`, in bind order; names compare case-insensitively
    /// (`0x64a4c0`).
    fn keys_of<'a>(&'a self, command: &'a str) -> impl Iterator<Item = &'a String> + 'a {
        self.live
            .iter()
            .filter(move |(_, c)| c.eq_ignore_ascii_case(command))
            .map(|(k, _)| k)
    }

    /// Open a section: a `HEADER_<header>` row, unless one of that name exists, which the reference
    /// refuses (`0x4b71c9`-`0x4b7258`) and loads the binding under the section already open.
    fn open_header(&mut self, header: &str) {
        let name = format!("HEADER_{header}");
        if !self
            .rows
            .iter()
            .any(|r| matches!(r, Row::Header(h) if h.eq_ignore_ascii_case(&name)))
        {
            self.rows.push(Row::Header(name));
        }
    }

    /// `SetBinding(key[, command])` on the live set: refused only for a key string the validator
    /// rejects (`0x4b762c`). The command is never read (`0x4b7490`), so a name no `Bindings.xml`
    /// declared is stored and answered like any other.
    fn set_binding(&mut self, key: &str, command: Option<&str>) -> bool {
        let Some(key) = normalize_binding_key(key) else {
            return false;
        };
        bind_key(&mut self.live, key, command);
        self.generation += 1;
        true
    }

    /// `LoadBindings(set)`: 0 defaults, 1 account, 2 character, which falls back to the account
    /// set (the reference's starts as a copy), each falling back to the defaults. Loading defaults
    /// keeps the current set, as the window's Reset To Default does.
    fn load(&mut self, set: u32) {
        let list = match set {
            0 => &self.defaults,
            1 => self.stored[0].as_ref().unwrap_or(&self.defaults),
            2 => self.stored[1]
                .as_ref()
                .or(self.stored[0].as_ref())
                .unwrap_or(&self.defaults),
            _ => return,
        };
        self.live = list.clone();
        if set != 0 {
            self.current_set = set;
        }
        self.generation += 1;
    }

    /// `SaveBindings(which)`: store the live table as set `which` and make it current. Saving the
    /// account set drops the character set, the window's confirmed delete.
    fn save(&mut self, which: u32) {
        if which != 1 && which != 2 {
            return;
        }
        self.stored[(which - 1) as usize] = Some(self.live.clone());
        if which == 1 {
            self.stored[1] = None;
        }
        self.current_set = which;
        self.requests.push(KeybindRequest::Save(which));
    }

    /// Append one parsed `Bindings.xml`, as the loader `0x4b6f70` does: a foreign `platform` skips
    /// the node (`0x4b70c3`-`0x4b70e5`); a name already defined skips it before its header is
    /// read (`0x4b70f1`-`0x4b717e`), so the first definition keeps it and no handed-out index
    /// moves; a `header` adds its `HEADER_<header>` row before the binding's own.
    ///
    /// `fallback_header` is an addon's name. Deviation: an addon file whose first binding has no
    /// header opens a `HEADER_<addon>` row, where the reference files those rows under the
    /// previous file's last header, because under a foreign header the player could not find
    /// them. The core file passes `None` and is read as the reference reads it.
    fn register(&mut self, fallback_header: Option<&str>, bindings: &[Binding]) {
        let mut opened = false;
        for b in bindings {
            if b.platform.as_deref().is_some_and(|p| p != THIS_PLATFORM) {
                continue;
            }
            let key = b.name.to_ascii_uppercase();
            if self.by_name.contains_key(&key) {
                continue;
            }
            match (&b.header, fallback_header) {
                (Some(h), _) => self.open_header(h),
                (None, Some(addon)) if !opened => self.open_header(addon),
                _ => {}
            }
            opened = true;
            let idx = self.entries.len();
            self.by_name.insert(key, idx);
            self.entries.push(Entry {
                name: b.name.clone(),
                run_on_up: b.run_on_up,
                hidden: b.hidden,
                body: b.body.clone(),
            });
            self.rows.push(Row::Command(idx));
        }
        self.generation += 1;
    }
}

/// The `platform="…"` value that registers: `"windows"` (`0x846f98`), the PC build's, on every OS,
/// since benilla answers as the PC build (`IsMacClient` nil, `client.rs`); the Mac build compares
/// against `"mac"` (`0x4e28f8`) and so has the `ITUNES_*` rows.
const THIS_PLATFORM: &str = "windows";

impl super::UiScript {
    /// Register the core's `Interface\FrameXML\Bindings.xml`, at the reference's point in the UI
    /// load: after the `FrameXML.toc` walk (`0x48ffed`), before the addons (`0x490018`).
    pub fn register_bindings(&self, bindings: &[Binding]) {
        self.model_mut().keybinds.register(None, bindings);
    }

    /// Register one addon's parsed `Bindings.xml`, at the reference's point in the addon load:
    /// after its `.toc` files, before its saved variables (`0x51f443`).
    pub fn register_addon_bindings(&mut self, addon: &str, bindings: &[Binding]) {
        self.model_mut().keybinds.register(Some(addon), bindings);
    }

    /// Set 0, the defaults, from `WTF\DefaultBindings.wtf` ([`parse_bindings_wtf`]), as
    /// `0x4b62b0` loads it: each pair through `SetBinding(0, key, command)`.
    pub fn set_default_bindings(&mut self, pairs: Vec<KeyBinding>) {
        self.model_mut().keybinds.defaults = key_list(pairs);
    }

    /// Set 0, the defaults, in file order.
    pub fn default_bindings(&self) -> Vec<KeyBinding> {
        self.model_mut().keybinds.defaults.clone()
    }

    /// Seed stored set 1 (account) or 2 (character) from `benilla-config/bindings/`; `None` clears
    /// it. The live table is untouched until [`Self::load_binding_set`].
    pub fn seed_binding_set(&mut self, set: u32, keys: Option<Vec<KeyBinding>>) {
        let keys = keys.map(key_list);
        let mut model = self.model_mut();
        match set {
            1 => model.keybinds.stored[0] = keys,
            2 => model.keybinds.stored[1] = keys,
            _ => {}
        }
    }

    /// Host-side `LoadBindings` (world entry: character set if it exists, else account).
    pub fn load_binding_set(&mut self, set: u32) {
        self.model_mut().keybinds.load(set);
    }

    /// The live keys as `(chord, command)` in bind order, for dispatch.
    pub fn binding_keys(&self) -> Vec<KeyBinding> {
        self.model_mut().keybinds.live.clone()
    }

    /// The live table as `(command, chords)`: every declared command in registration order, then
    /// any other name a key is bound to, for the save and the tests.
    pub fn keybind_snapshot(&self) -> Vec<(String, Vec<String>)> {
        let model = self.model_mut();
        let kb = &model.keybinds;
        let mut out: Vec<(String, Vec<String>)> = kb
            .entries
            .iter()
            .map(|e| (e.name.clone(), kb.keys_of(&e.name).cloned().collect()))
            .collect();
        for (key, command) in &kb.live {
            if kb.by_name.contains_key(&command.to_ascii_uppercase()) {
                continue;
            }
            match out
                .iter_mut()
                .find(|(n, _)| n.eq_ignore_ascii_case(command))
            {
                Some((_, keys)) => keys.push(key.clone()),
                None => out.push((command.clone(), vec![key.clone()])),
            }
        }
        out
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
}

/// Register one addon's `Bindings.xml` from a bare `&Lua`: `LoadAddOn` runs inside a Lua binding
/// with no `UiScript` to reach.
pub(crate) fn register_addon_bindings(lua: &Lua, addon: &str, bindings: &[Binding]) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .keybinds
        .register(Some(addon), bindings);
}

/// `CBindings::RunCommand` (`0x4b7b50`): look the command up by name (`0x4b7b77`); on a release
/// run nothing unless it is `runOnUp` (`0x4b7bf1`); else set the global `keystate` to `"down"` or
/// `"up"` (`0x4b7c0f`), run the body, and set `keystate` to nil (`0x4b7c42`-`0x4b7c4e`), since the
/// 1.12 client's in-world `_G` has none (`reference/1.12-globals.tsv`). `Ok(false)` when nothing
/// ran.
pub(crate) fn run_command(lua: &Lua, command: &str, down: bool) -> mlua::Result<bool> {
    let (name, body) = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let kb = &model.keybinds;
        let Some(&i) = kb.by_name.get(&command.to_ascii_uppercase()) else {
            return Ok(false);
        };
        let e = &kb.entries[i];
        if !down && !e.run_on_up {
            return Ok(false);
        }
        (e.name.clone(), e.body.clone())
    };
    let g = lua.globals();
    g.set("keystate", if down { "down" } else { "up" })?;
    let ran = lua
        .load(body.as_str())
        .set_name(name)
        .set_mode(mlua::ChunkMode::Text)
        .exec();
    g.set("keystate", Value::Nil)?;
    ran.map(|()| true)
}

/// The binding globals. `GetBinding(i)` answers the row's name and then every key bound to it
/// (`0x4b7f60`: `1 + matches` values), so a `HEADER_*` row answers its name alone, which the stock
/// window reads as three names (`Blizzard_BindingUI.lua:84`, `:87`). Only `GetNumBindings` and
/// `GetBinding` skip hidden rows.
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
            let kb = &model.keybinds;
            let Some(row) = i.checked_sub(1).and_then(|i| kb.visible().nth(i)) else {
                return Ok(MultiValue::new());
            };
            let mut out = Vec::new();
            match row {
                Row::Header(h) => out.push(Value::String(lua.create_string(h)?)),
                Row::Command(c) => {
                    let name = &kb.entries[*c].name;
                    out.push(Value::String(lua.create_string(name)?));
                    for k in kb.keys_of(name) {
                        out.push(Value::String(lua.create_string(k)?));
                    }
                }
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
    // `RunBinding(command[, "up"])` (`0x4b8180`): the command's body through `RunCommand`
    // (`0x4b81e0`), a release when the second argument is `"up"` in any case (`0x4b81c5`), else a
    // press. Not a hardware event, so the movement functions' gate refuses unless a key's own
    // body is already running.
    lua.globals().set(
        "RunBinding",
        lua.create_function(|lua, (command, state): (Value, Option<Value>)| {
            let command =
                super::binding_abi::string_arg(lua, command, "Usage: RunBinding(\"COMMAND\")")?;
            let up = state
                .as_ref()
                .and_then(|v| super::binding_abi::optional_string(lua, v))
                .is_some_and(|s| s.eq_ignore_ascii_case("up"));
            run_command(lua, &command, !up)?;
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "GetBindingKey",
        lua.create_function(|lua, command: String| {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            let kb = &model.keybinds;
            let mut out = Vec::new();
            for k in kb.keys_of(&command).take(2) {
                out.push(Value::String(lua.create_string(k)?));
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
                .live
                .iter()
                .find(|(k, _)| k == &key)
                .map(|(_, c)| c.as_str())
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_bindings_wtf, KeybindRequest};
    use crate::bindings_xml::parse;
    use crate::script::UiScript;

    /// Three commands in the reference's shape, bodies counting their runs, and their defaults.
    const CORE: &str = r#"<Bindings>
        <Binding name="MOVEFORWARD" runOnUp="true" header="MOVEMENT">
            FWD_LAST = keystate
        </Binding>
        <Binding name="JUMP">JUMPS = (JUMPS or 0) + 1</Binding>
        <Binding name="CAMERAZOOMIN" header="CAMERA">ZOOMS = (ZOOMS or 0) + 1</Binding>
    </Bindings>"#;
    const DEFAULTS: &str = "bind W MOVEFORWARD\r\nbind UP MOVEFORWARD\r\nbind SPACE JUMP\r\n\
                            bind NUMPAD0 JUMP\r\nbind MOUSEWHEELUP CAMERAZOOMIN\r\n";

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_default_bindings(parse_bindings_wtf(DEFAULTS));
        s.load_binding_set(1);
        s.register_bindings(&parse(CORE).unwrap());
        s
    }

    /// Every row `GetBinding` answers, each as its values in order.
    fn rows(s: &UiScript) -> Vec<Vec<String>> {
        s.eval(
            "local out = {} \
             for i = 1, GetNumBindings() do out[i] = { GetBinding(i) } end \
             return out",
        )
        .unwrap()
    }

    fn row(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn the_list_reads_like_the_reference() {
        let s = script();
        // A header row answers its name alone; a command, its name and then every key, no
        // category (`0x4b7f60`).
        assert_eq!(
            rows(&s),
            [
                row(&["HEADER_MOVEMENT"]),
                row(&["MOVEFORWARD", "W", "UP"]),
                row(&["JUMP", "SPACE", "NUMPAD0"]),
                row(&["HEADER_CAMERA"]),
                row(&["CAMERAZOOMIN", "MOUSEWHEELUP"]),
            ]
        );
        // Every key, not two.
        s.run(r#"SetBinding("F", "JUMP")"#).unwrap();
        assert_eq!(rows(&s)[2], row(&["JUMP", "SPACE", "NUMPAD0", "F"]));
        assert!(s
            .eval::<bool>("return GetBinding(0) == nil and GetBinding(6) == nil")
            .unwrap());
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("NUMPAD0")"#)
                .unwrap(),
            "JUMP"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("G")"#).unwrap(),
            "",
            "an unbound key reads as the empty command, like the client"
        );
    }

    /// The core file is read as the reference reads it: a first row with no header files under
    /// none, where an addon's opens a row of its own name (the deviation on `register`).
    #[test]
    fn the_core_file_opens_no_header_of_its_own() {
        let s = UiScript::new().unwrap();
        s.register_bindings(
            &parse(r#"<Bindings><Binding name="A">x()</Binding></Bindings>"#).unwrap(),
        );
        assert_eq!(rows(&s), [row(&["A"])]);
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
        // The command is never read (`0x4b7490`): a name no file declared is stored and
        // answered.
        assert!(s
            .eval::<bool>(r#"return SetBinding("CTRL-Y", "TOGGLESTATS") == 1"#)
            .unwrap());
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("CTRL-Y")"#)
                .unwrap(),
            "TOGGLESTATS"
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
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("W")"#).unwrap(),
            "MOVEFORWARD"
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
        let parsed = parse(
            r#"<Bindings>
                <Binding name="PROBEHOLD" runOnUp="true" header="PROBE">
                    PROBE_LAST = keystate
                </Binding>
                <Binding name="PROBEEDGE">PROBE_EDGE = 1</Binding>
                <Binding name="PROBEHIDDEN" hidden="true">Hidden();</Binding>
                <Binding name="MOVEFORWARD">HIJACKED = 1</Binding>
            </Bindings>"#,
        )
        .expect("well-formed");
        let g0 = s.keybinds_generation();
        s.register_addon_bindings("ProbeAddon", &parsed);
        assert!(
            s.keybinds_generation() > g0,
            "registration must move the generation — it is the app's re-derive-dispatch signal"
        );

        // Its header row, then two commands: the addon's `MOVEFORWARD` is skipped and
        // `PROBEHIDDEN` is not listed.
        assert_eq!(s.eval::<usize>("return GetNumBindings()").unwrap(), 8);
        assert_eq!(
            rows(&s)[5..],
            [
                row(&["HEADER_PROBE"]),
                row(&["PROBEHOLD"]),
                row(&["PROBEEDGE"])
            ]
        );
        assert!(s
            .eval::<bool>(
                r#"local k1, k2 = GetBindingKey("MOVEFORWARD"); return k1 == "W" and k2 == "UP""#
            )
            .unwrap());
        assert!(s.execute_binding("MOVEFORWARD", true).unwrap());
        assert!(
            s.eval::<bool>("return HIJACKED == nil").unwrap(),
            "the first definition keeps the name"
        );

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

        // A duplicate's header is never read (`0x4b70f1`-`0x4b717e`); a header already in the
        // list is refused and the row files under the open one (`0x4b71c9`-`0x4b7258`); a hidden
        // row's header is a row, since the header is written before the hidden test (`0x4b73bc`).
        let parsed = parse(
            r#"<Bindings>
                <Binding name="JUMP" header="DUPLICATE">Dup();</Binding>
                <Binding name="PROBEAGAIN" header="PROBE">Again();</Binding>
                <Binding name="PROBEHIDDENHEAD" header="QUIET" hidden="true">Quiet();</Binding>
            </Bindings>"#,
        )
        .expect("well-formed");
        s.register_addon_bindings("ProbeMore", &parsed);
        assert_eq!(
            rows(&s)[8..],
            [row(&["PROBEAGAIN"]), row(&["HEADER_QUIET"])]
        );

        // A file with no header opens a row of the addon's name (the deviation on `register`).
        let parsed = parse(r#"<Bindings><Binding name="LIBKEY">Lib();</Binding></Bindings>"#)
            .expect("well-formed");
        s.register_addon_bindings("ProbeLib", &parsed);
        assert_eq!(
            rows(&s)[10..],
            [row(&["HEADER_ProbeLib"]), row(&["LIBKEY"])]
        );
    }

    /// The PC build's filter on every OS (`0x4b70c3`-`0x4b70e5` against `"windows"`), so the
    /// stock `platform="mac"` iTunes rows are never commands.
    #[test]
    fn a_binding_for_another_platform_does_not_register() {
        let mut s = script();
        let parsed = parse(
            r#"<Bindings>
                <Binding name="PROBEHERE" platform="Windows">Here();</Binding>
                <Binding name="PROBEMAC" platform="mac" header="ITUNES_REMOTE">Mac();</Binding>
                <Binding name="PROBEANYWHERE">Anywhere();</Binding>
            </Bindings>"#,
        )
        .unwrap();
        s.register_addon_bindings("Probe", &parsed);

        let names: Vec<String> = s.keybind_snapshot().into_iter().map(|(n, _)| n).collect();
        assert!(names.iter().any(|n| n == "PROBEHERE"), "{names:?}");
        assert!(names.iter().any(|n| n == "PROBEANYWHERE"), "{names:?}");
        assert!(
            !names.iter().any(|n| n == "PROBEMAC"),
            "a foreign-platform row registered: {names:?}"
        );
        assert!(
            !rows(&s).contains(&row(&["HEADER_ITUNES_REMOTE"])),
            "the skipped node's header is never read"
        );
    }

    /// A stored set is the whole key table, as the reference's `bindings-cache.wtf` is: loading it
    /// replaces the live keys, and a key it lacks is unbound.
    #[test]
    fn host_seeding_feeds_load() {
        let mut s = script();
        s.seed_binding_set(1, Some(vec![("F".into(), "JUMP".into())]));
        s.load_binding_set(1);
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("F")"#).unwrap(),
            "JUMP"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("W")"#).unwrap(),
            ""
        );
        let snap = s.keybind_snapshot();
        assert_eq!(snap[1], ("JUMP".to_string(), vec!["F".to_string()]));
    }

    /// The `.wtf` reader (`0x4b6140`): CR or LF ends a line, `bind ` opens one in any case, the key
    /// runs to the next space and the command is the rest; `SetBinding` then refuses a bad key and
    /// a later line steals an earlier one's key.
    #[test]
    fn a_bindings_wtf_reads_as_the_reference_reads_it() {
        let pairs = parse_bindings_wtf(
            "bind W MOVEFORWARD\r\nBIND UP MOVEFORWARD\n# a note\r\nbound X Y\r\n\
             bind SCROLLLOCK JUMP\r\nbind SPACE JUMP\r\nbind SPACE SITORSTAND\r\n",
        );
        assert_eq!(pairs.len(), 5, "{pairs:?}");
        let mut s = UiScript::new().unwrap();
        s.set_default_bindings(pairs);
        assert_eq!(
            s.default_bindings(),
            [
                ("W".to_string(), "MOVEFORWARD".to_string()),
                ("UP".to_string(), "MOVEFORWARD".to_string()),
                ("SPACE".to_string(), "SITORSTAND".to_string()),
            ]
        );
    }

    /// `RunBinding(command[, "up"])` runs the body through `RunCommand` in the call
    /// (`0x4b8180`-`0x4b81e0`): `keystate` set for the run and nil after (`0x4b7c42`), a release
    /// only for a `runOnUp` command (`0x4b7bf1`), an unknown name a no-op, no name an error.
    #[test]
    fn run_binding_runs_the_body_in_the_call() {
        let s = script();
        s.run(r#"RunBinding("JUMP") RunBinding("JUMP", "up") RunBinding("NOSUCHCOMMAND")"#)
            .unwrap();
        assert_eq!(s.eval::<i64>("return JUMPS").unwrap(), 1);
        s.run(r#"RunBinding("moveforward")"#).unwrap();
        assert_eq!(s.eval::<String>("return FWD_LAST").unwrap(), "down");
        s.run(r#"RunBinding("MOVEFORWARD", "UP")"#).unwrap();
        assert_eq!(s.eval::<String>("return FWD_LAST").unwrap(), "up");
        assert!(s.eval::<bool>("return keystate == nil").unwrap());
        let err = s.run("RunBinding()").unwrap_err().to_string();
        assert!(err.contains(r#"Usage: RunBinding("COMMAND")"#), "{err}");
        // Nested, the inner run's nil is what the outer body sees after it: nothing restores.
        s.register_bindings(
            &parse(
                r#"<Bindings><Binding name="OUTER">
                    RunBinding("JUMP") OUTER_AFTER = tostring(keystate)
                </Binding></Bindings>"#,
            )
            .unwrap(),
        );
        assert!(s.execute_binding("OUTER", true).unwrap());
        assert_eq!(s.eval::<String>("return OUTER_AFTER").unwrap(), "nil");
    }

    /// `execute_binding` is `ExecuteBinding`'s `RunCommand`: the press runs, the release runs only
    /// a `runOnUp` body, and nothing ran answers false.
    #[test]
    fn execute_binding_answers_whether_a_body_ran() {
        let s = script();
        assert!(s.execute_binding("JUMP", true).unwrap());
        assert!(!s.execute_binding("JUMP", false).unwrap());
        assert!(s.execute_binding("MOVEFORWARD", false).unwrap());
        assert!(!s.execute_binding("TOGGLESTATS", true).unwrap());
        assert_eq!(s.eval::<i64>("return JUMPS").unwrap(), 1);
    }
}

#[cfg(test)]
mod addon_persistence_tests {
    use crate::bindings_xml::Binding;
    use crate::script::UiScript;

    fn binding(name: &str) -> Binding {
        Binding {
            name: name.into(),
            header: None,
            run_on_up: false,
            hidden: false,
            platform: None,
            body: "AddonBindingRan = true".into(),
        }
    }

    fn keys(s: &UiScript, name: &str) -> Vec<String> {
        s.keybind_snapshot()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, k)| k)
            .unwrap_or_default()
    }

    /// The stored set is seeded at boot, before the addon's `Bindings.xml` registers at world
    /// entry; the keys are their own table, so the chord is there when the command arrives.
    #[test]
    fn an_addon_binding_restores_its_stored_chord_registered_after_the_seed() {
        let mut s = UiScript::new().unwrap();
        s.seed_binding_set(
            1,
            Some(vec![
                ("SPACE".into(), "JUMP".into()),
                ("CTRL-X".into(), "MYADDONTOGGLE".into()),
            ]),
        );
        s.load_binding_set(1);
        s.register_addon_bindings("MyAddon", &[binding("MYADDONTOGGLE")]);
        assert_eq!(keys(&s, "MYADDONTOGGLE"), ["CTRL-X"]);
        assert!(s.execute_binding("MYADDONTOGGLE", true).unwrap());
    }

    #[test]
    fn an_addon_binding_with_no_stored_chord_registers_unbound() {
        let mut s = UiScript::new().unwrap();
        s.seed_binding_set(1, Some(vec![("Q".into(), "SOMETHINGELSE".into())]));
        s.load_binding_set(1);
        s.register_addon_bindings("MyAddon", &[binding("MYADDONTOGGLE")]);
        assert!(keys(&s, "MYADDONTOGGLE").is_empty());
    }

    #[test]
    fn the_character_set_wins_for_an_addon_command() {
        let mut s = UiScript::new().unwrap();
        s.seed_binding_set(1, Some(vec![("CTRL-X".into(), "MYADDONTOGGLE".into())]));
        s.seed_binding_set(2, Some(vec![("ALT-Z".into(), "MYADDONTOGGLE".into())]));
        s.load_binding_set(2);
        s.register_addon_bindings("MyAddon", &[binding("MYADDONTOGGLE")]);
        assert_eq!(keys(&s, "MYADDONTOGGLE"), ["ALT-Z"]);
    }
}
