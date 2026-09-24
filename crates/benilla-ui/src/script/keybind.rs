//! The **key-binding table** — the client's chord→command store, engine side (decision 0997).
//!
//! In the reference every rebindable action is a *binding*: a command name (`MOVEFORWARD`)
//! carrying a category header and up to two bound key chords, stored key→command and saved to
//! `bindings-cache.wtf`. 1.12 exposes the whole system to Lua (`Blizzard_BindingUI` is plain
//! FrameXML over it): `GetNumBindings`/`GetBinding(i)`, `SetBinding(key[, command])` with its
//! steal-the-key law, `GetBindingKey`/`GetBindingAction`, and the three-set model —
//! `LoadBindings(0 default | 1 account | 2 character)`, `SaveBindings(which)`,
//! `GetCurrentBindingSet()`.
//!
//! benilla keeps the same shape at the same seam, split like the CVar table (0954): the **host
//! registers** the commands it actually implements (honest-tree — no row without a real engine
//! action) with their 1.12 default chords, **this table is the string-domain truth** the window's
//! Lua reads and writes synchronously, and the app derives its dispatch view (parsed chords →
//! latched commands) whenever [`super::UiScript::keybinds_generation`] moves, persisting on the
//! queued [`KeybindRequest::Save`]. Chord strings are the reference's own canon —
//! `[ALT-][CTRL-][SHIFT-]<TOKEN>` (`W`, `SPACE`, `NUMPAD0`, `BUTTON4`, `MOUSEWHEELUP`) — matched
//! by string equality exactly as the client does (decision 0585's law, now engine-wide).
//!
//! The stored account/character sets live here too (seeded by the app from
//! `benilla-config/bindings/…`, re-snapshotted on save), so `LoadBindings` is **synchronous** like the
//! reference's — the window calls it and repaints in the same tick, no host round-trip.
//!
//! **Addons register into the same table** (decision 1188 phase 4). An addon's `Bindings.xml`
//! ([`crate::bindings_xml`]) arrives through [`super::UiScript::register_addon_bindings`] and
//! becomes ordinary rows: same names, same chords, same window, same save file. The one thing an
//! addon row carries that a host row does not is a **body** — the Lua chunk the app's dispatch
//! runs on the press (and, for `runOnUp`, again on the release with `keystate = "up"`) — because a
//! host command's action is engine-side and has no Lua to hold. That body is the *only* asymmetry;
//! everything downstream of registration treats the two identically.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use crate::bindings_xml::AddonBinding;

use super::Model;

/// One host-registered command: the 1.12 name, its category header's global-string key
/// (`BINDING_HEADER_MOVEMENT`), whether it has press+release semantics (`runOnUp` — the
/// release half of a press rides this — `RunCommand 0x4b7bf1`, and nothing else: it is NOT a
/// bindability rule, see [`normalize_binding_key`]), and the 1.12 default chords.
#[derive(Clone, Copy, Debug)]
pub struct KeybindCommand {
    pub name: &'static str,
    pub category: &'static str,
    pub run_on_up: bool,
    pub default1: Option<&'static str>,
    pub default2: Option<&'static str>,
}

/// One addon-declared binding's **runnable half** — what the app's dispatch needs to fire a row
/// it cannot find in its own `SPECS` registry (decision 1188 phase 4).
///
/// The dispatch view is re-derived whenever [`super::UiScript::keybinds_generation`] moves, so
/// this is a snapshot like [`super::UiScript::keybind_snapshot`] rather than a borrow — same
/// reason: the app holds it across frames while Lua keeps editing the table underneath.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddonBindingBody {
    /// The command name, exactly as registered — what a [`keybind_snapshot`] row is keyed by.
    ///
    /// [`keybind_snapshot`]: super::UiScript::keybind_snapshot
    pub name: String,
    /// Run the body a second time on the release, with `keystate = "up"` (`runOnUp="true"`).
    pub run_on_up: bool,
    /// The `<Binding>` element's Lua chunk, verbatim.
    pub body: String,
}

/// A host request queued by Lua: persist the live table as set `1`/`2` (`Save`), on which the
/// app writes `benilla-config/bindings/…` — and, for `Save(1)` issued while the character set was
/// active, deletes the character file (the reference's confirmed delete-on-switch); or fire a
/// named binding's action outright (`Run`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeybindRequest {
    Save(u32),
    /// `RunBinding("SCREENSHOT")` — run the command's action as if its chord had been pressed.
    /// It is a *request* rather than something the VM can do itself because a host command's
    /// action is engine-side by construction (see this module's header): the VM holds the command
    /// name and its chords, never its body.
    Run(String),
}

/// The reference's key-string gate — the whole of `SetBinding`'s refusal (decision 1295), in its
/// two halves: the alias normalization at the head of `CBindings::SetBinding 0x4b7490`, and the
/// validator `IsValidBindingKeyString 0x4b7890` it then calls.
///
/// `Some` is the string to STORE — upper-cased, because the table's hash and its bucket compare
/// both fold case (`0x64b3f0` / `0x64a4c0`), so `"shift-w"` and `"SHIFT-W"` are one key. `None` is
/// the refusal, and it is the *only* one the reference has: no command is ever consulted.
///
/// The two halves disagree about case on purpose, and that is verified, not a guess: the alias
/// compare is case-insensitive, the validator is case-SENSITIVE at all three of its sites
/// (`0x64a480` → `0x40de80`). So `SetBinding("mousewheelup", …)` answers nil while
/// `SetBinding("MOUSEWHEELUP", …)` binds — and a *lookup* for either finds it.
pub fn normalize_binding_key(key: &str) -> Option<String> {
    // Eleven punctuation NAMES become their literal character. A WHOLE-STRING compare, so
    // `SHIFT-LEFTBRACKET` is not normalized — and then fails the validator, where the accepted
    // spelling `SHIFT-[` passes.
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
    // 1. Strip `SHIFT-`/`CTRL-`/`ALT-` repeatedly — the loop restarts while any matched
    //    (`0x4b78a0`–`0x4b78d0`), so the validator takes modifiers in ANY order and REPEATED,
    //    unlike the fixed ALT-CTRL-SHIFT order the chord builder emits.
    let mut rest = key;
    while let Some(next) = ["SHIFT-", "CTRL-", "ALT-"]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
    {
        rest = next;
    }
    // 2. One character passes (`0x41aab0` decode, `0x4b78e6`) — which is how every letter, digit
    //    and punctuation key is bindable without being named anywhere. The empty string lands in
    //    the same arm and passes too; `SHIFT-` alone is therefore a legal (unpressable) key.
    if rest.chars().count() <= 1 {
        return true;
    }
    // 3. `F`/`NUMPAD`/`BUTTON` (the table at `0x846c04`) followed by digits and nothing else.
    //    `NUMPADPLUS` falls through here to arm 4, its remainder not being digits.
    for prefix in ["F", "NUMPAD", "BUTTON"] {
        if let Some(digits) = rest.strip_prefix(prefix) {
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                return true;
            }
        }
    }
    // 4. The 26 names at `0x846c20`…`0x846c84`. `SHIFT`/`CTRL`/`ALT`/`UNKNOWN` are in none of the
    //    arms, which is why a lone modifier can never be bound — and `SCROLLLOCK`/`PAUSE` are in
    //    none either, matching the namer that calls their key codes `UNKNOWN` (`0x210`/`0x211`).
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
    /// The live bound chords, in bind order (the reference's per-command key list; the first two
    /// are what `GetBindingKey` and the window's Key 1/Key 2 columns show).
    keys: Vec<String>,
    /// `hidden="true"` on an addon's `<Binding>` — bindable, saved and dispatched like any other
    /// row, but absent from the window's enumeration ([`KeybindState::visible`]). Always `false`
    /// for a host command: the registry is an honest tree, so there is nothing in it to hide.
    hidden: bool,
    /// An addon binding's Lua chunk; `None` for a host command, whose action is engine-side. This
    /// is what makes a row an addon's — see the module doc.
    body: Option<String>,
}

/// The table + the stored sets + the Lua→host seams. Lives in [`Model`].
#[derive(Default)]
pub(crate) struct KeybindState {
    entries: Vec<Entry>,
    by_name: HashMap<String, usize>,
    /// Saved snapshots: `stored[0]` = account (set 1), `stored[1]` = character (set 2, `None`
    /// when no character-specific bindings exist). Each is per-entry key lists, entry order.
    stored: [Option<Vec<Vec<String>>>; 2],
    /// The same two stored sets **by command name** (decision 1201).
    ///
    /// [`Self::stored`] is positional over [`Self::entries`], which is exactly right for the
    /// commands that exist when a set is seeded and useless for the ones that do not: an addon's
    /// `Bindings.xml` registers at world entry, long after the account set was seeded at boot, so
    /// its row in the stored file had no slot to land in and its chord was silently dropped. The
    /// binding registered, listed in the window, dispatched — and forgot its key every restart
    /// (the defect 1192 §4 recorded). Keyed by the UPPERCASED name, like `by_name`.
    stored_by_name: [HashMap<String, Vec<String>>; 2],
    /// `1` account, `2` character — which set the live table edits (`GetCurrentBindingSet`).
    current_set: u32,
    /// Bumped by every live-table mutation and every seed — the app's re-derive-dispatch signal.
    generation: u64,
    requests: Vec<KeybindRequest>,
    /// The Keybindings page's capture arm (`BenillaBindCapture(armed)`): while set, the host
    /// swallows raw input and calls the page back with the canonical chord string.
    capture_armed: bool,
}

impl KeybindState {
    /// The rows the Key Bindings window enumerates — everything except the `hidden="true"` ones
    /// (decision 1188 phase 4).
    ///
    /// The filter lives at the *Lua enumeration* (`GetNumBindings`/`GetBinding`) rather than at
    /// registration, because a hidden binding is a real binding everywhere else: `SetBinding` can
    /// bind it, `GetBindingAction` reports it, the app dispatches it, and the save file carries
    /// it. That is 1.12's own split — the shipped `Blizzard_BindingUI.lua` filters on nothing but
    /// the `HEADER` prefix (l.87), so `hidden` can only be the engine's own exclusion from the
    /// enumeration it walks, and the twelve hidden rows (the debug toggles, `TURNORACTION`,
    /// `CAMERAORSELECTORMOVE`…) are exactly the ones no 1.12 Key Bindings window shows.
    ///
    /// Inert for everything shipped today: no host command is hidden, so the enumeration is
    /// entry-for-entry what it was before addons could register.
    fn visible(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.hidden)
    }

    /// The reference's `SetBinding(key[, command])`: unbind `key` wherever it is, then (with a
    /// command) append it to that command's key list.
    ///
    /// **The only thing it refuses is a key string that is not a key** (decision 1295). We used to
    /// refuse a mousewheel chord on a press+release (`runOnUp`) command — the reading everyone
    /// takes from `Blizzard_BindingUI`'s `if not SetBinding(…) then … KEYBINDINGFRAME_MOUSEWHEEL_
    /// ERROR`. It is wrong: `CBindings::SetBinding 0x4b7490` never reads a command node at all
    /// (neither the target's nor the incumbent's), and its one reachable falsey return is the
    /// validator at `0x4b762c`. `"runOnUp"` has exactly one reader image-wide — `0x4b7bf1`, in
    /// `RunCommand`, gating the up half. So 1.12 binds the wheel to an action button happily, and
    /// B265 was ours.
    fn set_binding(&mut self, key: &str, command: Option<&str>) -> bool {
        let Some(key) = normalize_binding_key(key) else {
            return false;
        };
        // The one divergence left here (1295): the reference stores the action string verbatim,
        // so `SetBinding("K", "NOSUCHCOMMAND")` answers 1 there and nil here. Our table is
        // command-centric — a key list per registered command — and has nowhere to put a binding
        // whose command does not exist. Nothing reachable today can hit it: the window offers
        // only registered rows, and an addon's own names register from its `Bindings.xml`.
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

    /// Snapshot the live key lists (entry order) — the stored-set payload.
    fn snapshot(&self) -> Vec<Vec<String>> {
        self.entries.iter().map(|e| e.keys.clone()).collect()
    }

    /// Replace the live key lists from a snapshot (entry-count mismatch pads/truncates — a
    /// stored set from before a command was registered simply leaves the new command on its
    /// live value).
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

    /// The reference's `LoadBindings(set)`: 0 = defaults, 1 = account, 2 = character (falling
    /// back to the account snapshot when no character set exists — the reference's character
    /// set starts as a copy). Loading 1/2 also moves `current_set`; loading defaults does not
    /// (you are still *editing* the set you had, as in the window's Reset To Default).
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

    /// The reference's `SaveBindings(which)`: snapshot live into set `which`, make it current,
    /// and — saving account while character bindings exist — drop the character set (the
    /// window's confirmed permanent delete). Queues the host persist.
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

    /// Append one addon's parsed `Bindings.xml` (decision 1188 phase 4).
    ///
    /// **Append-only and idempotent per uppercased name**, exactly like [`super::UiScript::
    /// register_bindings`] — which is also what makes a host command win outright: `MOVEFORWARD`
    /// is already in the table when an addon declares it, so the addon's row is skipped and the
    /// engine's real action keeps the name. Nothing is ever replaced, so an index handed out
    /// earlier (the dispatch view's, the window's row) stays valid.
    ///
    /// **The section carry-forward** is applied here, where the addon's identity is known: a
    /// `header` attribute opens a section that runs until the next one (the reference's list is
    /// flat with `HEADER_*` pseudo-entries in it, so 13 headers cover its 228 bindings — and our
    /// own `SPECS` table already encodes exactly that reading, giving `MOVEBACKWARD` the
    /// `MOVEMENT` header its `<Binding>` never states).
    ///
    /// **A file whose first bindings declare no header at all gets the addon's own name** as
    /// their section token, and that is a deliberate divergence: the reference would continue
    /// whatever section the *previous file* ended in, which under our per-entry category (0997's
    /// era-shaped `GetBinding`) would file an addon's keys under `BINDING_HEADER_CAMERA` and make
    /// them unfindable. The window renders an unknown token literally
    /// (`KeyBindings_String(token, token)`), so the section simply reads as the addon's name —
    /// which is the honest answer to "where did these come from". A declared header keeps the
    /// ecosystem's convention (`header="MYADDON"` → `BINDING_HEADER_MYADDON`, the global string
    /// the addon defines in its own Lua).
    fn register_addon(&mut self, addon: &str, bindings: &[AddonBinding]) {
        let mut section = addon.to_string();
        for b in bindings {
            // **`platform` skips the whole NODE, header and all** — `0x4b70c3`-`0x4b70e5` bails
            // out of the element before anything on it is read. It matters here and nowhere else:
            // `header="ITUNES_REMOTE"` rides on `ITUNES_PLAYPAUSE`, the first of the five
            // `platform="mac"` rows, so on a build that is not the Mac one that category does not
            // merely come up empty — it never exists. Keeping the header of a skipped row would
            // leave its section open over whatever followed.
            if b.platform.as_deref().is_some_and(|p| p != THIS_PLATFORM) {
                continue;
            }
            // Before the duplicate skip below: a duplicate name still *closes* a section the same
            // way, because the header is a property of the file's order, not of the row that
            // survived it. (Not the platform case above — a duplicate is a node the loader read
            // and rejected; a foreign-platform node it never read at all.)
            if let Some(h) = &b.header {
                section = format!("BINDING_HEADER_{h}");
            }
            let key = b.name.to_ascii_uppercase();
            if self.by_name.contains_key(&key) {
                continue;
            }
            let idx = self.entries.len();
            // The player's stored chord for this command, if they ever set one (decision 1201).
            // The CURRENT set first, then the account set behind it — the same precedence `load`
            // applies, so an addon binding restores exactly like a shipped one.
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
                // 1.12's `<Binding>` carries no default chord — the shipped defaults live in the
                // engine's own table, which is why every addon binding starts unbound and the
                // player binds it in the window.
                defaults: [None, None],
                keys: stored.unwrap_or_default(),
                hidden: b.hidden,
                body: Some(b.body.clone()),
            });
        }
        self.generation += 1;
    }
}

/// What `platform="…"` has to equal for a binding to register here — the reference's own two
/// spellings, read out of both binaries: the Windows build compares against `"windows"`
/// (`0x846f98`, tested at `0x4b70c3`-`0x4b70e5`) and the Mac slice against `"mac"` (`0x4e28f8`).
/// Every other OS benilla runs on takes the `"windows"` name rather than inventing a third — the
/// only thing this attribute has ever distinguished is a 2006 Carbon app from everything else.
const THIS_PLATFORM: &str = if cfg!(target_os = "macos") {
    "mac"
} else {
    "windows"
};

impl super::UiScript {
    /// Register the host-implemented commands (boot-time, order = the 1.12 `Bindings.xml`
    /// order). Idempotent per name; live keys seed from the defaults.
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

    /// Register one addon's parsed `Bindings.xml` — the runtime-`String` sibling of
    /// [`Self::register_bindings`] (decision 1188 phase 4), called at the reference's own position
    /// in the addon load: after that addon's `.toc` files, before its saved variables
    /// (`0x51f400`).
    ///
    /// Two methods rather than one because the two payloads are genuinely different: a host
    /// command is `&'static str` all the way down (it names a Rust action and ships default
    /// chords), an addon's is owned text read off disk this session (it names nothing and carries
    /// a Lua body). The rules they register under are identical — see
    /// [`KeybindState::register_addon`].
    pub fn register_addon_bindings(&mut self, addon: &str, bindings: &[AddonBinding]) {
        self.model_mut().keybinds.register_addon(addon, bindings);
    }

    /// Every addon-declared binding's runnable half, in registration order — the app's dispatch
    /// derivation reads this beside [`Self::keybind_snapshot`] and indexes into it.
    ///
    /// Host commands are absent by construction (they have no body), which is what lets the app's
    /// dispatch target be a two-armed enum rather than an index space with a sentinel in it.
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

    /// Host-side seed of a stored set (`1` account / `2` character) — the loaded
    /// `benilla-config/bindings/…` state, already resolved to full per-command key lists by the app's
    /// diff layer. Passing the character set marks it existing (the window's checkbox state);
    /// `seed_binding_set(2, None)` clears it. Does not touch the live table — call
    /// [`Self::load_binding_set`] after seeding to activate one.
    pub fn seed_binding_set(&mut self, set: u32, keys: Option<Vec<(String, Vec<String>)>>) {
        let mut model = self.model_mut();
        let kb = &mut model.keybinds;
        // Keep the pairs BY NAME as well as positionally: a command that does not exist yet
        // (an addon's, registered at world entry) has no position to occupy, and the by-name map
        // is what `register_addon` consults when it finally does (decision 1201).
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

    /// The live table as `(command, bound chords)` in registration order — the app's dispatch
    /// derivation and the save diff read this.
    pub fn keybind_snapshot(&self) -> Vec<(String, Vec<String>)> {
        self.model_mut()
            .keybinds
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.keys.clone()))
            .collect()
    }

    /// Bumped by every table mutation (Lua or host) — cheap to poll; re-derive dispatch when it
    /// moves (the macros-generation pattern).
    pub fn keybinds_generation(&self) -> u64 {
        self.model_mut().keybinds.generation
    }

    /// Drain the queued host requests (persist-on-save).
    pub fn take_keybind_requests(&mut self) -> Vec<KeybindRequest> {
        std::mem::take(&mut self.model_mut().keybinds.requests)
    }

    /// Which set the live table edits: 1 account, 2 character.
    pub fn current_binding_set(&self) -> u32 {
        self.model_mut().keybinds.current_set
    }

    /// Whether a character-specific stored set exists (the checkbox's persisted truth).
    pub fn character_bindings_exist(&self) -> bool {
        self.model_mut().keybinds.stored[1].is_some()
    }

    /// The Keybindings page's capture arm — while true, the host swallows raw input and
    /// calls `KeyBindings_OnHostKey("<chord>")` instead of dispatching it.
    pub fn bind_capture_armed(&self) -> bool {
        self.model_mut().keybinds.capture_armed
    }
}

/// Register one addon's parsed `Bindings.xml` from a bare `&Lua` — the `LoadAddOn` path
/// ([`super::addon::load_addon`]) runs *inside* a Lua binding, synchronously, and has no
/// [`super::UiScript`] to reach. The exact shape `load_saved_variables` already uses next door,
/// and for the same reason: both halves of the addon load must do the same thing at the same
/// position, whichever entered.
pub(crate) fn register_addon_bindings(lua: &Lua, addon: &str, bindings: &[AddonBinding]) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .keybinds
        .register_addon(addon, bindings);
}

/// Register the binding globals (1.12 names; `GetBinding` returns the era 4-tuple —
/// command, category token, key1, key2 — the categorized window's shape).
///
/// `GetNumBindings`/`GetBinding` walk [`KeybindState::visible`] — the table minus the
/// `hidden="true"` rows — while every other verb here keys on name or chord and so sees the whole
/// table (a hidden binding is bindable, saved and dispatched, just not listed).
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
    // `RunBinding(command)` — the reference's own passthrough verb. Its live caller is
    // `CinematicFrame`'s `OnKeyDown`, which hands the SCREENSHOT chord back to its binding while
    // swallowing every other key, so a player can still photograph a fly-by. Anything the VM
    // holds a body for it could run itself; a host command's action is engine-side, so this
    // queues and the app fires it in the same frame.
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
        // Era 4-tuple: command, category token, key1, key2 — defaults seeded live.
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
        // Bind W to JUMP: stolen from MOVEFORWARD (its slot 1 empties, UP slides up), appended
        // to JUMP's list after its defaults.
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
        // Unbind by key: SetBinding(key) with no command.
        s.run(r#"SetBinding("SPACE")"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("SPACE")"#)
                .unwrap(),
            ""
        );
        // **The wheel binds to a press+release command** (B265, decision 1295). This assertion
        // used to read `== nil`, on the universal misreading of `Blizzard_BindingUI`'s
        // `if not SetBinding(…) then … KEYBINDINGFRAME_MOUSEWHEEL_ERROR`. `0x4b7490` never looks
        // at a command node, so 1.12 takes it — and the notch runs both halves.
        assert!(s
            .eval::<bool>(r#"return SetBinding("SHIFT-MOUSEWHEELUP", "MOVEFORWARD") == 1"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return SetBinding("MOUSEWHEELDOWN", "CAMERAZOOMIN") == 1"#)
            .unwrap());
        // The refusal that IS there: `IsValidBindingKeyString 0x4b7890`. A lone modifier, a name
        // in none of its four arms, and an alias that is not the whole string all fail; the
        // pure unbind runs the same validator, so it fails on the same strings.
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
        // …and the ones that pass: one character (with modifiers in any order and repeated),
        // F/NUMPAD/BUTTON + digits, the 26 names, and a whole-string punctuation alias.
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
        // The validator is case-SENSITIVE where the lookup behind it is not: a lower-case key
        // string never reaches the table at all.
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
        // Rebind, save as character: stored set 2 exists, current set moves.
        s.run(r#"SetBinding("F", "JUMP")"#).unwrap();
        s.run("SaveBindings(2)").unwrap();
        assert_eq!(s.current_binding_set(), 2);
        assert!(s.character_bindings_exist());
        assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(2)]);
        // Reset To Default: LoadBindings(0) restores defaults but stays on the character set.
        s.run("LoadBindings(0)").unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("F")"#).unwrap(),
            ""
        );
        assert_eq!(s.current_binding_set(), 2);
        // Cancel's revert: LoadBindings(current) restores the saved character set.
        s.run("LoadBindings(2)").unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("F")"#).unwrap(),
            "JUMP"
        );
        // Okay back to general: SaveBindings(1) drops the character set — the confirmed
        // permanent delete — and the app is asked to persist (and unlink the character file).
        s.run("SaveBindings(1)").unwrap();
        assert_eq!(s.current_binding_set(), 1);
        assert!(!s.character_bindings_exist());
        assert_eq!(s.take_keybind_requests(), vec![KeybindRequest::Save(1)]);
    }

    /// **An addon's `Bindings.xml` becomes ordinary rows** (decision 1188 phase 4) — same table,
    /// same window, same `SetBinding` laws — with exactly two things that only an addon row has:
    /// a Lua body, and the `hidden` bit.
    ///
    /// What each assertion would catch, since this is the seam a real addon's keys arrive
    /// through:
    /// - **the count and the tuple**: a registration that lands somewhere the window cannot see
    ///   (the whole symptom 1188 phase 4 exists to remove);
    /// - **the carried header**: a per-row reading of `header`, which would leave 215 of the
    ///   reference's own 228 bindings in a nameless category, and a doubled `BINDING_HEADER_`
    ///   prefix if the parser's job were done twice;
    /// - **the addon-name fallback**: a header-less file whose rows land in whatever section
    ///   happened to be last — unfindable in the window, which is why we diverge here;
    /// - **`MOVEFORWARD`**: an addon quietly *replacing* a host command by declaring its name,
    ///   which would swap a real engine action for a Lua body;
    /// - **the hidden row**: `hidden="true"` read as "do not register" rather than "do not list",
    ///   which would make the binding unbindable and undispatchable;
    /// - **the bodies**: a host row leaking into [`super::UiScript::addon_binding_bodies`], which
    ///   is what makes the app's dispatch target a two-armed enum instead of a sentinel index.
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

        // Three host commands + the two listable addon rows. `MOVEFORWARD` did not register
        // twice, and `PROBEHIDDEN` is registered but not listed.
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
        // The host's own command kept its row, its keys and its engine action.
        assert!(s
            .eval::<bool>(
                r#"local k1, k2 = GetBindingKey("MOVEFORWARD"); return k1 == "W" and k2 == "UP""#
            )
            .unwrap());

        // The hidden row is absent from the enumeration…
        assert!(s
            .eval::<bool>(
                r#"for i = 1, GetNumBindings() do
                       if GetBinding(i) == "PROBEHIDDEN" then return false end
                   end
                   return true"#
            )
            .unwrap());
        // …and is a binding in every other respect: bindable, and found by key.
        assert!(s
            .eval::<bool>(r#"return SetBinding("H", "PROBEHIDDEN") == 1"#)
            .unwrap());
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("H")"#).unwrap(),
            "PROBEHIDDEN"
        );
        // And it rides the same laws: `runOnUp` decides whether the body runs again on the
        // release, never whether the row may take a wheel chord (1295) — so both bind.
        assert!(s
            .eval::<bool>(r#"return SetBinding("MOUSEWHEELUP", "PROBEHOLD") == 1"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return SetBinding("MOUSEWHEELDOWN", "PROBEEDGE") == 1"#)
            .unwrap());

        // A second addon, declaring no header at all: its rows are its own section, named for it.
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

        // The runnable halves the app's dispatch indexes — addon rows only, in registration
        // order, hidden included.
        let bodies = s.addon_binding_bodies();
        let names: Vec<&str> = bodies.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["PROBEHOLD", "PROBEEDGE", "PROBEHIDDEN", "LIBKEY"]);
        assert!(bodies[0].run_on_up && !bodies[1].run_on_up);
        assert!(bodies[0].body.contains(r#"keystate == "down""#));
    }

    /// **The file's own OS gate.** `platform="mac"` is 1.12's one OS-specific device in
    /// `Bindings.xml` — five `ITUNES_REMOTE` rows in the Mac build and nowhere else — and a row
    /// for the other build is not registered at all. Not hidden, not registered-and-inert:
    /// registering it would list a command in the Key Bindings window whose body calls functions
    /// this build does not have, and let the player spend a key on it.
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
        // …and it is not merely hidden from the window either: nothing can bind it.
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
        // Commands absent from the seed keep their live keys (a stored set from before a
        // command was registered leaves the new command alone).
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

    /// **An addon's binding remembers its key across a restart** — the defect decision 1192 §4
    /// recorded and left open: "the binding registers, lists, dispatches, and forgets its key."
    ///
    /// The shape is the whole point, so it is reproduced literally: at BOOT the stored set is
    /// seeded while only the host's own commands exist, and the addon's `Bindings.xml` registers
    /// hours later at world entry. The stored row therefore has no slot to land in, which is why
    /// the positional snapshot alone could never carry it.
    #[test]
    fn an_addon_binding_restores_its_stored_chord_registered_after_the_seed() {
        let mut s = UiScript::new().unwrap();
        // Boot: the host's commands, then the account set off disk — which already carries the
        // player's chord for a command that does not exist in this VM yet.
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

        // World entry: the addon's Bindings.xml registers.
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

    /// A command the stored set says nothing about still registers **unbound**, which is 1.12's
    /// own rule for an addon binding (its `<Binding>` carries no default chord).
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

    /// The character set wins over the account set for an addon command too — the same precedence
    /// `load` applies to every other command, so an addon binding restores like a shipped one.
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
