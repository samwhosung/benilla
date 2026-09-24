//! The **CVar table** — the client's named-console-variable store, engine side (decision 0954).
//!
//! In the reference every player setting is a CVar: a case-insensitively named string value with
//! a registered default, read and written from Lua through `GetCVar`/`SetCVar`/`GetCVarDefault`
//! (the 1.12 options panels are thin UI over this table). benilla keeps the same shape at the
//! same seam: the **host registers** the vars it implements
//! ([`super::UiScript::register_cvars`] — most backed by an engine knob, a few consumed by Lua
//! alone), Lua reads/writes them synchronously, and each Lua write lands on the **change queue** the host
//! drains per frame ([`super::UiScript::take_cvar_changes`]) to sync its resources and mark the
//! config file dirty. Host-side writes ([`super::UiScript::set_cvar_host`] — the loaded config,
//! an env override) do NOT ride the queue: the host already knows, and an echo would re-dirty
//! the file it just loaded.
//!
//! Values are **strings**, like the client's (`GetCVar` returns a string; consumers parse and
//! clamp at their own edge). An unknown name warns once and no-ops — a benilla CVar is either
//! host-registered or addon-declared through `RegisterCVar` (decision 1195), so an unknown key
//! is a typo or an unshipped feature, never a storage slot.
//!
//! **The table dies with the VM, so persistence bridges it** (decision 1291): in the reference
//! this store is engine memory and survives every `ReloadUI`; ours is per-VM state, replaced at
//! every login and reload (1290/1291). The host hands each fresh VM the config file's values
//! ([`super::UiScript::set_cvar_saved_base`]) before anything registers, and registration —
//! either kind — starts a key at its saved value. The host folds the dying VM's table back into
//! its persist state on the session edge, so the two halves meet.

use mlua::{Lua, Value};

use super::{Model, ScriptValue};

/// One registered CVar: the host's spelling (for change events and the config file), the live
/// value, and the registered default (`GetCVarDefault`, and the saver's "only write what moved").
#[derive(Clone, Debug)]
pub(crate) struct CvarSlot {
    /// The registered spelling (`MasterVolume`), reported on change events and in snapshots;
    /// the table key is the lowercase form (the client's lookups are case-insensitive).
    pub name: String,
    pub value: String,
    pub default: String,
    /// The reference's flag bit1 (`rec+0x1c & 0x2`, decision 2303): a write lands in
    /// [`Self::pending`] instead of [`Self::value`], so `GetCVar` keeps answering the applied
    /// value until the host commits the latch (`CVar::Update 0x63e060` — for the `gx*` rows,
    /// inside `RestartGx`). Seeded by the host ([`super::UiScript::seed_cvars`]); a row the VM
    /// registers on its own is never latched, exactly like the console `set` command's create
    /// path in the reference.
    pub latched: bool,
    /// The staged value of a latched row — what the host will commit at the boundary, and what
    /// it is told about through the change queue the moment it is written.
    pub pending: Option<String>,
}

/// One row of the host's registry, as the VM's mirror is seeded from it (decision 2303): the
/// registered spelling, the **applied** value, the registered default, and the latch flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeededCvar {
    pub name: String,
    pub value: String,
    pub default: String,
    pub latched: bool,
}

/// One device-supported multisample format, as the Video options dropdown shows it.
///
/// The reference's distilled triple (`[0xb4b444]`, count `[0xb4b440]`, built by `0x48c3e0`), and
/// the shape `MULTISAMPLING_FORMAT_STRING` formats: `"%d-bit color %d-bit depth %dx multisample"`.
///
/// `samples == 1` is the encoding of **no multisampling**, not "one sample of it" — the convention
/// both of the reference's enumerators normalise to (`0x58b8b1`'s jump table maps
/// `D3DMULTISAMPLE_NONE → 1`; the GL path at `0x58d5fa`/`0x58d5fe`/`0x58d605` turns a failed
/// `WGL_SAMPLES_ARB` query or a value of 0 into 1), and the same convention the device paths test
/// with `> 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultisampleFormat {
    pub color_bits: u32,
    pub depth_bits: u32,
    pub samples: u32,
}

/// One screen resolution the Video options dropdown offers, in **physical pixels**.
///
/// The reference enumerates the graphics device's own display modes; benilla has no exclusive
/// mode-set to make (`benilla_app::video`'s module doc: Wayland has no client-side mode-setting,
/// X11's is XRandR on the *desktop*, macOS has none, and WoW itself deleted exclusive fullscreen in
/// 8.0.1), so the list is the sizes this client can actually present a window at. Either way it is
/// a fact about the display that the VM cannot ask for itself, so the host pushes it.
///
/// **The `"WxH"` spelling is the contract, not a convenience.** `OptionsFrame.lua:281-284` finds
/// the `x`, `strsub`s both sides and evaluates `width/height > 4/3` for the WIDESCREEN tag, and
/// `CT_Viewport.lua:107` re-reads it with `string.find(currRes, "(%d+)x(%d+)")`. Two independent
/// consumers, both of which fail silently on a space, a suffix, or an `X`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenResolution {
    pub width: u32,
    pub height: u32,
}

impl ScreenResolution {
    /// The reference's ordering key, made total: **pixel area first** (`0x58be40`'s whole body is
    /// `a.h*a.w - b.h*b.w`), then width, then height. The tie-break is ours — see
    /// [`super::UiScript::set_screen_resolutions`].
    pub(crate) fn sort_key(&self) -> (u64, u32, u32) {
        (
            u64::from(self.width) * u64::from(self.height),
            self.width,
            self.height,
        )
    }
}

impl std::fmt::Display for ScreenResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// What `GetVideoCaps` answers with — the seven values `OptionsFrame_Load` destructures at
/// `OptionsFrame.lua:59`, in that order.
///
/// ```lua
/// local hasAnisotropic, hasPixelShaders, hasVertexShaders, hasTrilinear,
///       hasTripleBuffering, maxAnisotropy, hasHardwareCursor = GetVideoCaps();
/// ```
///
/// In the reference these are the D3D/GL device's own answers, cached at start-up. Here they are
/// what wgpu and this client's own presentation path really do — pushed by the host, which is the
/// only side that holds a `RenderAdapter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct VideoCaps {
    pub anisotropic: bool,
    pub pixel_shaders: bool,
    pub vertex_shaders: bool,
    pub trilinear: bool,
    /// **A BACKEND constant, not a device capability** — `caps+0x20` is written by the two device
    /// constructors only (1 under Direct3D, 0 under OpenGL) and read by `GetVideoCaps` alone, so
    /// `hasTripleBuffering == 1` means "the backend is Direct3D". Its name is the one INFERRED
    /// label in the seven (`GetVideoCaps 0x48db40`).
    ///
    /// `false` here, and it is pushed as the **number 0**, never nil: benilla is not Direct3D and
    /// exposes no buffering knob, so check button 13 is hidden and button 6 re-seated
    /// (`OptionsFrame.lua:168-175`) — while the file's two `not hasTripleBuffering` clauses stay
    /// dead, which is what the reference does in both backends.
    pub triple_buffering: bool,
    /// The raw maximum sample count, matched against `ANISOTROPIC_VALUES = {"1","2","4","8","16"}`
    /// with `tonumber` (`OptionsFrame.lua:124`) — so it must be one of those numbers to move the
    /// slider's ceiling, and anything else leaves `value.maxValue` where it was. Pushed raw, or
    /// **nil iff 0**; the reference is effectively never nil here (its base ctor seeds the field
    /// to 1), which is why its consumers gate on slot 1 or on `< 2` rather than on nil.
    pub max_anisotropy: u32,
    pub hardware_cursor: bool,
}

impl super::UiScript {
    /// Hand registration the config file's persisted values (decision 1291): name → value, keys
    /// lowercased here. Set **before** any `register_cvars` / addon `RegisterCVar` runs in this
    /// VM; a name registered while present here starts at the saved value instead of its default.
    ///
    /// This is the bridge that makes the per-VM table behave like the reference's engine-side
    /// one (where the store outlives every `ReloadUI`): a CVar with no host knob
    /// (`statusBarText`) and a CVar only an addon declares would otherwise revert to default on
    /// every VM replacement — and the saver, seeing value == default, would then *strip the
    /// player's setting from the file*.
    pub fn set_cvar_saved_base(&mut self, entries: impl IntoIterator<Item = (String, String)>) {
        self.model_mut().cvars_saved_base = entries
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect();
    }

    /// Register the host-backed CVars (name, default) — boot-time, idempotent. A re-register of
    /// a live table refreshes defaults but never clobbers a value someone already set. A fresh
    /// registration starts at the saved-base value when the config file carries one
    /// ([`Self::set_cvar_saved_base`]), else at the default.
    ///
    /// Takes `&self` rather than `&mut self` because the table lives behind the VM's app-data
    /// (interior mutability), and the interface loader has to be able to guarantee it from a
    /// `&UiScript` — the stock `UIOptionsFrame.xml` reads CVars in its own `OnLoad`, so a VM that
    /// loads the client's interface without the client's CVar table is not the client
    /// (decision 2115).
    pub fn register_cvars<'a>(&self, vars: impl IntoIterator<Item = (&'a str, &'a str)>) {
        let mut model = self.model_mut();
        for (name, default) in vars {
            let key = name.to_ascii_lowercase();
            match model.cvars.get_mut(&key) {
                Some(slot) => slot.default = default.to_string(),
                None => {
                    let value = model
                        .cvars_saved_base
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| default.to_string());
                    model.cvars.insert(
                        key,
                        CvarSlot {
                            name: name.to_string(),
                            value,
                            default: default.to_string(),
                            latched: false,
                            pending: None,
                        },
                    );
                }
            }
        }
    }

    /// Seed the mirror from the host's registry (decision 2303): every row's spelling, applied
    /// value, default and latch flag, creating or overwriting the slot. This is what a fresh VM
    /// gets at claim time and at the world-entry edge — the host's table is the store that
    /// outlives the VM, so the mirror starts wherever it stands, and a re-seed of a live table
    /// is the host re-asserting its truth (a staged value is dropped with it: the host holds
    /// the pending copy and pushes it back through the outbox if it still stands).
    ///
    /// `&self` for [`Self::register_cvars`]'s reason — the interface loader seeds from a
    /// `&UiScript` before the stock files read their first CVar (decision 2115).
    pub fn seed_cvars(&self, rows: impl IntoIterator<Item = SeededCvar>) {
        let mut model = self.model_mut();
        for row in rows {
            let key = row.name.to_ascii_lowercase();
            model.cvars.insert(
                key,
                CvarSlot {
                    name: row.name,
                    value: row.value,
                    default: row.default,
                    latched: row.latched,
                    pending: None,
                },
            );
        }
    }

    /// Host-side write (a loaded value, a committed latch, the host registry's outbox): sets
    /// the value WITHOUT queueing a change event — the host is the caller, an echo would
    /// re-dirty the file it just read — and clears a staged value, because a host write IS the
    /// commit. Unknown names warn once, same posture as the Lua side.
    pub fn set_cvar_host(&mut self, name: &str, value: &str) {
        let mut model = self.model_mut();
        let key = name.to_ascii_lowercase();
        match model.cvars.get_mut(&key) {
            Some(slot) => {
                slot.value = value.to_string();
                slot.pending = None;
            }
            None => warn_unknown(&mut model, name),
        }
    }

    /// Read one CVar's live value (host side).
    pub fn cvar(&self, name: &str) -> Option<String> {
        self.model_mut()
            .cvars
            .get(&name.to_ascii_lowercase())
            .map(|s| s.value.clone())
    }

    /// Snapshot the whole table as `(name, value, default)` — the saver writes the entries whose
    /// value moved off the default, and nothing else (the config file stays a diff, not a dump).
    pub fn cvars_snapshot(&self) -> Vec<(String, String, String)> {
        self.model_mut()
            .cvars
            .values()
            .map(|s| (s.name.clone(), s.value.clone(), s.default.clone()))
            .collect()
    }

    /// Drain the `(name, new_value)` changes Lua `SetCVar` queued since the last call — the
    /// host's cue to sync its resources and mark the config dirty. Names are the registered
    /// spelling regardless of the caller's casing.
    pub fn take_cvar_changes(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.model_mut().cvar_changes)
    }

    /// Drain the `(name, default)` rows an addon's `RegisterCVar` created since the last call
    /// (decision 2303) — the host's cue to give each one a row in its own registry, which is the
    /// store that survives this VM. In the reference an addon's registration lands in the same
    /// engine-side table as the client's own; this is how ours does.
    pub fn take_cvar_registrations(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.model_mut().cvar_registrations)
    }

    /// A native surface's write — [`set_from_engine`]'s public face: sets the value AND queues
    /// the change like a Lua `SetCVar`, so the host's sync dirties the config file. The glue
    /// AddOns screen's *Load out of date AddOns* checkbox is the caller (decision 1293): a
    /// native widget editing a CVar is the minimap-zoom pattern, reached from outside the crate.
    pub fn set_cvar_engine(&mut self, name: &str, value: &str) {
        set_from_engine(&mut self.model_mut(), name, value.to_string());
    }

    /// Publish the multisample formats this run's device actually accepts — what the Video
    /// options dropdown offers, in the order it offers them.
    ///
    /// The host's half of the reference's `0x48c3e0`: there, a D3D `CheckDeviceMultiSampleType`
    /// sweep over the nine-entry candidate table `0x85a83c` or a GL `wglGetPixelFormatAttribivARB`
    /// sweep over every pixel format; here, `benilla_world::view::supported_sample_counts` asking
    /// wgpu the same question. Pushed rather than pulled because the VM lives in `benilla-ui`,
    /// which has no render adapter and should not grow one.
    pub fn set_multisample_formats(&mut self, formats: Vec<MultisampleFormat>) {
        self.model_mut().multisample_formats = formats;
    }

    /// Publish the screen resolutions the Video options dropdown offers, and which one the client
    /// is currently at.
    ///
    /// **`current` is a promise, not a hint.** `CT_Viewport.lua:201` reads its own screen size as
    /// `arg[GetCurrentResolution()]` over `GetScreenResolutions()`'s varargs and silently falls
    /// back to 4:3 on a miss, so a current size that is not in the list is a wrong answer that
    /// succeeds. The host therefore hands the live size here and this pushes it INTO the list if
    /// the display's own mode table does not carry it — a windowed client at 1600×900 on a 4K
    /// panel is exactly that case, and it is the common one.
    ///
    /// Both halves land together because they are one fact: a list without its index cannot be
    /// read, and an index into a list that moved is worse than none.
    ///
    /// **Ordered by pixel AREA**, which is the reference's own key (`0x58be40`: `a.w*a.h - b.w*b.h`,
    /// with no tie-break at all). Ours breaks ties on width then height, deliberately: the
    /// reference's comparator plus its adjacent-only dedupe can leak a duplicate `"WxH"` when two
    /// distinct modes share an area — 1280×960 and 1600×768 are both 1 228 800 — and reproducing an
    /// unstable sort is aping a quirk, not implementing the mechanism.
    pub fn set_screen_resolutions(
        &mut self,
        mut offered: Vec<ScreenResolution>,
        current: Option<ScreenResolution>,
    ) {
        offered.sort_by_key(ScreenResolution::sort_key);
        offered.dedup();
        let current = current.map(|c| {
            offered.iter().position(|r| *r == c).unwrap_or_else(|| {
                let at = offered.partition_point(|r| r.sort_key() < c.sort_key());
                offered.insert(at, c);
                at
            })
        });
        let mut model = self.model_mut();
        model.screen_resolutions = offered;
        model.current_resolution = current;
    }

    /// Publish what this run's device and presentation path really offer, behind `GetVideoCaps`.
    pub fn set_video_caps(&mut self, caps: VideoCaps) {
        self.model_mut().video_caps = caps;
    }

    /// Drain the `RestartGx()` calls queued since the last call — each is one "apply the staged
    /// video settings now", and the host answers by re-asserting them against the window.
    pub fn take_restart_gx_asks(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().restart_gx_asks)
    }
}

/// An **engine-side** write: a value the engine owns moved, and the CVar table mirrors it. The
/// minimap's zoom is the case that needs it — the client's `set_zoom` (`0x6da8e0`) writes the live
/// index *and* `CVar::Set`s `minimapZoom`/`minimapInsideZoom` in the same breath, which is exactly
/// why the level survives a restart. So this queues the change like a Lua `SetCVar` would (the host
/// must hear it and dirty the config file), unlike [`super::UiScript::set_cvar_host`], whose whole
/// point is *not* to echo the value it just loaded.
///
/// A name the host never registered is a **silent** no-op here, not a warning: engine writes are
/// code, not UI content, so a miss means this build's host doesn't back the var (a bare test VM) —
/// and the registered set is welded to the code truths by `crate::cvars`'s own test.
pub(super) fn set_from_engine(model: &mut Model, name: &str, value: String) {
    store(model, &name.to_ascii_lowercase(), value);
}

/// **The one store every write goes through** — Lua's `SetCVar`, `ConsoleExec`, and the engine
/// writes above. Returns the registered spelling when the write moved something (and queued it
/// for the host), `None` for an unknown key or a write that changed nothing.
///
/// A **latched** slot (decision 2303) takes the write as its staged value and leaves `value`
/// alone, which is the reference's `Set 0x63df50`: `latchedValue` gets the string, `InternalSet`
/// does not run, and `GetCVar` keeps answering what is applied. Writing a latched slot back to
/// its applied value clears the stage instead of staging a no-op. Either way the host hears it
/// through the change queue — it holds the pending copy and commits it at the boundary.
fn store(model: &mut Model, key: &str, value: String) -> Option<String> {
    let slot = model.cvars.get_mut(key)?;
    if slot.latched {
        let staged = (value != slot.value).then_some(value.clone());
        if slot.pending == staged && staged.is_some() {
            return None;
        }
        if staged.is_none() && slot.pending.is_none() {
            return None;
        }
        slot.pending = staged;
    } else {
        if slot.value == value {
            return None;
        }
        slot.value = value.clone();
    }
    let registered = slot.name.clone();
    model.cvar_changes.push((registered.clone(), value));
    Some(registered)
}

/// Push the warn-once for an unknown CVar name into the model's warning stream.
/// The `SetCVar` write, shared with `ConsoleExec` (which is `/console name value`): the slot
/// matched case-insensitively, the change queued for the host only when the value moved, and
/// `CVAR_UPDATE` fired only when a token was passed. `false` = no such CVar (warned once).
pub(super) fn write_cvar(
    model: &mut Model,
    name: &str,
    value: String,
    token: Option<String>,
) -> bool {
    let key = name.to_ascii_lowercase();
    if !model.cvars.contains_key(&key) {
        warn_unknown(model, name);
        return false;
    }
    if store(model, &key, value.clone()).is_some() {
        if let Some(token) = token {
            model.pending_events.push((
                "CVAR_UPDATE".to_string(),
                vec![ScriptValue::Str(token), ScriptValue::Str(value)],
            ));
        }
    }
    true
}

fn warn_unknown(model: &mut Model, name: &str) {
    let key = name.to_ascii_lowercase();
    if model.cvars_warned.insert(key) {
        model.record_warning(format!(
            "unknown CVar '{name}' (not host-registered) — ignored"
        ));
    }
}

/// Coerce the Lua argument to the string the table stores: the client stringifies numbers and
/// booleans the same way (`SetCVar("MusicVolume", 0.4)` is the common call shape).
fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => s.to_str().ok().map(|s| s.to_owned()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Boolean(b) => Some(if *b { "1".into() } else { "0".into() }),
        _ => None,
    }
}

/// Register the `GetCVar`/`SetCVar`/`GetCVarDefault`/`RegisterCVar` globals.
/// The two nameplate toggles' registered CVar names — **defined here, in the crate that publishes
/// the four verbs that write them**, so the binding and the name cannot drift apart. The app reads
/// these consts rather than re-spelling the strings, and welds them to its registered table with
/// its own test.
///
/// They are the LATER era's names: 1.12 registers no nameplate CVar at all (censused — see
/// [`install_nameplate_verbs`]), so benilla's persistence takes the era spelling rather than
/// inventing one, the same posture as `autoLootDefault`.
pub const CVAR_NAMEPLATE_ENEMIES: &str = "nameplateShowEnemies";
/// The friendly-plate half of [`CVAR_NAMEPLATE_ENEMIES`].
pub const CVAR_NAMEPLATE_FRIENDS: &str = "nameplateShowFriends";

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // `RegisterCVar(name, value)` — declare a CVar the client does not ship (decision 1195).
    //
    // This is how an addon gets a persisted setting without a saved-variables file, and it is why
    // `GetCVar` on an unknown name is a *warning* rather than an error: an addon that calls
    // `RegisterCVar` at load and `GetCVar` after expects the second to answer. Registering an
    // existing name is a **no-op, not an overwrite** — the reference will not let an addon reset a
    // client CVar's live value by re-declaring it, and a re-run of the addon's own load must not
    // wipe the player's setting either.
    lua.globals().set(
        "RegisterCVar",
        lua.create_function(|lua, (name, value): (String, Option<Value>)| {
            let value = value.as_ref().and_then(value_to_string).unwrap_or_default();
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let key = name.to_ascii_lowercase();
            // The saved base outranks the declared default (decision 1291): an addon-declared
            // CVar the player has set persists in the config file, and this lookup is how the
            // saved value survives the VM being replaced — the declared value stays the DEFAULT,
            // so the saver still knows what "moved off default" means for this key.
            let saved = model.cvars_saved_base.get(&key).cloned();
            if model.cvars.contains_key(&key) {
                return Ok(());
            }
            // The host learns the row through its own queue (decision 2303): an addon-declared
            // CVar gets a row in the host's registry, which is the store that survives this VM.
            model.cvar_registrations.push((name.clone(), value.clone()));
            model.cvars.insert(
                key,
                CvarSlot {
                    name,
                    value: saved.unwrap_or_else(|| value.clone()),
                    default: value,
                    latched: false,
                    pending: None,
                },
            );
            Ok(())
        })?,
    )?;

    lua.globals().set(
        "GetCVar",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            match model.cvars.get(&name.to_ascii_lowercase()) {
                Some(slot) => Ok(Value::String(lua.create_string(&slot.value)?)),
                None => {
                    warn_unknown(&mut model, &name);
                    Ok(Value::Nil)
                }
            }
        })?,
    )?;
    lua.globals().set(
        "GetCVarDefault",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            match model.cvars.get(&name.to_ascii_lowercase()) {
                Some(slot) => Ok(Value::String(lua.create_string(&slot.default)?)),
                None => {
                    warn_unknown(&mut model, &name);
                    Ok(Value::Nil)
                }
            }
        })?,
    )?;
    // ── The Video options Multisampling dropdown ──────────────────────────────────────────────
    // Three bindings over one list, exactly as `OptionsFrame.lua` consumes them: `_OnLoad` seeds
    // the selection with `GetCurrentMultisampleFormat()`, `_Initialize` walks
    // `GetMultisampleFormats()` three varargs at a time building the menu, and the Okay handler
    // (l.240) calls `SetMultisampleFormat(UIDropDownMenu_GetSelectedID(...))`. Identities and
    // behaviour from the registration table `0x83de68`, records `0x83e2c0`/`0x83e2c8`/`0x83e2d0`.
    lua.globals().set(
        "GetMultisampleFormats",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // Flat, three per entry — `0x48c360` "pushes all three fields of every entry", and the
            // Lua walks `for i=1, arg.n, 3`. A device with nothing to offer returns nothing, and
            // the loop runs zero times: an empty dropdown, not a fabricated one.
            let mut out = mlua::MultiValue::new();
            for f in &model.multisample_formats {
                out.push_back(Value::Number(f.color_bits as f64));
                out.push_back(Value::Number(f.depth_bits as f64));
                out.push_back(Value::Number(f.samples as f64));
            }
            Ok(out)
        })?,
    )?;
    lua.globals().set(
        "GetCurrentMultisampleFormat",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let cvar = |n: &str| model.cvars.get(n).and_then(|s| s.value.parse::<u32>().ok());
            let (Some(color), Some(depth), Some(samples)) = (
                cvar("gxcolorbits"),
                cvar("gxdepthbits"),
                cvar("gxmultisample"),
            ) else {
                return Ok(1.0f64);
            };
            // **1-based, and 1.0 on no match** — `0x48c580` "returns the 1-based index of the
            // matching triple, or 1.0 on no match". Not 0 and not nil: `_OnLoad` feeds this
            // straight to `UIDropDownMenu_SetSelectedID`, so a miss has to name a real row.
            let idx = model
                .multisample_formats
                .iter()
                .position(|f| {
                    f.color_bits == color && f.depth_bits == depth && f.samples == samples
                })
                .map_or(1.0, |i| (i + 1) as f64);
            Ok(idx)
        })?,
    )?;
    lua.globals().set(
        "SetMultisampleFormat",
        lua.create_function(|lua, id: Option<f64>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // 1-based from `UIDropDownMenu_GetSelectedID`; anything off the end is ignored rather
            // than clamped, because a clamp would silently apply a format the player did not pick.
            let Some(f) = id
                .filter(|v| *v >= 1.0)
                .and_then(|v| model.multisample_formats.get(v as usize - 1).copied())
            else {
                return Ok(());
            };
            // All three, from the chosen entry — `0x48c640` "writes those three CVars from the
            // chosen entry". Through `set_from_engine` so each ride the host's change queue and
            // the config file is marked dirty, the same route a Lua `SetCVar` takes.
            set_from_engine(&mut model, "gxColorBits", f.color_bits.to_string());
            set_from_engine(&mut model, "gxDepthBits", f.depth_bits.to_string());
            set_from_engine(&mut model, "gxMultisample", f.samples.to_string());
            // The reference then sets `gxRestart` (`0x842978`, `0x63ce00`). We deliberately do not
            // register that CVar: `gxMultisample` is latched here too (decision 1629 — the camera
            // reads its sample count once, at spawn), so "applies at next launch" is already the
            // behaviour and a second flag saying so would be a flag nothing reads.
            Ok(())
        })?,
    )?;

    install_video_verbs(lua)?;

    lua.globals().set(
        "SetCVar",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let mut it = args.iter();
            let (Some(Value::String(name)), Some(v)) = (it.next(), it.next()) else {
                return Err(mlua::Error::runtime("Usage: SetCVar(\"name\", value)"));
            };
            let name = name.to_str()?.to_owned();
            let Some(value) = value_to_string(v) else {
                return Err(mlua::Error::runtime("Usage: SetCVar(\"name\", value)"));
            };
            // The THIRD argument is the `CVAR_UPDATE` token, and it is the whole mechanism behind
            // that event (decision 1140). 1.12's own callers are its two options panels —
            // `SetCVar(value.cvar, value.value, index)` (UIOptionsFrame.lua l.335/343/345,
            // OptionsFrame.lua l.192) — where `index` is the CheckButtons table's KEY, an
            // uppercase display name like "STATUS_BAR_TEXT". The engine passes it straight through
            // as arg1, which is why `UIOptionsFrame_OnEvent` can look the row up with
            // `UIOptionsFrameCheckButtons[arg1]`, and why `TextStatusBar_OnEvent` compares against
            // "STATUS_BAR_TEXT" rather than "statusBarText".
            //
            // A caller that omits it fires nothing — the event is opt-in per write, not a
            // property of the variable. (Which means `ReputationFrame.xml`'s
            // `arg1 == "statusBarText"` arm is dead code in shipped 1.12: nothing ever passes the
            // CVar's own name as the token. Transcribed as found, not repaired.)
            let token = it.next().and_then(value_to_string);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            write_cvar(&mut model, &name, value, token);
            Ok(())
        })?,
    )?;

    install_nameplate_verbs(lua)?;
    install_world_detail_verbs(lua)
}

/// **What `RestoreVideoDefaults` puts back** — the settings this client's video path backs that the
/// reference's own verb writes, each named by the row it is.
///
/// **The reference does NOT restore registered defaults, and this list is not a guess at which it
/// does.** `0x48dad0` → `0x639a20` maps the already-matched `VideoHardware.dbc` row through
/// nine lookup tables into a hardware-recommended settings struct, copies the `CGxFormat` **preset**
/// that row names for this adapter (`[0xc4e6ac]` → `0xc518e0 + 0x38·n`) into the working device
/// record, writes that record into twelve `gx*` CVars, performs a full synchronous `RestartGx`, and
/// then writes sixteen further graphics CVars from the recommendations. The registered strings
/// (`"640x480"`, `"75"`, `"16"`…) are never consulted.
///
/// **benilla has no such pass, and that is why its registered defaults ARE its recommended
/// configuration.** There is no `VideoHardware.dbc` matching here and no preset table: what a fresh
/// install runs is each `REGISTERED` row's own value, clamped by the device where the device has a
/// say (`MsaaFormats::clamp`, `tex_filter`'s aniso ceiling). So this restores each row to its
/// registered default — the same *meaning*, reached the way this client reaches it — and that is a
/// stated divergence rather than a transcription.
///
/// The membership, though, is the reference's: these are the names in its twelve-plus-sixteen that
/// benilla registers. It does **not** touch `uiScale`, `useUiScale` or `anisotropic` — an earlier
/// draft of this list did, reasoning from the video window's Lua tables instead of from the verb.
///
/// **`WorldDetail` is ours and rides with `frillDensity`**: 2163 welded the panel stop to the
/// reference's own clutter CVar, so restoring one without the other would leave the pair describing
/// two different detail levels.
///
/// `benilla_app::cvars` holds the matching test that every name here is registered — the weld that
/// keeps a rename or a retirement from turning a row into a silent no-op.
pub const VIDEO_DEFAULT_CVARS: &[&str] = &[
    // of the twelve `gx*` the device record writes (`0x639c40`), the six benilla registers
    "gxWindow",
    "gxResolution",
    "gxVSync",
    "gxColorBits",
    "gxDepthBits",
    "gxMultisample",
    // of the sixteen further graphics CVars (`0x639a60`), the three benilla registers
    "farclip",
    "frillDensity",
    "trilinear",
    // ours, welded to `frillDensity` (2163)
    "WorldDetail",
];

/// **The Video options window's own engine verbs** — the nine `OptionsFrame.xml` needs that
/// nothing else in this client provides.
///
/// The window is loaded off the player's chain and kept hidden (decision 2177, the shape 2115 and
/// 2147 set for the Interface and Sound windows): nothing of ours shows it, and it exists so that
/// an addon reaching for the reference's video-options names finds real frames and real functions
/// instead of an alias onto a window of ours.
///
/// **Three of the nine run at LOAD** — `GetScreenResolutions`, `GetCurrentResolution` and
/// `GetRefreshRates`, from the resolution and refresh dropdowns' `<OnLoad>`, because
/// `UIDropDownMenu_Initialize` calls the initializer it is handed *immediately*
/// (`UIDropDownMenu.lua:48-50`). The file cannot load at all without them, and the window being
/// `hidden="true"` is precisely what keeps the other four out of that set: they hang off `OnShow`
/// and the three buttons. The **tenth** name that load reaches, which benilla's own census of this
/// window missed, is plain `GetCVar` (`OptionsFrame.lua:300`, `GetCVar("gxRefresh")`).
///
/// The three multisample verbs the same window needs are above; `GetWorldDetail`/`SetWorldDetail`
/// are in [`install_world_detail_verbs`]. `GetGamma`/`SetGamma` are the last two: 2177 shipped
/// seven and left that pair **loudly absent** under 1203, because a byte-faithful copy would have
/// driven a hardware ramp this client never uploads. 2182 built the render feature behind them, so
/// they have a real reader now and the window's engine verbs are nine of nine.
///
/// **What must NOT be defined, which is as load-bearing as what is.** `OptionsFrame_Load:110` does
/// `getglobal("Get"..value.func)` over the nine slider rows and branches on the result; of the
/// eighteen composed names, six resolve to real bindings in the reference
/// (`Get/SetWorldDetail`, `Get/SetTerrainMip`, `Get/SetBaseMip`) and **ten must resolve to nil** —
/// `Getuiscale`, `Getfarclip`, `Getanisotropic`, `GetspellEffectLevel`, `GetweatherDensity` and
/// their five setters — so those rows fall through to `GetCVar`/`SetCVar`. Defining any of them
/// changes this window's behaviour without erroring. The trap is `GetFarclip`/`SetFarclip`, which
/// DO exist at `0x488f00`/`0x488f30` with a capital F while `value.func` is `"farclip"`: the stock
/// client takes the CVar path only because `getglobal` is case-sensitive. Pinned by
/// `benilla_app`'s `the_video_windows_ten_composed_names_stay_nil`.
fn install_video_verbs(lua: &Lua) -> mlua::Result<()> {
    // ── GetScreenResolutions ─────────────────────────────────────────────────────────────────
    // A vararg of `"WxH"` strings. The format is pinned by two independent consumers, neither of
    // which raises on a wrong one: `OptionsFrame.lua:281-284` splits on the `x` and evaluates
    // `width/height > 4/3` to decide the WIDESCREEN tag, and `CT_Viewport.lua:107` re-parses with
    // `string.find(currRes, "(%d+)x(%d+)")` to size its viewport. A space, an `X`, or a trailing
    // suffix silently gives one of them the wrong answer.
    lua.globals().set(
        "GetScreenResolutions",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = mlua::MultiValue::new();
            for r in &model.screen_resolutions {
                out.push_back(Value::String(lua.create_string(r.to_string())?));
            }
            Ok(out)
        })?,
    )?;
    // ── GetCurrentResolution ─────────────────────────────────────────────────────────────────
    // The **1-based index** into that list — `0x48bfa2 inc eax` over the same array, which is the
    // only reading `arg[GetCurrentResolution()]` (`CT_Viewport.lua:105`) can have.
    //
    // **`1.0` on a miss, never nil and never 0** (`0x48bf88 push 0x3ff00000` — the f64 `1.0`), and
    // the reference takes that path on every failure: no CVar record, a NULL string, an empty
    // list, or a value simply not in it. It is the family idiom — `GetCurrentMultisampleFormat`
    // `0x48c580` answers `1.0` the same way, and `SetMultisampleFormat` above already does. A `1`
    // is therefore not proof of a match, which is exactly the trap the reference walks into: its
    // `gxResolution` registers at `"640x480"`, below its own list's 800×600 floor, so a fresh
    // config reports index 1 while running 640×480.
    //
    // **Where ours reads from is a stated divergence.** The reference parses the `gxResolution`
    // CVar (`0x48bf20`), which on a mode-setting client IS the live mode. benilla ships no
    // exclusive mode-set: `gxResolution` is the WINDOWED size, and while the client is borderless
    // fullscreen the two differ. The host therefore pushes the live window size, because "what
    // resolution am I at" is what every consumer means — `CT_Viewport` scales its viewport by it —
    // and reporting a windowed size while filling a 4K panel would be a wrong answer that
    // succeeds. A pick made while fullscreen still lands in `gxResolution` and takes effect on the
    // way out, which is the honest reading of a windowed-size setting.
    lua.globals().set(
        "GetCurrentResolution",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.current_resolution.map_or(1.0, |i| (i + 1) as f64))
        })?,
    )?;
    // ── SetScreenResolution ──────────────────────────────────────────────────────────────────
    // 1-based, from `UIDropDownMenu_GetSelectedID(OptionsFrameResolutionDropDown)`
    // (`OptionsFrame.lua:239`). Three things about the argument, all carved at `0x48bfd0`:
    //
    // * **The TOLERANT family, not the raising one.** `0x48bfe6 xor esi,esi` seats a default index
    //   of 0 and there is no `0x6f4940` anywhere in the body, so a missing, nil or non-numeric
    //   argument silently selects **the first entry** rather than erroring. A numeric string is
    //   accepted (the `0x6f7c20` coercion).
    // * **Truncated toward zero** — `0x40a2b0 __ftol` with RC=11, so `2.7` is entry 2.
    // * **Out of range is where we deliberately diverge.** `0x48c008 dec eax` / `0x48c009 cmp` /
    //   the failure block at `0x48c00d` converge index 0, negatives and anything past the count on
    //   `list[count]` — one element past the last, read unguarded. It does not fault only because
    //   the array keeps a spare slot. We clamp to the last entry instead: reproducing an
    //   out-of-bounds read to apply an uninitialised resolution is not fidelity.
    //
    // **The same-value guard is real behaviour and is reproduced** (`0x48c081 je 0x48c0c1`): if the
    // requested size equals the one `gxResolution` already holds, the reference returns having done
    // nothing at all — no write, no restart — and so does this. `set_from_engine` would swallow the
    // write anyway; the restart below is what makes the guard observable.
    //
    // **And it issues its own `gxRestart`** (`0x48c0b7 mov ecx,0x842978` → the console executor),
    // which is why `OptionsFrame_Save` calling `RestartGx()` afterwards restarts a resolution
    // change twice. Ours does the same, through the same counted request the host drains — benilla
    // applies the size live, so the restart is a re-assertion rather than a device rebuild, but the
    // CALL is the reference's and belongs here rather than only at the Okay button.
    lua.globals().set(
        "SetScreenResolution",
        lua.create_function(|lua, id: Option<f64>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.screen_resolutions.is_empty() {
                return Ok(());
            }
            // nil / non-numeric → 0 → entry 1. Truncate, then clamp both ends.
            let idx = id.map_or(0.0, f64::trunc);
            let last = model.screen_resolutions.len() - 1;
            let r = model.screen_resolutions[if idx < 1.0 {
                0
            } else {
                (idx as usize - 1).min(last)
            }];
            let want = r.to_string();
            if model
                .cvars
                .get("gxresolution")
                .is_some_and(|slot| slot.value.eq_ignore_ascii_case(&want))
            {
                return Ok(());
            }
            set_from_engine(&mut model, "gxResolution", want);
            model.restart_gx_asks += 1;
            Ok(())
        })?,
    )?;
    // ── GetRefreshRates ─────────────────────────────────────────────────────────────────────
    // **Zero return values, and that IS the reference's "nothing to offer" answer** —
    // `0x48c136 xor eax,eax; ret`. It was written here as a single `0` first, from
    // `OptionsFrame_GetRefreshRates`'s `if ( arg.n == 1 and arg[1] == 0 )` opening; the binary
    // contains **no `push 0` path at all**. That FrameXML branch is reachable only when the OS
    // reports `dmDisplayFrequency == 0` for every matching mode — Win32's encoding for "the
    // hardware default" — which the adjacent dedupe collapses to one `0`. It does not special-case
    // the empty answer: `for i = 1, 0` runs zero times and the dropdown is left **empty but
    // enabled**, which is what the reference does on a device with nothing to say.
    //
    // benilla's answer is always the empty one, because a refresh rate is only selectable through
    // an exclusive mode-set and this client ships none on any target (`benilla_app::video`'s
    // module doc walks each). Pushing the monitor's rates would be a dropdown whose every pick did
    // nothing — 2115 §2's wrong answer that succeeds — and pushing a lone `0` to grey the control
    // would be inventing a value the binary never produces.
    //
    // **The optional index argument is tolerated and ignored**, which is the reference's own shape
    // (`0x48c0ec xor esi,esi`): with no argument it asks for the rates of the FIRST — smallest —
    // resolution in the list, never the selected one, and the stock caller passes nothing.
    lua.globals().set(
        "GetRefreshRates",
        lua.create_function(|_, _: mlua::MultiValue| Ok(mlua::MultiValue::new()))?,
    )?;
    // ── GetVideoCaps ─────────────────────────────────────────────────────────────────────────
    // Seven values in `OptionsFrame.lua:59`'s own order, and **THREE push shapes, not two**
    // (`0x48db40`, each polarity read through to `lua_pushnil 0x6f37f0`):
    //
    // * slots 1,2,3,4,7 — **nil, or the number 1**. `caps+0xa8/0x98/0x94/0xa4/0xc4`. Two of them
    //   test against `-1` rather than 0, because D3D maps ps_1_1 and vs_1_1 to the *value* 0 and a
    //   `!= 0` test would report "no shaders" on hardware that has them.
    // * slot 5 `hasTripleBuffering` — **`0x48dbcb fild dword [edi+0x20]`, straight-line, NO
    //   branch**: always a number, never nil.
    // * slot 6 `maxAnisotropy` — **nil iff 0**, else the raw value.
    //
    // No `lua_pushboolean` anywhere; the `1` is the f64 `1.0`.
    //
    // **Slot 5 is a backend constant, and getting its TYPE right is what reproduces a stock
    // defect.** `caps+0x20` has two writers image-wide (the two device constructors: 1 under
    // Direct3D, 0 under OpenGL) and one reader, this verb — so `== 1` means "the backend is
    // Direct3D", independent of GPU, driver and the `gxTripleBuffer` CVar. `OptionsFrame_Load`
    // tests `not hasTripleBuffering` twice and `hasTripleBuffering == 1` once; `0` is TRUTHY in
    // Lua and slot 5 is never nil, so **the two `not` tests can never fire in either backend** and
    // only the `== 1` one works. benilla is not Direct3D and exposes no buffering knob, so it
    // answers the number **0**: check button 13 is hidden and button 6 re-seated
    // (`OptionsFrame.lua:168-175`), and the two dead clauses stay dead, which is correct.
    // Answering nil instead would *revive* them — a branch the reference cannot reach.
    lua.globals().set(
        "GetVideoCaps",
        lua.create_function(|lua, ()| {
            let caps = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .video_caps;
            let flag = |b: bool| if b { Value::Number(1.0) } else { Value::Nil };
            let mut out = mlua::MultiValue::new();
            out.push_back(flag(caps.anisotropic));
            out.push_back(flag(caps.pixel_shaders));
            out.push_back(flag(caps.vertex_shaders));
            out.push_back(flag(caps.trilinear));
            // Slot 5: unconditional number, per the straight-line `fild` above.
            out.push_back(Value::Number(if caps.triple_buffering { 1.0 } else { 0.0 }));
            // Slot 6: nil iff 0. The reference is effectively never nil here — its base ctor seeds
            // `caps+0xac = 1` — so a consumer gates on slot 1 or on `< 2`, never on nil.
            out.push_back(if caps.max_anisotropy == 0 {
                Value::Nil
            } else {
                Value::Number(caps.max_anisotropy as f64)
            });
            out.push_back(flag(caps.hardware_cursor));
            Ok(out)
        })?,
    )?;
    // ── RestartGx ────────────────────────────────────────────────────────────────────────────
    // "Apply the staged video settings now." `0x48dab0` is three instructions — it hands the line
    // `"gxRestart"` to the CONSOLE COMMAND EXECUTOR `0x63ce00`, which looks the action up and calls
    // its handler `0x639f60` **synchronously**. That handler reads no CVar by name: the staging was
    // done at `SetCVar` time by each gx CVar's own change callback, which writes the pending device
    // format record and can veto the write outright; the handler validates that record, applies it
    // with `DeviceSetFormat`, and then COMMITS the fourteen latched CVars so `GetCVar` catches up.
    //
    // **benilla's latch is the host's** (decision 2303): a gx write stages in the host's CVar
    // registry, and this verb is the host's cue to commit the gx stages (the fourteen-record
    // commit, `Cvars::commit_latched`) and re-assert them against the window. The one difference
    // left is when a change callback runs — the reference's at `SetCVar`, ours at the commit.
    //
    // A counted request the host drains (`take_restart_gx_asks`), the shape `Screenshot()` uses and
    // for the same reason: no payload, and two calls in a frame are two requests. **It is not a
    // stub** — a stub would be an empty body pretending the settings had been applied.
    // `gxMultisample` is the one setting no restart can deliver here, and none can in the reference
    // either: 1629 latches the sample count at camera spawn, which is what `SetMultisampleFormat`
    // above already documents as "applies at next launch".
    lua.globals().set(
        "RestartGx",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .restart_gx_asks += 1;
            Ok(())
        })?,
    )?;
    // ── RestoreVideoDefaults ─────────────────────────────────────────────────────────────────
    // **The Defaults button's whole effect, and the FrameXML proves it.** `OptionsFrame_SetDefaults`
    // sets every check button and slider from `GetCVarDefault` — and then the button's own
    // `<OnClick>` calls `HideUIPanel(OptionsFrame)` (`OptionsFrame.xml:688-690`), throwing those
    // widget values away without ever running `_Save`. So the widgets are cosmetic; this verb is
    // what actually moves the settings, and it must, or Defaults would do nothing at all.
    //
    // What it restores, and why that is not the reference's own values, is
    // [`VIDEO_DEFAULT_CVARS`]'s doc. Each row rides the ordinary change queue, so the host's knob
    // sync applies it and the config file is marked dirty — the same route a Lua `SetCVar` takes.
    // `set_from_engine`, not `write_cvar`, because this is the engine moving a value it owns rather
    // than a script asking: no `CVAR_UPDATE` token, the minimap-zoom shape.
    //
    // **The restart is the reference's own, in the reference's own place** — `0x639a51` calls the
    // `gxRestart` handler directly and synchronously, between the twelve gx writes and the sixteen
    // quality ones, and it destroys and recreates the device *and the OS window*. Ours issues the
    // same counted request; here it is a re-assertion rather than a rebuild, but the call belongs.
    lua.globals().set(
        "RestoreVideoDefaults",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            for name in VIDEO_DEFAULT_CVARS {
                let Some(default) = model
                    .cvars
                    .get(&name.to_ascii_lowercase())
                    .map(|slot| slot.default.clone())
                else {
                    // A bare VM with no host behind it registers none of these; there is nothing to
                    // restore and nothing to warn about, exactly as `set_from_engine` reasons.
                    continue;
                };
                set_from_engine(&mut model, name, default);
            }
            model.restart_gx_asks += 1;
            Ok(())
        })?,
    )?;
    // ── GetGamma / SetGamma ──────────────────────────────────────────────────────────────────
    // **The display-brightness pair** (2182) — `GetGamma 0x4891c0` and `SetGamma 0x4891f0`, carved
    // end to end.
    //
    // The one thing about them that is not obvious from the name: **the unit is the SLIDER's
    // offset, not the CVar's.** `0x4891d0` is `dc 2d`, ModRM reg field 5 = **FSUBR** (`mem − ST(0)`,
    // not `FSUB`), over `[0x8015b8]` = the f64 `1.0`. So `GetGamma()` answers `1.0 − gamma` and
    // `SetGamma(v)` writes `gamma := 1.0 − v` — the two compose to the identity, and with the
    // registered `gamma = "1.0"` the getter answers **0.0**, the exact centre of the stock
    // slider's `[-0.5, 0.5]`. Reading the pair as "gamma in, gamma out" would put a 1.0 through
    // `SetGamma` and write `gamma = 0`, i.e. `pow(x, 0) = 1`, a fully white ramp.
    //
    // **No clamp exists anywhere in the reference** — not in the binding, not in `CVar::Set`, not
    // in the change callback, not in the ramp builder — and the absence is earned rather than
    // assumed: the positive control is `baseMip`'s own callback `0x689090`, which *does* reject
    // out-of-`[0,1]` with `al = 0`. `SetGamma(5)` is accepted and writes `gamma = "-4.000000"`.
    // benilla's clamp is at the consumer (`ui_gamma::GAMMA_RANGE`), which is this crate's standing
    // posture and leaves the store's truth alone.
    lua.globals().set(
        "GetGamma",
        // No argument is read (the getter never calls `lua_gettop`), and one number comes back.
        lua.create_function(|lua, _: mlua::MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let gamma = model
                .cvars
                .get(&CVAR_GAMMA.to_ascii_lowercase())
                .and_then(|slot| slot.value.parse::<f64>().ok())
                // A VM whose host registered nothing (a bare test kit) answers as if the
                // registered default were in place, which is 0.0 — the slider's centre. The
                // reference has no such arm: its `0x63de30` lookup CREATES the record on a miss,
                // so `[eax+0x24]` is always readable.
                .unwrap_or(1.0);
            Ok(1.0 - gamma)
        })?,
    )?;
    lua.globals().set(
        "SetGamma",
        lua.create_function(|lua, value: Value| {
            // `lua_isnumber` (`0x4891fe`): a number, or a string Lua can convert to one. Anything
            // else raises through `0x6f4940`, which does not return — the trailing `xor eax,eax`
            // at `0x489215` is dead code.
            let v = match &value {
                Value::Integer(i) => *i as f64,
                Value::Number(n) => *n,
                Value::String(s) => {
                    match s.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()) {
                        Some(n) => n,
                        None => return Err(mlua::Error::runtime(USAGE_SET_GAMMA)),
                    }
                }
                _ => return Err(mlua::Error::runtime(USAGE_SET_GAMMA)),
            };
            // `SStrPrintf(buf, 0x10, "%f", 1.0 - v)` — six decimals into a SIXTEEN-byte buffer, so
            // a long value is truncated to 15 characters rather than rejected. Reproduced because
            // the truncated string is what the CVar then holds and what `GetCVar("gamma")` answers.
            let mut written = format!("{:.6}", 1.0 - v);
            written.truncate(15);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            write_cvar(&mut model, CVAR_GAMMA, written, None);
            // Zero return values, not nil (`eax = 0` at every `ret`).
            Ok(mlua::MultiValue::new())
        })?,
    )?;
    Ok(())
}

/// The reference's own display-gamma CVar — `0x402d70`, name string `0x82e924`, registered `"1.0"`,
/// flags 0. Defined in the crate that publishes the two verbs that read and write it, so the
/// binding and the name cannot drift apart; the app welds it to its own table.
pub const CVAR_GAMMA: &str = "gamma";

/// `0x8424cc`, verbatim.
const USAGE_SET_GAMMA: &str = "Usage: SetGamma(value)";

/// The `WorldDetail` slider's **stop**, benilla's spelling — the panel position `0`/`1`/`2`, held as
/// a CVar because 1.12 has no such CVar and our own Graphics page needs somewhere to keep it (the
/// `autoLootDefault` posture; the app's table carries the reasoning and the `Same` row).
///
/// Defined here, in the crate that publishes the two verbs that read and write it, so the binding
/// and the name cannot drift apart — as for [`CVAR_NAMEPLATE_ENEMIES`]. The app welds both with its
/// own test.
pub const CVAR_WORLD_DETAIL: &str = "WorldDetail";

/// The reference's own Environment Detail CVar — cells visited per chunk by the detail-doodad
/// scatter. [`WORLD_DETAIL_STOPS`] is what the slider writes into it.
pub const CVAR_FRILL_DENSITY: &str = "frillDensity";

/// **`SetWorldDetail`'s preset table, verbatim** — the three dwords at `0x804518`, `{0x10, 0x20,
/// 0x30}`. Read out of `WoW.exe` rather than transcribed from a note.
pub const WORLD_DETAIL_STOPS: [u32; 3] = [16, 32, 48];

/// **The Environment Detail pair** — `SetWorldDetail 0x488dd0` and `GetWorldDetail 0x488d70`, the
/// two engine bindings 1.12's own options panel drives that slider through (`OptionsFrame.lua`'s
/// row 3 is `func = "WorldDetail"`, and `OptionsFrame_Save`/`_Load` prefer `getglobal("Set"..func)`
/// / `getglobal("Get"..func)` over `SetCVar`/`GetCVar`). Decision 2163.
///
/// **The setter, carved end to end** (own decode):
///
/// ```text
/// 488ddf  call 0x6f34d0             ; lua_isnumber(L,1)? else error 0x8423e8
/// 488e05  call 0x6f3620             ; lua_tonumber(L,1)
/// 488e0a  call 0x40a2b0             ; -> int, RC forced to CHOP (`or ah,0x0c`): TRUNCATE toward zero
/// 488e11  test esi,esi; jl  0x488e9b ; n < 0  -> error 0x8423b8
/// 488e19  cmp  esi,3;   jge 0x488e9b ; n >= 3 -> error 0x8423b8
/// 488e1e  mov eax,[esi*4 + 0x804518] ; {16,32,48}      -> "%d"  -> CVar::Set "frillDensity"
/// 488e56  fld dword [esi*4+0x804524] ; {0.07,0.04,0.01} -> "%f" -> CVar::Set "smallCull"
/// ```
///
/// The two error strings are `0x8423e8` `"Usage: SetWorldDetail(value)"` and `0x8423b8` `"value
/// must be in the range 0, 2"` — both read out of the image, lowercase `v` and all. Every path
/// returns **zero** Lua values.
///
/// Three shapes worth stating because the obvious reading is wrong on each:
///
/// - **The truncation is toward zero, not a floor.** `0x40a2b0` saves the FPU control word, ORs
///   `0x0c` into the high byte (rounding-control = chop) and `fistp`s. So `SetWorldDetail(2.9)`
///   is stop 2 and `SetWorldDetail(-0.5)` is stop **0** — a negative that truncates to zero is
///   accepted, and only `<= -1` raises.
/// - **`lua_isnumber` accepts a numeric string**, so `SetWorldDetail("2")` works there and here.
/// - **The getter is not the setter's inverse.** `0x488d70` reads only `smallCull`, seeds its
///   result at **2**, and runs `for i in 0..3 { if v <= tbl[i] { result = i } }` over the
///   *descending* `{0.07, 0.04, 0.01}` with **no break** — so the last match wins, and any
///   `smallCull` above `0.07` (legal to 2.0) saturates to 2, i.e. the least detail reads back as
///   the most. A shipped defect, and one of two: `OptionsFrame_SetDefaults` tests that same
///   descending table with ascending `elseif`s, so Restore Defaults always parks the slider at 0.
///
/// **Where benilla diverges, and why it is not the smallCull ladder.** `SmallCull` is a dead knob
/// in the reference — `[0x868620]` has one writer and no reader image-wide — so its *only*
/// consumer there is this getter,
/// which makes it storage for a stop the client keeps nowhere else. benilla keeps the stop:
/// [`CVAR_WORLD_DETAIL`] is it, and `frillDensity` is the same knob in the reference's unit. So the
/// getter reads the stop directly and `SmallCull` is not registered, because registering a CVar
/// whose only purpose is to hold a number we already hold is the silent pretence 1203 forbids.
///
/// The observable consequence is **nil at boot and nil for every consumer we know**: a fresh
/// reference client has `frillDensity 16` with `smallCull 0.04` — stop 0's frill with stop 1's cull
/// — so its getter answers **1**, and benilla's `WorldDetail` registers at `"1"` for exactly that
/// reason. The one input that separates them is a bare `frillDensity` write with no stop write,
/// which there leaves the getter on the stale stop and here moves it; pfUI's `hdgraphic` is the
/// only known caller and its own replacement short-circuits above 48, so it never reaches the
/// difference.
fn install_world_detail_verbs(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "SetWorldDetail",
        lua.create_function(|lua, value: Value| {
            // `lua_isnumber`: a number, or a string Lua can convert to one. Anything else is the
            // usage error — NOT a silent no-op.
            let n = match &value {
                Value::Integer(i) => *i as f64,
                Value::Number(n) => *n,
                Value::String(s) => {
                    match s.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()) {
                        Some(n) => n,
                        None => return Err(mlua::Error::runtime(USAGE_SET_WORLD_DETAIL)),
                    }
                }
                _ => return Err(mlua::Error::runtime(USAGE_SET_WORLD_DETAIL)),
            };
            // Truncate toward zero (`0x40a2b0`'s chop), then the reference's own two bounds. NaN
            // has no truncation — the reference's `fistp` yields the integer indefinite, which is
            // negative and raises, so raising here is the same answer by the same door.
            if n.is_nan() {
                return Err(mlua::Error::runtime(RANGE_SET_WORLD_DETAIL));
            }
            let stop = n.trunc();
            if !(0.0..3.0).contains(&stop) {
                return Err(mlua::Error::runtime(RANGE_SET_WORLD_DETAIL));
            }
            let stop = stop as usize;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // TWO CVars per stop, as `0x488dd0` writes two: there it is `frillDensity` + the dead
            // `smallCull`; here it is `frillDensity` + the stop the panel keeps. Both land on one
            // clutter knob in the host, so applying both is idempotent — and writing only one
            // would leave the other stale for a `GetWorldDetail` in the same Lua tick, since the
            // host does not drain the change queue until the end of the frame.
            write_cvar(
                &mut model,
                CVAR_FRILL_DENSITY,
                WORLD_DETAIL_STOPS[stop].to_string(),
                None,
            );
            write_cvar(&mut model, CVAR_WORLD_DETAIL, stop.to_string(), None);
            // Zero return values, not nil (`eax = 0` at every `ret`).
            Ok(mlua::MultiValue::new())
        })?,
    )?;
    g.set(
        "GetWorldDetail",
        // `MultiValue` in: the reference reads argument 1 for the SETTER only; the getter never
        // calls `lua_gettop`, so an argument is accepted and ignored — and pfUI's replacement is
        // declared `function _G.GetWorldDetail(arg)`, so it is passed one in practice.
        lua.create_function(|lua, _: mlua::MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let stop = model
                .cvars
                .get(&CVAR_WORLD_DETAIL.to_ascii_lowercase())
                .and_then(|slot| slot.value.parse::<f64>().ok())
                // The reference can only ever answer 0, 1 or 2 (its result is an index into a
                // three-entry table), so an off-grid stop — which ours can hold, because a console
                // `frillDensity 200` moves the same knob — reports the nearest one rather than a
                // number no caller has a branch for.
                .map_or(0, |v| v.round().clamp(0.0, 2.0) as i64);
            Ok(stop)
        })?,
    )?;
    Ok(())
}

/// `0x8423e8`, verbatim.
const USAGE_SET_WORLD_DETAIL: &str = "Usage: SetWorldDetail(value)";

/// `0x8423b8`, verbatim — lowercase `v`, and the odd comma is the reference's own.
const RANGE_SET_WORLD_DETAIL: &str = "value must be in the range 0, 2";

/// **The four nameplate verbs** — `ShowNameplates 0x489450`, `HideNameplates 0x489460`,
/// `ShowFriendNameplates 0x489470`, `HideFriendNameplates 0x489480`.
///
/// Each is a **10-byte body** over one of two setters, and the reference's shape is worth stating
/// because two obvious readings are wrong:
///
/// - **They read NO argument.** `ShowNameplates(false)` still shows. There is no `lua_gettop`, no
///   `lua_toboolean`, nothing — the verb *is* the value, which is why there are four of them
///   rather than two taking a flag.
/// - **They return ZERO Lua values** — not `nil`, nothing. (`eax` at the `ret` is the return
///   count, read off the VM's own C arm `0x6f61a8`.)
/// - They are **four separate bodies**, not a masked pair like `UnitIsTapped`/`ByPlayer`:
///   `0x489450` and `0x489470` are literally identical byte strings, rel32 included, over setters
///   that differ only in their `or`/`and` masks.
///
/// The engine state is bits `0x1` (enemy) and `0x8` (friend) of one runtime dword `[0xc4da34]`.
/// **There is no getter** — 15 references image-wide, all `.text`, and the 1153 `{name, fn}`
/// records across all 54 registrar tables intersect the readers in the empty set — so an addon can
/// set the state and never read it back, and neither can we.
///
/// **Where benilla diverges, deliberately and now knowingly.** 1.12 registers no nameplate CVar
/// (censused: `0x63db90`'s 214 sites, 207 literal names, zero matching `/plate/i`), and the engine
/// clears **both bits on every `EnterWorld`** — not once at boot. FrameXML replays them from its
/// own `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON` saved variables. Our two CVars reproduce *FrameXML's
/// replay*, not the engine's state model: a benilla player's plates survive a zone-in because the
/// CVar store outlives it. That is a divergence in favour of the player, recorded here rather than
/// mistaken for fidelity.
fn install_nameplate_verbs(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    for (name, cvar, on) in [
        ("ShowNameplates", CVAR_NAMEPLATE_ENEMIES, true),
        ("HideNameplates", CVAR_NAMEPLATE_ENEMIES, false),
        ("ShowFriendNameplates", CVAR_NAMEPLATE_FRIENDS, true),
        ("HideFriendNameplates", CVAR_NAMEPLATE_FRIENDS, false),
    ] {
        g.set(
            name,
            // `MultiValue` and not `()`: mlua would otherwise reject a call that passes anything,
            // and the reference accepts and ignores every argument. Returning `MultiValue::new()`
            // is the zero-value return, which is NOT the same as pushing nil.
            lua.create_function(move |lua, _: mlua::MultiValue| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let key = cvar.to_ascii_lowercase();
                if let Some(slot) = model.cvars.get_mut(&key) {
                    let value = if on { "1" } else { "0" };
                    if slot.value != value {
                        slot.value = value.to_string();
                        let reg_name = slot.name.clone();
                        model.cvar_changes.push((reg_name, value.to_string()));
                    }
                }
                Ok(mlua::MultiValue::new())
            })?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MultisampleFormat, SeededCvar};
    use crate::script::UiScript;

    fn script_with_volume() -> UiScript {
        let s = UiScript::new().unwrap();
        s.register_cvars([("MusicVolume", "0.4"), ("MasterVolume", "1.0")]);
        s
    }

    #[test]
    fn the_table_round_trips_and_queues_changes_case_insensitively() {
        let mut s = script_with_volume();
        // Registration seeds value = default; reads are case-insensitive.
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("musicvolume")"#)
                .unwrap(),
            "0.4"
        );
        // A Lua write queues ONE change under the REGISTERED spelling; the number stringifies.
        s.run(r#"SetCVar("MUSICVOLUME", 0.7)"#).unwrap();
        assert_eq!(
            s.take_cvar_changes(),
            vec![("MusicVolume".to_string(), "0.7".to_string())]
        );
        assert!(s.take_cvar_changes().is_empty(), "drained");
        // The default is unmoved and separately readable.
        assert_eq!(
            s.eval::<String>(r#"return GetCVarDefault("MusicVolume")"#)
                .unwrap(),
            "0.4"
        );
        assert_eq!(s.cvar("MusicVolume").as_deref(), Some("0.7"));
        // A write to the same value queues nothing (quiet frames stay quiet).
        s.run(r#"SetCVar("MusicVolume", "0.7")"#).unwrap();
        assert!(s.take_cvar_changes().is_empty());
    }

    /// **A latched row stages the write and keeps answering the applied value** (decision 2303)
    /// — the reference's `Set 0x63df50` on flag bit1: `latchedValue` takes the string, `InternalSet`
    /// never runs, so `GetCVar` (which reads `rec+0x20`) answers the old value until
    /// `CVar::Update 0x63e060` commits it. The host still hears every staged write through the
    /// change queue (it holds the pending copy), and its commit arrives as a host write, which
    /// clears the stage. Writing the applied value back over a stage clears the stage too.
    #[test]
    fn a_latched_row_stages_the_write_until_the_host_commits_it() {
        let mut s = UiScript::new().unwrap();
        s.seed_cvars([
            SeededCvar {
                name: "gxVSync".into(),
                value: "1".into(),
                default: "1".into(),
                latched: true,
            },
            SeededCvar {
                name: "MusicVolume".into(),
                value: "0.4".into(),
                default: "0.4".into(),
                latched: false,
            },
        ]);
        s.run(r#"SetCVar("gxVSync", 0)"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxVSync")"#).unwrap(),
            "1",
            "the applied value stands until the boundary"
        );
        assert_eq!(
            s.take_cvar_changes(),
            vec![("gxVSync".to_string(), "0".to_string())],
            "the host hears the staged write"
        );
        // Staging the same value again is quiet; staging the applied value back clears the stage
        // and is reported, so the host can drop its pending copy too.
        s.run(r#"SetCVar("gxVSync", "0")"#).unwrap();
        assert!(s.take_cvar_changes().is_empty());
        s.run(r#"SetCVar("gxVSync", "1")"#).unwrap();
        assert_eq!(
            s.take_cvar_changes(),
            vec![("gxVSync".to_string(), "1".to_string())]
        );
        assert!(
            s.take_cvar_changes().is_empty(),
            "nothing staged, nothing to say"
        );
        // The commit is a host write: the value moves, the stage clears, no echo.
        s.run(r#"SetCVar("gxVSync", 0)"#).unwrap();
        s.take_cvar_changes();
        s.set_cvar_host("gxVSync", "0");
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxVSync")"#).unwrap(),
            "0"
        );
        assert!(s.take_cvar_changes().is_empty());
        // An unlatched sibling is untouched by any of this.
        s.run(r#"SetCVar("MusicVolume", 0.7)"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("MusicVolume")"#)
                .unwrap(),
            "0.7"
        );
    }

    /// **An addon's `RegisterCVar` reaches the host** (decision 2303): the row it creates is
    /// reported once, with the declared default, so the host's registry — the store that
    /// outlives this VM — can carry it. A re-declaration of a live name is the no-op it always
    /// was, and reports nothing.
    #[test]
    fn an_addon_registration_is_reported_to_the_host_once() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"RegisterCVar("myAddonKnob", "7")"#).unwrap();
        s.run(r#"RegisterCVar("myAddonKnob", "9")"#).unwrap();
        assert_eq!(
            s.take_cvar_registrations(),
            vec![("myAddonKnob".to_string(), "7".to_string())]
        );
        assert!(s.take_cvar_registrations().is_empty(), "drained");
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("myAddonKnob")"#)
                .unwrap(),
            "7"
        );
    }

    /// **A bare CVar name through `ConsoleExec` is the host's to answer** (decision 2303): the
    /// reference's per-CVar console command prints `CVar "%s" is "%s"` on an empty argument
    /// (`0x63dde0`), and that printing lives host-side with the rest of the command registry.
    /// A name WITH a value is still written synchronously, so the next Lua line reads it back.
    #[test]
    fn console_exec_writes_a_valued_cvar_and_hands_a_bare_name_to_the_host() {
        let mut s = script_with_volume();
        s.run(r#"ConsoleExec("MusicVolume 0.2")"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("MusicVolume")"#)
                .unwrap(),
            "0.2"
        );
        assert!(
            s.take_console_lines().is_empty(),
            "a valued write is consumed here"
        );
        s.run(r#"ConsoleExec("MusicVolume")"#).unwrap();
        assert_eq!(s.take_console_lines(), vec!["MusicVolume".to_string()]);
    }

    /// **`SetCVar`'s third argument is the `CVAR_UPDATE` token** (decision 1140) — the whole
    /// mechanism behind that event. 1.12's options panels pass their CheckButtons table KEY
    /// (`SetCVar(value.cvar, value.value, index)`), an uppercase display name, and the engine
    /// hands it back verbatim as arg1 — which is what lets `UIOptionsFrame_OnEvent` do
    /// `UIOptionsFrameCheckButtons[arg1]`. Omit it and nothing fires: the event is opt-in per
    /// write. A write that changes nothing fires nothing either, like the change queue.
    #[test]
    fn the_third_argument_is_the_cvar_update_token() {
        let mut s = script_with_volume();
        s.run(
            "SEEN = {} \
             f = CreateFrame(\"Frame\") \
             f:RegisterEvent(\"CVAR_UPDATE\") \
             f:SetScript(\"OnEvent\", function() table.insert(SEEN, arg1 .. \"=\" .. arg2) end)",
        )
        .unwrap();

        // No token: the value moves, the change queues, the event does not fire.
        s.run(r#"SetCVar("MusicVolume", "0.5")"#).unwrap();
        s.tick(0.0);
        assert_eq!(s.eval::<f64>("return getn(SEEN)").unwrap(), 0.0);

        // With one: arg1 is the token verbatim (the display name, NOT the CVar's own name),
        // arg2 the new value.
        s.run(r#"SetCVar("MusicVolume", "0.6", "MUSIC_VOLUME")"#)
            .unwrap();
        s.tick(0.0);
        assert_eq!(
            s.eval::<String>("return SEEN[1]").unwrap(),
            "MUSIC_VOLUME=0.6"
        );

        // A no-op write is silent on both channels.
        s.run(r#"SetCVar("MusicVolume", "0.6", "MUSIC_VOLUME")"#)
            .unwrap();
        s.tick(0.0);
        assert_eq!(s.eval::<f64>("return getn(SEEN)").unwrap(), 1.0);
        assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    }

    #[test]
    fn host_writes_do_not_echo_and_snapshots_carry_defaults() {
        let mut s = script_with_volume();
        s.set_cvar_host("MasterVolume", "0.25");
        assert!(
            s.take_cvar_changes().is_empty(),
            "a host write must not re-dirty the config it just loaded"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("MasterVolume")"#)
                .unwrap(),
            "0.25"
        );
        let snap = s.cvars_snapshot();
        let master = snap.iter().find(|(n, _, _)| n == "MasterVolume").unwrap();
        assert_eq!((master.1.as_str(), master.2.as_str()), ("0.25", "1.0"));
    }

    #[test]
    fn unknown_names_warn_once_and_no_op() {
        let mut s = script_with_volume();
        assert!(s
            .eval::<bool>(r#"return GetCVar("bogusKnob") == nil"#)
            .unwrap());
        s.run(r#"SetCVar("bogusKnob", 1)"#).unwrap();
        s.run(r#"SetCVar("bogusKnob", 2)"#).unwrap();
        assert!(s.take_cvar_changes().is_empty());
        let warns: Vec<String> = s.take_warnings();
        assert_eq!(warns.len(), 1, "warn-once: {warns:?}");
        assert!(warns[0].contains("bogusKnob"));
        // Registration is idempotent and never clobbers a live value.
        s.set_cvar_host("MusicVolume", "0.9");
        s.register_cvars([("MusicVolume", "0.4")]);
        assert_eq!(s.cvar("MusicVolume").as_deref(), Some("0.9"));
    }
    /// **`GetCVar("realmName")` answers the session's realm, set by the one seam that owns it.**
    ///
    /// It is a real 1.12 CVar (`0x83f2d0`, persisted — the client builds its SavedVariables path
    /// from it) and it had no value here at all. `Ace/AceState.lua:27` is
    /// **Registration honors the saved base** (decision 1291) — the bridge that makes the
    /// per-VM table behave like the reference's engine-side store: a knobless CVar keeps the
    /// player's persisted value across a VM replacement, and the default stays the DEFAULT so
    /// "moved off default" still means something to the saver.
    #[test]
    fn registration_starts_at_the_saved_value_not_the_default() {
        let mut s = UiScript::new().unwrap();
        s.set_cvar_saved_base([("StatusBarText".to_string(), "1".to_string())]);
        s.register_cvars([("statusBarText", "0"), ("farclip", "500")]);

        assert_eq!(
            s.eval::<String>(r#"return GetCVar("statusBarText")"#)
                .unwrap(),
            "1",
            "a saved value outranks the registered default (any key case)"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVarDefault("statusBarText")"#)
                .unwrap(),
            "0",
            "…while the default stays the default"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("farclip")"#).unwrap(),
            "500",
            "a key the file never carried starts at its default"
        );
    }

    /// The same law through the Lua half: an addon's `RegisterCVar` (decision 1195) starts at
    /// the persisted value, so its setting survives the VM being replaced (1290/1291).
    #[test]
    fn an_addon_registered_cvar_starts_at_its_saved_value() {
        let mut s = UiScript::new().unwrap();
        s.set_cvar_saved_base([("myaddon_scale".to_string(), "2.5".to_string())]);
        s.run(r#"RegisterCVar("MyAddon_Scale", "1.0")"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("MyAddon_Scale")"#)
                .unwrap(),
            "2.5"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVarDefault("MyAddon_Scale")"#)
                .unwrap(),
            "1.0",
            "the declared value is the DEFAULT, not the live value"
        );
    }

    /// The Video options Multisampling dropdown, driven exactly as `OptionsFrame.lua` drives it:
    /// `_Initialize` walks `GetMultisampleFormats()` three varargs at a time, `_OnLoad` seeds the
    /// selection from `GetCurrentMultisampleFormat()`, and the Okay handler (l.240) writes back
    /// through `SetMultisampleFormat(id)`.
    #[test]
    fn the_multisample_dropdown_round_trips_through_the_three_bindings() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([
            ("gxColorBits", "32"),
            ("gxDepthBits", "32"),
            ("gxMultisample", "1"),
        ]);
        s.set_multisample_formats(vec![
            MultisampleFormat {
                color_bits: 32,
                depth_bits: 32,
                samples: 1,
            },
            MultisampleFormat {
                color_bits: 32,
                depth_bits: 32,
                samples: 2,
            },
            MultisampleFormat {
                color_bits: 32,
                depth_bits: 32,
                samples: 4,
            },
        ]);

        // Flat triples, in order — the shape `for i=1, arg.n, 3` walks.
        let flat: Vec<f64> = s.eval(r#"return { GetMultisampleFormats() }"#).unwrap();
        assert_eq!(
            flat,
            vec![32.0, 32.0, 1.0, 32.0, 32.0, 2.0, 32.0, 32.0, 4.0],
            "three fields per entry, entries in offer order"
        );

        // 1-BASED: the default `gxMultisample "1"` is the FIRST row, not the zeroth.
        assert_eq!(
            s.eval::<f64>("return GetCurrentMultisampleFormat()")
                .unwrap(),
            1.0
        );

        // Pick 4x — the third row — and all three CVars move together, as `0x48c640` writes them.
        s.eval::<()>("SetMultisampleFormat(3)").unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxMultisample")"#)
                .unwrap(),
            "4"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxColorBits")"#)
                .unwrap(),
            "32"
        );
        assert_eq!(
            s.eval::<f64>("return GetCurrentMultisampleFormat()")
                .unwrap(),
            3.0,
            "the selection round-trips: what Set wrote, GetCurrent finds"
        );

        // The host sees the write, so the config file is dirtied — the same route a Lua SetCVar
        // takes. Without this the player's choice would live only until they quit.
        let changed: Vec<String> = s
            .take_cvar_changes()
            .into_iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect();
        assert!(
            changed.contains(&"gxMultisample=4".to_string()),
            "expected gxMultisample on the change queue, got {changed:?}"
        );

        // Off the end is ignored, never clamped: a clamp would silently apply a format the player
        // did not choose.
        s.eval::<()>("SetMultisampleFormat(99)").unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxMultisample")"#)
                .unwrap(),
            "4",
            "an out-of-range id must leave the selection alone"
        );
    }

    /// `ace.trim(GetCVar("realmName"))` inside `SetGameState`, which EVERY Ace addon runs at
    /// PLAYER_ENTERING_WORLD, so nil became `gsub(nil, ...)` and took the whole family down.
    ///
    /// Asserted through `set_realm_name` rather than by writing the CVar directly, because that is
    /// the point: the CVar and `GetRealmName()` are the SAME fact and must not be settable apart.
    #[test]
    fn the_realm_name_cvar_and_get_realm_name_are_one_fact() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("realmName", "")]);

        // Before a session: empty, never nil — `ace.trim` must have a string to gsub.
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("realmName")"#).unwrap(),
            ""
        );

        s.set_realm_name("Archimonde");
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("realmName")"#).unwrap(),
            "Archimonde",
            "the realm seam must write the CVar too, or the two facts drift"
        );

        // Ace's own line, run for real.
        let trimmed: String = s
            .eval(r#"return string.gsub(GetCVar("realmName"), "^%s*", "")"#)
            .unwrap();
        assert_eq!(trimmed, "Archimonde");
    }
}
