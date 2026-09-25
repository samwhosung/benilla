//! The font engine as the script VM holds it: the app's half of
//! [`benilla_ui::script::TextMeasure`], so `GetStringWidth` answers inline, as in the reference.
//! It holds the very [`TextEngine`] the render path holds, behind the same lock, and fills only
//! the metrics half ([`TextEngine::ensure_metrics`]). [`measure_request`] is the one measuring
//! body: the batch pass (`ui_script::extract::measure_fontstrings`) and [`AtlasMeasurer`] call it.

use std::sync::{Arc, Mutex};

use benilla_ui::script::{MeasureRequest, TextMeasure};

use super::engine::TextEngine;

/// Answer one [`MeasureRequest`] as `(laid_out_w, laid_out_h, natural_w)` in frame-local units. It
/// measures at the drawn raster size, the host's `seam` ([`crate::ui_script::seam_scale`]) times
/// the owner frame's `effective_scale`, and divides that back out: whole-pixel glyph steps do not
/// commute with scaling.
pub(crate) fn measure_request(
    e: &mut TextEngine,
    seam: f32,
    r: &MeasureRequest,
) -> (f32, f32, f32) {
    let rs = seam * r.scale;
    let spec = || super::FontSpec {
        path: r.font.as_deref(),
        // The render pass's drawn px: `GetStringWidth` and `GetStringHeight` report the drawn
        // size (`0x772890`).
        height: super::drawn_px(r.height, r.text_height, rs),
        // THICK adds 1 px per glyph to the step law (`0x5ca2b0`), so the measure needs it.
        outline: r.outline,
        alpha_gradient: None, // alpha never changes metrics
    };
    let wrap = r.wrap_width.map(|w| w * rs);
    let (w, h) = super::measure_text(e, &r.text, wrap, spec());
    // `GetStringWidth` answers the unwrapped width (the extent measure `0x5c6940`), which only a
    // wrapped region needs a second pass for.
    let natural = if wrap.is_some() {
        super::measure_text(e, &r.text, None, spec()).0
    } else {
        w
    };
    (w / rs, h / rs, natural / rs)
}

/// The measurer installed into the VM: the shared engine and the host's screen seam. It answers
/// only for the seam it was built under, so the host installs a fresh one when the seam moves
/// (`ui_script::extract::seat_text_measurer`).
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
        // Taken and released inside this call: a VM tick can land here, so nothing on the app side
        // may hold the engine lock across one.
        let mut e = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        measure_request(&mut e, self.seam, req)
    }
}
