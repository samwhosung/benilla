//! The font engine as the script VM holds it: the app's half of
//! [`benilla_ui::script::TextMeasure`], so `GetStringWidth` answers inline, as in the reference.
//! It holds the very [`TextEngine`] the render path holds, behind the same lock, and fills only
//! the metrics half ([`TextEngine::ensure_metrics`]). [`measure_request`] is the one measuring
//! body: the batch pass (`ui_script::extract::measure_fontstrings`) and [`AtlasMeasurer`] call it.

use std::sync::{Arc, Mutex};

use benilla_ui::script::{EditBoxAdvanceRequest, MeasureRequest, TextMeasure};

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

/// Answer one EditBox advance table as `(cumulative widths, row starts, row pitch)` in screen UI
/// units: measured at the drawn size, the seam times the box's scale, and divided by the seam
/// alone, like the mouse feed. A multi-line box also gets the draw's row starts and pitch. The one
/// body for the extract's round trip and [`AtlasMeasurer`]'s inline answer.
pub(crate) fn editbox_advances(
    e: &mut TextEngine,
    seam: f32,
    req: &EditBoxAdvanceRequest,
) -> (Vec<f32>, Vec<usize>, f32) {
    let spec = super::FontSpec {
        path: req.font.as_deref(),
        height: super::drawn_px(req.height, None, seam * req.scale),
        outline: req.outline,
        alpha_gradient: None, // alpha never changes metrics
    };
    let cum = super::line_advances(e, &req.text, spec)
        .iter()
        .map(|a| a / seam)
        .collect();
    let (rows, cell_h) = match req.wrap_width {
        Some(w) => super::line_rows(e, &req.text, w * seam, spec),
        None => (vec![0], 0.0),
    };
    (cum, rows, cell_h / seam)
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

    fn editbox_advances(
        &mut self,
        req: &EditBoxAdvanceRequest,
    ) -> Option<(Vec<f32>, Vec<usize>, f32)> {
        let mut e = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(editbox_advances(&mut e, self.seam, req))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use benilla_ui::script::UiScript;

    use super::AtlasMeasurer;

    /// A multi-line EditBox is its text's measured height tall (`0x77d4d0` @`0x77d8ad`), one
    /// line's when empty, and the line law the host measures by (`0x5c2070`) folds a trailing
    /// break into the line it ends, so the break alone adds no line.
    #[test]
    fn a_multi_line_boxs_trailing_newline_adds_no_line() {
        benilla_formats::wow_data_or_skip!();
        let engine = super::super::engine::test_engine(1.0).expect("the client faces open");
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.set_text_measurer(Box::new(AtlasMeasurer::new(
            Arc::new(Mutex::new(engine)),
            1.0,
        )));
        s.run(
            r#"
            E = CreateFrame("EditBox", "E")
            E:SetAutoFocus(false); E:SetMultiLine(true)
            E:SetPoint("TOPLEFT", 100, -100); E:SetWidth(270); E:SetHeight(200)
            E:SetFont("Fonts\\MORPHEUS.TTF", 15)
        "#,
        )
        .unwrap();
        s.resolve();
        let mut height = |text: &str| {
            s.run(&format!("E:SetText({text:?})")).unwrap();
            s.tick(0.016);
            s.resolve();
            s.eval::<f32>("return E:GetHeight()").unwrap()
        };
        let empty = height("");
        let one = height("abc");
        assert_eq!(one, empty, "an empty box is one line tall");
        assert_eq!(height("abc\n"), one, "the break alone");
        let two = height("abc\nd");
        assert!(
            (two - 2.0 * one).abs() < 1e-4,
            "the first character on the new line: {two} against {one}"
        );
    }
}
