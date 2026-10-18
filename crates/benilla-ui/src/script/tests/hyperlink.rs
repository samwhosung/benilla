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
    crate::assert_near!(
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
    crate::assert_near!(s.eval::<f64>("return CF:GetLeft()").unwrap(), 150.0);
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

fn set_spans_at(s: &mut UiScript, link: &str, dx: f32) {
    let cf = s.model_ref().arena.lookup("CF").expect("CF");
    s.set_link_spans(vec![(
        cf,
        Rect::new(120.0, 110.0 + dx, 134.0, 160.0 + dx),
        link.to_string(),
        format!("|H{link}|h[x]|h"),
    )]);
}

fn log_hover(s: &mut UiScript) {
    s.run(
        r#"
        CF:SetScript("OnHyperlinkEnter", function() table.insert(log, "enter:"..arg1) end)
        CF:SetScript("OnHyperlinkLeave", function() table.insert(log, "leave:"..arg1) end)
        "#,
    )
    .unwrap();
}

/// The rebuild (`0x788c00`) compares nothing: a relayout with the same spans, here a scroll down
/// refused at the bottom (`0x788650` → `0x788b80`, flag at `0x788bf1`), still hides the hovered
/// button (`0x7a3920`), which leaves, and the re-placed one enters on the next pick. The span
/// arriving with no flag up rebuilds nothing.
#[test]
fn a_relayout_with_the_same_spans_leaves_and_re_enters_the_hovered_link() {
    let mut s = chat();
    log_hover(&mut s);
    // A scroll on an empty frame answers 0 at once (`0x788659`), so the frame holds a line.
    s.run("CF:AddMessage('x')").unwrap();
    set_link(&mut s, "CF");
    s.mouse_move(130.0, 127.0);
    set_link(&mut s, "CF");
    assert_eq!(log_of(&s), format!("enter:{LINK}"), "no flag: no rebuild");
    s.run("CF:ScrollDown()").unwrap();
    set_link(&mut s, "CF");
    s.mouse_move(131.0, 127.0);
    assert_eq!(
        log_of(&s),
        format!("enter:{LINK} leave:{LINK} enter:{LINK}")
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A new line's relayout under a hovered link: the leave carries the link it entered with, and
/// the button that takes the new span enters afresh.
#[test]
fn a_new_line_leaves_the_hovered_link_with_its_old_link() {
    let mut s = chat();
    log_hover(&mut s);
    s.mouse_move(130.0, 127.0);
    s.run("CF:AddMessage('x')").unwrap();
    set_spans_at(&mut s, "item:2589", 0.0);
    s.mouse_move(131.0, 127.0);
    s.mouse_move(300.0, 150.0);
    assert_eq!(
        log_of(&s),
        format!("enter:{LINK} leave:{LINK} enter:item:2589 leave:item:2589")
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A frame that only moves carries its buttons along (`0x7a3441` anchors them to the line): no
/// leave, no enter, and the button under the cursor is the same one.
#[test]
fn a_moved_frame_carries_its_buttons_with_no_leave_or_enter() {
    let mut s = chat();
    log_hover(&mut s);
    s.mouse_move(130.0, 127.0);
    let before = s.eval::<mlua::Value>("return GetMouseFocus()").unwrap();
    set_spans_at(&mut s, LINK, 5.0);
    s.mouse_move(131.0, 127.0);
    assert_eq!(log_of(&s), format!("enter:{LINK}"));
    let after = s.eval::<mlua::Value>("return GetMouseFocus()").unwrap();
    assert_eq!(before, after, "the same button");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A rescale changes the frame's on-screen size, which its rect callback compares
/// (`0x788394`), so it relayouts and rebuilds: the hovered link leaves and enters again, though
/// its span is where it was.
#[test]
fn a_rescaled_frame_rebuilds_its_buttons() {
    let mut s = chat();
    log_hover(&mut s);
    s.mouse_move(130.0, 127.0);
    s.run("CF:SetScale(0.9)").unwrap();
    s.resolve();
    set_link(&mut s, "CF");
    s.mouse_move(131.0, 127.0);
    assert_eq!(
        log_of(&s),
        format!("enter:{LINK} leave:{LINK} enter:{LINK}")
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A press held over a link through a rebuild loses its capture with the hide, so the release
/// clicks nothing; the next press and release click the new link.
#[test]
fn a_rebuild_under_a_held_press_drops_the_click() {
    let mut s = chat();
    s.mouse_move(130.0, 127.0);
    s.mouse_button(130.0, 127.0, "LeftButton", true);
    s.run("CF:AddMessage('x')").unwrap();
    set_spans_at(&mut s, "item:2589", 0.0);
    s.mouse_button(130.0, 127.0, "LeftButton", false);
    assert_eq!(log_of(&s), "");
    click(&mut s, (130.0, 127.0), "LeftButton");
    assert_eq!(log_of(&s), "click:item:2589#|Hitem:2589|h[x]|h#LeftButton");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A SimpleHTML `SH` at (100, 100) 430×120 on 800×600 holding one `<A href>`, logging its link
/// events, with its one span fed over [110..160]×[120..134].
fn html() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r##"
        log = {}
        SH = CreateFrame("SimpleHTML", "SH")
        SH:SetPoint("BOTTOMLEFT", 100, 100); SH:SetWidth(430); SH:SetHeight(120)
        SH:SetFont("Fonts\FRIZQT__.TTF", 12)
        SH:SetText('<html><body><p><a href="u">t</a></p></body></html>')
        SH:SetScript("OnHyperlinkClick", function()
            table.insert(log, "click:"..arg1.."#"..arg2.."#"..arg3)
        end)
        SH:SetScript("OnHyperlinkEnter", function() table.insert(log, "enter:"..arg1) end)
        SH:SetScript("OnHyperlinkLeave", function() table.insert(log, "leave:"..arg1) end)
        "##,
    )
    .unwrap();
    s.resolve();
    set_html_link(&mut s);
    s
}

fn set_html_link(s: &mut UiScript) {
    let owner = s.model_ref().arena.lookup("SH").expect("SH");
    s.set_link_spans(vec![(
        owner,
        Rect::new(120.0, 110.0, 134.0, 160.0),
        "u".to_string(),
        "|Hu|ht|h".to_string(),
    )]);
}

/// A SimpleHTML's link takes a button from the pool (`0x78b052` → `0x7a3880`), whose click fires
/// the frame's `OnHyperlinkClick(link, text, button)`, as the stock `ServerAlertText` relies on
/// (GlueXML `AccountLogin.xml`).
#[test]
fn a_simple_html_link_fires_on_hyperlink_click() {
    let mut s = html();
    assert_eq!(s.eval::<i64>("return SH:GetNumChildren()").unwrap(), 1);
    click(&mut s, (130.0, 127.0), "LeftButton");
    assert_eq!(log_of(&s), "enter:u click:u#|Hu|ht|h#LeftButton");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Only `SetText` raises a SimpleHTML's rebuild flag (`0x78a50d`): a resize re-seats the hovered
/// button with no leave, where a new `SetText` rebuilds and leaves it.
#[test]
fn a_simple_html_rebuilds_its_links_on_set_text_alone() {
    let mut s = html();
    s.mouse_move(130.0, 127.0);
    s.run("SH:SetWidth(400)").unwrap();
    s.resolve();
    set_html_link(&mut s);
    s.mouse_move(131.0, 127.0);
    assert_eq!(log_of(&s), "enter:u", "a resize rebuilds nothing");
    s.run(r#"SH:SetText('<html><body><p><a href="u">t</a></p></body></html>')"#)
        .unwrap();
    set_html_link(&mut s);
    s.mouse_move(130.0, 127.0);
    assert_eq!(log_of(&s), "enter:u leave:u enter:u");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The text the extract hands the app names its link owner for a SimpleHTML block and a scrolling
/// message line, and none for a plain FontString or a MessageFrame line, whose links stay text.
#[test]
fn only_simple_html_blocks_and_scrolling_lines_name_a_link_owner() {
    let mut s = html();
    s.run(
        r#"
        CF = CreateFrame("ScrollingMessageFrame", "CF")
        CF:SetPoint("TOPLEFT", 0, 0); CF:SetWidth(200); CF:SetHeight(60)
        CF:AddMessage("|Hitem:1|h[a]|h", 1, 1, 1)
        EF = CreateFrame("MessageFrame", "EF")
        EF:SetPoint("TOPRIGHT", 0, 0); EF:SetWidth(200); EF:SetHeight(60)
        EF:AddMessage("|Hitem:2|h[b]|h", 1, 1, 1)
        local p = CreateFrame("Frame", "P")
        p:SetPoint("CENTER", 0, 0); p:SetWidth(200); p:SetHeight(60)
        local fs = p:CreateFontString("PFS", "ARTWORK")
        fs:SetFont("Fonts\FRIZQT__.TTF", 12)
        fs:SetAllPoints(p); fs:SetText("|Hitem:3|h[c]|h")
        "#,
    )
    .unwrap();
    s.resolve();
    let owner_of = |s: &UiScript, needle: &str| {
        s.extract().into_iter().find_map(|q| match q.content {
            crate::script::QuadContent::Text {
                text: Some(t),
                link_owner,
                ..
            } if t.contains(needle) => Some(link_owner),
            _ => None,
        })
    };
    let lookup = |s: &UiScript, name: &str| s.model_ref().arena.lookup(name);
    assert_eq!(owner_of(&s, "|Hu|h"), Some(lookup(&s, "SH")));
    assert_eq!(owner_of(&s, "item:1"), Some(lookup(&s, "CF")));
    assert_eq!(owner_of(&s, "item:2"), Some(None));
    assert_eq!(owner_of(&s, "item:3"), Some(None));
}
