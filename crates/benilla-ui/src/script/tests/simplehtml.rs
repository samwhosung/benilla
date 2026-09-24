//! The `SimpleHTML` widget end to end — the markup engine's *model* half.
//!
//! The parse itself (strict XML, the three fallback routes, the collapse, the splices) is unit
//! tested in `script::simplehtml::parse`, against the byte law directly. What is asserted here is
//! everything that only exists once the blocks are real regions: the anchor chain, the width
//! snapshot, the element-font resolution and its H1→P fallback, the spacing step, the free on
//! rebuild, and the 19-name Lua table.

use super::common::script;
use crate::layout::Point;
use crate::script::{Model, UiScript};
use crate::widget::RegionKind;

/// One block as a reader would describe it: what it says, how it is justified, what face it is in,
/// how wide it was told to be, and where it hangs from.
#[derive(Clone, Debug, PartialEq)]
struct Blk {
    text: Option<String>,
    texture: Option<String>,
    justify_h: &'static str,
    font: Option<String>,
    font_height: Option<f32>,
    /// The `size` the widget wrote: `(frame width, 0)` for a text block — a zero height is how the
    /// measure round-trip is asked for the intrinsic one.
    size: Option<(f32, f32)>,
    /// `(own point, relative point, x, y, target)`, `None` when the block carries no anchor at all.
    anchor: Option<(Point, Point, f32, f32, Rel)>,
}

/// What a block's single anchor points at.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Rel {
    /// The SimpleHTML frame itself — only block 0 ever does this.
    Frame,
    /// A previous block, by index in the block list.
    Block(usize),
    /// Something else entirely (never expected; a failure would print it).
    Other,
}

/// Read every block of the named SimpleHTML off the model, in emission order.
fn blocks(s: &UiScript, name: &str) -> Vec<Blk> {
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("SimpleHTML frame");
    let frame_id = model.frame_to_id.get(&fh).copied();
    let st = model.simple_html.get(&fh).expect("SimpleHTML state");
    let ids: Vec<Option<u32>> = st
        .blocks
        .iter()
        .map(|rh| model.region_to_id.get(rh).copied())
        .collect();
    st.blocks
        .iter()
        .map(|&rh| {
            let d = model.region_data.get(&rh).expect("block region data");
            let kind = model.arena.region(rh).expect("live block").kind;
            let anchor = d.anchors.first().map(|a| {
                let rel = if Some(a.relative_to) == frame_id {
                    Rel::Frame
                } else {
                    ids.iter()
                        .position(|id| *id == Some(a.relative_to))
                        .map_or(Rel::Other, Rel::Block)
                };
                (a.point, a.relative_point, a.x_off, a.y_off, rel)
            });
            Blk {
                text: d.text.clone(),
                texture: d.texture.clone(),
                justify_h: d.justify.name_h(),
                font: d.font_path.clone(),
                font_height: d.font_height,
                size: d.size,
                anchor,
                // A texture block has no text; keeping the kind out of the struct and asserting it
                // here keeps the expected-value literals readable.
            }
            .tap_kind(kind)
        })
        .collect()
}

impl Blk {
    /// A tiny consistency check folded into the read: a text block is a FontString, an image block
    /// is a Texture. A regression that made blocks the wrong region kind would otherwise pass
    /// every text assertion below.
    fn tap_kind(self, kind: RegionKind) -> Blk {
        match (&self.text, &self.texture) {
            (Some(_), None) => assert_eq!(kind, RegionKind::FontString),
            (None, Some(_)) => assert_eq!(kind, RegionKind::Texture),
            _ => {}
        }
        self
    }
}

/// Just the strings, for the tests that only care about the walk.
fn texts(s: &UiScript, name: &str) -> Vec<String> {
    blocks(s, name)
        .into_iter()
        .map(|b| b.text.unwrap_or_default())
        .collect()
}

/// How many regions the frame owns in total — the free-on-rebuild check's instrument. A block that
/// was orphaned rather than destroyed still sits in its owner's region list and still draws.
fn region_count(s: &UiScript, name: &str) -> usize {
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("frame");
    model.arena.frame(fh).expect("live frame").regions.len()
}

/// A `SimpleHTML` named `Page`, 270×304 like the reference's `ItemTextPageText`, with one font
/// object behind it (the `<FontString inherits="ItemTextFontNormal"/>` shape).
fn page(s: &UiScript) {
    s.run(
        r#"
        local f = CreateFont("ItemTextFontNormal")
        f:SetFont("Fonts\\MORPHEUS.TTF", 15)
        f:SetTextColor(0.18, 0.12, 0.06)
        Page = CreateFrame("SimpleHTML", "Page")
        Page:SetPoint("TOPLEFT", 0, 0)
        Page:SetWidth(270); Page:SetHeight(304)
        Page:SetFontObject("P", ItemTextFontNormal)
    "#,
    )
    .unwrap();
}

/// Answer every pending measure at 16px a line and 100 wide, so the resolved rects below are
/// arithmetic a reader can check.
///
/// The line count is the client's own kernel `0x5c2070`, not "breaks + 1": it counts a line per
/// class-2 token consumed and exits at the terminator, so a **trailing** break opens no new line
/// and the one-byte `"\n"` a `<BR/>` block carries is **one** line, not two. An empty string
/// never enters the loop body at all and measures 0 — the empty-`<P>` edge.
fn measure_at_16px(s: &mut UiScript) {
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    let answers: Vec<(u32, f32, f32, u64)> = reqs
        .iter()
        .map(|r| (r.id, 100.0, 16.0 * client_lines(&r.text) as f32, r.key))
        .collect();
    s.set_measured_text_unwrapped(&answers);
    s.resolve();
}

/// `0x5c2070`'s line count for a string, at the fidelity these tests need.
fn client_lines(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let breaks = text.matches('\n').count() + text.matches("|n").count();
    if text.ends_with('\n') || text.ends_with("|n") {
        breaks
    } else {
        breaks + 1
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The walk, as blocks on the frame
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A well-formed body: one block per tag, each carrying the tag's own `align` and — because only
/// `<FontString>` was declared — every one of them in `P`'s face, the `<H1>` included.
#[test]
fn a_well_formed_body_is_one_block_per_tag() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    page(&s);
    s.run(
        r#"Page:SetText("<HTML><BODY>" ..
            "<H1 align=\"center\">Title</H1>" ..
            "<P>Left body.</P>" ..
            "<P align=\"right\">Right body.</P>" ..
            "</BODY></HTML>")"#,
    )
    .unwrap();

    let b = blocks(&s, "Page");
    assert_eq!(b.len(), 3);
    assert_eq!(
        b.iter()
            .map(|x| (x.text.clone().unwrap(), x.justify_h))
            .collect::<Vec<_>>(),
        [
            ("Title".to_string(), "CENTER"),
            ("Left body.".to_string(), "LEFT"),
            ("Right body.".to_string(), "RIGHT"),
        ]
    );
    // `elementFont[1]`'s path is empty, so `0x78ae30` substitutes `elementFont[0]`. There
    // is NO header scaling anywhere in the TU: the H1 is the same 15px as the paragraphs.
    for blk in &b {
        assert_eq!(blk.font.as_deref(), Some("Fonts\\MORPHEUS.TTF"));
        assert_eq!(blk.font_height, Some(15.0));
        // `SetWidth(frame.GetWidth())`, and NO height — the intrinsic one comes from the measure.
        assert_eq!(blk.size, Some((270.0, 0.0)));
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The anchor chain (`CLayoutFrame::SetPoint 0x767c70`) and the flush default (`0x770d89` zeroes
/// spacing): block 0 pins TOPLEFT→frame TOPLEFT, every later block TOPLEFT→**the previous block's**
/// BOTTOMLEFT at `−spacing` — which is `0` out of the box, so the blocks touch.
#[test]
fn blocks_chain_bottom_to_top_and_are_flush_at_spacing_zero() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    page(&s);
    s.run(r#"Page:SetText("<HTML><BODY><P>one</P><P>two</P><P>three</P></BODY></HTML>")"#)
        .unwrap();

    let b = blocks(&s, "Page");
    assert_eq!(
        b.iter().map(|x| x.anchor.unwrap()).collect::<Vec<_>>(),
        [
            (Point::TopLeft, Point::TopLeft, 0.0, 0.0, Rel::Frame),
            (Point::TopLeft, Point::BottomLeft, 0.0, 0.0, Rel::Block(0)),
            (Point::TopLeft, Point::BottomLeft, 0.0, 0.0, Rel::Block(1)),
        ]
    );

    // …and the resolved rects really do stack with no gap. Frame top is 600 (screen top, TOPLEFT
    // at 0,0); three 16px lines run 600→584→568→552.
    measure_at_16px(&mut s);
    let tops = resolved_tops(&s, "Page");
    assert_eq!(tops, [(600.0, 584.0), (584.0, 568.0), (568.0, 552.0)]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetSpacing` is the only thing that opens a gap, and it opens it by exactly its own value.
#[test]
fn spacing_steps_the_blocks_apart() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    page(&s);
    s.run(
        r#"
        Page:SetSpacing(4)
        Page:SetText("<HTML><BODY><P>one</P><P>two</P></BODY></HTML>")
    "#,
    )
    .unwrap();
    let b = blocks(&s, "Page");
    assert_eq!(
        b[1].anchor.unwrap(),
        (Point::TopLeft, Point::BottomLeft, 0.0, -4.0, Rel::Block(0)),
        "block N+1 sits at block N's BOTTOMLEFT displaced by -spacing"
    );
    assert_eq!(s.eval::<f32>("return Page:GetSpacing()").unwrap(), 4.0);
    // A negative spacing is clamped to 0 before it is stored (`0x772246`).
    s.run("Page:SetSpacing(-9)").unwrap();
    assert_eq!(s.eval::<f32>("return Page:GetSpacing()").unwrap(), 0.0);

    measure_at_16px(&mut s);
    assert_eq!(
        resolved_tops(&s, "Page"),
        [(600.0, 584.0), (580.0, 564.0)],
        "the second block starts 4px below the first's bottom"
    );
}

/// `<BR/>` at BODY level is its own `"\n"` block, and `0x5c2070` counts **one** line for it — one
/// blank line, not two and not a paragraph gap.
#[test]
fn a_body_level_br_is_exactly_one_blank_line() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    page(&s);
    s.run(r#"Page:SetText("<HTML><BODY><P>a</P><BR/><P>b</P></BODY></HTML>")"#)
        .unwrap();
    assert_eq!(texts(&s, "Page"), ["a", "\n", "b"]);
    measure_at_16px(&mut s);
    assert_eq!(
        resolved_tops(&s, "Page"),
        [(600.0, 584.0), (584.0, 568.0), (568.0, 552.0)],
        "the BR block is one line tall and pushes `b` down by exactly that"
    );
}

/// Inline `<BR/>` is a different animal: it splices the two bytes `|n` into the SAME block, which
/// the font engine turns into a line break within it.
#[test]
fn an_inline_br_stays_inside_its_block() {
    let s = script();
    page(&s);
    s.run(r#"Page:SetText("<HTML><BODY><P>a<BR/>b</P></BODY></HTML>")"#)
        .unwrap();
    assert_eq!(texts(&s, "Page"), ["a|nb"]);
}

/// All three fallback routes land on ONE block holding the RAW string — no collapse, so embedded
/// newlines survive as real line breaks. This is the common case for a vmangos `page_text`.
#[test]
fn the_three_fallback_routes_each_land_on_one_raw_block() {
    let s = script();
    page(&s);
    for raw in [
        "Plain prose, with an & and a < in it.\nSecond line.",
        "<FOO><BAR/></FOO>",
        "<HTML><HEAD>nothing</HEAD></HTML>",
    ] {
        s.run(&format!("Page:SetText({})", lua_str(raw))).unwrap();
        let b = blocks(&s, "Page");
        assert_eq!(b.len(), 1, "{raw}");
        assert_eq!(b[0].text.as_deref(), Some(raw), "{raw}");
        assert_eq!(b[0].justify_h, "LEFT", "{raw}");
        assert_eq!(
            b[0].anchor.unwrap(),
            (Point::TopLeft, Point::TopLeft, 0.0, 0.0, Rel::Frame),
            "{raw}"
        );
    }
}

/// Malformed markup **falls back**; it never raises and never renders half a document.
#[test]
fn malformed_markup_falls_back_rather_than_erroring() {
    let s = script();
    page(&s);
    for raw in [
        "<HTML><BODY><P>a&nbsp;b</P></BODY></HTML>",
        "<HTML><BODY><P>hi</p></BODY></HTML>",
        "<HTML><BODY><P>hi</P></BODY></HTML>\nFrom: Bob",
    ] {
        s.run(&format!("Page:SetText({})", lua_str(raw))).unwrap();
        assert_eq!(texts(&s, "Page"), [raw], "{raw}");
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **A second `SetText` destroys the first parse's blocks.** Not hides, not orphans — the regions
/// are gone from the frame, so nothing of the old page can draw behind the new one.
#[test]
fn a_second_set_text_replaces_the_blocks() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    page(&s);
    s.run(r#"Page:SetText("<HTML><BODY><P>one</P><P>two</P><P>three</P></BODY></HTML>")"#)
        .unwrap();
    assert_eq!(region_count(&s, "Page"), 3);
    measure_at_16px(&mut s);
    assert_eq!(page_texts_on_screen(&s).len(), 3);

    s.run(r#"Page:SetText("<HTML><BODY><P>only</P></BODY></HTML>")"#)
        .unwrap();
    assert_eq!(texts(&s, "Page"), ["only"]);
    assert_eq!(
        region_count(&s, "Page"),
        1,
        "the previous parse's FontStrings are freed, not left on the frame"
    );
    measure_at_16px(&mut s);
    assert_eq!(
        page_texts_on_screen(&s),
        ["only"],
        "and nothing of the old page still draws"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The real `page_text` shape `ItemTextFrame.xml`'s ItemTextPageText renders, driven through the
/// widget: the `<H1>` at the `<P>` size, every block centred, each `<BR/>` one blank line, and the
/// body's own inter-tag newlines adding nothing at all.
#[test]
fn the_page_text_body_renders_the_block_list_a_reader_would_draw() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    page(&s);
    s.run(
        "Page:SetText(\"<HTML>\\n<BODY>\\n\
         <H1 align=\\\"center\\\">The Green Hills of Stranglethorn</H1>\\n<BR/>\\n\
         <P align=\\\"center\\\">Chapter One:\\nThe Mysteries of the Jungle</P>\\n<BR/>\\n\
         <P align=\\\"center\\\">Deep in the jungle, all is not as it seems.</P>\\n\
         </BODY>\\n</HTML>\")",
    )
    .unwrap();

    let b = blocks(&s, "Page");
    assert_eq!(
        b.iter()
            .map(|x| (x.text.clone().unwrap(), x.justify_h, x.font_height))
            .collect::<Vec<_>>(),
        [
            (
                "The Green Hills of Stranglethorn".to_string(),
                "CENTER",
                Some(15.0)
            ),
            ("\n".to_string(), "LEFT", Some(15.0)),
            (
                "Chapter One: The Mysteries of the Jungle".to_string(),
                "CENTER",
                Some(15.0)
            ),
            ("\n".to_string(), "LEFT", Some(15.0)),
            (
                "Deep in the jungle, all is not as it seems.".to_string(),
                "CENTER",
                Some(15.0)
            ),
        ],
        "the H1 renders at the P size — nothing in the TU scales a header"
    );
    // Five blocks flush at 16px each: 600 → 520.
    measure_at_16px(&mut s);
    let tops = resolved_tops(&s, "Page");
    assert_eq!(tops.first(), Some(&(600.0, 584.0)));
    assert_eq!(tops.last(), Some(&(536.0, 520.0)));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **The exact string the reader builds**, on the exact body B240 reports — `ItemTextFrame.xml`'s
/// READY handler pads an authorless page as `"\n" .. ItemTextGetText() .. "\n"`, and that padding
/// is the one thing standing between a formatted page and the raw markup Goudy photographed.
/// Whitespace before and after the root element is legal XML, so this must take the MARKUP path;
/// if it ever takes the fallback, every book in the world silently renders as its own source.
///
/// The body is vmangos `page_text` 2676 verbatim — the *Alliance Military Ranks* plaque in
/// Stormwind's Old Town, `GameObject` 3011, the object in the report's screenshots.
#[test]
fn the_readers_own_newline_padding_still_parses_as_markup() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    page(&s);
    s.run(
        "Page:SetText(\"\\n\" .. \
         \"<HTML>\\n<BODY>\\n\
         <H1 align=\\\"center\\\">ALLIANCE MILITARY RANKS</H1><BR/>\\n\
         <P align=\\\"center\\\">OFFICERS</P><BR/>\\n\
         <P align=\\\"center\\\">Grand Marshal</P>\\n\
         <P align=\\\"center\\\">Field Marshal</P>\\n\
         <P align=\\\"center\\\">Knight</P><BR/>\\n\
         <P align=\\\"center\\\">ENLISTED</P><BR/>\\n\
         <P align=\\\"center\\\">Private</P>\\n\
         </BODY>\\n</HTML>\" .. \"\\n\")",
    )
    .unwrap();

    let texts = texts(&s, "Page");
    assert_eq!(
        texts,
        [
            "ALLIANCE MILITARY RANKS",
            "\n",
            "OFFICERS",
            "\n",
            "Grand Marshal",
            "Field Marshal",
            "Knight",
            "\n",
            "ENLISTED",
            "\n",
            "Private",
        ],
        "the padded page took the markup path — one block per tag, one blank line per <BR/>, and \
         BODY's own inter-tag newlines adding nothing"
    );
    // The falsification, stated: the fallback would put the WHOLE source in one block.
    assert!(
        !texts[0].contains('<'),
        "a fallback would render the markup itself — the reported symptom"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The other half of the same guard: `page_text` **1510** (the Crystal Pylon manual) ends
/// `</HTML.` — a typo in Blizzard's own shipped data. Strict XML rejects it, so it falls back to
/// raw markup **on the reference client too**, and ours must do the same rather than paper over
/// it. This is the one of the world's 62 HTML bodies that is not well-formed.
#[test]
fn blizzards_own_malformed_page_falls_back_exactly_as_the_reference_does() {
    let s = script();
    page(&s);
    s.run(
        "Page:SetText(\"<HTML> <BODY> <H1 align=\\\"center\\\"> CRYSTAL PYLON USER'S MANUAL </H1> \
         <BR/> <P align=\\\"left\\\">Chapter 1: The Northern Pylon </P> </BODY> </HTML.\")",
    )
    .unwrap();

    let texts = texts(&s, "Page");
    assert_eq!(texts.len(), 1, "one raw block, not a parsed page");
    assert!(
        texts[0].starts_with("<HTML>") && texts[0].ends_with("</HTML."),
        "the fallback ships the ORIGINAL string, unmodified and uncollapsed"
    );
}

/// A declared header font **is** honoured — the fallback fires only on an element with no font
/// from any source, so this is the control for the test above.
#[test]
fn a_declared_header_font_wins_over_the_p_fallback() {
    let s = script();
    page(&s);
    s.run(
        r#"
        local h = CreateFont("BookHeader")
        h:SetFont("Fonts\\SKURRI.TTF", 24)
        Page:SetFontObject("H1", BookHeader)
        Page:SetText("<HTML><BODY><H1>Title</H1><P>Body</P></BODY></HTML>")
    "#,
    )
    .unwrap();
    let b = blocks(&s, "Page");
    assert_eq!(
        (b[0].font.as_deref(), b[0].font_height),
        (Some("Fonts\\SKURRI.TTF"), Some(24.0))
    );
    assert_eq!(
        (b[1].font.as_deref(), b[1].font_height),
        (Some("Fonts\\MORPHEUS.TTF"), Some(15.0))
    );
}

/// An `<IMG>` sizes itself, anchors by `align`, and — when it is NOT floated — reserves its own
/// height in the flow while leaving `prevBlock` pointing at the last **text** block.
#[test]
fn an_unfloated_image_reserves_height_without_becoming_the_anchor() {
    let s = script();
    page(&s);
    s.run(
        r#"Page:SetText("<HTML><BODY><P>a</P>" ..
            "<IMG src=\"Interface\\Pic\" width=\"64\" height=\"32\"/>" ..
            "<P>b</P></BODY></HTML>")"#,
    )
    .unwrap();
    let b = blocks(&s, "Page");
    assert_eq!(b[1].texture.as_deref(), Some("Interface\\Pic"));
    assert_eq!(b[1].size, Some((64.0, 32.0)));
    assert_eq!(
        b[1].anchor.unwrap(),
        (Point::TopLeft, Point::BottomLeft, 0.0, 0.0, Rel::Block(0))
    );
    assert_eq!(
        b[2].anchor.unwrap(),
        (Point::TopLeft, Point::BottomLeft, 0.0, -32.0, Rel::Block(0)),
        "the next text block still hangs off the last TEXT block, only 32px lower"
    );
}

/// **B342, at the engine.** Goudy, 2026-08-27 (`#bugs` `1542371921486811236`): *"html images in
/// books are not scaled correctly"* — the Alliance crest on *A Treatise on Military Ranks* drawn
/// several times the reference's size, with the page's text over the top of it.
///
/// The body is `page_text` 2654 and its `<IMG>` carries **no `width=` and no `height=`**, so the
/// block is sized `0 × 0`. In the reference that is not "no size": the resolver's size call is
/// VIRTUAL, and `CSimpleTexture::GetWidth 0x770720` / `GetHeight 0x770790` answer an authored `0.0`
/// with the loaded texture's own texel extent, through bit-for-bit the converter `<AbsDimension>`
/// uses — **one texel is one FrameXML unit** (decision 1349).
/// `PvPRankAlliance.blp` is 128×128, so the crest is a 128-unit square.
///
/// The falsification is the reported shape itself, asserted below: a zero span leaves the opposite
/// edge unresolved (`combine_edge`'s `span != 0.0` leg), and the region sweep's owner-edge fallback
/// then hands the image the rest of the **270-wide** page.
#[test]
fn the_books_unsized_crest_is_its_arts_texel_square_not_the_whole_page() {
    const CREST: &str = "Interface\\PvPRankBadges\\PvPRankAlliance";
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_texture_size_probe(Box::new(|path| (path == CREST).then_some((128, 128))));
    page(&s);
    s.run(&format!(
        "Page:SetText({})",
        lua_str(
            "<HTML><BODY>\n\
             <H1 align=\"center\">A TREATISE ON MILITARY RANKS</H1>\n\
             <BR/>\n<BR/>\n\
             <IMG src=\"Interface\\PvPRankBadges\\PvPRankAlliance\" align=\"left\"/>\n\
             <BR/>\n\
             <P align=\"right\">What follows are</P>\n\
             </BODY></HTML>"
        )
    ))
    .unwrap();
    measure_at_16px(&mut s);

    let b = blocks(&s, "Page");
    let img = b
        .iter()
        .position(|x| x.texture.is_some())
        .expect("an IMG block");
    assert_eq!(b[img].texture.as_deref(), Some(CREST));
    assert_eq!(
        b[img].size,
        Some((0.0, 0.0)),
        "the block still stores what the markup authored — the span is DERIVED at resolve, as the \
         reference's virtual getter derives it"
    );

    let rect = resolved_rect(&s, "Page", img);
    assert_eq!(
        (rect.right - rect.left, rect.top - rect.bottom),
        (128.0, 128.0),
        "a texel is a FrameXML unit: the 128x128 crest is a 128-unit square, not the page"
    );
    // The reported symptom, stated as the thing that must not come back: the page is 270 wide and
    // 304 tall, and the crest used to take all of the first and everything left of the second.
    assert!(
        rect.right - rect.left < 270.0,
        "the crest is stretched across the page width again"
    );
}

/// The flow half of the same law (`0x78ad07`): an **unfloated** image reserves
/// `texture.GetHeight()` — `CSimpleTexture`'s override — so one with no `height=` reserves
/// its art's texel height, and the next text block hangs that far lower. A floated one (the book's
/// `align="left"`) still reserves nothing, which is why its text overlaps it in both clients.
#[test]
fn an_unsized_image_reserves_its_arts_texel_height_unless_it_is_floated() {
    let mut s = script();
    s.set_texture_size_probe(Box::new(|_| Some((64, 48))));
    page(&s);
    s.run(
        r#"Page:SetText("<HTML><BODY><P>a</P>" ..
            "<IMG src=\"Interface\\Pic\"/>" ..
            "<P>b</P></BODY></HTML>")"#,
    )
    .unwrap();
    assert_eq!(
        blocks(&s, "Page")[2].anchor.unwrap(),
        (Point::TopLeft, Point::BottomLeft, 0.0, -48.0, Rel::Block(0)),
        "no height= reserves the ART's height, not nothing"
    );

    let mut s = script();
    s.set_texture_size_probe(Box::new(|_| Some((64, 48))));
    page(&s);
    s.run(
        r#"Page:SetText("<HTML><BODY><P>a</P>" ..
            "<IMG src=\"Interface\\Pic\" align=\"left\"/>" ..
            "<P>b</P></BODY></HTML>")"#,
    )
    .unwrap();
    assert_eq!(
        blocks(&s, "Page")[2].anchor.unwrap(),
        (Point::TopLeft, Point::BottomLeft, 0.0, 0.0, Rel::Block(0)),
        "a FLOATED image reserves nothing — the text runs over it, in both clients"
    );
}

/// A VM with no host oracle has no art to measure, so a zero-size image has **no span on either
/// axis** — and with one corner pinned and nothing to add to it, it does not resolve at all. That
/// is the reference's own answer for a texture with no `CGxTex*`: `combineEdge`'s legs both fail
/// their `span != 0.0` test, `assemble 0x767a20` returns 0, and the texture emits a degenerate
/// all-zero quad. Stated so the
/// engine-less path cannot quietly go back to borrowing the page's width.
#[test]
fn without_a_size_oracle_an_unsized_image_does_not_resolve() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    page(&s);
    s.run(r#"Page:SetText("<HTML><BODY><P>a</P><IMG src=\"Interface\\Pic\"/></BODY></HTML>")"#)
        .unwrap();
    measure_at_16px(&mut s);
    let b = blocks(&s, "Page");
    let img = b
        .iter()
        .position(|x| x.texture.is_some())
        .expect("an IMG block");
    assert!(
        maybe_resolved_rect(&s, "Page", img).is_none(),
        "no art, no span, no rect"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The Lua table
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The 19 names of `.data 0x87ba80`, and only those 19 — `GetText` is **not** one of them in
/// build 5875, and a name a table does not carry is as wrong as a missing one.
#[test]
fn the_method_table_is_the_nineteen_names() {
    let s = script();
    s.run(r#"H = CreateFrame("SimpleHTML")"#).unwrap();
    for name in [
        "SetFontObject",
        "GetFontObject",
        "SetFont",
        "GetFont",
        "SetTextColor",
        "GetTextColor",
        "SetShadowColor",
        "GetShadowColor",
        "SetShadowOffset",
        "GetShadowOffset",
        "SetSpacing",
        "GetSpacing",
        "SetJustifyH",
        "GetJustifyH",
        "SetJustifyV",
        "GetJustifyV",
        "SetText",
        "SetHyperlinkFormat",
        "GetHyperlinkFormat",
    ] {
        assert!(
            s.eval::<bool>(&format!("return H.{name} ~= nil")).unwrap(),
            "SimpleHTML:{name} is missing"
        );
    }
    assert!(
        !s.eval::<bool>("return H.GetText ~= nil").unwrap(),
        "1.12.1's SimpleHTML table has no GetText"
    );
    // The names are the SimpleHTML's own — no other widget kind answers them.
    s.run(r#"F = CreateFrame("Frame")"#).unwrap();
    assert!(!s
        .eval::<bool>("return F.SetHyperlinkFormat ~= nil")
        .unwrap());
}

/// The optional element-name argument (`0x795d80`): omitting it addresses `P`, the four names are
/// case-insensitive, and a **string that is not one of the four is not removed** — it becomes the
/// shared implementation's first real argument.
#[test]
fn the_element_name_argument_is_optional_case_insensitive_and_non_matching_falls_through() {
    let s = script();
    s.run(
        r#"
        H = CreateFrame("SimpleHTML")
        H:SetFont("Fonts\\A.TTF", 10)          -- no element name: targets P
        H:SetFont("h2", "Fonts\\B.TTF", 20)    -- "h2" matches, case-insensitively
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(String, f32)>("local p, h = H:GetFont(); return p, h")
            .unwrap(),
        ("Fonts\\A.TTF".to_string(), 10.0)
    );
    assert_eq!(
        s.eval::<(String, f32)>("local p, h = H:GetFont(\"H2\"); return p, h")
            .unwrap(),
        ("Fonts\\B.TTF".to_string(), 20.0)
    );
    // "h4" is not an element: it stays on the stack and is consumed as SetFont's PATH, so this
    // targets P and sets the face to the literal "h4".
    s.run(r#"H:SetFont("h4", 12)"#).unwrap();
    assert_eq!(
        s.eval::<(String, f32)>("local p, h = H:GetFont(); return p, h")
            .unwrap(),
        ("h4".to_string(), 12.0)
    );
}

/// `SetHyperlinkFormat` governs how an `<A>` is spliced, and takes effect on the next parse.
#[test]
fn the_hyperlink_format_shapes_the_a_splice() {
    let s = script();
    page(&s);
    assert_eq!(
        s.eval::<String>("return Page:GetHyperlinkFormat()")
            .unwrap(),
        "|H%s|h%s|h"
    );
    s.run(r#"Page:SetText("<HTML><BODY><P>see <A href=\"item:1\">this</A></P></BODY></HTML>")"#)
        .unwrap();
    assert_eq!(texts(&s, "Page"), ["see |Hitem:1|hthis|h"]);

    s.run(r#"Page:SetHyperlinkFormat("|cff33ff99|H%s|h[%s]|h|r")"#)
        .unwrap();
    s.run(r#"Page:SetText("<HTML><BODY><P>see <A href=\"item:1\">this</A></P></BODY></HTML>")"#)
        .unwrap();
    assert_eq!(texts(&s, "Page"), ["see |cff33ff99|Hitem:1|h[this]|h|r"]);

    // A non-string argument raises the reference's own usage string rather than no-opping.
    assert!(s.run("Page:SetHyperlinkFormat(7)").is_err());
}

/// The colour/shadow/justify getters answer per element, and `SetTextColor` reaches the blocks the
/// next parse builds.
#[test]
fn the_paint_setters_land_on_the_element_and_reach_the_next_parse() {
    let s = script();
    page(&s);
    s.run(
        r#"
        Page:SetTextColor("H1", 1, 0, 0)
        Page:SetShadowColor(0, 0, 0, 0.5)
        Page:SetShadowOffset(1, -1)
        Page:SetText("<HTML><BODY><H1>t</H1><P>b</P></BODY></HTML>")
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return Page:GetTextColor(\"H1\")")
            .unwrap(),
        (1.0, 0.0, 0.0, 1.0)
    );
    // P keeps the font object's own colour — the element arg really did scope the write.
    let (r, g, b, _) = s
        .eval::<(f32, f32, f32, f32)>("return Page:GetTextColor(\"P\")")
        .unwrap();
    assert_eq!((r, g, b), (0.18, 0.12, 0.06));
    assert_eq!(
        s.eval::<(f32, f32)>("return Page:GetShadowOffset()")
            .unwrap(),
        (1.0, -1.0)
    );
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return Page:GetShadowColor()")
            .unwrap(),
        (0.0, 0.0, 0.0, 0.5)
    );
}

/// **The empty-path fallback substitutes the WHOLE element font, colour included** — so an `H1`
/// given a colour and nothing else renders in `P`'s colour, and its own red is never drawn.
///
/// That is the shape of `0x78ae29`–`0x78ae54`: the register holding `elementFont[elem]` is
/// *replaced* by `elementFont[0]` before the single `SetFontObject` call, so the test is on the
/// path and the consequence is on everything. Surprising, faithful, and exactly the sort of thing
/// a re-implementation gets wrong by merging property-by-property instead.
#[test]
fn an_element_with_no_font_of_its_own_loses_its_own_colour_too() {
    let s = script();
    page(&s);
    s.run(
        r#"
        Page:SetTextColor("H1", 1, 0, 0)
        Page:SetText("<HTML><BODY><H1>t</H1></BODY></HTML>")
    "#,
    )
    .unwrap();
    // The getter still answers the red — the element font really does hold it (`0x795d3e` reads
    // `[this + idx*4 + 0x350]`, never a block).
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return Page:GetTextColor(\"H1\")")
            .unwrap(),
        (1.0, 0.0, 0.0, 1.0)
    );
    assert_eq!(
        block_color(&s, "Page", 0),
        Some([0.18, 0.12, 0.06, 1.0]),
        "the block took elementFont[0] wholesale, so H1's red never reaches it"
    );

    // Give H1 a path of its own and the fallback stops firing — now the red draws.
    s.run(
        r#"
        Page:SetFont("H1", "Fonts\\SKURRI.TTF", 24)
        Page:SetText("<HTML><BODY><H1>t</H1></BODY></HTML>")
    "#,
    )
    .unwrap();
    assert_eq!(block_color(&s, "Page", 0), Some([1.0, 0.0, 0.0, 1.0]));
}

/// The `vertex_color` of block `i` — a FontString has no texel, so its vertex colour IS the colour
/// it draws.
fn block_color(s: &UiScript, name: &str, i: usize) -> Option<[f32; 4]> {
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("frame");
    let st = model.simple_html.get(&fh).expect("state");
    model.region_data.get(&st.blocks[i])?.vertex_color
}

/// The two halves of `SetFontObject`'s severance law, which the element font has to reproduce by
/// hand because it resolves its object lazily rather than copying the paint at the call:
///
/// - a **re-point does not reset** the explicit mask (`CSimpleFont+0x2c`/`CSimpleFontString+0xD4`:
///   the inheritMask bit a local setter clears is never restored), so a face set on the element
///   survives being pointed at a different object;
/// - the **nil form severs the link and leaves the paint standing** — it does not blank the
///   element.
#[test]
fn set_font_object_keeps_local_overrides_and_the_nil_form_leaves_the_paint_standing() {
    let s = script();
    s.run(
        r#"
        local a = CreateFont("FaceA"); a:SetFont("Fonts\\A.TTF", 10); a:SetTextColor(1, 0, 0)
        local b = CreateFont("FaceB"); b:SetFont("Fonts\\B.TTF", 20); b:SetTextColor(0, 1, 0)
        H = CreateFrame("SimpleHTML")
        H:SetFontObject(FaceA)
        H:SetFont("Fonts\\OWN.TTF", 33)   -- an explicit face + height
        H:SetFontObject(FaceB)             -- re-point: the colour follows, the face does not
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(String, f32)>("local p, h = H:GetFont(); return p, h")
            .unwrap(),
        ("Fonts\\OWN.TTF".to_string(), 33.0)
    );
    assert_eq!(
        s.eval::<(f32, f32, f32)>("local r, g, b = H:GetTextColor(); return r, g, b")
            .unwrap(),
        (0.0, 1.0, 0.0),
        "the colour was never set locally, so it re-reads from the new object"
    );

    s.run("H:SetFontObject(nil)").unwrap();
    assert!(s.eval::<bool>("return H:GetFontObject() == nil").unwrap());
    assert_eq!(
        s.eval::<(f32, f32, f32)>("local r, g, b = H:GetTextColor(); return r, g, b")
            .unwrap(),
        (0.0, 1.0, 0.0),
        "severing the link leaves the resolved paint standing, it does not blank the element"
    );
}

/// `SetJustifyH` is **inert for rendered text** — the byte law, not a gap: `0x78adb0` overwrites
/// every block's justifyH with the tag's `align`. The getter still answers what was stored.
#[test]
fn set_justify_h_is_stored_and_answered_but_never_drawn() {
    let s = script();
    page(&s);
    s.run(
        r#"
        Page:SetJustifyH("RIGHT")
        Page:SetText("<HTML><BODY><P>a</P></BODY></HTML>")
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return Page:GetJustifyH()").unwrap(),
        "RIGHT"
    );
    assert_eq!(
        blocks(&s, "Page")[0].justify_h,
        "LEFT",
        "the tag's absent align (LEFT) wins over the element font's justify"
    );
    // A non-token raises, exactly as `0x87c77c`'s usage arm does.
    assert!(s.run(r#"Page:SetJustifyH("sideways")"#).is_err());
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Helpers that need the resolve
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Every block's resolved `(top, bottom)`, in block order.
fn resolved_tops(s: &UiScript, name: &str) -> Vec<(f32, f32)> {
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("frame");
    let st = model.simple_html.get(&fh).expect("state");
    st.blocks
        .iter()
        .map(|rh| {
            let r = model.region_resolved.get(rh).expect("block rect");
            (r.top, r.bottom)
        })
        .collect()
}

/// One block's resolved rect if it HAS one — the shape a region with no span reaches.
fn maybe_resolved_rect(s: &UiScript, name: &str, block: usize) -> Option<crate::layout::Rect> {
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("frame");
    let st = model.simple_html.get(&fh).expect("state");
    model.region_resolved.get(&st.blocks[block]).copied()
}

/// One block's resolved rect, by index in the block list.
fn resolved_rect(s: &UiScript, name: &str, block: usize) -> crate::layout::Rect {
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("frame");
    let st = model.simple_html.get(&fh).expect("state");
    *model
        .region_resolved
        .get(&st.blocks[block])
        .expect("block rect")
}

/// The text of every quad the extract emits for the page — what is actually on screen.
fn page_texts_on_screen(s: &UiScript) -> Vec<String> {
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            crate::script::QuadContent::Text { text: Some(t), .. } => Some(t),
            _ => None,
        })
        .collect()
}

/// A Rust string as a Lua long-bracket literal — the bodies here are full of quotes and
/// backslashes, and escaping them twice is how a test asserts on the wrong string.
fn lua_str(s: &str) -> String {
    format!("[==[{s}]==]")
}
