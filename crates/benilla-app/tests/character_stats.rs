//! Drives the REAL `Interface\FrameXML\PaperDollFrame.xml` stat rows through the engine — the
//! first test that executes the paper doll's own `PaperDollFrame_Set*` Lua at all, and since
//! decision 1751 that Lua is the reference's own rather than a transcription of it.
//!
//! That gap is why this file exists. The character sheet's whole buff-decomposition surface — the
//! green number, the red number, the `(base +x -y)` tooltip — sat broken from the day it shipped
//! because the four `POSSTAT`/`NEGSTAT`/`RESISTANCEBUFFMODS*` descriptor arrays were
//! decoded as f32 when the wire carries INT, so every real buff arrived as a rounded `0` and every
//! row took its "nothing is modifying this" leg. Both gates were green throughout: the bindings'
//! own unit tests fed the snapshot directly, and nothing ever ran the Lua that colours the row.
//!
//! So the assertions here are deliberately at the *rendered string*: what `CharacterStatFrameNStatText`
//! and `MagicResTextN` actually carry after a repaint, colour escapes included. Three shapes per
//! family — untouched (plain), buffed (green), debuffed (red) — plus the tooltip's base arithmetic,
//! which is the guard for `UnitStat`'s first return being the RAW field rather than a pre-subtracted
//! one (the ref Lua subtracts `posBuff`/`negBuff` itself, so a pre-subtracted first
//! return deducts the buff twice — a defect the f32 bug was hiding).

mod common;

use benilla_ui::script::{UiScript, UnitCombatStats, UnitState};

/// The paper doll's load prefix, in `assets/ui/benilla.toc` order.
///
/// **`CharacterFrame.xml` is deliberately not here.** These tests repaint the stat rows directly;
/// they never open the window, and `PaperDollFrame`'s only tie to its container is a `parent=`
/// name, which the loader warns about and falls back from. Pulling the container in would drag
/// `CharacterFrame_OnLoad`'s four external dependencies (the unit frames, the XP bar,
/// `TextStatusBar.lua`, the panel-tab kit — see `ui_script::test_ui::CHARACTER_UI`) into a test
/// about arithmetic.
///
/// `GlobalStrings.lua` and `BasicControls.xml` are not scenery either: the stock file has no
/// `X = X or "…"` fallbacks, and every row here formats a real string — `SPELL_STAT0_NAME`..`4`
/// through `TEXT()` (`PaperDollFrame.lua:143`), `RESISTANCE<n>_NAME` and
/// `RESISTANCE_TOOLTIP_SUBTEXT` (`:184`/`:224`), `ARMOR` and `ARMOR_TOOLTIP` (`:236`/`:249`).
const FILES: &[&str] = &[
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    "Interface\\FrameXML\\BasicControls.xml",
    // The reference's own since 1751 window 24 — `common::load_ui` speaks both stores.
    "Interface\\FrameXML\\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    // `Model_OnLoad` — the model pane's `<OnLoad>` calls it, so this is a LOAD-time dependency of
    // the paper doll, not of the window. The reference declares it in `UIParent.lua`; ours lives
    // in our counterpart of that file.
    r"Interface\FrameXML\UIParent.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine (1960)
    "Interface\\FrameXML\\GameTooltip.xml",
    // `CooldownFrameTemplate` and `CooldownFrame_SetTimer`. Not scenery: every one of the 20 slot
    // buttons runs `PaperDollItemSlotButton_Update` from its own OnLoad, which calls
    // `CooldownFrame_SetTimer` on `$parentCooldown` unconditionally (`PaperDollFrame.lua:692`) —
    // so leaving this out is 20 loader errors before the first assertion.
    "Interface\\FrameXML\\Cooldown.xml",
    "Interface\\FrameXML\\PaperDollFrame.xml",
];

fn load_ui(script: &UiScript) {
    for file in FILES {
        common::load_ui(script, file);
    }
}

/// The two colour escapes the sheet writes, as `Fonts.xml` defines them — asserted against the
/// rendered string rather than re-derived, so a palette change fails here loudly instead of
/// silently agreeing with itself.
const GREEN: &str = "|cff20ff20";
const RED: &str = "|cffff2020";

/// A level-60 body carrying one of each case: strength heavily geared (+105), agility cursed
/// (−12), stamina untouched, and the same three shapes across the resistance schools. The negative
/// halves are the wire words an **x86**-hosted server sends; an arm64 host saturates a debuff to a
/// flat `0` on the way out, which is exactly why the red leg has to be exercised
/// here and cannot be exercised against the local deploy.
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

/// Repaint the sheet the way an in-world `UNIT_STATS` does and read a row's rendered text back.
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

    // Strength: 225 total off a 120 base, +105 from gear. Green number, and the tooltip's
    // parenthesised base is the ONE subtraction the ref Lua performs (225 − 105 − 0 = 120).
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
    // Stamina has neither half — the ref's `posBuff == 0 and negBuff == 0` leg: a bare number with
    // no escape at all. This is the assertion the shipped bug would have PASSED, and it is here so
    // a future over-correction that colours everything is caught.
    let stamina = text_of(&mut s, "CharacterStatFrame3StatText");
    assert_eq!(
        stamina, "178",
        "an unmodified stat carries no colour escape at all"
    );
    // Red wins over green when both halves are in play — the ref colours on `negBuff < 0` first.
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

    // Resolve each row's school from the FRAME's own `id=`, never from a table: that is exactly
    // what the reference's `PaperDollFrame_SetResistances` does (`UnitResistance("player",
    // frame:GetID())`), and it means a row-order change cannot silently re-point these assertions.
    // It used to read our deleted file's `BENILLA_MAGICRES_IDS`; the stock XML carries the same
    // mapping on the frames themselves (`MagicResFrame1 id="6"` arcane, then 2..5 fire/nature/
    // frost/shadow — `PaperDollFrame.xml:654-758`).
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

/// **The melee block: the seven-value `UnitDamage` unpacked positionally by the reference's own
/// Lua**, and the arithmetic it does with the last three.
///
/// The stat rows above pin `UnitStat`/`UnitResistance`/`UnitArmor`. This one pins the family
/// decision 1793 warns about hardest, because its consumer is arithmetic rather than a lookup:
/// `PaperDollFrame_SetDamage` destructures **seven** values and immediately divides by the
/// seventh. One value short and `percent` is nil — `(minDamage / nil)` raises; one value long and
/// every term after it is off by a slot and the numbers come out plausible and wrong. Asserting
/// the rendered strings is what tells those two apart from correct.
///
/// The numbers below are the reference's own formula, not ours: `displayMin`/`displayMax` are the
/// raw range floored/ceiled, while the TOOLTIP range is the same numbers with the multiplier
/// divided out and the flat bonuses subtracted (`PaperDollFrame.lua:279-283`) — which is why
/// "40 - 62" and "27 - 47" are both right and different.
#[test]
fn the_melee_block_unpacks_seven_values_and_does_the_references_arithmetic() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = seated();
    // 40.5-61.5 at 2.6 s, +12/-3 flat and a x1.1 multiplier: every one of the seven slots is
    // distinct and non-zero, so a slot read from the wrong position cannot coincide with the
    // right answer.
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

    // The ROW: the raw range, green because the bonuses are net positive.
    assert_eq!(
        text_of(&mut s, "CharacterDamageFrameStatText"),
        format!("{GREEN}40 - 62|r")
    );
    // The TOOLTIP: the same range with the multiplier divided out and the flat bonuses removed,
    // then each modifier appended in the reference's own order and colours.
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
    // No offhand weapon: the reference NILS the offhand speed, which is what its hover branches on.
    assert!(s
        .eval::<Option<f64>>("return CharacterDamageFrame.offhandAttackSpeed")
        .unwrap()
        .is_none());

    // Attack power goes through `PaperDollFormatStat`, whose colour law is "red if anything is
    // negative, green otherwise" — so a mixed pair is RED even though the net is positive.
    assert_eq!(
        text_of(&mut s, "CharacterAttackPowerFrameStatText"),
        format!("{RED}800|r")
    );
    let ap_tip = tooltip_of(&mut s, "CharacterAttackPowerFrame");
    assert!(
        ap_tip.contains("800") && ap_tip.contains("(780") && ap_tip.contains("+30"),
        "the AP tooltip names the effective, the base and the buff, got {ap_tip:?}"
    );

    // The plain leg: no bonuses and no multiplier, so `totalBonus == 0` and the row carries no
    // colour escape at all — the branch a wrong `percent` would silently take.
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

/// **The ranged block with nothing in the ranged slot** — the `NOT_APPLICABLE` fallback all three
/// ranged rows share, and the `PaperDollFrame.noRanged` latch that carries it between them.
///
/// It is the reachable state for eight of nine classes at level 1, and it is the branch that
/// decides whether `PaperDollFrame_SetRangedDamage` unpacks `UnitRangedDamage`'s six values at
/// all — so it has to be pinned before the numbers are worth anything.
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
    // …and the hover has nothing to show, which is the guard
    // `CharacterRangedDamageFrame_OnEnter` opens with.
    assert!(s
        .eval::<Option<String>>("return CharacterRangedDamageFrame.damage")
        .unwrap()
        .is_none());
}

/// Armor is the one row of this family the local server never decomposes — item armor lands in
/// `UNIT_FIELD_RESISTANCES[0]` itself, not in the buff split — so its plain-number leg is the live
/// shape and worth pinning as such.
#[test]
fn armor_with_no_buff_split_renders_plain() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = seated();
    painted(&mut s);
    assert_eq!(text_of(&mut s, "CharacterArmorFrameStatText"), "2965");
}
