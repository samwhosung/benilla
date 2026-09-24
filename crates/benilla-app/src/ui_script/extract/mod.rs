//! The extraction pass: [`tick_script`] + [`paint_script`] turn [`UiScript::extract`]'s per-frame output into
//! [`UiQuads`] for the render pass (screen size → tick → resolve → extract → quads), including the
//! held-cursor icon overlay ([`cursor_icon_quad`]) — CAPTURE-ONLY since decision 0216 §5, where
//! the held payload's icon became the hardware cursor ([`crate::cursor`]) in a normal run; the
//! quad survives only as the visual harness's machine-checkable view (the OS cursor can't appear
//! in capture pixels). Split out of [`super`] purely for size — the plugin wiring and the input
//! pass live there and in [`super::input`] respectively.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::{QuadContent, TexCoords, UiScript};

use crate::ui_pass::{UiQuad, UiQuads, UvRect};
use crate::ui_text::UiFontAtlas;
use benilla_assets::WorldAssets;

mod colorselect;
mod text;

/// `WOW_UI_COST=1` — the untraced per-frame cost meter for this system's phases (the premise
/// instrument for the UI epoch-gate lane, 0730's warm slice): one `[ui-cost]` line per frame with
/// each phase's wall μs, the quad counts, the layout gate's decision, and whether the diff found
/// the produced quads changed. Untraced by design — the campaign grades untraced cpu_ms, and
/// `trace_chrome` inflates exactly the fine-grained spans this measures (0718's calibration).
pub(crate) fn ui_cost_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_UI_COST").as_deref() == Ok("1"))
}

/// `WOW_UI_GATE=1` — the extract gate's miss reporter ([`report_gate_miss`]). Off by default and
/// read once, the `[ui-cost]` meter's posture.
fn gate_log_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_UI_GATE").as_deref() == Ok("1"))
}

/// `WOW_UI_DIFF=1` — the base-lane rebuild-trigger probe (`ui_pass`' twin). Read once; it also
/// pins the conversion to the FULL path (the probe's job is naming the first differing quad of a
/// whole-list diff, which the per-entry splice below deliberately never computes).
fn ui_diff_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_UI_DIFF").is_some())
}

/// `WOW_UI_SPLICE_VERIFY=1` — the splice's own adversary (decision 1638). Read once, off by
/// default, and *expensive by design*: it makes every spliced frame also pay the full conversion.
fn splice_verify_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_UI_SPLICE_VERIFY").is_some())
}

/// `WOW_UI_PICK=<x>,<y>[,<r>]` — **what drew this pixel?** The UI's answer to the world hover
/// inspector. Every converted quad whose screen rect covers that point (or, with `r`, comes
/// within `r` px of it — how a hairline thinner than the probe is caught) names itself once: the
/// `ZTarget` (frame or region handle), the rect, the paint key, and the content — a texture's
/// BLP path and crop included. Coordinates are LOGICAL window px, y-down from the top-left, the
/// same space [`convert_entry`]'s `rect` lives in (a 2x-scale capture's device pixel is half
/// this). Each `z_key` reports once per run, so a 200-frame capture prints the list once.
///
/// Built for a hairline nobody could name (the world map's dark seams): without it, "which
/// region is that one dark row" costs a bisect through FrameXML with a rebuild per guess.
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

/// One `[ui-pick]` line per covering quad — see [`ui_pick_point`]. Deduped by paint key so a
/// steady UI reports its stack once, not once a frame.
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

/// A content arm's name, for a one-line log ([`report_gate_miss`]'s splice twin). Short and
/// stable: these strings are histogrammed in the shell, not read as prose.
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

/// Which axes of a texture region's UV mapping run PAST the texture — the reference's tiling idiom
/// (`SetTexCoord(0, n, 0, 1)` repeats the art n times along u) — and so must sample with `Repeat`.
/// Per axis, because the two answers are independent and the wrong one on the bounded axis is a
/// visible bleed (decision 2000): with both axes on `Repeat`, a strip that spans exactly `[0, 1]`
/// in v has its top pixel row filtered against the texture's LAST row. The stance shelf's middle
/// piece keeps an opaque grey row there, and drew it as a hairline along its own top edge at
/// every four-form bar. The same tolerance [`uv_clamp_window`] uses to tell a tiling axis from a
/// whole-texture one, so the two laws cannot disagree about which axis is which.
pub(crate) fn tiling_axes(uv: &UvRect) -> (bool, bool) {
    let past = |axis: usize| {
        uv.corners
            .iter()
            .any(|c| !(-0.001..=1.001).contains(&c[axis]))
    };
    (past(0), past(1))
}

/// The half-texel-inset UV window a **cropped** quad may sample — [`UiQuad::uv_clamp`]'s producer
/// (decision 1608), `None` when neither axis needs one.
///
/// `CLAMP_TO_EDGE` clamps at the IMAGE's edge; a `SetTexCoord` crop into an ATLAS has no such
/// guard, so a magnified cell's outermost destination pixels sample half a texel past the crop and
/// linear-filter in whatever the neighbouring cell authored. Half a texel of inset is exactly where
/// a standalone clamped texture of that cell stops — the edge texel's CENTRE.
///
/// Decided **per axis**, and the two tests are the whole law:
/// - an axis running past `[0,1]` is the reference's TILING idiom (`SetTexCoord(0, n, 0, 1)` on the
///   stance shelf repeats the art n times); insetting it would walk the art along the run, so it is
///   left alone — the same bounded-axis rule [`benilla_ui::script::inset_atlas_bleed`] applies to
///   the `Backdrop` slices;
/// - an axis spanning the whole texture needs nothing: the sampler's own clamp already is this.
fn uv_clamp_window(uv: &UvRect, size: (u32, u32)) -> Option<[f32; 4]> {
    let mut out = [1.0, 1.0, 0.0, 0.0]; // both axes off — `min > max` (see `UiQuad::uv_clamp`)
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
        // A crop under one texel wide has no interior either: pin both bounds to its centre, which
        // is the one texel it means.
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

/// One line naming why the extract gate did not skip this frame — the input that differed, and for
/// the render list the first entry that moved. A settled UI is what makes the paint pass free; when
/// it is never settled, this says what is moving.
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
        // Named, because it is the one miss with no visible cause at all: the glyph sheet filled
        // and was repacked from empty, so every held quad's UV moved. It should be very rare —
        // `WOW_GLYPH_CACHE=1` reports the occupancy that led here (decision 1342).
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
        // Nothing in this pass's own inputs moved — the miss came from the capture-mode arm.
        None => eprintln!("[ui-gate] miss: capture mode (the gate never skips under a capture)"),
    }
}

/// Last frame's extract-gate inputs (decision 0740), held together as one `Local` — the gate
/// compares all of them or none, and a Bevy system has a hard param budget this was eating.
#[derive(Default)]
pub(super) struct GateInputs {
    extracted: Vec<benilla_ui::script::ExtractedQuad>,
    text_ui: Option<benilla_ui::script::EditBoxTextUi>,
    dims: Option<(u32, u32, u32, u32)>,
    portraits: std::collections::HashMap<String, crate::portrait::PortraitSource>,
    /// The `UiFontAtlas::generation` the held quads' glyph UVs came from — see the gate's own
    /// note below. Moves only when the glyph sheet resets (decisions 1339, 1342).
    generation: Option<u64>,
    /// Per-entry prefix ends into the conversion's output: entry `i`'s quads occupy
    /// `spans[i-1]..spans[i]` of `UiQuads::quads` (0 for `i = 0`). Recorded by the full
    /// conversion, kept current by the splice — this is what lets a one-entry change (the resting
    /// blink) re-convert one entry instead of the whole interface.
    spans: Vec<u32>,
    /// The stitch's ping-pong buffer: last frame's quad allocation, emptied. A stitch drains the
    /// live list into a fresh one and parks the drained allocation here, so a steady stream of
    /// changes (a hovered tooltip re-filling every frame) reuses two buffers forever instead of
    /// allocating ~1,400 quads a frame.
    held: Vec<UiQuad>,
}

/// How this frame's entry list lines up with last frame's — what the splice re-converts, and
/// where everything it keeps came from.
struct Alignment {
    /// For each entry of the NEW list: the OLD index whose already-converted quads it reuses, or
    /// `None` — it must be converted. Kept indices are strictly increasing.
    source: Vec<Option<usize>>,
    /// The OLD entries nothing reuses. Their quads leave the list, so their conversion's side
    /// effects (a parked minimap slot, a link span, a tile request) would have to be *undone* — the
    /// splice cannot, so it only proceeds when every one of them was quads-only.
    dropped: Vec<usize>,
}

/// The alignment: a merge over `z`, which both lists are sorted by ([`benilla_ui::script::UiScript::extract`]).
///
/// **Why `z` and not the index.** `z` is a packed `(strata, level, frame-insertion, layer, …)`
/// key ([`benilla_ui::order::ZKey`]) — an entry's *identity*, unchanged when a neighbour is
/// inserted or removed. A positional compare cannot see that: hovering one item slot inserts a
/// single highlight entry at index 359 of 520 and shifts the 160 entries behind it, every one of
/// which then compares unequal by index and equal by `z`. Before this, that frame took the full
/// conversion — 86 % of all hover frames did (decision 1638).
///
/// **The merge FINDS the alignment; it is never trusted.** Quads are reused only for a pair that
/// compares fully equal, and [`convert_entry`] is a pure function of the entry plus the raster
/// environment the splice's guards pin — so an equal pair converts to equal quads whatever their
/// indices. A merge that mis-pairs therefore costs conversions and never correctness, which is
/// what makes it safe to key on a `z` we do not prove unique or even prove sorted. (It is not
/// quite sorted: a ScrollingMessageFrame's own content line is emitted at its frame's slot and
/// sorts above the frame's BACKGROUND regions, so the list dips locally around a chat window.
/// Identical dips on both sides match anyway; a differing one re-converts a handful of entries.)
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
            // An entry that is gone: last frame drew something this frame does not.
            std::cmp::Ordering::Less => i += 1,
            // An entry that is new.
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

/// Whether an entry's conversion writes **quads and nothing else** — the splice's admission test.
/// The full path plumbs three side channels (the engine's link spans, the parked minimap slot,
/// the booth panes and tile requests); the splice keeps last frame's values for all of them,
/// which is only sound for entries that never write one, or write them idempotently.
fn splice_simple(eq: &benilla_ui::script::ExtractedQuad) -> bool {
    match &eq.content {
        QuadContent::Frame
        | QuadContent::Backdrop { .. }
        // A model pane writes at most one quad and one idempotent tile request (decision 2008)
        // — and the map's arrow changes facing on every turn while the map is open.
        | QuadContent::ModelPane { .. }
        // Both colour-picker arms write one quad and nothing else — and they change on every
        // step of a drag, which is exactly the traffic the splice exists for.
        | QuadContent::ColorWheel
        | QuadContent::ColorValue { .. } => true,
        // A FontString's text writes nothing but quads, and that is every label, every unit
        // frame's name, every tooltip line — the traffic this whole path exists for (decision
        // 1638). A FRAME-targeted Text quad is a message-frame ring line, whose hyperlink spans
        // are a REPLACE-THE-WHOLE-SET channel (`set_link_spans`): a line that stops carrying a
        // link would leave a stale clickable rect behind, and the splice's post-conversion
        // tripwire cannot see that, because the scratch it inspects is empty in exactly that
        // case. So the target, not the tripwire, is the guard.
        QuadContent::Text { .. } => matches!(eq.target, benilla_ui::order::ZTarget::Region(_)),
        QuadContent::Texture {
            portrait_unit: None,
            ..
        } => true,
        _ => false,
    }
}

/// The splice's adversary ([`splice_verify_enabled`]): re-run the FULL conversion over the same
/// entry list and prove the spliced list is what it would have produced — quads AND span table.
///
/// This is the check the splice's whole argument rests on, made by machine instead of by
/// reasoning. It is what turns "an equal entry converts to equal quads" from a claim about
/// [`convert_entry`] into a measurement over a real interface: a live client with ~520 entries, a
/// tooltip re-filling every frame, and a hover highlight inserting and removing itself under it
/// (decision 1638). A mismatch names the quad, the entry that owns it, and that entry's owner.
///
/// The side channels are collected into throwaway buffers and dropped — the point is the quads.
/// `booths.panes` is the one exception, because [`convert_entry`] writes it through the shared
/// bridge; re-adding the same tokens is idempotent, so a verify run's panes end up where the full
/// path would have put them anyway.
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
    // Which entry owns quad `i` — the span table is a prefix-end list, so the first end past it.
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

/// One quad in a log line — enough to see WHICH way two conversions disagree (geometry, paint,
/// crop, or which texture), never the whole struct.
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

/// **Hand the VM the host's font engine** — the one construction site for
/// [`crate::ui_text::AtlasMeasurer`], so a `SetText` → `GetStringWidth` pair *inside one Lua call*
/// returns a real number instead of 0 (the reference answers that getter inline, `0x79e510` →
/// `0x772890`).
///
/// `seam` is the raster seam the atlas answers under ([`super::seam_scale`]): a measurer built
/// under one seam does not answer for another, which is why both callers re-seat rather than
/// check first.
///
/// Two callers, and the second one is the point (decision 2028): this pass, at the seam edge and
/// on the first frame the atlas exists — and [`super::lifecycle::load_ingame_ui_on_world_entry`],
/// **before the manifest loads**, because that is where every `<OnLoad>` in the in-game UI runs.
/// Seated only from here, a VM born and loaded inside one exclusive system — which is exactly
/// what `ReloadUI()` does — runs its whole load edge measuring 0.
pub(crate) fn seat_text_measurer(script: &mut UiScript, atlas: &UiFontAtlas, seam: f32) {
    script.set_text_measurer(Box::new(crate::ui_text::AtlasMeasurer::new(
        atlas.engine(),
        seam,
    )));
}

/// **The UI pass, first half: the VM's frame** — screen size -> `tick` (OnUpdate) -> the measure
/// round-trip -> `resolve`. Script errors drain to the log (throttled by being drained, each fires
/// once). The quads are the second half's ([`paint_script`]).
///
/// **The pass is two systems because a widget can be anchored to the world** (decision 2168). The
/// hit test has to run before `WorldStage::Input` (`PlayerUiHover` feeds `PointerOverUi`, which the
/// camera reads), so this half stays there. Building the quads there too made every world-anchored
/// widget a frame stale, and since 2148 the nameplates ARE widgets — real `WorldFrame` children
/// positioned by `vplates::drive_vplates` from a camera that only exists later in the frame. The
/// plate was therefore painted at the unit's PREVIOUS frame's screen point: a median 1.9 px and a
/// p90 of 16 px of lag per frame at 640x360 while a guard walked (the `vpl` trace, 2026-09-10), and
/// proportionally more at a real window. That is the director's "way more jittered when the
/// creature is moving".
pub(super) fn tick_script(
    script: Option<NonSendMut<UiScript>>,
    window: Query<&Window, With<PrimaryWindow>>,
    // REAL time, deliberately: this drives the VM's `GetTime()` session clock, and the reference
    // clock is the OS wall clock. Bevy's default virtual time clamps every frame delta to
    // `max_delta` (250 ms), so hitches, loading stalls, and macOS occlusion throttling (~1 fps
    // when the window is covered) permanently drop UI-clock time — every GetTime-anchored timer
    // (the cooldown sweep, aura expirations, fades) then runs LONG against the wall-clock
    // cooldown store and the server, which is exactly "I can charge before the sweep ends"
    // (verified live: an occluded run's UI clock fell 27 s behind in 43 s of wall time).
    time: Res<Time<Real>>,
    mut ui_clock: ResMut<super::UiClock>,
    mut font_atlas: Option<ResMut<UiFontAtlas>>,
    // The booth seam's DPI half, and the pane map this half invalidates when the VM or the window
    // is gone — see the two early returns.
    mut booths: crate::portrait::BoothBridge,
    ui_cost_wanted: Res<super::UiCostWanted>,
    // The seam scale the engine's text-metric caches were answered under. When `s` moves (window
    // resize / fullscreen toggle / uiScale change), every cached measure is stale — see the
    // invalidation below.
    //
    // Deliberately NOT a `VmMemo` (decision 1290's sweep): this is a fact about the RASTER, not
    // about what the VM has been told, and it stays true across the VM's death and rebirth. A fresh
    // VM has no cached measures to invalidate and no measurer at all, and the re-seat below already
    // catches that on its own test (`!script.has_text_measurer()`).
    mut last_seam: Local<f32>,
    // The `scale_factor` this pass last answered measures under — the other half of the same
    // staleness edge (decision 1342). `0.0` until the first frame, which is also a real edge.
    mut last_dpi: Local<f32>,
    // The uiScale dial folded into the seam scale (decision 0584).
    ui_scale: Res<super::UiScaleCvar>,
    // This frame's phase split, published for whoever asked (the `[ui-cost]` line, `hover_log`).
    mut ui_cost: ResMut<super::UiFrameCost>,
    // What the paint half needs from this one — and whether it may run at all.
    mut pass: ResMut<super::UiPassState>,
) {
    pass.live = false;
    let Some(mut script) = script else {
        // No VM ⇒ no UI is sampling any booth pane. The panes map must not outlive its writer:
        // every OTHER return in this system provably keeps pane presence unchanged (the settled
        // gate compares the whole extracted list; the splice refuses `portrait_unit` entries),
        // but a VM dying mid-world with the character window up would strand its pane here and
        // the paper-doll camera would render behind a dead UI until the next full conversion.
        booths.panes.0.clear();
        return;
    };
    let Ok(window) = window.single() else {
        // Same law as the no-VM arm: no window, no sampling — a pane map with no writer lies.
        booths.panes.0.clear();
        return;
    };
    let (w, h) = (window.width(), window.height());
    // The 768-virtual UI space (decision 0582 — byte law: the client's FrameXML space is ALWAYS
    // 768 units tall, every aspect, mapped to the window; the converter identity `f(screenH) = 768`
    // (`0x41ad10`) + the caret `H_px/192` law (`0x77b8c0`)), times the uiScale dial (decision 0584
    // — the VM's screen is `768/uiScale` units tall, so a dial below 1 shrinks everything). The VM
    // lives entirely in that space: this seam scales quads ×s on the way out, mouse ÷s on the
    // way in (input.rs), and measures ÷s on the way back. At a 768-tall window with the dial at
    // 1 the whole pipeline is bit-identical to the pre-virtual behavior.
    let s = super::seam_scale(h, ui_scale.0);
    // The raster environment moved (resize / fullscreen / uiScale): every text metric the engine
    // caches — FontString measures, chat row counts, editbox advance tables, the digit feed — was
    // answered under the OLD `s`, and integer-stepped advances measured at one raster size do not
    // rescale to another (the font-size snap alone shifts a string's unit width by several
    // percent — enough that a boot-size measure fails the fullscreen fit test and the ellipsis
    // eats fitting text: the director's "Contr..." rows, reproduced by `WOW_RESIZE`). Declare the
    // staleness at the seam and let every round-trip re-answer under the new `s`.
    //
    // **The DPI is the other half of the same edge** (decision 1342). A logical height becomes an
    // integer DEVICE-pixel raster size, so a monitor hop at an unchanged window size leaves `s`
    // exactly where it was and still moves every measured width. (1296/1339 caught this through
    // the atlas's bake generation, which followed the same two terms; with nothing to re-bake, the
    // honest thing to watch is the term itself.)
    let dpi = window.scale_factor();
    // The tile renderer sizes its cells in device pixels off this (decision 2008).
    booths.tiles.dpi = dpi;
    let seam_moved = *last_seam != s || *last_dpi != dpi;
    if seam_moved {
        if *last_seam != 0.0 {
            script.invalidate_text_measures();
        }
        *last_seam = s;
        *last_dpi = dpi;
    }
    // …and re-seat the VM's own font engine at the same edge, for the same reason: an
    // [`crate::ui_text::AtlasMeasurer`] answers only for the seam it was built under. This is what
    // makes a `SetText` → `GetStringWidth` pair *inside one Lua update* return a real number
    // instead of 0 — the reference answers that getter inline (`0x79e510` → `0x772890`), and the
    // corpus writes it that way (`Bagnon_Forever/database/ui.lua:58-59`).
    //
    // Seated BEFORE the tick below, so the first update that runs already has it, and rebuilt only
    // on the seam edge or the frame the atlas first exists — an `Arc` clone and an `f32`, never a
    // per-frame cost. This is the *frame* edge; the *load* edge is
    // [`super::lifecycle::load_ingame_ui_on_world_entry`]'s own call to the same seat (2028).
    if let Some(atlas) = font_atlas.as_deref() {
        if seam_moved || !script.has_text_measurer() {
            seat_text_measurer(&mut script, atlas, s);
        }
    }
    // Phase spans (visible under `bevy/trace_chrome`): this system is the biggest flat CPU cost
    // on an idle frame, and the ledger can only rank what has a name — tick (Lua OnUpdate),
    // resolve (layout), measure (the text round-trips), extract (tree walk + rasterize), diff.
    // The phase marks feed two consumers now: the `[ui-cost]` line and the hover recorder
    // (`hover_log`), which writes the same split per frame to a file. Either one arms them.
    let printing = ui_cost_enabled();
    let cost_on = printing || ui_cost_wanted.0;
    let solves_before = cost_on.then(|| script.layout_solves());
    let derives_before = cost_on.then(|| script.layout_derivations());
    // The measure counters are PER FRAME: `measure_fontstrings` adds to them, and both publish
    // sites below carry them forward so the two measure passes sum into one frame's row. Nothing
    // else zeroes them, so without this the recorder's "re-shaped strings" column is a lifetime
    // total wearing a per-frame label — it read a flat 219/frame on a glue screen that measures
    // nothing after startup, and cost one wrong theory before the CSV contradicted it.
    ui_cost.measured = 0;
    ui_cost.measured_texts.clear();
    let mut t_mark = cost_on.then(std::time::Instant::now);
    // Marks phase boundaries under the meter: returns μs since the previous mark and re-arms.
    // With the meter off it does nothing (a run carries six no-op calls, not six clock reads).
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
        // A resize re-runs everything the interface COMPUTED from the old screen size — see
        // [`super::manifest::on_screen_resized`].
        if resized {
            super::manifest::on_screen_resized(&script);
        }
        script.tick(time.delta_secs());
    }
    // The frame's clock pair ([`super::UiClock`]): the VM value the tick just produced, anchored
    // at the exact `Instant` whose frame-to-frame deltas that clock accumulates — `Time<Real>`'s
    // own last-update. Every Instant→GetTime conversion goes through this pair; sampling
    // `Instant::now()` at a conversion site instead re-measures the tick→feed scheduling gap
    // every frame and wobbles the derived start by that jitter (the resource's own doc).
    *ui_clock = super::UiClock {
        anchor: time.last_update().unwrap_or_else(std::time::Instant::now),
        ui_now: script.now(),
    };
    let us_tick = lap();
    // ── The FontString measure round-trip, BEFORE the resolve ────────────────────────────────
    // `fontstrings_needing_measure` reads only `region_data` — each FontString's text, font, and
    // its explicit/wrap-pinned size — and never a resolved rect; nothing in `resolve` writes a
    // region's size either. So the answers do not need a layout pass to exist, and running them
    // first means the frame's ONE resolve already sees them.
    //
    // Measuring after the resolve (the old order) cost a second FULL solve on every frame whose
    // text changed, because answering a measure moves the anchor solve's read set. On the shipped
    // UI that solve walks 2,164 frames and sweeps 6,297 regions per round — for a hover whose
    // change touches ten FontStrings inside one frame. Measured on the default UI with a tooltip
    // content change per frame (`resolve_bench`): 3.22 ms/frame at 2.00 solves → 2.37 ms at 1.00.
    // The live shape this comes from: a hover frame that solved cost ~10 ms against ~0.2 ms for
    // one that did not (`WOW_HOVER_LOG`, director's run).
    let mut measured_any = false;
    if let Some(atlas) = font_atlas.as_deref_mut() {
        measured_any = measure_fontstrings(&mut script, atlas, s, &mut ui_cost, ui_cost_wanted.0);
    }
    {
        let _span = bevy::log::info_span!("ui_script: resolve").entered();
        script.resolve();
    }
    // The backstop: only when this frame actually measured something can a fresh request exist
    // that the pass above could not have seen. Empty on every frame in the bench — and skipping it
    // otherwise keeps the quiet frame at ONE sweep of the 6,297-region map, not two.
    if measured_any {
        if let Some(atlas) = font_atlas.as_deref_mut() {
            if measure_fontstrings(&mut script, atlas, s, &mut ui_cost, ui_cost_wanted.0) {
                script.resolve();
            }
        }
    }
    let us_resolve = lap();
    let measure_span = bevy::log::info_span!("ui_script: measure").entered();
    // The message-line half of the round-trip: chat ring lines ask for their wrapped ROW COUNT at
    // the frame's resolved width, so the emit pass can stack real content heights (a long line
    // pushes older lines up instead of overlapping them). Same-frame like the FontString half, but
    // no re-resolve — rows shift only the emitted bands, never the anchor graph.
    if let Some(atlas) = font_atlas.as_deref_mut() {
        let requests = script.message_lines_needing_measure();
        if !requests.is_empty() {
            let rows: Vec<(u32, u32, u16, u64)> = requests
                .iter()
                .map(|r| {
                    let n = crate::ui_text::measure_wrapped_rows(
                        &mut atlas.lock(),
                        &r.text,
                        // wrap_width is the RESOLVED frame width (scale already in it) — ×s only;
                        // the font height is frame-local — ×s × the frame's scale, the drawn size.
                        r.wrap_width * s,
                        crate::ui_text::FontSpec {
                            path: r.font.as_deref(),
                            // The band's own drawn px (one-to-one regime; inert ≤ 32).
                            height: crate::ui_text::drawn_px(r.height, None, s * r.scale),
                            outline: r.outline, // step-law width bias, as in the measures above
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
    // The player's half of the error contract before the host's (decision 1305): every caught
    // script error goes to the Lua error handler — `_ERRORMESSAGE` → the ScriptErrors dialog
    // since BasicControls installs it, an addon's own handler if it chose one — and then the
    // same errors drain to the log as always. Dispatch first so a handler that itself fails
    // lands in this frame's drain, not the next one's.
    script.dispatch_script_errors_to_handler();
    for err in script.take_errors() {
        warn!("ui_script: {err}");
    }
    for w in script.take_warnings() {
        warn!("ui_script: {w}");
    }

    // ── The handover ────────────────────────────────────────────────────────────────────────
    // Everything the paint half derives from the window and the meter, published once here so the
    // two halves cannot disagree about the frame they are in.
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

/// **The UI pass, second half: the quads** — a second (incremental) measure + `resolve`, the tree
/// walk, and the conversion into [`UiQuads`].
///
/// Split out of [`tick_script`] (decision 2168) and scheduled **after the camera and after the
/// plate driver**, which is the whole point: a widget positioned from this frame's camera has to be
/// walked after that camera exists. The re-resolve is what makes the plates' fresh anchors real —
/// it is the layout ledger's incremental pass, so on a frame where nothing but the plates moved it
/// solves the plates and nothing else.
pub(super) fn paint_script(
    script: Option<NonSendMut<UiScript>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut quads: ResMut<UiQuads>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut font_atlas: Option<ResMut<UiFontAtlas>>,
    // Both directions of the booth seam: the token -> off-screen-baked-face bridge a
    // `SetPortraitTexture`-bound region samples, and the pane geometry this pass publishes back
    // for the booths' projection aspect + render gate (decision 1069).
    mut booths: crate::portrait::BoothBridge,
    // Two facts about the RUN, tupled to stay inside Bevy's 16-element system-param limit (the
    // same squeeze `player::control` and `GateInputs` above record):
    // · the held-cursor icon quad (decision 0216 §5) is CAPTURE-ONLY, the same presence check
    //   every other capture-only system uses (`ui_script::capture_ui_active`'s sibling pattern);
    // · whether anything wants this pass's phase split at all ([`super::UiCostWanted`]).
    run: (
        Option<Res<crate::run_mode::CaptureMode>>,
        Res<super::UiCostWanted>,
    ),
    // The append-lane hole this pass parks for its UiQuadAppend producer: the `<Minimap>` widget
    // slot (`minimap::emit_minimap` fills it with tile/arrow quads — decision 0203 phase 1). (The
    // autocast shine's sites used to ride beside it; the shine is a model tile since 2014.)
    mut parked_minimap: ResMut<crate::minimap::MinimapWidget>,
    // ── The extract gate's memory (decision 0740): last frame's conversion inputs ─────────────
    // The conversion loop below is a pure function of (extracted, text_ui, the RASTER
    // ENVIRONMENT, the portrait token map, the glyph-sheet generation) — the sprite caches are
    // monotone path→handle, so equal inputs reproduce the same `UiQuads` the diff would then
    // discard. Capture mode never skips (the harness wants exact per-frame output, including the
    // cursor-icon quad's live mouse position).
    //
    // The raster environment is window size × seam scale × **`scale_factor`**, and that last term
    // is decision 1342's correction. A monitor hop at an unchanged window size moves nothing else
    // in this list, but it changes the integer device-pixel size every glyph rasterizes at
    // (`TextEngine::ppem`) — so the quads held from last frame are the wrong size and the gate
    // must miss. (1339 caught the same hole through an atlas-bake counter, which was the right
    // fix for a design where the atlas re-baked; there is nothing to re-bake now, so the term that
    // survives is the one that actually moved.)
    //
    // The generation term is the glyph sheet RESETTING — the one event that moves a cached cell's
    // UV, which happens when the sheet fills and is repacked from empty. Held quads carry the old
    // UVs and would draw letters as fragments of other letters.
    //
    // A [`crate::ui_script::VmMemo`] because a skip is not only a quad-conversion skip: it also skips
    // `set_link_spans` and the minimap-slot / booth-pane refills, which are pushes INTO the VM. The
    // VM lives for one login (decision 1290), so a fresh VM must never be gated on what the previous
    // one extracted — its first frame is always a real conversion.
    mut prev: Local<crate::ui_script::VmMemo<GateInputs>>,
    // This frame's phase split, published for whoever asked (the `[ui-cost]` line, `hover_log`).
    mut ui_cost: ResMut<super::UiFrameCost>,
    // Whether the tick half ran this frame, and what it derived.
    pass: Res<super::UiPassState>,
) {
    let (capture, ui_cost_wanted) = run;
    // The paint half stands down whenever the tick half did — no VM, or no window. Both of those
    // arms already cleared the booth panes, and re-deriving the seam here would be inventing a
    // frame the VM never had.
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
    // Marks phase boundaries under the meter: returns μs since the previous mark and re-arms.
    let mut lap = move || -> u128 {
        if !cost_on {
            return 0;
        }
        t_mark
            .replace(std::time::Instant::now())
            .map_or(0, |t| t.elapsed().as_micros())
    };
    // ── The SECOND measure + resolve, and the reason this half is its own system ─────────────
    // Everything written into the VM since the tick half ran becomes real here — the nameplate
    // anchors above all, which `vplates::drive_vplates` writes from a camera that did not exist
    // when the tick half ran. Incremental: the ledger solves what is dirty, so a frame in which
    // nothing moved pays a walk and no solve. A plate whose unit's NAME changed also needs its
    // measure served here, which is why the round-trip comes along rather than the resolve alone.
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
    // The glyph sheet's repack counter — the extract gate's term, read here because the gate is
    // here (a repack moves every cached cell's UV, so held quads would draw letter fragments).
    let generation = font_atlas.as_deref().map(|a| a.generation);
    let extract_span = bevy::log::info_span!("ui_script: extract").entered();
    let mut out = Vec::new();
    let mut assets = world_assets;
    let mut minimap_slot = None;
    // Hyperlink spans collected while rasterizing message-line Text quads (frame-targeted only —
    // chat lines; FontString-region links are a later arc), fed back to the engine's click
    // hit-test after the loop. Rects flip back to the engine's y-up space.
    let mut link_spans: Vec<(
        benilla_ui::widget::FrameHandle,
        benilla_ui::layout::Rect,
        String,
        String,
    )> = Vec::new();
    // The EditBox advance-table answer (the metrics half of the mouse/selection law): the
    // focused box's display string measured per-byte under the same shaping + step law as its
    // draw, so the engine's click→index, drag-select, and scroll window land exactly on the
    // glyphs. (The request reads the region's load-resolved font; the transient state-font
    // overlay below never applies to an editbox text region.)
    if let Some(atlas) = font_atlas.as_deref_mut() {
        if let Some(req) = script.editbox_advances_request() {
            let spec = crate::ui_text::FontSpec {
                path: req.font.as_deref(),
                // Measured at the DRAWN raster size (seam × the box frame's scale) but divided
                // back by the seam alone: the advances return in screen UI units, the space the
                // engine's ÷s mouse feed and the box's scale-multiplied rect live in (the
                // request's `scale` doc).
                height: crate::ui_text::drawn_px(req.height, None, s * req.scale),
                outline: req.outline,
                alpha_gradient: None, // alpha never changes metrics
            };
            let cum: Vec<f32> = crate::ui_text::line_advances(&mut atlas.lock(), &req.text, spec)
                .iter()
                .map(|a| a / s)
                .collect();
            // A multiline box also gets its wrapped-row starts + row pitch — the same wrap pass
            // the draw uses, so the engine's (row, x) caret/click law lands on the drawn rows.
            // Advances/pitch return in UI units (÷s): the engine's click→index and row math
            // compare them against the ÷s mouse feed.
            let (rows, cell_h) = match req.wrap_width {
                Some(w) => crate::ui_text::line_rows(&mut atlas.lock(), &req.text, w * s, spec),
                None => (vec![0], 0.0),
            };
            script.set_editbox_advances(req.id, req.key, cum, rows, cell_h / s);
        }
    }
    // The focused edit box's text-UI geometry (the engine leaves caret/highlight geometry to the
    // host): which Text quad is the box's, the scroll window to draw, and the caret/selection
    // x-spans within it — advance-derived engine-side (the blink phase too, `0x77a790`'s 0.5 s
    // law in the engine tick), matched in the Text arm below.
    let text_ui = script.focused_editbox_text_ui();
    let _ = lap(); // re-arm: the editbox seam above is not the walk's cost
    let extracted = script.extract();
    let us_exm = lap();
    let n_extracted = extracted.len();
    // ── The extract gate (decision 0740) ──────────────────────────────────────────────────────
    // Every input the conversion below reads, compared against last frame's. Equal inputs make
    // the loop a pure re-derivation of `UiQuads` the diff at the bottom would discard — at the
    // LBRS pin that was every single settled frame, ~0.3 ms/frame of glyph re-rasterization for
    // an identical `Vec`. On a skip, `quads`/the parked minimap slot/the
    // engine's link spans all keep last frame's values, which the equal inputs prove are this
    // frame's values too.
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
    // `WOW_UI_GATE=1` — why the gate MISSED. A skip is worth the whole conversion, so the
    // question "what moved?" is where a per-frame cost always ends, and a `Vec` inequality does
    // not answer it. Off by default, like the `[ui-cost]` meter beside it.
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
    // ── The per-entry splice: 1361's shape one layer up ──────────────────────────────────────
    // The gate above is all-or-nothing, so ONE animating entry (the resting blink, a sweeping
    // cooldown) used to re-convert the whole interface every frame. When the only input that
    // moved is the extracted list itself, the lists are index-aligned (same length — the
    // traversal is stable when nothing was created/destroyed/restacked), few entries differ,
    // and every differing entry converts to nothing but quads (no link spans, no minimap slot,
    // no booth panes), the changed entries re-convert alone — through the same [`convert_entry`]
    // body, which is what makes the spliced output equal the full path's by construction — and
    // stitch into last frame's `UiQuads` at the ranges `prev.spans` remembers. Everything else
    // (the side channels, the panes map, the engine's link spans) keeps last frame's values,
    // which the unchanged entries prove are this frame's too: the settled path's own argument,
    // applied per entry.
    'splice: {
        // `WOW_UI_GATE=1` names why the SPLICE declined too — one line per frame, the same dial
        // and the same posture as `[ui-gate]` above, because "the gate missed, and then the
        // splice missed too" is one question asked twice. Histogram it in the shell.
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
        // The focused edit box carries the caret BLINK — wall-clock state that appears in no
        // entry, and the one time-dependent input the Text arm reads. It is what makes text
        // splice-safe at all (decision 1638): everything else the conversion reads is either in
        // the entry or pinned by a guard above.
        if text_ui != prev.text_ui {
            no_splice!("focused editbox text-ui moved");
        }
        if booths.images.0 != prev.portraits {
            no_splice!("portrait sources moved");
        }
        // spans describe last conversion's `quads.quads` — if any future writer replaces the
        // base lane out from under this pass, degrade to the full conversion instead of
        // mis-stitching.
        if prev.spans.len() != prev.extracted.len()
            || prev.spans.last().copied().unwrap_or(0) as usize != quads.quads.len()
        {
            no_splice!("span table stale");
        }
        let align = align_entries(&prev.extracted, &extracted);
        // The entries this frame has to pay for, bounded: a real UI transition (a window
        // opening) moves many, and the full conversion is the right tool there anyway.
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
        // The all-equal case belongs to the settled gate above; reaching here with nothing to do
        // means a comparison razor slipped — take the full path rather than risk skipping a
        // change. (A frame that only *drops* entries has real work and is not this case.)
        if changed.is_empty() && align.dropped.is_empty() {
            no_splice!("no entry differs (a comparison razor slipped)");
        }
        // Checked on both ends: every entry being converted, and every entry LEAVING the list —
        // a departing minimap slot would have to be un-parked, which the splice has
        // no way to do.
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
        // Re-convert just those entries. The scratch side channels are a tripwire: `simple`
        // makes them unreachable today, and if an arm ever grows a new side effect the full path
        // is the only safe answer.
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
        // The in-place case: nothing was inserted or removed, so every kept entry is still at
        // its own index and every changed one still owns the same stretch of the list. This is
        // the resting blink's shape and it writes bytes without moving any.
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
            // Stitch a fresh list: the kept runs MOVED out of last frame's (a `UiQuad` carries an
            // `Arc` texture handle, and copying ~1,400 of them a frame to rebuild a list is pure
            // refcount traffic for no pixel), the re-converted entries from the scratch, and a
            // re-derived span table. `prev.held` is the ping-pong buffer: the drain leaves last
            // frame's allocation empty and we keep it for the next stitch, so a steady stream of
            // tooltip changes allocates nothing.
            let mut old = std::mem::take(&mut prev.held);
            std::mem::swap(&mut old, &mut quads.quads);
            quads.quads.clear();
            quads.quads.reserve(old.len() + scratch.len());
            let mut new_spans: Vec<u32> = Vec::with_capacity(extracted.len());
            {
                let mut src = old.drain(..);
                // How many of last frame's quads the drain has already yielded — kept runs are
                // strictly increasing (the merge only ever advances), so one forward pass does it.
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
    // This frame's booth panes, refilled by the loop below (decision 1069). Cleared only HERE, on
    // the un-skipped path: a settled frame draws exactly what the last one did, so the map it left
    // is still this frame's truth — clearing it above the gate would make every quiet frame put the
    // body panes' cameras to sleep and freeze their animation.
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
    // The payload held on the cursor draws last (a 32×32 icon at the mouse, over the whole UI) —
    // but ONLY in capture mode (decision 0216 §5): a normal run shows it as the hardware cursor
    // instead (`crate::cursor`), which can't appear in a screenshot's pixels, so the quad is the
    // capture harness's stand-in. Any arm, matching the hardware cursor's own `payload_icon`
    // (item/spell/action alike — the item-only read here predated the spell/action producers).
    // Purely visual either way — the drag state lives in the engine; the wire settles the move.
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

    // The rasterized hyperlink spans replace last frame's set — the engine's release dispatch
    // (`OnHyperlinkClick`) hit-tests against exactly what is on screen.
    script.set_link_spans(link_spans);

    // Park this frame's Minimap widget slot (or clear it — a hidden cluster extracts nothing);
    // `minimap::emit_minimap` runs later in the frame (UiQuadAppend) and fills the hole.
    parked_minimap.0 = minimap_slot;
    drop(extract_span);
    let us_exa = lap();

    let _span = bevy::log::info_span!("ui_script: diff").entered();
    let n_quads = out.len();
    let changed = quads.quads != out;
    if changed {
        // `WOW_UI_DIFF=1` — the base-lane half of the rebuild-trigger probe (`ui_pass`' twin):
        // names the first Lua-UI quad that differs from last frame's extraction.
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

/// One extracted entry converted to its screen quads, pushed onto `out` — with the side channels
/// some arms carry: chat link spans (Text), the minimap widget slot (Minimap), the booth pane
/// aspects (portrait-bound Texture), the model tile requests (ModelPane, decision 2013). The ONE
/// conversion body: the full pass and the per-entry
/// splice both call this, which is what makes the splice's output equal the full path's by
/// construction. An arm is splice-eligible only if it writes nothing but `out` — keep
/// [`splice_simple`] in agreement when an arm's side effects change.
fn convert_entry(
    eq: benilla_ui::script::ExtractedQuad,
    s: f32,
    // The window, logical px. `h` flips y-up WoW space into the y-down quad pass; `w` is here for
    // the one producer whose law needs the SCREEN's shape — a model tile's layout unit and
    // particle unit run on the screen diagonal (decision 2013).
    w: f32,
    h: f32,
    // The window's DEVICE scale, for the one arm that resamples its art to physical pixels: the
    // nameplate border's 0188 sharpen, which is a no-op at any other size.
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
    // WoW UI space is y-up from the bottom-left in 768-virtual units; the quad pass is
    // y-down window px from the top-left — scale ×s, then flip through the window height.
    let rect = Rect::new(r.left * s, h - r.top * s, r.right * s, h - r.bottom * s);
    if let Some((at, r)) = ui_pick_point() {
        report_ui_pick(&eq, rect, at, r);
    }
    // The ScrollFrame clip (decision 0112), through the same conversion as `rect` —
    // `UiQuad::clip` is the CPU-clip stand-in `ui_pass` already applies uniformly to
    // every quad (texture, backdrop, and glyph alike), so this is the entire app-side plumb.
    let clip = eq
        .clip
        .map(|c| Rect::new(c.left * s, h - c.top * s, c.right * s, h - c.bottom * s));
    match eq.content {
        // Frames draw nothing themselves in v1 (regions carry the visuals).
        QuadContent::Frame => {}
        // The `<Minimap>` widget's content hole: parked for the minimap renderer (an
        // UiQuadAppend producer), which fills it at this exact z — the widget slot itself
        // emits nothing here (decision 0203 phase 1).
        QuadContent::Minimap { zoom, inside_zoom } => {
            *minimap_slot = Some(crate::minimap::MinimapSlot {
                rect,
                z: eq.z,
                zoom,
                inside_zoom,
                alpha: eq.alpha,
            });
        }
        // A `<Model>`/`<PlayerModel>` pane's content: the off-screen body bake its window keeps,
        // sampled square edge to edge. The pane→booth join is
        // [`crate::portrait::model_pane_booth`], whose doc says why it is a table of frame names.
        //
        // This is the same draw the `portrait_unit` arm below makes for a Texture region, and it
        // used to BE that draw: until decision 1751 a file of ours put a Texture inside each pane
        // and named the slot with `BenillaSetBoothTexture`. A migrated window runs the reference's
        // own file, which declares a bare `<PlayerModel>` and no Texture at all — so the widget
        // itself has to draw, or the character sheet's paper doll is an empty rectangle.
        //
        // A FILE pane is a tile (2013; the map arrow among them since 2015); a UNIT pane joins
        // the booth its window keeps. A pane with no name, or one no window claims, draws
        // nothing. Nothing is stubbed white.
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
            // A FILE pane (decision 2008): publish what the tile renderer needs — the pane's
            // device-pixel size, the reference's unit ladder off it, and where the composite
            // goes (its rect, paint key, alpha and clip). The request is idempotent, so the
            // memoized conversion may re-publish it freely; which panes draw is the engine's
            // paint list, and the quad itself is the renderer's per-frame output
            // (`ui_models::compose_tiles`, decision 2023) — NOT pushed here, because a cell
            // packed after this conversion would wait on a re-conversion nothing triggers.
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
                        // `1 model unit = 1280 · modelScale · layoutScale` FrameXML units
                        // (`0x76d1a0`).
                        px_per_unit: 1280.0 * model_scale * layout * s * dpi,
                        // `SetPosition` is in layout units: `768 · √(a²+1)` FrameXML per unit.
                        pos_px_per_unit: 768.0 * diag * layout * s * dpi,
                        // A particle's half-extent: eye space, neither scale (`0x7b2a50`).
                        star_px_per_unit: 768.0 * diag * s * dpi,
                        facing,
                        position: Vec3::new(position.0, position.1, position.2),
                        // The PERSPECTIVE leg's root (decision 2027), which is in model units,
                        // not pixels: `T(pos · layoutScale) · R(facing) · S(s)` with
                        // `s = G48·(5/3)·modelScale·layoutScale` — and `G48·(5/3)` is exactly
                        // `√((4/3)²+1)/√(a²+1)`, the 4:3 renormalizer (`0x80655c`). The
                        // camera is carried through the same matrix, so both terms cancel for
                        // framing; they are here because the record's near/far are NOT scaled
                        // with them, and because the geometry has to be drawn somewhere.
                        root_scale: (5.0 / 3.0) / diag * model_scale * layout,
                        root_pos: Vec3::new(position.0, position.1, position.2) * layout,
                        camera,
                        light,
                        fog,
                        icon: icon.clone(),
                        rect,
                        z_key: eq.z,
                        // The instance draws at the widget's OWN alpha (`0x76d120`).
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
            // The aspect the bake must render at, and the fact that it is on screen at all
            // (decision 1069) — published before the readiness check below for the same reason the
            // `portrait_unit` arm does it there: a pane whose bake has not landed yet is still a
            // pane being drawn, and gating the publish on the image would be a standoff.
            if rect.height() > 0.0 {
                booths
                    .panes
                    .0
                    .insert(slot.to_string(), rect.width() / rect.height());
            }
            // The bake is a render target and carries PREMULTIPLIED colour; the 2D stand-in the
            // slot shows while a model streams is an ordinary straight-alpha BLP.
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
            // A live unit portrait (`SetPortraitTexture(region, unit)`): sample this token's
            // source ([`crate::portrait::PortraitImages`]) — the off-screen model bake, or the
            // ref's 2D TemporaryPortrait stand-in while the model streams in. Absent entry (no
            // booth yet) draws nothing rather than the run-splitter's white default.
            if let Some(token) = &portrait_unit {
                use crate::portrait::PortraitSource;
                // Publish the rect's aspect so a booth can bake at the shape it will be
                // stretched into, and know it is on screen at all (decision 1069). Recorded
                // before the readiness `continue` below — a pane whose bake hasn't landed yet is
                // still a pane being drawn, which is also what keeps the gate below from being a
                // chicken-and-egg (nothing drawn → no bake → nothing drawn). The region's rect is
                // the whole answer because no pane crops its bake; a pane that grew `<TexCoords>`
                // would have to fold that UV window in here too.
                //
                // ROUND bindings are recorded too, since 1576. The aspect is inert for them (both
                // of 1069's consumers are body-booth-only — see [`crate::portrait::BoothPanes`]);
                // what the row carries for a round slot is the fact of being drawn, which is the
                // `"targettarget"` portrait's cost gate. The `circular` flag itself still decides
                // the draw below, and nothing else changed here.
                if rect.height() > 0.0 {
                    booths
                        .panes
                        .0
                        .insert(token.clone(), rect.width() / rect.height());
                }
                // The bake is a render target and carries PREMULTIPLIED colour; the 2D stand-in
                // is an ordinary straight-alpha BLP. The quad pass has to be told which, or it
                // premultiplies the bake a second time and erases every effect the pane draws
                // over empty space (see [`crate::ui_pass::UiQuad::premultiplied`]).
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
                    // A portrait binding honours `<TexCoords>`/`SetTexCoord` like any other
                    // texture region — the bake is just the sampled image. The ref crops one
                    // this way: the character micro button samples the same portrait slot as
                    // the unit frame through a narrow (0.2..0.8, 0.0666..0.9) window, and
                    // swaps that window when the button is pushed. This branch used to pin
                    // UvRect::FULL, which silently squashed the whole square bake into
                    // whatever rect the region had.
                    uv: match tex_coords {
                        Some(TexCoords::Rect(edges)) => UvRect::from_tex_coords(edges),
                        Some(TexCoords::Corners(corners)) => UvRect::from_corners(corners),
                        None => UvRect::FULL,
                    },
                    color: [1.0, 1.0, 1.0, eq.alpha],
                    // The binding's mask flag: `SetPortraitTexture` regions cut the inscribed
                    // circle (the ref stamps the same shape into its 64² bake's alpha — and
                    // rounds the square stand-in art the same way); `BenillaSetBoothTexture`
                    // (the paper-doll model pane, decision 0208 §5) samples the bake square.
                    circular,
                    premultiplied,
                    clip,
                    ..default()
                });
                return;
            }
            // An unset texture region (no file, no color — e.g. a cleared icon) draws nothing;
            // defaulting it to white would paint phantom quads.
            if path.is_none() && color.is_none() {
                return;
            }
            // A portrait region samples the circular-masked variant so the square icon/model
            // doesn't poke past the frame ring's thin band (SetPortraitToTexture, decision 0084).
            //
            // A path the archives don't have draws **nothing** — never a white slab. This arm
            // used to fall through to `color.unwrap_or(WHITE)` with a `None` texture, which
            // `ui_pass` renders as the shared 1×1 white image tinted white: an opaque white
            // rectangle at the region's rect, which is how B221's macro icons reached the
            // director's screen. What the reference does: `TextureCreate` does
            // build an 8×8 placeholder, but `CSimpleTexture::SetTexture` (`0x770200`) checks the
            // status severity and at ≥2 releases it and returns **without touching the widget's
            // texture** — the widget keeps what it had, and Lua gets `nil`. Nothing goes white.
            // We can't keep the *previous* art (a path is re-resolved per frame at extract, not
            // latched at `SetTexture`), so the faithful-enough result is an empty cell; what
            // matters is that it is never a phantom quad. The `Backdrop` and live-portrait arms
            // already guard exactly this way.
            //
            // `assets` missing ENTIRELY is a data-less run (the headless UI tests), not a bad
            // path — those keep the old behaviour rather than blanking every textured quad.
            //
            // `<TexCoords>`/`SetTexCoord` slices the sampled sub-rect. The 4-edge form
            // `[left,right,top,bottom]` maps to raw UV corners (`left→u0, right→u1, top→v0,
            // bottom→v1`) — carried through [`UvRect`] rather than a normalized `Rect` so a
            // mirrored slice (`left>right`, e.g. PlayerFrameTexture) keeps its flip to the
            // vertex buffer. The 8-arg affine form is already per-corner in the `push_quad`
            // winding (the route-line quads). Absent = the full texture. Resolved BEFORE the
            // handle because the wrap mode follows it:
            let uv = match tex_coords {
                Some(TexCoords::Rect(edges)) => UvRect::from_tex_coords(edges),
                Some(TexCoords::Corners(corners)) => UvRect::from_corners(corners),
                None => UvRect::FULL,
            };
            // A slice that runs PAST the texture is the reference's tiling idiom, not a crop:
            // `SetTexCoord(0, n, 0, 1)` on an n-slots-wide strip repeats the art n times — the
            // stance shelf's middle carries one slot per extra form exactly that way
            // (stock `ShapeshiftBar_Update`). Clamp-sampled it smears the last column
            // across the extra width instead. Clamp/repeat bake into the `Image`, so this picks
            // a wrapped GPU image + cache entry — wrapped on the tiling axis ALONE
            // (`tiling_axes`, decision 2000): the other axis spans the whole texture, and a
            // `Repeat` sampler there is a bleed, not a tile — bilinear at `v = 0` weighs in the
            // texture's last row, which on `ShapeshiftBarMiddle` is opaque grey, so every
            // four-form stance bar drew a one-px grey hairline along the top of its middle
            // strip, over the world. The `Backdrop` arm below keeps both axes on: its bg tiles
            // both ways and its edge crops never reach the image edge (`inset_atlas_bleed`).
            let wrap = tiling_axes(&uv);
            let handle = match (path.as_deref(), assets.as_mut()) {
                (Some(p), Some(a)) => {
                    // The tabard designer's emblem cells (1977): the reference installs a
                    // generated 128×64 / 128×32 image — white RGB carrying the emblem BLP's own
                    // alpha — into the Texture the setter was handed; the region carries that as
                    // a token path and the resolver builds exactly that image.
                    let resolved = if p == benilla_ui::script::nameplate::BORDER_TEXTURE {
                        // **Where the plate actually PAINTED**, on the same `vpl` tag the driver's
                        // own seat line rides (`vplates`'s jitter decomposition). The driver says
                        // where it put the plate; this says where the frame system drew it, in the
                        // same window px — so "is the paint this frame's or last frame's?" is a
                        // diff of two numbers instead of an argument about the schedule. It is the
                        // measurement decision 2168 was taken on, and the one that proves it
                        // stays fixed.
                        if benilla_assets::trace::enabled_for("vpl") {
                            benilla_assets::trace::line(
                                "vpl",
                                &format!(
                                    "paint=({:.1},{:.1})..({:.1},{:.1})",
                                    rect.min.x, rect.min.y, rect.max.x, rect.max.y
                                ),
                            );
                        }
                        // The V-plate's frame art, resampled to the quad's exact PHYSICAL size
                        // with the sharp kernel instead of GPU-magnified (0188 — the director's
                        // "sharpen the same frame", carried across decision 2148's move of the
                        // plate into the frame system). Keyed by that size, so it re-rasterises on
                        // a resize and is a cache hit on every frame in between. An addon that
                        // blanks the border with `SetTexture("")` never reaches here at all: the
                        // path is gone, and the arm above draws nothing.
                        let px = |v: f32| (v * dpi).round().max(1.0) as u32;
                        a.resampled_sprite(
                            p,
                            (px(rect.width()), px(rect.height())),
                            images,
                            crate::vplates::border::resample_sharp,
                        )
                    } else if let Some(blp) = benilla_ui::script::emblem_mask_path(p) {
                        a.emblem_mask_texture(blp, images)
                    } else if circular {
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
            // The atlas-cell guard (decision 1608): a `SetTexCoord` crop is a cell of a sheet,
            // and bilinear magnification reaches past it into the neighbour unless the fragment
            // is told where the cell ends. The world map's zone POIs are the case that forced it
            // — `POIIcons` cell 15 is fully transparent and the cell above it is a coffin whose
            // bottom row is opaque black, so every zone landmark wore a black hairline.
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
                // The engine's SetRotation is counterclockwise-positive; the quad pass spins
                // clockwise-on-screen (`UiQuad::rotation`) — negate to convert.
                rotation: -rotation,
                desaturated,
                ..default()
            });
        }
        // The colour picker's hue disc: a generated image (there is no BLP that is a colour
        // wheel), tinted by the widget's value. See [`colorselect`] for why one static image
        // covers every colour.
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
        // Its brightness strip: one greyscale ramp tinted to the live hue at full value, which is
        // exactly `rgb(h, s, v)` at every row.
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
            // A frame Backdrop piece (bg or one of the 8 border pieces). Repeat-sampled when
            // `tile` (edges tile past UV 1; a tiled bg wraps) — its own GPU image + cache
            // (`sprite_texture_tiled`) since clamp/repeat bake into the `Image`. The four UVs are
            // explicit per-corner (the rotated TOP/BOTTOM edges), so they go straight to `UvRect`.
            let handle = assets.as_mut().and_then(|a| {
                if tile {
                    a.sprite_texture_tiled(&path, images)
                } else {
                    a.sprite_texture(&path, images)
                }
            });
            // No texture (missing BLP) ⇒ draw nothing rather than a phantom solid quad.
            let Some(handle) = handle else { return };
            // The 8 border pieces share one atlas, so bilinear at a piece's own edge blends in the
            // NEIGHBOURING piece's first column unless the UVs are pulled half a texel inward
            // (1402). `tile` is exactly the border-piece predicate here — every one of the 8 sets
            // it, and a stretched bg (the only `tile: false` piece) is a whole texture, not a
            // slice of one, so it is left alone.
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
            // No atlas (no client data / Friz Quadrata unreadable — see `ui_text`'s startup
            // system) means text simply doesn't render, same graceful-absence posture as a
            // missing `WorldAssets`. The rasterization itself (editbox window, ellipsis,
            // shadow, links, caret) lives in `text::emit`.
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

/// One pass of the FontString measure round-trip: hand every unmeasured FontString to the font
/// engine and push the answers back. Returns whether anything was measured — the caller's gate for
/// its backstop pass (see the call site for why the order matters).
fn measure_fontstrings(
    script: &mut UiScript,
    atlas: &mut UiFontAtlas,
    s: f32,
    ui_cost: &mut super::UiFrameCost,
    // The churn column is the recorder's alone ([`super::UiCostWanted`]) — the `[ui-cost]` line
    // reports counts, not the strings behind them, and collecting them is not free.
    record_texts: bool,
) -> bool {
    let requests = script.fontstrings_needing_measure();
    // The recorder's churn column: WHICH strings a frame had to re-shape. A steady hover that
    // keeps asking is the whole question (`hover_log`), and the answer is a string, not a count —
    // so the first few come along by name.
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
    // Every answer goes through the ONE measure body ([`crate::ui_text::measure_request`]) the VM's
    // own synchronous measurer calls, against the same shared engine — so a string measured here
    // and the same string measured mid-tick are not two computations that agree, they are one.
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

/// The held-cursor icon quad (CAPTURE-ONLY — see the module doc): a 32×32 icon TOP-LEFT anchored
/// at the mouse (`pos`, y-down logical px — the same space as the extracted quad rects and the
/// hardware cursor's own coordinate origin), `[pos, pos + 32]` — matching the hardware cursor's
/// `(0, 0)` hotspot (decision 0216 §5): the pointer sits at the icon's top-left corner and the
/// icon hangs down-right, the reference look, rather than centering on the mouse. Seated above the
/// whole UI (`z_key = u64::MAX` sorts last → drawn on top). Pure geometry so it's
/// machine-checkable without a live mouse.
fn cursor_icon_quad(pos: Vec2, texture: Handle<Image>) -> UiQuad {
    const SIZE: f32 = 32.0;
    UiQuad {
        rect: Rect::new(pos.x, pos.y, pos.x + SIZE, pos.y + SIZE),
        z_key: u64::MAX,
        texture: Some(texture),
        ..default()
    }
}

/// The atlas-cell guard's law (decision 1608) — see [`uv_clamp_window`]. Each case is a shape the
/// shipped UI actually draws, and the three that return `None` are the three ways a crop is not a
/// cell.
#[cfg(test)]
mod uv_clamp_tests {
    use super::{tiling_axes, uv_clamp_window, UvRect};

    /// The bug's own numbers: `POIIcons` is 128², a world-map POI samples cell (7,1), and the
    /// window has to stop half a texel (`0.5/128`) inside it — a hair below texel row 16's centre
    /// is where the coffin above stopped leaking in.
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

    /// The whole texture is already clamped by the sampler — a window here would only cost a
    /// batch split.
    #[test]
    fn the_whole_texture_asks_for_no_window() {
        assert!(uv_clamp_window(&UvRect::FULL, (128, 128)).is_none());
    }

    /// The stance shelf's middle strip past two forms: `SetTexCoord(0, n-2, 0, 1)` tiles along u
    /// and spans the whole texture in v — only u wraps. Both axes `Repeat` was the four-form
    /// bar's hairline (decision 2000).
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

    /// Three forms: `SetTexCoord(0, 1, 0, 1)` is the whole texture — nothing tiles, the plain
    /// clamped sprite serves it. And a crop inside the texture is a cell, never a tile.
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

    /// A mapping past the texture on both axes (a tiled backdrop bg) wraps both — the tiled
    /// image's case, unchanged.
    #[test]
    fn a_two_axis_tile_wraps_both() {
        assert_eq!(
            tiling_axes(&UvRect::from_tex_coords([0.0, 4.0, 0.0, 2.5])),
            (true, true)
        );
    }

    /// `SetTexCoord(0, n, 0, 1)` on an n-slot strip is the reference's TILING idiom (the stance
    /// shelf): the repeating axis keeps its exact period, the bounded one still gets its window.
    #[test]
    fn a_tiling_axis_is_left_alone_while_its_bounded_partner_is_not() {
        let uv = UvRect::from_tex_coords([0.0, 3.0, 0.0, 0.5]);
        let w = uv_clamp_window(&uv, (64, 64)).expect("the v axis is a bounded crop");
        assert!(w[0] > w[2], "u tiles, so its axis must read as OFF: {w:?}");
        assert!((w[1] - 0.5 / 64.0).abs() < 1e-6);
        assert!((w[3] - (0.5 - 0.5 / 64.0)).abs() < 1e-6);
    }

    /// A mirrored slice (`left > right` — the PlayerFrame ring) is still one cell: the window is
    /// built from the corner EXTENTS, so the flip survives it untouched.
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

    /// A crop thinner than one texel has no interior to inset toward — both bounds pin to its
    /// centre, which is the single texel it means (and stays a VALID, enabled range).
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
        // 32×32, top-left anchored on the mouse (the hardware cursor's (0,0) hotspot) — hangs
        // down-right: [100..132, 200..232].
        assert_eq!(
            (q.rect.min.x, q.rect.min.y, q.rect.max.x, q.rect.max.y),
            (100.0, 200.0, 132.0, 232.0)
        );
        assert_eq!(q.rect.width(), 32.0);
        assert_eq!(q.rect.height(), 32.0);
        // Above every frame, and textured (straight-alpha, not additive).
        assert_eq!(q.z_key, u64::MAX);
        assert!(q.texture.is_some());
        assert!(!q.additive);
    }
}

/// The app-side half of the ScrollFrame clip plumb (decision 0112): does a [`UiQuad`] built from a
/// clipped [`QuadContent::Texture`] actually carry `clip` through the UI pass? Drives the real
/// systems in a minimal headless `App` (no `DefaultPlugins` — just the resources the pass
/// reads), rather than re-deriving the y-up→y-down flip by hand: that flip is exactly the seam this
/// test exists to catch a regression in.
#[cfg(test)]
mod clip_plumb_tests {
    use bevy::math::Rect;
    use bevy::prelude::*;
    use bevy::window::PrimaryWindow;

    use benilla_ui::script::UiScript;

    use super::{paint_script, tick_script, UiQuad, UiQuads};
    use crate::portrait::PortraitImages;

    /// A headless app with exactly the resources/entities the UI pass reads: a `NonSend` VM
    /// carrying a ScrollFrame + scrolled-out child + a colored (pathless) Texture region marker, a
    /// 1024×768 primary window (the 768-virtual design height — s = 1, so the WoW-space rects are
    /// known by hand; decision 0582), and every other resource
    /// left at its default (`WorldAssets`/`UiFontAtlas` absent — no BLP/font pipeline needed to prove
    /// the clip carries through a plain colored quad).
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
            marker:SetAllPoints()               -- templateless Lua region: no implicit anchor (1310)
        "#,
            )
            .unwrap();

        let mut app = App::new();
        app.insert_non_send_resource(script);
        app.init_resource::<UiQuads>();
        // The pass's own handover (2168) — the tick half writes it, the paint half reads it.
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
        // The uiScale dial at its identity Default (1.0) — tests pin the byte-identity base;
        // the shipped app inserts the taste default instead (decision 0584).
        app.init_resource::<crate::ui_script::UiScaleCvar>();
        app.world_mut().spawn((
            Window {
                resolution: UVec2::new(1024, 768).into(),
                ..default()
            },
            PrimaryWindow,
        ));
        // The pass is two systems since 2168, and a test that drives it drives BOTH — the quads are
        // the second half's.
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

        // WoW space: frame rect [bottom 468, left 0, top 668, right 300]; the quad pass is y-down
        // from the top-left, so `y_down = window_height - y_up`: bottom(468)->300, top(668)->100.
        assert_eq!(
            marker.clip,
            Some(Rect::new(0.0, 100.0, 300.0, 300.0)),
            "the engine's ScrollFrame clip survives the y-up→y-down flip into UiQuad::clip"
        );
    }

    /// The uiScale dial (decision 0584) through the same plumb, by hand: at dial 0.5 on the same
    /// 1024×768 window, s = 0.5 and the VM sees a 2048×1536 virtual screen — the TOPLEFT-anchored
    /// frame hangs from virtual top 1536, and every window-px rect halves.
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

        // WoW space: frame top = 1536 − 100 = 1436, bottom 1236, right 300. ×0.5 → px y-up
        // [618..718, 0..150]; y-down through the 768 window: top 768−718 = 50, bottom 768−618 = 150.
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
            m:SetAllPoints()  -- templateless Lua region: no implicit anchor (1310)
        "#,
            )
            .unwrap();

        let mut app = App::new();
        app.insert_non_send_resource(script);
        app.init_resource::<UiQuads>();
        // The pass's own handover (2168) — the tick half writes it, the paint half reads it.
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
        // The pass is two systems since 2168, and a test that drives it drives BOTH — the quads are
        // the second half's.
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

/// The extract gate (decision 0740): a settled frame skips the whole conversion loop, and — the
/// dangerous direction — any extract-visible change must reopen it, INCLUDING paint-only writes
/// that never dirty the layout gate. Uses the same minimal headless app as `clip_plumb_tests`.
/// The alignment merge ([`align_entries`]) — the half of the splice that sees through a shift.
///
/// The cases are built from a REAL extract list, because `FrameHandle` keeps its fields private
/// on purpose: an entry's `z` is a packed [`benilla_ui::order::ZKey`] and hand-forging one would
/// test a fiction rather than the traversal's own keys.
#[cfg(test)]
mod align_tests {
    use benilla_ui::script::{ExtractedQuad, QuadContent, UiScript};

    use super::align_entries;

    /// Four regions across two frames — a z-sorted list with genuine keys to permute.
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

    /// A repaint of one entry: it converts, its old self is dropped, and every other entry keeps
    /// its own quads — the shape the splice has always handled, restated on the new machinery.
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

    /// **The case a positional compare cannot see** (decision 1638): one entry appears in the
    /// middle and every entry behind it shifts by one. All of them must keep their quads —
    /// before this, index `j` was compared against a stranger and the whole list re-converted.
    #[test]
    fn an_insertion_shifts_the_tail_and_costs_one_conversion() {
        let was = entries();
        // The insertion needs a `z` strictly between its neighbours' — the merge reads a sorted
        // list — so seat it at the first pair with room. Sibling regions differ by 1 in the key's
        // declaration-order field; a frame boundary leaves a wide gap.
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

    /// The mirror: an entry vanishes, nothing converts at all, and the tail keeps its quads from
    /// one place forward. The splice must still run — the quads have to leave the list.
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

    /// Two lists with nothing in common align to nothing kept — the bound above this call is
    /// what then sends the frame to the full conversion.
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
            marker:SetAllPoints()  -- templateless Lua region: no implicit anchor (1310)
        "#,
        )
    }

    /// The same headless app around an arbitrary boot script. The splice tests boot a SECOND
    /// app straight into a mutated app's final state: its first frame is by construction the
    /// full conversion the spliced output must equal.
    fn app_from_script(lua: &str) -> App {
        let script = UiScript::new().unwrap();
        script.run(lua).unwrap();
        let mut app = App::new();
        app.insert_non_send_resource(script);
        app.init_resource::<UiQuads>();
        // The pass's own handover (2168) — the tick half writes it, the paint half reads it.
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
        // The pass is two systems since 2168, and a test that drives it drives BOTH — the quads are
        // the second half's.
        app.add_systems(Update, (tick_script, paint_script).chain());
        app
    }

    #[test]
    fn a_settled_frame_skips_and_a_paint_write_reopens_the_gate() {
        let mut app = app_with_marker();
        app.update(); // frame 1: builds the quads
        assert!(app.world().resource::<UiQuads>().dirty, "first frame draws");
        app.world_mut().resource_mut::<UiQuads>().dirty = false;

        app.update(); // frame 2: identical inputs — the gate must leave the resource alone
        assert!(
            !app.world().resource::<UiQuads>().dirty,
            "a settled frame must not re-mark the quads dirty"
        );

        // A PAINT-ONLY write: invisible to the layout gate (no anchors/size moved), so only the
        // extracted-list compare can reopen extraction. If the gate wrongly ate it, the marker
        // would stay red on screen.
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

    /// **A `<PlayerModel>` pane draws its window's booth, and an unclaimed one draws nothing**
    /// (decision 1810).
    ///
    /// The engine verb the stock character sheet needed. `PaperDollFrame.xml` declares a bare
    /// `<PlayerModel name="CharacterModelFrame">` with no texture in it at all — our own deleted
    /// file put a `<Texture>` there and named the booth with `BenillaSetBoothTexture` — so without
    /// this the migrated window's paper doll is an empty rectangle, which no loader error and no
    /// missing global would have said a word about.
    ///
    /// Both halves are asserted because the second is what keeps the first honest: the join is a
    /// table of frame names ([`crate::portrait::model_pane_booth`]), and a pane no window claims —
    /// pfUI's anonymous `CreateFrame("Model")` autocast shine is the corpus case — must draw
    /// nothing rather than borrow somebody's bake.
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
        // …and the pane published its aspect, which is what lets the booth render at the shape it
        // will be stretched into (decision 1069) and what tells it it is on screen at all.
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

    /// **A booth bake reaches the quad pass flagged PREMULTIPLIED, an ordinary region does not**
    /// (decision 1347). This is the wiring half of the paper-doll/dressing-room fix, and it is the
    /// half that can silently rot: the shader's `select(a, k, premultiplied)` is only correct if
    /// exactly the render-target quads carry the flag. Drop it and every additive effect the pane
    /// draws over EMPTY space is multiplied by its own zero alpha again — the R14 pauldrons' fire
    /// gone, a weapon glow chopped at the model's silhouette (Goudy, `#bugs` 2026-07-27).
    #[test]
    fn a_booth_bake_quad_is_flagged_premultiplied_and_a_plain_one_is_not() {
        let mut app = app_with_marker();
        // The paper doll's own binding — the square booth pane (`BenillaSetBoothTexture`).
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

    /// The per-entry splice, in-place half: a one-entry paint write must ride the splice
    /// (`UiFrameCost::spliced == 1` — nonzero is the proof the path FIRED, without which this
    /// test is vacuous) and produce byte-identically what the full conversion produces —
    /// checked against a fresh app booted straight into the final state.
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
            marker:SetAllPoints()  -- templateless Lua region: no implicit anchor (1310)
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
            marker:SetAllPoints()  -- templateless Lua region: no implicit anchor (1310)
        "#,
        );
        reference.update();
        assert!(
            app.world().resource::<UiQuads>().quads
                == reference.world().resource::<UiQuads>().quads,
            "post-stitch splice output equals the full conversion"
        );
    }

    /// **A frame appearing shifts every entry behind it, and the splice sees through that**
    /// (decision 1638). Showing a hidden sibling inserts its entries into the MIDDLE of the
    /// render list, so every entry after them lands at a new index. Compared index-wise they all
    /// looked changed and the frame took the full conversion — which is what 86 % of hover frames
    /// were doing. Both directions, and both must equal the full conversion of the same model.
    #[test]
    fn a_shown_sibling_splices_through_the_shift_and_matches_the_full_conversion() {
        // A, B, C in declaration order, so B's entries sort between A's and C's: C is the tail
        // that shifts. B starts hidden and contributes nothing.
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
        // The reference must share the app's HISTORY, not just its end state: `Show()` on a
        // hidden frame re-stacks it to the tail of its draw bucket — the client's own
        // effective-visibility show `0x76ae10` re-adding it to the level's intrusive list — so B
        // draws above C afterwards. (The splice got that right on its own; this reference did
        // not, which is how the difference surfaced.)
        let mut reference = app_from_script(&format!("{BUILD}\nhidden:Hide()\nhidden:Show()"));
        reference.update();
        assert!(
            app.world().resource::<UiQuads>().quads
                == reference.world().resource::<UiQuads>().quads,
            "the spliced list equals the full conversion of the same model"
        );

        // ── Deletion ─────────────────────────────────────────────────────────────────────────
        // The mirror, and the reason `dropped` exists: nothing converts at all, so `spliced` is
        // zero on a frame that most certainly did splice.
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

    /// A raster-environment change (window resize) must pin the conversion to the FULL path —
    /// the splice's held quads were rasterized under the old seam and every one of them is
    /// wrong-sized (decision 1342's edge, applied to the splice).
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
