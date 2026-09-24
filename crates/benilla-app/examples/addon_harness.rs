//! `addon_harness` — load a folder of addons, one per VM, and print what happened.
//!
//! ```text
//! cargo run -q -p benilla-app --example addon_harness -- <folder> [--verbose] [--why <substr>] [--deep [n]] [--status <file>] [--diff <file>]
//!   or: ... -- <folder> --probe <Name> [--eval <lua> | --mouse <x>,<y> | --tick <secs>]...   (one addon, then
//!       ask its VM — the steps run in the order given, so a read can be taken with the cursor
//!       parked somewhere the addon cares about)
//! ```
//!
//! The instrument decision 1188 phase 6 asks for: *"which addons work" is a number that can be
//! re-read on any day*. The mechanics, and what the numbers are and are not worth, are in
//! [`benilla_app::addon_harness`]'s module doc — read it before quoting anything from here.
//!
//! **Expect a long tail and do not chase 100 %** (1188's own instruction). The report is a
//! distribution, not a pass/fail: a handful of addons will always want features we have not built.
use benilla_app::addon_harness;

/// How much traceback `--why` prints per addon. Two lines gave the message and mlua's first
/// frame, which was enough to RANK a row and never enough to FIND one: chasing a `SetTexture`
/// failure to its call site needed the frames below it, and the addon/file/line only appear there.
const WHY_TRACEBACK_LINES: usize = 8;

/// Print a ranked demand table — and **say what was dropped**.
///
/// Every one of these lists is a queue, and each was printed as a silent top-N. A silent cut reads
/// as "that is the whole list", and this arc has one expensive instance of exactly that: the
/// "a reference frame we do not build" class was swept, found clean, and recorded CLOSED — while
/// `ShapeshiftBarLeft` sat below the cut with two addons behind it, and stayed there until an
/// addon's first error named it (1219's class, first regions). The sweep was honest; the list it
/// swept was not complete and did not say so.
///
/// So the tail is now stated: how many rows, and how many addon-mentions, were not shown. The
/// caller keeps its own `take` — this only refuses to hide the remainder.
///
/// `--deep [n]` overrides every caller's `take` (see [`DEEP`]). 1242 made the cut VISIBLE and
/// rejected raising it — an unreadable report helps nobody. But "visible" only tells a sweep that
/// rows exist; it still cannot read them, and `--why` opens one name at a time, which is no way to
/// walk 1930 of them. So the cut stays where it is for the report, and a sweep asks for the rest.
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

/// One of the probe's two error lists, printed with its count — and printed even when EMPTY.
///
/// A silent absence and a list nobody asked for read identically, and the whole point of a probe
/// run is to tell "this addon loaded clean and died in a handler" from "it never loaded at all".
fn report_lines(label: &str, lines: &[String]) {
    println!("  {label}: {}", lines.len());
    for line in lines {
        for (n, l) in line.lines().enumerate() {
            println!("    {}{}", if n == 0 { "" } else { "  " }, l.trim_end());
        }
    }
}

/// The `--deep` override, set once in `main`. `Some(n)` shows `n` rows of EVERY ranked list;
/// `--deep` with no number means all of them. Absent, each list keeps the `take` its caller chose.
static DEEP: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();

/// The row's frame names, bounded for the line and **saying so when it bounds**.
///
/// `RenderReport::frames` carries every named frame now; the cap that used to live in the
/// collection (and silently evicted the very names a test asserted on) lives here instead.
fn render_frames(frames: &[String]) -> String {
    use benilla_app::addon_harness::render::MAX_NAMED_FRAMES;
    // `--deep` opens THIS bound too. It was added for the ranked tables, but the principle is one
    // principle: a bounded view is fine, a bounded view you cannot open is not.
    let cap = DEEP.get().copied().flatten().unwrap_or(MAX_NAMED_FRAMES);
    if frames.len() <= cap {
        return frames.join(",");
    }
    format!("{},+{} more", frames[..cap].join(","), frames.len() - cap)
}

/// Our `_G` against the captured 1.12 `_G` — decision 1189's diff, re-runnable.
///
/// 1189 did this once, by hand, and its finding was that **a superset is not free**: 1.12 addons
/// branch on presence (`if SomeName then`), so a name we publish that the reference does not have
/// can route an addon down a path the real client never takes. That makes BOTH directions of this
/// diff interesting, and the extra-names side the one nothing else in this harness can see — every
/// other ranking here is demand-driven, so it can only ever report what an addon *asked* for.
///
/// The reference side is `reference/1.12-globals.tsv`, generated from a live reference capture.
/// `lod` rows are the twelve LoadOnDemand `Blizzard_*` addons' names, unioned in by 1200 because a
/// live dump misses them unless the player opened those windows; they are counted separately rather
/// than silently folded in, since "absent from us" means something different for a window nobody
/// opened.
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

    // `--surface-dump <file>` writes our raw `_G` as `name<TAB>type`, the same shape the reference
    // TSV has, so the two can be joined by anything. The printed report answers the questions I
    // thought to ask; a sweep two sessions from now will have different ones, and re-deriving our
    // side by scraping a human-readable report is exactly how a measurement quietly goes wrong.
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

    println!("\n  SURFACE DIFF — our _G vs the captured 1.12 _G (decision 1189)");
    println!("  FrameXML digest : {}", addon_harness::framexml_digest());
    println!("  reference names : {}", reference.len());
    println!("  ours            : {}", ours.len());

    // Absent from us, by the reference's own type — a function we lack is a different problem from
    // a string we lack, and `lod` is a third thing again.
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

    // The direction 1189 cared about and nothing else here can see.
    //
    // **Split by OUR type, because the total on its own misleads.** Most of these are `table` — the
    // frame and region names our own FrameXML publishes, which differ from Blizzard's simply
    // because our XML is our own reimplementation and names its pieces itself. That is not the
    // hazard 1189 described. The hazard is a **function**: `if SomeApiName then` is how a 1.12
    // addon feature-tests, and a verb we publish that the client never had can route it down a path
    // the real client never takes. So the function row is the one to read first, and the one a
    // future landing should be able to drive to zero.
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
        "\n  PUBLISHED BY US, ABSENT FROM 1.12 ({}) — a superset is not free (1189):",
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
    // Functions in full even without --deep: it is the row that matters and it has to be readable.
    //
    // Split again on the `Benilla` prefix, which is the difference between a name that CAN collide
    // with an addon's expectations and one that cannot. Nothing in the 1.12 corpus feature-tests
    // `BenillaPaperDollSlot_OnClick`; our own namespace is safe by construction, and leaving 443 of those
    // in the list would bury the ones that are not.
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
             a naming question — but a POST-1.12 API name here is 1189's hazard exactly, because an\n      \
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

    // `--surface` — decision 1189's comparison, made re-runnable. It needs no corpus, so it is
    // handled before the root is used for anything.
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
    // `--why <substring>` — the addons behind one row, with their verbatim errors. The
    // ranked table collapses quoted names by design (1193); this is the read-back, and two of this
    // arc's decisions came from doing it by hand (1206, 1210). It matches the normalised row AND
    // the raw text, so `--why <a row>` and `--why <a name>` both work — the second did not, and
    // that is what made the whole widget-method class unreadable here.
    //
    // It reads back through the **method demand tables** too, which nothing did: every ranking here
    // is built by scanning, three of them have opened with fiction (1210, 1218, 1227), and the rule
    // that came out of that is to open the corpus line before quoting a row. Verifying the per-kind
    // table's own first head cost a temporary debug print and a second corpus run for want of this.
    let why = rest
        .iter()
        .position(|a| a == "--why")
        .and_then(|i| rest.get(i + 1))
        .cloned();
    // `--status` writes the per-addon ok/fail roster; `--diff <file>` compares against one.
    //
    // **The instrument this survey was missing, and the reason it is missing is instructive.**
    // Every column here is a TOTAL, so a landing that fixes one addon and breaks another reads as
    // a clean zero — indistinguishable from a change that did nothing. That happened: the shadow
    // accessors (`7824c154`) gained FuBar_NavigatorFu and held the column at 107, so something was
    // lost, and there was no way to ask what. Every "zero delta" recorded in this arc before now
    // could have been hiding the same swap.
    // `--status <file>` WRITES the roster, rather than printing it. It printed at first, and the
    // obvious `--status > roster.txt` then captured the whole report — 53 report lines parsed as
    // addon rows, because `--diff` splits on the last space and almost anything satisfies that. An
    // instrument whose output needs a `grep` incantation to be usable is one that will be used
    // wrong; the file is the artefact, so the tool writes the file.
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

    // `--together` — the whole folder in ONE VM, the control for the survey's one-VM-per-addon
    // bound. Not a column and not a headline (see `addon_harness::together`'s header for what it
    // cannot answer); `--diff <a survey roster>` names the rows the bound is costing, which is
    // the question it exists for.
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
        // Named, never folded into either count: Lua hashes a table key by its pointer, so a
        // registry keyed by objects walks in a different order every process (see the module
        // doc). A row that raises in some runs and not others is that showing through.
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
            // The survey's roster, read back: `ok` there and clean here is agreement; `fail`
            // there and clean here is the bound, priced.
            let prior: std::collections::BTreeMap<String, bool> = std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.rsplit_once(' '))
                .map(|(n, v)| (n.trim().to_string(), v.trim() == "ok"))
                .collect();
            let mut freed: Vec<&str> = Vec::new();
            let mut only_together: Vec<&str> = Vec::new();
            for r in &rows {
                // Only the rows that are the same in every run are compared — an order-sensitive
                // one belongs to neither list, which is the whole point of naming it separately.
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

    // `--probe <Name> [--eval <lua> ...]` — ONE addon, loaded the way the survey loads it, then
    // asked. Handled before the survey because it is not one: it prints no column and it is not a
    // measurement (an eval can mutate the VM), so mixing the two outputs would invite a probe
    // number into a record. See `addon_harness::probe`'s header for what it is worth and where it
    // deliberately stops — session start, before the render and use probes touch anything.
    if let Some(name) = rest
        .iter()
        .position(|a| a == "--probe")
        .and_then(|i| rest.get(i + 1))
    {
        // `--eval <lua>` and `--mouse <x>,<y>` are ONE ordered list, not two: a read taken
        // before the cursor arrived and one taken after answer different questions, and which is
        // which is the order the caller typed.
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
    // Which VM the survey ran against. Without an install there is no GlobalStrings.lua, ~5,000
    // globals are missing, and every number below is worse for a reason that has nothing to do
    // with the client — say so rather than letting two machines' numbers be compared in silence.
    println!(
        "  VM: the stock 1.12 FrameXML off the player's chain + a seated session{}\n",
        if addon_harness::seated_with_global_strings() {
            " + the real GlobalStrings.lua"
        } else {
            "  ** no install found: GlobalStrings absent, these numbers are NOT comparable **"
        }
    );
    // The tree these numbers came from. Two runs are comparable only if this matches: in a dev
    // build `assets/ui` is read from the SOURCE TREE, so anything else editing the checkout moves
    // the headline with no rebuild. Quoting a delta across two different digests is how a wrong
    // attribution got into a decision record (1209).
    println!(
        "  FrameXML digest                    : {}",
        addon_harness::framexml_digest()
    );
    println!(
        "  loaded without a single load error : {loaded}/{}",
        reports.len()
    );
    // **The reconciliation line, printed always** (decision 2155). `loaded` counts what RAISED;
    // a manifest entry naming a file the package does not contain is not that — the reference logs
    // `Couldn't open %s` and carries on — and it used to be counted here. Every past record's
    // figure was the stricter one, so the stricter one is printed beside the honest one rather
    // than left for a reader to reconstruct: two numbers cannot be silently confused, one can.
    let strict = reports.iter().filter(|r| r.errors.is_empty()).count();
    println!(
        "      (…{} of those name a file their own package does not contain, which the reference \
         logs and carries on from; the pre-2155 column counted those as failures: {strict}/{})",
        loaded - strict,
        reports.len()
    );
    println!(
        "  ...and calling nothing we lack     : {clean}/{}",
        reports.len()
    );
    println!("  with a dependency not installed    : {blocked}");
    // The stricter column, and the one that answers what the survey is really asking. Every other
    // number here is LOAD-time; this one drives the client's own session start
    // (ADDON_LOADED -> VARIABLES_LOADED -> PLAYER_LOGIN -> PLAYER_ENTERING_WORLD, then a second of
    // ticks) and reports what the addon's HANDLERS raised. Four decision records in this arc end
    // with "the headline cannot see this"; this is the number that can.
    let survived = reports
        .iter()
        .filter(|r| r.loaded && r.session_errors.is_empty())
        .count();
    println!(
        "  ...and survived a session start    : {survived}/{}",
        reports.len()
    );
    // The UI-probe column: of those, how many survive having their OVERRIDES actually invoked.
    // The director found this blind spot by playing the game — an addon that replaces
    // `ToggleBackpack` and is never called looked identical to one that works.
    let probed = reports
        .iter()
        .filter(|r| r.loaded && r.session_errors.is_empty() && r.probe_errors.is_empty())
        .count();
    println!(
        "  ...and survived a UI probe         : {probed}/{}",
        reports.len()
    );
    // **The render column** — the only one here that asks whether anything was DRAWN. Every number
    // above it asks whether something raised, and the director's Bagnon report raised nothing at
    // all: a window with a title, a gold line and no bag slots, scoring a clean pass on all four.
    // `addon_harness::render`'s header is the design and its honest bounds.
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
    // The actionable list, and the reason this column exists: an addon that loads clean, survives a
    // session start AND a UI probe, and still puts nothing on screen. Nothing else here can name
    // one. Bagnon was on this list.
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
        // **This list is a QUESTION, not a defect list, and the header used to say otherwise.**
        //
        // It read "read this list first", which invites treating every row as a bug. Most are not.
        // The survey seats a player and an empty world: no buffs, no target, no combat, no cursor
        // over an item. An addon with nothing to draw in that world draws nothing CORRECTLY — and
        // that is most of this list. Measured on the vanilla corpus rather than assumed: of 48 rows
        // checked, 6 ship no XML at all (pure libraries — Ace, LibStub, Stubby, DevTools), and of
        // the rest the buff bars (CT_BuffMod 29 frames, ElkBuffBar 3, neither hidden at birth) are
        // waiting on buffs that a fresh login does not have either.
        //
        // What the row DOES mean is "nothing here can be ruled out by the other four columns" —
        // which is worth printing, and is not the same as "broken".
        //
        // The instrument change that would sharpen it is a POPULATED session fixture (a buff, a
        // target, a live cooldown) so that "nothing to draw" and "failed to draw" stop sharing a
        // row. Named here rather than done, because seating content changes the fixture for all
        // 218 addons and deserves its own controlled A/B.
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

    // **The use column** — the only one here that TOUCHES anything. Every number above it asks
    // whether something ran or appeared; the director drew the line by hovering Bagnon's freshly
    // drawn bag slots and getting a wall of `attempt to call global
    // 'ContainerFrameItemButton_OnEnter'` while all five columns above scored it a pass.
    //
    // Printed as THREE numbers, never one, because "nothing raised" and "nothing was touched" are
    // different answers and a probe that quietly drove zero targets reporting "clean" is this
    // instrument's own oldest failure mode (`addon_harness::use_probe`'s header).
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
    // The list this column exists to be able to print: an addon whose UI is fully on screen and
    // falls over the moment anyone uses it.
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

    // **What was WARNED about, ranked** — the channel that reached nobody until 2135. It is
    // printed after the error rankings and before the distribution on purpose: these are not
    // blockers (nothing raised, the addon is running), so they must not outrank a row somebody is
    // stuck on — but they are the only column that can see an addon quietly getting the wrong
    // thing, which is the class `render` was added for and reaches later.
    //
    // Ranked by how many ADDONS hit each row, not by total occurrences: a warning one addon fires
    // in an OnUpdate would otherwise bury one that fifty addons hit once.
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
        // Through `ranked`, not a bare `take(12)`: this list is a queue like every other one here,
        // and a silent cut reads as "that is the whole list" (1242's rule, which this block was
        // written outside of). It cost a measurement — asking the corpus how many `SetPoint`
        // targets fail to resolve, the answer sat below the cut and the column read zero.
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

    // **WHOSE package is incomplete** — printed immediately before the ranked blockers, because it
    // is the line that stops a session hunting for a client bug that is not one. A `.toc` entry
    // whose file the addon does not ship is the ADDON's defect, and the reference client's
    // behaviour there is what ours already does: log `Couldn't open %s` and carry on (`0x6edaa0`).
    // Nothing is subtracted from the headline — 1213 — so both readings stay available.
    let own: Vec<&addon_harness::AddonReport> = reports
        .iter()
        .filter(|r| !r.absent_own_files.is_empty())
        .collect();
    let foreign: Vec<&addon_harness::AddonReport> = reports
        .iter()
        .filter(|r| !r.absent_foreign_files.is_empty())
        .collect();
    // **"the WHOLE reason" is a claim, and it is checked before it is printed.** An addon can be
    // short a file AND raise somewhere else; saying "this is why it fails" on the strength of
    // `!loaded` alone would be the same overclaim this column exists to stop. It holds only when
    // the absent entries account for every load error the addon has.
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

    // What actually STOPPED them — the ranked first error. Read this before the demand list: a
    // wall 60 addons hit is worth more than a verb 60 addons would like (decision 1193).
    println!("\n  what stopped them (addons whose FIRST load error was each):");
    for (err, count) in addon_harness::blockers(&reports).into_iter().take(12) {
        println!("    {count:>4}  {err}");
    }

    // Templates an addon names in `CreateFrame(..., "Template")` that we have never declared
    // (decision 1203). Its own list because it is invisible to every other number here: an
    // unresolved template raises no load error, so the addon scores as a pass and paints nothing.
    let templates = addon_harness::template_demand(&reports);
    if !templates.is_empty() {
        println!("\n  most-wanted missing TEMPLATES (addons naming each in CreateFrame):");
        ranked(templates, 12);
    }

    // The same question over the OTHER axis, and the one that actually moves the headline: a
    // template named in an addon's own XML `inherits=`. Unlike the list above this failure is
    // usually LOUD — the element's `<OnLoad>` fires at load and its first line is normally
    // `getglobal(this:GetName().."Text")` — which is why transcribing the reference's shared kit
    // was worth twelve addons while the CreateFrame list predicted none of them. Printed second
    // and separately because merging the two would hide exactly that difference.
    let inherits = addon_harness::inherits_demand(&reports);
    if !inherits.is_empty() {
        println!("\n  most-wanted missing TEMPLATES (addons naming each in an XML inherits=):");
        ranked(inherits, 12);
    }

    // Frames and tables, ranked separately: a missing function is a Rust verb to write, a missing
    // frame is FrameXML to transcribe, and the two queues go to different people. The scan was
    // blind to this whole shape until 2026-08-11 — a window 86 addons reach scored 0.
    let tables = addon_harness::table_demand(&reports);
    if !tables.is_empty() {
        println!("\n  most-wanted missing FRAMES/TABLES (addons indexing each):");
        ranked(tables, 16);
    }

    // **Widget METHODS** — the third queue, and the one this report was blind to while the arc
    // spent a day building exactly these (GetTexture, SetShadowColor, SetNonSpaceWrap, GetBackdrop
    // all landed within hours). A method is not a global and not an indexed table, so neither list
    // above could ever hold one; the only trace a missing method left was an error row reading
    // `attempt to call method 'X' (a nil value)`, with the name collapsed away by the very
    // normalisation that makes the row readable.
    //
    // Resolved against the live `__index` dispatcher, not a name list — so a whole *kind* left
    // unwired shows up here as loudly as a verb never written. It over-reports (a scanner cannot
    // type the receiver of a `:` call); `AddonReport::missing_methods` states exactly how far, and
    // `--why <name>` now reads any row back by name.
    let methods = addon_harness::method_demand(&reports);
    if !methods.is_empty() {
        // The header states BOTH limits, because the list read as a build queue and is not one
        // (decision 1240). It counts addons that NAME the verb, which is neither "addons blocked
        // by it" nor "call sites": its top three were once RegisterTabCompletion/IsModule/
        // IsModuleActive — AceConsole-2.0's and AceAddon-2.0's own methods on their own objects,
        // one library file replicated into 56 and 38 addons — and its fourth, EnableKeyboard, was
        // a real absent Frame verb that was blocking none of its 8, every one of which died
        // earlier and elsewhere.
        println!("\n  most-wanted missing METHODS — addons that NAME each as obj:Name(), and");
        println!("  no widget answers. NOT a blocker list and NOT a build queue:");
        println!("    · a big number is usually ONE library file replicated (1207/1210), and");
        println!(
            "    · a third-party library's own method on its own object is not ours to write."
        );
        println!("  Open the call site before ranking it (decision 1240).");
        ranked(methods, 16);
    }

    // **Per KIND** — the question the table above structurally cannot ask. It resolves a name
    // against every probe and stops at the first hit, so a verb wired to one class and forgotten on
    // its sibling comes back present: `MessageFrame:AddMessage` scored zero for as long as it
    // existed, answered by the ScrollingMessageFrame probe, while three corpus addons had
    // `UIErrorsFrame:AddMessage` as their FIRST load error (decision 1228). These rows are the
    // call sites whose receiver the survey could TYPE — from a `CreateFrame("Kind", …)` local, or
    // from the kind our own arena publishes that name as — asked against that kind alone.
    let by_kind = addon_harness::kind_method_demand(&reports);
    if !by_kind.is_empty() {
        println!(
            "\n  most-wanted missing METHODS BY KIND (receiver typed from the call site; \"on X\" \
             = the sibling that does answer it):"
        );
        ranked(by_kind, 16);
    }

    // ...and the residue: a receiver nothing could type, on a name whose answer DEPENDS on the
    // kind. An upper bound, printed rather than swallowed — resolving these against the whole probe
    // set and calling them present is the exact blindness above. The length of this table measures
    // the ATTRIBUTOR, not the widget surface: every receiver shape the scan learns to type moves
    // rows out of it.
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

    // The other half of the same scan, and never merged into it: methods addons call **behind a
    // feature test**, so nobody is stuck on one. `if sliderFrame.SetTopLevel then` is Dewdrop's,
    // and `SetTopLevel` is a real 1.12 widget method 60 corpus addons quietly do without. Building
    // one of these fixes no error and improves N addons' behaviour, which is a different decision
    // from the list above — so it is a different table.
    let optional = addon_harness::optional_method_demand(&reports);
    if !optional.is_empty() {
        println!("\n  ...and methods they FEATURE-TEST and work around (not blockers):");
        ranked(optional, 8);
    }

    println!("\n  most-wanted missing globals (addons wanting each):");
    ranked(addon_harness::demand(&reports), 30);

    if let Some(pattern) = &why {
        let hits = addon_harness::blocked_by(&reports, pattern);
        // "first error" was a lie in two directions and both cost this instrument a whole class:
        // the match ran against the NORMALISED row only, where every quoted name is already `'X'`,
        // so `--why GetBackdrop` answered `(none)` while an addon was dying on exactly that; and
        // it read only the first error, while a session start keeps firing handlers after one dies.
        // A row with no `#` is the addon's first error — the one the tables above rank — so the
        // index-less rows still count out to the ranked row exactly.
        println!(
            "\n  errors matching {pattern:?} ({}) — a row without a '#' is the FIRST error, i.e. \
             the one the tables above rank:",
            hits.len()
        );
        for (name, err) in &hits {
            println!("    {name}");
            // Two lines, not one. The first is the message; the SECOND is mlua's first traceback
            // frame, and for the row this instrument is most often pointed at that frame is the
            // whole answer — `in local '(for generator)'` is what tells a generic-for
            // (decision 1202) apart from any other call of a table value.
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
        // ...and the same question of the DEMAND tables, which no read-back reached until now.
        // Every ranking above is a scan, and this arc has found fiction at the top of three of
        // them (1210, 1218, 1227); the rule that came out of it is "open the corpus line before
        // quoting the row", and this is what makes that one command instead of a code edit.
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
        // ...and WHICH ADDONS each demand ranking counted. The rankings print a number per name and
        // nothing could ask what it was made of, so a row could disagree with the corpus and no
        // command would say so. It happened: `GetChannelList` ranked 4 while exactly ONE addon's
        // source names it, and finding that took a hand-rolled grep across a symlinked corpus.
        // A count you cannot open is a claim, not a measurement.
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
            // The SESSION column, not just the load one. `loaded` means "no LOAD errors" and
            // always has (1213); an addon that loads clean and dies in its PLAYER_LOGIN handler
            // read as a pass here, which is the exact overstatement 1213 measured at four to one.
            // "does this addon actually work" is the two columns together, so print both.
            let session = match r.session_errors.first() {
                None if r.loaded => "session=ok".to_string(),
                None => "session=-".to_string(),
                Some(e) => format!("session: {}", e.lines().next().unwrap_or(e)),
            };
            // The render verdict, with the quad count and the frames it was charged to — the row
            // that turns "this addon is fine" into "this addon is fine AND you can see it".
            let drew = format!(
                "drew={}({})",
                r.render.drew().word(),
                r.render.own_quads + r.render.overlay_quads
            );
            // The use verdict, ALWAYS with the target count beside it — a bare `used=ok` would be
            // unreadable exactly where it matters, because it reads the same whether the probe
            // drove eight of the addon's frames or none at all.
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
    // The per-addon roster: `<name> ok|fail`, sorted, one per line — a shape `--diff` can read back
    // and a human can eyeball. `ok` is the SESSION column (loaded AND no session error), because
    // that is the one this arc treats as "works" (1213).
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
                // **The digest gate.** `--status` stamps the roster's header with the FrameXML
                // digest it was taken at, and a delta is only attributable when this run's matches:
                // in a dev build `assets/ui` is read from the SOURCE TREE, so any *other* session
                // landing an interface change moves the headline under you with no rebuild of
                // yours. The stamp was written for this and then never checked, so the check was
                // the reader's to remember — which is exactly how 1209's wrong attribution reached
                // a decision record, and it nearly happened again on the run that added this gate
                // (a rebase pulled in a neighbour's interface change between the baseline and the
                // measurement).
                //
                // So this REFUSES rather than warns. A warning printed beside a `net +0` still
                // leaves the number sitting there to be quoted; withholding the number is the only
                // form that cannot be misread.
                let base_digest = text
                    .lines()
                    .find(|l| l.trim_start().starts_with("# per-addon status"))
                    .and_then(|l| l.split_whitespace().last());
                let now = addon_harness::framexml_digest().to_string();
                if let Some(was) = base_digest.filter(|d| **d != now) {
                    println!("\n  DIFF vs {path}: REFUSED — the baseline is at a different tree");
                    println!("    baseline digest {was}   this run {now}");
                    println!(
                        "    A delta across two digests is not attributable to your change \
                         (decision 1209).\n    Re-take the baseline on this tree: \
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
                // Both lists always print, even when empty. A silent "lost" section would let a
                // regression read as a clean run, which is the fault this whole mode exists for.
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
