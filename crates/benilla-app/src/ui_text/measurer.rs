//! **The font engine, as the script VM holds it** — the app's half of
//! [`benilla_ui::script::TextMeasure`].
//!
//! The engine seam (`benilla-ui`'s `script::measure`) exists because the reference answers
//! `GetStringWidth` from its font engine *inline* and ours could not: measuring needed the atlas,
//! and the atlas only met the VM at the frame boundary. What closes the gap is that the client's
//! width law is a **table**, not a computation — a sum of per-glyph steps with no kerning and no
//! neighbour term — so a measure needs only the face, the raster size, and the characters.
//!
//! **The same engine, not a copy of it.** This holds the very [`TextEngine`] the render path holds,
//! behind the same lock, so the answer a Lua call gets mid-tick and the answer the extract pass
//! computes are not merely equal — they are the same lookup in the same table. That property used
//! to be bought by making the table immutable and `Arc`-sharing it; since decision 1342 the table
//! fills on demand, so the sharing has to carry mutation, and the lock is how. A measure fills only
//! the **metrics** half ([`TextEngine::ensure_metrics`]): a string that is measured and never drawn
//! costs a shaping, never a raster and never a texture page.
//!
//! **One body, both callers.** [`measure_request`] is the whole of it; the batch pass
//! (`ui_script::extract::measure_fontstrings`) and [`AtlasMeasurer`] both call it and neither has
//! any measuring logic of its own. That is the trait's own contract, kept structurally rather than
//! by discipline.

use std::sync::{Arc, Mutex};

use benilla_ui::script::{MeasureRequest, TextMeasure};

use super::engine::TextEngine;

/// Answer one [`MeasureRequest`]: `(laid_out_w, laid_out_h, natural_w)` in the region's own
/// frame-local units.
///
/// `seam` is the host's screen scale ([`crate::ui_script::seam_scale`]); the request's own `scale`
/// is the owner frame's `effective_scale`. Measuring happens at the product — the exact drawn
/// raster size — and the answer divides the whole product back out, because integer-stepped glyph
/// advances do not commute with scaling (see [`MeasureRequest::scale`]).
pub(crate) fn measure_request(
    e: &mut TextEngine,
    seam: f32,
    r: &MeasureRequest,
) -> (f32, f32, f32) {
    let rs = seam * r.scale;
    let spec = || super::FontSpec {
        path: r.font.as_deref(),
        // The render pass's exact drawn px (two regimes × the full scale) — measure == render, and
        // Lua's GetStringWidth/Height echo the DRAWN size (0x772890); results divide back to
        // frame-local UI units below.
        height: super::drawn_px(r.height, r.text_height, rs),
        // The TRUE outline: THICK biases the client's step law (+1px per glyph — GlyphStepBase
        // 0x5ca2b0, THICK-only), so measure must see it.
        outline: r.outline,
        alpha_gradient: None, // alpha never changes metrics
    };
    let wrap = r.wrap_width.map(|w| w * rs);
    let (w, h) = super::measure_text(e, &r.text, wrap, spec());
    // …and the NATURAL width, which is what `GetStringWidth` answers with (the reference measures
    // its getter's string with no wrap constraint — the extent measure `0x5c6940`). A second pass
    // only for the regions that actually carry a declared width; for the rest the two are one
    // number.
    let natural = if wrap.is_some() {
        super::measure_text(e, &r.text, None, spec()).0
    } else {
        w
    };
    (w / rs, h / rs, natural / rs)
}

/// The measurer installed into the VM — the shared engine plus the host's current screen seam.
///
/// **Rebuilt, not mutated, when the seam moves.** A measurer is only correct for the seam it was
/// made under (the seam multiplies into every requested height before it becomes a raster size), so
/// the host installs a fresh one at exactly the moment it calls
/// [`UiScript::forget_text_metrics`](benilla_ui::script::UiScript::forget_text_metrics) — the same
/// edge, for the same reason. Building one is an `Arc` clone and an `f32`.
pub(crate) struct AtlasMeasurer {
    engine: Arc<Mutex<TextEngine>>,
    seam: f32,
}

impl AtlasMeasurer {
    pub(crate) fn new(engine: Arc<Mutex<TextEngine>>, seam: f32) -> Self {
        Self { engine, seam }
    }
}

impl TextMeasure for AtlasMeasurer {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        // Taken and released inside this call. Nothing on the app side may hold the engine lock
        // across a VM tick, because a tick can land here (see `ui_text::engine`'s lock discipline).
        let mut e = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        measure_request(&mut e, self.seam, req)
    }
}
