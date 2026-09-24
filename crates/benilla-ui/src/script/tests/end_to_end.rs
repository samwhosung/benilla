//! End-to-end: build a two-frame anchored tree with textures, extract in ZKey order.

use super::common::script;
use crate::layout::Rect;
use crate::script::*;

#[test]
fn end_to_end_two_frame_tree_extracts_in_zkey_order() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        -- A parent frame with a background texture, and a child frame (lower in insertion order,
        -- same strata/level) with a texture and a fontstring. ~30 lines of ordinary FrameXML Lua.
        local parent = CreateFrame("Frame", "Root")
        parent:SetPoint("TOPLEFT", 0, 0)          -- anchored to the screen root
        parent:SetWidth(400); parent:SetHeight(300)
        -- SetAllPoints on each region: a templateless Lua region gets NO implicit anchor
        -- (decision 1310 — rect-less, never drawn), so real addon code anchors it, and so do we.
        local pbg = parent:CreateTexture(nil, "BACKGROUND")
        pbg:SetTexture("Interface\\Parent.blp")
        pbg:SetAllPoints()

        local child = CreateFrame("Frame", "Leaf", parent)
        child:SetPoint("TOPLEFT", parent, "TOPLEFT", 10, -10)
        child:SetWidth(100); child:SetHeight(50)
        local cbg = child:CreateTexture(nil, "ARTWORK")
        cbg:SetTexture("Interface\\Child.blp")
        cbg:SetAllPoints()
        local ctext = child:CreateFontString(nil, "OVERLAY")
        ctext:SetText("Hello")
        ctext:SetVertexColor(1, 0, 0, 1)
        ctext:SetAllPoints()
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();

    // Painter order: the parent draws before the child (earlier insertion, same strata and level),
    // each frame's regions with it.
    let content: Vec<String> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(format!("tex:{p}")),
            QuadContent::Text { text: Some(t), .. } => Some(format!("txt:{t}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        content,
        vec![
            "tex:Interface\\Parent.blp",
            "tex:Interface\\Child.blp",
            "txt:Hello",
        ]
    );

    // `SetAllPoints` gives each region its frame's rect: the parent Rect(300, 0, 600, 400), the
    // child (TOPLEFT +10, -10 of the parent, 100×50) Rect(540, 10, 590, 110).
    let child_text = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Hello"))
        .unwrap();
    assert_eq!(child_text.rect, Some(Rect::new(540.0, 10.0, 590.0, 110.0)));
    assert!(
        matches!(&child_text.content, QuadContent::Text { color: Some(c), .. } if *c == [1.0, 0.0, 0.0, 1.0])
    );

    let parent_bg = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.ends_with("Parent.blp")))
        .unwrap();
    assert_eq!(parent_bg.rect, Some(Rect::new(300.0, 0.0, 600.0, 400.0)));

    let zs: Vec<u64> = quads.iter().map(|q| q.z).collect();
    let mut sorted = zs.clone();
    sorted.sort_unstable();
    assert_eq!(zs, sorted, "extract must already be in ZKey order");
}
