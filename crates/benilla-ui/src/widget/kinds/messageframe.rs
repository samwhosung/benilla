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
    /// The display alpha: `AddMessage`'s (on a ScrollingMessageFrame opaque with a colour,
    /// `0x792add`, else the frame font's alpha, `0x792b39`; the alpha argument on a MessageFrame,
    /// `0x795752`) until the fade overwrites it with the byte ramp
    /// `trunc(remaining / fadeDuration * 255)` (`0x788547`, `0x786364`). At 0 a scrolling line
    /// keeps its ring slot and rows; a MessageFrame frees it (`0x786570`).
    pub alpha: f32,
    /// The record's shown flag (`+0x8`): set by the store and a re-arm (`0x788204`, `0x788b0d`),
    /// cleared when the fade hides the line (`0x788525`); the fade skips a line without it
    /// (`0x7884bb`), and the time setters rewrite only lines with it.
    pub shown: bool,
    /// The rows the line wraps into at the frame's width and font, host-measured; 1 until then.
    pub rows: u16,
    /// Hash of the text, font, height and wrap width `rows` was measured at; 0 is unmeasured.
    pub rows_key: u64,
}

/// A `CSimpleMessageScrollFrame`'s runtime state (ctor `0x787670`): a drop-oldest ring of lines,
/// the ring's write position and anchor slot, the fade config new lines copy, and the three fields
/// only its relayout ([`ScrollingMessageState::layout`], `0x788750`) writes: lines shown, at top
/// and at bottom. Every reader reads those cached fields, so a hidden frame, whose relayout
/// returns before it places anything (`0x78878e`), keeps its last visible answers.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollingMessageState {
    /// The frame's own `SetJustifyH`/`SetJustifyV` (method table `0x87b5c0`), `None` until called
    /// and then winning over the font's. Deviation: the reference has one field, the font's, but
    /// ours is the holder region's (`message_frame_font`), kept apart; so the holder's own
    /// `GetJustifyH` does not see this.
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
    /// The write position (`+0x344`): the ring slot of the newest line, from 0 and wrapping at
    /// `max_lines` (`0x788183`-`0x788198`); −1 when empty (`0x7878d7`, `0x787f95`, `0x7882d8`).
    pub write_pos: i32,
    /// The anchor (`+0x348`): the ring slot on the bottom row, stored by the relayout alone
    /// (`0x788788`) and reset to 0 by the ctor and `SetMaxLines` (`0x7878ca`, `0x787f9f`).
    pub anchor: i32,
    /// Lines shown (`+0x34c`): what the last relayout placed, 0 when it ran hidden (`0x788782`).
    pub lines_shown: usize,
    /// At top (`+0x350`): the last visible relayout placed every line from the anchor back
    /// (`0x788a37`-`0x788a48`); `Clear` and `SetMaxLines` zero it (`0x7882ce`, `0x787eb1`).
    pub at_top: bool,
    /// At bottom (`+0x354`): the last visible relayout anchored the newest line (`0x7887aa`,
    /// `0x7887b6`); the ctor and `SetMaxLines` set it (`0x78776b`, `0x787ea7`).
    pub at_bottom: bool,
    /// The text insets `(left, right, top, bottom)` the lines' box keeps from the frame's rect
    /// (`+0x37c`-`+0x388`); only XML `<TextInsets>` sets them (`0x787fb0`).
    pub insets: [crate::layout::Len; 4],
    /// The detached FontString that holds the lines' font (the frame's own `+0x338`), the font
    /// verbs' receiver; never in `GetRegions`, as the reference's font is no region.
    pub font_region: Option<crate::widget::RegionHandle>,
    /// The link-button rebuild flag (`+0x3b8`, cleared by the ctor at `0x7878d0`): set by a
    /// relayout (`0x788a8e`), a line fading out (`0x788538`) and a fade re-arm (`0x788bf1`), and
    /// spent by the per-frame rebuild (`0x788595` → `0x788c00`).
    pub links_dirty: bool,
    /// The font the last relayout laid out with, as a key: a change is a font change
    /// (`0x788086`), which relayouts at the anchor.
    pub laid_font: Option<u64>,
    /// The layout-unit rect the frame's size was last tested against (`0x788321`-`0x788339`): a
    /// resize past the epsilon relayouts at the anchor (`0x788427`).
    pub laid_rect: Option<crate::layout::Rect>,
    /// Relayouts that moved the anchor since the binding last fired `OnMessageScrollChanged` for
    /// them (`0x78877a`, `0x788a89`-`0x788aad`).
    pub scroll_changes: u32,
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
            write_pos: -1,
            anchor: 0,
            lines_shown: 0,
            at_top: false,
            at_bottom: true,
            insets: [crate::layout::Len::Script(0.0); 4],
            font_region: None,
            links_dirty: false,
            laid_font: None,
            laid_rect: None,
            scroll_changes: 0,
        }
    }
}
/// A `0..1` colour component as a byte, rounded half up as `AddMessage` does (`0x788150`).
pub fn quantize_u8(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5).trunc() as u8
}

/// The space a ScrollingMessageFrame's layout fits messages into, in screen pixels. The relayout
/// charges each message its block height, which spaces its rows by the spacing times the scale
/// snapped up to a whole pixel (`0x772a07`-`0x772a0d` forms it, `0x5c2143`-`0x5c215c` snaps it),
/// plus the gap to the next message, the spacing snapped unscaled and then scaled
/// (`0x788954`-`0x788975`, `0x7889a3`-`0x7889ad`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// The frame's height.
    pub height: f32,
    /// One row's height.
    pub line_h: f32,
    /// The gap between the rows of one message, snapped up to a whole pixel.
    pub row_gap: f32,
    /// The gap between one message and the next.
    pub message_gap: f32,
    /// Whether the frame is visible (`+0xd4`), without which the relayout places nothing.
    pub visible: bool,
}

impl Viewport {
    /// A viewport of `n` unit rows with no gaps, for a test that has no font.
    #[cfg(test)]
    pub fn rows(n: usize) -> Viewport {
        Viewport {
            height: n as f32,
            line_h: 1.0,
            row_gap: 0.0,
            message_gap: 0.0,
            visible: true,
        }
    }

    /// The height of a message of `rows` wrapped rows, as `GxuFontGetTextBlockHeight` gives it:
    /// `rows` lines and `rows - 1` row gaps (`0x5c2168`).
    pub fn block(&self, rows: u16) -> f32 {
        let rows = f32::from(rows.max(1));
        rows * self.line_h + (rows - 1.0) * self.row_gap
    }

    /// What a message of `rows` wrapped rows takes from the frame: its block and the gap after it.
    pub fn cost(&self, rows: u16) -> f32 {
        self.block(rows) + self.message_gap
    }
}

#[cfg(test)]
impl From<usize> for Viewport {
    fn from(n: usize) -> Viewport {
        Viewport::rows(n)
    }
}

impl ScrollingMessageState {
    /// The ring's limit as the reference's `int` (`+0x340`).
    fn limit(&self) -> i32 {
        i32::try_from(self.max_lines).unwrap_or(i32::MAX)
    }

    /// The line count (`+0x33c`).
    fn count(&self) -> i32 {
        self.lines.len() as i32
    }

    /// How many lines the anchor sits above the newest: `(pos − anchor) mod limit`, 0 when empty.
    /// Past the oldest line (a hidden frame's `ScrollUp` with a stale at-top) it reaches the
    /// count or beyond, and the relayout then places nothing.
    pub fn scroll_offset(&self) -> usize {
        if self.write_pos < 0 {
            return 0;
        }
        self.write_pos
            .wrapping_sub(self.anchor)
            .rem_euclid(self.limit()) as usize
    }

    /// The ring index of `slot`, if it holds a line.
    fn index_of(&self, slot: i32) -> Option<usize> {
        if self.write_pos < 0 {
            return None;
        }
        let back = self.write_pos.wrapping_sub(slot).rem_euclid(self.limit()) as usize;
        (back < self.lines.len()).then(|| self.lines.len() - 1 - back)
    }

    /// The ring indices of the lines the last relayout placed, oldest first: its nodes, the
    /// anchor slot and the `lines_shown - 1` before it (`0x788a03`-`0x788a0d`).
    pub fn shown_range(&self) -> std::ops::Range<usize> {
        match self.index_of(self.anchor) {
            Some(top) if self.lines_shown > 0 => top + 1 - self.lines_shown.min(top + 1)..top + 1,
            _ => 0..0,
        }
    }

    /// The relayout's walk from the anchor in `viewport`, read only: how many lines it places,
    /// how many it may place (`count + anchor − pos`, less the limit past a wrap,
    /// `0x7887dc`-`0x7887ea`), and whether it stopped on one that did not fit. From the anchor
    /// back each line costs its block height plus the gap to the next (`0x7889a3`-`0x7889ad`);
    /// one that does not fit whole in what is left ends the walk (`0x7889b3`-`0x7889be`,
    /// `0x788a2f`) unless it is the anchored one (`0x7889c0`-`0x7889c5`), which always shows. A
    /// faded line is placed all the same (`0x7889c7`-`0x7889ee` only hides it).
    fn walk(&self, viewport: Viewport) -> (usize, i64, bool) {
        let avail = i64::from(self.count()) - self.scroll_offset() as i64;
        let Some(top) = self.index_of(self.anchor).filter(|_| avail > 0) else {
            return (0, avail, false);
        };
        let mut left = viewport.height;
        let mut placed = 0usize;
        for idx in (0..=top).rev().take(avail as usize) {
            let cost = viewport.cost(self.lines[idx].rows);
            if placed > 0 && left < cost {
                return (placed, avail, true);
            }
            left -= cost;
            placed += 1;
        }
        (placed, avail, false)
    }

    /// `LayoutLines(anchor)` (`0x788750`). An empty ring returns at once (`0x78875f`). Otherwise
    /// it zeroes lines shown and stores the anchor (`0x788782`, `0x788788`); a frame that is not
    /// visible stops there (`0x78878e`) but for the link flag (`0x788a8e`). A visible one sets at
    /// bottom to whether the anchor is the newest line, forcing a re-arm when it lands there from
    /// off the bottom (`0x788799`-`0x7887b6`); walks and re-arms each line it reaches unless at the
    /// bottom unforced (`0x788af0` at `0x7888be`, before the fit test, so the first line that does
    /// not fit is re-armed too); then counts lines shown and sets at top to whether it placed every
    /// line it could (`0x788a20`, `0x788a37`).
    pub fn layout(&mut self, anchor: i32, viewport: impl Into<Viewport>) {
        let viewport = viewport.into();
        if self.write_pos == -1 {
            return;
        }
        // A moved anchor fires `OnMessageScrollChanged` at the tail, hidden or not (`0x788a89`).
        if anchor != self.anchor {
            self.scroll_changes += 1;
        }
        self.lines_shown = 0;
        self.anchor = anchor;
        self.links_dirty = true;
        if !viewport.visible {
            return;
        }
        let force = anchor == self.write_pos && !self.at_bottom;
        self.at_bottom = anchor == self.write_pos;
        let (placed, avail, broke) = self.walk(viewport);
        if force || !self.at_bottom {
            if let Some(top) = self.index_of(anchor) {
                let walked = (placed + usize::from(broke)).min(top + 1);
                self.rearm(top + 1 - walked..top + 1);
            }
        }
        self.lines_shown = placed;
        self.at_top = placed as i64 == avail;
    }

    /// A relayout at the anchor: a resize (`0x788427`), a font change (`0x788086`), a show
    /// (`0x788451`) and `UpdateColorByID` (`0x78829e`).
    pub fn relayout(&mut self, viewport: impl Into<Viewport>) {
        self.layout(self.anchor, viewport);
    }

    /// Re-count lines shown and at top on a visible frame from rows the host measured after the
    /// relayout ran, standing for the reference's measure inside the walk; nothing is re-armed.
    pub fn recount(&mut self, viewport: impl Into<Viewport>) {
        let viewport = viewport.into();
        if self.write_pos == -1 || !viewport.visible {
            return;
        }
        let (placed, avail, _) = self.walk(viewport);
        self.lines_shown = placed;
        self.at_top = placed as i64 == avail;
    }

    /// How many lines the view places in `viewport` from the anchor, live: what the draw
    /// (`emit_message_lines`) walks, and the relayout's count while the frame is visible.
    pub fn displayed_count(&self, viewport: impl Into<Viewport>) -> usize {
        self.walk(viewport.into()).0
    }

    /// `UpdateColorByID(id, r, g, b)` (`0x7932b0`): recolour every line tagged `id`, returning how
    /// many changed. `id == 0` matches nothing (the guard at `0x788250`): a line printed without
    /// an id stores 0 (`0x7929b7`), and `ChatFrame.lua:1357` recolours REPLY, whose id is 0.
    pub fn update_color_by_id(
        &mut self,
        id: u32,
        r: f32,
        g: f32,
        b: f32,
        viewport: impl Into<Viewport>,
    ) -> usize {
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
        // Past the guard the relayout runs whether or not a line matched (`0x78829e`).
        self.relayout(viewport);
        moved
    }

    /// [`Self::add_with_id`] for a state that has no viewport yet, here eight rows high.
    #[cfg(test)]
    pub fn add(&mut self, text: String, r: f32, g: f32, b: f32) {
        self.add_with_id(text, r, g, b, 0, 8);
    }

    /// `AddMessage(text, r, g, b, id)` (`0x792900`, `0x788150`): store the line through
    /// [`Self::add_packed`] with the colour quantized and opaque.
    pub fn add_with_id(
        &mut self,
        text: String,
        r: f32,
        g: f32,
        b: f32,
        id: u32,
        viewport: impl Into<Viewport>,
    ) {
        // The binding packs the given colour with the alpha byte 0xff (`0x792add`).
        let color = crate::color::Rgba8::rgba(quantize_u8(r), quantize_u8(g), quantize_u8(b), 255);
        self.add_packed(text, color, id, viewport);
    }

    /// `StoreLine` (`0x788150`) with the record's colour dword as given, its alpha byte the line's
    /// starting alpha: `AddMessage` without a colour copies the frame font's whole dword
    /// (`0x792b39`-`0x792b45`) and the store keeps it (`0x7881f9`-`0x7881fe`). The count grows to
    /// the limit and the write position advances, wrapping (`0x788177`-`0x788198`). At the bottom
    /// it relayouts at the new line (`0x788234`); off it, when the write lands on the topmost
    /// shown slot, `anchor − shown + 1` (`0x78819e`-`0x7881bb`), it scrolls down once
    /// (`0x788245`); else nothing moves, so a scrolled-up view keeps its lines.
    pub fn add_packed(
        &mut self,
        text: String,
        color: crate::color::Rgba8,
        id: u32,
        viewport: impl Into<Viewport>,
    ) {
        let viewport = viewport.into();
        self.lines_gen = self.lines_gen.wrapping_add(1);
        let limit = self.limit();
        self.write_pos += 1;
        if self.write_pos >= limit {
            self.write_pos -= limit;
        }
        let mut topmost = self.anchor - self.lines_shown as i32 + 1;
        if topmost < 0 {
            topmost += limit;
        }
        let overwrites_topmost = self.write_pos == topmost;
        self.lines.push_back(MessageLine {
            text,
            id,
            color: [color.r, color.g, color.b],
            time_left: self.time_visible,
            fade_left: self.fade_duration,
            alpha: f32::from(color.a) / 255.0,
            shown: true,
            rows: 1,
            rows_key: 0,
        });
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
        }
        if self.at_bottom {
            self.layout(self.write_pos, viewport);
        } else if overwrites_topmost {
            self.step_down(viewport);
        }
    }

    /// `SetMaxLines(n)` (`0x787dd0`): free every line, then the count 0, the limit, lines shown 0,
    /// at bottom, not at top, an empty write position and anchor 0 (`0x787e95`-`0x787eb1`,
    /// `0x787f95`, `0x787f9f`). It does not relayout.
    pub fn set_max_lines(&mut self, n: usize) {
        self.lines_gen = self.lines_gen.wrapping_add(1);
        self.lines.clear();
        self.max_lines = n.max(1);
        self.lines_shown = 0;
        self.at_bottom = true;
        self.at_top = false;
        self.write_pos = -1;
        self.anchor = 0;
    }

    /// `Clear` (`0x7882b0`): the count and write position 0, a relayout at slot 0 (`0x7882c9`),
    /// then at top 0 and the write position −1 (`0x7882ce`, `0x7882d8`). The relayout runs on a
    /// ring it sees as holding a line at slot 0, so a visible frame lands at the bottom.
    pub fn clear(&mut self, viewport: impl Into<Viewport>) {
        self.clear_layout(viewport);
        self.clear_finish();
    }

    /// [`Self::clear`] up to its relayout, inside which `OnMessageScrollChanged` fires
    /// (`0x7882c9`), so a handler reads the count and write position 0.
    pub fn clear_layout(&mut self, viewport: impl Into<Viewport>) {
        self.lines_gen = self.lines_gen.wrapping_add(1);
        self.lines.clear();
        self.write_pos = 0;
        self.layout(0, viewport);
    }

    /// [`Self::clear`] after its relayout: at top 0 and the write position −1.
    pub fn clear_finish(&mut self) {
        self.at_top = false;
        self.write_pos = -1;
    }

    /// Re-arm the fade of the lines shown (`0x788b80`): full alpha and the frame's current
    /// `timeVisible`/`fadeDuration`, so even a fully faded line comes back (expiry frees nothing,
    /// `0x788525`), then mark the links (`0x788bf1`). The refused scroll's re-arm; a scroll that
    /// moves re-arms what its relayout walks. Engine-only, not a 1.12 Lua verb.
    pub fn reset_all_fade_times(&mut self) {
        self.links_dirty = true;
        self.rearm(self.shown_range());
    }

    fn rearm(&mut self, range: std::ops::Range<usize>) {
        let (time_visible, fade_duration) = (self.time_visible, self.fade_duration);
        for line in self.lines.range_mut(range) {
            line.time_left = time_visible;
            line.fade_left = fade_duration;
            line.alpha = 1.0;
            line.shown = true;
        }
    }

    // ── The scroll entries ──────────────────────────────────────────────────────────────────
    //
    // Each tests the cached flags. A scroll that cannot move calls `0x788b80` directly
    // (`0x788626`, `0x788666`, `0x7886e5`, `0x788739`); one that moves relayouts (`0x788750`).

    /// `ScrollUp` (`0x788610`).
    pub fn scroll_up(&mut self, viewport: impl Into<Viewport>) {
        self.step_up(viewport);
    }

    /// `ScrollDown` (`0x788650`).
    pub fn scroll_down(&mut self, viewport: impl Into<Viewport>) {
        self.step_down(viewport);
    }

    /// `0x788610`'s answer: whether the view moved. An empty frame answers 0 and touches nothing
    /// (`0x788619`); one at the top re-arms and answers 0 (`0x78861c`-`0x788626`); else it
    /// relayouts one slot older, wrapping at the limit (`0x78862e`-`0x78863e`).
    pub fn step_up(&mut self, viewport: impl Into<Viewport>) -> bool {
        if self.write_pos == -1 {
            return false;
        }
        if self.at_top {
            self.reset_all_fade_times();
            return false;
        }
        let mut slot = self.anchor - 1;
        if slot < 0 {
            slot += self.limit();
        }
        self.layout(slot, viewport);
        true
    }

    /// `0x788650`'s answer: the same toward the newest line, refused at the bottom (`0x788659`,
    /// `0x788666`), one slot newer wrapping at the limit (`0x78866e`-`0x788682`).
    pub fn step_down(&mut self, viewport: impl Into<Viewport>) -> bool {
        if self.write_pos == -1 {
            return false;
        }
        if self.at_bottom {
            self.reset_all_fade_times();
            return false;
        }
        let mut slot = self.anchor + 1;
        if slot >= self.limit() {
            slot -= self.limit();
        }
        self.layout(slot, viewport);
        true
    }

    /// `ScrollToBottom` (`0x7886d0`): an empty frame returns at once (`0x7886d9`); at the bottom it
    /// re-arms (`0x7886e5`), else it relayouts at the newest line (`0x7886eb`).
    pub fn scroll_to_bottom(&mut self, viewport: impl Into<Viewport>) {
        if self.write_pos == -1 {
            return;
        }
        if self.at_bottom {
            self.reset_all_fade_times();
        } else {
            self.layout(self.write_pos, viewport);
        }
    }

    /// `ScrollToTop` (`0x788690`): an empty frame returns at once (`0x78869a`); else it relayouts
    /// at the oldest slot, `pos + 1` on a full ring and 0 otherwise (`0x78869c`-`0x7886ba`).
    pub fn scroll_to_top(&mut self, viewport: impl Into<Viewport>) {
        if self.write_pos == -1 {
            return;
        }
        let limit = self.limit();
        let mut slot = 0;
        if self.count() == limit {
            slot = self.write_pos + 1;
            if slot == limit {
                slot = 0;
            }
        }
        self.layout(slot, viewport);
    }

    /// `SetScrollFromBottom(n)` (`0x788700`): nothing on an empty frame (`0x78870c`, `0x788717`);
    /// else the slot `(pos − n + count) % count`, a truncated remainder floored at 0
    /// (`0x788719`-`0x788729`; the clamp to the count after it cannot bind), then a re-arm in place
    /// when it is the anchor (`0x788739`) and a relayout there otherwise (`0x788744`).
    pub fn set_scroll_from_bottom(&mut self, n: i32, viewport: impl Into<Viewport>) {
        let count = self.count();
        if count == 0 || self.write_pos == -1 {
            return;
        }
        let rem = self
            .write_pos
            .wrapping_sub(n)
            .wrapping_add(count)
            .wrapping_rem(count);
        let slot = rem.max(0).min(count);
        if slot == self.anchor {
            self.reset_all_fade_times();
        } else {
            self.layout(slot, viewport);
        }
    }

    /// `GetCurrentScroll`'s `0x788ac0`: 0 when empty (`0x788ac9`, `0x788ad4`), else
    /// `(pos − anchor + count) % count`, a truncated remainder (`0x788ad6`-`0x788ae2`).
    pub fn current_scroll(&self) -> i32 {
        let count = self.count();
        if count == 0 || self.write_pos == -1 {
            return 0;
        }
        self.write_pos
            .wrapping_sub(self.anchor)
            .wrapping_add(count)
            .wrapping_rem(count)
    }

    /// `SetTimeVisible(s)`'s store (`0x788090`): each line shown with its shown flag takes `s` as
    /// its visible time (`0x7880b4`), then the frame (`0x7880cc`).
    pub fn set_time_visible(&mut self, s: f32) {
        let range = self.shown_range();
        for line in self.lines.range_mut(range).filter(|l| l.shown) {
            line.time_left = s;
        }
        self.time_visible = s;
    }

    /// `SetFadeDuration(s)`'s store (`0x7880f0`): the same over the fade time (`0x788114`,
    /// `0x78812c`).
    pub fn set_fade_duration(&mut self, s: f32) {
        let range = self.shown_range();
        for line in self.lines.range_mut(range).filter(|l| l.shown) {
            line.fade_left = s;
        }
        self.fade_duration = s;
    }

    /// `PageUp` (`0x7885b0`): `ScrollUp` once per line shown, the cached count read once up front
    /// (`0x7885b4`); a step that cannot move ends the page there (`0x7885ca`), else one `ScrollDown`
    /// follows (`0x7885d4`). With no line shown the page is that step back alone. The binding runs
    /// this loop a step at a time, so each step's `OnMessageScrollChanged` fires in its place.
    #[cfg(test)]
    pub fn page_up(&mut self, viewport: impl Into<Viewport>) {
        let viewport = viewport.into();
        for _ in 0..self.lines_shown {
            if !self.step_up(viewport) {
                return;
            }
        }
        self.step_down(viewport);
    }

    /// `PageDown` (`0x7885e0`): the mirror, `ScrollDown` per line shown (`0x7885f3`), a refusal
    /// ending the page (`0x7885fa`), else one `ScrollUp` (`0x788604`).
    #[cfg(test)]
    pub fn page_down(&mut self, viewport: impl Into<Viewport>) {
        let viewport = viewport.into();
        for _ in 0..self.lines_shown {
            if !self.step_down(viewport) {
                return;
            }
        }
        self.step_up(viewport);
    }

    /// Advance the fade by `dt` (OnUpdate, `0x788460`): only with fading on, at the bottom and
    /// lines shown, as cached (`0x788480`, `0x78848c`, `0x78849b`), and over the lines shown alone,
    /// so a line out of view keeps its fade where it stopped. The scroll entries, not this, bring
    /// faded lines back. Each line keys on its own countdowns, each tested `!= 0` (`test ah,0x44`):
    /// a nonzero visible time counts down and is stored (`0x7884d7`); once below 0 the line hides if
    /// its fade time is 0 (`0x7884f9`) and otherwise starts the fade next tick at time 0
    /// (`0x7884fb`). At time 0 a fade time of 0 leaves the line as it is (`0x78850e`); any other
    /// counts down, hiding below 0 (`0x788521`, so exactly 0 stays shown at alpha 0), else stored
    /// with the alpha byte `ftol(left / fadeDuration · 255)`, unclamped (`0x788544`-`0x788558`), from
    /// the unrounded x87 value.
    pub fn tick(&mut self, dt: f32) {
        if !self.fading_enabled || !self.at_bottom || self.lines_shown == 0 {
            return;
        }
        let dt = f64::from(dt);
        let duration = f64::from(self.fade_duration);
        let range = self.shown_range();
        let mut hid = false;
        for line in self.lines.range_mut(range) {
            if !line.shown {
                continue;
            }
            // The hide (`0x788525`): the flag cleared, the node hidden and the links marked
            // (`0x788538`). The record's alpha byte is left; here it reads 0, which nothing can
            // see while the flag is clear.
            let mut hide = |line: &mut MessageLine| {
                line.shown = false;
                line.alpha = 0.0;
                hid = true;
            };
            if line.time_left != 0.0 {
                let left = f64::from(line.time_left) - dt;
                line.time_left = left as f32;
                if left < 0.0 {
                    if line.fade_left == 0.0 {
                        hide(line);
                    } else {
                        line.time_left = 0.0;
                    }
                }
                continue;
            }
            if line.fade_left == 0.0 {
                continue;
            }
            let left = f64::from(line.fade_left) - dt;
            if left < 0.0 {
                hide(line);
                continue;
            }
            line.fade_left = left as f32;
            let byte = crate::script::binding_abi::ftol(left / duration * 255.0) as u8;
            line.alpha = f32::from(byte) / 255.0;
        }
        self.links_dirty |= hid;
    }
}

/// The fade's alpha byte, truncated with no `+0.5` (`0x788547`).
fn quantize_fade(x: f32) -> u8 {
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
    /// The text insets `(left, right, top, bottom)` the lines' box keeps from the frame's rect
    /// (`+0x330`-`+0x33c`, zeroed by the ctor at `0x785675`); only XML `<TextInsets>` sets them
    /// (`0x785b80`).
    pub insets: [crate::layout::Len; 4],
    /// The lines' font holder (`+0x320`), as [`ScrollingMessageState::font_region`].
    pub font_region: Option<crate::widget::RegionHandle>,
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
            insets: [crate::layout::Len::Script(0.0); 4],
            font_region: None,
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
            shown: true,
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
