//! Drives the stock `PaperDollFrame.xml` stat rows through the engine and asserts the rendered
//! strings, colour escapes included: untouched (plain), buffed (green) and debuffed (red), plus the
//! tooltip's base, which the stock Lua computes by subtracting both buff halves from `UnitStat`'s
//! raw first return.

mod common;

use benilla_ui::script::{UiScript, UnitCombatStats, UnitState};

/// The paper doll's load prefix, in `assets/ui/benilla.toc` order. `CharacterFrame.xml` is left
/// out: the rows are repainted directly and the window never opens. Every row formats a global
/// string: `SPELL_STAT<n>_NAME` (`PaperDollFrame.lua:140`), `RESISTANCE<n>_NAME` and
/// `RESISTANCE_TOOLTIP_SUBTEXT` (`:187`/`:226`), `ARMOR` and `ARMOR_TOOLTIP` (`:244`/`:249`).
const FILES: &[&str] = &[
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    // `Model_OnLoad` (`UIParent.lua:1421`), which the model pane's `<OnLoad>` calls at load.
    r"Interface\FrameXML\UIParent.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine
    "Interface\\FrameXML\\GameTooltip.xml",
    // Each slot button's OnLoad calls `CooldownFrame_SetTimer` (`PaperDollFrame.lua:692`).
    "Interface\\FrameXML\\Cooldown.xml",
    "Interface\\FrameXML\\PaperDollFrame.xml",
];

fn load_ui(script: &UiScript) {
    for file in FILES {
        common::load_ui(script, file);
    }
}

/// The sheet's two colour escapes, as `Fonts.xml` defines them.
const GREEN: &str = "|cff20ff20";
const RED: &str = "|cffff2020";

/// A level-60 body with a geared (+105), a cursed (−12) and an untouched stat, and the same three
/// shapes across the resistances. The negative halves are what an x86-hosted server sends; an
/// arm64 host saturates a debuff to 0.
fn stats() -> UnitCombatStats {
    UnitCombatStats {
        stats: [225, 68, 178, 34, 51],
        stat_pos: [105, 0, 0, 4, 0],
        stat_neg: [0, -12, 0, 0, 0],
        // schools: [0] armor, then holy/fire/nature/frost/shadow/arcane.
        resistances: [2965, 0, 65, 20, 0, 0, 0],
        resistance_pos: [0, 0, 65, 0, 0, 0, 0],
        resistance_neg: [0, 0, 0, -10, 0, 0, 0],
        ..Default::default()
    }
}

fn player() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Probefour".into()),
        level: 60,
        race: Some("Human".into()),
        class: Some("Warrior".into()),
        ..Default::default()
    }
}

/// Repaint the sheet as an in-world `UNIT_STATS` does.
fn painted(script: &mut UiScript) {
    script
        .run("PaperDollFrame_SetStats() PaperDollFrame_SetResistances() PaperDollFrame_SetArmor()")
        .expect("the paper doll's stat rows repaint");
}

fn text_of(script: &mut UiScript, region: &str) -> String {
    script
        .eval::<String>(&format!(r#"return getglobal("{region}"):GetText()"#))
        .unwrap_or_else(|e| panic!("reading {region}: {e}"))
}

fn tooltip_of_field(script: &mut UiScript, frame: &str, field: &str) -> String {
    script
        .eval::<String>(&format!(r#"return getglobal("{frame}").{field}"#))
        .unwrap_or_else(|e| panic!("reading {frame}.{field}: {e}"))
}

fn tooltip_of(script: &mut UiScript, frame: &str) -> String {
    script
        .eval::<String>(&format!(r#"return getglobal("{frame}").tooltip"#))
        .unwrap_or_else(|e| panic!("reading {frame}.tooltip: {e}"))
}

fn seated() -> UiScript {
    let mut script = UiScript::new().expect("a UI VM");
    load_ui(&script);
    script.set_unit("player", Some(player()));
    script.set_player_combat_stats(Some(stats()));
    script
}

#[test]
fn a_geared_stat_renders_green_and_its_tooltip_names_the_unbuffed_base() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = seated();
    painted(&mut s);

    // Strength 225 = 120 base + 105 gear; the stock Lua subtracts the buff once for the base.
    let text = text_of(&mut s, "CharacterStatFrame1StatText");
    assert!(
        text.starts_with(GREEN) && text.contains("225"),
        "a gear-boosted strength must render green, got {text:?}"
    );
    let tip = tooltip_of(&mut s, "CharacterStatFrame1");
    assert!(
        tip.contains("Strength 225") && tip.contains("(120") && tip.contains("+105"),
        "the tooltip must read the total, the unbuffed base and the delta, got {tip:?}"
    );
    assert!(
        !tip.contains("(15"),
        "base 15 = 225 − 105 − 105: the buff was deducted twice, so UnitStat's first return is \
         pre-subtracted. It must be the raw UNIT_FIELD_STAT — {tip:?}"
    );
}

#[test]
fn a_debuffed_stat_renders_red_and_an_untouched_one_stays_plain() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = seated();
    painted(&mut s);

    let agility = text_of(&mut s, "CharacterStatFrame2StatText");
    assert!(
        agility.starts_with(RED) && agility.contains("68"),
        "a cursed agility must render red, got {agility:?}"
    );
    // Stamina has neither half: the stock `posBuff == 0 and negBuff == 0` leg, no escape.
    let stamina = text_of(&mut s, "CharacterStatFrame3StatText");
    assert_eq!(
        stamina, "178",
        "an unmodified stat carries no colour escape at all"
    );
    // Red wins over green: the stock Lua tests `negBuff < 0` first.
    s.set_player_combat_stats(Some(UnitCombatStats {
        stat_pos: [105, 30, 0, 4, 0],
        ..stats()
    }));
    painted(&mut s);
    let mixed = text_of(&mut s, "CharacterStatFrame2StatText");
    assert!(
        mixed.starts_with(RED),
        "a stat with both a buff and a debuff renders RED, got {mixed:?}"
    );
}

#[test]
fn resistance_rows_colour_by_which_half_is_bigger() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = seated();
    painted(&mut s);

    // A row's school is its frame's `id=`, as `PaperDollFrame_SetResistances` reads it:
    // `MagicResFrame1` is 6 (arcane), then 2..5 (`PaperDollFrame.xml:654-738`).
    let row_of = |s: &mut UiScript, school: i64| -> usize {
        s.eval::<i64>(&format!(
            "for i = 1, NUM_RESISTANCE_TYPES do \
               if getglobal(\"MagicResFrame\"..i):GetID() == {school} then return i end \
             end return 0"
        ))
        .expect("the school→row map") as usize
    };

    let fire = row_of(&mut s, 2);
    let nature = row_of(&mut s, 3);
    assert!(fire > 0 && nature > 0, "fire and nature both have rows");

    let fire_text = text_of(&mut s, &format!("MagicResText{fire}"));
    assert!(
        fire_text.starts_with(GREEN) && fire_text.contains("65"),
        "a +65 fire resistance renders green, got {fire_text:?}"
    );
    let nature_text = text_of(&mut s, &format!("MagicResText{nature}"));
    assert!(
        nature_text.starts_with(RED),
        "a nature school whose debuff outweighs its buff renders red, got {nature_text:?}"
    );
}

/// `PaperDollFrame_SetDamage` unpacks `UnitDamage`'s seven values by position and divides by the
/// seventh. The row shows the raw range floored and ceiled; the tooltip range has the multiplier
/// divided out and the flat bonuses subtracted (`PaperDollFrame.lua:295-299`).
#[test]
fn the_melee_block_unpacks_seven_values_and_does_the_references_arithmetic() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = seated();
    // Every one of the seven slots is distinct and non-zero, so a misplaced slot cannot pass.
    s.set_player_combat_stats(Some(UnitCombatStats {
        main_attack_time_ms: 2600,
        min_damage: 40.5,
        max_damage: 61.5,
        physical_bonus_pos: 12,
        physical_bonus_neg: -3,
        damage_percent: 1.1,
        attack_power: 780,
        attack_power_pos: 30,
        attack_power_neg: -10,
        ..stats()
    }));
    s.run("PaperDollFrame_SetDamage() PaperDollFrame_SetAttackPower()")
        .expect("the melee rows repaint");

    // The row: the raw range, green because the bonuses are net positive.
    assert_eq!(
        text_of(&mut s, "CharacterDamageFrameStatText"),
        format!("{GREEN}40 - 62|r")
    );
    // The tooltip: the base range, then each modifier in the stock order and colours.
    assert_eq!(
        tooltip_of_field(&mut s, "CharacterDamageFrame", "damage"),
        format!("27 - 47{GREEN} +12|r{RED} -3|r{GREEN} x110%|r")
    );
    let speed: f64 = s
        .eval("return CharacterDamageFrame.attackSpeed")
        .expect("the hover's attack speed");
    assert!((speed - 2.6).abs() < 1e-6, "got {speed}");
    // dps = fullDamage / speed, fullDamage = (base + pos + neg) * percent = 51.0.
    let dps: f64 = s.eval("return CharacterDamageFrame.dps").expect("the dps");
    assert!((dps - 51.0 / 2.6).abs() < 0.01, "got {dps}");
    // No offhand weapon: the offhand speed is nil, which the hover branches on.
    assert!(s
        .eval::<Option<f64>>("return CharacterDamageFrame.offhandAttackSpeed")
        .unwrap()
        .is_none());

    // `PaperDollFormatStat` colours red if anything is negative, so a mixed pair is red.
    assert_eq!(
        text_of(&mut s, "CharacterAttackPowerFrameStatText"),
        format!("{RED}800|r")
    );
    let ap_tip = tooltip_of(&mut s, "CharacterAttackPowerFrame");
    assert!(
        ap_tip.contains("800") && ap_tip.contains("(780") && ap_tip.contains("+30"),
        "the AP tooltip names the effective, the base and the buff, got {ap_tip:?}"
    );

    // No bonuses and no multiplier: `totalBonus == 0`, no colour escape.
    s.set_player_combat_stats(Some(UnitCombatStats {
        main_attack_time_ms: 2600,
        min_damage: 40.5,
        max_damage: 61.5,
        damage_percent: 1.0,
        ..stats()
    }));
    s.run("PaperDollFrame_SetDamage()").unwrap();
    assert_eq!(text_of(&mut s, "CharacterDamageFrameStatText"), "40 - 62");
}

/// With the ranged slot empty, all three ranged rows read `NOT_APPLICABLE`, carried between them
/// by the `PaperDollFrame.noRanged` latch (`PaperDollFrame.lua:436-441`).
#[test]
fn the_ranged_rows_fall_back_to_not_applicable_with_an_empty_ranged_slot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = seated();
    s.run(
        "PaperDollFrame_SetRangedAttack() PaperDollFrame_SetRangedDamage()          PaperDollFrame_SetRangedAttackPower()",
    )
    .expect("the ranged rows repaint");

    let na: String = s
        .eval("return NOT_APPLICABLE")
        .expect("the ref's own string");
    for row in [
        "CharacterRangedAttackFrameStatText",
        "CharacterRangedDamageFrameStatText",
        "CharacterRangedAttackPowerFrameStatText",
    ] {
        assert_eq!(text_of(&mut s, row), na, "{row}");
    }
    // The guard `CharacterRangedDamageFrame_OnEnter` opens with.
    assert!(s
        .eval::<Option<String>>("return CharacterRangedDamageFrame.damage")
        .unwrap()
        .is_none());
}

/// Item armor lands in `UNIT_FIELD_RESISTANCES[0]` itself, not in the buff split.
#[test]
fn armor_with_no_buff_split_renders_plain() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = seated();
    painted(&mut s);
    assert_eq!(text_of(&mut s, "CharacterArmorFrameStatText"), "2965");
}
