//! The text region's rect (`0x77b8c0`, re-seated on every resize by `0x77a8d0`) and a multi-line
//! box's own height (`0x77d4d0` @`0x77d8ad`), driven through the Lua methods and the tick.

use crate::script::{MeasureRequest, QuadContent, TextMeasure, UiScript};

/// A stand-in font engine: each character [`CW`] wide and each line `.0` tall. Hard breaks split
/// lines, a trailing one opening none as in the host's line law, and each line wraps by
/// characters at the request's wrap width.
struct Lines(f32);

const CW: f32 = 7.0;
const LH: f32 = 14.0;

impl TextMeasure for Lines {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        let mut lines: Vec<&str> = req.text.split('\n').collect();
        if lines.len() > 1 && lines.last() == Some(&"") {
            lines.pop();
        }
        let width = |l: &str| l.chars().count() as f32 * CW;
        let natural = lines.iter().map(|l| width(l)).fold(0.0, f32::max);
        let wrap = req.wrap_width.filter(|w| *w > 0.0);
        let rows: f32 = lines
            .iter()
            .map(|l| match wrap {
                Some(w) if width(l) > w => (width(l) / w).ceil(),
                _ => 1.0,
            })
            .sum();
        (
            wrap.map_or(natural, |w| natural.min(w)),
            rows * self.0,
            natural,
        )
    }
}

fn script() -> UiScript {
    let mut s = UiScript::new().expect("construct UiScript");
    s.set_screen_size(800.0, 600.0);
    s.set_text_measurer(Box::new(Lines(LH)));
    s
}

/// One frame of the host loop: the tick (where the flush relayouts), then the resolve.
fn frame(s: &mut UiScript) {
    s.tick(0.016);
    s.resolve();
}

fn height(s: &UiScript, name: &str) -> f64 {
    s.eval(&format!("return {name}:GetHeight()")).unwrap()
}

/// A 200-wide multi-line box with insets (left 15, right 13, top 2, bottom 3), authored 200 tall.
fn inset_box(s: &mut UiScript) {
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetAutoFocus(false)
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetWidth(200); E:SetHeight(200)
        E:SetTextInsets(15, 13, 2, 3)
        E:SetMultiLine(true)
    "#,
    )
    .unwrap();
    s.resolve();
}

/// The box is `(top + bottom) + h` tall, `h` the text's wrapped height, one line's when empty; it
/// grows and shrinks with its text, and the authored 200 lasts only until the first flush.
#[test]
fn a_multi_line_box_is_as_tall_as_its_text_and_its_insets() {
    let mut s = script();
    inset_box(&mut s);
    assert_eq!(
        height(&s, "E"),
        200.0,
        "the authored height, before any flush"
    );

    frame(&mut s);
    assert_eq!(
        height(&s, "E"),
        f64::from(5.0 + LH),
        "an empty box is one line tall: the first flush replaces the authored 200"
    );

    s.run(r#"E:SetText("one\ntwo\nthree")"#).unwrap();
    frame(&mut s);
    assert_eq!(height(&s, "E"), f64::from(5.0 + 3.0 * LH), "three lines");

    // 30 characters are 210 wide against the 172 between the side insets: two rows.
    s.run(r#"E:SetText(string.rep("x", 30))"#).unwrap();
    frame(&mut s);
    assert_eq!(
        height(&s, "E"),
        f64::from(5.0 + 2.0 * LH),
        "a line wraps at the box's width less its side insets"
    );

    s.run(r#"E:SetText("")"#).unwrap();
    frame(&mut s);
    assert_eq!(height(&s, "E"), f64::from(5.0 + LH), "it shrinks back");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The relayout runs ahead of the `OnTextChanged` fire (`0x77d447` before `0x77d498`), so the
/// handler reads the new height.
#[test]
fn on_text_changed_sees_the_height_the_flush_just_set() {
    let mut s = script();
    inset_box(&mut s);
    s.run(r#"E:SetScript("OnTextChanged", function() SEEN = this:GetHeight() end)"#)
        .unwrap();
    frame(&mut s);
    s.run(r#"E:SetText("a\nb\nc\nd")"#).unwrap();
    s.tick(0.016);
    assert_eq!(
        s.eval::<f64>("return SEEN").unwrap(),
        f64::from(5.0 + 4.0 * LH)
    );
}

/// A new raster re-measures the empty box's line: the host's `invalidate_text_measures` drops the
/// cached line height with every other measure.
#[test]
fn a_new_raster_re_measures_the_empty_boxs_line() {
    let mut s = script();
    inset_box(&mut s);
    frame(&mut s);
    assert_eq!(height(&s, "E"), f64::from(5.0 + LH));
    s.set_text_measurer(Box::new(Lines(2.0 * LH)));
    s.invalidate_text_measures();
    frame(&mut s);
    assert_eq!(height(&s, "E"), f64::from(5.0 + 2.0 * LH));
}

/// A `SetHeight` lasts until the next flush, which writes the text's height back over it.
#[test]
fn a_scripted_height_is_written_over_at_the_next_flush() {
    let mut s = script();
    inset_box(&mut s);
    s.run(r#"E:SetText("a\nb")"#).unwrap();
    frame(&mut s);
    s.run("E:SetHeight(300)").unwrap();
    assert_eq!(
        height(&s, "E"),
        300.0,
        "the script's height, until the flush"
    );
    frame(&mut s);
    assert_eq!(height(&s, "E"), f64::from(5.0 + 2.0 * LH));
}

/// A trailing newline opens no line: the text's measure folds it into the line it ends, and the
/// box takes that measure as it is.
#[test]
fn a_trailing_newline_alone_adds_no_line() {
    let mut s = script();
    inset_box(&mut s);
    s.run(r#"E:SetText("abc")"#).unwrap();
    frame(&mut s);
    let one = height(&s, "E");
    s.run(r#"E:SetText("abc\n")"#).unwrap();
    frame(&mut s);
    assert_eq!(height(&s, "E"), one, "the break alone");
    s.run(r#"E:SetText("abc\nd")"#).unwrap();
    frame(&mut s);
    assert_eq!(
        height(&s, "E"),
        one + f64::from(LH),
        "the first character on the new line"
    );
}

/// Only a multi-line box is sized (`0x77d86a je` skips the write).
#[test]
fn a_single_line_box_keeps_its_height() {
    let mut s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetAutoFocus(false)
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetWidth(200); E:SetHeight(32)
        E:SetText("a\nb\nc")
    "#,
    )
    .unwrap();
    s.resolve();
    frame(&mut s);
    frame(&mut s);
    assert_eq!(height(&s, "E"), 32.0);
}

/// The multi-line text region sits `TOPLEFT` at `(left, −top)`, as wide as the box less its side
/// insets and as tall as its text, inside the box.
#[test]
fn the_text_region_is_pinned_top_left_as_wide_as_the_box_and_as_tall_as_its_text() {
    let mut s = script();
    inset_box(&mut s);
    s.run(r#"E:SetText("hello")"#).unwrap();
    frame(&mut s);
    frame(&mut s);
    let rect = s
        .extract()
        .into_iter()
        .find_map(|q| match (&q.content, q.rect) {
            (QuadContent::Text { text: Some(t), .. }, Some(r)) if t == "hello" => Some(r),
            _ => None,
        })
        .expect("the text draws");
    // The box is 5 + 14 tall from y 50: its top is 69.
    assert_eq!(
        (rect.left, rect.right, rect.top, rect.bottom),
        (115.0, 287.0, 67.0, 53.0)
    );
}

/// The height's move fires the box's `OnSizeChanged(width, height)`, and a flush that keeps it
/// fires nothing.
#[test]
fn on_size_changed_fires_when_the_text_moves_the_height() {
    let mut s = script();
    inset_box(&mut s);
    s.run(
        r#"
        fires = {}
        E:SetScript("OnSizeChanged", function() table.insert(fires, { w = arg1, h = arg2 }) end)
    "#,
    )
    .unwrap();
    frame(&mut s);
    let fires = |s: &UiScript| s.eval::<i64>("return table.getn(fires)").unwrap();
    let last = |s: &UiScript| {
        s.eval::<(f64, f64)>("local f = fires[table.getn(fires)] return f.w, f.h")
            .unwrap()
    };
    assert_eq!(fires(&s), 1, "the first flush replaces the authored height");
    assert_eq!(last(&s), (200.0, f64::from(5.0 + LH)));

    s.run(r#"E:SetText("a\nb\nc")"#).unwrap();
    frame(&mut s);
    assert_eq!(fires(&s), 2);
    assert_eq!(last(&s), (200.0, f64::from(5.0 + 3.0 * LH)));

    frame(&mut s);
    s.run(r#"E:SetText("x\ny\nz")"#).unwrap();
    frame(&mut s);
    assert_eq!(fires(&s), 2, "same line count, same height: no fire");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The stock mail shape (`MailFrame.xml`): `SendMailBodyEditBox`, 270×200 at `TOPLEFT (20, −10)`
/// in the 300×255 scroll child of the 296×257 `SendMailScrollFrame`, whose `OnTextChanged` is
/// `ScrollingEdit_OnTextChanged`. The box's height carries the text into the range union, so 30
/// lines give `max(255, 10 + 30·L) − 257`.
#[test]
fn a_scroll_frame_ranges_over_the_whole_text_of_a_multi_line_box() {
    let mut s = script();
    s.run(
        r#"
        local sf = CreateFrame("ScrollFrame", "MailScroll", UIParent)
        sf:SetPoint("TOPLEFT", 21, -97); sf:SetWidth(296); sf:SetHeight(257)
        local child = CreateFrame("Frame", "MailScrollChild", sf)
        child:SetWidth(300); child:SetHeight(255)
        child:SetPoint("TOPLEFT", 0, 0)
        sf:SetScrollChild(child)
        local body = CreateFrame("EditBox", "MailBody", child)
        body:SetMultiLine(true); body:SetAutoFocus(false)
        body:SetWidth(270); body:SetHeight(200)
        body:SetPoint("TOPLEFT", 20, -10)
        body:SetScript("OnTextChanged", function() ScrollingEdit_OnTextChanged(MailScroll) end)
        sf:SetScript("OnScrollRangeChanged", function() RANGE = arg2 end)
        function ScrollingEdit_OnTextChanged(scrollFrame) scrollFrame:UpdateScrollChildRect() end
    "#,
    )
    .unwrap();
    s.resolve();
    frame(&mut s);
    assert_eq!(
        s.eval::<f64>("return MailScroll:GetVerticalScrollRange()")
            .unwrap(),
        0.0,
        "an empty body is one line: the 255-tall child fits the 257-tall frame"
    );

    s.run(
        r#"
        local lines = {}
        for i = 1, 30 do lines[i] = "line " .. i end
        MailBody:SetText(table.concat(lines, "\n"))
    "#,
    )
    .unwrap();
    frame(&mut s);
    assert_eq!(
        height(&s, "MailBody"),
        f64::from(30.0 * LH),
        "the box itself spans the text, not only its text region"
    );
    let want = f64::from((10.0 + 30.0 * LH).max(255.0) - 257.0);
    assert_eq!(
        s.eval::<f64>("return RANGE").unwrap(),
        want,
        "the range ScrollingEdit_OnTextChanged's recompute reports"
    );
    assert_eq!(
        s.eval::<f64>("return MailScroll:GetVerticalScrollRange()")
            .unwrap(),
        want
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The text quad's rect as `(left, right, top, bottom)`, `None` when it draws nowhere.
fn text_rect(s: &UiScript, text: &str) -> Option<(f32, f32, f32, f32)> {
    s.extract().into_iter().find_map(|q| match &q.content {
        QuadContent::Text { text: Some(t), .. } if t == text => {
            q.rect.map(|r| (r.left, r.right, r.top, r.bottom))
        }
        _ => None,
    })
}

/// A box built in Lua, with no `<FontString>` and no `SetTextInsets`, has its text seated at its
/// first resolve: `ApplyRect` (`0x76b580`) sees it resized from the ctor's zero rect and the
/// override `0x77a8d0` runs `0x77b8c0` (`0x77a8e7`).
#[test]
fn a_lua_box_with_a_font_seats_its_text_at_its_first_resolve() {
    let mut s = script();
    s.run(
        r#"
        E = CreateFrame("EditBox", "E")
        E:SetAutoFocus(false)
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetWidth(200); E:SetHeight(32)
        E:SetFont("Fonts\\FRIZQT__.TTF", 14)
        E:SetText("hello")
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(text_rect(&s, "hello"), Some((100.0, 300.0, 82.0, 50.0)));
}

/// A later resize re-seats the text of any box: a multi-line box re-wraps at its new width, and a
/// single-line box's moved text region is put back between its insets.
#[test]
fn a_resized_box_re_seats_its_text() {
    let mut s = script();
    inset_box(&mut s);
    // 30 characters are 210 wide: two rows in 200 less the side insets, one row in 300 less them.
    s.run(r#"E:SetText(string.rep("x", 30))"#).unwrap();
    frame(&mut s);
    frame(&mut s);
    assert_eq!(height(&s, "E"), f64::from(5.0 + 2.0 * LH));
    s.run("E:SetWidth(300)").unwrap();
    frame(&mut s);
    frame(&mut s);
    assert_eq!(
        text_rect(&s, &"x".repeat(30)).map(|r| (r.0, r.1)),
        Some((115.0, 387.0)),
        "the text region takes the new width less the side insets"
    );
    assert_eq!(
        height(&s, "E"),
        f64::from(5.0 + LH),
        "and the text re-wraps in it"
    );

    s.run(
        r#"
        S = CreateFrame("EditBox", "S")
        S:SetAutoFocus(false)
        S:SetPoint("BOTTOMLEFT", 100, 200)
        S:SetWidth(200); S:SetHeight(32)
        S:SetText("single")
    "#,
    )
    .unwrap();
    frame(&mut s);
    s.run(
        r#"
        local text = S:GetRegions()
        text:ClearAllPoints(); text:SetPoint("TOPLEFT", S, "TOPLEFT", 40, 40)
    "#,
    )
    .unwrap();
    frame(&mut s);
    assert_ne!(
        text_rect(&s, "single").map(|r| r.0),
        Some(100.0),
        "the script moved the text region"
    );
    s.run("S:SetWidth(250)").unwrap();
    frame(&mut s);
    assert_eq!(text_rect(&s, "single"), Some((100.0, 350.0, 232.0, 200.0)));
}
