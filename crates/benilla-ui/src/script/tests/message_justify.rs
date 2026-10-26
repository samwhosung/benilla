//! A message frame's justify is one word on its font, as a Font object's is: the font
//! constructor writes `0x212` (`0x783a87`), CENTER and MIDDLE, which neither frame's constructor
//! changes (`0x78778f`, `0x785700`); `SetJustifyH` and `SetJustifyV` replace their own axis of it
//! (`0x79fc57`-`0x79fc64`, `0x79fd1d`); the getters read it (`0x79fcb0`, `0x79fd70`); and each line
//! follows the frame's font (`0x7888fd`).

use super::common::script;
use crate::script::{JustifyH, QuadContent, UiScript};

fn load(s: &UiScript, xml: &str) {
    let doc = crate::framexml::parse(xml).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

/// Where the one line `text` draws, horizontally.
fn drawn(s: &mut UiScript, text: &str) -> JustifyH {
    s.resolve();
    s.extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                justify_h,
                ..
            } if t == text => Some(justify_h),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{text:?} draws"))
}

fn answers(s: &UiScript, frame: &str) -> (String, String) {
    s.eval(&format!(
        "return {frame}:GetJustifyH(), {frame}:GetJustifyV()"
    ))
    .unwrap()
}

/// A Lua-created frame has the font constructor's word: it answers CENTER and MIDDLE and centres
/// its lines.
#[test]
fn a_lua_created_message_frame_is_centred() {
    let mut s = script();
    s.run(
        "for i, kind in ipairs({'MessageFrame', 'ScrollingMessageFrame'}) do\n\
           local f = CreateFrame(kind, 'MJ'..i)\n\
           f:SetWidth(300) f:SetHeight(80) f:SetPoint('CENTER', 0, 0)\n\
           f:AddMessage('line'..i)\n\
         end",
    )
    .unwrap();
    for (i, frame) in ["MJ1", "MJ2"].into_iter().enumerate() {
        assert_eq!(
            answers(&s, frame),
            ("CENTER".to_string(), "MIDDLE".to_string()),
            "{frame}"
        );
        assert_eq!(drawn(&mut s, &format!("line{}", i + 1)), JustifyH::Center);
    }
}

/// The setters replace their own axis of the font's word, and the lines follow it: on a chat
/// frame (LEFT and MIDDLE from its `<FontString justifyH>`), `SetJustifyV("TOP")` keeps LEFT, and
/// a token of the other axis erases the axis it is given to.
#[test]
fn the_setters_replace_one_axis_of_the_fonts_word() {
    let mut s = script();
    load(
        &s,
        r#"<Ui>
             <Font name="MJChatFont" font="Fonts\ARIALN.TTF" virtual="true">
               <FontHeight><AbsValue val="14"/></FontHeight>
             </Font>
             <ScrollingMessageFrame name="MJChat">
               <Size><AbsDimension x="300" y="80"/></Size>
               <Anchors><Anchor point="CENTER"/></Anchors>
               <FontString inherits="MJChatFont" justifyH="LEFT"/>
             </ScrollingMessageFrame>
           </Ui>"#,
    );
    s.run("MJChat:AddMessage('chat')").unwrap();
    assert_eq!(
        answers(&s, "MJChat"),
        ("LEFT".to_string(), "MIDDLE".to_string())
    );
    assert_eq!(drawn(&mut s, "chat"), JustifyH::Left);

    s.run("MJChat:SetJustifyV('TOP')").unwrap();
    assert_eq!(
        answers(&s, "MJChat"),
        ("LEFT".to_string(), "TOP".to_string()),
        "SetJustifyV leaves the horizontal axis alone"
    );
    assert_eq!(drawn(&mut s, "chat"), JustifyH::Left);

    s.run("MJChat:SetJustifyH('RIGHT')").unwrap();
    assert_eq!(
        answers(&s, "MJChat"),
        ("RIGHT".to_string(), "TOP".to_string())
    );
    assert_eq!(drawn(&mut s, "chat"), JustifyH::Right);

    s.run("MJChat:SetJustifyH('BOTTOM')").unwrap();
    assert_eq!(
        answers(&s, "MJChat"),
        ("UNKNOWN".to_string(), "TOP".to_string()),
        "a vertical token erases the horizontal axis"
    );
    assert_eq!(
        drawn(&mut s, "chat"),
        JustifyH::Center,
        "and an erased axis draws centred"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The word is the font's own: until a setter writes it the frame follows the Font object it was
/// given, and a setter's write stops the following for both axes (`0x79fc7c`, as a FontString's
/// does), so a later change to the object no longer reaches it.
#[test]
fn a_set_word_stops_following_the_font_object() {
    let s = script();
    s.run(
        "fo = CreateFont('MJFont')\n\
         fo:SetFont('Fonts\\\\FRIZQT__.TTF', 14) fo:SetJustifyH('LEFT') fo:SetJustifyV('TOP')\n\
         f = CreateFrame('MessageFrame', 'MJFollow')\n\
         f:SetFontObject(fo)",
    )
    .unwrap();
    assert_eq!(
        answers(&s, "MJFollow"),
        ("LEFT".to_string(), "TOP".to_string())
    );
    s.run("fo:SetJustifyH('RIGHT')").unwrap();
    assert_eq!(
        answers(&s, "MJFollow"),
        ("RIGHT".to_string(), "TOP".to_string()),
        "still following"
    );
    s.run("MJFollow:SetJustifyV('BOTTOM') fo:SetJustifyH('CENTER') fo:SetJustifyV('MIDDLE')")
        .unwrap();
    assert_eq!(
        answers(&s, "MJFollow"),
        ("RIGHT".to_string(), "BOTTOM".to_string())
    );
}

/// The setters raise the usage line for an unknown token, before the font exists.
#[test]
fn an_unknown_justify_token_raises_the_usage_line() {
    let s = script();
    s.run("CreateFrame('MessageFrame', 'MJUsage')").unwrap();
    let err = s
        .run("MJUsage:SetJustifyH('SIDEWAYS')")
        .expect_err("raises")
        .to_string();
    assert!(
        err.contains("Usage: MJUsage:SetJustifyH(\"justify\")"),
        "{err}"
    );
}
