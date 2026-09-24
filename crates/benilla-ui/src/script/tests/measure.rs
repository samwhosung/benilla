//! The measure round-trip + frame→region anchors, end to end.

use super::common::script;
use crate::script::*;

/// The measure round-trip + frame→region anchors, end to end: a height-less FontString reports a
/// [`MeasureRequest`]; the host answer becomes its implicit size; and a FRAME anchored to that
/// FontString by name binds to its measured bottom in resolve's second round — the real gossip
/// structure (option rows hang off the greeting's laid-out height, ref-GossipFrame.xml l.258-261).
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
    // Row1: TOPLEFT → Greeting BOTTOMLEFT +(-10,-20) ⇒ top 341, left 23 — bound in round 2.
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

/// A stored measure serves ONLY the current text: after `SetText` to a DIFFERENT string, the old
/// string's width must not leak through `GetWidth`/`GetStringWidth` — the whisper-header cursor
/// bug: the chat edit box ran `SetTextInsets(15 + header:GetWidth(), …)` on a type switch
/// (Say → "Tell Alice:") and its `w > 1` settle gate passed with the PREVIOUS header's measure,
/// latching the caret inside the new header. The metric read is key-checked
/// ([`crate::script::RegionData`]'s measure key): a changed string reads 0 until its own measure
/// lands, so poll-until-nonzero callers converge on the RIGHT width.
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
    // The type switch: same region, new text. The old measure must NOT serve.
    s.run(r#"getglobal("Header"):SetText("Tell Alice: ")"#)
        .unwrap();
    assert_eq!(
        s.eval::<f64>(r#"return getglobal("Header"):GetWidth()"#)
            .unwrap(),
        0.0,
        "a stale measure must not serve for changed text"
    );
    // The round-trip re-measures the new string; the true width serves.
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

/// A zero-WIDTH FontString with an explicit height auto-sizes its width to the measured line —
/// the reference label idiom (`<Size x="0" y="16"/>` anchored TOPRIGHT→TOPLEFT: MailFrame's
/// "From:"/"Subject:" labels end at the anchor and grow leftward, and the value string anchored
/// LEFT→label RIGHT starts past them, never overlapping). Gating the measure on height alone
/// left these rects zero-width — "From" and the sender name painted on top of each other.
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
    // Value: starts 5 past the label's real right edge — no overlap.
    let v_left: f32 = s.eval("return FromValue:GetLeft()").unwrap();
    assert_eq!(v_left, 119.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The frame-scale seam (0219 §2's divergence, closed): a `SetScale`'d frame's quads carry its
/// `effective_scale` out to the renderer — the rect is already scale-multiplied by layout, and
/// the renderer needs the same factor for the GLYPH raster size — and the measure round-trip
/// rides it too: the request names the scale (the host measures at the drawn size), the cache
/// key holds it (a re-scale re-measures), all before any quad draws stale text.
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
    // The measure request carries the owner's effective scale.
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(r.scale, 0.8, "request names the drawn-size scale");
    let old_key = r.key;
    let (id, key) = (r.id, r.key);
    s.set_measured_text_unwrapped(&[(id, 50.0, 16.0, key)]);
    s.resolve();
    assert!(s.fontstrings_needing_measure().is_empty(), "cache warm");
    // Every quad of the scaled frame carries the scale — frame slot and region alike.
    let quads = s.extract();
    let label = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Options"))
        .expect("label quad");
    assert_eq!(label.scale, 0.8);
    // The label's rect is scale-multiplied by layout (width 50 × 0.8 = 40) — the quad's scale is
    // for the glyph raster, not a second rect multiply.
    let lr = label.rect.expect("label rect");
    assert!((lr.width() - 40.0).abs() < 0.01, "width {}", lr.width());
    // A re-scale invalidates the measure key: the same text re-measures at the new drawn size.
    s.run(r#"Win:SetScale(1.25)"#).unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1, "SetScale re-measures");
    assert_eq!(reqs[0].scale, 1.25);
    assert_ne!(reqs[0].key, old_key, "key carries the scale");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The host's raster-environment invalidation ([`UiScript::invalidate_text_measures`]): a warm
/// FontString measure cache re-requests after the call — same content, same key (the key hashes
/// content, not the host's seam scale; the STALENESS is the host's to declare, which is the whole
/// seam). This is the engine half of the fullscreen-truncation fix: measures answered under one
/// seam scale kept satisfying fit tests run under another, and the ellipsis ate fitting text
/// ("Contr...", director 2026-08-04; reproduced end-to-end by `WOW_RESIZE`).
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
    // The measured extent is gone (GetStringWidth back to its unmeasured 0)…
    assert_eq!(
        s.eval::<f64>("return Label:GetStringWidth()").unwrap(),
        0.0,
        "stale measure dropped, not served"
    );
    // …and the round-trip reopens with the SAME content key — the request is the host's cue to
    // re-measure under its new environment; nothing about the region itself changed.
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1, "re-requests after invalidation");
    assert_eq!(
        reqs[0].key, key,
        "content key unchanged — only the answer was stale"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// **`GetStringWidth` is the NATURAL width — never the box, never the wrapped extent**
/// (`0x79e510`→`0x772890`: the reference re-measures the raw text with no wrap constraint, so "Lua
/// sees the natural, unwrapped, un-truncated width at the DRAWN size").
///
/// This is the distinction whose absence made the reference's own `PanelTemplates_TabResize` a
/// feedback loop in this engine (decision 0997): the kit sized a tab from `GetStringWidth`, set that
/// width on the label, and read its own output back next frame — a tab that changed width every
/// single frame. Three separate things are pinned here because each one was wrong on its own:
///
/// 1. a region with a DECLARED width still gets a measure request (it used to be skipped: no
///    auto-sized axis, no request — so a constrained label could never learn its natural width);
/// 2. `GetStringWidth` answers with the natural width, while `GetWidth` keeps echoing the laid-out
///    extent that auto-size depends on;
/// 3. the answer does not move when the declared width does.
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

    // 1 · Both axes are declared, so nothing about the LAYOUT needs a measure — and the request is
    //     issued anyway, because `GetStringWidth` has no other way to learn the natural width.
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

    // 3 · Narrowing the box re-opens the round trip (the key carries the wrap) and, once answered,
    //     leaves the natural width exactly where it was. This is the loop that used to close.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The SYNCHRONOUS measure — a host font engine installed into the VM ([`TextMeasure`])
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A stand-in font engine: every character is `PER_CHAR` wide and every line `LINE_H` tall, wrapped
/// greedily at the request's wrap width. Deliberately not the real one — what these tests are about
/// is *when* the answer arrives, and a fake with arithmetic anyone can do in their head is what
/// makes the assertions readable. The real engine's own numbers are the app's to test.
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

/// **The director's bug, at the engine.** `SetText` then `GetStringWidth` in ONE tick must answer
/// with the string's width — the shape `Bagnon_Forever/database/ui.lua:58-59` writes over every
/// saved character (`button:SetText(player)` then `button:GetTextWidth() + 40`), and the shape the
/// reference's own `SmallMoneyFrame` writes (MoneyFrame.lua l.202).
///
/// Without an engine installed this is 0 until the host's round-trip lands at extract — a frame
/// later. That is what sized Bagnon's character dropdown to 40px with seven names hanging out of it.
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

/// The same read WITHOUT an engine stays 0 and stays served by the round-trip — the pre-existing
/// behaviour, still exactly itself. Every engine test and every headless run is this VM, so the
/// synchronous path may never be a correctness precondition for anything.
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

/// A FontString's **box** is right in the frame its text was set, not the frame after: `resolve`
/// closes the round-trip itself when an engine is installed, so a caller reading `GetWidth()` (the
/// laid-out extent, not the natural one) gets this string's number and not the previous string's.
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

/// The same-tick measure and the batch round-trip write the **same cache**, so asking twice costs
/// one measure — and a caller polling `GetStringWidth` every frame does not re-measure a string
/// that has not changed.
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
    // …and a new string invalidates it exactly as it always did.
    s.run("Label:SetText('a longer label')").unwrap();
    assert_eq!(
        s.eval::<f32>("return Label:GetStringWidth()").unwrap(),
        "a longer label".len() as f32 * PER_CHAR,
        "a stale measure is not this string's metric"
    );
}

/// The measure LEDGER's completeness guard: `fontstrings_needing_measure` no longer walks the
/// roster on the armed path — a measure-input write the ledger never hears about is a label
/// whose box silently never updates again. The exposing sequence is SETTLE (drain the ledger
/// dry), then one silent write, then sweep: enrolled, the sweep names exactly that region;
/// missed, it returns empty and this test is the red light. (Watched red with the `SetText`
/// enrollment removed — the earlier tests in this file stay green through that removal, because
/// their regions ride the ledger's own re-enrollment of still-unanswered requests.)
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
        // `SetFormattedText` was a second row here; it is gone (2142), and its era spelling
        // `SetText(format(...))` is the row above's path exactly.
        (
            "SetFont",
            r#"Label:SetFont("Fonts\\FRIZQT__.TTF", 14, "OUTLINE")"#,
        ),
        ("SetTextHeight", "Label:SetTextHeight(20)"),
        ("SetWidth (the wrap width)", "Label:SetWidth(123)"),
        // The conservative lane: a scale write cannot name descendants one by one, so it must
        // put the ledger back on the whole-roster walk — which refinds Label (scale is in the
        // measure key).
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
    // The button label lane writes through its own binding (`Button:SetText` routes to the
    // ButtonText region) — same guard, second write path.
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
