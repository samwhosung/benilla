//! The UI pass: [`tick_script`] runs the VM's frame and [`paint_script`] turns
//! [`UiScript::extract`]'s output into [`UiQuads`]. The held payload's icon is the hardware cursor
//! ([`crate::cursor`]); a capture, which cannot show that cursor, draws [`cursor_icon_quad`].

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::{QuadContent, TexCoords, UiScript};

use crate::ui_pass::{UiQuad, UiQuads, UvRect};
use crate::ui_text::UiFontAtlas;
use benilla_assets::WorldAssets;

mod colorselect;
mod text;

/// `WOW_UI_COST=1`: one untraced `[ui-cost]` line per frame, each phase's μs and the quad counts.
pub(crate) fn ui_cost_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_UI_COST").as_deref() == Ok("1"))
}

/// `WOW_UI_GATE=1`: log why the extract gate and the splice missed ([`report_gate_miss`]).
fn gate_log_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_UI_GATE").as_deref() == Ok("1"))
}

/// `WOW_UI_DIFF=1`: name the first base-lane quad that changed; it pins the full conversion.
fn ui_diff_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_UI_DIFF").is_some())
}

/// `WOW_UI_SPLICE_VERIFY=1`: also run the full conversion on each spliced frame, and compare.
fn splice_verify_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_UI_SPLICE_VERIFY").is_some())
}

/// `WOW_UI_PICK=<x>,<y>[,<r>]`: log every quad whose rect covers that point, or comes within `r`
/// px, once per `z` key; logical window px, y-down, as [`convert_entry`]'s `rect`.
fn ui_pick_point() -> Option<(Vec2, f32)> {
    static AT: std::sync::OnceLock<Option<(Vec2, f32)>> = std::sync::OnceLock::new();
    *AT.get_or_init(|| {
        let raw = std::env::var("WOW_UI_PICK").ok()?;
        let mut it = raw.split(',');
        let x: f32 = it.next()?.trim().parse().ok()?;
        let y: f32 = it.next()?.trim().parse().ok()?;
        let r = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0.0);
        Some((Vec2::new(x, y), r))
    })
}

/// One `[ui-pick]` line per covering quad ([`ui_pick_point`]), deduped by paint key.
fn report_ui_pick(eq: &benilla_ui::script::ExtractedQuad, rect: Rect, at: Vec2, r: f32) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: std::sync::OnceLock<Mutex<HashSet<u64>>> = std::sync::OnceLock::new();
    if !rect.inflate(r).contains(at) {
        return;
    }
    if !SEEN
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut s| s.insert(eq.z))
        .unwrap_or(false)
    {
        return;
    }
    let what = match &eq.content {
        QuadContent::Frame => "frame-slot".to_string(),
        QuadContent::Minimap { .. } => "minimap".to_string(),
        QuadContent::Texture {
            path,
            color,
            tex_coords,
            ..
        } => format!(
            "texture {:?} color={color:?} crop={tex_coords:?}",
            path.as_deref().unwrap_or("<none>")
        ),
        other => format!("{other:?}"),
    };
    info!(
        "[ui-pick] {:?} z={} rect=({:.2},{:.2})-({:.2},{:.2}) alpha={:.3} {what}",
        eq.target, eq.z, rect.min.x, rect.min.y, rect.max.x, rect.max.y, eq.alpha,
    );
}

/// A content arm's short, stable name for a log line.
fn content_kind(c: &QuadContent) -> &'static str {
    match c {
        QuadContent::Frame => "frame",
        QuadContent::Minimap { .. } => "minimap",
        QuadContent::ModelPane { .. } => "modelpane",
        QuadContent::Texture { .. } => "texture",
        QuadContent::ColorWheel => "colorwheel",
        QuadContent::ColorValue { .. } => "colorvalue",
        QuadContent::Backdrop { .. } => "backdrop",
        QuadContent::Text { .. } => "text",
    }
}

/// Which axes run past the texture, the reference's tiling idiom (`SetTexCoord(0, n, 0, 1)`
/// repeats the art n times), and so sample `Repeat`; a bounded axis on `Repeat` would filter its
/// edge against the opposite one. Its tolerance must match [`uv_clamp_window`]'s.
pub(crate) fn tiling_axes(uv: &UvRect) -> (bool, bool) {
    let past = |axis: usize| {
        uv.corners
            .iter()
            .any(|c| !(-0.001..=1.001).contains(&c[axis]))
    };
    (past(0), past(1))
}

/// The [`UiQuad::uv_clamp`] window of an atlas crop, inset half a texel so a magnified cell does
/// not filter in its neighbour. A tiling axis is left alone, as
/// [`benilla_ui::script::inset_atlas_bleed`] does; a whole-texture axis needs no window.
fn uv_clamp_window(uv: &UvRect, size: (u32, u32)) -> Option<[f32; 4]> {
    let mut out = [1.0, 1.0, 0.0, 0.0]; // both axes off: `min > max`
    let mut any = false;
    for (axis, texels) in [(0usize, size.0), (1usize, size.1)] {
        if texels < 2 {
            continue; // a 1-texel axis has no interior to inset toward
        }
        let (lo, hi) = uv.corners.iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| {
            (lo.min(c[axis]), hi.max(c[axis]))
        });
        // Outside the texture (tiling), or the whole of it (the sampler's clamp is the window).
        if lo < -0.001 || hi > 1.001 || (lo <= 0.001 && hi >= 0.999) {
            continue;
        }
        let half = 0.5 / texels as f32;
        // A crop under one texel wide pins both bounds to its centre.
        let (lo, hi) = match hi - lo > 2.0 * half {
            true => (lo + half, hi - half),
            false => {
                let mid = 0.5 * (lo + hi);
                (mid, mid)
            }
        };
        out[axis] = lo;
        out[axis + 2] = hi;
        any = true;
    }
    any.then_some(out)
}

/// Log why the extract gate did not skip: the input that differed, or the first entry that moved.
fn report_gate_miss(
    script: &UiScript,
    now: &[benilla_ui::script::ExtractedQuad],
    prev: &[benilla_ui::script::ExtractedQuad],
    dims_eq: bool,
    generation_eq: bool,
    text_ui_eq: bool,
    portraits_eq: bool,
) {
    if !dims_eq {
        eprintln!("[ui-gate] miss: window size / seam scale / DPI changed");
        return;
    }
    if !generation_eq {
        // A full sheet was repacked, moving every UV (`WOW_GLYPH_CACHE=1` shows its occupancy).
        eprintln!("[ui-gate] miss: the glyph sheet reset");
        return;
    }
    if !text_ui_eq {
        eprintln!("[ui-gate] miss: focused editbox text-UI changed");
        return;
    }
    if !portraits_eq {
        eprintln!("[ui-gate] miss: portrait sources changed");
        return;
    }
    if now.len() != prev.len() {
        eprintln!(
            "[ui-gate] miss: render list {} -> {} entries",
            prev.len(),
            now.len()
        );
        return;
    }
    match now.iter().zip(prev).position(|(a, b)| a != b) {
        Some(i) => {
            let (a, b) = (&now[i], &prev[i]);
            let (was, now_s) = (format!("{b:?}"), format!("{a:?}"));
            let owner = match a.target {
                benilla_ui::order::ZTarget::Frame(fh) => script.frame_name(fh),
                benilla_ui::order::ZTarget::Region(_) => None,
            };
            eprintln!(
                "[ui-gate] miss: entry {i}/{} target={:?} name={owner:?}\n           was {}\n           now {}",
                now.len(),
                a.target,
                &was[..was.len().min(280)],
                &now_s[..now_s.len().min(280)],
            );
        }
        // Nothing moved: capture mode never skips.
        None => eprintln!("[ui-gate] miss: capture mode (the gate never skips under a capture)"),
    }
}

/// Last frame's extract-gate inputs, in one `Local` as the gate compares all or none.
#[derive(Default)]
pub(super) struct GateInputs {
    extracted: Vec<benilla_ui::script::ExtractedQuad>,
    text_ui: Option<benilla_ui::script::EditBoxTextUi>,
    dims: Option<(u32, u32, u32, u32)>,
    portraits: std::collections::HashMap<String, crate::portrait::PortraitSource>,
    /// The `UiFontAtlas::generation` the held glyph UVs came from; it moves only on a reset.
    generation: Option<u64>,
    /// Per-entry prefix ends: entry `i`'s quads are `spans[i-1]..spans[i]` of `UiQuads::quads`
    /// (from 0 for `i = 0`), which lets the splice re-convert one entry alone.
    spans: Vec<u32>,
    /// The stitch's ping-pong buffer: last frame's emptied allocation.
    held: Vec<UiQuad>,
}

/// How this frame's entry list lines up with last frame's.
struct Alignment {
    /// Per new entry, the old index whose quads it reuses, or `None` to convert.
    source: Vec<Option<usize>>,
    /// Old entries nothing reuses; the splice cannot undo their side effects.
    dropped: Vec<usize>,
}

/// A merge over `z`, a packed [`benilla_ui::order::ZKey`] that an insertion does not shift. Only
/// fully equal pairs reuse quads, so a mis-pairing costs conversions, never correctness: `z` is
/// not proven unique or sorted (a ScrollingMessageFrame's line sorts above its BACKGROUND layer).
fn align_entries(
    was: &[benilla_ui::script::ExtractedQuad],
    now: &[benilla_ui::script::ExtractedQuad],
) -> Alignment {
    let mut source: Vec<Option<usize>> = Vec::with_capacity(now.len());
    let mut kept = vec![false; was.len()];
    let (mut i, mut j) = (0usize, 0usize);
    while j < now.len() {
        let Some(w) = was.get(i) else {
            // Last frame's list is spent: everything left is new.
            source.push(None);
            j += 1;
            continue;
        };
        match w.z.cmp(&now[j].z) {
            std::cmp::Ordering::Equal => {
                let same = *w == now[j];
                source.push(same.then_some(i));
                kept[i] = same;
                i += 1;
                j += 1;
            }
            // Gone this frame.
            std::cmp::Ordering::Less => i += 1,
            // New this frame.
            std::cmp::Ordering::Greater => {
                source.push(None);
                j += 1;
            }
        }
    }
    let dropped = kept
        .iter()
        .enumerate()
        .filter_map(|(i, k)| (!k).then_some(i))
        .collect();
    Alignment { source, dropped }
}

/// Entry `i`'s quad range under `spans` (the [`GateInputs::spans`] encoding).
fn span_bounds(spans: &[u32], i: usize) -> (usize, usize) {
    let a = if i == 0 { 0 } else { spans[i - 1] as usize };
    (a, spans[i] as usize)
}

/// Whether an entry's conversion writes only quads, or its side channels idempotently: the splice
/// keeps last frame's link spans, minimap slot, booth panes and tile requests.
fn splice_simple(eq: &benilla_ui::script::ExtractedQuad) -> bool {
    match &eq.content {
        QuadContent::Frame
        | QuadContent::Backdrop { .. }
        // At most one quad and one idempotent tile request.
        | QuadContent::ModelPane { .. }
        | QuadContent::ColorWheel
        | QuadContent::ColorValue { .. } => true,
        // Region text only: a message line's link spans replace the whole set, and a line losing
        // its link would leave a stale rect while the tripwire's scratch stays empty.
        QuadContent::Text { .. } => matches!(eq.target, benilla_ui::order::ZTarget::Region(_)),
        QuadContent::Texture {
            portrait_unit: None,
            ..
        } => true,
        _ => false,
    }
}

/// Re-run the full conversion and log the first quad or span where the spliced list differs; of
/// the side channels only `booths.panes` is written, and re-adding its tokens is idempotent.
fn verify_splice(
    prev: &GateInputs,
    quads: &UiQuads,
    s: f32,
    w: f32,
    h: f32,
    dpi: f32,
    assets: &mut Option<ResMut<WorldAssets>>,
    images: &mut Assets<Image>,
    font_atlas: &mut Option<ResMut<UiFontAtlas>>,
    booths: &mut crate::portrait::BoothBridge,
    text_ui: Option<&benilla_ui::script::EditBoxTextUi>,
    caret_pinned: bool,
    script: &UiScript,
) {
    let mut check: Vec<UiQuad> = Vec::with_capacity(quads.quads.len());
    let mut spans: Vec<u32> = Vec::with_capacity(prev.extracted.len());
    let (mut links, mut slot) = (Vec::new(), None);
    for eq in prev.extracted.iter().cloned() {
        convert_entry(
            eq,
            s,
            w,
            h,
            dpi,
            assets,
            images,
            font_atlas,
            booths,
            text_ui,
            caret_pinned,
            &mut check,
            &mut links,
            &mut slot,
        );
        spans.push(check.len() as u32);
    }
    if spans != prev.spans {
        let at = spans
            .iter()
            .zip(&prev.spans)
            .position(|(a, b)| a != b)
            .unwrap_or(spans.len().min(prev.spans.len()));
        eprintln!(
            "[ui-splice] VERIFY FAIL: span table diverges at entry {at} of {} \
             (spliced end {:?}, full end {:?})",
            prev.extracted.len(),
            prev.spans.get(at),
            spans.get(at),
        );
        return;
    }
    let Some(i) = check
        .iter()
        .zip(&quads.quads)
        .position(|(a, b)| a != b)
        .or_else(|| {
            (check.len() != quads.quads.len()).then_some(check.len().min(quads.quads.len()))
        })
    else {
        return;
    };
    // The entry owning quad `i`: the first prefix end past it.
    let owner = spans.partition_point(|&e| e as usize <= i);
    let name = prev
        .extracted
        .get(owner)
        .and_then(|eq| script.target_owner_name(eq.target));
    eprintln!(
        "[ui-splice] VERIFY FAIL: quad {i} of {}/{} differs — entry {owner} ({}, {})\n  spliced {}\n  full    {}",
        quads.quads.len(),
        check.len(),
        name.as_deref().unwrap_or("?"),
        prev.extracted
            .get(owner)
            .map_or("<none>", |eq| content_kind(&eq.content)),
        quad_summary(quads.quads.get(i)),
        quad_summary(check.get(i)),
    );
}

fn quad_summary(q: Option<&UiQuad>) -> String {
    let Some(q) = q else {
        return "<past the end>".into();
    };
    format!(
        "z={} rect=({:.2},{:.2})-({:.2},{:.2}) color={:?} uv={:?} tex={:?}",
        q.z_key,
        q.rect.min.x,
        q.rect.min.y,
        q.rect.max.x,
        q.rect.max.y,
        q.color,
        q.uv.corners,
        q.texture.as_ref().map(|t| t.id()),
    )
}

/// Hand the VM the host's font engine for `seam`, so `GetStringWidth` right after `SetText`
/// measures, as the reference answers it inline (`0x79e510` → `0x772890`). Also seated before the
/// manifest loads ([`super::lifecycle::load_ingame_ui_on_world_entry`]), for every `<OnLoad>`.
pub(crate) fn seat_text_measurer(script: &mut UiScript, atlas: &UiFontAtlas, seam: f32) {
    script.set_text_measurer(Box::new(crate::ui_text::AtlasMeasurer::new(
        atlas.engine(),
        seam,
    )));
}

/// The UI pass's first half, before `WorldStage::Input` because the camera reads the UI hover:
/// screen size, `tick`, the measure round-trip, `resolve` and the script errors. The quads are
/// [`paint_script`]'s, after the camera that places the `WorldFrame` nameplates.
pub(super) fn tick_script(
    script: Option<NonSendMut<UiScript>>,
    window: Query<&Window, With<PrimaryWindow>>,
    // Real time: the reference's `GetTime` is the OS tick count, and virtual time clamps each
    // delta to 250 ms, so a stall would leave every GetTime-anchored timer running long.
    time: Res<Time<Real>>,
    mut ui_clock: ResMut<super::UiClock>,
    mut font_atlas: Option<ResMut<UiFontAtlas>>,
    // The booth seam's DPI, and the pane map cleared when the VM or the window is gone.
    mut booths: crate::portrait::BoothBridge,
    ui_cost_wanted: Res<super::UiCostWanted>,
    // The seam scale the engine's text-metric caches were answered under. Not a `VmMemo`: it is a
    // fact about the raster, and a new VM re-seats on `!has_text_measurer()` below.
    mut last_seam: Local<f32>,
    // The `scale_factor` measures were last answered under; 0 until the first frame.
    mut last_dpi: Local<f32>,
    ui_scale: Res<super::UiScaleCvar>,
    mut ui_cost: ResMut<super::UiFrameCost>,
    mut pass: ResMut<super::UiPassState>,
) {
    pass.live = false;
    let Some(mut script) = script else {
        // No VM, no UI sampling a booth pane: a stranded pane would keep the paper-doll camera
        // rendering behind a dead UI.
        booths.panes.0.clear();
        return;
    };
    let Ok(window) = window.single() else {
        booths.panes.0.clear();
        return;
    };
    let (w, h) = (window.width(), window.height());
    // The client's UI space is 768 units tall at every aspect (`f(screenH) = 768`, `0x41ad10`; the
    // caret's `H_px/192`, `0x77b8c0`), `768/uiScale` with the dial. The VM lives in it: quads
    // scale ×s out, and the mouse and the measures ÷s in.
    let s = super::seam_scale(h, ui_scale.0);
    // A moved seam scale or DPI stales every text metric the engine caches: integer-stepped
    // advances do not rescale, and the size snap moves a string's width by several percent,
    // enough for the ellipsis to eat fitting text. A monitor hop moves the DPI alone.
    let dpi = window.scale_factor();
    // The tile renderer sizes its cells in device pixels off this.
    booths.tiles.dpi = dpi;
    let seam_moved = *last_seam != s || *last_dpi != dpi;
    if seam_moved {
        if *last_seam != 0.0 {
            script.invalidate_text_measures();
        }
        *last_seam = s;
        *last_dpi = dpi;
    }
    // Re-seat the VM's measurer on the same edge, before the tick so the first update has it.
    if let Some(atlas) = font_atlas.as_deref() {
        if seam_moved || !script.has_text_measurer() {
            seat_text_measurer(&mut script, atlas, s);
        }
    }
    // The phase marks feed the `[ui-cost]` line and the hover recorder (`hover_log`).
    let printing = ui_cost_enabled();
    let cost_on = printing || ui_cost_wanted.0;
    let solves_before = cost_on.then(|| script.layout_solves());
    let derives_before = cost_on.then(|| script.layout_derivations());
    // Per frame: both halves' measure passes add to these, and only this zeroes them.
    ui_cost.measured = 0;
    ui_cost.measured_texts.clear();
    let mut t_mark = cost_on.then(std::time::Instant::now);
    // μs since the previous mark, re-armed; nothing with the meter off.
    let mut lap = move || -> u128 {
        if !cost_on {
            return 0;
        }
        t_mark
            .replace(std::time::Instant::now())
            .map_or(0, |t| t.elapsed().as_micros())
    };
    {
        let _span = bevy::log::info_span!("ui_script: tick").entered();
        let resized = script.set_screen_size(w / s, if h > 0.0 { h / s } else { 768.0 });
        // A resize re-runs what the interface computed from the old screen size.
        if resized {
            super::manifest::on_screen_resized(&script);
        }
        script.tick(time.delta_secs());
    }
    // The VM's clock anchored at `Time<Real>`'s last update, whose deltas it accumulates; every
    // `Instant` to `GetTime` conversion goes through this pair, never `Instant::now()`.
    *ui_clock = super::UiClock {
        anchor: time.last_update().unwrap_or_else(std::time::Instant::now),
        ui_now: script.now(),
    };
    let us_tick = lap();
    // ── The FontString measure round-trip, before the resolve ────────────────────────────────
    // A request reads only region data, never a resolved rect, and `resolve` writes no region's
    // size, so measuring first lets the frame's one resolve see the answers.
    let mut measured_any = false;
    if let Some(atlas) = font_atlas.as_deref_mut() {
        measured_any = measure_fontstrings(&mut script, atlas, s, &mut ui_cost, ui_cost_wanted.0);
    }
    {
        let _span = bevy::log::info_span!("ui_script: resolve").entered();
        script.resolve();
    }
    // Only a frame that measured can have a fresh request the pass above missed.
    if measured_any {
        if let Some(atlas) = font_atlas.as_deref_mut() {
            if measure_fontstrings(&mut script, atlas, s, &mut ui_cost, ui_cost_wanted.0) {
                script.resolve();
            }
        }
    }
    let us_resolve = lap();
    let measure_span = bevy::log::info_span!("ui_script: measure").entered();
    // Message lines ask for their wrapped row count at the frame's width, so a long line pushes
    // older ones up; no re-resolve, as rows move only the bands, never the anchors.
    if let Some(atlas) = font_atlas.as_deref_mut() {
        let requests = script.message_lines_needing_measure();
        if !requests.is_empty() {
            let rows: Vec<(u32, u32, u16, u64)> = requests
                .iter()
                .map(|r| {
                    let n = crate::ui_text::measure_wrapped_rows(
                        &mut atlas.lock(),
                        &r.text,
                        // The resolved width carries the frame scale; the font height does not.
                        r.wrap_width * s,
                        crate::ui_text::FontSpec {
                            path: r.font.as_deref(),
                            height: crate::ui_text::drawn_px(r.height, None, s * r.scale),
                            outline: r.outline, // the outline biases the stepped width
                            alpha_gradient: None,
                        },
                    );
                    (r.frame, r.index, n, r.key)
                })
                .collect();
            script.set_message_line_rows(&rows);
        }
    }
    drop(measure_span);
    let us_measure = lap();
    // Script errors reach the Lua handler (`_ERRORMESSAGE`, `BasicControls.xml:16`) before the
    // log, so a failing handler lands in this frame's drain.
    script.dispatch_script_errors_to_handler();
    for err in script.take_errors() {
        warn!("ui_script: {err}");
    }
    for w in script.take_warnings() {
        warn!("ui_script: {w}");
    }

    // ── The handover ────────────────────────────────────────────────────────────────────────
    *pass = super::UiPassState {
        live: true,
        seam: s,
        dpi,
        us_tick,
        us_resolve,
        us_measure,
        solves_before: solves_before.unwrap_or(0),
        derives_before: derives_before.unwrap_or(0),
    };
}

/// The UI pass's second half, after the camera and the plate driver: an incremental measure and
/// `resolve` for what was written since the tick, the walk, and the conversion into [`UiQuads`].
pub(super) fn paint_script(
    script: Option<NonSendMut<UiScript>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut quads: ResMut<UiQuads>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut font_atlas: Option<ResMut<UiFontAtlas>>,
    // The booth seam both ways: the bakes regions sample, and the pane aspects published back.
    mut booths: crate::portrait::BoothBridge,
    // Capture mode (the cursor icon quad) and [`super::UiCostWanted`], tupled to stay inside
    // Bevy's 16-parameter system limit.
    run: (
        Option<Res<crate::run_mode::CaptureMode>>,
        Res<super::UiCostWanted>,
    ),
    // The `<Minimap>` widget slot, parked for `minimap::emit_minimap` to fill later in the frame.
    mut parked_minimap: ResMut<crate::minimap::MinimapWidget>,
    // The extract gate's memory. The conversion is a pure function of the extracted list, the
    // editbox text UI, the raster (window size, seam scale, `scale_factor`), the portraits and the
    // glyph-sheet generation. A `VmMemo`, since a skip also skips pushes into the VM, so a new
    // VM's first frame always converts.
    mut prev: Local<crate::ui_script::VmMemo<GateInputs>>,
    mut ui_cost: ResMut<super::UiFrameCost>,
    pass: Res<super::UiPassState>,
) {
    let (capture, ui_cost_wanted) = run;
    // Stand down with the tick half, which already cleared the booth panes.
    if !pass.live {
        return;
    }
    let Some(mut script) = script else {
        return;
    };
    let prev = prev.get(&script);
    let Ok(window) = window.single() else {
        return;
    };
    let (w, h) = (window.width(), window.height());
    let (s, dpi) = (pass.seam, pass.dpi);
    let (us_tick, us_resolve) = (pass.us_tick, pass.us_resolve);
    let (solves_before, derives_before) = (Some(pass.solves_before), Some(pass.derives_before));
    let printing = ui_cost_enabled();
    let cost_on = printing || ui_cost_wanted.0;
    let mut t_mark = cost_on.then(std::time::Instant::now);
    let mut lap = move || -> u128 {
        if !cost_on {
            return 0;
        }
        t_mark
            .replace(std::time::Instant::now())
            .map_or(0, |t| t.elapsed().as_micros())
    };
    // ── The second measure and resolve ───────────────────────────────────────────────────────
    // What was written since the tick half, above all the nameplate anchors from this frame's
    // camera (`vplates::drive_vplates`); a renamed plate needs its measure too.
    let mut measured_any = false;
    if let Some(atlas) = font_atlas.as_deref_mut() {
        measured_any = measure_fontstrings(&mut script, atlas, s, &mut ui_cost, ui_cost_wanted.0);
    }
    {
        let _span = bevy::log::info_span!("ui_script: resolve").entered();
        script.resolve();
    }
    if measured_any {
        if let Some(atlas) = font_atlas.as_deref_mut() {
            if measure_fontstrings(&mut script, atlas, s, &mut ui_cost, ui_cost_wanted.0) {
                script.resolve();
            }
        }
    }
    // Folded into the tick half's own measure figure: one frame, one row.
    let us_measure = pass.us_measure + lap();
    // The glyph sheet's repack counter: a repack moves every cached cell's UV.
    let generation = font_atlas.as_deref().map(|a| a.generation);
    let extract_span = bevy::log::info_span!("ui_script: extract").entered();
    let mut out = Vec::new();
    let mut assets = world_assets;
    let mut minimap_slot = None;
    // Message lines' link spans for the click hit-test; region FontStrings' are not collected.
    let mut link_spans: Vec<(
        benilla_ui::widget::FrameHandle,
        benilla_ui::layout::Rect,
        String,
        String,
    )> = Vec::new();
    // The focused editbox's per-byte advances, measured as it draws, for click and scroll.
    if let Some(atlas) = font_atlas.as_deref_mut() {
        if let Some(req) = script.editbox_advances_request() {
            let spec = crate::ui_text::FontSpec {
                path: req.font.as_deref(),
                // At the drawn size, divided by the seam alone: screen UI units, like the mouse.
                height: crate::ui_text::drawn_px(req.height, None, s * req.scale),
                outline: req.outline,
                alpha_gradient: None, // alpha never changes metrics
            };
            let cum: Vec<f32> = crate::ui_text::line_advances(&mut atlas.lock(), &req.text, spec)
                .iter()
                .map(|a| a / s)
                .collect();
            // A multiline box also gets the draw's row starts and pitch, in UI units.
            let (rows, cell_h) = match req.wrap_width {
                Some(w) => crate::ui_text::line_rows(&mut atlas.lock(), &req.text, w * s, spec),
                None => (vec![0], 0.0),
            };
            script.set_editbox_advances(req.id, req.key, cum, rows, cell_h / s);
        }
    }
    // The focused editbox's text UI: its Text quad, scroll window, caret and selection spans,
    // and the blink phase (0.5 s, `0x77a790`).
    let text_ui = script.focused_editbox_text_ui();
    let _ = lap(); // re-arm: the editbox seam above is not the walk's cost
    let extracted = script.extract();
    let us_exm = lap();
    let n_extracted = extracted.len();
    // ── The extract gate ──────────────────────────────────────────────────────
    // Equal inputs skip the conversion; the quads, minimap slot and link spans stay as they are.
    let dims = (w.to_bits(), h.to_bits(), s.to_bits(), dpi.to_bits());
    let settled = capture.is_none()
        && prev.dims == Some(dims)
        && prev.generation == generation
        && text_ui == prev.text_ui
        && booths.images.0 == prev.portraits
        && extracted == prev.extracted;
    if settled {
        drop(extract_span);
        let us_cmp = lap();
        if cost_on {
            let solves = script.layout_solves() - solves_before.unwrap_or(0);
            *ui_cost = super::UiFrameCost {
                measured: ui_cost.measured,
                measured_texts: std::mem::take(&mut ui_cost.measured_texts),
                tick: us_tick,
                resolve: us_resolve,
                measure: us_measure,
                extract: us_exm,
                convert: us_cmp,
                diff: 0,
                quads: quads.quads.len(),
                solves,
                derives: script.layout_derivations() - derives_before.unwrap_or(0),
                skipped: true,
                spliced: 0,
                dropped: 0,
            };
        }
        if printing {
            let solves = script.layout_solves() - solves_before.unwrap_or(0);
            eprintln!(
                "[ui-cost] tick={us_tick} resolve={us_resolve} measure={us_measure} \
                 exm={us_exm} exa={us_cmp} diff=0 eq={n_extracted} quads={} \
                 solves={solves} changed=0 skip=1",
                quads.quads.len()
            );
        }
        return;
    }
    if gate_log_enabled() {
        report_gate_miss(
            &script,
            &extracted,
            &prev.extracted,
            prev.dims == Some(dims),
            prev.generation == generation,
            text_ui == prev.text_ui,
            booths.images.0 == prev.portraits,
        );
    }
    // ── The per-entry splice ─────────────────────────────────────────────────────────────────
    // When only the list moved and the few changed entries write only quads, they re-convert
    // alone and stitch into last frame's `UiQuads` at the ranges `prev.spans` holds.
    'splice: {
        macro_rules! no_splice {
            ($($arg:tt)*) => {{
                if gate_log_enabled() {
                    eprintln!("[ui-splice] miss: {}", format_args!($($arg)*));
                }
                break 'splice;
            }};
        }
        if capture.is_some() {
            no_splice!("capture mode");
        }
        if ui_diff_enabled() {
            no_splice!("WOW_UI_DIFF pins the full path");
        }
        if prev.dims != Some(dims) {
            no_splice!("window dims moved");
        }
        if prev.generation != generation {
            no_splice!("font atlas generation moved");
        }
        // The caret blink is wall-clock state in no entry, the Text arm's one time input.
        if text_ui != prev.text_ui {
            no_splice!("focused editbox text-ui moved");
        }
        if booths.images.0 != prev.portraits {
            no_splice!("portrait sources moved");
        }
        // The spans describe last conversion's list; if anything replaced it, convert in full.
        if prev.spans.len() != prev.extracted.len()
            || prev.spans.last().copied().unwrap_or(0) as usize != quads.quads.len()
        {
            no_splice!("span table stale");
        }
        let align = align_entries(&prev.extracted, &extracted);
        // A window opening moves many entries, which the full conversion serves.
        const SPLICE_MAX: usize = 64;
        let changed: Vec<usize> = align
            .source
            .iter()
            .enumerate()
            .filter_map(|(j, s)| s.is_none().then_some(j))
            .collect();
        if changed.len() + align.dropped.len() > SPLICE_MAX {
            no_splice!(
                "{} entries changed and {} dropped, over the {SPLICE_MAX} bound",
                changed.len(),
                align.dropped.len()
            );
        }
        // All-equal belongs to the gate above; reaching here means two comparisons disagree.
        if changed.is_empty() && align.dropped.is_empty() {
            no_splice!("no entry differs (a comparison razor slipped)");
        }
        // Departing entries too: the splice cannot un-park a departing minimap slot.
        if let Some(eq) = changed
            .iter()
            .map(|&j| &extracted[j])
            .chain(align.dropped.iter().map(|&i| &prev.extracted[i]))
            .find(|eq| !splice_simple(eq))
        {
            no_splice!(
                "a {} entry ({}) is not spliceable",
                content_kind(&eq.content),
                script.target_owner_name(eq.target).unwrap_or_default()
            );
        }
        // The scratch side channels are a tripwire: an entry that writes one takes the full path.
        let mut scratch: Vec<UiQuad> = Vec::new();
        let mut scratch_ranges: Vec<std::ops::Range<usize>> = Vec::with_capacity(changed.len());
        let mut scratch_links = Vec::new();
        let mut scratch_slot = None;
        for &j in &changed {
            let at = scratch.len();
            convert_entry(
                extracted[j].clone(),
                s,
                w,
                h,
                dpi,
                &mut assets,
                &mut images,
                &mut font_atlas,
                &mut booths,
                text_ui.as_ref(),
                capture.is_some(),
                &mut scratch,
                &mut scratch_links,
                &mut scratch_slot,
            );
            scratch_ranges.push(at..scratch.len());
        }
        if !scratch_links.is_empty() || scratch_slot.is_some() {
            no_splice!("a re-converted entry wrote a side channel");
        }
        // In place: nothing inserted or removed, and each changed entry keeps its quad count.
        let identity = prev.extracted.len() == extracted.len()
            && align
                .source
                .iter()
                .enumerate()
                .all(|(j, s)| s.is_none_or(|i| i == j));
        let counts_stable = identity
            && changed.iter().zip(&scratch_ranges).all(|(&j, r)| {
                let (a, b) = span_bounds(&prev.spans, j);
                b - a == r.len()
            });
        let mut dirtied = false;
        if counts_stable {
            for (&j, r) in changed.iter().zip(&scratch_ranges) {
                let (a, b) = span_bounds(&prev.spans, j);
                if quads.quads[a..b] != scratch[r.clone()] {
                    quads.quads[a..b].clone_from_slice(&scratch[r.clone()]);
                    dirtied = true;
                }
            }
        } else {
            // Stitch a fresh list: kept runs moved out of last frame's (no `Arc` clones), the
            // re-converted entries from the scratch, new spans; `prev.held` keeps the allocation.
            let mut old = std::mem::take(&mut prev.held);
            std::mem::swap(&mut old, &mut quads.quads);
            quads.quads.clear();
            quads.quads.reserve(old.len() + scratch.len());
            let mut new_spans: Vec<u32> = Vec::with_capacity(extracted.len());
            {
                let mut src = old.drain(..);
                // Quads the drain has yielded; kept runs strictly increase, so one pass does it.
                let mut cursor = 0usize;
                let mut ci = 0usize;
                for s in &align.source {
                    match s {
                        Some(i) => {
                            let (a, b) = span_bounds(&prev.spans, *i);
                            if a > cursor {
                                src.by_ref().nth(a - cursor - 1);
                            }
                            quads.quads.extend(src.by_ref().take(b - a));
                            cursor = b;
                        }
                        None => {
                            quads
                                .quads
                                .extend_from_slice(&scratch[scratch_ranges[ci].clone()]);
                            ci += 1;
                        }
                    }
                    new_spans.push(quads.quads.len() as u32);
                }
            }
            prev.held = old;
            prev.spans = new_spans;
            dirtied = true;
        }
        if dirtied {
            quads.dirty = true;
        }
        prev.extracted = extracted;
        if splice_verify_enabled() {
            verify_splice(
                prev,
                &quads,
                s,
                w,
                h,
                dpi,
                &mut assets,
                &mut images,
                &mut font_atlas,
                &mut booths,
                text_ui.as_ref(),
                capture.is_some(),
                &script,
            );
        }
        drop(extract_span);
        let us_spl = lap();
        if cost_on {
            let solves = script.layout_solves() - solves_before.unwrap_or(0);
            *ui_cost = super::UiFrameCost {
                measured: ui_cost.measured,
                measured_texts: std::mem::take(&mut ui_cost.measured_texts),
                tick: us_tick,
                resolve: us_resolve,
                measure: us_measure,
                extract: us_exm,
                convert: us_spl,
                diff: 0,
                quads: quads.quads.len(),
                solves,
                derives: script.layout_derivations() - derives_before.unwrap_or(0),
                skipped: false,
                spliced: changed.len(),
                dropped: align.dropped.len(),
            };
        }
        if printing {
            let solves = script.layout_solves() - solves_before.unwrap_or(0);
            eprintln!(
                "[ui-cost] tick={us_tick} resolve={us_resolve} measure={us_measure} \
                 exm={us_exm} exa={us_spl} diff=0 eq={n_extracted} quads={} \
                 solves={solves} changed={} skip=0 spliced={} dropped={}",
                quads.quads.len(),
                u8::from(dirtied),
                changed.len(),
                align.dropped.len()
            );
        }
        return;
    }
    prev.dims = Some(dims);
    prev.generation = generation;
    prev.text_ui = text_ui.clone();
    prev.portraits = booths.images.0.clone();
    prev.extracted = extracted.clone();
    // Cleared only on the full-conversion path: a settled frame's map is still true, and
    // clearing it would put the body panes' cameras to sleep.
    booths.panes.0.clear();
    let mut spans: Vec<u32> = Vec::with_capacity(prev.extracted.len());
    for eq in extracted {
        convert_entry(
            eq,
            s,
            w,
            h,
            dpi,
            &mut assets,
            &mut images,
            &mut font_atlas,
            &mut booths,
            text_ui.as_ref(),
            capture.is_some(),
            &mut out,
            &mut link_spans,
            &mut minimap_slot,
        );
        spans.push(out.len() as u32);
    }
    prev.spans = spans;
    // Capture mode only: the held payload's icon at the mouse, for the hardware cursor a capture
    // cannot show; every payload kind, as `crate::cursor`'s `payload_icon`.
    if capture.is_some() {
        use benilla_ui::script::CursorPayload;
        let texture = script.cursor_payload().and_then(|p| match p {
            CursorPayload::Item(i) => i.texture,
            CursorPayload::Spell(s) => s.texture,
            CursorPayload::Action(a) => a.texture,
            CursorPayload::Macro(m) => m.texture,
            CursorPayload::PetAction(p) => p.texture,
            CursorPayload::StablePet(p) => Some(p.texture),
            CursorPayload::Merchant(m) => m.texture,
            CursorPayload::Money(m) => {
                Some(benilla_ui::script::coin_icon(i64::from(m.copper)).to_string())
            }
        });
        if let (Some(texture), Some(pos)) = (texture, window.cursor_position()) {
            if let Some(handle) = assets
                .as_mut()
                .and_then(|a| a.sprite_texture(&texture, &mut images))
            {
                out.push(cursor_icon_quad(pos, handle));
            }
        }
    }

    // The link spans replace last frame's set, so `OnHyperlinkClick` hit-tests what is on screen.
    script.set_link_spans(link_spans);

    // Park the slot for `minimap::emit_minimap`; `None` when the minimap is hidden.
    parked_minimap.0 = minimap_slot;
    drop(extract_span);
    let us_exa = lap();

    let _span = bevy::log::info_span!("ui_script: diff").entered();
    let n_quads = out.len();
    let changed = quads.quads != out;
    if changed {
        if ui_diff_enabled() {
            match quads.quads.iter().zip(&out).position(|(a, b)| a != b) {
                Some(i) => {
                    let (b, a) = (&quads.quads[i], &out[i]);
                    eprintln!(
                        "[ui-diff-base] quad {i}/{n_quads}: tex={:?} rect {:?} -> {:?} uv_changed={} color_changed={}",
                        a.texture.as_ref().and_then(|t| t.path()),
                        b.rect,
                        a.rect,
                        a.uv != b.uv,
                        a.color != b.color,
                    );
                }
                None => eprintln!(
                    "[ui-diff-base] quad COUNT changed: {} -> {}",
                    quads.quads.len(),
                    out.len()
                ),
            }
        }
        quads.quads = out;
        quads.dirty = true;
    }
    if cost_on {
        let us_diff = lap();
        let solves = script.layout_solves() - solves_before.unwrap_or(0);
        *ui_cost = super::UiFrameCost {
            measured: ui_cost.measured,
            measured_texts: std::mem::take(&mut ui_cost.measured_texts),
            tick: us_tick,
            resolve: us_resolve,
            measure: us_measure,
            extract: us_exm,
            convert: us_exa,
            diff: us_diff,
            quads: n_quads,
            solves,
            derives: script.layout_derivations() - derives_before.unwrap_or(0),
            skipped: false,
            spliced: 0,
            dropped: 0,
        };
        if printing {
            eprintln!(
                "[ui-cost] tick={us_tick} resolve={us_resolve} measure={us_measure} \
                 exm={us_exm} exa={us_exa} diff={us_diff} eq={n_extracted} quads={n_quads} \
                 solves={solves} changed={} skip=0",
                u8::from(changed)
            );
        }
    }
}

/// One extracted entry to its screen quads, plus the side channels some arms write: link spans,
/// the minimap slot, booth pane aspects and tile requests. The full pass and the splice both call
/// this; keep [`splice_simple`] in step with each arm's side effects.
fn convert_entry(
    eq: benilla_ui::script::ExtractedQuad,
    s: f32,
    // The window in logical px: `h` flips y, `w` gives model tiles the screen diagonal.
    w: f32,
    h: f32,
    // The device scale, for the nameplate border resampled to physical pixels.
    dpi: f32,
    assets: &mut Option<ResMut<WorldAssets>>,
    images: &mut Assets<Image>,
    font_atlas: &mut Option<ResMut<UiFontAtlas>>,
    booths: &mut crate::portrait::BoothBridge,
    text_ui: Option<&benilla_ui::script::EditBoxTextUi>,
    caret_pinned: bool,
    out: &mut Vec<UiQuad>,
    link_spans: &mut Vec<(
        benilla_ui::widget::FrameHandle,
        benilla_ui::layout::Rect,
        String,
        String,
    )>,
    minimap_slot: &mut Option<crate::minimap::MinimapSlot>,
) {
    let Some(r) = eq.rect else { return };
    // WoW UI space is y-up from the bottom-left: scale ×s, then flip through the window height.
    let rect = Rect::new(r.left * s, h - r.top * s, r.right * s, h - r.bottom * s);
    if let Some((at, r)) = ui_pick_point() {
        report_ui_pick(&eq, rect, at, r);
    }
    // The ScrollFrame clip, converted like `rect`.
    let clip = eq
        .clip
        .map(|c| Rect::new(c.left * s, h - c.top * s, c.right * s, h - c.bottom * s));
    match eq.content {
        // A frame draws nothing itself; its regions carry the visuals.
        QuadContent::Frame => {}
        // The `<Minimap>` content hole, parked for the minimap renderer to fill at this `z`.
        QuadContent::Minimap { zoom, inside_zoom } => {
            *minimap_slot = Some(crate::minimap::MinimapSlot {
                rect,
                z: eq.z,
                zoom,
                inside_zoom,
                alpha: eq.alpha,
            });
        }
        // A `<Model>`/`<PlayerModel>` pane draws itself, as the stock files declare a bare pane
        // with no Texture: a file pane is a model tile, a unit pane samples its window's body
        // bake square ([`crate::portrait::model_pane_booth`]), and an unclaimed pane draws nothing.
        QuadContent::ModelPane {
            handle,
            name,
            model,
            facing,
            model_scale,
            position,
            own_alpha,
            icon,
            camera,
            light,
            fog,
        } => {
            use crate::portrait::PortraitSource;
            // A file pane publishes an idempotent tile request; `ui_models::compose_tiles` draws
            // its quad each frame, so a cell packed later needs no re-conversion.
            if let Some(path) = model.as_deref() {
                let tiles = &mut booths.tiles;
                let dpi = tiles.dpi.max(0.01);
                let aspect = if h > 0.0 { w / h } else { 4.0 / 3.0 };
                let diag = (aspect * aspect + 1.0).sqrt();
                let layout = eq.scale;
                let size_px = UVec2::new(
                    (rect.width() * dpi).round().max(1.0) as u32,
                    (rect.height() * dpi).round().max(1.0) as u32,
                );
                tiles.requests.insert(
                    handle,
                    crate::ui_models::TileRequest {
                        path: path.to_string(),
                        size_px,
                        // 1 model unit is `1280 · modelScale · layoutScale` FrameXML units
                        // (`0x76d1a0`).
                        px_per_unit: 1280.0 * model_scale * layout * s * dpi,
                        // `SetPosition` is in layout units: `768 · √(a²+1)` FrameXML per unit.
                        pos_px_per_unit: 768.0 * diag * layout * s * dpi,
                        // A particle's half-extent: eye space, neither scale (`0x7b2a50`).
                        star_px_per_unit: 768.0 * diag * s * dpi,
                        facing,
                        position: Vec3::new(position.0, position.1, position.2),
                        // The perspective root, in model units: translate `pos · layoutScale`,
                        // rotate `facing`, scale `G48 · (5/3) · modelScale · layoutScale`, where
                        // `G48 · (5/3)` is the 4:3 renormalizer `√((4/3)²+1)/√(a²+1)`
                        // (`0x80655c`); the near and far planes do not scale with it.
                        root_scale: (5.0 / 3.0) / diag * model_scale * layout,
                        root_pos: Vec3::new(position.0, position.1, position.2) * layout,
                        camera,
                        light,
                        fog,
                        icon: icon.clone(),
                        rect,
                        z_key: eq.z,
                        // The instance draws at the widget's own alpha (`0x76d120`).
                        alpha: own_alpha,
                        clip,
                    },
                );
                if crate::ui_models::trace_on() {
                    info!(
                        "tile-trace: composite {path} pane {handle:?} {} rect=({:.1},{:.1})-({:.1},{:.1}) size_px={}x{} z={:?} alpha={own_alpha:.2} cell={:?} atlas={:?}",
                        name.as_deref().unwrap_or("<anonymous>"),
                        rect.min.x,
                        rect.min.y,
                        rect.max.x,
                        rect.max.y,
                        size_px.x,
                        size_px.y,
                        eq.z,
                        tiles.cells.get(&handle),
                        tiles.atlas_size
                    );
                }
                return;
            }
            let Some(slot) = name.as_deref().and_then(crate::portrait::model_pane_booth) else {
                return;
            };
            // The aspect is published before the readiness check: the bake waits on the publish.
            if rect.height() > 0.0 {
                booths
                    .panes
                    .0
                    .insert(slot.to_string(), rect.width() / rect.height());
            }
            // The bake is premultiplied; the 2D stand-in while a model streams is straight alpha.
            let (handle, premultiplied) = match booths.images.0.get(slot) {
                Some(PortraitSource::Live(h)) => (Some(h.clone()), true),
                Some(PortraitSource::File(p)) => (
                    assets.as_mut().and_then(|a| a.sprite_texture(p, images)),
                    false,
                ),
                None => (None, false),
            };
            let Some(handle) = handle else {
                return;
            };
            out.push(UiQuad {
                rect,
                z_key: eq.z,
                texture: Some(handle),
                color: [1.0, 1.0, 1.0, eq.alpha],
                premultiplied,
                clip,
                ..default()
            });
        }
        QuadContent::Texture {
            path,
            color,
            additive,
            tex_coords,
            circular,
            portrait_unit,
            rotation,
            desaturated,
        } => {
            // A live unit portrait (`SetPortraitTexture(region, unit)`): the model bake, or the
            // reference's 2D TemporaryPortrait while the model streams; no entry yet draws nothing.
            if let Some(token) = &portrait_unit {
                use crate::portrait::PortraitSource;
                // The aspect, published before the readiness check as in the model-pane arm; a
                // round slot's row only marks it drawn, which gates the `"targettarget"` cost.
                if rect.height() > 0.0 {
                    booths
                        .panes
                        .0
                        .insert(token.clone(), rect.width() / rect.height());
                }
                // The bake is premultiplied, the stand-in straight alpha; unflagged, the pass
                // would premultiply the bake twice and erase what it draws over empty space.
                let (handle, premultiplied) = match booths.images.0.get(token) {
                    Some(PortraitSource::Live(h)) => (Some(h.clone()), true),
                    Some(PortraitSource::File(p)) => (
                        assets.as_mut().and_then(|a| a.sprite_texture(p, images)),
                        false,
                    ),
                    None => (None, false),
                };
                let Some(handle) = handle else {
                    return;
                };
                out.push(UiQuad {
                    rect,
                    z_key: eq.z,
                    texture: Some(handle),
                    // A portrait binding honours `SetTexCoord`: the character micro button crops
                    // the player portrait (`MainMenuBarMicroButtons.lua:110,115`).
                    uv: match tex_coords {
                        Some(TexCoords::Rect(edges)) => UvRect::from_tex_coords(edges),
                        Some(TexCoords::Corners(corners)) => UvRect::from_corners(corners),
                        None => UvRect::FULL,
                    },
                    color: [1.0, 1.0, 1.0, eq.alpha],
                    // `SetPortraitTexture` cuts the inscribed circle, as the reference stamps into
                    // its 64² bake's alpha; `BenillaSetBoothTexture` samples square.
                    circular,
                    premultiplied,
                    clip,
                    ..default()
                });
                return;
            }
            // An unset texture region (no file, no colour, as a cleared icon) draws nothing.
            if path.is_none() && color.is_none() {
                return;
            }
            // A path the archives lack draws nothing. The reference keeps the widget's previous
            // texture and returns nil to Lua (`CSimpleTexture::SetTexture`, `0x770200`); a path
            // here is re-resolved every frame, so the cell is empty. With no `WorldAssets` at all
            // (a data-less run) the quad draws untextured.
            //
            // `SetTexCoord`'s edges map to raw UV corners, so a mirrored slice (`left > right`)
            // keeps its flip; resolved before the handle because the wrap mode follows it.
            let uv = match tex_coords {
                Some(TexCoords::Rect(edges)) => UvRect::from_tex_coords(edges),
                Some(TexCoords::Corners(corners)) => UvRect::from_corners(corners),
                None => UvRect::FULL,
            };
            // A slice past the texture tiles (`BonusActionBarFrame.lua:173`), on that axis alone.
            let wrap = tiling_axes(&uv);
            let handle = match (path.as_deref(), assets.as_mut()) {
                (Some(p), Some(a)) => {
                    let resolved = if p == benilla_ui::script::nameplate::BORDER_TEXTURE {
                        // Where the frame system painted the plate, beside the driver's `vpl` line.
                        if benilla_assets::trace::enabled_for("vpl") {
                            benilla_assets::trace::line(
                                "vpl",
                                &format!(
                                    "paint=({:.1},{:.1})..({:.1},{:.1})",
                                    rect.min.x, rect.min.y, rect.max.x, rect.max.y
                                ),
                            );
                        }
                        // Deviation: the V-plate border is resampled sharp to the quad's physical
                        // size rather than GPU-magnified, so its 1 px bevel stays crisp; keyed by
                        // that size, so only a resize re-rasterises it.
                        let px = |v: f32| (v * dpi).round().max(1.0) as u32;
                        a.resampled_sprite(
                            p,
                            (px(rect.width()), px(rect.height())),
                            images,
                            crate::vplates::border::resample_sharp,
                        )
                    } else if let Some(blp) = benilla_ui::script::emblem_mask_path(p) {
                        // A tabard emblem cell: the reference installs a generated 128×64 or
                        // 128×32 image, white carrying the emblem BLP's alpha.
                        a.emblem_mask_texture(blp, images)
                    } else if circular {
                        // The circle-masked variant (`SetPortraitToTexture`).
                        a.portrait_texture(p, images)
                    } else if wrap != (false, false) {
                        a.sprite_texture_wrapped(p, wrap, images)
                    } else {
                        a.sprite_texture(p, images)
                    };
                    if resolved.is_none() {
                        return;
                    }
                    resolved
                }
                _ => None,
            };
            // A crop's magnified edge would filter in the neighbouring atlas cell.
            let uv_clamp = handle
                .as_ref()
                .and_then(|h| images.get(h))
                .map(|img| img.texture_descriptor.size)
                .and_then(|sz| uv_clamp_window(&uv, (sz.width, sz.height)));
            // A pathless Texture region is a solid color; a textured one tints by it.
            let mut color = color.unwrap_or([1.0, 1.0, 1.0, 1.0]);
            color[3] *= eq.alpha;
            out.push(UiQuad {
                rect,
                z_key: eq.z,
                texture: handle,
                uv,
                uv_clamp,
                color,
                additive,
                clip,
                // `SetRotation` is counter-clockwise-positive, `UiQuad::rotation` clockwise.
                rotation: -rotation,
                desaturated,
                ..default()
            });
        }
        // The colour picker's hue disc, a generated image drawn at full value ([`colorselect`]).
        QuadContent::ColorWheel => {
            let Some(assets) = assets.as_mut() else {
                return;
            };
            let handle =
                assets.generated_sprite(colorselect::WHEEL_KEY, images, colorselect::wheel_pixels);
            out.push(UiQuad {
                rect,
                z_key: eq.z,
                texture: Some(handle),
                uv: UvRect::FULL,
                color: colorselect::wheel_tint(eq.alpha),
                clip,
                ..default()
            });
        }
        // Its brightness strip: the grey ramp tinted `rgb(h, s, 1)`.
        QuadContent::ColorValue { hue, sat } => {
            let Some(assets) = assets.as_mut() else {
                return;
            };
            let handle =
                assets.generated_sprite(colorselect::RAMP_KEY, images, colorselect::ramp_pixels);
            out.push(UiQuad {
                rect,
                z_key: eq.z,
                texture: Some(handle),
                uv: UvRect::FULL,
                color: colorselect::ramp_tint(hue, sat, eq.alpha),
                clip,
                ..default()
            });
        }
        QuadContent::Backdrop {
            path,
            color,
            uvs,
            tile,
        } => {
            // A backdrop piece, the bg or one of the 8 border pieces, sampled `Repeat` when `tile`;
            // its UVs are per-corner, as the TOP and BOTTOM edges are rotated.
            let handle = assets.as_mut().and_then(|a| {
                if tile {
                    a.sprite_texture_tiled(&path, images)
                } else {
                    a.sprite_texture(&path, images)
                }
            });
            // A missing BLP draws nothing.
            let Some(handle) = handle else { return };
            // The 8 border pieces share one atlas, so a `tile` piece is pulled half a texel in on
            // its bounded axes; a stretched bg is a whole texture.
            let uvs = match images.get(&handle) {
                Some(img) if tile => {
                    let sz = img.texture_descriptor.size;
                    benilla_ui::script::inset_atlas_bleed(uvs, sz.width as f32, sz.height as f32)
                }
                _ => uvs,
            };
            let mut color = color;
            color[3] *= eq.alpha;
            out.push(UiQuad {
                rect,
                z_key: eq.z,
                texture: Some(handle),
                uv: UvRect::from_corners(uvs),
                color,
                clip,
                ..default()
            });
        }
        QuadContent::Text {
            text,
            color,
            justify_h,
            justify_v,
            font,
            font_height,
            text_height,
            shadow,
            outline,
            alpha_gradient,
            world_seat,
        } => {
            // No atlas (no client data, or an unreadable font) draws no text.
            let (Some(atlas), Some(text)) = (font_atlas.as_deref_mut(), text) else {
                return;
            };
            text::emit(
                atlas,
                &text,
                text::TextStyle {
                    color,
                    justify_h,
                    justify_v,
                    font,
                    font_height,
                    text_height,
                    shadow,
                    outline,
                    alpha_gradient,
                    world_seat,
                },
                text::TextHost {
                    z: eq.z,
                    alpha: eq.alpha,
                    target: eq.target,
                    rect,
                    clip,
                    ebox: text_ui,
                    screen_h: h,
                    scale: s,
                    font_scale: eq.scale,
                    caret_pinned,
                },
                out,
                link_spans,
            );
        }
    }
}

/// One pass of the FontString measure round-trip; true when anything was measured.
fn measure_fontstrings(
    script: &mut UiScript,
    atlas: &mut UiFontAtlas,
    s: f32,
    ui_cost: &mut super::UiFrameCost,
    // The hover recorder's churn column; the `[ui-cost]` line reports only counts.
    record_texts: bool,
) -> bool {
    let requests = script.fontstrings_needing_measure();
    // The first few strings a frame re-shaped, by name.
    if record_texts {
        ui_cost.measured += requests.len();
        ui_cost.measured_texts.extend(
            requests
                .iter()
                .take(3)
                .map(|r| r.text.chars().take(40).collect::<String>()),
        );
    }
    if requests.is_empty() {
        return false;
    }
    // The VM's synchronous measurer's own body and engine ([`crate::ui_text::measure_request`]).
    let measures: Vec<(u32, f32, f32, f32, u64)> = requests
        .iter()
        .map(|r| {
            let (w, h, natural) = crate::ui_text::measure_request(&mut atlas.lock(), s, r);
            (r.id, w, h, natural, r.key)
        })
        .collect();
    script.set_measured_text(&measures);
    true
}

/// The held-cursor icon for captures: 32×32 hanging down-right from the mouse, the reference's
/// look with the hardware cursor's `(0, 0)` hotspot, above the whole UI.
fn cursor_icon_quad(pos: Vec2, texture: Handle<Image>) -> UiQuad {
    const SIZE: f32 = 32.0;
    UiQuad {
        rect: Rect::new(pos.x, pos.y, pos.x + SIZE, pos.y + SIZE),
        z_key: u64::MAX,
        texture: Some(texture),
        ..default()
    }
}

/// [`uv_clamp_window`] and [`tiling_axes`] on shapes the stock UI draws.
#[cfg(test)]
mod uv_clamp_tests {
    use super::{tiling_axes, uv_clamp_window, UvRect};

    /// `POIIcons` is 128², and a world-map POI samples its cell (7, 1).
    #[test]
    fn a_poi_icons_cell_stops_half_a_texel_inside_itself() {
        let uv = UvRect::from_tex_coords([0.875, 1.0, 0.125, 0.25]);
        let w = uv_clamp_window(&uv, (128, 128)).expect("an atlas cell is clamped");
        let half = 0.5 / 128.0;
        assert!((w[0] - (0.875 + half)).abs() < 1e-6, "u_min {}", w[0]);
        assert!((w[1] - (0.125 + half)).abs() < 1e-6, "v_min {}", w[1]);
        assert!((w[2] - (1.0 - half)).abs() < 1e-6, "u_max {}", w[2]);
        assert!((w[3] - (0.25 - half)).abs() < 1e-6, "v_max {}", w[3]);
    }

    #[test]
    fn the_whole_texture_asks_for_no_window() {
        assert!(uv_clamp_window(&UvRect::FULL, (128, 128)).is_none());
    }

    /// The stance bar's middle strip, `SetTexCoord(0, n-2, 0, 1)` (`BonusActionBarFrame.lua:173`).
    #[test]
    fn a_one_axis_strip_wraps_that_axis_only() {
        assert_eq!(
            tiling_axes(&UvRect::from_tex_coords([0.0, 2.0, 0.0, 1.0])),
            (true, false)
        );
        assert_eq!(
            tiling_axes(&UvRect::from_tex_coords([0.0, 1.0, 0.0, 3.0])),
            (false, true)
        );
    }

    #[test]
    fn a_bounded_mapping_tiles_nothing() {
        assert_eq!(
            tiling_axes(&UvRect::from_tex_coords([0.0, 1.0, 0.0, 1.0])),
            (false, false)
        );
        assert_eq!(tiling_axes(&UvRect::FULL), (false, false));
        assert_eq!(
            tiling_axes(&UvRect::from_tex_coords([0.453125, 0.875, 0.0, 1.0])),
            (false, false)
        );
    }

    #[test]
    fn a_two_axis_tile_wraps_both() {
        assert_eq!(
            tiling_axes(&UvRect::from_tex_coords([0.0, 4.0, 0.0, 2.5])),
            (true, true)
        );
    }

    #[test]
    fn a_tiling_axis_is_left_alone_while_its_bounded_partner_is_not() {
        let uv = UvRect::from_tex_coords([0.0, 3.0, 0.0, 0.5]);
        let w = uv_clamp_window(&uv, (64, 64)).expect("the v axis is a bounded crop");
        assert!(w[0] > w[2], "u tiles, so its axis must read as OFF: {w:?}");
        assert!((w[1] - 0.5 / 64.0).abs() < 1e-6);
        assert!((w[3] - (0.5 - 0.5 / 64.0)).abs() < 1e-6);
    }

    #[test]
    fn a_mirrored_slice_is_clamped_by_its_extents() {
        let uv = UvRect::from_tex_coords([0.5, 0.25, 0.0, 1.0]);
        let w = uv_clamp_window(&uv, (64, 64)).expect("a mirrored cell is still a cell");
        assert!((w[0] - (0.25 + 0.5 / 64.0)).abs() < 1e-6);
        assert!((w[2] - (0.5 - 0.5 / 64.0)).abs() < 1e-6);
        assert!(
            w[1] > w[3],
            "v spans the whole texture, so it reads as OFF: {w:?}"
        );
    }

    #[test]
    fn a_sub_texel_crop_pins_to_the_texel_it_names() {
        let uv = UvRect::from_tex_coords([0.5, 0.505, 0.0, 1.0]);
        let w = uv_clamp_window(&uv, (64, 64)).expect("still a crop");
        assert!((w[0] - 0.5025).abs() < 1e-6);
        assert!(w[0] <= w[2], "the axis must stay enabled: {w:?}");
        assert!((w[2] - 0.5025).abs() < 1e-6);
    }
}

#[cfg(test)]
mod cursor_quad_tests {
    use super::cursor_icon_quad;
    use bevy::prelude::*;

    #[test]
    fn cursor_icon_quad_is_32px_top_left_anchored_at_the_hotspot() {
        let q = cursor_icon_quad(Vec2::new(100.0, 200.0), Handle::default());
        assert_eq!(
            (q.rect.min.x, q.rect.min.y, q.rect.max.x, q.rect.max.y),
            (100.0, 200.0, 132.0, 232.0)
        );
        assert_eq!(q.rect.width(), 32.0);
        assert_eq!(q.rect.height(), 32.0);
        assert_eq!(q.z_key, u64::MAX);
        assert!(q.texture.is_some());
        assert!(!q.additive);
    }
}

/// A ScrollFrame clip through the real UI pass systems in a headless `App`, y flip included.
#[cfg(test)]
mod clip_plumb_tests {
    use bevy::math::Rect;
    use bevy::prelude::*;
    use bevy::window::PrimaryWindow;

    use benilla_ui::script::UiScript;

    use super::{paint_script, tick_script, UiQuad, UiQuads};
    use crate::portrait::PortraitImages;

    /// A ScrollFrame with a coloured marker in its scroll child, on a 1024×768 window (`s = 1`).
    fn app_with_scrolled_marker() -> App {
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
            local frame = CreateFrame("ScrollFrame", "SF")
            frame:SetPoint("TOPLEFT", 0, -100)  -- screen top 768 -> frame top 668
            frame:SetWidth(300); frame:SetHeight(200)  -- frame bottom 300, right 300
            local child = CreateFrame("Frame", "Child")
            child:SetWidth(300); child:SetHeight(600)
            frame:SetScrollChild(child)
            local marker = child:CreateTexture(nil, "ARTWORK")
            marker:SetTexture(1, 0, 0)          -- pathless colored quad: no BLP asset needed
            marker:SetAllPoints()               -- templateless Lua region: no implicit anchor
        "#,
            )
            .unwrap();

        let mut app = App::new();
        app.insert_non_send_resource(script);
        app.init_resource::<UiQuads>();
        app.init_resource::<crate::ui_script::UiPassState>();
        app.init_resource::<Assets<Image>>();
        app.init_resource::<PortraitImages>();
        app.init_resource::<crate::portrait::BoothPanes>();
        app.init_resource::<crate::ui_models::UiModelTiles>();
        app.init_resource::<crate::minimap::MinimapWidget>();
        app.init_resource::<crate::ui_script::UiFrameCost>();
        app.init_resource::<crate::ui_script::UiCostWanted>();
        app.init_resource::<Time>();
        app.init_resource::<Time<Real>>();
        app.init_resource::<crate::ui_script::UiClock>();
        // The uiScale dial at 1.0, the identity.
        app.init_resource::<crate::ui_script::UiScaleCvar>();
        app.world_mut().spawn((
            Window {
                resolution: UVec2::new(1024, 768).into(),
                ..default()
            },
            PrimaryWindow,
        ));
        // Both halves of the pass: the quads are the paint half's.
        app.add_systems(Update, (tick_script, paint_script).chain());
        app
    }

    #[test]
    fn a_clipped_texture_quad_carries_its_clip_into_the_uiquad() {
        let mut app = app_with_scrolled_marker();
        app.update();

        let quads = &app.world().resource::<UiQuads>().quads;
        let marker: &UiQuad = quads
            .iter()
            .find(|q| q.color[0] == 1.0 && q.color[1] == 0.0 && q.color[2] == 0.0)
            .expect("the colored marker quad extracted");

        // WoW space: bottom 468, left 0, top 668, right 300; y-down through the 768 window, the
        // top is 100 and the bottom 300.
        assert_eq!(
            marker.clip,
            Some(Rect::new(0.0, 100.0, 300.0, 300.0)),
            "the engine's ScrollFrame clip survives the y-up→y-down flip into UiQuad::clip"
        );
    }

    /// At dial 0.5 the VM sees a 2048×1536 screen, and every window-px rect halves.
    #[test]
    fn the_uiscale_dial_scales_the_extracted_rects() {
        let mut app = app_with_scrolled_marker();
        app.insert_resource(crate::ui_script::UiScaleCvar(0.5));
        app.update();

        let quads = &app.world().resource::<UiQuads>().quads;
        let marker: &UiQuad = quads
            .iter()
            .find(|q| q.color[0] == 1.0 && q.color[1] == 0.0 && q.color[2] == 0.0)
            .expect("the colored marker quad extracted");

        // WoW space: top 1536 − 100 = 1436, bottom 1236, right 300; ×0.5 is y-up px 618..718,
        // and y-down through the 768 window the top is 50 and the bottom 150.
        assert_eq!(
            marker.clip,
            Some(Rect::new(0.0, 50.0, 150.0, 150.0)),
            "dial 0.5 halves the px rect and re-hangs the frame from the taller virtual top"
        );
    }

    #[test]
    fn an_unclipped_texture_quad_carries_no_clip() {
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
            local plain = CreateFrame("Frame", "Plain")
            plain:SetPoint("TOPLEFT", 0, 0)
            plain:SetWidth(50); plain:SetHeight(50)
            local m = plain:CreateTexture(nil, "ARTWORK")
            m:SetTexture(0, 1, 0)
            m:SetAllPoints()  -- templateless Lua region: no implicit anchor
        "#,
            )
            .unwrap();

        let mut app = App::new();
        app.insert_non_send_resource(script);
        app.init_resource::<UiQuads>();
        app.init_resource::<crate::ui_script::UiPassState>();
        app.init_resource::<Assets<Image>>();
        app.init_resource::<PortraitImages>();
        app.init_resource::<crate::portrait::BoothPanes>();
        app.init_resource::<crate::ui_models::UiModelTiles>();
        app.init_resource::<crate::minimap::MinimapWidget>();
        app.init_resource::<crate::ui_script::UiFrameCost>();
        app.init_resource::<crate::ui_script::UiCostWanted>();
        app.init_resource::<Time>();
        app.init_resource::<Time<Real>>();
        app.init_resource::<crate::ui_script::UiClock>();
        app.init_resource::<crate::ui_script::UiScaleCvar>();
        app.world_mut().spawn((
            Window {
                resolution: UVec2::new(1024, 768).into(),
                ..default()
            },
            PrimaryWindow,
        ));
        // Both halves of the pass: the quads are the paint half's.
        app.add_systems(Update, (tick_script, paint_script).chain());
        app.update();

        let quads = &app.world().resource::<UiQuads>().quads;
        let marker = quads
            .iter()
            .find(|q| q.color[0] == 0.0 && q.color[1] == 1.0 && q.color[2] == 0.0)
            .expect("the green marker quad extracted");
        assert_eq!(marker.clip, None);
    }
}

/// [`align_entries`] on a real extract list: `FrameHandle`'s fields are private, and a forged
/// [`benilla_ui::order::ZKey`] would test a fiction.
#[cfg(test)]
mod align_tests {
    use benilla_ui::script::{ExtractedQuad, QuadContent, UiScript};

    use super::align_entries;

    /// Four regions across two frames: a z-sorted list with genuine keys.
    fn entries() -> Vec<ExtractedQuad> {
        let mut script = UiScript::new().unwrap();
        script
            .run(
                r#"
            for i = 1, 2 do
              local f = CreateFrame("Frame", "F" .. i)
              f:SetPoint("TOPLEFT", 0, 0)
              f:SetWidth(50); f:SetHeight(50)
              for j = 1, 2 do
                local t = f:CreateTexture(nil, "ARTWORK")
                t:SetTexture(i / 4, j / 4, 0)
                t:SetAllPoints()
              end
            end
        "#,
            )
            .unwrap();
        script.resolve();
        let list = script.extract();
        assert!(
            list.len() >= 6,
            "two frames and four regions: {}",
            list.len()
        );
        list
    }

    #[test]
    fn a_changed_entry_is_the_only_one_that_converts() {
        let was = entries();
        let mut now = was.clone();
        let at = now
            .iter()
            .position(|e| matches!(e.content, QuadContent::Texture { .. }))
            .unwrap();
        let QuadContent::Texture { color, .. } = &mut now[at].content else {
            unreachable!()
        };
        *color = Some([0.0, 0.0, 1.0, 1.0]);

        let a = align_entries(&was, &now);
        assert_eq!(a.dropped, vec![at], "the repainted entry's old quads go");
        for (j, s) in a.source.iter().enumerate() {
            assert_eq!(
                *s,
                (j != at).then_some(j),
                "entry {j} reuses its own quads unless it is the one that changed"
            );
        }
    }

    #[test]
    fn an_insertion_shifts_the_tail_and_costs_one_conversion() {
        let was = entries();
        // The new `z` goes strictly between its neighbours', at the first pair with room:
        // sibling regions differ by 1, a frame boundary leaves a wide gap.
        let at = (1..was.len())
            .find(|&i| was[i].z - was[i - 1].z > 1)
            .expect("some adjacent pair has room between its keys");
        let mut now = was.clone();
        let mut fresh = was[at].clone();
        fresh.z = was[at - 1].z + 1;
        now.insert(at, fresh);

        let a = align_entries(&was, &now);
        assert!(a.dropped.is_empty(), "nothing left the list");
        assert_eq!(a.source[at], None, "the new entry is the one conversion");
        for j in 0..at {
            assert_eq!(a.source[j], Some(j), "the head is untouched");
        }
        for j in at + 1..now.len() {
            assert_eq!(
                a.source[j],
                Some(j - 1),
                "entry {j} keeps the quads it had at {}, one place back",
                j - 1
            );
        }
    }

    #[test]
    fn a_deletion_converts_nothing_and_still_has_work() {
        let was = entries();
        let at = was.len() / 2;
        let mut now = was.clone();
        now.remove(at);

        let a = align_entries(&was, &now);
        assert_eq!(
            a.dropped,
            vec![at],
            "the vanished entry's quads are dropped"
        );
        assert!(
            a.source.iter().all(Option::is_some),
            "a deletion converts nothing"
        );
        for j in at..now.len() {
            assert_eq!(a.source[j], Some(j + 1), "the tail shifts back one");
        }
    }

    #[test]
    fn a_wholly_new_list_keeps_nothing() {
        let was = entries();
        let now: Vec<ExtractedQuad> = was
            .iter()
            .cloned()
            .map(|mut e| {
                e.z += 1;
                e
            })
            .collect();
        let a = align_entries(&was, &now);
        assert!(a.source.iter().all(Option::is_none));
        assert_eq!(a.dropped.len(), was.len());
    }
}

/// A settled frame skips the conversion, and any extract-visible change reopens it, paint-only
/// writes included.
#[cfg(test)]
mod extract_gate_tests {
    use bevy::prelude::*;
    use bevy::window::PrimaryWindow;

    use benilla_ui::script::UiScript;

    use super::{paint_script, tick_script, UiQuads};
    use crate::portrait::PortraitImages;

    fn app_with_marker() -> App {
        app_from_script(
            r#"
            local plain = CreateFrame("Frame", "Plain")
            plain:SetPoint("TOPLEFT", 0, 0)
            plain:SetWidth(50); plain:SetHeight(50)
            marker = plain:CreateTexture(nil, "ARTWORK")
            marker:SetTexture(1, 0, 0)
            marker:SetAllPoints()  -- templateless Lua region: no implicit anchor
        "#,
        )
    }

    /// The headless app around a boot script; one booted into another's end state converts in
    /// full on its first frame, the result a splice must equal.
    fn app_from_script(lua: &str) -> App {
        let script = UiScript::new().unwrap();
        script.run(lua).unwrap();
        let mut app = App::new();
        app.insert_non_send_resource(script);
        app.init_resource::<UiQuads>();
        app.init_resource::<crate::ui_script::UiPassState>();
        app.init_resource::<Assets<Image>>();
        app.init_resource::<PortraitImages>();
        app.init_resource::<crate::portrait::BoothPanes>();
        app.init_resource::<crate::ui_models::UiModelTiles>();
        app.init_resource::<crate::minimap::MinimapWidget>();
        app.init_resource::<crate::ui_script::UiFrameCost>();
        app.init_resource::<crate::ui_script::UiCostWanted>();
        app.init_resource::<Time>();
        app.init_resource::<Time<Real>>();
        app.init_resource::<crate::ui_script::UiClock>();
        app.init_resource::<crate::ui_script::UiScaleCvar>();
        app.world_mut().spawn((
            Window {
                resolution: UVec2::new(1024, 768).into(),
                ..default()
            },
            PrimaryWindow,
        ));
        // Both halves of the pass: the quads are the paint half's.
        app.add_systems(Update, (tick_script, paint_script).chain());
        app
    }

    #[test]
    fn a_settled_frame_skips_and_a_paint_write_reopens_the_gate() {
        let mut app = app_with_marker();
        app.update(); // frame 1: builds the quads
        assert!(app.world().resource::<UiQuads>().dirty, "first frame draws");
        app.world_mut().resource_mut::<UiQuads>().dirty = false;

        app.update(); // frame 2: identical inputs
        assert!(
            !app.world().resource::<UiQuads>().dirty,
            "a settled frame must not re-mark the quads dirty"
        );

        // A paint-only write, invisible to the layout gate: only the extracted-list compare can
        // reopen extraction.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("marker:SetTexture(0, 0, 1)")
            .unwrap();
        app.update();
        let quads = app.world().resource::<UiQuads>();
        assert!(quads.dirty, "a paint write must reopen the extract gate");
        assert!(
            quads
                .quads
                .iter()
                .any(|q| q.color[0] == 0.0 && q.color[2] == 1.0),
            "and the produced quads carry the new color"
        );
    }

    #[test]
    fn a_layout_write_reopens_the_gate_too() {
        let mut app = app_with_marker();
        app.update();
        app.update();
        app.world_mut().resource_mut::<UiQuads>().dirty = false;

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("getglobal('Plain'):SetPoint('TOPLEFT', 40, -40)")
            .unwrap();
        app.update();
        assert!(
            app.world().resource::<UiQuads>().dirty,
            "a moved frame must re-extract"
        );
    }

    /// `PaperDollFrame.xml:204` declares a bare `<PlayerModel name="CharacterModelFrame">`; a pane
    /// no window claims ([`crate::portrait::model_pane_booth`]) must not borrow a bake.
    #[test]
    fn a_named_model_pane_samples_its_booth_and_an_unclaimed_one_draws_nothing() {
        let mut app = app_from_script(
            r#"
            local doll = CreateFrame("PlayerModel", "CharacterModelFrame")
            doll:SetPoint("TOPLEFT", 0, 0)
            doll:SetWidth(233); doll:SetHeight(224)
            doll:SetUnit("player")
            local stray = CreateFrame("PlayerModel", "SomeAddonsModelPane")
            stray:SetPoint("TOPLEFT", 300, 0)
            stray:SetWidth(233); stray:SetHeight(224)
            stray:SetUnit("player")
        "#,
        );
        let bake = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        app.world_mut().resource_mut::<PortraitImages>().0.insert(
            "paperdoll".to_string(),
            crate::portrait::PortraitSource::Live(bake.clone()),
        );
        app.update();

        let quads = &app.world().resource::<UiQuads>().quads;
        let panes: Vec<_> = quads
            .iter()
            .filter(|q| q.texture.as_ref() == Some(&bake))
            .collect();
        assert_eq!(
            panes.len(),
            1,
            "exactly the claimed pane draws the paper doll's bake"
        );
        assert!(
            panes[0].premultiplied,
            "a render-target bake carries premultiplied colour"
        );
        let aspect = app
            .world()
            .resource::<crate::portrait::BoothPanes>()
            .0
            .get("paperdoll")
            .copied()
            .expect("the pane publishes its aspect");
        assert!(
            (aspect - 233.0 / 224.0).abs() < 0.01,
            "the pane's own rect is the aspect, got {aspect}"
        );
    }

    /// The shader's `select(a, k, premultiplied)` is right only if exactly the render-target quads
    /// carry the flag; unflagged, an additive effect over empty space is lost.
    #[test]
    fn a_booth_bake_quad_is_flagged_premultiplied_and_a_plain_one_is_not() {
        let mut app = app_with_marker();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("BenillaSetBoothTexture(marker, 'paperdoll')")
            .unwrap();
        // The booth publishes a live bake for that slot; without an entry the region draws nothing.
        let bake = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        app.world_mut().resource_mut::<PortraitImages>().0.insert(
            "paperdoll".to_string(),
            crate::portrait::PortraitSource::Live(bake.clone()),
        );
        app.update();

        let quads = &app.world().resource::<UiQuads>().quads;
        let pane = quads
            .iter()
            .find(|q| q.texture.as_ref() == Some(&bake))
            .expect("the booth pane's quad reached the pass");
        assert!(
            pane.premultiplied,
            "a render-target bake carries premultiplied colour — the pass must not re-weight it"
        );
        assert!(
            quads
                .iter()
                .filter(|q| q.texture.as_ref() != Some(&bake))
                .all(|q| !q.premultiplied),
            "every straight-alpha region stays unflagged — the flag is the booth's alone"
        );
    }

    /// `spliced == 1` proves the splice fired; the result must equal a fresh app's full conversion.
    #[test]
    fn a_one_entry_paint_write_splices_and_matches_the_full_conversion() {
        let mut app = app_with_marker();
        app.world_mut()
            .resource_mut::<crate::ui_script::UiCostWanted>()
            .0 = true;
        app.update();
        app.world_mut().resource_mut::<UiQuads>().dirty = false;
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("marker:SetTexture(0, 0, 1)")
            .unwrap();
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::ui_script::UiFrameCost>()
                .spliced,
            1,
            "the color write is one splice-simple entry"
        );
        assert!(
            app.world().resource::<UiQuads>().dirty,
            "the spliced write re-marks the quads dirty"
        );
        let mut reference = app_from_script(
            r#"
            local plain = CreateFrame("Frame", "Plain")
            plain:SetPoint("TOPLEFT", 0, 0)
            plain:SetWidth(50); plain:SetHeight(50)
            marker = plain:CreateTexture(nil, "ARTWORK")
            marker:SetTexture(0, 0, 1)
            marker:SetAllPoints()  -- templateless Lua region: no implicit anchor
        "#,
        );
        reference.update();
        assert!(
            app.world().resource::<UiQuads>().quads
                == reference.world().resource::<UiQuads>().quads,
            "spliced output equals the full conversion of the same model"
        );
    }

    #[test]
    fn a_count_changing_write_stitches_and_keeps_the_spans_true() {
        let mut app = app_with_marker();
        app.world_mut()
            .resource_mut::<crate::ui_script::UiCostWanted>()
            .0 = true;
        app.update();
        let before = app.world().resource::<UiQuads>().quads.len();
        app.world_mut().resource_mut::<UiQuads>().dirty = false;
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("marker:SetTexture(nil)")
            .unwrap();
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::ui_script::UiFrameCost>()
                .spliced,
            1,
            "the clear rides the splice (stitch branch)"
        );
        let quads = app.world().resource::<UiQuads>();
        assert!(quads.dirty, "a vanished quad is a real change");
        assert_eq!(
            quads.quads.len(),
            before - 1,
            "the cleared marker's quad is gone from the stitched list"
        );
        // The follow-up (0 → 1 quads) splices against the re-derived spans.
        app.world_mut().resource_mut::<UiQuads>().dirty = false;
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("marker:SetTexture(0, 1, 0)")
            .unwrap();
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::ui_script::UiFrameCost>()
                .spliced,
            1
        );
        let mut reference = app_from_script(
            r#"
            local plain = CreateFrame("Frame", "Plain")
            plain:SetPoint("TOPLEFT", 0, 0)
            plain:SetWidth(50); plain:SetHeight(50)
            marker = plain:CreateTexture(nil, "ARTWORK")
            marker:SetTexture(0, 1, 0)
            marker:SetAllPoints()  -- templateless Lua region: no implicit anchor
        "#,
        );
        reference.update();
        assert!(
            app.world().resource::<UiQuads>().quads
                == reference.world().resource::<UiQuads>().quads,
            "post-stitch splice output equals the full conversion"
        );
    }

    /// Showing a hidden sibling inserts entries mid-list, and hiding it removes them.
    #[test]
    fn a_shown_sibling_splices_through_the_shift_and_matches_the_full_conversion() {
        // A, B, C in declaration order, so B's entries sort between A's and C's; B starts hidden.
        const BUILD: &str = r#"
            local function box(name, x, r, g, b)
              local f = CreateFrame("Frame", name)
              f:SetPoint("TOPLEFT", x, 0)
              f:SetWidth(50); f:SetHeight(50)
              local t = f:CreateTexture(nil, "ARTWORK")
              t:SetTexture(r, g, b)
              t:SetAllPoints()
              return f
            end
            box("A", 0, 1, 0, 0)
            hidden = box("B", 60, 0, 1, 0)
            box("C", 120, 0, 0, 1)
        "#;
        let mut app = app_from_script(&format!("{BUILD}\nhidden:Hide()"));
        app.world_mut()
            .resource_mut::<crate::ui_script::UiCostWanted>()
            .0 = true;
        app.update();
        let without = app.world().resource::<UiQuads>().quads.len();
        app.world_mut().resource_mut::<UiQuads>().dirty = false;

        // ── Insertion ────────────────────────────────────────────────────────────────────────
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("hidden:Show()")
            .unwrap();
        app.update();
        let cost = app.world().resource::<crate::ui_script::UiFrameCost>();
        assert!(
            cost.spliced > 0 && cost.dropped == 0,
            "B's entries are the only conversions and nothing left the list \
             (spliced={}, dropped={})",
            cost.spliced,
            cost.dropped
        );
        let quads = app.world().resource::<UiQuads>();
        assert!(quads.dirty, "a frame appearing is a real change");
        assert!(
            quads.quads.len() > without,
            "the shown frame's quad actually joined the list ({} -> {})",
            without,
            quads.quads.len()
        );
        // The comparison app replays the history: `Show()` re-stacks a hidden frame to the tail of
        // its draw bucket (the client's show, `0x76ae10`), so B draws above C.
        let mut reference = app_from_script(&format!("{BUILD}\nhidden:Hide()\nhidden:Show()"));
        reference.update();
        assert!(
            app.world().resource::<UiQuads>().quads
                == reference.world().resource::<UiQuads>().quads,
            "the spliced list equals the full conversion of the same model"
        );

        // ── Deletion ─────────────────────────────────────────────────────────────────────────
        // Nothing converts, so `dropped` is what shows the splice ran.
        app.world_mut().resource_mut::<UiQuads>().dirty = false;
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("hidden:Hide()")
            .unwrap();
        app.update();
        let cost = app.world().resource::<crate::ui_script::UiFrameCost>();
        assert_eq!(cost.spliced, 0, "a pure deletion converts nothing");
        assert!(
            cost.dropped > 0,
            "and it is the drop count that proves it spliced"
        );
        let quads = app.world().resource::<UiQuads>();
        assert!(quads.dirty, "a vanished frame is a real change");
        assert_eq!(
            quads.quads.len(),
            without,
            "the list is back to what it was before B appeared"
        );
        let mut reference = app_from_script(&format!(
            "{BUILD}\nhidden:Hide()\nhidden:Show()\nhidden:Hide()"
        ));
        reference.update();
        assert!(
            app.world().resource::<UiQuads>().quads
                == reference.world().resource::<UiQuads>().quads,
            "the post-deletion list equals the full conversion of the same model"
        );
    }

    /// The held quads were rasterized under the old seam, so a resize takes the full path.
    #[test]
    fn a_resize_takes_the_full_path_not_the_splice() {
        let mut app = app_with_marker();
        app.world_mut()
            .resource_mut::<crate::ui_script::UiCostWanted>()
            .0 = true;
        app.update();
        let win = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut().get_mut::<Window>(win).unwrap().resolution = UVec2::new(800, 600).into();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("marker:SetTexture(0, 0, 1)")
            .unwrap();
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::ui_script::UiFrameCost>()
                .spliced,
            0,
            "a moved raster environment forbids splicing"
        );
    }
}
