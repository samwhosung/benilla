//! `SetSpacing`/`GetSpacing`, the line gap every text-bearing table carries: one shared
//! implementation (`0x79fb40`, `0x79fbe0`) for the Font object, EditBox, the two message frames and
//! SimpleHTML, and the FontString's own pair (`0x79e2b0`, `0x79e1d0`), which clamps.

use super::common::script;
use crate::script::{QuadContent, UiScript};

/// The six receivers, each named so the usage line can name it, as `(global, kind)`.
const KINDS: &[(&str, &str)] = &[
    ("SFont", "Font"),
    ("SFS", "FontString"),
    ("SEB", "EditBox"),
    ("SMF", "MessageFrame"),
    ("SSMF", "ScrollingMessageFrame"),
    ("SHTML", "SimpleHTML"),
];

fn receivers() -> UiScript {
    let s = script();
    s.run(
        r#"
        CreateFont("SFont")
        CreateFrame("Frame", "SHost"):CreateFontString("SFS")
        CreateFrame("EditBox", "SEB")
        CreateFrame("MessageFrame", "SMF")
        CreateFrame("ScrollingMessageFrame", "SSMF")
        CreateFrame("SimpleHTML", "SHTML")
    "#,
    )
    .unwrap();
    s
}

fn spacing(s: &UiScript, obj: &str) -> f32 {
    s.eval::<f32>(&format!("return {obj}:GetSpacing()"))
        .unwrap_or_else(|e| panic!("{obj}:GetSpacing raised {e}"))
}

/// Every table carries the pair (`reference/bindings.tsv`, the `SetSpacing` rows), starting at
/// the constructors' 0 (`0x783a81`, `0x770d89`), and a number round-trips.
#[test]
fn every_text_table_round_trips_its_spacing() {
    let s = receivers();
    for &(obj, kind) in KINDS {
        assert_eq!(spacing(&s, obj), 0.0, "{kind}: the constructor's 0");
        s.run(&format!("{obj}:SetSpacing(4)"))
            .unwrap_or_else(|e| panic!("{kind}: SetSpacing(4) raised {e}"));
        assert_eq!(spacing(&s, obj), 4.0, "{kind}");
        // The gate is `lua_isnumber` (`0x79fb54`, `0x79e340`): a numeric string passes.
        s.run(&format!("{obj}:SetSpacing(\"2.5\")")).unwrap();
        assert_eq!(spacing(&s, obj), 2.5, "{kind}");
        // Nothing is returned (`0x79fbbe xor eax,eax`).
        assert_eq!(
            s.eval::<i64>(&format!(
                "return (function(...) return arg.n end)({obj}:SetSpacing(1))"
            ))
            .unwrap(),
            0,
            "{kind}"
        );
    }
}

/// Anything `lua_isnumber` refuses raises `Usage: %s:SetSpacing(spacing)` (`0x87c708`), naming the
/// receiver or `<unnamed>`.
#[test]
fn a_non_number_raises_the_usage_line() {
    let s = receivers();
    for &(obj, kind) in KINDS {
        let want = format!("Usage: {obj}:SetSpacing(spacing)");
        for bad in ["", "nil", "false", "\"wide\"", "{}"] {
            let err = s
                .run(&format!("{obj}:SetSpacing({bad})"))
                .expect_err("a non-number raises");
            assert!(
                err.to_string().contains(&want),
                "{kind}: SetSpacing({bad}) raised {err}, not {want:?}"
            );
        }
    }
    s.run("U = CreateFrame('Frame'):CreateFontString()")
        .unwrap();
    let err = s.run("U:SetSpacing()").unwrap_err();
    assert!(
        err.to_string()
            .contains("Usage: <unnamed>:SetSpacing(spacing)"),
        "{err}"
    );
}

/// Only the FontString clamps: its setter `0x772240` stores 0 for a negative and its getter reads
/// that field, where the shared setter stores the number as given (`0x79fba2`).
#[test]
fn only_the_fontstring_clamps_a_negative_spacing() {
    let s = receivers();
    for &(obj, kind) in KINDS {
        s.run(&format!("{obj}:SetSpacing(-3)")).unwrap();
        let want = if kind == "FontString" { 0.0 } else { -3.0 };
        assert_eq!(spacing(&s, obj), want, "{kind}");
    }
}

/// A FontString inherits its font object's spacing like the other font fields (the merge
/// `0x770800` at `0x7708fd`), live, until it sets its own, which clears inherit bit `0x10`
/// (`0x79e37b`).
#[test]
fn a_fontstring_inherits_spacing_from_its_font_object_until_it_sets_its_own() {
    let s = script();
    s.run(
        r#"
        CreateFont("GapFont")
        GapFont:SetSpacing(5)
        local f = CreateFrame("Frame")
        A = f:CreateFontString()
        B = f:CreateFontString()
        A:SetFontObject(GapFont)
        B:SetFontObject("GapFont")
        B:SetSpacing(2)
    "#,
    )
    .unwrap();
    assert_eq!(spacing(&s, "A"), 5.0);
    assert_eq!(spacing(&s, "B"), 2.0);
    s.run("GapFont:SetSpacing(6)").unwrap();
    assert_eq!(spacing(&s, "A"), 6.0, "the link is live");
    assert_eq!(
        spacing(&s, "B"),
        2.0,
        "a local set survives the font object's change"
    );
    s.run("GapFont:SetSpacing(-1)").unwrap();
    assert_eq!(
        spacing(&s, "A"),
        0.0,
        "inheriting runs through the clamping setter"
    );
}

/// `spacing=` is an attribute of `<Font>` (`0x783ee0`) and of `<FontString>` (`0x771257`); the
/// FontString's beats the one its font object passes down.
#[test]
fn the_spacing_attribute_reaches_fonts_and_fontstrings() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
             <Font name="XGapFont" font="Fonts\FRIZQT__.TTF" spacing="3">
               <FontHeight><AbsValue val="12"/></FontHeight>
             </Font>
             <Frame name="XGapHost">
               <Layers><Layer level="ARTWORK">
                 <FontString name="XInherit" inherits="XGapFont"/>
                 <FontString name="XOwn" inherits="XGapFont" spacing="7"/>
                 <FontString name="XPlain" spacing="2"/>
               </Layer></Layers>
             </Frame>
           </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(spacing(&s, "XGapFont"), 3.0);
    assert_eq!(spacing(&s, "XInherit"), 3.0);
    assert_eq!(spacing(&s, "XOwn"), 7.0);
    assert_eq!(spacing(&s, "XPlain"), 2.0);
}

/// SpellAlert's alert frame is a ScrollingMessageFrame whose `OnLoad` calls `SetSpacing` before the
/// rest of its setup; the call no longer aborts the handler.
#[test]
fn a_scrolling_message_frames_onload_runs_past_set_spacing() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
             <ScrollingMessageFrame name="AlertProbe">
               <Scripts><OnLoad>
                 this:SetSpacing(2)
                 this:SetMaxLines(3)
                 this:SetTimeVisible(5)
                 this:SetFadeDuration(1.5)
                 this:SetAlpha(0.5)
                 ALERT_PROBE_DONE = true
               </OnLoad></Scripts>
             </ScrollingMessageFrame>
           </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert!(s.eval::<bool>("return ALERT_PROBE_DONE == true").unwrap());
    assert_eq!(spacing(&s, "AlertProbe"), 2.0);
    assert_eq!(s.eval::<i64>("return AlertProbe:GetMaxLines()").unwrap(), 3);
    assert_eq!(
        s.eval::<f32>("return AlertProbe:GetFadeDuration()")
            .unwrap(),
        1.5
    );
}

/// The `(text, bottom, top)` of every message band on screen.
fn bands(s: &UiScript) -> Vec<(String, f32, f32)> {
    s.extract()
        .iter()
        .filter_map(|q| match (&q.content, q.rect) {
            (QuadContent::Text { text: Some(t), .. }, Some(r)) => {
                Some((t.clone(), r.bottom, r.top))
            }
            _ => None,
        })
        .collect()
}

/// A scrolling frame stacks each message's line string on the previous one's top `spacing` higher
/// (`0x788954`-`0x788975`), and a wrapped message is `rows·S + (rows−1)·gap` tall (`0x5c2070`), so
/// every row costs `S + gap` and the newest sits on the bottom edge.
#[test]
fn a_scrolling_frame_steps_its_messages_and_rows_by_the_spacing() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        "local f = CreateFrame('ScrollingMessageFrame', 'GapChat')\n\
         f:SetPoint('BOTTOMLEFT', 0, 90)\n\
         f:SetWidth(400)\n\
         f:SetHeight(100)\n\
         f:SetFont('Fonts\\\\FRIZQT__.TTF', 14)\n\
         f:SetSpacing(4)\n\
         f:AddMessage('old', 1, 1, 1)\n\
         f:AddMessage('wrapped', 1, 1, 1)\n\
         f:AddMessage('new', 1, 1, 1)",
    )
    .unwrap();
    s.resolve();
    let answers: Vec<(u32, u32, u16, u64)> = s
        .message_lines_needing_measure()
        .iter()
        .map(|r| (r.frame, r.index, if r.index == 1 { 2 } else { 1 }, r.key))
        .collect();
    s.set_message_line_rows(&answers);
    let got = bands(&s);
    // Farthest displayed line first, the scrolling class's emit order.
    let want = [
        ("old", 144.0, 158.0),
        ("wrapped", 108.0, 140.0),
        ("new", 90.0, 104.0),
    ];
    assert_eq!(got.len(), want.len(), "{got:?}");
    for ((t, b, top), (wt, wb, wtop)) in got.iter().zip(want) {
        assert_eq!(t, wt);
        assert!(
            (b - wb).abs() < 0.01 && (top - wtop).abs() < 0.01,
            "{t}: [{b}, {top}] against [{wb}, {wtop}]"
        );
    }
}

/// The plain class fits `h / (S + gap)` lines (`0x785f03`-`0x785f61`).
#[test]
fn a_plain_message_frame_fits_rows_at_the_spaced_pitch() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        "local f = CreateFrame('MessageFrame', 'GapErrors')\n\
         f:SetPoint('BOTTOMLEFT', 0, 0)\n\
         f:SetWidth(400)\n\
         f:SetHeight(42)\n\
         f:SetFading(false)\n\
         f:SetSpacing(4)",
    )
    .unwrap();
    for n in 0..9 {
        s.run(&format!("GapErrors:AddMessage('m{n}', 1, 1, 1)"))
            .unwrap();
    }
    s.resolve();
    s.tick(0.1);
    let kept = {
        let m = s.model_ref();
        let h = m.arena.lookup("GapErrors").expect("named frame");
        m.arena
            .frame(h)
            .and_then(|f| f.kind_state.message_lines())
            .map_or(0, |l| l.len())
    };
    assert_eq!(kept, 2, "42 / (14 + 4) holds two rows");
}

/// SimpleHTML's element font is a font instance: its getter answers what was set, negative
/// included, while the block step reads the block FontString's clamped field (`0x78aeaf`).
#[test]
fn simplehtml_keeps_a_negative_spacing_but_steps_its_blocks_by_zero() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("SimpleHTML", "GapPage")
        f:SetWidth(200)
        f:SetFont("Fonts\\FRIZQT__.TTF", 12)
        f:SetSpacing(-9)
        f:SetText("<HTML><BODY><P>one</P><P>two</P></BODY></HTML>")
    "#,
    )
    .unwrap();
    assert_eq!(spacing(&s, "GapPage"), -9.0);
    let m = s.model_ref();
    let fh = m.arena.lookup("GapPage").expect("frame");
    let blocks = &m.simple_html.get(&fh).expect("state").blocks;
    let second = m.region_data.get(&blocks[1]).expect("block 1");
    let y = second
        .anchors
        .get(crate::layout::Point::TopLeft)
        .expect("the TOPLEFT anchor")
        .y_off;
    assert_eq!(y, 0.0, "the step is the clamped spacing");
}

/// How many messages a plain frame keeps after a tick: its capacity.
fn plain_capacity(height: f32, spacing: &str) -> usize {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(&format!(
        "local f = CreateFrame('MessageFrame', 'CapFrame')\n\
         f:SetPoint('BOTTOMLEFT', 0, 0)\n\
         f:SetWidth(400)\n\
         f:SetHeight({height})\n\
         f:SetFading(false)\n\
         f:SetFont('Fonts\\\\FRIZQT__.TTF', 14)\n\
         f:SetSpacing({spacing})"
    ))
    .unwrap();
    for n in 0..12 {
        s.run(&format!("CapFrame:AddMessage('m{n}', 1, 1, 1)"))
            .unwrap();
    }
    s.resolve();
    s.tick(0.1);
    let m = s.model_ref();
    let h = m.arena.lookup("CapFrame").expect("named frame");
    m.arena
        .frame(h)
        .and_then(|f| f.kind_state.message_lines())
        .map_or(0, |l| l.len())
}

/// The plain class's pitch reads the font's spacing unclamped (`0x785f06`) and pixel-snaps both
/// terms (`0x766750`, `0x785f13`/`0x785f25`): a negative one truncates toward zero and shortens the
/// pitch, a fractional positive one rounds up, and a pitch of 0 or less fits nothing (`0x785f8f`).
#[test]
fn a_plain_message_frames_pitch_is_the_snapped_raw_spacing() {
    assert_eq!(plain_capacity(42.0, "-4"), 4, "42 / (14 - 4)");
    assert_eq!(plain_capacity(100.0, "2.5"), 5, "100 / (14 + 3)");
    assert_eq!(plain_capacity(42.0, "-2.5"), 3, "42 / (14 - 2)");
    // A zero pitch fits nothing, so no slot draws.
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        "local f = CreateFrame('MessageFrame', 'NoPitch')\n\
         f:SetPoint('BOTTOMLEFT', 0, 0)\n\
         f:SetWidth(400)\n\
         f:SetHeight(42)\n\
         f:SetFont('Fonts\\\\FRIZQT__.TTF', 14)\n\
         f:SetSpacing(-14)\n\
         f:AddMessage('m', 1, 1, 1)",
    )
    .unwrap();
    s.resolve();
    s.tick(0.1);
    assert!(bands(&s).is_empty(), "{:?}", bands(&s));
}

/// The plain class steps its slots by the same raw, snapped pitch (`0x786184`), so a negative
/// spacing overlaps them.
#[test]
fn a_plain_message_frames_slots_step_by_the_raw_pitch() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        "local f = CreateFrame('MessageFrame', 'SlotFrame')\n\
         f:SetPoint('BOTTOMLEFT', 0, 100)\n\
         f:SetWidth(400)\n\
         f:SetHeight(60)\n\
         f:SetInsertMode('BOTTOM')\n\
         f:SetFont('Fonts\\\\FRIZQT__.TTF', 14)\n\
         f:SetSpacing(-4)\n\
         f:AddMessage('one', 1, 1, 1)\n\
         f:AddMessage('two', 1, 1, 1)",
    )
    .unwrap();
    s.resolve();
    let mut got = bands(&s);
    got.sort_by(|a, b| a.1.total_cmp(&b.1));
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].0, "two");
    assert!((got[0].1 - 100.0).abs() < 0.01, "{got:?}");
    assert_eq!(got[1].0, "one");
    assert!(
        (got[1].1 - 110.0).abs() < 0.01,
        "one slot up is 10: {got:?}"
    );
}

/// A scrolling frame charges each message its height plus the line string's spacing rounded up to
/// a whole pixel (`0x78899e` → `0x766750`), and a wrapped message's inner gap likewise
/// (`0x5c2143`-`0x5c215c`).
#[test]
fn a_scrolling_frame_rounds_a_fractional_spacing_up() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        "local f = CreateFrame('ScrollingMessageFrame', 'GapChat2')\n\
         f:SetPoint('BOTTOMLEFT', 0, 90)\n\
         f:SetWidth(400)\n\
         f:SetHeight(100)\n\
         f:SetFont('Fonts\\\\FRIZQT__.TTF', 14)\n\
         f:SetSpacing(2.5)\n\
         f:AddMessage('old', 1, 1, 1)\n\
         f:AddMessage('wrapped', 1, 1, 1)\n\
         f:AddMessage('new', 1, 1, 1)",
    )
    .unwrap();
    s.resolve();
    let answers: Vec<(u32, u32, u16, u64)> = s
        .message_lines_needing_measure()
        .iter()
        .map(|r| (r.frame, r.index, if r.index == 1 { 2 } else { 1 }, r.key))
        .collect();
    s.set_message_line_rows(&answers);
    let got = bands(&s);
    // Farthest displayed line first, the scrolling class's emit order.
    let want = [
        ("old", 141.0, 155.0),
        ("wrapped", 107.0, 138.0),
        ("new", 90.0, 104.0),
    ];
    assert_eq!(got.len(), want.len(), "{got:?}");
    for ((t, b, top), (wt, wb, wtop)) in got.iter().zip(want) {
        assert_eq!(t, wt);
        assert!(
            (b - wb).abs() < 0.01 && (top - wtop).abs() < 0.01,
            "{t}: [{b}, {top}] against [{wb}, {wtop}]"
        );
    }
}

/// A scrolling frame at `scale`, spacing `spacing`, `height` tall, holding 'old', 'wrapped' (two
/// rows) and 'new', as `(text, bottom, top)` in draw order.
fn spaced_bands(scale: f32, spacing: f32, height: f32) -> Vec<(String, f32, f32)> {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(&format!(
        "local f = CreateFrame('ScrollingMessageFrame', 'GapChat3')\n\
         f:SetPoint('BOTTOMLEFT', 0, 90)\n\
         f:SetWidth(400)\n\
         f:SetHeight({height})\n\
         f:SetScale({scale})\n\
         f:SetFont('Fonts\\\\FRIZQT__.TTF', 14)\n\
         f:SetSpacing({spacing})\n\
         f:AddMessage('old', 1, 1, 1)\n\
         f:AddMessage('wrapped', 1, 1, 1)\n\
         f:AddMessage('new', 1, 1, 1)"
    ))
    .unwrap();
    s.resolve();
    let answers: Vec<(u32, u32, u16, u64)> = s
        .message_lines_needing_measure()
        .iter()
        .map(|r| (r.frame, r.index, if r.index == 1 { 2 } else { 1 }, r.key))
        .collect();
    s.set_message_line_rows(&answers);
    bands(&s)
}

/// The gap between messages is the unscaled spacing snapped, then scaled (`0x788954`-`0x788975`);
/// the gap between the rows of one message is the spacing times the scale snapped up to a whole
/// pixel (`0x5c2143`-`0x5c215c`). At 0.75 with spacing 1, that is 0.75 between messages and 1
/// between rows, where snapping the scaled value for both gives 1 and 1.
#[test]
fn a_scrolling_frames_gaps_scale_rather_than_snap_scaled() {
    let got = spaced_bands(0.75, 1.0, 100.0);
    assert_eq!(got.len(), 3, "{got:?}");
    let by = |t: &str| got.iter().find(|g| g.0 == t).expect(t).clone();
    let (old, wrapped, new) = (by("old"), by("wrapped"), by("new"));
    // 14 px type at 0.75 is 10.5 px a row: two rows and the snapped gap make 22.
    assert!((wrapped.2 - wrapped.1 - 22.0).abs() < 0.01, "{wrapped:?}");
    assert!(
        (wrapped.1 - new.1 - 11.25).abs() < 0.01,
        "{new:?} {wrapped:?}"
    );
    assert!(
        (old.1 - wrapped.1 - 22.75).abs() < 0.01,
        "{wrapped:?} {old:?}"
    );
}

/// The fit is per message: each takes its own block height and the gap after it from what is left
/// (`0x7889a3`-`0x7889be`). At 0.75 with spacing 1 a message costs 10.5 + 0.75 and a wrapped one
/// 21 + 1 + 0.75, 34 together. A count of equal rows of 11.25 would hold 'wrapped' in 33.9 px, and
/// per message it does not; in 34.2 px both do.
#[test]
fn a_scrolling_frames_fit_is_per_message() {
    let texts = |height_px: f32| -> Vec<String> {
        // The frame is at 0.75, so its set height is the screen height over the scale.
        spaced_bands(0.75, 1.0, height_px / 0.75)
            .into_iter()
            .map(|g| g.0)
            .collect()
    };
    assert_eq!(texts(33.9), ["new"]);
    assert_eq!(texts(34.2), ["wrapped", "new"]);
}

/// `spacing=` is read with `atof` (`0x64aaa0`): the numeric prefix, 0 for none.
#[test]
fn the_spacing_attribute_is_read_with_atof() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
             <Font name="AtofFont" font="Fonts\FRIZQT__.TTF" spacing=" 3px"/>
             <Frame name="AtofHost">
               <Layers><Layer level="ARTWORK">
                 <FontString name="AtofOwn" spacing="2.5em"/>
               </Layer></Layers>
             </Frame>
           </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(spacing(&s, "AtofFont"), 3.0);
    assert_eq!(spacing(&s, "AtofOwn"), 2.5);
}
