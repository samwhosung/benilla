//! The world mouseover's loss against the UI hover walk: the driver and the walk together, as a
//! frame runs them.

use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::tests::{eval, hover, hover_unit, mouseover_app, WOLF};
use crate::target::{Hovered, HoveredObject};

/// A full-screen `UIParent` and `WorldFrame`, a `GameTooltip` whose default anchor seats it in
/// `anchor`, and a bag button at (100, 100)–(140, 140) whose `OnEnter` records what it found and
/// takes the tooltip, as a stock slot does.
fn world_and_bag(app: &mut App, anchor: &str) {
    let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
    script.set_screen_size(800.0, 600.0);
    crate::ui_script::test_ui::load_tooltip_template(&script);
    script
        .run(&format!(
            r#"
            UIParent = CreateFrame("Frame", "UIParent"); UIParent:SetAllPoints()
            WorldFrame = CreateFrame("WorldFrame", "WorldFrame"); WorldFrame:SetAllPoints()
            local tt = CreateFrame("GameTooltip", "GameTooltip", UIParent, "TestTooltipTemplate"); tt:Hide()
            tt:SetScript("OnTooltipSetDefaultAnchor", function()
                GameTooltip:SetOwner(UIParent, "{anchor}")
            end)
            Bag = CreateFrame("Button", "Bag", UIParent)
            Bag:SetPoint("BOTTOMLEFT", 100, 100); Bag:SetWidth(40); Bag:SetHeight(40)
            Bag:EnableMouse(true)
            Bag:SetScript("OnEnter", function()
                entered_mouseover = UnitExists("mouseover")
                entered_visible = GameTooltip:IsVisible()
                GameTooltip:SetOwner(this, "ANCHOR_RIGHT")
                GameTooltip:SetText("Bag slot")
            end)
            "#
        ))
        .unwrap();
    script.resolve();
    // The cursor on the world: the WorldFrame holds the hover.
    script.mouse_move(400.0, 300.0);
}

fn script(app: &mut App) -> Mut<'_, UiScript> {
    app.world_mut().non_send_resource_mut::<UiScript>()
}

/// From a world unit onto a window: the walk runs the WorldFrame's leave before the button's
/// enter (`0x766229`, then `0x766239`), and that leave publishes the empty mouseover
/// (`0x483b10` → `0x483b46`), whose change requests the fade (`0x492909` → `0x530ae0`). So the
/// button's `OnEnter` already finds `"mouseover"` naming nobody and a cursor-seated plate gone,
/// and its own tooltip, taken after, is not faded by the hover loss.
#[test]
fn a_window_entered_from_a_world_unit_keeps_its_tooltip() {
    let (mut app, wolf, _) = mouseover_app();
    world_and_bag(&mut app, "ANCHOR_CURSOR");
    hover_unit(&mut app, wolf, WOLF);
    assert_eq!(
        eval(&mut app, "return GameTooltip:IsShown()"),
        Some(1),
        "the world plate is up"
    );

    // This frame's walk moves onto the bag; the world pick then stands down over the UI.
    script(&mut app).mouse_move(120.0, 120.0);
    assert_eq!(
        eval(&mut app, "return entered_mouseover"),
        None,
        "the WorldFrame's leave cleared the mouseover before the button's enter"
    );
    assert_eq!(
        eval(&mut app, "return entered_visible"),
        None,
        "the fade request hid the cursor-seated plate before the button's enter"
    );
    hover(&mut app, Hovered::default(), HoveredObject::default());
    script(&mut app).tick(2.5);
    assert_eq!(
        eval(
            &mut app,
            r#"return GameTooltip:IsShown() and GameTooltip:IsOwned(Bag)
                and GameTooltipTextLeft1:GetText() == "Bag slot"
                and GameTooltip:GetAlpha() == 1 and 1"#
        ),
        Some(1),
        "the button's tooltip stays at full alpha"
    );
    assert!(script(&mut app).take_errors().is_empty());
}

/// With the cursor on the world, every mouseover change requests the fade on `GameTooltip`
/// (`0x492909`), whoever holds it: an addon's tooltip shown over the world fades with the hover.
#[test]
fn an_addon_tooltip_over_the_world_fades_when_the_mouseover_changes() {
    let (mut app, wolf, _) = mouseover_app();
    world_and_bag(&mut app, "ANCHOR_NONE");
    hover_unit(&mut app, wolf, WOLF);
    script(&mut app)
        .run(r#"GameTooltip:SetOwner(UIParent, "ANCHOR_NONE"); GameTooltip:SetText("Addon")"#)
        .unwrap();
    hover(&mut app, Hovered::default(), HoveredObject::default());
    script(&mut app).tick(2.5);
    assert_eq!(
        eval(&mut app, "return GameTooltip:IsShown()"),
        None,
        "the addon's tooltip faded with the hover"
    );
    assert!(script(&mut app).take_errors().is_empty());
}

/// A hovered plate retired by its unit leaving takes the plate's leave (`0x764cce` → `0x7cb8b0`),
/// which publishes the empty mouseover and fades the plate's tooltip; the hover then re-picks to
/// the WorldFrame. The pick taken before the retire still names the plate's unit, and must not
/// publish it again: no second build, no second `UPDATE_MOUSEOVER_UNIT`, no `"mouseover"`.
#[test]
fn a_retired_hovered_plate_is_not_published_again() {
    use benilla_ui::script::{PlateGeometry, PlateState};

    let (mut app, wolf, _) = mouseover_app();
    world_and_bag(&mut app, "ANCHOR_CURSOR");
    let geometry = PlateGeometry {
        width: 128.0,
        height: 32.0,
        bar_off_x: 4.0,
        bar_off_y: 4.0,
        bar_width: 103.0,
        bar_height: 9.0,
        level_off_x: 11.8,
        level_off_y: 9.1,
        skull_size: 12.8,
        raid_size: 25.6,
        name_height: 12.8,
        level_height: 11.0,
        shadow_offset: 1.0,
    };
    let plate = PlateState {
        key: WOLF,
        top_centre: (400.0, 400.0),
        health: 30.0,
        max_health: 40.0,
        bar_colour: [1.0, 0.0, 0.0],
        name: "Wolf".into(),
        level: Some(12),
        skull: false,
        level_colour: [1.0, 1.0, 0.0],
        raid_icon: None,
        alpha: 255,
        lit: false,
        hovered: false,
    };
    {
        let mut s = script(&mut app);
        s.run(
            r#"
            builds, updates = 0, 0
            GameTooltip:SetScript("OnShow", function() builds = builds + 1 end)
            GameTooltip:RegisterEvent("UPDATE_MOUSEOVER_UNIT")
            GameTooltip:SetScript("OnEvent", function() updates = updates + 1 end)
            "#,
        )
        .unwrap();
        s.sync_nameplates(geometry, std::slice::from_ref(&plate));
        s.resolve();
        s.mouse_move(400.0, 384.0);
        assert_eq!(
            s.hovered_nameplate(),
            Some(WOLF),
            "the plate holds the hover"
        );
    }
    // The plate's pick, as the pick takes it off this frame's walk.
    let on_plate = Hovered {
        target: Some(wolf),
        guid: Some(WOLF),
        distance: 0.0,
        plate: true,
        ..Default::default()
    };
    hover(&mut app, on_plate, HoveredObject::default());
    assert_eq!(eval(&mut app, "return builds"), Some(1));
    // The plate driver retires it after the pick, in the same frame.
    script(&mut app).sync_nameplates(geometry, &[]);
    assert_eq!(
        eval(&mut app, "return GameTooltip:IsVisible()"),
        None,
        "faded"
    );
    // The next frame's driver still reads that pick.
    hover(&mut app, on_plate, HoveredObject::default());
    hover(&mut app, Hovered::default(), HoveredObject::default());
    assert_eq!(
        eval(
            &mut app,
            "return builds == 1 and updates == 1 and UnitExists('mouseover') == nil \
             and not GameTooltip:IsVisible() and 1 or nil"
        ),
        Some(1),
        "built once, announced once, never named again"
    );
    assert!(script(&mut app).take_errors().is_empty());
}
