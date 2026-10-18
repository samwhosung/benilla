//! The setters whose 1.12.1 body rejects a number by range after `lua_isnumber`, and the one that
//! stores it as it is.

use super::common::script;
use crate::script::*;

fn err(s: &UiScript, code: &str) -> String {
    s.run(code).unwrap_err().to_string()
}

fn fixture() -> UiScript {
    let s = script();
    s.run(
        r#"
        RgF = CreateFrame("Frame", "RgF")
        RgFS = RgF:CreateFontString("RgFS")
        RgAnonFS = RgF:CreateFontString()
        RgSMF = CreateFrame("ScrollingMessageFrame", "RgSMF")
        RgEB = CreateFrame("EditBox", "RgEB")
    "#,
    )
    .unwrap();
    s
}

/// `SetTextHeight` (`0x79e3c0`) compares the float with 0.0 (`0x79e469`) and goes on only for a
/// greater or unordered value; else it raises `%s:SetTextHeight(): invalid text height: %f`
/// (`0x87c750`, `0x79e49f`), the `%f` a Lua number printed as `%.14g`.
#[test]
fn set_text_height_raises_for_a_height_not_above_zero() {
    let s = fixture();
    for (call, want) in [
        (
            "RgFS:SetTextHeight(0)",
            "RgFS:SetTextHeight(): invalid text height: 0",
        ),
        (
            "RgFS:SetTextHeight(-3)",
            "RgFS:SetTextHeight(): invalid text height: -3",
        ),
        (
            "RgFS:SetTextHeight('-0.5')",
            "RgFS:SetTextHeight(): invalid text height: -0.5",
        ),
        (
            "RgAnonFS:SetTextHeight(0)",
            "<unnamed>:SetTextHeight(): invalid text height: 0",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(want), "`{call}` -> {e}");
    }
    // Greater, and unordered (NaN), go on.
    s.run("RgFS:SetTextHeight(12) RgFS:SetTextHeight(0.5) RgFS:SetTextHeight(0/0)")
        .unwrap();
    // The compare is on the double (`0x79e466`): above 0 passes even where the float copy is 0.
    s.run("RgFS:SetTextHeight(1e-46)").unwrap();
    assert!(err(&s, "RgFS:SetTextHeight(-1e-46)").contains("invalid text height:"));
}

/// `SetFrameLevel` (`0x774560`) `_ftol`s the number and raises on a negative one (`jl`,
/// `0x77460c`): `%s:SetFrameLevel(): Passed negative frame level: %d` (`0x879628`, `0x77463b`).
#[test]
fn set_frame_level_raises_for_a_negative_level() {
    let s = fixture();
    let e = err(&s, "RgF:SetFrameLevel(-1)");
    assert!(
        e.contains("RgF:SetFrameLevel(): Passed negative frame level: -1"),
        "{e}"
    );
    let e = err(&s, "RgF:SetFrameLevel('-7.9')");
    assert!(
        e.contains("RgF:SetFrameLevel(): Passed negative frame level: -7"),
        "{e}"
    );
    // -0.9 truncates to 0, which is not negative.
    s.run("RgF:SetFrameLevel(-0.9) assert(RgF:GetFrameLevel() == 0) RgF:SetFrameLevel(4)")
        .unwrap();
}

/// `SetMaxLines` (`0x7938a0`) and `SetHistoryLines` (`0x799450`) take a count above 0 after
/// `_ftol` (`jle`, `0x793949`, `0x7994f9`); anything else is their usage line.
#[test]
fn max_lines_and_history_lines_raise_their_usage_line_for_a_count_not_above_zero() {
    let s = fixture();
    for (call, line) in [
        ("RgSMF:SetMaxLines(0)", "RgSMF:SetMaxLines(maxLines)"),
        ("RgSMF:SetMaxLines(-5)", "RgSMF:SetMaxLines(maxLines)"),
        ("RgSMF:SetMaxLines(0.9)", "RgSMF:SetMaxLines(maxLines)"),
        ("RgEB:SetHistoryLines(0)", "RgEB:SetHistoryLines(numLines)"),
        ("RgEB:SetHistoryLines(-1)", "RgEB:SetHistoryLines(numLines)"),
        (
            "RgEB:SetHistoryLines('0.5')",
            "RgEB:SetHistoryLines(numLines)",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(&format!("Usage: {line}")), "`{call}` -> {e}");
    }
    s.run(
        r#"
        RgSMF:SetMaxLines(1) assert(RgSMF:GetMaxLines() == 1)
        RgSMF:SetMaxLines(1.9) assert(RgSMF:GetMaxLines() == 1)
        RgEB:SetHistoryLines(7) assert(RgEB:GetHistoryLines() == 7)
    "#,
    )
    .unwrap();
}

/// The plain MessageFrame's `SetTimeVisible` (`0x785c20`) and `SetFadeDuration` (`0x785c80`) store
/// the number as it is (`0x785c65`, `0x785cc5`), and every line whose own time is not 0 takes it.
#[test]
fn message_frame_time_setters_store_unclamped() {
    let s = script();
    s.run("MFq = CreateFrame('MessageFrame', 'MFq')").unwrap();
    s.run(
        r#"
        MFq:SetTimeVisible(-2) assert(MFq:GetTimeVisible() == -2)
        MFq:SetFadeDuration(-1.5) assert(MFq:GetFadeDuration() == -1.5)
    "#,
    )
    .unwrap();
}

#[test]
fn message_frame_time_setters_retime_the_lines_already_shown() {
    let mut s = script();
    s.run("CreateFrame('MessageFrame', 'MFr') MFr:AddMessage('x', 1, 1, 1)")
        .unwrap();
    // The line holds its 10 s and 3 s from the ctor; the setters replace both.
    s.run("MFr:SetTimeVisible(0.5) MFr:SetFadeDuration(0.2)")
        .unwrap();
    s.tick(0.6);
    s.tick(0.3);
    let m = s.model_ref();
    let h = m.arena.lookup("MFr").expect("named frame");
    let lines = m
        .arena
        .frame(h)
        .and_then(|f| f.kind_state.message_lines())
        .map_or(0, |l| l.len());
    assert_eq!(lines, 0, "the line took the new times and finished");
}

/// The XML `historyLines` reaches the ring's setter (`0x77a175`-`0x77a18d`, `0x77cd90`) with 0 or
/// more, where the Lua binding refuses a count not above 0: stock `MailFrame.xml` and
/// `TradeFrame.xml` declare `historyLines="0"`.
#[test]
fn xml_history_lines_zero_loads_where_the_lua_setter_refuses() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui><EditBox name="XmlHist0" historyLines="0"/><EditBox name="XmlHist4" historyLines="4"/></Ui>"#,
    )
    .expect("parses");
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    s.run(
        r#"
        assert(XmlHist0:GetHistoryLines() == 0 and XmlHist4:GetHistoryLines() == 4)
    "#,
    )
    .unwrap();
}

/// A line spends its time to exactly 0 (`0x786323`) and then its fade (`0x78633c`); the setters
/// skip a line whose time is 0 (`0x785c3d`), so a line already fading is not retimed.
#[test]
fn a_line_already_fading_is_untouched_by_set_time_visible() {
    let mut s = script();
    s.run("CreateFrame('MessageFrame', 'MFf') MFf:AddMessage('x', 1, 1, 1)")
        .unwrap();
    let alive = |s: &UiScript| {
        let m = s.model_ref();
        let h = m.arena.lookup("MFf").expect("named frame");
        m.arena
            .frame(h)
            .and_then(|f| f.kind_state.message_lines())
            .map_or(0, |l| l.len())
    };
    s.run("MFf:SetTimeVisible(0.5)").unwrap();
    s.tick(0.6); // the time runs out: stored as exactly 0
    s.tick(1.0); // 3 s of fade, 2 left
    assert_eq!(alive(&s), 1);
    s.run("MFf:SetTimeVisible(100)").unwrap();
    s.tick(2.5); // the fade runs out
    assert_eq!(alive(&s), 0, "the fading line kept its own clock");
}
