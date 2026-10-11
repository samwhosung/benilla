//! The engine half of a hover leave: what the WorldFrame, a nameplate and the Minimap do to the
//! mouseover and to `GameTooltip` inside the hover walk, before the next frame's `OnEnter`.

use super::common::script;
use crate::script::UiScript;

/// A full-screen `UIParent`, a mouse-enabled `Minimap` at (600, 400)–(740, 540), a `GameTooltip`,
/// and a bag button at (100, 100)–(140, 140) whose `OnEnter` records whether the tooltip was up
/// and then takes it.
fn minimap_and_bag() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        UIParent = CreateFrame("Frame", "UIParent"); UIParent:SetAllPoints()
        local map = CreateFrame("Minimap", "Minimap", UIParent)
        map:SetPoint("BOTTOMLEFT", 600, 400); map:SetWidth(140); map:SetHeight(140)
        map:EnableMouse(true)
        CreateFrame("GameTooltip", "GameTooltip", UIParent):Hide()
        Bag = CreateFrame("Button", "Bag", UIParent)
        Bag:SetPoint("BOTTOMLEFT", 100, 100); Bag:SetWidth(40); Bag:SetHeight(40)
        Bag:EnableMouse(true)
        Bag:SetScript("OnEnter", function()
            entered_shown = GameTooltip:IsShown()
            GameTooltip:SetOwner(this, "ANCHOR_RIGHT")
            GameTooltip:SetText("Bag slot")
        end)
        "#,
    )
    .unwrap();
    s.resolve();
    s
}

/// The Minimap's leave (`0x4eb880`, its `vtbl+0x50`) hides `GameTooltip` (`0x4eb88e`–`0x4eb898`)
/// before its base leave, and the walk runs it before the next frame's enter, so a blip's tooltip
/// is gone when the button takes the tooltip, and the button's stays.
#[test]
fn leaving_the_minimap_hides_the_tooltip_before_the_next_enter() {
    let mut s = minimap_and_bag();
    s.mouse_move(670.0, 470.0);
    assert!(s.minimap_tooltip("Stormwind", false));
    s.mouse_move(120.0, 120.0);
    assert!(
        s.eval::<bool>("return entered_shown == nil").unwrap(),
        "the blip's tooltip was hidden before the bag's OnEnter"
    );
    assert!(s
        .eval::<bool>(r#"return GameTooltip:IsShown() == 1 and GameTooltip:IsOwned(Bag) == 1"#)
        .unwrap());
    assert!(s.take_errors().is_empty());
}
