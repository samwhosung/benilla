//! **The AddOn API** (1188 phase 2) — the eleven globals an addon manager and a demand-loader use,
//! over a registry the host fills at discovery.
//!
//! Every name here is in the 1.12.1 client's own `_G` with origin `engine`
//! (`reference/1.12-globals.tsv`), so all eleven are ours to implement in Rust and the set is
//! measured rather than remembered. `SaveAddOns`/`ResetAddOns`/`GetAddOnEnableState` are
//! deliberately **not** here: they exist only in the *glue* namespace (`Interface\GlueXML\
//! AddonList.lua`), which is a different `_G` from the in-world one this VM is.
//!
//! ## The shapes are the reference's, read off its own call sites
//!
//! Not from memory — from `Interface\GlueXML\AddonList.lua` and `FrameXML\UIParent.lua` in the
//! 1.12 MPQs:
//!
//! ```lua
//! name, title, notes, url, loadable, reason, security, newVersion = GetAddOnInfo(index)
//! local loaded, reason = LoadAddOn(name)          -- UIParent.lua's UIParentLoadAddOn
//! ```
//!
//! `reason` is a **token**, spliced by the reference into `getglobal("ADDON_"..reason)` — so its
//! spelling is load-bearing and its full set is enumerable from `GlobalStrings.lua`: `MISSING`,
//! `DISABLED`, `CORRUPT`, `BANNED`, `INSECURE`, `INTERFACE_VERSION`, `NOT_DEMAND_LOADED`, the
//! `DEP_*` mirrors of each, and `UNKNOWN_ERROR`. `security` is likewise a token, matched against
//! `"SECURE"`/`"INSECURE"`/`"BANNED"` by the glue's icon picker.
//!
//! **Index or name, everywhere.** Every verb takes either, 1-based, because the reference's do.
//!
//! ## The load law lives next door
//!
//! Every "can this load, and why not" answer comes from [`super::addon_gate`] — the reference's
//! `AddOn_CanLoad 0x51e780` as one pure function (decision 1292). What 1191 §6 was missing has
//! now landed: the version gate is an
//! exact `== 11200` whose refusal the `checkAddonVersion` CVar suppresses by **actively
//! resetting** the reason — so `INTERFACE_VERSION` is now enforced here exactly as the client
//! enforces it, with the *Load out of date AddOns* toggle as the player's escape, instead of
//! 1191's report-but-never-act interim. The CVar is read live per query (`0x51e780`), which is
//! why a checkbox click needs nothing but a list repaint.

use std::path::PathBuf;

use mlua::{Lua, MultiValue, Value};

use super::addon_gate::{can_load, GateRow, Verdict};
use super::binding_abi::flag;
use super::Model;

/// A reader for a chain-sourced addon's files, by chain-internal path — what the host seats
/// through [`super::UiScript::set_addon_chain_reader`] (1957).
pub type AddonChainReader = Box<dyn Fn(&str) -> Option<Vec<u8>>>;

/// [`AddonChainReader`] as the model holds it: shared, so a load can borrow it without holding
/// the model.
pub(crate) type SharedChainReader = std::rc::Rc<dyn Fn(&str) -> Option<Vec<u8>>>;

/// One addon, as the AddOn API sees it — the host fills this at discovery
/// ([`super::UiScript::register_addons`]).
#[derive(Clone, Debug, Default)]
pub struct AddOnInfo {
    /// The folder name. `GetAddOnInfo`'s first return, `ADDON_LOADED`'s `arg1`, and the key every
    /// other verb resolves a name against (case-insensitively, as the reference does).
    pub name: String,
    /// `## Title`. `None` when the manifest declares none — and the reference really does return
    /// nil there, which is why the glue writes `if title then … else SetText(name) end`.
    pub title: Option<String>,
    /// `## Notes`.
    pub notes: Option<String>,
    /// `## URL`.
    pub url: Option<String>,
    /// `## Secure: 1` — Blizzard's own addons carry it. Drives the `security` token.
    pub secure: bool,
    /// `## LoadOnDemand: 1`.
    pub load_on_demand: bool,
    /// `## Dependencies` / `## RequiredDeps`.
    pub dependencies: Vec<String>,
    /// Every `## Key: Value` in manifest order, for `GetAddOnMetadata`.
    pub directives: Vec<(String, String)>,
    /// The `.toc`'s ordered file list — what `LoadAddOn` runs.
    pub files: Vec<String>,
    /// `## SavedVariables` — globals restored from, and written to, the account-scoped file.
    pub saved_variables: Vec<String>,
    /// `## SavedVariablesPerCharacter` — the same, per character. Loaded **second**, so a
    /// per-character value wins over the account one (`0x51f4b5` then `0x51f53b`).
    pub saved_variables_per_character: Vec<String>,
    /// Enable state, from `AddOns.txt`. An addon nobody has ever disabled is enabled.
    pub enabled: bool,
    /// `enabled` as registration found it — the last-SAVED state, which `ResetDisabledAddOns`
    /// reverts to. Stamped by [`super::UiScript::register_addons`]; callers need not set it.
    pub saved_enabled: bool,
    /// Has it loaded this session?
    pub loaded: bool,
    /// `## Interface` as the client parses it (`Toc::interface_version` — the leading integer,
    /// `0` when the line is absent). What the version gate compares (decision 1292).
    pub interface: u32,
    /// **The server excluded this record from the Lua index space** — `[rec+0x29]`, which
    /// `AddOn_ReadAddonInfoReply 0x51da70` sets to 1 for every `SMSG_ADDON_INFO` record whose
    /// `status` byte is **2** (`0x51db84`), and which the array rebuild then drops (`0x51dc4f
    /// mov al,[ebx+0x29]` / `0x51dc54 jne`). It is written nowhere else but the ctor's zero
    /// (`0x52059f`), so an addon the reply never covers stays visible.
    ///
    /// The reply covers exactly the `## Secure:` addons, in the order the client sent them, and
    /// the 2006 retail capture answers `status = 2` for all twelve — which is why the stock
    /// AddOns list shows the player's addons and none of Blizzard's.
    pub hidden: bool,
    /// The addon's files sit in the player's patch chain, not the AddOns folder — Blizzard's own
    /// LoadOnDemand addons (`Blizzard_TrainerUI` and its eleven siblings), which the reference
    /// loads through the same `LoadAddOn` path as a player's (1957). Read through the host's
    /// chain reader ([`super::UiScript::set_addon_chain_reader`]) at
    /// `Interface/AddOns/<Name>/<file>`; no folder root is needed for them.
    pub chain: bool,
}

/// The verbs' own `Usage:` literals, read out of `.data` rather than reconstructed — the message
/// an addon's error handler prints, so the spelling is the contract. `%s`-free: each is a whole
/// string in the image.
const USAGE_INFO: &str = "Usage: GetAddOnInfo(index or \"name\")"; // 0x842d68
const USAGE_METADATA: &str = "Usage: GetAddOnMetadata(index or \"name\", \"variable\")"; // 0x842d90
const USAGE_DEPENDENCIES: &str = "Usage: GetAddOnDependencies(index or \"name\")"; // 0x842dc8
const USAGE_ENABLE: &str = "Usage: EnableAddOn(index or \"name\")"; // 0x842df8
const USAGE_DISABLE: &str = "Usage: DisableAddOn(index or \"name\")"; // 0x842e1c
const USAGE_LOAD_ON_DEMAND: &str = "Usage: IsAddOnLoadOnDemand(index or \"name\")"; // 0x842e44
const USAGE_LOADED: &str = "Usage: IsAddOnLoaded(index or \"name\")"; // 0x842e70
const USAGE_LOAD: &str = "Usage: LoadAddOn(index or \"name\")"; // 0x842e98

/// **The `index or "name"` argument every in-game addon verb opens with — and its two raises.**
///
/// All eight bindings (`GetAddOnInfo 0x48e390`, `GetAddOnMetadata 0x48e530`,
/// `GetAddOnDependencies 0x48e5e0`, `EnableAddOn 0x48e690`, `DisableAddOn 0x48e760`,
/// `IsAddOnLoadOnDemand 0x48e840`, `IsAddOnLoaded 0x48e8e0`, `LoadAddOn 0x48e980`) open with the
/// *same eleven instructions*, and the shape has three arms, not one:
///
/// ```text
/// edx=1; call 0x6f34d0            ; lua_isnumber — tag 3 OR a numeric string
///   je  <string>
///   call 0x6f3620 / 0x40a2b0      ; lua_tonumber, then double->int32 TRUNCATING toward zero
///   dec eax                       ; the index is 1-BASED
///   call 0x51df00                 ; name-at(index0); NULL is the bounds failure
///   jne <shared>
///   call 0x51def0                 ; the addon COUNT
///   luaL_error("AddOn index must be in the range of 1 to %d" /*0x837d70*/, count)
/// <string>:
///   call 0x6f3510                 ; lua_isstring
///   je  <usage>
///   call 0x6f3690                 ; lua_tostring — used AS GIVEN, never validated here
/// <usage>:
///   luaL_error("Usage: <Verb>(index or \"name\")")
/// ```
///
/// **`luaL_error 0x6f4940` does not return** ([`super::binding_abi`]): it longjmps, so both arms
/// abandon the caller's statement. We answered a placeholder, a `nil` or a silent no-op for every
/// one of them — the failure mode that module's header names: *"a client that answers `nil` there
/// keeps executing a statement the real client never finishes."*
///
/// Three consequences that are easy to get wrong and are all byte-read:
///
/// - **The bound is UNSIGNED** (`0x51df00 cmp ecx,[0xbe1b90]; jb`), so `f(0)` decrements to
///   `0xFFFFFFFF` and raises by the same route as `f(count+1)`.
/// - **A numeric STRING is an INDEX**, because `0x6f34d0` coerces one — `GetAddOnInfo("2")` is the
///   second addon, not an addon named `2`.
/// - **A name is never checked against the registry by this prologue.** Each verb's own body
///   decides what a miss means, and they do not agree: `GetAddOnInfo` echoes the name back with
///   placeholders, `IsAddOnLoaded` answers nil, `GetAddOnDependencies` answers nothing.
enum AddonKey {
    /// A bounds-checked 0-based position in the registry.
    Index(usize),
    /// A name exactly as the caller spelled it — unvalidated, per the prologue above.
    Name(String),
}

/// The prologue itself. `usage` is the verb's own `.data` literal, verbatim.
fn addon_key(lua: &Lua, model: &Model, key: &Value, usage: &'static str) -> mlua::Result<AddonKey> {
    // `lua_isnumber 0x6f34d0` — and `coerce_number` is its exact analogue, numeric strings and all.
    if let Some(n) = lua.coerce_number(key.clone())? {
        // `_ftol 0x40a2b0` truncates toward zero (not `floor`), then `dec eax`, then an unsigned
        // compare — so the whole out-of-range family collapses onto one `u32` bound test.
        let index0 = (n as i64 as i32).wrapping_sub(1) as u32 as usize;
        // **The index space is NOT the registry** (decision 2175). `0x51df00` reads
        // `[[0xbe1b94] + 4*idx]` — the flat, Title-sorted, hidden-filtered name array — and its
        // bound is that array's own count `[0xbe1b90]`, which `GetNumAddOns 0x51def0` returns.
        // The registry list is a different order over a different set.
        return match model.addon_index.get(index0) {
            Some(&row) => Ok(AddonKey::Index(row)),
            None => Err(mlua::Error::RuntimeError(format!(
                "AddOn index must be in the range of 1 to {}",
                model.addon_index.len()
            ))),
        };
    }
    // `lua_isstring 0x6f3510` → `lua_tostring 0x6f3690`. Lossy for the reason 1193/2138 give: a
    // 5.0 string is bytes, and a stray one costs a glyph, not the call.
    match key {
        Value::String(s) => Ok(AddonKey::Name(s.to_string_lossy())),
        _ => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

/// **Rebuild the Lua index space** — the tail block `[0x51dc30, 0x51dcdf)` of
/// `AddOn_ReadAddonInfoReply 0x51da70`, and the only place that array is ever built
/// (decision 2175).
///
/// Three properties, all byte-read, all easy to get wrong:
///
/// - **The set is filtered.** `0x51dc4f mov al,[ebx+0x29]` / `0x51dc54 jne` drops every record the
///   server marked `status = 2`. On a stock install that is all twelve `Blizzard_*` addons, which
///   is why the AddOns list shows only the player's own.
/// - **The order is by `## Title:`, not by folder name.** Comparator `0x51deb0` resolves both
///   sides through `AddOn_GetTitle 0x51df20` and falls back to the folder name only when the
///   record declares no `Title` (`0x51ded0`/`0x51ded6`), then compares with `SStrCmpI` — case
///   INSENSITIVE. A materially different permutation from the registry's.
/// - **No reply, no array.** `[0xbe1b90]` starts at 0 and is written nowhere else, so
///   `GetNumAddOns()` answers 0 until the server replies.
///
/// The reference's `qsort 0x73f727` is **not stable**, so two records with equal keys have no
/// defined relative order there; ours is stable, which is a strict refinement of "undefined" and
/// cannot disagree with a defined case.
fn rebuild_index(model: &mut Model) {
    let Some(hidden) = model.addon_info_hidden.as_ref() else {
        model.addon_index = Vec::new(); // no reply yet — the reference's own empty array
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

/// A name → a position, folded. Names compare case-insensitively for the same reason dependency
/// lookup does: a `.toc` may spell a name any way.
fn by_name(model: &Model, name: &str) -> Option<usize> {
    model
        .addons
        .iter()
        .position(|a| a.name.eq_ignore_ascii_case(name))
}

/// A validated key → a row, for the verbs whose miss answer is "not found" rather than a raise.
fn row_of(model: &Model, key: &AddonKey) -> Option<usize> {
    match key {
        AddonKey::Index(i) => Some(*i),
        AddonKey::Name(n) => by_name(model, n),
    }
}

/// Lower the registry into the gate's rows — ONE adapter, so every verb consults the same law
/// ([`super::addon_gate`], decision 1292) over the same facts.
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

/// The live `checkAddonVersion` read — the gate's CVar half, re-read per query like the
/// reference's (`IsAddonVersionCheckEnabled 0x51f180` inside `AddOn_CanLoad`). An absent
/// table (a bare test VM, or a query before the host's seed) answers the registrar default:
/// check ON, `"1"`.
fn version_check(model: &Model) -> bool {
    model
        .cvars
        .get("checkaddonversion")
        .is_none_or(|s| s.value != "0")
}

/// [`can_load`] over the registry, in the in-game flavour (`dl=1` — the surface these verbs
/// are): the addon's own verdict, with the loaded short-circuit the in-game `GetAddOnInfo`
/// applies (`0x48e390`: an already-loaded addon reports loadable/nil before the gate runs, so
/// flipping its toggle mid-session cannot retroactively mark it unloadable).
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
            // `0x51def0` is six bytes: `return [0xbe1b90]` — the count of the SORTED array, not the
            // registry size (decision 2175).
            Ok(lua
                .app_data_ref::<Model>()
                .expect("model")
                .addon_index
                .len())
        })?,
    )?;

    // `name, title, notes, enabled, loadable, reason, security` — SEVEN values, and slot 4 is
    // **`enabled`**, not `url`.
    //
    // **The client registers `GetAddOnInfo` TWICE, as two different functions**, and this is the
    // in-game one. Glue `0x46d460` returns EIGHT with `url` at slot 4 and `newVersion` at 8;
    // in-game `0x48e390` returns SEVEN with `enabled` at slot 4 and no `url` at all. They differ in
    // behaviour too — the in-game one passes `dl=1`,
    // so `NOT_DEMAND_LOADED` is reachable here and never from glue, and it short-circuits an
    // already-loaded addon to `1, nil` before the gate.
    //
    // We shipped the GLUE shape in the IN-GAME VM. Slots 5/6/7 aligned by luck; slot 4 did not, and
    // it is the one the ecosystem reads:
    //
    //     local name, _, _, enabled, loadable = GetAddOnInfo(major)   -- AceLibrary-1.0:400
    //
    // `## URL` is rare, so `enabled` came back nil for nearly every addon and Ace's
    // `if enabled and loadable` refused to load its own dependency. 70 corpus folders reach it
    // (AceLibrary/AceAddon replicated — one library, which is what makes it wide rather than
    // narrow). Nothing errored: a nil where a flag belongs is a legal Lua value.
    //
    // `enabled` is the number 1 or nil, never boolean `true` — every push in these functions is
    // `lua_pushnumber(1.0)` via `0x6f3810`, with no `lua_pushboolean` anywhere. Same for
    // `loadable`. `flag` already answers in that shape.
    //
    // `url` is not lost, it moves to where the client keeps it in-game: `GetAddOnMetadata(name,
    // "URL")`.
    g.set(
        "GetAddOnInfo",
        lua.create_function(|lua, key: Value| {
            let model = lua.app_data_ref::<Model>().expect("model");
            // **The two argument forms miss differently** (decision 1845). The numeric one cannot
            // reach here at all — [`addon_key`] has already raised the range error for it.
            let i = match addon_key(lua, &model, &key, USAGE_INFO)? {
                AddonKey::Index(i) => i,
                AddonKey::Name(name) => match by_name(&model, &name) {
                    Some(i) => i,
                    // The STRING form is not existence-checked: it answers seven placeholders, two
                    // of which are not nil — and **slot 1 is the caller's own string echoed back**
                    // (`0x48e401`, non-NULL by `lua_isstring`). We used to answer the literal
                    // `"NoSuchAddon"`, which was an illustrative example, not a constant
                    // the image contains. Slot 4 is `enabled` in-game (the glue table's fourth is
                    // `url`, and its eighth is an appended `newVersion` — nothing shifts).
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
            // The one arbiter (decision 1292): loaded short-circuit, then `AddOn_CanLoad` in the
            // in-game flavour — so NOT_DEMAND_LOADED and INTERFACE_VERSION are reachable here,
            // exactly as `0x48e390` reports them.
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
            // `0x51e6f0` takes the resolved NAME and answers `[rec+0x18]`; a miss pushes nil
            // (`0x48e95b`), so an unknown name is one value, not a raise.
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

    // Varargs of dependency names — the glue folds them into a tooltip line with `AddonTooltip_
    // BuildDeps(GetAddOnDependencies(id))`, so "none" is zero returns, not an empty string.
    g.set(
        "GetAddOnDependencies",
        lua.create_function(|lua, key: Value| {
            let model = lua.app_data_ref::<Model>().expect("model");
            let key = addon_key(lua, &model, &key, USAGE_DEPENDENCIES)?;
            // A name the registry does not hold reaches `0x51e350` and comes back NULL, which
            // takes `0x48e68a` — zero values, no raise.
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

    // The raw `## Key: Value`, by key — how an addon reads its own `## Version`.
    g.set(
        "GetAddOnMetadata",
        lua.create_function(|lua, (key, field): (Value, Value)| {
            let model = lua.app_data_ref::<Model>().expect("model");
            let key = addon_key(lua, &model, &key, USAGE_METADATA)?;
            // Argument 2 is gated by its own `lua_isstring` (`0x48e59c`, edx=2) onto the SAME
            // usage raise (`0x48e5cb`) — so `GetAddOnMetadata("Foo")` raises rather than
            // answering nil.
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

    // **One argument, index or name — the `(character, index)` form is GLUE-ONLY** (in-game
    // `0x48e690`/`0x48e760` read arg1 alone and key the store on `0x5abdc0()`, the logged-in
    // character; the two-argument shape belongs to the glue registrars `0x46d7b0`/`0x46d8a0`, and
    // our glue screen is native — no caller exists).
    // A numeric index out of range is a **Lua error** in the reference (`luaL_error` via
    // `0x51df00`); an unknown NAME is where we diverge, disclosed: the reference creates a
    // phantom enable-hash entry for the typo, we no-op — the safer direction, and `resolve`'s
    // established semantics (1191).
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

    // No arguments read at all (`0x48e720`/`0x48e7f0` — the loop re-evaluates the bound and the
    // current character each iteration; ours is one pass over the same store).
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

    // `ResetDisabledAddOns 0x48e830` — **revert-to-last-saved**: the reference destroys the
    // current character's enable hash and reloads it from the on-disk `AddOns.txt` — so unsaved
    // toggles of BOTH polarities revert,
    // and a disable already on disk stays disabled. Within one session the current character's
    // file is immutable until the shutdown tail writes it (the glue can only edit it with no
    // session up), so the registration-time `enabled` — read from that very file — IS the
    // last-saved state, and reverting to it is the same operation without re-reading disk.
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

    // loaded, reason — see `load_addon`.
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

/// Load one addon on demand, dependencies first — `AddOn_Load 0x51f240` reached from `LoadAddOn`
/// rather than from the startup sweep.
///
/// **This runs inside a Lua binding, synchronously**, which is the whole reason the loader takes
/// `&Lua` (see `crate::loader::load_into`): the reference's `UIParentLoadAddOn` does
/// `local loaded = LoadAddOn(name)` and touches the addon's frames on the next line, so a deferred
/// load would be a different function wearing the same name.
///
/// `Err` carries the reference's own reason token.
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
        return Ok(()); // the reference answers a redundant load with success, not an error
    }
    // The one arbiter (decision 1292): the reference's own shape — `LoadAddOn 0x48e980` refuses
    // through `AddOn_CanLoad` (via `AddOn_Load`'s step 3) and re-derives the reason from the
    // same gate on failure. The version gate is in here now: an out-of-date addon demand-loads
    // only under force-load, exactly as `0x48ea8c` records.
    {
        let model = lua.app_data_ref::<Model>().expect("model");
        if let refused @ Verdict::Refused { .. } = verdict(&model, i) {
            return Err(refused.token().expect("a refusal always carries a token"));
        }
    }
    // Where this addon's bytes come from: the AddOns folder for a player's addon, the patch chain
    // for Blizzard's own LoadOnDemand addons (`AddOnInfo::chain`). Either absent is `MISSING` —
    // a hermetic capture has no folder, a bare VM has no chain.
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

    // ── The *loaded* stamp goes HERE, before the dependency walk — and it is the whole reason a
    // dependency cycle terminates (decision 2139).
    //
    // `AddOn_Load 0x51f240` carries **no re-entrancy guard of its own**: an image-wide census of
    // the visiting byte `[UIADDON+0x2d]` puts every live site inside `AddOn_CanLoad 0x51e780`
    // (writers `0x5205a8`/`0x51e8ae`/`0x51e8f1`, reader `0x51e812`) and none in the loader. What
    // bounds the recursion is this byte: `0x51f313 mov byte [rec+0x18],1` sits between
    // `0x51f311 test eax,eax` and `0x51f317 jbe` — a flag-neutral, therefore **unconditional**
    // store — in the dependency loop's PREAMBLE, ahead of both the OptionalDeps body
    // (`0x51f320`) and the required-dep body (`0x51f343`). So a re-entered frame hits the
    // already-loaded early-out at `0x51f2d6`/`0x51f2db` and returns 1.
    //
    // We stamped it *after* the recursion instead, which is why `LoadAddOn` on two mutually
    // dependent LoadOnDemand addons overflowed the stack and took the process down with SIGABRT —
    // not a Lua error, so nothing `pcall` or the error handler could reach.
    //
    // **The gate strictly dominates this store** (`0x51f2fa`/`0x51f301` precede `0x51f313`), so a
    // refusal leaves the byte clear and the load stays retryable. That ordering is why the stamp
    // is here and not at the top of the function.
    //
    // **The consequence is real and is the reference's own**: inside a cycle an addon is flagged
    // loaded before its own files run, so `IsAddOnLoaded("A")` answers 1 for the whole of B's
    // execution, and `ADDON_LOADED` fires B before A. That is not a wart we are copying blindly —
    // it is what makes the recursion finite.
    {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        model.addons[i].loaded = true;
    }

    // The gate said yes, so a dependency failing HERE is a load-time failure (its files errored,
    // it was uninstalled mid-session) — still mapped to the DEP_ mirror, applied once.
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
                            // The prefix applies exactly once at any nesting depth
                            // (`0x51e8ce`/`0x51e8cf`).
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
    // `Bindings.xml` (1188 phase 4) attaches here, between the files and the saved variables.
    load_bindings(lua, &name, &read);
    load_saved_variables(lua, i);
    // The verified position: after the files, at the end of this addon's load (`0x51f5ad`).
    super::event::fire_global(lua, "ADDON_LOADED", &[super::ScriptValue::Str(name)]);
    Ok(())
}

/// Register this addon's `Bindings.xml` into the key-binding table (decision 1188 phase 4) — the
/// demand-load twin of what the host's startup walk does at the same position.
///
/// **Position is the mechanism here too**: the reference loads it at `0x51f400`, after the
/// addon's own `.toc` files and before its saved variables. After the files, because a binding's
/// body calls the functions those files define; before the saved variables, because they are the
/// next step of the same verified sequence and `ADDON_LOADED` closes it.
///
/// A missing file is the normal case and silent — most addons declare no bindings. `Bindings.xml`
/// is the reference's own spelling and the only one probed; the sandbox is [`read_under`]'s, so
/// this reads exactly what the addon's other files may read and nothing else.
fn load_bindings(lua: &Lua, name: &str, read: &Reader) {
    let path = crate::loader::join_ref(name, "Bindings.xml");
    let Some(bytes) = read(&path) else {
        return;
    };
    match crate::bindings_xml::parse(&crate::source::decode(&bytes)) {
        Ok(bindings) => super::keybind::register_addon_bindings(lua, name, &bindings),
        // Reported rather than swallowed, like a failed saved-variables chunk: a binding file that
        // stopped parsing means keys silently stop working, which is exactly what a player cannot
        // diagnose on their own.
        Err(e) => log_error(lua, &format!("{name}/Bindings.xml: {e}")),
    }
}

/// Execute this addon's saved-variables files — **account first, per-character second**, which is
/// the reference's order (`0x51f4b5` then `0x51f53b`) and therefore why a per-character value wins.
///
/// Each file is *executed as a Lua chunk in the shared global state*, not parsed: the reference
/// does exactly that (`0x704bc0` names the chunk, `0x704ae0` runs `luaL_loadbuffer` +
/// `lua_pcall`), with no `setfenv` and no restricted environment. A missing file is a silent
/// no-op (`0x51f4a9`, `0x51f530`) — it is the normal first-run case.
///
/// **Position is the mechanism**: this runs after the addon's own files (which assign the
/// file-scope defaults) and before `ADDON_LOADED` (whose handlers are specified to see the
/// restored value). Reverse either and the saved value can never win.
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
            continue; // an addon that declares nothing has no file, even if one is lying there
        }
        let Some(dir) = dir else { continue };
        let path = dir.join(format!("{name}.lua"));
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, // the first-run case
            Err(e) => {
                // Present but unreadable is not "absent": hold it, like a file that will not run.
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
            // The reference fails this silently. We do not: a settings file that stopped parsing
            // is exactly the thing a player needs told, and the file is left on disk untouched —
            // held, so the shutdown write does not replace it with the defaults this session is
            // running on (`UiScript::hold_saved_file`).
            log_error(lua, &format!("{}: {e}", path.display()));
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if !model.held_saved_files.contains(&path) {
                model.held_saved_files.push(path);
            }
        }
    }
}

/// Run one addon's `.toc`-listed files, in listed order — the demand-load twin of the host's
/// `Addon::load_files`, and deliberately the same rules: `.lua` is a chunk, anything else is
/// FrameXML, and every reference resolves against the *including file's* directory (1186) inside
/// the AddOns root as the sandbox.
/// One addon's byte source, by a path relative to the AddOns root (`<Name>/<file>`) — the folder
/// reader for a player's addon, the chain reader for a Blizzard LoadOnDemand one.
type Reader = Box<dyn Fn(&str) -> Option<Vec<u8>>>;

fn run_files(lua: &Lua, name: &str, read: &Reader, files: &[String]) {
    let provider = |req: &str| -> Option<Vec<u8>> { read(req) };
    for file in files {
        let path = crate::loader::join_ref(name, file);
        let Some(bytes) = read(&path) else {
            // **A manifest entry the package does not ship is the PACKAGE's defect** — 1450's
            // rule, and until 2107 this path had its own copy of it that said the opposite.
            // The reference logs `Couldn't open %s` and carries on
            // (`diagnostics::record_load_failure`'s doc has the bytes), so the addon still loads
            // and `IsAddOnLoaded` still answers 1; the miss is retained and warned, never a
            // script error. It cost 61 corpus addons their session-start row: FuBar's
            // `LoadLoadOnDemandPlugins` demand-loads 55 plugins whose `.toc`s list a koKR
            // localization file none of them ships.
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
                // The walk's severity for the same thing: a document that will not parse is a
                // load failure, not a raise — the reference answers it with a `FrameXML.log` line
                // and silence (`Couldn't parse XML in %s` `0x846fd8`, sink severity 2).
                load_failure(lua, &format!("{name}/{file}: {e}"));
                continue;
            }
        };
        let report = crate::loader::load_into(lua, &doc, &path, &provider);
        // The warnings were dropped on the floor here until 2135 — an unresolved `inherits=`, a
        // template of the wrong kind, an attribute nobody reads: the addon loads "clean", paints
        // nothing, and the one thing that knew why was a `LoadReport` that went out of scope.
        // They carry the `<Addon>/<file>` prefix because a bare loader warning names no document.
        for w in report.warnings {
            crate::script::diagnostics::record_warning(lua, &format!("{name}/{file}: {w}"));
        }
        for e in report.errors {
            log_error(lua, &format!("{name}/{file}: {e}"));
        }
    }
}

/// A `.toc` line naming a file the package does not contain, classified the one way
/// ([`crate::script::diagnostics::record_load_failure`]) and logged the one severity (1450: a
/// player's addon is never an ERROR of ours).
///
/// Both halves are the startup walk's, verbatim in effect: the walk pairs its `warn!` with
/// `report_load_failure`, and this pairs the host warning channel — which the app drains at
/// `warn!` — with the same retention call. What it deliberately does **not** touch is
/// [`Model::errors`], the instruments' script-error channel: nothing raised.
fn load_miss(lua: &Lua, name: &str, file: &str) {
    load_failure(lua, &format!("{name}/{file}: not found"));
}

/// Record + say out loud, the pair [`log_error`]'s louder sibling makes for a failure that
/// **raised**. `benilla-ui` has no logger of its own, so the second half goes on the host's
/// non-fatal warning channel, which the app drains at `warn!` — the walk's own severity for the
/// same failure.
fn load_failure(lua: &Lua, msg: &str) {
    let msg = format!("LoadAddOn: {msg}");
    crate::script::diagnostics::record_load_failure(lua, &msg);
    // `warn_host_only`, not `record_warning`: `record_load_failure` above has already retained
    // this exact sentence as a `Load` row, which is the truer kind (the addon is not running).
    lua.app_data_mut::<Model>()
        .expect("model")
        .warn_host_only(msg);
}

/// `root/rel`, refusing to escape `root` — the AddOns-root sandbox (1186), lexical and applied
/// before any filesystem call. `join_ref` has already resolved the path, so an escape survives as
/// a leading `..` and is what the `Normal`-component test rejects.
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

/// Run a chunk that came off disk, from `&Lua` — [`crate::script::UiScript::run_chunk`]'s body for
/// the demand-load path, which never holds a `UiScript` (1191 §4). Bytes and a BOM strip, for the
/// reasons in [`crate::source`].
fn run_chunk(lua: &Lua, bytes: &[u8], name: &str) -> mlua::Result<()> {
    lua.load(crate::source::chunk(bytes))
        .set_name(name)
        .set_mode(mlua::ChunkMode::Text)
        .exec()
}

/// Record a demand-load failure the way the startup walk records its own
/// ([`super::UiScript::report_script_error`]): retained as a Load row, and dispatched to the
/// player's error handler — plus the host drain the instruments read. It went to that drain alone
/// until now, so a `LoadAddOn` whose file raised surfaced as a terminal line and nothing a player
/// could see.
fn log_error(lua: &Lua, msg: &str) {
    let msg = format!("LoadAddOn: {msg}");
    super::diagnostics::record_load_failure(lua, &msg);
    let mut model = lua.app_data_mut::<Model>().expect("model");
    model.pending_error_dispatch.push(msg.clone());
    model.errors.push(msg);
}

/// The host's push: the discovered registry and the root its files live under.
impl super::UiScript {
    /// Seat the host's chain reader — what a chain-sourced addon (`AddOnInfo::chain`) is read
    /// through, by chain-internal path (1957).
    pub fn set_addon_chain_reader(&self, reader: AddonChainReader) {
        self.model_mut().addons_chain_reader = Some(std::rc::Rc::from(reader));
    }
    /// Replace the AddOn registry — called once at world entry, after discovery
    /// ([`crate::script`]'s `register_bindings` is the same shape: the host owns the facts, the
    /// engine owns the verbs).
    pub fn register_addons(
        &mut self,
        mut addons: Vec<AddOnInfo>,
        root: Option<PathBuf>,
        saved_account: Option<PathBuf>,
        saved_character: Option<PathBuf>,
    ) {
        // The registration-time enable state IS the last-saved state (the host read it off the
        // enable file moments ago) — snapshot it here so `ResetDisabledAddOns` has its revert
        // point and no caller has to remember to provide one.
        for a in &mut addons {
            a.saved_enabled = a.enabled;
        }
        let mut model = self.model_mut();
        model.addons = addons;
        model.addons_root = root;
        model.addons_saved_account = saved_account;
        model.addons_saved_character = saved_character;
        // The reference builds the registry at glue login and the Lua array only when the server
        // answers; our registry is built at world entry, by which time the reply has long landed.
        // So the rebuild runs here, off whatever the reply said — and does nothing at all if no
        // reply ever came, which is the reference's own pre-reply state.
        rebuild_index(&mut model);
    }

    /// **The `SMSG_ADDON_INFO` (`0x2ef`) verdict** — the names the server answered `status = 2`
    /// for, which the client stores as `[rec+0x29] = 1` and which the Lua index array then drops
    /// (decision 2175).
    ///
    /// The reply carries no count and no names: it is one record per `## Secure:` addon, in the
    /// order the client itself sent them in `CMSG_AUTH_SESSION`, so the caller does the pairing
    /// and hands us the names (`0x51da70`).
    /// Calling this with an empty slice is meaningful and different from never calling it: it
    /// records that a reply arrived and hid nothing.
    pub fn note_addon_info_reply(&mut self, hidden: &[String]) {
        let mut model = self.model_mut();
        model.addon_info_hidden = Some(hidden.iter().map(|n| n.to_ascii_lowercase()).collect());
        rebuild_index(&mut model);
    }

    /// Execute one addon's saved-variables files, at the startup walk's verified position — the
    /// twin of what [`load_saved_variables`] does inside `LoadAddOn`, so both halves restore state
    /// the same way.
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

    /// Every **loaded** addon that declares saved variables, as
    /// `(name, account-scoped names, per-character names)` — what the host writes at shutdown.
    ///
    /// The gate is *loaded*, not a dirty bit: the reference gates its write on the record's loaded
    /// byte (`0x51f711`) and has no dirty tracking at all. An addon that never loaded this session
    /// has no globals to write, and writing it would blank the file it never read.
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

    /// Mark an addon loaded — the host's startup walk reporting what it ran, so `IsAddOnLoaded`
    /// answers for the startup half exactly as it does for the `LoadAddOn` half.
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

    /// `(name, enabled)` for every registered addon, in registry order — what the host writes back
    /// to `AddOns.txt`, so a `DisableAddOn` from Lua survives the session.
    pub fn addon_enable_states(&self) -> Vec<(String, bool)> {
        self.model_ref()
            .addons
            .iter()
            .map(|a| (a.name.clone(), a.enabled))
            .collect()
    }
}
