//! The eight stock micro buttons, loaded behind `MainMenuBarArtFrame`, which they anchor to.

use benilla_ui::script::{QuadContent, ScriptValue, TexCoords, UiScript, UnitState};

const ROW: [&str; 8] = [
    "CharacterMicroButton",
    "SpellbookMicroButton",
    "TalentMicroButton",
    "QuestLogMicroButton",
    "SocialsMicroButton",
    "WorldMapMicroButton",
    "MainMenuMicroButton",
    "HelpMicroButton",
];

/// At 1024 wide the 1024-wide bar sits at x 0, so every stock offset is a screen coordinate.
fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        r"Interface\FrameXML\UIParent.xml",
        // The labels each button's OnLoad reads, through `TEXT()`.
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
    ] {
        super::test_ui::load_ui(&s, file);
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

fn set_player_level(s: &mut UiScript, level: u32) {
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level,
            ..Default::default()
        }),
    );
    s.fire_event("UNIT_LEVEL", vec![ScriptValue::Str("player".into())]);
}

fn left_of(s: &UiScript, name: &str) -> f64 {
    s.eval::<f64>(&format!("return {name}:GetLeft()"))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// `CharacterMicroButton` sits at the art frame's `BOTTOMLEFT` +(552, 2), each button 29x58,
/// chained at -3 for a 26 px stride; `UpdateTalentButton` re-seats the quest log at -2
/// (`MainMenuBarMicroButtons.lua:139`), so past its first pass at 10+ the tail sits 1 px right.
#[test]
fn the_micro_row_sits_where_the_reference_puts_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.resolve();

    for (i, name) in ROW.iter().enumerate() {
        let (left, bottom, w, h) = s
            .eval::<(f64, f64, f64, f64)>(&format!(
                "return {name}:GetLeft(), {name}:GetBottom(), {name}:GetWidth(), {name}:GetHeight()"
            ))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((w, h), (29.0, 58.0), "{name} size");
        assert_eq!(
            left,
            552.0 + 26.0 * i as f64,
            "{name} left edge, as declared"
        );
        assert_eq!(bottom, 2.0, "{name} sits 2 above the bar's bottom");
    }

    set_player_level(&mut s, 60);
    s.resolve();
    for (i, name) in ROW.iter().enumerate().skip(3) {
        assert_eq!(
            left_of(&s, name),
            553.0 + 26.0 * i as f64,
            "{name} after the talent gate's own −2"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `HitRectInsets top="18"` (`MainMenuBarMicroButtons.xml:8`): the art fills the lower 40 of 58.
#[test]
fn the_transparent_top_of_a_micro_button_does_not_capture_the_mouse() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.resolve();

    // Mid-button (552 + 14); the art band is y 2..42, the inset header y 42..60.
    assert_eq!(
        s.hit_test_name(566.0, 20.0).as_deref(),
        Some("CharacterMicroButton"),
        "the art band takes the mouse"
    );
    assert_ne!(
        s.hit_test_name(566.0, 50.0).as_deref(),
        Some("CharacterMicroButton"),
        "the inset header must be transparent to the mouse"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `MicroButtonPortrait` samples the `"player"` portrait through a crop window, so the 18x25
/// region shows a face; pushing swaps the window and dims it (`MainMenuBarMicroButtons.lua:109`).
#[test]
fn the_character_button_carries_the_player_portrait_through_the_reference_crop() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.resolve();

    let window = |s: &mut UiScript| {
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Texture {
                    portrait_unit: Some(unit),
                    tex_coords,
                    circular,
                    ..
                } => Some((unit, tex_coords, circular)),
                _ => None,
            })
            .expect("the micro button's portrait quad")
    };

    let (unit, coords, circular) = window(&mut s);
    assert_eq!(unit, "player");
    assert!(
        circular,
        "the reference's round stencil lives in the bake's own UV space, so the crop below yields \
         a rectangular slice OF a masked face — not an ellipse fitted to this 18x25 region"
    );
    let round4 = |c: TexCoords| match c {
        TexCoords::Rect(e) => e.map(|v| (v * 10_000.0).round() / 10_000.0),
        TexCoords::Corners(_) => panic!("the 4-edge form"),
    };
    assert_eq!(
        round4(coords.expect("a crop window")),
        [0.2, 0.8, 0.0666, 0.9],
        "the normal window (CharacterMicroButton_SetNormal)"
    );

    s.run("CharacterMicroButton_SetPushed()").unwrap();
    s.resolve();
    let (_, coords, _) = window(&mut s);
    assert_eq!(
        round4(coords.expect("a crop window")),
        [0.2666, 0.8666, 0.0, 0.8333],
        "the held-down window (CharacterMicroButton_SetPushed)"
    );
    assert_eq!(
        s.eval::<f64>("return MicroButtonPortrait:GetAlpha()")
            .unwrap(),
        0.5,
        "…and the face dims while the button is down"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Under level 10 `UpdateTalentButton` hides the talent button and seats the quest log on its
/// anchor (`MainMenuBarMicroButtons.lua:133`).
#[test]
fn the_talent_button_appears_at_level_ten_and_the_row_closes_up_below_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    set_player_level(&mut s, 9);
    s.resolve();
    assert!(
        !s.eval::<bool>("return TalentMicroButton:IsVisible()")
            .unwrap(),
        "no talents before 10"
    );
    // The quest log takes the talent button's seat: slot 3, not 4.
    assert_eq!(left_of(&s, "QuestLogMicroButton"), 552.0 + 26.0 * 2.0);
    assert_eq!(
        left_of(&s, "HelpMicroButton"),
        552.0 + 26.0 * 6.0,
        "the whole tail moves up one slot with it"
    );

    set_player_level(&mut s, 10);
    s.resolve();
    assert!(
        s.eval::<bool>("return TalentMicroButton:IsVisible()")
            .unwrap(),
        "the button returns at 10"
    );
    assert_eq!(
        left_of(&s, "QuestLogMicroButton"),
        553.0 + 26.0 * 3.0,
        "…and the tail goes back out, at the gate's own −2"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The template's `OnEnter` (`MainMenuBarMicroButtons.xml:12`) reads `this`, so the hover goes
/// through the engine; a bound label gains its key on `UPDATE_BINDINGS`.
#[test]
fn every_micro_button_hovers_with_its_reference_explanation() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("SHOW_NEWBIE_TIPS = \"1\"").unwrap();
    s.register_bindings(&crate::bindings::registry_commands());
    assert_eq!(
        s.eval::<Option<i64>>("return SetBinding(\"C\", \"TOGGLECHARACTER0\")")
            .unwrap(),
        Some(1)
    );
    s.fire_event("UPDATE_BINDINGS", vec![]);
    s.resolve();

    for (button, label, newbie) in [
        (
            "CharacterMicroButton",
            "CHARACTER_BUTTON",
            "NEWBIE_TOOLTIP_CHARACTER",
        ),
        // The spellbook's own `OnEnter` (`MainMenuBarMicroButtons.xml:79`) picks its label.
        (
            "SpellbookMicroButton",
            "PlayerHasSpells() and SPELLBOOK_ABILITIES_BUTTON or ABILITYBOOK_BUTTON",
            "NEWBIE_TOOLTIP_SPELLBOOK",
        ),
        (
            "TalentMicroButton",
            "TALENTS_BUTTON",
            "NEWBIE_TOOLTIP_TALENTS",
        ),
        (
            "QuestLogMicroButton",
            "QUESTLOG_BUTTON",
            "NEWBIE_TOOLTIP_QUESTLOG",
        ),
        (
            "SocialsMicroButton",
            "SOCIAL_BUTTON",
            "NEWBIE_TOOLTIP_SOCIAL",
        ),
        (
            "WorldMapMicroButton",
            "WORLDMAP_BUTTON",
            "NEWBIE_TOOLTIP_WORLDMAP",
        ),
        (
            "MainMenuMicroButton",
            "MAINMENU_BUTTON",
            "NEWBIE_TOOLTIP_MAINMENU",
        ),
        ("HelpMicroButton", "HELP_BUTTON", "NEWBIE_TOOLTIP_HELP"),
    ] {
        super::test_ui::hover(&mut s, button);
        assert_eq!(
            s.eval::<i64>("return GameTooltip:NumLines()").unwrap(),
            2,
            "{button}: the label, then the explanation"
        );
        let label = s.eval::<String>(&format!("return {label}")).unwrap();
        let line1 = s
            .eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap();
        assert!(
            line1.starts_with(&label),
            "{button}: line 1 is {line1:?}, expected it to open with {label:?}"
        );
        assert_eq!(
            s.eval::<String>("return GameTooltipTextLeft2:GetText()")
                .unwrap(),
            s.eval::<String>(&format!("return {newbie}")).unwrap(),
            "{button}: line 2 is the reference's {newbie}, verbatim"
        );
        assert_eq!(
            s.eval::<i64>("return GameTooltip.default").unwrap(),
            1,
            "{button}: the default-corner anchor, not ANCHOR_RIGHT off the button"
        );
        super::test_ui::unhover(&mut s);
    }

    super::test_ui::hover(&mut s, "CharacterMicroButton");
    let line1 = s
        .eval::<String>("return GameTooltipTextLeft1:GetText()")
        .unwrap();
    assert_eq!(
        line1,
        s.eval::<String>(
            "return CHARACTER_BUTTON .. \" \" .. NORMAL_FONT_COLOR_CODE .. \"(C)\" .. FONT_COLOR_CODE_CLOSE"
        )
        .unwrap(),
        "the key read on UPDATE_BINDINGS"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The whole manifest, since `UpdateMicroButtons` reads every panel. A ding pulses the talent
/// button for 60 s unless the character sheet is open (`MainMenuBarMicroButtons.lua:120`).
#[test]
fn a_micro_button_pushes_while_its_panel_is_open_and_the_talent_button_pulses_on_a_ding() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // A player exists before the manifest loads; the sheet formats its race and class.
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            race: Some("Night Elf".into()),
            race_file: Some("NightElf".into()),
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            sex: 2,
            is_player: true,
            player_controlled: true,
            ..Default::default()
        }),
    );
    // The app runs GlobalStrings ahead of the manifest; the ding's chat lines format through it.
    super::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    assert!(super::load_default_ui(&s).is_empty());
    s.resolve();

    let state = |s: &UiScript, button: &str| {
        s.eval::<String>(&format!("return {button}:GetButtonState()"))
            .unwrap()
    };
    let face = |s: &UiScript| {
        s.eval::<f64>("return MicroButtonPortrait:GetAlpha()")
            .unwrap()
    };

    assert_eq!(state(&s, "CharacterMicroButton"), "NORMAL");
    assert_eq!(face(&s), 1.0);
    // The button's `OnClick` (`MainMenuBarMicroButtons.xml:49`); the sheet's `OnShow` calls back.
    s.run("ToggleCharacter(\"PaperDollFrame\")").unwrap();
    assert_eq!(
        state(&s, "CharacterMicroButton"),
        "PUSHED",
        "open sheet ⇒ button held down"
    );
    assert_eq!(
        face(&s),
        0.5,
        "…and the face dims (CharacterMicroButton_SetPushed)"
    );
    s.run("ToggleCharacter(\"PaperDollFrame\")").unwrap();
    assert_eq!(state(&s, "CharacterMicroButton"), "NORMAL");
    assert_eq!(face(&s), 1.0);

    s.run("ToggleWorldMap()").unwrap();
    assert_eq!(state(&s, "WorldMapMicroButton"), "PUSHED");
    s.run("ToggleWorldMap()").unwrap();
    assert_eq!(state(&s, "WorldMapMicroButton"), "NORMAL");

    // The nine args `ui_unit` fires: level, health, power, talent points, five stats.
    s.fire_event(
        "PLAYER_LEVEL_UP",
        vec![
            ScriptValue::Int(61),
            ScriptValue::Int(12),
            ScriptValue::Int(0),
            ScriptValue::Int(1),
            ScriptValue::Int(1),
            ScriptValue::Int(1),
            ScriptValue::Int(1),
            ScriptValue::Int(0),
            ScriptValue::Int(0),
        ],
    );
    assert_eq!(
        s.eval::<f64>("return TalentMicroButton.pulseTimeLeft")
            .unwrap(),
        60.0,
        "the sixty-second pulse"
    );
    assert!(
        s.eval::<bool>(
            "for _, b in ipairs(PULSEBUTTONS) do if b == TalentMicroButton then return true end end \
             return false"
        )
        .unwrap(),
        "on the pulse list"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
