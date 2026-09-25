//! The addon corpus harness: load every addon in a folder, one at a time, and report what each did
//! as numbers.
//!
//! Each addon is surveyed in a fresh [`UiScript`] seated with the whole in-game interface (the
//! stock FrameXML off the player's chain plus the files still ours), so one addon's failure cannot
//! be charged to another. The 1.12 client loads every addon into one Lua state, so a library
//! embedded in any addon is global to those after it; here it is not, and an addon relying on a
//! sibling's copy fails. Every count is therefore a floor
//! ([`dependency_tests::a_sibling_addons_embedded_library_is_invisible`]).
//!
//! The missing-name lists are static reads of the addon's source minus what the VM and the addon
//! define: they over-report (an untaken branch counts) and miss names built at runtime, so read the
//! ranked aggregate, never one row. Each ranks its own queue: functions to write, FrameXML to
//! transcribe, widget methods no kind has, methods the receiver's kind lacks, and untyped calls.
//!
//! ```text
//! cargo run -q -p benilla-app --example addon_harness -- <folder of addons>
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use benilla_ui::script::UiScript;
use benilla_ui::toc::Toc;

/// Why a global is nil: one addon loaded as the survey loads it, then Lua evaluated against the VM.
pub mod probe;
#[cfg(test)]
mod probe_tests;
/// The render column: did the addon put anything on screen.
pub mod render;
#[cfg(test)]
mod render_tests;
/// The control for the one-VM-per-addon bound: the whole folder in one VM, as the 1.12 client runs
/// it.
pub mod together;
/// The use column: does what the addon drew survive being touched.
mod use_probe;
#[cfg(test)]
mod use_probe_tests;

use render::{measure_render, RenderBaseline};
pub use render::{Drew, RenderReport};
use use_probe::measure_use;
pub use use_probe::{UseReport, Used, MAX_USE_TARGETS};

/// What one addon did.
#[derive(Debug, Clone, Default)]
pub struct AddonReport {
    pub name: String,
    /// `## Interface`, as written (1.12 is `11200`); older values are not refused.
    pub interface: Vec<u32>,
    /// No manifest entry raised, failed to parse or dropped a frame. An entry naming no file does
    /// not count: the 1.12 client logs `Couldn't open %s` and carries on (`0x6edaa0`); its row
    /// stays in [`Self::errors`].
    pub loaded: bool,
    /// Load errors, verbatim, tagged by file.
    pub errors: Vec<String>,
    /// Manifest entries the addon's own package does not contain, also in [`Self::errors`]: the
    /// package's fault, since the 1.12 client logs `Couldn't open %s` and carries on, as ours does.
    pub absent_own_files: Vec<String>,
    /// Manifest entries resolving outside the addon's folder, as resolved paths (the `..` collapsed
    /// as `0x6ede10` does), that neither the AddOns tree nor the patch chain holds: the addon wants
    /// a neighbour that is not installed.
    pub absent_foreign_files: Vec<String>,
    /// Names it calls that the VM does not have (see the module doc).
    pub missing_globals: Vec<String>,
    /// Dependencies named in its `.toc` that are not in the folder.
    pub missing_deps: Vec<String>,
    /// Templates it names in `CreateFrame(kind, name, parent, "Template")` that the VM never
    /// declared. An unresolved one raises nothing: the addon gets a bare frame and paints nothing.
    pub missing_templates: Vec<String>,
    /// Templates it names in an XML `inherits=` that the VM never declared. A name registered as a
    /// template or a font resolves, since `<FontString inherits="GameFontNormal">` names a font.
    pub missing_inherits: Vec<String>,
    /// Names it indexes (`Foo.bar`, `Foo:baz()`) that the VM does not have: frames and tables,
    /// where [`Self::missing_globals`] is functions.
    pub missing_tables: Vec<String>,
    /// Names it calls as `obj:Name(...)` that no widget kind we ship answers, looked up through one
    /// probe of every kind's live `__index`; a method on the wrong kind is
    /// [`Self::kind_missing_methods`]. Over-reports: a `:` receiver is unknown, so a library method
    /// not defined by the addon or its loaded dependencies counts, and a method bound as
    /// `T["N"] = …` cannot be subtracted (`strip_lua_noise` blanks the string).
    pub missing_methods: Vec<String>,
    /// Methods it calls only after feature-testing them (`if f.SetTopLevel then … end`) that no
    /// widget we ship provides: not blockers, so kept out of [`Self::missing_methods`].
    pub optional_methods: Vec<String>,
    /// `Kind:Name (on …)`: a method called on a receiver of known kind that the kind does not
    /// answer, naming the kinds that do (`(on no kind)` when none). A receiver is typed when the
    /// file binds it from a widget factory (`CreateFrame`, `f:CreateTexture()`) or it is a
    /// published name the arena knows ([`UiScript::widget_kind`]); `self:`, `this:`, `a.b:` and
    /// `getglobal(n):` receivers go to [`Self::ambiguous_methods`].
    pub kind_missing_methods: Vec<String>,
    /// `Name (only on …)`: a method called on an untyped receiver that some kinds we ship answer
    /// and some do not. An upper bound on what [`scan_receivers`] cannot type; a name no kind
    /// answers is in [`Self::missing_methods`].
    pub ambiguous_methods: Vec<String>,
    /// Errors raised after the addon's own files loaded, while the session start is driven: its
    /// `ADDON_LOADED`, `VARIABLES_LOADED`, `PLAYER_LOGIN`, `PLAYER_ENTERING_WORLD`, then a few
    /// ticks of `OnUpdate`. Each dependency's `ADDON_LOADED` already fired beside its own files,
    /// where `0x51f5ad` fires it ([`load_dependencies`]). Kept apart from [`Self::loaded`], which
    /// counts load errors.
    pub session_errors: Vec<String>,
    /// What the addon's UI overrides raised when invoked (see [`drive_ui_probe`]).
    pub probe_errors: Vec<String>,
    /// Warnings raised while the addon loaded and ran: failures that raise nothing, such as a
    /// dropped `inherits=`, an unresolved `SetPoint` anchor or an unregistered `SetCVar`. Read off
    /// the retained diagnostic log by `seq`, so loader warnings count too; a warning whose exact
    /// text the FrameXML underneath already produced dedupes onto that row and is missed.
    pub warnings: Vec<String>,
    /// What it put on screen (see [`render`]).
    pub render: RenderReport,
    /// What happened when the UI it drew was used (see [`use_probe`]). A row that touched nothing
    /// is [`Used::Untouched`], which is not a pass.
    pub used: UseReport,
}

/// Survey every addon folder under `root`, an AddOns folder; a subfolder without a `<Name>.toc` is
/// skipped, as discovery skips it.
pub fn survey(root: &Path) -> Vec<AddonReport> {
    let (names, installed, registry) = corpus(root);

    // Folder -> the method names its source defines, memoised: one library folder is a dependency
    // of dozens of addons.
    let mut defined_methods: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    names
        .iter()
        .map(|n| survey_one(root, n, &installed, &registry, &mut defined_methods))
        .collect()
}

/// The corpus every VM sees: the folders with a manifest, the case-folded installed set, and the
/// AddOn registry. [`probe`] needs the identical environment, so the installed set belongs to the
/// folder, never the selection.
fn corpus(
    root: &Path,
) -> (
    Vec<String>,
    BTreeSet<String>,
    Vec<benilla_ui::script::AddOnInfo>,
) {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| manifest_path(root, n).is_some())
        .collect();
    // The live walk's order, from the one definition `ui_script::addons` uses.
    crate::ui_script::addons::sort_by_directory_order(&mut names);

    let installed: BTreeSet<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();

    // The AddOn registry, seated into every VM: without it `GetNumAddOns()` answers 0 and
    // `GetAddOnInfo` nothing, and AceAddon and AceLibrary find their dependencies through
    // `GetAddOnInfo`. Every addon is registered enabled; `loaded` stays false, since each VM loads
    // one addon and its dependencies.
    let registry: Vec<benilla_ui::script::AddOnInfo> = names
        .iter()
        .filter_map(|n| {
            let toc = Toc::parse(&benilla_ui::source::decode(
                &std::fs::read(manifest_path(root, n)?).unwrap_or_default(),
            ));
            Some(crate::ui_script::addons::info_from_toc(n, &toc))
        })
        .collect();

    (names, installed, registry)
}

/// The method names one addon folder's source defines (`function T:N`, `function T.N`, `T.N = …`),
/// memoised by folder. A method lives on an object the survey cannot name, so its dependencies'
/// source stands in for the VM read `known` does for globals.
fn methods_defined_by<'a>(
    root: &Path,
    folder: &str,
    memo: &'a mut BTreeMap<String, BTreeSet<String>>,
) -> &'a BTreeSet<String> {
    if !memo.contains_key(folder) {
        let mut scan = Scan::default();
        if let Some(toc) = manifest_path(root, folder)
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| Toc::parse(&benilla_ui::source::decode(&b)))
        {
            for path in source_files(root, folder, &toc) {
                if let Some(text) = read_text(root, &path) {
                    scan_source(&path, &text, &mut scan);
                }
            }
        }
        memo.insert(folder.to_string(), scan.defined_methods);
    }
    &memo[folder]
}

/// `<root>/<name>/<name>.toc`, matched case-insensitively: a 1.12 addon may ship
/// `MyAddon/myaddon.toc`.
fn manifest_path(root: &Path, name: &str) -> Option<PathBuf> {
    let want = format!("{name}.toc");
    std::fs::read_dir(root.join(name))
        .ok()?
        .flatten()
        .find(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|f| f.eq_ignore_ascii_case(&want))
        })
        .map(|e| e.path())
}

/// The per-addon VM-instruction bound, shared with the live client's world-entry arming (the
/// heaviest corpus addon runs 4M, 214 of 218 under 1M).
const ADDON_INSTRUCTION_BUDGET: u64 = crate::ui_script::addons::LOAD_INSTRUCTION_BUDGET;

fn survey_one(
    root: &Path,
    name: &str,
    installed: &BTreeSet<String>,
    registry: &[benilla_ui::script::AddOnInfo],
    defined_methods: &mut BTreeMap<String, BTreeSet<String>>,
) -> AddonReport {
    let Some(toc_path) = manifest_path(root, name) else {
        return AddonReport {
            name: name.to_string(),
            errors: vec!["no manifest".into()],
            ..Default::default()
        };
    };
    // Decoded, not `read_to_string`: some manifests are cp1252 and would parse as an empty `.toc`.
    let toc = Toc::parse(&benilla_ui::source::decode(
        &std::fs::read(&toc_path).unwrap_or_default(),
    ));
    let missing_deps: Vec<String> = toc
        .dependencies()
        .into_iter()
        .filter(|d| !installed.contains(&d.to_ascii_lowercase()))
        .map(str::to_owned)
        .collect();

    // A VM with our whole interface under it (see the module doc).
    let mut script = match UiScript::new() {
        Ok(s) => s,
        Err(e) => {
            return AddonReport {
                name: name.to_string(),
                interface: toc.interface_versions(),
                errors: vec![format!("VM: {e}")],
                missing_deps,
                ..Default::default()
            }
        }
    };
    // A time bound, so one non-terminating addon cannot stall the survey: about 50x the heaviest
    // corpus addon (Enchantrix, 4M instructions), so a runaway reports in about a second.
    script.set_instruction_budget(ADDON_INSTRUCTION_BUDGET);
    script.set_screen_size(1024.0, 768.0);
    // The AddOn API answers for the whole installed set before anything runs. The saved-variable
    // roots are `None`, so a survey never reads or writes a player's; the AddOns root is passed, so
    // `LoadAddOn("<folder addon>")` does not answer `MISSING`.
    script.register_addons(registry.to_vec(), Some(root.to_path_buf()), None, None);
    seat_a_session(&mut script);
    let _ = crate::ui_script::load_default_ui(&script);
    // The addon's dependencies, first and recursively, as `AddOn_Load` (`0x51f240`) does in its
    // first two steps. Loaded before `globals_of` below, so a name a dependency provides is not
    // missing.
    let mut dep_loads: Vec<LoadedDep> = Vec::new();
    load_dependencies(
        &mut script,
        root,
        &toc,
        installed,
        &mut BTreeSet::new(),
        &mut dep_loads,
    );
    let dep_order: Vec<String> = dep_loads.into_iter().map(|d| d.name).collect();
    let known = globals_of(&script);
    // The methods the dependency chain defines, read from its source (a method has no VM read).
    let dep_methods: BTreeSet<String> = dep_order
        .iter()
        .flat_map(|d| methods_defined_by(root, d, defined_methods).clone())
        .collect();

    // The render baseline: every widget that exists before this addon runs, taken after the
    // dependency chain so a library's frames are not charged to its consumer.
    let baseline = RenderBaseline::of(&script);
    // The warning mark, taken at the same seam: `seq` is monotonic and never reused, so a
    // high-water mark is a valid cut of a log that evicts and dedupes.
    let warn_mark = script.diagnostics().last().map_or(0, |d| d.seq);

    let files = load_addon_files(&script, root, name, &toc);
    // The live walk stamps each addon as loaded just before its `ADDON_LOADED` (`0x51f5ad`);
    // without it `IsAddOnLoaded` answers nil and a LoadOnDemand dependent gets
    // `DEP_NOT_DEMAND_LOADED`. The dependencies stamped themselves in `load_dependencies`.
    script.mark_addon_loaded(name);
    let wants = missing_calls(root, name, &toc, &known, &dep_methods);
    // After the addon's files, so a template its own XML declares is registered.
    let missing_templates = missing_templates(&script, root, name, &toc);
    let missing_inherits = missing_inherits(&script, root, name, &toc);
    let session_errors = drive_session_start(&mut script, name, installed);
    let probe_errors = drive_ui_probe(&mut script);
    // Warnings only; errors have their own columns.
    let warnings: Vec<String> = script
        .diagnostics()
        .into_iter()
        .filter(|d| {
            d.seq > warn_mark && d.kind == benilla_ui::script::diagnostics::DiagnosticKind::Warning
        })
        .map(|d| {
            if d.count > 1 {
                format!("{} (x{})", d.message, d.count)
            } else {
                d.message
            }
        })
        .collect();
    // After the UI probe, because it reopens what the probe's second toggle closed; before the
    // method oracle, whose probe widgets would otherwise count as this addon's drawing.
    let (render, painted) = measure_render(&mut script, &baseline);
    // Then use what it drew: after every other column so the input it invents perturbs none, before
    // the oracle so its widgets never become targets. It reuses `render`'s attribution.
    let used = measure_use(&mut script, &baseline, &painted);
    // Last: the oracle writes probe frames and a global into the VM, and no addon code runs after
    // this, so every number above is unperturbed by it. One oracle call serves both question sets
    // and the per-kind pass.
    let asked: BTreeSet<String> = wants
        .wanted_methods
        .union(&wants.tested_methods)
        .cloned()
        .collect();
    let oracle = widget_method_kinds(&script, &asked);
    let missing_methods = unresolved_from(&oracle, &wants.wanted_methods);
    let optional_methods = unresolved_from(&oracle, &wants.tested_methods);
    // The receiver-typed half, read off the same VM: the kind a call lands on is the kind our
    // object graph publishes.
    let (kind_missing_methods, ambiguous_methods) = per_kind_rows(&script, &oracle, &wants);

    AddonReport {
        name: name.to_string(),
        interface: toc.interface_versions(),
        loaded: !files.raised,
        errors: files.errors,
        absent_own_files: files.absent.own,
        absent_foreign_files: files.absent.foreign,
        missing_globals: wants.missing_globals,
        missing_deps,
        missing_templates,
        missing_inherits,
        missing_tables: wants.missing_tables,
        missing_methods,
        optional_methods,
        kind_missing_methods,
        ambiguous_methods,
        session_errors,
        probe_errors,
        warnings,
        render,
        used,
    }
}

/// Which widget kinds answer each wanted name, asked of the live `__index` dispatcher through one
/// probe per kind (the method tables are not enumerable from Lua). A kind `CreateFrame` refuses is
/// skipped. `None` means no probe could be made, and every reader of it reports the whole wanted
/// set as missing rather than nothing.
fn widget_method_kinds(script: &UiScript, wanted: &BTreeSet<String>) -> Option<MethodOracle> {
    if wanted.is_empty() {
        return Some(MethodOracle::default());
    }
    // Safe to interpolate unquoted: the scanner's identifiers hold only letters, digits and `_`.
    let names = wanted
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(",");
    let kinds = PROBE_FRAME_KINDS
        .iter()
        .map(|k| format!("\"{k}\""))
        .collect::<Vec<_>>()
        .join(",");
    let chunk = format!(
        r#"
        local kinds, probes = {{}}, {{}}
        -- Every kind `frame_kind_from_str` accepts. A kind that is refused is simply skipped: this
        -- list going stale must never turn into a wrong ANSWER, only into a narrower probe set.
        for _, kind in ipairs({{{kinds}}}) do
            local ok, f = pcall(CreateFrame, kind)
            if ok and type(f) == "table" then
                table.insert(kinds, kind)
                table.insert(probes, f)
            end
        end
        -- The two REGION leaves. They carry their own metatable (the "tag" table), so a
        -- FontString's SetFont and a Texture's SetTexCoord are unreachable from any frame probe —
        -- and region methods are a large slice of what the corpus calls.
        if probes[1] then
            local okt, tex = pcall(function() return probes[1]:CreateTexture() end)
            if okt and type(tex) == "table" then
                table.insert(kinds, "Texture") table.insert(probes, tex)
            end
            local okf, fs = pcall(function() return probes[1]:CreateFontString() end)
            if okf and type(fs) == "table" then
                table.insert(kinds, "FontString") table.insert(probes, fs)
            end
        end
        if table.getn(probes) == 0 then error("no widget probes could be created") end
        -- Row 1 is the ROSTER: which kinds actually stood up. Without it the caller has to infer
        -- the probe count from the answers, and "how many kinds exist" then depends on whether
        -- some name happened to be answered by all of them — a number that silently shrinks with
        -- the question set.
        local out = {{"*=" .. table.concat(kinds, ",")}}
        for _, name in ipairs({{{names}}}) do
            local have = ""
            for i = 1, table.getn(probes) do
                if type(probes[i][name]) == "function" then
                    if have == "" then have = kinds[i] else have = have .. "," .. kinds[i] end
                end
            end
            table.insert(out, name .. "=" .. have)
        end
        return out
    "#
    );
    let rows = script.eval::<Vec<String>>(&chunk).ok()?;
    let mut oracle = MethodOracle::default();
    for (name, have) in rows.iter().filter_map(|row| row.split_once('=')) {
        let list: Vec<String> = have
            .split(',')
            .filter(|k| !k.is_empty())
            .map(str::to_owned)
            .collect();
        if name == "*" {
            oracle.probes = list;
        } else {
            oracle.answered.insert(name.to_string(), list);
        }
    }
    Some(oracle)
}

/// What the live `__index` dispatcher answered, per kind.
#[derive(Default)]
struct MethodOracle {
    /// The kinds that stood up, in probe order.
    probes: Vec<String>,
    /// Wanted name -> the kinds that answer it (empty: no widget we ship has it).
    answered: BTreeMap<String, Vec<String>>,
}

/// The names in `wanted` that no kind answers; an oracle that could not run reports all of them.
fn unresolved_from(oracle: &Option<MethodOracle>, wanted: &BTreeSet<String>) -> Vec<String> {
    let Some(oracle) = oracle else {
        return wanted.iter().cloned().collect();
    };
    wanted
        .iter()
        .filter(|n| oracle.answered.get(*n).is_none_or(Vec::is_empty))
        .cloned()
        .collect()
}

/// The two per-kind lists, one verdict per wanted name:
///
/// | the call site | the verdict |
/// |---|---|
/// | receiver typed, the kind answers | silent |
/// | receiver typed, the kind does not | `Kind:Name`, a blocker whoever else answers it |
/// | receiver untypable, some kinds answer and some do not | `Name (only on …)`, an upper bound |
///
/// Feature-tested calls are not blockers, and names the addon defines itself are subtracted
/// (`f.Update = function … f:Update()` on its own frame is the common idiom).
fn per_kind_rows(
    script: &UiScript,
    oracle: &Option<MethodOracle>,
    wants: &Wants,
) -> (Vec<String>, Vec<String>) {
    let empty = Vec::new();
    let answered = |name: &str| -> &[String] {
        oracle
            .as_ref()
            .and_then(|o| o.answered.get(name))
            .unwrap_or(&empty)
            .as_slice()
    };
    // Every typed call site, as `name -> the kinds it was called on`.
    let mut typed: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (kind, name) in &wants.kind_calls {
        if wants.wanted_methods.contains(name) {
            typed.entry(name).or_default().insert(kind);
        }
    }
    // ...plus the published names, resolved against the live arena.
    let mut untyped_by_name: BTreeSet<&str> = BTreeSet::new();
    for (receiver, name) in &wants.global_calls {
        if !wants.wanted_methods.contains(name) {
            continue;
        }
        match script.widget_kind(receiver) {
            Some(kind) => {
                typed.entry(name).or_default().insert(kind);
            }
            // Nothing publishes that name, so the receiver is a runtime value, like `self:Foo()`.
            None => {
                untyped_by_name.insert(name);
            }
        }
    }

    let mut kind_missing: Vec<String> = Vec::new();
    for (name, called_on) in &typed {
        let have = answered(name);
        for kind in called_on {
            if !have.iter().any(|k| k == kind) {
                kind_missing.push(format!("{kind}:{name} ({})", elsewhere(have)));
            }
        }
    }

    // Ambiguous only with an untyped call site in this addon and an answer that depends on the
    // kind; a name no kind answers is already in `missing_methods`.
    let probes: &[String] = oracle.as_ref().map_or(&[], |o| o.probes.as_slice());
    let mut ambiguous: Vec<String> = Vec::new();
    for name in wants
        .wanted_methods
        .iter()
        .filter(|n| wants.loose_methods.contains(*n) || untyped_by_name.contains(n.as_str()))
    {
        let have = answered(name);
        if !have.is_empty() && have.len() < probes.len() {
            ambiguous.push(format!("{name} ({})", only_on(have, probes)));
        }
    }
    kind_missing.sort();
    (kind_missing, ambiguous)
}

/// `(on ScrollingMessageFrame)` / `(on no kind)`: the kinds that do answer a name the called kind
/// does not. "On no kind" is a verb to write; a named kind is a verb to wire to its sibling.
fn elsewhere(have: &[String]) -> String {
    match have.len() {
        0 => "on no kind".to_string(),
        1..=4 => format!("on {}", have.join(", ")),
        n => format!("on {} +{} more", have[..3].join(", "), n - 3),
    }
}

/// `(only on Texture)` / `(missing on Texture, FontString)`, whichever side is shorter;
/// deterministic, since it is part of a ranking key.
fn only_on(have: &[String], probes: &[String]) -> String {
    if have.len() * 2 <= probes.len() {
        // Never truncated: its length is the row's rank ([`ambiguous_method_demand`] counts its
        // separators), bounded because at most half the kinds answer here.
        format!("only on {}", have.join(", "))
    } else {
        let lack: Vec<&str> = probes
            .iter()
            .map(String::as_str)
            .filter(|k| !have.iter().any(|h| h == k))
            .collect();
        match lack.len() {
            0..=4 => format!("missing on {}", lack.join(", ")),
            n => format!("missing on {} +{} more", lack[..3].join(", "), n - 3),
        }
    }
}

/// Drive the session-start sequence over a loaded addon and report what its handlers raised.
///
/// The order is the 1.12 client's (`UI_Init`, `0x48fbf0`): `ADDON_LOADED`, `VARIABLES_LOADED`,
/// `PLAYER_LOGIN`, with `PLAYER_ENTERING_WORLD` in the cascade, then ten ticks of 0.1 s for
/// `OnUpdate` and first-tick timers (Ace's one-second `AceEvent_FullyInitialized` included; a
/// longer timer is not reached). Only the surveyed addon's `ADDON_LOADED` fires here: each
/// dependency's fired in [`load_dependencies`], inside its own `AddOn_Load` (`0x51f5ad`). Not
/// `ui_script::finish_ui_load`, which would read the real saved variables.
fn drive_session_start(
    script: &mut UiScript,
    name: &str,
    installed: &BTreeSet<String>,
) -> Vec<String> {
    // Errors are attributed by the raising chunk, not the event window: AceAddon runs every queued
    // consumer's `OnInitialize` on any `ADDON_LOADED` it sees (`AceAddon-2.0.lua:104-105`, `:230`),
    // so the surveyed addon's code can run inside a dependency's event. The mark opens before the
    // surveyed addon's event, and a raise in another addon's file is exempted afterwards by its
    // chunk name (`@Interface\AddOns\<Folder>\<File>`).
    let before = script.errors().len();
    script.fire_event(
        "ADDON_LOADED",
        vec![benilla_ui::script::ScriptValue::Str(name.to_string())],
    );
    for event in ["VARIABLES_LOADED", "PLAYER_LOGIN", "PLAYER_ENTERING_WORLD"] {
        script.fire_event(event, Vec::new());
    }
    for _ in 0..10 {
        script.tick(0.1);
    }
    let raised = script.errors().split_off(before);
    raised
        .into_iter()
        .filter(|e| !raised_inside_another_addons_own_file(e, name, installed))
        .collect()
}

/// Does this raise belong to another installed addon's file? A library that fails is its own row;
/// charging its consumers would count one fault once per addon that embeds it.
///
/// Only the first line, the raise site, is consulted. Any installed addon counts, not only declared
/// dependencies: `LoadAddOn` runs siblings' file scope (FuBar demand-loads every installed
/// `FuBar_*` plugin, `FuBar.lua:1034`). A chunk naming no addon folder (our FrameXML, an
/// `[string "Frame:OnEvent"]` handler) is kept, and the surveyed addon's own folder is tested
/// first, so its own `Libs\Ace\…` is never handed to an addon named `Ace`.
fn raised_inside_another_addons_own_file(
    err: &str,
    name: &str,
    installed: &BTreeSet<String>,
) -> bool {
    let Some(first) = err.lines().next().map(str::to_ascii_lowercase) else {
        return false;
    };
    let owns = |line: &str, folder: &str| {
        line.contains(&format!("\\{folder}\\"))
            || line.contains(&format!("{folder}/"))
            || line.starts_with(&format!("{folder}\\"))
    };
    let mine = name.to_ascii_lowercase();
    // The surveyed addon's own folder wins, even when another addon's name is a substring of a path
    // in it.
    if owns(&first, &mine) {
        return false;
    }
    if installed.iter().any(|d| owns(&first, d)) {
        return true;
    }
    raised_inside_a_demand_load(err, &mine)
}

/// The raise site names no file: did it happen inside a `LoadAddOn` this addon made? An inline
/// `<OnEvent>` body is compiled under the frame's name (`[string "AuctioneerFrame:OnEvent"]`), so
/// the traceback is walked from the raise site down, and a `LoadAddOn` frame before any frame in
/// the surveyed addon's folder makes it the loaded addon's raise. The surveyed addon's own inline
/// handler fired inside such a load is exempted too.
fn raised_inside_a_demand_load(err: &str, mine: &str) -> bool {
    for line in err.lines().skip(1).map(str::to_ascii_lowercase) {
        if line.contains(&format!("\\{mine}\\")) || line.contains(&format!("{mine}/")) {
            return false; // our own frame is above the boundary: our raise
        }
        if line.contains("in function 'loadaddon'") {
            return true;
        }
    }
    false
}

/// Invoke the stock UI entry points corpus addons replace, so their overrides execute: the survey
/// otherwise never clicks anything, and an override never invoked looks like one that works.
///
/// | entry point | who replaces it |
/// |---|---|
/// | `ToggleBackpack` | Bagnon (`Bagnon_Core/core/Overrides.lua`) |
/// | `UnitFrame_OnEnter`/`OnLeave` | TipBuddy (`TipBuddy.lua:2770`) |
/// | `ActionButton_Update` | zBar, zBarEx, CT_BarMod |
///
/// Each call is guarded on the global existing and wrapped so a raise is recorded. It proves the
/// body ran, not that its result is right.
fn drive_ui_probe(script: &mut UiScript) -> Vec<String> {
    let before = script.errors().len();
    // `this` is set for the `this`-shaped entry points, which read it.
    let _ = script.run(
        r#"
        -- The pcall CAPTURES the error; it does not discard it. Written the obvious way first
        -- (`pcall(fn)` alone) this probe could never report anything at all — it drove the
        -- overrides correctly and swallowed every raise, so the corpus showed a spotless column
        -- forever. `the_ui_probe_records_what_an_override_raises` is the test that caught it, and
        -- it exists because a silent instrument is worse than no instrument.
        BENILLA_PROBE_ERRORS = {}
        local function try(fn)
            if type(fn) == "function" then
                local ok, err = pcall(fn)
                if not ok then
                    table.insert(BENILLA_PROBE_ERRORS, tostring(err))
                end
            end
        end
        -- Open, then close: a toggle left open would leak state into nothing here, but the second
        -- call is what exercises an override's close path.
        try(ToggleBackpack); try(ToggleBackpack)
        if PlayerFrame then
            this = PlayerFrame
            try(UnitFrame_OnEnter); try(UnitFrame_OnLeave)
        end
        if ActionButton1 then
            this = ActionButton1
            try(ActionButton_Update)
        end
        -- HOVER. Nothing above puts a tooltip on screen, and a whole class of addon only runs
        -- there: hooks on GameTooltip's OnShow/OnHide, the FrameXML globals an addon replaces
        -- (GameTooltip_SetDefaultAnchor), and the ones that SCRAPE the line regions
        -- (`GameTooltipTextLeft1:GetText()`). Decision 1220 fixed a raise squarely in that class
        -- and this column could not see it, which is the reason this block exists.
        --
        -- Each call is guarded on the global being a FUNCTION, not merely non-nil: an unguarded
        -- call to a missing FrameXML global would land in every addon's probe column as that
        -- addon's fault, which is exactly the mis-attribution 1209 was written about.
        if GameTooltip and UIParent then
            if type(GameTooltip_SetDefaultAnchor) == "function" then
                try(function() GameTooltip_SetDefaultAnchor(GameTooltip, UIParent) end)
            end
            try(function()
                GameTooltip:SetOwner(UIParent, "ANCHOR_NONE")
                GameTooltip:SetText("benilla probe")
                GameTooltip:Show()
            end)
            try(function() GameTooltip:Hide() end)
        end
        this = nil
    "#,
    );
    let mut out = script.errors().split_off(before);
    // The captured raises, in order.
    if let Ok(n) = script.eval::<i64>("return table.getn(BENILLA_PROBE_ERRORS)") {
        for i in 1..=n {
            if let Ok(e) = script.eval::<String>(&format!("return BENILLA_PROBE_ERRORS[{i}]")) {
                out.push(e);
            }
        }
    }
    out
}

/// Load an addon's declared dependencies into the VM, depth-first, each at most once.
///
/// The 1.12 client's order (`AddOn_Load`, `0x51f240`): optional dependencies first, failures
/// ignored; required ones next, a failure aborting the dependent; then its own files. The abort is
/// not reproduced, only the state the dependent's file scope meets; a missing required dependency
/// is reported ([`AddonReport::missing_deps`]), an absent optional one is silent. Errors inside a
/// dependency are its own row, never the dependent's.
fn load_dependencies(
    script: &mut UiScript,
    root: &Path,
    toc: &Toc,
    installed: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
    loaded: &mut Vec<LoadedDep>,
) {
    // Optional first, then required: `FuBar_BagFu` lists `FuBarPlugin-2.0.lua` before
    // `AceLibrary.lua`, and FuBarPlugin raises unless the optional `Ace2` loaded first.
    let deps: Vec<&str> = toc
        .optional_dependencies()
        .into_iter()
        .chain(toc.dependencies())
        .collect();
    for dep in deps {
        let key = dep.to_ascii_lowercase();
        if !installed.contains(&key) || !seen.insert(key) {
            continue; // not installed (already reported), or already in this VM
        }
        let Some(dep_toc) = manifest_path(root, dep)
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| Toc::parse(&benilla_ui::source::decode(&b)))
        else {
            continue;
        };
        load_dependencies(script, root, &dep_toc, installed, seen, loaded);
        let files = load_addon_files(script, root, dep, &dep_toc);
        // `ADDON_LOADED` fires here, at the 1.12 client's position: `AddOn_Load` (`0x51f240`) runs
        // an addon's dependencies, its `.toc` files (`0x51f3fa`), `Bindings.xml`, the two
        // SavedVariables chunks, then `ADDON_LOADED` (event 429, `0x51f5ad`), before the next
        // addon. The loaded flag is set first (`[rec+0x18]=1` at `0x51f313`), so a handler's
        // `IsAddOnLoaded` is true. Fired outside the surveyed addon's error mark: a library raising
        // in its initialiser is its own row.
        script.mark_addon_loaded(dep);
        script.fire_event(
            "ADDON_LOADED",
            vec![benilla_ui::script::ScriptValue::Str(dep.to_string())],
        );
        loaded.push(LoadedDep {
            name: dep.to_string(),
            files,
        });
    }
}

/// One dependency the walk pulled in, and what its own files did; [`together`] needs the failures,
/// since a library loads once in its shared VM, under whichever dependent reached it first.
struct LoadedDep {
    name: String,
    files: FileLoad,
}

/// Template names the addon passes to `CreateFrame` that the VM cannot resolve.
///
/// Scanned, not traced (the calls run in handlers a load-time survey never fires) and not
/// comment-stripped (a commented call counts). The fourth argument resolves as a string literal or
/// an identifier bound to one in the same file (`local tpl = "ContainerFrameItemButtonTemplate"`);
/// a concatenated name, a function argument, a table read or a binding in another file is not seen.
fn missing_templates(script: &UiScript, root: &Path, name: &str, toc: &Toc) -> Vec<String> {
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    for path in source_files(root, name, toc) {
        let Some(text) = read_text(root, &path) else {
            continue;
        };
        // `ident = "literal"` bindings in this file; a name bound to several literals keeps all of
        // them.
        let mut bound: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (i, _) in text.match_indices('=') {
            // Not `==`, `~=`, `<=`, `>=`.
            if text[i + 1..].starts_with('=')
                || text[..i].ends_with(['=', '~', '<', '>'])
                || text[i + 1..].trim_start().is_empty()
            {
                continue;
            }
            let lhs = text[..i].trim_end();
            let lhs = lhs.rsplit(['\n', ';', ' ', '\t']).next().unwrap_or("");
            if !is_ident(lhs) {
                continue;
            }
            let rhs = text[i + 1..].trim_start();
            let Some(q) = rhs.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let Some(end) = rhs[1..].find(q) else {
                continue;
            };
            let lit = &rhs[1..end + 1];
            // Disqualified on a binary operator after the literal (on a Lua string, almost only
            // `..`), so `local built = "A" .. "B"` binds nothing while any statement end still lets
            // it through.
            let tail = rhs[end + 2..].trim_start_matches([' ', '\t']);
            let part_of_expression = tail.starts_with("..")
                || tail.starts_with(['+', '-', '*', '/', '%', '^', '<', '>'])
                || tail.starts_with("==")
                || tail.starts_with("~=")
                || tail.starts_with("and ")
                || tail.starts_with("or ");
            let whole = !part_of_expression;
            if !lit.is_empty() && whole {
                bound.entry(lhs).or_default().push(lit);
            }
        }
        for (i, _) in text.match_indices("CreateFrame") {
            let rest = &text[i + "CreateFrame".len()..];
            let Some(open) = rest.find('(') else { continue };
            let Some(close) = rest[open..].find(')') else {
                continue;
            };
            // Split the argument list at depth 0 only: a plain split cuts inside a nested call such
            // as AceGUI's
            // `CreateFrame("ScrollFrame", format("%s@%s@%s", Type, "ScrollFrame", …), …)`.
            let args = &rest[open + 1..open + close];
            let mut depth = 0i32;
            let mut quote: Option<char> = None;
            let mut fields: Vec<&str> = Vec::new();
            let mut start = 0usize;
            for (bi, c) in args.char_indices() {
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, '"' | '\'') => quote = Some(c),
                    (None, '(' | '[' | '{') => depth += 1,
                    (None, ')' | ']' | '}') => depth -= 1,
                    (None, ',') if depth == 0 => {
                        fields.push(&args[start..bi]);
                        start = bi + 1;
                    }
                    _ => {}
                }
            }
            fields.push(&args[start..]);
            let Some(fourth) = fields.get(3) else {
                continue;
            };
            let f = fourth.trim();
            // A quoted literal, either quote style, taken whole: 1.12 looks up the string it was
            // handed, once.
            let lit = f
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| f.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')));
            if let Some(lit) = lit.filter(|s| !s.is_empty()) {
                wanted.insert(lit.to_string());
            } else if let Some(lits) = bound.get(f) {
                // A bare identifier bound to a literal in this file.
                wanted.extend(lits.iter().map(|s| (*s).to_string()));
            }
        }
    }
    wanted
        .into_iter()
        .filter(|name| !script.has_framexml_template(name))
        .collect()
}

/// Template names the addon names in an XML `inherits=` that the VM cannot resolve, read as a plain
/// attribute over the addon's XML only. A name registered as a font resolves (`inherits=` spans
/// both namespaces), and templates the addon declares itself are already registered, since this
/// runs after its files load.
fn missing_inherits(script: &UiScript, root: &Path, name: &str, toc: &Toc) -> Vec<String> {
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    for path in source_files(root, name, toc) {
        if !path.to_ascii_lowercase().ends_with(".xml") {
            continue;
        }
        let Some(text) = read_text(root, &path) else {
            continue;
        };
        for (i, _) in text.match_indices("inherits=") {
            let rest = &text[i + "inherits=".len()..];
            let quote = match rest.chars().next() {
                Some(q @ ('"' | '\'')) => q,
                _ => continue,
            };
            let Some(end) = rest[1..].find(quote) else {
                continue;
            };
            // One name, verbatim: the loader looks up the whole attribute value, commas and spaces
            // included.
            wanted.extend(
                [rest[1..1 + end].to_string()]
                    .into_iter()
                    .filter(|s| !s.is_empty()),
            );
        }
    }
    wanted
        .into_iter()
        .filter(|n| !script.has_framexml_template(n) && !script.has_font_object(n))
        .collect()
}

/// Every source file an addon reaches: its manifest entries plus the `<Script file=>`/`<Include>`
/// tree hanging off them, where much of an addon's Lua lives.
fn source_files(root: &Path, name: &str, toc: &Toc) -> Vec<String> {
    let base = addon_base(name);
    let mut pending: Vec<String> = toc
        .files
        .iter()
        .map(|f| benilla_ui::loader::join_ref(&base, f))
        .collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    while let Some(path) = pending.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        if let Some(text) = read_text(root, &path) {
            let base = path.rfind('/').map_or("", |i| &path[..i]);
            for m in refs_in_xml(&text) {
                pending.push(benilla_ui::loader::join_ref(base, &m));
            }
        }
        out.push(path);
    }
    out
}

/// The FrameXML digest of the interface this survey loaded; print it beside every number. In a dev
/// build the manifest and our files are read from the source tree, so an edit moves the headline
/// without a rebuild. A chain entry contributes its name only, so migrating a window changes the
/// digest and the player's install does not.
pub fn framexml_digest() -> String {
    crate::ui_script::framexml_digest()
}

/// Every name our VM publishes into `_G`, with its Lua type, with no addons loaded: our side of the
/// diff against the 1.12 client's in-world `_G` (`reference/1.12-globals.tsv`). Dumped in Lua,
/// since `_G` is what an addon sees.
pub fn surface() -> Vec<(String, String)> {
    let Ok(mut script) = UiScript::new() else {
        return Vec::new();
    };
    script.set_instruction_budget(ADDON_INSTRUCTION_BUDGET);
    script.set_screen_size(1024.0, 768.0);
    script.register_addons(Vec::new(), None, None, None);
    seat_a_session(&mut script);
    let _ = crate::ui_script::load_default_ui(&script);

    let dump: String = script
        .eval(
            r#"
            local out = {}
            for k, v in pairs(_G) do
              if type(k) == "string" then
                out[table.getn(out) + 1] = k .. "\t" .. type(v)
              end
            end
            table.sort(out)
            return table.concat(out, "\n")
        "#,
        )
        .unwrap_or_default();

    dump.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect()
}

/// Whether the survey is seated with the real `GlobalStrings.lua` off the install's patch chain
/// (about 5,000 globals such as `FACTION_ALLIANCE` and every `ERR_*`, read at file scope). Without
/// an install the survey still runs on a worse VM, and this says which mode a run was in.
pub fn seated_with_global_strings() -> bool {
    global_strings().is_some()
}

/// The real `GlobalStrings.lua`, read once per process, or `None` with no install.
fn global_strings() -> Option<&'static str> {
    use std::sync::OnceLock;
    static SRC: OnceLock<Option<String>> = OnceLock::new();
    SRC.get_or_init(|| {
        let data = benilla_formats::wow_data()?;
        let mut chain = benilla_formats::open_chain(&data).ok()?;
        let bytes = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .ok()?;
        Some(benilla_ui::source::decode(&bytes).into_owned())
    })
    .as_deref()
}

/// Put a player and a realm in the VM before the addon loads: the 1.12 client runs `AddOn_Load`
/// from `UI_Init`, after the world is entered, so an addon's file scope always sees a real
/// character (AceDB-2.0 opens with
/// `string.format(PLAYER_OF_REALM, UnitName("player"), GetRealmName())`).
///
/// Every seat below is a state an in-world character is always in, kept minimal and ordinary: a VM
/// answering nil or zero there would fail addons on a state the 1.12 client cannot be in.
fn seat_a_session(script: &mut UiScript) {
    // The 1.12 client boots FrameXML with this file first; so does our app.
    if let Some(src) = global_strings() {
        let _ = script.run(src);
    }
    // The shipped CVar table, before the realm below writes into it.
    script.register_cvars(crate::cvars::registered_pairs());
    // A display: `GetScreenResolutions()` empty and `GetCurrentResolution()` 0 is no state the 1.12
    // client can be in (`CT_Viewport` indexes its screen size by it at load). One mode, the
    // `set_screen_size(1024, 768)` the interface is already told.
    script.set_screen_resolutions(
        vec![benilla_ui::script::ScreenResolution {
            width: 1024,
            height: 768,
        }],
        Some(benilla_ui::script::ScreenResolution {
            width: 1024,
            height: 768,
        }),
    );
    // What this run's device offers, as `crate::cvars` pushes it in the app.
    script.set_video_caps(benilla_ui::script::VideoCaps {
        anisotropic: true,
        pixel_shaders: true,
        vertex_shaders: true,
        trilinear: true,
        triple_buffering: false,
        max_anisotropy: *benilla_assets::ANISO_RANGE.end(),
        hardware_cursor: true,
    });
    script.set_realm_name("Harness");
    // The bind point: the server sends `SMSG_BINDPOINTUPDATE` at login, before any addon runs.
    script.set_bind_location("Stormwind City");
    script.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Harness".into()),
            health: 100,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 100,
            max_power: 100,
            race: Some("Human".into()),
            race_file: Some("Human".into()),
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            sex: 2,
            is_player: true,
            // `UnitFactionGroup("player")`: every playable race has a side, and AceDB-2.0
            // concatenates it into its per-realm key at file scope.
            faction_group: Some("Alliance".into()),
            // `0x5efe00`'s team digit for Human (race 1), the rank-title key's second `%d`.
            pvp_team: crate::ui_unit::race_pvp_team(1),
            ..Default::default()
        }),
    );

    // ── A populated world ──
    //
    // One buff, one target and one action with a running cooldown, so an addon that only draws when
    // there is content (a buff bar, a target frame, cooldown text) is separated from one that
    // failed.
    script.set_auras(
        "player",
        Some(vec![benilla_ui::script::AuraState {
            spell_id: 1243,
            name: Some("Power Word: Fortitude".into()),
            icon: Some("Interface\\Icons\\Spell_Holy_WordFortitude".into()),
            count: 1,
            helpful: true,
            cancelable: true,
            ..Default::default()
        }]),
    );
    script.set_unit(
        "target",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Target Dummy".into()),
            health: 80,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 50,
            max_power: 100,
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            ..Default::default()
        }),
    );
    script.set_action(
        1,
        Some(benilla_ui::script::ActionSlot {
            texture: Some("Interface\\Icons\\Ability_SteelMelee".into()),
            kind: 0x00,
            action: 100,
            count: 0,
            consumable: false,
        }),
    );
    script.set_action_state(
        1,
        Some(benilla_ui::script::ActionState {
            usable: true,
            // (startTime ms, duration ms, enable): a cooldown with time left, which cooldown-text
            // addons need.
            cooldown: Some((1, 30_000, true)),
            ..Default::default()
        }),
    );
    // A spellbook (seated below) and a quest log. An empty book fails `TheoryCraftEngine.lua:306`
    // (`for i=1, numSpells` off `GetSpellTabInfo(1)`); two tabs, four spells, `Attack` in slot 1.
    // The quest log is a zone header row (`isHeader`), an in-progress quest with one objective done
    // and one not, and a complete one (`complete = 1`); failed (-1) and timed quests are not
    // seated.
    {
        use benilla_ui::script::{QuestLogEntryView, QuestLogObjectiveView, QuestLogState};
        let objective = |text: &str, cur: u32, req: u32| QuestLogObjectiveView {
            text: text.into(),
            kind: "monster".into(),
            finished: cur >= req,
            cur,
            req,
        };
        let header = QuestLogEntryView {
            quest_id: 0,
            title: "Elwynn Forest".into(),
            is_header: true,
            ..Default::default()
        };
        let in_progress = QuestLogEntryView {
            quest_id: 62,
            title: "The Fargodeep Mine".into(),
            level: 10,
            objectives: vec![
                objective("Kobold Miner slain: 8/12", 8, 12),
                objective("Kobold Vermin slain: 6/6", 6, 6),
            ],
            ..Default::default()
        };
        let done = QuestLogEntryView {
            quest_id: 176,
            title: "Kobold Candles".into(),
            level: 8,
            complete: 1,
            ..Default::default()
        };
        script.set_quest_log(QuestLogState {
            entries: vec![header, in_progress, done],
            num_quests: 2,
            hidden_quest_ids: Vec::new(),
        });
    }

    // The purse: 12_345_678 copper (1234g 56s 78c), every denomination non-zero, so a coin
    // formatter that drops a field or divides in the wrong order shows it.
    script.set_money(12_345_678);

    // Equipped gear: head, chest and main hand, enough for a slot walk to meet items, empty slots
    // and no ammo.
    {
        let mut slots: benilla_ui::script::InventorySlots = Default::default();
        slots[1] = Some(equip_slot(12640, "Lionheart Helm"));
        slots[5] = Some(equip_slot(11726, "Bloodmail Hauberk"));
        slots[16] = Some(equip_slot(871, "Flurry Axe"));
        script.set_inventory_slots(slots);
    }
    // The backpack: bag 0 is 16 slots from level 1. Bags 1..4 are equipped bags, which a fresh
    // character does not have, so they are not seated.
    {
        let mut slots = std::collections::HashMap::new();
        slots.insert(1, bag_slot(6948, "Hearthstone", 1));
        slots.insert(5, bag_slot(2589, "Linen Cloth", 12));
        script.set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                name: Some("Backpack".into()),
                num_slots: 16,
                slots,
            }),
        );
    }
    script.set_spellbook(benilla_ui::script::SpellBookState {
        tabs: vec![
            benilla_ui::script::SpellTabView {
                name: "General".into(),
                texture: Some("Interface\\Icons\\INV_Misc_QuestionMark".into()),
                offset: 0,
                num_spells: 2,
            },
            benilla_ui::script::SpellTabView {
                name: "Arms".into(),
                texture: Some("Interface\\Icons\\Ability_Rogue_Eviscerate".into()),
                offset: 2,
                num_spells: 2,
            },
        ],
        slots: vec![
            spell_slot(6603, "Attack", None),
            spell_slot(78, "Heroic Strike", Some("Rank 1")),
            spell_slot(100, "Charge", Some("Rank 1")),
            spell_slot(772, "Rend", Some("Rank 1")),
        ],
    });

    // ── The login-scoped catalogues ──
    //
    // Answered from DBCs with no session behind them, off the player's own chain. The
    // `SMSG_ADDON_INFO` reply is seated empty: the Lua index space (`GetNumAddOns`, the index form
    // of every AddOn verb) is built only when the server answers, which an in-world client always
    // had, and no corpus addon is `## Secure:`. The auction class tree (`ItemClass.dbc` in the
    // browse menu's order) is read at file scope, as by `Auctioneer`'s `AucCore.lua:105`.
    script.note_addon_info_reply(&[]);
    script.set_auction_item_classes(auction_item_classes());

    // The talent tree (`Talent.dbc` x `TalentTab.dbc`), read by position at file scope
    // (`KTM_Data.lua:376`). Zero ranks and the full pool, a fresh 60 (51 points, `level - 9`), so a
    // rank is a number rather than nil.
    if let Some(state) = talent_snapshot(script) {
        script.set_talents(state);
    }
}

/// The 1.12 auction browse tree, read once off the player's chain by the builder the live client
/// uses (`ui_auction::categories`); empty with no install.
fn auction_item_classes() -> Vec<benilla_ui::script::AuctionCategory> {
    use std::sync::OnceLock;
    static TREE: OnceLock<Vec<benilla_ui::script::AuctionCategory>> = OnceLock::new();
    TREE.get_or_init(|| {
        let Some(data) = benilla_formats::wow_data() else {
            return Vec::new();
        };
        let Ok(mut chain) = benilla_formats::open_chain(&data) else {
            return Vec::new();
        };
        let classes = benilla_formats::load_item_classes(&mut chain)
            .ok()
            .map(crate::ui_items::ItemClasses);
        let subclasses = benilla_formats::load_item_sub_classes(&mut chain)
            .ok()
            .map(crate::ui_items::ItemSubClasses);
        crate::ui_auction::categories(classes.as_ref(), subclasses.as_ref())
    })
    .clone()
}

/// The seated character's talent pages, built by the live client's `build_pages`. The DBCs are read
/// once per process; the pages are built per VM, since the requirement lines come from that VM's
/// `GlobalStrings.lua`. `None` with no install.
fn talent_snapshot(script: &UiScript) -> Option<benilla_ui::script::TalentUiState> {
    use std::sync::OnceLock;
    #[allow(clippy::type_complexity)]
    static DBCS: OnceLock<
        Option<(
            benilla_formats::TalentCatalog,
            benilla_formats::SpellCatalog,
        )>,
    > = OnceLock::new();
    let (talents, spells) = DBCS
        .get_or_init(|| {
            let data = benilla_formats::wow_data()?;
            let mut chain = benilla_formats::open_chain(&data).ok()?;
            let talents = benilla_formats::load_talent_catalog(&mut chain).ok()?;
            let spells = benilla_formats::load_spell_catalog(&mut chain).ok()?;
            Some((talents, spells))
        })
        .as_ref()?;
    let get = |key: &str| {
        script
            .lua()
            .globals()
            .get::<String>(key)
            .ok()
            .filter(|t| !t.is_empty())
    };
    // Human (`ChrRaces.dbc` 1) warrior (`ChrClasses.dbc` 1), the character `seat_a_session` seats.
    Some(crate::ui_talent::build_pages(
        talents,
        &BTreeSet::new(),
        spells,
        1,
        1,
        (51, 0),
        &get,
    ))
}

/// One seated equipment slot, at full durability.
fn equip_slot(item_id: u32, name: &str) -> benilla_ui::script::InvSlotView {
    benilla_ui::script::InvSlotView {
        item_id,
        icon: Some("Interface\\Icons\\INV_Misc_QuestionMark".into()),
        count: 1,
        quality: 2,
        name: Some(name.to_string()),
        // Full durability: the setter fires `UPDATE_INVENTORY_ALERTS`, and a worn item would light
        // DurabilityFrame.
        durability: Some((100, 100)),
        link: Some(format!("|cff1eff00|Hitem:{item_id}:0:0:0|h[{name}]|h|r")),
        ..Default::default()
    }
}

/// One seated backpack slot.
fn bag_slot(item_id: u32, name: &str, count: u32) -> benilla_ui::script::ContainerSlot {
    benilla_ui::script::ContainerSlot {
        texture: Some("Interface\\Icons\\INV_Misc_QuestionMark".into()),
        count,
        quality: Some(1),
        item_id,
        link: Some(format!("|cffffffff|Hitem:{item_id}:0:0:0|h[{name}]|h|r")),
        ..Default::default()
    }
}

/// One seated spellbook slot: the fields a book reader reads, defaulted otherwise.
fn spell_slot(spell_id: u32, name: &str, rank: Option<&str>) -> benilla_ui::script::SpellSlotView {
    benilla_ui::script::SpellSlotView {
        spell_id,
        name: name.to_string(),
        rank: rank.map(str::to_string),
        texture: Some("Interface\\Icons\\INV_Misc_QuestionMark".into()),
        ..Default::default()
    }
}

/// Every string key in the VM's `_G`.
fn globals_of(script: &UiScript) -> BTreeSet<String> {
    script
        .eval::<Vec<String>>(
            "local out = {} \
             for k in pairs(_G) do if type(k) == 'string' then table.insert(out, k) end end \
             return out",
        )
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// What a manifest entry that does not resolve is (see [`AddonReport::absent_own_files`]).
#[derive(Debug, Default)]
pub struct AbsentFiles {
    /// Entries under the addon's own folder that the package does not contain.
    pub own: Vec<String>,
    /// Entries resolving outside it, into a folder that is not installed.
    pub foreign: Vec<String>,
}

/// What one addon's manifest did: every load failure, which were absent files, and whether anything
/// raised.
struct FileLoad {
    /// Every load failure, in order, unchanged.
    errors: Vec<String>,
    /// The subset of [`Self::errors`] that names a file the provider does not have, split by whose
    /// package is incomplete.
    absent: AbsentFiles,
    /// Anything raised, failed to parse or dropped a frame ([`AddonReport::loaded`]). False when
    /// the only failures are absent files: `0x6edaa0` logs `"Couldn't open %s"`, returns null, and
    /// the walk carries on.
    raised: bool,
}

/// Run the addon's manifest through the loader's two arms, `.lua` as a chunk and anything else as
/// FrameXML, in the install-relative path space (`Interface/AddOns/<Folder>`, as
/// `ui_script::addons::Addon::prefix`), so chunk names match a live session's.
fn load_addon_files(script: &UiScript, root: &Path, name: &str, toc: &Toc) -> FileLoad {
    let provider = |req: &str| -> Option<Vec<u8>> { read_under(root, req) };
    let base = addon_base(name);
    let mut errors = Vec::new();
    let mut absent = AbsentFiles::default();
    let mut raised = false;
    for file in &toc.files {
        let path = benilla_ui::loader::join_ref(&base, file);
        let Some(bytes) = read_under(root, &path) else {
            errors.push(format!("{file}: not found"));
            // Whose package is incomplete: `join_ref` has collapsed the `..`s as the 1.12 client
            // does (`0x6ede10`), so a path still inside the addon's folder is its own manifest
            // unsatisfied, and one outside it wants a neighbour.
            let own = path
                .strip_prefix(base.as_str())
                .is_some_and(|rest| rest.starts_with('/'));
            if own {
                absent.own.push(file.clone());
            } else {
                absent.foreign.push(path.clone());
            }
            continue;
        };
        if is_lua(file) {
            // Named as the 1.12 client names it (`@Interface\AddOns\<Folder>\<File>`, from the
            // resolved path, as the loader names `<Script file=>`): the FuBar family parses
            // tracebacks for its own folder.
            if let Err(e) = script.run_chunk_named(&bytes, &format!("@{}", path.replace('/', "\\")))
            {
                errors.push(format!("{file}: {e}"));
                raised = true;
            }
            continue;
        }
        match benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes)) {
            Ok(doc) => {
                let report = benilla_ui::loader::load_in(script, &doc, &path, &provider);
                // Retained with the file prefix, so the `warnings` column sees them.
                for w in report.warnings {
                    script.report_warning(&format!("{file}: {w}"));
                }
                // A file the document named that the provider lacks: recorded as absent, own or
                // foreign, never as raised; the 1.12 client logs `Couldn't open %s` for an
                // `<Include>` and `Error loading %s` for a `<Script file=>`, and carries on.
                for m in report.missing_files {
                    let own = m
                        .split_once("no provider hit for \"")
                        .and_then(|(_, rest)| rest.split_once('"'))
                        .is_some_and(|(p, _)| {
                            p.strip_prefix(base.as_str())
                                .is_some_and(|r| r.starts_with('/'))
                        });
                    errors.push(format!("{file}: {m}"));
                    if own {
                        absent.own.push(format!("{file}: {m}"));
                    } else {
                        absent.foreign.push(format!("{file}: {m}"));
                    }
                }
                raised |= !report.errors.is_empty();
                errors.extend(report.errors.into_iter().map(|e| format!("{file}: {e}")));
            }
            Err(e) => {
                errors.push(format!("{file}: {e}"));
                raised = true;
            }
        }
    }
    FileLoad {
        errors,
        absent,
        raised,
    }
}

fn is_lua(entry: &str) -> bool {
    entry
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(entry)
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("lua"))
}

/// One addon file as bytes, through the client's own reader: the loose AddOns tree, then the
/// player's patch chain ([`crate::ui_script::addons::read_addon_file`]).
fn read_under(root: &Path, rel: &str) -> Option<Vec<u8>> {
    crate::ui_script::addons::read_addon_file(root, rel)
}

/// Where one addon's files sit in the install's path space, `Interface/AddOns/<Folder>`; must match
/// [`crate::ui_script::addons::Addon::prefix`].
fn addon_base(name: &str) -> String {
    format!("Interface/AddOns/{name}")
}

/// [`read_under`] as text, for the source scanner.
fn read_text(root: &Path, rel: &str) -> Option<String> {
    read_under(root, rel).map(|b| benilla_ui::source::decode(&b).into_owned())
}

/// Names the addon calls like functions that the VM does not have, minus its own definitions and
/// its locals (`local function Foo` and `local Foo = …`, capitalised or not).
fn missing_calls(
    root: &Path,
    name: &str,
    toc: &Toc,
    known: &BTreeSet<String>,
    dep_methods: &BTreeSet<String>,
) -> Wants {
    let mut scan = Scan::default();
    for path in source_files(root, name, toc) {
        if let Some(text) = read_text(root, &path) {
            scan_source(&path, &text, &mut scan);
        }
    }
    let Scan {
        called,
        indexed,
        methods,
        defined,
        defined_methods,
        tested_fields,
        kind_calls,
        global_calls,
        loose_methods,
    } = scan;
    let absent = |set: BTreeSet<String>| -> Vec<String> {
        set.into_iter()
            .filter(|n| !known.contains(n) && !defined.contains(n))
            .collect()
    };
    // Method candidates stay raw: `known` holds globals, so only the VM ([`widget_method_kinds`])
    // can answer a method. Subtracted here are the names the addon or its dependencies define; the
    // feature-tested ones are split off, not dropped.
    let (wanted_methods, tested_methods): (BTreeSet<String>, BTreeSet<String>) = methods
        .into_iter()
        .filter(|n| !defined_methods.contains(n) && !dep_methods.contains(n))
        .partition(|n| !tested_fields.contains(n));
    // Three queues: a missing function is a verb to write, a missing frame or table is FrameXML to
    // transcribe, a missing method is a widget binding.
    Wants {
        missing_globals: absent(called),
        missing_tables: absent(indexed),
        wanted_methods,
        tested_methods,
        kind_calls,
        global_calls,
        loose_methods,
    }
}

/// What one addon's source scan wants.
struct Wants {
    missing_globals: Vec<String>,
    missing_tables: Vec<String>,
    /// Method names the addon calls and does not define: the census's question set.
    wanted_methods: BTreeSet<String>,
    /// ...and those it feature-tests first, which are not blockers.
    tested_methods: BTreeSet<String>,
    kind_calls: BTreeSet<(String, String)>,
    global_calls: BTreeSet<(String, String)>,
    loose_methods: BTreeSet<String>,
}

/// `<Script file=>` / `<Include file=>` targets.
fn refs_in_xml(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices("file=\"") {
        let rest = &text[i + 6..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
        }
    }
    out
}

/// Blank out `<!-- … -->`, keeping newlines; run over an XML file before [`strip_lua_noise`], which
/// cannot see them. An unterminated `<!--` swallows the rest of the file, as an XML parser does.
fn strip_xml_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("<!--") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 4..];
        match after.find("-->") {
            Some(close) => {
                // Keep the newlines so a reported line is still a real one.
                out.extend(after[..close].chars().filter(|c| *c == '\n'));
                rest = &after[close + 3..];
            }
            None => {
                out.extend(after.chars().filter(|c| *c == '\n'));
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Blank out Lua comments and string literals, keeping newlines, so words in credits comments and
/// messages are not read as call targets.
fn strip_lua_noise(text: &str) -> String {
    strip_lua(text, false)
}

/// [`strip_lua_noise`] with string contents kept, comments still gone: the kind in
/// `CreateFrame("MessageFrame", …)` is inside a literal. Never used for the call-site scan
/// ([`scan_lua`], [`attribute_calls`]), where a name inside a string would read as an identifier.
fn strip_lua_comments_only(text: &str) -> String {
    strip_lua(text, true)
}

fn strip_lua(text: &str, keep_strings: bool) -> String {
    let src: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        // Long bracket `[[ … ]]` (a string, or a `--[[ … ]]` comment): both end the same way.
        let long_open = c == '[' && i + 1 < src.len() && src[i + 1] == '[';
        let line_comment = c == '-' && i + 1 < src.len() && src[i + 1] == '-';
        if line_comment && i + 3 < src.len() && src[i + 2] == '[' && src[i + 3] == '[' {
            i += 4;
            while i + 1 < src.len() && !(src[i] == ']' && src[i + 1] == ']') {
                if src[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i = (i + 2).min(src.len());
            continue;
        }
        if line_comment {
            while i < src.len() && src[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if long_open {
            i += 2;
            if keep_strings {
                out.push_str("[[");
            }
            while i + 1 < src.len() && !(src[i] == ']' && src[i + 1] == ']') {
                if keep_strings || src[i] == '\n' {
                    out.push(src[i]);
                }
                i += 1;
            }
            i = (i + 2).min(src.len());
            if keep_strings {
                out.push_str("]]");
            }
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let start = i;
            while i < src.len() && src[i] != quote {
                if src[i] == '\\' {
                    i += 1; // an escaped quote does not close the literal
                }
                i += 1;
            }
            if keep_strings {
                out.push(quote);
                out.extend(src[start..i.min(src.len())].iter());
                out.push(quote);
            } else {
                out.push_str("\"\""); // keep it an expression, drop its contents
            }
            i = (i + 1).min(src.len());
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// [`scan_lua`] over one source file. An XML file is scanned as Lua (its `<Script>` and handler
/// bodies are Lua) after its `<!-- … -->` comments are blanked; its attribute values are `"…"`,
/// which the Lua string rule blanks.
fn scan_source(path: &str, text: &str, scan: &mut Scan) {
    let text: std::borrow::Cow<'_, str> = if is_lua(path) {
        std::borrow::Cow::Borrowed(text)
    } else {
        std::borrow::Cow::Owned(strip_xml_comments(text))
    };
    scan_lua(&text, scan);
    // The receiver pass is per file: a `local f` in one file says nothing about an `f` in the next.
    scan_receivers(&text, scan);
}

/// Every kind `CreateFrame` accepts, in `frame_kind_from_str`'s spelling: the census's probe set
/// and the vocabulary [`UiScript::widget_kind`] answers in. A refused kind is skipped by the
/// oracle.
const PROBE_FRAME_KINDS: &[&str] = &[
    "Frame",
    "Button",
    "CheckButton",
    "EditBox",
    "StatusBar",
    "Slider",
    "ScrollFrame",
    "Model",
    "PlayerModel",
    "MessageFrame",
    "ScrollingMessageFrame",
    "ColorSelect",
    "SimpleHTML",
    "MovieFrame",
    "GameTooltip",
    "Minimap",
];

/// Type the receiver of a `:` call where the file says what it is, for the per-kind census. Two
/// shapes are typed:
///
/// - a local or plain global bound from a widget factory (`local f = CreateFrame("Frame")`,
///   `f:CreateTexture()`, `f:CreateFontString()`);
/// - a published name, resolved later by [`UiScript::widget_kind`] against our own object graph,
///   which is where the addon's call really lands.
///
/// `self:Foo()`, `this:Foo()`, `a.b:Foo()` and `getglobal(n):Foo()` stay untyped and are reported
/// ([`AddonReport::ambiguous_methods`]).
fn scan_receivers(text: &str, scan: &mut Scan) {
    // Pass 1 needs the literal inside `CreateFrame("MessageFrame")`, which the ordinary stripper
    // blanks; pass 2 must not see identifiers inside strings.
    let typed = local_widget_kinds(&strip_lua_comments_only(text));
    attribute_calls(&strip_lua_noise(text), &typed, scan);
}

/// Identifiers this file binds to a widget of a known kind: `Some(kind)`, or `None` for a name
/// whose bindings disagree or that is ever a loop variable or function parameter.
fn local_widget_kinds(text: &str) -> BTreeMap<String, Option<&'static str>> {
    let mut out: BTreeMap<String, Option<&'static str>> = BTreeMap::new();
    let mut bind = |name: &str, kind: Option<&'static str>| match out.get(name) {
        Some(prev) if *prev == kind => {}
        Some(_) => {
            out.insert(name.to_string(), None);
        }
        None => {
            out.insert(name.to_string(), kind);
        }
    };
    for line in text.lines() {
        let Some(eq) = assignment_at(line) else {
            continue;
        };
        let lhs = line[..eq].trim();
        let lhs = lhs.strip_prefix("local ").unwrap_or(lhs).trim();
        if !is_ident(lhs) {
            continue; // a comma list, a dotted field or an index: not a name we can follow
        }
        bind(lhs, widget_kind_of_expression(&line[eq + 1..]));
    }
    // Over the whole file: a name ever bound by `for` or as a function parameter is untyped
    // wherever it is bound, so a file-top `local frame = CreateFrame("Frame")` does not type a
    // helper's `frame`.
    for name in binder_names(text) {
        bind(&name, None);
    }
    out
}

/// The byte offset of the line's first real `=` (never `==`, `<=`, `>=`, `~=`), or `None`.
fn assignment_at(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    (0..b.len()).find(|&i| {
        b[i] == b'='
            && b.get(i + 1) != Some(&b'=')
            && !matches!(
                i.checked_sub(1).map(|p| b[p]),
                Some(b'=' | b'<' | b'>' | b'~')
            )
    })
}

/// Every identifier the file binds through a `for` header or a `function` parameter list.
fn binder_names(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let b: Vec<char> = text.chars().collect();
    let word_at = |i: usize, w: &str| -> bool {
        let n = w.chars().count();
        i + n <= b.len()
            && b[i..i + n].iter().copied().eq(w.chars())
            && (i == 0 || !(b[i - 1].is_alphanumeric() || b[i - 1] == '_'))
            && b.get(i + n)
                .is_none_or(|c| !(c.is_alphanumeric() || *c == '_'))
    };
    let take = |out: &mut BTreeSet<String>, s: &str| {
        for name in s.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if is_ident(name) {
                out.insert(name.to_string());
            }
        }
    };
    for i in 0..b.len() {
        if word_at(i, "for") {
            // `for i = 1, n do` and `for k, v in pairs(t) do`: the header ends at the `=`/`in`.
            let rest: String = b[i + 3..].iter().take(200).collect();
            let head = rest
                .find(" in ")
                .map(|p| &rest[..p])
                .or_else(|| assignment_at(&rest).map(|p| &rest[..p]))
                .unwrap_or(&rest);
            take(&mut out, head);
        }
        if word_at(i, "function") {
            let rest: String = b[i + 8..].iter().take(400).collect();
            if let Some(open) = rest.find('(') {
                if let Some(close) = rest[open..].find(')') {
                    take(&mut out, &rest[open + 1..open + close]);
                }
            }
        }
    }
    out
}

/// The widget kind a right-hand side provably produces: a factory call with a literal kind, never
/// `CreateFrame(kind, …)` with a variable first argument.
fn widget_kind_of_expression(rhs: &str) -> Option<&'static str> {
    // The one pass that reads string literals, so a match inside one (`"CreateFrame('Button')"`)
    // types nothing; an odd count of quotes before the match is the test.
    let outside_string =
        |i: usize| rhs[..i].chars().filter(|c| *c == '"' || *c == '\'').count() % 2 == 0;
    if let Some(i) = rhs
        .match_indices("CreateFrame")
        .find(|(i, _)| outside_string(*i))
    {
        let i = i.0;
        // Not `lib.CreateFrame(...)`: a qualified call is another function of the same name.
        let qualified = rhs[..i].trim_end().ends_with(['.', ':']);
        let after = &rhs[i + "CreateFrame".len()..];
        if !qualified && after.trim_start().starts_with('(') {
            let arg = after[after.find('(')? + 1..].trim_start();
            let quote = arg.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let lit = &arg[1..arg[1..].find(quote)? + 1];
            return PROBE_FRAME_KINDS
                .iter()
                .find(|k| k.eq_ignore_ascii_case(lit))
                .copied();
        }
        return None;
    }
    // The two region leaves have factories of their own.
    for (call, kind) in [
        (":CreateTexture", "Texture"),
        (":CreateFontString", "FontString"),
    ] {
        if rhs.match_indices(call).any(|(i, _)| outside_string(i)) {
            return Some(kind);
        }
    }
    None
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

/// Walk `obj:Name(` call sites and file each under the receiver we can (or cannot) type.
fn attribute_calls(text: &str, typed: &BTreeMap<String, Option<&'static str>>, scan: &mut Scan) {
    let src: Vec<char> = text.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = 0;
    while i < src.len() {
        if src[i] != ':' || (i + 1 < src.len() && src[i + 1] == ':') {
            i += 1;
            continue;
        }
        // The method name, right of the colon.
        let mut m = i + 1;
        while m < src.len() && src[m] == ' ' {
            m += 1;
        }
        let start = m;
        while m < src.len() && is_word(src[m]) {
            m += 1;
        }
        let name: String = src[start..m].iter().collect();
        let mut p = m;
        while p < src.len() && src[p] == ' ' {
            p += 1;
        }
        // Same guards as `scan_lua`'s method arm: called, and capitalised.
        if name.is_empty()
            || p >= src.len()
            || src[p] != '('
            || !name.starts_with(|c: char| c.is_uppercase())
        {
            i += 1;
            continue;
        }
        // The receiver, left of the colon: a bare identifier only; `a.b:C()`, `getglobal(n):C()`
        // and `t[1]:C()` are not typable.
        let mut r = i;
        while r > 0 && src[r - 1] == ' ' {
            r -= 1;
        }
        let end = r;
        while r > 0 && is_word(src[r - 1]) {
            r -= 1;
        }
        let receiver: String = src[r..end].iter().collect();
        let plain =
            !receiver.is_empty() && (r == 0 || !matches!(src[r - 1], '.' | ':' | ']' | ')'));
        if !plain {
            scan.loose_methods.insert(name);
        } else {
            match typed.get(&receiver) {
                Some(Some(kind)) => {
                    scan.kind_calls.insert(((*kind).to_string(), name));
                }
                // The file rebinds this name, so it is untypable here and must not fall through to
                // the published-name lookup (a local called `Minimap` is not the global).
                Some(None) => {
                    scan.loose_methods.insert(name);
                }
                // Possibly a published widget; only the live arena knows.
                None => {
                    scan.global_calls.insert((receiver, name));
                }
            }
        }
        i = p;
    }
}

/// What one pass of [`scan_lua`] found.
#[derive(Default)]
struct Scan {
    /// `Foo(`: API-shaped call sites, for [`AddonReport::missing_globals`].
    called: BTreeSet<String>,
    /// `Foo.bar` / `Foo:baz`: names the addon indexes, for [`AddonReport::missing_tables`].
    indexed: BTreeSet<String>,
    /// `obj:Name(`: method calls, receiver unknown, for [`AddonReport::missing_methods`].
    methods: BTreeSet<String>,
    /// Globals the addon binds itself, in any of the shapes below.
    defined: BTreeSet<String>,
    /// Fields the addon binds itself (`function T:N`, `function T.N`, `T.N = …`): subtracting them
    /// keeps an embedded library's own `self:Foo()` calls out of the widget-method ranking.
    defined_methods: BTreeSet<String>,
    /// Fields the addon reads without calling (`if self.OnMouseUp then`): feature tests, not
    /// blockers.
    tested_fields: BTreeSet<String>,
    /// `(kind, method)`: call sites [`scan_receivers`] typed from a widget-factory binding in the
    /// file, for [`AddonReport::kind_missing_methods`].
    kind_calls: BTreeSet<(String, String)>,
    /// `(receiver name, method)`: a bare receiver the file does not bind, possibly a published
    /// widget ([`UiScript::widget_kind`]), resolved later.
    global_calls: BTreeSet<(String, String)>,
    /// Methods called on a receiver nothing can type (`self:Foo()`, `a.b:Foo()`,
    /// `getglobal(n):Foo()`), for [`AddonReport::ambiguous_methods`].
    loose_methods: BTreeSet<String>,
}

/// Collect API-shaped call sites and the file's own top-level definitions.
fn scan_lua(text: &str, scan: &mut Scan) {
    let Scan {
        called,
        indexed,
        methods,
        defined,
        defined_methods,
        tested_fields,
        ..
    } = scan;
    let text = &strip_lua_noise(text);
    let bytes: Vec<char> = text.chars().collect();
    let ident = |start: usize| -> (String, usize) {
        let mut i = start;
        while i < bytes.len() && (bytes[i].is_alphanumeric() || bytes[i] == '_') {
            i += 1;
        }
        (bytes[start..i].iter().collect(), i)
    };
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_alphabetic() || bytes[i] == '_' {
            // A qualified name (`self.foo`, `string.format`) is not a global call site.
            let prev = if i > 0 { bytes[i - 1] } else { '\0' };
            let qualified = prev == '.' || prev == ':';
            let (word, next) = ident(i);
            let mut j = next;
            while j < bytes.len() && bytes[j] == ' ' {
                j += 1;
            }
            let capitalised = word.chars().next().is_some_and(char::is_uppercase);
            if !qualified && j < bytes.len() && bytes[j] == '(' && capitalised {
                called.insert(word.clone());
            }
            // `obj:Name(`: a widget method. The receiver is ignored (a static scan cannot type it);
            // the leading capital is the filter, since every 1.12 widget method is capitalised.
            if prev == ':' && j < bytes.len() && bytes[j] == '(' && capitalised {
                methods.insert(word.clone());
            }
            if prev == '.' {
                let assigned = bytes.get(j) == Some(&'=') && bytes.get(j + 1) != Some(&'=');
                if assigned {
                    // `T.Name = …`: a field the addon binds, the corpus's other way of writing a
                    // method (`f.Update = function`).
                    defined_methods.insert(word.clone());
                } else if bytes.get(j) != Some(&'(') {
                    // A field read rather than called is a feature test, and a feature-tested
                    // method is not a blocker: the addon has its branch for when it is absent
                    // (`FuBarPlugin-2.0.lua:768`,
                    // `if type(self.OnMouseUp) == "function" then self:OnMouseUp(arg1) end`). A
                    // dotted call is neither.
                    tested_fields.insert(word.clone());
                }
            }
            // A name the addon indexes (`ColorPickerFrame.func = …`, `GameTooltip:AddLine(…)`): a
            // frame or table global it expects. Same guards and `defined` subtraction as the call
            // arm.
            if !qualified && j < bytes.len() && (bytes[j] == '.' || bytes[j] == ':') && capitalised
            {
                indexed.insert(word.clone());
            }
            if word == "function" {
                let mut k = next;
                while k < bytes.len() && bytes[k] == ' ' {
                    k += 1;
                }
                if k < bytes.len() && (bytes[k].is_alphabetic() || bytes[k] == '_') {
                    let (fname, mut end) = ident(k);
                    defined.insert(fname);
                    // `function T:N(`, `function T.N(`, `function A.B.C:D(`: every name after the
                    // first is a field on a table the addon has, not a global.
                    while end < bytes.len() && (bytes[end] == '.' || bytes[end] == ':') {
                        let (part, next_end) = ident(end + 1);
                        if part.is_empty() {
                            break;
                        }
                        defined_methods.insert(part);
                        end = next_end;
                    }
                }
            }
            i = next;
            continue;
        }
        i += 1;
    }
    // Assignments: `Foo = …` defines a global, and `local Foo = …` a local, which the capital
    // filter alone does not exclude (`local CheckShow = function` in `FuBarPlugin-2.0.lua`).
    // `defined` is addon-wide while a local is file-scoped, so a shadowed API name is hidden
    // addon-wide: an under-report bounded by the addon.
    for line in text.lines() {
        let t = line.trim_start();
        let body = t.strip_prefix("local ").unwrap_or(t);
        let Some(eq) = body.find('=') else { continue };
        let (lhs, rhs) = body.split_at(eq);
        for name in lhs.split(',') {
            let name = name.trim();
            if name.is_empty()
                || !name.chars().all(|c| c.is_alphanumeric() || c == '_')
                || name.chars().next().is_some_and(|c| c.is_ascii_digit())
            {
                continue;
            }
            // A self-localisation is not a definition: `local GetTime = GetTime` (and
            // `local a, b = a, b`, `local X = X or {}`) binds the local from the global, which is
            // therefore demanded.
            if rhs
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|tok| tok == name)
            {
                continue;
            }
            defined.insert(name.to_string());
        }
    }
}

/// How many addons want each missing global, most-wanted first.
pub fn demand(reports: &[AddonReport]) -> Vec<(String, usize)> {
    rank(reports, |r| &r.missing_globals)
}

/// How many addons want each name, most-wanted first, ties alphabetical. The unit is the addon,
/// never the call site, so a library replicated across sixty folders does not decide the ranking.
fn rank(
    reports: &[AddonReport],
    pick: impl Fn(&AddonReport) -> &Vec<String>,
) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for r in reports {
        for n in pick(r) {
            *counts.entry(n.as_str()).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// [`demand`] over templates: how many addons name each template we never declared.
pub fn template_demand(reports: &[AddonReport]) -> Vec<(String, usize)> {
    rank(reports, |r| &r.missing_templates)
}

/// [`demand`] over [`AddonReport::missing_tables`]: frames and tables to transcribe.
pub fn table_demand(reports: &[AddonReport]) -> Vec<(String, usize)> {
    rank(reports, |r| &r.missing_tables)
}

/// [`demand`] over [`AddonReport::missing_methods`]: the widget-method queue. It over-reports; see
/// that field.
pub fn method_demand(reports: &[AddonReport]) -> Vec<(String, usize)> {
    rank(reports, |r| &r.missing_methods)
}

/// [`method_demand`] over [`AddonReport::optional_methods`]: methods addons work around, read as
/// "implementing this improves N addons", not "N addons are broken".
pub fn optional_method_demand(reports: &[AddonReport]) -> Vec<(String, usize)> {
    rank(reports, |r| &r.optional_methods)
}

/// [`demand`] over [`AddonReport::kind_missing_methods`]: the per-kind widget-method queue, each
/// row a blocker whatever other kind answers the verb. `(on no kind)` is a verb to write; a named
/// kind is a verb to wire to its sibling.
pub fn kind_method_demand(reports: &[AddonReport]) -> Vec<(String, usize)> {
    rank(reports, |r| &r.kind_missing_methods)
}

/// [`demand`] over [`AddonReport::ambiguous_methods`]: names whose answer depends on the kind at an
/// untyped call site, an upper bound.
///
/// Ranked narrowest first, then by demand: the fewer kinds answer a name, the likelier an untyped
/// receiver is one that does not (`AddMessage` is answered by the two message frames only), while a
/// name absent only on the region leaves (`StopMovingOrSizing`) is noise. Nothing is filtered.
pub fn ambiguous_method_demand(reports: &[AddonReport]) -> Vec<(String, usize)> {
    let mut rows = rank(reports, |r| &r.ambiguous_methods);
    // The row text is the sort key: `only on` is [`only_on`]'s untruncated branch, so counting
    // separators recovers the set size exactly.
    rows.sort_by_key(|(row, count)| {
        let narrow = row.contains("(only on ");
        (
            !narrow,
            if narrow { row.matches(", ").count() } else { 0 },
            std::cmp::Reverse(*count),
        )
    });
    rows
}

/// [`template_demand`] over [`AddonReport::missing_inherits`].
pub fn inherits_demand(reports: &[AddonReport]) -> Vec<(String, usize)> {
    rank(reports, |r| &r.missing_inherits)
}

/// The first error of every addon that failed to load, normalised and ranked: a chunk stops at its
/// first raise, so later errors would count one cause once per victim. Normalisation collapses
/// quoted names to `'X'`, drops source positions and the `<file>: ` prefix.
pub fn blockers(reports: &[AddonReport]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for r in reports.iter().filter(|r| !r.loaded) {
        if let Some(e) = r.errors.first() {
            *counts.entry(normalise_error(e)).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// The addons behind one [`blockers`] row, with their verbatim errors.
///
/// `pattern` is a substring matched against the raw error text or its normalised row (which turns
/// every quoted name into `'X'`), over every error, not only the ranked one. A hit on the error the
/// tables counted (a failed addon's first load error, a loaded addon's first session error) is
/// labelled `[load]`/`[session]` with no index, so the index-less rows reproduce the ranked count;
/// any other is `[load #3]`, `[session #2]`.
pub fn blocked_by(reports: &[AddonReport], pattern: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for r in reports {
        // A failed addon ranks through its load error, a clean one through its first session error;
        // both lists are searched either way. The use column's first error is ranked too (its table
        // has no `loaded` filter); the UI probe ranks nothing, so its rows always carry an index.
        let ranked_kind = if r.loaded { "session" } else { "load" };
        for (list, kind) in [
            (&r.errors, "load"),
            (&r.session_errors, "session"),
            (&r.probe_errors, "probe"),
            (&r.used.errors, "used"),
        ] {
            for (i, e) in list.iter().enumerate() {
                if !(e.contains(pattern) || normalise_error(e).contains(pattern)) {
                    continue;
                }
                let ranked = i == 0 && (kind == ranked_kind || kind == "used");
                let label = if ranked {
                    kind.to_string()
                } else {
                    format!("{kind} #{}", i + 1)
                };
                out.push((format!("{} [{label}]", r.name), e.clone()));
            }
        }
    }
    out
}

/// Which addons carry `pattern` in one demand list, matched case-insensitively on a substring: the
/// read-back behind a ranked count.
pub fn wanters(
    reports: &[AddonReport],
    pattern: &str,
    pick: impl Fn(&AddonReport) -> &Vec<String>,
) -> Vec<(String, String)> {
    let needle = pattern.to_ascii_lowercase();
    let mut out = Vec::new();
    for r in reports {
        for n in pick(r) {
            if n.to_ascii_lowercase().contains(&needle) {
                out.push((r.name.clone(), n.clone()));
            }
        }
    }
    out
}

/// The addons behind one method-table row: a case-sensitive substring match against the row as
/// printed, over all four method tables, each hit labelled with its table, so
/// `--why "EditBox:SetFontObject"` names the addons and `--why SetFontObject` finds every table.
pub fn method_rows_matching(reports: &[AddonReport], pattern: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for r in reports {
        for (list, table) in [
            (&r.kind_missing_methods, "by-kind"),
            (&r.missing_methods, "missing"),
            (&r.ambiguous_methods, "ambiguous"),
            (&r.optional_methods, "feature-tested"),
        ] {
            for row in list.iter().filter(|row| row.contains(pattern)) {
                out.push((format!("{} [{table}]", r.name), row.clone()));
            }
        }
    }
    out
}

/// [`normalise_error`], public so session-start errors rank with the load-time collapse.
pub fn normalise(raw: &str) -> String {
    normalise_error(raw)
}

/// The byte index of the next `'` that opens a quote: one without a letter or digit on both sides.
fn quote_open(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = s[from..].find('\'') {
        let i = from + rel;
        let prev_word = i > 0 && b[i - 1].is_ascii_alphanumeric();
        let next_word = b.get(i + 1).is_some_and(u8::is_ascii_alphanumeric);
        if !(prev_word && next_word) {
            return Some(i);
        }
        from = i + 1;
    }
    None
}

/// One load error with everything addon-specific removed, so two addons hitting the same wall
/// produce the same string.
fn normalise_error(raw: &str) -> String {
    // 1 · From the last `error: ` on, which drops every `<file>: <Script file="…">: ` prefix, and
    //     without mlua's `stack traceback:` tail.
    let core = raw.rfind("error: ").map_or(raw, |i| &raw[i..]);
    let core = core
        .split_once("stack traceback:")
        .map_or(core, |(head, _)| head)
        .trim();

    // 2 · Every quoted name becomes `'X'`; both quote kinds, since mlua writes a chunk name as
    //     `[string "MyFrame:OnLoad"]`. An apostrophe inside a word (`Couldn't`) is not a quote.
    let squashed = core.replace('"', "'");
    let mut collapsed = String::with_capacity(squashed.len());
    let mut rest = squashed.as_str();
    while let Some(open) = quote_open(rest) {
        collapsed.push_str(&rest[..open]);
        collapsed.push_str("'X'");
        match rest[open + 1..].find('\'') {
            Some(close) => rest = &rest[open + 1 + close + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    collapsed.push_str(rest);

    // 3 · Source positions carry nothing once the name is gone.
    collapsed
        .split_whitespace()
        .filter(|t| !is_position(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Is this token a source position (`<where>:<line>[:<col>]:`, the only shape mlua emits)?
fn is_position(tok: &str) -> bool {
    if tok == "[string" {
        return true; // the opening half of mlua's `[string "…"]:N:` chunk name
    }
    let t = tok.strip_suffix(':').unwrap_or(tok);
    let mut tail = t.rsplit(':');
    let last = tail.next().unwrap_or("");
    if last.is_empty() || !last.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // `<where>:<line>` is enough; a third field is a column.
    tail.next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two addons hitting one wall produce one row.
    #[test]
    fn one_wall_is_one_row_however_it_was_reported() {
        let same = [
            "libs\\AceLibrary\\AceLibrary.lua: runtime error: crates/benilla-ui/src/script/mod.rs:406:305: 'setn' is obsolete",
            "Libs/AceLibrary.lua: runtime error: crates/benilla-ui/src/script/mod.rs:406:301: 'setn' is obsolete",
            "embeds.xml: <Script file=\"AceLibrary.lua\">: runtime error: crates/benilla-ui/src/loader/mod.rs:218:13: 'setn' is obsolete",
        ];
        let normalised: BTreeSet<String> = same.iter().map(|e| normalise_error(e)).collect();
        assert_eq!(
            normalised.into_iter().collect::<Vec<_>>(),
            vec!["error: 'X' is obsolete"],
            "the file, the source position and the quoted name are all per-addon noise"
        );
    }

    /// A raise in a demand-loaded sibling's file (FuBar loads every installed `FuBar_*`,
    /// `FuBar.lua:1034`) is that sibling's row, not the surveyed addon's.
    #[test]
    fn a_raise_in_another_installed_addons_file_is_that_addons_row() {
        let installed: BTreeSet<String> = ["fubar", "fubar_moneyfu", "fubar_battlegroundfu", "ace"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let sibling = "LoadAddOn: FuBar_BattlegroundFu/lib\\Glory-2.0\\Glory-2.0.lua: runtime \
                       error: ...Ons\\FuBar_BattlegroundFu\\lib\\Glory-2.0\\Glory-2.0.lua:23: \
                       Glory-2.0 requires Deformat-2.0";
        assert!(
            raised_inside_another_addons_own_file(sibling, "FuBar_MoneyFu", &installed),
            "a sibling's own file is the sibling's row"
        );
        // The surveyed addon's own file is its own row, even a library whose folder name is an
        // installed addon.
        let mine = "runtime error: Interface\\AddOns\\FuBar_MoneyFu\\Libs\\Ace\\Ace.lua:8: boom";
        assert!(!raised_inside_another_addons_own_file(
            mine,
            "FuBar_MoneyFu",
            &installed
        ));
        // A chunk naming no addon folder is kept: the surveyed addon drove it.
        let ours = "runtime error: [string \"Frame:OnEvent\"]:2: attempt to index a nil value";
        assert!(!raised_inside_another_addons_own_file(
            ours,
            "FuBar_MoneyFu",
            &installed
        ));
        // A folder that is not installed is not another addon's row.
        let stranger = "runtime error: Interface\\AddOns\\NotInstalled\\x.lua:1: boom";
        assert!(!raised_inside_another_addons_own_file(
            stranger,
            "FuBar_MoneyFu",
            &installed
        ));
    }

    /// A raise inside a `LoadAddOn` the surveyed addon made is the loaded addon's row, even when
    /// the raise site names no file (`Stubby.lua:581` loading Auctioneer).
    #[test]
    fn a_raise_inside_a_demand_load_belongs_to_the_addon_being_loaded() {
        let installed: BTreeSet<String> = ["stubby", "auctioneer", "enchantrix"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let through_loadaddon =
            "runtime error: [string \"AuctioneerFrame:OnEvent\"]:4: attempt to \
             index field 'Core' (a nil value)\n\
             stack traceback:\n\
             \t[C]: in ?\n\
             \t[string \"AuctioneerFrame:OnEvent\"]:4: in function <...>\n\
             \t[C]: in function 'LoadAddOn'\n\
             \tInterface\\AddOns\\Stubby\\Stubby.lua:581: in upvalue 'inspectAddOn'";
        assert!(
            raised_inside_another_addons_own_file(through_loadaddon, "Stubby", &installed),
            "the LoadAddOn frame sits between the raise and Stubby's own code"
        );
        assert!(
            raised_inside_another_addons_own_file(through_loadaddon, "Enchantrix", &installed),
            "…and the same for an addon that only pulled Stubby in"
        );
        // The surveyed addon's own frame above the boundary keeps the raise.
        let ours_first = "runtime error: [string \"StubbyFrame:OnEvent\"]:2: boom\n\
             stack traceback:\n\
             \tInterface\\AddOns\\Stubby\\Stubby.lua:12: in function 'Stubby.Thing'\n\
             \t[C]: in function 'LoadAddOn'";
        assert!(!raised_inside_another_addons_own_file(
            ours_first, "Stubby", &installed
        ));
        // No `LoadAddOn` anywhere: the raise is kept.
        let plain = "runtime error: [string \"StubbyFrame:OnEvent\"]:2: boom\n\
             stack traceback:\n\
             \t[C]: in ?";
        assert!(!raised_inside_another_addons_own_file(
            plain, "Stubby", &installed
        ));
    }

    /// mlua's `[string "Frame:OnLoad"]:2:` chunk name is a position, not words.
    #[test]
    fn a_chunk_name_position_is_not_mistaken_for_the_message() {
        assert_eq!(
            normalise_error(
                "Outfitter: OnLoad: runtime error: [string \"OutfitterShowMinimapButton:OnLoad\"]:2: attempt to index a nil value"
            ),
            "error: attempt to index a nil value"
        );
    }

    /// An XML comment is not scanned, and the Lua inside the file still is.
    #[test]
    fn an_xml_comment_is_not_scanned_but_the_script_body_is() {
        let xml = "<Ui>\n\
                   <!-- MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the\n\
                        GNU General Public License. $Id: Thing.xml 1 2006 $ -->\n\
                   <Script><![CDATA[\n\
                   GameTooltip:AddLine(\"x\")\n\
                   MyHelper()\n\
                   ]]></Script>\n\
                   </Ui>";
        let mut s = Scan::default();
        scan_lua(&strip_xml_comments(xml), &mut s);
        for noise in ["PURPOSE", "License", "Id"] {
            assert!(
                !s.indexed.contains(noise) && !s.called.contains(noise),
                "{noise} is license boilerplate, not a surface the addon expects"
            );
        }
        assert!(
            s.indexed.contains("GameTooltip"),
            "the script body must still be scanned: {:?}",
            s.indexed
        );
        assert!(
            s.called.contains("MyHelper"),
            "the script body must still be scanned: {:?}",
            s.called
        );
        // The method arm obeys the same stripper.
        assert!(
            s.methods.contains("AddLine"),
            "the script body's method call must be scanned: {:?}",
            s.methods
        );
    }

    /// An unterminated `<!--` takes the rest of the file, as an XML parser does.
    #[test]
    fn an_unterminated_xml_comment_swallows_the_rest() {
        let out = strip_xml_comments("Kept.Alpha\n<!-- Dropped.Beta\nDropped.Gamma\n");
        assert!(out.contains("Kept"));
        assert!(
            !out.contains("Beta") && !out.contains("Gamma"),
            "got: {out:?}"
        );
        // Line structure survives.
        assert_eq!(out.matches('\n').count(), 3);
    }

    /// A capitalised Lua local is not a missing API (`FuBarPlugin-2.0.lua`'s `local X = function`).
    #[test]
    fn a_capitalised_local_is_not_a_missing_global() {
        let mut s = Scan::default();
        scan_lua(
            "local CheckShow = function(self, panelId) end\n\
             local DropDownList1_Show = DropDownList1.Show\n\
             local A, B = 1, 2\n\
             local function Direct() end\n\
             Global = function() end\n\
             CheckShow(self, 1)\n\
             DropDownList1_Show(DropDownList1)\n\
             A() B() Direct() Global()\n\
             UnitName(\"player\")\n",
            &mut s,
        );
        let missing: Vec<&str> = s
            .called
            .iter()
            .filter(|n| !s.defined.contains(*n))
            .map(String::as_str)
            .collect();
        assert_eq!(
            missing,
            vec!["UnitName"],
            "every capitalised name the file binds itself is the file's, however it binds it"
        );
    }

    /// ...but a self-localisation (`local GetTime = GetTime`) still demands the global.
    #[test]
    fn localising_a_global_still_demands_it() {
        let mut s = Scan::default();
        scan_lua(
            "local GetTime = GetTime\n\
             local UnitName, UnitClass = UnitName, UnitClass\n\
             local MyCache = MyCache or {}\n\
             local Helper = function() end\n\
             GetTime() UnitName('player') UnitClass('player') MyCache() Helper()\n",
            &mut s,
        );
        let mut missing: Vec<&str> = s
            .called
            .iter()
            .filter(|n| !s.defined.contains(*n))
            .map(String::as_str)
            .collect();
        missing.sort_unstable();
        assert_eq!(
            missing,
            vec!["GetTime", "MyCache", "UnitClass", "UnitName"],
            "self-localisation in every shape — single, comma list, and the `or` form — keeps \
             the demand; only the genuinely-new `Helper` is the file's own"
        );
    }

    /// A "not found" has no `error: ` marker and survives whole: a missing file is its own wall.
    #[test]
    fn a_missing_file_stays_its_own_row() {
        assert_eq!(
            normalise_error("..\\..\\FrameXML\\Fonts.xml: not found"),
            "..\\..\\FrameXML\\Fonts.xml: not found"
        );
    }

    /// Only the first error of a failed addon counts.
    #[test]
    fn only_the_first_error_of_a_failed_addon_is_counted() {
        let report = |name: &str, loaded: bool, errors: Vec<String>| AddonReport {
            name: name.into(),
            loaded,
            errors,
            ..Default::default()
        };
        let ranked = blockers(&[
            report(
                "A",
                false,
                vec![
                    "x.lua: runtime error: 'setn' is obsolete".into(),
                    "y.lua: runtime error: 'other' is obsolete".into(),
                ],
            ),
            report(
                "B",
                false,
                vec!["z.lua: runtime error: 'setn' is obsolete".into()],
            ),
            report("C", true, vec![]),
        ]);
        assert_eq!(ranked, vec![("error: 'X' is obsolete".to_string(), 2)]);
    }
}

#[cfg(test)]
mod dependency_tests {
    use super::*;

    /// A dependency is loaded before its dependent, depth-first, as `AddOn_Load` (`0x51f240`) does.
    #[test]
    fn a_dependency_runs_before_the_addon_that_declares_it() {
        let tmp = std::env::temp_dir().join(format!("benilla-harness-deps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |name: &str, toc: &str, file: &str, body: &str| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
            std::fs::write(dir.join(file), body).unwrap();
        };
        // A library, a middle layer on it and a leaf on the middle, so the walk must be
        // depth-first.
        write(
            "Lib",
            "## Interface: 11200\nlib.lua\n",
            "lib.lua",
            "LibReady = 1\n",
        );
        write(
            "Mid",
            "## Interface: 11200\n## Dependencies: Lib\nmid.lua\n",
            "mid.lua",
            "MidReady = LibReady + 1\n",
        );
        write(
            "Leaf",
            "## Interface: 11200\n## Dependencies: Mid\nleaf.lua\n",
            "leaf.lua",
            "LeafReady = MidReady + 1\n",
        );

        let reports = survey(&tmp);
        let leaf = reports.iter().find(|r| r.name == "Leaf").unwrap();
        assert!(
            leaf.loaded,
            "the leaf loaded because its chain ran first: {:?}",
            leaf.errors
        );
        assert!(leaf.missing_deps.is_empty());

        // ...and a dependency that is not installed is still reported.
        write(
            "Orphan",
            "## Interface: 11200\n## Dependencies: Nowhere\norphan.lua\n",
            "orphan.lua",
            "OrphanReady = 1\n",
        );
        let reports = survey(&tmp);
        let orphan = reports.iter().find(|r| r.name == "Orphan").unwrap();
        assert_eq!(orphan.missing_deps, vec!["Nowhere".to_string()]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// An indexed frame (`ColorPickerFrame.func`, `ColorPickerFrame:SetColorRGB`) is a missing
    /// surface, not a missing function.
    #[test]
    fn an_indexed_frame_is_a_missing_surface_not_a_missing_function() {
        let mut s = Scan::default();
        scan_lua(
            "ColorPickerFrame.func = function() end\n\
             ColorPickerFrame:SetColorRGB(1, 0, 0)\n\
             GameTooltip:AddLine('hi')\n\
             MyAddon = {}\n\
             MyAddon.thing = 1\n\
             local Cache = {}\n\
             Cache.x = 1\n\
             UnitName('player')\n\
             self.wrong = 1\n",
            &mut s,
        );
        let live: Vec<&str> = s
            .indexed
            .iter()
            .filter(|n| !s.defined.contains(*n))
            .map(String::as_str)
            .collect();
        assert_eq!(
            live,
            vec!["ColorPickerFrame", "GameTooltip"],
            "the addon's own MyAddon and its local Cache are its own; `self` is lowercase and \
             qualified reads never count"
        );
        assert_eq!(
            s.called.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["UnitName"],
            "and the lists do not bleed into each other"
        );
        // `ColorPickerFrame.func = function` binds a field, so `func` is credited; `SetColorRGB`
        // and `AddLine` are only called, so they are demanded.
        assert_eq!(
            s.methods.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["AddLine", "SetColorRGB"],
            "a `:` call is a method demand and never a global one"
        );
        assert!(
            s.defined_methods.contains("func"),
            "a dotted assignment is the addon binding its own field: {:?}",
            s.defined_methods
        );
    }

    /// `blocked_by` matches the row as printed and the raw error text, so a quoted name the
    /// normalisation collapsed is findable.
    #[test]
    fn blocked_by_reads_back_by_row_and_by_name() {
        let report = |name: &str, loaded: bool, first: &str| AddonReport {
            name: name.into(),
            loaded,
            errors: vec![first.into()],
            ..Default::default()
        };
        let reports = [
            report(
                "A",
                false,
                "a.lua: runtime error: bad argument #1 to 'tinsert' (table expected, got nil)",
            ),
            report(
                "B",
                false,
                "b.xml: runtime error: bad argument #1 to 'tremove' (table expected, got nil)",
            ),
            report(
                "C",
                false,
                "c.lua: runtime error: attempt to call a table value",
            ),
            report("D", true, ""),
        ];
        let wall = blocked_by(&reports, "table expected");
        let hits: Vec<&str> = wall.iter().map(|(n, _)| n.as_str()).collect();
        // Both failed to load, so both read back through the load table.
        assert_eq!(
            hits,
            vec!["A [load]", "B [load]"],
            "two different verbs, one wall"
        );
        // The collapsed name finds exactly the addon that used it.
        let by_name: Vec<String> = blocked_by(&reports, "tinsert")
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            by_name,
            vec!["A [load]"],
            "a quoted name is what a reader types, and it must reach the addon behind it"
        );
        // ...and the verbatim error comes back.
        assert!(wall[0].1.contains("tinsert"));
    }

    /// A later error is found and labelled `#N`, outside the ranked count; the ranked one carries
    /// no index.
    #[test]
    fn a_later_error_is_found_and_labelled_as_not_the_ranked_one() {
        let reports = [AddonReport {
            name: "Late".into(),
            loaded: true,
            session_errors: vec![
                "runtime error: attempt to call global 'Foo' (a nil value)".into(),
                "runtime error: attempt to index a nil value".into(),
                "runtime error: attempt to call method 'GetBackdrop' (a nil value)".into(),
            ],
            ..Default::default()
        }];
        let ranked: Vec<String> = blocked_by(&reports, "attempt to call global")
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            ranked,
            vec!["Late [session]"],
            "no index: this is the row the table counted"
        );

        let late: Vec<String> = blocked_by(&reports, "GetBackdrop")
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            late,
            vec!["Late [session #3]"],
            "the method that killed a handler was the addon's THIRD error, and it must still be \
             reachable by the only name anyone would search for"
        );
    }

    /// A clean load is not a working addon: a raise from a `PLAYER_LOGIN` handler or from
    /// `OnUpdate` lands in `session_errors`, and `loaded` still counts load errors only.
    #[test]
    fn a_clean_load_is_not_a_working_addon() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |name: &str, body: &str| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{name}.toc")),
                "## Interface: 11200\na.lua\n",
            )
            .unwrap();
            std::fs::write(dir.join("a.lua"), body).unwrap();
        };
        write(
            "LoginBreaker",
            "local f = CreateFrame('Frame')\n\
             f:RegisterEvent('PLAYER_LOGIN')\n\
             f:SetScript('OnEvent', function() error('boom at login') end)\n",
        );
        write(
            "TickBreaker",
            "local f = CreateFrame('Frame')\n\
             f:SetScript('OnUpdate', function() error('boom on update') end)\n",
        );
        write(
            "Fine",
            "local f = CreateFrame('Frame')\n\
             f:RegisterEvent('PLAYER_LOGIN')\n\
             f:SetScript('OnEvent', function() FineRan = 1 end)\n",
        );

        let reports = survey(&tmp);
        let get = |n: &str| reports.iter().find(|r| r.name == n).unwrap();

        for n in ["LoginBreaker", "TickBreaker", "Fine"] {
            assert!(
                get(n).loaded,
                "{n}: `loaded` is LOAD errors only and must not change meaning: {:?}",
                get(n).errors
            );
        }
        assert!(
            !get("LoginBreaker").session_errors.is_empty(),
            "a handler that raises on PLAYER_LOGIN is exactly what no other column can see"
        );
        assert!(
            !get("TickBreaker").session_errors.is_empty(),
            "and an OnUpdate needs the ticks, not just the events"
        );
        assert!(
            get("Fine").session_errors.is_empty(),
            "{:?}",
            get("Fine").session_errors
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A consumer's own code raising inside a dependency's `ADDON_LOADED` is the consumer's row:
    /// AceAddon runs queued consumers' `OnInitialize` on any `ADDON_LOADED` it sees
    /// (`AceAddon-2.0.lua:104-105`, `:230`). Attribution is by the raising chunk.
    #[test]
    fn a_consumers_raise_inside_a_dependency_window_is_the_consumers_row() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-attrib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |name: &str, toc: &str, file: &str, body: &str| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
            std::fs::write(dir.join(file), body).unwrap();
        };
        // AceAddon's shape: drain every queued consumer on the first `ADDON_LOADED`, whoever it
        // names.
        write(
            "QueueLib",
            "## Interface: 11200\nlib.lua\n",
            "lib.lua",
            "QueueLibQueue = {}\n\
             QueueLibFrame = CreateFrame(\"Frame\")\n\
             QueueLibFrame:RegisterEvent(\"ADDON_LOADED\")\n\
             QueueLibFrame:SetScript(\"OnEvent\", function()\n\
             while table.getn(QueueLibQueue) > 0 do\n\
             local f = table.remove(QueueLibQueue, 1) f()\n\
             end\n\
             end)\n",
        );
        // The consumer queues a callback that raises from its own file.
        write(
            "QueueUser",
            "## Interface: 11200\n## Dependencies: QueueLib\nuse.lua\n",
            "use.lua",
            "table.insert(QueueLibQueue, function() error(\"consumer init blew up\") end)\n",
        );

        let reports = survey(&tmp);
        let of = |n: &str| reports.iter().find(|r| r.name == n).unwrap();

        assert!(
            of("QueueUser")
                .session_errors
                .iter()
                .any(|e| e.contains("consumer init blew up")),
            "the consumer's own file raised — window or not, it is the consumer's row: {:?}",
            of("QueueUser").session_errors
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Every addon in the VM gets its own `ADDON_LOADED`, its own name in `arg1`: a dependency's
    /// initialiser is gated on it (`Atlas.lua:326`). A dependency's own handler raising is the
    /// dependency's row, never its consumers', since those events fire outside the error mark.
    #[test]
    fn each_loaded_addon_gets_its_own_addon_loaded_event() {
        let tmp = std::env::temp_dir().join(format!(
            "benilla-harness-addonloaded-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |name: &str, toc: &str, file: &str, body: &str| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
            std::fs::write(dir.join(file), body).unwrap();
        };
        // Atlas's shape (`Atlas.lua:326`): a library that initialises only on its own
        // `ADDON_LOADED`.
        write(
            "TheLib",
            "## Interface: 11200\nlib.lua\n",
            "lib.lua",
            "TheLibFrame = CreateFrame(\"Frame\")\n\
             TheLibFrame:RegisterEvent(\"ADDON_LOADED\")\n\
             TheLibFrame:SetScript(\"OnEvent\", function()\n\
             if event == \"ADDON_LOADED\" and arg1 == \"TheLib\" then TheLibOptions = {} end\n\
             end)\n",
        );
        // The consumer reads the library's initialised state on a later event.
        write(
            "TheUser",
            "## Interface: 11200\n## Dependencies: TheLib\nuse.lua\n",
            "use.lua",
            "TheUserFrame = CreateFrame(\"Frame\")\n\
             TheUserFrame:RegisterEvent(\"PLAYER_LOGIN\")\n\
             TheUserFrame:SetScript(\"OnEvent\", function() local _ = TheLibOptions.anything end)\n",
        );
        // A library whose own handler raises: its row, not its consumers'.
        write(
            "BadLib",
            "## Interface: 11200\nbad.lua\n",
            "bad.lua",
            "BadLibFrame = CreateFrame(\"Frame\")\n\
             BadLibFrame:RegisterEvent(\"ADDON_LOADED\")\n\
             BadLibFrame:SetScript(\"OnEvent\", function()\n\
             if event == \"ADDON_LOADED\" and arg1 == \"BadLib\" then error(\"lib init blew up\") end\n\
             end)\n",
        );
        write(
            "BadUser",
            "## Interface: 11200\n## Dependencies: BadLib\nquiet.lua\n",
            "quiet.lua",
            "QuietGlobal = 1\n",
        );

        let reports = survey(&tmp);
        let of = |n: &str| reports.iter().find(|r| r.name == n).unwrap();

        assert!(
            of("TheUser").session_errors.is_empty(),
            "the dependency must have received its OWN ADDON_LOADED and initialised: {:?}",
            of("TheUser").session_errors
        );
        assert!(
            of("BadUser").session_errors.is_empty(),
            "a dependency's own handler raising is ITS row, not its consumer's: {:?}",
            of("BadUser").session_errors
        );
        assert!(
            of("BadLib")
                .session_errors
                .iter()
                .any(|e| e.contains("lib init blew up")),
            "...and it must still be reported against the library itself: {:?}",
            of("BadLib").session_errors
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A dependency's `ADDON_LOADED` lands before the dependent's first line runs: the 1.12 client
    /// emits per addon its dependencies, its `.toc` files (`0x51f3fa`), `Bindings.xml`, the
    /// SavedVariables chunks, then `ADDON_LOADED` (`0x51f5ad`). The read is at file scope (as
    /// `KTMAutoHider`'s `<OnLoad>` reads `KLHThreatMeter`'s GUI), and `EagerLib` initialises on its
    /// own event only.
    #[test]
    fn a_dependencys_addon_loaded_precedes_the_dependents_file_scope() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-eager-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |name: &str, toc: &str, file: &str, body: &str| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
            std::fs::write(dir.join(file), body).unwrap();
        };
        write(
            "EagerLib",
            "## Interface: 11200\nlib.lua\n",
            "lib.lua",
            "EagerLibFrame = CreateFrame(\"Frame\")\n\
             EagerLibFrame:RegisterEvent(\"ADDON_LOADED\")\n\
             EagerLibFrame:SetScript(\"OnEvent\", function()\n\
             if event == \"ADDON_LOADED\" and arg1 == \"EagerLib\" then EagerLibReady = {} end\n\
             end)\n",
        );
        // The consumer reads it at file scope.
        write(
            "EagerUser",
            "## Interface: 11200\n## Dependencies: EagerLib\nuse.lua\n",
            "use.lua",
            "EagerUserSaw = EagerLibReady.anything\n",
        );

        let reports = survey(&tmp);
        let of = |n: &str| reports.iter().find(|r| r.name == n).unwrap();

        assert!(
            of("EagerUser").errors.is_empty(),
            "the dependency's ADDON_LOADED must precede the dependent's file scope: {:?}",
            of("EagerUser").errors
        );
        assert!(
            of("EagerUser").loaded && of("EagerUser").session_errors.is_empty(),
            "...and the row is clean end to end: {:?}",
            of("EagerUser").session_errors
        );
        // The control: `EagerLib`'s own row gets its own event and raises nothing.
        assert!(
            of("EagerLib").loaded && of("EagerLib").session_errors.is_empty(),
            "the library's own row is unaffected: {:?}",
            of("EagerLib").session_errors
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A template named through a local resolves, and the census's bound is asserted as an exact
    /// set: a concatenated name, a table field and a parameter stay invisible (a fragment of
    /// `"Unseen" .. "ByConcat"` must not enter).
    #[test]
    fn the_template_census_resolves_a_local_and_says_what_it_still_cannot_see() {
        let tmp = std::env::temp_dir().join(format!("benilla-harness-tpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("TplUser");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("TplUser.toc"), "## Interface: 11200\nuse.lua\n").unwrap();
        // Every shape in one file; only the first three are seen.
        std::fs::write(
            dir.join("use.lua"),
            r#"
            local tpl = "SeenViaLocal"
            if bag == -1 then tpl = "SeenViaRebind" end
            CreateFrame("Button", "a", nil, tpl)
            CreateFrame("Button", "b", nil, "SeenAsLiteral")

            -- Still invisible, deliberately: built by concatenation, read from a table, and
            -- passed in as a parameter. Naming them here is the bound.
            local built = "Unseen" .. "ByConcat"
            CreateFrame("Button", "c", nil, built)
            CreateFrame("Button", "d", nil, cfg.template)
            function f(passed) CreateFrame("Button", "e", nil, passed) end
            "#,
        )
        .unwrap();

        let reports = survey(&tmp);
        let r = reports.iter().find(|r| r.name == "TplUser").unwrap();
        let got: BTreeSet<&str> = r.missing_templates.iter().map(String::as_str).collect();

        for want in ["SeenViaLocal", "SeenViaRebind", "SeenAsLiteral"] {
            assert!(got.contains(want), "{want} must be seen — got {got:?}");
        }
        // A name rebound to two literals keeps both.
        assert!(
            got.contains("SeenViaLocal") && got.contains("SeenViaRebind"),
            "a rebound local keeps every literal it was bound to"
        );
        // The bound as an exact set, so a future widening shows here.
        let want: BTreeSet<&str> = ["SeenViaLocal", "SeenViaRebind", "SeenAsLiteral"]
            .into_iter()
            .collect();
        assert_eq!(
            got, want,
            "the census sees exactly these three shapes — concatenation, a table field and a \
             parameter stay invisible, and that bound lives here rather than only in a doc comment"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A manifest entry with no file is split by whose package is short: `own`, a `.toc` listing a
    /// file its folder lacks (the 1.12 client logs `Couldn't open %s` at `0x6edaa0` and carries
    /// on), or `foreign`, a `..` entry (collapsed as `0x6ede10` does) into a folder not installed.
    /// The two never merge.
    #[test]
    fn an_absent_manifest_entry_is_attributed_to_whose_package_is_short() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |name: &str, toc: &str, files: &[(&str, &str)]| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
            for (f, body) in files {
                std::fs::write(dir.join(f), body).unwrap();
            }
        };
        // Ships one of the two files its own manifest lists.
        write(
            "ShortPackage",
            "## Interface: 11200\nhere.lua\ngone.lua\n",
            &[("here.lua", "ShortPackageRan = 1\n")],
        );
        // Reaches a neighbour that is not installed, through `..`.
        write(
            "WantsNeighbour",
            "## Interface: 11200\nown.lua\n..\\NotInstalled\\templates.xml\n",
            &[("own.lua", "WantsNeighbourRan = 1\n")],
        );

        let reports = survey(&tmp);
        let of = |n: &str| reports.iter().find(|r| r.name == n).unwrap();

        let short = of("ShortPackage");
        assert_eq!(
            short.absent_own_files,
            vec!["gone.lua".to_string()],
            "the entry inside its own folder is the addon's own package being short"
        );
        assert!(
            short.absent_foreign_files.is_empty(),
            "and it is NOT a missing neighbour: {:?}",
            short.absent_foreign_files
        );

        let wants = of("WantsNeighbour");
        assert_eq!(
            wants.absent_foreign_files,
            vec!["Interface/AddOns/NotInstalled/templates.xml".to_string()],
            "`..` is collapsed the way the client collapses it, and the RESOLVED path is what is \
             reported — the collapse is the interesting half. The path is INSTALL-relative since \
             2155, which is the space the reference's file layer is actually handed: this exact \
             shape is Auctioneer's, and its real neighbour resolves off the chain now."
        );
        assert!(
            wants.absent_own_files.is_empty(),
            "its own package is complete: {:?}",
            wants.absent_own_files
        );

        // Nothing is subtracted from `errors`, and both addons count as loaded: `0x6edaa0` logs
        // `Couldn't open %s` and the walk carries on with nothing raised.
        for r in [short, wants] {
            assert!(
                r.loaded,
                "{}: a file the package does not contain is not something that RAISED",
                r.name
            );
            assert!(
                r.errors.iter().any(|e| e.contains("not found")),
                "{}: …and the row is still there verbatim: {:?}",
                r.name,
                r.errors
            );
        }
        // ...and the file that exists ran: a missing entry does not abort the manifest.
        assert!(
            !short.errors.iter().any(|e| e.contains("here.lua")),
            "the surviving file loads: {:?}",
            short.errors
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A library another addon embeds is invisible here, the price of one VM per addon: an addon
    /// leaning on a sibling's copy (`FuBar_CustomMenuFu` calls `AceLibrary("Tablet-2.0")`) fails
    /// here and works on the 1.12 client, which shares one Lua state.
    #[test]
    fn a_sibling_addons_embedded_library_is_invisible() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-sibling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |name: &str, toc: &str, file: &str, body: &str| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
            std::fs::write(dir.join(file), body).unwrap();
        };
        // One addon embeds a library; another uses it with no declared relationship, which works on
        // the 1.12 client because they share a Lua state.
        write(
            "Embedder",
            "## Interface: 11200\nembedded.lua\n",
            "embedded.lua",
            "SharedLibGlobal = 1\n",
        );
        write(
            "Freeloader",
            "## Interface: 11200\nuse.lua\n",
            "use.lua",
            "if not SharedLibGlobal then error('needs the sibling library') end\n",
        );

        let reports = survey(&tmp);
        assert!(
            reports
                .iter()
                .find(|r| r.name == "Embedder")
                .unwrap()
                .loaded,
            "the addon that ships it is fine"
        );
        let free = reports.iter().find(|r| r.name == "Freeloader").unwrap();
        assert!(
            !free.loaded,
            "and the one that borrows it fails HERE while working on the real client — the \
             isolation's price, not a gap in the API surface"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// An optional dependency loads first (`AddOn_Load`'s order; `FuBar_BagFu` needs `Ace2` first),
    /// and an uninstalled one is silent: `missing_deps` is required-only.
    #[test]
    fn an_optional_dependency_loads_first_and_a_missing_one_is_silent() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-optdeps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |name: &str, toc: &str, file: &str, body: &str| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
            std::fs::write(dir.join(file), body).unwrap();
        };
        // The library, and a dependent whose own file order needs it to have gone first.
        write(
            "TheLib",
            "## Interface: 11200\nlib.lua\n",
            "lib.lua",
            "TheLibGlobal = 1\n",
        );
        write(
            "Dependent",
            "## Interface: 11200\n## OptionalDeps: TheLib, NotInstalled\nuse.lua\n",
            "use.lua",
            "if not TheLibGlobal then error('Dependent requires TheLib') end\nDependentReady = 1\n",
        );

        let reports = survey(&tmp);
        let r = reports.iter().find(|r| r.name == "Dependent").unwrap();
        assert!(
            r.loaded,
            "the optional dependency ran first: {:?}",
            r.errors
        );
        assert!(
            r.missing_deps.is_empty(),
            "an uninstalled OPTIONAL dep is silent — missing_deps is the required-only list: {:?}",
            r.missing_deps
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The `inherits=` census counts a missing template, and neither a font name nor a virtual the
    /// addon declares itself.
    #[test]
    fn the_inherits_census_counts_templates_and_not_fonts() {
        benilla_formats::wow_data_or_skip!();
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-inherits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("Inheritor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Inheritor.toc"), "## Interface: 11200\nui.xml\n").unwrap();
        std::fs::write(
            dir.join("ui.xml"),
            r#"<Ui>
                <Button name="InheritorOwnTemplate" virtual="true"/>
                <Frame name="InheritorRoot">
                    <Layers><Layer level="ARTWORK">
                        <FontString name="$parentLabel" inherits="GameFontNormal" text="hi"/>
                    </Layer></Layers>
                    <Frames>
                        <Button name="$parentMine" inherits="InheritorOwnTemplate"/>
                        <Button name="$parentReal" inherits="UIPanelButtonTemplate"/>
                        <Button name="$parentGone" inherits="NoSuchTemplate"/>
                    </Frames>
                </Frame>
            </Ui>
"#,
        )
        .unwrap();

        let reports = survey(&tmp);
        let r = reports.iter().find(|r| r.name == "Inheritor").unwrap();
        assert_eq!(
            r.missing_inherits,
            vec!["NoSuchTemplate".to_string()],
            "GameFontNormal is a font, InheritorOwnTemplate is the addon's own, and \
             UIPanelButtonTemplate is one we now ship"
        );
        assert!(
            r.missing_templates.is_empty(),
            "and none of it is visible to the CreateFrame scanner — the point of the twin"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
    /// The widget-method census finds an invented method no widget provides, and none of these:
    ///
    /// | called | why it must not appear |
    /// |---|---|
    /// | `f:SetWidth` | a frame method we ship |
    /// | `f:CreateTexture` | a frame method we ship; it makes the region probe |
    /// | `t:SetTexCoord` | a region method, reachable only from the Texture probe |
    /// | `Probe:OwnMethod` | the addon declares it (`function T:N`) |
    /// | `f:Hooked` | the addon declares it (`T.N = function`) |
    /// | `MethodicalLib:LibOnly` | a loaded dependency declares it |
    /// | `f:OptionalHook` | feature-tested one line up, so not a blocker |
    ///
    /// A silent-empty oracle loses `NoSuchWidgetMethod`; a blanket-report one gains `SetWidth`.
    #[test]
    fn the_method_census_finds_a_method_no_widget_provides() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-methods-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let lib = tmp.join("MethodicalLib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(
            lib.join("MethodicalLib.toc"),
            "## Interface: 11200\nlib.lua\n",
        )
        .unwrap();
        std::fs::write(
            lib.join("lib.lua"),
            "MethodicalLib = {}\nfunction MethodicalLib:LibOnly() end\n",
        )
        .unwrap();
        let dir = tmp.join("Methodical");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Methodical.toc"),
            "## Interface: 11200\n## Dependencies: MethodicalLib\na.lua\n",
        )
        .unwrap();
        // Inside a function never called: the census is static, and an unrun body keeps `loaded`
        // clean.
        std::fs::write(
            dir.join("a.lua"),
            "local f = CreateFrame(\"Frame\")\n\
             Probe = {}\n\
             function Probe:OwnMethod() end\n\
             f.Hooked = function() end\n\
             function Methodical_Never()\n\
             f:SetWidth(10)\n\
             local t = f:CreateTexture()\n\
             t:SetTexCoord(0, 1, 0, 1)\n\
             f:NoSuchWidgetMethod(1)\n\
             Probe:OwnMethod()\n\
             f:Hooked()\n\
             MethodicalLib:LibOnly()\n\
             if type(f.OptionalHook) == \"function\" then f:OptionalHook() end\n\
             end\n",
        )
        .unwrap();

        let reports = survey(&tmp);
        let r = reports.iter().find(|r| r.name == "Methodical").unwrap();
        assert!(r.loaded, "the fixture must load clean: {:?}", r.errors);
        assert_eq!(
            r.missing_methods,
            vec!["NoSuchWidgetMethod".to_string()],
            "the invented method must be found and nothing else may be"
        );
        // The feature-tested one is its own row, not lost.
        assert_eq!(
            r.optional_methods,
            vec!["OptionalHook".to_string()],
            "a guarded call is its own row, never a silent omission"
        );
        // ...and it reaches the ranking; the library calls nothing.
        assert_eq!(
            method_demand(&reports),
            vec![("NoSuchWidgetMethod".to_string(), 1)]
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The per-kind census end to end over a real addon folder:
    ///
    /// | the call | what must happen |
    /// |---|---|
    /// | `smf:SetInsertMode()`, a ScrollingMessageFrame local | a row: this kind lacks it |
    /// | `mf:SetInsertMode()`, a MessageFrame local | no row: the control |
    /// | `UIParent:AddMessage()` | a row via the published name: a plain Frame |
    /// | `UIErrorsFrame:AddMessage()` | no row: ours is a `<MessageFrame>` |
    /// | `self:SetInsertMode()` | an ambiguous row: `self` is untyped |
    ///
    /// `ghost` is typed only inside a `--` comment and `faker` only inside a string, both as
    /// `Button`, so a leaking stripper would mint an otherwise impossible `Button:AddMessage` row.
    /// `missing_methods` stays empty: some widget answers every name here.
    #[test]
    fn the_per_kind_census_finds_a_verb_wired_to_the_wrong_kind() {
        benilla_formats::wow_data_or_skip!();
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-perkind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("PerKind");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("PerKind.toc"), "## Interface: 11200\na.lua\n").unwrap();
        // Never executed: the scan is static, and an unrun body keeps `loaded` clean.
        std::fs::write(
            dir.join("a.lua"),
            "local smf = CreateFrame(\"ScrollingMessageFrame\")\n\
             local mf = CreateFrame(\"MessageFrame\")\n\
             -- local ghost = CreateFrame(\"Button\")\n\
             local faker = \"CreateFrame('Button')\"\n\
             function PerKind_Never(self)\n\
             smf:SetInsertMode(\"TOP\")\n\
             mf:SetInsertMode(\"TOP\")\n\
             UIParent:AddMessage(\"x\")\n\
             UIErrorsFrame:AddMessage(\"x\")\n\
             self:SetInsertMode(\"TOP\")\n\
             ghost:AddMessage(\"x\")\n\
             faker:AddMessage(\"x\")\n\
             end\n",
        )
        .unwrap();

        let reports = survey(&tmp);
        let r = reports.iter().find(|r| r.name == "PerKind").unwrap();
        assert!(r.loaded, "the fixture must load clean: {:?}", r.errors);
        assert!(
            r.missing_methods.is_empty(),
            "every name here IS answered by some widget — which is the whole reason the ANY-kind \
             table cannot see this class: {:?}",
            r.missing_methods
        );
        assert_eq!(
            r.kind_missing_methods,
            vec![
                "Frame:AddMessage (on MessageFrame, ScrollingMessageFrame)".to_string(),
                "ScrollingMessageFrame:SetInsertMode (on MessageFrame)".to_string(),
            ],
            "the typed call sites, and only the ones whose kind cannot answer — no Button row, so \
             neither the comment nor the string literal was read as code"
        );
        // `self`, `ghost` and `faker` are untyped, so the name is reported as conditional.
        assert_eq!(
            r.ambiguous_methods,
            vec![
                "AddMessage (only on MessageFrame, ScrollingMessageFrame)".to_string(),
                "SetInsertMode (only on MessageFrame)".to_string(),
            ],
            "an untypable receiver is a stated unknown, never a silent pass"
        );
        // ...and it reaches the rankings.
        assert_eq!(
            kind_method_demand(&reports),
            vec![
                (
                    "Frame:AddMessage (on MessageFrame, ScrollingMessageFrame)".to_string(),
                    1
                ),
                (
                    "ScrollingMessageFrame:SetInsertMode (on MessageFrame)".to_string(),
                    1
                ),
            ]
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A name the file rebinds (shadowed, a `for` variable, a parameter) is not typed; the one
    /// bound once keeps its kind.
    #[test]
    fn a_rebound_name_is_not_typed() {
        let kinds = local_widget_kinds(
            "local kept = CreateFrame(\"MessageFrame\")\n\
             local shadowed = CreateFrame(\"MessageFrame\")\n\
             local shadowed = SomethingElse()\n\
             local looped = CreateFrame(\"MessageFrame\")\n\
             for _, looped in ipairs(t) do end\n\
             local passed = CreateFrame(\"MessageFrame\")\n\
             local function helper(passed) end\n",
        );
        assert_eq!(kinds.get("kept"), Some(&Some("MessageFrame")));
        for rebound in ["shadowed", "looped", "passed"] {
            assert_eq!(
                kinds.get(rebound),
                Some(&None),
                "{rebound} is bound twice, so nothing here can say what it holds"
            );
        }
    }

    /// A VM shaped like the survey's, for the oracle tests below.
    fn seated_vm() -> UiScript {
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        seat_a_session(&mut script);
        let _ = crate::ui_script::load_default_ui(&script);
        script
    }

    /// The seated session is visible through the Lua surface an addon sees, so a seat that silently
    /// fails to take is caught.
    #[test]
    fn the_seated_session_is_visible_from_lua() {
        let s = seated_vm();

        // The backpack: 16 slots, two filled.
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 16);
        assert_eq!(
            s.eval::<String>("return GetBagName(0)").unwrap(),
            "Backpack"
        );
        assert_eq!(
            s.eval::<i64>("local _, c = GetContainerItemInfo(0, 5) return c")
                .unwrap(),
            12,
            "the Linen Cloth stack"
        );
        // Bags 1..4 are not seated.
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(1)").unwrap(), 0);

        // The quest log through the API an addon walks: `GetNumQuestLogEntries` returns (rows,
        // quests), three rows but two quests, since the header is a row.
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumQuestLogEntries()")
                .unwrap(),
            (3, 2),
            "three rows, two quests — the header is a row"
        );
        assert_eq!(
            s.eval::<String>("local t = GetQuestLogTitle(1) return t")
                .unwrap(),
            "Elwynn Forest"
        );
        assert!(
            s.eval::<bool>("local _,_,_,h = GetQuestLogTitle(1) return h and true or false")
                .unwrap(),
            "row 1 is the header"
        );
        // Both ends of isComplete: nil for in-progress, 1 for complete.
        assert!(s
            .eval::<Option<i64>>("local _,_,_,_,_,c = GetQuestLogTitle(2) return c")
            .unwrap()
            .is_none());
        assert_eq!(
            s.eval::<i64>("local _,_,_,_,_,c = GetQuestLogTitle(3) return c")
                .unwrap(),
            1
        );
        // A leaderboard walk sees one finished objective and one not.
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetNumQuestLeaderBoards()").unwrap(),
            2
        );
        assert_eq!(
            s.eval::<(bool, bool)>(
                "local _,_,f1 = GetQuestLogLeaderBoard(1)                  local _,_,f2 = GetQuestLogLeaderBoard(2) return f1, f2"
            )
            .unwrap(),
            (false, true),
            "one objective outstanding, one done"
        );

        // The bind point, a string an addon concatenates (`Necrosis.lua:1089`).
        assert_eq!(
            s.eval::<String>("return GetBindLocation()").unwrap(),
            "Stormwind City"
        );

        // The purse, asserted as its three coin fields.
        assert_eq!(s.eval::<i64>("return GetMoney()").unwrap(), 12_345_678);
        assert_eq!(
            s.eval::<(i64, i64, i64)>(
                "local m = GetMoney()                  return floor(m / 10000), mod(floor(m / 100), 100), mod(m, 100)"
            )
            .unwrap(),
            (1234, 56, 78),
            "gold/silver/copper are each non-zero, so no field can be dropped unnoticed"
        );

        // Equipped gear: head, chest, main hand, the rest empty.
        assert_eq!(
            s.eval::<String>(r#"return GetInventoryItemTexture("player", 16)"#)
                .unwrap(),
            "Interface\\Icons\\INV_Misc_QuestionMark"
        );
        assert!(
            s.eval::<String>(r#"return GetInventoryItemLink("player", 5)"#)
                .unwrap()
                .contains("Bloodmail Hauberk"),
            "the chest slot's link"
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemQuality("player", 1)"#)
                .unwrap(),
            2
        );
        // An empty slot answers the absent shape.
        assert!(s
            .eval::<bool>(r#"return GetInventoryItemLink("player", 10) == nil"#)
            .unwrap());
        // The slot ids match `GetInventorySlotInfo`'s table.
        assert_eq!(
            s.eval::<i64>(r#"return GetInventorySlotInfo("MainHandSlot")"#)
                .unwrap(),
            16
        );

        // The spellbook.
        assert_eq!(
            s.eval::<String>(r#"return GetSpellName(1, "spell")"#)
                .unwrap(),
            "Attack"
        );
        assert_eq!(
            s.eval::<String>(r#"local _, r = GetSpellName(1, "spell") return r"#)
                .unwrap(),
            "",
            "and its rank is the empty string, not nil"
        );
    }

    fn wanted(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// An oracle that cannot probe (an addon may replace `CreateFrame`) reports every wanted name
    /// missing, never nothing; the first assertion is the live control.
    #[test]
    fn the_method_oracle_fails_loudly_when_it_cannot_probe() {
        let script = seated_vm();
        let want = wanted(&["SetWidth", "SetTexCoord"]);
        assert!(
            unresolved_from(&widget_method_kinds(&script, &want), &want).is_empty(),
            "a frame method and a region method we both ship must resolve"
        );

        script
            .run("CreateFrame = function() error('no frames for you') end")
            .unwrap();
        assert_eq!(
            unresolved_from(&widget_method_kinds(&script, &want), &want),
            vec!["SetTexCoord".to_string(), "SetWidth".to_string()],
            "with no probes there is no answer, and the honest report of no answer is the whole \
             wanted set"
        );
    }

    /// The per-kind pass fails loudly too: with the oracle dead, every typed call site is reported
    /// against its kind, and the ambiguous table is empty (an unknown answer is not conditional).
    #[test]
    fn the_per_kind_census_fails_loudly_when_it_cannot_probe() {
        let script = seated_vm();
        let wants = Wants {
            missing_globals: Vec::new(),
            missing_tables: Vec::new(),
            wanted_methods: wanted(&["SetWidth", "SetInsertMode"]),
            tested_methods: BTreeSet::new(),
            kind_calls: [("Frame".to_string(), "SetWidth".to_string())]
                .into_iter()
                .collect(),
            global_calls: BTreeSet::new(),
            loose_methods: wanted(&["SetInsertMode"]),
        };
        let (missing, ambiguous) = per_kind_rows(
            &script,
            &widget_method_kinds(&script, &wanted(&["SetWidth", "SetInsertMode"])),
            &wants,
        );
        assert!(
            missing.is_empty(),
            "a live oracle answers SetWidth on a Frame: {missing:?}"
        );
        assert_eq!(
            ambiguous,
            vec!["SetInsertMode (only on MessageFrame)".to_string()],
            "...and the untypable one is reported as conditional, with the kind named"
        );

        // Now break it: `None` is the oracle saying it could not run.
        let (missing, ambiguous) = per_kind_rows(&script, &None, &wants);
        assert_eq!(
            missing,
            vec!["Frame:SetWidth (on no kind)".to_string()],
            "with no answer at all, every typed call site is reported — loud, not silent"
        );
        assert!(
            ambiguous.is_empty(),
            "and nothing is called merely conditional when nothing is known: {ambiguous:?}"
        );
    }

    /// A method on a sibling kind is invisible to the any-kind table and found per kind:
    /// `SetInsertMode` is a MessageFrame binding only, so a ScrollingMessageFrame call to it is a
    /// row, and the same call on a MessageFrame is none.
    #[test]
    fn a_method_on_a_sibling_kind_is_invisible_to_any_and_found_per_kind() {
        let script = seated_vm();
        let want = wanted(&["SetInsertMode"]);
        let oracle = widget_method_kinds(&script, &want);
        assert!(
            unresolved_from(&oracle, &want).is_empty(),
            "the ANY-kind table answers `some widget has it`, so a per-kind gap is still not what \
             IT finds — and that number must not move"
        );

        let called_on = |kind: &str| {
            per_kind_rows(
                &script,
                &oracle,
                &Wants {
                    missing_globals: Vec::new(),
                    missing_tables: Vec::new(),
                    wanted_methods: want.clone(),
                    tested_methods: BTreeSet::new(),
                    kind_calls: [(kind.to_string(), "SetInsertMode".to_string())]
                        .into_iter()
                        .collect(),
                    global_calls: BTreeSet::new(),
                    loose_methods: BTreeSet::new(),
                },
            )
            .0
        };
        assert_eq!(
            called_on("ScrollingMessageFrame"),
            vec!["ScrollingMessageFrame:SetInsertMode (on MessageFrame)".to_string()],
            "the row 1228 could not print: the sibling that DOES answer it is named, because \
             `wire the kind` and `write the verb` are different jobs"
        );
        assert!(
            called_on("MessageFrame").is_empty(),
            "...and the kind that answers it produces nothing — the pass is not just over-reporting"
        );
    }

    /// A published name's kind is read off the live arena: our `UIErrorsFrame` is a
    /// `<MessageFrame>`, `UIParent` a plain `<Frame>`, `GameTooltipTextLeft1` a region, and an
    /// unpublished name `None`.
    #[test]
    fn a_published_name_carries_its_kind() {
        benilla_formats::wow_data_or_skip!();
        let script = seated_vm();
        assert_eq!(script.widget_kind("UIErrorsFrame"), Some("MessageFrame"));
        assert_eq!(script.widget_kind("UIParent"), Some("Frame"));
        assert_eq!(
            script.widget_kind("ChatFrame1"),
            Some("ScrollingMessageFrame")
        );
        assert_eq!(script.widget_kind("GameTooltip"), Some("GameTooltip"));
        assert_eq!(
            script.widget_kind("GameTooltipTextLeft1"),
            Some("FontString"),
            "the region leaves publish into their own name table, and the corpus scrapes them"
        );
        assert_eq!(script.widget_kind("NoSuchFrameAnywhere"), None);
    }

    /// The seated session answers the login-scoped catalogues at file scope: `Auctioneer`
    /// subscripts `classes[10]` (`AucCore.lua:106`) and `KLHThreatMeter` negates a talent rank
    /// (`KTM_My.lua:721`). The contents come off the player's DBCs, so the reads are asserted, not
    /// the data.
    #[test]
    fn the_seated_session_answers_the_login_scoped_catalogues() {
        if benilla_formats::wow_data().is_none() {
            eprintln!("skipping: no WoW client data");
            return;
        }
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        seat_a_session(&mut script);

        // Auctioneer's line, reduced: ten subscripts into the packed return.
        assert_eq!(
            script
                .eval::<i64>(
                    "local c = {GetAuctionItemClasses()} \
                     local t = {} for i = 1, 10 do t[c[i]] = true end \
                     return table.getn(c)"
                )
                .ok(),
            Some(10),
            "the browse tree is the reference's ten auctionable classes"
        );
        // KLHThreatMeter's line, reduced: a positional read and arithmetic on the rank.
        assert_eq!(
            script
                .eval::<i64>("local _, _, _, _, rank = GetTalentInfo(3, 13) return -rank")
                .ok(),
            Some(0),
            "a talent read by POSITION answers a number — nil is what the corpus was getting"
        );
        assert!(
            script
                .eval::<bool>("return GetNumTalentTabs() == 3 and GetNumTalents(1) > 0")
                .unwrap_or(false),
            "the seated warrior has his three pages"
        );
    }

    /// The UI probe invokes an addon's override: a hook installed as Bagnon installs one is
    /// counted.
    #[test]
    fn the_ui_probe_reaches_an_addon_override() {
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        seat_a_session(&mut script);
        let _ = crate::ui_script::load_default_ui(&script);

        // Bagnon's idiom, on the verb Bagnon replaces.
        script
            .run("BENILLA_PROBE_HITS = 0 ToggleBackpack = function() BENILLA_PROBE_HITS = BENILLA_PROBE_HITS + 1 end")
            .unwrap();

        let errs = drive_ui_probe(&mut script);
        assert!(errs.is_empty(), "the probe must not raise here: {errs:?}");
        assert_eq!(
            script.eval::<i64>("return BENILLA_PROBE_HITS").ok(),
            Some(2),
            "the probe drives ToggleBackpack twice (open, then the override's close path)"
        );
    }

    /// And it records a raise rather than swallowing it.
    #[test]
    fn the_ui_probe_records_what_an_override_raises() {
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        seat_a_session(&mut script);
        let _ = crate::ui_script::load_default_ui(&script);
        script
            .run("ToggleBackpack = function() error('addon blew up') end")
            .unwrap();

        let errs = drive_ui_probe(&mut script);
        assert!(
            errs.iter().any(|e| e.contains("addon blew up")),
            "a raising override must land in the probe column: {errs:?}"
        );
    }

    /// The hover arm shows the tooltip, so a hook on `GameTooltip`'s `OnShow` runs and its raise is
    /// recorded.
    #[test]
    fn the_ui_probe_hovers_and_records_what_a_tooltip_hook_raises() {
        benilla_formats::wow_data_or_skip!();
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        seat_a_session(&mut script);
        let _ = crate::ui_script::load_default_ui(&script);
        // The corpus shape: hook the tooltip's own OnShow, which runs only if something shows it.
        script
            .run(
                "GameTooltip:SetScript(\"OnShow\", function() error(\"tooltip hook blew up\") end)",
            )
            .unwrap();

        let errs = drive_ui_probe(&mut script);
        assert!(
            errs.iter().any(|e| e.contains("tooltip hook blew up")),
            "the hover arm must actually show the tooltip, and report the hook's raise: {errs:?}"
        );
    }

    /// ...and the probe raises nothing on a clean VM with no addon loaded, having driven every
    /// entry point: `ToggleBackpack` comes from the stock `ContainerFrame.lua:67` off the player's
    /// install, so the entry points are asserted to exist first and the test needs client data.
    #[test]
    fn the_ui_probe_is_silent_on_a_clean_vm() {
        let _data = benilla_formats::wow_data_or_skip!();
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        seat_a_session(&mut script);
        let _ = crate::ui_script::load_default_ui(&script);
        // Each entry point is `try`-guarded in `drive_ui_probe`, so a missing one would pass
        // silently.
        for entry in [
            "ToggleBackpack",
            "UnitFrame_OnEnter",
            "UnitFrame_OnLeave",
            "ActionButton_Update",
            "GameTooltip_SetDefaultAnchor",
        ] {
            assert_eq!(
                script
                    .eval::<String>(&format!("return type({entry})"))
                    .unwrap(),
                "function",
                "{entry} is not in the VM, so the probe would skip it and this test would pass \
                 without driving anything"
            );
        }

        let errs = drive_ui_probe(&mut script);
        assert!(
            errs.is_empty(),
            "no addon is loaded — every probe error here would be charged to somebody else: {errs:?}"
        );
    }
}
