use super::RegionHandle;

/// The text span a cursor motion or deletion operates over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditUnit {
    /// One character, a whole link counting as one: plain arrow, Backspace, Delete.
    Char,
    /// One word run ([`EditBoxState::word_boundary`]): Ctrl/Option+arrow.
    Word,
    /// The caret's line, which ends at a newline: HOME/END (`0x77c980`/`0x77c9f0`), and
    /// Cmd+Left/Right on a Mac.
    Line,
    /// Text start going back, text end going forward: Ctrl+HOME/END (`0x77ca60`/`0x77cac0`),
    /// and Cmd+Up/Down on a Mac.
    Edge,
    /// One wrapped row up or down: UP/DOWN in a multi-line box (`0x77cb20`). A single-line box
    /// recalls its history instead (`0x77d030`/`0x77cfd0`), older going back.
    Row,
}

/// One semantic editing operation for [`EditBoxState::apply`]: the host's per-OS keymap picks the
/// action, the reference's edit-box law decides its effect. Clipboard operations stay host-side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    /// Move the caret one `unit`; `extend` (Shift) drags the selection by the reference's rule
    /// ([`EditBoxState::extend_to`]). The alt-arrow gate (`0x77b18e`) acts upstream, on the key.
    Move {
        unit: EditUnit,
        back: bool,
        extend: bool,
    },
    /// Delete one `unit` from the caret, or the selection if any; `Edge` back is Cmd+Backspace.
    Delete { unit: EditUnit, back: bool },
    /// Ctrl+A: select all (`HighlightText(0, -1)`), the caret left where it is.
    SelectAll,
}

/// A `CSimpleEditBox`'s runtime state; `E+` offsets are from its `CScriptObject` `this`. Byte
/// offsets stay on char boundaries, as the client's boundary snap (`0x77bd30`) keeps them.
#[derive(Clone, Debug, PartialEq)]
pub struct EditBoxState {
    /// The real text (`E+0x32c`); `password` masks only the display (`E+0x334`, `0x77d4d0`).
    pub text: String,
    /// The caret, a byte offset into the text (`E+0x36c`, clamped to `[0, len]`). It can sit
    /// anywhere against the selection: `HighlightText` writes only the selection.
    pub cursor: usize,
    /// Selection start (`E+0x35c`); equal to `sel_end` when nothing is selected.
    pub sel_start: usize,
    /// Selection end (`E+0x360`); an insert replaces a non-empty selection first (`0x77cd70`).
    pub sel_end: usize,
    /// `autoFocus` (`E+0x318` bit 0): take the focus when shown if nothing holds it (the OnShow
    /// override `0x77a750`, `0x77a76d`) and on a key or char event while nothing is focused.
    pub auto_focus: bool,
    /// `multiLine` (bit 1): Enter inserts a newline instead of firing `OnEnterPressed`.
    pub multi_line: bool,
    /// `numeric` (bit 2): an insert with any non-digit is refused whole, not filtered (`0x77bf41`).
    pub numeric: bool,
    /// `password` (bit 3): the display is one `*` per character (`E+0x334`); the text is untouched.
    pub password: bool,
    /// Alt-arrow mode (bit 4; XML `ignoreArrows`, Lua `SetAltArrowKeyMode`): a focused box lets the
    /// four arrows through to their bindings unless Alt (`0x41f8f0(2)`) is held (`0x77b18e`), so
    /// you can turn while typing. It consumes every other key (`0x77b35e`).
    pub alt_arrow_key_mode: bool,
    /// `maxLetters` (`E+0x340`), 0 for unlimited.
    pub max_letters: usize,
    /// `maxBytes` (`E+0x33c`); the client's `-1`, unlimited, is `None`.
    pub max_bytes: Option<usize>,
    /// The implicit FontString the text renders through (`E+0x328`, ctor `0x779bee`).
    pub text_region: Option<RegionHandle>,
    /// The three selection-highlight textures (`E+0x350`/`0x354`/`0x358`, ctor `0x779c41`). They
    /// paint nothing: the host draws the highlight from `sel_start`/`sel_end`.
    pub selection_regions: [Option<RegionHandle>; 3],
    /// The caret texture (`E+0x368`, ctor `0x779c86`), a solid quad above the text. It paints
    /// nothing: the host draws the caret from `caret_shown`.
    pub caret_region: Option<RegionHandle>,
    /// The submitted lines, oldest first, which UP and DOWN recall in a single-line box
    /// ([`Self::history_step`]).
    pub history: Vec<String>,
    /// `historyLines`, the most lines kept; 0 (the default) is no history.
    pub history_max: usize,
    /// The recall position, `None` while editing. `SetText` must not end the browse: the chat
    /// parser rewrites a recalled slash line through it.
    pub history_pos: Option<usize>,
    /// The live line stashed when browsing starts, restored when DOWN walks past the newest entry.
    pub history_draft: Option<String>,
    /// `SetTextInsets(l, r, t, b)` (`+0x3ec`, `+0x3f4`, `+0x3f0`, `+0x3e8`), which seat the text
    /// region and pad a multi-line box's height.
    pub text_insets: [f32; 4],
    /// One line's measured height in the text region's font (`0x7727b0(fs, 1)`), the height of an
    /// empty multi-line box's text, with the measure key it was taken under.
    pub line_height: Option<(u64, f32)>,
    /// The box's rect when its resize was last checked, `ApplyRect`'s old rect (`0x76b580`);
    /// `None` before its first resolve, which compares against the ctor's zero rect.
    pub notified_rect: Option<crate::layout::Rect>,
    /// The host-measured width of `display[..i]` per byte `i`, a continuation byte repeating its
    /// lead's; hit-testing (`0x77d0d0`) and the scroll window read it. Empty until answered.
    pub advances: Vec<f32>,
    /// Hash of the display text and font [`Self::advances`] was measured for.
    pub advances_key: u64,
    /// The display byte each wrapped row starts at, measured by the host with the draw's wrap pass;
    /// at least `[0]`. Bytes swallowed at a break (the space, the `\n`) belong to the row they end.
    pub rows: Vec<usize>,
    /// The row pitch in pixels (the snapped font em), answered with the advances; 0 until then.
    pub cell_h: f32,
    /// The dirty word (`[E+0x31c]`) the box's flush (`0x77d3e0`) drains: [`Self::DIRTY_TEXT`] and
    /// [`Self::DIRTY_CURSOR`]. The ctor sets bit 0 (`0x779a34`), so a box fires `OnTextChanged` at
    /// its first flush once shown.
    pub dirty: u8,
    /// A multi-line relayout that found its text's measure pending, retried at each flush until it
    /// lands; with no font engine installed the measure arrives from the host a tick later.
    pub relayout_owed: bool,
    /// The first visible display byte of a single-line box (`E+0x348`), scrolled by whole chars
    /// to keep the caret in view; `0x77da80` hides a caret outside the window.
    pub scroll_start: usize,
    /// A left-button press in the box is held (`E+0x364`); moves extend the selection (`0x77a860`).
    pub drag_active: bool,
    /// Caret blink half-period in seconds (`E+0x370`; ctor 0.5, XML `blinkSpeed`).
    pub blink_period: f32,
    /// Blink accumulator (`E+0x374`): grows while focused; crossing the period toggles the caret.
    pub blink_accum: f32,
    /// The caret shows this half-period. The flush's caret leg, which every caret move, edit and
    /// focus change raises, turns it on in the focused box (`0x77de35`), off in others
    /// (`0x77de51`).
    pub caret_shown: bool,
    /// The selection tint (`SetHighlightColor`), RGBA 0..1; the ctor default is `0xFF606060`.
    pub highlight_color: [f32; 4],
    /// The bits of `Set/GetJustifyH` and `Set/GetJustifyV` (`0x797990`-`0x797bd0`), kept on the
    /// box and not drawn. For V that is the reference: `SetMultiLine` (`0x77a4a0`) makes the
    /// text's V bits local, so a `SetJustifyV` is masked out (`0x77086e`) and only the getter sees
    /// it. Whether the draw (`0x77da80`) reads H is untraced.
    pub justify: u32,
}

impl EditBoxState {
    /// The horizontal justify bits (0-2).
    pub const JUSTIFY_H_MASK: u32 = crate::justify::H_MASK;
    /// The vertical justify bits (3-5).
    pub const JUSTIFY_V_MASK: u32 = crate::justify::V_MASK;
    /// Dirty bit 0, the text changed: every edit raises it, and the flush relayouts (`0x77d447`)
    /// and fires `OnTextChanged` (`0x77d498`).
    pub const DIRTY_TEXT: u8 = 1;
    /// Dirty bit 2, the caret: the cursor setter (`0x77e380`), a step that moves it (`0x77c73b`),
    /// an edit, a focus change and a re-seat of the text raise it, and the flush runs the caret
    /// leg (`0x77d475` → `0x77da80`), which fires `OnCursorChanged`. Bit 1, the
    /// highlight's (`0x77d950`), has no counterpart: the host paints the selection each frame.
    pub const DIRTY_CURSOR: u8 = 4;

    /// A justify token's bit; `None` makes the caller raise the reference's
    /// `Usage: %s:SetJustifyH("justify")`.
    pub fn justify_bit(token: &str) -> Option<u32> {
        crate::justify::parse_bits(token)
    }

    /// Replace one axis's bits with `parsed`'s. `SetJustifyH("TOP")` parses but clears the H bits,
    /// so `GetJustifyH()` then answers `"UNKNOWN"`, with no error.
    pub fn set_justify_axis(&mut self, mask: u32, parsed: u32) {
        self.justify = crate::justify::set_axis(self.justify, mask, parsed);
    }

    /// The token for one axis.
    pub fn justify_token(&self, mask: u32) -> &'static str {
        crate::justify::name_of(self.justify, mask)
    }
}

impl Default for EditBoxState {
    fn default() -> Self {
        EditBoxState {
            text: String::new(),
            cursor: 0,
            sel_start: 0,
            sel_end: 0,
            // The ctor sets the flags to 1 (`0x779a29`/`0x779a2e`), autoFocus alone, so a box
            // that says nothing self-focuses when shown.
            auto_focus: true,
            multi_line: false,
            numeric: false,
            password: false,
            alt_arrow_key_mode: false,
            max_letters: 0,
            max_bytes: None,
            text_region: None,
            history: Vec::new(),
            history_max: 0,
            history_pos: None,
            history_draft: None,
            text_insets: [0.0; 4],
            line_height: None,
            notified_rect: None,
            advances: Vec::new(),
            advances_key: 0,
            rows: vec![0],
            cell_h: 0.0,
            dirty: Self::DIRTY_TEXT,
            relayout_owed: false,
            scroll_start: 0,
            drag_active: false,
            blink_period: 0.5,
            blink_accum: 0.0,
            caret_shown: true,
            // Filled by `WidgetArena::create`'s ctor pass, which `Default` cannot reach.
            selection_regions: [None; 3],
            caret_region: None,
            highlight_color: [96.0 / 255.0, 96.0 / 255.0, 96.0 / 255.0, 1.0],
            // The ctor's `0x211` less the unread bit 0x200: LEFT | MIDDLE, since the ctor
            // overrides the font default's CENTER (`0x779be4`).
            justify: 0x01 | 0x10,
        }
    }
}

impl EditBoxState {
    /// `AddHistoryLine`: append the line, dropping the oldest past the cap, and end any browse.
    pub fn add_history_line(&mut self, line: &str) {
        if self.history_max == 0 || line.is_empty() {
            return;
        }
        self.history.push(line.to_string());
        let over = self.history.len().saturating_sub(self.history_max);
        if over > 0 {
            self.history.drain(..over);
        }
        self.history_pos = None;
        self.history_draft = None;
    }

    /// One UP (`older`) or DOWN step, returning the text to show: the first UP stashes the draft,
    /// and DOWN past the newest entry restores it.
    ///
    /// Deviation: the history browses as a shell's does, a small everyday gain: the first UP
    /// stashes the line being typed and DOWN past the newest line brings it back, UP holds at the
    /// oldest, DOWN does nothing when not browsing, typing or a focus gain ends the browse, and an
    /// empty line is not kept. The reference's history is a ring of `historyLines` slots that
    /// keeps any line (`0x77cf40`); UP scans back for the previous filled slot, wrapping
    /// (`0x77d030`), DOWN forward with `idiv` (`0x77cfd0`), each a `SetText` (`0x77be00`), with no
    /// draft and no stop at either end.
    pub fn history_step(&mut self, older: bool) -> Option<String> {
        if self.history.is_empty() {
            return None;
        }
        match (self.history_pos, older) {
            (None, true) => {
                self.history_draft = Some(self.text.clone());
                self.history_pos = Some(self.history.len() - 1);
            }
            (None, false) => return None,
            (Some(0), true) => return None, // at the oldest: hold
            (Some(p), true) => self.history_pos = Some(p - 1),
            (Some(p), false) if p + 1 < self.history.len() => self.history_pos = Some(p + 1),
            (Some(_), false) => {
                // Past the newest: back to the stashed draft.
                self.history_pos = None;
                return Some(self.history_draft.take().unwrap_or_default());
            }
        }
        self.history_pos.map(|p| self.history[p].clone())
    }

    /// End any history browse; `SetText` must not call this (see [`Self::history_pos`]).
    pub fn end_history_browse(&mut self) {
        self.history_pos = None;
        self.history_draft = None;
    }

    // ── selection / geometry law ─────────────────────────────────────────────────────────────

    /// What the box draws and hit-tests: the text, or one `*` per character under `password`
    /// (`E+0x334`; the draw `0x77da80` and the hit-test `0x77d0d0` both branch on the flag).
    pub fn display(&self) -> String {
        if self.password {
            "*".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    /// A text byte offset as a display byte offset.
    pub fn text_to_display(&self, byte: usize) -> usize {
        if self.password {
            self.text[..byte.min(self.text.len())].chars().count()
        } else {
            byte
        }
    }

    /// A display byte offset as a text byte offset.
    pub fn display_to_text(&self, dbyte: usize) -> usize {
        if self.password {
            self.text
                .char_indices()
                .nth(dbyte)
                .map_or(self.text.len(), |(b, _)| b)
        } else {
            dbyte
        }
    }

    /// The display index nearest pixel `x` from the text origin, the caller adding the scroll
    /// offset. It rounds to the nearest stop; `0x77d0d0`'s rounding is untraced.
    pub fn index_at_x(&self, x: f32) -> usize {
        let display = self.display();
        self.index_at_x_in(x, 0, display.len(), &display)
    }

    /// [`Self::index_at_x`] within `[start, end]`, `x` measured from `advances[start]`. It lands
    /// only on cursor stops: the hit-test walks tokens with `atomicLinks = 0` (`0x77d0d0`, at
    /// `0x77d2f6`), so a click reaches each visible character of a link but never an escape.
    fn index_at_x_in(&self, x: f32, start: usize, end: usize, display: &str) -> usize {
        if self.advances.len() != display.len() + 1 {
            return end;
        }
        let origin = self.advances[start];
        let mut prev = start;
        for b in self.stops_in(start, end, display) {
            if self.advances[b] - origin >= x {
                // x lies between stops `prev` and `b`: pick the nearer.
                return if x - (self.advances[prev] - origin) <= (self.advances[b] - origin) - x {
                    prev
                } else {
                    b
                };
            }
            prev = b;
        }
        end
    }

    /// The cursor stops in `(start, end]`, in order, walked with links not atomic (the mouse walk);
    /// always ends at `end`.
    fn stops_in(&self, start: usize, end: usize, display: &str) -> Vec<usize> {
        let map = crate::markup::ClassMap::new(display);
        let mut stops = Vec::new();
        let mut at = start;
        loop {
            let next = map.advance(at, 1, false).min(end);
            if next <= at {
                break;
            }
            stops.push(next);
            at = next;
        }
        if stops.last() != Some(&end) {
            stops.push(end);
        }
        stops
    }

    /// The wrapped row holding display byte `b`; a byte on a wrap boundary heads the new row.
    pub fn row_of(&self, b: usize) -> usize {
        match self.rows.binary_search(&b) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    /// Row `i`'s display byte range, with the break bytes it swallowed at its tail.
    pub fn row_range(&self, i: usize, display_len: usize) -> (usize, usize) {
        let start = self.rows.get(i).copied().unwrap_or(0);
        let end = self.rows.get(i + 1).copied().unwrap_or(display_len);
        (start, end.max(start))
    }

    /// The caret's wrapped `(row, x)`, `x` measured from the row's start.
    pub fn caret_row_x(&self, cursor_d: usize) -> (usize, f32) {
        let display = self.display();
        if self.advances.len() != display.len() + 1 {
            return (0, 0.0);
        }
        let cursor_d = cursor_d.min(display.len());
        let row = self.row_of(cursor_d);
        let start = self.rows.get(row).copied().unwrap_or(0).min(display.len());
        (row, self.advances[cursor_d] - self.advances[start])
    }

    /// The display index nearest `(x, y)` (`0x77d0d0`): `y`, down from the text top, picks the row
    /// by the pitch and `x` walks that row. Without rows it is [`Self::index_at_x`].
    pub fn index_at_pos(&self, x: f32, y: f32) -> usize {
        let display = self.display();
        if self.rows.len() <= 1 || self.cell_h <= 0.0 {
            return self.index_at_x(x);
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = ((y / self.cell_h).floor().max(0.0) as usize).min(self.rows.len() - 1);
        let (start, end) = self.row_range(row, display.len());
        self.index_at_x_in(
            x,
            start.min(display.len()),
            end.min(display.len()),
            &display,
        )
    }

    /// The Ctrl+arrow target. A word here is a run of ASCII alphanumerics or non-ASCII bytes; the
    /// reference walks its per-byte class array (`E+0x330`).
    pub fn word_boundary(&self, forward: bool) -> usize {
        let bytes = self.text.as_bytes();
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b >= 0x80;
        if forward {
            let mut i = self.cursor;
            while i < bytes.len() && !is_word(bytes[i]) {
                i += 1;
            }
            while i < bytes.len() && is_word(bytes[i]) {
                i += 1;
            }
            i
        } else {
            let mut i = self.cursor;
            while i > 0 && !is_word(bytes[i - 1]) {
                i -= 1;
            }
            while i > 0 && is_word(bytes[i - 1]) {
                i -= 1;
            }
            i
        }
    }

    /// Show the caret and restart its blink, as the flush's caret leg does for the focused box
    /// (`0x77dd68`, `0x77de35`); the glue fields, which have no flush, call it on each change.
    pub fn reset_blink(&mut self) {
        self.caret_shown = true;
        self.blink_accum = 0.0;
    }

    /// Scroll the single-line window (`E+0x348`) by whole chars to keep the caret in `avail` px.
    pub fn clamp_scroll(&mut self, avail: f32) {
        if self.multi_line {
            // A multiline box wraps; its parent ScrollFrame scrolls it.
            self.scroll_start = 0;
            return;
        }
        let display = self.display();
        if self.advances.len() != display.len() + 1 || avail <= 0.0 {
            return;
        }
        let cursor_d = self.text_to_display(self.cursor);
        // Snap a stale start back onto a char boundary.
        self.scroll_start = self.scroll_start.min(display.len());
        while self.scroll_start > 0 && !display.is_char_boundary(self.scroll_start) {
            self.scroll_start -= 1;
        }
        if cursor_d < self.scroll_start {
            self.scroll_start = cursor_d;
            return;
        }
        while self.advances[cursor_d] - self.advances[self.scroll_start] > avail {
            // Advance the window one char; the loop is bounded by cursor_d.
            let mut next = self.scroll_start + 1;
            while next < display.len() && !display.is_char_boundary(next) {
                next += 1;
            }
            if next > cursor_d {
                break;
            }
            self.scroll_start = next;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The editing law: pure over the state, no Lua, no widget tree
// ─────────────────────────────────────────────────────────────────────────────────────────────
//
// `script::editbox` wraps these and fires the Lua events an `EditOutcome` names; the glue
// screens use them with no Lua at all.

/// What an edit changed, for the FrameXML wrapper's `OnTextChanged` and `OnSpacePressed` fires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditOutcome {
    /// The text changed: `OnTextChanged`.
    pub text_changed: bool,
    /// An insert went in, so `OnChar` fires (`0x77c13c`); a `numeric` abort that only deleted the
    /// selection did not.
    pub inserted: bool,
    /// Typed spaces inserted, one `OnSpacePressed` each; a paste reports none.
    pub spaces: usize,
}

impl EditOutcome {
    /// An outcome with nothing inserted.
    fn changed(text_changed: bool) -> Self {
        EditOutcome {
            text_changed,
            ..EditOutcome::default()
        }
    }
}

impl EditBoxState {
    /// Apply one [`EditAction`] from the host's per-OS chord table.
    pub fn apply(&mut self, action: EditAction) -> EditOutcome {
        match action {
            EditAction::Move { unit, back, extend } => {
                match unit {
                    // The alt-arrow gate is on the key: the host declines a gated arrow before
                    // it becomes an action (`UiScript::editbox_alt_arrow_mode`).
                    EditUnit::Char => self.move_by_char(!back, extend),
                    EditUnit::Word => self.move_by_word(!back, extend),
                    EditUnit::Line => self.move_to_line_edge(!back, extend),
                    EditUnit::Edge => self.move_to_edge(!back, extend),
                    EditUnit::Row if self.multi_line => self.move_by_row(!back, extend),
                    // The caller recalls history through `SetText`, so `OnTextSet` fires.
                    EditUnit::Row => {}
                }
                EditOutcome::default()
            }
            EditAction::Delete { unit, back } => EditOutcome::changed(match unit {
                EditUnit::Char => self.delete_dir(!back),
                EditUnit::Word => {
                    let t = self.word_boundary(!back);
                    self.delete_to(t)
                }
                EditUnit::Edge => {
                    let t = if back { 0 } else { self.text.len() };
                    self.delete_to(t)
                }
                // No keymap deletes by line or row.
                EditUnit::Line | EditUnit::Row => false,
            }),
            EditAction::SelectAll => {
                self.highlight_text(0, -1);
                EditOutcome::default()
            }
        }
    }

    /// Insert at the cursor (`0x77bee0`), in its order: the link guard, the selection's deletion
    /// (`0x77bf13`–`0x77bf23`), then the `numeric` test (`0x77bf41`), which refuses the whole
    /// insert on any non-digit and leaves the deletion standing.
    pub fn insert(&mut self, ins: &str) -> EditOutcome {
        // Typing with the caret inside a hyperlink, where only the mouse can put it, is
        // swallowed: the opening guard of `0x77bee0`.
        if !crate::markup::ClassMap::new(&self.text).insert_allowed(self.cursor) {
            return EditOutcome::default();
        }
        let replaced = self.sel_start != self.sel_end;
        self.delete_selection();
        if self.numeric && !ins.chars().all(|c| c.is_ascii_digit()) {
            if replaced {
                self.end_history_browse();
                self.reset_blink();
            }
            return EditOutcome::changed(replaced);
        }
        self.end_history_browse();
        self.text.insert_str(self.cursor, ins);
        self.cursor += ins.len();
        self.collapse();
        self.enforce_caps();
        self.reset_blink();
        self.dirty |= Self::DIRTY_TEXT | Self::DIRTY_CURSOR; // `0x77c033 or edx,5`
        EditOutcome {
            text_changed: true,
            inserted: true,
            spaces: ins.matches(' ').count(),
        }
    }

    /// Insert clipboard text with its control characters dropped, bar `\n` in a multiline box; a
    /// paste fires no `OnSpacePressed`.
    pub fn paste(&mut self, text: &str) -> EditOutcome {
        let cleaned: String = text
            .chars()
            .filter(|&c| c as u32 >= 0x20 || (self.multi_line && c == '\n'))
            .collect();
        if cleaned.is_empty() {
            return EditOutcome::default();
        }
        EditOutcome::changed(self.insert(&cleaned).text_changed)
    }

    /// `SetText` (`0x77be00`), in order: collapse the selection even when the text is equal; stop
    /// if equal (`SStrCmp`, case-sensitive, `0x77be4b`), firing nothing; else clear all
    /// (`0x77c500`), then `Insert` (`0x77bee0`) with its gates. So a `numeric` box given a
    /// non-digit, as by `SetNumber(-5)`, ends empty and still reports a change.
    pub fn set_text(&mut self, s: &str) -> bool {
        self.collapse(); // even when the text is equal
        if self.text == s {
            return false;
        }
        // Clear all, then Insert, whose `numeric` gate refuses the whole string.
        self.text = if self.numeric && !s.chars().all(|c| c.is_ascii_digit()) {
            String::new()
        } else {
            s.to_string()
        };
        self.cursor = self.text.len();
        self.collapse();
        self.enforce_caps();
        self.reset_blink();
        self.dirty |= Self::DIRTY_TEXT | Self::DIRTY_CURSOR;
        true
    }

    /// The selection for Ctrl+C (`0x77e1d0`); a password box yields its mask, never the text.
    pub fn selected_text(&self) -> Option<String> {
        if self.sel_start == self.sel_end {
            return None;
        }
        let (a, b) = (
            self.sel_start.min(self.sel_end),
            self.sel_start.max(self.sel_end),
        );
        Some(if self.password {
            "*".repeat(self.text[a..b].chars().count())
        } else {
            self.text[a..b].to_string()
        })
    }

    /// Ctrl+X: [`selected_text`](Self::selected_text), then delete the selection.
    pub fn cut_selection(&mut self) -> Option<String> {
        let taken = self.selected_text()?;
        self.end_history_browse();
        self.delete_selection();
        self.reset_blink();
        Some(taken)
    }

    /// `HighlightText` (`0x77cca0`) with the client's clamp, so `(0, -1)` selects all. It writes
    /// only the selection (`+0x35c`/`+0x360`) and raises bit 1 (`0x77ccc8`): the caret stays put,
    /// so no `OnCursorChanged` fires and the blink runs on.
    pub fn highlight_text(&mut self, start: i64, end: i64) {
        let len = self.text.len() as i64;
        let s = start.clamp(0, len);
        let mut e = if end < 0 || end > len { len } else { end };
        if e < s {
            e = len;
        }
        self.sel_start = snap_down(&self.text, s as usize);
        self.sel_end = snap_down(&self.text, e as usize);
    }

    /// Backspace (`forward = false`) or Delete: the selection if any, else one step.
    pub fn delete_dir(&mut self, forward: bool) -> bool {
        self.end_history_browse();
        let did = 'del: {
            if self.sel_start != self.sel_end {
                self.delete_selection();
                break 'del true;
            }
            // One atomic token step (`0x77c280`, `atomicLinks = 1` at `0x77c2a3`), then the span
            // snap (`0x77c510`): one Backspace takes a whole item link, escapes and `|r` included.
            let target = crate::markup::ClassMap::new(&self.text).advance(
                self.cursor,
                if forward { 1 } else { -1 },
                true,
            );
            if target != self.cursor {
                self.delete_span(target.min(self.cursor)..target.max(self.cursor));
                break 'del true;
            }
            false
        };
        if did {
            self.reset_blink();
        }
        did
    }

    /// Word or edge delete: the selection if any, else the span from the caret to `target`.
    pub fn delete_to(&mut self, target: usize) -> bool {
        self.end_history_browse();
        let did = if self.sel_start != self.sel_end {
            self.delete_selection();
            true
        } else {
            let (a, b) = (target.min(self.cursor), target.max(self.cursor));
            if a == b {
                false
            } else {
                self.delete_span(a..b);
                true
            }
        };
        if did {
            self.reset_blink();
        }
        did
    }

    /// Left or Right one step, `extend` dragging the selection ([`Self::extend_to`]). Bit 2 and
    /// the blink's restart come only when the caret moved: the helpers step it only while it can
    /// (`0x77c750` `cursor < len`, `0x77c870` `cursor > 0`), and the step raises the bit
    /// (`0x77c73b`), whose flush restarts the blink (`0x77dd68`).
    pub fn move_by_char(&mut self, right: bool, extend: bool) {
        let from = self.cursor;
        // One token step, links atomic (`0x77bb30`, `atomicLinks = 1` at `0x77c6d2`): a press
        // crosses a whole link, never into an escape, and Shift+arrow selects all of it.
        let target = crate::markup::ClassMap::new(&self.text).advance(
            self.cursor,
            if right { 1 } else { -1 },
            true,
        );
        if extend {
            self.extend_to(target);
        } else if self.sel_start != self.sel_end {
            // Deviation: a plain Left or Right with a selection collapses it to that edge, as
            // every OS's own text fields do; the reference clears the selection and steps once
            // from the caret (`0x77c6b0` from `0x77c70b`).
            self.cursor = if right { self.sel_end } else { self.sel_start };
            self.collapse();
        } else {
            self.cursor = target;
            self.collapse();
        }
        if self.cursor != from {
            self.reset_blink();
            self.dirty |= Self::DIRTY_CURSOR;
        }
    }

    /// Ctrl/Option+arrow: the caret to the [`word_boundary`](Self::word_boundary) by single atomic
    /// steps (`0x77c8c0`/`0x77c7a0` loop `0x77c6b0`), so it always lands on a reachable stop.
    pub fn move_by_word(&mut self, right: bool, extend: bool) {
        let word = self.word_boundary(right);
        self.walk(right, extend, |_, _, next| {
            if right {
                next <= word
            } else {
                next >= word
            }
        });
    }

    /// HOME/END: the caret back to its line's start or on to its end, a step at a time while the
    /// byte before the caret (`0x77c99a`), or at it (`0x77ca0e`), is not a newline. With no
    /// newline it goes to the text's edge, as in a single-line box whose text was typed: typing
    /// drops a newline there (`0x77c234`–`0x77c240`), though `SetText` keeps one.
    pub fn move_to_line_edge(&mut self, end: bool, extend: bool) {
        self.walk(end, extend, |text, at, _| {
            let bytes = text.as_bytes();
            if end {
                bytes.get(at) != Some(&b'\n')
            } else {
                at > 0 && bytes[at - 1] != b'\n'
            }
        });
    }

    /// Ctrl+HOME/END and Cmd+Up/Down: the caret to `0` / `len`, a step at a time (`0x77ca60`,
    /// `0x77cac0`).
    pub fn move_to_edge(&mut self, end: bool, extend: bool) {
        self.walk(end, extend, |_, _, _| true);
    }

    /// UP/DOWN in a multi-line box (`0x77cb20`, through `0x77cc80`/`0x77cc60`): the caret to the
    /// same column of the wrapped row above or below. The column is the letters from the row's
    /// start to the caret (`0x77bc80`), walked out from the target row's start with links not
    /// atomic (`0x77bb30`, `atomicLinks = 0` pushed at `0x77cba4`). A walk that reaches the next
    /// row's start steps back one stop (`0x77cbc6`–`0x77cbdd`). On the first or last row the caret
    /// stays, and without `extend` the selection clears (`0x77cb50`). The rows are the draw's
    /// ([`Self::rows`]), one while they are unmeasured.
    ///
    /// Deviation: on the last row a walk may end at the text's end, because the end starts no row;
    /// the reference steps back from it as from a row's start (its line table ends with the
    /// text's length, `0x77d6ef`), so DOWN onto a short last line stops before its last letter.
    pub fn move_by_row(&mut self, down: bool, extend: bool) {
        let len = self.text.len();
        let display_len = self.display().len();
        let measured = self.advances.len() == display_len + 1
            && self.rows.first() == Some(&0)
            && self.rows.windows(2).all(|w| w[0] < w[1])
            && self.rows.last().is_some_and(|&r| r <= display_len);
        // Each row's first text byte, then the text's length.
        let mut starts: Vec<usize> = if measured {
            self.rows
                .iter()
                .map(|&d| snap_down(&self.text, self.display_to_text(d)))
                .collect()
        } else {
            vec![0]
        };
        let rows = starts.len();
        starts.push(len);
        // The caret's row (`0x77bd50`): a caret on a row's start heads that row.
        let mut row = 0;
        while row + 1 < rows && self.cursor >= starts[row + 1] {
            row += 1;
        }
        let target = if down {
            (row + 1).min(rows - 1)
        } else {
            row.saturating_sub(1)
        };
        if target == row {
            if !extend {
                self.collapse();
            }
            return;
        }
        let map = crate::markup::ClassMap::new(&self.text);
        let column = map.letters(starts[row], self.cursor - starts[row]);
        let (start, next) = (starts[target], starts[target + 1]);
        let mut to = map.advance(start, column as isize, false);
        if target + 1 < rows && next > start && to >= next {
            to = map.advance(next, -1, false);
        }
        if extend {
            self.extend_to(to);
        } else {
            self.cursor = to;
            self.collapse();
        }
        self.reset_blink();
        self.dirty |= Self::DIRTY_CURSOR; // `0x77cc40`
    }

    /// The loops of single steps behind the word, line and edge moves (`0x77c6b0`, links atomic):
    /// the caret steps while `take(text, caret, next)` allows the step to `next`, each extending
    /// step dragging the selection by [`Self::extend_to`]; a plain move collapses it at the end.
    fn walk(&mut self, right: bool, extend: bool, take: impl Fn(&str, usize, usize) -> bool) {
        let from = self.cursor;
        let map = crate::markup::ClassMap::new(&self.text);
        loop {
            let next = map.advance(self.cursor, if right { 1 } else { -1 }, true);
            if next == self.cursor || !take(&self.text, self.cursor, next) {
                break;
            }
            if extend {
                self.extend_to(next);
            } else {
                self.cursor = next;
            }
        }
        if !extend {
            self.collapse();
        }
        if self.cursor != from {
            self.reset_blink();
            self.dirty |= Self::DIRTY_CURSOR;
        }
    }

    /// One extending caret move to `to`, the Shift leg of `0x77c6b0` and `0x77cb20`: an empty
    /// selection first collapses onto the caret (`0x77ccf0`), then `0x77cd10` moves one edge by
    /// the move's delta. A move left past `sel_start` moves `sel_start`, any other move left moves
    /// `sel_end`, and a move right mirrors that, so a caret inside a selection trims it.
    pub fn extend_to(&mut self, to: usize) {
        if self.sel_start == self.sel_end {
            self.collapse();
        }
        if to < self.cursor {
            if to < self.sel_start {
                self.sel_start = to;
            } else {
                self.sel_end = to;
            }
        } else if to > self.cursor {
            if to > self.sel_end {
                self.sel_end = to;
            } else {
                self.sel_start = to;
            }
        }
        self.cursor = to;
    }

    /// Place the caret at `target`, extending the selection from its anchor when `extend`: the
    /// click (`0x77b800`) and the drag (`0x77a860`). Bit 2 rises only when the caret moved; a
    /// click raises it on its own.
    ///
    /// Deviation: the drag selects from its anchor, as every OS's own text fields do, where the
    /// reference's applies `0x77cd10` to each mouse move's whole delta (`0x77a8a1`). The two part
    /// in two cases. One move that carries the caret across the anchor keeps the far end selected
    /// too in the reference. And a drag that starts with the caret inside a selection, as after a
    /// focus that selects all, gives `(0, t)` here dragging right to `t`, where the reference
    /// gives `(t, len)`.
    pub fn move_caret_to(&mut self, target: usize, extend: bool) {
        let from = self.cursor;
        let target = snap_down(&self.text, target.min(self.text.len()));
        if extend {
            let anchor = self.selection_anchor();
            self.cursor = target;
            self.set_span(anchor, target);
        } else {
            self.cursor = target;
            self.collapse();
        }
        self.reset_blink();
        if self.cursor != from {
            self.dirty |= Self::DIRTY_CURSOR;
        }
    }

    fn delete_selection(&mut self) {
        if self.sel_start == self.sel_end {
            return;
        }
        let (a, b) = (
            self.sel_start.min(self.sel_end),
            self.sel_start.max(self.sel_end),
        );
        self.delete_span(a..b);
    }

    /// Remove a byte span widened to whole hyperlinks (`0x77c510`), so a drag selection that clips
    /// a link deletes all of it; the caret lands at the span's start.
    fn delete_span(&mut self, span: std::ops::Range<usize>) {
        let span = crate::markup::ClassMap::new(&self.text).snap_delete_range(span);
        self.cursor = span.start;
        self.text.replace_range(span, "");
        self.collapse();
        self.dirty |= Self::DIRTY_TEXT | Self::DIRTY_CURSOR; // `0x77c683 or ecx,5`
    }

    /// Collapse the selection onto the caret (`0x77ccf0`), as every delete does and as a screen
    /// losing the keyboard does to the box it leaves.
    pub fn collapse(&mut self) {
        self.sel_start = self.cursor;
        self.sel_end = self.cursor;
    }

    /// The selection end that is not the caret.
    fn selection_anchor(&self) -> usize {
        if self.sel_start == self.cursor {
            self.sel_end
        } else {
            self.sel_start
        }
    }

    fn set_span(&mut self, a: usize, b: usize) {
        self.sel_start = a.min(b);
        self.sel_end = a.max(b);
    }

    /// After an edit, trim from the end to `maxBytes`, then `maxLetters` (`0x77c02d`).
    fn enforce_caps(&mut self) {
        if let Some(mb) = self.max_bytes {
            while self.text.len() > mb {
                self.text.pop();
            }
        }
        if self.max_letters > 0 {
            // Letters, not chars: `0x77bc80` counts classes 2, 3 and 6, so an item link costs only
            // its visible name. The trim is the Backspace step (`0x77c280(-1)`, links atomic),
            // re-counting after each pop (`0x77c0e4`), so it sheds a whole link at once.
            loop {
                let map = crate::markup::ClassMap::new(&self.text);
                if map.num_letters() <= self.max_letters {
                    break;
                }
                let cut = map.advance(self.text.len(), -1, true);
                if cut >= self.text.len() {
                    break;
                }
                self.text.truncate(cut);
            }
        }
        let len = self.text.len();
        self.cursor = snap_down(&self.text, self.cursor.min(len));
        self.sel_start = snap_down(&self.text, self.sel_start.min(len));
        self.sel_end = snap_down(&self.text, self.sel_end.min(len));
    }
}

/// The char boundary at or below `i`.
fn snap_down(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod row_law_tests {
    use super::*;

    /// "the quick brown" wrapped after "quick" (the break's space swallowed): rows [0, 10],
    /// advances 7 px per byte, 14 px pitch.
    fn two_row_box() -> EditBoxState {
        EditBoxState {
            text: "the quick brown".into(),
            multi_line: true,
            advances: (0..=15).map(|i| i as f32 * 7.0).collect(),
            rows: vec![0, 10],
            cell_h: 14.0,
            ..EditBoxState::default()
        }
    }

    #[test]
    fn the_caret_seats_by_row_and_row_local_x() {
        let eb = two_row_box();
        assert_eq!(eb.caret_row_x(5), (0, 35.0));
        // A cursor on the wrap boundary heads the new row.
        assert_eq!(eb.caret_row_x(10), (1, 0.0));
        assert_eq!(eb.caret_row_x(15), (1, 35.0));
    }

    #[test]
    fn a_click_picks_its_row_then_walks_it() {
        let eb = two_row_box();
        // Row 1 (y past one pitch), x 21: the row's third stop, byte 13.
        assert_eq!(eb.index_at_pos(21.0, 20.0), 13);
        // Above the block clamps to row 0; below clamps to the last row.
        assert_eq!(eb.index_at_pos(0.0, -5.0), 0);
        assert_eq!(eb.index_at_pos(9999.0, 999.0), 15);
        // Clicking past a wrapped row's ink lands at its wrap point, not the next row.
        assert_eq!(eb.index_at_pos(9999.0, 5.0), 10);
    }

    #[test]
    fn a_single_line_box_degrades_to_the_1d_walk() {
        let eb = EditBoxState {
            text: "abc".into(),
            advances: vec![0.0, 7.0, 14.0, 21.0],
            ..EditBoxState::default()
        };
        assert_eq!(eb.index_at_pos(15.0, 500.0), 2);
    }
}

#[cfg(test)]
#[path = "editbox_move_tests.rs"]
mod move_law_tests;
