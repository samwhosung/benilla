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

// ── Font to font, live ──

/// `A` (Friz 10), `B` inheriting it with its own red, and `fs` on `B`.
fn chain(s: &UiScript) {
    load(
        s,
        r#"<Ui>
             <Font name="A" font="Fonts\FRIZQT__.TTF">
               <FontHeight><AbsValue val="10"/></FontHeight>
               <Color r="1" g="1" b="1"/>
             </Font>
             <Font name="B" inherits="A"><Color r="1" g="0" b="0"/></Font>
           </Ui>"#,
    );
    s.run(
        r#"
        local f = CreateFrame("Frame", "Host", UIParent)
        fs = f:CreateFontString(nil, "ARTWORK", "B")
    "#,
    )
    .unwrap();
}

fn font_of(s: &UiScript, who: &str) -> (String, f32) {
    s.eval::<(String, f32)>(&format!("local f, h = {who}:GetFont(); return f, h"))
        .unwrap()
}

fn color_of(s: &UiScript, who: &str) -> (f32, f32, f32) {
    s.eval::<(f32, f32, f32)>(&format!("return {who}:GetTextColor()"))
        .unwrap()
}

/// `inherits=` links (`0x783ce6`): a change on the parent reaches the font declared from it and
/// that font's strings, two hops; the child's own colour cleared its bit (`0x784035`), so the
/// parent's colour stops there.
#[test]
fn a_change_on_a_parent_font_reaches_its_chain() {
    let s = script();
    chain(&s);
    s.run(r#"A:SetFont("Fonts\\MORPHEUS.TTF", 14)"#).unwrap();
    let want = ("Fonts\\MORPHEUS.TTF".to_string(), 14.0);
    assert_eq!(font_of(&s, "B"), want);
    assert_eq!(font_of(&s, "fs"), want);
    s.run("A:SetTextColor(0, 1, 0)").unwrap();
    assert_eq!(
        color_of(&s, "B"),
        (crate::color::quant::read32(1.0), 0.0, 0.0)
    );
    assert_eq!(
        color_of(&s, "fs"),
        (crate::color::quant::read32(1.0), 0.0, 0.0)
    );
}

/// A font's own setter clears its bit (`0x79f33a`): the parent's later font no longer reaches it.
#[test]
fn a_fonts_own_setter_outlasts_its_parent() {
    let s = script();
    chain(&s);
    s.run(r#"A:SetFont("Fonts\\SKURRI.TTF", 16)"#).unwrap();
    assert_eq!(font_of(&s, "B").1, 16.0, "B follows until it sets its own");
    s.run(
        r#"
        B:SetFont("Fonts\\ARIALN.TTF", 9)
        A:SetFont("Fonts\\MORPHEUS.TTF", 14)
    "#,
    )
    .unwrap();
    let want = ("Fonts\\ARIALN.TTF".to_string(), 9.0);
    assert_eq!(font_of(&s, "B"), want);
    assert_eq!(font_of(&s, "fs"), want);
}

/// Only what changed is pushed (`0x770800` tests the changed bits): a colour change leaves a
/// string's own text height alone.
#[test]
fn a_push_carries_only_the_group_that_changed() {
    let s = script();
    chain(&s);
    s.run("fs:SetTextHeight(20); B:SetTextColor(0, 0, 1)")
        .unwrap();
    assert_eq!(font_of(&s, "fs").1, 20.0);
    assert_eq!(
        color_of(&s, "fs"),
        (0.0, 0.0, crate::color::quant::read32(1.0))
    );
}

/// `GameFontHighlight` is declared from `GameFontNormal` (`Fonts.xml:76`), so restyling the base
/// font restyles it and every string declared from it.
#[test]
fn restyling_game_font_normal_reaches_game_font_highlight() {
    let s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="GameFontNormal" font="Fonts\FRIZQT__.TTF" virtual="true">
               <FontHeight><AbsValue val="12"/></FontHeight>
               <Color r="1.0" g="0.82" b="0"/>
             </Font>
             <Font name="GameFontHighlight" inherits="GameFontNormal" virtual="true">
               <Color r="1.0" g="1.0" b="1.0"/>
             </Font>
             <Frame name="Host"><Layers><Layer level="ARTWORK">
               <FontString name="HostText" inherits="GameFontHighlight"/>
             </Layer></Layers></Frame>
           </Ui>"#,
    );
    s.run(r#"GameFontNormal:SetFont("Fonts\\ARIALN.TTF", 20)"#)
        .unwrap();
    let want = ("Fonts\\ARIALN.TTF".to_string(), 20.0);
    assert_eq!(font_of(&s, "GameFontHighlight"), want);
    assert_eq!(font_of(&s, "HostText"), want);
    assert_eq!(
        color_of(&s, "HostText"),
        (
            crate::color::quant::read32(1.0),
            crate::color::quant::read32(1.0),
            crate::color::quant::read32(1.0)
        )
    );
}

/// `GetFontObject` on a Font answers its parent (`0x7a00f0`); nil unlinks and keeps the block; a
/// link back down the chain raises (`0x79f045`, `0x87c8b0`); `font=` naming a font links too
/// (`0x783d22`).
#[test]
fn font_links_are_read_cut_refused_and_made_by_font() {
    let s = script();
    chain(&s);
    assert!(s.eval::<bool>("return B:GetFontObject() == A").unwrap());
    assert!(s.eval::<bool>("return A:GetFontObject() == nil").unwrap());

    let err = s.run("A:SetFontObject(B)").unwrap_err().to_string();
    assert!(
        err.contains("A:SetFontObject(): Can't create a font object loop"),
        "{err}"
    );
    let err = s.run("A:SetFontObject(A)").unwrap_err().to_string();
    assert!(err.contains("Can't create a font object loop"), "{err}");

    load(&s, r#"<Ui><Font name="C" font="A"/></Ui>"#);
    s.run(r#"A:SetFont("Fonts\\MORPHEUS.TTF", 14)"#).unwrap();
    assert_eq!(font_of(&s, "C"), ("Fonts\\MORPHEUS.TTF".to_string(), 14.0));
    assert!(s.eval::<bool>("return C:GetFontObject() == A").unwrap());

    s.run(r#"B:SetFontObject(nil); A:SetFont("Fonts\\ARIALN.TTF", 8)"#)
        .unwrap();
    assert!(s.eval::<bool>("return B:GetFontObject() == nil").unwrap());
    assert_eq!(font_of(&s, "B"), ("Fonts\\MORPHEUS.TTF".to_string(), 14.0));
}

/// A re-declared `<Font>` is the same object (`0x7839ab`): it keeps its followers, and its new
/// attributes reach them.
#[test]
fn a_redeclared_font_keeps_its_followers() {
    let s = script();
    chain(&s);
    load(
        &s,
        r#"<Ui><Font name="B" inherits="A"><Color r="0" g="1" b="0"/></Font></Ui>"#,
    );
    assert_eq!(
        color_of(&s, "fs"),
        (0.0, crate::color::quant::read32(1.0), 0.0)
    );
}

/// The font hash folds case (`SStrHashHT 0x64b3f0`, `SStrCmpI` at `0x7838c7`): a name in any
/// case is the one object, its handle the global, and its changes reach the string.
#[test]
fn a_font_name_in_any_case_is_the_one_object() {
    let s = script();
    chain(&s);
    s.run("fs:SetFontObject('a')").unwrap();
    assert!(s.eval::<bool>("return fs:GetFontObject() == A").unwrap());
    s.run(r#"A:SetFont("Fonts\\MORPHEUS.TTF", 14)"#).unwrap();
    assert_eq!(font_of(&s, "fs").1, 14.0);
}

/// `SetFontObject`'s argument errors are the reference's (`0x79d1a0`, `0x79ef10`), naming the
/// receiver by its own name or `<unnamed>`.
#[test]
fn set_font_object_raises_the_reference_texts() {
    let s = script();
    chain(&s);
    let err = |code: &str| s.run(code).unwrap_err().to_string();
    for (code, want) in [
        (
            "fs:SetFontObject('Nope')",
            "<unnamed>:SetFontObject(): Couldn't find font named Nope",
        ),
        (
            "fs:SetFontObject(Host)",
            "<unnamed>:SetFontObject(): Wrong object type, expected font",
        ),
        (
            "fs:SetFontObject({})",
            "<unnamed>:SetFontObject(): Couldn't find 'this' in font object",
        ),
        (
            "fs:SetFontObject(5)",
            "Usage: <unnamed>:SetFontObject(font or \"font\" or nil)",
        ),
        (
            "fs:SetFontObject()",
            "Usage: <unnamed>:SetFontObject(font or \"font\" or nil)",
        ),
        (
            "B:SetFontObject('Nope')",
            "B:SetFontObject(): Couldn't find font named Nope",
        ),
        (
            "B:CopyFontObject(nil)",
            "Usage: B:CopyFontObject(font or \"font\")",
        ),
        (
            "B:CopyFontObject('Nope')",
            "B:CopyFontObject(): Couldn't find font named Nope",
        ),
    ] {
        let e = err(code);
        assert!(e.contains(want), "{code}: {e}");
    }
}

/// `CopyFontObject` ends in `CSimpleFont::SetFontObject` (`0x7841d0`): it merges what the source
/// holds (`0x7841ea`), then ANDs its own held word with `+0x6c` (`0x7841f4`), which is 0 in every
/// font (the constructor `0x783aca` and `0x7841e7` are its only stores). The copy holds nothing,
/// but its getters read the values, which keep what the source did not hold (`0x79f680` reads
/// `+0x58` whatever the word says): a colour the source never held stays the copy's own.
#[test]
fn a_copy_keeps_a_value_the_source_does_not_hold() {
    let s = script();
    s.run(
        r#"
        F = CreateFont("CopySourceF")
        F:SetFont("Fonts\\ARIALN.TTF", 9)
        B = CreateFont("CopyTargetB")
        B:SetTextColor(1, 0, 0)
        B:CopyFontObject(F)
    "#,
    )
    .unwrap();
    assert_eq!(
        color_of(&s, "B"),
        (crate::color::quant::read32(1.0), 0.0, 0.0)
    );
    assert_eq!(font_of(&s, "B"), ("Fonts\\ARIALN.TTF".to_string(), 9.0));
}

/// The copy's merge skips a face equal ignoring case, with an equal height and flags
/// (`0x770b20`-`0x770b44`), so the copy keeps its own spelling of it.
#[test]
fn a_copy_keeps_its_spelling_of_an_equal_face() {
    let s = script();
    s.run(
        r#"
        F = CreateFont("SpellingSource")
        F:SetFont("fonts\\arialn.ttf", 9)
        B = CreateFont("SpellingCopy")
        B:SetFont("Fonts\\ARIALN.TTF", 9)
        B:CopyFontObject(F)
    "#,
    )
    .unwrap();
    assert_eq!(font_of(&s, "B"), ("Fonts\\ARIALN.TTF".to_string(), 9.0));
}

/// The copy follows the font the source follows (`0x7841fe`-`0x78426d`), so a later change on that
/// font reaches it through the follow mask it took from the source (`0x7841fb`).
#[test]
fn a_copy_of_a_follower_follows_its_font() {
    let s = script();
    chain(&s);
    s.run(r#"C = CreateFont("CopyOfB") C:CopyFontObject(B)"#)
        .unwrap();
    assert!(s.eval::<bool>("return C:GetFontObject() == A").unwrap());
    s.run(r#"A:SetFont("Fonts\\MORPHEUS.TTF", 14)"#).unwrap();
    assert_eq!(font_of(&s, "C"), ("Fonts\\MORPHEUS.TTF".to_string(), 14.0));
}

/// The copy takes the source's follow mask whole (`0x7841fb`): a group the copy had stopped
/// following by its own setter is followed again when the source still follows it.
#[test]
fn a_copy_takes_the_sources_follow_mask() {
    let s = script();
    chain(&s);
    s.run(
        r#"
        C = CreateFont("MaskCopy")
        C:SetTextColor(0, 1, 0)
        D = CreateFont("MaskSource")
        D:SetFontObject(A)
        C:CopyFontObject(D)
        A:SetTextColor(0, 0, 1)
    "#,
    )
    .unwrap();
    assert_eq!(
        color_of(&s, "C"),
        (0.0, 0.0, crate::color::quant::read32(1.0))
    );
}

/// The copy holds nothing (`0x7841f4`), so its slot-5 push returns at once (`0x78418c`) and a
/// FontString that follows it, before or after the copy, takes none of the copied values
/// (`0x770800` writes only what a font holds). A setter then stores even an equal value, since
/// its group is not held (`0x79f65a`), and pushes it.
#[test]
fn a_copy_holds_nothing_until_a_setter_writes() {
    let s = script();
    chain(&s);
    s.run(
        r#"
        local host = CreateFrame("Frame", "CopyHost", UIParent)
        early = host:CreateFontString()
        late = host:CreateFontString()
        C = CreateFont("HoldsNothing")
        early:SetFontObject(C)
        C:CopyFontObject(B)
        late:SetFontObject(C)
    "#,
    )
    .unwrap();
    let own = color_of(&s, "CopyHost:CreateFontString()");
    let red = (crate::color::quant::read32(1.0), 0.0, 0.0);
    assert_ne!(own, red);
    assert_eq!(color_of(&s, "C"), red, "the value is copied");
    assert_eq!(color_of(&s, "early"), own, "nothing is pushed");
    assert_eq!(color_of(&s, "late"), own, "nothing is held");
    s.run("C:SetTextColor(1, 0, 0)").unwrap();
    assert_eq!(color_of(&s, "early"), red);
    assert_eq!(color_of(&s, "late"), red);
}

/// The follow walks the source's chain and drops it when the chain reaches the copy
/// (`0x784207`-`0x784214`): copying a font that follows the copy, one hop down or two, leaves the
/// copy following nothing.
#[test]
fn a_copy_of_its_own_follower_follows_nothing() {
    let s = script();
    chain(&s);
    s.run(r#"Z = CreateFont("ChainRoot") A:SetFontObject(Z) A:CopyFontObject(B)"#)
        .unwrap();
    assert!(s.eval::<bool>("return A:GetFontObject() == nil").unwrap());
    s.run(
        r#"
        A:SetFontObject(Z)
        C = CreateFont("ChainMiddle")
        C:SetFontObject(A)
        B:SetFontObject(C)
        A:CopyFontObject(B)
    "#,
    )
    .unwrap();
    assert!(
        s.eval::<bool>("return A:GetFontObject() == nil").unwrap(),
        "two hops down, the chain still reaches the copy"
    );
}
