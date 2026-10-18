//! The CVar table: the reference's store of case-insensitively named string settings with
//! registered defaults, behind `GetCVar`/`SetCVar`/`GetCVarDefault`/`RegisterCVar`.
//!
//! The host registers the vars it backs and drains Lua's writes from a change queue; its own
//! writes do not queue. The table dies with the VM, where the reference's outlives every
//! `ReloadUI`, so the host seeds each new VM with the saved values before anything registers.
//! `GetCVar`, `SetCVar` and `GetCVarDefault` raise `Couldn't find CVar named '%s'` on an unknown
//! name (`0x842310`, at `0x488be7`, `0x488c53` and `0x488d37`); the host-side writes and the
//! other by-name bindings warn once and ignore it.

use mlua::{Lua, Value};

use super::binding_abi::lua_number_text;
use super::cvar_checks::CvarCheck;
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
    /// The change callback that may refuse a write ([`super::cvar_checks`]); host-seeded only.
    pub check: Option<CvarCheck>,
    /// The record's reset text (`rec+0x34`), which `cvar_reset` restores (`0x63dff0`): the saved
    /// value the row started at, else the first value its own console command stored, else none.
    pub reset: Option<String>,
    /// The session, not the player, owns the row (an env lever): a reset of every row passes it.
    pub session: bool,
    /// The row's place in the mirror's list, its insertion order, which a walk of every row
    /// follows.
    pub seq: usize,
}

/// One row of the host's registry as the VM's mirror is seeded from it; `value` is the applied
/// value.
#[derive(Clone, Debug)]
pub struct SeededCvar {
    pub name: String,
    pub value: String,
    pub default: String,
    pub latched: bool,
    pub check: Option<CvarCheck>,
    /// The reset text ([`CvarSlot::reset`]).
    pub reset: Option<String>,
    /// Owned by the session ([`CvarSlot::session`]).
    pub session: bool,
}

/// One write the mirror made, queued for the host's registry in the order it was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CvarWrite {
    /// `CVar::Set` (`0x63df50`) past its callback: a latched row's stage, else its new value.
    Set { name: String, value: String },
    /// The record's empty reset slot filled (`0x63e158`) by the CVar's own console command
    /// (`0x63de23` passes `setReset`), whether or not the value moved.
    FillReset { name: String, value: String },
    /// `CVar::Reset` (`0x63dff0`) or `CVar::Default` (`0x63e010`): `InternalSet` called directly,
    /// so no callback ran and a latched row took the value at once, its stage kept.
    Restore { name: String, value: String },
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
    /// file has one. A row registered here has no change callback; [`Self::seed_cvars`] carries
    /// the host registry's. `&self` so the interface loader can register from a `&UiScript`: the stock
    /// `UIOptionsFrame.xml` reads CVars in its own `OnLoad`.
    pub fn register_cvars<'a>(&self, vars: impl IntoIterator<Item = (&'a str, &'a str)>) {
        let mut model = self.model_mut();
        for (name, default) in vars {
            let key = name.to_ascii_lowercase();
            match model.cvars.get_mut(&key) {
                Some(slot) => slot.default = default.to_string(),
                None => {
                    // A saved value is also the reset text, as the config line's `set` made the
                    // record with it (`0x63d582`).
                    let saved = model.cvars_saved_base.get(&key).cloned();
                    let seq = model.cvars.len();
                    model.cvars.insert(
                        key,
                        CvarSlot {
                            name: name.to_string(),
                            value: saved.clone().unwrap_or_else(|| default.to_string()),
                            default: default.to_string(),
                            latched: false,
                            pending: None,
                            check: None,
                            reset: saved,
                            session: false,
                            seq,
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
            let seq = model.cvars.get(&key).map_or(model.cvars.len(), |s| s.seq);
            model.cvars.insert(
                key,
                CvarSlot {
                    name: row.name,
                    value: row.value,
                    default: row.default,
                    latched: row.latched,
                    pending: None,
                    check: row.check,
                    reset: row.reset,
                    session: row.session,
                    seq,
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

    /// One CVar's integer, the record's `+0x28`: Storm's `SStrToInt` of its text, as the set
    /// stores it (`0x63e127`). `None` with no such record.
    pub fn cvar_int(&self, name: &str) -> Option<i32> {
        self.model_mut()
            .cvars
            .get(&name.to_ascii_lowercase())
            .map(|s| super::binding_abi::s_str_to_int(&s.value))
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

    /// Drain the lines CVar change callbacks printed since the last call, for the console.
    pub fn take_cvar_console_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().cvar_console)
    }

    /// Whether a Lua write, registration or callback line waits for [`Self::take_cvar_changes`],
    /// [`Self::take_cvar_registrations`] or [`Self::take_cvar_console_lines`].
    pub fn has_cvar_traffic(&self) -> bool {
        let model = self.model_mut();
        !model.cvar_changes.is_empty()
            || !model.cvar_registrations.is_empty()
            || !model.cvar_console.is_empty()
    }

    /// Drain the writes queued since the last call, in order, under the registered spelling: the
    /// host's cue to sync its resources and mark the config dirty.
    pub fn take_cvar_writes(&mut self) -> Vec<CvarWrite> {
        std::mem::take(&mut self.model_mut().cvar_changes)
    }

    /// [`Self::take_cvar_writes`] as `(name, value)` per value written, a stage or a restore; a
    /// reset slot's fill is dropped.
    pub fn take_cvar_changes(&mut self) -> Vec<(String, String)> {
        self.take_cvar_writes()
            .into_iter()
            .filter_map(|w| match w {
                CvarWrite::Set { name, value } | CvarWrite::Restore { name, value } => {
                    Some((name, value))
                }
                CvarWrite::FillReset { .. } => None,
            })
            .collect()
    }

    /// Drain the `(name, default)` rows an addon's `RegisterCVar` created since the last call; the
    /// host's registry keeps them beside its own, as the reference's one table does.
    pub fn take_cvar_registrations(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.model_mut().cvar_registrations)
    }

    /// The process's count of CVars Lua has created ([`LUA_CVAR_LIMIT`]), which outlives the VM
    /// as the reference's `[0xb4e3c8]` outlives every `ReloadUI`: the host seeds each new VM with
    /// it and reads it back.
    pub fn set_cvar_lua_created(&mut self, count: u32) {
        self.model_mut().cvar_lua_created = count;
    }

    /// See [`Self::set_cvar_lua_created`].
    pub fn cvar_lua_created(&self) -> u32 {
        self.model_mut().cvar_lua_created
    }

    /// Flag a CVar read-only, or clear it: `CVar::SetReadOnly` (`0x63e030`). A flagged name makes
    /// `SetCVar` raise `"%s" is read-only` (`0x488c67`-`0x488c78`), and every other write to it
    /// (`ConsoleExec`, an engine write) stores nothing, as `CVar::Set` (`0x63df50`) stores through
    /// `InternalSet`, which returns at the flag (`0x63e0ba`). The host's mirror sync
    /// ([`Self::set_cvar_host`]) is not a write and passes. The name need not be registered yet.
    pub fn set_cvar_read_only(&mut self, name: &str, read_only: bool) {
        let key = name.to_ascii_lowercase();
        let mut model = self.model_mut();
        if read_only {
            model.cvars_read_only.insert(key);
        } else {
            model.cvars_read_only.remove(&key);
        }
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
/// and the change was queued. The row's check runs first and may refuse the write, as the
/// reference's `Set 0x63df50` calls the callback before anything else (`0x63df7d`). A latched
/// slot stages the write, as `Set` stores `latchedValue` and skips `InternalSet`; writing back the
/// applied value unstages it. Otherwise it is `InternalSet` (`0x63e0b0`): nothing for a read-only
/// row (`0x63e0ba`-`0x63e0c2`), nothing for text equal to the value case-blind (`SStrCmpI`,
/// `0x63e0ec`), and any other text stored as it is, a number or not.
fn store(model: &mut Model, key: &str, value: String) -> Option<String> {
    store_with(model, key, value, false)
}

/// [`store`], and with `fill_reset` the CVar's own console command (`0x63dde0`), which passes
/// `setReset` (`0x63de1c`): past the callback, an unlatched row that is not read-only takes the
/// text as its reset text if it has none (`0x63e13f`-`0x63e158`), moved or not. A latched row
/// returns at its stage first (`0x63dfc1`).
fn store_with(model: &mut Model, key: &str, value: String, fill_reset: bool) -> Option<String> {
    let verdict = {
        let cvars = &model.cvars;
        let slot = cvars.get(key)?;
        slot.check.map(|check| {
            check(&value, &slot.value, &|name| {
                cvars
                    .get(&name.to_ascii_lowercase())
                    .map(|s| s.value.clone())
            })
        })
    };
    if let Some(verdict) = verdict {
        model.cvar_console.extend(verdict.lines);
        if !verdict.accept {
            return None;
        }
    }
    let slot = model.cvars.get_mut(key)?;
    if slot.latched {
        let staged = (!value.eq_ignore_ascii_case(&slot.value)).then_some(value.clone());
        if slot.pending == staged && staged.is_some() {
            return None;
        }
        if staged.is_none() && slot.pending.is_none() {
            return None;
        }
        slot.pending = staged;
    } else {
        if model.cvars_read_only.contains(key) {
            return None;
        }
        let fill = (fill_reset && slot.reset.is_none()).then(|| {
            slot.reset = Some(value.clone());
            CvarWrite::FillReset {
                name: slot.name.clone(),
                value: value.clone(),
            }
        });
        if slot.value.eq_ignore_ascii_case(&value) {
            model.cvar_changes.extend(fill);
            return None;
        }
        slot.value = value.clone();
        let registered = slot.name.clone();
        model.cvar_changes.push(CvarWrite::Set {
            name: registered.clone(),
            value,
        });
        model.cvar_changes.extend(fill);
        return Some(registered);
    }
    let registered = slot.name.clone();
    model.cvar_changes.push(CvarWrite::Set {
        name: registered.clone(),
        value,
    });
    Some(registered)
}

/// The CVar's own console command with a value (`0x63dde0`): `CVar::Set(rest, 1, 1, 0, 1)`
/// (`0x63de18`-`0x63de23`), a write that also fills an empty reset slot.
pub(super) fn console_set(model: &mut Model, key: &str, value: String) {
    store_with(model, key, value, true);
}

/// `CVar::Reset` (`0x63dff0`, the reset text, else the default) or, with `to_default`,
/// `CVar::Default` (`0x63e010`, the default, else the reset text), each `InternalSet(text, 1, 0,
/// 0, 1)` called directly: no callback runs and none can refuse, a latched row takes the value at
/// once and keeps its stage, and a read-only row (`0x63e0ba`) or text equal to the value
/// case-blind (`0x63e0ec`) stores nothing. A row always has a default here, so `to_default` never
/// falls back.
pub(super) fn restore_cvar(model: &mut Model, key: &str, to_default: bool) {
    if model.cvars_read_only.contains(key) {
        return;
    }
    let Some(slot) = model.cvars.get_mut(key) else {
        return;
    };
    let text = match (&slot.reset, to_default) {
        (Some(reset), false) => reset.clone(),
        _ => slot.default.clone(),
    };
    if slot.value.eq_ignore_ascii_case(&text) {
        return;
    }
    slot.value = text.clone();
    model.cvar_changes.push(CvarWrite::Restore {
        name: slot.name.clone(),
        value: text,
    });
}

/// A by-name write from a binding that raises nothing (`ConsoleExec`, `SetGamma`,
/// `SetWorldDetail`, the video pairs): the change is queued only when the value moved, and an
/// unknown name warns once.
pub(super) fn write_cvar(model: &mut Model, name: &str, value: String) {
    let key = name.to_ascii_lowercase();
    if !model.cvars.contains_key(&key) {
        warn_unknown(model, name);
        return;
    }
    store(model, &key, value);
}

/// `Couldn't find CVar named '%s'` (`0x842310`), the error `GetCVar`, `SetCVar` and
/// `GetCVarDefault` raise with the name as `lua_tostring` gave it.
fn unknown_cvar(name: &str) -> mlua::Error {
    mlua::Error::runtime(format!("Couldn't find CVar named '{name}'"))
}

fn warn_unknown(model: &mut Model, name: &str) {
    let key = name.to_ascii_lowercase();
    if model.cvars_warned.insert(key) {
        model.record_warning(format!(
            "unknown CVar '{name}' (not host-registered) — ignored"
        ));
    }
}

/// An argument as the CVar bindings read it, `lua_tostring` (`0x6f3690`): a string as it is, a
/// number as Lua's `%.14g` text (`luaV_tostring` `0x6f7c80`), so the stock Sound slider's
/// single-precision 0.4 stores `"0.40000000596046"`. `None` for anything `lua_isstring`
/// (`0x6f3510`) refuses: nil, an absent argument, a boolean, a table or a function.
fn cvar_arg(v: Option<&Value>) -> Option<String> {
    match v? {
        // Lossy: the reference takes bytes, and invalid UTF-8 must not read as absent.
        Value::String(s) => Some(s.to_string_lossy()),
        Value::Integer(i) => Some(lua_number_text(*i as f64)),
        Value::Number(n) => Some(lua_number_text(*n)),
        _ => None,
    }
}

/// A value as the video setters print it, `SStrPrintf(buf, 0x10, "%f", v)` (`0x64a7f0` with the
/// format at `0x835160`): six decimals in a 16-byte buffer, so the stored string is cut at 15
/// characters.
pub(super) fn format_f(v: f64) -> String {
    let mut written = format!("{v:.6}");
    written.truncate(15);
    written
}

/// What `SetCVar` and `RegisterCVar` store for a value with no string form (`0x82e570`, loaded
/// at `0x488c98` and `0x488b6f`).
const NO_STRING_FORM: &str = "0";

/// `0x842330`, verbatim.
const USAGE_GET_CVAR: &str = "Usage: GetCVar(\"cvar\")";

/// `0x84238c`, verbatim: the reference's `GetCVarDefault` names itself `SetCVarDefault`.
const USAGE_GET_CVAR_DEFAULT: &str = "Usage: SetCVarDefault(\"cvar\")";

/// `0x84235c`, verbatim, the unbalanced parenthesis included.
const USAGE_SET_CVAR: &str = "Usage: SetCVar(\"cvar\", value [, \"scriptCvar\")";

/// `0x8422e8`, verbatim.
const USAGE_REGISTER_CVAR: &str = "Usage: RegisterCVar(\"cvar\" [, default])";

/// How many CVars `RegisterCVar` creates per process (`0x488b25`: `cmp [0xb4e3c8], 0x1e`).
pub const LUA_CVAR_LIMIT: u32 = 30;

/// `0x8422c0`, verbatim.
const TOO_MANY_CVARS: &str = "Attempted to register too many CVars";

/// The CVars `CGGameUI::InitializeGame` flags read-only for the whole world session, the table
/// at `0x83de4c` walked by `0x48f566`-`0x48f584`; `ShutdownGame` clears them again
/// (`0x491240`-`0x49125d`). The host flags them on each in-game VM before its UI loads, as
/// `InitializeGame` does before `UI_Init`, and on its own registry for the session.
pub const IN_WORLD_READ_ONLY_CVARS: [&str; 3] = ["realmList", "realmName", "scriptMemory"];

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // `RegisterCVar(name, value)` (`0x488b00`) declares a CVar the client does not ship, which is
    // how an addon persists a setting. Re-declaring a live name is a no-op: the reference never
    // resets a live value on re-registration (`0x488b7d`).
    lua.globals().set(
        "RegisterCVar",
        lua.create_function(|lua, args: mlua::MultiValue| {
            // `lua_isstring(1)` (`0x488b09`): a string or a number names it.
            let Some(name) = cvar_arg(args.front()) else {
                return Err(mlua::Error::runtime(USAGE_REGISTER_CVAR));
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // The limit is tested before the name is looked up (`0x488b25`), so at the limit a
            // re-declaration raises too.
            if model.cvar_lua_created >= LUA_CVAR_LIMIT {
                return Err(mlua::Error::runtime(TOO_MANY_CVARS));
            }
            // The default only when `lua_isstring(2)` passes (`0x488b56`), else "0".
            let value = cvar_arg(args.get(1)).unwrap_or_else(|| NO_STRING_FORM.to_string());
            let key = name.to_ascii_lowercase();
            // A saved value outranks the declared one, which stays the default for the saver.
            let saved = model.cvars_saved_base.get(&key).cloned();
            if model.cvars.contains_key(&key) {
                return Ok(());
            }
            // Only a call that creates the record counts (`0x488b8f`, past the lookup at
            // `0x488b7d`). A saved name is no creation: the reference's config load already made
            // its record (`set`, `0x63d582`), so its lookup finds one.
            if saved.is_none() {
                model.cvar_lua_created += 1;
            }
            // The host gives the row a place in its registry, which outlives this VM.
            model.cvar_registrations.push((name.clone(), value.clone()));
            let seq = model.cvars.len();
            model.cvars.insert(
                key,
                CvarSlot {
                    name,
                    value: saved.clone().unwrap_or_else(|| value.clone()),
                    default: value,
                    latched: false,
                    pending: None,
                    check: None,
                    // The saved value is the reset text of the record its config line made.
                    reset: saved,
                    session: false,
                    seq,
                },
            );
            Ok(())
        })?,
    )?;

    // `GetCVar(name)` (`0x488ba0`).
    lua.globals().set(
        "GetCVar",
        lua.create_function(|lua, args: mlua::MultiValue| {
            // `lua_isstring(1)` (`0x488ba8`): a string or a number names it.
            let Some(name) = cvar_arg(args.front()) else {
                return Err(mlua::Error::runtime(USAGE_GET_CVAR));
            };
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            match model.cvars.get(&name.to_ascii_lowercase()) {
                Some(slot) => Ok(Value::String(lua.create_string(&slot.value)?)),
                None => Err(unknown_cvar(&name)),
            }
        })?,
    )?;
    // `GetCVarDefault(name)` (`0x488cf0`).
    lua.globals().set(
        "GetCVarDefault",
        lua.create_function(|lua, args: mlua::MultiValue| {
            // `lua_isstring(1)` (`0x488cf8`): a string or a number names it.
            let Some(name) = cvar_arg(args.front()) else {
                return Err(mlua::Error::runtime(USAGE_GET_CVAR_DEFAULT));
            };
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            match model.cvars.get(&name.to_ascii_lowercase()) {
                Some(slot) => Ok(Value::String(lua.create_string(&slot.default)?)),
                None => Err(unknown_cvar(&name)),
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

    // `SetCVar(name, value [, token])` (`0x488c10`).
    lua.globals().set(
        "SetCVar",
        lua.create_function(|lua, args: mlua::MultiValue| {
            // `lua_isstring(1)` (`0x488c1a`): a string or a number names it.
            let Some(name) = cvar_arg(args.front()) else {
                return Err(mlua::Error::runtime(USAGE_SET_CVAR));
            };
            // A value with no string form stores "0" (`0x488c8d`-`0x488c98`): an unchecked
            // CheckButton's `GetChecked()` is nil, and `SoundOptionsCheckButton_OnClick` writes
            // it as is.
            let value = cvar_arg(args.get(1)).unwrap_or_else(|| NO_STRING_FORM.to_string());
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                // The lookup comes first (`0x488c47`-`0x488c59`): an unknown name raises before
                // the read-only flag is read.
                let key = name.to_ascii_lowercase();
                if !model.cvars.contains_key(&key) {
                    return Err(unknown_cvar(&name));
                }
                // Flag bit2 (`0x488c67`), tested before anything is written.
                if model.cvars_read_only.contains(&key) {
                    return Err(mlua::Error::runtime(format!("\"{name}\" is read-only")));
                }
                store(&mut model, &key, value.clone());
            }
            // A third argument that passes `lua_isstring` is the `CVAR_UPDATE` token, arg1, with
            // the value as arg2 (`0x488cad`-`0x488cd5`), fired on every such call, changed or not,
            // and inside it: `SignalEvent2` (`0x703f50`) walks the listeners in place. The 1.12
            // options panels pass their CheckButtons key, such as "STATUS_BAR_TEXT"
            // (`UIOptionsFrame.lua` l.335), which is how `UIOptionsFrame_OnEvent` finds its row.
            if let Some(token) = cvar_arg(args.get(2)) {
                super::tick::fire_event_into(
                    lua,
                    "CVAR_UPDATE",
                    vec![ScriptValue::Str(token), ScriptValue::Str(value)],
                );
            }
            Ok(())
        })?,
    )?;

    install_world_detail_verbs(lua)?;
    super::video_pairs::install(lua)
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
    // Of the sixteen further graphics CVars (`0x639a60`), the seven benilla registers, in the
    // pass's order.
    "farclip",
    "shadowLevel",
    "frillDensity",
    "doodadAnim",
    "trilinear",
    CVAR_SMALL_CULL,
    "baseMip",
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
/// `GetFarclip`/`SetFarclip` (`0x488f00`/`0x488f30`, in [`super::video_pairs`]) do exist;
/// `getglobal` is case-sensitive.
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
        lua.create_function(|lua, id: Option<Value>| {
            // `is-number` (`0x48bfe8`) false leaves the index at entry 1 (`0x48bfe6`).
            let idx = match id {
                Some(v) => lua.coerce_number(v)?.map_or(0, super::binding_abi::ftol),
                None => 0,
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.screen_resolutions.is_empty() {
                return Ok(());
            }
            let last = model.screen_resolutions.len() - 1;
            let r = model.screen_resolutions[if idx < 1 {
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
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            write_cvar(&mut model, CVAR_GAMMA, format_f(1.0 - v));
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

/// The reference's object-size cull (`0x68854a`, name `0x8696f8`), the other half of each
/// Environment Detail stop. `SetWorldDetail` looks it up as `smallCull` (`0x8423ac`), the same
/// record under the table's case-folding hash.
pub const CVAR_SMALL_CULL: &str = "SmallCull";

/// `SetWorldDetail`'s `frillDensity` per stop, the three dwords at `0x804518`.
pub const WORLD_DETAIL_STOPS: [u32; 3] = [16, 32, 48];

/// `SetWorldDetail`'s `smallCull` per stop, the three f32s at `0x804524`.
const SMALL_CULL_STOPS: [f32; 3] = [0.07, 0.04, 0.01];

/// The `SmallCull` text `SetWorldDetail` writes for a stop (clamped to 2): the f32 as "%f".
pub fn small_cull_text(stop: usize) -> String {
    format_f(f64::from(SMALL_CULL_STOPS[stop.min(2)]))
}

/// The Environment Detail pair, `SetWorldDetail 0x488dd0` and `GetWorldDetail 0x488d70`, which
/// `OptionsFrame.lua` row 3 (`func = "WorldDetail"`) drives in place of `SetCVar`/`GetCVar`. The
/// setter truncates toward zero (`0x40a2b0`), raises outside 0..=2, and writes `frillDensity` from
/// [`WORLD_DETAIL_STOPS`] and `smallCull` from `{0.07, 0.04, 0.01}` (`0x804524`).
///
/// Deviation: the getter reads [`CVAR_WORLD_DETAIL`], where the reference's reads only `smallCull`
/// (`0x488d77`), because benilla's stop follows the knob that draws: a console `frillDensity`
/// write moves the ground clutter and `WorldDetail` with it, and no renderer reads `smallCull`
/// (its callback's `[0x868620]` has no reader). A fresh reference client reads stop 1
/// (`smallCull 0.04`), hence `WorldDetail`'s `"1"`.
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
            // The reference's two writes, `frillDensity` as "%d" (`0x488e26`) and `smallCull` as
            // "%f" of the f32 (`0x488e63`), then ours, the stop: writing it here keeps
            // `GetWorldDetail` current before the host drains the queue.
            write_cvar(
                &mut model,
                CVAR_FRILL_DENSITY,
                WORLD_DETAIL_STOPS[stop].to_string(),
            );
            write_cvar(&mut model, CVAR_SMALL_CULL, small_cull_text(stop));
            write_cvar(&mut model, CVAR_WORLD_DETAIL, stop.to_string());
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
                check: None,
                reset: None,
                session: false,
            },
            SeededCvar {
                name: "MusicVolume".into(),
                value: "0.4".into(),
                default: "0.4".into(),
                latched: false,
                check: None,
                reset: None,
                session: false,
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

    /// `SetScreenResolution` tests its argument with `is-number` (`0x48bfe8`): a false answer
    /// leaves the index at entry 1 (`0x48bfe6`) and never raises; a numeric string counts.
    #[test]
    fn set_screen_resolution_takes_entry_one_for_a_non_number() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("gxResolution", "1024x768")]);
        s.set_screen_resolutions(
            vec![
                super::ScreenResolution {
                    width: 800,
                    height: 600,
                },
                super::ScreenResolution {
                    width: 1024,
                    height: 768,
                },
                super::ScreenResolution {
                    width: 1280,
                    height: 960,
                },
            ],
            None,
        );
        let pick = |s: &UiScript, call: &str| {
            s.eval::<String>(&format!("{call} return GetCVar('gxResolution')"))
                .unwrap()
        };
        assert_eq!(pick(&s, "SetScreenResolution('3')"), "1280x960");
        assert_eq!(pick(&s, "SetScreenResolution('abc')"), "800x600");
        assert_eq!(pick(&s, "SetScreenResolution(' 2 ')"), "1024x768");
        assert_eq!(pick(&s, "SetScreenResolution({})"), "800x600");
        assert_eq!(pick(&s, "SetScreenResolution(3)"), "1280x960");
        assert_eq!(pick(&s, "SetScreenResolution(true)"), "800x600");
        assert!(s.take_errors().is_empty());
    }

    /// `0x488bd1`-`0x488bed`, `0x488c47`-`0x488c59`, `0x488d21`-`0x488d3d`: the lookup `0x63dec0`
    /// returns 0 and the verb raises `0x842310` with the name; nothing is returned or written.
    #[test]
    fn an_unknown_name_raises_in_the_three_verbs() {
        let mut s = script_with_volume();
        for call in [
            r#"GetCVar("bogusKnob")"#,
            r#"SetCVar("bogusKnob", 1)"#,
            r#"GetCVarDefault("bogusKnob")"#,
        ] {
            let observed: (bool, String) = s
                .eval(&format!(
                    "local ok, err = pcall(function() return {call} end) \
                     return ok, tostring(err)"
                ))
                .unwrap();
            assert!(!observed.0, "{call} must raise");
            assert!(
                observed.1.contains("Couldn't find CVar named 'bogusKnob'"),
                "{call}: {}",
                observed.1
            );
        }
        assert!(s.take_cvar_changes().is_empty(), "nothing is written");
        // The name is `lua_tostring`'s, so a number names one too (`0x488bca`).
        let err = s.run("GetCVar(7)").expect_err("7 names no CVar");
        assert!(
            err.to_string().contains("Couldn't find CVar named '7'"),
            "{err}"
        );
        // The lookup precedes the read-only test (`0x488c47` before `0x488c67`).
        s.set_cvar_read_only("bogusKnob", true);
        let err = s
            .run(r#"SetCVar("bogusKnob", 1)"#)
            .expect_err("still unknown");
        assert!(err.to_string().contains("Couldn't find CVar"), "{err}");
    }

    #[test]
    fn host_side_unknown_names_warn_once_and_no_op() {
        let mut s = script_with_volume();
        s.set_cvar_host("bogusKnob", "1");
        s.set_cvar_host("bogusKnob", "2");
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
