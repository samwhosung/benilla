//! Text measurement: the host round-trip, the same-tick font engine, and the layout they feed.

use super::common::script;
use crate::script::*;

/// A height-less FontString's [`MeasureRequest`] answer becomes its size, and a frame anchored to
/// it binds to the measured bottom in resolve's second round, as gossip option rows hang off the
/// greeting text (`GossipFrame.xml:258`).
#[test]
fn measured_fontstring_height_feeds_frame_anchors() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local w = CreateFrame("Frame", "Win")
        w:SetPoint("TOPLEFT", 0, -100); w:SetWidth(384); w:SetHeight(512)
        local g = w:CreateFontString("Greeting", "ARTWORK")
        g:SetText("a long greeting that wraps")
        g:SetWidth(270)
        g:SetPoint("TOPLEFT", 33, -91)
        local row = CreateFrame("Button", "Row1")
        row:SetWidth(300); row:SetHeight(16)
        row:SetPoint("TOPLEFT", "Greeting", "BOTTOMLEFT", -10, -20)
    "#,
    )
    .unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1, "one height-less FontString wants measuring");
    let r = &reqs[0];
    assert_eq!(r.wrap_width, Some(270.0));
    assert_eq!(r.text, "a long greeting that wraps");
    // Host answers: 3 wrapped lines of 16px ⇒ 48 tall.
    s.set_measured_text_unwrapped(&[(r.id, 250.0, 48.0, r.key)]);
    s.resolve();
    assert!(
        s.fontstrings_needing_measure().is_empty(),
        "cache key satisfied — no re-measure on a quiet frame"
    );
    let quads = s.extract();
    // Greeting: TOPLEFT of Win +(33,-91) ⇒ top 409 (win top 500), measured height 48 ⇒ bottom 361.
    let g = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } if t.starts_with("a long") => q.rect,
            _ => None,
        })
        .expect("greeting rect");
    assert_eq!((g.top, g.bottom, g.left), (409.0, 361.0, 33.0));
    // Row1: TOPLEFT → Greeting BOTTOMLEFT +(-10,-20) ⇒ top 341, left 23, bound in round 2.
    let row = quads
        .iter()
        .find_map(|q| match q.target {
            crate::order::ZTarget::Frame(_) => q.rect.filter(|r| (r.width() - 300.0).abs() < 0.1),
            _ => None,
        })
        .expect("row rect");
    assert_eq!((row.top, row.left), (341.0, 23.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A stored measure serves only its own text: reads check [`crate::script::RegionData`]'s measure
/// key, so a changed string reads 0 until its measure lands, never the old width. The chat header
/// reads its width right after a `SetText` (`ChatFrame.lua:1912`).
#[test]
fn a_changed_text_reads_zero_until_its_own_measure_lands() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local w = CreateFrame("Frame", "Win")
        w:SetPoint("TOPLEFT", 0, -100); w:SetWidth(384); w:SetHeight(512)
        local h = w:CreateFontString("Header", "ARTWORK")
        h:SetText("Say: ")
        h:SetPoint("LEFT", 13, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1);
    let r = reqs[0].clone();
    assert_eq!(r.text, "Say: ");
    s.set_measured_text_unwrapped(&[(r.id, 30.0, 16.0, r.key)]);
    assert_eq!(
        s.eval::<f64>(r#"return getglobal("Header"):GetWidth()"#)
            .unwrap(),
        30.0
    );
    // The type switch: same region, new text.
    s.run(r#"getglobal("Header"):SetText("Tell Alice: ")"#)
        .unwrap();
    assert_eq!(
        s.eval::<f64>(r#"return getglobal("Header"):GetWidth()"#)
            .unwrap(),
        0.0,
        "a stale measure must not serve for changed text"
    );
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1, "the changed text wants re-measuring");
    let r2 = reqs[0].clone();
    assert_eq!(r2.text, "Tell Alice: ");
    assert_ne!(r2.key, r.key, "the key tracks the text");
    s.set_measured_text_unwrapped(&[(r2.id, 72.0, 16.0, r2.key)]);
    assert_eq!(
        s.eval::<f64>(r#"return getglobal("Header"):GetWidth()"#)
            .unwrap(),
        72.0
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A zero-width FontString with a height auto-sizes its width to the measured line: MailFrame's
/// `OpenMailSenderLabel` (`MailFrame.xml:1391`) grows leftward from its anchor, and the sender
/// name anchored to its right edge starts past it.
#[test]
fn zero_width_fontstring_autosizes_to_its_line() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local w = CreateFrame("Frame", "Win")
        w:SetPoint("TOPLEFT", 0, -100); w:SetWidth(384); w:SetHeight(512)
        local label = w:CreateFontString("FromLabel", "ARTWORK")
        label:SetText("From:")
        label:SetWidth(0); label:SetHeight(16)
        label:SetPoint("TOPRIGHT", "Win", "TOPLEFT", 114, -45)
        local value = w:CreateFontString("FromValue", "ARTWORK")
        value:SetText("Thrall")
        value:SetWidth(110); value:SetHeight(0)
        value:SetPoint("LEFT", "FromLabel", "RIGHT", 5, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    let label_req = reqs
        .iter()
        .find(|r| r.text == "From:")
        .expect("the zero-width label asks for a measure");
    assert_eq!(
        label_req.wrap_width, None,
        "width 0 = unwrapped single line"
    );
    let value_req = reqs.iter().find(|r| r.text == "Thrall").expect("value");
    let answers = [
        (label_req.id, 40.0, 16.0, label_req.key),
        (value_req.id, 45.0, 16.0, value_req.key),
    ];
    s.set_measured_text_unwrapped(&answers);
    s.resolve();
    // Label: right edge pinned at Win left +114, measured width 40 ⇒ [74, 114].
    let (l_left, l_right, l_w): (f32, f32, f32) = s
        .eval("return FromLabel:GetLeft(), FromLabel:GetRight(), FromLabel:GetStringWidth()")
        .unwrap();
    assert_eq!((l_left, l_right, l_w), (74.0, 114.0, 40.0));
    // Value: 5 past the label's measured right edge.
    let v_left: f32 = s.eval("return FromValue:GetLeft()").unwrap();
    assert_eq!(v_left, 119.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A scaled frame's quads carry its effective scale for the glyph raster (layout already scaled
/// the rect), and the measure request and its cache key carry it too.
#[test]
fn frame_scale_rides_the_quads_and_the_measure_key() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local w = CreateFrame("Frame", "Win")
        w:SetPoint("TOPLEFT", 0, -100); w:SetWidth(400); w:SetHeight(300)
        w:SetScale(0.8)
        local t = w:CreateFontString("ScaledLabel", "ARTWORK")
        t:SetText("Options")
        t:SetPoint("TOPLEFT", 10, -10)
    "#,
    )
    .unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(r.scale, 0.8, "request names the drawn-size scale");
    let old_key = r.key;
    let (id, key) = (r.id, r.key);
    s.set_measured_text_unwrapped(&[(id, 50.0, 16.0, key)]);
    s.resolve();
    assert!(s.fontstrings_needing_measure().is_empty(), "cache warm");
    let quads = s.extract();
    let label = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Options"))
        .expect("label quad");
    assert_eq!(label.scale, 0.8);
    // Layout already scaled the rect (50 × 0.8 = 40); the quad's scale is for the glyphs only.
    let lr = label.rect.expect("label rect");
    assert!((lr.width() - 40.0).abs() < 0.01, "width {}", lr.width());
    s.run(r#"Win:SetScale(1.25)"#).unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1, "SetScale re-measures");
    assert_eq!(reqs[0].scale, 1.25);
    assert_ne!(reqs[0].key, old_key, "key carries the scale");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// [`UiScript::invalidate_text_measures`] re-requests every warm measure under the same key: the
/// key hashes content, not the host's raster scale, so only the host knows when a measure is
/// stale, and a stale one truncates text that fits.
#[test]
fn invalidate_text_measures_reopens_the_round_trip() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local w = CreateFrame("Frame", "Win")
        w:SetPoint("TOPLEFT", 0, -100); w:SetWidth(400); w:SetHeight(300)
        local t = w:CreateFontString("Label", "ARTWORK")
        t:SetText("Keybindings")
        t:SetPoint("TOPLEFT", 10, -10)
    "#,
    )
    .unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1);
    let (id, key) = (reqs[0].id, reqs[0].key);
    s.set_measured_text_unwrapped(&[(id, 74.0, 12.0, key)]);
    s.resolve();
    assert!(s.fontstrings_needing_measure().is_empty(), "cache warm");
    assert_eq!(
        s.eval::<f64>("return Label:GetStringWidth()").unwrap(),
        74.0
    );

    s.invalidate_text_measures();
    assert_eq!(
        s.eval::<f64>("return Label:GetStringWidth()").unwrap(),
        0.0,
        "stale measure dropped, not served"
    );
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1, "re-requests after invalidation");
    assert_eq!(
        reqs[0].key, key,
        "content key unchanged — only the answer was stale"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `GetStringWidth` is the natural width, unwrapped and untruncated at the drawn size: the
/// reference re-measures the raw text with no wrap (`0x79e510`, `0x772890`). `GetWidth` stays the
/// laid-out box.
#[test]
fn get_string_width_is_the_natural_width_not_the_box() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Win = CreateFrame("Frame")
        Win:SetWidth(400) Win:SetHeight(300)
        Win:SetPoint("TOPLEFT", 0, 0)
        Label = Win:CreateFontString("Label")
        Label:SetText("a string that is wider than its box")
        Label:SetWidth(60) Label:SetHeight(13)
        Label:SetPoint("TOPLEFT", 10, -10)
    "#,
    )
    .unwrap();
    s.resolve();

    // 1 · A fully declared box still requests a measure, for the natural width.
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1, "a fully-sized FontString still measures");
    let r = &reqs[0];
    assert_eq!(r.wrap_width, Some(60.0), "the request carries the box");

    // The host answers with both: laid out inside 60, natural 200.
    s.set_measured_text(&[(r.id, 60.0, 39.0, 200.0, r.key)]);
    s.resolve();

    // 2 · The two getters answer different questions.
    assert_eq!(
        s.eval::<f64>("return Label:GetStringWidth()").unwrap(),
        200.0,
        "GetStringWidth is the natural, unwrapped extent"
    );
    assert_eq!(
        s.eval::<f64>("return Label:GetWidth()").unwrap(),
        60.0,
        "GetWidth is the laid-out box the auto-size path depends on"
    );

    // 3 · A narrower box re-measures (the key carries the wrap); the natural width stays put.
    s.run("Label:SetWidth(30)").unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1, "a new box is a new measure key");
    s.set_measured_text(&[(reqs[0].id, 30.0, 91.0, 200.0, reqs[0].key)]);
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return Label:GetStringWidth()").unwrap(),
        200.0,
        "the string did not change, so neither did its natural width"
    );
}

// ── The synchronous measure: a host font engine installed in the VM (`TextMeasure`) ──

/// A stand-in font engine: each character is `PER_CHAR` wide and each line `LINE_H` tall, wrapped
/// greedily at the request's wrap width.
struct BlockFont;

const PER_CHAR: f32 = 7.0;
const LINE_H: f32 = 14.0;

impl TextMeasure for BlockFont {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        let natural = req.text.chars().count() as f32 * PER_CHAR;
        match req.wrap_width {
            Some(w) if w > 0.0 && natural > w => {
                let rows = (natural / w).ceil();
                (w, rows * LINE_H, natural)
            }
            _ => (natural, LINE_H, natural),
        }
    }
}

/// With a font engine installed, `SetText` then a width read in the same tick gets the string's
/// width, as the money frame's `SetText` then `GetTextWidth` expects (`MoneyFrame.lua:202`).
#[test]
fn a_width_read_in_the_tick_that_set_the_text_is_not_zero() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_text_measurer(Box::new(BlockFont));
    s.run(
        r#"
        local f = CreateFrame("Frame", "Host")
        f:SetPoint("TOPLEFT", 0, 0); f:SetWidth(400); f:SetHeight(300)
        local fs = f:CreateFontString("Label", "ARTWORK")
        fs:SetPoint("TOPLEFT", 0, 0)
        -- the corpus idiom: set, then measure, in one statement sequence
        fs:SetText("Onewarrior")
        Answer = fs:GetStringWidth()
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<f32>("return Answer").unwrap(),
        "Onewarrior".len() as f32 * PER_CHAR,
        "GetStringWidth must answer inside the tick that set the text"
    );
}

/// Without an engine the read is 0 until the round-trip answers; headless runs and most tests have
/// no engine, so nothing may depend on the synchronous path.
#[test]
fn with_no_engine_installed_the_round_trip_is_still_the_only_answer() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Host")
        f:SetPoint("TOPLEFT", 0, 0); f:SetWidth(400); f:SetHeight(300)
        local fs = f:CreateFontString("Label", "ARTWORK")
        fs:SetPoint("TOPLEFT", 0, 0)
        fs:SetText("Onewarrior")
        Answer = fs:GetStringWidth()
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<f32>("return Answer").unwrap(), 0.0);
    assert_eq!(
        s.fontstrings_needing_measure().len(),
        1,
        "and the request is still queued for the host"
    );
}

/// With an engine installed, `resolve` answers its own measure requests, so `GetWidth()` is right
/// in the frame the text was set.
#[test]
fn resolve_closes_the_round_trip_when_an_engine_is_installed() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_text_measurer(Box::new(BlockFont));
    s.run(
        r#"
        local f = CreateFrame("Frame", "Host")
        f:SetPoint("TOPLEFT", 0, 0); f:SetWidth(400); f:SetHeight(300)
        local fs = f:CreateFontString("Label", "ARTWORK")
        fs:SetPoint("TOPLEFT", 0, 0)
        fs:SetWidth(35)                 -- a declared width ⇒ the two extents differ
        fs:SetText("Onewarrior")        -- 70 natural, wraps to 2 rows inside 35
    "#,
    )
    .unwrap();
    s.resolve();
    assert!(
        s.fontstrings_needing_measure().is_empty(),
        "the engine answered its own request during resolve"
    );
    assert_eq!(
        s.eval::<Vec<f32>>(
            "return { Label:GetWidth(), Label:GetHeight(), Label:GetStringWidth() }"
        )
        .unwrap(),
        vec![35.0, LINE_H * 2.0, 70.0],
        "laid-out extent from the box, natural width unwrapped — the two must not be conflated"
    );
}

/// The same-tick measure and the batch round-trip share one cache, so an unchanged string is
/// measured once however often it is read.
#[test]
fn a_synchronous_measure_satisfies_the_batch_request_too() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_text_measurer(Box::new(BlockFont));
    s.run(
        r#"
        local f = CreateFrame("Frame", "Host")
        f:SetPoint("TOPLEFT", 0, 0); f:SetWidth(400); f:SetHeight(300)
        local fs = f:CreateFontString("Label", "ARTWORK")
        fs:SetPoint("TOPLEFT", 0, 0)
        fs:SetText("hello")
        Answer = fs:GetStringWidth()
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<f32>("return Answer").unwrap(), 5.0 * PER_CHAR);
    assert!(
        s.fontstrings_needing_measure().is_empty(),
        "the Lua read already filled the cache the batch pass keys on"
    );
    s.run("Label:SetText('a longer label')").unwrap();
    assert_eq!(
        s.eval::<f32>("return Label:GetStringWidth()").unwrap(),
        "a longer label".len() as f32 * PER_CHAR,
        "a stale measure is not this string's metric"
    );
}

/// `fontstrings_needing_measure` sweeps only the regions a write enrolled, so every measure-input
/// write must enroll its region: settle, write once, and the sweep must name that region.
#[test]
fn a_settled_region_is_refound_after_each_measure_input_write() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Host")
        f:SetPoint("TOPLEFT", 0, 0); f:SetWidth(400); f:SetHeight(300)
        local fs = f:CreateFontString("Label", "ARTWORK")
        fs:SetPoint("TOPLEFT", 0, 0)
        fs:SetText("one")
    "#,
    )
    .unwrap();
    let writes: &[(&str, &str)] = &[
        ("SetText", "Label:SetText('two')"),
        (
            "SetFont",
            r#"Label:SetFont("Fonts\\FRIZQT__.TTF", 14, "OUTLINE")"#,
        ),
        ("SetTextHeight", "Label:SetTextHeight(20)"),
        ("SetWidth (the wrap width)", "Label:SetWidth(123)"),
        // A scale write cannot name its descendants, so it falls back to the whole-roster walk.
        ("SetScale (conservative)", "Host:SetScale(2.0)"),
    ];
    for (label, lua) in writes {
        // Settle: answer whatever is pending, then prove the sweep is dry.
        loop {
            let reqs = s.fontstrings_needing_measure();
            if reqs.is_empty() {
                break;
            }
            let answers: Vec<_> = reqs.iter().map(|r| (r.id, 10.0, 16.0, r.key)).collect();
            s.set_measured_text_unwrapped(&answers);
        }
        assert!(s.fontstrings_needing_measure().is_empty(), "settled");
        s.run(lua).unwrap();
        let reqs = s.fontstrings_needing_measure();
        assert_eq!(
            reqs.len(),
            1,
            "{label}: a silent measure-input write must re-surface its region to the sweep"
        );
    }
    // `Button:SetText` writes its ButtonText region through its own binding.
    s.run(
        r#"
        local b = CreateFrame("Button", "Btn")
        b:SetPoint("TOPLEFT", 0, -50); b:SetWidth(80); b:SetHeight(22)
    "#,
    )
    .unwrap();
    while !{
        let reqs = s.fontstrings_needing_measure();
        let answers: Vec<_> = reqs.iter().map(|r| (r.id, 10.0, 16.0, r.key)).collect();
        s.set_measured_text_unwrapped(&answers);
        reqs.is_empty()
    } {}
    s.run("Btn:SetText('Go')").unwrap();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(
        reqs.len(),
        1,
        "Button:SetText: the label region must be re-surfaced to the sweep"
    );
    assert_eq!(reqs[0].text, "Go");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
