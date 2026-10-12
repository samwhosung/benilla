//! The glue AddOns verbs (registrar `0x46ddf0`, table `0x8373b8`, `0x46d420`-`0x46da50`): the
//! screen's twins of the in-game AddOn API ([`super::addon`]), over the same registry, the same
//! Lua index space (`0x51def0`, `0x51df00`) and the same load law ([`super::addon_gate`]).
//!
//! What differs is the character. The glue has none logged in, so its enable state is per
//! character by name: the `ADDONSTATELIST` ([`EnableStore`]), a node per character on the realm's
//! list, loaded as the roster is (`0x4724d0` drops every node with `0x51f0b0(NULL)`, then the
//! appender `0x472300` loads each character's `AddOns.txt` through `0x51ebe0`). `nil` names every
//! node. `GetAddOnInfo` asks the load law for no character at all (`0x46d526`), so a row is
//! enabled when the query over every node says 1 or 2 (`0x51e829`).

use std::rc::Rc;

use mlua::{Lua, MultiValue, Value};

use super::addon::{index_row, AddonSecurity};
use super::addon_enable::{AddonStateFiles, EnableStore};
use super::addon_gate::{can_load, GateRow};
use super::binding_abi::{bool_or_default, flag, number_arg, s_str_to_int, string_arg};
use super::glue_core::{queue, GlueRequest};
use super::Model;

/// `GetNumAddOns` is `0x46d420`; its in-game namesake is `0x48e350` and answers the same count.
const USAGE_INFO: &str = "Usage: GetAddOnInfo(index)"; // 0x837d9c
const USAGE_LAUNCH: &str = "Usage: LaunchAddOnURL(index)"; // 0x837db8
const USAGE_DEPENDENCIES: &str = "Usage: GetAddOnDependencies(index)"; // 0x837dd8
const USAGE_ENABLE_STATE: &str = "Usage: GetAddOnEnableState(\"character\", index)"; // 0x837dfc
const USAGE_ENABLE: &str = "Usage: EnableAddOn(\"character\", index)"; // 0x837e2c
const USAGE_DISABLE: &str = "Usage: DisableAddOn(\"character\", index)"; // 0x837e54
const USAGE_SCRIPT_MEMORY: &str = "Usage: SetScriptMemory(MB)"; // 0x837e7c

/// The CVars the verbs read and write through the reference's records: `checkAddonVersion`
/// (`0x882630`) and `scriptMemory` (`0x882654`, KB).
const CVAR_VERSION_CHECK: &str = "checkAddonVersion";
const CVAR_SCRIPT_MEMORY: &str = "scriptMemory";

/// The glue's AddOn state: the enable nodes and the files they come from and go to.
#[derive(Default)]
pub(crate) struct GlueAddons {
    /// `ADDONSTATELIST` (`0xbe1bd0`).
    store: EnableStore,
    /// The host's `AddOns.txt` reader and writer; without one a node reads empty and a save
    /// writes nothing.
    files: Option<Rc<dyn AddonStateFiles>>,
}

/// A file seam that has no files, for a VM whose host seated none.
struct NoFiles;

impl AddonStateFiles for NoFiles {
    fn read(&self, _: &str, _: &str) -> Option<Vec<(String, bool)>> {
        None
    }
    fn write(&self, _: &str, _: &str, _: &[(String, bool)]) {}
}

impl GlueAddons {
    fn files(&self) -> Rc<dyn AddonStateFiles> {
        self.files.clone().unwrap_or_else(|| Rc::new(NoFiles))
    }

    /// `0x51f0b0(NULL)` and the appender `0x472300` per record: every node dropped, then a node
    /// per character in list order, each loaded from its file (`0x51ebe0`).
    pub(super) fn load(&mut self, realm: &str, characters: &[String]) {
        self.store = EnableStore::load(realm, characters, &*self.files());
    }

    /// The rename's carry (`0x4725f0` at `0x4726ff`, `0x472706`): the old name's node dropped
    /// (`0x51f0b0`), then the new name's read from its file, which the host has moved by now.
    pub(super) fn rename(&mut self, old: &str, new: &str) {
        let files = self.files();
        self.store.drop_node(old);
        self.store.load_node(new, &*files);
    }
}

impl super::UiScript {
    /// Seat the reader and writer of the `AddOns.txt` files the glue's nodes come from and go to.
    pub fn set_addon_state_files(&mut self, files: Box<dyn AddonStateFiles>) {
        self.model_mut().glue.addons.files = Some(Rc::from(files));
    }
}

/// The registry as the load law reads it from the glue (`0x46d542`): each row enabled when the
/// query for no character, over every node, answers nonzero (`0x51e824`), nothing loaded.
fn gate_rows<'a>(model: &'a Model, store: &EnableStore) -> Vec<GateRow<'a>> {
    model
        .addons
        .iter()
        .map(|a| GateRow {
            name: &a.name,
            enabled: store.enabled_for(&a.name, a.default_state, None),
            interface: a.interface,
            load_on_demand: a.load_on_demand,
            banned: a.security == AddonSecurity::Banned,
            corrupt: a.corrupt,
            loaded: a.loaded,
            dependencies: a.dependencies.iter().map(String::as_str).collect(),
        })
        .collect()
}

fn model(lua: &Lua) -> mlua::AppDataRefMut<'_, Model> {
    lua.app_data_mut::<Model>().expect("model app_data")
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

fn arg(args: &MultiValue, i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

/// The index every one-index verb reads: a number or numeric string (`0x6f34d0`) or the verb's
/// `Usage:`, then the registry row of that place in the Lua index space, or `0x837d70`'s range
/// error.
fn row_arg(lua: &Lua, model: &Model, v: Value, usage: &'static str) -> mlua::Result<usize> {
    let n = lua
        .coerce_number(v)?
        .ok_or_else(|| mlua::Error::RuntimeError(usage.into()))?;
    index_row(model, n)
}

/// The `(character, index)` pair of the three per-character verbs: argument 1 is a string or
/// number (`0x6f3510`) or an explicit nil (`0x6f3400` answers tag 0), which names every node;
/// an absent argument is the usage error. Argument 2 is a number. The character is read before
/// the index is resolved (`0x46d6fd`-`0x46d75e`).
fn character_and_row(
    lua: &Lua,
    args: &MultiValue,
    usage: &'static str,
) -> mlua::Result<(Option<String>, usize)> {
    let character = match args.front() {
        Some(Value::Nil) => None,
        Some(v) => Some(string_arg(lua, v.clone(), usage)?),
        None => return Err(mlua::Error::RuntimeError(usage.into())),
    };
    let n = lua
        .coerce_number(arg(args, 1))?
        .ok_or_else(|| mlua::Error::RuntimeError(usage.into()))?;
    let row = index_row(&model(lua), n)?;
    Ok((character, row))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x46d420`: `0x51def0`, the sorted, hidden-filtered array's count, pushed as a number.
    g.set(
        "GetNumAddOns",
        lua.create_function(|lua, _: MultiValue| Ok(model(lua).addon_index.len() as f64))?,
    )?;

    // `0x46d460`: one number or the usage error, the row or the range error, then eight values on
    // every returning path: name, title, notes, url, loadable, reason, security, newVersion. The
    // load law runs with `dl = 0` for no character (`0x46d526`-`0x46d542`).
    g.set(
        "GetAddOnInfo",
        lua.create_function(|lua, args: MultiValue| {
            let model = model(lua);
            let i = row_arg(lua, &model, arg(&args, 0), USAGE_INFO)?;
            let rows = gate_rows(&model, &model.glue.addons.store);
            let verdict = can_load(&rows, i, false, super::addon::version_check(&model));
            let a = &model.addons[i];
            Ok(MultiValue::from_vec(vec![
                lua_str(lua, &a.name)?,
                opt_str(lua, &a.title)?,
                opt_str(lua, &a.notes)?,
                // `0x51e180`: the server's url, written only by `SMSG_ADDON_INFO` (`0x51db6a`); nil
                // when empty or for a banned addon (`0x51e203`-`0x51e20e`).
                a.url
                    .as_ref()
                    .filter(|u| !u.is_empty() && a.security != AddonSecurity::Banned)
                    .map_or(Ok(Value::Nil), |u| lua_str(lua, u))?,
                flag(verdict.loadable()),
                match verdict.token() {
                    Some(reason) => lua_str(lua, &reason)?,
                    None => Value::Nil,
                },
                lua_str(lua, a.security.token())?,
                // `0x51e3e0`: `[rec+0xc0]`, set when the server's revision is past the installed one
                // (`0x51db0d`).
                flag(a.new_version),
            ]))
        })?,
    )?;

    // `0x46d5e0`: the index (`Usage: LaunchAddOnURL(index)`, `0x837db8`) and the range error, then
    // `0x51e180`'s url, nil when empty or banned, straight into the launcher `0x429ff0`: unlike
    // `LaunchURL` (`0x5abd00`) it asks no whitelist (`0x5abc10`). Nothing returned.
    g.set(
        "LaunchAddOnURL",
        lua.create_function(|lua, args: MultiValue| {
            let url = {
                let model = model(lua);
                let i = row_arg(lua, &model, arg(&args, 0), USAGE_LAUNCH)?;
                let a = &model.addons[i];
                a.url
                    .clone()
                    .filter(|u| !u.is_empty() && a.security != AddonSecurity::Banned)
            };
            if let Some(url) = url.as_deref().and_then(super::glue_core::launch_addon_url) {
                queue(lua, GlueRequest::LaunchAddonUrl(url));
            }
            Ok(())
        })?,
    )?;

    // `0x46d650`: the index only; one return per dependency (`0x51e350`'s array), none for none.
    g.set(
        "GetAddOnDependencies",
        lua.create_function(|lua, args: MultiValue| {
            let model = model(lua);
            let i = row_arg(lua, &model, arg(&args, 0), USAGE_DEPENDENCIES)?;
            let mut out = Vec::new();
            for dep in &model.addons[i].dependencies {
                out.push(lua_str(lua, dep)?);
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // `0x46d6f0`: `0x51e470(addon, character, useDefault = 1)` as a number, 0, 1 or 2.
    g.set(
        "GetAddOnEnableState",
        lua.create_function(|lua, args: MultiValue| {
            let (character, i) = character_and_row(lua, &args, USAGE_ENABLE_STATE)?;
            let model = model(lua);
            let a = &model.addons[i];
            Ok(f64::from(model.glue.addons.store.state(
                &a.name,
                a.default_state,
                character.as_deref(),
            )))
        })?,
    )?;

    // `0x46d7b0`, `0x46d8a0`: the setter `0x51ea20` on the named character, or on every node for
    // nil. Nothing returned.
    for (name, on, usage) in [
        ("EnableAddOn", true, USAGE_ENABLE),
        ("DisableAddOn", false, USAGE_DISABLE),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, args: MultiValue| {
                let (character, i) = character_and_row(lua, &args, usage)?;
                let mut model = model(lua);
                let addon = model.addons[i].name.clone();
                model
                    .glue
                    .addons
                    .store
                    .set(&addon, character.as_deref(), on);
                Ok(())
            })?,
        )?;
    }

    // `0x46d850`, `0x46d940`: the first argument is read with no test (`0x6f3690`), so anything
    // but a string or number names every node; then the setter over the whole index array, its
    // bound re-read each pass.
    for (name, on) in [("EnableAllAddOns", true), ("DisableAllAddOns", false)] {
        g.set(
            name,
            lua.create_function(move |lua, args: MultiValue| {
                let character = lua
                    .coerce_string(arg(&args, 0))?
                    .map(|s| s.to_string_lossy());
                let mut model = model(lua);
                for k in 0..model.addon_index.len() {
                    let addon = model.addons[model.addon_index[k]].name.clone();
                    model
                        .glue
                        .addons
                        .store
                        .set(&addon, character.as_deref(), on);
                }
                Ok(())
            })?,
        )?;
    }

    // `0x46d990`: the writer `0x51ef20`, every dirty node.
    g.set(
        "SaveAddOns",
        lua.create_function(|lua, _: MultiValue| {
            let mut model = model(lua);
            let files = model.glue.addons.files();
            model.glue.addons.store.save(&*files);
            Ok(())
        })?,
    )?;

    // `0x46d9a0`: `0x51ebe0(NULL)`, every cached node reloaded from its file.
    g.set(
        "ResetAddOns",
        lua.create_function(|lua, _: MultiValue| {
            let mut model = model(lua);
            let files = model.glue.addons.files();
            model.glue.addons.store.reload(&*files);
            Ok(())
        })?,
    )?;

    // `0x46d9b0`: `0x51f180`, the CVar's integer nonzero, as the number 1 or nil.
    g.set(
        "IsAddonVersionCheckEnabled",
        lua.create_function(|lua, _: MultiValue| {
            Ok(flag(super::addon::version_check(&model(lua))))
        })?,
    )?;

    // `0x46d9e0`: the flag through `GetBoolOrDefault` (`0x6f1c10`, default 1) into `0x51f1a0`,
    // which sets the CVar to "1" or "0".
    g.set(
        "SetAddonVersionCheck",
        lua.create_function(|lua, args: MultiValue| {
            let on = bool_or_default(args.front(), true);
            super::cvars::set_from_engine(
                &mut model(lua),
                CVAR_VERSION_CHECK,
                if on { "1" } else { "0" }.to_string(),
            );
            Ok(())
        })?,
    )?;

    // `0x46da10`: the `scriptMemory` CVar's integer in KB, `>> 10` with the sign bias (a division
    // truncating toward zero), pushed as a number.
    g.set(
        "GetScriptMemory",
        lua.create_function(|lua, _: MultiValue| {
            let kb = model(lua)
                .cvars
                .get(&CVAR_SCRIPT_MEMORY.to_ascii_lowercase())
                .map_or(0, |slot| s_str_to_int(&slot.value) as i32);
            Ok(f64::from(kb.wrapping_add((kb >> 31) & 0x3ff) >> 10))
        })?,
    )?;

    // `0x46da50`: one number or the usage error, `_ftol << 10`, printed `%d` into the CVar.
    g.set(
        "SetScriptMemory",
        lua.create_function(|lua, args: MultiValue| {
            let mb = number_arg(lua, arg(&args, 0), USAGE_SCRIPT_MEMORY)?;
            super::cvars::set_from_engine(
                &mut model(lua),
                CVAR_SCRIPT_MEMORY,
                (mb << 10).to_string(),
            );
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::super::glue_select::GlueCharacter;
    use super::super::{AddOnInfo, AddonReply, UiScript};
    use super::*;

    type Rows = Vec<(String, bool)>;

    /// `AddOns.txt` files in memory, shared with the test so it can read what a save wrote.
    #[derive(Clone, Default)]
    struct Files(Rc<RefCell<HashMap<(String, String), Rows>>>);

    impl Files {
        fn put(&self, character: &str, rows: &[(&str, bool)]) {
            self.0.borrow_mut().insert(
                ("Realm".into(), character.into()),
                rows.iter().map(|(n, v)| ((*n).into(), *v)).collect(),
            );
        }

        fn get(&self, character: &str) -> Option<Vec<(String, bool)>> {
            self.0
                .borrow()
                .get(&("Realm".to_string(), character.to_string()))
                .cloned()
        }
    }

    impl AddonStateFiles for Files {
        fn read(&self, realm: &str, character: &str) -> Option<Vec<(String, bool)>> {
            self.0
                .borrow()
                .get(&(realm.to_string(), character.to_string()))
                .cloned()
        }
        fn write(&self, realm: &str, character: &str, rows: &[(String, bool)]) {
            self.0
                .borrow_mut()
                .insert((realm.into(), character.into()), rows.to_vec());
        }
    }

    fn addon(name: &str, title: Option<&str>) -> AddOnInfo {
        AddOnInfo {
            name: name.into(),
            title: title.map(Into::into),
            notes: Some(format!("{name} notes")),
            interface: 11200,
            default_state: true,
            enabled: true,
            ..AddOnInfo::default()
        }
    }

    fn record(name: &str, status: u8) -> AddonReply {
        AddonReply {
            name: name.into(),
            status,
            ..AddonReply::default()
        }
    }

    fn character(guid: u64, name: &str) -> GlueCharacter {
        GlueCharacter {
            guid,
            name: name.into(),
            race: String::new(),
            class: String::new(),
            level: 1,
            zone: None,
            file_string: String::new(),
            gender: 0,
            flags: 0,
            model: false,
        }
    }

    /// Index space, by title: Alpha, Beta (needs Alpha), Gamma (out of date), Zeta (needs a
    /// missing one); `Hidden` is hidden by the reply. Characters Ann and Bob.
    fn vm(files: &Files) -> UiScript {
        let mut s = UiScript::new_glue().unwrap();
        s.register_cvars([
            ("lastCharacterIndex", "0"),
            ("checkAddonVersion", "1"),
            ("scriptMemory", "49152"),
        ]);
        let mut beta = addon("Beta", Some("Beta"));
        beta.dependencies = vec!["Alpha".into()];
        let mut gamma = addon("Gamma", None);
        gamma.interface = 10100;
        let mut zeta = addon("Zeta", Some("Zeta"));
        zeta.dependencies = vec!["Nowhere".into()];
        s.register_addons(
            vec![
                zeta,
                addon("Hidden", None),
                gamma,
                beta,
                addon("Alpha", Some("Alpha")),
            ],
            None,
            None,
            None,
        );
        s.set_addon_state_files(Box::new(files.clone()));
        s.register_cvars([("realmName", "Realm")]);
        let _ = s.load_glue_roster(vec![character(1, "Ann"), character(2, "Bob")]);
        s
    }

    fn after_reply(s: &mut UiScript) {
        s.note_addon_info_reply(&["Hidden".to_string()]);
    }

    fn count(call: &str) -> String {
        format!("local function n(...) return arg.n end return n({call})")
    }

    fn raised(s: &UiScript, chunk: &str) -> String {
        s.run(chunk).unwrap_err().to_string()
    }

    /// The array is empty until the reply lands (`[0xbe1b90]`), and the reply's hidden addon is
    /// not in it: the count is a number.
    #[test]
    fn the_count_is_the_index_space_and_empty_until_the_reply() {
        let files = Files::default();
        let mut s = vm(&files);
        assert_eq!(s.eval::<f64>("return GetNumAddOns()").unwrap(), 0.0);
        after_reply(&mut s);
        assert_eq!(s.eval::<f64>("return GetNumAddOns()").unwrap(), 4.0);
        assert_eq!(
            s.eval::<String>("return type(GetNumAddOns())").unwrap(),
            "number"
        );
        let names = s
            .eval::<Vec<String>>(
                "local t = {} for i = 1, GetNumAddOns() do t[i] = (GetAddOnInfo(i)) end return t",
            )
            .unwrap();
        assert_eq!(
            names,
            ["Alpha", "Beta", "Gamma", "Zeta"],
            "sorted by title or name"
        );
    }

    /// Eight values on every returning path (`0x46d5c2`), url fourth and nil, loadable the number
    /// 1 and reason nil for a loadable addon, security a string, newVersion nil.
    #[test]
    fn get_addon_info_answers_eight_values() {
        let files = Files::default();
        let mut s = vm(&files);
        after_reply(&mut s);
        assert_eq!(s.eval::<f64>(&count("GetAddOnInfo(1)")).unwrap(), 8.0);
        assert_eq!(s.eval::<f64>(&count("GetAddOnInfo(3)")).unwrap(), 8.0);
        let kinds = s
            .eval::<Vec<String>>(
                "local a, b, c, d, e, f, g, h = GetAddOnInfo(1) \
                 return {type(a), type(b), type(c), type(d), type(e), type(f), type(g), type(h)}",
            )
            .unwrap();
        assert_eq!(
            kinds,
            ["string", "string", "string", "nil", "number", "nil", "string", "nil"]
        );
        assert_eq!(
            s.eval::<(String, String, f64, String)>(
                "local name, title, notes, url, loadable, reason, security = GetAddOnInfo(1) \
                 return name, notes, loadable, security"
            )
            .unwrap(),
            ("Alpha".into(), "Alpha notes".into(), 1.0, "INSECURE".into())
        );
    }

    /// The load law with `dl = 0` and no character: out of date, dependency and disabled
    /// verdicts, each reason spliced into `ADDON_<reason>`; force-load clears the version one.
    #[test]
    fn the_load_verdict_has_the_glues_reasons() {
        let files = Files::default();
        let mut s = vm(&files);
        after_reply(&mut s);
        let info = |s: &UiScript, i: u32| {
            s.eval::<(Option<f64>, Option<String>)>(&format!(
                "local _, _, _, _, loadable, reason = GetAddOnInfo({i}) return loadable, reason"
            ))
            .unwrap()
        };
        assert_eq!(info(&s, 1), (Some(1.0), None));
        assert_eq!(info(&s, 3), (None, Some("INTERFACE_VERSION".into())));
        assert_eq!(info(&s, 4), (None, Some("DEP_MISSING".into())));
        // Beta needs Alpha: Alpha disabled for every character is Alpha DISABLED, Beta DEP_DISABLED.
        s.run("DisableAddOn(nil, 1)").unwrap();
        assert_eq!(info(&s, 1), (None, Some("DISABLED".into())));
        assert_eq!(info(&s, 2), (None, Some("DEP_DISABLED".into())));
        // One character on is enough to be loadable.
        s.run("EnableAddOn('Bob', 1)").unwrap();
        assert_eq!(info(&s, 1), (Some(1.0), None));
        // Load out of date AddOns: the version reason is reset (`0x51e89d`).
        s.run("SetAddonVersionCheck(0)").unwrap();
        assert_eq!(info(&s, 3), (Some(1.0), None));
    }

    /// The index argument: a number or numeric string, else the verb's usage; off the array the
    /// range error with the count (`0x837d70`).
    #[test]
    fn the_index_argument_is_checked_as_the_reference_does() {
        let files = Files::default();
        let mut s = vm(&files);
        after_reply(&mut s);
        assert_eq!(
            s.eval::<String>("return (GetAddOnInfo('2'))").unwrap(),
            "Beta"
        );
        for call in [
            "GetAddOnInfo()",
            "GetAddOnInfo('Alpha')",
            "GetAddOnInfo(nil)",
        ] {
            assert!(
                raised(&s, call).contains("Usage: GetAddOnInfo(index)"),
                "{call}"
            );
        }
        for call in ["GetAddOnInfo(0)", "GetAddOnInfo(5)", "GetAddOnInfo(-1)"] {
            assert!(
                raised(&s, call).contains("AddOn index must be in the range of 1 to 4"),
                "{call}"
            );
        }
        assert!(
            raised(&s, "GetAddOnDependencies('x')").contains("Usage: GetAddOnDependencies(index)")
        );
        assert!(raised(&s, "GetAddOnDependencies(9)").contains("range of 1 to 4"));
    }

    /// One return per dependency, none for none (`0x46d6e5`).
    #[test]
    fn dependencies_are_one_return_each() {
        let files = Files::default();
        let mut s = vm(&files);
        after_reply(&mut s);
        assert_eq!(
            s.eval::<f64>(&count("GetAddOnDependencies(1)")).unwrap(),
            0.0
        );
        assert_eq!(
            s.eval::<String>("return GetAddOnDependencies(2)").unwrap(),
            "Alpha"
        );
        assert_eq!(
            s.eval::<f64>(&count("GetAddOnDependencies(2)")).unwrap(),
            1.0
        );
    }

    /// `GetAddOnEnableState(nil, i)` counts every node: 2 when all say on, 0 when all say off, 1
    /// between; a name counts its one node. A row nobody set takes `## DefaultState`.
    #[test]
    fn the_enable_state_is_the_tri_state_over_the_characters() {
        let files = Files::default();
        files.put("Ann", &[("Alpha", true), ("Beta", false)]);
        files.put("Bob", &[("Alpha", true), ("Beta", true)]);
        let mut s = vm(&files);
        after_reply(&mut s);
        let state = |s: &UiScript, who: &str, i: u32| {
            s.eval::<f64>(&format!("return GetAddOnEnableState({who}, {i})"))
                .unwrap()
        };
        assert_eq!(state(&s, "nil", 1), 2.0, "Alpha is on for both");
        assert_eq!(state(&s, "nil", 2), 1.0, "Beta is on for Bob only");
        assert_eq!(state(&s, "'Ann'", 2), 0.0);
        assert_eq!(state(&s, "'bob'", 2), 2.0, "a name compares without case");
        assert_eq!(
            state(&s, "nil", 3),
            2.0,
            "nobody set Gamma: its DefaultState, on"
        );
        s.run("DisableAddOn(nil, 1)").unwrap();
        assert_eq!(state(&s, "nil", 1), 0.0);
        assert_eq!(
            s.eval::<String>("return type(GetAddOnEnableState(nil, 1))")
                .unwrap(),
            "number"
        );
        // An explicit nil passes, an omitted character does not (`0x6f3400` answers -1).
        assert!(raised(&s, "GetAddOnEnableState()").contains("Usage: GetAddOnEnableState"));
        assert!(raised(&s, "GetAddOnEnableState(1)").contains("Usage: GetAddOnEnableState"));
        assert!(raised(&s, "GetAddOnEnableState({}, 1)").contains("Usage: GetAddOnEnableState"));
        assert!(raised(&s, "GetAddOnEnableState(nil, 'x')").contains("Usage: GetAddOnEnableState"));
        assert!(raised(&s, "GetAddOnEnableState(nil, 9)").contains("range of 1 to 4"));
    }

    /// The setters write the named character's node, every node for nil, and nothing for a name
    /// no node has; they return nothing and raise their own usage.
    #[test]
    fn the_setters_write_the_named_node_or_all_of_them() {
        let files = Files::default();
        let mut s = vm(&files);
        after_reply(&mut s);
        assert_eq!(s.eval::<f64>(&count("EnableAddOn('Ann', 1)")).unwrap(), 0.0);
        assert_eq!(
            s.eval::<f64>(&count("DisableAddOn('Ann', 1)")).unwrap(),
            0.0
        );
        let state = |s: &UiScript, who: &str| {
            s.eval::<f64>(&format!("return GetAddOnEnableState({who}, 1)"))
                .unwrap()
        };
        assert_eq!(state(&s, "'Ann'"), 0.0);
        assert_eq!(
            state(&s, "'Bob'"),
            0.0,
            "Bob has no row: Ann's off is the consensus"
        );
        s.run("EnableAddOn('Ann', 1)").unwrap();
        assert_eq!(state(&s, "nil"), 2.0);
        s.run("DisableAddOn('Nobody', 1)").unwrap();
        assert_eq!(state(&s, "nil"), 2.0, "no node, no effect");
        assert!(raised(&s, "EnableAddOn(1)").contains("Usage: EnableAddOn(\"character\", index)"));
        assert!(
            raised(&s, "DisableAddOn('Ann')").contains("Usage: DisableAddOn(\"character\", index)")
        );
        assert!(raised(&s, "EnableAddOn('Ann', 7)").contains("range of 1 to 4"));
    }

    /// The all-verbs walk the index array, so a hidden addon is untouched; nil, and anything that
    /// is not a string or number, names every node.
    #[test]
    fn the_all_verbs_walk_the_index_space() {
        let files = Files::default();
        let mut s = vm(&files);
        after_reply(&mut s);
        s.run("DisableAllAddOns('Ann')").unwrap();
        assert_eq!(
            s.eval::<Vec<f64>>(
                "local t = {} for i = 1, 4 do t[i] = GetAddOnEnableState('Ann', i) end return t"
            )
            .unwrap(),
            [0.0; 4]
        );
        s.run("EnableAllAddOns()").unwrap();
        assert_eq!(
            s.eval::<Vec<f64>>(
                "local t = {} for i = 1, 4 do t[i] = GetAddOnEnableState(nil, i) end return t"
            )
            .unwrap(),
            [2.0; 4]
        );
        s.run("DisableAllAddOns(true)").unwrap();
        assert_eq!(
            s.eval::<f64>("return GetAddOnEnableState(nil, 1)").unwrap(),
            0.0
        );
        // The hidden addon is outside the index array, so no all-verb reached it.
        assert_eq!(
            s.model_ref()
                .glue
                .addons
                .store
                .node("Ann")
                .and_then(|h| h.get("Hidden")),
            None
        );
    }

    /// `SaveAddOns` writes each dirty node whole, a clean one not at all, and `ResetAddOns` reads
    /// every file again: the toggles since are gone.
    #[test]
    fn save_writes_the_dirty_nodes_and_reset_reverts_the_rest() {
        let files = Files::default();
        files.put("Ann", &[("Alpha", true), ("Gone", false)]);
        files.put("Bob", &[("Alpha", true)]);
        let mut s = vm(&files);
        after_reply(&mut s);
        s.run("DisableAddOn('Ann', 1)").unwrap();
        s.run("ResetAddOns()").unwrap();
        assert_eq!(
            s.eval::<f64>("return GetAddOnEnableState('Ann', 1)")
                .unwrap(),
            2.0,
            "reset reads the files again"
        );
        s.run("DisableAddOn('Ann', 1)").unwrap();
        s.run("SaveAddOns()").unwrap();
        assert_eq!(
            files.get("Ann"),
            Some(vec![("Alpha".into(), false), ("Gone".into(), false)]),
            "the whole node, an uninstalled addon's row kept"
        );
        assert_eq!(
            files.get("Bob"),
            Some(vec![("Alpha".into(), true)]),
            "a node nothing changed is not rewritten"
        );
        s.run("ResetAddOns()").unwrap();
        assert_eq!(
            s.eval::<f64>("return GetAddOnEnableState('Ann', 1)")
                .unwrap(),
            0.0,
            "what was saved is what a reset returns to"
        );
    }

    /// The rename's carry (`0x4726ff`, `0x472706`): the old name's node goes, unsaved toggles with
    /// it, and the new name's comes from its file, which the host moved there first.
    #[test]
    fn a_rename_carries_the_node_to_the_new_name() {
        let files = Files::default();
        files.put("Bob", &[("Alpha", false)]);
        let mut s = vm(&files);
        after_reply(&mut s);
        let mut bob = character(2, "Bob");
        bob.flags = 0x4000;
        let _ = s.load_glue_roster(vec![character(1, "Ann"), bob]);
        let state = |s: &UiScript, who: &str| {
            s.eval::<f64>(&format!("return GetAddOnEnableState('{who}', 1)"))
                .unwrap()
        };
        assert_eq!(state(&s, "Bob"), 0.0);
        // A toggle left unsaved goes with the old node.
        s.run("EnableAddOn('Bob', 1)").unwrap();
        // The host's move, then the applier.
        files.0.borrow_mut().remove(&("Realm".into(), "Bob".into()));
        files.put("Newname", &[("Alpha", false)]);
        s.apply_glue_rename(2, "Newname");
        assert_eq!(state(&s, "Newname"), 0.0, "the moved file's state");
        assert_eq!(state(&s, "Bob"), 2.0, "no node: the default");
        // The new node saves under the new name.
        s.run("EnableAddOn('Newname', 1) SaveAddOns()").unwrap();
        assert_eq!(files.get("Newname"), Some(vec![("Alpha".into(), true)]));
        assert_eq!(files.get("Bob"), None);
    }

    /// The roster load drops every node and loads the new list (`0x51f0b0(NULL)`, `0x472300`).
    #[test]
    fn a_roster_load_rebuilds_the_nodes() {
        let files = Files::default();
        files.put("Cat", &[("Alpha", false)]);
        let mut s = vm(&files);
        after_reply(&mut s);
        assert_eq!(
            s.eval::<f64>("return GetAddOnEnableState('Cat', 1)")
                .unwrap(),
            2.0,
            "no node yet: the default"
        );
        let _ = s.load_glue_roster(vec![character(3, "Cat")]);
        assert_eq!(
            s.eval::<f64>("return GetAddOnEnableState('Cat', 1)")
                .unwrap(),
            0.0
        );
        assert_eq!(
            s.eval::<f64>("return GetAddOnEnableState('Ann', 1)")
                .unwrap(),
            2.0,
            "Ann's node is gone"
        );
    }

    /// `IsAddonVersionCheckEnabled` answers the number 1 or nil; the setter takes a flag with
    /// default 1 (`0x6f1c10`).
    #[test]
    fn the_version_check_is_a_one_or_nil_flag() {
        let files = Files::default();
        let s = vm(&files);
        let check = |s: &UiScript| {
            s.eval::<Option<f64>>("return IsAddonVersionCheckEnabled()")
                .unwrap()
        };
        assert_eq!(check(&s), Some(1.0));
        s.run("SetAddonVersionCheck(0)").unwrap();
        assert_eq!(check(&s), None);
        s.run("SetAddonVersionCheck()").unwrap();
        assert_eq!(check(&s), Some(1.0), "a bare call enables");
        s.run("SetAddonVersionCheck('0')").unwrap();
        assert_eq!(check(&s), None);
        s.run("SetAddonVersionCheck(1)").unwrap();
        assert_eq!(check(&s), Some(1.0));
        assert_eq!(
            s.eval::<f64>(&count("SetAddonVersionCheck(0)")).unwrap(),
            0.0
        );
    }

    /// The script memory is the CVar in KB, shown in MB: `>> 10` toward zero, and `<< 10` back.
    #[test]
    fn the_script_memory_is_the_cvar_in_kilobytes() {
        let files = Files::default();
        let mut s = vm(&files);
        assert_eq!(s.eval::<f64>("return GetScriptMemory()").unwrap(), 48.0);
        s.run("SetScriptMemory(64)").unwrap();
        assert_eq!(s.cvar("scriptMemory").as_deref(), Some("65536"));
        assert_eq!(s.eval::<f64>("return GetScriptMemory()").unwrap(), 64.0);
        s.run("SetScriptMemory('32.9')").unwrap();
        assert_eq!(s.cvar("scriptMemory").as_deref(), Some("32768"));
        s.set_cvar_engine("scriptMemory", "-1");
        assert_eq!(s.eval::<f64>("return GetScriptMemory()").unwrap(), 0.0);
        s.set_cvar_engine("scriptMemory", "-2048");
        assert_eq!(s.eval::<f64>("return GetScriptMemory()").unwrap(), -2.0);
        // The CVar's integer is `SStrToInt` of its text (`+0x28`): no whitespace skip, no plus.
        for (text, mb) in [
            ("12288x", 12.0),
            (" 4096", 0.0),
            ("+4096", 0.0),
            ("-3072", -3.0),
        ] {
            s.set_cvar_engine("scriptMemory", text);
            assert_eq!(
                s.eval::<f64>("return GetScriptMemory()").unwrap(),
                mb,
                "{text:?}"
            );
        }
        assert!(raised(&s, "SetScriptMemory('')").contains("Usage: SetScriptMemory(MB)"));
        assert!(raised(&s, "SetScriptMemory()").contains("Usage: SetScriptMemory(MB)"));
    }

    /// The reply's records land on the rows they name (`0x51da70`): the url and the new-version
    /// byte the glue returns fourth and eighth, the security string seventh, and BANNED refuses
    /// the addon and, as `DEP_BANNED`, whatever depends on it. A banned addon's url is nil.
    #[test]
    fn the_replys_records_reach_the_glue_verbs() {
        let files = Files::default();
        let mut s = vm(&files);
        // Alpha is banned with a url; Gamma is listed with a newer revision and a url.
        s.note_addon_info_records(vec![
            AddonReply {
                name: "Alpha".into(),
                status: 0,
                url: Some("http://example.com/alpha".into()),
                ..AddonReply::default()
            },
            AddonReply {
                name: "Gamma".into(),
                status: 1,
                revision: Some(5),
                url: Some("http://example.com/gamma".into()),
            },
            record("Hidden", 2),
            // A revision equal to the installed one (0 here) is not a new version (`jbe`, 0x51db0b).
            AddonReply {
                name: "Zeta".into(),
                status: 1,
                revision: Some(0),
                ..AddonReply::default()
            },
        ]);
        type Info = (
            Option<String>,
            Option<f64>,
            Option<String>,
            String,
            Option<f64>,
        );
        let info = |s: &UiScript, i: u32| -> Info {
            s.eval::<Info>(&format!(
                "local _, _, _, url, loadable, reason, security, new = GetAddOnInfo({i}) \
                 return url, loadable, reason, security, new"
            ))
            .unwrap()
        };
        assert_eq!(s.eval::<f64>("return GetNumAddOns()").unwrap(), 4.0);
        assert_eq!(
            info(&s, 1),
            (None, None, Some("BANNED".into()), "BANNED".into(), None),
            "banned: refused, security BANNED, and no url (0x51e208)"
        );
        assert_eq!(
            info(&s, 2),
            (
                None,
                None,
                Some("DEP_BANNED".into()),
                "INSECURE".into(),
                None
            ),
            "Beta needs the banned Alpha"
        );
        assert_eq!(
            info(&s, 3),
            (
                Some("http://example.com/gamma".into()),
                None,
                Some("INTERFACE_VERSION".into()),
                "INSECURE".into(),
                Some(1.0)
            ),
            "a revision past the installed 0 is a new version; the url is the server's"
        );
        assert_eq!(s.eval::<f64>(&count("GetAddOnInfo(3)")).unwrap(), 8.0);
        // Not hidden any more by status 1; the one named with 2 is.
        assert_eq!(
            s.eval::<String>("return (GetAddOnInfo(4))").unwrap(),
            "Zeta"
        );
        assert_eq!(info(&s, 4).4, None, "the same revision as installed");
    }

    /// Status 0 bans without asking the signature; any other status runs it, and the model's
    /// answer 3 is SECURE, hidden or not. A row the reply never names stays INSECURE.
    #[test]
    fn the_signature_answer_makes_secure_and_status_zero_bans() {
        let files = Files::default();
        let mut s = vm(&files);
        s.model_mut().addons.iter_mut().for_each(|a| {
            if ["Alpha", "Beta", "Gamma"].contains(&a.name.as_str()) {
                a.signature_ok = true;
            }
        });
        s.note_addon_info_records(vec![
            record("Alpha", 2),
            record("Beta", 1),
            record("Gamma", 0),
        ]);
        let security = |s: &UiScript, i: u32| {
            s.eval::<String>(&format!(
                "local _, _, _, _, _, _, sec = GetAddOnInfo({i}) return sec"
            ))
            .unwrap()
        };
        // The index is Beta, Gamma, Hidden, Zeta: Alpha is hidden.
        assert_eq!(s.eval::<f64>("return GetNumAddOns()").unwrap(), 4.0);
        assert_eq!(security(&s, 1), "SECURE");
        assert_eq!(security(&s, 2), "BANNED", "status 0 asks no signature");
        assert_eq!(security(&s, 4), "INSECURE", "never named");
        assert!(s.addon_is_secure("Alpha"), "status 2 still runs the check");
        assert!(!s.addon_is_secure("Gamma") && !s.addon_is_secure("Nowhere"));
    }

    /// `0x51f180` tests the CVar's integer, not its text: anything that does not read as a number
    /// is off, and a number other than 0 is on.
    #[test]
    fn the_version_check_is_the_cvars_integer() {
        let files = Files::default();
        let mut s = vm(&files);
        let check = |s: &UiScript| {
            s.eval::<Option<f64>>("return IsAddonVersionCheckEnabled()")
                .unwrap()
        };
        for (value, on) in [
            ("1", true),
            ("0", false),
            ("", false),
            ("abc", false),
            ("false", false),
            ("2", true),
            ("-1", true),
            ("0.5", false),
            ("00", false),
            ("1x", true),
        ] {
            s.set_cvar_engine("checkAddonVersion", value);
            assert_eq!(check(&s).is_some(), on, "{value:?}");
        }
    }

    /// `LaunchAddOnURL` takes one index (`Usage: LaunchAddOnURL(index)`, range error), sends the
    /// server's url straight to the launcher with no whitelist, and sends nothing for no url or a
    /// banned addon.
    #[test]
    fn launch_addon_url_sends_the_servers_url_ungated_by_the_whitelist() {
        let files = Files::default();
        let mut s = vm(&files);
        s.note_addon_info_records(vec![
            AddonReply {
                name: "Alpha".into(),
                status: 1,
                url: Some("http://example.com:8080/a%20b?x=1".into()),
                ..AddonReply::default()
            },
            AddonReply {
                name: "Beta".into(),
                status: 0,
                url: Some("http://example.com/banned".into()),
                ..AddonReply::default()
            },
            AddonReply {
                name: "Gamma".into(),
                status: 1,
                url: Some("file:///etc/passwd".into()),
                ..AddonReply::default()
            },
            record("Hidden", 2),
        ]);
        // Alpha: not on LaunchURL's list and carrying a query, a port and an escape; Beta banned;
        // Gamma a file path; Zeta no url at all.
        s.run("LaunchAddOnURL(1) LaunchAddOnURL(2) LaunchAddOnURL(3) LaunchAddOnURL(4)")
            .unwrap();
        let launched: Vec<String> = s
            .take_glue_requests()
            .into_iter()
            .filter_map(|r| match r {
                GlueRequest::LaunchAddonUrl(u) => Some(u),
                _ => None,
            })
            .collect();
        assert_eq!(
            launched,
            ["http://example.com:8080/a%20b?x=1"],
            "Alpha only, its escape decoded as 0x429ff0 does and the space encoded again"
        );
        assert!(raised(&s, "LaunchAddOnURL('x')").contains("Usage: LaunchAddOnURL(index)"));
        assert!(raised(&s, "LaunchAddOnURL(9)").contains("range of 1 to 4"));
    }
}
