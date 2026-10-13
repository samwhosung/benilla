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
    set_link(&mut s, "CF");
    s
}

fn set_link(s: &mut UiScript, owner: &str) {
    let owner = s.model_ref().arena.lookup(owner).expect("owner");
    s.set_link_spans(vec![(
        owner,
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

/// The take `0x7a3880` is called from the ScrollingMessageFrame and SimpleHTML updates alone; the
/// MessageFrame's update (`0x786200`) reaches neither, so a span on one builds no button and the
/// pointer over it passes through.
#[test]
fn a_message_frame_line_gets_no_link_button() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        EF = CreateFrame("MessageFrame", "EF")
        EF:SetPoint("BOTTOMLEFT", 100, 100); EF:SetWidth(430); EF:SetHeight(120)
        "#,
    )
    .unwrap();
    s.resolve();
    set_link(&mut s, "EF");
    assert_eq!(s.eval::<i64>("return EF:GetNumChildren()").unwrap(), 0);
    s.mouse_move(130.0, 127.0);
    assert!(s.eval::<bool>("return GetMouseFocus() == nil").unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

fn set_spans_to(s: &mut UiScript, link: &str) {
    let cf = s.model_ref().arena.lookup("CF").expect("CF");
    s.set_link_spans(vec![(
        cf,
        Rect::new(120.0, 110.0, 134.0, 160.0),
        link.to_string(),
        format!("|H{link}|h[x]|h"),
    )]);
}

/// A rebuild (`0x788c00`) hides every live button first (`0x7a3920`), so the hovered one leaves
/// with the link it entered with, and the button that takes the new span enters afresh. The same
/// spans again rebuild nothing.
#[test]
fn a_changed_span_list_leaves_the_hovered_link_before_the_new_one_enters() {
    let mut s = chat();
    s.run(
        r#"
        CF:SetScript("OnHyperlinkEnter", function() table.insert(log, "enter:"..arg1) end)
        CF:SetScript("OnHyperlinkLeave", function() table.insert(log, "leave:"..arg1) end)
        "#,
    )
    .unwrap();
    s.mouse_move(130.0, 127.0);
    set_link(&mut s, "CF");
    assert_eq!(
        log_of(&s),
        format!("enter:{LINK}"),
        "unchanged spans: no rebuild"
    );
    set_spans_to(&mut s, "item:2589");
    s.mouse_move(131.0, 127.0);
    s.mouse_move(300.0, 150.0);
    assert_eq!(
        log_of(&s),
        format!("enter:{LINK} leave:{LINK} enter:item:2589 leave:item:2589")
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A press held over a link whose span list changes before the release loses its capture with
/// the hide, so the release clicks nothing.
#[test]
fn a_rebuild_under_a_held_press_drops_the_click() {
    let mut s = chat();
    s.mouse_move(130.0, 127.0);
    s.mouse_button(130.0, 127.0, "LeftButton", true);
    set_spans_to(&mut s, "item:2589");
    s.mouse_button(130.0, 127.0, "LeftButton", false);
    assert_eq!(log_of(&s), "");
    // The next press and release click the new link.
    click(&mut s, (130.0, 127.0), "LeftButton");
    assert_eq!(log_of(&s), "click:item:2589#|Hitem:2589|h[x]|h#LeftButton");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
