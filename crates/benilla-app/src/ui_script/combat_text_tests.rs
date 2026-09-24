//! The center-screen scrolling combat text (decision 0578) — the Blizzard_CombatText
//! transcription driven end-to-end through the real loader: the COMBAT_TEXT_UPDATE pipeline,
//! the scroll/fade/crit-pop envelope, the option gating, and the Lua-side low-health trigger.

use benilla_ui::script::{ScriptValue, UiScript, UnitState};

use super::test_ui::load_ui as load_xml;

/// The window loaded **with the feature switched on** — which is not how it ships.
///
/// `SHOW_COMBAT_TEXT` boots at the reference's `"0"` since 1804 (it was `"1"` from 0578, when the
/// director's ask for scrolling combat text was read as the shipped experience). The master is
/// enforced at the source: `CombatText_UpdateDisplayedMessages` registers no events at all while
/// it is off, so a fresh VM answers every question below with "nothing happened". These tests are
/// about what the feature *does* once a player ticks the Combat page's box, so the harness ticks
/// it for them exactly the way that row does — assign the global, then re-run the family's
/// applyFunc. The gating itself is [`combat_text_master_toggle_unregisters`]'s subject, not theirs.
fn load_combat_text() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua"); // ENTERING_COMBAT & co.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml"); // TEXT()
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    // The reference's own addon, LoadOnDemand off the chain, loaded the way the options window
    // loads it (1964).
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_CombatText");
    // The family's saved-variable defaults are the options window's file-scope block (the
    // reference's UIOptionsFrame.lua l.135-152, carried by our OptionsFrame.xml since 1964); a
    // harness without that window seats the same block by hand, the master switched on.
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

/// A damage message paints "-N" red at the base height, scrolls upward over its 1.9 s life,
/// fades past 1.3 s, and frees its string at expiry. A gated type (MANA, var default "0")
/// shows nothing.
#[test]
fn combat_text_damage_scrolls_and_expires() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combat_text();

    // All 20 pool strings start hidden.
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

    // Half a second in: the string has scrolled upward (mode 1 flows up, 384 → 609).
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

    // Past the fade-out start: alpha drops below 1.
    s.tick(1.0); // 1.5 s total, fade began at 1.3
    let fading: bool = s
        .eval("return CombatText1:GetAlpha() < 1 and CombatText1:GetAlpha() > 0")
        .unwrap();
    assert!(fading, "fading past 1.3 s ({:?})", s.errors());

    // Past the 1.9 s scroll life: removed and hidden (the ref tests scrollTime BEFORE advancing,
    // so expiry lands on the tick after the threshold is crossed).
    s.tick(0.5);
    s.tick(0.1);
    let gone: bool = s
        .eval("return CombatText1:IsShown() == nil and getn(COMBAT_TEXT_TO_ANIMATE) == 0")
        .unwrap();
    assert!(gone, "expired at 1.9 s ({:?})", s.errors());

    // A var-gated type at its ref default "0" shows nothing.
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

/// A crit pops through the scale envelope: seeded at 30, grown toward 60 inside the first
/// 0.05 s, shrinking back toward 30 by 0.2 s — and it parks (endY = startY: the crit never
/// scrolls away from the seat).
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
        "crit pop animates the height UNCAPPED past 32 (decision 0582), got {h}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The drawn height via the engine extract (the SetTextHeight override on the matching Text
/// quad) — the two-regime split (decision 0582): the font object stays 25, the override is the
/// drawn size, uncapped.
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

/// The event-side triggers: PLAYER_REGEN_DISABLED paints "Entering Combat"; a sub-20% UNIT_HEALTH
/// paints "Health Low" once and re-arms only after recovering above the threshold.
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

    // Recover, then drop again: the latch re-arms.
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

/// The crit pop's PEAK reaches a true 60 (decision 0582: SetTextHeight sizes are uncapped —
/// the pre-split renderer clamped them to the 32-unit one-to-one cap, so crits never popped
/// past 32). The VM's screen is the 768-virtual space by construction (the app seam feeds it),
/// so the ref constants stand verbatim.
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
    // floor() at the f32 tick boundary lands 59 or 60 — the claim under test is that the peak
    // is UNCAPPED (the pre-split renderer clamped it to 32).
    assert!((59.0..=60.0).contains(&h), "the pop peaks at ~60, got {h}");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The master toggle: SHOW_COMBAT_TEXT = "0" + CombatText_UpdateDisplayedMessages unregisters
/// everything — a subsequent damage event paints nothing (ref-identical gating). Since 1804 that
/// "0" is also what a fresh client ships (it was 0578's named divergence, `"1"`, until then), so
/// the test walks the switch **down from the harness's planted ON**: it fires a damage message
/// first and watches it paint, because "nothing painted" only means the toggle worked if
/// something would otherwise have.
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

    // The switch, then the file's own list clear — the message already in flight belongs to the
    // planted ON state, and the claim under test is about what arrives AFTER the gate closes.
    s.run("SHOW_COMBAT_TEXT = \"0\"; CombatText_UpdateDisplayedMessages()")
        .unwrap();
    s.run("CombatText_ClearAnimationList()").unwrap();
    // The stock `CombatText_ClearAnimationList` hides the strings and leaves the list to the
    // ticker (our transcription had emptied it): "nothing paints" is the list not growing and no
    // string on screen.
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

/// **`DAMAGE_TEXT_FONT` is a Lua global the engine reads, not a hardcoded face** (decision 2156).
///
/// `0x6c847c` is the only instruction in `WoW.exe` that reads it, through `FrameScript_GetText`'s
/// fast arm into a plain `lua_gettable(LUA_GLOBALSINDEX)` — so whatever is in the global when the
/// world-entry load edge closes is what the numbers draw in. Stock `Fonts.xml` puts Friz there;
/// MikScrollingBattleText and pfUI put their own face there at `ADDON_LOADED`, which the reference
/// reads because `0x6c8470` runs *after* the addon load, not at CRT init.
///
/// Skips without client data.
#[test]
fn the_damage_text_font_is_read_from_the_lua_global() {
    let data = benilla_formats::wow_data_or_skip!();
    let _ = data;
    let s = UiScript::new().unwrap();

    // Before anything assigns it, there is nothing to bind — the reference's `0x704350` failure
    // block leaves `0x703bf0`'s pre-seeded `""`, and an empty name is what the font factory's own
    // guard rejects.
    assert_eq!(crate::combat_text::read_damage_text_font(&s).0, None);

    // Stock FrameXML, off the player's own chain: `Fonts.xml:6`.
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    assert_eq!(
        crate::combat_text::read_damage_text_font(&s).0.as_deref(),
        Some("Fonts\\FRIZQT__.TTF"),
    );

    // An addon assigning it at ADDON_LOADED — MikScrollingBattleText's line 255, in shape.
    s.run(r#"DAMAGE_TEXT_FONT = "Interface\\AddOns\\MSBT\\Fonts\\Adventure.ttf""#)
        .expect("the assignment runs");
    assert_eq!(
        crate::combat_text::read_damage_text_font(&s).0.as_deref(),
        Some("Interface\\AddOns\\MSBT\\Fonts\\Adventure.ttf"),
        "the addon's face, not Friz"
    );
}
