//! **The synchronous text measure** — the host's font engine, installed *into* the VM so a
//! `GetStringWidth` answers during the Lua call that asked, not a frame later.
//!
//! ## Why this exists
//!
//! The real client's `GetStringWidth 0x79e510` calls straight through to its font engine
//! (`0x772890`) and returns. Ours could not: measuring needs the baked glyph atlas, the atlas
//! lives in the app, and the app only meets the VM at the frame boundary — so the metric was
//! served from the [`MeasureRequest`](super::MeasureRequest) round-trip, which lands at **extract,
//! one frame after the tick that set the text**. Every same-tick `SetText` → `GetStringWidth` pair
//! read **0**.
//!
//! That is not a rounding difference, it is the difference between a laid-out UI and a broken one,
//! and it is the shape the corpus actually writes:
//!
//! ```lua
//! button:SetText(player)                      -- Bagnon_Forever/database/ui.lua:58
//! if button:GetTextWidth() + 40 > width then   -- …:59, same tick
//! ```
//!
//! With the answer 0, that dropdown sized itself to 40px and seven character names drew outside
//! their own frame. The reference's own `SmallMoneyFrame` does the same thing (`MoneyFrame.lua`
//! l.202), which is why `UiScript::set_digit_advances`
//! existed: a digits-only stand-in for exactly this, built because the general answer was missing.
//! This is the general answer, and it retires that special case.
//!
//! ## The seam
//!
//! [`TextMeasure`] is one method over the request type the round-trip already speaks, so the host
//! answers a same-tick measure and a batch measure with the same code — there is no second
//! measuring path to drift. The host owns the engine; the engine owns the *when*.
//!
//! **Absent by default, and that is a supported state.** A VM with no measurer installed (every
//! engine test, every headless run without a font atlas) behaves exactly as it did: metrics stay
//! 0 until the round-trip fills them. Nothing here is load-bearing for correctness of the
//! round-trip; it only removes the latency.

use mlua::Lua;

use super::{MeasureRequest, MeasuredText, Model};
use crate::widget::RegionHandle;

/// The host's font engine, as the VM sees it — installed with
/// [`UiScript::set_text_measurer`](super::UiScript::set_text_measurer).
///
/// One method, taking the same [`MeasureRequest`] the batch round-trip carries: the host must
/// answer a synchronous measure with the identical code that answers
/// [`UiScript::fontstrings_needing_measure`](super::UiScript::fontstrings_needing_measure), or the
/// two disagree and a string's width depends on *when* it was asked for.
pub trait TextMeasure {
    /// `(laid_out_w, laid_out_h, natural_w)` for `req`, in the region's own frame-local units.
    ///
    /// `laid_out_*` is the text wrapped inside `req.wrap_width` when it has one; `natural_w` is
    /// the width it would take **unwrapped**, which is the number `GetStringWidth` reports (see
    /// [`MeasuredText::natural_w`]). For a request with no wrap width the two are one number.
    ///
    /// Must be **pure with respect to the VM** — it is called with the model mutably borrowed and
    /// must not re-enter Lua.
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32);
}

/// Measure region `rh` **now** if its stored measure is stale and a measurer is installed.
///
/// The one entry point for the metric reads (`GetStringWidth`, `GetWidth`/`GetHeight` on a
/// FontString): they already treat a key-mismatched measure as absent, so filling the cache here
/// is invisible to them beyond the answer arriving. A no-op without a measurer, and a no-op when
/// the stored measure is already current — which is what keeps a per-frame poll from re-measuring.
pub(super) fn ensure_measured(lua: &Lua, rh: RegionHandle) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if model.measurer.is_none() {
        return;
    }
    let Some(req) = super::layout::measure_request_for(&mut model, rh) else {
        return; // not a FontString, no text, or the stored measure is already this string's
    };
    // Taken out for the call and put straight back: the measurer needs `&mut self` (a font shaper
    // is stateful) while the model is already mutably borrowed. Safe because the trait forbids
    // re-entering the VM, so nothing can observe the gap.
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
        // The KEY always lands — this IS the measure cache, and a stale key re-requests forever.
        d.measured = Some(new);
    }
    if moved {
        // Measured extents are the auto-size axes' inputs — the layout gate's read set, the
        // same touch `set_measured_text` does. Guarded on a change the LAYOUT can see: a
        // re-measure that returns the same box (a same-width countdown tick) must not open
        // tier 1, or it costs a whole-roster hash to conclude nothing moved.
        // A new extent moves no edge and no roster membership, so it names its node (1388).
        model.touch_layout_region(rh);
    }
}

impl super::UiScript {
    /// Install the host's font engine, so metric reads answer inside the tick that asked.
    ///
    /// Call again to replace it — which the host must do whenever its **raster environment**
    /// changes (window resize, `uiScale`), for the same reason
    /// `UiScript::forget_text_metrics` exists: glyph
    /// advances step to whole *physical* pixels, so a measurer built under one seam does not
    /// answer for another.
    pub fn set_text_measurer(&mut self, measurer: Box<dyn TextMeasure>) {
        self.model_mut().measurer = Some(measurer);
    }

    /// Whether a [`TextMeasure`] is installed — the host's gate for skipping its own batch measure
    /// pass, which would find nothing to do anyway.
    pub fn has_text_measurer(&self) -> bool {
        self.model_mut().measurer.is_some()
    }

    /// Answer every pending FontString measure inline. Returns whether anything changed (i.e.
    /// whether the layout needs another solve). No-op without a measurer.
    ///
    /// This is [`UiScript::resolve`](super::UiScript::resolve)'s own half of the round-trip: with
    /// an engine installed the VM closes the loop itself, so a FontString's box is right in the
    /// frame its text was set rather than the frame after — and the host's batch pass finds an
    /// empty request list and does nothing.
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
