//! The control for the survey's one-VM-per-addon bound: the whole folder in one VM, as a real
//! client runs it, walked in load order with each `ADDON_LOADED` at its own position, then one
//! session start. Rows differ from the survey's where an addon relies on a library a neighbour
//! ships.
//!
//! It cannot attribute render or use results, and cannot tell clean from never reached;
//! attribution is by the raising chunk alone.
//!
//! A shared VM is not reproducible: Lua hashes a table key by its pointer, so `pairs()` over an
//! object-keyed registry (every Ace2 library's) walks in a different order per process under
//! ASLR, as the reference's Lua does. So the walk runs [`DEFAULT_RUNS`] times in fresh VMs and
//! each row reports how many runs raised: every run is a real failure, some is order-sensitive.
//!
//! `## LoadOnDemand: 1` is ignored: the reference's boot walk skips those (`0x51f600` loads only
//! records whose LoadOnDemand byte is 0), but corpus addons demand-load them at once (FuBar's
//! `LoadLoadOnDemandPlugins`, Auctioneer's stub), so honouring the flag would model a shorter
//! session than anyone plays.

use std::collections::BTreeSet;
use std::path::Path;

use benilla_ui::script::UiScript;

use super::{
    corpus, load_addon_files, load_dependencies, manifest_path, LoadedDep, ADDON_INSTRUCTION_BUDGET,
};
use benilla_ui::toc::Toc;

/// How many fresh-VM runs [`survey_together`] makes: three is the fewest that tell "always" from
/// "sometimes".
pub const DEFAULT_RUNS: usize = 3;

/// One addon's verdict in the shared VM.
pub struct TogetherRow {
    pub name: String,
    /// Raises whose first stack frame is in this addon's folder, load and session together, from
    /// the first run that produced any, so the text is one real traceback.
    pub errors: Vec<String>,
    /// How many of the [`runs`](Self::runs) raised at all.
    pub raised_in: usize,
    pub runs: usize,
}

impl TogetherRow {
    /// Raised in every run: a failure the neighbours do not fix.
    pub fn always_raises(&self) -> bool {
        self.raised_in == self.runs
    }
    /// Raised in some runs and not others: the pointer-hash order showing through.
    pub fn order_sensitive(&self) -> bool {
        self.raised_in > 0 && self.raised_in < self.runs
    }
}

/// Load every addon under `root` into one VM and drive the session start, [`DEFAULT_RUNS`] times
/// in fresh VMs.
pub fn survey_together(root: &Path) -> Vec<TogetherRow> {
    let mut merged: Vec<TogetherRow> = Vec::new();
    for run in 0..DEFAULT_RUNS {
        let rows = survey_together_once(root);
        if run == 0 {
            merged = rows
                .into_iter()
                .map(|(name, errors)| TogetherRow {
                    name,
                    raised_in: usize::from(!errors.is_empty()),
                    errors,
                    runs: DEFAULT_RUNS,
                })
                .collect();
            continue;
        }
        for (name, errors) in rows {
            let Some(row) = merged.iter_mut().find(|r| r.name == name) else {
                continue;
            };
            if errors.is_empty() {
                continue;
            }
            row.raised_in += 1;
            if row.errors.is_empty() {
                row.errors = errors;
            }
        }
    }
    merged
}

/// One walk in one VM.
fn survey_together_once(root: &Path) -> Vec<(String, Vec<String>)> {
    let (names, installed, registry) = corpus(root);
    let Ok(mut script) = UiScript::new() else {
        return Vec::new();
    };
    script.set_screen_size(1024.0, 768.0);
    script.register_addons(registry, Some(root.to_path_buf()), None, None);
    super::seat_a_session(&mut script);
    let _ = crate::ui_script::load_default_ui(&script);

    // `seen` spans the whole walk: a library declared by many dependents loads once.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // `(folder, its load failures)`, kept beside the walk: the VM's diagnostics log caps at 256
    // distinct rows and evicts.
    let mut load_errors: Vec<(String, Vec<String>)> = Vec::new();
    for name in &names {
        let Some(toc) = manifest_path(root, name)
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| Toc::parse(&benilla_ui::source::decode(&b)))
        else {
            continue;
        };
        if !seen.insert(name.to_ascii_lowercase()) {
            continue; // already loaded as a dependency
        }
        // Re-armed per addon, as the live walk does, so one runaway does not starve the rest.
        script.set_instruction_budget(ADDON_INSTRUCTION_BUDGET);
        let mut pulled: Vec<LoadedDep> = Vec::new();
        load_dependencies(&mut script, root, &toc, &installed, &mut seen, &mut pulled);
        // A dependency's load failures are collected only here: it loads once, under the first
        // dependent to reach it, and never gets a turn of its own.
        for dep in pulled {
            load_errors.push((dep.name, dep.files.errors));
        }
        let files = load_addon_files(&script, root, name, &toc);
        load_errors.push((name.clone(), files.errors));
        script.mark_addon_loaded(name);
        script.fire_event(
            "ADDON_LOADED",
            vec![benilla_ui::script::ScriptValue::Str(name.clone())],
        );
    }
    script.set_instruction_budget(ADDON_INSTRUCTION_BUDGET);
    for event in ["VARIABLES_LOADED", "PLAYER_LOGIN", "PLAYER_ENTERING_WORLD"] {
        script.fire_event(event, Vec::new());
    }
    for _ in 0..10 {
        script.tick(0.1);
    }

    // Attribution by the raising chunk, matched as `\<Folder>\` rather than a prefix: Lua
    // truncates a long chunk name from the left (`...Ons\FuBar_DakSmak\Libs\…`).
    let mut rows: Vec<(String, Vec<String>)> =
        names.iter().map(|n| (n.clone(), Vec::new())).collect();
    // Load failures first, attributed by whose manifest was walked; a missing named file is not a
    // raise, as in the survey.
    for (folder, errs) in load_errors {
        let Some(row) = rows.iter_mut().find(|r| r.0 == folder) else {
            continue;
        };
        row.1
            .extend(errs.into_iter().filter(|e| !is_absent_file(e)));
    }
    for err in script.errors() {
        let Some(first) = err.lines().next().map(str::to_ascii_lowercase) else {
            continue;
        };
        // Longest folder name first, so `FuBar` never claims a raise inside `FuBar_AtlasFu`.
        let mut owner: Option<usize> = None;
        for (i, row) in rows.iter().enumerate() {
            let folder = row.0.to_ascii_lowercase();
            let hit = first.contains(&format!("\\{folder}\\"))
                || first.contains(&format!("{folder}/"))
                || first.starts_with(&format!("{folder}\\"));
            if hit && owner.is_none_or(|o| rows[o].0.len() < row.0.len()) {
                owner = Some(i);
            }
        }
        if let Some(i) = owner {
            rows[i].1.push(err);
        }
    }
    rows
}

/// A load failure that is only a file the package does not contain: the reference logs
/// `Couldn't open %s` and carries on.
fn is_absent_file(err: &str) -> bool {
    err.ends_with(": not found")
        || err.contains("no provider hit for")
        || err.contains("; the whole document it names is missing")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An addon short a library its neighbour ships fails alone and is clean together.
    #[test]
    fn a_library_a_neighbour_ships_is_there_in_the_shared_vm() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-harness-together-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |name: &str, toc: &str, file: &str, body: &str| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
            std::fs::write(dir.join(file), body).unwrap();
        };
        // Loads first (`A` before `Z` under the walk's NTFS collation) and puts the library in the
        // global state, as corpus packages that ship `Tablet-2.0` do.
        write(
            "AlphaShipsIt",
            "## Interface: 11200\nlib.lua\n",
            "lib.lua",
            "SharedLibrary = { greet = function() return 1 end }\n",
        );
        // Declares no dependency on it and does not ship it.
        write(
            "ZuluWantsIt",
            "## Interface: 11200\nuse.lua\n",
            "use.lua",
            "ZuluSaw = SharedLibrary.greet()\n",
        );

        let alone = crate::addon_harness::survey(&tmp);
        let zulu = alone.iter().find(|r| r.name == "ZuluWantsIt").unwrap();
        assert!(
            !zulu.loaded,
            "ALONE it must fail — otherwise this control is measuring nothing: {:?}",
            zulu.errors
        );

        let rows = survey_together(&tmp);
        let of = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        assert_eq!(
            of("ZuluWantsIt").raised_in,
            0,
            "TOGETHER the neighbour's library is simply there, in every run: {:?}",
            of("ZuluWantsIt").errors
        );
        assert_eq!(
            of("AlphaShipsIt").raised_in,
            0,
            "and the provider is unaffected: {:?}",
            of("AlphaShipsIt").errors
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A raise in the shared VM is still attributed to its folder.
    #[test]
    fn a_raise_in_the_shared_vm_lands_on_the_folder_that_raised() {
        let tmp = std::env::temp_dir().join(format!(
            "benilla-harness-together-attrib-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        for (name, body) in [("Quiet", "QuietGlobal = 1\n"), ("Loud", "error('boom')\n")] {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{name}.toc")),
                "## Interface: 11200\nf.lua\n",
            )
            .unwrap();
            std::fs::write(dir.join("f.lua"), body).unwrap();
        }
        let rows = survey_together(&tmp);
        let of = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        assert!(
            of("Loud").always_raises() && of("Loud").errors.iter().any(|e| e.contains("boom")),
            "the raise is the loud one's, in every run: {:?}",
            of("Loud").errors
        );
        assert_eq!(
            of("Quiet").raised_in,
            0,
            "and not its neighbour's: {:?}",
            of("Quiet").errors
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
