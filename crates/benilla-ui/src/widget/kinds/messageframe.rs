use std::collections::VecDeque;

/// One line held in a [`ScrollingMessageState`] ring — its text, its already-quantized color, its
/// live fade state (a per-line snapshot of the frame's `timeVisible`/`fadeDuration` at insert,
/// MessageData `+0xc`/`+0x10`), and its host-measured wrapped row count (the message-line half of
/// the measure round-trip — a long chat line occupies as many display rows as it wraps into, so the
/// bands above it shift up by real content height).
#[derive(Clone, Debug, PartialEq)]
pub struct MessageLine {
    /// The line text (already formatted app-side: `[Name]: text`, `Name yells: text`, …).
    pub text: String,
    /// The RGB the line draws at, **byte-quantized** at insert (`AddMessage` `trunc(x*255+0.5)`,
    /// round-half-up; alpha is never stored — it is forced opaque and then driven by the fade).
    pub color: [u8; 3],
    /// `AddMessage`'s fifth argument — the chat-type index the line was printed under
    /// (`GetChatTypeIndex`), so `UpdateColorByID` can find every line of a type when
    /// `UPDATE_CHAT_COLOR` lands. 0 for a line printed without one.
    pub id: u32,
    /// Remaining phase-1 countdown (the `timeVisible` snapshot ticking down); while `> 0` the line
    /// holds full alpha.
    pub time_left: f32,
    /// Remaining phase-2 countdown (the `fadeDuration` snapshot). Once `time_left` hits `0` this
    /// counts down and drives [`Self::alpha`].
    pub fade_left: f32,
    /// The current display alpha in `[0, 1]`. Phase 1 leaves it at whatever `AddMessage` set —
    /// forced opaque on a ScrollingMessageFrame (`792add: or edi,0xffffff00`), the caller's real
    /// alpha arg on a MessageFrame (`795752`, default 1.0) — and phase 2 **overwrites** it with the
    /// byte-quantized ramp `trunc(remaining/fadeDuration*255)`, the client's own store into the
    /// line's alpha byte (`0x788547` / `786364`→`[edi+0xb]`). `0` once fully faded — in a
    /// ScrollingMessageFrame the line stays in the ring (a slot is freed only by drop-oldest /
    /// `SetMaxLines`) and merely draws nothing, its rows still holding their place; a MessageFrame
    /// has no ring, so its retire helper (`0x786570`) frees the line outright and the rest re-pack.
    pub alpha: f32,
    /// How many display rows the line wraps into at the frame's current width/font — host-measured
    /// through the message-line measure round-trip
    /// ([`crate::script::UiScript::message_lines_needing_measure`]). `1` until measured.
    pub rows: u16,
    /// Cache key of the [`Self::rows`] measurement — hash of (text, font, height, wrap width),
    /// computed engine-side. `0` = unmeasured (drawn as one row until the same-frame answer lands);
    /// a width/font change mismatches the key and re-requests.
    pub rows_key: u64,
}

/// A `CSimpleMessageScrollFrame`'s runtime state (ctor `0x787670`): a true ring of `max_lines`
/// (drop-oldest, independent of how many display), each line carrying its own fade snapshot; a
/// scrollback cursor counted **up from the bottom** (`0` = newest = AtBottom); and the frame-level
/// fade config the per-line snapshots copy at insert.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollingMessageState {
    /// The frame's OWN justification, set by `SetJustifyH`/`SetJustifyV` — `None` until one of
    /// them is called.
    ///
    /// Present on both classes: the ScrollingMessageFrame method table `0x87b5c0`
    /// (the one whose `AddMessage` is `0x792900`) carries
    /// `SetJustifyH 0x792600`/`GetJustifyH`/`SetJustifyV`/`GetJustifyV`, and the MessageFrame table
    /// `0x87b960` (`AddMessage 0x795590`, the `SetInsertMode` sibling) carries the same four. Read
    /// off the `{const char*, void*}` pair bytes; the same walk reproduces the Button table at
    /// `0x879d00` exactly, and confirms Button has no `SetJustifyH` — which is
    /// why the family cannot be inferred and had to be read.
    ///
    /// **`Option`, and the reason is ours not the client's.** The real object keeps ONE justify
    /// dword: its font instance's. Ours reads a message frame's font instance off its first
    /// `<FontString>` region (`message_frame_font`), which is where XML's
    /// `<FontString … justifyH="LEFT"/>` lands — but a frame built by `CreateFrame` has no such
    /// child and would have nowhere to store. So `None` means "never told, defer to the font
    /// object" and keeps the shipped chat rendering byte-for-byte what it was; `Some` is an
    /// explicit call and wins. The divergence that buys: on a frame that HAS a FontString, setting
    /// the frame's justify does not change what that FontString's own `GetJustifyH` reports, where
    /// the client would have them be one field.
    pub justify: Option<crate::justify::Justify>,

    /// The line ring, newest at the back. Capacity is enforced by drop-oldest in [`Self::add`].
    pub lines: VecDeque<MessageLine>,
    /// The ring capacity (`maxLines`; ctor default 8, ChatFrame.xml sets 128). `SetMaxLines` is
    /// **destructive** (`0x787dd0`).
    pub max_lines: usize,
    /// `timeVisible`/`displayDuration` — phase-1 duration a new line holds full alpha (ctor 10.0s;
    /// ChatFrame 120.0s).
    pub time_visible: f32,
    /// `fadeDuration` — phase-2 fade ramp length (ctor 3.0s). `0` ⇒ the line vanishes instantly at
    /// phase-1 expiry (no ramp).
    pub fade_duration: f32,
    /// Bumped by every path that can change a line's TEXT set (add, clear, SetMaxLines, and the
    /// generic mut door `KindState::message_lines_mut`) — the measure sweep's skip token
    /// (`message_lines_needing_measure`): a frame whose generation and measure environment both
    /// match its last clean sweep hashes no lines. Fade ticking deliberately does NOT bump (it
    /// moves alpha/time, never text), or a fading chat line would keep the sweep hot for its
    /// whole 2-minute ride.
    pub lines_gen: u64,
    /// `fadingEnabled` (ctor 1). While false, lines never fade.
    pub fading_enabled: bool,
    /// The scrollback offset, counted up from the newest line: `0` = pinned to the bottom (AtBottom,
    /// the only state in which fades tick — though every scroll entry re-arms the displayed lines
    /// regardless, [`Self::reset_all_fade_times`]); `n` = the view is `n` lines older. Clamped in
    /// [`Self::scroll_up`].
    pub scroll_offset: usize,
}

impl Default for ScrollingMessageState {
    fn default() -> ScrollingMessageState {
        // The shared CSimpleMessageScrollFrame ctor (`0x787670`) defaults: fadingEnabled=1,
        // timeVisible=10.0, fadeDuration=3.0; SMF maxLines=8.
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

/// Byte-quantize a `0..1` color/alpha component the way `AddMessage` does (`clamp[0,1]`, `*255`,
/// `+0.5`, truncate — round-half-up; `AddMessage 0x788150`).
pub fn quantize_u8(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5).trunc() as u8
}

impl ScrollingMessageState {
    /// `AddMessage(text, r, g, b)` (`0x788150`): quantize the color, snapshot the current
    /// `timeVisible`/`fadeDuration` onto the new line, and push it at the ring's newest slot,
    /// dropping the oldest when over `max_lines`. A view scrolled up stays anchored on the same
    /// content (the ring cursor is a slot, not an offset).
    /// `ScrollingMessageFrame:UpdateColorByID(id, r, g, b)` (`0x7932b0` → `0x788250`) — every line
    /// tagged `id` takes the new colour, quantised the way `AddMessage` quantised the old one.
    /// Returns how many lines moved; a recolour bumps the generation so a settled frame redraws.
    ///
    /// **`id == 0` matches nothing, and that is a guard the reference opens with, not a
    /// consequence**.
    /// `0x788250` is `mov edi,[ebp+8]; test edi,edi; je 0x7882a4` → `ret 8`: zero never reaches a
    /// comparison and the record walk is not entered at all.
    ///
    /// Without it this was a live repaint of every line ever printed with no explicit colour,
    /// because `AddMessage`'s absent-id case stores an explicit literal `0` (`0x7929b7 xor edi,edi`
    /// → `0x78821d mov [edi+0x14],ecx`) and `ChatTypeInfo["REPLY"].id` is 0 too — FrameXML declares
    /// REPLY and the engine's 94-row registry does not carry it. So `ChatFrame_OnEvent`'s
    /// `UPDATE_CHAT_COLOR` arm mirroring WHISPER into REPLY (`ChatFrame.lua` l.1357-1365) called
    /// `UpdateColorByID(0, 1.0, 0.5, 1.0)` and turned every `AceConsole:Print` in the window
    /// whisper-pink. Measured `(255,128,255)` on Bartender2's login line where the reference reads
    /// `(255,255,255)`.
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

    /// `AddMessage(text, r, g, b, id)` — the line, tagged with the chat-type index it was
    /// printed under so a later `UpdateColorByID(id, …)` can recolour it.
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
            // The dropped line was above the view — walk the anchor back down with it.
            self.scroll_offset = self.scroll_offset.saturating_sub(1);
        }
        self.clamp_scroll();
    }

    /// `SetMaxLines(n)` — **destructive** (`0x787dd0`): frees every line + resets the cursor, then
    /// sets the new capacity. Not a preserving resize.
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

    /// Whether the view is pinned to the newest line (`AtBottom` — the only state in which fades
    /// tick).
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

    /// **Re-arm the displayed lines' fade** — the engine's `0x788b80`, which every scroll entry
    /// reaches (see the scroll methods below). Per displayed line it writes the quartet
    /// `alpha = 0xFF` (`0x788baf`), the in-use flag (`0x788bb3`), and the frame's *current*
    /// `timeVisible`/`fadeDuration` **defaults** back onto the record's `+0xc`/`+0x10` snapshots
    /// (`0x788bc0`/`0x788bc9`) — full values, never partial, and the frame default rather than
    /// whatever the line was inserted with.
    ///
    /// Two properties that are easy to get wrong and are both byte-verified:
    ///
    /// - **The displayed set only, never the whole ring.** `0x788b80` walks `[+0x34c]` entries of
    ///   the display vector `[+0x3a4]`, reaching each record through `node.+0xc`. A line scrolled
    ///   out of view keeps whatever fade state it had.
    /// - **A fully-faded line is recoverable.** Expiry (`0x788525`–`0x788538`) only clears the
    ///   in-use flag, blanks the display object and hides it; `0x788460` contains no allocator call
    ///   at all, so the record, its text and its wrapped height all survive and come straight back.
    ///
    /// Not a Lua binding: it is in no vtable and no data dword, and 1.12's FrameXML has no
    /// `ResetAllFadeTimes` caller. It is engine behavior, reached only through the scroll entries.
    pub fn reset_all_fade_times(&mut self, viewport_rows: usize) {
        let (time_visible, fade_duration) = (self.time_visible, self.fade_duration);
        for line in self.lines.range_mut(self.displayed_range(viewport_rows)) {
            line.time_left = time_visible;
            line.fade_left = fade_duration;
            line.alpha = 1.0;
        }
    }

    /// The ring indices the view currently shows, oldest-first — the set
    /// [`Self::reset_all_fade_times`] walks. The newest displayed line sits at the top end (it is
    /// the bottom row on screen); the walk runs older from there for [`Self::displayed_count`]
    /// messages.
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
    // **Every one of them re-arms the displayed lines' fade**, by one of two engine routes that
    // between them leave no gap:
    //
    // - the cursor does NOT move (a scroll that is already at the boundary, or a no-op jump) — the
    //   binding calls `0x788b80` directly: `ScrollUp` at AtTop (`0x788626`), `ScrollDown` at
    //   AtBottom (`0x788666`), `ScrollToBottom` at AtBottom (`0x7886e5`);
    // - the cursor DOES move — the relayout `0x788750` runs and its per-line helper `0x788af0`
    //   (sole caller `0x7888be`) writes the same quartet under `arg3 != 0 || AtBottom == 0`, where
    //   `arg3` is the relayout's own 0→1 AtBottom edge flag (`0x7887a3`). Land off the bottom and
    //   the second term fires; land back ON the bottom and the edge flag does.
    //
    // So the re-arm is unconditional at this level and the two routes collapse into one call
    // after the cursor has moved — which is also the right order, since the displayed set the
    // engine walks is the one the new cursor selects. **Correction (2026-08-29):** benilla
    // previously had scrolling merely *freeze* the countdown. Faded chat could never be brought
    // back; the director reported it.

    /// `ScrollUp` (`0x788610`) — one line older (no cursor move at the top), then re-arm.
    pub fn scroll_up(&mut self, viewport_rows: usize) {
        self.scroll_offset = (self.scroll_offset + 1).min(self.max_scroll());
        self.reset_all_fade_times(viewport_rows);
    }

    /// `ScrollDown` (`0x788650`) — one line newer (no cursor move at the bottom), then re-arm.
    pub fn scroll_down(&mut self, viewport_rows: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        self.reset_all_fade_times(viewport_rows);
    }

    /// `ScrollToBottom` (`0x7886d0`) — jump to the newest line, then re-arm.
    pub fn scroll_to_bottom(&mut self, viewport_rows: usize) {
        self.scroll_offset = 0;
        self.reset_all_fade_times(viewport_rows);
    }

    /// `ScrollToTop` (`0x788690`) — jump to the oldest line, then re-arm. This one never reaches
    /// `0x788b80`; it re-arms because it always relayouts with AtBottom = 0.
    pub fn scroll_to_top(&mut self, viewport_rows: usize) {
        self.scroll_offset = self.max_scroll();
        self.reset_all_fade_times(viewport_rows);
    }

    /// How many messages the view shows from the current anchor, given the viewport's row budget
    /// (`floor(frame height / pitch)`): walk older from the anchor summing each message's wrapped
    /// [`MessageLine::rows`] until the budget is spent. A partially-fitting message counts (it draws,
    /// clipped), matching [`emit`](crate::script::UiScript::extract)'s band walk. At least 1 when
    /// any line exists.
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

    /// `PageUp` — the client pages by `numLinesDisplayed` scroll steps then one back
    /// (`0x7885b0`: net page = displayed − 1, one line of overlap).
    pub fn page_up(&mut self, viewport_rows: usize) {
        let page = self.displayed_count(viewport_rows).saturating_sub(1).max(1);
        self.scroll_offset = (self.scroll_offset + page).min(self.max_scroll());
        self.reset_all_fade_times(viewport_rows);
    }

    /// `PageDown` — the same page size toward the newest line. Pages inherit both re-arm routes
    /// from the `ScrollUp`/`ScrollDown` steps they are built out of.
    pub fn page_down(&mut self, viewport_rows: usize) {
        let page = self.displayed_count(viewport_rows).saturating_sub(1).max(1);
        self.scroll_offset = self.scroll_offset.saturating_sub(page);
        self.reset_all_fade_times(viewport_rows);
    }

    /// Advance the fade by `dt` (the OnUpdate tick, `0x788460`). The entry gate is **AtBottom** and
    /// `fading_enabled` — scrolled up, every line holds its current alpha and *this* function never
    /// un-fades one. Un-fading is the scroll entries' job, through
    /// [`Self::reset_all_fade_times`] — do not read the freeze below as "nothing ever comes back",
    /// which is the reading that cost us the bug. Each
    /// line runs phase 1 (`time_left` countdown at full alpha), then phase 2 (`fade_left` countdown,
    /// alpha = `trunc(fade_left/fade_duration*255)`); `fade_duration == 0` snaps straight to 0.
    ///
    /// **Both countdowns are `fst`-without-pop** (`0x7884d7` phase 1, `0x788544` phase 2): memory
    /// takes the `f32` rounding of `remaining − dt` while the x87 stack keeps the un-rounded value,
    /// and it is the *un-rounded* one that the `fdiv`/`fmul 255.0`/`__ftol` ramp then consumes. So
    /// the arithmetic runs wide and only the stored countdown narrows to `f32`. Phase 1's negative
    /// overshoot is discarded — it stores exactly `0`, and phase 2 starts on the next tick with the
    /// full fade rather than the remainder.
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
                // No ramp — the line vanishes the instant phase 1 expires.
                line.alpha = 0.0;
                continue;
            }
            let remaining = f64::from(line.fade_left) - dt;
            line.fade_left = remaining as f32;
            if remaining <= 0.0 {
                line.fade_left = 0.0;
                line.alpha = 0.0;
            } else {
                // Divisor is the LIVE frame fadeDuration (a mid-fade SetFadeDuration rescales the
                // ramp), byte-quantized like the client — off the wide `remaining`, not the stored
                // f32, per the `fst`-no-pop shape above.
                let byte = quantize_fade_wide(remaining / f64::from(self.fade_duration));
                line.alpha = f32::from(byte) / 255.0;
            }
        }
    }
}

/// Phase-2 alpha quantization: `trunc(x*255)` (no `+0.5` — the fade tick truncates, `0x788547`).
fn quantize_fade(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).trunc() as u8
}

/// [`quantize_fade`] off the x87 stack's un-rounded ratio — see [`ScrollingMessageState::tick`] on
/// the `fst`-without-pop shape that makes the ramp wider than the countdown it stores.
fn quantize_fade_wide(x: f64) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).trunc() as u8
}

/// Where a [`MessageFrameState`]'s newest message enters the display stack — the `insertMode` XML
/// attribute (`0x87a618`) and the `SetInsertMode`/`GetInsertMode` pair (`0x794ed0`/`0x794ff0`).
///
/// **MessageFrame only**, which is why it lives here and not on [`ScrollingMessageState`]: the
/// scrolling class has neither binding and no such XML attribute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InsertMode {
    /// `"TOP"` (the client's `0`): the newest message takes the frame's **top** line and older ones
    /// step down. The reference's own `UIErrorsFrame.xml:4` asks for this, and so does every corpus
    /// caller of `SetInsertMode` (`BigWigs/Plugins/Messages.lua:209`).
    Top,
    /// `"BOTTOM"` (the client's `1`, and the **ctor default**): newest at the bottom, older stepping
    /// up — the chat shape.
    #[default]
    Bottom,
}

/// A `CSimpleMessageFrame`'s runtime state (ctor `0x785640`) — the class `UIErrorsFrame` is, and
/// [`ScrollingMessageState`]'s **sibling, never its base**. The two come from different ctors and
/// their `AddMessage` bindings are different functions with different tails, so nothing is shared
/// here beyond the line record itself:
///
/// | | MessageFrame (this) | ScrollingMessageFrame |
/// |---|---|---|
/// | store | display lines only — no ring, **no `maxLines`**; the cap is what fits vertically | a true ring of `maxLines`, drop-oldest |
/// | `AddMessage` tail | a real **alpha**, default 1.0 (`0x795590`) | an **id**; alpha forced `0xFF` (`0x792900`) |
/// | scrollback | none at all | cursor + scroll/page set |
/// | `SetInsertMode` | yes ([`InsertMode`]) | no binding, no attribute |
/// | a fully-faded line | retired and the rest re-pack (`0x786570`) | keeps its ring slot and its rows |
///
/// The one recorded shape this does **not** model is the pending queue: the real class only
/// *enqueues* in `AddMessage` and drains at OnUpdate, dropping the message outright if
/// `numLinesDisplayed` is 0 at drain time (`786265`). Here a message lands in [`Self::lines`]
/// immediately and [`Self::trim_to_viewport`] applies the vertical cap at the next tick — the same
/// observable result one tick earlier, without a second buffer whose only visible consequence is
/// that a message added to a zero-height frame vanishes.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageFrameState {
    /// The frame's OWN justification, set by `SetJustifyH`/`SetJustifyV` — `None` until one of
    /// them is called.
    ///
    /// Present on both classes: the ScrollingMessageFrame method table `0x87b5c0`
    /// (the one whose `AddMessage` is `0x792900`) carries
    /// `SetJustifyH 0x792600`/`GetJustifyH`/`SetJustifyV`/`GetJustifyV`, and the MessageFrame table
    /// `0x87b960` (`AddMessage 0x795590`, the `SetInsertMode` sibling) carries the same four. Read
    /// off the `{const char*, void*}` pair bytes; the same walk reproduces the Button table at
    /// `0x879d00` exactly, and confirms Button has no `SetJustifyH` — which is
    /// why the family cannot be inferred and had to be read.
    ///
    /// **`Option`, and the reason is ours not the client's.** The real object keeps ONE justify
    /// dword: its font instance's. Ours reads a message frame's font instance off its first
    /// `<FontString>` region (`message_frame_font`), which is where XML's
    /// `<FontString … justifyH="LEFT"/>` lands — but a frame built by `CreateFrame` has no such
    /// child and would have nowhere to store. So `None` means "never told, defer to the font
    /// object" and keeps the shipped chat rendering byte-for-byte what it was; `Some` is an
    /// explicit call and wins. The divergence that buys: on a frame that HAS a FontString, setting
    /// the frame's justify does not change what that FontString's own `GetJustifyH` reports, where
    /// the client would have them be one field.
    pub justify: Option<crate::justify::Justify>,

    /// See [`ScrollingMessageState::lines_gen`] — the same sweep-skip token.
    pub lines_gen: u64,
    /// The display lines, **newest at the back always** — [`InsertMode`] is a *display direction*
    /// resolved at emit, not a storage order, so "evict the oldest" is one `pop_front` in both
    /// modes.
    pub lines: VecDeque<MessageLine>,
    /// `insertMode` — which end of the frame new messages enter from (ctor default BOTTOM).
    pub insert_mode: InsertMode,
    /// `timeVisible`/`displayDuration` — phase-1 duration a new line holds its insert alpha (ctor
    /// 10.0s; `UIErrorsFrame.xml` sets 5).
    pub time_visible: f32,
    /// `fadeDuration` — phase-2 ramp length (ctor 3.0s). `0` ⇒ the line vanishes at phase-1 expiry
    /// with no ramp.
    pub fade_duration: f32,
    /// `fadingEnabled` (ctor 1). While false nothing fades — and, because retirement is what bounds
    /// this list, nothing retires either; the vertical cap is then the only bound (which is exactly
    /// the client's own arrangement).
    pub fading_enabled: bool,
}

impl Default for MessageFrameState {
    fn default() -> MessageFrameState {
        // The CSimpleMessageFrame ctor (`0x785640`) defaults: fadingEnabled=1,
        // timeVisible=10.0, fadeDuration=3.0; insertMode 1 = BOTTOM. Note the fade pair is
        // shared with the scrolling class but `maxLines` has no counterpart here.
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
    /// `AddMessage(text, r, g, b, a)` (`0x795590` → `0x785d00`): quantize the colour the same
    /// round-half-up way the scrolling class does, take the **alpha argument** as the line's
    /// starting alpha (this class's fourth numeric really is alpha — the scrolling one's is an id
    /// and forces `0xFF`), snapshot the frame's fade config, and append.
    pub fn add(&mut self, text: String, r: f32, g: f32, b: f32, a: f32) {
        self.lines_gen = self.lines_gen.wrapping_add(1);
        self.lines.push_back(MessageLine {
            text,
            id: 0,
            color: [quantize_u8(r), quantize_u8(g), quantize_u8(b)],
            time_left: self.time_visible,
            fade_left: self.fade_duration,
            // Quantized like the colour: the client packs all four channels into one 0xAARRGGBB
            // dword through the same `ftol(v*255 + 0.5)`.
            alpha: f32::from(quantize_u8(a)) / 255.0,
            rows: 1,
            rows_key: 0,
        });
    }

    /// `Clear` — retire every line immediately, no fade. (`_LazyPig` calls `UIErrorsFrame:Clear()`
    /// eleven times; it is the one non-`AddMessage` verb the corpus actually uses on this class.)
    pub fn clear(&mut self) {
        self.lines_gen = self.lines_gen.wrapping_add(1);
        self.lines.clear();
    }

    /// Advance the fade by `dt` — the OnUpdate virtual `0x786200`, whose gate is
    /// `activeCount != 0 && fadingEnabled != 0 && numLinesDisplayed > 0`. **No scroll gate**: this
    /// class has no scrollback, so unlike [`ScrollingMessageState::tick`] nothing can freeze it.
    ///
    /// Two phases per line, identical formula to the scrolling class: `time_left` counts down at the
    /// insert alpha, then `fade_left` counts down and *overwrites* the alpha with
    /// `trunc(fade_left/fade_duration*255)` against the **live** frame `fade_duration`. A finished
    /// line is then **freed** rather than left in place — this class has no ring to hold a slot, so
    /// the survivors re-pack (`0x786570`, the retire helper that decrements the active count).
    ///
    /// Deliberately still narrow arithmetic: the `fst`-without-pop shape is byte-cited for the
    /// *scrolling* class's tick (`0x7884d7`/`0x788544`), and `0x786200` has not been walked for it.
    /// Copying the wide form across on the strength of "same formula" would be the scope error
    /// decision 1692 is about, one class over.
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
        // Retired lines are gone, not blank: `alpha == 0` past phase 1 is the finished state.
        self.lines.retain(|l| l.time_left > 0.0 || l.alpha > 0.0);
    }

    /// Evict the oldest lines until only what fits in `viewport_rows` display rows remains — this
    /// class's whole capacity law ("no `maxLines` anywhere on this class — the cap is what fits
    /// vertically"), with each line costing its host-measured wrapped [`MessageLine::rows`].
    ///
    /// `viewport_rows == 0` means the frame has no resolved rect yet (or a degenerate one) and is
    /// left alone rather than emptied: dropping there would lose a message posted from an `OnLoad`
    /// before the first solve, which is a v1 ordering artefact and not the client's zero-height
    /// case. The fade is what bounds the list in that window — and with `fading_enabled` false it
    /// is unbounded until the frame resolves, which is stated rather than papered over.
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
