//! The `SimpleHTML` widget once its blocks are regions: the anchor chain, block widths, element
//! fonts and their `P` fallback, spacing, the free on rebuild and the 19-name Lua table.

use super::common::script;
use crate::layout::Point;
use crate::script::{Model, UiScript};
use crate::widget::RegionKind;

/// One block as a reader would describe it.
#[derive(Clone, Debug, PartialEq)]
struct Blk {
    text: Option<String>,
    texture: Option<String>,
    justify_h: &'static str,
    font: Option<String>,
    font_height: Option<f32>,
    /// `(frame width, 0)` for a text block: a 0 height takes the measured one.
    size: Option<(f32, f32)>,
    /// `(own point, relative point, x, y, target)`.
    anchor: Option<(Point, Point, f32, f32, Rel)>,
}

/// What a block's single anchor points at.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Rel {
    /// The SimpleHTML frame; only block 0 anchors to it.
    Frame,
    Block(usize),
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
            }
            .tap_kind(kind)
        })
        .collect()
}

impl Blk {
    /// Asserts a text block is a FontString and an image block a Texture.
    fn tap_kind(self, kind: RegionKind) -> Blk {
        match (&self.text, &self.texture) {
            (Some(_), None) => assert_eq!(kind, RegionKind::FontString),
            (None, Some(_)) => assert_eq!(kind, RegionKind::Texture),
            _ => {}
        }
        self
    }
}

fn texts(s: &UiScript, name: &str) -> Vec<String> {
    blocks(s, name)
        .into_iter()
        .map(|b| b.text.unwrap_or_default())
        .collect()
}

/// The frame's region count, which still counts a block orphaned rather than freed.
fn region_count(s: &UiScript, name: &str) -> usize {
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("frame");
    model.arena.frame(fh).expect("live frame").regions.len()
}

/// A `SimpleHTML` named `Page`, 270×304 and in `ItemTextFontNormal` like the stock
/// `ItemTextPageText` (`ItemTextFrame.xml:203`).
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

/// Answer every pending measure at 16 px a line and 100 wide, with the reference's line count
/// (`0x5c2070`): a trailing break opens no line, so a `<BR/>` block's `"\n"` is one line, and an
/// empty string is none.
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

// ── The walk, as blocks on the frame ──

/// Each tag's block carries its `align`, and with only `P` declared every block is in `P`'s face.
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
    // `elementFont[1]` has no path, so `0x78ae30` substitutes `elementFont[0]`; nothing scales a
    // header.
    for blk in &b {
        assert_eq!(blk.font.as_deref(), Some("Fonts\\MORPHEUS.TTF"));
        assert_eq!(blk.font_height, Some(15.0));
        assert_eq!(blk.size, Some((270.0, 0.0)));
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Block 0 pins TOPLEFT to the frame's TOPLEFT and each later block TOPLEFT to the previous one's
/// BOTTOMLEFT at `-spacing` (`0x767c70`); spacing starts at 0 (`0x770d89`), so the blocks touch.
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

    // The frame top is the screen top, 600; three 16 px lines run down to 552.
    measure_at_16px(&mut s);
    let tops = resolved_tops(&s, "Page");
    assert_eq!(tops, [(600.0, 584.0), (584.0, 568.0), (568.0, 552.0)]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

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

/// A BODY-level `<BR/>` is its own `"\n"` block, which `0x5c2070` counts as one line.
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

/// An inline `<BR/>` splices `|n` into the enclosing block, a line break within it.
#[test]
fn an_inline_br_stays_inside_its_block() {
    let s = script();
    page(&s);
    s.run(r#"Page:SetText("<HTML><BODY><P>a<BR/>b</P></BODY></HTML>")"#)
        .unwrap();
    assert_eq!(texts(&s, "Page"), ["a|nb"]);
}

/// Each fallback route is one block holding the raw, uncollapsed string; most vmangos `page_text`
/// rows are plain text and take it.
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

/// A second `SetText` frees the first parse's regions, so nothing of the old page draws.
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

/// A real `page_text` body: the `<H1>` at the `<P>` size, each `<BR/>` one blank line, and the
/// newlines between tags adding nothing.
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
    // Five flush 16 px blocks: 600 down to 520.
    measure_at_16px(&mut s);
    let tops = resolved_tops(&s, "Page");
    assert_eq!(tops.first(), Some(&(600.0, 584.0)));
    assert_eq!(tops.last(), Some(&(536.0, 520.0)));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `ItemTextFrame.lua:44` pads an authorless page as `"\n" .. ItemTextGetText() .. "\n"`, and
/// whitespace around the root element is legal XML, so it still takes the markup path. The body is
/// vmangos `page_text` 2676, the Alliance Military Ranks plaque (`GameObject` 3011).
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
    // The fallback would put the whole source in one block.
    assert!(
        !texts[0].contains('<'),
        "a fallback would render the markup itself — the reported symptom"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `page_text` 1510, the Crystal Pylon manual, ends `</HTML.`, so strict XML rejects it and the
/// reference shows the raw markup too; it is the only malformed one of the 62 HTML bodies.
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

/// An unfloated `<IMG>` reserves its height in the flow, but `prevBlock` stays the last text block.
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

/// The `<IMG>` in `page_text` 2654 has no `width=` or `height=`, and the reference's
/// `CSimpleTexture::GetWidth 0x770720`/`GetHeight 0x770790` answer an authored 0 with the art's
/// texel size, a texel to a FrameXML unit: the 128×128 crest is a 128-unit square.
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
    // The page is 270 wide; the crest must not span it.
    assert!(
        rect.right - rect.left < 270.0,
        "the crest is stretched across the page width again"
    );
}

/// An unfloated image reserves `GetHeight()` in the flow (`0x78ad07`), its art's texel height when
/// it has no `height=`; a floated one reserves nothing, so text runs over it in the reference too.
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

/// With no size oracle a zero-size image has no span and does not resolve, as in the reference for
/// a texture with no `CGxTex*`: `assemble 0x767a20` returns 0 and the quad is all zeros.
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

// ── The Lua table ──

/// The 19 names of `.data 0x87ba80`, and only those: there is no `GetText`.
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
    // The names are the SimpleHTML's own: a Frame does not answer them.
    s.run(r#"F = CreateFrame("Frame")"#).unwrap();
    assert!(!s
        .eval::<bool>("return F.SetHyperlinkFormat ~= nil")
        .unwrap());
}

/// The optional element name (`0x795d80`): absent means `P`, the four names match in any case, and
/// any other string stays as the first real argument.
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
    // "h4" is no element, so it is `SetFont`'s path and the call targets `P`.
    s.run(r#"H:SetFont("h4", 12)"#).unwrap();
    assert_eq!(
        s.eval::<(String, f32)>("local p, h = H:GetFont(); return p, h")
            .unwrap(),
        ("h4".to_string(), 12.0)
    );
}

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

    // A non-string argument raises the reference's usage string.
    assert!(s.run("Page:SetHyperlinkFormat(7)").is_err());
}

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
    // `P` keeps the font object's colour: the element argument scoped the write.
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

/// An element with no font path takes `elementFont[0]` whole, colour included: `0x78ae29` to
/// `0x78ae54` swap the register before the one `SetFontObject`, so an `H1` given only a colour
/// draws in `P`'s.
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
    // The getter still answers red: it reads the element font, never a block (`0x795d3e` reads
    // `[this + idx*4 + 0x350]`).
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

    // With a path of its own, `H1` draws its red.
    s.run(
        r#"
        Page:SetFont("H1", "Fonts\\SKURRI.TTF", 24)
        Page:SetText("<HTML><BODY><H1>t</H1></BODY></HTML>")
    "#,
    )
    .unwrap();
    assert_eq!(block_color(&s, "Page", 0), Some([1.0, 0.0, 0.0, 1.0]));
}

/// Block `i`'s `vertex_color`, which for a FontString is the colour it draws.
fn block_color(s: &UiScript, name: &str, i: usize) -> Option<[f32; 4]> {
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("frame");
    let st = model.simple_html.get(&fh).expect("state");
    model.region_data.get(&st.blocks[i])?.vertex_color
}

/// `SetFontObject` on an element font: a re-point keeps what a local setter set (the inheritMask
/// bit at `CSimpleFont+0x2c`/`CSimpleFontString+0xD4` is never restored), and nil unlinks the
/// object but leaves the paint.
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

/// `0x78adb0` gives every block its tag's `align`, so `SetJustifyH` is stored and answered but
/// never drawn.
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
    // A non-token raises the usage string (`0x87c77c`).
    assert!(s.run(r#"Page:SetJustifyH("sideways")"#).is_err());
}

// ── Helpers that need the resolve ──

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

fn maybe_resolved_rect(s: &UiScript, name: &str, block: usize) -> Option<crate::layout::Rect> {
    let lua = s.lua();
    let model = lua.app_data_ref::<Model>().expect("model");
    let fh = model.arena.lookup(name).expect("frame");
    let st = model.simple_html.get(&fh).expect("state");
    model.region_resolved.get(&st.blocks[block]).copied()
}

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

/// The text of every quad the extract emits: what is on screen.
fn page_texts_on_screen(s: &UiScript) -> Vec<String> {
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            crate::script::QuadContent::Text { text: Some(t), .. } => Some(t),
            _ => None,
        })
        .collect()
}

/// A Rust string as a Lua long-bracket literal, so quotes and backslashes need no second escape.
fn lua_str(s: &str) -> String {
    format!("[==[{s}]==]")
}
