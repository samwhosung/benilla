//! The EditBox seam the host drives each frame: the advance-table round trip, the caret,
//! selection and scroll geometry derived from it, and the clipboard pair.

use crate::script::{types, UiScript};
use crate::widget::KindState;

impl UiScript {
    /// The focused EditBox's request to measure its display string's per-byte cumulative widths,
    /// when its text, font, scale or wrap width changed; an empty display needs no trip. Answer it
    /// with [`Self::set_editbox_advances`] before [`Self::focused_editbox_text_ui`].
    pub fn editbox_advances_request(&mut self) -> Option<types::EditBoxAdvanceRequest> {
        use std::hash::{Hash, Hasher};
        let h = self.model_mut().focused_editbox?;
        // The lazy text region first: its font keys the table, and a new box has none until now.
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
        // The host measures at the drawn raster size, so the advances land on the drawn glyphs.
        let scale = model
            .arena
            .frame(h)
            .map(|f| f.effective_scale)
            .unwrap_or(1.0);
        // A multi-line box wraps at the text region's width, as the draw does; a resize re-keys.
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

    /// Answer an [`Self::editbox_advances_request`]: the cumulative widths (len+1 entries from 0),
    /// the wrapped-row starts and the row pitch, which only a multi-line box reads.
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

    /// The focused EditBox's text geometry this frame: its text quad, the scroll window and the
    /// caret and selection spans in it, after scrolling the caret into view (`0x77da80`). `None`
    /// until the advance table is answered: the text draws, without caret or highlight.
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
        // Materialize the lazy text region, so an empty, just-focused box has a caret home.
        let rh = super::ensure_text_region(self.lua(), h)?;
        let mut model = self.model_mut();
        // An unanchored text region draws over its owner, so the frame's own width stands in.
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
            eb.advances = vec![0.0];
            eb.rows = vec![0];
        }
        // Keep the caret inside the window with room at the right edge; a multi-line box wraps
        // instead, its window pinned at 0.
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
            // One span per touched row, its overlap with [da, db), measured from the row's origin.
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

    /// Ctrl/Cmd+C: the focused EditBox's selection for the OS clipboard; a password box yields its
    /// mask run, never the text, where the client copies the empty string (`0x882748`).
    pub fn editbox_copy(&mut self) -> Option<String> {
        super::copy_selection(self.lua())
    }

    /// Ctrl/Cmd+X for the focused EditBox: copy, then delete the selection.
    pub fn editbox_cut(&mut self) -> Option<String> {
        super::cut_selection(self.lua())
    }

    /// Push a sent line, as its canonical slash line, into the named box's history where the
    /// reference calls `ChatEdit_AddHistory`. By name: the router runs after the box has cleared
    /// and lost focus. `false` when `box_name` is no live EditBox.
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
