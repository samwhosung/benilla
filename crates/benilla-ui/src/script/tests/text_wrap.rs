//! A FontString's `nonspacewrap` and `maxLines`: read from XML by the loader (`0x77103f`,
//! `0x7710b2`), handed to the host on the text quad and its measure request.

use super::common::script;
use crate::script::{QuadContent, UiScript};

fn load(s: &UiScript, xml: &str) {
    let doc = crate::framexml::parse(xml).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "loader errors: {:?}",
        report.errors
    );
}

/// The `(non_space_wrap, max_lines)` of the one text quad reading `text`.
fn wrap_of(s: &UiScript, text: &str) -> (bool, u32) {
    s.extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                non_space_wrap,
                max_lines,
                ..
            } if t == text => Some((non_space_wrap, max_lines)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no text quad reading {text:?}"))
}

fn screen() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s
}

const XML: &str = r#"<Ui>
    <Frame name="WrapHost">
      <Size><AbsDimension x="200" y="100"/></Size>
      <Anchors><Anchor point="CENTER"/></Anchors>
      <Layers><Layer level="ARTWORK">
        <FontString name="WrapPlain" text="plain" font="Fonts\FRIZQT__.TTF">
          <Size><AbsDimension x="100" y="12"/></Size>
          <Anchors><Anchor point="TOPLEFT"/></Anchors>
        </FontString>
        <FontString name="WrapFlagged" text="flagged" font="Fonts\FRIZQT__.TTF"
                    nonspacewrap="true" maxLines="3">
          <Size><AbsDimension x="100" y="12"/></Size>
          <Anchors><Anchor point="TOPRIGHT"/></Anchors>
        </FontString>
        <FontString name="WrapOff" text="off" font="Fonts\FRIZQT__.TTF" nonspacewrap="false"
                    maxLines="1x">
          <Size><AbsDimension x="100" y="12"/></Size>
          <Anchors><Anchor point="BOTTOMLEFT"/></Anchors>
        </FontString>
      </Layer></Layers>
    </Frame>
  </Ui>"#;

/// The loader reads both attributes into the FontString; neither is on a string without them.
#[test]
fn the_loader_reads_nonspacewrap_and_maxlines() {
    let mut s = screen();
    load(&s, XML);
    s.resolve();
    assert_eq!(wrap_of(&s, "plain"), (false, 0));
    assert_eq!(wrap_of(&s, "flagged"), (true, 3));
    // `false` is parsed as a boolean, and `maxLines` reads its leading digits (`0x64ade0`).
    assert_eq!(wrap_of(&s, "off"), (false, 1));
    for (name, want) in [
        ("WrapPlain", None),
        ("WrapFlagged", Some(1)),
        ("WrapOff", None),
    ] {
        assert_eq!(
            s.eval::<Option<i64>>(&format!("return {name}:CanNonSpaceWrap()"))
                .unwrap(),
            want,
            "{name}:CanNonSpaceWrap()"
        );
    }
}

/// A Button's `<ButtonText>` is built by the same loader, so it takes the attributes too: the
/// stock quest log title button sets `nonspacewrap` there (`QuestLogFrame.xml:92`).
#[test]
fn a_buttons_label_takes_nonspacewrap_from_its_buttontext() {
    let s = screen();
    load(
        &s,
        r#"<Ui>
             <Button name="WrapButton">
               <Size><AbsDimension x="100" y="16"/></Size>
               <Anchors><Anchor point="CENTER"/></Anchors>
               <ButtonText name="WrapButtonText" nonspacewrap="true"/>
             </Button>
           </Ui>"#,
    );
    assert_eq!(
        s.eval::<Option<i64>>("return WrapButtonText:CanNonSpaceWrap()")
            .unwrap(),
        Some(1)
    );
}

/// The flag is part of the measure's key: a measured size is stale once it flips.
#[test]
fn the_flag_and_the_line_limit_re_arm_the_measure() {
    let mut s = screen();
    load(&s, XML);
    s.resolve();
    // Nothing is measured here, so every string with text asks.
    let ask = |s: &mut UiScript| s.fontstrings_needing_measure();
    let first = ask(&mut s);
    let plain = first
        .iter()
        .find(|r| r.text == "plain")
        .expect("the plain string asks");
    assert_eq!((plain.non_space_wrap, plain.max_lines), (false, 0));
    let flagged = first
        .iter()
        .find(|r| r.text == "flagged")
        .expect("the flagged string asks");
    assert_eq!((flagged.non_space_wrap, flagged.max_lines), (true, 3));
    assert_ne!(plain.key, flagged.key);

    // Answered, then flipped: it asks again under another key.
    s.set_measured_text(&[(plain.id, 10.0, 12.0, 10.0, plain.key)]);
    s.run("WrapPlain:SetNonSpaceWrap(true)").unwrap();
    let again = ask(&mut s);
    let plain2 = again
        .iter()
        .find(|r| r.text == "plain")
        .expect("flipping the flag asks again");
    assert!(plain2.non_space_wrap);
    assert_ne!(plain2.key, plain.key);
}

/// A multi-line EditBox's text is `nonspacewrap` (`0x77a58f` sets bit `0x1000` of its label), a
/// single-line box's is not.
#[test]
fn a_multi_line_editboxs_text_wraps_nonspace() {
    let mut s = screen();
    load(
        &s,
        r#"<Ui>
             <EditBox name="WrapEdit" multiLine="true" autoFocus="false">
               <Size><AbsDimension x="100" y="40"/></Size>
               <Anchors><Anchor point="CENTER"/></Anchors>
               <FontString font="Fonts\FRIZQT__.TTF"/>
             </EditBox>
           </Ui>"#,
    );
    s.run(r#"WrapEdit:SetText("some text")"#).unwrap();
    s.resolve();
    assert!(wrap_of(&s, "some text").0);
    s.run("WrapEdit:SetMultiLine(false)").unwrap();
    s.resolve();
    assert!(!wrap_of(&s, "some text").0);
}
