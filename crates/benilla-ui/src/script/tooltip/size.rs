//! The tooltip sizer, `GameTooltipSize` (`0x530640`), which only the show (`0x530aad`),
//! `AppendText`'s helper (`0x530615`) and the cache answers (`0x52ad59`, `0x52b029`, `0x52e5ae`,
//! `0x52f869`) run: a line writer sizes nothing, so `GetWidth` after `AddLine` answers the last
//! sizing. It measures in the call, as the reference's font strings do.
//!
//! In a VM with no font engine installed a measure is answered a frame later
//! ([`crate::script::MeasureRequest`]). There the sizer leaves the plate's size alone while a
//! measure is owed and runs again at the next resolve ([`size_owed`]); this fallback is benilla's
//! own, as the reference always measures inline.

use super::{apply_anchor, tip_mut};
use crate::layout::{Anchor, Point};
use crate::script::layout::FONTSTRING_MIN_SPAN;
use crate::script::{MeasuredText, Model};
use crate::widget::{
    FrameHandle, KindState, RegionHandle, TooltipAnchor, TOOLTIP_COLUMN_GAP, TOOLTIP_LINE_GAP,
    TOOLTIP_PLATE_PAD, TOOLTIP_WRAP_WIDTH,
};

/// Entries the measure memo holds before it is emptied: thirty lines on each of a few plates,
/// three widths a wrapped line.
const MEMO_CAP: usize = 1024;

/// `GameTooltipSize` (`0x530640`) on tooltip `h`; nothing without an owner (`0x530649`).
pub(in crate::script) fn size_tooltip(model: &mut Model, h: FrameHandle) {
    let Some(KindState::Tooltip(t)) = model.arena.frame(h).map(|f| &f.kind_state) else {
        return;
    };
    let Some(owner) = t.owner else {
        if let Ok(t) = tip_mut(model, h) {
            t.relayout_owed = false;
        }
        return;
    };
    let n = t.num_lines.min(t.left_lines.len()).min(t.right_lines.len());
    let lines: Vec<(RegionHandle, RegionHandle, bool)> = (0..n)
        .map(|i| {
            let wrapped = t.wrapped.get(i).copied().unwrap_or(false);
            (t.left_lines[i], t.right_lines[i], wrapped)
        })
        .collect();
    let (min_width, padding, anchor, (x, y)) = (t.min_width, t.padding, t.anchor, t.owner_offset);
    let mut pending = false;

    // The unwrapped lines (`0x530686`): the left and right widths of the shown cells, and the
    // column gap when both show (`0x5306cf`). The minimum width is the floor (`0x530665`).
    let mut widest = min_width;
    for &(left, right, wrapped) in &lines {
        if wrapped {
            continue;
        }
        let (left_shown, right_shown) = (shown(model, left), shown(model, right));
        let mut w = if left_shown && right_shown {
            TOOLTIP_COLUMN_GAP
        } else {
            0.0
        };
        if left_shown {
            w += get_width(model, left, &mut pending);
        }
        if right_shown {
            w += get_width(model, right, &mut pending);
        }
        // `fcomp; test ah,5; jp` (`0x53075b`): only a larger number replaces, never a NaN.
        if widest < w {
            widest = w;
        }
    }

    // The wrapped lines in order, shown or not (`0x530790`): the text's width capped at 230.4
    // (`0x5307a9`-`0x5307ef`); wider than the widest so far, the text is split there and the widest
    // piece counts (`0x530837`-`0x5308be`). The line is then as wide as the widest so far
    // (`0x5308ef`), so a later, wider line leaves an earlier one narrower.
    for &(left, _, wrapped) in &lines {
        if !wrapped {
            continue;
        }
        let Some(natural) = cached_width(model, left) else {
            pending = true;
            continue;
        };
        let cap = if TOOLTIP_WRAP_WIDTH > natural {
            natural
        } else {
            TOOLTIP_WRAP_WIDTH
        };
        let mut piece = cap;
        if cap > widest {
            piece = match probe(model, left, Some(cap)) {
                Some(m) => m.w.max(0.0),
                // Headless, the cap stands in until the split's measure lands.
                None => {
                    pending = true;
                    cap
                }
            };
        }
        // `fcomp; test ah,0x41; je` (`0x5308cc`): kept only when larger, so a NaN piece is taken.
        if widest.partial_cmp(&piece) != Some(std::cmp::Ordering::Greater) {
            widest = piece;
        }
        set_line_width(model, left, widest);
    }

    // Each shown right text at the widest line's width from its left text's LEFT (`0x53094f`).
    let units = model.layout_units();
    for &(left, right, _) in &lines {
        if !shown(model, right) {
            continue;
        }
        let left_id = model.region_id(left);
        let new = Anchor::new(Point::Right, left_id, Point::Left, widest, 0.0);
        let stored = model
            .region_data
            .entry(right)
            .or_default()
            .anchors
            .set(new, units);
        if stored {
            model.invalidate_region(right, true);
        }
    }

    // The height: the shown left texts and a 2.0 gap after the first (`0x530972`-`0x5309ca`); a
    // right text's height never counts.
    let mut height = 0.0f32;
    for &(left, _, _) in &lines {
        if !shown(model, left) {
            continue;
        }
        if height != 0.0 {
            height += TOOLTIP_LINE_GAP;
        }
        height += get_height(model, left, &mut pending);
    }

    if let Ok(t) = tip_mut(model, h) {
        t.relayout_owed = pending;
    }
    if !pending {
        // `2·pad + padding + widest` by `2·pad + height` (`0x5309f4`, `0x530a0c`), through the
        // plate's `SetWidth` and `SetHeight` (`0x530a2e`, `0x530a39`).
        let w = 2.0 * TOOLTIP_PLATE_PAD + padding + widest;
        let ht = 2.0 * TOOLTIP_PLATE_PAD + height;
        let input = model.layout_inputs.entry(h).or_default();
        let changed = input.width.to_bits() != w.to_bits();
        input.width = w;
        model.invalidate_frame(h, changed);
        let input = model.layout_inputs.entry(h).or_default();
        let changed = input.height.to_bits() != ht.to_bits();
        input.height = ht;
        model.invalidate_frame(h, changed);
    }
    // The anchor again, without its clear (`0x530a49`), except for the cursor mode, which its
    // update places (`0x530a3c`); then the plate is invalidated for the next resolve. Deviation:
    // `0x530a52` lays it out at once; getters resolve first, so only OnSizeChanged fires later.
    if anchor != TooltipAnchor::Cursor {
        apply_anchor(model, h, owner, anchor, x, y, false);
    }
    model.invalidate_frame(h, false);
}

/// A line's `SetWidth`, its height kept, through [`Model::note_line_width`]; an unmoved width
/// invalidates only a line whose rect a resolve would not give back.
pub(super) fn set_line_width(model: &mut Model, rh: RegionHandle, w: f32) {
    let d = model.region_data.entry(rh).or_default();
    let new = Some((w, d.size.map_or(0.0, |s| s.1)));
    if crate::script::region::size_bits_eq(d.size, new) {
        model.invalidate_region(rh, false);
        return;
    }
    d.size = new;
    model.touch_measure(rh);
    model.note_line_width(rh);
}

/// The headless fallback: each tooltip whose sizing waited on a measure is sized again, from the
/// top of every resolve.
pub(in crate::script) fn size_owed(model: &mut Model) {
    let owed: Vec<FrameHandle> = model
        .arena
        .tooltip_kinds()
        .iter()
        .copied()
        .filter(|&h| {
            matches!(
                model.arena.frame(h).map(|f| &f.kind_state),
                Some(KindState::Tooltip(t)) if t.relayout_owed
            )
        })
        .collect();
    for h in owed {
        size_tooltip(model, h);
    }
}

/// A font string's own shown bit (`+0xc4`), the one the sizer tests.
fn shown(model: &Model, rh: RegionHandle) -> bool {
    model.region_data.get(&rh).is_none_or(|d| !d.hidden)
}

/// The owner's effective scale, which a font string's measure key carries.
fn scale_of(model: &Model, rh: RegionHandle) -> f32 {
    model
        .arena
        .region(rh)
        .and_then(|r| model.arena.frame(r.owner))
        .map_or(1.0, |f| f.effective_scale)
}

/// `rh`'s text laid out in `wrap` (`None`, unwrapped), not stored as the region's own: the region's
/// own measure when it was taken that way, the memo, or the font engine. A string with no text or
/// no font measures 0 (`0x7728b6`). `None` is a measure still owed, which only a VM with no font
/// engine has.
fn probe(model: &mut Model, rh: RegionHandle, wrap: Option<f32>) -> Option<MeasuredText> {
    let scale = scale_of(model, rh);
    let non_space_wrap = model.non_space_wrap_of(rh);
    let zero = |key| MeasuredText {
        w: 0.0,
        h: 0.0,
        natural_w: 0.0,
        key,
    };
    let Some(d) = model.region_data.get(&rh) else {
        return Some(zero(0));
    };
    let text = d.text.as_deref().unwrap_or("");
    let key = d.measure_key_at(text, wrap, scale);
    if text.is_empty() {
        return Some(zero(key));
    }
    let memo_key = (key, non_space_wrap);
    if let Some(m) = d.measured.filter(|m| m.key == key) {
        if !model.tooltip_measures.contains_key(&memo_key) {
            memo_insert(model, memo_key, m);
        }
        return Some(m);
    }
    if let Some(&m) = model.tooltip_measures.get(&memo_key) {
        return Some(m);
    }
    if !crate::script::font_block::has_font(model, rh) {
        return Some(zero(key));
    }
    model.measurer.as_ref()?;
    let req = crate::script::layout::measure_request_at(model, rh, wrap, key, scale)?;
    let mut engine = model.measurer.take().expect("checked above");
    let (w, h, natural_w) = crate::script::TextMeasure::measure(&mut engine, &req);
    model.measurer = Some(engine);
    let m = MeasuredText {
        w,
        h,
        natural_w,
        key,
    };
    memo_insert(model, memo_key, m);
    Some(m)
}

fn memo_insert(model: &mut Model, key: (u64, bool), m: MeasuredText) {
    if model.tooltip_measures.len() >= MEMO_CAP {
        model.tooltip_measures.clear();
    }
    model.tooltip_measures.insert(key, m);
}

/// `rh`'s measure at its own width, stored as its own, as a font string's getters measure in
/// place (`0x772890`, `0x7729b0`).
fn current(model: &mut Model, rh: RegionHandle) -> Option<MeasuredText> {
    let wrap = model.region_data.get(&rh)?.wrap_width();
    let m = probe(model, rh, wrap)?;
    let stale = model
        .region_data
        .get(&rh)
        .is_some_and(|d| d.measured.map(|s| s.key) != Some(m.key));
    let has_text = model
        .region_data
        .get(&rh)
        .is_some_and(|d| d.text.as_deref().is_some_and(|t| !t.is_empty()));
    if stale && has_text {
        let id = model.region_id(rh);
        crate::script::measure::store_measured(model, rh, id, m);
    }
    Some(m)
}

/// The authored size of `rh` and whether its text is empty.
fn authored(model: &Model, rh: RegionHandle) -> ((f32, f32), bool) {
    model.region_data.get(&rh).map_or(((0.0, 0.0), true), |d| {
        (
            d.size.unwrap_or((0.0, 0.0)),
            d.text.as_deref().is_none_or(str::is_empty),
        )
    })
}

/// A font string's `GetWidth` (`0x772930`): its authored width when non-zero, else its text's
/// unwrapped width, never below one unit (`0x772957`-`0x772983`, which a NaN takes too).
fn get_width(model: &mut Model, rh: RegionHandle, pending: &mut bool) -> f32 {
    let ((w, _), empty) = authored(model, rh);
    if w != 0.0 {
        return w.max(FONTSTRING_MIN_SPAN);
    }
    if empty {
        return FONTSTRING_MIN_SPAN;
    }
    match current(model, rh) {
        Some(m) => m.natural_w.max(FONTSTRING_MIN_SPAN),
        None => {
            *pending = true;
            0.0
        }
    }
}

/// A font string's `GetHeight` (`0x772a60`): its authored height when non-zero, else its text's
/// height laid out in its width, never below one unit.
fn get_height(model: &mut Model, rh: RegionHandle, pending: &mut bool) -> f32 {
    let ((_, h), empty) = authored(model, rh);
    if h != 0.0 {
        return h.max(FONTSTRING_MIN_SPAN);
    }
    if empty {
        return FONTSTRING_MIN_SPAN;
    }
    match current(model, rh) {
        Some(m) => m.h.max(FONTSTRING_MIN_SPAN),
        None => {
            *pending = true;
            0.0
        }
    }
}

/// A font string's cached text width (`0x772890`): the unwrapped width, 0 with no text or font.
/// `None` while owed.
fn cached_width(model: &mut Model, rh: RegionHandle) -> Option<f32> {
    let wrap = model.region_data.get(&rh).and_then(|d| d.wrap_width());
    probe(model, rh, wrap).map(|m| m.natural_w)
}
