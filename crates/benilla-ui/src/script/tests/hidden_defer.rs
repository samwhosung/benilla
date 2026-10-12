//! A hidden frame's layout waits. Hiding sets the layout object's deferral bit, on the frame and
//! its regions (`0x76ad50` → `0x768060(1)`; regions `0x77fc60`), and a load sets it too
//! (`0x769770`), which the load's end clears only for a visible frame (`0x76a2f0`). While it is set
//! an invalidate marks the object pending and passes the invalidate to what anchors to it, but
//! queues no resolve (`0x768169`–`0x76819c`), so `ApplyRect` and its `OnSizeChanged` (`0x76b580`)
//! wait for one of three things: the show, which resolves the frame before its regions and
//! children show (`0x76ae58` → `0x768060(0)` → `0x7680e0(1)`); a getter, which resolves it and
//! leaves the bit set (`0x7a1980`, `0x7a1e00`); or an anchor read by an object that resolves, which
//! clears the bit for good (`0x7a2fb8`–`0x7a2fc0`).

use super::common::script;
use crate::script::UiScript;

fn xml(s: &UiScript, text: &str) {
    let doc = crate::framexml::parse(text).expect("valid FrameXML");
    let report = crate::loader::load(s, &doc, &|_: &str| None);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
}

fn int(s: &UiScript, expr: &str) -> i64 {
    s.eval::<i64>(&format!("return {expr}")).unwrap()
}

fn num(s: &UiScript, expr: &str) -> f64 {
    s.eval::<f64>(&format!("return {expr}")).unwrap()
}

/// Size changes made while hidden fire nothing at the drain; the show fires once, with the last
/// size, inside `Show` and ahead of the frame's own `OnShow`.
#[test]
fn a_size_change_while_hidden_fires_once_inside_show() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        fires, log = 0, {}
        DeferPanel = CreateFrame("Frame", "DeferPanel")
        DeferPanel:SetPoint("BOTTOMLEFT", 10, 10)
        DeferPanel:SetWidth(100); DeferPanel:SetHeight(50)
    "#,
    )
    .unwrap();
    s.resolve();
    s.run(
        r#"
        DeferPanel:Hide()
        DeferPanel:SetScript("OnSizeChanged", function()
            fires = fires + 1; last_w = arg1; table.insert(log, "size")
        end)
        DeferPanel:SetScript("OnShow", function() table.insert(log, "show") end)
        DeferPanel:SetWidth(20)
    "#,
    )
    .unwrap();
    s.resolve();
    s.run("DeferPanel:SetWidth(30)").unwrap();
    s.resolve();
    s.resolve();
    assert_eq!(int(&s, "fires"), 0, "a hidden frame's resolve waits");

    s.run("DeferPanel:Show(); fires_in_show = fires").unwrap();
    assert_eq!(
        int(&s, "fires_in_show"),
        1,
        "the show resolves it, inside Show"
    );
    assert_eq!(num(&s, "last_w"), 30.0, "once, with the last size");
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "size,show",
        "the show resolves the frame before it is shown and notified"
    );
    s.resolve();
    assert_eq!(int(&s, "fires"), 1, "and the drain owes nothing more");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A `hidden="true"` frame is deferred from its load on, so its `OnSizeChanged` waits for its
/// show; a shown one fires at its first resolve.
#[test]
fn a_hidden_xml_frame_does_not_fire_on_size_changed_until_shown() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    xml(
        &s,
        r#"<Ui>
            <Frame name="DeferXmlHidden" hidden="true">
                <Size><AbsDimension x="100" y="40"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <Scripts><OnSizeChanged>hidden_fires = (hidden_fires or 0) + 1</OnSizeChanged></Scripts>
            </Frame>
            <Frame name="DeferXmlShown">
                <Size><AbsDimension x="100" y="40"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <Scripts><OnSizeChanged>shown_fires = (shown_fires or 0) + 1</OnSizeChanged></Scripts>
            </Frame>
        </Ui>"#,
    );
    for _ in 0..3 {
        s.resolve();
    }
    assert_eq!(int(&s, "shown_fires or 0"), 1, "the shown frame fires");
    assert_eq!(int(&s, "hidden_fires or 0"), 0, "the hidden one does not");

    s.run("DeferXmlHidden:Show(); in_show = hidden_fires or 0")
        .unwrap();
    assert_eq!(int(&s, "in_show"), 1, "its show resolves it");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A getter resolves a hidden frame on read and answers the new size, but leaves it deferred: a
/// later write it does not read waits again.
#[test]
fn a_getter_on_a_hidden_frame_answers_the_new_size_and_leaves_it_deferred() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        fires = 0
        DeferRead = CreateFrame("Frame", "DeferRead")
        DeferRead:SetPoint("BOTTOMLEFT", 10, 10)
        DeferRead:SetWidth(100); DeferRead:SetHeight(50)
        DeferRead:SetScript("OnSizeChanged", function() fires = fires + 1 end)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(int(&s, "fires"), 1);

    s.run("DeferRead:Hide(); DeferRead:SetWidth(20)").unwrap();
    assert_eq!(num(&s, "DeferRead:GetRight() - DeferRead:GetLeft()"), 20.0);
    assert_eq!(num(&s, "DeferRead:GetWidth()"), 20.0);
    s.resolve();
    assert_eq!(
        int(&s, "fires"),
        2,
        "the read resolved it, so its size change fires"
    );

    s.run("DeferRead:SetWidth(40)").unwrap();
    s.resolve();
    s.resolve();
    assert_eq!(
        int(&s, "fires"),
        2,
        "unread, the next write waits: the read left it deferred"
    );
    s.run("DeferRead:Show()").unwrap();
    assert_eq!(int(&s, "fires"), 3);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A visible frame anchored to a hidden one is invalidated through it, and its resolve reads the
/// hidden frame's anchor, which resolves that frame and clears its deferral. A hidden frame no
/// visible one reads still waits, and a chain of hidden frames resolves through to the reader.
#[test]
fn a_visible_frame_anchored_to_a_hidden_one_resolves_through_it() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        read_fires, unread_fires, link_fires = 0, 0, 0
        DeferTarget = CreateFrame("Frame", "DeferTarget")
        DeferTarget:SetPoint("BOTTOMLEFT", 10, 10)
        DeferTarget:SetWidth(100); DeferTarget:SetHeight(50)
        DeferReader = CreateFrame("Frame", "DeferReader")
        DeferReader:SetPoint("BOTTOMLEFT", DeferTarget, "BOTTOMRIGHT", 5, 0)
        DeferReader:SetWidth(10); DeferReader:SetHeight(10)

        DeferUnread = CreateFrame("Frame", "DeferUnread")
        DeferUnread:SetPoint("TOPLEFT", 10, -10)
        DeferUnread:SetWidth(100); DeferUnread:SetHeight(50)

        -- A chain: a visible reader of a hidden link that reads a hidden root.
        DeferRoot = CreateFrame("Frame", "DeferRoot")
        DeferRoot:SetPoint("TOPRIGHT", -10, -10)
        DeferRoot:SetWidth(100); DeferRoot:SetHeight(50)
        DeferLink = CreateFrame("Frame", "DeferLink")
        DeferLink:SetPoint("TOPRIGHT", DeferRoot, "TOPLEFT", 0, 0)
        DeferLink:SetWidth(20); DeferLink:SetHeight(20)
        DeferTail = CreateFrame("Frame", "DeferTail")
        DeferTail:SetPoint("TOPRIGHT", DeferLink, "TOPLEFT", 0, 0)
        DeferTail:SetWidth(10); DeferTail:SetHeight(10)
    "#,
    )
    .unwrap();
    s.resolve();
    s.run(
        r#"
        DeferTarget:Hide(); DeferUnread:Hide(); DeferRoot:Hide(); DeferLink:Hide()
        DeferTarget:SetScript("OnSizeChanged", function() read_fires = read_fires + 1 end)
        DeferUnread:SetScript("OnSizeChanged", function() unread_fires = unread_fires + 1 end)
        DeferLink:SetScript("OnSizeChanged", function() link_fires = link_fires + 1 end)
        DeferTarget:SetWidth(200)
        DeferUnread:SetWidth(200)
        DeferRoot:SetWidth(300)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        int(&s, "read_fires"),
        1,
        "the reader's anchor read resolved it"
    );
    assert_eq!(int(&s, "unread_fires"), 0, "nothing read this one");
    assert_eq!(int(&s, "link_fires"), 0, "the link only moved");

    s.run(
        r#"
        reader_left = DeferReader:GetLeft(); target_right = DeferTarget:GetRight()
        tail_right = DeferTail:GetRight(); root_left = DeferRoot:GetLeft()
    "#,
    )
    .unwrap();
    assert_eq!(num(&s, "reader_left"), num(&s, "target_right") + 5.0);
    assert_eq!(num(&s, "root_left - DeferRoot:GetRight()"), -300.0);
    assert_eq!(
        num(&s, "tail_right"),
        num(&s, "root_left") - 20.0,
        "the tail reads through the hidden link to the hidden root"
    );

    // The read cleared the target's deferral for good: hidden, it now resolves at the drain.
    s.run("DeferTarget:SetWidth(250)").unwrap();
    s.resolve();
    assert_eq!(int(&s, "read_fires"), 2);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A hidden frame's regions are deferred with it: a region read resolves the region and, through
/// its anchor, the frame; and the show resolves both.
#[test]
fn a_region_of_a_hidden_frame_waits_with_it() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        fires = 0
        DeferOwner = CreateFrame("Frame", "DeferOwner")
        DeferOwner:SetPoint("BOTTOMLEFT", 10, 10)
        DeferOwner:SetWidth(100); DeferOwner:SetHeight(50)
        DeferArt = DeferOwner:CreateTexture(nil, "ARTWORK")
        DeferArt:SetPoint("TOPLEFT", DeferOwner, "TOPLEFT", 0, 0)
        DeferArt:SetPoint("BOTTOMRIGHT", DeferOwner, "BOTTOMRIGHT", 0, 0)
        DeferOwner:SetScript("OnSizeChanged", function() fires = fires + 1 end)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(int(&s, "fires"), 1);
    assert_eq!(num(&s, "DeferArt:GetRight() - DeferArt:GetLeft()"), 100.0);

    s.run("DeferOwner:Hide(); DeferOwner:SetWidth(60)").unwrap();
    s.resolve();
    assert_eq!(int(&s, "fires"), 1, "the frame waits");
    assert_eq!(num(&s, "DeferArt:GetRight() - DeferArt:GetLeft()"), 60.0);
    s.resolve();
    assert_eq!(
        int(&s, "fires"),
        2,
        "the region's read resolved its anchor target, the frame"
    );

    s.run("DeferOwner:SetWidth(80)").unwrap();
    s.resolve();
    assert_eq!(
        int(&s, "fires"),
        3,
        "that anchor read cleared the frame's deferral for good"
    );
    s.run("DeferOwner:Show()").unwrap();
    s.resolve();
    assert_eq!(num(&s, "DeferArt:GetRight() - DeferArt:GetLeft()"), 80.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
