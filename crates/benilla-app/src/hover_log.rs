//! `hover_log.rs` — the **hover-cost recorder** (`WOW_HOVER_LOG`).
//!
//! The director reports dropped frames while hovering bag items and spells. Reproducing that needs
//! a live session and a *hand* on the mouse, which the headless capture harness cannot supply and
//! reading `[ui-cost]` lines off a terminal cannot survive: the interesting frames are a handful of
//! spikes inside thousands of quiet ones. So this records **every frame** to a file, tagged with
//! what the tooltip was showing at the time, and prints a report at exit that ranks the cost
//! *against the tooltip state* — hovering vs not, and which hover.
//!
//! Run: `WOW_HOVER_LOG=1 cargo run --release -p benilla` (writes `target/hover-log.csv`; a path
//! value writes there instead). Hover whatever feels bad, for as long as you like, then quit — the
//! report prints to the terminal and the raw rows stay in the file.
//!
//! Columns are one frame each: the wall frame time and the process-CPU delta (the meter vsync
//! cannot rail — `perf.rs`), the UI pass's own phase split (tick/resolve/measure/extract/diff,
//! μs — the same marks `WOW_UI_COST=1` prints), and the tooltip context (shown, line count, owner
//! frame, first line). A spike with flat UI phases is NOT a UI-pass cost, and that negative is the
//! most valuable thing this can say.

use std::io::Write;

use bevy::prelude::*;
use bevy::time::Real;

use benilla_ui::script::UiScript;

/// The per-frame phase split this recorder writes to the CSV. Produced and owned by the UI pass
/// itself — an instrument reads the fact, it does not define it.
use crate::ui_script::{UiCostWanted, UiFrameCost};

/// Where to write, from `$WOW_HOVER_LOG`: unset ⇒ off, `1` ⇒
/// `benilla-config/Diagnostics/hover-log.csv` (the one folder, 0954/1486 — never the cwd-relative
/// `target/…` this once named, which is the install folder when the binary is launched from
/// inside it), anything else ⇒ that path. A hermetic run has no folder: `1` is then off, and
/// says so, the sound probe's rule.
fn log_path() -> Option<String> {
    match std::env::var("WOW_HOVER_LOG") {
        Err(_) => None,
        Ok(v) if v.is_empty() || v == "0" => None,
        Ok(v) if v == "1" => match crate::local_state::diagnostics_dir() {
            Some(dir) => Some(dir.join("hover-log.csv").to_string_lossy().into_owned()),
            None => {
                warn!(
                    "hover log: no benilla-config folder to write into (hermetic run) — set \
                     WOW_HOVER_LOG=<path> to name one. Not recording."
                );
                None
            }
        },
        Ok(v) => Some(v),
    }
}

/// Is the recorder on? Read once — the phase marks in the UI pass (`extract::tick_script`/`paint_script`) consult this every frame.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| log_path().is_some())
}

/// One recorded frame, kept in memory for the exit report (the file gets it either way).
#[derive(Clone)]
struct Row {
    frame_ms: f32,
    cpu_ms: f32,
    cost: UiFrameCost,
    /// The RENDER-side half of the same frame (`ui_pass::UiMeshCost`). Added after the 1625 fix
    /// left the symptom standing: the UI-script phases read flat on a hovered frame while the mesh
    /// rebuild below them was doing whole-interface work, and this recorder could not see it.
    mesh: crate::ui_pass::UiMeshCost,
    /// The tooltip context: `None` = no tooltip up. `(owner frame, line count, first line)`.
    tip: Option<(String, i64, String)>,
}

#[derive(Resource)]
struct Recorder {
    out: std::io::BufWriter<std::fs::File>,
    path: String,
    rows: Vec<Row>,
    prev_cpu: Option<f64>,
}

pub struct HoverLogPlugin;

impl Plugin for HoverLogPlugin {
    fn build(&self, app: &mut App) {
        if !enabled() {
            return;
        }
        let path = log_path().expect("enabled() checked");
        // Ask the UI pass for its phase split. `init_resource` first so this holds whichever
        // plugin builds first — `UiScriptPlugin`'s own init is then a no-op.
        app.init_resource::<UiCostWanted>();
        app.world_mut().resource_mut::<UiCostWanted>().0 = true;
        if let Some(dir) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::File::create(&path) {
            Ok(f) => {
                let mut out = std::io::BufWriter::new(f);
                let _ = writeln!(
                    out,
                    "frame_ms,cpu_ms,tick_us,resolve_us,measure_us,extract_us,convert_us,\
                     diff_us,quads,solves,derives,skipped,measured,measured_texts,tip_owner,tip_lines,\
                     tip_first_line,spliced,mesh_us,mesh_sort_us,mesh_split_us,mesh_write_us,\
                     mesh_quads,mesh_runs,mesh_rewrites,dropped"
                );
                info!("hover log: recording every frame to {path}");
                app.insert_resource(Recorder {
                    out,
                    path,
                    rows: Vec::with_capacity(64_000),
                    prev_cpu: None,
                })
                .add_systems(Last, (record, report_on_exit).chain());
            }
            Err(e) => error!("hover log: cannot write {path}: {e}"),
        }
    }
}

/// The tooltip context, asked of the VM itself so it reads the same names the director sees. One
/// tiny chunk per frame, only while recording.
fn tooltip_context(script: &UiScript) -> Option<(String, i64, String)> {
    let chunk = r#"
        if not GameTooltip or not GameTooltip:IsShown() then return "" end
        local owner = GameTooltip:BenillaGetTooltipOwner()
        local name = "(no owner)"
        if owner and owner.GetName and owner:GetName() then name = owner:GetName() end
        local first = ""
        if GameTooltipTextLeft1 and GameTooltipTextLeft1:GetText() then
            first = GameTooltipTextLeft1:GetText()
        end
        return name .. "\t" .. GameTooltip:NumLines() .. "\t" .. first
    "#;
    let s: String = script.eval(chunk).ok()?;
    if s.is_empty() {
        return None;
    }
    let mut parts = s.split('\t');
    let owner = parts.next()?.to_string();
    let lines = parts.next()?.trim().parse::<f64>().ok()? as i64;
    let first = parts.next().unwrap_or("").to_string();
    Some((owner, lines, first))
}

/// A CSV cell: quote and escape, and keep it to one line (tooltip text carries `|c` markup).
fn cell(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\"").replace(['\n', '\r'], " "))
}

fn record(
    time: Res<Time<Real>>,
    cost: Res<UiFrameCost>,
    mesh: Res<crate::ui_pass::UiMeshCost>,
    script: Option<NonSend<UiScript>>,
    mut rec: ResMut<Recorder>,
) {
    let frame_ms = time.delta_secs() * 1000.0;
    let cpu_now = crate::perf::process_cpu_secs();
    let cpu_ms = match (rec.prev_cpu, cpu_now) {
        (Some(prev), Some(now)) => ((now - prev) * 1000.0) as f32,
        _ => 0.0,
    };
    rec.prev_cpu = cpu_now;
    // The very first frame's delta is the startup gap, not a frame.
    if rec.rows.is_empty() && frame_ms > 100.0 {
        return;
    }
    let tip = script.as_deref().and_then(tooltip_context);
    let c = cost.clone();
    let (owner, lines, first) = tip
        .clone()
        .unwrap_or_else(|| (String::new(), 0, String::new()));
    let _ = write!(
        rec.out,
        "{frame_ms:.3},{cpu_ms:.3},{},{},{},{},{},{},{},{},{},{},{},{},{},{lines},{},{}",
        c.tick,
        c.resolve,
        c.measure,
        c.extract,
        c.convert,
        c.diff,
        c.quads,
        c.solves,
        c.derives,
        u8::from(c.skipped),
        c.measured,
        cell(&c.measured_texts.join(" ⏎ ")),
        cell(&owner),
        cell(&first),
        c.spliced,
    );
    let _ = writeln!(
        rec.out,
        ",{},{},{},{},{},{},{},{}",
        mesh.total,
        mesh.sort,
        mesh.split,
        mesh.write,
        mesh.quads,
        mesh.runs,
        mesh.rewrites,
        c.dropped,
    );
    rec.rows.push(Row {
        frame_ms,
        cpu_ms,
        cost: c,
        mesh: mesh.clone(),
        tip,
    });
}

fn pct(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let i = ((sorted.len() - 1) as f32 * p).round() as usize;
    sorted[i]
}

/// One population's line in the report: how many frames, and what they cost.
fn summarize(label: &str, rows: &[&Row]) -> String {
    if rows.is_empty() {
        return format!("  {label:<34} (no frames)");
    }
    let mut wall: Vec<f32> = rows.iter().map(|r| r.frame_ms).collect();
    let mut cpu: Vec<f32> = rows.iter().map(|r| r.cpu_ms).collect();
    wall.sort_by(f32::total_cmp);
    cpu.sort_by(f32::total_cmp);
    #[allow(clippy::cast_precision_loss)]
    let n = rows.len() as f32;
    let ui_us = |f: fn(&UiFrameCost) -> u128| {
        rows.iter().map(|r| f(&r.cost) as f64).sum::<f64>() / f64::from(n)
    };
    // The churn number: mean FontStrings re-shaped per frame. A settled UI sits at 0.00; anything
    // above it in a steady hover population is a string whose measure key moves every frame.
    let measured = rows.iter().map(|r| r.cost.measured as f64).sum::<f64>() / f64::from(n);
    // What the frame actually DID, as opposed to what it cost: how often the layout fixpoint ran
    // (a solve walks the whole anchor graph, not the tooltip's corner of it) and how often the
    // extract gate skipped the conversion+rasterize loop outright.
    #[allow(clippy::cast_precision_loss)]
    let solves = rows.iter().map(|r| r.cost.solves as f64).sum::<f64>() / f64::from(n);
    // The term `solves` hides: a solve that had to DERIVE the layout graph first
    // costs an order of magnitude more than one that used the ledger, and both count as one solve.
    // The law is zero — anything else is a write site that gave up naming its node, which
    // `WOW_LAYOUT_DERIVE_TRACE=<secs>:<n>` will backtrace on a live run.
    let derives = rows.iter().map(|r| r.cost.derives as f64).sum::<f64>() / f64::from(n);
    // The RENDER half of the same frame. `rebuilt%` is the one to read first: a population that
    // rebuilds the mesh on every frame is paying whole-interface work per frame, whatever the
    // script-pass phases say.
    let mesh_us = |f: fn(&crate::ui_pass::UiMeshCost) -> u128| {
        rows.iter().map(|r| f(&r.mesh) as f64).sum::<f64>() / f64::from(n)
    };
    let rebuilt = 100.0 * rows.iter().filter(|r| r.mesh.rebuilt).count() as f32 / n;
    let mesh_runs = rows.iter().map(|r| r.mesh.runs as f64).sum::<f64>() / f64::from(n);
    // The number that reaches Bevy: a rewritten batch re-extracts, and `rewrites` tracking `runs`
    // means the skip gate is being defeated for every batch at once.
    let mesh_rw = rows.iter().map(|r| r.mesh.rewrites as f64).sum::<f64>() / f64::from(n);
    let skips = 100.0 * rows.iter().filter(|r| r.cost.skipped).count() as f32 / n;
    // Counted against the DROPPED threshold, not the raw budget: synced wall time rails at the
    // display's present grant and jitters around it, so counting frames over 16.7 ms mostly counts
    // jitter (`perf.rs`'s `DROPPED_FACTOR` — a missed interval is what a player feels).
    let dropped = rows
        .iter()
        .filter(|r| r.frame_ms > crate::perf::FRAME_BUDGET_MS * 1.5)
        .count();
    format!(
        "  {label:<34} n={:<6} wall p50={:>6.2} p99={:>6.2} max={:>7.2}  cpu p50={:>6.2} \
         p99={:>6.2}  dropped={:>5.1}%\n      ui μs: tick={:>6.0} resolve={:>6.0} \
         measure={:>5.0} extract={:>6.0} convert={:>6.0} diff={:>5.0}\n      solves/frame={:.2} \
         derives/frame={:.2} reshaped/frame={:.2} extract-gate-skipped={:.0}%\n      mesh μs: \
         total={:>6.0} sort={:>5.0} split={:>5.0} write={:>6.0}  rebuilt={:.0}% runs={:.0} rewrites={:.1}",
        rows.len(),
        pct(&wall, 0.50),
        pct(&wall, 0.99),
        wall.last().copied().unwrap_or(0.0),
        pct(&cpu, 0.50),
        pct(&cpu, 0.99),
        100.0 * dropped as f32 / n,
        ui_us(|c| c.tick),
        ui_us(|c| c.resolve),
        ui_us(|c| c.measure),
        ui_us(|c| c.extract),
        ui_us(|c| c.convert),
        ui_us(|c| c.diff),
        solves,
        derives,
        measured,
        skips,
        mesh_us(|m| m.total),
        mesh_us(|m| m.sort),
        mesh_us(|m| m.split),
        mesh_us(|m| m.write),
        rebuilt,
        mesh_runs,
        mesh_rw,
    )
}

/// At quit: the ranked read. Splits the run by tooltip state — that split IS the question, and a
/// per-owner breakdown says whether one hover channel (bag slot vs spell button) is the expensive
/// one. The worst frames print with their tooltip context so a spike is attributable to what was
/// under the cursor at the time.
fn report_on_exit(mut exits: MessageReader<AppExit>, mut rec: ResMut<Recorder>) {
    if exits.read().next().is_none() {
        return;
    }
    let _ = rec.out.flush();
    let rows: Vec<Row> = rec.rows.clone();
    if rows.is_empty() {
        info!("hover log: no frames recorded");
        return;
    }
    println!("{}", report(&rows, &rec.path));
}

/// Re-read a recorded CSV and print the report — `WOW_HOVER_LOG_REPORT=<path>`, handled before the
/// app starts (see `main`). The analysis moves faster than the sessions that feed it: a question
/// this report cannot yet answer is one column away, and re-deriving it from rows already on disk
/// costs nobody a second play session.
pub fn report_recorded_file(path: &str) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("hover log: cannot read {path}: {e}");
            return;
        }
    };
    let mut rows = Vec::new();
    for line in text.lines().skip(1) {
        if let Some(r) = parse_row(line) {
            rows.push(r);
        }
    }
    if rows.is_empty() {
        eprintln!("hover log: no rows parsed from {path}");
        return;
    }
    println!("{}", report(&rows, path));
}

/// One CSV line back into a [`Row`]. Splits on commas OUTSIDE quotes (the text columns carry
/// commas and doubled quotes — `cell`'s escaping, read back).
fn parse_row(line: &str) -> Option<Row> {
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    fields.push(cur);
    if fields.len() < 17 {
        return None;
    }
    // The render-side columns are APPENDED (a pre-1625 recording has none) — absent reads as zero,
    // the same posture `spliced` took when it was added.
    let n = |i: usize| fields.get(i).and_then(|f| f.parse().ok()).unwrap_or(0);
    let mesh = crate::ui_pass::UiMeshCost {
        rebuilt: n(18) > 0,
        total: n(18),
        sort: n(19),
        split: n(20),
        write: n(21),
        quads: n(22) as usize,
        runs: n(23) as usize,
        rewrites: n(24) as usize,
    };
    let num = |i: usize| fields[i].parse::<f64>().ok();
    let owner = fields[14].clone();
    Some(Row {
        frame_ms: num(0)? as f32,
        cpu_ms: num(1)? as f32,
        cost: UiFrameCost {
            measured: num(12)? as usize,
            measured_texts: if fields[13].is_empty() {
                Vec::new()
            } else {
                fields[13].split(" ⏎ ").map(str::to_string).collect()
            },
            tick: num(2)? as u128,
            resolve: num(3)? as u128,
            measure: num(4)? as u128,
            extract: num(5)? as u128,
            convert: num(6)? as u128,
            diff: num(7)? as u128,
            quads: num(8)? as usize,
            solves: num(9)? as u64,
            derives: num(10)? as u64,
            skipped: num(11)? != 0.0,
            // Appended column (absent in pre-splice recordings — those read back as 0).
            spliced: fields.get(17).and_then(|f| f.parse().ok()).unwrap_or(0),
            // Appended after the mesh block for the same reason `spliced` was appended after
            // the tip block: a new column goes on the END so every recording made before it
            // still reads.
            dropped: fields.get(25).and_then(|f| f.parse().ok()).unwrap_or(0),
        },
        mesh,
        tip: if owner.is_empty() {
            None
        } else {
            Some((owner, num(15)? as i64, fields[16].clone()))
        },
    })
}

/// The report body, shared by the live exit path and the offline reader.
fn report(rows: &[Row], path: &str) -> String {
    let mut out = String::new();
    out.push_str("\n==================== HOVER LOG ====================\n");
    let all: Vec<&Row> = rows.iter().collect();
    let idle: Vec<&Row> = rows.iter().filter(|r| r.tip.is_none()).collect();
    let hover: Vec<&Row> = rows.iter().filter(|r| r.tip.is_some()).collect();
    out.push_str(&summarize("ALL", &all));
    out.push('\n');
    out.push_str(&summarize("no tooltip", &idle));
    out.push('\n');
    out.push_str(&summarize("tooltip up", &hover));
    out.push('\n');
    // Per hover target, busiest first — one line per owner frame the tooltip was anchored to.
    let mut by_owner: std::collections::BTreeMap<String, Vec<&Row>> = Default::default();
    for r in &hover {
        if let Some((owner, _, _)) = &r.tip {
            by_owner.entry(owner.clone()).or_default().push(r);
        }
    }
    let mut owners: Vec<(String, Vec<&Row>)> = by_owner.into_iter().collect();
    owners.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (owner, rs) in owners.iter().take(8) {
        out.push_str(&summarize(&format!("  ↳ {owner}"), rs));
        out.push('\n');
    }
    // The split that separates a STEADY tax from a per-CHANGE one: frames that ran the layout
    // fixpoint against frames that did not. A hover whose quiet frames are as expensive as its
    // solving ones has a per-frame problem; one where the cost lives entirely in the solving
    // frames is paying per content CHANGE — and moving the mouse across a bag grid changes content
    // constantly, which is the same felt hitch by a different mechanism and a different fix.
    let solved: Vec<&Row> = hover
        .iter()
        .copied()
        .filter(|r| r.cost.solves > 0)
        .collect();
    let quiet: Vec<&Row> = hover
        .iter()
        .copied()
        .filter(|r| r.cost.solves == 0)
        .collect();
    out.push_str(&summarize("tooltip up · frames that SOLVED", &solved));
    out.push('\n');
    out.push_str(&summarize("tooltip up · frames that did NOT", &quiet));
    out.push('\n');

    // The churn roll-up: which strings the font engine was asked to re-shape, and on how many
    // frames. A string appearing on hundreds of frames is a measure key that never settles — the
    // per-frame re-shape and the forced second resolve both hang off it, and it is the fix's
    // target. Counted per frame it appears on, not per request, so one loud string reads as one
    // row.
    let mut churn: std::collections::HashMap<String, (usize, usize)> = Default::default();
    for r in rows {
        for t in &r.cost.measured_texts {
            let e = churn.entry(t.clone()).or_default();
            e.0 += 1;
            e.1 += usize::from(r.tip.is_some());
        }
    }
    let mut churn: Vec<(String, (usize, usize))> = churn.into_iter().collect();
    churn.sort_by_key(|(_, (all, _))| std::cmp::Reverse(*all));
    if !churn.is_empty() {
        out.push_str("  re-shaped strings (frames seen / of those, with a tooltip up):\n");
        for (text, (all, hovering)) in churn.iter().take(15) {
            out.push_str(&format!("    {all:>6} {hovering:>6}   {text:?}\n"));
        }
    }

    // The worst frames, with what was under the cursor.
    // Ranked by CPU, not wall: under vsync every wall reading rails at the present interval and
    // says nothing (`perf.rs`) — CPU is the cost meter the rail cannot fool.
    let mut worst = rows.to_vec();
    worst.sort_by(|a, b| b.cpu_ms.total_cmp(&a.cpu_ms));
    out.push_str(
        "  worst frames by CPU (cpu ms / wall ms / ui tick+resolve+convert μs / tooltip):\n",
    );
    for r in worst.iter().take(12) {
        let tip = r.tip.as_ref().map_or_else(
            || "-".to_string(),
            |(o, n, f)| format!("{o} [{n} lines] {f}"),
        );
        out.push_str(&format!(
            "    {:>8.2} {:>7.2}  {:>6}+{:>6}+{:>6}   {tip}\n",
            r.cpu_ms, r.frame_ms, r.cost.tick, r.cost.resolve, r.cost.convert
        ));
    }
    out.push_str(&format!("  rows: {path}\n"));
    out.push_str("===================================================\n");
    out
}
