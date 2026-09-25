//! `addon_harness`: load a folder of addons, one per VM, and print what happened.
//!
//! ```text
//! cargo run -q -p benilla-app --example addon_harness -- <folder> [--verbose] [--why <substr>]
//!     [--deep [n]] [--status <file>] [--diff <file>]
//!   or: ... -- <folder> --probe <Name> [--eval <lua> | --mouse <x>,<y> | --tick <secs>]...
//!     (one addon, then ask its VM; the steps run in the order given)
//! ```
//!
//! Measures which addons work, as a number that can be re-read any day; what the numbers are
//! worth is in [`benilla_app::addon_harness`]'s module doc. Expect a long tail: the report is a
//! distribution, not a pass/fail.
use benilla_app::addon_harness;

/// How much traceback `--why` prints per addon: the addon, file and line sit below the first frame.
const WHY_TRACEBACK_LINES: usize = 8;

/// Print a ranked table, stating how many rows and addon-mentions fell below the cut; `--deep [n]`
/// overrides every caller's `take` (see [`DEEP`]).
fn ranked(rows: Vec<(String, usize)>, take: usize) {
    let take = DEEP.get().copied().flatten().unwrap_or(take);
    let total = rows.len();
    for (name, count) in rows.iter().take(take) {
        println!("    {count:>4}  {name}");
    }
    if total > take {
        let tail: usize = rows[take..].iter().map(|(_, c)| c).sum();
        println!(
            "    …{} more rows below the cut ({tail} addon-mentions) — TRUNCATED, not exhausted; \
             `--why <name>` opens any row.",
            total - take
        );
    }
}

/// One of the probe's two error lists with its count, printed even when empty, so "loaded clean
/// and died in a handler" reads apart from "never loaded".
fn report_lines(label: &str, lines: &[String]) {
    println!("  {label}: {}", lines.len());
    for line in lines {
        for (n, l) in line.lines().enumerate() {
            println!("    {}{}", if n == 0 { "" } else { "  " }, l.trim_end());
        }
    }
}

/// The `--deep` override, set once in `main`: `Some(n)` shows `n` rows of every ranked list,
/// a bare `--deep` all of them; absent, each list keeps its caller's `take`.
static DEEP: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();

/// The row's frame names, bounded for the line and saying so when it bounds.
fn render_frames(frames: &[String]) -> String {
    use benilla_app::addon_harness::render::MAX_NAMED_FRAMES;
    // `--deep` opens this bound too.
    let cap = DEEP.get().copied().flatten().unwrap_or(MAX_NAMED_FRAMES);
    if frames.len() <= cap {
        return frames.join(",");
    }
    format!("{},+{} more", frames[..cap].join(","), frames.len() - cap)
}

/// Our `_G` against the captured 1.12 `_G` (`reference/1.12-globals.tsv`). Both directions
/// matter: 1.12 addons feature-test with `if SomeName then`, so a name we publish that the
/// reference lacks can send an addon down a path the real client never takes. `lod` rows are the
/// twelve LoadOnDemand `Blizzard_*` addons' names, counted apart as a live capture misses them.
fn surface_report(deep: bool, dump_path: Option<String>) {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-globals.tsv"
    );
    let Ok(text) = std::fs::read_to_string(tsv) else {
        eprintln!("cannot read the reference surface at {tsv}");
        std::process::exit(1);
    };

    let mut reference: std::collections::BTreeMap<String, String> = Default::default();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let mut f = line.split('\t');
        if let (Some(name), Some(kind)) = (f.next(), f.next()) {
            if !name.is_empty() {
                reference.insert(name.to_string(), kind.to_string());
            }
        }
    }

    let ours: std::collections::BTreeMap<String, String> =
        addon_harness::surface().into_iter().collect();

    // `--surface-dump <file>` writes our raw `_G` as `name<TAB>type`, the reference TSV's shape.
    if let Some(path) = dump_path {
        let body: String = ours
            .iter()
            .map(|(n, k)| format!("{n}\t{k}\n"))
            .collect::<Vec<_>>()
            .concat();
        match std::fs::write(&path, format!("# our _G — {} names\n{body}", ours.len())) {
            Ok(()) => println!("  wrote {} names to {path}", ours.len()),
            Err(e) => eprintln!("  could not write {path}: {e}"),
        }
    }

    println!("\n  SURFACE DIFF — our _G vs the captured 1.12 _G");
    println!("  FrameXML digest : {}", addon_harness::framexml_digest());
    println!("  reference names : {}", reference.len());
    println!("  ours            : {}", ours.len());

    // Absent from us, by the reference's own type.
    let mut missing_by_kind: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (name, kind) in &reference {
        if !ours.contains_key(name) {
            missing_by_kind
                .entry(kind.as_str())
                .or_default()
                .push(name.as_str());
        }
    }
    println!("\n  ABSENT FROM US, by the reference's type:");
    for (kind, names) in &missing_by_kind {
        println!("    {:<10} {}", kind, names.len());
    }

    // Published by us and absent from 1.12, split by our type: most are `table`, our FrameXML's
    // own frame names; a `function` is the hazard, since addons feature-test on it.
    let extra: Vec<(&str, &str)> = ours
        .iter()
        .filter(|(n, _)| !reference.contains_key(n.as_str()))
        .map(|(n, k)| (n.as_str(), k.as_str()))
        .collect();
    let mut extra_by_kind: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (name, kind) in &extra {
        extra_by_kind.entry(kind).or_default().push(name);
    }
    println!(
        "\n  PUBLISHED BY US, ABSENT FROM 1.12 ({}) — a superset is not free:",
        extra.len()
    );
    for (kind, names) in &extra_by_kind {
        let note = match *kind {
            "function" => "  <- the ones an addon feature-tests; read these first",
            "table" => "  <- mostly our own FrameXML's frame/region names",
            _ => "",
        };
        println!("    {:<10} {}{}", kind, names.len(), note);
    }
    // Functions in full even without --deep, split on the `Benilla` prefix: no 1.12 addon
    // feature-tests our own namespace.
    if let Some(fns) = extra_by_kind.get("function") {
        let (ours_ns, unprefixed): (Vec<&&str>, Vec<&&str>) = fns
            .iter()
            .partition(|n| n.starts_with("Benilla") || n.starts_with("BENILLA"));
        println!(
            "\n  ...of those functions: {} are Benilla*-namespaced (cannot collide), {} are NOT:",
            ours_ns.len(),
            unprefixed.len()
        );
        for chunk in unprefixed.chunks(4) {
            let row: Vec<&str> = chunk.iter().map(|s| **s).collect();
            println!("      {}", row.join("  "));
        }
        println!(
            "    ^ read these: an unprefixed verb 1.12 lacks is what `if SomeName then` finds.\n      \
             Most are our own UI's helpers (KeyBindings_*, Options*), which are only\n      \
             a naming question — but a POST-1.12 API name here is the hazard, because an\n      \
             addon that feature-tests it takes a branch written for a client we are not."
        );
    }
    if deep {
        for (kind, names) in &extra_by_kind {
            if *kind == "function" {
                continue;
            }
            println!("\n  ...EXTRA {} ({}):", kind, names.len());
            for chunk in names.chunks(4) {
                println!("      {}", chunk.join("  "));
            }
        }
    }

    if deep {
        for (kind, names) in &missing_by_kind {
            println!("\n  ABSENT FROM US — {} ({}):", kind, names.len());
            for chunk in names.chunks(4) {
                println!("      {}", chunk.join("  "));
            }
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!(
            "usage: addon_harness <folder of addons> [--verbose] [--why <blocker substring>]\n   \
             or: addon_harness --surface   (our _G vs the captured 1.12 surface; takes no corpus)"
        );
        std::process::exit(2);
    };
    let rest: Vec<String> = args.collect();

    // `--surface` needs no corpus, so it is handled before the root is used.
    if root == "--surface" || rest.iter().any(|a| a == "--surface") {
        let dump_path = rest
            .iter()
            .position(|a| a == "--surface-dump")
            .and_then(|i| rest.get(i + 1))
            .cloned();
        surface_report(rest.iter().any(|a| a == "--deep"), dump_path);
        return;
    }

    let verbose = rest.iter().any(|a| a == "--verbose");
    // `--why <substring>`: the addons behind one row, with their verbatim errors. Matches the
    // normalised row and the raw text, and reads back through the method demand tables too.
    let why = rest
        .iter()
        .position(|a| a == "--why")
        .and_then(|i| rest.get(i + 1))
        .cloned();
    // `--status <file>` writes the per-addon ok/fail roster to a file; `--diff <file>` compares
    // against one, since a total hides a fix and a break that cancel out.
    let status = rest
        .iter()
        .position(|a| a == "--status")
        .and_then(|i| rest.get(i + 1))
        .cloned();
    let diff = rest
        .iter()
        .position(|a| a == "--diff")
        .and_then(|i| rest.get(i + 1))
        .cloned();
    // `--deep [n]`: open the tails. A bare `--deep` (or one followed by the next flag) means all.
    let deep = rest.iter().position(|a| a == "--deep").map(|i| {
        rest.get(i + 1)
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(usize::MAX)
    });
    let _ = DEEP.set(deep);
    let root = std::path::PathBuf::from(root);

    // `--together`: the whole folder in one VM, the control for the survey's one-VM-per-addon
    // bound; `--diff <a survey roster>` names the rows the bound costs.
    if rest.iter().any(|a| a == "--together") {
        let rows = addon_harness::together::survey_together(&root);
        if rows.is_empty() {
            eprintln!(
                "no addons under {} — is that an AddOns folder?",
                root.display()
            );
            std::process::exit(1);
        }
        let raised: Vec<&addon_harness::together::TogetherRow> =
            rows.iter().filter(|r| r.always_raises()).collect();
        let wobbly: Vec<&addon_harness::together::TogetherRow> =
            rows.iter().filter(|r| r.order_sensitive()).collect();
        println!(
            "\n{} addon(s) under {}, ALL IN ONE VM — {} runs",
            rows.len(),
            root.display(),
            addon_harness::together::DEFAULT_RUNS
        );
        println!(
            "  raised in EVERY run : {}/{}  (clean in every run: {})",
            raised.len(),
            rows.len(),
            rows.len() - raised.len() - wobbly.len()
        );
        // Named apart: Lua hashes a table key by its pointer, so a registry keyed by objects walks
        // in a different order every process.
        println!(
            "  ORDER-SENSITIVE — raised in some runs, not all ({}):",
            wobbly.len()
        );
        for r in &wobbly {
            println!(
                "    {:<28} {}/{}  {}",
                r.name,
                r.raised_in,
                r.runs,
                r.errors[0].lines().next().unwrap_or("")
            );
        }
        if let Some(path) = &diff {
            // `fail` in the survey and clean here is the bound, priced.
            let prior: std::collections::BTreeMap<String, bool> = std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.rsplit_once(' '))
                .map(|(n, v)| (n.trim().to_string(), v.trim() == "ok"))
                .collect();
            let mut freed: Vec<&str> = Vec::new();
            let mut only_together: Vec<&str> = Vec::new();
            for r in &rows {
                // Only rows the same in every run are compared.
                match (prior.get(&r.name), r.raised_in) {
                    (Some(false), 0) => freed.push(&r.name),
                    (Some(true), n) if n == r.runs => only_together.push(&r.name),
                    _ => {}
                }
            }
            println!(
                "\n  FAILS ALONE, CLEAN TOGETHER ({}) — the one-VM bound, priced:",
                freed.len()
            );
            for n in &freed {
                println!("    {n}");
            }
            println!(
                "\n  CLEAN ALONE, RAISES TOGETHER ({}) — a neighbour's global, or an order the\n                 \x20 survey never reaches:",
                only_together.len()
            );
            for n in &only_together {
                println!("    {n}");
            }
        }
        println!(
            "\n  STILL RAISING IN EVERY RUN, WITH EVERY NEIGHBOUR PRESENT (first error each):"
        );
        for r in &raised {
            let first = r.errors[0].lines().next().unwrap_or("");
            println!("    {:<28} {first}", r.name);
        }
        return;
    }

    // `--probe <Name> [--eval <lua> ...]`: one addon, loaded as the survey loads it, then asked.
    // Not a measurement (an eval can mutate the VM), so it prints no column.
    if let Some(name) = rest
        .iter()
        .position(|a| a == "--probe")
        .and_then(|i| rest.get(i + 1))
    {
        // `--eval`, `--tick` and `--mouse` are one ordered list, run in the order typed.
        let steps: Vec<addon_harness::probe::Step> = rest
            .iter()
            .enumerate()
            .filter_map(|(i, a)| match a.as_str() {
                "--eval" => Some(addon_harness::probe::Step::Eval(rest.get(i + 1)?.clone())),
                "--tick" => Some(addon_harness::probe::Step::Tick(
                    rest.get(i + 1)?.parse().ok()?,
                )),
                "--mouse" => {
                    let (x, y) = rest.get(i + 1)?.split_once(',')?;
                    Some(addon_harness::probe::Step::Mouse(
                        x.trim().parse().ok()?,
                        y.trim().parse().ok()?,
                    ))
                }
                _ => None,
            })
            .collect();
        let Some(out) = addon_harness::probe::probe(&root, name, &steps) else {
            eprintln!(
                "no manifest under {}/{name} — is that an addon folder?",
                root.display()
            );
            std::process::exit(1);
        };
        println!("\n{} — probed under {}", out.name, root.display());
        report_lines("load errors", &out.load_errors);
        report_lines("session errors", &out.session_errors);
        if out.answers.is_empty() {
            println!("  (no --eval/--mouse given — load and session errors only)");
        }
        for (chunk, answer) in &out.answers {
            println!("\n  {chunk}");
            println!("    {answer}");
        }
        return;
    }

    let reports = addon_harness::survey(&root);
    if reports.is_empty() {
        eprintln!(
            "no addons under {} — is that an AddOns folder?",
            root.display()
        );
        std::process::exit(1);
    }

    let loaded = reports.iter().filter(|r| r.loaded).count();
    let clean = reports
        .iter()
        .filter(|r| r.loaded && r.missing_globals.is_empty())
        .count();
    let blocked = reports
        .iter()
        .filter(|r| !r.missing_deps.is_empty())
        .count();

    println!("\n{} addon(s) under {}", reports.len(), root.display());
    // Without an install there is no GlobalStrings.lua and about 5,000 globals are missing.
    println!(
        "  VM: the stock 1.12 FrameXML off the player's chain + a seated session{}\n",
        if addon_harness::seated_with_global_strings() {
            " + the real GlobalStrings.lua"
        } else {
            "  ** no install found: GlobalStrings absent, these numbers are NOT comparable **"
        }
    );
    // Two runs compare only at the same digest: a dev build reads `assets/ui` from the source tree.
    println!(
        "  FrameXML digest                    : {}",
        addon_harness::framexml_digest()
    );
    println!(
        "  loaded without a single load error : {loaded}/{}",
        reports.len()
    );
    // `loaded` counts what raised; a manifest entry naming a missing file is not that (the
    // reference logs `Couldn't open %s` and carries on), so the stricter count prints beside it.
    let strict = reports.iter().filter(|r| r.errors.is_empty()).count();
    println!(
        "      (…{} of those name a file their own package does not contain, which the reference \
         logs and carries on from; the strict column counts those as failures: {strict}/{})",
        loaded - strict,
        reports.len()
    );
    println!(
        "  ...and calling nothing we lack     : {clean}/{}",
        reports.len()
    );
    println!("  with a dependency not installed    : {blocked}");
    // The session column: drives ADDON_LOADED -> VARIABLES_LOADED -> PLAYER_LOGIN ->
    // PLAYER_ENTERING_WORLD and a second of ticks, and reports what the handlers raised.
    let survived = reports
        .iter()
        .filter(|r| r.loaded && r.session_errors.is_empty())
        .count();
    println!(
        "  ...and survived a session start    : {survived}/{}",
        reports.len()
    );
    // The UI-probe column: of those, how many survive having their overrides invoked.
    let probed = reports
        .iter()
        .filter(|r| r.loaded && r.session_errors.is_empty() && r.probe_errors.is_empty())
        .count();
    println!(
        "  ...and survived a UI probe         : {probed}/{}",
        reports.len()
    );
    // The render column, the only one that asks whether anything was drawn; see
    // `addon_harness::render`'s header.
    let drew = reports
        .iter()
        .filter(|r| r.render.drew() != addon_harness::Drew::Nothing)
        .count();
    println!(
        "  ...and DREW something on screen    : {drew}/{}",
        reports.len()
    );
    let (own, overlay) = (
        reports
            .iter()
            .filter(|r| r.render.drew() == addon_harness::Drew::Own)
            .count(),
        reports
            .iter()
            .filter(|r| r.render.drew() == addon_harness::Drew::Overlay)
            .count(),
    );
    println!("      of those: {own} drew a window of their own, {overlay} painted onto ours");
    // Addons that load, survive a session start and a UI probe, and still draw nothing.
    let silent: Vec<&str> = reports
        .iter()
        .filter(|r| {
            r.loaded
                && r.session_errors.is_empty()
                && r.probe_errors.is_empty()
                && r.render.drew() == addon_harness::Drew::Nothing
        })
        .map(|r| r.name.as_str())
        .collect();
    if !silent.is_empty() {
        // A question, not a defect list: the survey seats a player in an empty world, so an addon
        // with nothing to draw there (a buff bar, a pure library) draws nothing correctly.
        println!(
            "\n  DREW NOTHING, and clean on every other column ({}) — a QUESTION, not a defect\n  \
             list: the seated world has one buff, one target and one running cooldown, so an addon\n  \
             waiting on combat, a cursor over an item or a slash command belongs here too:",
            silent.len()
        );
        for chunk in silent.chunks(4) {
            println!("    {}", chunk.join("  "));
        }
    }

    // The use column, the only one that touches anything: three numbers, because "nothing
    // raised" and "nothing was touched" are different answers (`addon_harness::use_probe`).
    let (survived_use, raised_use, untouched_use) = (
        reports
            .iter()
            .filter(|r| r.used.verdict() == addon_harness::Used::Survived)
            .count(),
        reports
            .iter()
            .filter(|r| r.used.verdict() == addon_harness::Used::Raised)
            .count(),
        reports
            .iter()
            .filter(|r| {
                r.render.drew() != addon_harness::Drew::Nothing
                    && r.used.verdict() == addon_harness::Used::Untouched
            })
            .count(),
    );
    println!(
        "  ...and SURVIVED BEING USED         : {survived_use}/{}",
        reports.len()
    );
    println!(
        "      of the {drew} that drew: {} raised on hover/click/drag, {survived_use} came \
         through clean, {untouched_use} had nothing a pointer can reach",
        raised_use
    );
    println!(
        "      (input driven: hover in+out, left click, right click, drag — up to {} of the \
         frames each addon painted itself)",
        addon_harness::MAX_USE_TARGETS
    );
    // Addons whose UI is fully on screen and falls over the moment anyone uses it.
    let inert: Vec<String> = reports
        .iter()
        .filter(|r| r.used.verdict() == addon_harness::Used::Raised)
        .map(|r| format!("{}({})", r.name, r.used.errors.len()))
        .collect();
    if !inert.is_empty() {
        println!(
            "\n  DREW SOMETHING AND IS DEAD TO THE TOUCH ({}) — (n) = raises, `--why <name>` for \
             the text:",
            inert.len()
        );
        for chunk in inert.chunks(4) {
            println!("    {}", chunk.join("  "));
        }
    }
    // ...and what the input broke, ranked like the session-start table above it.
    let mut used_rows: std::collections::BTreeMap<String, usize> = Default::default();
    for r in &reports {
        if let Some(e) = r.used.errors.first() {
            *used_rows.entry(addon_harness::normalise(e)).or_default() += 1;
        }
    }
    if !used_rows.is_empty() {
        let mut rows: Vec<(String, usize)> = used_rows.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        println!("\n  what broke when the UI was USED (first error each):");
        for (err, count) in rows.into_iter().take(12) {
            println!("    {count:>4}  {err}");
        }
    }

    // What the session start broke, ranked the way `blockers` ranks load failures.
    let mut session: std::collections::BTreeMap<String, usize> = Default::default();
    for r in reports.iter().filter(|r| r.loaded) {
        if let Some(e) = r.session_errors.first() {
            *session.entry(addon_harness::normalise(e)).or_default() += 1;
        }
    }
    if !session.is_empty() {
        let mut rows: Vec<(String, usize)> = session.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        println!("\n  what broke at SESSION START (addons that loaded clean, first error each):");
        for (err, count) in rows.into_iter().take(12) {
            println!("    {count:>4}  {err}");
        }
    }

    // What was warned about: not blockers, so after the error rankings, ranked by how many addons
    // hit each row so one noisy OnUpdate does not bury a warning fifty addons hit once.
    let mut warned: std::collections::BTreeMap<String, usize> = Default::default();
    for r in &reports {
        let mut seen: std::collections::BTreeSet<String> = Default::default();
        for w in &r.warnings {
            seen.insert(addon_harness::normalise(w));
        }
        for w in seen {
            *warned.entry(w).or_default() += 1;
        }
    }
    if !warned.is_empty() {
        let mut rows: Vec<(String, usize)> = warned.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let addons = reports.iter().filter(|r| !r.warnings.is_empty()).count();
        println!(
            "\n  what was WARNED about ({addons} addons raised at least one, by addon count):"
        );
        // Through `ranked`, so the cut is stated.
        ranked(rows, 12);
    }

    // The distribution, because a mean would hide the shape.
    let mut buckets = [0usize; 5];
    for r in &reports {
        let n = r.missing_globals.len();
        buckets[match n {
            0 => 0,
            1..=2 => 1,
            3..=5 => 2,
            6..=15 => 3,
            _ => 4,
        }] += 1;
    }
    println!("\n  missing-global count per addon:");
    for (label, n) in ["0", "1-2", "3-5", "6-15", "16+"].iter().zip(buckets) {
        println!("    {label:>5}  {n:>4}  {}", "#".repeat(n.min(60)));
    }

    // Whose package is incomplete: a `.toc` entry whose file the addon does not ship is the
    // addon's defect, and the reference logs `Couldn't open %s` and carries on (`0x6edaa0`).
    // Nothing is subtracted from the headline.
    let own: Vec<&addon_harness::AddonReport> = reports
        .iter()
        .filter(|r| !r.absent_own_files.is_empty())
        .collect();
    let foreign: Vec<&addon_harness::AddonReport> = reports
        .iter()
        .filter(|r| !r.absent_foreign_files.is_empty())
        .collect();
    // "The whole reason" holds only when the absent entries account for every load error.
    let sole_cause = |r: &addon_harness::AddonReport| {
        !r.loaded && r.errors.len() == r.absent_own_files.len() + r.absent_foreign_files.len()
    };
    if !own.is_empty() || !foreign.is_empty() {
        println!(
            "\n  MANIFEST ENTRIES WITH NO FILE — counted in the numbers above, listed here so"
        );
        println!("  the reader can tell a broken package from a broken client:");
        for r in &own {
            println!(
                "    {} — its own .toc lists {} file(s) the package does not contain{}",
                r.name,
                r.absent_own_files.len(),
                if sole_cause(r) {
                    "  [and that is its ONLY load error]"
                } else {
                    ""
                }
            );
            for f in r.absent_own_files.iter().take(4) {
                println!("        missing: {f}");
            }
        }
        for r in &foreign {
            println!(
                "    {} — wants a folder that is not installed{}",
                r.name,
                if sole_cause(r) {
                    "  [and that is its ONLY load error]"
                } else {
                    ""
                }
            );
            for f in r.absent_foreign_files.iter().take(4) {
                println!("        missing: {f}");
            }
        }
        let theirs = own.iter().filter(|r| sole_cause(r)).count();
        let ours = foreign.iter().filter(|r| sole_cause(r)).count();
        println!(
            "    ({} with an incomplete package of their own, {theirs} of which fail for that \
             alone; {} wanting a neighbour, {ours} for that alone)",
            own.len(),
            foreign.len()
        );
    }

    // What stopped them: the ranked first error.
    println!("\n  what stopped them (addons whose FIRST load error was each):");
    for (err, count) in addon_harness::blockers(&reports).into_iter().take(12) {
        println!("    {count:>4}  {err}");
    }

    // Templates named in `CreateFrame(..., "Template")` that we never declared: an unresolved
    // template raises no load error, so the addon passes and paints nothing.
    let templates = addon_harness::template_demand(&reports);
    if !templates.is_empty() {
        println!("\n  most-wanted missing TEMPLATES (addons naming each in CreateFrame):");
        ranked(templates, 12);
    }

    // Templates named in an addon's own XML `inherits=`: usually loud, since the element's
    // `<OnLoad>` fires at load. Kept apart from the list above.
    let inherits = addon_harness::inherits_demand(&reports);
    if !inherits.is_empty() {
        println!("\n  most-wanted missing TEMPLATES (addons naming each in an XML inherits=):");
        ranked(inherits, 12);
    }

    // Frames and tables, ranked separately: a missing function is a Rust verb, a missing frame is
    // FrameXML.
    let tables = addon_harness::table_demand(&reports);
    if !tables.is_empty() {
        println!("\n  most-wanted missing FRAMES/TABLES (addons indexing each):");
        ranked(tables, 16);
    }

    // Widget methods: neither a global nor an indexed table, so only these tables see them.
    // Resolved against the live `__index` dispatcher; it over-reports, since a scanner cannot type
    // a `:` call's receiver (`AddonReport::missing_methods`).
    let methods = addon_harness::method_demand(&reports);
    if !methods.is_empty() {
        // It counts addons that name the verb, neither addons blocked by it nor call sites: a
        // library replicated into many addons inflates a row.
        println!("\n  most-wanted missing METHODS — addons that NAME each as obj:Name(), and");
        println!("  no widget answers. NOT a blocker list and NOT a build queue:");
        println!("    · a big number is usually ONE library file replicated, and");
        println!(
            "    · a third-party library's own method on its own object is not ours to write."
        );
        println!("  Open the call site before ranking it.");
        ranked(methods, 16);
    }

    // Per kind: rows whose receiver the survey could type, from a `CreateFrame("Kind", …)` local
    // or our arena's kind for that name, asked against that kind alone, so a verb wired to one
    // class and missing on its sibling shows.
    let by_kind = addon_harness::kind_method_demand(&reports);
    if !by_kind.is_empty() {
        println!(
            "\n  most-wanted missing METHODS BY KIND (receiver typed from the call site; \"on X\" \
             = the sibling that does answer it):"
        );
        ranked(by_kind, 16);
    }

    // Receivers nothing could type, on a name whose answer depends on the kind: an upper bound
    // that shrinks as the scan learns to type receivers.
    let ambiguous = addon_harness::ambiguous_method_demand(&reports);
    if !ambiguous.is_empty() {
        println!(
            "\n  ...and names an UNTYPABLE receiver may or may not have ({} in all — an upper \
             bound, not a queue; NARROWEST first, because a name only a minority of kinds answer \
             is the AddMessage shape and one every kind but the two regions answers is not):",
            ambiguous.len()
        );
        ranked(ambiguous, 12);
    }

    // Methods addons call behind a feature test (`if sliderFrame.SetTopLevel then`): nobody is
    // stuck on one, so they are a separate table.
    let optional = addon_harness::optional_method_demand(&reports);
    if !optional.is_empty() {
        println!("\n  ...and methods they FEATURE-TEST and work around (not blockers):");
        ranked(optional, 8);
    }

    println!("\n  most-wanted missing globals (addons wanting each):");
    ranked(addon_harness::demand(&reports), 30);

    if let Some(pattern) = &why {
        let hits = addon_harness::blocked_by(&reports, pattern);
        // Matches the raw text as well as the normalised row, over every error, not just the first.
        // A row with no `#` is the addon's first error, the one the tables above rank.
        println!(
            "\n  errors matching {pattern:?} ({}) — a row without a '#' is the FIRST error, i.e. \
             the one the tables above rank:",
            hits.len()
        );
        for (name, err) in &hits {
            println!("    {name}");
            // The message, then mlua's traceback frames; the first tells a generic-for
            // (`in local '(for generator)'`) apart from any other call of a table value.
            for line in err.lines().take(WHY_TRACEBACK_LINES) {
                println!("        {}", line.trim());
            }
        }
        if hits.is_empty() {
            println!(
                "    (none — no load or session error of any addon contains that text, and no \
                 normalised row does either)"
            );
        }
        // ...and the same question of the method demand tables.
        let rows = addon_harness::method_rows_matching(&reports, pattern);
        println!(
            "\n  method-table rows matching {pattern:?} ({}) — which addons carry the row, and \
             from which table:",
            rows.len()
        );
        for (name, row) in &rows {
            println!("    {name:<36} {row}");
        }
        if rows.is_empty() {
            println!("    (none — no addon's method tables contain that text)");
        }
        // ...and which addons each demand ranking counted, so a row can be checked against the
        // corpus.
        for (label, rows) in [
            (
                "globals",
                addon_harness::wanters(&reports, pattern, |r| &r.missing_globals),
            ),
            (
                "tables",
                addon_harness::wanters(&reports, pattern, |r| &r.missing_tables),
            ),
            (
                "methods",
                addon_harness::wanters(&reports, pattern, |r| &r.missing_methods),
            ),
        ] {
            if rows.is_empty() {
                continue;
            }
            println!(
                "\n  addons whose missing-{label} list matches {pattern:?} ({}) — this is what the \
                 ranking counted:",
                rows.len()
            );
            for (addon, name) in &rows {
                println!("    {addon:<36} {name}");
            }
        }
    }

    if verbose {
        println!("\n  per addon:");
        for r in &reports {
            let iface = r
                .interface
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            // The session column beside the load one: an addon can load clean and die in its
            // PLAYER_LOGIN handler.
            let session = match r.session_errors.first() {
                None if r.loaded => "session=ok".to_string(),
                None => "session=-".to_string(),
                Some(e) => format!("session: {}", e.lines().next().unwrap_or(e)),
            };
            // The render verdict, with the quad count and the frames it was charged to.
            let drew = format!(
                "drew={}({})",
                r.render.drew().word(),
                r.render.own_quads + r.render.overlay_quads
            );
            // The use verdict, always with the target count: `used=ok` reads the same whether the
            // probe drove eight frames or none.
            let used = format!(
                "used={}({}/{})",
                r.used.verdict().word(),
                r.used.driven,
                r.used.touchable
            );
            println!(
                "    {:<28} iface={:<12} {:<8} missing={:<4} {:<11} {drew:<16} {used:<20} {} {}",
                r.name,
                if iface.is_empty() { "-".into() } else { iface },
                if r.loaded { "loaded" } else { "ERRORS" },
                r.missing_globals.len(),
                session,
                render_frames(&r.render.frames),
                r.errors.first().map(String::as_str).unwrap_or("")
            );
        }
    }
    // The per-addon roster: `<name> ok|fail`, sorted, one per line. `ok` is the session column
    // (loaded and no session error).
    let roster: Vec<(String, bool)> = {
        let mut v: Vec<(String, bool)> = reports
            .iter()
            .map(|r| (r.name.clone(), r.loaded && r.session_errors.is_empty()))
            .collect();
        v.sort();
        v
    };
    if let Some(path) = &status {
        let mut out = format!(
            "# per-addon status ({} ok / {} fail) — digest {}\n",
            roster.iter().filter(|(_, ok)| *ok).count(),
            roster.iter().filter(|(_, ok)| !*ok).count(),
            addon_harness::framexml_digest()
        );
        for (name, ok) in &roster {
            out.push_str(&format!("{name} {}\n", if *ok { "ok" } else { "fail" }));
        }
        match std::fs::write(path, out) {
            Ok(()) => println!("\n  wrote roster to {path} ({} addons)", roster.len()),
            Err(e) => println!("\n  --status {path}: {e}"),
        }
    }
    if let Some(path) = &diff {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                // The digest gate: a delta is attributable only when this run's FrameXML digest
                // matches the roster's, since a dev build reads `assets/ui` from the source tree.
                // It refuses rather than warns, so a mismatched number is never printed.
                let base_digest = text
                    .lines()
                    .find(|l| l.trim_start().starts_with("# per-addon status"))
                    .and_then(|l| l.split_whitespace().last());
                let now = addon_harness::framexml_digest().to_string();
                if let Some(was) = base_digest.filter(|d| **d != now) {
                    println!("\n  DIFF vs {path}: REFUSED — the baseline is at a different tree");
                    println!("    baseline digest {was}   this run {now}");
                    println!(
                        "    A delta across two digests is not attributable \
                         to your change.\n    Re-take the baseline on this tree: \
                         --status <file>, then re-run --diff."
                    );
                    return;
                }
                let base: std::collections::BTreeMap<&str, bool> = text
                    .lines()
                    .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
                    .filter_map(|l| l.rsplit_once(' '))
                    .map(|(n, v)| (n.trim(), v.trim() == "ok"))
                    .collect();
                let (mut gained, mut lost, mut appeared) = (Vec::new(), Vec::new(), Vec::new());
                for (name, ok) in &roster {
                    match base.get(name.as_str()) {
                        Some(was) if *was != *ok => {
                            if *ok {
                                gained.push(name)
                            } else {
                                lost.push(name)
                            }
                        }
                        None => appeared.push(name),
                        _ => {}
                    }
                }
                let vanished: Vec<&&str> = base
                    .keys()
                    .filter(|n| !roster.iter().any(|(m, _)| m == **n))
                    .collect();
                println!("\n  DIFF vs {path}");
                println!(
                    "    net {:+}  (gained {}, lost {})",
                    gained.len() as i64 - lost.len() as i64,
                    gained.len(),
                    lost.len()
                );
                // Both lists always print, even when empty.
                for (label, list) in [("GAINED", &gained), ("LOST", &lost)] {
                    println!("    {label} ({}):", list.len());
                    for n in list.iter() {
                        println!("      {n}");
                    }
                }
                if !appeared.is_empty() || !vanished.is_empty() {
                    println!(
                        "    (roster changed: {} not in the baseline, {} baseline rows absent — \
                         the two runs surveyed different folders)",
                        appeared.len(),
                        vanished.len()
                    );
                }
            }
            Err(e) => println!("\n  --diff {path}: {e}"),
        }
    }
    println!();
}
