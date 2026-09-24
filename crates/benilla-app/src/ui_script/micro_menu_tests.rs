//! The stock `Interface\FrameXML\MainMenuBarMicroButtons.xml` — the eight micro buttons in the
//! main bar's right-hand recess — off the player's chain behind the stock bar (their anchor
//! target `MainMenuBarArtFrame`) into a bare engine.
//!
//! What these guard, in order: the row's geometry (29×58 at art-frame BOTTOMLEFT +(552,2), a
//! 26 px stride, and the reference's own 1 px nudge once the talent gate has run); the
//! `HitRectInsets` header that keeps a button's transparent top from eating the mouse; the
//! character button's portrait crop; `UpdateTalentButton`'s level-10 gate closing the row up
//! behind the hidden talent button; the two-line hover with the bound key in its label; and,
//! under the whole shipped manifest, `UpdateMicroButtons` following a real panel and the
//! level-up pulse the reference hangs on the talent button.

use benilla_ui::script::{QuadContent, ScriptValue, TexCoords, UiScript, UnitState};

/// The eight, in row order.
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

/// The stock bar, then the stock row, into a 1024×768 engine, asserting both load clean. 1024
/// wide is deliberate: the 1024-wide bar then centers at x=0, so every reference offset below is
/// also an absolute screen coordinate.
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
        // The eight labels and their NEWBIE_TOOLTIP_* lines, read at each button's OnLoad…
        "Interface\\FrameXML\\GlobalStrings.lua",
        // …through `TEXT()`.
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

/// Set the player's level and fire the `UNIT_LEVEL` the talent gate listens for.
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

/// The row's geometry, the reference's own numbers: CharacterMicroButton at the art frame's
/// BOTTOMLEFT +(552,2), each button 29×58, the rest chained BOTTOMLEFT-to-previous-BOTTOMRIGHT
/// +(-3,0) ⇒ a 26 px stride, so the eight run x 552..757 as declared — between the page arrows
/// (x≈522) and the bag cluster at the far right. **As declared** is the operative phrase: the XML
/// seats the quest-log button at −3 and `UpdateTalentButton` re-seats it at −2
/// (`MainMenuBarMicroButtons.lua:139`), so the first level pass at 10+ nudges the tail right by
/// one pixel. A real row is 1 px wider past its first `PLAYER_ENTERING_WORLD` than its XML
/// says; that is the client's, and it is asserted here as the reference's law rather than
/// sanded off (our retired file wrote −3 both ways).
#[test]
fn the_micro_row_sits_where_the_reference_puts_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.resolve();

    // As declared, before any level pass: a clean 26 px stride.
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

    // Past level 10 the gate re-seats the quest-log button at −2: the tail moves 1 px right.
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

/// The `HitRectInsets top="18"` header: the micro-button art fills only the lower ~40 of the 58,
/// and the empty top must stay transparent to the mouse — otherwise the row eats hover over the
/// bar's XP strip and the sliver of world above it.
#[test]
fn the_transparent_top_of_a_micro_button_does_not_capture_the_mouse() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.resolve();

    // Mid-button horizontally (552 + 14), inside the art band (y 2..42) ⇒ captures.
    assert_eq!(
        s.hit_test_name(566.0, 20.0).as_deref(),
        Some("CharacterMicroButton"),
        "the art band takes the mouse"
    );
    // Same column, in the dead 18-unit header (y 42..60) ⇒ nothing of the row captures it.
    assert_ne!(
        s.hit_test_name(566.0, 50.0).as_deref(),
        Some("CharacterMicroButton"),
        "the inset header must be transparent to the mouse"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The character button's face: `MicroButtonPortrait` binds the SAME `"player"` portrait slot
/// the unit frame samples, and carries the reference's crop window out to the renderer. Both
/// halves matter — the crop is what makes an 18×25 rectangle show a face instead of the whole
/// square bake squashed into it. Pushing the button swaps the window and dims it
/// (`CharacterMicroButton_SetPushed`, `MainMenuBarMicroButtons.lua:109-117`).
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

/// `UpdateTalentButton` (l.133-141): under level 10 the talent button is hidden and the quest
/// log button slides onto its seat, so the row has no hole in it; at 10 the button returns and
/// the tail shifts back out — by one stride less two, the gate's own −2.
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
    // The quest log button takes the talent button's own seat — slot 3, not slot 4.
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

/// The hover (the template's OnEnter, `MainMenuBarMicroButtons.xml:12-14`): every button's plate
/// is the reference's TWO-line `GameTooltip_AddNewbieTip` — the label, then that button's own
/// `NEWBIE_TOOLTIP_*` explanation in gold. 1.12 ships detailed tips ON (`SHOW_NEWBIE_TIPS = "1"`,
/// `UIOptionsFrame.lua:100`; ours sits in OptionsFrame.xml's uvar block), so the paragraph is the
/// DEFAULT hover. Through the engine's own hover, because the stock OnEnter reads
/// the firing frame off `this`. For the bound ones the label carries the key the way the
/// reference prints it — `GetBindingKey`'s raw token in `NORMAL_FONT_COLOR_CODE` parentheses
/// (`MicroButtonTooltipText`, l.11-18) — re-read on `UPDATE_BINDINGS`, which is the event
/// `bindings.rs` fires after every (re)load of the table.
#[test]
fn every_micro_button_hovers_with_its_reference_explanation() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("SHOW_NEWBIE_TIPS = \"1\"").unwrap();
    // A key the character button's label can carry: the registry, then the reference's own
    // `SetBinding`, then the event the row re-reads its labels on.
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
        // The spellbook's own OnEnter (l.79-87) picks its label by `PlayerHasSpells()`.
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

    // The bound one carries its key the reference's way: the raw token, gold, in parentheses,
    // after a space — `MicroButtonTooltipText`'s own concatenation.
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

/// Under the whole shipped manifest — the only VM in which every panel `UpdateMicroButtons`
/// (l.20-84) reads exists — a button is PUSHED exactly while its panel is up, the character
/// button's face dims with it, and the level-up pulse the reference hangs on the talent button
/// (`TalentMicroButton_OnEvent`, l.119-124: sixty seconds, unless the character sheet is open)
/// is live — 0304's "no microbutton pulse" scope-out, retired by the file that owns the pulse.
#[test]
fn a_micro_button_pushes_while_its_panel_is_open_and_the_talent_button_pulses_on_a_ding() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // A player always exists by the time the manifest loads (1051/1848) — with a race and a
    // class, which the sheet's own `PaperDollFrame_SetLevel` formats into its title on show.
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
    // The reference's strings, which the app runs ahead of the manifest (lifecycle.rs) — the
    // ding's chat lines below format `SPELL_STAT0_NAME`..`4` through them.
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
    // The button's own click (its OnClick, l.49-51) — the sheet's OnShow calls back.
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

    // The reference's own full-screen panel; its OnShow/OnHide call back the same way.
    s.run("ToggleWorldMap()").unwrap();
    assert_eq!(state(&s, "WorldMapMicroButton"), "PUSHED");
    s.run("ToggleWorldMap()").unwrap();
    assert_eq!(state(&s, "WorldMapMicroButton"), "NORMAL");

    // A ding with the sheet closed: sixty seconds on the pulse list (UiPanels.xml's
    // SetButtonPulse, the reference's UIParent.lua machinery). The nine args are the ones
    // `ui_unit` fires (level, health, power, talent points, five stats).
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
