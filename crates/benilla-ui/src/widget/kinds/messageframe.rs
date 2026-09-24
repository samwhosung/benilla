use std::collections::VecDeque;

/// One message line: its text, quantized color, own fade countdowns (the record's `+0xc`/`+0x10`
/// snapshots of `timeVisible`/`fadeDuration`) and host-measured row count.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageLine {
    /// The line as printed, already formatted by the caller.
    pub text: String,
    /// The line's RGB, byte-quantized at insert ([`quantize_u8`]).
    pub color: [u8; 3],
    /// `AddMessage`'s fifth argument, the chat-type id `UpdateColorByID` matches; 0 when absent.
    pub id: u32,
    /// Seconds left before the fade starts, from the `timeVisible` snapshot.
    pub time_left: f32,
    /// Seconds of fade left, from the `fadeDuration` snapshot; counts once `time_left` is spent.
    pub fade_left: f32,
    /// The display alpha: `AddMessage`'s (opaque on a ScrollingMessageFrame, `0x792add`; the alpha
    /// argument on a MessageFrame, `0x795752`) until the fade overwrites it with the byte ramp
    /// `trunc(remaining / fadeDuration * 255)` (`0x788547`, `0x786364`). At 0 a scrolling line
    /// keeps its ring slot and rows; a MessageFrame frees it (`0x786570`).
    pub alpha: f32,
    /// The rows the line wraps into at the frame's width and font, host-measured; 1 until then.
    pub rows: u16,
    /// Hash of the text, font, height and wrap width `rows` was measured at; 0 is unmeasured.
    pub rows_key: u64,
}

/// A `CSimpleMessageScrollFrame`'s runtime state (ctor `0x787670`): a drop-oldest ring of lines,
/// a scrollback cursor counted up from the newest line, and the fade config new lines copy.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollingMessageState {
    /// The frame's own `SetJustifyH`/`SetJustifyV` (method table `0x87b5c0`), `None` until called
    /// and then winning over the font's. Deviation: the reference has one field, the font's, but
    /// our font is read off the first `<FontString>` region (`message_frame_font`), which a
    /// `CreateFrame` frame lacks; so that FontString's own `GetJustifyH` does not see this.
    pub justify: Option<crate::justify::Justify>,

    /// The line ring, newest at the back.
    pub lines: VecDeque<MessageLine>,
    /// `maxLines`, the ring capacity (ctor 8); `SetMaxLines` clears the ring (`0x787dd0`).
    pub max_lines: usize,
    /// `timeVisible`/`displayDuration`: seconds a new line holds full alpha (ctor 10).
    pub time_visible: f32,
    /// `fadeDuration`: the fade's length in seconds (ctor 3); 0 hides the line at once.
    pub fade_duration: f32,
    /// Bumped by every change to the lines' text or colour, `KindState::message_lines_mut` too,
    /// so the measure sweep skips an unchanged frame; the fade tick must not bump it.
    pub lines_gen: u64,
    /// `fadingEnabled` (ctor 1).
    pub fading_enabled: bool,
    /// Lines scrolled back from the newest; 0 is AtBottom, the only state in which fades tick.
    pub scroll_offset: usize,
}

impl Default for ScrollingMessageState {
    fn default() -> ScrollingMessageState {
        // The ctor's defaults (`0x787670`).
        ScrollingMessageState {
            justify: None,
            lines: VecDeque::new(),
            max_lines: 8,
            time_visible: 10.0,
            fade_duration: 3.0,
            fading_enabled: true,
            lines_gen: 0,
            scroll_offset: 0,
        }
    }
}

/// A `0..1` colour component as a byte, rounded half up as `AddMessage` does (`0x788150`).
pub fn quantize_u8(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5).trunc() as u8
}

impl ScrollingMessageState {
    /// `UpdateColorByID(id, r, g, b)` (`0x7932b0`): recolour every line tagged `id`, returning how
    /// many changed. `id == 0` matches nothing (the guard at `0x788250`): a line printed without
    /// an id stores 0 (`0x7929b7`), and `ChatFrame.lua:1357` recolours REPLY, whose id is 0.
    pub fn update_color_by_id(&mut self, id: u32, r: f32, g: f32, b: f32) -> usize {
        if id == 0 {
            return 0;
        }
        let rgb = [quantize_u8(r), quantize_u8(g), quantize_u8(b)];
        let mut moved = 0;
        for line in self.lines.iter_mut().filter(|l| l.id == id) {
            if line.color != rgb {
                line.color = rgb;
                moved += 1;
            }
        }
        if moved > 0 {
            self.lines_gen = self.lines_gen.wrapping_add(1);
        }
        moved
    }

    pub fn add(&mut self, text: String, r: f32, g: f32, b: f32) {
        self.add_with_id(text, r, g, b, 0);
    }

    /// `AddMessage(text, r, g, b, id)` (`0x792900`, `0x788150`): push the line and drop the oldest
    /// past `max_lines`; a scrolled-up view stays on the same lines.
    pub fn add_with_id(&mut self, text: String, r: f32, g: f32, b: f32, id: u32) {
        self.lines_gen = self.lines_gen.wrapping_add(1);
        let line = MessageLine {
            text,
            id,
            color: [quantize_u8(r), quantize_u8(g), quantize_u8(b)],
            time_left: self.time_visible,
            fade_left: self.fade_duration,
            alpha: 1.0,
            rows: 1,
            rows_key: 0,
        };
        self.lines.push_back(line);
        // Scrolled up: keep the same lines in view as the ring grows below them.
        if self.scroll_offset > 0 {
            self.scroll_offset += 1;
        }
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
            // The dropped line was above the view: walk the anchor back down with it.
            self.scroll_offset = self.scroll_offset.saturating_sub(1);
        }
        self.clamp_scroll();
    }

    /// `SetMaxLines(n)` (`0x787dd0`): free every line and reset the scroll, then set the capacity.
    pub fn set_max_lines(&mut self, n: usize) {
        self.lines_gen = self.lines_gen.wrapping_add(1);
        self.lines.clear();
        self.scroll_offset = 0;
        self.max_lines = n.max(1);
    }

    /// `Clear` (`0x7882b0`): retire every line immediately (no fade), reset to the bottom.
    pub fn clear(&mut self) {
        self.lines_gen = self.lines_gen.wrapping_add(1);
        self.lines.clear();
        self.scroll_offset = 0;
    }

    /// Whether the view is pinned to the newest line (`AtBottom`).
    pub fn at_bottom(&self) -> bool {
        self.scroll_offset == 0
    }

    /// Whether the view is scrolled as far back as the ring allows (`AtTop`).
    pub fn at_top(&self) -> bool {
        self.scroll_offset >= self.max_scroll()
    }

    /// The furthest the view can scroll up: enough to bring the oldest line to the bottom row.
    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }

    fn clamp_scroll(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.max_scroll());
    }

    /// Re-arm the fade of the displayed lines only (`0x788b80`): full alpha and the frame's
    /// current `timeVisible`/`fadeDuration`, so even a fully faded line comes back (expiry frees
    /// nothing, `0x788525`). Engine-only, not a 1.12 Lua verb; every scroll entry reaches it.
    pub fn reset_all_fade_times(&mut self, viewport_rows: usize) {
        let (time_visible, fade_duration) = (self.time_visible, self.fade_duration);
        for line in self.lines.range_mut(self.displayed_range(viewport_rows)) {
            line.time_left = time_visible;
            line.fade_left = fade_duration;
            line.alpha = 1.0;
        }
    }

    /// The ring indices the view shows, oldest first.
    pub fn displayed_range(&self, viewport_rows: usize) -> std::ops::Range<usize> {
        let count = self.displayed_count(viewport_rows);
        if count == 0 {
            return 0..0;
        }
        let newest = self.lines.len().saturating_sub(1 + self.scroll_offset);
        newest + 1 - count.min(newest + 1)..newest + 1
    }

    // ── The scroll entries ──────────────────────────────────────────────────────────────────
    //
    // Each re-arms the displayed lines' fade after moving the cursor. A scroll that cannot move
    // calls `0x788b80` directly (`0x788626`, `0x788666`, `0x7886e5`); one that moves relayouts
    // (`0x788750`), whose helper `0x788af0` re-arms on landing off the bottom or back onto it.

    /// `ScrollUp` (`0x788610`): one line older, then re-arm.
    pub fn scroll_up(&mut self, viewport_rows: usize) {
        self.scroll_offset = (self.scroll_offset + 1).min(self.max_scroll());
        self.reset_all_fade_times(viewport_rows);
    }

    /// `ScrollDown` (`0x788650`): one line newer, then re-arm.
    pub fn scroll_down(&mut self, viewport_rows: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        self.reset_all_fade_times(viewport_rows);
    }

    /// `ScrollToBottom` (`0x7886d0`): to the newest line, then re-arm.
    pub fn scroll_to_bottom(&mut self, viewport_rows: usize) {
        self.scroll_offset = 0;
        self.reset_all_fade_times(viewport_rows);
    }

    /// `ScrollToTop` (`0x788690`): to the oldest line, then re-arm, as its relayout always lands
    /// off the bottom.
    pub fn scroll_to_top(&mut self, viewport_rows: usize) {
        self.scroll_offset = self.max_scroll();
        self.reset_all_fade_times(viewport_rows);
    }

    /// How many messages fit `viewport_rows` from the anchor, each costing its wrapped rows; a
    /// partly fitting one counts, as in the draw's band walk (`emit_message_lines`).
    pub fn displayed_count(&self, viewport_rows: usize) -> usize {
        if self.lines.is_empty() || viewport_rows == 0 {
            return 0;
        }
        let top_index = self.lines.len().saturating_sub(1 + self.scroll_offset);
        let mut used = 0usize;
        let mut count = 0usize;
        for idx in (0..=top_index).rev() {
            if used >= viewport_rows {
                break;
            }
            used += usize::from(self.lines[idx].rows.max(1));
            count += 1;
        }
        count.max(1)
    }

    /// `PageUp` (`0x7885b0`): back by the displayed count less one, then re-arm.
    pub fn page_up(&mut self, viewport_rows: usize) {
        let page = self.displayed_count(viewport_rows).saturating_sub(1).max(1);
        self.scroll_offset = (self.scroll_offset + page).min(self.max_scroll());
        self.reset_all_fade_times(viewport_rows);
    }

    /// `PageDown`: the same page toward the newest line, then re-arm.
    pub fn page_down(&mut self, viewport_rows: usize) {
        let page = self.displayed_count(viewport_rows).saturating_sub(1).max(1);
        self.scroll_offset = self.scroll_offset.saturating_sub(page);
        self.reset_all_fade_times(viewport_rows);
    }

    /// Advance the fade by `dt` (OnUpdate, `0x788460`), only at the bottom with fading on; the
    /// scroll entries, not this, bring faded lines back. Both countdowns store `f32` while the
    /// ramp reads the unrounded x87 value (`fst` without pop, `0x7884d7`, `0x788544`), and phase
    /// 1's overshoot is dropped, so the fade starts whole on the next tick.
    pub fn tick(&mut self, dt: f32) {
        if !self.fading_enabled || !self.at_bottom() {
            return;
        }
        let dt = f64::from(dt);
        for line in &mut self.lines {
            if line.time_left > 0.0 {
                line.time_left = ((f64::from(line.time_left) - dt).max(0.0)) as f32;
                continue;
            }
            if self.fade_duration <= 0.0 {
                // No ramp: the line vanishes when phase 1 ends.
                line.alpha = 0.0;
                continue;
            }
            let remaining = f64::from(line.fade_left) - dt;
            line.fade_left = remaining as f32;
            if remaining <= 0.0 {
                line.fade_left = 0.0;
                line.alpha = 0.0;
            } else {
                // Over the live fadeDuration (a mid-fade change rescales the ramp), from the
                // unrounded `remaining`, not the stored f32.
                let byte = quantize_fade_wide(remaining / f64::from(self.fade_duration));
                line.alpha = f32::from(byte) / 255.0;
            }
        }
    }
}

/// The fade's alpha byte, truncated with no `+0.5` (`0x788547`).
fn quantize_fade(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).trunc() as u8
}

/// [`quantize_fade`] of the unrounded ratio (see [`ScrollingMessageState::tick`]).
fn quantize_fade_wide(x: f64) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).trunc() as u8
}

/// Where a MessageFrame's newest message enters: XML `insertMode` (`0x87a618`) and
/// `SetInsertMode`/`GetInsertMode` (`0x794ed0`/`0x794ff0`); the scrolling class has neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InsertMode {
    /// `"TOP"` (0): newest on the top line, older stepping down, as `UIErrorsFrame.xml:4` asks.
    Top,
    /// `"BOTTOM"` (1, the ctor default): newest on the bottom line, older stepping up.
    #[default]
    Bottom,
}

/// A `CSimpleMessageFrame`'s runtime state (ctor `0x785640`), the class of `UIErrorsFrame` and a
/// sibling of [`ScrollingMessageState`]: no ring, no `maxLines` (the cap is what fits), no
/// scrollback, an `AddMessage` alpha in place of the id, and a faded line freed (`0x786570`).
///
/// The reference queues `AddMessage` and drains it at OnUpdate, dropping a message while
/// `numLinesDisplayed` is 0 (`0x786265`); here a line lands at once and
/// [`Self::trim_to_viewport`] applies the cap.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageFrameState {
    /// The frame's own `SetJustifyH`/`SetJustifyV` (method table `0x87b960`), `None` until called.
    /// Deviation: kept apart from the font's, as [`ScrollingMessageState::justify`] is.
    pub justify: Option<crate::justify::Justify>,

    /// The measure sweep's skip token, as [`ScrollingMessageState::lines_gen`].
    pub lines_gen: u64,
    /// The lines, newest at the back in both insert modes: [`InsertMode`] only orders the draw.
    pub lines: VecDeque<MessageLine>,
    /// `insertMode` (ctor BOTTOM).
    pub insert_mode: InsertMode,
    /// `timeVisible`/`displayDuration`: seconds a new line holds its insert alpha (ctor 10).
    pub time_visible: f32,
    /// `fadeDuration`: the fade's length in seconds (ctor 3); 0 hides the line at once.
    pub fade_duration: f32,
    /// `fadingEnabled` (ctor 1); while false only the vertical cap bounds the lines.
    pub fading_enabled: bool,
}

impl Default for MessageFrameState {
    fn default() -> MessageFrameState {
        // The ctor's defaults (`0x785640`).
        MessageFrameState {
            justify: None,
            lines: VecDeque::new(),
            insert_mode: InsertMode::default(),
            time_visible: 10.0,
            fade_duration: 3.0,
            fading_enabled: true,
            lines_gen: 0,
        }
    }
}

impl MessageFrameState {
    /// `AddMessage(text, r, g, b, a)` (`0x795590`, `0x785d00`): the fourth number is the line's
    /// starting alpha, where the scrolling class takes an id and forces opaque.
    pub fn add(&mut self, text: String, r: f32, g: f32, b: f32, a: f32) {
        self.lines_gen = self.lines_gen.wrapping_add(1);
        self.lines.push_back(MessageLine {
            text,
            id: 0,
            color: [quantize_u8(r), quantize_u8(g), quantize_u8(b)],
            time_left: self.time_visible,
            fade_left: self.fade_duration,
            // Quantized like the colour: all four channels pack into one ARGB dword.
            alpha: f32::from(quantize_u8(a)) / 255.0,
            rows: 1,
            rows_key: 0,
        });
    }

    /// `Clear`: drop every line at once, no fade.
    pub fn clear(&mut self) {
        self.lines_gen = self.lines_gen.wrapping_add(1);
        self.lines.clear();
    }

    /// Advance the fade by `dt` (OnUpdate, `0x786200`) in the scrolling class's two phases, with no
    /// scroll gate, freeing a finished line (`0x786570`). The reference also gates on
    /// `numLinesDisplayed > 0`, which this does not check. The arithmetic is `f32`: whether
    /// `0x786200` keeps the unrounded x87 value like the scrolling tick is untraced.
    pub fn tick(&mut self, dt: f32) {
        if !self.fading_enabled {
            return;
        }
        for line in &mut self.lines {
            if line.time_left > 0.0 {
                line.time_left -= dt;
                continue;
            }
            if self.fade_duration <= 0.0 {
                line.alpha = 0.0;
                continue;
            }
            line.fade_left -= dt;
            if line.fade_left <= 0.0 {
                line.fade_left = 0.0;
                line.alpha = 0.0;
            } else {
                line.alpha = f32::from(quantize_fade(line.fade_left / self.fade_duration)) / 255.0;
            }
        }
        // A line past phase 1 at alpha 0 is finished, and freed.
        self.lines.retain(|l| l.time_left > 0.0 || l.alpha > 0.0);
    }

    /// Evict the oldest lines until the rest fit `viewport_rows`, each costing its wrapped rows.
    /// Zero rows means no rect yet and evicts nothing, so a message posted from `OnLoad` before
    /// the first layout survives; until then only the fade bounds the list.
    pub fn trim_to_viewport(&mut self, viewport_rows: usize) {
        if viewport_rows == 0 {
            return;
        }
        let mut used = 0usize;
        let mut keep = 0usize;
        for line in self.lines.iter().rev() {
            if used >= viewport_rows {
                break;
            }
            used += usize::from(line.rows.max(1));
            keep += 1;
        }
        while self.lines.len() > keep.max(1) {
            self.lines.pop_front();
        }
    }
}
