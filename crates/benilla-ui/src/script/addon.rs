//! The in-game AddOn API, the twelve `engine` addon globals of the 1.12 `_G`
//! (`reference/1.12-globals.tsv`), over a registry the host fills at discovery. A verb that names
//! an addon takes a 1-based index or a name. `reason` is a token the stock UI splices into
//! `getglobal("ADDON_"..reason)` (`UIParent.lua:642`), so its spelling is the contract. Verdicts
//! come from [`super::addon_gate`], the reference's `AddOn_CanLoad` (`0x51e780`).

use std::path::PathBuf;

use mlua::{Lua, MultiValue, Value};

use super::addon_gate::{can_load, GateRow, Verdict};
use super::binding_abi::flag;
use super::Model;

/// Reads a chain-sourced addon's file by its path inside the patch chain.
pub type AddonChainReader = Box<dyn Fn(&str) -> Option<Vec<u8>>>;

/// [`AddonChainReader`], shared so a load can call it without holding the model.
pub(crate) type SharedChainReader = std::rc::Rc<dyn Fn(&str) -> Option<Vec<u8>>>;

/// One addon as the AddOn API sees it, filled by the host at discovery.
#[derive(Clone, Debug, Default)]
pub struct AddOnInfo {
    /// The folder name: `GetAddOnInfo`'s first return and `ADDON_LOADED`'s `arg1`.
    pub name: String,
    /// `## Title`; `None` when absent, which the reference returns as nil.
    pub title: Option<String>,
    pub notes: Option<String>,
    pub url: Option<String>,
    /// `## Secure: 1`, which Blizzard's own addons carry; drives the `security` token.
    pub secure: bool,
    pub load_on_demand: bool,
    /// `## Dependencies` / `## RequiredDeps`.
    pub dependencies: Vec<String>,
    /// Every `## Key: Value` in manifest order, for `GetAddOnMetadata`.
    pub directives: Vec<(String, String)>,
    /// The `.toc`'s file list in order: what `LoadAddOn` runs.
    pub files: Vec<String>,
    /// `## SavedVariables`: globals restored from and written to the account-wide file.
    pub saved_variables: Vec<String>,
    /// `## SavedVariablesPerCharacter`, loaded after the account file so its values win.
    pub saved_variables_per_character: Vec<String>,
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

/// The prologue; `usage` is the verb's own `.data` string.
fn addon_key(lua: &Lua, model: &Model, key: &Value, usage: &'static str) -> mlua::Result<AddonKey> {
    // `coerce_number` is `lua_isnumber` (`0x6f34d0`), numeric strings included.
    if let Some(n) = lua.coerce_number(key.clone())? {
        // `_ftol` (`0x40a2b0`) truncates; minus one, compared unsigned, so 0 raises like count + 1.
        let index0 = (n as i64 as i32).wrapping_sub(1) as u32 as usize;
        // An index into the sorted, filtered name array, not the registry (`0x51df00` reads
        // `[[0xbe1b94] + 4*idx]`, bound `[0xbe1b90]`); the message is `0x837d70`'s.
        return match model.addon_index.get(index0) {
            Some(&row) => Ok(AddonKey::Index(row)),
            None => Err(mlua::Error::RuntimeError(format!(
                "AddOn index must be in the range of 1 to {}",
                model.addon_index.len()
            ))),
        };
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
/// `AddOn_GetTitle 0x51df20`, `SStrCmpI`). Empty until the server replies. The reference's
/// `qsort` (`0x73f727`) is unstable, which a stable sort refines.
fn rebuild_index(model: &mut Model) {
    let Some(hidden) = model.addon_info_hidden.as_ref() else {
        model.addon_index = Vec::new(); // no reply yet, as in the reference
        return;
    };
    let hidden: std::collections::HashSet<&str> = hidden.iter().map(String::as_str).collect();
    for a in &mut model.addons {
        a.hidden = hidden.contains(a.name.to_ascii_lowercase().as_str());
    }
    let mut index: Vec<usize> = (0..model.addons.len())
        .filter(|&i| !model.addons[i].hidden)
        .collect();
    index.sort_by_key(|&i| {
        let a = &model.addons[i];
        a.title.as_deref().unwrap_or(&a.name).to_ascii_lowercase()
    });
    model.addon_index = index;
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
            loaded: a.loaded,
            dependencies: a.dependencies.iter().map(String::as_str).collect(),
        })
        .collect()
}

/// `checkAddonVersion`, read per query as `IsAddonVersionCheckEnabled` (`0x51f180`) does; unset,
/// it is the registered default `"1"`, check on.
fn version_check(model: &Model) -> bool {
    model
        .cvars
        .get("checkaddonversion")
        .is_none_or(|s| s.value != "0")
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
    // `0x46d460` returns eight, `url` fourth; in-game the URL is `GetAddOnMetadata(name, "URL")`.
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
                lua_str(lua, if a.secure { "SECURE" } else { "INSECURE" })?,
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
            match model.addons[i]
                .directives
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&field))
            {
                Some((_, v)) => lua_str(lua, v),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // One argument, keyed on the logged-in character (`0x48e690`, `0x48e760`, via `0x5abdc0`);
    // the `(character, index)` form is the glue's (`0x46d7b0`, `0x46d8a0`). Deviation: an unknown
    // name is a no-op, where the reference adds an enable-hash entry for it, so that a typo leaves
    // no row behind.
    for (name, on, usage) in [
        ("EnableAddOn", true, USAGE_ENABLE),
        ("DisableAddOn", false, USAGE_DISABLE),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, key: Value| {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let key = addon_key(lua, &model, &key, usage)?;
                if let Some(i) = row_of(&model, &key) {
                    model.addons[i].enabled = on;
                }
                Ok(())
            })?,
        )?;
    }

    // No arguments read (`0x48e720`, `0x48e7f0`).
    for (name, on) in [("EnableAllAddOns", true), ("DisableAllAddOns", false)] {
        g.set(
            name,
            lua.create_function(move |lua, _: MultiValue| {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                for a in &mut model.addons {
                    a.enabled = on;
                }
                Ok(())
            })?,
        )?;
    }

    // `0x48e830` reloads the enable state from `AddOns.txt`, which is unchanged until the shutdown
    // write, so reverting to the state as registered is the same thing.
    g.set(
        "ResetDisabledAddOns",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model");
            for a in &mut model.addons {
                a.enabled = a.saved_enabled;
            }
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
            match load_addon(lua, i) {
                Ok(()) => Ok(MultiValue::from_vec(vec![Value::Integer(1), Value::Nil])),
                Err(reason) => Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    lua_str(lua, &reason)?,
                ])),
            }
        })?,
    )?;

    Ok(())
}

/// `AddOn_Load` (`0x51f240`), dependencies first; `Err` is the reason token. Synchronous inside
/// the Lua call, as stock callers use the addon on the next line (`UIParent.lua:577`).
fn load_addon(lua: &Lua, i: usize) -> Result<(), String> {
    let (name, root, files, deps, already, chain, chain_reader) = {
        let model = lua.app_data_ref::<Model>().expect("model");
        let a = &model.addons[i];
        (
            a.name.clone(),
            model.addons_root.clone(),
            a.files.clone(),
            a.dependencies.clone(),
            a.loaded,
            a.chain,
            model.addons_chain_reader.clone(),
        )
    };
    if already {
        return Ok(()); // a redundant load succeeds, as in the reference
    }
    // `LoadAddOn` (`0x48e980`) refuses through `AddOn_CanLoad` and takes the reason from the same
    // gate; an out-of-date addon loads only with the version check off (`0x48ea8c`).
    {
        let model = lua.app_data_ref::<Model>().expect("model");
        if let refused @ Verdict::Refused { .. } = verdict(&model, i) {
            return Err(refused.token().expect("a refusal always carries a token"));
        }
    }
    // An absent source (a capture has no folder, a bare VM no chain) is `MISSING`.
    let read: Reader = if chain {
        let Some(reader) = chain_reader else {
            return Err("MISSING".into());
        };
        Box::new(move |req: &str| reader(&format!("Interface/AddOns/{req}")))
    } else {
        let Some(root) = root else {
            return Err("MISSING".into());
        };
        Box::new(move |req: &str| read_under(&root, req))
    };

    // Loaded is stamped after the gate (`0x51f2fa`), so a refusal stays retryable, and before the
    // dependencies, which is what ends a cycle: `AddOn_Load` has no re-entrancy guard, but its
    // store `0x51f313` precedes both dependency loops (`0x51f320`, `0x51f343`), so a re-entry
    // takes the loaded early-out (`0x51f2d6`). Inside a cycle, then, `IsAddOnLoaded("A")` is 1
    // while B runs and `ADDON_LOADED` fires B before A, as in the reference.
    {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        model.addons[i].loaded = true;
    }

    // Past the gate, a dependency that fails to load maps to its `DEP_` reason.
    for dep in &deps {
        let d = {
            let model = lua.app_data_ref::<Model>().expect("model");
            model
                .addons
                .iter()
                .position(|a| a.name.eq_ignore_ascii_case(dep))
        };
        match d {
            None => return Err("DEP_MISSING".into()),
            Some(d) => {
                let loaded = {
                    let model = lua.app_data_ref::<Model>().expect("model");
                    model.addons[d].loaded
                };
                if !loaded {
                    load_addon(lua, d).map_err(|r| {
                        if r.starts_with("DEP_") {
                            // `DEP_` applies once at any depth (`0x51e8ce`, `0x51e8cf`).
                            r
                        } else {
                            format!("DEP_{r}")
                        }
                    })?;
                }
            }
        }
    }

    run_files(lua, &name, &read, &files);
    // `Bindings.xml` loads between the files and the saved variables.
    load_bindings(lua, &name, &read);
    load_saved_variables(lua, i);
    // `ADDON_LOADED` closes the addon's load (`0x51f5ad`).
    super::event::fire_global(lua, "ADDON_LOADED", &[super::ScriptValue::Str(name)]);
    Ok(())
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
        Err(e) => log_error(lua, &format!("{name}/Bindings.xml: {e}")),
    }
}

/// Run the addon's saved-variables files, account then per-character (`0x51f4b5`, `0x51f53b`),
/// as chunks in the shared globals with no `setfenv` (`0x704bc0`, `0x704ae0`), between its files
/// and `ADDON_LOADED` so saved values win; a missing file is skipped (`0x51f4a9`, `0x51f530`).
fn load_saved_variables(lua: &Lua, i: usize) {
    let (name, account, character, has_account, has_character) = {
        let model = lua.app_data_ref::<Model>().expect("model");
        let a = &model.addons[i];
        (
            a.name.clone(),
            model.addons_saved_account.clone(),
            model.addons_saved_character.clone(),
            !a.saved_variables.is_empty(),
            !a.saved_variables_per_character.is_empty(),
        )
    };
    for (dir, declared) in [(account, has_account), (character, has_character)] {
        if !declared {
            continue; // declares none: no file is read, even if one exists
        }
        let Some(dir) = dir else { continue };
        let path = dir.join(format!("{name}.lua"));
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, // the first-run case
            Err(e) => {
                // Present but unreadable is held, like a file that will not run.
                log_error(lua, &format!("{}: {e}", path.display()));
                let mut model = lua.app_data_mut::<Model>().expect("model");
                if !model.held_saved_files.contains(&path) {
                    model.held_saved_files.push(path);
                }
                continue;
            }
        };
        if let Err(e) = run_chunk(
            lua,
            &bytes,
            &crate::script::addon_chunk_name(&name.to_string(), &format!("{name}.lua")),
        ) {
            // Deviation: the reference fails silently; we report it, as a player needs to know,
            // and hold the file so the shutdown write keeps it.
            log_error(lua, &format!("{}: {e}", path.display()));
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if !model.held_saved_files.contains(&path) {
                model.held_saved_files.push(path);
            }
        }
    }
}

/// One addon's byte source by path under the AddOns root (`<Name>/<file>`): the folder for a
/// player's addon, the patch chain for a Blizzard LoadOnDemand one.
type Reader = Box<dyn Fn(&str) -> Option<Vec<u8>>>;

/// Run the addon's `.toc` files in order, by the startup walk's rules: `.lua` is a chunk,
/// anything else FrameXML, each path resolved against the including file's directory.
fn run_files(lua: &Lua, name: &str, read: &Reader, files: &[String]) {
    let provider = |req: &str| -> Option<Vec<u8>> { read(req) };
    for file in files {
        let path = crate::loader::join_ref(name, file);
        let Some(bytes) = read(&path) else {
            // The reference logs `Couldn't open %s` and carries on, so the addon still loads and
            // `IsAddOnLoaded` answers 1; the miss is warned, never a script error.
            load_miss(lua, name, file);
            continue;
        };
        if std::path::Path::new(file)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("lua"))
        {
            if let Err(e) = run_chunk(lua, &bytes, &crate::script::addon_chunk_name(name, file)) {
                log_error(lua, &format!("{name}/{file}: {e}"));
            }
            continue;
        }
        let doc = match crate::framexml::parse(&crate::source::decode(&bytes)) {
            Ok(d) => d,
            Err(e) => {
                // A document that will not parse is a load failure, not a raise: the reference
                // logs `Couldn't parse XML in %s` (`0x846fd8`) to `FrameXML.log` and goes on.
                load_failure(lua, &format!("{name}/{file}: {e}"));
                continue;
            }
        };
        let report = crate::loader::load_into(lua, &doc, &path, &provider);
        // Warnings take the `<Addon>/<file>` prefix; a bare loader warning names no document.
        for w in report.warnings {
            crate::script::diagnostics::record_warning(lua, &format!("{name}/{file}: {w}"));
        }
        for e in report.errors {
            log_error(lua, &format!("{name}/{file}: {e}"));
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
/// a `Load` row, dispatched to the player's error handler and put on the host drain.
fn log_error(lua: &Lua, msg: &str) {
    let msg = format!("LoadAddOn: {msg}");
    super::diagnostics::record_load_failure(lua, &msg);
    let mut model = lua.app_data_mut::<Model>().expect("model");
    model.pending_error_dispatch.push(msg.clone());
    model.errors.push(msg);
}

/// The host's side: the registry, where its files live, and what to write back.
impl super::UiScript {
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
        let mut model = self.model_mut();
        model.addon_info_hidden = Some(hidden.iter().map(|n| n.to_ascii_lowercase()).collect());
        rebuild_index(&mut model);
    }

    /// Run one addon's saved-variables files at the startup walk's position, as `LoadAddOn` does.
    pub fn load_addon_saved_variables(&mut self, name: &str) {
        let i = self
            .model_ref()
            .addons
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(name));
        if let Some(i) = i {
            load_saved_variables(self.lua(), i);
        }
    }

    /// `(name, account names, per-character names)` to write at shutdown, for each loaded addon
    /// that declares any: the reference gates the write on the loaded byte (`0x51f711`).
    pub fn addon_saved_variable_sets(&self) -> Vec<(String, Vec<String>, Vec<String>)> {
        self.model_ref()
            .addons
            .iter()
            .filter(|a| a.loaded)
            .filter(|a| {
                !a.saved_variables.is_empty() || !a.saved_variables_per_character.is_empty()
            })
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

    /// `(name, enabled)` per registered addon, in order: what the host writes to `AddOns.txt`.
    pub fn addon_enable_states(&self) -> Vec<(String, bool)> {
        self.model_ref()
            .addons
            .iter()
            .map(|a| (a.name.clone(), a.enabled))
            .collect()
    }
}
