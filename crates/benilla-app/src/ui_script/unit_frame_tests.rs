use benilla_ui::script::{
    QuadContent, ScriptValue, SelectionRequest, SoundRequest, UiScript, UnitState,
};

use super::test_ui::{hover, load_ui as load_xml, unhover};

/// The unit frames' load prefix, in the manifest's order.
fn load_unit_frames(s: &UiScript) {
    // Stock `GlobalStrings.lua` first, as the app runs it: the unit-frame files read it at load
    // (`CombatFeedback.lua:6-17`, `UnitFrame.lua:2-7`).
    load_xml(s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(s, r"Interface\FrameXML\UIParent.xml");
    load_xml(s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(s, "Interface\\FrameXML\\GameTooltip.xml");
    // `FACTION_BAR_COLORS` (`ReputationFrame.lua:3`) for stock `GameTooltip_UnitColor`.
    load_xml(s, r"Interface\FrameXML\ReputationFrame.lua");
    load_xml(s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(s, "Interface\\FrameXML\\BasicControls.xml"); // `TEXT`, read by UnitPopup.lua at load
    load_xml(s, "Interface\\FrameXML\\UnitPopup.xml");
    load_xml(s, "Interface\\FrameXML\\BuffFrame.xml");
    load_xml(s, "Interface\\FrameXML\\UnitFrame.xml");
    load_xml(s, "Interface\\FrameXML\\CombatFeedback.xml");
    load_xml(s, "Interface\\FrameXML\\PlayerFrame.xml");
    load_xml(s, "Interface\\FrameXML\\PartyFrame.xml");
    load_xml(s, "Interface\\FrameXML\\TargetFrame.xml");
    load_xml(s, "Interface\\FrameXML\\PetFrame.xml");
}

/// The stock unit frames driven by snapshots: bar fill, the target's hide and show, a late name.
#[test]
fn shipped_unit_frames_drive_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Only the target frame hides: `PlayerFrame_Update` has no else arm (`PlayerFrame.lua:29-37`),
    // `TargetFrame_Update` does (`TargetFrame.lua:37-56`).
    let shape: bool = s
        .eval("return PlayerFrame:IsVisible() and not TargetFrame:IsVisible()")
        .unwrap();
    assert!(
        shape,
        "the player plate is always up; only the target frame hides while its unit is absent"
    );

    // `PLAYER_ENTERING_WORLD` repaints the whole frame (`PlayerFrame.lua:96-100`); a bar's own
    // events repaint only that bar (`UnitFrame.lua:150-151`, `:190-199`).
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: None,
            health: 72,
            max_health: 100,
            level: 12,
            power_type: 0,
            power: 45,
            max_power: 80,
            dead: false,
            reaction: 0,
            // Else the disconnect leg pins the power bar full and grey (`UnitFrame.lua:214-216`).
            is_connected: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert!(s.eval::<bool>("return PlayerFrame:IsVisible()").unwrap());
    let ok: bool = s
        .eval(
            r#"
            local hb, pb = PlayerFrameHealthBar, PlayerFrameManaBar
            local _, hmax = hb:GetMinMaxValues()
            local _, pmax = pb:GetMinMaxValues()
            local r, g, b = pb:GetStatusBarColor()
            return hb:GetValue() == 72 and hmax == 100
               and pb:GetValue() == 45 and pmax == 80 and pb:IsVisible()
               and b == 1 and r == 0 -- mana blue
        "#,
        )
        .unwrap();
    assert!(ok, "player frame painted from the snapshot");

    // Blank while unresolved: `PlayerName` has no `text=` (`PlayerFrame.xml:58`).
    assert_eq!(
        s.eval::<Option<String>>("return PlayerName:GetText()")
            .unwrap(),
        None,
        "no name yet: the stock file has no \"Unknown\" literal to fall back on"
    );

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Benilla".into()),
            health: 72,
            max_health: 100,
            level: 12,
            power_type: 0,
            power: 45,
            max_power: 80,
            dead: false,
            reaction: 0,
            is_connected: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_NAME_UPDATE", vec![ScriptValue::Str("player".into())]);
    assert_eq!(
        s.eval::<String>("return PlayerName:GetText()").unwrap(),
        "Benilla"
    );

    // A powerless target's bar runs empty, not hidden: only a bar with a `TextString` hides when
    // trackless (`TextStatusBar.lua:55-56`), and `TargetFrame.xml:486-487` names undeclared ones.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Young Wolf".into()),
            health: 30,
            max_health: 50,
            level: 3,
            power_type: 0,
            power: 0,
            max_power: 0,
            dead: false,
            reaction: 4, // neutral
            is_connected: true,
            ..UnitState::default()
        }),
    );
    s.take_sounds(); // drain anything earlier; the target select/deselect pair is under test below
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local _, pmax = TargetFrameManaBar:GetMinMaxValues()
            return TargetFrame:IsVisible()
               and TargetFrameManaBar:GetValue() == 0 and pmax == 0
               and TargetFrameManaBar.TextString == nil -- the reference declares no text region
               and TargetName:GetText() == "Young Wolf"
               and TargetLevelText:GetText() == "3"
               and not TargetDeadText:IsShown() -- living target: no dead word
        "#,
        )
        .unwrap();
    assert!(
        ok,
        "target frame painted; a powerless unit's power bar runs empty over an empty track"
    );
    // `TargetFrame_OnShow` picks the kit (`TargetFrame.lua:104-112`); reaction 4 is neutral.
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCreatureNeutralSelect".into())],
        "neutral target select kit"
    );

    // Reaction 4 tints the plate yellow: `UnitReactionColor[4]` is (1,1,0) (`TargetFrame.lua:10`).
    s.resolve();
    let plate = s
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Texture {
                path: Some(p),
                color: Some(c),
                ..
            } if p.contains("LevelBackground") => Some(c),
            _ => None,
        })
        .expect("target name-plate quad present");
    assert!(
        (plate[0] - 1.0).abs() < 1e-6 && (plate[1] - 1.0).abs() < 1e-6 && plate[2].abs() < 1e-6,
        "neutral name plate is yellow, got {plate:?}"
    );

    // The health fill is 30/50 of the 119px bar (`TargetFrame.xml:253-255`).
    s.resolve();
    let quads = s.extract();
    let bar_rect = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-StatusBar"))
        })
        .filter_map(|q| q.rect)
        .find(|r| (r.width() - 119.0 * 0.6).abs() < 0.01)
        .expect("target health fill at 60% of 119px");
    assert!((bar_rect.width() - 71.4).abs() < 0.01);

    s.set_unit("target", None);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!s.eval::<bool>("return TargetFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName(
            "INTERFACESOUND_LOSTTARGETUNIT".into()
        )],
        "deselect plays the lost-target kit"
    );

    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Kobold Vermin".into()),
            health: 40,
            max_health: 40,
            level: 1,
            reaction: 2,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(s
        .eval::<bool>("return UnitIsEnemy(\"target\", \"player\") == 1")
        .unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCreatureAggroSelect".into())],
        "hostile target select kit"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The drawn height of the text quad reading `text`: the `SetTextHeight` override, while `GetFont`
/// keeps the font object's own 30, as in the reference.
fn extracted_text_height(s: &mut UiScript, text: &str) -> Option<f32> {
    s.resolve();
    s.extract().into_iter().find_map(|q| match q.content {
        QuadContent::Text {
            text: Some(t),
            text_height,
            ..
        } if t == text => Some(text_height),
        _ => None,
    })?
}

/// `UNIT_COMBAT` drives stock `CombatFeedback` on the player frame: a wound white at height 30, a
/// spell crit yellow at ×1.5, an absorb's word at ×0.75, faded 0.2 s in, held 0.7 s, 0.3 s out
/// (`CombatFeedback.lua:2-4`). The frame takes only `arg1 == "player"` (`PlayerFrame.lua:88-91`).
#[test]
fn unit_combat_drives_the_player_hit_indicator() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            health: 72,
            max_health: 100,
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);

    let ev = |unit: &str, action: &str, flags: &str, amount: i64, school: i64| {
        vec![
            ScriptValue::Str(unit.into()),
            ScriptValue::Str(action.into()),
            ScriptValue::Str(flags.into()),
            ScriptValue::Int(amount),
            ScriptValue::Int(school),
        ]
    };

    s.fire_event("UNIT_COMBAT", ev("player", "WOUND", "", 17, 0));
    let ok: bool = s
        .eval(
            r#"
            local ind = PlayerHitIndicator
            return ind:IsShown() ~= nil and tostring(ind:GetText()) == "17"
        "#,
        )
        .unwrap();
    assert!(ok, "physical wound paints the amount ({:?})", s.errors());
    assert_eq!(
        extracted_text_height(&mut s, "17"),
        Some(30.0),
        "base height 30 (the SetTextHeight regime)"
    );

    s.fire_event("UNIT_COMBAT", ev("player", "WOUND", "CRITICAL", 64, 4));
    let ok: bool = s
        .eval("return tostring(PlayerHitIndicator:GetText()) == \"64\"")
        .unwrap();
    assert!(ok, "spell crit paints ({:?})", s.errors());
    assert_eq!(
        extracted_text_height(&mut s, "64"),
        Some(45.0),
        "×1.5 crit height, UNCAPPED past 32"
    );

    s.fire_event("UNIT_COMBAT", ev("player", "WOUND", "ABSORB", 0, 0));
    let ok: bool = s
        .eval("return PlayerHitIndicator:GetText() == \"Absorb\"")
        .unwrap();
    assert!(ok, "full absorb paints the word ({:?})", s.errors());
    assert_eq!(
        extracted_text_height(&mut s, "Absorb"),
        Some(22.5),
        "the word at ×0.75"
    );

    s.tick(0.5); // 0.2 fade-in + into the hold
    let ok: bool = s
        .eval(
            r#"
            local ind = PlayerHitIndicator
            return ind:IsShown() ~= nil and ind:GetAlpha() == 1.0
        "#,
        )
        .unwrap();
    assert!(ok, "mid-hold: opaque ({:?})", s.errors());
    s.tick(0.8); // past fade-in + hold + fade-out (1.2 s total)
    assert!(
        s.eval::<bool>("return PlayerHitIndicator:IsShown() == nil")
            .unwrap(),
        "the envelope ends in a Hide ({:?})",
        s.errors()
    );

    s.fire_event("UNIT_COMBAT", ev("target", "WOUND", "", 99, 0));
    assert!(
        s.eval::<bool>("return PlayerHitIndicator:IsShown() == nil")
            .unwrap(),
        "a target event never touches the player indicator"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `PlayerFrame_OnClick` (`PlayerFrame.lua:156-172`) targets the player on a left click and
/// opens the SELF popup on a right click: no menu solo, the leader's rows in a party.
#[test]
fn left_clicking_the_player_frame_targets_self() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            health: 72,
            max_health: 100,
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    s.resolve();
    assert!(
        s.take_selection_requests().is_empty(),
        "no request before any click"
    );

    let (cx, cy) = s
        .eval::<(f64, f64)>("return PlayerFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    assert!(
        s.take_selection_requests().is_empty(),
        "right-click queues no target"
    );
    // Solo, every SELF row but Cancel is gated off (`UnitPopup.lua:69`, no PvP row): no menu.
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "no SELF menu while solo"
    );

    s.mouse_button(cx as f32, cy as f32, "LeftButton", true);
    s.mouse_button(cx as f32, cy as f32, "LeftButton", false);
    assert_eq!(
        s.take_selection_requests(),
        vec![SelectionRequest::Unit("player".into())],
        "left-click queues a self-target"
    );

    s.set_party(benilla_ui::script::PartyState {
        members: vec![benilla_ui::script::PartyMemberInfo {
            name: "Alice".into(),
            guid: 0xA11CE,
        }],
        leader_index: 0, // we lead
        // The player's own guid, unset; 0 is also the ungrouped sentinel, but there is a member.
        leader_guid: 0,
        own_guid: 0,
        raid: Vec::new(),
        loot_method: "group".into(),
        master_looter: None,
        loot_threshold: 2,
    });
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the SELF menu opens for a party leader"
    );
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        6,
        "title + Loot Method + Loot Threshold + Leave Party + Raid Target Icon + Cancel"
    );
    // `PARTY_LEAVE` is "Leave party", lower-case (`GlobalStrings.lua:2991`).
    assert_eq!(
        s.eval::<String>("return DropDownList1Button4:GetText()")
            .unwrap(),
        "Leave party"
    );
    assert!(
        s.eval::<bool>("return DropDownList1Button2ExpandArrow:IsVisible()")
            .unwrap(),
        "Loot Method is nested for the leader"
    );
    s.resolve();
    let (rx, ry) = s
        .eval::<(f64, f64)>("return DropDownList1Button4:GetCenter()")
        .unwrap();
    s.mouse_button(rx as f32, ry as f32, "LeftButton", true);
    s.mouse_button(rx as f32, ry as f32, "LeftButton", false);
    assert_eq!(
        s.take_party_requests(),
        vec![benilla_ui::script::PartyRequest::Leave],
        "Leave Party queues the leave intent"
    );
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "the click closes the list"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A leader opens the SELF popup, hovers Raid Target Icon (a `hasArrow` row's OnEnter opens
/// `DropDownList2`) and clicks Skull; the mark queues against the menu's unit.
#[test]
fn raid_mark_clicks_through_the_nested_level() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            health: 72,
            max_health: 100,
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    s.set_party(benilla_ui::script::PartyState {
        members: vec![benilla_ui::script::PartyMemberInfo {
            name: "Alice".into(),
            guid: 0xA11CE,
        }],
        leader_index: 0, // we lead: the mark rows are leader-gated
        leader_guid: 0,  // the player's own guid; this fixture leaves it unset
        own_guid: 0,
        raid: Vec::new(),
        loot_method: "group".into(),
        master_looter: None,
        loot_threshold: 2,
    });
    s.resolve();

    let (cx, cy) = s
        .eval::<(f64, f64)>("return PlayerFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the SELF menu opens"
    );
    s.resolve();

    assert_eq!(
        s.eval::<String>("return DropDownList1Button5:GetText()")
            .unwrap(),
        "Raid Target Icon"
    );
    let (rx, ry) = s
        .eval::<(f64, f64)>("return DropDownList1Button5:GetCenter()")
        .unwrap();
    s.mouse_move(rx as f32, ry as f32);
    assert!(
        s.eval::<bool>("return DropDownList2:IsVisible()").unwrap(),
        "hovering the nested row opens level 2"
    );
    assert_eq!(
        s.eval::<i64>("return DropDownList2.numButtons").unwrap(),
        9,
        "the eight marks + None"
    );
    s.resolve();

    assert_eq!(
        s.eval::<String>("return DropDownList2Button8:GetText()")
            .unwrap(),
        "Skull"
    );
    let (sx, sy) = s
        .eval::<(f64, f64)>("return DropDownList2Button8:GetCenter()")
        .unwrap();
    s.mouse_button(sx as f32, sy as f32, "LeftButton", true);
    s.mouse_button(sx as f32, sy as f32, "LeftButton", false);
    // The row calls stock `SetRaidTargetIcon` (`TargetFrame.lua:486-492`), whose body is the
    // engine verb `SetRaidTarget(unit, 0 or index)`.
    assert_eq!(
        s.take_party_requests(),
        vec![benilla_ui::script::PartyRequest::SetRaidTarget {
            unit: "player".into(),
            index: 8
        }],
        "Skull queues the mark intent for the menu's unit"
    );
    // The click hides only its own list (`UIDropDownMenu.lua:495`), and a list's OnHide closes
    // only deeper levels (`UIDropDownMenu.xml:14-28`), so level 1 lingers until its 2 s timer.
    assert!(
        s.eval::<bool>("return not DropDownList2:IsVisible() and DropDownList1:IsVisible()")
            .unwrap(),
        "the click closes its own level; level 1 lingers (ref)"
    );
    // Two ticks: the list hides on the tick after its timer passes zero (`UIDropDownMenu.lua:75`).
    s.mouse_move(5.0, 5.0);
    s.tick(2.1);
    s.tick(0.1);
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "level 1 times out 2s after the pointer leaves the chain"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `TargetFrame_CheckLevel` (`TargetFrame.lua:119-142`) over stock `GetDifficultyColor`
/// (`QuestLogFrame.lua:14-20`, `:585-599`): an attackable target's number takes its difficulty
/// colour, and a corpse or `UnitLevel` −1 (hostile, 10+ up) shows the skull instead.
#[test]
fn shipped_target_frame_runs_the_level_law() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::PlayerReqState;
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    // `GetDifficultyColor`'s load chain: the stock quest log window.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\CharacterFrameTemplates.xml"); // the window tab
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    // Level 3 on both feeds, which the app keeps in step: the snapshot `UnitLevel("player")` reads
    // and the requirement state the −1 gate and `GetQuestGreenRange` read.
    s.set_player_req_state(PlayerReqState {
        level: 3,
        ..Default::default()
    });
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 3,
            is_player: true,
            player_controlled: true,
            health: 50,
            max_health: 50,
            ..UnitState::default()
        }),
    );

    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Elder Mottled Boar".into()),
            health: 40,
            max_health: 40,
            level: 8,
            reaction: 4,
            can_attack: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local lvl = getglobal("TargetLevelText")
            local skull = getglobal("TargetHighLevelTexture")
            local c = GetDifficultyColor(8)
            return lvl:IsShown() ~= nil and skull:IsShown() == nil
               and tostring(lvl:GetText()) == "8"
               and c.r == 1.00 and c.g == 0.10 and c.b == 0.10
        "#,
        )
        .unwrap();
    assert!(ok, "attackable +5: red number, no skull ({:?})", s.errors());

    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Guard".into()),
            health: 400,
            max_health: 400,
            level: 13,
            reaction: 2,
            can_attack: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local lvl = getglobal("TargetLevelText")
            local skull = getglobal("TargetHighLevelTexture")
            return UnitLevel("target") == -1
               and skull:IsShown() ~= nil and lvl:IsShown() == nil
        "#,
        )
        .unwrap();
    assert!(ok, "hostile +10: the skull shows ({:?})", s.errors());

    // A dead mob is not a corpse: `UnitIsCorpse` (`0x5161c0`) checks for a corpse object and
    // `UnitLevel` (`0x517fc0`) ignores health, so its number shows.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Elder Mottled Boar".into()),
            health: 0,
            max_health: 40,
            level: 8,
            reaction: 4,
            dead: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local lvl = getglobal("TargetLevelText")
            local skull = getglobal("TargetHighLevelTexture")
            return UnitIsCorpse("target") == nil
               and lvl:IsShown() ~= nil and skull:IsShown() == nil
               and tostring(lvl:GetText()) == "8"
        "#,
        )
        .unwrap();
    assert!(
        ok,
        "dead mob: the number shows, no skull ({:?})",
        s.errors()
    );

    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Corpse of Somebody".into()),
            level: 8,
            corpse_object: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let ok: bool = s
        .eval(
            r#"
            local skull = getglobal("TargetHighLevelTexture")
            return UnitIsCorpse("target") == 1 and skull:IsShown() ~= nil
        "#,
        )
        .unwrap();
    assert!(ok, "corpse object: the skull shows ({:?})", s.errors());

    // At level 30 `GetQuestGreenRange` is 7: 23 is still green, 22 grey.
    s.set_player_req_state(PlayerReqState {
        level: 30,
        ..Default::default()
    });
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 30,
            is_player: true,
            player_controlled: true,
            health: 50,
            max_health: 50,
            ..UnitState::default()
        }),
    );
    let ok: bool = s
        .eval(
            r#"
            local g, t = GetDifficultyColor(23), GetDifficultyColor(22)
            return GetQuestGreenRange() == 7
               and g.r == 0.25 and g.g == 0.75 and g.b == 0.25
               and t.r == 0.50 and t.g == 0.50 and t.b == 0.50
        "#,
        )
        .unwrap();
    assert!(ok, "green range boundary at level 30 ({:?})", s.errors());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The PvP icon's three branches (`PlayerFrame.lua:55-79`): FFA outranks the faction flag, which
/// needs a side, and `igPVPUpdate` sounds on `UNIT_FACTION`, not on every repaint.
#[test]
fn pvp_icon_follows_the_three_branch_law() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    let _ = s.take_sounds(); // the frames' own load-time kits (target hide) aren't ours

    let icon_shown = |s: &UiScript, unit: &str| -> bool {
        s.eval(&format!(
            "return {unit}PVPIcon:IsVisible() and true or false"
        ))
        .unwrap()
    };
    let icon_path = |s: &mut UiScript, needle: &str| -> bool {
        s.resolve();
        s.extract().into_iter().any(
            |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle)),
        )
    };

    let alliance_player = |pvp: bool, ffa: bool| UnitState {
        exists: true,
        name: Some("Benilla".into()),
        health: 50,
        max_health: 50,
        level: 10,
        is_player: true,
        player_controlled: true,
        faction_group: Some("Alliance".into()),
        pvp,
        is_pvp_ffa: ffa,
        ..UnitState::default()
    };

    s.set_unit("player", Some(alliance_player(false, false)));
    s.fire_event("UNIT_FACTION", vec![ScriptValue::Str("player".into())]);
    assert!(!icon_shown(&s, "Player"), "unflagged shows none");
    assert!(s.take_sounds().is_empty(), "no sound while unflagged");

    s.set_unit("player", Some(alliance_player(true, false)));
    s.fire_event("UNIT_FACTION", vec![ScriptValue::Str("player".into())]);
    assert!(icon_shown(&s, "Player"));
    assert!(icon_path(&mut s, "UI-PVP-Alliance"), "faction leg art");
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igPVPUpdate".into())],
        "the flag change sounds once"
    );

    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    assert!(s.take_sounds().is_empty(), "repaints don't re-sound");

    s.set_unit("player", Some(alliance_player(true, true)));
    s.fire_event("UNIT_FACTION", vec![ScriptValue::Str("player".into())]);
    assert!(icon_path(&mut s, "UI-PVP-FFA"), "FFA wins the branch");
    let _ = s.take_sounds();

    // No side, no icon however flagged: the `factionGroup and UnitIsPVP` gate.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Kobold Vermin".into()),
            health: 40,
            max_health: 40,
            level: 8,
            reaction: 2,
            pvp: true,
            faction_group: None,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(
        !icon_shown(&s, "Target"),
        "flagged but sideless draws no icon"
    );

    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Orgrimmar Grunt".into()),
            health: 40,
            max_health: 40,
            level: 8,
            reaction: 2,
            pvp: true,
            faction_group: Some("Horde".into()),
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(icon_shown(&s, "Target"));
    assert!(icon_path(&mut s, "UI-PVP-Horde"), "the target's own side");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A friendly player's plate is green when PvP-flagged, else blue (`TargetFrame.lua:163-172`).
#[test]
fn flagged_friendly_player_plate_is_green() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let plate_color = |s: &mut UiScript| -> [f32; 4] {
        s.resolve();
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Texture {
                    path: Some(p),
                    color: Some(c),
                    ..
                } if p.contains("LevelBackground") => Some(c),
                _ => None,
            })
            .expect("target name-plate quad present")
    };
    let friendly_player = |pvp: bool| UnitState {
        exists: true,
        name: Some("Guildmate".into()),
        health: 50,
        max_health: 50,
        level: 20,
        is_player: true,
        player_controlled: true,
        reaction: 5,
        can_attack: false,
        faction_group: Some("Alliance".into()),
        pvp,
        ..UnitState::default()
    };

    s.set_unit("target", Some(friendly_player(false)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let blue = plate_color(&mut s);
    assert!(
        blue[0].abs() < 1e-6 && blue[1].abs() < 1e-6 && (blue[2] - 1.0).abs() < 1e-6,
        "an unflagged friendly player is blue, got {blue:?}"
    );

    s.set_unit("target", Some(friendly_player(true)));
    s.fire_event("UNIT_FACTION", vec![ScriptValue::Str("target".into())]);
    let green = plate_color(&mut s);
    assert!(
        green[0].abs() < 1e-6 && (green[1] - 1.0).abs() < 1e-6 && green[2].abs() < 1e-6,
        "a PvP-flagged friendly player is green (UnitReactionColor[6]), got {green:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `TargetFrame_CheckClassification` (`TargetFrame.lua:205-218`), asserted on the drawn
/// quads: elite, rare-elite and world boss share the Elite art (1.12 ships no rare-elite border),
/// and `UNIT_CLASSIFICATION_CHANGED` alone repaints it.
#[test]
fn target_frame_border_follows_the_classification_law() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    // Every TargetingFrame border in the draw list; the player frame always adds the plain one.
    let borders = |s: &mut UiScript| -> Vec<String> {
        s.resolve();
        let mut v: Vec<String> = s
            .extract()
            .into_iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-TargetingFrame") && !p.contains("LevelBackground") =>
                {
                    Some(p.clone())
                }
                _ => None,
            })
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let has = |v: &[String], suffix: &str| v.iter().any(|p| p.ends_with(suffix));

    let mob = |rank: u32| UnitState {
        exists: true,
        name: Some("Ol' Sooty".into()),
        health: 400,
        max_health: 400,
        level: 26,
        reaction: 2,
        can_attack: true,
        rank,
        ..UnitState::default()
    };

    s.set_unit("target", Some(mob(0)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let v = borders(&mut s);
    assert_eq!(
        s.eval::<String>(r#"return UnitClassification("target")"#)
            .unwrap(),
        "normal"
    );
    assert!(
        !has(&v, "UI-TargetingFrame-Elite") && !has(&v, "UI-TargetingFrame-Rare"),
        "rank 0 wears the plain border, got {v:?}"
    );

    for (rank, word) in [(1, "elite"), (2, "rareelite"), (3, "worldboss")] {
        s.set_unit("target", Some(mob(rank)));
        s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
        let v = borders(&mut s);
        assert_eq!(
            s.eval::<String>(r#"return UnitClassification("target")"#)
                .unwrap(),
            word
        );
        assert!(
            has(&v, "UI-TargetingFrame-Elite") && !has(&v, "UI-TargetingFrame-Rare"),
            "rank {rank} ({word}) takes the Elite border, got {v:?}"
        );
    }

    s.set_unit("target", Some(mob(4)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let v = borders(&mut s);
    assert_eq!(
        s.eval::<String>(r#"return UnitClassification("target")"#)
            .unwrap(),
        "rare"
    );
    assert!(
        has(&v, "UI-TargetingFrame-Rare") && !has(&v, "UI-TargetingFrame-Elite"),
        "rank 4 takes the Rare border, got {v:?}"
    );

    // A creature query landing on the current target raises its rank; the event alone repaints.
    s.set_unit("target", Some(mob(0)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(
        !has(&borders(&mut s), "UI-TargetingFrame-Elite"),
        "precondition: plain border before the query lands"
    );
    s.set_unit("target", Some(mob(1)));
    s.fire_event(
        "UNIT_CLASSIFICATION_CHANGED",
        vec![ScriptValue::Str("target".into())],
    );
    assert!(
        has(&borders(&mut s), "UI-TargetingFrame-Elite"),
        "the event alone repaints the border"
    );

    let plain_on_player: bool = s
        .eval(
            r#"
            local p = getglobal("PlayerFrameTexture")
            return p ~= nil and PlayerFrame.frameTexture == nil
        "#,
        )
        .unwrap();
    assert!(
        plain_on_player,
        "the player frame has the region but caches no handle, so nothing can swap it"
    );

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The ring art paints over the bar fills. Within a frame level the draw layer outranks the frame,
/// so BACKGROUND art clears ARTWORK fills only from a higher level: the player's art hangs two
/// frames down (`PlayerFrame.xml:50-55`), the target's bars drop one (`TargetFrame.lua:32-34`).
#[test]
fn the_ring_art_paints_over_the_bars() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Onemage".into()),
            health: 60,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 60,
            max_power: 100,
            is_connected: true, // else the power bar takes the disconnect leg's grey max fill
            ..UnitState::default()
        }),
    );
    // The name repaints on `PLAYER_ENTERING_WORLD` (`PlayerFrame.lua:96-100`), not `UNIT_HEALTH`.
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
    let quads = s.extract();

    // The ring art is `UI-TargetingFrame` exactly, not its `-LevelBackground` or `-Elite` siblings.
    let ring = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-TargetingFrame"))
        })
        .expect("the player frame's ring art");
    let fill = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-StatusBar"))
        })
        .map(|q| q.z)
        .max()
        .expect("a bar fill");
    let name = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Onemage"))
        .expect("the name text");

    assert!(
        ring.z > fill,
        "the ring art must paint OVER the bar fills (ring z={:#x}, fill z={fill:#x})",
        ring.z
    );
    assert!(
        name.z > fill,
        "the name text must paint OVER the bar fills (name z={:#x}, fill z={fill:#x})",
        name.z
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The same on the party member frames, which reach it through their own template.
#[test]
fn the_party_art_paints_over_the_bars() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\ReputationFrame.lua"); // FACTION_BAR_COLORS
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    // Before UnitPopup, which reads `TEXT` and `ITEM_QUALITY_COLORS` (`UIParent.lua:65`) at load.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitPopup.xml");
    // The stock kit in the manifest's order. `TargetofTargetTextureFrame`'s OnLoad calls
    // `RaiseFrameLevel` (`UIParent.lua:1894-1896`), loaded above.
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\BuffFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\CombatFeedback.xml");
    load_xml(&s, "Interface\\FrameXML\\PlayerFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PartyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\TargetFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PetFrame.xml");

    s.set_unit(
        "party1",
        Some(UnitState {
            exists: true,
            name: Some("Onepriest".into()),
            health: 60,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 60,
            max_power: 100,
            is_connected: true,
            ..UnitState::default()
        }),
    );
    // The roster shows a party row, not the unit: `PartyMemberFrame_UpdateMember` gates on
    // `GetPartyMember(id)` and hides the row otherwise (`PartyMemberFrame.lua:42-57`).
    s.set_party(benilla_ui::script::PartyState {
        members: vec![benilla_ui::script::PartyMemberInfo {
            name: "Onepriest".into(),
            guid: 0x0_0B12,
        }],
        leader_index: 0,
        leader_guid: 0,
        own_guid: 0,
        raid: Vec::new(),
        loot_method: "group".into(),
        master_looter: None,
        loot_threshold: 2,
    });
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    s.resolve();
    let quads = s.extract();

    // Scoped to member frame 1 by owner, so no other frame's `UI-StatusBar` stands in.
    let mine = |q: &benilla_ui::script::ExtractedQuad| {
        s.quad_owner_name(q.target)
            .is_some_and(|n| n.starts_with("PartyMemberFrame1"))
    };
    // The art's owner has no name, two anonymous frames down (`PartyFrameTemplates.xml:235-240`):
    // rows 2-4 are hidden, so the one `UI-PartyFrame` quad is checked against member 1's rect.
    let art_quads: Vec<_> = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-PartyFrame"))
        })
        .collect();
    assert_eq!(
        art_quads.len(),
        1,
        "one member in the party, one row of art drawn"
    );
    let art = art_quads[0];
    let row: Vec<f32> = s
        .eval(
            "return { PartyMemberFrame1:GetLeft(), PartyMemberFrame1:GetBottom(), \
                      PartyMemberFrame1:GetRight(), PartyMemberFrame1:GetTop() }",
        )
        .unwrap();
    let r = art.rect.expect("the art quad has a resolved rect");
    assert!(
        r.left >= row[0] - 1.0 && r.right <= row[2] + 1.0 && r.top <= row[3] + 3.0,
        "the art quad sits on PartyMemberFrame1 (art {r:?}, row {row:?})"
    );
    let fill = quads
        .iter()
        .filter(|q| {
            mine(q)
                && matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-StatusBar"))
        })
        .map(|q| q.z)
        .max()
        .expect("a bar fill");
    assert!(
        art.z > fill,
        "the party art must paint OVER the bar fills (art z={:#x}, fill z={fill:#x})",
        art.z
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A feigning target: `UNIT_DYNFLAG_DEAD` zeroes `UnitHealth` (`0x5174d0`) and `UnitMana` but not
/// the maxima (`UnitHealthMax`, `0x5175b0`), so both bars run empty over a full track and the dead
/// text lights on `UnitHealth("target") <= 0` (`TargetFrame.lua:221`).
#[test]
fn a_feigning_target_paints_empty_bars_and_the_dead_text() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let hunter = |health: u32, power: u32, dead: bool| {
        Some(UnitState {
            exists: true,
            is_connected: true, // CheckDead's second term: a feign is not a disconnect
            name: Some("Corvane".into()),
            health,
            max_health: 1500,
            level: 60,
            power_type: 0,
            power,
            max_power: 900,
            dead,
            reaction: 2, // hostile
            ..UnitState::default()
        })
    };

    s.set_unit("target", hunter(1200, 300, false));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let alive: bool = s
        .eval(
            r#"
            local hb, pb = TargetFrameHealthBar, TargetFrameManaBar
            return hb:GetValue() == 1200 and pb:GetValue() == 300
               and not TargetDeadText:IsShown()
        "#,
        )
        .unwrap();
    assert!(alive, "the control: a live hunter reads live");

    // He feigns. The reference fires both events on the flag's edge (`0x6004c5`, `0x6004f0`);
    // each stock bar listens only for its own (`UnitFrame.lua:150-151`, `:190-199`).
    s.set_unit("target", hunter(0, 0, true));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("target".into())]);
    s.fire_event("UNIT_MANA", vec![ScriptValue::Str("target".into())]);
    let (hp, hmax, mana, mmax): (f64, f64, f64, f64) = (
        s.eval("return TargetFrameHealthBar:GetValue()").unwrap(),
        s.eval("local _, m = TargetFrameHealthBar:GetMinMaxValues() return m")
            .unwrap(),
        s.eval("return TargetFrameManaBar:GetValue()").unwrap(),
        s.eval("local _, m = TargetFrameManaBar:GetMinMaxValues() return m")
            .unwrap(),
    );
    assert_eq!((hp, hmax), (0.0, 1500.0), "empty health bar, real track");
    assert_eq!((mana, mmax), (0.0, 900.0), "empty mana bar, real track");
    assert!(
        s.eval::<bool>("return TargetFrameManaBar:IsVisible()")
            .unwrap(),
        "the mana bar empties, it does not disappear — UnitManaMax 0x5177e0 is ungated"
    );
    assert!(
        s.eval::<bool>("return TargetDeadText:IsShown() and true or false")
            .unwrap(),
        "TargetFrame_CheckDead's UnitHealth(unit) <= 0 test, tripped by the flag"
    );
    assert_eq!(
        s.eval::<String>("return TargetDeadText:GetText()").unwrap(),
        "Dead",
        "the WORD is the GlobalString `DEAD` (l.898), never the key: a literal \"DEAD\" here \
         is the caps bug seen on Onyxia"
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsDead("target")"#).unwrap(),
        1,
        "UnitIsDead 0x517ac0's dynflag leg reaches the API too — as the number 1"
    );

    s.set_unit("target", hunter(1200, 300, false));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("target".into())]);
    s.fire_event("UNIT_MANA", vec![ScriptValue::Str("target".into())]);
    let up: bool = s
        .eval(
            r#"
            return TargetFrameHealthBar:GetValue() == 1200
               and TargetFrameManaBar:GetValue() == 300
               and not TargetDeadText:IsShown()
               and not UnitIsDead("target")
        "#,
        )
        .unwrap();
    assert!(up, "the feign ends and the frame reads live again");
}

/// Stock `PlayerFrame_UpdateStatus` (`PlayerFrame.lua:181-212`): resting shows the gold ring, the
/// zzz and its glow, pulsed on a 0.5 s wave by `PlayerFrame_OnUpdate`; auto-attack shows the red
/// ring, swords and disc; resting wins when both hold, and neither clears them all.
#[test]
fn the_player_frame_flashes_zzz_while_resting() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Prober".into()),
            health: 100,
            max_health: 100,
            level: 12,
            power_type: 0,
            power: 80,
            max_power: 80,
            dead: false,
            reaction: 0,
            ..UnitState::default()
        }),
    );

    // Into the inn.
    s.set_rest_state(1, 500, true);
    s.fire_event("PLAYER_UPDATE_RESTING", vec![]);
    let resting: bool = s
        .eval(
            r#"
            local u = "Player"
            return getglobal(u .. "StatusTexture"):IsShown()
               and getglobal(u .. "RestIcon"):IsShown()
               and not getglobal(u .. "AttackIcon"):IsShown()
               and PlayerStatusGlow:IsShown()
               and PlayerRestGlow:IsShown()
               and not PlayerAttackGlow:IsShown()
               and not getglobal(u .. "AttackBackground"):IsShown()
        "#,
        )
        .unwrap();
    assert!(
        resting,
        "resting shows the gold ring + zzz + glow, nothing red"
    );

    let a0: f64 = s.eval("return PlayerStatusTexture:GetAlpha()").unwrap();
    s.run("this = PlayerFrame; PlayerFrame_OnUpdate(0.25)")
        .unwrap();
    let a1: f64 = s.eval("return PlayerStatusTexture:GetAlpha()").unwrap();
    assert!(
        (a0 - a1).abs() > 0.1,
        "the flash moves the status alpha ({a0} → {a1})"
    );

    s.fire_event("PLAYER_ENTER_COMBAT", vec![]);
    assert!(
        s.eval::<bool>("return PlayerRestIcon:IsShown()").unwrap(),
        "resting outranks auto-attack"
    );

    // Out of the inn, still swinging.
    s.set_rest_state(2, 0, false);
    s.fire_event("PLAYER_UPDATE_RESTING", vec![]);
    let attacking: bool = s
        .eval(
            r#"
            local u = "Player"
            return getglobal(u .. "StatusTexture"):IsShown()
               and getglobal(u .. "AttackIcon"):IsShown()
               and not getglobal(u .. "RestIcon"):IsShown()
               and PlayerAttackGlow:IsShown()
               and getglobal(u .. "AttackBackground"):IsShown()
        "#,
        )
        .unwrap();
    assert!(attacking, "auto-attack shows the red ring + swords + disc");

    s.fire_event("PLAYER_LEAVE_COMBAT", vec![]);
    let clear: bool = s
        .eval(
            r#"
            local u = "Player"
            return not getglobal(u .. "StatusTexture"):IsShown()
               and not getglobal(u .. "RestIcon"):IsShown()
               and not getglobal(u .. "AttackIcon"):IsShown()
               and not PlayerStatusGlow:IsShown()
               and not getglobal(u .. "AttackBackground"):IsShown()
        "#,
        )
        .unwrap();
    assert!(clear, "neither state → no status dressing at all");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The zzz badge covers the level number: the number is in the BACKGROUND layer
/// (`PlayerFrame.xml:70`) and the badge in OVERLAY (`:171`), and text never ducks under a texture
/// of its own layer.
#[test]
fn the_rest_badge_covers_the_level_number() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Prober".into()),
            health: 100,
            max_health: 100,
            level: 12,
            power_type: 0,
            power: 80,
            max_power: 80,
            dead: false,
            reaction: 0,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.set_rest_state(1, 500, true);
    s.fire_event("PLAYER_UPDATE_RESTING", vec![]);
    s.resolve();
    let quads = s.extract();

    // Every `UI-StateIcon` quad, badge and glow: even the lowest must clear the number.
    let badge = quads
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("UI-StateIcon"))
        })
        .map(|q| q.z)
        .min()
        .expect("the zzz badge");
    let level = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "12"))
        .expect("the level text");
    assert!(
        badge > level.z,
        "the badge must cover the level number (badge z={badge:#x}, level z={:#x})",
        level.z
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The Status Bar Text switch pins numerals on the `textLockable` bars: the player's two
/// (`PlayerFrame.xml:302`, `:330`), the pet's two and the XP bar. The target's bars have no text
/// region (`TargetFrame.xml:486-487` names undeclared ones); a hover there shows the unit tooltip.
#[test]
fn status_bar_text_paints_the_player_numerals_but_not_the_targets() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let alive = |health: u32, power: u32, power_type: u8| UnitState {
        exists: true,
        name: Some("Somebody".into()),
        health,
        max_health: 100,
        level: 12,
        power_type,
        power,
        max_power: 80,
        dead: false,
        reaction: 4,
        // Else the disconnect leg pins the bar full, with no prefix (`UnitFrame.lua:214-220`).
        is_connected: true,
        ..UnitState::default()
    };
    s.set_unit("player", Some(alive(72, 45, 1))); // power_type 1 = RAGE
    s.set_unit("target", Some(alive(50, 20, 0)));
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);

    let text = |s: &UiScript, name: &str| -> String {
        s.eval::<String>(&format!("return tostring(({name}:GetText()) or \"\")"))
            .unwrap()
    };
    let shown = |s: &UiScript, name: &str| -> bool {
        s.eval::<bool>(&format!("return {name}:IsShown()")).unwrap()
    };

    assert!(!shown(&s, "PlayerFrameHealthBarText"));
    assert!(!shown(&s, "PlayerFrameManaBarText"));

    // On, through the switch's own event, with no repaint.
    s.register_cvars([("statusBarText", "0")]);
    s.run("SetCVar(\"statusBarText\", \"1\", \"STATUS_BAR_TEXT\")")
        .unwrap();
    s.tick(0.0);
    assert!(
        shown(&s, "PlayerFrameHealthBarText"),
        "your own health numerals pin on"
    );
    // No prefix: `CharacterFrame_OnLoad` sets "Health" (`CharacterFrame.lua:55`) and this fixture
    // has no character window; a full run reads "Health 72 / 100".
    assert_eq!(
        text(&s, "PlayerFrameHealthBarText"),
        "72 / 100",
        "no prefix without the character window, which is what sets HEALTH"
    );
    // `UnitFrame_UpdateManaType` sets the power bar's prefix on every update (`UnitFrame.lua:129`).
    assert_eq!(
        s.eval::<String>("return PlayerFrameManaBar.prefix")
            .unwrap(),
        "Rage",
        "the prefix follows the resource — this player runs on rage"
    );
    // The string still lacks it, as in the reference: `CVAR_UPDATE` only shows the string
    // (`TextStatusBar.lua:14-24`), and its last render ran before the prefix was set
    // (`UnitFrame.lua:219-220`) with the switch off (`:130-132`). The next repaint labels it.
    assert_eq!(
        text(&s, "PlayerFrameManaBarText"),
        "45 / 80",
        "flipping the switch shows the string, it does not re-render it"
    );

    assert!(
        s.eval::<bool>(
            "return TargetFrameHealthBarText == nil and TargetFrameManaBarText == nil \
             and TargetFrameHealthBar.TextString == nil and TargetFrameManaBar.TextString == nil"
        )
        .unwrap(),
        "the reference declares no numerals on the target frame, at any switch setting"
    );

    // Through the real pointer path: the tooltip arm reads `this` (`TextStatusBar.xml:16-26`).
    hover(&mut s, "TargetFrameHealthBar");
    assert!(
        s.eval::<bool>(
            "return GameTooltip:IsShown() and GameTooltip:IsOwned(TargetFrameHealthBar)"
        )
        .unwrap(),
        "the target's bar hover is a tooltip, not numerals ({:?})",
        s.errors()
    );
    assert!(
        s.eval::<String>("return tostring(GameTooltipTextLeft1:GetText())")
            .unwrap()
            .contains("Somebody"),
        "and it is the unit's own plate"
    );
    unhover(&mut s);
    assert!(
        s.eval::<bool>("return not GameTooltip:IsShown()").unwrap(),
        "the template's OnLeave hides it (ref TextStatusBar.xml l.28-31)"
    );

    // Switched off, a hover still reveals the player's numerals (`TextStatusBar.lua:77-102`).
    s.run("SetCVar(\"statusBarText\", \"0\", \"STATUS_BAR_TEXT\")")
        .unwrap();
    s.tick(0.0);
    assert!(!shown(&s, "PlayerFrameHealthBarText"));
    hover(&mut s, "PlayerFrameHealthBar");
    assert!(
        shown(&s, "PlayerFrameHealthBarText"),
        "hover shows them even with the option off ({:?})",
        s.errors()
    );
    unhover(&mut s);
    assert!(
        !shown(&s, "PlayerFrameHealthBarText"),
        "and go away again — the lockShow refcount balances"
    );
    s.run("SetCVar(\"statusBarText\", \"1\", \"STATUS_BAR_TEXT\")")
        .unwrap();
    s.tick(0.0);

    s.set_unit("player", Some(alive(31, 45, 1)));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    assert_eq!(text(&s, "PlayerFrameHealthBarText"), "31 / 100");

    // `UNIT_DISPLAYPOWER` re-labels through `UnitFrame_UpdateManaType` (`UnitFrame.lua:40-43`).
    s.set_unit("player", Some(alive(31, 60, 3)));
    s.fire_event("UNIT_DISPLAYPOWER", vec![ScriptValue::Str("player".into())]);
    assert_eq!(text(&s, "PlayerFrameManaBarText"), "Energy 60 / 80");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The numeral strings sit the reference's distance apart: the pet's bars are 70×8 at (47,-22) and
/// (47,-29), a 7px pitch, so its mana text drops clear to (82,-38) under a bar that ends at -37
/// (`PetFrame.xml:87-104`).
#[test]
fn no_two_numeral_strings_overlap_on_any_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let alive = |health: u32, power: u32| UnitState {
        exists: true,
        name: Some("Onehunter".into()),
        health,
        max_health: health,
        level: 40,
        power_type: 0,
        power,
        max_power: power,
        dead: false,
        reaction: 4,
        ..UnitState::default()
    };
    s.set_unit("player", Some(alive(4122, 3300)));
    s.set_unit("target", Some(alive(4122, 3300)));
    s.set_unit("pet", Some(alive(256, 100)));
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    s.register_cvars([("statusBarText", "0")]);
    s.run("SetCVar(\"statusBarText\", \"1\", \"STATUS_BAR_TEXT\")")
        .unwrap();
    s.tick(0.0);

    // Measure at NumberFontNormal's size (14px tall, about 7px a digit): unmeasured, a
    // CENTER-anchored string takes its owner's rect. Answers land a frame late, so settle.
    for _ in 0..4 {
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .into_iter()
            .map(|r| (r.id, r.text.chars().count() as f32 * 7.0, 14.0, r.key))
            .collect();
        s.set_measured_text_unwrapped(&answers);
        s.tick(0.05);
        s.resolve();
    }

    // The check is the seats' separation, not box overlap: the player's seats are 12px apart
    // (`PlayerFrame.xml:79-96`) at a 14px font, so their boxes overlap while the ink does not.
    for (frame, upper, lower, want) in [
        (
            "player",
            "PlayerFrameHealthBarText",
            "PlayerFrameManaBarText",
            12.0,
        ),
        ("pet", "PetFrameHealthBarText", "PetFrameManaBarText", 11.0),
    ] {
        let mid = |s: &UiScript, n: &str| -> f64 {
            s.eval::<f64>(&format!("return ({n}:GetTop() + {n}:GetBottom()) / 2"))
                .unwrap()
        };
        let apart = mid(&s, upper) - mid(&s, lower);
        assert!(
            (apart - want).abs() < 0.51,
            "{frame}: the numerals sit {apart:.1} px apart, the reference authors {want:.0}"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Addons hook `UnitFrame_OnEnter` by capturing and replacing the global
/// (`TipBuddy.lua:2770-2773`), so the stock `this`-shaped names must exist.
#[test]
fn the_unit_frame_hover_hooks_carry_the_references_names() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_unit_frames(&s);

    assert!(
        s.eval::<bool>(
            "return type(UnitFrame_OnEnter) == 'function' \
             and type(UnitFrame_OnLeave) == 'function'"
        )
        .unwrap(),
        "the reference's names must exist for an addon to capture"
    );

    s.run(
        r#"
        HOVERS = 0
        local original = UnitFrame_OnEnter
        function UnitFrame_OnEnter()
            HOVERS = HOVERS + 1
            original()
        end
        this = PlayerFrame
        UnitFrame_OnEnter()
    "#,
    )
    .unwrap();

    assert_eq!(
        s.eval::<i64>("return HOVERS").unwrap(),
        1,
        "the addon's replacement must run"
    );
    assert!(
        s.errors().is_empty(),
        "and calling through to the original must not raise: {:?}",
        s.errors()
    );
}

/// Stock `SetRaidTargetIconTexture` (`TargetFrame.lua:475-484`) puts each mark on its cell of the
/// 4×4 sheet; its row divides by `ROWS`, not `COLUMNS`, which is harmless at 4×4.
#[test]
fn the_raid_mark_helper_maps_each_index_to_its_cell() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_unit_frames(&s);
    s.run(r#"RTMark = UIParent:CreateTexture("RTMark", "OVERLAY")"#)
        .unwrap();

    // (index, left, right, top, bottom): 0.25 per cell, four across, then the next row.
    let cells = [
        (1, 0.00, 0.25, 0.00, 0.25), // star
        (2, 0.25, 0.50, 0.00, 0.25), // circle
        (3, 0.50, 0.75, 0.00, 0.25), // diamond
        (4, 0.75, 1.00, 0.00, 0.25), // triangle
        (5, 0.00, 0.25, 0.25, 0.50), // moon: the wrap onto row 2
        (6, 0.25, 0.50, 0.25, 0.50), // square
        (7, 0.50, 0.75, 0.25, 0.50), // cross
        (8, 0.75, 1.00, 0.25, 0.50), // skull
    ];
    for (i, l, r, t, b) in cells {
        s.run(&format!("SetRaidTargetIconTexture(RTMark, {i})"))
            .unwrap();
        // `GetTexCoord` answers eight values (UL, LL, UR, LR as x,y pairs); the `(l, r, t, b)`
        // rect is `ULx, URx, ULy, LLy`, positions 1, 5, 2, 4.
        let (gl, gt, _, gb, gr, ..): (f64, f64, f64, f64, f64, f64, f64, f64) =
            s.eval("return RTMark:GetTexCoord()").unwrap();
        assert_eq!(
            (gl, gr, gt, gb),
            (l, r, t, b),
            "mark {i} must sample the cell at ({l}, {r}, {t}, {b})"
        );
    }
    assert!(s.errors().is_empty(), "no errors: {:?}", s.errors());
}

/// Stock `PlayerFrame_UpdatePartyLeader` (`PlayerFrame.lua:39-53`): the leader icon asks
/// `IsPartyLeader()`, the master icon asks that `GetLootMethod`'s party index is 0, the player's
/// own seat, and that we are grouped.
#[test]
fn the_player_frame_wears_the_leader_and_master_looter_icons() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{PartyMemberInfo, PartyState};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Aldwyn".into()),
            health: 100,
            max_health: 100,
            level: 60,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);

    let shown = |s: &UiScript, region: &str| -> bool {
        s.eval::<bool>(&format!("return {region}:IsVisible()"))
            .unwrap()
    };
    let leader = "PlayerLeaderIcon";
    let master = "PlayerMasterIcon";

    assert!(!shown(&s, leader), "solo: no leader icon");
    assert!(!shown(&s, master), "solo: no master-looter icon");

    let party = |leader_index: u32, master_looter: Option<u32>, method: &str| PartyState {
        members: vec![PartyMemberInfo {
            name: "Brisca".into(),
            guid: 0x7A17,
        }],
        leader_index,
        // Follows `leader_index`: 0 = the player (unset here), else the member who leads.
        leader_guid: if leader_index == 0 { 0 } else { 0x7A17 },
        own_guid: 0,
        raid: Vec::new(),
        loot_method: method.into(),
        master_looter,
        loot_threshold: 2,
    };

    s.set_party(party(0, None, "group"));
    s.fire_event("PARTY_LEADER_CHANGED", vec![]);
    assert!(shown(&s, leader), "we lead: the leader icon shows");
    assert!(!shown(&s, master), "group loot: no master-looter icon");

    s.set_party(party(0, Some(0), "master"));
    s.fire_event("PARTY_LOOT_METHOD_CHANGED", vec![]);
    assert!(shown(&s, master), "we are master looter: the icon shows");

    s.set_party(party(0, Some(1), "master"));
    s.fire_event("PARTY_LOOT_METHOD_CHANGED", vec![]);
    assert!(!shown(&s, master), "somebody else masters: our icon hides");
    assert!(shown(&s, leader), "…and we still lead");

    s.set_party(party(1, Some(1), "master"));
    s.fire_event("PARTY_LEADER_CHANGED", vec![]);
    assert!(!shown(&s, leader), "we no longer lead: the crown hides");

    s.set_party(PartyState::default());
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    assert!(
        !shown(&s, leader) && !shown(&s, master),
        "ungrouped: both hide"
    );

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The globals the stock unit-frame files declare exist, 1.12's `ManaBar` names and never the later
/// `PowerBar`: addons read them unguarded (ShaguTweaks' `health-numbers.lua:22`). The list is the
/// reference's own, so additions pass and only an absence fails.
#[test]
fn the_unit_frames_publish_every_name_the_reference_declares() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_unit_frames(&s);

    // Names from `PlayerFrame.xml`, `TargetFrame.xml` and `PetFrame.xml`, less virtual templates
    // and the buff and debuff buttons; collected so a failure reports the whole gap.
    let mut missing = Vec::new();
    for name in [
        // PlayerFrame.xml
        "PlayerAttackBackground",
        "PlayerAttackGlow",
        "PlayerAttackIcon",
        "PlayerFrame",
        "PlayerFrameBackground",
        "PlayerFrameGroupIndicator",
        "PlayerFrameGroupIndicatorLeft",
        "PlayerFrameGroupIndicatorMiddle",
        "PlayerFrameGroupIndicatorRight",
        "PlayerFrameGroupIndicatorText",
        "PlayerFrameHealthBar",
        "PlayerFrameHealthBarText",
        "PlayerFrameManaBar",
        "PlayerFrameManaBarText",
        "PlayerFrameTexture",
        "PlayerHitIndicator",
        "PlayerLeaderIcon",
        "PlayerLevelText",
        "PlayerMasterIcon",
        "PlayerName",
        "PlayerPortrait",
        "PlayerPVPIcon",
        "PlayerRestGlow",
        "PlayerRestIcon",
        "PlayerStatusGlow",
        "PlayerStatusTexture",
        // TargetFrame.xml
        "TargetDeadText",
        "TargetFrame",
        "TargetFrameBackground",
        "TargetFrameHealthBar",
        "TargetFrameManaBar",
        "TargetFrameNameBackground",
        "TargetFrameTexture",
        "TargetFrameTextureFrame",
        "TargetHighLevelTexture",
        "TargetLevelText",
        "TargetName",
        "TargetPortrait",
        "TargetPVPIcon",
        // PetFrame.xml
        "PetAttackModeTexture",
        "PetFrame",
        "PetFrameHappiness",
        "PetFrameHappinessTexture",
        "PetFrameHealthBar",
        "PetFrameHealthBarText",
        "PetFrameManaBar",
        "PetFrameManaBarText",
        "PetFrameTexture",
        "PetName",
        "PetPortrait",
    ] {
        if !s
            .eval::<bool>(&format!("return getglobal('{name}') ~= nil"))
            .unwrap()
        {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "the reference declares these and we do not publish them — an addon reading any of them \
         by name finds nil: {missing:?}"
    );

    // The colour table is `ManaBarColor` (`UnitFrame.lua:2`); 1.12 has no `PowerBarColor`.
    assert!(s.eval::<bool>("return ManaBarColor ~= nil").unwrap());
    assert!(
        s.eval::<bool>("return PowerBarColor == nil").unwrap(),
        "PowerBarColor is the later client's name — 1.12 has no such global"
    );

    // The frame fields `UnitFrame_Initialize` sets (`UnitFrame.lua:13-14`), read off the frame.
    assert!(
        s.eval::<bool>("return PlayerFrame.manabar ~= nil and PlayerFrame.healthbar ~= nil")
            .unwrap(),
        "the reference's field names are `manabar`/`healthbar`, both lowercase"
    );

    // ShaguTweaks' own line, in shape (health-numbers.lua:22).
    s.run("TargetFrameManaBar:SetStatusBarColor(0, 0, 1)")
        .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `PLAYER_FLAGS_CHANGED`'s only 1.12 consumer, `TargetFrame.lua:88-95`, refreshes the target's
/// leader crown (1.12 has no AFK or DND badge on a unit frame), so leadership passing to the held
/// target shows without a re-target.
#[test]
fn the_target_leader_crown_follows_player_flags_changed() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_unit_frames(&s);

    let mate = |leader: bool| {
        Some(UnitState {
            exists: true,
            has_object: true,
            name: Some("Groupmate".into()),
            health: 100,
            max_health: 100,
            level: 30,
            power_type: 0,
            power: 50,
            max_power: 50,
            // The descriptor's leader bit (PLAYER_FLAGS 0x1) and its dword, moved together.
            group_leader: leader,
            player_flags: u32::from(leader),
            // A non-zero guid, or this cannot fail: `UnitIsPartyLeader` also compares the guid with
            // the group's leader guid, unguarded, and an empty group's leader guid is 0.
            guid: 0x4000_0000_0000_0009,
            ..UnitState::default()
        })
    };
    let crown =
        |s: &UiScript| -> bool { s.eval::<bool>("return TargetLeaderIcon:IsShown()").unwrap() };

    s.set_unit("target", mate(false));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!crown(&s), "not the leader: no crown");

    s.set_unit("target", mate(true));
    // The control first: the handler is gated on `arg1 == "target"`, so another token does nothing.
    s.fire_event(
        "PLAYER_FLAGS_CHANGED",
        vec![ScriptValue::Str("player".into())],
    );
    assert!(
        !crown(&s),
        "arg1 = \"player\" is not this frame's unit — the token gate is real"
    );

    s.fire_event(
        "PLAYER_FLAGS_CHANGED",
        vec![ScriptValue::Str("target".into())],
    );
    assert!(crown(&s), "the crown appears without a re-target");

    // And back down: the event fires on the flags' XOR-diff, so the clear edge fires it too.
    s.set_unit("target", mate(false));
    s.fire_event(
        "PLAYER_FLAGS_CHANGED",
        vec![ScriptValue::Str("target".into())],
    );
    assert!(!crown(&s), "leadership leaves and the crown goes with it");

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
