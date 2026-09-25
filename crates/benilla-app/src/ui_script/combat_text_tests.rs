//! Scrolling combat text: the stock `Blizzard_CombatText` addon driven through the real loader.

use benilla_ui::script::{ScriptValue, UiScript, UnitState};

use super::test_ui::load_ui as load_xml;

/// The addon with `SHOW_COMBAT_TEXT` on; it ships `"0"`, and while it is off
/// `CombatText_UpdateDisplayedMessages` registers no events.
fn load_combat_text() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua"); // ENTERING_COMBAT & co.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml"); // TEXT()
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_CombatText");
    // The options window's defaults block (`UIOptionsFrame.lua:125`), seated by hand, master on.
    s.run(
        r#"SHOW_COMBAT_TEXT = "1"
           COMBAT_TEXT_SHOW_LOW_HEALTH_MANA = "1"
           COMBAT_TEXT_SHOW_AURAS = "1"
           COMBAT_TEXT_SHOW_AURA_FADE = "0"
           COMBAT_TEXT_SHOW_COMBAT_STATE = "1"
           COMBAT_TEXT_SHOW_DODGE_PARRY_MISS = "0"
           COMBAT_TEXT_SHOW_RESISTANCES = "0"
           COMBAT_TEXT_SHOW_REPUTATION = "0"
           COMBAT_TEXT_SHOW_REACTIVES = "0"
           COMBAT_TEXT_SHOW_FRIENDLY_NAMES = "0"
           COMBAT_TEXT_SHOW_COMBO_POINTS = "0"
           COMBAT_TEXT_SHOW_MANA = "0"
           COMBAT_TEXT_FLOAT_MODE = "1"
           COMBAT_TEXT_SHOW_HONOR_GAINED = "1"
           UIParentLoadAddOn("Blizzard_CombatText")
           CombatText_UpdateDisplayedMessages()"#,
    )
    .unwrap();
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// The stock envelope: height 25, a 1.9 s scroll life, a fade from 1.3 s.
#[test]
fn combat_text_damage_scrolls_and_expires() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combat_text();

    // The pool is `NUM_COMBAT_TEXT_LINES`, 20 strings.
    let hidden: bool = s
        .eval(
            r#"
            for i = 1, 20 do
                if getglobal("CombatText" .. i):IsShown() then return false end
            end
            return true
        "#,
        )
        .unwrap();
    assert!(hidden, "the pool starts hidden");

    s.fire_event(
        "COMBAT_TEXT_UPDATE",
        vec![
            ScriptValue::Str("DAMAGE".into()),
            ScriptValue::Str("17".into()),
        ],
    );
    let ok: bool = s
        .eval(
            r#"
            local t = CombatText1
            local _, h = t:GetFont()
            local y0 = COMBAT_TEXT_TO_ANIMATE[1].yPos
            return t:IsShown() ~= nil and t:GetText() == "-17" and h == 25 and y0 ~= nil
        "#,
        )
        .unwrap();
    assert!(ok, "damage paints -17 at height 25 ({:?})", s.errors());

    // Float mode 1 scrolls up, from y 384 to 609.
    s.tick(0.5);
    let ok: bool = s
        .eval(
            r#"
            local v = COMBAT_TEXT_TO_ANIMATE[1]
            return v.scrollTime > 0.4 and v.yPos > 384 and CombatText1:GetAlpha() == 1
        "#,
        )
        .unwrap();
    assert!(ok, "scrolled up, still opaque ({:?})", s.errors());

    s.tick(1.0); // 1.5 s total, fade began at 1.3
    let fading: bool = s
        .eval("return CombatText1:GetAlpha() < 1 and CombatText1:GetAlpha() > 0")
        .unwrap();
    assert!(fading, "fading past 1.3 s ({:?})", s.errors());

    // Expiry tests `scrollTime` before advancing it, so it lands a tick after crossing 1.9 s.
    s.tick(0.5);
    s.tick(0.1);
    let gone: bool = s
        .eval("return CombatText1:IsShown() == nil and getn(COMBAT_TEXT_TO_ANIMATE) == 0")
        .unwrap();
    assert!(gone, "expired at 1.9 s ({:?})", s.errors());

    s.fire_event(
        "COMBAT_TEXT_UPDATE",
        vec![
            ScriptValue::Str("MANA".into()),
            ScriptValue::Str("50".into()),
        ],
    );
    let none: bool = s.eval("return getn(COMBAT_TEXT_TO_ANIMATE) == 0").unwrap();
    assert!(none, "MANA is gated off by default");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A crit seeds at 30, peaks at 60 at 0.05 s, is back by 0.2 s, and parks: `endY` is `startY`.
#[test]
fn combat_text_crit_pops_and_parks() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combat_text();
    s.fire_event(
        "COMBAT_TEXT_UPDATE",
        vec![
            ScriptValue::Str("DAMAGE_CRIT".into()),
            ScriptValue::Str("64".into()),
        ],
    );
    let ok: bool = s
        .eval(
            r#"
            local v = COMBAT_TEXT_TO_ANIMATE[1]
            return CombatText1:GetText() == "-64" and v.endY == COMBAT_TEXT_LOCATIONS.startY
        "#,
        )
        .unwrap();
    assert!(ok, "crit paints and parks ({:?})", s.errors());
    assert_eq!(
        extracted_text_height(&mut s, "-64"),
        Some(30.0),
        "crit seeds at 30"
    );
    s.tick(0.1); // inside the shrink window (0.05..0.2): height strictly between 30 and 60
    let h = extracted_text_height(&mut s, "-64").expect("crit still drawn");
    assert!(
        h > 30.0 && h <= 60.0,
        "crit pop animates the height UNCAPPED past 32, got {h}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The drawn height: the `SetTextHeight` override on the matching text quad, not the font's 25.
fn extracted_text_height(s: &mut UiScript, text: &str) -> Option<f32> {
    s.resolve();
    s.extract().into_iter().find_map(|q| match q.content {
        benilla_ui::script::QuadContent::Text {
            text: Some(t),
            text_height,
            ..
        } if t == text => Some(text_height),
        _ => None,
    })?
}

/// Low health is `COMBAT_TEXT_LOW_HEALTH_THRESHOLD` (0.2), latched until health recovers above it.
#[test]
fn combat_text_state_and_low_health_triggers() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combat_text();
    s.fire_event("PLAYER_REGEN_DISABLED", vec![]);
    let ok: bool = s
        .eval("return CombatText1:GetText() == \"Entering Combat\"")
        .unwrap();
    assert!(ok, "entering combat paints ({:?})", s.errors());

    let mut player = UnitState {
        exists: true,
        health: 15,
        max_health: 100,
        ..UnitState::default()
    };
    s.set_unit("player", Some(player.clone()));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    let count: i64 = s
        .eval(
            r#"
            local n = 0
            for i = 1, 20 do
                if getglobal("CombatText" .. i):GetText() == "Health Low"
                    and getglobal("CombatText" .. i):IsShown() then
                    n = n + 1
                end
            end
            return n
        "#,
        )
        .unwrap();
    assert_eq!(
        count,
        1,
        "low health fires once, latched ({:?})",
        s.errors()
    );

    player.health = 90;
    s.set_unit("player", Some(player.clone()));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    player.health = 10;
    s.set_unit("player", Some(player));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    let count: i64 = s
        .eval(
            r#"
            local n = 0
            for i = 1, 20 do
                if getglobal("CombatText" .. i):GetText() == "Health Low"
                    and getglobal("CombatText" .. i):IsShown() then
                    n = n + 1
                end
            end
            return n
        "#,
        )
        .unwrap();
    assert_eq!(count, 2, "the latch re-armed ({:?})", s.errors());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `SetTextHeight` sizes are uncapped, so the pop peaks at 60. The VM's screen is the 768-high
/// virtual space, so the stock 384/609 locations apply verbatim.
#[test]
fn combat_text_crit_peak_is_uncapped() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combat_text();
    let ok: bool = s
        .eval("return COMBAT_TEXT_LOCATIONS.startY == 384 and COMBAT_TEXT_LOCATIONS.endY == 609")
        .unwrap();
    assert!(
        ok,
        "ref-verbatim locations in the 768 space ({:?})",
        s.errors()
    );
    s.fire_event(
        "COMBAT_TEXT_UPDATE",
        vec![
            ScriptValue::Str("DAMAGE_CRIT".into()),
            ScriptValue::Str("99".into()),
        ],
    );
    s.tick(0.05); // the scale window's end: SetTextHeight(60), the pop peak
    let h = extracted_text_height(&mut s, "-99").expect("crit drawn at the peak");
    // `floor()` at the f32 tick boundary lands 59 or 60.
    assert!((59.0..=60.0).contains(&h), "the pop peaks at ~60, got {h}");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Walks the switch down from on: a message paints first, so "nothing paints" after is the gate.
#[test]
fn combat_text_master_toggle_unregisters() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combat_text();
    s.fire_event(
        "COMBAT_TEXT_UPDATE",
        vec![
            ScriptValue::Str("DAMAGE".into()),
            ScriptValue::Str("17".into()),
        ],
    );
    let painted: bool = s
        .eval("return getn(COMBAT_TEXT_TO_ANIMATE) == 1 and CombatText1:IsShown() ~= nil")
        .unwrap();
    assert!(painted, "enabled: the message paints ({:?})", s.errors());

    s.run("SHOW_COMBAT_TEXT = \"0\"; CombatText_UpdateDisplayedMessages()")
        .unwrap();
    s.run("CombatText_ClearAnimationList()").unwrap();
    // `CombatText_ClearAnimationList` hides the strings but leaves the list to the ticker, so
    // "nothing paints" is the list not growing and no string shown.
    let before: i64 = s.eval("return getn(COMBAT_TEXT_TO_ANIMATE)").unwrap();
    s.fire_event(
        "COMBAT_TEXT_UPDATE",
        vec![
            ScriptValue::Str("DAMAGE".into()),
            ScriptValue::Str("17".into()),
        ],
    );
    let none: bool = s
        .eval(&format!(
            "return getn(COMBAT_TEXT_TO_ANIMATE) == {before} and CombatText1:IsShown() == nil"
        ))
        .unwrap();
    assert!(none, "disabled: nothing paints ({:?})", s.errors());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The reference reads the `DAMAGE_TEXT_FONT` global (`0x6c847c`) after the addons load
/// (`0x6c8470`), so an addon's `ADDON_LOADED` assignment wins over stock Friz.
#[test]
fn the_damage_text_font_is_read_from_the_lua_global() {
    let data = benilla_formats::wow_data_or_skip!();
    let _ = data;
    let s = UiScript::new().unwrap();

    // Unassigned, the reference's failure block (`0x704350`) leaves `0x703bf0`'s pre-seeded `""`,
    // which the font factory rejects.
    assert_eq!(crate::combat_text::read_damage_text_font(&s).0, None);

    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    assert_eq!(
        crate::combat_text::read_damage_text_font(&s).0.as_deref(),
        Some("Fonts\\FRIZQT__.TTF"),
    );

    s.run(r#"DAMAGE_TEXT_FONT = "Interface\\AddOns\\MSBT\\Fonts\\Adventure.ttf""#)
        .expect("the assignment runs");
    assert_eq!(
        crate::combat_text::read_damage_text_font(&s).0.as_deref(),
        Some("Interface\\AddOns\\MSBT\\Fonts\\Adventure.ttf"),
        "the addon's face, not Friz"
    );
}
