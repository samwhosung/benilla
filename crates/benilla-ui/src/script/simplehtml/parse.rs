//! The markup parse of `CSimpleHTML::SetText`: a string in, a block list out, with no Lua or model.
//!
//! The markup is strict XML: `0x78a422` calls `XMLTree::Parse` (`0x6f2a30`), an embedded expat
//! 1.95.5 (`0x882094`). An unclosed `<BR>`, a `</p>` closing a `<P>` (XML matching is
//! case-sensitive, SimpleHTML's own name compares are not), a bare `&`, an entity such as `&nbsp;`,
//! a duplicate attribute or text after `</HTML>` fails the parse, and the widget renders the raw
//! string (`0x78a501`). [`roxmltree`] gives expat's outcome on each of those and on what passes
//! (trailing whitespace, numeric references, comments, CDATA), and its interleaved children give
//! the `<A>` and `<BR>` splice points directly. A DOCTYPE takes the fallback here; whether the
//! reference's expat accepts one is untraced, and no `page_text` body carries one.

use crate::justify;

/// The four block elements in the client's `elementFont` order (`+0x350`), the numbering the ctor
/// (`0x789e3e`), the block builder (`0x78ae29`) and the Lua resolver (`0x795d80`) share.
pub(crate) const ELEMENT_NAMES: [&str; 4] = ["P", "H1", "H2", "H3"];

/// `P`: the element of the fallback block (`0x78a503`), of `<BR/>`, and of a Lua call without a
/// known element name (`0x795e50`).
pub(crate) const ELEM_P: usize = 0;

/// The `align` default, set before the attribute is read (`0x78a7c8` for a block, `0x78ab59` for
/// an `<IMG>`): LEFT, which `0x78ae78` writes over the FontString ctor's CENTER.
pub(crate) const ALIGN_LEFT: u32 = 0x01;
/// `align="center"`: the value `0x6f1990` writes from the shared enum table (`0x811ad0`).
pub(crate) const ALIGN_CENTER: u32 = 0x02;
/// `align="right"`.
pub(crate) const ALIGN_RIGHT: u32 = 0x04;

/// The ctor's `hyperlinkFormat` (`0x87a838`, set at `0x789ea7` and `0x78a540`): `<A href="X">Y</A>`
/// becomes `|HX|hY|h`, a hyperlink to the FontString, which never disables `|H` (`0x771d80`).
pub(crate) const DEFAULT_HYPERLINK_FORMAT: &str = "|H%s|h%s|h";

/// One block, the arguments of `AddTextBlock` (`0x78adb0`) or `AddImage` (`0x78ab40`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    /// A text block: one `CSimpleFontString`, `SetWidth(frame width)`, no height.
    Text {
        /// The spliced, collapsed string, or the raw input on the fallback path (`0x78a501`).
        text: String,
        /// The `elementFont[]` index this block draws with, before the empty-path fallback.
        elem: usize,
        /// The `align` bits (`0x811ad0`'s values); masked `& 7` into the block's justifyH.
        align: u32,
    },
    /// An `<IMG>`: one `CSimpleTexture`, sized by `width`/`height` and anchored by `align`.
    Image {
        /// `src=`, used verbatim as the texture path (`0x78ad02`).
        src: Option<String>,
        /// `width=` and `height=` in UI units, as `<AbsDimension>` (`0x78ab9e`, `0x78abd6`); 0
        /// when absent.
        width: f32,
        height: f32,
        /// The `align` bits; selects which corner anchors to the previous block.
        align: u32,
        /// Reserves no height, so the text after overlaps it (`0x78ab55`). Set only when `align`
        /// is present, so `<IMG src=…/>` and `<IMG align="left" src=…/>` anchor alike but flow
        /// differently.
        floated: bool,
    },
}

/// The outcome of one `SetText` parse.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Parse {
    pub(crate) blocks: Vec<Block>,
    /// `SetText`'s return (`0x78a519`): whether the markup path ran. The Lua shim (`0x796a90`)
    /// drops it.
    pub(crate) used_markup: bool,
    /// The console lines the error sink received, formatted with the frame's name.
    pub(crate) errors: Vec<String>,
}

/// The parse half of `SetText` (`0x78a3a0`). `frame` names the widget in error lines;
/// `hyperlink_format` is the current `+0x360`.
pub(crate) fn parse_markup(raw: &str, frame: &str, hyperlink_format: &str) -> Parse {
    let mut out = Parse {
        blocks: Vec::new(),
        used_markup: false,
        errors: Vec::new(),
    };
    // `usedMarkup` is raised before the BODY walk (`0x78a4b6`), so an empty `<BODY/>` renders
    // nothing rather than the raw string.
    if let Ok(doc) = roxmltree::Document::parse(raw) {
        let root = doc.root_element();
        if root.tag_name().name().eq_ignore_ascii_case("HTML") {
            // Only the first `BODY` is walked, and a second never errors (`0x78a4bf` leaves the
            // loop after `0x78a660` returns).
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
        // A failed parse, a non-`HTML` root or no `BODY` all land on `0x78a501`: the raw string
        // as one LEFT `P` block, without the collapse, as `0x78a7b0` is never entered, so its `\n`
        // stay line breaks, which stock `ItemTextFrame.lua` relies on.
        out.blocks.push(Block::Text {
            text: raw.to_string(),
            elem: ELEM_P,
            align: ALIGN_LEFT,
        });
    }
    out
}

/// The BODY walker (`0x78a660`), by child name, case-insensitively. BODY's own text is dropped, as
/// nothing reads it (`+0x0c`), so the newlines between a `page_text` body's tags add nothing.
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
            // A `\n` block of its own (`0x78a726`), one blank line tall (`0x5c2070`).
            out.blocks.push(Block::Text {
                text: "\n".to_string(),
                elem: ELEM_P,
                align: ALIGN_LEFT,
            });
        } else if tag.eq_ignore_ascii_case("IMG") {
            out.blocks.push(image(node));
        } else {
            // Logged (`0x87a95c`) and skipped; the walk goes on (`0x78a762`-`0x78a796`).
            out.errors
                .push(format!("Frame {frame}: Unknown element type: {tag}"));
        }
    }
}

/// One `<P>`, `<H1>`, `<H2>` or `<H3>` into one block (`0x78a7b0`).
fn paragraph(
    node: roxmltree::Node,
    elem: usize,
    frame: &str,
    hyperlink_format: &str,
    errors: &mut Vec<String>,
) -> Block {
    let align = align_of(node);
    // The reference splices each inline child into the node's text at its recorded offset
    // (`+0x18`); appending roxmltree's interleaved children in order puts each at the same place.
    let mut buf = String::new();
    for child in node.children() {
        if child.is_text() {
            buf.push_str(child.text().unwrap_or(""));
            continue;
        }
        if !child.is_element() {
            // A comment or PI, which expat does not deliver as text either.
            continue;
        }
        let tag = child.tag_name().name();
        if tag.eq_ignore_ascii_case("BR") {
            // An inline `<BR/>` is `|n`, a break inside this block (`0x78a86b`, `0x87a9a0`).
            buf.push_str("|n");
        } else if tag.eq_ignore_ascii_case("A") {
            // `<A>` needs a non-empty `href` and text, or adds nothing (`0x78a8f0`,
            // `0x78a914`-`0x78a922`).
            let href = attr_ci(child, "href").unwrap_or("");
            let inner = direct_text(child);
            if !href.is_empty() && !inner.is_empty() {
                buf.push_str(&sprintf_two(hyperlink_format, href, &inner));
            }
        } else {
            // Logged as at BODY level (`0x78a9f9`-`0x78aa2c`); its own text, which expat keeps on
            // the child, is lost and the text around it kept.
            errors.push(format!("Frame {frame}: Unknown element type: {tag}"));
        }
    }
    Block::Text {
        text: collapse(&buf),
        elem,
        align,
    }
}

/// One `<IMG>` (`0x78ab40`).
fn image(node: roxmltree::Node) -> Block {
    let mut align = ALIGN_LEFT;
    let mut floated = false;
    // Floating is decided only when `align` is present and non-empty (`0x78ab55`-`0x78ab6c`):
    //   absent or empty: align 1, not floated     `left`, `right`: 1, 4, floated
    //   `center`: 2, not floated                  `top`, `middle`, `bottom`: 8, 16, 32, not floated
    //   anything else: align stays 1, floated (`0x6f1990` leaves the default)
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

/// A block's `align` (`0x78a7cf`) through the shared enum (`0x811ad0`, [`justify::parse_bits`]):
/// absent, empty or unknown stays LEFT, as `0x6f1990` writes only on a hit. `top`, `middle` and
/// `bottom` give 8, 16 and 32, which leave no justifyH bit, in the reference too.
fn align_of(node: roxmltree::Node) -> u32 {
    attr_ci(node, "align")
        .filter(|v| !v.is_empty())
        .and_then(justify::parse_bits)
        .unwrap_or(ALIGN_LEFT)
}

/// Case-insensitive attribute lookup, as the reference compares names (`0x64a4c0`, `SStrCmpI`
/// `0x414310`); roxmltree's own lookup is case-sensitive.
fn attr_ci<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value())
}

/// An element's direct text, all of it: roxmltree's `Node::text` gives only the first text child,
/// which a comment splits.
fn direct_text(node: roxmltree::Node) -> String {
    node.children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|n| n.text())
        .collect()
}

/// `SStrPrintf` of the format with `href` and the inner text (`0x78a97b`, `0x64a7f0`). Deviation:
/// only `%s` and `%%` are expanded and other specs copied, because the real `printf` would read the
/// string pointers as other types.
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

/// C's `atof` (`0x64aaa0`) for `width=` and `height=`: the longest numeric prefix after leading
/// whitespace, 0 when there is none, so `"12px"` is 12.
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
    // An exponent counts only when complete: `"1e"` is 1, as `strtod` backtracks.
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

/// The whitespace collapse of markup blocks (`0x78a8af`-`0x78aa87`; the fallback at `0x78a501`
/// skips it): leading and trailing whitespace dropped, each run of tabs, newlines, CRs and spaces
/// made one space, and whitespace on either side of `|n` removed. Its arms: whitespace
/// (`0x78aa34`), `|n` (`0x78aa46`, `0x78aa54`), any other byte (`0x78aa66`), and a tail that drops
/// a pending space (`0x78aa7b`). A byte from `0x80` up widens negative (`0x78a8c5`) and takes the
/// verbatim arm, so UTF-8 is never split.
fn collapse(buf: &str) -> String {
    let src = buf.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    // Suppression starts on, so a block never starts with a space.
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
        // A space right before `|n` is removed; whitespace right after one is suppressed.
        assert_eq!(collapse("a |n b"), "a|nb");
        // A `|` not before `n` is verbatim, so colour codes survive.
        assert_eq!(
            collapse("  |cffff0000red|r  text  "),
            "|cffff0000red|r text"
        );
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
        // `usedMarkup` rises before the walk (`0x78a4b6`), so this does not fall back.
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

    /// A stock `page_text` body: an `<H1>` title, `<BR/>`s and centred paragraphs.
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

    /// Stock `ItemTextFrame.lua` appends `From: <creator>` after `</HTML>`, expat error 9
    /// (`0x881fec`), so a signed page renders as raw markup on the reference too.
    #[test]
    fn a_signed_html_page_falls_back_exactly_as_the_reference_does() {
        let raw = "\n<HTML><BODY><P>The letter body.</P></BODY></HTML>\n\nFrom:\nMankrik\n\n";
        let p = parse_markup(raw, "ItemTextPageText", DEFAULT_HYPERLINK_FORMAT);
        assert!(!p.used_markup);
        assert_eq!(texts(&p), [(raw, ELEM_P, ALIGN_LEFT)]);
    }
}
