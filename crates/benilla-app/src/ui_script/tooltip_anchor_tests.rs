//! Where each hover seats the tooltip, over the stock files. World, unit-frame and action-bar
//! hovers use `GameTooltip_SetDefaultAnchor` (`GameTooltip.lua:73-77`): the screen's bottom-right
//! corner, `-CONTAINER_OFFSET_X - 13` in from the right and `CONTAINER_OFFSET_Y` up.

use benilla_ui::script::{AuraState, UiScript, UnitState};

use super::test_ui::load_ui as load_xml;

/// A 1024x768 screen with fonts, `UIParent` and `GameTooltip`, plus `extra`. The kit seeds
/// `CONTAINER_OFFSET_X/Y` with the stock 0 and 70 (`ContainerFrame.lua:11-12`), so with no bar
/// raised the default corner is x = 1024 - 13 = 1011, y = 70.
fn harness(extra: &[&str]) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    // `FACTION_BAR_COLORS`, which `GameTooltip_UnitColor` indexes on every unit hover
    // (`ReputationFrame.lua:3`).
    load_xml(&s, r"Interface\FrameXML\ReputationFrame.lua");
    for f in extra {
        load_xml(&s, f);
    }
    // The stock tooltip sizes from its lines, so its rect needs a text measurer. Detailed tips
    // default on in 1.12 (`UIOptionsFrame.lua:100`); this kit loads no options window.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    s.run("SHOW_NEWBIE_TIPS = \"1\"").unwrap();
    s
}

fn wolf() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Timber Wolf".into()),
        health: 30,
        max_health: 50,
        level: 10,
        reaction: 2,
        creature_type_name: Some("Beast".into()),
        ..Default::default()
    }
}

/// `UIParent` is a named, full-screen frame (`UIParent.xml:5`).
#[test]
fn uiparent_is_a_real_full_screen_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    s.resolve();
    let ok: bool = s
        .eval(
            "return UIParent ~= nil and UIParent:GetName() == \"UIParent\" \
               and UIParent:GetLeft() == 0 and UIParent:GetBottom() == 0 \
               and UIParent:GetRight() == 1024 and UIParent:GetTop() == 768",
        )
        .unwrap();
    assert!(ok, "UIParent exists and fills the screen");
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// A world hover seats the plate at the default corner: the engine fires
/// `OnTooltipSetDefaultAnchor` (`GameTooltipTemplate.xml:617-619`), which anchors BOTTOMRIGHT to
/// `UIParent` at (-13, 70).
#[test]
fn world_hover_seats_the_default_corner() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    s.set_unit("mouseover", Some(wolf()));
    assert!(s.world_tooltip_unit("mouseover"), "the hover shows");
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    s.resolve();
    let ok: bool = s
        .eval(
            "return GameTooltip:IsVisible() \
               and GameTooltip:GetRight() == 1011 and GameTooltip:GetBottom() == 70",
        )
        .unwrap();
    assert!(ok, "world tooltip sits at the screen's bottom-right corner");
}

/// Unit-frame hovers take the default corner too (`UnitFrame.lua:56`). With detailed tips on, the
/// 1.12 default, leaving hides the plate at once; `FadeOut` is the tips-off arm
/// (`UnitFrame.lua:84-88`).
#[test]
fn unit_frame_hover_takes_the_default_corner_and_drops_on_leave() {
    benilla_formats::wow_data_or_skip!();
    // The dropdown kit and popups precede the unit frames, whose dropdowns initialize at OnLoad.
    let mut s = harness(&[
        // The unit frames read GlobalStrings at load, and `UnitFrame_OnEnter` passes
        // `PARTY_OPTIONS_LABEL` to `SetText`, which raises on nil.
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        "Interface\\FrameXML\\PetFrame.xml",
    ]);
    s.set_unit("target", Some(wolf()));
    // `UnitFrame_OnEnter` takes no arguments and reads `this` (`UnitFrame.lua:47`), as the stock
    // `<OnEnter>` calls it (`TargetFrame.xml:505`).
    s.run("this = TargetFrame UnitFrame_OnEnter() this = nil")
        .unwrap();
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    s.resolve();
    let ok: bool = s
        .eval(
            "return GameTooltip:IsVisible() \
               and GameTooltip:GetRight() == 1011 and GameTooltip:GetBottom() == 70 \
               and GameTooltip:IsOwned(TargetFrame)",
        )
        .unwrap();
    assert!(
        ok,
        "unit-frame tooltip sits at the default corner, owned by the frame"
    );
    // A wolf is not player-controlled, so the hover is the ordinary unit readout.
    assert!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap()
            .contains("Wolf"),
        "a non-player target still gets the unit lines"
    );
    s.run("this = TargetFrame UnitFrame_OnLeave() this = nil")
        .unwrap();
    let hidden: bool = s.eval("return not GameTooltip:IsShown()").unwrap();
    assert!(
        hidden,
        "unit-frame tooltip hides on leave, it does not fade"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// While the creature query is in flight a world hover is titled `UNKNOWNOBJECT`, read from the
/// chain's GlobalStrings (`0x703bf0`) so a translated install translates it; the answer then
/// replaces it.
#[test]
fn a_pending_name_hover_titles_the_chains_unknownobject() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness(&["Interface\\FrameXML\\GlobalStrings.lua"]);
    // Before `SMSG_CREATURE_QUERY_RESPONSE`: the descriptor is in, the name and type word are not.
    s.set_unit(
        "mouseover",
        Some(UnitState {
            name: None,
            creature_type_name: None,
            ..wolf()
        }),
    );
    assert!(s.world_tooltip_unit("mouseover"), "the hover shows");
    let global = s.eval::<String>("return UNKNOWNOBJECT").unwrap();
    assert!(
        !global.is_empty(),
        "the chain's GlobalStrings.lua defines UNKNOWNOBJECT"
    );
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        global,
        "a name in flight titles the plate with the GlobalString, not an empty line"
    );

    s.set_unit("mouseover", Some(wolf()));
    assert!(s.world_tooltip_unit("mouseover"));
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Timber Wolf"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// With detailed tips on, a unit frame explains its right-click menu and returns before `SetUnit`
/// (`UnitFrame.lua:58-66`): your own portrait always, and any other frame but a party member's
/// while the target is another player-controlled unit.
#[test]
fn your_own_portrait_explains_the_menu_instead_of_showing_your_health() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[
        // The unit frames read GlobalStrings at load, and `UnitFrame_OnEnter` passes
        // `PARTY_OPTIONS_LABEL` to `SetText`, which raises on nil.
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        "Interface\\FrameXML\\PetFrame.xml",
    ]);
    s.set_unit("player", Some(wolf()));

    s.run("this = PlayerFrame UnitFrame_OnEnter() this = nil")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Party Options",
        "your own frame explains the party menu"
    );
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft2:GetText()")
            .unwrap(),
        s.eval::<String>("return NEWBIE_TOOLTIP_PARTYOPTIONS")
            .unwrap()
    );
    assert_eq!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap(),
        2,
        "it RETURNS before SetUnit — no health/level lines underneath"
    );

    // A player target takes the other arm, which reads `UnitPlayerControlled("target")` alone
    // (`UnitFrame.lua:62`), never the hovered frame's unit: `player` stays a wolf.
    s.set_unit(
        "target",
        Some(UnitState {
            is_player: true,
            player_controlled: true,
            name: Some("Someone".into()),
            ..wolf()
        }),
    );
    s.run("this = TargetFrame UnitFrame_OnEnter() this = nil")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Player Options",
        "another player's frame explains the player menu"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Action-bar hovers take the default corner: `ActionButton_SetTooltip` branches on `UberTooltips`
/// (`ActionButton.lua:366-372`), whose stock default is "1" (`0x48fdd9`, string `0x82e748`). An
/// empty slot draws nothing, so the anchor is read through `GetPoint`.
#[test]
fn action_button_hover_takes_the_default_corner() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(&[
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
    ]);
    s.register_cvars(crate::cvars::registered_pairs());
    s.run("this = ActionButton3 ActionButton_SetTooltip()")
        .unwrap();
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    let ok: bool = s
        .eval(
            "local p, rel, rp, x, y = GameTooltip:GetPoint() \
             return p == \"BOTTOMRIGHT\" and rel ~= nil and rel:GetName() == \"UIParent\" \
               and rp == \"BOTTOMRIGHT\" and x == -13 and y == 70",
        )
        .unwrap();
    assert!(
        ok,
        "action-button hover anchors the plate to UIParent's bottom-right"
    );
}

/// With `UberTooltips` off an action button's plate seats beside it: `ANCHOR_LEFT` under
/// `MultiBarBottomRight`, `MultiBarRight` and `MultiBarLeft`, `ANCHOR_RIGHT` elsewhere
/// (`ActionButton.lua:369-373`). `SetOwner` writes `ANCHOR_RIGHT` as the plate's BOTTOMLEFT on the
/// button's TOPRIGHT, and `ANCHOR_LEFT` as its mirror.
#[test]
fn ubertooltips_off_seats_action_bar_plates_beside_the_button() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
        "Interface\\FrameXML\\MultiActionBars.xml",
    ]);
    s.register_cvars(crate::cvars::registered_pairs());
    s.set_cvar_engine("UberTooltips", "0");

    let seat = |s: &UiScript| {
        s.eval::<String>(
            "local p, rel, rp = GameTooltip:GetPoint() \
             return p .. \"/\" .. (rel and rel:GetName() or \"?\") .. \"/\" .. rp",
        )
        .unwrap()
    };

    s.run("this = ActionButton3 ActionButton_SetTooltip()")
        .unwrap();
    assert!(
        s.eval::<bool>("return GameTooltip.default == nil").unwrap(),
        "off: the main bar's plate is owner-anchored, not the default corner"
    );
    assert!(
        s.eval::<bool>("return GameTooltip:IsOwned(ActionButton3)")
            .unwrap(),
        "…owned by the button it opened from"
    );
    assert_eq!(
        seat(&s),
        "BOTTOMLEFT/ActionButton3/TOPRIGHT",
        "ANCHOR_RIGHT — the main bar is not in the ref's LEFT set"
    );

    // The LEFT set is by parent frame, not visibility: the vertical bars are hidden here.
    for bar in ["MultiBarBottomRight", "MultiBarRight", "MultiBarLeft"] {
        s.run(&format!("this = {bar}Button1 ActionButton_SetTooltip()"))
            .unwrap();
        assert_eq!(
            seat(&s),
            format!("BOTTOMRIGHT/{bar}Button1/TOPLEFT"),
            "ANCHOR_LEFT — {bar} opens toward screen centre"
        );
    }

    s.set_cvar_engine("UberTooltips", "1");
    s.run("this = MultiBarBottomRightButton1 ActionButton_SetTooltip()")
        .unwrap();
    assert!(
        s.eval::<bool>("return GameTooltip.default ~= nil").unwrap(),
        "on: back to the screen corner"
    );
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
}

/// The stance buttons' own branch (`BonusActionBarFrame.xml:40-45`): the corner when
/// `UberTooltips` is on, `ANCHOR_RIGHT` when off.
#[test]
fn ubertooltips_off_seats_stance_plates_beside_the_button() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
    ]);
    s.register_cvars(crate::cvars::registered_pairs());
    s.set_shapeshift_forms(vec![benilla_ui::script::ShapeshiftFormView {
        spell_id: 5487,
        name: "Bear Form".into(),
        texture: Some("Interface\\Icons\\Ability_Racial_BearForm".into()),
        active: false,
        castable: true,
        cooldown: None,
    }]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    s.resolve();

    s.run("this = ShapeshiftButton1 ShapeshiftButton1:GetScript(\"OnEnter\")()")
        .unwrap();
    assert!(
        s.eval::<bool>("return GameTooltip.default ~= nil").unwrap(),
        "on (the stock default): the screen corner"
    );

    s.set_cvar_engine("UberTooltips", "0");
    s.run("this = ShapeshiftButton1 ShapeshiftButton1:GetScript(\"OnEnter\")()")
        .unwrap();
    assert_eq!(
        s.eval::<String>(
            "local p, rel, rp = GameTooltip:GetPoint() \
             return p .. \"/\" .. (rel and rel:GetName() or \"?\") .. \"/\" .. rp",
        )
        .unwrap(),
        "BOTTOMLEFT/ShapeshiftButton1/TOPRIGHT",
        "off: ANCHOR_RIGHT, beside the button"
    );
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
}

/// Buff hovers hang below the button: `ANCHOR_BOTTOMLEFT` (`BuffFrame.xml:37`) seats the plate's
/// TOPRIGHT on the button's BOTTOMLEFT.
#[test]
fn buff_hover_hangs_below_left_of_the_button() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
    ]);
    s.set_auras(
        "player",
        Some(vec![AuraState {
            spell_id: 1459,
            name: Some("Arcane Intellect".into()),
            icon: Some("Interface\\Icons\\Spell_Holy_MagicalSentry".into()),
            count: 1,
            debuff_type: None,
            duration: 1800.0,
            expiration_time: 1800.0,
            helpful: true,
            cancelable: true,
            until_cancelled: false,
            channeled: false,
        }]),
    );
    // The event the stock buff buttons register (`BuffFrame.lua:113`).
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    s.resolve();
    // The template's inline `<OnEnter>`, called directly: the engine sets `this` only when it
    // fires a handler (`0x704d50`), so it is set by hand.
    s.run("this = BuffButton0; BuffButton0:GetScript(\"OnEnter\")(BuffButton0)")
        .unwrap();
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    let ok: bool = s
        .eval(
            "local p, rel, rp = GameTooltip:GetPoint() \
             return p == \"TOPRIGHT\" and rel ~= nil and rel:GetName() == \"BuffButton0\" \
               and rp == \"BOTTOMLEFT\"",
        )
        .unwrap();
    assert!(
        ok,
        "buff tooltip hangs its TOPRIGHT on the button's BOTTOMLEFT"
    );
}

/// The cursor-seated GameObject plate is owned: the reference's publisher sets the owner through
/// the SetOwner core (`0x492a01` calls `0x52ffe0(owner, 6, 0, 0)`, whose `0x53000c` writes
/// `+0x314`). The cursor arm fires no `OnTooltipSetDefaultAnchor`, so no Lua sets it.
#[test]
fn a_cursor_seated_gameobject_plate_is_owned() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    assert!(s.world_tooltip_gameobject("Brill", &[], Some((512.0, 384.0))));
    let owned: bool = s.eval("return GameTooltip:IsOwned(UIParent)").unwrap();
    assert!(
        owned,
        "the signpost plate is owned; errors: {:?}",
        s.errors()
    );
}

/// An addon `OnShow` hook that calls `GameTooltip:Show()` (as `!Questie`'s does) leaves the plate
/// up: `:Show()` is the existence gate `0x530a80`, which hides a plate with no owner or no lines
/// (`0x530a60`), and the publisher sets the owner before the plate shows.
#[test]
fn a_cursor_seated_gameobject_plate_survives_an_addons_on_show_hook() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    // Questie's hook, in one line: the plate's own show event calls Show() again.
    s.run(r#"GameTooltip:SetScript("OnShow", function() GameTooltip:Show() end)"#)
        .unwrap();
    assert!(s.world_tooltip_gameobject("Brill", &[], Some((512.0, 384.0))));
    let shown: bool = s
        .eval("return GameTooltip:IsShown() and true or false")
        .unwrap();
    assert!(
        shown,
        "the signpost plate is still up; errors: {:?}",
        s.errors()
    );
}

/// The corner arm (`0x492a42`) writes no owner itself: its `+0x444` handler,
/// `OnTooltipSetDefaultAnchor`, sets it through `GameTooltip_SetDefaultAnchor`'s
/// `SetOwner(UIParent, "ANCHOR_NONE")`. Every hover starts unowned, since `Tooltip::Hide`
/// (`0x530a60`) is `SetOwner(NULL, 0, 0, 0)`.
#[test]
fn a_corner_seated_gameobject_plate_is_owned_and_shown() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    assert!(s.world_tooltip_gameobject("Ironforge Main Gate", &[], None));
    let owned: bool = s.eval("return GameTooltip:IsOwned(UIParent)").unwrap();
    assert!(owned, "the corner plate is owned; errors: {:?}", s.errors());
    let shown: bool = s
        .eval("return GameTooltip:IsShown() and true or false")
        .unwrap();
    assert!(shown, "and it is on screen; errors: {:?}", s.errors());
}

/// The corner plate survives the same `OnShow` hook through the existence gate `0x530a80`.
#[test]
fn a_corner_seated_gameobject_plate_survives_an_addons_on_show_hook() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    s.run(r#"GameTooltip:SetScript("OnShow", function() GameTooltip:Show() end)"#)
        .unwrap();
    assert!(s.world_tooltip_gameobject("Ironforge Main Gate", &[], None));
    let shown: bool = s
        .eval("return GameTooltip:IsShown() and true or false")
        .unwrap();
    assert!(
        shown,
        "the corner plate is still up; errors: {:?}",
        s.errors()
    );
}
