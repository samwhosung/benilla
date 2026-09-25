//! The four extra bars (MultiActionBars.xml) and the stance bar (BonusActionBarFrame.xml), end to
//! end. The bars start down, so most tests first raise theirs through the Options rows' globals.

use benilla_ui::script::{ActionSlot, QuadContent, ScriptValue, SpellTooltipView, UiScript};

use super::test_ui::load_ui as load_xml;

/// The main bar's stock files, with UIParent.xml for the manage pass the stance bar's OnShow runs
/// and GameTooltip.xml for the OnLeave a bar hiding under the cursor fires.
fn load_action_bar(s: &UiScript) {
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        // `ExhaustionTick_Update` reads `ReputationWatchBar`, which ReputationFrame.xml declares.
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
        // `MultiActionBarFrame_OnLoad` writes `UIOptionsFrameCheckButtons`, so UIOptionsFrame.xml
        // loads first, as in FrameXML.toc (l.21, l.39); it also declares `ALWAYS_SHOW_MULTIBARS`.
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        r"Interface\FrameXML\OptionsFrame.lua",
        r"Interface\FrameXML\UIOptionsFrame.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "ScrollTemplates.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
    ] {
        load_xml(s, file);
    }
}

/// Raise exactly the bars named, through the globals the Options rows write.
fn show_bars(s: &UiScript, bars: &[u32]) {
    let mut lua = String::new();
    for bar in 1..=4u32 {
        lua.push_str(&format!(
            "SHOW_MULTI_ACTIONBAR_{bar} = {} ",
            if bars.contains(&bar) { "1" } else { "nil" }
        ));
    }
    // …then the manage pass, which the options row runs after a toggle (UIOptionsFrame.xml:666)
    // and `MultiActionBar_Update` does not.
    lua.push_str("MultiActionBar_Update() UIParent_ManageFramePositions()");
    s.run(&lua).unwrap();
}

/// The two bottom bars, raised: fixed pages (BottomLeft 61-72, BottomRight 49-60, from
/// `ActionButton_GetPagedID`'s parent-name fork, ActionButton.lua:455-466), empty wells hidden
/// unless a payload is held, and ids no bonus page moves.
#[test]
fn shipped_multibars_drive_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    let frames = super::test_ui::load_ui(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    assert_eq!(
        frames, 100,
        "what stock MultiActionBars.xml declares: the four bar frames and their 48 buttons, \
         each with a $parentCooldown — the same 100 ours built for the same seats"
    );

    // Occupy main slot 1, BottomLeft slot 1 (action 61), BottomRight slot 1 (action 49).
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Main".into()),
            kind: 0x00,
            action: 100,
            count: 0,
            consumable: false,
        }),
    );
    s.set_action(
        61,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_BL".into()),
            kind: 0x00,
            action: 200,
            count: 0,
            consumable: false,
        }),
    );
    s.set_action(
        49,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_BR".into()),
            kind: 0x00,
            action: 300,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    show_bars(&s, &[1, 2]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let icon = |path: &str| {
        quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
            .and_then(|q| q.rect)
    };

    // Main button 1 spans x[8,44] y[4,40], and BottomLeft's BOTTOMLEFT is that button's TOPLEFT
    // + (0, 17): its button 1 spans x[8,44] y[57,93]. BottomRight's LEFT is BottomLeft's
    // (500-wide) RIGHT + (10, 0), so its left is 518 (MultiActionBars.xml:496-513).
    let bl = icon("Interface\\Icons\\Spell_BL").expect("BottomLeft button 1 icon");
    assert_eq!(
        (bl.left, bl.bottom, bl.right, bl.top),
        (8.0, 57.0, 44.0, 93.0)
    );
    let br = icon("Interface\\Icons\\Spell_BR").expect("BottomRight button 1 icon");
    assert_eq!(
        (br.left, br.bottom, br.right, br.top),
        (518.0, 57.0, 554.0, 93.0)
    );

    let rings = |quads: &[benilla_ui::script::ExtractedQuad], path: &str| {
        quads
            .iter()
            .filter(
                |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path),
            )
            .count()
    };
    assert_eq!(
        rings(&quads, "Interface\\Buttons\\UI-Quickslot2"),
        3,
        "the occupied main slot + the 2 occupied multibar buttons; every empty well is hidden, \
         the main bar's included (ActionButton.lua:69-70)"
    );

    // BottomLeft button 1 (centre (26, 75)) queues its fixed id 61, bonus page or not.
    s.mouse_button(26.0, 75.0, "LeftButton", true);
    s.mouse_button(26.0, 75.0, "LeftButton", false);
    assert_eq!(
        s.take_action_uses()
            .into_iter()
            .map(|u| u.action)
            .collect::<Vec<_>>(),
        vec![61]
    );
    s.set_bonus_bar_offset(1);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    s.mouse_button(26.0, 75.0, "LeftButton", true);
    s.mouse_button(26.0, 75.0, "LeftButton", false);
    assert_eq!(
        s.take_action_uses()
            .into_iter()
            .map(|u| u.action)
            .collect::<Vec<_>>(),
        vec![61],
        "a bonus page never re-pages a multibar"
    );
    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    // Let the bonus overlay finish its descent (one OnUpdate past 0.15 s), or its 12 wells answer
    // the SHOWGRID below.
    s.tick(0.2);
    s.tick(0.01);

    // While a payload is held every empty well shows the `UI-Quickslot` ring: 11 main, 22 multibar.
    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    s.resolve();
    assert_eq!(
        rings(&s.extract(), "Interface\\Buttons\\UI-Quickslot"),
        33,
        "grid shows every empty well as a drop target"
    );
    s.fire_event("ACTIONBAR_HIDEGRID", vec![]);
    s.resolve();
    assert_eq!(rings(&s.extract(), "Interface\\Buttons\\UI-Quickslot"), 0);
    assert_eq!(
        rings(&s.extract(), "Interface\\Buttons\\UI-Quickslot2"),
        3,
        "letting go hides every empty well again, the main bar's included; the three occupied \
         buttons keep their rings"
    );

    // An unbound button's hotkey text is `RANGE_INDICATOR` while `IsActionInRange` is non-nil, set
    // on PLAYER_TARGET_CHANGED (ActionButton.lua:121-144, 332-334); `ActionButton_OnUpdate` shows
    // it only out of range (l.389-395) and tints it red on the range recheck (l.416-428).
    use benilla_ui::script::ActionState;
    s.set_action_state(
        61,
        Some(ActionState {
            usable: true,
            in_range: Some(false),
            has_range: true,
            ..Default::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.tick(0.5); // past the 0.2 s range recheck
    s.resolve();
    let dot_shown = |quads: &[benilla_ui::script::ExtractedQuad]| {
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "●"))
    };
    assert!(dot_shown(&s.extract()), "out of range shows the red dot");
    s.set_action_state(
        61,
        Some(ActionState {
            usable: true,
            in_range: Some(true),
            has_range: true,
            ..Default::default()
        }),
    );
    s.tick(0.5);
    s.resolve();
    assert!(!dot_shown(&s.extract()), "back in range clears the dot");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The stance bar (`ShapeshiftBarFrame`) over the reference's form list (`0xb71100`): hidden at
/// zero forms, the active form checked, a non-castable one greyed, a click queuing its spell.
#[test]
fn shipped_stance_bar_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // `ShapeshiftBarFrame` is parented to `MainMenuBar` and its update calls
    // `CooldownFrame_SetTimer` (BonusActionBarFrame.lua:201), so both load first.
    load_action_bar(&s);
    // Bar 1 up: the stance bar's OnShow runs the manage pass (BonusActionBarFrame.xml:350-352),
    // whose `ShapeshiftBarFrame` row adds 45 only while bar 1 is up (UIParent.lua:1583, 1605).
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    show_bars(&s, &[1]);
    assert!(
        s.eval::<bool>("return ShapeshiftBarFrame ~= nil and ShapeshiftButton10 ~= nil")
            .unwrap(),
        "the stance bar's frames come with the stock bonus-bar file"
    );

    // No forms pushed (a mage): the frame is hidden.
    assert!(!s
        .eval::<bool>("return ShapeshiftBarFrame:IsShown()")
        .unwrap());

    // A warrior's two stances: battle active, defensive known but not castable.
    s.set_shapeshift_forms(vec![
        ShapeshiftFormView {
            spell_id: 2457,
            texture: Some("Interface\\Icons\\Stance_A".into()),
            name: "Battle Stance".into(),
            active: true,
            castable: true,
            cooldown: None,
        },
        ShapeshiftFormView {
            spell_id: 71,
            texture: Some("Interface\\Icons\\Stance_B".into()),
            name: "Defensive Stance".into(),
            active: false,
            castable: false,
            cooldown: None,
        },
    ]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();

    // Geometry: the stance frame's BOTTOMLEFT = MainMenuBar (1024×53, screen-bottom
    // centered ⇒ left edge 0) TOPLEFT +(30,45) = (30,98); button 1 at frame BOTTOMLEFT +(11,3),
    // 30×30 ⇒ x[41,71] y[101,131]; button 2 chains +7 ⇒ left 78.
    let icon = |quads: &[benilla_ui::script::ExtractedQuad], path: &str| {
        quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
            .and_then(|q| q.rect)
    };
    let a = icon(&quads, "Interface\\Icons\\Stance_A").expect("stance button 1 icon");
    assert_eq!(
        (a.left, a.bottom, a.right, a.top),
        (41.0, 101.0, 71.0, 131.0)
    );
    let b = icon(&quads, "Interface\\Icons\\Stance_B").expect("stance button 2 icon");
    assert_eq!(b.left, 78.0);

    assert!(s
        .eval::<bool>("return ShapeshiftButton1:GetChecked()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return ShapeshiftButton2:GetChecked()")
        .unwrap());
    let grey = quads.iter().find_map(|q| match &q.content {
        QuadContent::Texture {
            path: Some(p),
            color: Some(c),
            ..
        } if p.contains("Stance_B") => Some(*c),
        _ => None,
    });
    assert_eq!(
        grey,
        Some([0.4, 0.4, 0.4, 1.0]),
        "not-castable greys the icon"
    );
    // Buttons past numForms stay hidden: exactly two stance icons drew.
    let stance_icons = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Stance_"))
        })
        .count();
    assert_eq!(stance_icons, 2);

    // Button 2 (centre (93, 116)) queues the form's spell id; cast or cancel is the app drain's.
    s.mouse_button(93.0, 116.0, "LeftButton", true);
    s.mouse_button(93.0, 116.0, "LeftButton", false);
    assert_eq!(s.take_shapeshift_casts(), vec![71]);

    // The OnClick undoes the CheckButton's own toggle (BonusActionBarFrame.xml:31-38), so the
    // ring waits for the form byte…
    assert!(
        !s.eval::<bool>("return ShapeshiftButton2:GetChecked()")
            .unwrap(),
        "a clicked non-active form must stay unchecked until the form byte confirms"
    );
    assert!(s
        .eval::<bool>("return ShapeshiftButton1:GetChecked()")
        .unwrap());

    // …and clicking the active form still queues its spell (the app drain no-ops it).
    s.mouse_button(56.0, 116.0, "LeftButton", true);
    s.mouse_button(56.0, 116.0, "LeftButton", false);
    assert_eq!(s.take_shapeshift_casts(), vec![2457]);
    assert!(
        s.eval::<bool>("return ShapeshiftButton1:GetChecked()")
            .unwrap(),
        "clicking the active stance must not untoggle its checked ring"
    );

    // Right-click does nothing here, as in the reference: `ShapeshiftButtonTemplate`'s OnLoad only
    // scales the cooldown (BonusActionBarFrame.xml:28-30), so the click mask stays left-up only,
    // and `0x77924b` gates the pushed texture on that mask.
    let depressed = |s: &UiScript| {
        s.extract().iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Quickslot-Depress"))
        })
    };
    s.mouse_move(56.0, 116.0);
    s.mouse_button(56.0, 116.0, "RightButton", true);
    assert!(
        !depressed(&s),
        "a right-press must NOT flash — unregistered"
    );
    s.mouse_button(56.0, 116.0, "RightButton", false);
    assert!(
        s.take_shapeshift_casts().is_empty(),
        "and must not cast — the ref never routes a right-click here at all"
    );
    s.mouse_button(56.0, 116.0, "LeftButton", true);
    assert!(depressed(&s), "the LEFT press does flash — the mask has it");
    s.mouse_button(56.0, 116.0, "LeftButton", false);
    let _ = s.take_shapeshift_casts();
    assert!(
        s.eval::<bool>("return ShapeshiftButton1:GetChecked()")
            .unwrap(),
        "and the active form stays lit through either"
    );

    // An emptied list hides the whole frame.
    s.set_shapeshift_forms(vec![]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    assert!(!s
        .eval::<bool>("return ShapeshiftBarFrame:IsShown()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A multibar button's tooltip shows its own paged action, not the main bar's slot of the same
/// index (`ActionButton_SetTooltip`, ActionButton.lua:365-382).
#[test]
fn multibar_hover_renders_the_buttons_own_action() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        // `ExhaustionTick_Update` reads `ReputationWatchBar`, which ReputationFrame.xml declares.
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        r"Interface\FrameXML\ReputationFrame.xml",
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
    ] {
        load_xml(&s, file);
    }

    // Main slot 2 stays empty under BottomLeft slot 2 (action 62), so a wrong id shows no tooltip.
    for (slot, spell) in [(1u32, 100u32), (61, 200), (49, 300), (62, 400)] {
        s.set_action(
            slot,
            Some(ActionSlot {
                texture: Some(format!("Interface\\Icons\\Spell_{spell}")),
                kind: 0x00,
                action: spell,
                count: 0,
                consumable: false,
            }),
        );
        s.set_spell_tooltip(
            spell,
            SpellTooltipView {
                name: format!("Spell {spell}"),
                description: "does a thing".into(),
                ..Default::default()
            },
        );
    }
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    show_bars(&s, &[1, 2]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let hover_name = |s: &UiScript, button: &str| -> Option<String> {
        s.run(&format!(
            "GameTooltip:Hide() this = {button} ActionButton_SetTooltip()"
        ))
        .unwrap();
        s.eval::<Option<String>>(
            "if not GameTooltip:IsShown() then return nil end return GameTooltipTextLeft1:GetText()",
        )
        .unwrap()
    };

    assert_eq!(
        hover_name(&s, "ActionButton1").as_deref(),
        Some("Spell 100"),
        "the main bar still reads its own slot"
    );
    assert_eq!(
        hover_name(&s, "MultiBarBottomLeftButton1").as_deref(),
        Some("Spell 200"),
        "BottomLeft button 1 is action 61 — not main slot 1 (the spell below it)"
    );
    assert_eq!(
        hover_name(&s, "MultiBarBottomRightButton1").as_deref(),
        Some("Spell 300"),
        "BottomRight button 1 is action 49"
    );
    assert_eq!(
        hover_name(&s, "MultiBarBottomLeftButton2").as_deref(),
        Some("Spell 400"),
        "an occupied multibar slot over an EMPTY main slot still renders (the no-tooltip half)"
    );

    s.set_bonus_bar_offset(1);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert_eq!(
        hover_name(&s, "MultiBarBottomLeftButton1").as_deref(),
        Some("Spell 200"),
        "a bonus page never re-pages a multibar hover"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `MultiBarRight` and `MultiBarLeft` are `parent="UIParent"` frames (MultiActionBars.xml:514,
/// 523) whose templates are `hidden="true"` (l.266, 381); Atlas (`Atlas.lua:387`) and Bartender2
/// (`Bartender2.lua:74`) index them by name.
#[test]
fn the_vertical_multibars_exist_hidden_on_the_reference_pages() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");

    for bar in ["MultiBarRight", "MultiBarLeft"] {
        assert!(
            s.eval::<bool>(&format!("return {bar} ~= nil")).unwrap(),
            "{bar} must exist — Atlas and Bartender2 index it by name at session start"
        );
        assert!(
            !s.eval::<bool>(&format!("return {bar}:IsShown()")).unwrap(),
            "{bar} must ship HIDDEN, exactly as VerticalMultiBar3/4 do"
        );
        // The two calls the addons make.
        s.run(&format!("{bar}:SetFrameStrata(\"MEDIUM\")")).unwrap();
        s.run(&format!("{bar}:ClearAllPoints()")).unwrap();
    }

    // ActionButton.lua:8-9: `RIGHT_ACTIONBAR_PAGE` 3 (actions 25..36), `LEFT_ACTIONBAR_PAGE` 4.
    for (bar, first, last) in [("MultiBarRight", 25, 36), ("MultiBarLeft", 37, 48)] {
        assert_eq!(
            s.eval::<i64>(&format!("return ActionButton_GetPagedID({bar}Button1)"))
                .unwrap(),
            first,
            "{bar}'s first slot"
        );
        assert_eq!(
            s.eval::<i64>(&format!("return ActionButton_GetPagedID({bar}Button12)"))
                .unwrap(),
            last,
            "{bar}'s last slot"
        );
    }
}

/// Each extra bar is down until its bit of `PLAYER_FIELD_BYTES` byte 2 is set; a fresh character's
/// byte is 0, and the bits name bars only in FrameXML (`0x4e76e0`). Bar 4 also needs bar 3
/// (MultiActionBars.lua:73), since `MultiBarLeft` anchors to `MultiBarRight`.
#[test]
fn every_extra_bar_stays_down_until_its_own_toggle_is_set() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");

    const BARS: [&str; 4] = [
        "MultiBarBottomLeft",
        "MultiBarBottomRight",
        "MultiBarRight",
        "MultiBarLeft",
    ];
    let shown =
        |s: &UiScript, bar: &str| s.eval::<bool>(&format!("return {bar}:IsShown()")).unwrap();

    // The zero byte, through `MultiActionBar_Update` as at login (UIParent.lua:364-365).
    s.run("MultiActionBar_Update()").unwrap();
    for bar in BARS {
        assert!(!shown(&s, bar), "{bar} must be down at a zero toggle byte");
    }

    for (flag, want) in [(1u32, 0usize), (2, 1), (3, 2)] {
        show_bars(&s, &[flag]);
        for (i, bar) in BARS.iter().enumerate() {
            assert_eq!(
                shown(&s, bar),
                i == want,
                "bar {flag} alone: {bar} shown {}, expected {}",
                shown(&s, bar),
                i == want
            );
        }
    }

    show_bars(&s, &[4]);
    for bar in BARS {
        assert!(
            !shown(&s, bar),
            "{bar}: SHOW_MULTI_ACTIONBAR_4 without 3 raises nothing"
        );
    }

    show_bars(&s, &[3, 4]);
    assert!(shown(&s, "MultiBarRight"));
    assert!(shown(&s, "MultiBarLeft"), "MultiBarLeft rides on bar 3");
    assert!(!shown(&s, "MultiBarBottomLeft"));
    assert!(!shown(&s, "MultiBarBottomRight"));

    show_bars(&s, &[1, 2, 3, 4]);
    for bar in BARS {
        assert!(shown(&s, bar), "{bar} up with the full byte");
    }
    show_bars(&s, &[]);
    for bar in BARS {
        assert!(!shown(&s, bar), "{bar} down again");
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Raising a bottom bar moves what shares the bottom band, through the manage pass the toggle runs
/// after `MultiActionBar_Update`: a variable row, `CONTAINER_OFFSET_Y` (70, +27), and a frame row,
/// `CastingBarFrame` (60, +40), each with `bottomEither` (UIParent.lua:1580, 1587).
#[test]
fn raising_a_bottom_bar_moves_the_managed_bottom_stack() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    load_xml(&s, "Interface\\FrameXML\\CastingBarFrame.xml");

    // Read the row's y off the anchor, so a hidden cast bar answers too.
    let cast_y = |s: &UiScript| {
        s.eval::<f64>("local _, _, _, _, y = CastingBarFrame:GetPoint() return y")
            .unwrap()
    };
    let offset_y = |s: &UiScript| s.eval::<f64>("return CONTAINER_OFFSET_Y").unwrap();

    show_bars(&s, &[]);
    assert_eq!(offset_y(&s), 70.0, "band clear: the row's base");
    let low = cast_y(&s);
    assert_eq!(low, 60.0, "CastingBarFrame's baseY");

    show_bars(&s, &[1]);
    assert_eq!(
        offset_y(&s),
        97.0,
        "bottom-left up: 70 + the row's bottomEither 27"
    );
    assert_eq!(cast_y(&s), 100.0, "the cast bar rises with it (60 + 40)");

    show_bars(&s, &[2]);
    assert_eq!(offset_y(&s), 97.0, "bottomEither is either");
    assert_eq!(cast_y(&s), 100.0);

    // Both up pays `bottomEither` once, and the row's `bottomRight` is 0.
    show_bars(&s, &[1, 2]);
    assert_eq!(offset_y(&s), 97.0);

    show_bars(&s, &[]);
    assert_eq!(offset_y(&s), 70.0);
    assert_eq!(cast_y(&s), low);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A raised bar takes its page out of the main bar's cycle and a lowered one gives it back:
/// ActionButton.lua:13 declares all six viewable, and `MultiActionBar_Update` blanks the raised
/// bars' pages (MultiActionBars.lua:49-80).
#[test]
fn viewable_action_bar_pages_follow_the_bar_toggles() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");

    let viewable = |s: &UiScript| {
        s.eval::<String>(
            "local out = {} \
             for i = 1, NUM_ACTIONBAR_PAGES do \
               if VIEWABLE_ACTION_BAR_PAGES[i] then table.insert(out, i) end \
             end \
             return table.concat(out, \",\")",
        )
        .unwrap()
    };

    show_bars(&s, &[]);
    assert_eq!(viewable(&s), "1,2,3,4,5,6", "no bar up, no page claimed");

    // ActionButton.lua:6-9: BottomLeft 6, BottomRight 5, Right 3, Left 4.
    show_bars(&s, &[1]);
    assert_eq!(viewable(&s), "1,2,3,4,5", "BottomLeft owns page 6");
    show_bars(&s, &[2]);
    assert_eq!(viewable(&s), "1,2,3,4,6", "BottomRight owns page 5");
    show_bars(&s, &[3]);
    assert_eq!(viewable(&s), "1,2,4,5,6", "MultiBarRight owns page 3");
    show_bars(&s, &[3, 4]);
    assert_eq!(
        viewable(&s),
        "1,2,5,6",
        "…and MultiBarLeft page 4 beside it"
    );

    // Bar 4 without bar 3 claims nothing: the pages follow the same conjunction as the bar.
    show_bars(&s, &[4]);
    assert_eq!(viewable(&s), "1,2,3,4,5,6");

    show_bars(&s, &[1, 2, 3, 4]);
    assert_eq!(viewable(&s), "1,2", "all four up: only the main pages left");
    show_bars(&s, &[]);
    assert_eq!(viewable(&s), "1,2,3,4,5,6", "and every page comes back");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Always Show ActionBars holds every extra bar's empty wells open. `showgrid` is a count
/// (ActionButton.lua:245, 255), so the option and a held payload are independent askers.
#[test]
fn the_grid_option_holds_the_extra_bars_empty_wells_open() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    show_bars(&s, &[1]);

    let well = |s: &UiScript| {
        s.eval::<bool>("return MultiBarBottomLeftButton5:IsShown()")
            .unwrap()
    };
    assert!(!well(&s), "an empty multibar well is hidden by default");

    s.run("ALWAYS_SHOW_MULTIBARS = \"1\" MultiActionBar_UpdateGridVisibility()")
        .unwrap();
    assert!(well(&s), "the option opens it with nothing in hand");
    // All four bars, raised or not (`MultiActionBar_ShowAllGrids`, MultiActionBars.lua:82-87).
    for bar in ["MultiBarBottomRight", "MultiBarRight", "MultiBarLeft"] {
        assert!(
            s.eval::<bool>(&format!("return {bar}Button5:IsShown()"))
                .unwrap(),
            "{bar}'s wells are held open too"
        );
    }

    // A payload on top of the option keeps the well open when the option lets go.
    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    assert!(well(&s));
    s.run("ALWAYS_SHOW_MULTIBARS = \"0\" MultiActionBar_UpdateGridVisibility()")
        .unwrap();
    assert!(well(&s), "the payload still holds the well open");
    s.fire_event("ACTIONBAR_HIDEGRID", vec![]);
    assert!(!well(&s), "and it closes when the last asker lets go");

    // The hide is a counted decrement (ActionButton.lua:251-259); the option applies it only on a
    // click (UIOptionsFrame.xml:683), and its VARIABLES_LOADED arm only shows
    // (UIOptionsFrame.lua:218-220). The load arm, off then on:
    s.run("ALWAYS_SHOW_MULTIBARS = \"0\"").unwrap();
    s.fire_event("VARIABLES_LOADED", vec![]);
    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    assert!(well(&s));
    s.fire_event("ACTIONBAR_HIDEGRID", vec![]);
    assert!(!well(&s), "a load with the option off left the count alone");
    s.run("ALWAYS_SHOW_MULTIBARS = \"1\"").unwrap();
    s.fire_event("VARIABLES_LOADED", vec![]);
    assert!(well(&s), "a load with the option on opens the wells");
    s.run("ALWAYS_SHOW_MULTIBARS = \"0\" MultiActionBar_UpdateGridVisibility()")
        .unwrap();
    assert!(!well(&s));

    // …and the click arm against a held payload: one apply closes the well the payload opened, and
    // the payload's own HIDEGRID would take the count to -1, as in the reference.
    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    assert!(well(&s));
    s.run("MultiActionBar_UpdateGridVisibility()").unwrap();
    assert!(
        !well(&s),
        "the option's hide counts against the payload's open"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A held payload ghosts the wells it opens: `ActionButton_ShowGrid` sets the ring's alpha to 0.5
/// (ActionButton.lua:246), and `UI-Quickslot` is a filled plate (black at 0.6), not a hollow ring.
/// `SetNormalTexture` keeps the region's colour (`0x778f10`) and `ActionButton_HideGrid` sets none,
/// so a ghosted ring stays dim after the payload goes.
#[test]
fn a_held_payload_ghosts_the_empty_wells_it_opens() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_SteelMelee".into()),
            kind: 0x00,
            action: 100,
            count: 0,
            consumable: false,
        }),
    );
    // Raise after PLAYER_ENTERING_WORLD: its arm re-reads the server's toggle byte and would lower
    // the bar again (UIParent.lua:362-365).
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    show_bars(&s, &[1]);
    s.resolve();

    /// The drawn alpha of every ring quad wearing `path`; no colour is untinted white.
    fn ring_alphas(s: &UiScript, path: &str) -> Vec<f32> {
        s.extract()
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(p),
                    color,
                    ..
                } if p == path => Some(color.map_or(1.0, |c| c[3])),
                _ => None,
            })
            .collect()
    }
    let quickslot = "Interface\\Buttons\\UI-Quickslot";
    let quickslot2 = "Interface\\Buttons\\UI-Quickslot2";

    assert_eq!(ring_alphas(&s, quickslot), Vec::<f32>::new());
    let resting = ring_alphas(&s, quickslot2);
    assert_eq!(
        resting.len(),
        1,
        "only the occupied main slot draws a ring: every empty well, main bar included, is \
         hidden while nothing is held (ActionButton.lua:69-70)"
    );
    assert!(
        resting.iter().all(|a| *a == 1.0),
        "a resting ring is opaque: {resting:?}"
    );

    s.fire_event("ACTIONBAR_SHOWGRID", vec![]);
    s.resolve();
    let ghosts = ring_alphas(&s, quickslot);
    assert_eq!(
        ghosts.len(),
        23,
        "11 empty main wells swap art + MultiBarBottomLeft's 12 appear"
    );
    assert!(
        ghosts.iter().all(|a| *a == 0.5),
        "every grid ring is the ref's half-alpha ghost, not an opaque plate: {ghosts:?}"
    );

    // …and the occupied ring with it: `ActionButton_ShowGrid` dims every button, occupied or not.
    let occupied = ring_alphas(&s, quickslot2);
    assert_eq!(occupied, vec![0.5], "the occupied ring ghosts too");

    // Filling a ghosted well runs `ActionButton_UpdateUsable` (ActionButton.lua:269-283), whose
    // three-argument `SetVertexColor(1, 1, 1)` sets alpha 1.0 in this engine; the reference keeps
    // the region's alpha (`0x79abd0`), which would leave this ring dim.
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_Rogue_Ambush".into()),
            kind: 0x00,
            action: 101,
            count: 0,
            consumable: false,
        }),
    );
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Number(2.0)]);
    s.resolve();
    assert_eq!(
        ring_alphas(&s, quickslot2),
        vec![0.5, 1.0],
        "the well that got an action is opaque again (its Update ran UpdateUsable); the other \
         occupied ring keeps its ghost, payload still held"
    );

    s.fire_event("ACTIONBAR_HIDEGRID", vec![]);
    s.resolve();
    assert_eq!(ring_alphas(&s, quickslot), Vec::<f32>::new());
    let after = ring_alphas(&s, quickslot2);
    assert_eq!(
        after,
        vec![0.5, 1.0],
        "HideGrid touches no colour: a ring the grid ghosted stays dim past the payload — the \
         reference's own rule (ours used to clear it here)"
    );
    // The ghosted ring's next `ActionButton_UpdateUsable` is not asserted: this engine resets its
    // alpha to 1.0, the reference keeps 0.5 (`0x79abd0`).
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `SetActionBarToggles` posts the whole byte, bits `0x01..0x08` for bars 1-4, one packet per call
/// (`0x4e76e0`); only the server's descriptor push moves the getter, which the
/// PLAYER_ENTERING_WORLD arm reads once (UIParent.lua:362-365).
#[test]
fn a_bar_toggle_sends_the_byte_its_globals_pack_to() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    let _ = s.take_action_bar_toggle_sends();

    // The Options row's sequence: assign, update the bars, send.
    s.run("SHOW_MULTI_ACTIONBAR_1 = 1 MultiActionBar_Update() SetActionBarToggles(SHOW_MULTI_ACTIONBAR_1, SHOW_MULTI_ACTIONBAR_2, SHOW_MULTI_ACTIONBAR_3, SHOW_MULTI_ACTIONBAR_4)")
        .unwrap();
    assert_eq!(s.take_action_bar_toggle_sends(), vec![0x01]);
    s.run("SHOW_MULTI_ACTIONBAR_3 = 1 MultiActionBar_Update() SetActionBarToggles(SHOW_MULTI_ACTIONBAR_1, SHOW_MULTI_ACTIONBAR_2, SHOW_MULTI_ACTIONBAR_3, SHOW_MULTI_ACTIONBAR_4)")
        .unwrap();
    assert_eq!(
        s.take_action_bar_toggle_sends(),
        vec![0x05],
        "the WHOLE byte re-sent, not a delta"
    );
    s.run("SHOW_MULTI_ACTIONBAR_4 = 1 MultiActionBar_Update() SetActionBarToggles(SHOW_MULTI_ACTIONBAR_1, SHOW_MULTI_ACTIONBAR_2, SHOW_MULTI_ACTIONBAR_3, SHOW_MULTI_ACTIONBAR_4)")
        .unwrap();
    assert_eq!(s.take_action_bar_toggle_sends(), vec![0x0d]);
    s.run("SHOW_MULTI_ACTIONBAR_1 = nil MultiActionBar_Update() SetActionBarToggles(SHOW_MULTI_ACTIONBAR_1, SHOW_MULTI_ACTIONBAR_2, SHOW_MULTI_ACTIONBAR_3, SHOW_MULTI_ACTIONBAR_4)")
        .unwrap();
    assert_eq!(
        s.take_action_bar_toggle_sends(),
        vec![0x0c],
        "the row turns its \"0\" into nil before the send, and nil is off"
    );

    // The globals hold 1 or nil, the shape `GetActionBarToggles` returns.
    assert_eq!(
        s.eval::<String>(
            "return type(SHOW_MULTI_ACTIONBAR_3) .. \"/\" .. type(SHOW_MULTI_ACTIONBAR_1)"
        )
        .unwrap(),
        "number/nil"
    );

    // The way back: push the server's byte and fire PLAYER_ENTERING_WORLD, its only reader.
    show_bars(&s, &[]);
    let _ = s.take_action_bar_toggle_sends();
    s.set_action_bar_toggles(0x0c);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert!(
        s.eval::<bool>("return MultiBarRight:IsShown() and MultiBarLeft:IsShown()")
            .unwrap(),
        "the seed brings back exactly the byte that was sent"
    );
    assert!(!s
        .eval::<bool>("return MultiBarBottomLeft:IsShown()")
        .unwrap());
    assert!(
        s.take_action_bar_toggle_sends().is_empty(),
        "the seed READS — a login must not post the byte back at the server"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Deviation: the Action Bars row passes `SetActionBarToggles` four arguments, not the fifth
/// `ALWAYS_SHOW_MULTIBARS` that `UIOptionsFrame_Save` adds (UIOptionsFrame.lua:363), because the
/// binding never reads it (`0x4e770e cmp esi,4`) and passing it makes the grid option look
/// server-backed. A fifth argument is invisible from outside, so a Lua spy replaces the binding.
#[test]
fn the_shipped_setter_passes_exactly_four_arguments() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");

    // Lua 5.0's `arg.n`: `...` as a value is not in the 1.12 client's grammar.
    s.run(
        r#"
        BENILLA_TEST_TOGGLE_ARGC = nil
        function SetActionBarToggles(...)
            BENILLA_TEST_TOGGLE_ARGC = arg.n
        end
        "#,
    )
    .unwrap();
    // The row's setter (our OptionsFrame.xml) calls `SetActionBarToggles`.
    s.run("BenillaOptionsFrameContainerBodyActionBarsRowMultiBar2Check:Click()")
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return BENILLA_TEST_TOGGLE_ARGC").unwrap(),
        4,
        "four — never the reference's five, which the binding drops on the floor"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The stance bar's seat is the manage pass's `ShapeshiftBarFrame` row (baseY 0, bottomLeft 45,
/// UIParent.lua:1583), in both bottom-bar states.
#[test]
fn the_stance_bar_sits_where_the_pass_puts_it() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    s.set_shapeshift_forms(vec![ShapeshiftFormView {
        spell_id: 2457,
        texture: Some("Interface\\Icons\\Stance_A".into()),
        name: "Battle Stance".into(),
        active: true,
        castable: true,
        cooldown: None,
    }]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);

    let seat = |s: &UiScript| {
        s.eval::<f64>("local _, _, _, _, y = ShapeshiftBarFrame:GetPoint() return y")
            .unwrap()
    };

    show_bars(&s, &[]);
    assert_eq!(
        seat(&s),
        0.0,
        "no bottom bar: the row's baseY, on the main bar"
    );
    show_bars(&s, &[1]);
    assert_eq!(seat(&s), 45.0, "bottom-left up: +45, the ref's raised seat");
    show_bars(&s, &[2]);
    assert_eq!(
        seat(&s),
        0.0,
        "the RIGHT bottom bar is not under it — the row's flag is bottomLeft, not bottomEither"
    );
    show_bars(&s, &[]);
    assert_eq!(seat(&s), 0.0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The stance shelf's art follows the manage pass (UIParent.lua:1705-1732): unraised, both end caps
/// show and the rings are 64; raised over the bottom-left bar, all three strips hide and the rings
/// drop to 50.
#[test]
fn the_stance_shelf_follows_the_bottom_left_bar() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");

    let form = |id: u32| ShapeshiftFormView {
        spell_id: id,
        texture: Some(format!("Interface\\Icons\\Stance_{id}")),
        name: format!("Form {id}"),
        active: false,
        castable: true,
        cooldown: None,
    };
    let shown = |s: &UiScript, region: &str| {
        s.eval::<bool>(&format!("return {region}:IsShown()"))
            .unwrap()
    };
    let ring = |s: &UiScript| {
        s.eval::<f64>("return ShapeshiftButton1NormalTexture:GetWidth()")
            .unwrap()
    };

    // A three-form warrior, no bottom bar: the whole shelf, and the big ring.
    s.set_shapeshift_forms(vec![form(2457), form(71), form(2458)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    show_bars(&s, &[]);
    assert!(shown(&s, "ShapeshiftBarLeft"), "the left end cap");
    assert!(shown(&s, "ShapeshiftBarRight"), "the right end cap");
    assert!(
        shown(&s, "ShapeshiftBarMiddle"),
        "3 forms: the middle strip"
    );
    assert_eq!(ring(&s), 64.0, "unraised rings are the ref's 64");

    // Two forms: `ShapeshiftBar_Update` hides the middle strip (BonusActionBarFrame.lua:166-167).
    s.set_shapeshift_forms(vec![form(2457), form(71)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    assert!(shown(&s, "ShapeshiftBarLeft"));
    assert!(shown(&s, "ShapeshiftBarRight"));
    assert!(
        !shown(&s, "ShapeshiftBarMiddle"),
        "exactly 2 forms: end caps only"
    );
    assert_eq!(ring(&s), 64.0);

    // Raising the bottom-left bar moves the seat and hides the art in one pass.
    show_bars(&s, &[1]);
    assert_eq!(
        s.eval::<f64>("local _, _, _, _, y = ShapeshiftBarFrame:GetPoint() return y")
            .unwrap(),
        45.0,
        "the seat rose"
    );
    for region in [
        "ShapeshiftBarLeft",
        "ShapeshiftBarMiddle",
        "ShapeshiftBarRight",
    ] {
        assert!(
            !shown(&s, region),
            "{region} must not draw across the row below"
        );
    }
    assert_eq!(ring(&s), 50.0, "raised rings are the ref's 50");

    // A third form learned while raised: `ShapeshiftBar_Update` shows the middle strip regardless
    // (BonusActionBarFrame.lua:169-175) and nothing runs the pass on that event, so the strip
    // draws over the row until the next pass (UIParent.lua:1706-1717).
    s.set_shapeshift_forms(vec![form(2457), form(71), form(2458)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    assert!(
        shown(&s, "ShapeshiftBarMiddle"),
        "Update shows the strip; only the pass takes it down"
    );
    s.run("UIParent_ManageFramePositions()").unwrap();
    assert!(!shown(&s, "ShapeshiftBarMiddle"));
    assert_eq!(ring(&s), 50.0);

    show_bars(&s, &[]);
    assert!(shown(&s, "ShapeshiftBarMiddle"));
    assert_eq!(ring(&s), 64.0);
    assert_eq!(
        s.eval::<f64>("local _, _, _, _, y = ShapeshiftBarFrame:GetPoint() return y")
            .unwrap(),
        0.0
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `ShapeshiftBar_Update` sizes the shelf to the form count (BonusActionBarFrame.lua:161-175).
/// The strips are declared as the three-form chain, and a hidden middle strip still resolves its
/// 38 px rect, so the right cap is re-pointed for one and two forms.
#[test]
fn the_stance_shelf_is_as_long_as_the_form_count() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    show_bars(&s, &[]); // unraised: the shelf art is the state under test

    let form = |id: u32| ShapeshiftFormView {
        spell_id: id,
        texture: Some(format!("Interface\\Icons\\Stance_{id}")),
        name: format!("Form {id}"),
        active: false,
        castable: true,
        cooldown: None,
    };
    let set_forms = |s: &mut UiScript, n: u32| {
        s.set_shapeshift_forms((0..n).map(|i| form(2450 + i)).collect());
        s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
        s.resolve();
    };
    // The right cap's left edge relative to the left cap's: the shelf in one number.
    let cap = |s: &UiScript| {
        s.eval::<f64>("return ShapeshiftBarRight:GetLeft() - ShapeshiftBarLeft:GetLeft()")
            .unwrap()
    };
    // Button 1 sits at +11 and is 30 wide, so its own span is 11..41 in the same space.
    const BUTTON1_RIGHT: f64 = 41.0;

    // One form (a rogue): the cap sits 12 px into the left cap (BonusActionBarFrame.lua:165).
    set_forms(&mut s, 1);
    assert_eq!(
        cap(&s),
        12.0,
        "one form: the cap sits 12px into the left cap"
    );
    assert!(
        cap(&s) + 42.0 > BUTTON1_RIGHT,
        "the one-form shelf still covers its button"
    );
    assert!(
        !s.eval::<bool>("return ShapeshiftBarMiddle:IsShown()")
            .unwrap(),
        "one form: no middle strip"
    );

    // Two forms: the caps butt together, 45 + 42 = 87 px of shelf (l.168).
    set_forms(&mut s, 2);
    assert_eq!(cap(&s), 45.0, "two forms: cap on the left cap's RIGHT edge");

    // Past two, the middle strip is 38 px per extra form and the cap chains off it (l.170-174).
    set_forms(&mut s, 3);
    assert_eq!(
        s.eval::<f64>("return ShapeshiftBarMiddle:GetWidth()")
            .unwrap(),
        38.0,
        "3 forms: one slot of middle"
    );
    assert_eq!(cap(&s), 83.0);

    set_forms(&mut s, 5);
    assert_eq!(
        s.eval::<f64>("return ShapeshiftBarMiddle:GetWidth()")
            .unwrap(),
        114.0,
        "5 forms: three slots of middle — the width the ref computes, not the declared 38"
    );
    assert_eq!(cap(&s), 159.0);
    // Button 5 spans 159..189 (11 + 4 * 37); the cap must clear it.
    assert!(
        cap(&s) >= 11.0 + 4.0 * 37.0,
        "the cap clears the last button"
    );

    set_forms(&mut s, 1);
    assert_eq!(cap(&s), 12.0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An extra bar's empty well keeps its bound key's label: `ActionButton_Update` touches only the
/// label's colour (ActionButton.lua:172), and `ActionButton_UpdateHotkeys` writes a bound key's
/// text whether or not the slot has an action (l.141-142).
#[test]
fn an_extra_bars_empty_well_keeps_its_bound_hotkey_label() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The real command set, so `MULTIACTIONBAR1BUTTONn` is bindable; it ships unbound.
    s.register_bindings(&crate::bindings::registry_commands());
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    show_bars(&s, &[1]);
    s.set_action(
        61,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_BL".into()),
            kind: 0x00,
            action: 200,
            count: 0,
            consumable: false,
        }),
    );
    // An occupied well and two empty ones, all bound, held open so the empty ones show.
    s.run(
        r#"SetBinding("Q", "MULTIACTIONBAR1BUTTON1")
           SetBinding("E", "MULTIACTIONBAR1BUTTON2")
           SetBinding("R", "MULTIACTIONBAR1BUTTON3")
           ALWAYS_SHOW_MULTIBARS = "1" MultiActionBar_UpdateGridVisibility()"#,
    )
    .unwrap();
    s.fire_event("UPDATE_BINDINGS", vec![]);

    let label = |s: &UiScript, button: &str| {
        s.eval::<Option<String>>(&format!("return {button}HotKey:GetText()"))
            .unwrap()
            .unwrap_or_default()
    };
    let drawn = |s: &mut UiScript, want: &str| {
        s.resolve();
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == want))
    };
    assert_eq!(label(&s, "MultiBarBottomLeftButton1"), "Q");
    assert_eq!(label(&s, "MultiBarBottomLeftButton2"), "E");
    assert_eq!(label(&s, "MultiBarBottomLeftButton3"), "R");
    assert!(drawn(&mut s, "E"), "the empty well's label is on screen");

    // Each event repaints the buttons through `ActionButton_Update`.
    for (event, what) in [
        ("PLAYER_ENTERING_WORLD", "a world enter"),
        ("ACTIONBAR_SHOWGRID", "a payload picked up over the bars"),
        ("ACTIONBAR_HIDEGRID", "and put down again"),
    ] {
        s.fire_event(event, vec![]);
        assert_eq!(label(&s, "MultiBarBottomLeftButton1"), "Q", "{what}");
        assert_eq!(
            label(&s, "MultiBarBottomLeftButton2"),
            "E",
            "the empty well keeps its bound label through {what}"
        );
        assert_eq!(label(&s, "MultiBarBottomLeftButton3"), "R", "{what}");
    }
    // The world enter re-seeded the toggles from a byte the harness never pushed, lowering bar 1;
    // raise it again to see the label drawn.
    show_bars(&s, &[1]);
    assert!(drawn(&mut s, "E"), "…and it is still on screen");

    // An unbound empty well stays blank (ActionButton.lua:129-131).
    assert_eq!(label(&s, "MultiBarBottomLeftButton4"), "");
    // Unbinding one takes its label away.
    s.run(r#"SetBinding("E", nil)"#).unwrap();
    s.fire_event("UPDATE_BINDINGS", vec![]);
    assert_eq!(label(&s, "MultiBarBottomLeftButton2"), "");
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert_eq!(label(&s, "MultiBarBottomLeftButton2"), "");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `ShapeshiftBar_Update`'s `SetTexCoord(0, n-2, 0, 1)` (BonusActionBarFrame.lua:173) tiles the
/// middle strip along u only. Wrapping v too would bleed the texture's opaque last row
/// (`ShapeshiftBarMiddle.blp` row 31) into its transparent top edge as a grey hairline.
#[test]
fn the_middle_strip_tiles_along_its_length_only() {
    benilla_formats::wow_data_or_skip!();
    use super::extract::tiling_axes;
    use crate::ui_pass::UvRect;
    use benilla_ui::script::{ShapeshiftFormView, TexCoords};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    show_bars(&s, &[]); // bars down: the shelf is the state under test

    let form = |id: u32| ShapeshiftFormView {
        spell_id: id,
        texture: Some(format!("Interface\\Icons\\Stance_{id}")),
        name: format!("Form {id}"),
        active: id == 2457,
        castable: true,
        cooldown: None,
    };
    // Every drawn piece whose texture path ends in `piece`, as the wrap it asks the renderer for.
    let wraps_of = |s: &UiScript, piece: &str| -> Vec<(bool, bool)> {
        s.extract()
            .into_iter()
            .filter_map(|q| match q.content {
                QuadContent::Texture {
                    path: Some(p),
                    tex_coords,
                    ..
                } if p.ends_with(piece) => Some(tiling_axes(&match tex_coords {
                    Some(TexCoords::Rect(e)) => UvRect::from_tex_coords(e),
                    Some(TexCoords::Corners(c)) => UvRect::from_corners(c),
                    None => UvRect::FULL,
                })),
                _ => None,
            })
            .collect()
    };

    // Four forms (a druid): two slots of middle, wrapped along u only; the end caps, atlas crops of
    // `ShapeshiftBarEnds`, never wrap.
    s.set_shapeshift_forms(vec![form(2457), form(71), form(768), form(2458)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    s.resolve();
    assert_eq!(
        wraps_of(&s, "ShapeshiftBarMiddle"),
        vec![(true, false)],
        "four forms: the strip tiles along its length and clamps across it"
    );
    assert_eq!(
        wraps_of(&s, "ShapeshiftBarEnds"),
        vec![(false, false), (false, false)],
        "the end caps are atlas crops and never tile"
    );

    // Three forms (a warrior): one slot, the whole texture, nothing tiles.
    s.set_shapeshift_forms(vec![form(2457), form(71), form(2458)]);
    s.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]);
    s.resolve();
    assert_eq!(wraps_of(&s, "ShapeshiftBarMiddle"), vec![(false, false)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The reference fires `UPDATE_SHAPESHIFT_FORMS` only when the form list changes (`0x4b28ff`,
/// `0x4b2e43`): its `ShapeshiftBar_Update` re-shows the middle strip with no manage pass
/// (BonusActionBarFrame.lua:170, 177), so a state change sent as one would draw the strip over the
/// raised bar. Driven through [`crate::ui_shapeshift::push_forms`].
#[test]
fn a_forms_state_change_leaves_the_shelf_down_over_the_raised_bar() {
    benilla_formats::wow_data_or_skip!();
    use crate::ui_shapeshift::{push_forms, FormsEdge, StanceMemory};
    use benilla_ui::script::ShapeshiftFormView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&s);
    load_xml(&s, "Interface\\FrameXML\\MultiActionBars.xml");
    // A recorder for the two events a push may announce.
    s.run(
        "STANCE_LOG = {} local f = CreateFrame('Frame') \
         f:RegisterEvent('UPDATE_SHAPESHIFT_FORMS') f:RegisterEvent('SPELL_UPDATE_USABLE') \
         f:SetScript('OnEvent', function() table.insert(STANCE_LOG, event) end)",
    )
    .unwrap();
    let log = |s: &UiScript| {
        s.eval::<String>("local l = table.concat(STANCE_LOG, ',') STANCE_LOG = {} return l")
            .unwrap()
    };
    let shown = |s: &UiScript, region: &str| {
        s.eval::<bool>(&format!("return {region}:IsShown()"))
            .unwrap()
    };
    let shelf = [
        "ShapeshiftBarLeft",
        "ShapeshiftBarMiddle",
        "ShapeshiftBarRight",
    ];
    let ring = |s: &UiScript| {
        s.eval::<f64>("return ShapeshiftButton1NormalTexture:GetWidth()")
            .unwrap()
    };
    let checked = |s: &UiScript, i: u32| {
        s.eval::<bool>(&format!("return ShapeshiftButton{i}:GetChecked() == 1"))
            .unwrap()
    };
    let stance = |id: u32, active: bool, castable: bool, cooldown| ShapeshiftFormView {
        spell_id: id,
        texture: Some(format!("Interface\\Icons\\Stance_{id}")),
        name: format!("Stance {id}"),
        active,
        castable,
        cooldown,
    };
    let mut memory = StanceMemory::default();

    // Login: the list arrives, a list edge, with Battle Stance active.
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, true, true, None),
                stance(71, false, true, None),
                stance(2458, false, true, None),
            ],
        ),
        FormsEdge::List
    );
    assert_eq!(log(&s), "UPDATE_SHAPESHIFT_FORMS");
    assert!(checked(&s, 1) && !checked(&s, 2));
    // The bottom-left bar up: the pass seats the stance bar a row higher and takes the shelf down
    // (UIParent.lua:1706-1716).
    show_bars(&s, &[1]);
    for region in shelf {
        assert!(!shown(&s, region), "{region} is down under a raised bar");
    }
    assert_eq!(ring(&s), 50.0);

    // A stance switch: the form byte flips to Defensive and category 47 arms for a second on all
    // three, a state move the reference announces through the aura and cooldown events.
    let cd = Some((0, 1000, true));
    let edge = push_forms(
        &mut s,
        &mut memory,
        vec![
            stance(2457, false, true, cd),
            stance(71, true, true, cd),
            stance(2458, false, true, cd),
        ],
    );
    for region in shelf {
        assert!(
            !shown(&s, region),
            "{region}: the shelf re-shown over the raised bar"
        );
    }
    assert_eq!(ring(&s), 50.0);
    assert_eq!(edge, FormsEdge::Silent);
    assert_eq!(log(&s), "", "a state move fires no list edge");
    // …and the checked ring follows the aura event the switch carries.
    assert!(
        checked(&s, 1) && !checked(&s, 2),
        "no repaint before the state event"
    );
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    assert!(
        !checked(&s, 1) && checked(&s, 2),
        "the aura event repaints the ring"
    );
    for region in shelf {
        assert!(
            !shown(&s, region),
            "{region}: a state repaint never touches the shelf"
        );
    }

    // The cooldown running out is silent too: the reference's store fires nothing at expiry.
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, false, true, None),
                stance(71, true, true, None),
                stance(2458, false, true, None),
            ],
        ),
        FormsEdge::Silent
    );
    assert_eq!(log(&s), "");
    assert!(!shown(&s, "ShapeshiftBarMiddle"));

    // A castable flip fires `SPELL_UPDATE_USABLE`; the bar greys the icon and leaves the shelf.
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, false, false, None),
                stance(71, true, true, None),
                stance(2458, false, true, None),
            ],
        ),
        FormsEdge::Usable
    );
    assert_eq!(log(&s), "SPELL_UPDATE_USABLE");
    let grey = s
        .eval::<f64>("local r = ShapeshiftButton1Icon:GetVertexColor() return r")
        .unwrap();
    assert!(
        (grey - 0.4).abs() < 1e-6,
        "not castable: the 0.4 grey, got {grey}"
    );
    assert!(!shown(&s, "ShapeshiftBarMiddle"));

    // Nothing moved: nothing pushed, nothing fired.
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, false, false, None),
                stance(71, true, true, None),
                stance(2458, false, true, None),
            ],
        ),
        FormsEdge::Unchanged
    );
    assert_eq!(log(&s), "");

    // A fourth stance learned is a list edge: `ShapeshiftBar_Update` shows the strip over the
    // raised bar until the next pass.
    assert_eq!(
        push_forms(
            &mut s,
            &mut memory,
            vec![
                stance(2457, false, false, None),
                stance(71, true, true, None),
                stance(2458, false, true, None),
                stance(768, false, true, None),
            ],
        ),
        FormsEdge::List
    );
    assert_eq!(log(&s), "UPDATE_SHAPESHIFT_FORMS");
    assert!(
        shown(&s, "ShapeshiftBarMiddle"),
        "the learn edge: Update shows the strip"
    );
    s.run("UIParent_ManageFramePositions()").unwrap();
    assert!(!shown(&s, "ShapeshiftBarMiddle"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
