//! A Button's three embedded fonts as real fonts (normal `+0x33c`, highlight `+0x3b8`, disabled
//! `+0x434`), the label following the state's one (`0x779810` → `0x770c60`).

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

/// `GameFontNormal`, `GameFontHighlight` declared from it (`Fonts.xml:76`), `GameFontDisable`, and
/// a button `B` of `UIPanelButtonTemplate`'s font shape, labelled "Label", at (100, 100) 100×20.
fn fixture(s: &mut UiScript) {
    s.set_screen_size(800.0, 600.0);
    load(
        s,
        r#"<Ui>
             <Font name="GameFontNormal" font="Fonts\FRIZQT__.TTF" virtual="true">
               <FontHeight><AbsValue val="12"/></FontHeight>
               <Color r="1.0" g="0.82" b="0"/>
             </Font>
             <Font name="GameFontHighlight" inherits="GameFontNormal" virtual="true">
               <Color r="1.0" g="1.0" b="1.0"/>
             </Font>
             <Font name="GameFontDisable" inherits="GameFontNormal" virtual="true">
               <Color r="0.5" g="0.5" b="0.5"/>
             </Font>
             <Button name="B">
               <Size><AbsDimension x="100" y="20"/></Size>
               <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="100" y="100"/></Offset></Anchor></Anchors>
               <NormalFont inherits="GameFontNormal"/>
               <HighlightFont inherits="GameFontHighlight"/>
               <DisabledFont inherits="GameFontDisable"/>
             </Button>
           </Ui>"#,
    );
    s.run(r#"B:SetText("Label")"#).unwrap();
    s.resolve();
}

/// The label's drawn colour.
fn drawn_color(s: &mut UiScript) -> Option<[f32; 4]> {
    s.resolve();
    s.extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(t),
                color,
                ..
            } if t == "Label" => Some(color),
            _ => None,
        })
        .expect("label text quad")
}

fn hover(s: &mut UiScript, on: bool) {
    s.resolve();
    if on {
        s.mouse_move(150.0, 110.0);
    } else {
        s.mouse_move(500.0, 400.0);
    }
}

/// The label follows the state's font, so its own getters answer what it draws: the highlight
/// font while hovered, the disabled font while disabled (`0x779810`).
#[test]
fn the_label_follows_the_states_font() {
    let mut s = script();
    fixture(&mut s);
    let color = |s: &UiScript| {
        s.eval::<(f32, f32, f32)>("return B:GetFontString():GetTextColor()")
            .unwrap()
    };
    assert_eq!(color(&s), (1.0, 0.82, 0.0));
    hover(&mut s, true);
    assert_eq!(color(&s), (1.0, 1.0, 1.0), "hovered: the highlight font");
    assert_eq!(drawn_color(&mut s), Some([1.0, 1.0, 1.0, 1.0]));
    hover(&mut s, false);
    assert_eq!(color(&s), (1.0, 0.82, 0.0));
    s.run("B:Disable()").unwrap();
    assert_eq!(color(&s), (0.5, 0.5, 0.5), "disabled: the disabled font");
}

/// The link and the push obey the label's mask (`0x770800`): a colour the label set itself
/// outlasts the button's.
#[test]
fn a_labels_own_colour_outlasts_the_buttons() {
    let mut s = script();
    fixture(&mut s);
    s.run("B:GetFontString():SetTextColor(1, 0, 0); B:SetTextColor(0, 1, 0)")
        .unwrap();
    assert_eq!(drawn_color(&mut s), Some([1.0, 0.0, 0.0, 1.0]));
    hover(&mut s, true);
    assert_eq!(drawn_color(&mut s), Some([1.0, 0.0, 0.0, 1.0]));
}

/// A state colour with no state font of its own still paints: the label links to that font
/// whatever it holds (`0x77983c`, `0x77985c`).
#[test]
fn a_state_colour_paints_without_a_state_font() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    load(
        &s,
        r#"<Ui>
             <Font name="Plain" font="Fonts\FRIZQT__.TTF">
               <FontHeight><AbsValue val="12"/></FontHeight>
               <Color r="1.0" g="0.82" b="0"/>
             </Font>
             <Button name="B">
               <Size><AbsDimension x="100" y="20"/></Size>
               <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="100" y="100"/></Offset></Anchor></Anchors>
               <NormalFont inherits="Plain"/>
             </Button>
           </Ui>"#,
    );
    s.run(
        r#"
        B:SetText("Label")
        B:SetHighlightTextColor(1, 0, 0)
        B:SetDisabledTextColor(0, 0, 1)
    "#,
    )
    .unwrap();
    hover(&mut s, true);
    assert_eq!(drawn_color(&mut s), Some([1.0, 0.0, 0.0, 1.0]), "hovered");
    hover(&mut s, false);
    assert_eq!(drawn_color(&mut s), Some([1.0, 0.82, 0.0, 1.0]), "rest");
    s.run("B:Disable()").unwrap();
    assert_eq!(drawn_color(&mut s), Some([0.0, 0.0, 1.0, 1.0]), "disabled");
}

/// A restyle of the base font reaches a button's label two hops down: `GameFontNormal` → the
/// normal font → the label.
#[test]
fn restyling_the_base_font_reaches_a_buttons_label() {
    let mut s = script();
    fixture(&mut s);
    s.run(r#"GameFontNormal:SetFont("Fonts\\ARIALN.TTF", 15)"#)
        .unwrap();
    assert_eq!(
        s.eval::<(String, f32)>("local f, h = B:GetFontString():GetFont(); return f, h")
            .unwrap(),
        ("Fonts\\ARIALN.TTF".to_string(), 15.0)
    );
    assert_eq!(
        s.eval::<(String, f32)>("local f, h = B:GetFont(); return f, h")
            .unwrap(),
        ("Fonts\\ARIALN.TTF".to_string(), 15.0)
    );
}

/// The state font getters (`0x7804c0`, `0x7807c0`, `0x780640`) answer the font each follows, and
/// the colour getters (`0x781100`, `0x781800`, `0x781480`) its colour.
#[test]
fn the_state_font_getters_read_the_three_fonts() {
    let mut s = script();
    fixture(&mut s);
    assert!(s
        .eval::<bool>(
            "return B:GetTextFontObject() == GameFontNormal \
             and B:GetHighlightFontObject() == GameFontHighlight \
             and B:GetDisabledFontObject() == GameFontDisable"
        )
        .unwrap());
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return B:GetHighlightTextColor()")
            .unwrap(),
        (1.0, 1.0, 1.0, 1.0)
    );
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return B:GetDisabledTextColor()")
            .unwrap(),
        (0.5, 0.5, 0.5, 1.0)
    );
    s.run(
        r#"
        fresh = CreateFrame("Button")
        B:SetTextFontObject(nil)
    "#,
    )
    .unwrap();
    assert!(s
        .eval::<bool>(
            "return fresh:GetTextFontObject() == nil and fresh:GetHighlightFontObject() == nil \
             and fresh:GetDisabledFontObject() == nil and B:GetTextFontObject() == nil"
        )
        .unwrap());
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return fresh:GetDisabledTextColor()")
            .unwrap(),
        (1.0, 1.0, 1.0, 1.0),
        "the constructor's white"
    );
}

/// The label's draw and its measure read one font (#1403): `Button:SetFont` reaches the measure
/// request, and so does the highlight font while hovered.
#[test]
fn the_label_is_measured_in_the_font_it_draws() {
    let mut s = script();
    fixture(&mut s);
    load(
        &s,
        r#"<Ui><Font name="Tall" font="Fonts\MORPHEUS.TTF">
             <FontHeight><AbsValue val="20"/></FontHeight>
           </Font></Ui>"#,
    );
    let request = |s: &mut UiScript| {
        s.resolve();
        let r = s
            .fontstrings_needing_measure()
            .into_iter()
            .find(|r| r.text == "Label")
            .expect("the label asks for its measure");
        // Answer it, so the next change is what asks again.
        s.set_measured_text_unwrapped(&[(r.id, 40.0, 12.0, r.key)]);
        (r.font, r.height)
    };
    assert_eq!(
        request(&mut s),
        (Some("Fonts\\FRIZQT__.TTF".to_string()), Some(12.0))
    );
    s.run(r#"B:SetFont("Fonts\\ARIALN.TTF", 16)"#).unwrap();
    assert_eq!(
        request(&mut s),
        (Some("Fonts\\ARIALN.TTF".to_string()), Some(16.0))
    );
    // The highlight font's own `SetFont` outlasts the object it now follows.
    s.run("B:SetHighlightFontObject(Tall)").unwrap();
    hover(&mut s, true);
    assert_eq!(
        s.eval::<f32>("local _, h = B:GetFontString():GetFont(); return h")
            .unwrap(),
        16.0
    );
    hover(&mut s, false);
    load(
        &s,
        r#"<Ui><Button name="C">
             <Size><AbsDimension x="100" y="20"/></Size>
             <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="300" y="100"/></Offset></Anchor></Anchors>
             <NormalFont inherits="GameFontNormal"/>
             <HighlightFont inherits="Tall"/>
           </Button></Ui>"#,
    );
    s.run(r#"B:SetText("B"); C:SetText("Label")"#).unwrap();
    s.resolve();
    s.mouse_move(350.0, 110.0);
    assert_eq!(
        request(&mut s),
        (Some("Fonts\\MORPHEUS.TTF".to_string()), Some(20.0)),
        "hovered: measured in the highlight font"
    );
}

/// `<NormalColor>` is the normal font's colour (`0x778a4a`); `<HighlightColor>` and
/// `<DisabledColor>` colour their own fonts (a written deviation: 1.12.1 sends both to the normal
/// colour).
#[test]
fn the_state_colour_elements_colour_their_fonts() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    load(
        &s,
        r#"<Ui><Button name="Tinted">
             <NormalColor r="1" g="0" b="0"/>
             <HighlightColor r="0" g="1" b="0"/>
             <DisabledColor r="0" g="0" b="1" a="0.5"/>
           </Button></Ui>"#,
    );
    let get = |verb: &str| {
        s.eval::<(f32, f32, f32, f32)>(&format!("return Tinted:{verb}()"))
            .unwrap()
    };
    assert_eq!(get("GetTextColor"), (1.0, 0.0, 0.0, 1.0));
    assert_eq!(get("GetHighlightTextColor"), (0.0, 1.0, 0.0, 1.0));
    assert_eq!(get("GetDisabledTextColor"), (0.0, 0.0, 1.0, 0.5));
}

/// A `<ButtonText>` builds its string first, its own `inherits=` linking it, and is installed
/// after (`0x7789d0` → `0x770f40`, then `0x7789d9` → `0x778daa`), so the label follows the normal
/// font and the button's colour reaches it with no hover.
#[test]
fn a_button_text_with_its_own_font_follows_the_normal_font() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    load(
        &s,
        r#"<Ui>
             <Font name="GameFontNormalSmall" font="Fonts\FRIZQT__.TTF">
               <FontHeight><AbsValue val="10"/></FontHeight>
               <Color r="1.0" g="0.82" b="0"/>
             </Font>
             <Font name="GameFontHighlight" font="Fonts\FRIZQT__.TTF">
               <FontHeight><AbsValue val="12"/></FontHeight>
               <Color r="1.0" g="1.0" b="1.0"/>
             </Font>
             <Button name="B">
               <Size><AbsDimension x="100" y="20"/></Size>
               <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="100" y="100"/></Offset></Anchor></Anchors>
               <ButtonText name="$parentText" text="Label" inherits="GameFontNormalSmall"/>
               <NormalFont inherits="GameFontHighlight"/>
             </Button>
           </Ui>"#,
    );
    s.run("B:SetTextColor(1, 0, 0)").unwrap();
    assert_eq!(drawn_color(&mut s), Some([1.0, 0.0, 0.0, 1.0]));
}

/// The hover refresh runs after the base enter and its `OnEnter` (`0x76b6a0`, then `0x7794ab`):
/// the script's normal colour still reaches the label, which then moves to an empty highlight
/// font that changes nothing.
#[test]
fn the_hover_refresh_follows_on_enter() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    load(
        &s,
        r#"<Ui>
             <Font name="Plain" font="Fonts\FRIZQT__.TTF">
               <FontHeight><AbsValue val="12"/></FontHeight>
               <Color r="1.0" g="0.82" b="0"/>
             </Font>
             <Button name="B">
               <Size><AbsDimension x="100" y="20"/></Size>
               <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="100" y="100"/></Offset></Anchor></Anchors>
               <NormalFont inherits="Plain"/>
               <Scripts><OnEnter>this:SetTextColor(1, 1, 1)</OnEnter></Scripts>
             </Button>
           </Ui>"#,
    );
    s.run(r#"B:SetText("Label")"#).unwrap();
    hover(&mut s, true);
    assert_eq!(drawn_color(&mut s), Some([1.0, 1.0, 1.0, 1.0]));
}

/// `SetChecked` moves no state, so it relinks nothing: a label left on its own font by a direct
/// `SetFontObject` stays there.
#[test]
fn set_checked_does_not_relink_the_label() {
    let mut s = script();
    fixture(&mut s);
    load(
        &s,
        r#"<Ui><CheckButton name="Box">
             <NormalFont inherits="GameFontNormal"/>
           </CheckButton></Ui>"#,
    );
    s.run(
        r#"
        Box:SetText("x")
        Box:GetFontString():SetFontObject(GameFontDisable)
        Box:SetChecked(1)
    "#,
    )
    .unwrap();
    assert!(s
        .eval::<bool>("return Box:GetFontString():GetFontObject() == GameFontDisable")
        .unwrap());
}
