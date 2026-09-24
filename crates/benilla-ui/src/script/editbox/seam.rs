//! The EditBox text-UI seam — the `impl UiScript` surface the host drives each frame: the
//! advance-table round trip (request → host measures → answer), the caret/selection/scroll
//! geometry derived from it, and the clipboard pair. Split beside its concern per the layout.rs
//! pattern (the measure-seam siblings live there).

use crate::script::{types, UiScript};
use crate::widget::KindState;

impl UiScript {
    /// The focused EditBox's advance-table request (the metrics half of the mouse/selection law):
    /// `Some` when the box's DISPLAY string needs its per-byte cumulative widths measured (text or
    /// font changed since the last answer). Answer via [`Self::set_editbox_advances`] before
    /// [`Self::focused_editbox_text_ui`] — an empty display settles engine-side without a trip.
    pub fn editbox_advances_request(&mut self) -> Option<types::EditBoxAdvanceRequest> {
        use std::hash::{Hash, Hasher};
        let h = self.model_mut().focused_editbox?;
        // Materialize the lazy text region first — its resolved font is the table's identity
        // (a just-focused CreateFrame'd box has none until here).
        let rh = super::ensure_text_region(self.lua(), h)?;
        let mut model = self.model_mut();
        let (display, multi_line) = match model.arena.frame(h).map(|f| &f.kind_state) {
            Some(KindState::EditBox(eb)) => (eb.display(), eb.multi_line),
            _ => return None,
        };
        let d = model.region_data.get(&rh);
        let font = d.and_then(|d| d.font_path.clone());
        let height = d.and_then(|d| d.font_height);
        let outline = d.map(|d| d.outline).unwrap_or_default();
        // The box's effective_scale rides the request ([`types::EditBoxAdvanceRequest::scale`]):
        // the host measures at the drawn raster size so the advances land on the drawn glyphs.
        let scale = model
            .arena
            .frame(h)
            .map(|f| f.effective_scale)
            .unwrap_or(1.0);
        // A multiline box also needs its wrap rows — at the text region's resolved width, the
        // same width the draw wraps at. Folded into the cache key: a resize re-wraps.
        let wrap_width = multi_line
            .then(|| {
                model
                    .region_resolved
                    .get(&rh)
                    .or_else(|| model.resolved.get(&h))
                    .map(|r| r.right - r.left)
            })
            .flatten()
            .filter(|w| *w > 0.0);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        display.hash(&mut hasher);
        font.hash(&mut hasher);
        height.map(f32::to_bits).hash(&mut hasher);
        (outline as u8).hash(&mut hasher);
        wrap_width.map(f32::to_bits).hash(&mut hasher);
        scale.to_bits().hash(&mut hasher);
        let key = hasher.finish();
        let id = model.frame_id(h);
        let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state)
        else {
            return None;
        };
        if eb.advances_key == key && eb.advances.len() == display.len() + 1 {
            return None; // cache warm
        }
        if display.is_empty() {
            eb.advances = vec![0.0];
            eb.rows = vec![0];
            eb.advances_key = key;
            return None;
        }
        Some(types::EditBoxAdvanceRequest {
            id,
            font,
            height,
            outline,
            scale,
            text: display,
            wrap_width,
            key,
        })
    }

    /// Answer an [`Self::editbox_advances_request`]: the per-byte cumulative laid-out widths of
    /// the requested display string (len+1 entries, `[0] = 0`), plus the wrapped-row starts and
    /// row pitch (single-line: `rows = [0]`, `cell_h` the line em — only a multiline box reads
    /// them, but the answer is uniform).
    pub fn set_editbox_advances(
        &mut self,
        id: u32,
        key: u64,
        cum: Vec<f32>,
        rows: Vec<usize>,
        cell_h: f32,
    ) {
        let mut model = self.model_mut();
        let Some(h) = model.id_to_frame.get(&id).copied() else {
            return;
        };
        if let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            eb.advances = cum;
            eb.rows = if rows.is_empty() { vec![0] } else { rows };
            eb.cell_h = cell_h;
            eb.advances_key = key;
        }
    }

    /// The focused EditBox's text-UI geometry for this frame (caret/highlight geometry is left to
    /// the host): which Text quad is the box's (`target`), the scroll window to draw
    /// (`display_from`), and the caret/selection x-spans within it — all advance-table-derived.
    /// Clamps the scroll window against the text region's resolved width first (the
    /// scroll-into-view invariant, `0x77da80`). `None` when nothing is focused, the box is
    /// off-screen, or the advance table hasn't been answered yet (text still draws; the caret and
    /// highlight sit that frame out).
    pub fn focused_editbox_text_ui(&mut self) -> Option<types::EditBoxTextUi> {
        let h = {
            let model = self.model_mut();
            let h = model.focused_editbox?;
            let f = model.arena.frame(h)?;
            if !f.effective_visible || !matches!(f.kind_state, KindState::EditBox(_)) {
                return None;
            }
            h
        };
        // Focus means "about to type": materialize the lazy text region (the same lazy
        // constructor `sync_text_region` uses), so an empty just-focused box has a caret home.
        let rh = super::ensure_text_region(self.lua(), h)?;
        let mut model = self.model_mut();
        // The text region's resolved width; an unanchored region (a CreateFrame'd box without
        // insets) draws over its owner, so the frame's own rect is its width.
        let avail = model
            .region_resolved
            .get(&rh)
            .or_else(|| model.resolved.get(&h))
            .map(|r| r.right - r.left)
            .unwrap_or(0.0);
        let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state)
        else {
            return None;
        };
        let display = eb.display();
        if !display.is_empty() && eb.advances.len() != display.len() + 1 {
            return None;
        }
        if display.is_empty() {
            // Trivial table — no host trip needed for an empty box.
            eb.advances = vec![0.0];
            eb.rows = vec![0];
        }
        // Keep the caret inside the window, leaving it a pixel to stand in at the right edge
        // (multiline: pins the window to 0 — the box wraps instead).
        eb.clamp_scroll((avail - 2.0).max(0.0));
        let from = eb.scroll_start.min(display.len());
        let origin = eb.advances[from];
        let cursor_d = eb.text_to_display(eb.cursor).min(display.len());
        let (a, b) = (eb.sel_start.min(eb.sel_end), eb.sel_start.max(eb.sel_end));
        let (da, db) = (
            eb.text_to_display(a).min(display.len()),
            eb.text_to_display(b).min(display.len()),
        );
        let (caret_row, caret_x, selection) = if eb.multi_line {
            let (row, x) = eb.caret_row_x(cursor_d);
            // The selection as per-row spans: each touched row contributes the intersection of
            // its byte range with [da, db), x-measured from the row's own origin.
            let mut spans = Vec::new();
            if da != db {
                for i in 0..eb.rows.len() {
                    let (rs, re) = eb.row_range(i, display.len());
                    let (s0, s1) = (da.max(rs), db.min(re));
                    if s0 < s1 {
                        let ro = eb.advances[rs.min(display.len())];
                        spans.push((i, eb.advances[s0] - ro, eb.advances[s1] - ro));
                    }
                }
            }
            (row, x, spans)
        } else {
            let spans = if da != db {
                vec![(
                    0,
                    (eb.advances[da] - origin).max(0.0),
                    eb.advances[db] - origin,
                )]
            } else {
                Vec::new()
            };
            (0, eb.advances[cursor_d] - origin, spans)
        };
        Some(types::EditBoxTextUi {
            target: crate::order::ZTarget::Region(rh),
            display_from: from,
            multi_line: eb.multi_line,
            caret_x,
            caret_row,
            cell_h: eb.cell_h,
            caret_on: eb.caret_shown,
            selection,
            highlight_color: eb.highlight_color,
        })
    }

    /// Ctrl/Cmd+C for the focused EditBox: the selected substring for the OS clipboard (`None` =
    /// no selection; a password box yields its mask run, never the real text — the client's fixed
    /// placeholder `0x882748`).
    pub fn editbox_copy(&mut self) -> Option<String> {
        super::copy_selection(self.lua())
    }

    /// Ctrl/Cmd+X for the focused EditBox: copy, then delete the selection.
    pub fn editbox_cut(&mut self) -> Option<String> {
        super::cut_selection(self.lua())
    }

    /// Push one submitted line into the named box's recall history — the ref's
    /// `ChatEdit_AddHistory` slot (the canonical slash line, added by the app's router AFTER it
    /// parses the send). By name, not focus: the submit pipeline is asynchronous,
    /// so by the time the router runs, the box has already cleared and dropped focus. Returns
    /// `false` when `box_name` names no live EditBox.
    pub fn editbox_add_history(&mut self, box_name: &str, line: &str) -> bool {
        let lua = self.lua();
        let Ok(t) = lua.globals().get::<mlua::Table>(box_name) else {
            return false;
        };
        let Ok(h) = crate::script::object::frame_handle_of(lua, &t) else {
            return false;
        };
        super::with_eb(lua, h, |eb| eb.add_history_line(line)).is_some()
    }
}
