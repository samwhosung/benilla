//! A text frame's hyperlink span is a frame of its own, a `CSimpleHyperlinkButton` (`0x7a3240`):
//! a mouse-enabled Button child that takes the hover, the press and the click, and hands its
//! enter, leave and click to the text frame's `OnHyperlinkEnter`/`OnHyperlinkLeave`/
//! `OnHyperlinkClick`.

use super::common::script;
use crate::layout::Rect;
use crate::script::UiScript;

const LINK: &str = "item:7073";
const MARKUP: &str = "|Hitem:7073|h[Broken Fang]|h";

/// A chat-like `CF` as `FloatingChatFrameTemplate` builds it (movable, toplevel, a full-frame title
/// region, no mouse) at (100, 100) 430×120 on 800×600, with one link span over [110..160]×[120..134].
fn chat() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r##"
        log = {}
        CF = CreateFrame("ScrollingMessageFrame", "CF")
        CF:SetToplevel(true); CF:SetMovable(true)
        CF:SetPoint("BOTTOMLEFT", 100, 100); CF:SetWidth(430); CF:SetHeight(120)
        CF:CreateTitleRegion():SetAllPoints(CF)
        CF:SetScript("OnHyperlinkClick", function()
            table.insert(log, "click:"..arg1.."#"..arg2.."#"..arg3)
        end)
        "##,
    )
    .unwrap();
    s.resolve();
    set_link(&mut s);
    s
}

fn set_link(s: &mut UiScript) {
    let cf = s.model_ref().arena.lookup("CF").expect("CF");
    s.set_link_spans(vec![(
        cf,
        Rect::new(120.0, 110.0, 134.0, 160.0),
        LINK.to_string(),
        MARKUP.to_string(),
    )]);
}

fn log_of(s: &UiScript) -> String {
    s.eval::<String>("return table.concat(log, ' ')").unwrap()
}

fn click(s: &mut UiScript, (x, y): (f32, f32), button: &str) {
    s.mouse_move(x, y);
    s.mouse_button(x, y, button, true);
    s.mouse_button(x, y, button, false);
}

/// The press lands on the link's button, whose title-region test finds none, so the chat window
/// is neither pinned nor user-placed; the button's click (`0x7a3510`) fires the window's
/// `OnHyperlinkClick(link, text, button)` (`0x7a37e0`), for either button (`0x500`, `0x7a327d`).
#[test]
fn a_link_click_fires_on_hyperlink_click_and_leaves_the_window_alone() {
    let mut s = chat();
    click(&mut s, (130.0, 127.0), "LeftButton");
    click(&mut s, (130.0, 127.0), "RightButton");
    assert_eq!(
        log_of(&s),
        format!("click:{LINK}#{MARKUP}#LeftButton click:{LINK}#{MARKUP}#RightButton")
    );
    assert!(!s.eval::<bool>("return CF:IsUserPlaced()").unwrap());
    let (point, x, y) = s
        .eval::<(String, f64, f64)>("local p, _, _, x, y = CF:GetPoint() return p, x, y")
        .unwrap();
    assert_eq!(
        (point.as_str(), x, y),
        ("BOTTOMLEFT", 100.0, 100.0),
        "never pinned"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The hover over a link is the link's button, a child of the window, not the window.
#[test]
fn the_mouse_focus_over_a_link_is_its_button() {
    let mut s = chat();
    s.mouse_move(130.0, 127.0);
    assert!(s
        .eval::<bool>(
            "local f = GetMouseFocus() return f ~= nil and f ~= CF and f:GetParent() == CF"
        )
        .unwrap());
    assert!(s
        .eval::<bool>("return GetMouseFocus():GetObjectType() == 'Button'")
        .unwrap());
    s.mouse_move(300.0, 150.0);
    assert!(
        s.eval::<bool>("return GetMouseFocus() == nil").unwrap(),
        "the body takes no mouse"
    );
}

/// The button's enter and leave (`0x7a34b0`/`0x7a34e0`) fire the window's `OnHyperlinkEnter` and
/// `OnHyperlinkLeave` with the link alone (`"%s"`, `0x7a37a0`); a span that goes away while hovered
/// hides its button, whose removal leaves.
#[test]
fn hovering_a_link_fires_on_hyperlink_enter_and_leave() {
    let mut s = chat();
    s.run(
        r#"
        CF:SetScript("OnHyperlinkEnter", function() table.insert(log, "enter:"..arg1) end)
        CF:SetScript("OnHyperlinkLeave", function() table.insert(log, "leave:"..arg1) end)
        "#,
    )
    .unwrap();
    s.mouse_move(130.0, 127.0);
    s.mouse_move(300.0, 150.0);
    s.mouse_move(130.0, 127.0);
    s.set_link_spans(Vec::new());
    assert_eq!(
        log_of(&s),
        format!("enter:{LINK} leave:{LINK} enter:{LINK} leave:{LINK}")
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// With the body made hittable, a press on it outside a link title-drags the window, and a press
/// on the link, a child a level above it, still clicks the link.
#[test]
fn a_hittable_body_title_drags_but_its_link_still_clicks() {
    let mut s = chat();
    s.run("CF:EnableMouse(true)").unwrap();
    s.resolve();
    click(&mut s, (130.0, 127.0), "LeftButton");
    assert_eq!(log_of(&s), format!("click:{LINK}#{MARKUP}#LeftButton"));
    assert!(!s.eval::<bool>("return CF:IsUserPlaced()").unwrap());
    s.mouse_move(300.0, 150.0);
    s.mouse_button(300.0, 150.0, "LeftButton", true);
    s.mouse_move(350.0, 170.0);
    s.mouse_button(350.0, 170.0, "LeftButton", false);
    s.resolve();
    assert_eq!(s.eval::<f64>("return CF:GetLeft()").unwrap(), 150.0);
    assert!(s.eval::<bool>("return CF:IsUserPlaced()").unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
