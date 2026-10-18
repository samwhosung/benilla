//! The `%s` of a method's usage line is the object's own name, its vtable `+4` (`GetName`), or
//! `<unnamed>` (`0x84c7f0`): never the widget type, and never a Lua `GetName` an addon overrides.

use super::common::script;
use crate::script::*;

fn err(s: &UiScript, code: &str) -> String {
    s.run(code).unwrap_err().to_string()
}

/// The name is read off the object, so a Lua `GetName` the addon replaced does not reach it
/// (FontString `SetSpacing` `0x79e38e`, Model `SetModel` `0x76da21`: `call [vt+0x4]`).
#[test]
fn a_usage_line_names_the_object_not_its_lua_get_name() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "UsageOwner")
        UsageFS = f:CreateFontString("UsageFS")
        UsageFS.GetName = function() return "Fake" end
        UsageModel = CreateFrame("Model", "UsageModel")
        UsageModel.GetName = function() return "Fake" end
    "#,
    )
    .unwrap();
    let e = err(&s, "UsageFS:SetSpacing(nil)");
    assert!(e.contains("Usage: UsageFS:SetSpacing(spacing)"), "{e}");
    let e = err(&s, "UsageModel:SetModel()");
    assert!(e.contains(r#"Usage: UsageModel:SetModel("file")"#), "{e}");
}

/// The six `SetJustifyH`/`SetJustifyV` bindings raise `Usage: %s:SetJustifyH("justify")`
/// (`0x87c77c`, `0x87c7a0`) for a non-string or a token that does not parse, naming the object:
/// FontString `0x79e7bb`, Font `0x7a0bf9`, ScrollingMessageFrame `0x79268f`, MessageFrame
/// `0x794c5f`, SimpleHTML `0x7967e7`, EditBox `0x797a1f`.
#[test]
fn set_justify_usage_lines_name_the_object() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "JustOwner")
        JustFS = f:CreateFontString("JustFS")
        JustAnon = f:CreateFontString()
        JustFont = CreateFont("JustFont")
        JustSMF = CreateFrame("ScrollingMessageFrame", "JustSMF")
        JustMF = CreateFrame("MessageFrame", "JustMF")
        JustHTML = CreateFrame("SimpleHTML", "JustHTML")
        JustBox = CreateFrame("EditBox", "JustBox")
        JustAnonBox = CreateFrame("EditBox")
    "#,
    )
    .unwrap();
    for (call, line) in [
        (
            "JustFS:SetJustifyH('X')",
            r#"Usage: JustFS:SetJustifyH("justify")"#,
        ),
        (
            "JustFS:SetJustifyH(nil)",
            r#"Usage: JustFS:SetJustifyH("justify")"#,
        ),
        (
            "JustFS:SetJustifyV({})",
            r#"Usage: JustFS:SetJustifyV("justify")"#,
        ),
        (
            "JustAnon:SetJustifyH('X')",
            r#"Usage: <unnamed>:SetJustifyH("justify")"#,
        ),
        (
            "JustFont:SetJustifyH('X')",
            r#"Usage: JustFont:SetJustifyH("justify")"#,
        ),
        (
            "JustFont:SetJustifyV(nil)",
            r#"Usage: JustFont:SetJustifyV("justify")"#,
        ),
        (
            "JustSMF:SetJustifyH('X')",
            r#"Usage: JustSMF:SetJustifyH("justify")"#,
        ),
        (
            "JustMF:SetJustifyV('X')",
            r#"Usage: JustMF:SetJustifyV("justify")"#,
        ),
        (
            "JustHTML:SetJustifyH('X')",
            r#"Usage: JustHTML:SetJustifyH("justify")"#,
        ),
        (
            "JustBox:SetJustifyH('X')",
            r#"Usage: JustBox:SetJustifyH("justify")"#,
        ),
        (
            "JustBox:SetJustifyV(nil)",
            r#"Usage: JustBox:SetJustifyV("justify")"#,
        ),
        (
            "JustAnonBox:SetJustifyH('X')",
            r#"Usage: <unnamed>:SetJustifyH("justify")"#,
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    // A token still parses on every class.
    s.run(
        r#"
        JustFS:SetJustifyH("LEFT"); assert(JustFS:GetJustifyH() == "LEFT")
        JustFont:SetJustifyV("TOP"); assert(JustFont:GetJustifyV() == "TOP")
        JustBox:SetJustifyH("right"); assert(JustBox:GetJustifyH() == "RIGHT")
    "#,
    )
    .unwrap();
}

/// `SetMaxBytes` and `SetMaxLetters` raise on an argument count other than one (`lua_gettop`
/// against 2, `0x798fbc`, `0x79919c`) with `Usage: %s:SetMaxBytes(max)` (`0x87bf14`) and
/// `Usage: %s:SetMaxLetters(max)` (`0x87bf30`), naming the box (`0x798fc6`, `0x7991a6`).
#[test]
fn edit_box_max_setters_name_the_box() {
    let s = script();
    s.run(
        r#"
        MaxBox = CreateFrame("EditBox", "MaxBox")
        MaxAnon = CreateFrame("EditBox")
    "#,
    )
    .unwrap();
    for (call, line) in [
        ("MaxBox:SetMaxLetters()", "Usage: MaxBox:SetMaxLetters(max)"),
        ("MaxBox:SetMaxBytes(1, 2)", "Usage: MaxBox:SetMaxBytes(max)"),
        (
            "MaxAnon:SetMaxLetters()",
            "Usage: <unnamed>:SetMaxLetters(max)",
        ),
        ("MaxAnon:SetMaxBytes()", "Usage: <unnamed>:SetMaxBytes(max)"),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
}

/// `SetVerticalScroll` (`0x791400`) and `SetHorizontalScroll` (`0x7912f0`) gate on `lua_isnumber`
/// (`0x791490`, `0x791380`), so a numeric string scrolls, and anything else raises
/// `Usage: %s:SetVerticalScroll(offset)` (`0x87b59c`) or the horizontal line (`0x87b574`).
#[test]
fn scroll_setters_raise_their_usage_lines() {
    let s = script();
    s.run(r#"ScrollUse = CreateFrame("ScrollFrame", "ScrollUse")"#)
        .unwrap();
    for (call, line) in [
        (
            "ScrollUse:SetVerticalScroll(nil)",
            "Usage: ScrollUse:SetVerticalScroll(offset)",
        ),
        (
            "ScrollUse:SetVerticalScroll({})",
            "Usage: ScrollUse:SetVerticalScroll(offset)",
        ),
        (
            "ScrollUse:SetVerticalScroll('x')",
            "Usage: ScrollUse:SetVerticalScroll(offset)",
        ),
        (
            "ScrollUse:SetHorizontalScroll()",
            "Usage: ScrollUse:SetHorizontalScroll(offset)",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(
        r#"
        ScrollUse:SetVerticalScroll("12")
        assert(ScrollUse:GetVerticalScroll() == 12)
        ScrollUse:SetHorizontalScroll("3")
        assert(ScrollUse:GetHorizontalScroll() == 3)
    "#,
    )
    .unwrap();
}

/// `SetHyperlinkFormat` (`0x796b40`) stores any `lua_isstring` value (`0x796bcc`), so a number
/// stores its text, and raises `Usage: %s:SetHyperlinkFormat("format")` (`0x87bb40`) naming the
/// frame (`0x796bf3`) for anything else.
#[test]
fn set_hyperlink_format_takes_a_number_and_names_the_frame() {
    let s = script();
    s.run(
        r#"
        LinkHTML = CreateFrame("SimpleHTML", "LinkHTML")
        LinkAnon = CreateFrame("SimpleHTML")
        LinkHTML:SetHyperlinkFormat(5)
        assert(LinkHTML:GetHyperlinkFormat() == "5", LinkHTML:GetHyperlinkFormat())
    "#,
    )
    .unwrap();
    for (call, line) in [
        (
            "LinkHTML:SetHyperlinkFormat({})",
            r#"Usage: LinkHTML:SetHyperlinkFormat("format")"#,
        ),
        (
            "LinkAnon:SetHyperlinkFormat()",
            r#"Usage: <unnamed>:SetHyperlinkFormat("format")"#,
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(r#"assert(LinkHTML:GetHyperlinkFormat() == "5")"#)
        .unwrap();
}

/// `SetInsertMode` (`0x794ed0`) takes a `lua_isstring` value compared whole, without case and
/// untrimmed, with "TOP" then "BOTTOM" (`0x794f7e`, `0x794fa0`); anything else raises
/// `Usage: %s:SetInsertMode("TOP" or "BOTTOM")` (`0x87ba50`) and leaves the mode.
#[test]
fn set_insert_mode_raises_on_anything_but_top_or_bottom() {
    let s = script();
    s.run(r#"InsertMF = CreateFrame("MessageFrame", "InsertMF")"#)
        .unwrap();
    for call in [
        r#"InsertMF:SetInsertMode(" TOP")"#,
        r#"InsertMF:SetInsertMode("BOTOM")"#,
        "InsertMF:SetInsertMode(1)",
        "InsertMF:SetInsertMode()",
    ] {
        let e = err(&s, call);
        assert!(
            e.contains(r#"Usage: InsertMF:SetInsertMode("TOP" or "BOTTOM")"#),
            "`{call}` -> {e}"
        );
    }
    s.run(
        r#"
        assert(InsertMF:GetInsertMode() == "BOTTOM", "a raise keeps the mode")
        InsertMF:SetInsertMode("top"); assert(InsertMF:GetInsertMode() == "TOP")
        InsertMF:SetInsertMode("bottom"); assert(InsertMF:GetInsertMode() == "BOTTOM")
    "#,
    )
    .unwrap();
    let e = err(&s, r#"CreateFrame("MessageFrame"):SetInsertMode("x")"#);
    assert!(e.contains("Usage: <unnamed>:SetInsertMode("), "{e}");
}

/// The XML `insertMode` never raises: a non-empty value is BOTTOM when it equals "BOTTOM" without
/// case, TOP otherwise (`0x785a46`-`0x785a68`).
#[test]
fn xml_insert_mode_is_bottom_or_top_without_a_raise() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
            <MessageFrame name="XmlMF1" insertMode="bottom"/>
            <MessageFrame name="XmlMF2" insertMode="SIDEWAYS"/>
            <MessageFrame name="XmlMF3" insertMode=" BOTTOM"/>
        </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_: &str| None);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    s.run(
        r#"
        assert(XmlMF1:GetInsertMode() == "BOTTOM")
        assert(XmlMF2:GetInsertMode() == "TOP")
        assert(XmlMF3:GetInsertMode() == "TOP")
    "#,
    )
    .unwrap();
}

/// The not-movable family names the frame or `<unnamed>`: `Frame %s is not movable` (`0x879810`,
/// `StartMoving` `0x77679b`-`0x7767cc`), `… resizable` (`0x879828`) and `… movable or resizable`
/// (`0x879844`, `SetUserPlaced` `0x776ae5`).
#[test]
fn not_movable_errors_name_an_unnamed_frame_unnamed() {
    let s = script();
    for (call, line) in [
        (
            r#"CreateFrame("Frame"):StartMoving()"#,
            "Frame <unnamed> is not movable",
        ),
        (
            r#"CreateFrame("Frame"):SetUserPlaced(1)"#,
            "Frame <unnamed> is not movable or resizable",
        ),
        (
            r#"CreateFrame("Frame", "StuckFrame"):StartMoving()"#,
            "Frame StuckFrame is not movable",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
}

/// `CreateFrame` (`0x7060b0`) gates in order: the type `lua_isstring` (`0x7060d5`) and the
/// parent's tag none, nil or table (`0x7060e9`-`0x706112`), both raising the usage line
/// (`0x872f60`); a table parent with no object in `[0]` (`0x87303c`) or one that is no frame
/// (`0x873004`); a template miss (`0x7061ed`); then the unknown type (`0x7062a0`, `0x872fa8`).
#[test]
fn create_frame_raises_in_the_reference_order() {
    const USAGE: &str = r#"Usage: CreateFrame("frameType" [, "name"] [, parent] [, "template"])"#;
    let s = script();
    s.run(r#"ParentHost = CreateFrame("Frame", "ParentHost")"#)
        .unwrap();
    for (call, line) in [
        (r#"CreateFrame("Frame", "ByName", "ParentHost")"#, USAGE),
        (r#"CreateFrame("Frame", nil, 5)"#, USAGE),
        (r#"CreateFrame("Frame", nil, true)"#, USAGE),
        ("CreateFrame(nil)", USAGE),
        ("CreateFrame({})", USAGE),
        ("CreateFrame()", USAGE),
        (
            r#"CreateFrame("Frame", "NoThis", {})"#,
            "CreateFrame: Couldn't find 'this' in parent object",
        ),
        (
            r#"CreateFrame("Frame", "OnTexture", ParentHost:CreateTexture())"#,
            "CreateFrame: Wrong parent object type, expected frame",
        ),
        (
            r#"CreateFrame("Frame", "OnFont", CreateFont("ParentFont"))"#,
            "CreateFrame: Wrong parent object type, expected frame",
        ),
        (
            r#"CreateFrame("Bogus", nil, nil, "NoSuchTemplate")"#,
            r#"CreateFrame(): Couldn't find inherited node "NoSuchTemplate""#,
        ),
        (
            r#"CreateFrame("Bogus")"#,
            "CreateFrame: Unknown frame type 'Bogus'",
        ),
        ("CreateFrame(5)", "CreateFrame: Unknown frame type '5'"),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(
        r#"
        assert(ByName == nil and NoThis == nil and OnTexture == nil and OnFont == nil)
        local child = CreateFrame("Frame", "UnderHost", ParentHost)
        assert(child:GetParent() == ParentHost)
    "#,
    )
    .unwrap();
}
