//! The synchronous text measure: the host's font engine installed into the VM, so
//! `GetStringWidth` answers inside the Lua call that asked, as the reference's does (`0x79e510`
//! calls its font engine, `0x772890`). UI code measures in the tick it sets text
//! (`MoneyFrame.lua:202`). With no measurer installed, metrics stay 0 until the
//! [`MeasureRequest`](super::MeasureRequest) round-trip fills them a frame later.

use mlua::Lua;

use super::{MeasureRequest, MeasuredText, Model};
use crate::widget::RegionHandle;

/// The host's font engine as the VM sees it. It must answer with the same code as the batch
/// round-trip, or a string's width depends on when it was asked for.
pub trait TextMeasure {
    /// `(laid_out_w, laid_out_h, natural_w)` for `req` in frame-local units; `natural_w`, the
    /// unwrapped width, is what `GetStringWidth` reports. It runs with the model mutably borrowed,
    /// so it must not re-enter Lua.
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32);
}

/// Measure region `rh` now if a measurer is installed and its stored measure is stale; a current
/// one is left alone, so a per-frame poll does not re-measure.
pub(super) fn ensure_measured(lua: &Lua, rh: RegionHandle) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if model.measurer.is_none() {
        return;
    }
    let Some(req) = super::layout::measure_request_for(&mut model, rh) else {
        return; // not a FontString, no text, or already current
    };
    // Taken out and put back: the measurer needs `&mut self` while the model is borrowed, and it
    // cannot re-enter the VM, so nothing sees the gap.
    let mut engine = model.measurer.take().expect("checked above");
    let (w, h, natural_w) = engine.measure(&req);
    model.measurer = Some(engine);
    let new = MeasuredText {
        w,
        h,
        natural_w,
        key: req.key,
    };
    let mut moved = false;
    if let Some(d) = model.region_data.get_mut(&rh) {
        moved = super::types::MeasuredText::layout_moved(d.measured, new);
        // The key always lands: a stale key re-requests forever.
        d.measured = Some(new);
    }
    if moved {
        // Only a box that moved touches the layout, as in `set_measured_text`: a same-width
        // re-measure (a countdown tick) must not cost a whole-roster hash.
        model.touch_layout_region(rh);
    }
}

impl super::UiScript {
    /// Install the host's font engine, so metric reads answer inside the tick that asked. Replace
    /// it whenever the window size or `uiScale` changes: glyph advances snap to whole physical
    /// pixels.
    pub fn set_text_measurer(&mut self, measurer: Box<dyn TextMeasure>) {
        self.model_mut().measurer = Some(measurer);
    }

    /// Whether a [`TextMeasure`] is installed; the host then skips its batch measure pass.
    pub fn has_text_measurer(&self) -> bool {
        self.model_mut().measurer.is_some()
    }

    /// Answer every pending FontString measure inline, so a box is right in the frame its text
    /// was set; returns whether anything changed.
    pub(super) fn fill_measures(&mut self) -> bool {
        if !self.has_text_measurer() {
            return false;
        }
        let requests = self.fontstrings_needing_measure();
        if requests.is_empty() {
            return false;
        }
        let answers: Vec<(u32, f32, f32, f32, u64)> = {
            let mut model = self.model_mut();
            let mut engine = model.measurer.take().expect("checked above");
            let answers = requests
                .iter()
                .map(|r| {
                    let (w, h, natural_w) = engine.measure(r);
                    (r.id, w, h, natural_w, r.key)
                })
                .collect();
            model.measurer = Some(engine);
            answers
        };
        self.set_measured_text(&answers);
        true
    }
}
