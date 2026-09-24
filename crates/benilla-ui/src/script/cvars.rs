//! The CVar table: the reference's store of case-insensitively named string settings with
//! registered defaults, behind `GetCVar`/`SetCVar`/`GetCVarDefault`/`RegisterCVar`.
//!
//! The host registers the vars it backs and drains Lua's writes from a change queue; its own
//! writes do not queue. The table dies with the VM, where the reference's outlives every
//! `ReloadUI`, so the host seeds each new VM with the saved values before anything registers.
//! An unknown name warns once and is ignored (`GetCVar` answers nil), where the reference's
//! `GetCVar`, `SetCVar` and `GetCVarDefault` raise `Couldn't find CVar named '%s'` (`0x842310`).

use mlua::{Lua, Value};

use super::{Model, ScriptValue};

/// One registered CVar.
#[derive(Clone, Debug)]
pub(crate) struct CvarSlot {
    /// The registered spelling, reported on change events and in snapshots; the table key is
    /// its lowercase form.
    pub name: String,
    pub value: String,
    pub default: String,
    /// The reference's flag bit1 (`rec+0x1c & 0x2`): a write goes to [`Self::pending`] and
    /// `GetCVar` answers the applied value until the host commits it (`CVar::Update 0x63e060`).
    /// Host-seeded only; a VM-registered row is never latched, like a console-created one.
    pub latched: bool,
    /// A latched row's staged value, reported to the host through the change queue when written.
    pub pending: Option<String>,
}

/// One row of the host's registry as the VM's mirror is seeded from it; `value` is the applied
/// value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeededCvar {
    pub name: String,
    pub value: String,
    pub default: String,
    pub latched: bool,
}

/// One multisample format the device supports, the reference's triple (`[0xb4b444]`, count
/// `[0xb4b440]`, built by `0x48c3e0`). `samples == 1` means no multisampling, as both reference
/// enumerators normalise it (D3D `0x58b8b1`, GL `0x58d5fa`) and the device paths test `> 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultisampleFormat {
    pub color_bits: u32,
    pub depth_bits: u32,
    pub samples: u32,
}

/// One screen resolution the Video options dropdown offers, in physical pixels. Deviation: the
/// list is the window sizes this client can present, not the device's display modes, because
/// benilla makes no exclusive mode-set. `"WxH"` is a contract: `OptionsFrame.lua:281-284` splits
/// on the `x` and `CT_Viewport.lua:107` matches `"(%d+)x(%d+)"`, both failing silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenResolution {
    pub width: u32,
    pub height: u32,
}

impl ScreenResolution {
    /// Pixel area first, the reference's whole comparator (`0x58be40`), then width, then height.
    /// Deviation: the tie-break is ours, because area alone with the reference's adjacent-only
    /// dedupe can leave a duplicate `"WxH"` when two modes share an area (1280x960, 1600x768).
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

/// What `GetVideoCaps` answers, in `OptionsFrame.lua:59`'s order: anisotropic, pixel shaders,
/// vertex shaders, trilinear, triple buffering, max anisotropy, hardware cursor. The host pushes
/// what wgpu and this client's presentation path really offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct VideoCaps {
    pub anisotropic: bool,
    pub pixel_shaders: bool,
    pub vertex_shaders: bool,
    pub trilinear: bool,
    /// A backend constant, not a device capability: 1 under Direct3D, 0 under OpenGL
    /// (`caps+0x20`, read only by `GetVideoCaps 0x48db40`). False here, pushed as the number 0.
    pub triple_buffering: bool,
    /// The raw maximum: `OptionsFrame.lua:122-127` makes a value in `ANISOTROPIC_VALUES` the
    /// slider's ceiling as its index, and any other number the ceiling as is. Pushed as nil when 0.
    pub max_anisotropy: u32,
    pub hardware_cursor: bool,
}

impl super::UiScript {
    /// Hand registration the config file's saved values (keys lowercased), before any
    /// `register_cvars` or `RegisterCVar` runs in this VM: a name registered while present here
    /// starts at its saved value. Without it a knobless or addon-declared CVar would revert to its
    /// default in every new VM, and the saver would then drop the player's setting from the file.
    pub fn set_cvar_saved_base(&mut self, entries: impl IntoIterator<Item = (String, String)>) {
        self.model_mut().cvars_saved_base = entries
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect();
    }

    /// Register the host-backed CVars as `(name, default)`, idempotently: a re-register refreshes
    /// the default and keeps a live value, and a new row starts at its saved value if the config
    /// file has one. `&self` so the interface loader can register from a `&UiScript`: the stock
    /// `UIOptionsFrame.xml` reads CVars in its own `OnLoad`.
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

    /// Seed the mirror from the host's registry, creating or overwriting each row. The host's
    /// table outlives the VM, so a re-seed re-asserts it and drops any staged value (the host
    /// holds the pending copy). `&self` for [`Self::register_cvars`]'s reason.
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

    /// Host-side write (a loaded value, a committed latch): sets the value and clears any staged
    /// one without queueing a change, since an echo would re-dirty the file the host just read.
    /// An unknown name warns once.
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

    /// Snapshot the table as `(name, value, default)`; the saver writes only rows off their
    /// default, so the config file stays a diff.
    pub fn cvars_snapshot(&self) -> Vec<(String, String, String)> {
        self.model_mut()
            .cvars
            .values()
            .map(|s| (s.name.clone(), s.value.clone(), s.default.clone()))
            .collect()
    }

    /// Drain the `(name, new_value)` changes queued since the last call, under the registered
    /// spelling: the host's cue to sync its resources and mark the config dirty.
    pub fn take_cvar_changes(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.model_mut().cvar_changes)
    }

    /// Drain the `(name, default)` rows an addon's `RegisterCVar` created since the last call; the
    /// host's registry keeps them beside its own, as the reference's one table does.
    pub fn take_cvar_registrations(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.model_mut().cvar_registrations)
    }

    /// A native widget's write (the glue AddOns screen's "Load out of date AddOns" checkbox): sets
    /// the value and queues the change like a Lua `SetCVar`, so the config file is dirtied.
    pub fn set_cvar_engine(&mut self, name: &str, value: &str) {
        set_from_engine(&mut self.model_mut(), name, value.to_string());
    }

    /// Publish the multisample formats this run's device accepts, in dropdown order. The
    /// reference's `0x48c3e0` sweeps its candidate table `0x85a83c` (D3D) or every pixel format
    /// (GL); here the host asks wgpu.
    pub fn set_multisample_formats(&mut self, formats: Vec<MultisampleFormat>) {
        self.model_mut().multisample_formats = formats;
    }

    /// Publish the dropdown's resolutions and the client's current one, sorted by
    /// [`ScreenResolution::sort_key`] and deduplicated. `current` is inserted when the list lacks
    /// it (a 1600x900 window on a 4K panel): `CT_Viewport.lua:201` reads
    /// `arg[GetCurrentResolution()]` and silently falls back to 4:3 on a miss.
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

    /// Publish what this run's device and presentation path offer, behind `GetVideoCaps`.
    pub fn set_video_caps(&mut self, caps: VideoCaps) {
        self.model_mut().video_caps = caps;
    }

    /// Drain the `RestartGx()` calls queued since the last call; the host answers each by
    /// re-asserting the staged video settings against the window.
    pub fn take_restart_gx_asks(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().restart_gx_asks)
    }
}

/// An engine-side write, for a value the engine owns: queued like a Lua `SetCVar` so the config
/// file is dirtied, as the reference's minimap `set_zoom` (`0x6da8e0`) also `CVar::Set`s
/// `minimapZoom`/`minimapInsideZoom`. An unregistered name is a silent no-op (a bare VM).
pub(super) fn set_from_engine(model: &mut Model, name: &str, value: String) {
    store(model, &name.to_ascii_lowercase(), value);
}

/// The one store every write goes through; returns the registered spelling when the value moved
/// and the change was queued. A latched slot stages the write, as the reference's `Set 0x63df50`
/// stores `latchedValue` and skips `InternalSet`; writing back the applied value unstages it.
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

/// The `SetCVar` write, shared with `ConsoleExec`: the change is queued only when the value
/// moved, and `CVAR_UPDATE` fires only when a token was passed. `false` for an unknown name.
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

/// Coerce a Lua argument to the stored string; the client takes a number too
/// (`SetCVar("MusicVolume", 0.4)` is the common call). A boolean becomes "1" or "0", where the
/// reference's `SetCVar` and `RegisterCVar` store "0" for anything but a string or a number
/// (`0x488c98`, `0x488b6f`).
fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => s.to_str().ok().map(|s| s.to_owned()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Boolean(b) => Some(if *b { "1".into() } else { "0".into() }),
        _ => None,
    }
}

/// The enemy-plate toggle's CVar, which the app reads rather than re-spelling. Deviation: 1.12
/// registers no nameplate CVar; the toggles persist under the later clients' names rather than
/// invented ones.
pub const CVAR_NAMEPLATE_ENEMIES: &str = "nameplateShowEnemies";
/// The friendly-plate half of [`CVAR_NAMEPLATE_ENEMIES`].
pub const CVAR_NAMEPLATE_FRIENDS: &str = "nameplateShowFriends";

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // `RegisterCVar(name, value)` declares a CVar the client does not ship, which is how an addon
    // persists a setting. Re-declaring a live name is a no-op: the reference never resets a live
    // value on re-registration.
    lua.globals().set(
        "RegisterCVar",
        lua.create_function(|lua, (name, value): (String, Option<Value>)| {
            let value = value.as_ref().and_then(value_to_string).unwrap_or_default();
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let key = name.to_ascii_lowercase();
            // A saved value outranks the declared one, which stays the default for the saver.
            let saved = model.cvars_saved_base.get(&key).cloned();
            if model.cvars.contains_key(&key) {
                return Ok(());
            }
            // The host gives the row a place in its registry, which outlives this VM.
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
    // As `OptionsFrame.lua` drives it: `_OnLoad` seeds the selection from
    // `GetCurrentMultisampleFormat()`, `_Initialize` walks `GetMultisampleFormats()` three values
    // at a time, and the Okay handler (l.240) calls `SetMultisampleFormat`. Registration table
    // `0x83de68`, records `0x83e2c0`/`0x83e2c8`/`0x83e2d0`.
    lua.globals().set(
        "GetMultisampleFormats",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // Flat, three per entry (`0x48c360`); no formats is no values and an empty menu.
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
            // 1-based, and 1.0 on no match (`0x48c580`): `_OnLoad` hands it straight to
            // `UIDropDownMenu_SetSelectedID`, so a miss must still name a real row.
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
            // 1-based from `UIDropDownMenu_GetSelectedID`. Deviation: an id out of range is
            // ignored, because the reference reads one entry past the last (`0x48c67c`) and a
            // clamp would apply a format the player did not pick. A missing id is ignored too,
            // where the reference takes the first entry (`0x48c657`).
            let Some(f) = id
                .filter(|v| *v >= 1.0)
                .and_then(|v| model.multisample_formats.get(v as usize - 1).copied())
            else {
                return Ok(());
            };
            // All three from the chosen entry, as `0x48c640` writes them, through the change queue.
            set_from_engine(&mut model, "gxColorBits", f.color_bits.to_string());
            set_from_engine(&mut model, "gxDepthBits", f.depth_bits.to_string());
            set_from_engine(&mut model, "gxMultisample", f.samples.to_string());
            // Deviation: the reference then runs the console command `gxRestart` (`0x842978`
            // through `0x63ce00`); none is queued here, because `gxMultisample` applies at next
            // launch anyway: the camera reads its sample count once, at spawn.
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
            // The third argument is the `CVAR_UPDATE` token, passed through as arg1; without one
            // nothing fires. The 1.12 options panels pass their CheckButtons key, such as
            // "STATUS_BAR_TEXT" (`UIOptionsFrame.lua` l.335, `OptionsFrame.lua` l.192), which is
            // how `UIOptionsFrame_OnEvent` finds its row.
            let token = it.next().and_then(value_to_string);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            write_cvar(&mut model, &name, value, token);
            Ok(())
        })?,
    )?;

    install_nameplate_verbs(lua)?;
    install_world_detail_verbs(lua)
}

/// What `RestoreVideoDefaults` puts back: the CVars among the reference verb's writes that benilla
/// registers, each at its registered default. The verb writes no `uiScale`, `useUiScale` or
/// `anisotropic`.
///
/// Deviation: the reference (`0x48dad0` into `0x639a20`) writes hardware-recommended values from
/// its matched `VideoHardware.dbc` row and the `CGxFormat` preset it names (`[0xc4e6ac]`): twelve
/// `gx*` CVars, a synchronous `RestartGx`, then sixteen more. benilla has no such matching, so its
/// registered defaults are its recommended configuration.
pub const VIDEO_DEFAULT_CVARS: &[&str] = &[
    // Of the twelve `gx*` the device record writes (`0x639c40`), the six benilla registers.
    "gxWindow",
    "gxResolution",
    "gxVSync",
    "gxColorBits",
    "gxDepthBits",
    "gxMultisample",
    // Of the sixteen further graphics CVars (`0x639a60`), the three benilla registers.
    "farclip",
    "frillDensity",
    "trilinear",
    // Ours, restored with `frillDensity` so the pair keeps describing one detail level.
    "WorldDetail",
];

/// The Video options window's engine verbs that nothing else here provides. Three run at load,
/// as `UIDropDownMenu_Initialize` calls its initializer at once (`UIDropDownMenu.lua:48-50`):
/// `GetScreenResolutions`, `GetCurrentResolution` and `GetRefreshRates`.
///
/// Ten names must stay nil so their slider rows fall back to `GetCVar`/`SetCVar`
/// (`OptionsFrame_Load:110` tries `getglobal("Get"..value.func)`): `Getuiscale`, `Getfarclip`,
/// `Getanisotropic`, `GetspellEffectLevel`, `GetweatherDensity` and their setters.
/// `GetFarclip`/`SetFarclip` do exist (`0x488f00`/`0x488f30`); `getglobal` is case-sensitive.
fn install_video_verbs(lua: &Lua) -> mlua::Result<()> {
    // ── GetScreenResolutions ─────────────────────────────────────────────────────────────────
    // A vararg of `"WxH"` strings, the spelling `ScreenResolution` pins.
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
    // The 1-based index into that list (`0x48bfa2`), and 1.0 on any miss, never nil or 0
    // (`0x48bf88`), so a 1 does not prove a match.
    //
    // Deviation: the index is of the live window size, not of the `gxResolution` CVar the
    // reference parses (`0x48bf20`), because without an exclusive mode-set `gxResolution` is the
    // windowed size, not the live one in borderless fullscreen, which `CT_Viewport` scales by.
    lua.globals().set(
        "GetCurrentResolution",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.current_resolution.map_or(1.0, |i| (i + 1) as f64))
        })?,
    )?;
    // ── SetScreenResolution ──────────────────────────────────────────────────────────────────
    // 1-based (`OptionsFrame.lua:239`), and `0x48bfd0` never raises: a missing or non-numeric
    // argument selects the first entry, a numeric string counts (`0x6f7c20`), and a number
    // truncates toward zero (`0x40a2b0`).
    //
    // Deviation: 0, a negative or an index past the count clamps to the nearer end, because the
    // reference reads one element past the last there (`0x48c00d`), uninitialised memory.
    //
    // A size equal to `gxResolution`'s does nothing, no write and no restart (`0x48c081`);
    // otherwise it issues its own `gxRestart` (`0x48c0b7`), so `OptionsFrame_Save`'s later
    // `RestartGx()` restarts a resolution change twice, here as in the reference.
    lua.globals().set(
        "SetScreenResolution",
        lua.create_function(|lua, id: Option<f64>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.screen_resolutions.is_empty() {
                return Ok(());
            }
            // nil selects entry 1; a non-numeric argument has already raised in the conversion,
            // where the reference takes entry 1. Truncate, then clamp both ends.
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
    // No values is the reference's "nothing to offer" (`0x48c136`), an empty but enabled
    // dropdown. Any argument is accepted; in the reference it picks the resolution, the first
    // when absent (`0x48c0ec`). Deviation: always empty, because a refresh rate is only
    // selectable through an exclusive mode-set and benilla has none.
    lua.globals().set(
        "GetRefreshRates",
        lua.create_function(|_, _: mlua::MultiValue| Ok(mlua::MultiValue::new()))?,
    )?;
    // ── GetVideoCaps ─────────────────────────────────────────────────────────────────────────
    // Seven values in `OptionsFrame.lua:59`'s order, never a boolean (`0x48db40`): slots 1-4 and
    // 7 are nil or the number 1, slot 5 is always a number, slot 6 is nil iff 0. Slot 5 is 0
    // under OpenGL and 1 under Direct3D; 0 is truthy, so `OptionsFrame_Load`'s two
    // `not hasTripleBuffering` tests never fire in the reference, and a nil would revive them.
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
            // Slot 5: always a number, a straight-line `fild` (`0x48dbcb`).
            out.push_back(Value::Number(if caps.triple_buffering { 1.0 } else { 0.0 }));
            // Slot 6: nil iff 0; the reference's is seeded to 1 (`caps+0xac`), so never nil there.
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
    // "Apply the staged video settings now": `0x48dab0` runs the console command `gxRestart`
    // (`0x63ce00`), whose handler `0x639f60` applies the staged device format and commits the
    // fourteen latched CVars. Here it queues a counted request, as `Screenshot()` does; the host
    // commits its gx stages (`Cvars::commit_latched`) and re-asserts them against the window.
    // The reference runs each gx change callback at `SetCVar`, where it can veto the write; the
    // host runs it at the commit. No restart applies `gxMultisample`: the camera reads it once.
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
    // The Defaults button's whole effect: `OptionsFrame_SetDefaults` only sets the widgets, which
    // the button's `OnClick` then hides without `_Save` (`OptionsFrame.xml:688-690`). Ends with a
    // restart, as the reference calls the `gxRestart` handler between its two passes (`0x639a51`).
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
                    // Unregistered in a bare VM: nothing to restore, nothing to warn about.
                    continue;
                };
                set_from_engine(&mut model, name, default);
            }
            model.restart_gx_asks += 1;
            Ok(())
        })?,
    )?;
    // ── GetGamma / SetGamma ──────────────────────────────────────────────────────────────────
    // `GetGamma 0x4891c0` and `SetGamma 0x4891f0` work in the slider's offset, not the CVar's
    // unit: `0x4891d0` is FSUBR over the f64 1.0 at `[0x8015b8]`, so `GetGamma()` answers
    // `1.0 - gamma` and `SetGamma(v)` writes `gamma = 1.0 - v`; the registered `"1.0"` reads as
    // 0.0, the slider's centre. The reference clamps nowhere (`SetGamma(5)` writes
    // `"-4.000000"`); the clamp is the consumer's (`ui_gamma::GAMMA_RANGE`).
    lua.globals().set(
        "GetGamma",
        // Any argument is ignored (the getter never calls `lua_gettop`); one number comes back.
        lua.create_function(|lua, _: mlua::MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let gamma = model
                .cvars
                .get(&CVAR_GAMMA.to_ascii_lowercase())
                .and_then(|slot| slot.value.parse::<f64>().ok())
                // A bare VM with no `gamma` row answers as if the default stood; the reference
                // always finds its record (`0x63de30`).
                .unwrap_or(1.0);
            Ok(1.0 - gamma)
        })?,
    )?;
    lua.globals().set(
        "SetGamma",
        lua.create_function(|lua, value: Value| {
            // `lua_isnumber` (`0x4891fe`): a number or a numeric string; anything else raises
            // (`0x6f4940`).
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
            // `SStrPrintf(buf, 0x10, "%f", 1.0 - v)`: a 16-byte buffer, so the stored string, and
            // what `GetCVar("gamma")` answers, is cut at 15 characters.
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

/// The reference's display-gamma CVar: registered at `0x402d70` as `"1.0"`, flags 0, name
/// string `0x82e924`.
pub const CVAR_GAMMA: &str = "gamma";

/// `0x8424cc`, verbatim.
const USAGE_SET_GAMMA: &str = "Usage: SetGamma(value)";

/// The `WorldDetail` slider's stop, 0, 1 or 2. Deviation: 1.12 has no such CVar; it exists because
/// our Graphics page needs somewhere to keep the stop, and takes the API's name.
pub const CVAR_WORLD_DETAIL: &str = "WorldDetail";

/// The reference's Environment Detail CVar: cells visited per chunk by the detail-doodad scatter.
pub const CVAR_FRILL_DENSITY: &str = "frillDensity";

/// `SetWorldDetail`'s `frillDensity` per stop, the three dwords at `0x804518`.
pub const WORLD_DETAIL_STOPS: [u32; 3] = [16, 32, 48];

/// The Environment Detail pair, `SetWorldDetail 0x488dd0` and `GetWorldDetail 0x488d70`, which
/// `OptionsFrame.lua` row 3 (`func = "WorldDetail"`) drives in place of `SetCVar`/`GetCVar`. The
/// setter truncates toward zero (`0x40a2b0`), raises outside 0..=2, and writes `frillDensity` from
/// [`WORLD_DETAIL_STOPS`] and `smallCull` from `{0.07, 0.04, 0.01}` (`0x804524`).
///
/// Deviation: the getter reads [`CVAR_WORLD_DETAIL`] and `smallCull` is not registered, because
/// the reference's getter is `smallCull`'s only reader (`[0x868620]` is never read). A fresh
/// reference client reads stop 1 (`frillDensity 16` with `smallCull 0.04`), hence
/// `WorldDetail`'s `"1"`; only a bare `frillDensity` write moves ours and not the reference's.
fn install_world_detail_verbs(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "SetWorldDetail",
        lua.create_function(|lua, value: Value| {
            // `lua_isnumber`: a number or a numeric string; anything else is the usage error.
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
            // Truncate toward zero, then the reference's bounds: -0.5 is stop 0 and only <= -1
            // raises. NaN raises too, as the reference's `fistp` gives the negative indefinite.
            if n.is_nan() {
                return Err(mlua::Error::runtime(RANGE_SET_WORLD_DETAIL));
            }
            let stop = n.trunc();
            if !(0.0..3.0).contains(&stop) {
                return Err(mlua::Error::runtime(RANGE_SET_WORLD_DETAIL));
            }
            let stop = stop as usize;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // Two CVars per stop, as `0x488dd0` writes two: `frillDensity` and, here, the stop.
            // Writing only one would leave `GetWorldDetail` stale until the host drains the queue.
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
        // `MultiValue`: an argument is accepted and ignored, as the getter never calls
        // `lua_gettop`.
        lua.create_function(|lua, _: mlua::MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let stop = model
                .cvars
                .get(&CVAR_WORLD_DETAIL.to_ascii_lowercase())
                .and_then(|slot| slot.value.parse::<f64>().ok())
                // The reference only answers 0, 1 or 2, so an off-grid stop (a console
                // `frillDensity 200` moves it) reports the nearest.
                .map_or(0, |v| v.round().clamp(0.0, 2.0) as i64);
            Ok(stop)
        })?,
    )?;
    Ok(())
}

/// `0x8423e8`, verbatim.
const USAGE_SET_WORLD_DETAIL: &str = "Usage: SetWorldDetail(value)";

/// `0x8423b8`, verbatim: the lowercase `v` and the odd comma are the reference's.
const RANGE_SET_WORLD_DETAIL: &str = "value must be in the range 0, 2";

/// The four nameplate verbs: `ShowNameplates 0x489450`, `HideNameplates 0x489460`,
/// `ShowFriendNameplates 0x489470`, `HideFriendNameplates 0x489480`. Each ignores its arguments
/// (`ShowNameplates(false)` still shows) and returns zero values, and there is no getter; the
/// state is bits `0x1` (enemy) and `0x8` (friend) of `[0xc4da34]`.
///
/// Deviation: the state lives in two CVars, so a player's plates survive a zone-in; the reference
/// registers no nameplate CVar (none of `0x63db90`'s sites), clears both bits on every
/// `EnterWorld`, and FrameXML replays its `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON` saved variables.
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
            // Any arguments are ignored, as the reference reads none; the empty `MultiValue`
            // returns zero values, which is not nil.
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
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("musicvolume")"#)
                .unwrap(),
            "0.4"
        );
        s.run(r#"SetCVar("MUSICVOLUME", 0.7)"#).unwrap();
        assert_eq!(
            s.take_cvar_changes(),
            vec![("MusicVolume".to_string(), "0.7".to_string())]
        );
        assert!(s.take_cvar_changes().is_empty(), "drained");
        assert_eq!(
            s.eval::<String>(r#"return GetCVarDefault("MusicVolume")"#)
                .unwrap(),
            "0.4"
        );
        assert_eq!(s.cvar("MusicVolume").as_deref(), Some("0.7"));
        s.run(r#"SetCVar("MusicVolume", "0.7")"#).unwrap();
        assert!(s.take_cvar_changes().is_empty());
    }

    /// The reference's `Set 0x63df50` on flag bit1: `GetCVar` (`rec+0x20`) answers the old value
    /// until `CVar::Update 0x63e060` commits, which here arrives as a host write.
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
        // and is reported, so the host drops its pending copy too.
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
        s.run(r#"SetCVar("MusicVolume", 0.7)"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("MusicVolume")"#)
                .unwrap(),
            "0.7"
        );
    }

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

    /// The reference prints `CVar "%s" is "%s"` for a bare name (`0x63dde0`); that printing is the
    /// host's, while a valued write lands here at once.
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

        s.run(r#"SetCVar("MusicVolume", "0.5")"#).unwrap();
        s.tick(0.0);
        assert_eq!(s.eval::<f64>("return getn(SEEN)").unwrap(), 0.0);

        // With one: arg1 is the token verbatim, not the CVar's name, and arg2 the new value.
        s.run(r#"SetCVar("MusicVolume", "0.6", "MUSIC_VOLUME")"#)
            .unwrap();
        s.tick(0.0);
        assert_eq!(
            s.eval::<String>("return SEEN[1]").unwrap(),
            "MUSIC_VOLUME=0.6"
        );

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

        let flat: Vec<f64> = s.eval(r#"return { GetMultisampleFormats() }"#).unwrap();
        assert_eq!(
            flat,
            vec![32.0, 32.0, 1.0, 32.0, 32.0, 2.0, 32.0, 32.0, 4.0],
            "three fields per entry, entries in offer order"
        );

        // 1-based: the default `gxMultisample "1"` is the first row.
        assert_eq!(
            s.eval::<f64>("return GetCurrentMultisampleFormat()")
                .unwrap(),
            1.0
        );

        // Pick 4x, the third row: all three CVars move together, as `0x48c640` writes them.
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

        let changed: Vec<String> = s
            .take_cvar_changes()
            .into_iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect();
        assert!(
            changed.contains(&"gxMultisample=4".to_string()),
            "expected gxMultisample on the change queue, got {changed:?}"
        );

        s.eval::<()>("SetMultisampleFormat(99)").unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxMultisample")"#)
                .unwrap(),
            "4",
            "an out-of-range id must leave the selection alone"
        );
    }

    /// `realmName` is a real 1.12 CVar (`0x83f2d0`) and must be a string: every Ace addon runs
    /// `ace.trim(GetCVar("realmName"))` at `PLAYER_ENTERING_WORLD` (`Ace/AceState.lua:28`). Set
    /// through `set_realm_name`, the seam `GetRealmName()` also reads.
    #[test]
    fn the_realm_name_cvar_and_get_realm_name_are_one_fact() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("realmName", "")]);

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
