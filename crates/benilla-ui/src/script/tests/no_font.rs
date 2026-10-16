//! A FontString with no font, the reference's null `CSimpleFontString+0xe0`: a string starts with
//! none (`0x770d30`), Lua's `SetText` raises on it (`0x79d7e5`), it measures 0 (`0x772bd0`,
//! `0x7728b6`) and it builds and emits nothing (`0x7724f8`, `0x772e5b`).

use super::common::script;
use crate::script::{MeasureRequest, QuadContent, TextMeasure, UiScript};

/// Each character 6 wide, one 13-tall line.
struct BlockFont;

impl TextMeasure for BlockFont {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        let w = req.text.chars().count() as f32 * 6.0;
        (w, 13.0, w)
    }
}

fn vm() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_text_measurer(Box::new(BlockFont));
    s.run(
        r#"Host = CreateFrame("Frame", "Host")
           Host:SetWidth(200); Host:SetHeight(40); Host:SetPoint("CENTER", 0, 0)"#,
    )
    .unwrap();
    s
}

/// Whether a Text quad draws `text`.
fn draws(s: &mut UiScript, text: &str) -> bool {
    s.resolve();
    s.extract()
        .iter()
        .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == text))
}

/// `%s:SetText(): Font not set` (`0x87c6cc`), the name or `<unnamed>` (`0x84c7f0`), raised before
/// the text is stored (`0x79d810` precedes `0x79d829`).
#[test]
fn set_text_on_a_bare_font_string_raises_and_sets_nothing() {
    let s = vm();
    let err = s
        .run(r#"Bare = Host:CreateFontString("Bare", "ARTWORK") Bare:SetText("hello")"#)
        .expect_err("a string with no font raises");
    assert!(
        err.to_string().contains("Bare:SetText(): Font not set"),
        "{err}"
    );
    assert!(s
        .eval::<Option<String>>("return Bare:GetText()")
        .unwrap()
        .is_none());

    let err = s
        .run(r#"local t = Host:CreateFontString() t:SetText("x")"#)
        .expect_err("an unnamed one too");
    assert!(
        err.to_string()
            .contains("<unnamed>:SetText(): Font not set"),
        "{err}"
    );
    // Any argument, nil included: the font test comes first (`0x79d7e5`).
    assert!(s.run("Bare:SetText(nil)").is_err());
}

/// XML `text=` stores through the string's own setter (`0x77103a` → `0x771d80`), so a font-less
/// string holds its text; it measures 0 and draws nothing.
#[test]
fn a_font_less_string_holds_xml_text_but_measures_zero_and_draws_nothing() {
    let mut s = vm();
    let doc = crate::framexml::parse(
        r#"<Ui><Frame name="XmlHost" setAllPoints="true"><Layers><Layer level="ARTWORK">
            <FontString name="NoFace" text="orphan"><Anchors><Anchor point="CENTER"/></Anchors></FontString>
        </Layer></Layers></Frame></Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        s.eval::<String>("return NoFace:GetText()").unwrap(),
        "orphan"
    );
    assert_eq!(
        s.eval::<f32>("return NoFace:GetStringWidth()").unwrap(),
        0.0
    );
    assert!(!draws(&mut s, "orphan"), "no font builds no string");
    assert!(
        s.fontstrings_needing_measure()
            .iter()
            .all(|r| r.text != "orphan"),
        "nothing asks the font engine"
    );

    // Given a font, the same string measures and draws.
    s.run(r#"NoFace:SetFont("Fonts\\FRIZQT__.TTF", 12)"#)
        .unwrap();
    assert_eq!(
        s.eval::<f32>("return NoFace:GetStringWidth()").unwrap(),
        36.0
    );
    assert!(draws(&mut s, "orphan"));
}

/// A font from a font object, by XML `inherits=`, by `CreateFontString`'s third argument or by
/// `SetFontObject`, is a font: `SetText` takes it and the string draws.
#[test]
fn a_font_object_given_font_still_sets_measures_and_draws() {
    let mut s = vm();
    let doc = crate::framexml::parse(
        r#"<Ui>
            <Font name="TestFontNormal" font="Fonts\FRIZQT__.TTF" virtual="true">
                <FontHeight><AbsValue val="12"/></FontHeight>
            </Font>
            <FontString name="TestLabelTemplate" inherits="TestFontNormal" virtual="true"/>
            <Frame name="Owner" setAllPoints="true"><Layers><Layer level="ARTWORK">
                <FontString name="ByXml" inherits="TestLabelTemplate">
                    <Anchors><Anchor point="TOP"/></Anchors>
                </FontString>
            </Layer></Layers></Frame>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    s.run(
        r#"ByXml:SetText("xml")
           local c = Host:CreateFontString("ByArg", "ARTWORK", "TestFontNormal")
           c:SetPoint("CENTER", 0, 0); c:SetText("arg")
           local o = Host:CreateFontString("ByObject", "ARTWORK")
           o:SetFontObject(TestFontNormal); o:SetPoint("BOTTOM", 0, 0); o:SetText("object")"#,
    )
    .unwrap();
    for (name, text) in [("ByXml", "xml"), ("ByArg", "arg"), ("ByObject", "object")] {
        assert!(draws(&mut s, text), "{name} draws");
        let width = s
            .eval::<f32>(&format!("return {name}:GetStringWidth()"))
            .unwrap();
        assert_eq!(width, text.len() as f32 * 6.0, "{name} measures");
    }
}

/// `SetFont` on a string (`0x79d4f0` → `0x7714f0`): an empty path is refused by the load
/// (`0x44d05c`) and a file that does not load fails it (`0x44d22b`), each answering nil and
/// keeping the font there was, none on a fresh string; a height of 0 frees the font and answers 1
/// (`0x77155b` → `0x7715b3`).
#[test]
fn set_font_refusals_keep_the_font_there_was_and_a_zero_height_frees_it() {
    let mut s = vm();
    s.set_font_probe(Box::new(|p: &str| !p.contains("MISSING")));
    s.run(r#"F = Host:CreateFontString("F", "ARTWORK")"#)
        .unwrap();

    // A fresh string: both refusals leave it with no font.
    assert!(s
        .eval::<bool>(r#"return F:SetFont("", 12) == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return F:SetFont("Fonts\\MISSING.TTF", 12) == nil"#)
        .unwrap());
    assert!(s.run(r#"F:SetText("a")"#).is_err(), "still no font");

    // A loaded font survives both refusals.
    assert_eq!(
        s.eval::<f32>(r#"return F:SetFont("Fonts\\FRIZQT__.TTF", 12)"#)
            .unwrap(),
        1.0
    );
    assert!(s
        .eval::<bool>(r#"return F:SetFont("", 12) == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return F:SetFont("Fonts\\MISSING.TTF", 12) == nil"#)
        .unwrap());
    assert_eq!(
        s.eval::<String>("return (F:GetFont())").unwrap(),
        "Fonts\\FRIZQT__.TTF"
    );
    s.run(r#"F:SetText("kept")"#).unwrap();

    // A height of 0 frees it: GetFont answers nil, 0, "" and SetText raises again.
    assert_eq!(
        s.eval::<f32>(r#"return F:SetFont("Fonts\\FRIZQT__.TTF", 0)"#)
            .unwrap(),
        1.0
    );
    assert_eq!(
        s.eval::<(Option<String>, f32, String)>("return F:GetFont()")
            .unwrap(),
        (None, 0.0, String::new())
    );
    assert!(s.run(r#"F:SetText("b")"#).is_err());
    assert_eq!(s.eval::<String>("return F:GetText()").unwrap(), "kept");
}

/// A Button's label follows the button's fonts: `Button:SetFont` (`0x780880`) and a
/// `<ButtonText font=…>` font object (`0x6f2780` → `0x771104`) each give it one.
#[test]
fn a_button_label_takes_the_buttons_font_and_its_own_font_attribute() {
    let mut s = vm();
    s.run(
        r#"B = CreateFrame("Button", "FontBtn", Host)
           B:SetPoint("CENTER", 0, 0); B:SetWidth(80); B:SetHeight(20)
           B:SetFont("Fonts\\FRIZQT__.TTF", 12)
           B:SetText("seed")
           B:GetFontString():SetText("label")"#,
    )
    .unwrap();
    assert!(draws(&mut s, "label"));

    let doc = crate::framexml::parse(
        r#"<Ui>
            <Font name="TestFontGreen" font="Fonts\FRIZQT__.TTF" virtual="true">
                <FontHeight><AbsValue val="12"/></FontHeight>
            </Font>
            <Button name="TabBtn"><Size x="34" y="8"/><Anchors><Anchor point="TOP"/></Anchors>
                <ButtonText font="TestFontGreen"/>
            </Button>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        s.eval::<String>("return (TabBtn:GetFontString():GetFont())")
            .unwrap(),
        "Fonts\\FRIZQT__.TTF"
    );
    s.run(r#"TabBtn:SetText("Tab")"#).unwrap();
    assert!(draws(&mut s, "Tab"));
}

/// An XML `font=` that names a file needs a first `<FontHeight>` above 0 (`0x77112f`-`0x771154`).
/// Else the loader reports `FontString %s: Missing font height in %s element` (`0x878e10`) at
/// severity 1 and returns (`0x771191`) before it sets the font, so the string has none.
#[test]
fn an_xml_font_file_without_a_positive_height_leaves_the_string_fontless() {
    let s = vm();
    let doc = crate::framexml::parse(
        r#"<Ui><Frame name="XmlHost" setAllPoints="true"><Layers><Layer level="ARTWORK">
            <FontString name="NoHeight" font="Fonts\FRIZQT__.TTF" outline="NORMAL" text="a"/>
            <FontString name="ZeroHeight" font="Fonts\FRIZQT__.TTF">
                <FontHeight><AbsValue val="0"/></FontHeight>
            </FontString>
            <FontString name="NegHeight" font="Fonts\FRIZQT__.TTF">
                <FontHeight><AbsValue val="-4"/></FontHeight>
            </FontString>
            <FontString font="Fonts\FRIZQT__.TTF"/>
            <FontString name="Sized" font="Fonts\FRIZQT__.TTF" outline="NORMAL">
                <FontHeight><AbsValue val="12"/></FontHeight>
            </FontString>
        </Layer></Layers></Frame></Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    for name in ["NoHeight", "ZeroHeight", "NegHeight", "<unnamed>"] {
        let line = format!("FontString {name}: Missing font height in FontString element");
        assert!(
            report.warnings.contains(&line),
            "{line}: {:?}",
            report.warnings
        );
        assert!(
            report.status.lines().contains(&line),
            "{line} in the status"
        );
    }
    assert_eq!(report.warnings.len(), 4, "{:?}", report.warnings);

    for name in ["NoHeight", "ZeroHeight", "NegHeight"] {
        assert!(
            s.eval::<Option<String>>(&format!("return (({name}):GetFont())"))
                .unwrap()
                .is_none(),
            "{name} has no font"
        );
        let err = s
            .run(&format!(r#"{name}:SetText("x")"#))
            .expect_err("a string with no font raises");
        assert!(
            err.to_string()
                .contains(&format!("{name}:SetText(): Font not set")),
            "{err}"
        );
    }

    // A positive height still sets the font, outline and all.
    assert_eq!(
        s.eval::<(String, f32, String)>("return Sized:GetFont()")
            .unwrap(),
        ("Fonts\\FRIZQT__.TTF".to_string(), 12.0, "".to_string())
    );
    s.run(r#"Sized:SetText("ok")"#).unwrap();
}

/// A `<ButtonText>` is the same loader (`0x7789d0` → `0x770f40`), and names itself by its tag.
#[test]
fn a_button_text_font_file_without_a_height_reports_and_stays_fontless() {
    let s = vm();
    let doc = crate::framexml::parse(
        r#"<Ui><Button name="Plain"><ButtonText name="PlainText" font="Fonts\FRIZQT__.TTF"/></Button></Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        report.warnings,
        ["FontString PlainText: Missing font height in ButtonText element"]
    );
    assert!(s.run(r#"PlainText:SetText("x")"#).is_err());
}
