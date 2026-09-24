// ─────────────────────────────────────────────────────────────────────────────────────────────
// Markup — ported verbatim from decision 0068's text-glyph probe (`probes/text-glyph`, retired
// from the tree by 2331; git history keeps it)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A `|H<link>|h<text>|h` hyperlink a run sits inside: the link payload (`item:2000:0:0:0`,
/// `player:Bob`) and the reconstructed link markup (`|H…|h[Name]|h` — the `OnHyperlinkClick`
/// `arg2`, what shift-click inserts into the edit box). Shared by every run the link's visible
/// text splits into (color changes, word wrap), so span collection can group by pointer identity.
#[derive(Debug, PartialEq)]
pub(crate) struct LinkInfo {
    pub(crate) link: String,
    pub(crate) markup: String,
}

/// One color run within a line: the literal text, the RGBA color (straight-alpha, 0..1) it draws
/// with — the same shape [`crate::ui_pass::UiQuad::color`] expects — and the hyperlink it belongs
/// to, if any.
#[derive(Clone)]
pub(super) struct ColorRun {
    pub(super) text: String,
    pub(super) color: [f32; 4],
    pub(super) link: Option<std::sync::Arc<LinkInfo>>,
}

/// Resolve `input`'s inline markup into lines of [`ColorRun`]s — the render side of the grammar
/// [`benilla_ui::markup`] owns (decision 1077).
///
/// The grammar itself is **not** this module's: `benilla-ui` holds it because the EditBox's cursor
/// model needs the identical token boundaries, and two copies of an escape grammar is a drift bug
/// waiting to happen. This function only decides what each token *draws*:
///
/// | token | drawn |
/// |---|---|
/// | `\|cAARRGGBB` | nothing — switches the color, at **this string's** alpha (the escape's own `AA` is parsed and thrown away, `0x5c2ab2`; the emitter substitutes `[edi+0x2f]` at `0x5cceb0`) |
/// | `\|r` | nothing — back to `base_color` |
/// | `\n` · `\r` · `\r\n` · `\|n` | a line break |
/// | `\|\|` | one literal `\|` |
/// | `\|H…\|h` / `\|h` | nothing — opens/closes the link every run inside it shares |
/// | anything else | itself |
///
/// **Stated divergence:** the client gates `|n` off for a single-line box (`K & 0x200`, set by
/// `SetMultiLine`'s single-line leg at `0x77a5e2`) while its *cursor* model parses it regardless —
/// a disagreement that reads as an anomaly, not a design. We have no flags word here and draw
/// `|n` as a break everywhere; nothing in our FrameXML emits one, and a user cannot type one
/// (`0x77c200` turns a typed `|` into `||`).
pub(super) fn parse_markup(input: &str, base_color: [f32; 4]) -> Vec<Vec<ColorRun>> {
    use benilla_ui::markup::TokenKind as T;

    let mut lines: Vec<Vec<ColorRun>> = Vec::new();
    let mut runs: Vec<ColorRun> = Vec::new();
    let mut color = base_color;
    let mut cur = String::new();
    // The open hyperlink, if any: (payload, visible-text accumulator, indices of runs already
    // flushed under it). The Arc is built at the closing `|h` and back-patched onto those runs.
    let mut link: Option<(String, String, Vec<usize>)> = None;

    for (_, token) in benilla_ui::markup::tokens(input) {
        match token.kind {
            T::Color(rgba) => {
                flush(&mut runs, &mut cur, color, &mut link);
                // At the STRING's alpha, not opaque: the decoder discards the escape's `AA` and the
                // emitter patches the FontString's own alpha over it (`0x5cceb0`). A fading chat
                // line's item link fades with it.
                color = rgba.to_f32_at(base_color[3]);
            }
            T::ColorReset => {
                flush(&mut runs, &mut cur, color, &mut link);
                color = base_color;
            }
            T::LineBreak => {
                flush(&mut runs, &mut cur, color, &mut link);
                lines.push(std::mem::take(&mut runs));
            }
            T::EscapedPipe => push_visible('|', &mut cur, &mut link),
            T::LinkOpen { payload } => {
                flush(&mut runs, &mut cur, color, &mut link);
                link = Some((payload.to_string(), String::new(), Vec::new()));
            }
            T::LinkClose => {
                flush(&mut runs, &mut cur, color, &mut link);
                // A close with no open is a stray token: nothing to back-patch, nothing drawn.
                if let Some((payload, visible, idxs)) = link.take() {
                    let info = std::sync::Arc::new(LinkInfo {
                        markup: format!("|H{payload}|h{visible}|h"),
                        link: payload,
                    });
                    for idx in idxs {
                        runs[idx].link = Some(info.clone());
                    }
                }
            }
            T::Char(c) => push_visible(c, &mut cur, &mut link),
        }
    }
    // An unterminated link degrades gracefully: its runs stay plain text (no span) — the `|H` was
    // consumed, the text still shows.
    flush(&mut runs, &mut cur, color, &mut link);
    lines.push(runs);
    lines
}

/// [`parse_markup`] under the FontString **line-count law** — what the client's height kernel
/// `GxuFont_GetTextBlockHeight 0x5c2070` actually counts.
///
/// The kernel walks the string: a bare line break at the cursor is an empty line and costs one;
/// otherwise it lays a line, and **that line's own terminating break is consumed with it**. So a
/// trailing break terminates its line rather than opening a new one, and the loop guard
/// (`cmp byte [esi],0`) means an empty string is **zero** lines with height `0.0`, not one.
///
/// The difference from a plain split is exactly one line, on exactly the strings that end in a
/// break — which is every `<BR/>` block SimpleHTML builds (`"\n"`: one blank line in the client,
/// two under a split) and every page body the item-text reader pads. B240's reader drew its
/// section gaps at double height until this existed (the director, 08-15, against his own 1.12.1
/// shot); decision 1343.
///
/// **Not the EditBox's law.** A multiline box that ends in a newline really does show the empty
/// row its caret sits on, and it reaches its rows through `line_rows`/`0x77da80`, not this kernel
/// — so [`parse_markup`] itself is left alone and this is a separate seam over it.
pub(super) fn fontstring_lines(input: &str, base_color: [f32; 4]) -> Vec<Vec<ColorRun>> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut lines = parse_markup(input, base_color);
    if lines.len() > 1
        && lines
            .last()
            .is_some_and(|l| l.iter().all(|r| r.text.is_empty()))
    {
        lines.pop();
    }
    lines
}

/// Accumulate one drawn char into the current run, and into the open link's visible text.
fn push_visible(c: char, cur: &mut String, link: &mut Option<(String, String, Vec<usize>)>) {
    if let Some((_, visible, _)) = link {
        visible.push(c);
    }
    cur.push(c);
}

fn flush(
    runs: &mut Vec<ColorRun>,
    cur: &mut String,
    color: [f32; 4],
    link: &mut Option<(String, String, Vec<usize>)>,
) {
    if !cur.is_empty() {
        if let Some((_, _, idxs)) = link {
            idxs.push(runs.len());
        }
        runs.push(ColorRun {
            text: std::mem::take(cur),
            color,
            link: None, // back-patched at the closing |h
        });
    }
}

/// `input`'s **drawn** text — what [`parse_markup`] would put on screen, markup resolved — paired
/// with the RAW byte offset of every boundary in it: `bounds.len() == drawn.len() + 1`, and
/// `bounds[k]` is the raw offset of the boundary *before* drawn byte `k`.
///
/// This is the raw↔drawn map the EditBox metrics ride on (decision 1075). The box **stores and
/// edits** the raw string (`|cffa335ee|Hitem:11684:0:0:0|h[Ironfoe]|h|r`) and **draws** only
/// `[Ironfoe]`, so an advance table indexed by raw byte has to charge every escape byte zero width.
/// Measuring the raw string instead put the caret 180 px — twice the drawn text's own width — to the
/// right of the text it was supposed to follow (director, 2026-08-06).
///
/// A **boundary** map rather than a per-byte one, because that is the question the metrics actually
/// ask: a glyph ending at drawn byte `e` files its width at raw `bounds[e]`, which is the raw byte
/// just past that glyph and *before* whatever escape follows it. The per-byte reading (raw offset of
/// drawn byte `e`, i.e. where the NEXT visible char starts) lands past the escape and leaves the
/// caret position immediately after that glyph reading the previous glyph's width — one glyph short,
/// at every escape in the string. It is also the only reading that survives `||`, whose one drawn
/// byte spans two raw ones.
pub(super) fn visible_map(input: &str) -> (String, Vec<usize>) {
    use benilla_ui::markup::TokenKind as T;

    let mut drawn = String::new();
    let mut bounds: Vec<usize> = Vec::new();
    let mut last_end = 0usize;
    for (at, token) in benilla_ui::markup::tokens(input) {
        let c = match token.kind {
            T::Char(c) => c,
            T::EscapedPipe => '|',
            T::LineBreak => '\n',
            // Zero-width: no drawn byte, so no boundary of its own.
            T::Color(_) | T::ColorReset | T::LinkOpen { .. } | T::LinkClose => continue,
        };
        // The boundary before this char is wherever the previous drawn token ENDED — not this
        // token's raw start, which would sit past any escape in between. A multi-byte char's
        // interior boundaries are its own raw bytes (never caret positions, but kept coherent).
        bounds.push(last_end);
        bounds.extend((1..c.len_utf8()).map(|k| at + k));
        last_end = at + token.byte_len;
        drawn.push(c);
    }
    bounds.push(last_end);
    (drawn, bounds)
}

#[cfg(test)]
mod markup_tests {
    use super::*;

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    #[test]
    fn plain_text_is_one_run() {
        let lines = parse_markup("hello", WHITE);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text, "hello");
        assert_eq!(lines[0][0].color, WHITE);
    }

    #[test]
    fn color_escape_switches_and_resets() {
        let lines = parse_markup("a|cffff0000b|rc", WHITE);
        assert_eq!(lines[0].len(), 3);
        assert_eq!(lines[0][0].text, "a");
        assert_eq!(lines[0][0].color, WHITE);
        assert_eq!(lines[0][1].text, "b");
        assert_eq!(lines[0][1].color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(lines[0][2].text, "c");
        assert_eq!(lines[0][2].color, WHITE);
    }

    /// The director-reported symptom, at the run level: a LOOT-green chat line whose item name
    /// draws in the item's **quality** colour, not the line's. The whole "You receive loot" fix
    /// hangs on the escape surviving intact from `ui_loot::receive_line` to here — this is the same
    /// string that function emits, with LOOT green (0,170,0) as the line's base colour.
    #[test]
    fn a_loot_line_draws_its_item_name_in_the_quality_color_and_the_count_in_the_line_color() {
        const LOOT_GREEN: [f32; 4] = [0.0, 170.0 / 255.0, 0.0, 1.0];
        let lines = parse_markup(
            "You receive loot: |cff9d9d9d|Hitem:7092:0:0:0|h[Chipped Claw]|h|rx2.",
            LOOT_GREEN,
        );
        assert_eq!(lines[0].len(), 3);
        assert_eq!(lines[0][0].text, "You receive loot: ");
        assert_eq!(lines[0][0].color, LOOT_GREEN);
        // The bracketed name: poor/grey, and clickable.
        assert_eq!(lines[0][1].text, "[Chipped Claw]");
        let grey = 0x9d as f32 / 255.0;
        assert_eq!(lines[0][1].color, [grey, grey, grey, 1.0]);
        assert_eq!(
            lines[0][1].link.as_ref().expect("linked run").link,
            "item:7092:0:0:0"
        );
        // The `|r` lands before the count, so `x2.` falls back to the line's own green.
        assert_eq!(lines[0][2].text, "x2.");
        assert_eq!(lines[0][2].color, LOOT_GREEN);
        assert!(lines[0][2].link.is_none());
    }

    #[test]
    fn newline_splits_lines() {
        let lines = parse_markup("one\ntwo", WHITE);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0].text, "one");
        assert_eq!(lines[1][0].text, "two");
    }

    /// There is no inline-texture escape in build 5875 — the remap table at `0x5c2b10` sends every
    /// `|`-lead but C/H/N/R to the ordinary-character arm. Our renderer used
    /// to strip `|T…|t`, a later-expansion feature we had invented; it now draws, like the client's.
    #[test]
    fn there_is_no_inline_texture_escape() {
        let lines = parse_markup("a|TInterface\\Icons\\Foo:16:16|tb", WHITE);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text, "a|TInterface\\Icons\\Foo:16:16|tb");
    }

    #[test]
    fn hyperlink_runs_carry_the_link_and_strip_the_markers() {
        // The canonical chat item link: color outside, |H..|h[Name]|h inside.
        let lines = parse_markup("|cff1eff00|Hitem:2000:0:0:0|h[Another Helm]|h|r ok", WHITE);
        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[0][0].text, "[Another Helm]");
        assert_eq!(lines[0][0].color, [0x1e as f32 / 255.0, 1.0, 0.0, 1.0]);
        let info = lines[0][0].link.as_ref().expect("linked run");
        assert_eq!(info.link, "item:2000:0:0:0");
        assert_eq!(info.markup, "|Hitem:2000:0:0:0|h[Another Helm]|h");
        assert_eq!(lines[0][1].text, " ok");
        assert!(lines[0][1].link.is_none());
        assert_eq!(lines[0][1].color, WHITE);
    }

    #[test]
    fn color_change_inside_a_link_still_shares_one_link() {
        let lines = parse_markup("|Hplayer:Bob|h[|cffff0000Bob|r]|h", WHITE);
        // Three runs ("[", "Bob", "]"), all sharing ONE LinkInfo (pointer identity).
        assert_eq!(lines[0].len(), 3);
        let first = lines[0][0].link.as_ref().expect("linked");
        for run in &lines[0] {
            let l = run.link.as_ref().expect("all runs linked");
            assert!(std::sync::Arc::ptr_eq(first, l));
        }
        assert_eq!(first.link, "player:Bob");
        assert_eq!(first.markup, "|Hplayer:Bob|h[Bob]|h");
    }

    // ── visible_map: the raw↔drawn boundary map the EditBox metrics ride on (1075/1077) ───────

    /// `visible_map`'s invariants, checked together: the drawn text is exactly what `parse_markup`
    /// would draw, and the boundary map is monotonic, `drawn.len() + 1` long, and lands only on raw
    /// char boundaries.
    fn check_map(raw: &str, expect_drawn: &str) -> Vec<usize> {
        let (drawn, bounds) = visible_map(raw);
        assert_eq!(drawn, expect_drawn, "drawn text of {raw:?}");
        let from_runs: String = parse_markup(raw, WHITE)
            .iter()
            .map(|l| l.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(drawn, from_runs, "visible_map agrees with parse_markup");
        assert_eq!(
            bounds.len(),
            drawn.len() + 1,
            "one boundary per drawn byte, plus the end"
        );
        for w in bounds.windows(2) {
            assert!(w[0] <= w[1], "monotonic: {bounds:?}");
        }
        for &b in &bounds {
            assert!(
                raw.is_char_boundary(b),
                "boundary {b} of {raw:?} is mid-char"
            );
        }
        bounds
    }

    /// The director's exact case (2026-08-06): a shift-clicked item link in the chat edit box. The
    /// end-of-buffer caret must land on the drawn text's full width, so the last boundary is the
    /// byte just past the `]` — with `|h|r` still to come — and not the raw length.
    #[test]
    fn an_item_link_maps_its_drawn_boundaries_onto_the_raw_buffer() {
        let raw = "|cffa335ee|Hitem:13984:0:0:0|h[The Plague Bearer]|h|r";
        let bounds = check_map(raw, "[The Plague Bearer]");
        // The boundary before the first drawn byte is 0 — before the leading escapes, which is
        // where the reachable cursor set puts it (`|c`/`|H` absorb forward).
        assert_eq!(bounds[0], 0);
        // The `]` glyph ends at drawn byte 19; its raw boundary is just past the `]`.
        let end = bounds[19];
        assert_eq!(&raw[end - 1..end], "]");
        assert_eq!(&raw[end..], "|h|r");
    }

    /// The case the boundary reading exists for: an escape sitting *between* two visible chars. The
    /// boundary after `b` must be 2, not 12 — the per-byte reading (where the next visible char
    /// begins) lands past the escape and leaves the caret right after `b` reading `a`'s width.
    #[test]
    fn a_boundary_stops_before_a_following_escape() {
        assert_eq!(check_map("ab|cffff0000cd", "abcd"), vec![0, 1, 2, 13, 14]);
    }

    /// Text typed straight after the link — the buffer the director was looking at. The typed bytes
    /// sit past the trailing `|r`, so their advances land after the drawn name, not after 53
    /// invisible characters.
    #[test]
    fn text_after_a_links_reset_maps_past_the_escape() {
        let raw = "|cffa335ee|Hitem:13984:0:0:0|h[The Plague Bearer]|h|rds";
        let bounds = check_map(raw, "[The Plague Bearer]ds");
        assert_eq!(&raw[bounds[19]..], "|h|rds");
        assert_eq!(bounds[bounds.len() - 1], raw.len());
    }

    #[test]
    fn visible_map_handles_plain_text_and_newlines() {
        assert_eq!(check_map("hello", "hello"), vec![0, 1, 2, 3, 4, 5]);
        // A newline draws as its own byte, and the map keeps crossing it.
        assert_eq!(
            check_map("ab\n|cffff0000cd|r", "ab\ncd"),
            vec![0, 1, 2, 3, 14, 15]
        );
        // A string that draws nothing has its one boundary at the start — there is no glyph for it
        // to sit after, and every raw offset forward-fills to x = 0 either way.
        assert_eq!(visible_map("|cffff0000|r"), (String::new(), vec![0]));
        assert_eq!(visible_map(""), (String::new(), vec![0]));
    }

    /// The three tokens 1075 could not see, now that the grammar is the engine's (1077): `||` draws
    /// ONE `|` out of two raw bytes — the case a per-byte map cannot represent at all — `|n` and
    /// `\r\n` are line breaks, and there is no `|T…|t` texture escape in 1.12.1, so it draws
    /// literally instead of being swallowed.
    #[test]
    fn the_tokens_the_engine_grammar_added() {
        // `a||b`: the drawn `|` spans raw 1..3, so the boundary after it is 3.
        assert_eq!(check_map("a||b", "a|b"), vec![0, 1, 3, 4]);
        assert_eq!(parse_markup("a||b", WHITE)[0][0].text, "a|b");
        // `|n` and `\r\n` both break the line.
        assert_eq!(parse_markup("a|nb", WHITE).len(), 2);
        assert_eq!(parse_markup("a\r\nb", WHITE).len(), 2);
        assert_eq!(check_map("a\r\nb", "a\nb"), vec![0, 1, 3, 4]);
        // No inline-texture escape exists in this build: it draws as text.
        let (drawn, _) = visible_map("a|Tfoo|tb");
        assert_eq!(drawn, "a|Tfoo|tb");
    }

    /// The FontString **line-count law** (`GxuFont_GetTextBlockHeight 0x5c2070`), pinned on the
    /// strings that separate it from a plain split: a **trailing** break terminates its line
    /// rather than opening a new one, and an empty string is **zero** lines.
    ///
    /// The director found this against his own 1.12.1 client (08-15): every `<BR/>` in a book page
    /// is a `"\n"` block, and under a split that is two blank lines instead of one — so the page's
    /// section gaps came out at double height and the text ran off the bottom. The second block
    /// below is the mutation check: the cases a split already got right must not move.
    #[test]
    fn a_trailing_break_does_not_open_a_line() {
        let n = |t: &str| fontstring_lines(t, [1.0, 1.0, 1.0, 1.0]).len();
        // The three the kernel counts differently from a split.
        assert_eq!(n("\n"), 1, "a <BR/> block is ONE blank line");
        assert_eq!(n("a\n"), 1);
        // The item tooltip's SET spacer, the reference's own literal `0x854b2c`: ONE row
        // carrying one space. Two under a split, and then the blank gold lines above the set
        // bonuses would each be a row taller than 1.12's.
        assert_eq!(n(" \n"), 1, "the set's blank gold spacer is one row");
        assert_eq!(n(""), 0, "an empty string is zero lines, height 0.0");
        // …and everything else is unchanged.
        assert_eq!(n("a"), 1);
        assert_eq!(n("a\nb"), 2);
        assert_eq!(
            n("\nbody\n"),
            2,
            "the reader's own padding: a blank line, then the body"
        );
        assert_eq!(n("\n\n"), 2);
        assert_eq!(n("a\n\nb"), 3);
        // `|n` is the same break token, so it obeys the same law.
        assert_eq!(n("a|n"), 1);
        assert_eq!(n("a|nb"), 2);
    }

    /// A malformed escape is literal text to the draw, so it must be literal to the map too —
    /// otherwise the metrics would charge zero width for bytes the user can see.
    #[test]
    fn a_malformed_escape_stays_visible_in_the_map() {
        check_map("a|cffzzb", "a|cffzzb");
        // Here the `|H…|h` OPEN is well formed — it is the closing `|h` that never comes, so the
        // link degrades (no clickable span) while its visible text still draws. Contrast the three
        // cases in `the_tokens_the_engine_grammar_added`, where the OPEN itself degrades.
        check_map("|Hitem:1|h[Broken", "[Broken");
    }

    #[test]
    fn unterminated_link_degrades_to_plain_text() {
        // No closing |h: the opener is consumed, the text still shows, no span attaches.
        let lines = parse_markup("|Hitem:1|h[Broken", WHITE);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text, "[Broken");
        assert!(lines[0][0].link.is_none());
    }
}
