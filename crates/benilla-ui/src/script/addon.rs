//! The in-game AddOn API, the twelve `engine` addon globals of the 1.12 `_G`
//! (`reference/lua_globals.tsv`), over a registry the host fills at discovery. A verb that names
//! an addon takes a 1-based index or a name. `reason` is a token the stock UI splices into
//! `getglobal("ADDON_"..reason)` (`UIParent.lua:642`), so its spelling is the contract. Verdicts
//! come from [`super::addon_gate`], the reference's `AddOn_CanLoad` (`0x51e780`).

use std::path::PathBuf;

use mlua::{Lua, MultiValue, Value};

use super::addon_gate::{can_load, GateRow, Verdict};
use super::binding_abi::flag;
use super::Model;
use crate::status::Status;

/// Reads a chain-sourced addon's file by its path inside the patch chain.
pub type AddonChainReader = Box<dyn Fn(&str) -> Option<Vec<u8>>>;

/// [`AddonChainReader`], shared so a load can call it without holding the model.
pub(crate) type SharedChainReader = std::rc::Rc<dyn Fn(&str) -> Option<Vec<u8>>>;

/// The security state `[rec+0x24]`, indexing the table at `0x85367c`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddonSecurity {
    /// 0: a signature verified (`0x51dbfb`).
    Secure,
    /// 1: the constructor's, and every record's start (`0x5205b6`, `0x51db77`).
    #[default]
    Insecure,
    /// 2: the server's status 0 (`0x51dc0a`).
    Banned,
}

impl AddonSecurity {
    /// `GetAddOnInfo`'s security return (`0x51e980`, `0x85367c`).
    pub fn token(self) -> &'static str {
        match self {
            AddonSecurity::Secure => "SECURE",
            AddonSecurity::Insecure => "INSECURE",
            AddonSecurity::Banned => "BANNED",
        }
    }
}

/// One record of the server's `SMSG_ADDON_INFO` (`0x51da70`), named for the addon it answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddonReply {
    pub name: String,
    /// 0 bans, 2 hides, any other leaves the addon listed.
    pub status: u8,
    /// The server's revision when the record carried an info block.
    pub revision: Option<u32>,
    /// The record's url string.
    pub url: Option<String>,
}

/// One addon as the AddOn API sees it, filled by the host at discovery.
#[derive(Clone, Debug, Default)]
pub struct AddOnInfo {
    /// The folder name: `GetAddOnInfo`'s first return and `ADDON_LOADED`'s `arg1`.
    pub name: String,
    /// `## Title`; `None` when absent, which the reference returns as nil.
    pub title: Option<String>,
    pub notes: Option<String>,
    pub url: Option<String>,
    /// `## Secure:` (`[rec+0x28]`): the addon is in the `CMSG_AUTH_SESSION` block and the server's
    /// reply speaks of it. It grants nothing by itself.
    pub claims_secure: bool,
    /// What the signature check `0x6f10f0` answers for this addon, modelled: true is its answer 3
    /// (`0x51dbfb` makes the addon SECURE), false any other (no `.sig` read, a size mismatch, a
    /// digest mismatch: `[rec+0x24]` stays 1 and a mismatch sets `[rec+0x2a]`, which the host
    /// does not model). benilla verifies no signature, having no Blizzard key: the host answers
    /// true for an addon read from the player's own patch chain that claims `## Secure`.
    pub signature_ok: bool,
    /// `## Revision:` (`[rec+0x20]`), which the server's revision is compared against.
    pub installed_revision: u32,
    /// The security state `[rec+0x24]`: INSECURE from the constructor (`0x5205b6`), and what
    /// `SMSG_ADDON_INFO` makes of it (`0x51db77`-`0x51dc0a`). Drives the `security` return, the
    /// BANNED refusal and the context the addon loads under ([`addon_load_context`]).
    pub security: AddonSecurity,
    /// `[rec+0x2a]`, set when the signature check answers 2 (`0x51dc04`): refused as CORRUPT.
    pub corrupt: bool,
    /// `[rec+0xc0]`: the server's revision is past the installed one (`0x51db0d`).
    pub new_version: bool,
    pub load_on_demand: bool,
    /// `## Dependencies` / `## RequiredDeps`.
    pub dependencies: Vec<String>,
    /// `## OptionalDeps`: loaded first by `AddOn_Load`, a failure ignored (`0x51f320`).
    pub optional_dependencies: Vec<String>,
    /// `## LoadWith`: the addons whose load loads this one (`[rec+0x58]`, `0x51cf0a`-`0x51cf8c`).
    pub load_with: Vec<String>,
    /// The registry rows whose `## LoadWith` names this addon, in registry order (`[rec+0x88]`):
    /// filled by [`super::UiScript::register_addons`], as the reference builds it once the scan
    /// ends (`0x51c7c0`-`0x51c8c9`). `AddOn_Load` loads each after this one's `ADDON_LOADED`.
    pub dependents: Vec<usize>,
    /// The lines that reach the reference's metadata map in manifest order ([`crate::toc::Toc::metadata`]):
    /// all `GetAddOnMetadata` answers from.
    pub directives: Vec<(String, String)>,
    /// The `.toc`'s file list in order: what `LoadAddOn` runs.
    pub files: Vec<String>,
    /// `## SavedVariables`: globals restored from and written to the account-wide file.
    pub saved_variables: Vec<String>,
    /// `## SavedVariablesPerCharacter`, loaded after the account file so its values win.
    pub saved_variables_per_character: Vec<String>,
    /// `## DefaultState:` (`[rec+0x2b]`, 1 unless the manifest says `disabled`): what the enable
    /// query `0x51e470` falls back to for a character with no row. The glue's AddOns screen asks
    /// it per character; the in-game verbs ask it once, for the logged-in one, at registration.
    pub default_state: bool,
    /// Enable state from `AddOns.txt`; an addon never disabled is enabled.
    pub enabled: bool,
    /// The last-saved `enabled`, which `ResetDisabledAddOns` reverts to; set by `register_addons`.
    pub saved_enabled: bool,
    pub loaded: bool,
    /// `## Interface`'s leading integer, 0 when absent: what the version gate compares.
    pub interface: u32,
    /// Left out of the Lua index: `[rec+0x29]`, set by `0x51da70` for a `SMSG_ADDON_INFO` status of
    /// 2 (`0x51db84`) and otherwise only zeroed (`0x52059f`), so an addon the reply skips stays
    /// visible. A retail server answers 2 for all twelve `Blizzard_*` addons.
    pub hidden: bool,
    /// The files live in the patch chain, not the AddOns folder: Blizzard's twelve LoadOnDemand
    /// addons, read at `Interface/AddOns/<Name>/<file>` and loaded as a player's addon is.
    pub chain: bool,
}

/// Each verb's `Usage:` string as the reference's `.data` holds it; an error handler prints it.
const USAGE_INFO: &str = "Usage: GetAddOnInfo(index or \"name\")"; // 0x842d68
const USAGE_METADATA: &str = "Usage: GetAddOnMetadata(index or \"name\", \"variable\")"; // 0x842d90
const USAGE_DEPENDENCIES: &str = "Usage: GetAddOnDependencies(index or \"name\")"; // 0x842dc8
const USAGE_ENABLE: &str = "Usage: EnableAddOn(index or \"name\")"; // 0x842df8
const USAGE_DISABLE: &str = "Usage: DisableAddOn(index or \"name\")"; // 0x842e1c
const USAGE_LOAD_ON_DEMAND: &str = "Usage: IsAddOnLoadOnDemand(index or \"name\")"; // 0x842e44
const USAGE_LOADED: &str = "Usage: IsAddOnLoaded(index or \"name\")"; // 0x842e70
const USAGE_LOAD: &str = "Usage: LoadAddOn(index or \"name\")"; // 0x842e98

/// The `index or "name"` argument, read by the prologue all eight in-game verbs share
/// (`GetAddOnInfo 0x48e390`, `GetAddOnMetadata 0x48e530`, `GetAddOnDependencies 0x48e5e0`,
/// `EnableAddOn 0x48e690`, `DisableAddOn 0x48e760`, `IsAddOnLoadOnDemand 0x48e840`,
/// `IsAddOnLoaded 0x48e8e0`, `LoadAddOn 0x48e980`). A numeric string is an index. An index out of
/// range, or a non-string, raises through `luaL_error` (`0x6f4940`), which does not return; a
/// name passes unchecked, and each verb answers a miss its own way.
enum AddonKey {
    /// A bounds-checked 0-based position in the registry.
    Index(usize),
    Name(String),
}

/// A Lua index as the verbs resolve it: `_ftol` (`0x40a2b0`) truncates; minus one, compared
/// unsigned, so 0 raises like count + 1. An index into the sorted, filtered name array, not the
/// registry (`0x51df00` reads `[[0xbe1b94] + 4*idx]`, bound `[0xbe1b90]`); the message is
/// `0x837d70`'s.
pub(super) fn index_row(model: &Model, n: f64) -> mlua::Result<usize> {
    let index0 = (super::binding_abi::ftol(n)).wrapping_sub(1) as u32 as usize;
    match model.addon_index.get(index0) {
        Some(&row) => Ok(row),
        None => Err(mlua::Error::RuntimeError(format!(
            "AddOn index must be in the range of 1 to {}",
            model.addon_index.len()
        ))),
    }
}

/// The prologue; `usage` is the verb's own `.data` string.
fn addon_key(lua: &Lua, model: &Model, key: &Value, usage: &'static str) -> mlua::Result<AddonKey> {
    // `coerce_number` is `lua_isnumber` (`0x6f34d0`), numeric strings included.
    if let Some(n) = lua.coerce_number(key.clone())? {
        return index_row(model, n).map(AddonKey::Index);
    }
    // `lua_isstring`, `lua_tostring` (`0x6f3510`, `0x6f3690`); lossy, as a Lua string is bytes.
    match key {
        Value::String(s) => Ok(AddonKey::Name(s.to_string_lossy())),
        _ => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

/// Rebuild the Lua index array as `AddOn_ReadAddonInfoReply` (`0x51da70`) does in its tail,
/// `[0x51dc30, 0x51dcdf)`, the array's only builder: hidden records dropped (`0x51dc4f`), the
/// rest sorted case-insensitively by `## Title`, else folder name (`0x51deb0` through
/// `AddOn_GetTitle 0x51df20`, `SStrCmpI`), by the CRT `qsort` (`0x73f727`) from the list's order.
/// Titles that match without case sit where `qsort` leaves them. Empty until the server replies.
fn rebuild_index(model: &mut Model) {
    let Some(reply) = model.addon_reply.take() else {
        model.addon_index = Vec::new(); // no reply yet, as in the reference
        return;
    };
    apply_reply(&mut model.addons, &reply);
    model.addon_reply = Some(reply);
    let mut index: Vec<usize> = (0..model.addons.len())
        .filter(|&i| !model.addons[i].hidden)
        .collect();
    let title = |i: usize| {
        let a = &model.addons[i];
        a.title.as_deref().unwrap_or(&a.name)
    };
    crate::crt::qsort(&mut index, |&a, &b| {
        crate::crt::str_cmp_i(title(a), title(b))
    });
    model.addon_index = index;
}

/// The reply's records onto the rows they name, as `0x51da70` walks them. Each record sets the
/// url when it carries one (`0x51db6a`), the new-version byte when its revision is past the
/// installed one (`0x51db0d`; nothing clears it), then always INSECURE and not corrupt
/// (`0x51db77`-`0x51db7e`); status 2 hides the row, any other status shows it (`0x51db84`,
/// `0x51db8c`); status 0 bans it with no signature check (`0x51dc0a`), and any other runs the
/// check, whose answer 3 makes it SECURE (`0x51dbfb`).
///
/// Deviation: the files the handler writes beside the install (`<Name>.pub` with the key,
/// `<Name>.url`; `0x51dce0`, `0x51dde0`) are not written, as the install is read-only; the key is
/// never read either, as there is no signature to verify with it.
fn apply_reply(addons: &mut [AddOnInfo], reply: &[AddonReply]) {
    for record in reply {
        let Some(a) = addons
            .iter_mut()
            .find(|a| a.name.eq_ignore_ascii_case(&record.name))
        else {
            continue;
        };
        if let Some(url) = &record.url {
            a.url = Some(url.clone());
        }
        if record.revision.is_some_and(|r| r > a.installed_revision) {
            a.new_version = true;
        }
        a.security = AddonSecurity::Insecure;
        a.corrupt = false;
        a.hidden = record.status == 2;
        if record.status == 0 {
            a.security = AddonSecurity::Banned;
        } else if a.signature_ok {
            a.security = AddonSecurity::Secure;
        }
    }
}

/// A name's registry row, case-insensitive: a `.toc` may spell a name any way.
fn by_name(model: &Model, name: &str) -> Option<usize> {
    model
        .addons
        .iter()
        .position(|a| a.name.eq_ignore_ascii_case(name))
}

/// A key's registry row, for the verbs that answer an unknown name rather than raise.
fn row_of(model: &Model, key: &AddonKey) -> Option<usize> {
    match key {
        AddonKey::Index(i) => Some(*i),
        AddonKey::Name(n) => by_name(model, n),
    }
}

/// The enable setter (`0x51ea20`), the one write every enable verb makes, for the current
/// character's row: the character's enable hash, which the setter dirties only when the row
/// changes or is new, and, for an installed addon (`row`), the registry bit the gate reads. A
/// name nothing installed answers to still gets its hash row, which the logout writer saves.
fn set_enabled(model: &mut Model, row: Option<usize>, name: &str, on: bool) {
    if let Some(i) = row {
        model.addons[i].enabled = on;
    }
    model.addon_enable.set(name, on);
}

/// The registry as [`super::addon_gate`] rows, the one input every verb's verdict reads.
fn gate_rows(model: &Model) -> Vec<GateRow<'_>> {
    model
        .addons
        .iter()
        .map(|a| GateRow {
            name: &a.name,
            enabled: a.enabled,
            interface: a.interface,
            load_on_demand: a.load_on_demand,
            banned: a.security == AddonSecurity::Banned,
            corrupt: a.corrupt,
            loaded: a.loaded,
            dependencies: a.dependencies.iter().map(String::as_str).collect(),
        })
        .collect()
}

/// `checkAddonVersion`, read per query as `IsAddonVersionCheckEnabled` (`0x51f180`) does: the
/// record's integer (`+0x28`, `SStrToInt` of the value, `0x63e0b0`) is nonzero, so `""`, `"abc"`
/// and `"false"` read as off; unset, it is the registered default `"1"`, check on.
pub(super) fn version_check(model: &Model) -> bool {
    model
        .cvars
        .get("checkaddonversion")
        .is_none_or(|s| super::binding_abi::s_str_to_int(&s.value) != 0)
}

/// [`can_load`] in the in-game flavour (`dl=1`). A loaded addon is loadable before the gate
/// runs, as in `0x48e390`, so disabling it mid-session does not mark it unloadable.
fn verdict(model: &Model, i: usize) -> Verdict {
    if model.addons[i].loaded {
        return Verdict::Loadable;
    }
    can_load(&gate_rows(model), i, true, version_check(model))
}

fn lua_str(lua: &Lua, s: &str) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(s)?))
}

fn opt_str(lua: &Lua, s: &Option<String>) -> mlua::Result<Value> {
    match s {
        Some(s) => lua_str(lua, s),
        None => Ok(Value::Nil),
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumAddOns",
        lua.create_function(|lua, ()| {
            // `0x51def0` returns `[0xbe1b90]`, the sorted array's count, not the registry's.
            Ok(lua
                .app_data_ref::<Model>()
                .expect("model")
                .addon_index
                .len())
        })?,
    )?;

    // In-game `0x48e390` returns seven: `name, title, notes, enabled, loadable, reason,
    // security`, the flags the number 1 or nil (`lua_pushnumber` via `0x6f3810`). The glue's
    // `0x46d460` returns eight, `url` fourth, from `0x51e180`: the server's, never the `.toc`'s.
    // In-game no verb returns it.
    g.set(
        "GetAddOnInfo",
        lua.create_function(|lua, key: Value| {
            let model = lua.app_data_ref::<Model>().expect("model");
            let i = match addon_key(lua, &model, &key, USAGE_INFO)? {
                AddonKey::Index(i) => i,
                AddonKey::Name(name) => match by_name(&model, &name) {
                    Some(i) => i,
                    // An unknown name answers placeholders, the caller's own string first
                    // (`0x48e401`).
                    None => {
                        return Ok(MultiValue::from_vec(vec![
                            lua_str(lua, &name)?,
                            Value::Nil,
                            Value::Nil,
                            Value::Nil,
                            Value::Nil,
                            lua_str(lua, "MISSING")?,
                            lua_str(lua, "INSECURE")?,
                        ]))
                    }
                },
            };
            let reason = verdict(&model, i).token();
            let a = &model.addons[i];
            Ok(MultiValue::from_vec(vec![
                lua_str(lua, &a.name)?,
                opt_str(lua, &a.title)?,
                opt_str(lua, &a.notes)?,
                flag(a.enabled),
                flag(reason.is_none()),
                match &reason {
                    Some(r) => lua_str(lua, r)?,
                    None => Value::Nil,
                },
                lua_str(lua, a.security.token())?,
            ]))
        })?,
    )?;

    g.set(
        "IsAddOnLoaded",
        lua.create_function(|lua, key: Value| {
            let model = lua.app_data_ref::<Model>().expect("model");
            let key = addon_key(lua, &model, &key, USAGE_LOADED)?;
            // `0x51e6f0` answers the record's loaded byte `[rec+0x18]`; an unknown name pushes nil
            // (`0x48e95b`).
            Ok(flag(
                row_of(&model, &key).is_some_and(|i| model.addons[i].loaded),
            ))
        })?,
    )?;

    g.set(
        "IsAddOnLoadOnDemand",
        lua.create_function(|lua, key: Value| {
            let model = lua.app_data_ref::<Model>().expect("model");
            let key = addon_key(lua, &model, &key, USAGE_LOAD_ON_DEMAND)?;
            Ok(flag(
                row_of(&model, &key).is_some_and(|i| model.addons[i].load_on_demand),
            ))
        })?,
    )?;

    // One return per dependency, none for none (the glue's `AddonList.lua:137` takes varargs).
    g.set(
        "GetAddOnDependencies",
        lua.create_function(|lua, key: Value| {
            let model = lua.app_data_ref::<Model>().expect("model");
            let key = addon_key(lua, &model, &key, USAGE_DEPENDENCIES)?;
            // An unknown name comes back NULL from `0x51e350` and answers nothing (`0x48e68a`).
            let Some(i) = row_of(&model, &key) else {
                return Ok(MultiValue::new());
            };
            let mut out = Vec::new();
            for dep in &model.addons[i].dependencies {
                out.push(lua_str(lua, dep)?);
            }
            Ok(MultiValue::from_iter(out))
        })?,
    )?;

    // The raw `## Key: Value` by key, as an addon reads its own `## Version`.
    g.set(
        "GetAddOnMetadata",
        lua.create_function(|lua, (key, field): (Value, Value)| {
            let model = lua.app_data_ref::<Model>().expect("model");
            let key = addon_key(lua, &model, &key, USAGE_METADATA)?;
            // A missing or non-string argument 2 raises the same `Usage:` (`0x48e59c`, `0x48e5cb`).
            let field = super::binding_abi::string_arg(lua, field, USAGE_METADATA)?;
            let Some(i) = row_of(&model, &key) else {
                return Ok(Value::Nil);
            };
            // The map's insert replaces (`0x51d77b`): a key a host lists twice answers its last.
            match model.addons[i]
                .directives
                .iter()
                .rev()
                .find(|(k, _)| k.eq_ignore_ascii_case(&field))
            {
                Some((_, v)) => lua_str(lua, v),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // One argument, keyed on the logged-in character (`0x48e690`, `0x48e760`, via `0x5abdc0`);
    // the `(character, index)` form is the glue's (`0x46d7b0`, `0x46d8a0`). A name goes to the
    // setter as the caller spelled it (`0x6f3690`), installed or not: the setter never checks the
    // installed list, and a name its hash lacks gets a new node (`0x51eaf2`, allocated `0x51eb62`,
    // appended at the tail with the state at `0x51eb6f`).
    for (name, on, usage) in [
        ("EnableAddOn", true, USAGE_ENABLE),
        ("DisableAddOn", false, USAGE_DISABLE),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, key: Value| {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let (row, name) = match addon_key(lua, &model, &key, usage)? {
                    AddonKey::Index(i) => (Some(i), model.addons[i].name.clone()),
                    AddonKey::Name(n) => (by_name(&model, &n), n),
                };
                set_enabled(&mut model, row, &name, on);
                Ok(())
            })?,
        )?;
    }

    // No arguments read (`0x48e720`, `0x48e7f0`). The loop runs over the Lua index array, not the
    // registry: bound `0x51def0`, each name from `0x51df00`, into the setter `0x51ea20`. The array
    // leaves out what `SMSG_ADDON_INFO` hid (`0x51dc4f`), which is every `Blizzard_*` addon on a
    // server that hides them (vmangos `AddonHandler.cpp:129`), so their state is never touched.
    for (name, on) in [("EnableAllAddOns", true), ("DisableAllAddOns", false)] {
        g.set(
            name,
            lua.create_function(move |lua, _: MultiValue| {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                for k in 0..model.addon_index.len() {
                    let i = model.addon_index[k];
                    let name = model.addons[i].name.clone();
                    set_enabled(&mut model, Some(i), &name, on);
                }
                Ok(())
            })?,
        )?;
    }

    // `0x48e830` reloads the enable hash from `AddOns.txt`, clean (`0x51ec59`), and the file is
    // unchanged until the shutdown write, so reverting to the state as registered is the same.
    g.set(
        "ResetDisabledAddOns",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model");
            for a in &mut model.addons {
                a.enabled = a.saved_enabled;
            }
            model.addon_enable = model.addon_enable_saved.clone();
            Ok(())
        })?,
    )?;

    g.set(
        "LoadAddOn",
        lua.create_function(|lua, key: Value| {
            let index = {
                let model = lua.app_data_ref::<Model>().expect("model");
                let key = addon_key(lua, &model, &key, USAGE_LOAD)?;
                row_of(&model, &key)
            };
            let Some(i) = index else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    lua_str(lua, "MISSING")?,
                ]));
            };
            // One record for the call and the calls inside it (`ds:0xb4e3e0` counts them,
            // `0x48ea85`/`0x48ea9d`), drained when the outermost returns (`0x48eab4`).
            lua.app_data_mut::<Model>()
                .expect("model")
                .load_log
                .demand_depth += 1;
            let mut host = DemandLoad { lua };
            let version_check = version_check(&lua.app_data_ref::<Model>().expect("model"));
            let loaded = load_row(lua, &mut host, i, true, version_check);
            {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let log = &mut model.load_log;
                log.demand_depth -= 1;
                if log.demand_depth == 0 && !log.demand.is_empty() {
                    let lines = log.demand.take_lines();
                    log.writes.push(crate::status::LogWrite::Append(lines));
                }
            }
            if loaded {
                return Ok(MultiValue::from_vec(vec![Value::Integer(1), Value::Nil]));
            }
            // `0x48eae7`: a refusal is explained by asking the gate again; a gate that now says yes
            // leaves `UNKNOWN_ERROR` (`0x842ebc`), and an addon with no files to read is `MISSING`.
            let reason = {
                let model = lua.app_data_ref::<Model>().expect("model");
                match verdict(&model, i).token() {
                    Some(reason) => reason,
                    None if !host.available(i) => "MISSING".to_owned(),
                    None => "UNKNOWN_ERROR".to_owned(),
                }
            };
            Ok(MultiValue::from_vec(vec![
                Value::Nil,
                lua_str(lua, &reason)?,
            ]))
        })?,
    )?;

    Ok(())
}

/// The part of `AddOn_Load` (`0x51f240`) that differs by caller: the startup walk (`0x51f600`)
/// and `LoadAddOn` (`0x48e980`) share [`load_row`] and bring their own way to run an addon's files
/// and to report what failed. Rows are registry indices.
pub trait AddonLoad {
    /// Whether row `i` has anything to read from: a capture has no AddOns folder. Asked after the
    /// gate; an addon that answers no does not load.
    fn available(&self, _i: usize) -> bool {
        true
    }

    /// Row `i`'s own load, between its dependencies and `ADDON_LOADED`: its `.toc` files,
    /// `Bindings.xml` and saved variables, under its load context (`0x51f3fa`-`0x51f53b`).
    fn run(&mut self, i: usize);

    /// The gate refused row `i` for a reason of its own (`DISABLED`, `INTERFACE_VERSION`, ...).
    fn refused(&mut self, _i: usize, _reason: &str) {}

    /// A required dependency of row `i` is missing or failed, whether the gate saw it before the
    /// load began or the load found it (`0x51f357`).
    fn dep_failed(&mut self, _i: usize, _dep: &str) {}
}

/// `AddOn_Load` (`0x51f240`) on registry row `i`, the one loader every surface runs; `demand` is
/// its `dl` (`LoadAddOn` 1, the startup walk 0) and `version_check` the live `checkAddonVersion`.
/// Returns the reference's bool. In its order:
/// - a loaded addon answers 1 at once (`0x51f2d6`);
/// - `AddOn_CanLoad` decides next (`0x51f2fa`), which already answers "can load" for an addon it is
///   checking, so a cycle of required dependencies passes;
/// - the addon is marked loaded (`0x51f313`) before any dependency, its files or `ADDON_LOADED`,
///   which is what ends a cycle: a re-entry takes the first line. Inside a cycle `IsAddOnLoaded`
///   is 1 from the addon's own files, and the second addon's `ADDON_LOADED` fires first;
/// - optional dependencies load, failures ignored (`0x51f320`); required ones load, and one that
///   fails clears the mark and fails this addon (`0x51f343`-`0x51f357`, `0x51f5ee`);
/// - the addon's files run, `ADDON_LOADED` fires (`0x51f5ad`), and each addon whose `## LoadWith`
///   names this one loads (`0x51f5b2`-`0x51f5e1`), failures ignored.
pub(super) fn load_row(
    lua: &Lua,
    host: &mut dyn AddonLoad,
    i: usize,
    demand: bool,
    version_check: bool,
) -> bool {
    let (name, optional, required, dependents) = {
        let model = lua.app_data_ref::<Model>().expect("model");
        let a = &model.addons[i];
        if a.loaded {
            return true;
        }
        match can_load(&gate_rows(&model), i, demand, version_check) {
            Verdict::Loadable => {}
            Verdict::Refused {
                reason: Some(reason),
                ..
            } => {
                drop(model);
                host.refused(i, reason);
                return false;
            }
            Verdict::Refused { reason: None, .. } => {
                let dep = failing_dependency(&model, i, demand, version_check);
                drop(model);
                host.dep_failed(i, &dep);
                return false;
            }
        }
        (
            a.name.clone(),
            a.optional_dependencies.clone(),
            a.dependencies.clone(),
            a.dependents.clone(),
        )
    };
    if !host.available(i) {
        return false;
    }
    lua.app_data_mut::<Model>().expect("model").addons[i].loaded = true;
    for dep in &optional {
        let row = by_name(&lua.app_data_ref::<Model>().expect("model"), dep);
        if let Some(d) = row {
            load_row(lua, host, d, demand, version_check);
        }
    }
    for dep in &required {
        let row = by_name(&lua.app_data_ref::<Model>().expect("model"), dep);
        if !row.is_some_and(|d| load_row(lua, host, d, demand, version_check)) {
            lua.app_data_mut::<Model>().expect("model").addons[i].loaded = false;
            host.dep_failed(i, dep);
            return false;
        }
    }
    host.run(i);
    super::event::fire_global(lua, "ADDON_LOADED", &[super::ScriptValue::Str(name)]);
    for d in dependents {
        load_row(lua, host, d, demand, version_check);
    }
    true
}

/// The first required dependency of row `i` that cannot load, for the gate's `DEP_` refusal: a
/// name no addon answers to, or an unloaded one the gate refuses.
fn failing_dependency(model: &Model, i: usize, demand: bool, version_check: bool) -> String {
    let rows = gate_rows(model);
    let a = &model.addons[i];
    a.dependencies
        .iter()
        .find(|dep| match by_name(model, dep) {
            None => true,
            Some(d) => {
                !model.addons[d].loaded && !can_load(&rows, d, demand, version_check).loadable()
            }
        })
        .or(a.dependencies.first())
        .cloned()
        .unwrap_or_default()
}

/// `LoadAddOn`'s way to run an addon: from the AddOns folder, or the patch chain for Blizzard's
/// own, into the demand-load record.
struct DemandLoad<'a> {
    lua: &'a Lua,
}

impl DemandLoad<'_> {
    /// The reader for row `i`'s files; `None` when its source is absent (a capture has no folder,
    /// a bare VM no chain).
    fn reader(&self, i: usize) -> Option<Reader> {
        let model = self.lua.app_data_ref::<Model>().expect("model");
        if model.addons[i].chain {
            let reader = model.addons_chain_reader.clone()?;
            Some(Box::new(move |req: &str| {
                reader(&format!("Interface/AddOns/{req}"))
            }))
        } else {
            let root = model.addons_root.clone()?;
            Some(Box::new(move |req: &str| read_under(&root, req)))
        }
    }
}

impl AddonLoad for DemandLoad<'_> {
    fn available(&self, i: usize) -> bool {
        self.reader(i).is_some()
    }

    fn run(&mut self, i: usize) {
        let lua = self.lua;
        let Some(read) = self.reader(i) else { return };
        let (name, files, secure) = {
            let model = lua.app_data_ref::<Model>().expect("model");
            let a = &model.addons[i];
            (
                a.name.clone(),
                a.files.clone(),
                a.security == AddonSecurity::Secure,
            )
        };
        // The files, `Bindings.xml` and the saved variables run under the addon's context.
        let outer = swap_exec_context(lua, addon_load_context(&name, secure));
        let mut toc = crate::status::Status::default();
        run_files(lua, &name, &read, &files, &mut toc);
        // The toc's record under its banner, then the add-on's (`0x6eddc8`, `0x51f464`).
        let debug = framexml_debug(lua);
        let mut own = crate::status::Status::default();
        let toc_path = crate::status::addon_toc_path(&name);
        toc.close_into(&mut own, debug, crate::status::toc_banner(&toc_path));
        {
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let banner = crate::status::addon_banner(&name);
            own.close_into(&mut model.load_log.demand, debug, banner);
        }
        // `Bindings.xml` loads between the files and the saved variables.
        load_bindings(lua, &name, &read);
        load_saved_variables(lua, i);
        swap_exec_context(lua, outer);
    }
}

/// Register the addon's `Bindings.xml` where the reference loads it (`0x51f400`): after the
/// addon's files, whose functions its bindings call, and before its saved variables. A missing
/// file is the normal case and silent; it reads through the same sandbox as the other files.
fn load_bindings(lua: &Lua, name: &str, read: &Reader) {
    let path = crate::loader::join_ref(name, "Bindings.xml");
    let Some(bytes) = read(&path) else {
        return;
    };
    match crate::bindings_xml::parse(&crate::source::decode(&bytes)) {
        Ok(bindings) => super::keybind::register_addon_bindings(lua, name, &bindings),
        // Reported, not swallowed: otherwise a broken binding file leaves keys dead silently.
        Err(e) => {
            let msg = format!("{name}/Bindings.xml: {e}");
            log_error(lua, &msg, &msg);
        }
    }
}

/// Run the addon's saved-variables files, account then per-character (`0x51f4b5`, `0x51f53b`),
/// as chunks in the shared globals with no `setfenv` (`0x704bc0`, `0x704ae0`), between its files
/// and `ADDON_LOADED` so saved values win. A file runs whenever it exists (`0x51f4a2`), whatever
/// the toc declares, which gates only the write; a missing file is skipped (`0x51f4a9`, `0x51f530`).
fn load_saved_variables(lua: &Lua, i: usize) {
    let (name, account, character) = {
        let model = lua.app_data_ref::<Model>().expect("model");
        (
            model.addons[i].name.clone(),
            model.addons_saved_account.clone(),
            model.addons_saved_character.clone(),
        )
    };
    let hold = |path: std::path::PathBuf| {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        if !model.held_saved_files.contains(&path) {
            model.held_saved_files.push(path);
        }
    };
    for dir in [account, character].into_iter().flatten() {
        let path = dir.join(format!("{name}.lua"));
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, // the first-run case
            Err(e) => {
                // Present but unreadable is held, like a file that will not run.
                let msg = format!("{}: {e}", path.display());
                log_error(lua, &msg, &msg);
                hold(path);
                continue;
            }
        };
        match run_chunk(
            lua,
            &bytes,
            &crate::script::addon_chunk_name(&name.to_string(), &format!("{name}.lua")),
        ) {
            Ok(()) => {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                if !model.loaded_saved_files.contains(&path) {
                    model.loaded_saved_files.push(path);
                }
            }
            Err(e) => {
                // Deviation: the reference fails silently; we report it, as a player needs to
                // know, and hold the file so the shutdown write keeps it.
                log_error(
                    lua,
                    &format!("{}: {e}", path.display()),
                    &super::lua_message(&e),
                );
                hold(path);
            }
        }
    }
}

/// One addon's byte source by path under the AddOns root (`<Name>/<file>`): the folder for a
/// player's addon, the patch chain for a Blizzard LoadOnDemand one.
type Reader = Box<dyn Fn(&str) -> Option<Vec<u8>>>;

/// `FrameXML_Debug` above 0, which puts every banner in the load log.
fn framexml_debug(lua: &Lua) -> bool {
    lua.app_data_ref::<Model>()
        .expect("model")
        .framexml_debug
        .get()
        > 0
}

/// Where the reader's paths sit in the install's, for the load log's lines.
const ADDONS_ROOT: &str = "Interface/AddOns/";

/// Run the addon's `.toc` files in order, by the startup walk's rules: `.lua` is a chunk,
/// anything else FrameXML, each path resolved against the including file's directory. `toc` is
/// the `.toc`'s load record (`0x6edb90`).
fn run_files(lua: &Lua, name: &str, read: &Reader, files: &[String], toc: &mut Status) {
    let provider = |req: &str| -> Option<Vec<u8>> { read(req) };
    for file in files {
        let path = crate::loader::join_ref(name, file);
        let install = format!("{ADDONS_ROOT}{path}");
        let lua_file = crate::status::runs_as_lua(&path);
        let Some(bytes) = read(&path) else {
            // The reference logs `Couldn't open %s` and carries on, so the addon still loads and
            // `IsAddOnLoaded` answers 1; the miss is warned, never a script error.
            toc.report(
                crate::status::FAILURE,
                crate::status::missing(&install, lua_file),
            );
            load_miss(lua, name, file);
            continue;
        };
        if lua_file {
            if let Err(e) = run_chunk(lua, &bytes, &crate::script::addon_chunk_name(name, file)) {
                log_error(lua, &format!("{name}/{file}: {e}"), &super::lua_message(&e));
            }
            continue;
        }
        let doc = match crate::framexml::parse(&crate::source::decode(&bytes)) {
            Ok(d) => d,
            Err(e) => {
                // A document that will not parse is a load failure, not a raise: the reference
                // logs `Couldn't parse XML in %s` (`0x846fd8`) to `FrameXML.log` and goes on.
                toc.report(crate::status::FAILURE, crate::status::unparsed(&install));
                load_failure(lua, &format!("{name}/{file}: {e}"));
                continue;
            }
        };
        let mut report = crate::loader::load_into_under(lua, &doc, &path, &provider, ADDONS_ROOT);
        let banner = crate::status::file_banner(&install);
        std::mem::take(&mut report.status).close_into(toc, framexml_debug(lua), banner);
        // Warnings take the `<Addon>/<file>` prefix; a bare loader warning names no document.
        for w in &report.warnings {
            crate::script::diagnostics::record_warning(lua, &format!("{name}/{file}: {w}"));
        }
        for (e, heard) in report.raised() {
            log_error(lua, &format!("{name}/{file}: {e}"), heard);
        }
    }
}

/// A `.toc` file the package does not ship: a load failure at warning severity, as the startup
/// walk records it, and never in [`Model::errors`], since nothing raised.
fn load_miss(lua: &Lua, name: &str, file: &str) {
    load_failure(lua, &format!("{name}/{file}: not found"));
}

/// Record a load failure that did not raise, and warn on the host channel the app drains at
/// `warn!` (`benilla-ui` has no logger).
fn load_failure(lua: &Lua, msg: &str) {
    let msg = format!("LoadAddOn: {msg}");
    crate::script::diagnostics::record_load_failure(lua, &msg);
    // `warn_host_only`, not `record_warning`: the line above already kept it as a `Load` row.
    lua.app_data_mut::<Model>()
        .expect("model")
        .warn_host_only(msg);
}

/// Read `root/rel`, refusing a path that leaves `root`, lexically before any filesystem call;
/// `join_ref` has already resolved the path, so an escape shows as a leading `..`.
fn read_under(root: &std::path::Path, rel: &str) -> Option<Vec<u8>> {
    let rel = std::path::Path::new(rel);
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    std::fs::read(root.join(rel)).ok()
}

/// Run a chunk read off disk, BOM stripped: [`crate::script::UiScript::run_chunk`] for the
/// demand-load path, which has no `UiScript`.
fn run_chunk(lua: &Lua, bytes: &[u8], name: &str) -> mlua::Result<()> {
    lua.load(crate::source::chunk(bytes))
        .set_name(name)
        .set_mode(mlua::ChunkMode::Text)
        .exec()
}

/// Record a demand-load error as the startup walk does ([`super::UiScript::report_script_error`]):
/// a `Load` row, dispatched to the player's error handler and put on the host drain. `handler` is
/// the bare Lua message of a raise; a fault that is no raise passes its own text.
fn log_error(lua: &Lua, msg: &str, handler: &str) {
    let msg = format!("LoadAddOn: {msg}");
    super::diagnostics::record_load_failure(lua, &msg);
    let mut model = lua.app_data_mut::<Model>().expect("model");
    model
        .pending_error_dispatch
        .push(super::model::HandlerArg::Text(handler.to_string()));
    model.errors.push(msg);
}

/// The script-execution context an addon's load runs under: `AddOn_Load` sets `[0xceeac0]` to the
/// record's name (`[rec+0x14]`) when its security state `[rec+0x24]` is non-zero, else to 0
/// (`0x51f37e`-`0x51f387`), so only a secure addon loads under the UI's own context. `None` is 0.
pub fn addon_load_context(name: &str, secure: bool) -> Option<String> {
    (!secure).then(|| name.to_owned())
}

/// Set the context word, returning the one it replaces for the caller to restore, as `AddOn_Load`
/// saves it (`0x51f36d`) and restores it before `ADDON_LOADED` (`0x51f584`).
fn swap_exec_context(lua: &Lua, context: Option<String>) -> Option<String> {
    lua.app_data_ref::<Model>()
        .expect("model")
        .exec_context
        .replace(context)
}

/// The host's side: the registry, where its files live, and what to write back.
impl super::UiScript {
    /// Set the script-execution context for the startup walk's addon loads
    /// ([`addon_load_context`]), returning the previous one to restore after.
    pub fn swap_exec_context(&self, context: Option<String>) -> Option<String> {
        swap_exec_context(self.lua(), context)
    }

    /// Seat the reader chain-sourced addons (`AddOnInfo::chain`) are read through.
    pub fn set_addon_chain_reader(&self, reader: AddonChainReader) {
        self.model_mut().addons_chain_reader = Some(std::rc::Rc::from(reader));
    }
    /// Replace the AddOn registry; called once at world entry, after discovery.
    pub fn register_addons(
        &mut self,
        mut addons: Vec<AddOnInfo>,
        root: Option<PathBuf>,
        saved_account: Option<PathBuf>,
        saved_character: Option<PathBuf>,
    ) {
        // The enable state as registered is the last-saved one, `ResetDisabledAddOns`'s target.
        for a in &mut addons {
            a.saved_enabled = a.enabled;
        }
        // The reverse of `## LoadWith`, once the whole list is known (`0x51c7c0`-`0x51c8c9`): each
        // addon is appended to every addon it names, in list order, a repeat included.
        for a in &mut addons {
            a.dependents.clear();
        }
        for r in 0..addons.len() {
            for t in 0..addons[r].load_with.len() {
                let target = addons
                    .iter()
                    .position(|a| a.name.eq_ignore_ascii_case(&addons[r].load_with[t]));
                if let Some(target) = target {
                    addons[target].dependents.push(r);
                }
            }
        }
        let mut model = self.model_mut();
        model.addons = addons;
        model.addons_root = root;
        model.addons_saved_account = saved_account;
        model.addons_saved_character = saved_character;
        // The reference builds the array when the server's reply lands, before world entry, so
        // the rebuild runs here; with no reply the array stays empty, as there.
        rebuild_index(&mut model);
    }

    /// Record the `SMSG_ADDON_INFO` (`0x2ef`) verdict, the addons answered status 2; the caller
    /// pairs the nameless records with `CMSG_AUTH_SESSION`'s order (`0x51da70`). An empty slice
    /// is a reply that hid nothing, unlike no call at all.
    pub fn note_addon_info_reply(&mut self, hidden: &[String]) {
        let records = hidden
            .iter()
            .map(|name| AddonReply {
                name: name.clone(),
                status: 2,
                ..AddonReply::default()
            })
            .collect();
        self.note_addon_info_records(records);
    }

    /// Record the `SMSG_ADDON_INFO` (`0x2ef`) reply whole: a record per addon the client sent,
    /// named for it by the caller. An empty list is a reply that changed nothing, unlike no
    /// call at all.
    pub fn note_addon_info_records(&mut self, records: Vec<AddonReply>) {
        let mut model = self.model_mut();
        model.addon_reply = Some(records);
        rebuild_index(&mut model);
    }

    /// Whether the registry row named `name` is SECURE (`[rec+0x24] == 0`), the context its files
    /// load under ([`addon_load_context`]); an unknown name is not.
    pub fn addon_is_secure(&self, name: &str) -> bool {
        self.model_ref()
            .addons
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .is_some_and(|a| a.security == AddonSecurity::Secure)
    }

    /// The registry row a folder name answers to, case-insensitively.
    pub fn addon_row(&self, name: &str) -> Option<usize> {
        by_name(&self.model_ref(), name)
    }

    /// The registry rows whose `## LoadWith` names row `i`, in the order `AddOn_Load` loads them
    /// after its `ADDON_LOADED` (`0x51f5b2`-`0x51f5e1`).
    pub fn addon_dependents(&self, i: usize) -> Vec<usize> {
        self.model_ref()
            .addons
            .get(i)
            .map(|a| a.dependents.clone())
            .unwrap_or_default()
    }

    /// `AddOn_Load` (`0x51f240`) on registry row `i` ([`load_row`]) for a host that runs the
    /// addon's files itself, as the startup walk does (`demand` false).
    pub fn load_addon_row(
        &self,
        host: &mut dyn AddonLoad,
        i: usize,
        demand: bool,
        version_check: bool,
    ) -> bool {
        load_row(self.lua(), host, i, demand, version_check)
    }

    /// Run one addon's saved-variables files at the startup walk's position, as `LoadAddOn` does.
    pub fn load_addon_saved_variables(&self, name: &str) {
        let i = self
            .model_ref()
            .addons
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(name));
        if let Some(i) = i {
            load_saved_variables(self.lua(), i);
        }
    }

    /// `(name, account names, per-character names)` to write at shutdown, for each loaded addon,
    /// one declaring none included (its files go, `0x51f859`): the reference gates the write on
    /// the loaded byte (`0x51f711`).
    pub fn addon_saved_variable_sets(&self) -> Vec<(String, Vec<String>, Vec<String>)> {
        self.model_ref()
            .addons
            .iter()
            .filter(|a| a.loaded)
            .map(|a| {
                (
                    a.name.clone(),
                    a.saved_variables.clone(),
                    a.saved_variables_per_character.clone(),
                )
            })
            .collect()
    }

    /// Mark an addon the startup walk ran as loaded, for `IsAddOnLoaded`.
    pub fn mark_addon_loaded(&mut self, name: &str) {
        if let Some(a) = self
            .model_mut()
            .addons
            .iter_mut()
            .find(|a| a.name.eq_ignore_ascii_case(name))
        {
            a.loaded = true;
        }
    }

    /// Seat the current character's enable hash as the loader read it from `AddOns.txt`
    /// (`0x51ebe0`, re-read at every UI boot, `0x48ff2a`).
    pub fn set_addon_enable_hash(&mut self, hash: super::EnableHash) {
        let mut model = self.model_mut();
        model.addon_enable_saved = hash.clone();
        model.addon_enable = hash;
    }

    /// The shutdown writer's rows (`0x490c88` into `0x51ef20`): the current character's whole
    /// enable hash when a verb changed it this session, else `None`, and nothing is written.
    pub fn take_addon_enable_rows(&self) -> Option<Vec<(String, bool)>> {
        self.model_mut().addon_enable.take_dirty()
    }
}
