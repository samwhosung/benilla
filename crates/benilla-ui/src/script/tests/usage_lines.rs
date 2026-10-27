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

/// EditBox `SetText` and `SetNumber` gate with `lua_isstring` (`0x79854c`, `0x79871c`) and raise
/// `Usage: %s:SetText("text")` (`0x87a198`) or `Usage: %s:SetNumber(number)` (`0x87beb0`) naming
/// the box; `Insert` skips a non-string (`0x798494`); `AddHistoryLine` takes exactly one argument
/// (`0x7989ec`) and raises `Usage: %s:AddHistoryLine("text")` (`0x87becc`).
#[test]
fn edit_box_text_setters_gate_their_argument() {
    let s = script();
    s.run(
        r#"
        TextBox = CreateFrame("EditBox", "TextBox")
        TextAnon = CreateFrame("EditBox")
    "#,
    )
    .unwrap();
    for (call, line) in [
        ("TextBox:SetText(nil)", r#"Usage: TextBox:SetText("text")"#),
        ("TextBox:SetText()", r#"Usage: TextBox:SetText("text")"#),
        ("TextBox:SetText(true)", r#"Usage: TextBox:SetText("text")"#),
        (
            "TextAnon:SetText({})",
            r#"Usage: <unnamed>:SetText("text")"#,
        ),
        ("TextBox:SetNumber(nil)", "Usage: TextBox:SetNumber(number)"),
        (
            "TextAnon:SetNumber(false)",
            "Usage: <unnamed>:SetNumber(number)",
        ),
        (
            "TextBox:AddHistoryLine()",
            r#"Usage: TextBox:AddHistoryLine("text")"#,
        ),
        (
            "TextAnon:AddHistoryLine('a', 'b')",
            r#"Usage: <unnamed>:AddHistoryLine("text")"#,
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(
        r#"
        TextBox:SetText(12) assert(TextBox:GetText() == "12")
        TextBox:SetNumber(1e20) assert(TextBox:GetText() == "1e+020")
        TextBox:SetText("ab") TextBox:Insert(true) TextBox:Insert({}) TextBox:Insert(nil)
        assert(TextBox:GetText() == "ab")
        TextBox:Insert(5) assert(TextBox:GetText() == "ab5")
        TextBox:AddHistoryLine("one")
    "#,
    )
    .unwrap();
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
        -- The offsets read back through the law at 4:3, `G(S(12))` and `G(S(3))`.
        ScrollUse:SetVerticalScroll("12")
        assert(ScrollUse:GetVerticalScroll() == 12.00000029802322)
        ScrollUse:SetHorizontalScroll("3")
        assert(ScrollUse:GetHorizontalScroll() == 3.000000074505805)
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
            <MessageFrame name="XmlMF1" insertMode="top"/>
            <MessageFrame name="XmlMF2" insertMode="SIDEWAYS"/>
            <MessageFrame name="XmlMF3" insertMode=" BOTTOM"/>
        </Ui>"#,
    )
    .expect("valid FrameXML");
    let report = crate::loader::load(&s, &doc, &|_: &str| None);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    s.run(
        r#"
        assert(XmlMF1:GetInsertMode() == "TOP", "off the ctor's BOTTOM")
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
            r#"CreateFrame("Frame", nil, {}, "NoSuchTemplate")"#,
            "CreateFrame: Couldn't find 'this' in parent object",
        ),
        (
            r#"CreateFrame("Bogus", nil, nil, "NoSuchTemplate")"#,
            r#"CreateFrame(): Couldn't find inherited node "NoSuchTemplate""#,
        ),
        (
            r#"CreateFrame("Frame", "EmptyTemplate", nil, "")"#,
            r#"CreateFrame(): Couldn't find inherited node """#,
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
        assert(EmptyTemplate == nil)
        local child = CreateFrame("Frame", "UnderHost", ParentHost)
        assert(child:GetParent() == ParentHost)
    "#,
    )
    .unwrap();
}

/// The StatusBar and Slider value setters gate on `lua_isnumber`, a numeric string passing, and
/// raise `Usage: %s:SetMinMaxValues(min, max)` (`0x87b160`), `Usage: %s:SetValue(value)`
/// (`0x87b184`) and the Slider's `Usage: %s:SetValueStep(value)` (`0x87b39c`), naming the frame.
#[test]
fn bar_and_slider_value_setters_raise_their_usage_lines() {
    let s = script();
    s.run(
        r#"
        UseSB = CreateFrame("StatusBar", "UseSB")
        UseSL = CreateFrame("Slider", "UseSL")
    "#,
    )
    .unwrap();
    for (call, line) in [
        (
            "UseSB:SetMinMaxValues(0)",
            "Usage: UseSB:SetMinMaxValues(min, max)",
        ),
        (
            "UseSB:SetMinMaxValues('x', 1)",
            "Usage: UseSB:SetMinMaxValues(min, max)",
        ),
        ("UseSB:SetValue(nil)", "Usage: UseSB:SetValue(value)"),
        (
            "UseSL:SetMinMaxValues({}, 1)",
            "Usage: UseSL:SetMinMaxValues(min, max)",
        ),
        ("UseSL:SetValue()", "Usage: UseSL:SetValue(value)"),
        (
            "UseSL:SetValueStep(true)",
            "Usage: UseSL:SetValueStep(value)",
        ),
        (
            r#"CreateFrame("StatusBar"):SetValue()"#,
            "Usage: <unnamed>:SetValue(value)",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(
        r#"
        UseSB:SetMinMaxValues("0", "10"); UseSB:SetValue("4")
        assert(UseSB:GetValue() == 4)
        UseSL:SetMinMaxValues(0, 10); UseSL:SetValueStep("2")
        assert(UseSL:GetValueStep() == 2)
    "#,
    )
    .unwrap();
}

/// `SetOrientation` on both (`0x78f290`, `0x790580`): a non-string raises
/// `Usage: %s:SetOrientation("orientation")` (`0x87b108`), a token the parse `0x6f1a20` misses
/// `%s:SetOrientation(): Unknown orientation: %s` (`0x87b130`).
#[test]
fn set_orientation_raises_the_reference_lines() {
    let s = script();
    s.run(
        r#"
        OriSB = CreateFrame("StatusBar", "OriSB")
        OriSL = CreateFrame("Slider", "OriSL")
    "#,
    )
    .unwrap();
    for (call, line) in [
        (
            "OriSB:SetOrientation('DIAGONAL')",
            "OriSB:SetOrientation(): Unknown orientation: DIAGONAL",
        ),
        (
            "OriSB:SetOrientation(nil)",
            r#"Usage: OriSB:SetOrientation("orientation")"#,
        ),
        (
            "OriSL:SetOrientation(5)",
            "OriSL:SetOrientation(): Unknown orientation: 5",
        ),
        (
            "OriSL:SetOrientation({})",
            r#"Usage: OriSL:SetOrientation("orientation")"#,
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(
        r#"
        OriSB:SetOrientation("vertical"); assert(OriSB:GetOrientation() == "VERTICAL")
        OriSL:SetOrientation("horizontal"); assert(OriSL:GetOrientation() == "HORIZONTAL")
    "#,
    )
    .unwrap();
}

/// `CreateFont` (`0x706040`) gates its name on `lua_isstring` (`0x706048`), so a number names the
/// object by its `%.14g` text, and raises `Usage: CreateFont("name")` (`0x872f40`) otherwise.
#[test]
fn create_font_raises_its_usage_line() {
    let s = script();
    for call in [
        "CreateFont()",
        "CreateFont(nil)",
        "CreateFont({})",
        "CreateFont(true)",
    ] {
        let e = err(&s, call);
        assert!(
            e.contains(r#"Usage: CreateFont("name")"#),
            "`{call}` -> {e}"
        );
    }
    s.run(
        r#"
        local f = CreateFont(12)
        assert(f:GetName() == "12" and getglobal("12") == f)
    "#,
    )
    .unwrap();
}

/// The Font's `IsObjectType` (`0x79fe60`) tests `lua_isstring` (`0x79feec`) and raises
/// `Usage: %s:IsObjectType("TYPE")` (`0x8794e4`) naming the font.
#[test]
fn font_is_object_type_raises_its_usage_line() {
    let s = script();
    s.run(r#"TypeFont = CreateFont("TypeFont")"#).unwrap();
    for call in ["TypeFont:IsObjectType()", "TypeFont:IsObjectType(true)"] {
        let e = err(&s, call);
        assert!(
            e.contains(r#"Usage: TypeFont:IsObjectType("TYPE")"#),
            "`{call}` -> {e}"
        );
    }
    s.run(r#"assert(TypeFont:IsObjectType("font") == 1 and TypeFont:IsObjectType(5) == nil)"#)
        .unwrap();
}

/// `IsFrameType` (`0x773700`) raises IsObjectType's line for a value `lua_isstring` refuses
/// (`0x77378c`, `Usage: %s:IsObjectType("TYPE")` `0x8794e4` pushed at `0x7737af`).
#[test]
fn is_frame_type_raises_is_object_type_line() {
    let s = script();
    s.run(r#"TypeFrame = CreateFrame("Frame", "TypeFrame")"#)
        .unwrap();
    for call in ["TypeFrame:IsFrameType()", "TypeFrame:IsFrameType({})"] {
        let e = err(&s, call);
        assert!(
            e.contains(r#"Usage: TypeFrame:IsObjectType("TYPE")"#),
            "`{call}` -> {e}"
        );
    }
    let e = err(&s, r#"CreateFrame("Frame"):IsFrameType(nil)"#);
    assert!(
        e.contains(r#"Usage: <unnamed>:IsObjectType("TYPE")"#),
        "{e}"
    );
}

/// The shared `SetShadowOffset` (`0x79f9c0`) behind the Font's and the SimpleHTML's bindings needs
/// both coordinates to pass `lua_isnumber` (`0x79f9d7`, `0x79f9eb`), a numeric string included,
/// else it raises `Usage: %s:SetShadowOffset(x, y)` (`0x87c6e8`) naming the receiver.
#[test]
fn set_shadow_offset_raises_its_usage_line_on_a_font_and_a_simplehtml() {
    let s = script();
    crate::test_support::install_law43(&s);
    s.run(r#"ShadowFont = CreateFont("ShadowFont"); ShadowHTML = CreateFrame("SimpleHTML", "ShadowHTML")"#)
        .unwrap();
    for (call, line) in [
        (
            "ShadowFont:SetShadowOffset(1)",
            "Usage: ShadowFont:SetShadowOffset(x, y)",
        ),
        (
            "ShadowHTML:SetShadowOffset({}, 1)",
            "Usage: ShadowHTML:SetShadowOffset(x, y)",
        ),
        (
            "ShadowHTML:SetShadowOffset('H1', 1)",
            "Usage: ShadowHTML:SetShadowOffset(x, y)",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(
        r#"
        ShadowFont:SetShadowOffset("2", -1); local x, y = ShadowFont:GetShadowOffset()
        assert(x == law(2) and y == law(-1))
        ShadowHTML:SetShadowOffset("H2", 3, "-4"); x, y = ShadowHTML:GetShadowOffset("H2")
        assert(x == law(3) and y == law(-4))
    "#,
    )
    .unwrap();
}

/// FontString's own `SetShadowOffset` (`0x79e070`, gates `0x79e102`, `0x79e117`) and the EditBox
/// and message-frame shims into `0x79f9c0` raise `Usage: %s:SetShadowOffset(x, y)` (`0x87c6e8`)
/// for a coordinate `lua_isnumber` refuses, naming the receiver.
#[test]
fn set_shadow_offset_raises_its_usage_line_on_font_blocks() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "ShadowOwner")
        ShadowFS = f:CreateFontString("ShadowFS")
        ShadowAnonFS = f:CreateFontString()
        ShadowBox = CreateFrame("EditBox", "ShadowBox")
        ShadowMF = CreateFrame("MessageFrame", "ShadowMF")
        ShadowSMF = CreateFrame("ScrollingMessageFrame", "ShadowSMF")
    "#,
    )
    .unwrap();
    for (call, line) in [
        (
            "ShadowFS:SetShadowOffset(nil, 1)",
            "Usage: ShadowFS:SetShadowOffset(x, y)",
        ),
        (
            "ShadowAnonFS:SetShadowOffset(1)",
            "Usage: <unnamed>:SetShadowOffset(x, y)",
        ),
        (
            "ShadowBox:SetShadowOffset(1, true)",
            "Usage: ShadowBox:SetShadowOffset(x, y)",
        ),
        (
            "ShadowMF:SetShadowOffset({}, 1)",
            "Usage: ShadowMF:SetShadowOffset(x, y)",
        ),
        (
            "ShadowSMF:SetShadowOffset('x', 1)",
            "Usage: ShadowSMF:SetShadowOffset(x, y)",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(r#"ShadowFS:SetShadowOffset("2", -1)"#).unwrap();
}

/// `SetButtonState` raises `Usage: %s:SetButtonState("state", lock)` (`0x87a0d8`) for a non-string
/// or an unknown token; `SetWidth`/`SetHeight` (`0x7a1fb0`, `0x7a21e0`) raise their lines
/// (`0x87cbec`, `0x87cc08`) on frames and regions; `SetLight` names the pane and closes its
/// brackets (`0x878c00`).
#[test]
fn button_size_and_light_setters_raise_the_reference_lines() {
    let s = script();
    s.run(
        r#"
        StateBtn = CreateFrame("Button", "StateBtn")
        SizeFrame = CreateFrame("Frame", "SizeFrame")
        SizeTex = SizeFrame:CreateTexture("SizeTex")
        SizeFS = SizeFrame:CreateFontString()
        LightM = CreateFrame("Model", "LightM")
    "#,
    )
    .unwrap();
    let light = "Usage: LightM:SetLight(enabled[, omni, dirX, dirY, dirZ, ambIntensity[, \
                 ambR, ambG, ambB], dirIntensity[, dirR, dirG, dirB]])";
    for (call, line) in [
        (
            "StateBtn:SetButtonState(nil)",
            r#"Usage: StateBtn:SetButtonState("state", lock)"#,
        ),
        (
            "StateBtn:SetButtonState('HELD')",
            r#"Usage: StateBtn:SetButtonState("state", lock)"#,
        ),
        (
            "SizeFrame:SetWidth(nil)",
            "Usage: SizeFrame:SetWidth(width)",
        ),
        (
            "SizeFrame:SetHeight({})",
            "Usage: SizeFrame:SetHeight(height)",
        ),
        ("SizeTex:SetWidth('x')", "Usage: SizeTex:SetWidth(width)"),
        ("SizeFS:SetHeight()", "Usage: <unnamed>:SetHeight(height)"),
        ("LightM:SetLight(1, 'x')", light),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(
        r#"
        StateBtn:SetButtonState("pushed") assert(StateBtn:GetButtonState() == "PUSHED")
        SizeFrame:SetWidth("12") SizeTex:SetHeight("3")
    "#,
    )
    .unwrap();
}

/// `SetParent` (`0x7a1550`) on a frame or a region: a table with no object raises
/// `%s:SetParent(): Couldn't find 'this' in parent object` (`0x87cbb4`), a non-frame object
/// `…Wrong parent object type, expected frame` (`0x87cb78`), and a number is a name (`0x7a1601`).
#[test]
fn set_parent_raises_the_reference_lines_for_a_bad_table() {
    let s = script();
    s.run(
        r#"
        PFrame = CreateFrame("Frame", "PFrame")
        PTex = PFrame:CreateTexture("PTex")
        CreateFrame("Frame", "5")
    "#,
    )
    .unwrap();
    for (call, line) in [
        (
            "PFrame:SetParent({})",
            "PFrame:SetParent(): Couldn't find 'this' in parent object",
        ),
        (
            "PFrame:SetParent(PTex)",
            "PFrame:SetParent(): Wrong parent object type, expected frame",
        ),
        (
            "PTex:SetParent({})",
            "PTex:SetParent(): Couldn't find 'this' in parent object",
        ),
        (
            "PTex:SetParent(PTex)",
            "PTex:SetParent(): Wrong parent object type, expected frame",
        ),
        (
            "PTex:SetParent('NoSuchFrame')",
            "PTex:SetParent(): Couldn't find region named 'NoSuchFrame'",
        ),
        (
            "PTex:SetParent()",
            "PTex:SetParent(): Couldn't find region named '(null)'",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run(
        r#"
        PFrame:SetParent(5) assert(PFrame:GetParent() == getglobal("5"))
        PTex:SetParent(5) assert(PTex:GetParent() == getglobal("5"))
    "#,
    )
    .unwrap();
}

/// `SetScrollFromBottom` (`0x793070`) runs the member prologue after its number gate: another
/// frame type fails the type test (`0x7930f0`) with "Wrong object type for member function"
/// (`0x847e98`), and a non-number returns before any receiver check (`0x79307f`).
#[test]
fn set_scroll_from_bottom_checks_its_receiver_late() {
    let s = script();
    s.run(
        r#"
        FromBottom = CreateFrame("ScrollingMessageFrame").SetScrollFromBottom
        FromBottom(CreateFrame("Frame"), nil)
    "#,
    )
    .unwrap();
    for (call, line) in [
        (
            "FromBottom(CreateFrame('Frame'), 1)",
            "Wrong object type for member function",
        ),
        (
            "FromBottom({}, 1)",
            "Attempt to find 'this' in non-framescript object",
        ),
        (
            "FromBottom(5, 1)",
            "Attempt to find 'this' in non-table object",
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    s.run("CreateFrame('ScrollingMessageFrame'):SetScrollFromBottom(1)")
        .unwrap();
}

/// The Frame verbs that take a string test it with `lua_isstring` after the receiver check and
/// raise their usage line naming the frame (RegisterEvent `0x8796dc`, UnregisterEvent `0x879700`,
/// GetScript `0x879698`, HasScript `0x87965c`, SetFrameStrata `0x8795b8`). SetScript also needs a
/// function or nil by `lua_type`, so a missing one raises (`0x774986`, `0x8796b4`); SetBackdrop
/// takes nil or a table, a bare call included in the raise (`0x777794`, `0x8798a8`); an inherits
/// miss names its method and frame (`0x879544`, `0x87957c`).
#[test]
fn frame_string_verbs_raise_their_usage_lines() {
    let s = script();
    s.run(
        r#"
        VerbFrame = CreateFrame("Frame", "VerbFrame")
        VerbAnon = CreateFrame("Frame")
        VerbFrame:SetBackdrop({ bgFile = "Interface\Tooltips\UI-Tooltip-Background" })
        VerbFrame:SetScript("OnShow", function() end)
    "#,
    )
    .unwrap();
    let backdrop = r#"Usage: VerbFrame:SetBackdrop(nil or {bgFile = "bgFile", edgeFile = "edgeFile", tile = false, tileSize = 0, edgeSize = 32, insets = { left = 0, right = 0, top = 0, bottom = 0 }})"#;
    for (call, line) in [
        (
            "VerbFrame:RegisterEvent(nil)",
            r#"Usage: VerbFrame:RegisterEvent("event")"#,
        ),
        (
            "VerbAnon:UnregisterEvent({})",
            r#"Usage: <unnamed>:UnregisterEvent("event")"#,
        ),
        (
            "VerbFrame:GetScript()",
            r#"Usage: VerbFrame:GetScript("type")"#,
        ),
        (
            "VerbFrame:HasScript(true)",
            r#"Usage: VerbFrame:HasScript("type")"#,
        ),
        (
            "VerbFrame:SetFrameStrata(nil)",
            "Usage: VerbFrame:SetFrameStrata(level)",
        ),
        (
            "VerbFrame:SetScript(nil, nil)",
            r#"Usage: VerbFrame:SetScript("type", function)"#,
        ),
        (
            "VerbFrame:SetScript('OnShow')",
            r#"Usage: VerbFrame:SetScript("type", function)"#,
        ),
        (
            "VerbFrame:SetScript('OnShow', 'f')",
            r#"Usage: VerbFrame:SetScript("type", function)"#,
        ),
        ("VerbFrame:SetBackdrop()", backdrop),
        ("VerbFrame:SetBackdrop('x')", backdrop),
        (
            "VerbFrame:CreateTexture(nil, nil, 'NoSuchNode')",
            r#"VerbFrame:CreateTexture(): Couldn't find inherited node "NoSuchNode""#,
        ),
        (
            "VerbAnon:CreateFontString(nil, nil, 'NoSuchNode')",
            r#"<unnamed>:CreateFontString(): Couldn't find inherited node "NoSuchNode""#,
        ),
    ] {
        let e = err(&s, call);
        assert!(e.contains(line), "`{call}` -> {e}");
    }
    // The raises left the script and the backdrop in place; an explicit nil clears each.
    s.run(
        r#"
        assert(VerbFrame:GetScript("OnShow") ~= nil and VerbFrame:GetBackdrop() ~= nil)
        VerbFrame:SetScript("OnShow", nil) VerbFrame:SetBackdrop(nil)
        assert(VerbFrame:GetScript("OnShow") == nil and VerbFrame:GetBackdrop() == nil)
        VerbFrame:SetFrameStrata("HIGH") VerbFrame:RegisterEvent("PLAYER_LOGIN")
    "#,
    )
    .unwrap();
}

/// `SortGuildRoster` tests argument 1 with `lua_isstring` (`0x4d1cb8`), so a number is its
/// string, and anything else raises `Usgae: SortGuildRoster("type")` (`0x849bf0`, `0x4d1cc7`),
/// the binary's spelling, with no call made.
#[test]
fn sort_guild_roster_raises_its_own_usage_line() {
    let mut s = UiScript::new().unwrap();
    s.keep_calls();
    for call in ["SortGuildRoster()", "SortGuildRoster({})"] {
        let e = err(&s, call);
        assert!(
            e.contains(r#"Usgae: SortGuildRoster("type")"#),
            "{call}: {e}"
        );
    }
    assert!(s.take_guild_calls().is_empty());
    s.run("SortGuildRoster(5)").unwrap();
    assert_eq!(s.take_guild_calls(), [GuildCall::Sort("5".into())]);
}

/// `is-number` (`0x6f34d0`) reads a string through `luaO_str2d` (`0x6f5900`), whose `strtod` takes
/// `d` as an exponent marker (`0x4112d1`) and no `inf` or `nan`: so `"1d0"` passes every gate
/// below and `"nan"`, `"inf"` and `"infinity"` raise each binding's own line.
#[test]
fn a_numeric_string_is_read_as_the_clients_strtod_reads_it() {
    let s = script();
    s.run(
        r#"
        StrtodFrame = CreateFrame("Frame", "StrtodFrame")
        StrtodModel = CreateFrame("Model", "StrtodModel")
        StrtodTT = CreateFrame("GameTooltip", "StrtodTT", nil, "TestTooltipTemplate")
    "#,
    )
    .unwrap();
    for ok in [
        r#"GetSpellName("1d0", "spell")"#,
        r#"PickupSpell("2D0", "spell")"#,
        r#"AcceptBattlefieldPort("5d0", 1)"#,
        r#"SetGuildRecruitmentMode("0d0")"#,
        "SetGuildRecruitmentMode(0/0)", // `_ftol` makes NaN mode 0 (`0x4a008f`)
        r#"SetGamma("1d0")"#,
        r#"SetWorldDetail("1d0")"#,
        r#"GetWorldStateUIInfo("1d0")"#,
        r#"StrtodFrame:SetMinResize("1d1", "2d1")"#,
    ] {
        s.run(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in ["nan", "inf", "infinity"] {
        for (call, line) in [
            (
                format!(r#"GetSpellName("{bad}", "spell")"#),
                "Invalid spell slot in GetSpellName",
            ),
            (
                format!(r#"CastSpell("{bad}", "spell")"#),
                "Invalid spell slot in CastSpell",
            ),
            (
                format!(r#"AcceptBattlefieldPort("{bad}", 1)"#),
                "Usage: AcceptBattlefieldPort(index, accept)",
            ),
            (
                format!(r#"SetGuildRecruitmentMode("{bad}")"#),
                "Usage: SetGuildRecruitmentMode(mode)",
            ),
            (format!(r#"SetGamma("{bad}")"#), "Usage: SetGamma(value)"),
            (
                format!(r#"SetWorldDetail("{bad}")"#),
                "Usage: SetWorldDetail",
            ),
            (
                format!(r#"GetWorldStateUIInfo("{bad}")"#),
                "Usage: GetWorldStateUIInfo(index)",
            ),
            (
                format!(r#"StrtodFrame:SetMinResize("{bad}", 1)"#),
                "Usage: StrtodFrame:SetMinResize(minWidth, minHeight)",
            ),
            (
                format!(r#"StrtodModel:SetLight("{bad}")"#),
                "Usage: StrtodModel:SetLight(enabled", // the name fills `%s` (`0x76e79f`)
            ),
            (
                format!(r#"StrtodTT:SetMerchantCompareItem("{bad}")"#),
                "Usage: SetMerchantCompareItem(",
            ),
        ] {
            let e = err(&s, &call);
            assert!(e.contains(line), "{call}: {e}");
        }
    }
}

/// The number is truncated by `_ftol` (`0x40a2b0`), whose QWORD `fistp` keeps the low dword: NaN
/// and an infinity are 0, and 2^32 + n is n (`SetWorldDetail` `0x488e0a`, `GetWorldStateUIInfo`
/// `0x4c5aad` then `dec` and an unsigned compare).
#[test]
fn number_arguments_truncate_through_ftol() {
    let mut s = script();
    s.register_cvars([
        (crate::script::CVAR_WORLD_DETAIL, "1"),
        (crate::script::CVAR_FRILL_DENSITY, "32"),
        (crate::script::CVAR_SMALL_CULL, "0.04"),
    ]);
    for (call, stop) in [
        ("SetWorldDetail(0/0)", 0),
        ("SetWorldDetail(4294967298)", 2),
        ("SetWorldDetail(1/0)", 0),
    ] {
        s.run(call).unwrap_or_else(|e| panic!("{call}: {e}"));
        assert_eq!(
            s.eval::<i64>("return GetWorldDetail()").unwrap(),
            stop,
            "{call}"
        );
    }
    s.set_world_state_ui(vec![WorldStateUiView {
        text: "Row".into(),
        ..Default::default()
    }]);
    let text = |s: &UiScript, i: &str| {
        s.eval::<mlua::Value>(&format!("local _, t = GetWorldStateUIInfo({i}) return t"))
            .unwrap()
    };
    assert!(matches!(text(&s, "4294967297"), mlua::Value::String(t) if t == "Row"));
    for i in ["0/0", "0", "4294967296"] {
        assert!(text(&s, i).is_nil(), "{i} is row 0, below the first");
    }
}

/// Every widget-specific method checks its receiver's type first and raises "Wrong object type for
/// member function" (`0x847e98`) for a widget of another type, as an addon that copies methods
/// between frames meets it; the same text the frame, region and font methods raise.
#[test]
fn a_widget_method_on_another_widget_raises_the_one_wrong_type_line() {
    let s = script();
    for (kind, method, args) in [
        ("Button", "SetText", "'x'"),
        ("Button", "GetText", ""),
        ("Button", "Enable", ""),
        ("Button", "SetNormalTexture", "'x'"),
        ("Button", "GetFontString", ""),
        ("Button", "SetTextFontObject", "nil"),
        ("CheckButton", "GetChecked", ""),
        ("ColorSelect", "SetColorRGB", "1, 1, 1"),
        ("ColorSelect", "GetColorRGB", ""),
        ("ColorSelect", "SetColorWheelTexture", "nil"),
        ("MessageFrame", "AddMessage", "'x'"),
        ("MessageFrame", "SetFadeDuration", "1"),
        ("ScrollingMessageFrame", "ScrollUp", ""),
        ("ScrollingMessageFrame", "AddMessage", "'x'"),
        ("Model", "SetModel", "'x'"),
        ("ScrollFrame", "SetVerticalScroll", "1"),
        ("ScrollFrame", "GetVerticalScroll", ""),
        ("SimpleHTML", "SetText", "'x'"),
        ("Slider", "SetValue", "1"),
        ("Slider", "GetValue", ""),
        ("Slider", "SetThumbTexture", "nil"),
        ("StatusBar", "SetValue", "1"),
        ("StatusBar", "GetValue", ""),
        ("StatusBar", "SetStatusBarTexture", "nil"),
        ("GameTooltip", "AddLine", "'x'"),
        ("GameTooltip", "ClearLines", ""),
        ("GameTooltip", "SetOwner", "UIParent"),
    ] {
        let sep = if args.is_empty() { "" } else { ", " };
        let call = format!("CreateFrame('{kind}').{method}(CreateFrame('Frame'){sep}{args})");
        let e = err(&s, &call);
        assert!(
            e.contains("Wrong object type for member function"),
            "{kind}.{method} on a Frame -> {e}"
        );
    }
}

/// The widget helpers a method reaches past the prologue raise the same line for a frame of
/// another kind (`0x847e98`), not a wording of their own.
#[test]
fn a_widget_helper_on_another_kind_raises_the_one_wrong_type_line() {
    use crate::script::object::ERR_WRONG_TYPE;
    let s = script();
    let frame: mlua::Table = s.eval("return CreateFrame('Frame')").unwrap();
    let e = crate::script::button::with_button(s.lua(), &frame, |_| ()).unwrap_err();
    assert!(e.to_string().contains(ERR_WRONG_TYPE), "{e}");
    let e = crate::script::slider::with_slider(s.lua(), &frame, |_| ()).unwrap_err();
    assert!(e.to_string().contains(ERR_WRONG_TYPE), "{e}");
}
