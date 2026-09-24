//! The **markup parse** half of `CSimpleHTML::SetText` — `&str` in, a block list out.
//!
//! Pure: no Lua, no model, no arena. Everything here is one transcription of the reference's
//! `SetText`/`WALK_BODY`/`ADD_PARAGRAPH` algorithm and the byte derivations behind it; [`super`]
//! does the materialization (regions, anchors, fonts).
//!
//! ## The one fact everything else follows from
//!
//! **The markup is parsed as strict XML, not as lenient HTML.** `0x78a422` calls
//! `XMLTree::Parse 0x6f2a30`, which is embedded **expat 1.95.5** (version string `0x882094`,
//! wrapper `__FILE__` `…\FrameXML\XMLTree.cpp` at `0x8715ec`). So an unclosed `<BR>`, a `</p>`
//! closing a `<P>` (XML open/close matching is case-SENSITIVE, even though *SimpleHTML's own*
//! name compares are not — `SStrCmpI 0x414310`), a bare `&`, a non-predefined entity such
//! as `&nbsp;`, a duplicate attribute, or any non-whitespace after `</HTML>` all fail the parse
//! outright, and the widget renders the **raw** string instead (`0x78a501`).
//!
//! [`roxmltree`] stands in for expat here, and it is the right stand-in rather than a convenient
//! one: it is a well-formedness-checking XML parser with exactly the five XML predefined entities
//! plus numeric character references, and it was checked case by case against the reference's parse
//! outcomes before this module was written — plain prose, a bare `&`, a bare `<`, an unclosed
//! `<BR>`, a case-mismatched close tag, `&nbsp;`, junk after the root and a duplicate attribute are
//! each an `Err` (expat codes 4/4/4/7/7/11/9/8), while trailing whitespace after `</HTML>`, a
//! leading newline, numeric refs, comments and CDATA are each an `Ok`. Its interleaved text/element
//! child order also hands us the `<A>`/`<BR>` splice positions directly, so none of the reference's
//! `offsetInParentText` arithmetic (`XMLTree` node `+0x18`) is needed.
//!
//! **The one behavioural difference found**, stated rather than hidden: roxmltree refuses a
//! document carrying an internal DTD subset (`XML with DTD detected`) where expat 1.95.5 may
//! accept one and honour its `<!ENTITY>` declarations. Expat's build options are unread for
//! exactly this case, and no `page_text` body carries a DOCTYPE; for us a DOCTYPE simply takes
//! the plain-text fallback, which is the same place a rejected parse lands.

use crate::justify;

/// The four block elements, indexed as the client indexes `elementFont[4]` at `+0x350`:
/// `0 = P, 1 = H1, 2 = H2, 3 = H3` (`0x789e3e` creates them in that order, `0x78ae29` reads them,
/// and the Lua element-name resolver `0x795d80` answers the same four numbers).
pub(crate) const ELEMENT_NAMES: [&str; 4] = ["P", "H1", "H2", "H3"];

/// `P` — the element every unqualified path addresses: the plain-text fallback's block (`0x78a503
/// push 0`), the `<BR/>` block, and the element a missing or unrecognised Lua element-name
/// argument resolves to (`0x795e50`).
pub(crate) const ELEM_P: usize = 0;

/// The `align` default, pre-loaded into the local before the attribute is even looked at
/// (`0x78a7c8 mov [ebp-0xc],1` for a block, `0x78ab59 mov [ebp-8],1` for an `<IMG>`): **LEFT**,
/// and emphatically *not* the `CSimpleFontString` ctor's CENTER, which `0x78ae78` overwrites on
/// every block it builds.
pub(crate) const ALIGN_LEFT: u32 = 0x01;
/// `align="center"` — the value `0x6f1990` writes from the shared enum table `.rdata 0x811ad0`.
pub(crate) const ALIGN_CENTER: u32 = 0x02;
/// `align="right"`.
pub(crate) const ALIGN_RIGHT: u32 = 0x04;

/// The ctor default of `hyperlinkFormat` (`+0x360`, string `0x87a838`, installed by
/// `0x789ea7`→`0x78a540`): `<A href="X">Y</A>` becomes `|HX|hY|h`, which the font engine then
/// parses as an ordinary hyperlink (`SetText 0x771d80` never touches `+0x120` — nothing disables
/// `|H` on a `CSimpleFontString`).
pub(crate) const DEFAULT_HYPERLINK_FORMAT: &str = "|H%s|h%s|h";

/// One block the walk produced — the arguments of `AddTextBlock 0x78adb0` / `AddImage 0x78ab40`,
/// in the order the BODY walker emitted them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    /// A text block: one `CSimpleFontString`, `SetWidth(frame width)`, no height.
    Text {
        /// The already-spliced, already-collapsed string handed to `SetText` — except on the
        /// plain-text fallback path, which hands over the **raw** input (`0x78a501`).
        text: String,
        /// The `elementFont[]` index this block draws with, before the empty-path fallback.
        elem: usize,
        /// The `align` bits (`0x811ad0`'s values); masked `& 7` into the block's justifyH.
        align: u32,
    },
    /// An `<IMG>`: one `CSimpleTexture`, sized by `width`/`height` and anchored by `align`.
    Image {
        /// `src=`, used **verbatim** as a texture path — no prefix, no extension fix-up, no
        /// validation in this TU (`0x78ad02`).
        src: Option<String>,
        /// `width=`/`height=` in logical UI pixels (`0x78ab9e`/`0x78abd6` — the same units as
        /// `<AbsDimension>`); `0` when the attribute is absent.
        width: f32,
        height: f32,
        /// The `align` bits; selects which corner anchors to the previous block.
        align: u32,
        /// The **separate** float byte `[ebp-1]` (zeroed at `0x78ab55`): a floated image reserves
        /// no height in the flow, so the following text overlaps it. Set only inside the
        /// attribute-present branch, which is why a bare `<IMG src=…/>` and
        /// `<IMG align="left" src=…/>` anchor identically and flow differently.
        floated: bool,
    },
}

/// The outcome of one `SetText` parse.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Parse {
    /// The blocks to build, in order.
    pub(crate) blocks: Vec<Block>,
    /// `SetText`'s own return value in `al` (`0x78a519`): whether the markup path ran. The Lua
    /// shim `0x796a90` discards it; we keep it because it is exactly "did this render as markup
    /// or as raw text", which is the thing a reader of a `page_text` body wants to know.
    pub(crate) used_markup: bool,
    /// The console lines the error sink received (`"Frame %s: Unknown element type: %s"` and its
    /// two `(expected …)` variants), already formatted with the frame's name.
    pub(crate) errors: Vec<String>,
}

/// `CSimpleHTML::SetText 0x78a3a0`'s parse half — its `WALK_BODY`/`ADD_PARAGRAPH` algorithm.
///
/// `frame` is the widget's name, the `%s` of the error strings. `hyperlink_format` is
/// `+0x360`'s current value, used by the `<A>` splice.
pub(crate) fn parse_markup(raw: &str, frame: &str, hyperlink_format: &str) -> Parse {
    let mut out = Parse {
        blocks: Vec::new(),
        used_markup: false,
        errors: Vec::new(),
    };
    // `usedMarkup` is `[ebp+0xf]`, zeroed at `0x78a415` and raised at exactly ONE site,
    // `0x78a4b6` — immediately BEFORE the BODY walker runs. That ordering is the whole reason an
    // empty `<BODY/>` renders nothing at all rather than falling back to the raw string.
    if let Ok(doc) = roxmltree::Document::parse(raw) {
        let root = doc.root_element();
        if root.tag_name().name().eq_ignore_ascii_case("HTML") {
            // Only the FIRST `BODY` is walked: `0x78a4ba call 0x78a660` is followed by
            // `0x78a4bf jmp 0x78a4ef`, which leaves the sibling loop — a second `<BODY>` is never
            // visited and never even errors.
            for child in root.children().filter(roxmltree::Node::is_element) {
                if child.tag_name().name().eq_ignore_ascii_case("BODY") {
                    out.used_markup = true;
                    walk_body(child, frame, hyperlink_format, &mut out);
                    break;
                }
                out.errors.push(format!(
                    "Frame {frame}: Unknown element type: {} (expected BODY)",
                    child.tag_name().name()
                ));
            }
        } else {
            out.errors.push(format!(
                "Frame {frame}: Unknown element type: {} (expected HTML)",
                root.tag_name().name()
            ));
        }
    }
    if !out.used_markup {
        // The three routes here — a failed parse, a non-`HTML` root, an `HTML` with no `BODY`
        // child — all land on `0x78a501`: the ORIGINAL, unmodified string as ONE block, element
        // `P`, justifyH LEFT. **Without the whitespace collapse**, because the collapse lives in
        // the paragraph builder `0x78a7b0` which this path never enters — so embedded `\n` stay
        // real line breaks. Stock `ItemTextFrame.lua` depends on exactly that, padding its page
        // with `SetText("\n" .. ItemTextGetText() .. "\n")`.
        out.blocks.push(Block::Text {
            text: raw.to_string(),
            elem: ELEM_P,
            align: ALIGN_LEFT,
        });
    }
    out
}

/// The BODY walker `0x78a660` — dispatch by child element name, case-insensitively.
///
/// **`BODY`'s own character data is DROPPED**: the walker reads only `firstChild`/`nextSibling`
/// and nothing ever reads `body->+0x0c`. The inter-tag newlines of a typical `page_text` body are
/// exactly that, which is why they contribute no blank lines.
fn walk_body(body: roxmltree::Node, frame: &str, hyperlink_format: &str, out: &mut Parse) {
    for node in body.children().filter(roxmltree::Node::is_element) {
        let tag = node.tag_name().name();
        if let Some(elem) = ELEMENT_NAMES
            .iter()
            .position(|e| tag.eq_ignore_ascii_case(e))
        {
            let block = paragraph(node, elem, frame, hyperlink_format, &mut out.errors);
            out.blocks.push(block);
        } else if tag.eq_ignore_ascii_case("BR") {
            // `0x78a726` → `0x78adb0("\n" @0x835144, elem 0, align 1)`: a block of its own whose
            // text is the single byte `\n`. The height kernel `0x5c2070` takes the class-2 arm
            // once and exits with `lines == 1`, so it is **exactly one blank line** — not two,
            // not a paragraph gap.
            out.blocks.push(Block::Text {
                text: "\n".to_string(),
                elem: ELEM_P,
                align: ALIGN_LEFT,
            });
        } else if tag.eq_ignore_ascii_case("IMG") {
            out.blocks.push(image(node));
        } else {
            // `0x78a762`–`0x78a793`: the name is formatted into `"Frame %s: Unknown element type:
            // %s"` (`0x87a95c`), pushed through the sink, and the node contributes NOTHING; the
            // sibling walk continues at `0x78a796`.
            out.errors
                .push(format!("Frame {frame}: Unknown element type: {tag}"));
        }
    }
}

/// `0x78a7b0(node, elem, sink)` — one `<P>`/`<H1>`/`<H2>`/`<H3>` into one block.
fn paragraph(
    node: roxmltree::Node,
    elem: usize,
    frame: &str,
    hyperlink_format: &str,
    errors: &mut Vec<String>,
) -> Block {
    let align = align_of(node);
    // `0x78a7b0` does this in one pass. The reference seeds the buffer with the node's whole
    // accumulated character data (expat concatenates the text either side of an inline child
    // into the SAME `+0x0c`) and then splices each child in at its recorded `+0x18` offset plus
    // the running `extra`. roxmltree hands us text and elements interleaved in document order
    // instead, so appending as we walk lands every splice exactly where the tag was, with no
    // offset arithmetic to get wrong.
    let mut buf = String::new();
    for child in node.children() {
        if child.is_text() {
            buf.push_str(child.text().unwrap_or(""));
            continue;
        }
        if !child.is_element() {
            // A comment or PI: expat never delivers it as character data either.
            continue;
        }
        let tag = child.tag_name().name();
        if tag.eq_ignore_ascii_case("BR") {
            // `0x78a86b` appends the two literal bytes `|n` (`0x87a9a0`) — a real line break
            // WITHIN this block, not a block of its own, and `extra += 2`.
            buf.push_str("|n");
        } else if tag.eq_ignore_ascii_case("A") {
            // `0x78a8f0` requires **both** a non-empty `href` and non-empty inner text
            // (`0x78a914`–`0x78a922`); otherwise the whole `<A>` contributes nothing at all.
            let href = attr_ci(child, "href").unwrap_or("");
            let inner = direct_text(child);
            if !href.is_empty() && !inner.is_empty() {
                buf.push_str(&sprintf_two(hyperlink_format, href, &inner));
            }
        } else {
            // `0x78a9f9`–`0x78aa2c`: the same message and the same outcome as at BODY level. The
            // child's own character data went into the CHILD's `+0x0c`, never the parent's, so
            // the text of an unknown inline element is silently lost while the text either side
            // of it is kept — which is what dropping the node here reproduces.
            errors.push(format!("Frame {frame}: Unknown element type: {tag}"));
        }
    }
    Block::Text {
        text: collapse(&buf),
        elem,
        align,
    }
}

/// `0x78ab40(node, sink)` — one `<IMG>`.
fn image(node: roxmltree::Node) -> Block {
    let mut align = ALIGN_LEFT;
    let mut floated = false;
    // The float byte `[ebp-1]` is zeroed at `0x78ab55`
    // and set only INSIDE the attribute-present branch (`0x78ab67`/`0x78ab6c` both skip past it),
    // so it is a property of *the attribute being written*, not of the value:
    //
    //   absent / empty  → align 1, floated 0        `left`/`right` → 1 / 4, floated 1
    //   `center`        → 2, floated 0              `top`/`middle`/`bottom` → 8/16/32, floated 0
    //   anything else   → align stays 1, floated **1** (`0x6f1990` left the local at its default)
    if let Some(v) = attr_ci(node, "align").filter(|v| !v.is_empty()) {
        if let Some(bits) = justify::parse_bits(v) {
            align = bits;
        }
        floated = align == ALIGN_LEFT || align == ALIGN_RIGHT;
    }
    Block::Image {
        src: attr_ci(node, "src").map(str::to_string),
        width: atof(attr_ci(node, "width").unwrap_or("")),
        height: atof(attr_ci(node, "height").unwrap_or("")),
        align,
        floated,
    }
}

/// `align` on a `<P>`/`<H1>`/`<H2>`/`<H3>` (`0x78a7cf`): the shared 6-entry enum at
/// `.rdata 0x811ad0`, which [`justify::parse_bits`] already owns one transcription of.
///
/// `0x6f1990` writes `*out` only on a hit, and the local was pre-loaded with `1`, so an **absent,
/// empty or unrecognised** `align` leaves the block at LEFT. `top`/`middle`/`bottom` are accepted
/// and yield 8/16/32, which mask to `& 7 == 0` — a block with no justifyH bit set at all. That is
/// a degenerate state the binary really reaches, not a rejection.
fn align_of(node: roxmltree::Node) -> u32 {
    attr_ci(node, "align")
        .filter(|v| !v.is_empty())
        .and_then(justify::parse_bits)
        .unwrap_or(ALIGN_LEFT)
}

/// Attribute lookup, **case-insensitively** — every attribute-name compare in this engine goes
/// through `0x64a4c0` → `SStrCmpI 0x414310`, so `ALIGN`/`Align`/`align` are one attribute.
/// roxmltree's own `Node::attribute` is case-sensitive, which is why this scan exists.
fn attr_ci<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value())
}

/// An element's own direct character data, concatenated in document order — the `<A>`'s inner
/// text. roxmltree's `Node::text` answers only the FIRST text child, which a comment or an entity
/// boundary is enough to split.
fn direct_text(node: roxmltree::Node) -> String {
    node.children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|n| n.text())
        .collect()
}

/// `SStrPrintf(tmp, len, hyperlinkFormat, href, innerText)` (`0x78a97b` → `0x64a7f0`) — a C
/// `printf` with exactly two arguments, both strings.
///
/// Only `%s` and `%%` are honoured; any other spec is copied through verbatim rather than
/// consuming an argument. A real `printf` given `%d` and a `char*` would read the pointer as an
/// integer, which is not behaviour worth reproducing, and `SetHyperlinkFormat` is a
/// player-reachable string — the ctor default `"|H%s|h%s|h"` and every format an addon writes are
/// two-`%s` templates.
fn sprintf_two(fmt: &str, a: &str, b: &str) -> String {
    let mut out = String::with_capacity(fmt.len() + a.len() + b.len());
    let mut used = 0usize;
    let mut it = fmt.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('%') => {
                it.next();
                out.push('%');
            }
            Some('s') => {
                it.next();
                out.push_str(match used {
                    0 => a,
                    1 => b,
                    _ => "",
                });
                used += 1;
            }
            _ => out.push('%'),
        }
    }
    out
}

/// C's `atof` (`0x64aaa0`), as `<IMG width=>`/`height=` reach it: leading whitespace skipped, the
/// longest valid numeric prefix parsed, **0 on no parse at all** — `"12px"` is 12, `"px"` is 0.
/// Rust's `str::parse` rejects a trailing suffix outright, which would turn a sloppy but working
/// `width="100 "` into a zero-width image.
fn atof(s: &str) -> f32 {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let start = i;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    // An exponent counts only when it is complete — `"1e"` is 1, matching strtod's own backtrack.
    if i < b.len() && (b[i] | 0x20) == b'e' {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    s[start..i].parse::<f32>().unwrap_or(0.0)
}

/// The HTML-style whitespace collapse — `0x78a8af`–`0x78aa87`, a three-arm dispatch through
/// the jump table at `.text 0x78aab8` with the byte remap at `.text 0x78aac4`.
///
/// | arm | characters | behaviour |
/// |---|---|---|
/// | 0 `0x78aa34` | `\t` `\n` `\r` ` ` | suppress-leading → drop; a space already pending → drop; else emit one `0x20` and set pending |
/// | 1 `0x78aa46` | `\|` followed by `n` | un-emit a pending space (`0x78aa54 dec eax`), clear pending, emit `\|n`, set suppress-leading, skip both bytes |
/// | 2 `0x78aa66` | everything else, `\|` **not** followed by `n` included | clear suppress-leading and pending, emit the byte verbatim |
///
/// Tail `0x78aa7b` removes a trailing pending space. Net: **leading and trailing whitespace
/// trimmed, every internal run of tabs/newlines/CRs/spaces collapsed to one space, and a space
/// immediately before a `|n` removed** — exactly HTML's rule.
///
/// It is byte-wise and can still never split a UTF-8 sequence: the index is computed with the
/// **signed** widening `0x78a8c5 movsx edi,bl`, so every byte `>= 0x80` goes negative and the
/// unsigned `cmp edi,0x73; ja` sends it to arm 2 — lead and continuation bytes alike are copied
/// verbatim. Operating on `&[u8]` here reproduces that exactly, and the result is valid UTF-8 by
/// construction because only ASCII whitespace is ever dropped.
///
/// **This applies to markup blocks only.** The plain-text path (`0x78a501`) bypasses it entirely.
fn collapse(buf: &str) -> String {
    let src = buf.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    // `dl` — "a space is already pending" — and `[ebp+0xb]`, "suppress leading whitespace",
    // initialised to **1** so a block never starts with a space.
    let mut pending_space = false;
    let mut suppress_leading = true;
    let mut i = 0;
    while i < src.len() {
        let ch = src[i];
        if ch == b'|' && src.get(i + 1) == Some(&b'n') {
            if pending_space {
                debug_assert_eq!(out.last(), Some(&b' '));
                out.pop();
            }
            pending_space = false;
            out.extend_from_slice(b"|n");
            suppress_leading = true;
            i += 2;
        } else if matches!(ch, b'\t' | b'\n' | b'\r' | b' ') {
            if !suppress_leading && !pending_space {
                out.push(b' ');
                pending_space = true;
            }
            i += 1;
        } else {
            suppress_leading = false;
            pending_space = false;
            out.push(ch);
            i += 1;
        }
    }
    if pending_space {
        debug_assert_eq!(out.last(), Some(&b' '));
        out.pop();
    }
    String::from_utf8(out).expect("collapse drops only ASCII whitespace")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(p: &Parse) -> Vec<(&str, usize, u32)> {
        p.blocks
            .iter()
            .map(|b| match b {
                Block::Text { text, elem, align } => (text.as_str(), *elem, *align),
                Block::Image { .. } => ("<IMG>", usize::MAX, 0),
            })
            .collect()
    }

    #[test]
    fn collapse_is_htmls_rule() {
        assert_eq!(collapse("  a \t\n b  "), "a b");
        assert_eq!(collapse("a \n\n b"), "a b");
        // A space immediately before a `|n` is removed; whitespace right after one is suppressed.
        assert_eq!(collapse("a |n b"), "a|nb");
        // A `|` not followed by `n` is verbatim — colour spans and hyperlinks survive.
        assert_eq!(
            collapse("  |cffff0000red|r  text  "),
            "|cffff0000red|r text"
        );
        // Multi-byte sequences are never split (the signed-widening arm-2 fall-through).
        assert_eq!(collapse("  héllo  wörld "), "héllo wörld");
        assert_eq!(collapse("   "), "");
    }

    #[test]
    fn atof_takes_the_numeric_prefix() {
        assert_eq!(atof("64"), 64.0);
        assert_eq!(atof(" 12.5px"), 12.5);
        assert_eq!(atof("-3"), -3.0);
        assert_eq!(atof("px"), 0.0);
        assert_eq!(atof(""), 0.0);
        assert_eq!(atof("1e2"), 100.0);
        assert_eq!(atof("1e"), 1.0);
    }

    #[test]
    fn hyperlink_format_takes_exactly_two_strings() {
        assert_eq!(
            sprintf_two(DEFAULT_HYPERLINK_FORMAT, "item:1234", "Thunderfury"),
            "|Hitem:1234|hThunderfury|h"
        );
        assert_eq!(sprintf_two("%s%%%s", "a", "b"), "a%b");
    }

    #[test]
    fn a_well_formed_body_is_one_block_per_tag() {
        let p = parse_markup(
            "<HTML><BODY><H1 align=\"center\">Title</H1><BR/><P>Body   text</P></BODY></HTML>",
            "F",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert!(p.used_markup);
        assert_eq!(
            texts(&p),
            [
                ("Title", 1, ALIGN_CENTER),
                ("\n", ELEM_P, ALIGN_LEFT),
                ("Body text", ELEM_P, ALIGN_LEFT),
            ]
        );
        assert!(p.errors.is_empty());
    }

    #[test]
    fn the_three_fallback_routes_each_render_the_raw_string() {
        for raw in [
            "Just some prose with an & in it.",       // parse failure
            "<FOO><BAR/></FOO>",                      // wrong root
            "<HTML><HEAD>no body here</HEAD></HTML>", // no BODY child
        ] {
            let p = parse_markup(raw, "F", DEFAULT_HYPERLINK_FORMAT);
            assert!(!p.used_markup, "{raw}");
            assert_eq!(texts(&p), [(raw, ELEM_P, ALIGN_LEFT)], "{raw}");
        }
    }

    #[test]
    fn the_fallback_keeps_raw_newlines_because_it_skips_the_collapse() {
        let p = parse_markup("\nline one\nline two\n", "F", DEFAULT_HYPERLINK_FORMAT);
        assert_eq!(texts(&p), [("\nline one\nline two\n", ELEM_P, ALIGN_LEFT)]);
    }

    #[test]
    fn an_empty_body_renders_nothing_at_all() {
        // `usedMarkup` is raised BEFORE the walker runs (`0x78a4b6`), so this does NOT fall back.
        let p = parse_markup("<HTML><BODY/></HTML>", "F", DEFAULT_HYPERLINK_FORMAT);
        assert!(p.used_markup);
        assert!(p.blocks.is_empty());
    }

    #[test]
    fn body_level_character_data_is_dropped() {
        let p = parse_markup(
            "<HTML><BODY>\nloose\n<P>kept</P>\nmore loose\n</BODY></HTML>",
            "F",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert_eq!(texts(&p), [("kept", ELEM_P, ALIGN_LEFT)]);
    }

    #[test]
    fn malformed_markup_falls_back_rather_than_erroring() {
        for raw in [
            "<HTML><BODY><P>a&nbsp;b</P></BODY></HTML>", // undefined entity (expat 11)
            "<HTML><BODY><P>hi</p></BODY></HTML>",       // case-mismatched close (expat 7)
            "<HTML><BODY><P>a<BR>b</P></BODY></HTML>",   // unclosed BR (expat 7)
            "<HTML><BODY><P>hi</P></BODY></HTML>\nFrom: Bob", // junk after root (expat 9)
            "<HTML><BODY><P align=\"l\" align=\"r\">x</P></BODY></HTML>", // dup attr (expat 8)
        ] {
            let p = parse_markup(raw, "F", DEFAULT_HYPERLINK_FORMAT);
            assert!(!p.used_markup, "{raw}");
            assert_eq!(texts(&p), [(raw, ELEM_P, ALIGN_LEFT)], "{raw}");
        }
    }

    #[test]
    fn tag_and_attribute_matching_is_case_insensitive() {
        let p = parse_markup(
            "<html><body><p ALIGN=\"Right\">x</p></body></html>",
            "F",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert_eq!(texts(&p), [("x", ELEM_P, ALIGN_RIGHT)]);
    }

    #[test]
    fn an_unrecognised_tag_is_dropped_and_the_walk_continues() {
        let p = parse_markup(
            "<HTML><BODY><P>one</P><TABLE/><P>two</P></BODY></HTML>",
            "Book",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert_eq!(
            texts(&p),
            [("one", ELEM_P, ALIGN_LEFT), ("two", ELEM_P, ALIGN_LEFT)]
        );
        assert_eq!(p.errors, ["Frame Book: Unknown element type: TABLE"]);
    }

    #[test]
    fn an_unknown_inline_loses_its_own_text_and_keeps_the_text_either_side() {
        let p = parse_markup(
            "<HTML><BODY><P>a<B>bold</B>b</P></BODY></HTML>",
            "Book",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert_eq!(texts(&p), [("ab", ELEM_P, ALIGN_LEFT)]);
        assert_eq!(p.errors, ["Frame Book: Unknown element type: B"]);
    }

    #[test]
    fn inline_br_and_a_splice_where_the_tag_was() {
        let p = parse_markup(
            "<HTML><BODY><P>see <A href=\"item:1\">this</A> now<BR/>and more</P></BODY></HTML>",
            "F",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert_eq!(
            texts(&p),
            [("see |Hitem:1|hthis|h now|nand more", ELEM_P, ALIGN_LEFT)]
        );
    }

    #[test]
    fn an_a_without_href_or_without_text_contributes_nothing() {
        let p = parse_markup(
            "<HTML><BODY><P>x<A>t</A>y<A href=\"h\"></A>z</P></BODY></HTML>",
            "F",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert_eq!(texts(&p), [("xyz", ELEM_P, ALIGN_LEFT)]);
    }

    #[test]
    fn only_the_first_body_is_walked_and_the_second_never_errors() {
        let p = parse_markup(
            "<HTML><BODY><P>one</P></BODY><BODY><P>two</P></BODY></HTML>",
            "F",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert_eq!(texts(&p), [("one", ELEM_P, ALIGN_LEFT)]);
        assert!(p.errors.is_empty());
    }

    #[test]
    fn a_non_body_child_of_html_errors_before_the_body_that_follows() {
        let p = parse_markup(
            "<HTML><HEAD/><BODY><P>x</P></BODY></HTML>",
            "F",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert!(p.used_markup);
        assert_eq!(texts(&p), [("x", ELEM_P, ALIGN_LEFT)]);
        assert_eq!(
            p.errors,
            ["Frame F: Unknown element type: HEAD (expected BODY)"]
        );
    }

    #[test]
    fn img_float_is_a_property_of_the_attribute_not_the_value() {
        let cases: [(&str, u32, bool); 6] = [
            ("<IMG src=\"a\"/>", ALIGN_LEFT, false),
            ("<IMG align=\"left\" src=\"a\"/>", ALIGN_LEFT, true),
            ("<IMG align=\"right\" src=\"a\"/>", ALIGN_RIGHT, true),
            ("<IMG align=\"center\" src=\"a\"/>", ALIGN_CENTER, false),
            ("<IMG align=\"top\" src=\"a\"/>", 0x08, false),
            ("<IMG align=\"foo\" src=\"a\"/>", ALIGN_LEFT, true),
        ];
        for (tag, want_align, want_float) in cases {
            let p = parse_markup(
                &format!("<HTML><BODY>{tag}</BODY></HTML>"),
                "F",
                DEFAULT_HYPERLINK_FORMAT,
            );
            match &p.blocks[..] {
                [Block::Image { align, floated, .. }] => {
                    assert_eq!((*align, *floated), (want_align, want_float), "{tag}")
                }
                other => panic!("{tag}: {other:?}"),
            }
        }
    }

    /// The stock `ItemTextFrame`'s `page_text` shape, end to end: an `<H1>` title, a `<BR/>`, and
    /// centred paragraphs, with the body's own inter-tag newlines contributing nothing.
    #[test]
    fn the_page_text_shape_walks_to_the_block_list_a_reader_would_draw() {
        let p = parse_markup(
            "<HTML>\n<BODY>\n<H1 align=\"center\">The Green Hills of Stranglethorn</H1>\n<BR/>\n\
             <P align=\"center\">Chapter One:\nThe Mysteries of the Jungle</P>\n<BR/>\n\
             <P align=\"center\">Deep in the jungle, all is not as it seems.</P>\n</BODY>\n</HTML>",
            "ItemTextPageText",
            DEFAULT_HYPERLINK_FORMAT,
        );
        assert!(p.used_markup);
        assert_eq!(
            texts(&p),
            [
                ("The Green Hills of Stranglethorn", 1, ALIGN_CENTER),
                ("\n", ELEM_P, ALIGN_LEFT),
                (
                    "Chapter One: The Mysteries of the Jungle",
                    ELEM_P,
                    ALIGN_CENTER
                ),
                ("\n", ELEM_P, ALIGN_LEFT),
                (
                    "Deep in the jungle, all is not as it seems.",
                    ELEM_P,
                    ALIGN_CENTER
                ),
            ]
        );
        assert!(p.errors.is_empty());
    }

    /// The signed-page consequence: `ItemTextFrame.lua` appends `From: <creator>` **after**
    /// `</HTML>`, which is expat error 9 (`0x881fec`) — so a signed HTML page renders as raw
    /// markup on the reference client too. Our fallback must reach the same place rather than
    /// "fixing" it.
    #[test]
    fn a_signed_html_page_falls_back_exactly_as_the_reference_does() {
        let raw = "\n<HTML><BODY><P>The letter body.</P></BODY></HTML>\n\nFrom:\nMankrik\n\n";
        let p = parse_markup(raw, "ItemTextPageText", DEFAULT_HYPERLINK_FORMAT);
        assert!(!p.used_markup);
        assert_eq!(texts(&p), [(raw, ELEM_P, ALIGN_LEFT)]);
    }
}
