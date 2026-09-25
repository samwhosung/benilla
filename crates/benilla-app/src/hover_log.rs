//! The hover-cost recorder (`WOW_HOVER_LOG`): every frame to a CSV, tagged with what the tooltip
//! showed, and a report at exit ranking cost by tooltip state.
//!
//! `WOW_HOVER_LOG=1` writes `benilla-config/Diagnostics/hover-log.csv`; any other value is a path.
//! Columns per frame: wall time, process-CPU delta, the UI pass's phase split in microseconds
//! (the `WOW_UI_COST=1` marks), the mesh rebuild's split, and the tooltip (owner, line count,
//! first line). A spike with flat UI phases is not a UI-pass cost.

use std::io::Write;

use bevy::prelude::*;
use bevy::time::Real;

use benilla_ui::script::UiScript;

use crate::ui_script::{UiCostWanted, UiFrameCost};

/// Where to write, from `$WOW_HOVER_LOG`: unset or `0` is off, `1` is
/// `benilla-config/Diagnostics/hover-log.csv` (off in a hermetic run), anything else a path.
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

/// Whether the recorder is on, read once; the UI pass's phase marks consult it every frame.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| log_path().is_some())
}

/// One recorded frame, kept for the exit report.
#[derive(Clone)]
struct Row {
    frame_ms: f32,
    cpu_ms: f32,
    cost: UiFrameCost,
    /// The same frame's mesh rebuild cost.
    mesh: crate::ui_pass::UiMeshCost,
    /// `(owner frame, line count, first line)`, or `None` with no tooltip up.
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
        // `init_resource` first so this holds whichever plugin builds first.
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

/// The tooltip context, asked of the Lua VM, once per frame while recording.
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
    // Mean FontStrings re-shaped per frame; a settled UI sits at 0.
    let measured = rows.iter().map(|r| r.cost.measured as f64).sum::<f64>() / f64::from(n);
    // How often the layout fixpoint ran, and how often the extract gate skipped conversion.
    #[allow(clippy::cast_precision_loss)]
    let solves = rows.iter().map(|r| r.cost.solves as f64).sum::<f64>() / f64::from(n);
    // Solves that had to derive the layout graph first; should be zero, and
    // `WOW_LAYOUT_DERIVE_TRACE=<secs>:<n>` backtraces one on a live run.
    let derives = rows.iter().map(|r| r.cost.derives as f64).sum::<f64>() / f64::from(n);
    // The mesh half: a population that rebuilds every frame pays whole-interface work.
    let mesh_us = |f: fn(&crate::ui_pass::UiMeshCost) -> u128| {
        rows.iter().map(|r| f(&r.mesh) as f64).sum::<f64>() / f64::from(n)
    };
    let rebuilt = 100.0 * rows.iter().filter(|r| r.mesh.rebuilt).count() as f32 / n;
    let mesh_runs = rows.iter().map(|r| r.mesh.runs as f64).sum::<f64>() / f64::from(n);
    // A rewritten batch re-extracts; `rewrites` tracking `runs` means the skip gate never holds.
    let mesh_rw = rows.iter().map(|r| r.mesh.rewrites as f64).sum::<f64>() / f64::from(n);
    let skips = 100.0 * rows.iter().filter(|r| r.cost.skipped).count() as f32 / n;
    // Over 1.5 budgets, not the raw budget: vsynced wall time jitters around the interval.
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

/// At quit: the report, split by tooltip state and owner, with the worst frames' tooltips.
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

/// Re-reads a recorded CSV and prints the report (`WOW_HOVER_LOG_REPORT=<path>`, before the app
/// starts).
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

/// One CSV line back into a [`Row`], splitting on commas outside quotes.
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
    // Later columns are appended at the end, and an older recording without them reads zero.
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
            spliced: fields.get(17).and_then(|f| f.parse().ok()).unwrap_or(0),
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
    // Per owner frame, busiest first.
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
    // Frames that solved layout against those that did not: a steady tax or a per-change one.
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

    // Re-shaped strings by frames seen; one on hundreds of frames has a measure key that never
    // settles.
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

    // The worst frames by CPU, since vsynced wall time rails at the present interval.
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
