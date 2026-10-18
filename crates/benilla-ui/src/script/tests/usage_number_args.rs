//! The setters that gate a number argument with `lua_isnumber` (`0x6f34d0`) and raise their own
//! usage line on a miss (the object's name in `%s`), and `date`'s `luaL_optnumber` type error.

use super::common::script;
use crate::script::*;

fn err(s: &UiScript, code: &str) -> String {
    s.run(code).unwrap_err().to_string()
}

/// Every method setter, with a non-number in each gated slot, raises its usage line and names the
/// object. `SetFrameLevel` `0x774669`, `SetID` `0x77540b`, `SetHitRectInsets` `0x775f0f`,
/// `SetTextHeight` `0x79e4f9`, `SetAlphaGradient` `0x79cd9b`, `SetMaxLines` `0x793971`, MessageFrame
/// `SetTimeVisible` `0x79530c` and `SetFadeDuration` `0x7954bc` (the scrolling copies `0x793bdb`,
/// `0x793d8b`), EditBox `SetHistoryLines` `0x799521`, `SetBlinkSpeed` `0x7982ff`, `SetTextInsets`
/// `0x798c11`.
#[test]
fn method_setters_raise_their_usage_line_on_a_non_number() {
    let s = script();
    s.run(
        r#"
        NumF = CreateFrame("Frame", "NumF")
        NumFS = NumF:CreateFontString("NumFS")
        NumSMF = CreateFrame("ScrollingMessageFrame", "NumSMF")
        NumMF = CreateFrame("MessageFrame", "NumMF")
        NumEB = CreateFrame("EditBox", "NumEB")
        NumAnon = CreateFrame("Frame")
    "#,
    )
    .unwrap();
    for (call, line) in [
        ("NumF:SetFrameLevel(nil)", "NumF:SetFrameLevel(level)"),
        ("NumF:SetFrameLevel('x')", "NumF:SetFrameLevel(level)"),
        ("NumF:SetFrameLevel()", "NumF:SetFrameLevel(level)"),
        ("NumF:SetID({})", "NumF:SetID(ID)"),
        ("NumAnon:SetID(nil)", "<unnamed>:SetID(ID)"),
        (
            "NumF:SetHitRectInsets(1, 2, 3)",
            "NumF:SetHitRectInsets(left, right, top, bottom)",
        ),
        (
            "NumF:SetHitRectInsets('a', 2, 3, 4)",
            "NumF:SetHitRectInsets(left, right, top, bottom)",
        ),
        (
            "NumFS:SetTextHeight(nil)",
            "NumFS:SetTextHeight(pixelHeight)",
        ),
        (
            "NumFS:SetAlphaGradient(1)",
            "NumFS:SetAlphaGradient(start, length)",
        ),
        (
            "NumFS:SetAlphaGradient('x', 1)",
            "NumFS:SetAlphaGradient(start, length)",
        ),
        ("NumSMF:SetMaxLines(nil)", "NumSMF:SetMaxLines(maxLines)"),
        (
            "NumSMF:SetTimeVisible('x')",
            "NumSMF:SetTimeVisible(seconds)",
        ),
        (
            "NumSMF:SetFadeDuration({})",
            "NumSMF:SetFadeDuration(seconds)",
        ),
        ("NumMF:SetTimeVisible(nil)", "NumMF:SetTimeVisible(seconds)"),
        (
            "NumMF:SetFadeDuration('x')",
            "NumMF:SetFadeDuration(seconds)",
        ),
        (
            "NumEB:SetHistoryLines(nil)",
            "NumEB:SetHistoryLines(numLines)",
        ),
        ("NumEB:SetBlinkSpeed('x')", "NumEB:SetBlinkSpeed(speed)"),
        (
            "NumEB:SetTextInsets(1, 2, 3)",
            "NumEB:SetTextInsets(l, r, t, b)",
        ),
        (
            "NumEB:SetTextInsets(1, 2, 3, {})",
            "NumEB:SetTextInsets(l, r, t, b)",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(&format!("Usage: {line}")), "`{call}` -> {e}");
    }
    // A numeric string passes `lua_isnumber`, and the number is `_ftol`ed.
    s.run(
        r#"
        NumF:SetID("7"); assert(NumF:GetID() == 7)
        NumF:SetID(3.9); assert(NumF:GetID() == 3)
        NumF:SetFrameLevel("5"); assert(NumF:GetFrameLevel() == 5)
        NumEB:SetHistoryLines("9"); assert(NumEB:GetHistoryLines() == 9)
        NumEB:SetBlinkSpeed("0.25"); assert(NumEB:GetBlinkSpeed() == 0.25)
        NumEB:SetTextInsets("1", 2, 3, 4)
        NumF:SetHitRectInsets("1", 2, 3, 4)
    "#,
    )
    .unwrap();
}

/// `ProcessMapClick` (`0x4a7f89`) and `UpdateMapHighlight` (`0x4a85f6`) test both coordinates with
/// `lua_isnumber` and raise their usage, which names no object.
#[test]
fn the_map_globals_raise_their_usage_line_on_a_non_number() {
    let s = script();
    for (call, line) in [
        ("ProcessMapClick(nil, 1)", "Usage: ProcessMapClick(x, y)"),
        ("ProcessMapClick(1)", "Usage: ProcessMapClick(x, y)"),
        ("ProcessMapClick(1, 'x')", "Usage: ProcessMapClick(x, y)"),
        ("UpdateMapHighlight()", "Usage: UpdateMapHighlight(x, y)"),
        (
            "UpdateMapHighlight(1, {})",
            "Usage: UpdateMapHighlight(x, y)",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run("ProcessMapClick('0.5', 0.5) UpdateMapHighlight('0.5', 0.5)")
        .unwrap();
}

/// `date`'s second argument is `luaL_optnumber` (`0x6f4cd0`, at `0x7033d1`): nil or absent is the
/// default, a number or numeric string passes, anything else is `luaL_checknumber`'s
/// `luaL_typerror` (`0x6f4b80` -> `0x6f48a0`, format `%s expected, got %s` `0x871690`) through
/// `luaL_argerror` (`0x6f4810`, "bad argument #%d to `%s' (%s)" `0x871644`).
#[test]
fn date_raises_the_number_type_error_for_its_second_argument() {
    let s = script();
    for (call, want) in [
        (
            "date('%c', {})",
            "bad argument #2 to `date' (number expected, got table)",
        ),
        (
            "date('%c', true)",
            "bad argument #2 to `date' (number expected, got boolean)",
        ),
        (
            "date('%c', 'x')",
            "bad argument #2 to `date' (number expected, got string)",
        ),
        (
            "date('%c', date)",
            "bad argument #2 to `date' (number expected, got function)",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(want), "`{call}` -> {e}");
    }
    s.run("assert(date('%Y', nil) and date('%Y', '86400') == '1970' and date('%Y') ~= nil)")
        .unwrap();
}
