//! The inherit model a font follower runs under: five groups, one mask bit each (a FontString's
//! `+0xd4`, a Font's `+0x2c`), and the FontString's per-bit justify mask (`+0x124`).

use super::common::script;
use crate::script::{QuadContent, UiScript};

/// Load a FrameXML fragment through the real loader, asserting it reported no errors.
fn load(s: &UiScript, xml: &str) {
    let doc = crate::framexml::parse(xml).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "loader errors: {:?}",
        report.errors
    );
}

/// A Font `A` (Friz 10, white), and a shown FontString `fs` on it reading "text".
fn fixture(s: &UiScript) {
    load(
        s,
        r#"<Ui>
             <Font name="A" font="Fonts\FRIZQT__.TTF" outline="NORMAL">
               <FontHeight><AbsValue val="10"/></FontHeight>
               <Color r="1" g="1" b="1"/>
             </Font>
           </Ui>"#,
    );
    s.run(
        r#"
        local f = CreateFrame("Frame", "Host", UIParent)
        f:SetWidth(200); f:SetHeight(50); f:SetPoint("CENTER", 0, 0)
        fs = f:CreateFontString(nil, "ARTWORK", "A")
        fs:SetPoint("CENTER", 0, 0)
        fs:SetText("text")
    "#,
    )
    .unwrap();
}

/// The drawn size override of the one text quad reading `text` (`QuadContent::Text::text_height`).
fn drawn_text_height(s: &UiScript, text: &str) -> Option<f32> {
    s.extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                text_height,
                ..
            } if t == text => Some(text_height),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no text quad reading {text:?}"))
}

/// One justify axis set locally stops the whole justify group: `SetJustifyH` clears the group
/// bit (`0x79e7a5`), so the font's later vertical justify never arrives either.
#[test]
fn one_local_justify_axis_stops_the_font_reaching_the_other() {
    let s = script();
    fixture(&s);
    s.run("fs:SetJustifyH('LEFT'); A:SetJustifyV('TOP')")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return fs:GetJustifyV()").unwrap(),
        "MIDDLE"
    );
}

/// A font's justify is one word: a font that sets only `justifyH` still holds the vertical axis at
/// the constructor's MIDDLE (`0x212`), and a link writes both (`0x770868`).
#[test]
fn a_fonts_justify_word_carries_both_axes() {
    let s = script();
    fixture(&s);
    load(
        &s,
        r#"<Ui>
             <Font name="Top" font="Fonts\FRIZQT__.TTF" justifyV="TOP">
               <FontHeight><AbsValue val="10"/></FontHeight>
             </Font>
             <Font name="Left" font="Fonts\FRIZQT__.TTF" justifyH="LEFT">
               <FontHeight><AbsValue val="10"/></FontHeight>
             </Font>
           </Ui>"#,
    );
    s.run("fs:SetFontObject(Top)").unwrap();
    assert_eq!(s.eval::<String>("return fs:GetJustifyV()").unwrap(), "TOP");
    s.run("fs:SetFontObject(Left)").unwrap();
    assert_eq!(
        s.eval::<(String, String)>("return fs:GetJustifyH(), fs:GetJustifyV()")
            .unwrap(),
        ("LEFT".to_string(), "MIDDLE".to_string())
    );
}

/// The face, height and flags are one group, set only when the font holds a face: an empty
/// `CreateFont` font leaves a string's outline alone (`0x770826` tests the set bit).
#[test]
fn an_empty_font_leaves_the_outline_alone() {
    let s = script();
    fixture(&s);
    s.run("fs:SetFontObject(CreateFont('Empty'))").unwrap();
    assert_eq!(
        s.eval::<String>("local _, _, f = fs:GetFont(); return f")
            .unwrap(),
        "OUTLINE"
    );
}

/// `SetTextHeight` writes the text height a font load writes (`+0xe4`): `GetFont` answers it, and
/// the next font group to arrive replaces it, the string still drawing scaled (bit `0x200` stays
/// clear).
#[test]
fn set_text_height_shares_the_height_a_font_load_writes() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    fixture(&s);
    s.run("fs:SetTextHeight(20)").unwrap();
    assert_eq!(
        s.eval::<f32>("local _, h = fs:GetFont(); return h")
            .unwrap(),
        20.0
    );
    s.resolve();
    assert_eq!(drawn_text_height(&s, "text"), Some(20.0));
    s.run(r#"A:SetFont("Fonts\\FRIZQT__.TTF", 12)"#).unwrap();
    assert_eq!(
        s.eval::<f32>("local _, h = fs:GetFont(); return h")
            .unwrap(),
        12.0
    );
    s.resolve();
    assert_eq!(drawn_text_height(&s, "text"), Some(12.0), "scaled at 12");
}

/// A height within 2^-22 of the current one changes nothing, the one-to-one bit included
/// (`0x77161c`).
#[test]
fn set_text_height_to_the_current_height_stays_one_to_one() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    fixture(&s);
    s.run("fs:SetTextHeight(10)").unwrap();
    s.resolve();
    assert_eq!(drawn_text_height(&s, "text"), None);
}
