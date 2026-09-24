use super::RegionHandle;

/// The text span a cursor motion or deletion operates over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditUnit {
    /// One character (the plain arrow / Backspace / Delete granularity).
    Char,
    /// One word run ([`EditBoxState::word_boundary`] — the Ctrl/Option-arrow granularity).
    Word,
    /// The line edge: text start going back, text end going forward (Home/End, Cmd+arrow).
    Edge,
}

/// One semantic text-editing operation on an edit box — fed to [`EditBoxState::apply`]. The host's
/// per-OS keymap (which physical chord means which action: Ctrl+Left on Windows, Option+Left on
/// macOS, …) translates key events into these; the *effect* of each action is the reference's own
/// edit-box law (selection anchoring, selection-first deletes, word classes). Clipboard operations
/// are deliberately absent: they need the OS pasteboard, so they stay host-side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    /// Move the caret one `unit` back/forward; `extend` drags the selection from its fixed
    /// anchor (the Shift family). (The alt-arrow gate is upstream, on the key — the
    /// ref guard `0x77b18e`; arrows are the only chord source of `Char` moves).
    Move {
        unit: EditUnit,
        back: bool,
        extend: bool,
    },
    /// Delete one `unit` back/forward from the caret — the selection first when one exists
    /// (every deletion gesture collapses to "delete the selection"). `Edge` going back is the
    /// macOS Cmd+Backspace "clear to start".
    Delete { unit: EditUnit, back: bool },
    /// Select the whole text, caret to the end (the ref's Ctrl+A, `HighlightText(0, -1)`).
    SelectAll,
    /// Recall the previous (older) submitted line into the box (`historyLines`).
    HistoryPrev,
    /// Step back toward the newest line; past it, restore the stashed draft.
    HistoryNext,
}

/// A `CSimpleEditBox`'s runtime state — the text/cursor/selection/flags model.
/// Offsets below are the client's **E-base** (the CScriptObject `this`), the base the runtime
/// input/text handlers address the object through.
///
/// The client stores text as a NUL-terminated `char*` with a parallel per-byte class array; benilla
/// holds it as a Rust `String` and keeps every byte offset ([`cursor`](Self::cursor)/
/// [`sel_start`](Self::sel_start)/[`sel_end`](Self::sel_end)) snapped to a UTF-8 char boundary — the
/// same invariant the client's boundary-snap (`0x77bd30`) maintains.
#[derive(Clone, Debug, PartialEq)]
pub struct EditBoxState {
    /// The real text buffer (`E+0x32c`, `char*`). `GetText`/`GetNumber` read this; `password` masks
    /// only the *display* (`E+0x334`, rebuilt by `0x77d4d0`), never this buffer.
    pub text: String,
    /// The insertion caret as a byte offset into [`text`](Self::text) (`E+0x36c`; sole setter
    /// clamps to `[0, len]`). Always on a char boundary.
    pub cursor: usize,
    /// Selection anchor (`E+0x35c`) — equal to [`sel_end`](Self::sel_end) (and the cursor) when no
    /// text is selected. Byte offset, char-boundary-snapped.
    pub sel_start: usize,
    /// Selection end (`E+0x360`). A non-empty selection is `sel_start != sel_end`; every insert
    /// replaces it first (`0x77cd70`).
    pub sel_end: usize,
    /// `autoFocus` (`flags@E+0x318` bit0). Two acquisition paths, and both are built:
    ///
    /// 1. **On SHOW** — the box self-focuses when it becomes visible, gated on nothing else holding
    ///    focus. The EditBox's own OnShow vtable override (`0x81c910` slot +0x30, `0x77a750`) fires
    ///    the Lua handler and then tail-jumps `SetFocus`:
    ///    `if ([0xcf4dc8] == 0 && (flags & 1)) jmp 0x77e3d0` @`0x77a76d`. The mirror slot +0x34
    ///    (`0x77a780`) tail-jumps `ClearFocus`, so **hiding an edit box releases the keyboard**.
    ///    Ours is [`crate::script::editbox`]'s `visibility_focus`.
    /// 2. **On the first key/char event** while nothing is focused — the self-acquire guard; the box
    ///    grabs focus and processes that same event.
    ///
    /// **Both corrections landed together, decision 1686 (2026-08-29).** The construction default
    /// came with it and is byte-read, not chosen — see [`Default`]'s `flags = 1` note below. Every
    /// `autoFocus` in the shipped 1.12.1 chain is `="false"`, ten opt-outs and no opt-ins, which is
    /// the authoring signature of exactly that default.
    pub auto_focus: bool,
    /// `multiLine` (bit1): Enter inserts a newline (rather than firing `OnEnterPressed`) and `\n` is
    /// accepted into the buffer.
    pub multi_line: bool,
    /// `numeric` (bit2): an insert containing ANY char outside `'0'..='9'` is aborted wholesale
    /// (not per-char filtered) — the char filter `0x77bf41`.
    pub numeric: bool,
    /// `password` (bit3): the display string is one `'*'` per *character* (the mask, `E+0x334`); the
    /// real text is untouched.
    pub password: bool,
    /// **Alt-arrow mode** (bit4) — the XML attribute spells it `ignoreArrows`, the Lua surface
    /// spells it `SetAltArrowKeyMode`/`GetAltArrowKeyMode` (`0x7996e0`/`0x799790`), and there is
    /// exactly one flag behind both names. `SetIgnoreArrows` does **not** exist in 5875: the
    /// 48-entry EditBox method table `[0x87bb68, 0x87bce8)` carries the Alt pair at 46/47 and no
    /// entry whose name contains "Ignore".
    ///
    /// **What it does, and it is not what "ignore" suggests.** With the flag set, the four arrow
    /// keys (`0x204` LEFT / `0x205` UP / `0x206` RIGHT / `0x207` DOWN) are **not consumed by the
    /// box at all** unless ALT is held — the guard `0x77b18e` returns 0, the strata walk carries
    /// on down to `CGWorldFrame`, and the key reaches its binding (`TURNLEFT`, `MOVEFORWARD`).
    /// That is the whole point: it is what lets you turn while the chat box has focus. With Alt
    /// held the guard falls through to ordinary handling and the caret moves.
    ///
    /// A focused box otherwise consumes **everything** (the handler's shared tail `0x77b35e`
    /// returns 1), so these four keys are the only ones it ever lets past.
    ///
    /// benilla read this as "consumed but inert, unless Ctrl" for two rounds — the modifier was
    /// wrong (`0x41f8f0(2)` is ALT, not Ctrl) and so was the consumption. Both are corrected above.
    pub alt_arrow_key_mode: bool,
    /// `maxLetters` (`E+0x340`, 0 = unlimited): after each insert, trim from the end while the
    /// *letter* (char) count exceeds this (`0x77c02d`).
    pub max_letters: usize,
    /// `maxBytes` (`E+0x33c`; the client's `-1` sentinel = unlimited → `None` here): trim from the
    /// end while the byte length exceeds this, applied before `maxLetters`.
    pub max_bytes: Option<usize>,
    /// The implicit FontString the text renders through (`E+0x328`, the EditBox's analogue of
    /// ButtonText). **Built by the ctor**, not lazily: `0x779bee` constructs it from allocator
    /// `0xcf4d10` with tag `0x846544` = `".?AVCSimpleFontString@@"` via `0x770d30(E, 2, 1)`, and
    /// that is the FIRST of the five regions a `CSimpleEditBox` is born with. Creation order is
    /// what `GetRegions 0x773f60` hands Lua — one flat creation-ordered list off `[frame+0x1b8]`,
    /// oldest first, no filter — so these five precede every `<Layers>` region.
    ///
    /// **DEFERRED, and named rather than silently taken:** the ctor's draw layer is 2 sub 1
    /// (ARTWORK), while this region is still created at OVERLAY sub 0 — what ships today and what
    /// the director has looked at. Moving it is a draw-order change to every EditBox in the client
    /// with nothing asking for it, so it is a separate change, not a rider on this one.
    pub text_region: Option<RegionHandle>,
    /// The **three selection-highlight quads** (`E+0x350`/`0x354`/`0x358`), built by the ctor loop
    /// `0x779c41–0x779c72` as `CSimpleTexture`s via `0x76fc40(E, 2, 0)` — allocator `0xcf4ce0`, tag
    /// `0x846588` = `".?AVCSimpleTexture@@"`, class-identical to the caret. They are
    /// members of the frame's region list like any other region (`0x76fc40`→`0x77f640`→`0x77fd10`,
    /// the single linker into `[frame+0x1b8]`), which is why they are built here.
    ///
    /// **They carry no art of their own and paint nothing.** benilla draws the selection highlight
    /// host-side, from [`sel_start`](Self::sel_start)/[`sel_end`](Self::sel_end) through the seam —
    /// so these are the real regions the client has, not yet the regions the highlight rides.
    /// Routing the paint through them is a separate, larger change (the host would have to read a
    /// region rect instead of a seam field); this is stated, not stubbed.
    pub selection_regions: [Option<RegionHandle>; 3],
    /// The **caret** (`E+0x368`), the ctor's fifth and last region: a `CSimpleTexture` built at
    /// `0x779c86–0x779cac` via `0x76fc40(E, 3, 1)` — a solid vertex-coloured quad, never a glyph
    /// (no texture path is ever assigned to `E+0x368`, band-wide census). Layer 3 over
    /// the text's 2 is why the caret draws above the text and the selection quads below it.
    ///
    /// Same standing as [`selection_regions`](Self::selection_regions): the region is real and in
    /// the list, the *painting* is still host-side off [`caret_shown`](Self::caret_shown).
    pub caret_region: Option<RegionHandle>,
    /// The submitted-line history (`AddHistoryLine`; the XML `historyLines` cap) — oldest first,
    /// newest last; UP recalls older from the end, DOWN newer. The exact recall keys + draft model
    /// are inferred: the 1.12 history controller is an untraced observer (`0x77b730`) — plain
    /// UP/DOWN with the in-progress line restored past the newest entry; still open on the chat arc.
    pub history: Vec<String>,
    /// `historyLines` — max entries kept (drop-oldest on add). `0` = history off (the widget
    /// default; ChatFrame's edit box declares 32).
    pub history_max: usize,
    /// The active recall position while browsing (an index into [`Self::history`]); `None` = live
    /// editing. Typed edits, [`Self::add_history_line`], and focus gain end browsing —
    /// programmatic `SetText` deliberately does NOT (the chat live parse rewrites the box on
    /// every recalled slash line; ending the browse there pinned every UP to the newest entry).
    pub history_pos: Option<usize>,
    /// The live line stashed when browsing starts, restored when DOWN walks past the newest entry.
    pub history_draft: Option<String>,
    /// `SetTextInsets(l, r, t, b)` / XML `<TextInsets>` — the text region's rect shrink inside the
    /// box (the chat edit box drives its left inset past the "Say:" header every header change).
    /// Applied as the text region's two corner anchors (`script::editbox::set_text_insets`).
    pub text_insets: [f32; 4],
    /// The focused box's per-byte cumulative advance table — the host's answer to the advance
    /// measure request: `advances[i]` = laid-out width of `display[..i]` (len+1 entries; a
    /// continuation byte repeats its lead's value). This is the metrics seam that makes
    /// click→index (`0x77d0d0`), drag-select, and the scroll window engine-local — the real
    /// client reads the same geometry off its embedded render object. Empty until answered.
    pub advances: Vec<f32>,
    /// Cache key of [`Self::advances`] (display text + font identity hash) — the same staleness
    /// discipline as the FontString measure round-trip.
    pub advances_key: u64,
    /// The wrapped-row starts of the DISPLAY string — answered with [`Self::advances`] by the
    /// host, which runs the same wrap pass the draw uses. `rows[i]` is the byte where row `i`
    /// begins; row `i` covers `rows[i]..rows[i+1]` (last row to end of display). Always at least
    /// `[0]`; a single-line box stays exactly `[0]`. The 2-D caret/click law
    /// ([`Self::caret_row_x`]/[`Self::index_at_pos`]) reads it; bytes swallowed at a break (the
    /// dropped trailing separator, the `\n` itself) belong to the row they end.
    pub rows: Vec<usize>,
    /// The row pitch in px (the snapped font em — the same `N·S` block law the host's measure
    /// uses), answered with [`Self::advances`]. `0.0` until answered.
    pub cell_h: f32,
    /// The `(row, x)` the last `OnCursorChanged` was fired with — the caret-flush edge's memory
    /// (`0x77da80`, whose one caller `0x77d475` is gated on dirty bit 2, so the fire is per
    /// *change*, not per frame). `None` = never fired, which is what makes the first flush after
    /// a focus fire even though the caret is at the home position.
    pub cursor_fired: Option<(usize, f32)>,
    /// First visible byte of the display window (`E+0x348` display/scroll start index) — the
    /// char-granular h-scroll. Clamped each read so the cursor stays inside the window
    /// (`0x77da80`'s early-out hides the caret outside `[start, start+visible]`).
    pub scroll_start: usize,
    /// A LeftButton press landed inside the box and hasn't released (`E+0x364`
    /// mouse-drag-active): every mouse move maps to a char index and extends the selection there,
    /// cursor following (`0x77a860`).
    pub drag_active: bool,
    /// Caret blink half-period in seconds (`E+0x370`; ctor default **0.5**, the XML `blinkSpeed`
    /// attr overrides).
    pub blink_period: f32,
    /// Blink accumulator (`E+0x374`): += dt while focused; crossing the period toggles
    /// [`Self::caret_shown`] and resets.
    pub blink_accum: f32,
    /// The blink phase — caret visible this half-period. Every cursor/text/selection change
    /// resets it to `true` with a fresh accumulator (the client's click path calls the dirty
    /// flush's blink reset before placing the cursor).
    pub caret_shown: bool,
    /// The selection highlight tint (`SetHighlightColor`, the `E+0x350` texture trio's color;
    /// ctor default **0xFF606060** — opaque medium gray). RGBA 0..1.
    pub highlight_color: [f32; 4],
    /// The justification bit word behind `Set/GetJustifyH` and `Set/GetJustifyV` — entries #12–#15
    /// of the EditBox method table (`0x797990`/`0x797a50`/`0x797b10`/`0x797bd0`, tail-calling the
    /// shared `0x79fc20`/`0x79fcb0`/`0x79fce0`/`0x79fd70`).
    ///
    /// **One dword, two axes** (`CSimpleFont+0x54` / `CSimpleFontString+0x120`), from the 6-entry
    /// enum at `.rdata 0x811ad0`: bits 0–2 horizontal (`LEFT` 0x01, `CENTER` 0x02, `RIGHT` 0x04),
    /// bits 3–5 vertical (`TOP` 0x08, `MIDDLE` 0x10, `BOTTOM` 0x20).
    ///
    /// **An EditBox's default is `0x211` — LEFT, not the generic CENTER.** The `CSimpleFont` ctor
    /// default really is `0x212` (`CENTER | MIDDLE | 0x200`), but the EditBox constructor
    /// *overrides* the horizontal axis immediately after linking its font instance —
    /// `0x779bcd mov ecx,[edi+0x54]; and eax,~6; or eax,1; 0x779be4 mov [edi+0x54],eax` — so
    /// `GetJustifyH()` on a fresh box answers **`"LEFT"`**. Bit `0x200` falls outside both axis
    /// masks and neither accessor reads it, so only `0x11` is modelled. (This **corrected** our
    /// first cut, which took the generic `0x212` and answered `"CENTER"`.)
    ///
    /// **Kept on the box, not on its text region.** Each font binding's shim hands the shared
    /// implementation `[this+0x324]` — the box's implicit FontString, i.e. [`Self::text_region`] —
    /// so on the reference these do write that string's justify field. But our EditBox draw law
    /// seats that region's vertical justification by `multiLine` (TOP / MIDDLE) and its horizontal
    /// one LEFT, and writing the region here would fight that law *and* be clobbered by the next
    /// `SetMultiLine`.
    ///
    /// **The vertical half of that is now confirmed exactly right, and for a reason worth
    /// keeping.** `CSimpleFontString+0x124` is a per-bit *inherit* mask over
    /// `+0x120`, and `SetMultiLine 0x77a4a0` clears the whole vertical group `0x38` from it on
    /// **both** legs while writing the V bits locally (multi-line → TOP, single-line → MIDDLE) —
    /// and the EditBox ctor calls `SetMultiLine` unconditionally at birth (`0x779c2f`, with the
    /// ctor's zero register). A census of all 256 `+0x124` operands image-wide found every
    /// `CSimpleFontString` writer to be an AND: **nothing ever ORs an inherit bit back.** So
    /// `SetJustifyV` writes the instance and is masked out at `0x77086e`, never reaching the
    /// rendered text, while `GetJustifyV` reads the instance (`0x79fd73`) and echoes it back:
    /// **the getter and the pixels disagree permanently, by construction, on the real client too.**
    /// Our rendered V justify is decided solely by `multiLine`, which is the same rule.
    ///
    /// **The horizontal half stays open, and is deliberately not guessed.** `SetMultiLine` never
    /// clears the `0x7` bits, so `SetJustifyH`'s value *does* reach the FontString's `+0x120` —
    /// but whether the editbox's own draw `0x77da80` reads it for placement, rather than
    /// left-anchoring at the insets rect the way the reference's windowed draw describes, is
    /// untraced.
    /// Verifying that the value propagates is not verifying that the draw honours it, so the
    /// reading that changes no pixels is the one taken and the question is named. It is the whole
    /// visible difference for `AceGUIWidget-Slider.lua:210`'s `editbox:SetJustifyH("CENTER")`
    /// (3 corpus addons), whose call now round-trips either way.
    pub justify: u32,
}

impl EditBoxState {
    /// The horizontal axis mask (bits 0–2) — `SetJustifyH`'s `(cur & ~7) | (parsed & 7)`.
    pub const JUSTIFY_H_MASK: u32 = crate::justify::H_MASK;
    /// The vertical axis mask (bits 3–5) — `SetJustifyV`'s.
    pub const JUSTIFY_V_MASK: u32 = crate::justify::V_MASK;

    /// Parse a justify token to its bit — [`crate::justify::parse_bits`]. `None` = no match, which
    /// is what makes the caller raise `Usage: %s:SetJustifyH("justify")`; the reference checks this
    /// one, unlike `SetTextColor`.
    pub fn justify_bit(token: &str) -> Option<u32> {
        crate::justify::parse_bits(token)
    }

    /// Replace one axis's bits with `parsed`'s — [`crate::justify::set_axis`].
    ///
    /// **The verified trap this reproduces:** `SetJustifyH("TOP")` *parses* (0x08) but
    /// `0x08 & 0x07 == 0`, so it **clears** justifyH and a later `GetJustifyH()` answers
    /// `"UNKNOWN"` — no error is raised. A plausible implementation that maps unknown-to-CENTER
    /// silently answers something the client never would; the FontString and `<Font>` tables both
    /// did exactly that until this law was lifted out of here and shared with them.
    pub fn set_justify_axis(&mut self, mask: u32, parsed: u32) {
        self.justify = crate::justify::set_axis(self.justify, mask, parsed);
    }

    /// The token for one axis — [`crate::justify::name_of`].
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
            // `flags = 1` at construction (`0x779a29`/`0x779a2e`): bit0 (autoFocus) SET, every
            // other flag clear. A box that says nothing self-focuses when shown.
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
            advances: Vec::new(),
            advances_key: 0,
            rows: vec![0],
            cell_h: 0.0,
            cursor_fired: None,
            scroll_start: 0,
            drag_active: false,
            blink_period: 0.5,
            blink_accum: 0.0,
            caret_shown: true,
            // Filled by `Arena::create`'s ctor pass — `Default` cannot reach the arena, and a
            // handle-less default is what a not-yet-created box has.
            selection_regions: [None; 3],
            caret_region: None,
            highlight_color: [96.0 / 255.0, 96.0 / 255.0, 96.0 / 255.0, 1.0],
            // The EditBox ctor's `0x211` minus the unread bit 0x200 — `LEFT | MIDDLE`. LEFT, not
            // the generic font default CENTER: `0x779be4` overrides the H axis at construction.
            justify: 0x01 | 0x10,
        }
    }
}

impl EditBoxState {
    /// `AddHistoryLine`: append (drop-oldest past [`Self::history_max`]) and end any browse in
    /// progress. Empty lines and a zero cap are no-ops.
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

    /// One UP (`older = true`) or DOWN step through the history. Returns the text the box should
    /// now show: entering browse mode stashes the live draft; stepping past the newest entry
    /// leaves browse mode and returns the draft. `None` = nothing to do (no history / already at
    /// the oldest / not browsing on DOWN).
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
            (Some(0), true) => return None, // already at the oldest — hold
            (Some(p), true) => self.history_pos = Some(p - 1),
            (Some(p), false) if p + 1 < self.history.len() => self.history_pos = Some(p + 1),
            (Some(_), false) => {
                // Past the newest — back to the stashed live line.
                self.history_pos = None;
                return Some(self.history_draft.take().unwrap_or_default());
            }
        }
        self.history_pos.map(|p| self.history[p].clone())
    }

    /// End any history browse (the recalled line becomes an ordinary draft; the stash is
    /// dropped). Called on typed edits, [`Self::add_history_line`], and focus gain — never on
    /// programmatic `SetText` (see [`Self::history_pos`]).
    pub fn end_history_browse(&mut self) {
        self.history_pos = None;
        self.history_draft = None;
    }

    // ── selection / geometry law ─────────────────────────────────────────────────────────────

    /// The DISPLAY string — what the box draws, what the advance table indexes, and what
    /// hit-tests run against: the text itself, or one `'*'` per character under `password`
    /// (the `E+0x334` mask; both draw `0x77da80` and hit-test `0x77d0d0` branch on the flag).
    pub fn display(&self) -> String {
        if self.password {
            "*".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    /// TEXT byte offset → DISPLAY byte offset (identity unless `password`, where each char is
    /// one `'*'` byte).
    pub fn text_to_display(&self, byte: usize) -> usize {
        if self.password {
            self.text[..byte.min(self.text.len())].chars().count()
        } else {
            byte
        }
    }

    /// DISPLAY byte offset → TEXT byte offset (inverse of [`Self::text_to_display`]).
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

    /// The char-boundary DISPLAY index nearest pixel `x` (x measured from the text origin, i.e.
    /// the advance table's zero — the caller adds the scroll window's offset first). Walks the
    /// cumulative table over char boundaries; nearest-boundary rounding (INFERRED — `0x77d0d0`'s
    /// exact rounding is undiffed; nearest is the Windows-edit convention). Empty table (host
    /// hasn't answered yet) → end of text.
    pub fn index_at_x(&self, x: f32) -> usize {
        let display = self.display();
        self.index_at_x_in(x, 0, display.len(), &display)
    }

    /// [`Self::index_at_x`] restricted to the boundary range `[start, end]` of `display` —
    /// the per-row walk of the 2-D law ([`Self::index_at_pos`]). `x` is measured from
    /// `advances[start]`.
    ///
    /// The candidates are the **reachable cursor stops**, not every char boundary: the client's
    /// hit-test converts its glyph index through the same token walk everything else uses, with
    /// `atomicLinks = 0` (`0x77d0d0`, `6a 00` @`0x77d2f6`), so a click stops on each visible
    /// character of a link's text but can never land inside an escape.
    fn index_at_x_in(&self, x: f32, start: usize, end: usize, display: &str) -> usize {
        if self.advances.len() != display.len() + 1 {
            return end;
        }
        let origin = self.advances[start];
        let mut prev = start;
        for b in self.stops_in(start, end, display) {
            if self.advances[b] - origin >= x {
                // x lies between boundaries `prev` and `b` — pick the nearer.
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

    /// The cursor stops strictly inside `(start, end]` of `display`, in order — the mouse-path walk
    /// of [`crate::markup::ClassMap::advance`] (`atomic_links = false`). Always ends at `end`.
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

    /// The wrapped row index containing DISPLAY byte `b` — the last row whose start is ≤ `b`
    /// (a byte exactly on a wrap boundary belongs to the row it *starts*, so the caret lands at
    /// the head of the new row, where typing continues).
    pub fn row_of(&self, b: usize) -> usize {
        match self.rows.binary_search(&b) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    /// Row `i`'s DISPLAY byte range `[start, end)` — `end` is the next row's start (the swallowed
    /// break bytes live at the tail of this range), or the display length for the last row.
    pub fn row_range(&self, i: usize, display_len: usize) -> (usize, usize) {
        let start = self.rows.get(i).copied().unwrap_or(0);
        let end = self.rows.get(i + 1).copied().unwrap_or(display_len);
        (start, end.max(start))
    }

    /// The caret's wrapped position: `(row, x)` with `x` the advance from the row's start —
    /// the 2-D twin of the single-line `advances[cursor]` read. Row 0 / x 0 until the host has
    /// answered the advance table.
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

    /// The char-boundary DISPLAY index nearest point `(x, y)` — the multiline click law
    /// (`0x77d0d0`'s 2-D half): `y` (px down from the text region's top) picks the row by the
    /// answered pitch, `x` walks that row's advances. Degrades to the 1-D walk while the rows/
    /// pitch are unanswered (single-line boxes stay exactly the old behavior). The row's END
    /// boundary excludes trailing break bytes only in x (clicking past a wrapped row's ink lands
    /// at the wrap point).
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

    /// The word-jump target from the cursor (Ctrl+arrow — the client walks its per-byte class
    /// array `E+0x330`; benilla approximates the classes with alphanumeric runs, INFERRED):
    /// forward = end of the current/next alnum run; back = start of the current/previous one.
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

    /// Show the caret and restart its blink cycle — the client's dirty flush resets the blink on
    /// every cursor/text/selection change (and the click path calls it before placing the cursor).
    pub fn reset_blink(&mut self) {
        self.caret_shown = true;
        self.blink_accum = 0.0;
    }

    /// Clamp the scroll window (`E+0x348`, a DISPLAY-byte start index) so the caret stays inside
    /// `avail` pixels: window start ≤ cursor, and the cursor's advance fits within the window's
    /// width. Whole-char steps (the client scrolls its display window by characters, never
    /// sub-pixel). No-op until the host has answered the advance table.
    pub fn clamp_scroll(&mut self, avail: f32) {
        if self.multi_line {
            // A multiline box wraps instead of windowing (the client's h-scroll `E+0x348` is the
            // single-line mechanism; multiline scrolls via its parent ScrollFrame).
            self.scroll_start = 0;
            return;
        }
        let display = self.display();
        if self.advances.len() != display.len() + 1 || avail <= 0.0 {
            return;
        }
        let cursor_d = self.text_to_display(self.cursor);
        // Snap a stale start (text shrank / mid-char) back onto a boundary.
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
// The editing law — pure over the state, no Lua, no widget tree
// ─────────────────────────────────────────────────────────────────────────────────────────────
//
// These used to live in `script::editbox` welded to the Lua layer, which meant the *only* way to
// get the byte-verified box law was to be a FrameXML EditBox. The glue screens (login, character
// create, the delete dialog) therefore each grew their own three-case imitation of it — append a
// char, Backspace, Tab — with no caret movement, no selection, and no clipboard.
// The law lives here now, and `script::editbox` is a thin wrapper that calls these and fires the
// Lua events an [`EditOutcome`] tells it to. Anything with a `&mut EditBoxState` gets the real
// law, whether or not there is a Lua VM anywhere near it.

/// What a pure edit changed, so a caller with events to fire knows which to fire. The FrameXML
/// wrapper turns this into `OnTextChanged`/`OnSpacePressed`; the glue screens ignore it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditOutcome {
    /// The text buffer changed — the `OnTextChanged` trigger.
    pub text_changed: bool,
    /// Spaces this edit inserted — the `OnSpacePressed` fire count. Only an insert of typed text
    /// reports these; a paste deliberately does not (a paste is not a typed space).
    pub spaces: usize,
}

impl EditOutcome {
    /// A change that fires `OnTextChanged` and nothing else.
    fn changed(text_changed: bool) -> Self {
        EditOutcome {
            text_changed,
            spaces: 0,
        }
    }
}

impl EditBoxState {
    /// Apply one semantic [`EditAction`] — the single entry point the host's per-OS chord table
    /// feeds (the host owns *which chord*, this owns *what it does*).
    pub fn apply(&mut self, action: EditAction) -> EditOutcome {
        match action {
            EditAction::Move { unit, back, extend } => {
                match unit {
                    // No flag test here any more. The alt-arrow gate is on the KEY, not on the
                    // action — with the flag set the box never sees the arrow at all, because the
                    // host declines it before the chord is dispatched (`UiScript::
                    // editbox_alt_arrow_fallthrough`). Anything that reaches this far was either
                    // Alt-held or not an arrow, and both move the caret normally.
                    EditUnit::Char => self.move_by_char(!back, extend),
                    EditUnit::Word => self.move_by_word(!back, extend),
                    EditUnit::Edge => self.move_to_edge(!back, extend),
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
            }),
            EditAction::SelectAll => {
                self.highlight_text(0, -1);
                EditOutcome::default()
            }
            // History recall is the caller's: it routes through the SetText path so the FrameXML
            // box also fires `OnTextSet`. `history_step` is the state half.
            EditAction::HistoryPrev | EditAction::HistoryNext => EditOutcome::default(),
        }
    }

    /// Insert `ins` at the cursor (`0x77bee0`): replace any selection first; `numeric` aborts the
    /// insert **wholesale** on any non-digit (not per-char filtering — the char filter `0x77bf41`);
    /// splice, advance the caret, enforce the caps.
    pub fn insert(&mut self, ins: &str) -> EditOutcome {
        if self.numeric && !ins.chars().all(|c| c.is_ascii_digit()) {
            return EditOutcome::default();
        }
        // Refused outright with the caret strictly inside a hyperlink — the opening guard of
        // `0x77bee0`. Only the mouse can put the caret there (the keyboard treats a
        // link as one unit), and the client then silently swallows the typing rather than letting
        // it split `|Hitem:…|h[Name]|h` into something unclickable.
        if !crate::markup::ClassMap::new(&self.text).insert_allowed(self.cursor) {
            return EditOutcome::default();
        }
        self.end_history_browse(); // a typed edit turns a recalled line into an ordinary draft
        self.delete_selection();
        self.text.insert_str(self.cursor, ins);
        self.cursor += ins.len();
        self.collapse();
        self.enforce_caps();
        self.reset_blink();
        EditOutcome {
            text_changed: true,
            spaces: ins.matches(' ').count(),
        }
    }

    /// Insert OS-clipboard text: [`insert`](Self::insert) with the paste sanitation — every control
    /// character is dropped, except `\n` into a multiline box. Reports no spaces: a paste is not a
    /// typed space, so it fires no `OnSpacePressed`.
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

    /// `SetText` (`0x77be00`) is a clear-all followed by `Insert`, not a plain assignment, and the
    /// order of its three parts is load-bearing:
    ///
    /// 1. the selection collapses **unconditionally**, before anything is compared — an identical
    ///    `SetText` still drops a highlight;
    /// 2. a case-sensitive equality test then short-circuits (`SStrCmp` at `0x77be4b`), skipping the
    ///    clear-all, the insert and both fires — so the caller fires nothing;
    /// 3. the changing leg is `0x77c500` (delete the whole buffer) then `0x77bee0` (`Insert`), which
    ///    means `SetText` **inherits `Insert`'s gates**. The one that shows is `numeric`: the gate
    ///    abandons the insert wholesale on any non-digit, and the clear-all has already run, so the
    ///    box is left **empty** rather than unchanged. `SetNumber(-5)` and `SetNumber(0.8)` both
    ///    empty a `numeric` box, because `-` and `.` are not digits.
    ///
    /// The clear-all raises the `textChanged` dirty bit on every path it reaches, so the aborted
    /// insert still reports a change and still fires — reporting `""`.
    pub fn set_text(&mut self, s: &str) -> bool {
        self.collapse(); // (1) unconditional, ahead of the comparison
        if self.text == s {
            return false; // (2)
        }
        // (3) the clear-all, then Insert — whose `numeric` gate refuses the whole string.
        self.text = if self.numeric && !s.chars().all(|c| c.is_ascii_digit()) {
            String::new()
        } else {
            s.to_string()
        };
        self.cursor = self.text.len();
        self.collapse();
        self.enforce_caps();
        self.reset_blink();
        true
    }

    /// The selected substring, or `None` with no selection (Ctrl+C, `0x77e1d0`). A password box
    /// yields its **mask run**, never the real text — the client copies a placeholder.
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

    /// `HighlightText` (`0x77cca0`), the client's exact clamp: `start = clamp(start, 0..=len)`;
    /// `end = (end < 0 || end > len) ? len : end`; then `if end < start { end = len }` — so
    /// `(0, -1)` selects all. Byte offsets, snapped to char boundaries.
    pub fn highlight_text(&mut self, start: i64, end: i64) {
        let len = self.text.len() as i64;
        let s = start.clamp(0, len);
        let mut e = if end < 0 || end > len { len } else { end };
        if e < s {
            e = len;
        }
        self.sel_start = snap_down(&self.text, s as usize);
        self.sel_end = snap_down(&self.text, e as usize);
        self.cursor = self.sel_end;
        self.reset_blink();
    }

    /// BACKSPACE / DELETE: the selection when there is one, else the char before (`forward=false`)
    /// or after the caret. `true` when anything was removed.
    pub fn delete_dir(&mut self, forward: bool) -> bool {
        self.end_history_browse();
        let did = 'del: {
            if self.sel_start != self.sel_end {
                self.delete_selection();
                break 'del true;
            }
            // One ATOMIC token step, then the endpoint snap — `0x77c280(±1)` passes
            // `atomicLinks = 1` (`6a 01` @`0x77c2a3`) and hands the span to `0x77c510`, so one
            // BACKSPACE takes a whole item link, escapes and trailing `|r` included (1077).
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

    /// Word/edge delete: the selection first when one exists, else the span between the caret and
    /// `target`. `true` when anything was removed.
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

    /// LEFT/RIGHT one char: `extend` drags the selection from its fixed anchor; otherwise the caret
    /// collapses onto the selection edge (when there is one) or steps one char.
    pub fn move_by_char(&mut self, right: bool, extend: bool) {
        // One TOKEN step, links atomic — `0x77bb30(±1, atomicLinks = 1)`, which every arrow path
        // reaches (`6a 01` @`0x77c6d2`). So one press crosses a whole `|cff…|Hitem:…|h[Name]|h|r`
        // rather than stepping into the middle of an escape, and Shift+arrow selects all of it.
        // Not a char step: an escape byte is not a cursor position.
        let step = |s: &str, i: usize| {
            crate::markup::ClassMap::new(s).advance(i, if right { 1 } else { -1 }, true)
        };
        if extend {
            let anchor = self.selection_anchor();
            self.cursor = step(&self.text, self.cursor);
            self.set_span(anchor, self.cursor);
        } else if self.sel_start != self.sel_end {
            self.cursor = if right { self.sel_end } else { self.sel_start };
            self.collapse();
        } else {
            self.cursor = step(&self.text, self.cursor);
            self.collapse();
        }
        self.reset_blink();
    }

    /// Ctrl/Option+arrow: the caret to the next [`word_boundary`](Self::word_boundary) — reached as
    /// a **loop of single atomic steps** (`0x77c8c0`/`0x77c7a0` loop `0x77c6b0`), so the landing
    /// place is always a reachable stop even when the word target falls inside an escape or
    /// part-way through a link.
    pub fn move_by_word(&mut self, right: bool, extend: bool) {
        let word = self.word_boundary(right);
        let mut target = self.cursor;
        loop {
            let next = crate::markup::ClassMap::new(&self.text).advance(
                target,
                if right { 1 } else { -1 },
                true,
            );
            if next == target || (right && next > word) || (!right && next < word) {
                break;
            }
            target = next;
            if target == word {
                break;
            }
        }
        self.move_caret_to(target, extend);
    }

    /// HOME/END (and Cmd+arrow): the caret to `0` / `len`.
    pub fn move_to_edge(&mut self, end: bool, extend: bool) {
        let target = if end { self.text.len() } else { 0 };
        self.move_caret_to(target, extend);
    }

    /// Place the caret at `target`, extending the selection from its fixed anchor when `extend`.
    /// The shared tail of every non-char move (and of a mouse click/drag).
    pub fn move_caret_to(&mut self, target: usize, extend: bool) {
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
    }

    /// Delete the current selection: remove `[min, max)` and collapse the caret to its left edge.
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

    /// Remove a byte span, first widening it out of any hyperlink it cuts — `0x77c510` (decision
    /// 1077), the second guarantee under the atomic walk: a mouse-drag selection that clips half an
    /// item link still deletes the whole `|c…|H…|h[text]|h|r` unit rather than leaving orphaned
    /// escape bytes. The caret collapses to the widened span's left edge.
    fn delete_span(&mut self, span: std::ops::Range<usize>) {
        let span = crate::markup::ClassMap::new(&self.text).snap_delete_range(span);
        self.cursor = span.start;
        self.text.replace_range(span, "");
        self.collapse();
    }

    /// Collapse the selection onto the caret — the client's own `0x77ccf0`, which every delete
    /// path runs and which a screen losing the keyboard runs on the box it is leaving.
    pub fn collapse(&mut self) {
        self.sel_start = self.cursor;
        self.sel_end = self.cursor;
    }

    /// The fixed end of the selection while extending — the endpoint that is *not* the caret (the
    /// caret itself when nothing is selected).
    fn selection_anchor(&self) -> usize {
        if self.sel_start == self.cursor {
            self.sel_end
        } else {
            self.sel_start
        }
    }

    /// Set the selection to `[min(a,b), max(a,b)]`.
    fn set_span(&mut self, a: usize, b: usize) {
        self.sel_start = a.min(b);
        self.sel_end = a.max(b);
    }

    /// Enforce the length caps after an edit (`0x77c02d`): trim whole chars from the end while over
    /// `maxBytes`, then while over `maxLetters`; clamp caret/selection back into the buffer.
    fn enforce_caps(&mut self) {
        if let Some(mb) = self.max_bytes {
            while self.text.len() > mb {
                self.text.pop();
            }
        }
        if self.max_letters > 0 {
            // LETTERS, not chars: `0x77bc80` counts classes 2, 3 and 6 only, so a 48-byte item link
            // costs 14 against `maxLetters` — its visible `[Chipped Claw]` and nothing more.
            // Counting raw chars made a 255-letter chat line fill up three times
            // too fast once it held links.
            //
            // The trim pops through `0x77c280(-1)` — the BACKSPACE primitive, `atomicLinks = 1` —
            // re-counting the whole buffer after every pop (`0x77c0e4`). So an over-long buffer
            // sheds a whole hyperlink in one bite, exactly as a keypress would, and can never shear
            // an escape in half.
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

/// The byte offset of the char boundary at or below `i` (identity if `i` is already on one).
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
        // A cursor exactly on the wrap boundary heads the NEW row (typing continues there).
        assert_eq!(eb.caret_row_x(10), (1, 0.0));
        assert_eq!(eb.caret_row_x(15), (1, 35.0));
    }

    #[test]
    fn a_click_picks_its_row_then_walks_it() {
        let eb = two_row_box();
        // Row 1 (y past one pitch), x 21 → boundary 3 within the row → byte 13.
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
