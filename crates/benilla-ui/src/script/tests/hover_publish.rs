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
        CreateFrame("GameTooltip", "GameTooltip", UIParent, "TestTooltipTemplate"):Hide()
        Bag = CreateFrame("Button", "Bag", UIParent)
        Bag:SetPoint("BOTTOMLEFT", 100, 100); Bag:SetWidth(40); Bag:SetHeight(40)
        Bag:EnableMouse(true)
        Bag:SetScript("OnEnter", function()
            entered_visible = GameTooltip:IsVisible()
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
        s.eval::<bool>("return entered_visible == nil").unwrap(),
        "the blip's tooltip was hidden before the bag's OnEnter"
    );
    assert!(s
        .eval::<bool>(r#"return GameTooltip:IsShown() == 1 and GameTooltip:IsOwned(Bag) and true"#)
        .unwrap());
    assert!(s.take_errors().is_empty());
}

/// The WorldFrame's leave (`0x483b10`) runs the base leave first (`0x483b1d`), so its `OnLeave`
/// still finds the mouseover, and then publishes the empty one (`0x483b46`): `"mouseover"` names
/// nobody and the published guid is gone. Its enter is the base one and publishes nothing.
#[test]
fn the_world_frames_leave_runs_its_onleave_then_publishes_the_loss() {
    let mut s = minimap_and_bag();
    s.run(
        r#"
        WorldFrame = CreateFrame("WorldFrame", "WorldFrame"); WorldFrame:SetAllPoints()
        WorldFrame:SetScript("OnLeave", function() left_mouseover = UnitExists("mouseover") end)
        "#,
    )
    .unwrap();
    s.resolve();
    const WOLF: u64 = 0xF130_0000_4500_0001;
    s.mouse_move(400.0, 300.0);
    assert_eq!(
        s.world_mouseover(),
        None,
        "the WorldFrame's enter publishes nothing"
    );
    assert!(s.publish_world_mouseover(Some(WOLF)));
    s.set_unit(
        "mouseover",
        Some(crate::script::UnitState {
            exists: true,
            guid: WOLF,
            ..Default::default()
        }),
    );
    s.mouse_move(120.0, 120.0);
    assert!(s
        .eval::<bool>("return left_mouseover == 1 and UnitExists('mouseover') == nil")
        .unwrap());
    assert_eq!(s.world_mouseover(), None);
    assert!(s.take_errors().is_empty());
}

/// With no capture held, the walk probes the Minimap only in its sweep, and the sweep only when no
/// frame ahead of it took the hit (`0x76620c`): off it, the probe drops the blip and hides the
/// tooltip (`0x4eb12b`–`0x4eb166`), so the same blip shows afresh later; onto a window over it,
/// the probe never runs and the blip stays held, though the Minimap's leave still hides the
/// tooltip. A held capture is probed before the sweep (the next test).
#[test]
fn the_minimap_probe_drops_its_blip_only_when_the_walk_probes_it() {
    let mut s = minimap_and_bag();
    s.run(
        r#"
        Cover = CreateFrame("Frame", "Cover", UIParent)
        Cover:SetPoint("BOTTOMLEFT", 700, 500); Cover:SetWidth(40); Cover:SetHeight(40)
        Cover:SetFrameStrata("DIALOG"); Cover:EnableMouse(true)
        "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_move(670.0, 470.0);
    assert!(s.minimap_tooltip("Stormwind", false));
    s.mouse_move(720.0, 520.0);
    assert_eq!(s.minimap_blip().as_deref(), Some("Stormwind"), "not probed");
    assert!(!s
        .eval::<bool>("return GameTooltip:IsVisible() == 1")
        .unwrap());
    s.mouse_move(670.0, 470.0);
    assert!(s.minimap_tooltip("Stormwind", false));
    s.mouse_move(300.0, 300.0);
    assert_eq!(s.minimap_blip(), None, "probed and missed");
    assert!(!s
        .eval::<bool>("return GameTooltip:IsVisible() == 1")
        .unwrap());
    assert!(s.take_errors().is_empty());
}

/// A window shown under a still cursor takes the hover at the pump's re-pick (`0x7657be` →
/// `0x7660d0`), after the update walk and its drain, and that walk runs the WorldFrame's leave
/// like a moving one: the loss is published before the window's `OnEnter`, in the same tick.
#[test]
fn the_re_pick_onto_a_shown_window_publishes_the_worlds_loss_first() {
    let mut s = minimap_and_bag();
    s.run(
        r#"
        WorldFrame = CreateFrame("WorldFrame", "WorldFrame"); WorldFrame:SetAllPoints()
        Bag:Hide()
        "#,
    )
    .unwrap();
    s.resolve();
    s.mouse_move(120.0, 120.0);
    assert!(s.publish_world_mouseover(Some(7)));
    s.run(r#"GameTooltip:SetOwner(UIParent, "ANCHOR_CURSOR"); GameTooltip:SetText("Wolf")"#)
        .unwrap();
    s.run("Bag:Show()").unwrap();
    s.tick(0.016);
    assert_eq!(s.world_mouseover(), None);
    assert!(s
        .eval::<bool>(r#"return entered_visible == nil and GameTooltip:IsOwned(Bag) and true"#)
        .unwrap());
    assert!(s.take_errors().is_empty());
}

/// Before its sweep the walk probes the capture frame (`[root+0x80]`, `0x7661ba`–`0x7661ca`),
/// unconditionally and for nothing but the probe's own work: a Minimap pressed and dragged off onto
/// a frame that wins the sweep is still probed, misses, and drops its blip (`0x4eb12b`).
#[test]
fn a_captured_minimap_is_probed_though_the_sweep_never_reaches_it() {
    let mut s = minimap_and_bag();
    s.run(r#"Bag:SetFrameStrata("DIALOG")"#).unwrap();
    s.resolve();
    s.mouse_move(670.0, 470.0);
    assert!(s.minimap_tooltip("Stormwind", false));
    s.mouse_button(670.0, 470.0, "LeftButton", true);
    s.mouse_move(120.0, 120.0);
    assert_eq!(s.minimap_blip(), None, "the capture's probe missed");
    s.mouse_button(120.0, 120.0, "LeftButton", false);
    assert!(s.take_errors().is_empty());
}
