//! The stock target frame's aura rows (`TargetFrame.lua:263`) under a stubbed feed.
//! BuffFrame.xml loads first for the `DebuffTypeColor` and `RefreshBuffs` the frames use.

use benilla_ui::script::{AuraState, QuadContent, ScriptValue, UiScript, UnitState};

use super::test_ui::load_ui as load_xml;

fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // TOOLTIP_DEFAULT_* for the dropdowns
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml"); // TargetFrameDropDown's template
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");
    // UIParent.xml comes first: `UnitPopup.lua:47` reads its `ITEM_QUALITY_COLORS` at file scope.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\BuffFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\CombatFeedback.xml");
    load_xml(&s, "Interface\\FrameXML\\PlayerFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PartyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\TargetFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PetFrame.xml");

    // `TargetofTargetFrame` loads shown (`TargetFrame.xml:515`) and hides only after the rows
    // lay out (`TargetFrame.lua:66`), so a first target lays its rows out for the 5-wide wrap;
    // one no-target change leaves it in the state every later target sees.
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s
}

/// Target a unit of the given reaction (2 = hostile, 5 = friendly) carrying `auras`.
fn target(s: &mut UiScript, reaction: u8, auras: Vec<AuraState>) {
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Subject".into()),
            health: 40,
            max_health: 40,
            level: 5,
            reaction,
            ..UnitState::default()
        }),
    );
    s.set_auras("target", Some(auras));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
}

fn debuff(spell_id: u32, name: &str, count: u8, debuff_type: Option<&str>) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        count,
        debuff_type: debuff_type.map(Into::into),
        // The 1.12 wire carries no duration for another unit's auras.
        duration: 0.0,
        expiration_time: 0.0,
        helpful: false,
        cancelable: false,
        // Only the player's auras carry `untilCancelled`, read by `GetPlayerBuff` alone.
        until_cancelled: false,
        channeled: false,
    }
}

fn buff(spell_id: u32, name: &str) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        count: 1,
        debuff_type: None,
        duration: 0.0,
        expiration_time: 0.0,
        helpful: true,
        cancelable: false,
        until_cancelled: false,
        channeled: false,
    }
}

fn shown(s: &UiScript, name: &str) -> bool {
    s.eval::<bool>(&format!("return {name}:IsVisible()"))
        .unwrap()
}

/// The first anchor of `name`: (point, relative frame's name, relativePoint, x, y).
fn anchor(s: &UiScript, name: &str) -> (String, String, String, f64, f64) {
    s.eval(&format!(
        r#"local p, rel, rp, x, y = {name}:GetPoint()
           return p, (rel and rel:GetName()) or "<screen>", rp, x, y"#
    ))
    .unwrap()
}

fn size(s: &UiScript, name: &str) -> (f64, f64) {
    s.eval(&format!("return {name}:GetWidth(), {name}:GetHeight()"))
        .unwrap()
}

#[test]
fn a_hostile_target_draws_debuffs_first_with_tint_and_count() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    target(
        &mut s,
        2,
        vec![
            debuff(589, "Shadow Word: Pain", 1, Some("Magic")),
            debuff(772, "Rend", 3, None),
            buff(1126, "Mark of the Wild"),
        ],
    );

    assert!(shown(&s, "TargetFrameDebuff1"), "first debuff shows");
    assert!(shown(&s, "TargetFrameDebuff2"), "second debuff shows");
    assert!(
        !shown(&s, "TargetFrameDebuff3"),
        "no third debuff — button hides"
    );
    assert!(shown(&s, "TargetFrameBuff1"), "the buff shows");
    assert!(!shown(&s, "TargetFrameBuff2"), "no second buff");

    // Hostile: debuffs open at BOTTOMLEFT (5, 32), buffs under Debuff7 (`TargetFrame.lua:329`).
    let (p, rel, rp, x, y) = anchor(&s, "TargetFrameDebuff1");
    assert_eq!(
        (p.as_str(), rel.as_str(), rp.as_str(), x, y),
        ("TOPLEFT", "TargetFrame", "BOTTOMLEFT", 5.0, 32.0),
        "hostile: debuffs first"
    );
    let (_, rel, _, _, _) = anchor(&s, "TargetFrameBuff1");
    assert_eq!(rel, "TargetFrameDebuff7", "hostile: buffs below row 2");

    // Under the wrap (2 < 6): full 21px icons, 23px borders.
    assert_eq!(size(&s, "TargetFrameDebuff1"), (21.0, 21.0));
    assert_eq!(size(&s, "TargetFrameDebuff1Border"), (23.0, 23.0));
    assert_eq!(size(&s, "TargetFrameBuff1"), (21.0, 21.0));

    // Stack count shows only above 1.
    assert_eq!(
        s.eval::<String>(r#"return tostring((TargetFrameDebuff2Count:GetText()) or "")"#)
            .unwrap(),
        "3"
    );
    assert_eq!(
        s.eval::<String>(r#"return tostring((TargetFrameDebuff1Count:GetText()) or "")"#)
            .unwrap(),
        ""
    );

    // `DebuffTypeColor` Magic is 0.20, 0.60, 1.00 and "none" 0.80, 0, 0 (`BuffFrame.lua:10`).
    s.resolve();
    let tints: Vec<[f32; 4]> = s
        .extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Texture {
                path: Some(p),
                color: Some(c),
                ..
            } if p.contains("UI-Debuff-Overlays") => Some(c),
            _ => None,
        })
        .collect();
    assert!(
        tints
            .iter()
            .any(|c| (c[0] - 0.20).abs() < 1e-3 && (c[1] - 0.60).abs() < 1e-3),
        "a Magic-tinted border drew, got {tints:?}"
    );
    assert!(
        tints
            .iter()
            .any(|c| (c[0] - 0.80).abs() < 1e-3 && c[1].abs() < 1e-3),
        "an untyped border wears the none-red, got {tints:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn a_friendly_target_puts_the_buff_row_first() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    target(
        &mut s,
        5,
        vec![
            buff(1126, "Mark of the Wild"),
            debuff(589, "Shadow Word: Pain", 1, Some("Magic")),
        ],
    );

    let (p, rel, rp, x, y) = anchor(&s, "TargetFrameBuff1");
    assert_eq!(
        (p.as_str(), rel.as_str(), rp.as_str(), x, y),
        ("TOPLEFT", "TargetFrame", "BOTTOMLEFT", 5.0, 32.0),
        "friendly: buffs first"
    );
    let (p, rel, rp, x, y) = anchor(&s, "TargetFrameDebuff1");
    assert_eq!(
        (p.as_str(), rel.as_str(), rp.as_str(), x, y),
        ("TOPLEFT", "TargetFrameBuff1", "BOTTOMLEFT", 0.0, -2.0),
        "friendly: debuffs under the buff row"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn reaching_the_wrap_shrinks_the_first_row_to_17px() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    let debuffs: Vec<AuraState> = (0..6)
        .map(|i| debuff(1000 + i, &format!("D{i}"), 1, None))
        .collect();
    target(&mut s, 2, debuffs);

    // 6 debuffs reach the wrap: 17px icons, 19px borders, first row only (`TargetFrame.lua:349`).
    assert_eq!(size(&s, "TargetFrameDebuff1"), (17.0, 17.0));
    assert_eq!(size(&s, "TargetFrameDebuff1Border"), (19.0, 19.0));
    assert_eq!(size(&s, "TargetFrameBuff1"), (17.0, 17.0));

    // Below the wrap they grow back; the feed re-fires UNIT_AURA on the change.
    s.set_auras("target", Some(vec![debuff(1000, "D0", 1, None)]));
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("target".into())]);
    assert_eq!(size(&s, "TargetFrameDebuff1"), (21.0, 21.0));
    assert_eq!(size(&s, "TargetFrameDebuff1Border"), (23.0, 23.0));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn clearing_the_list_or_the_target_hides_the_buttons() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    target(&mut s, 2, vec![debuff(589, "Pain", 1, Some("Magic"))]);
    assert!(shown(&s, "TargetFrameDebuff1"));

    s.set_auras("target", Some(vec![]));
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("target".into())]);
    assert!(
        !shown(&s, "TargetFrameDebuff1"),
        "an emptied list hides the button"
    );

    // Deselect: the frame and its buttons hide; the token clears without a UNIT_AURA.
    target(&mut s, 2, vec![debuff(589, "Pain", 1, Some("Magic"))]);
    assert!(shown(&s, "TargetFrameDebuff1"));
    s.set_unit("target", None);
    s.set_auras("target", None);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!shown(&s, "TargetFrameDebuff1"), "no target, no buttons");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
