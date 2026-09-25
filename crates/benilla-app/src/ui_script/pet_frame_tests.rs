//! The stock pet frame, `PetFrame.xml`, over synthetic `"pet"` snapshots and the feed's events.
//! `UNIT_PET` names the owner (`arg1 == "player"`, `0x4bc84f`) and every other `UNIT_*` names the
//! pet: a frame that mixes the two repaints off the player's health.

use benilla_ui::script::{
    AuraState, QuadContent, ScriptValue, SelectionRequest, UiScript, UnitState,
};

use super::test_ui::load_ui as load_xml;

/// The pet frame's load chain, with a player seated: `PetFrame` is a child of `PlayerFrame`
/// (PetFrame.xml:4), and `UnitFrame_Update` hides a frame whose unit does not exist.
fn load_pet_frame() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
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
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Tri".into()),
            health: 100,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 100,
            max_power: 100,
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    s
}

/// A pet snapshot; `max_power == 0` is a powerless pet, which swaps the art.
fn pet(name: &str, health: u32, power: u32, max_power: u32, power_type: u8) -> UnitState {
    UnitState {
        exists: true,
        name: Some(name.into()),
        health,
        max_health: 100,
        level: 60,
        power_type,
        power,
        max_power,
        // The feed marks every unit it pushes connected; stock `UnitFrameManaBar_Update` greys a
        // disconnected unit's bar (UnitFrame.lua:214-216).
        is_connected: true,
        ..UnitState::default()
    }
}

/// Every texture path drawn this frame, with its vertex tint.
fn drawn(s: &mut UiScript) -> Vec<(String, Option<[f32; 4]>)> {
    s.resolve();
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } => Some((p, color)),
            _ => None,
        })
        .collect()
}

/// Exact, not `contains`: `UI-SmallTargetingFrame` prefixes the `-NoMana` plate's path.
fn draws(s: &mut UiScript, path: &str) -> bool {
    drawn(s).iter().any(|(p, _)| p == path)
}

/// One helpful aura: the pet frame's row shows buffs (PetFrame.lua:37,56).
fn pet_buff(spell_id: u32, name: &str, count: u8) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        count,
        debuff_type: None,
        // Only your own auras carry a duration on the 1.12 wire.
        duration: 0.0,
        expiration_time: 0.0,
        helpful: true,
        cancelable: false,
        until_cancelled: false,
        channeled: false,
    }
}

/// `UNIT_PET` is the one event for both the summon and the dismiss.
#[test]
fn the_pet_frame_appears_on_a_summon_and_leaves_on_a_dismiss() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return PetFrame:IsVisible()").unwrap(),
        "no pet, no frame"
    );

    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    let ok: bool = s
        .eval(
            r#"
            local hb, mb = PetFrameHealthBar, PetFrameManaBar
            local _, hmax = hb:GetMinMaxValues()
            local _, mmax = mb:GetMinMaxValues()
            return PetFrame:IsVisible()
               and PetName:GetText() == "Grimjaw"
               and hb:GetValue() == 72 and hmax == 100
               and mb:GetValue() == 45 and mmax == 80 and mb:IsVisible()
            "#,
        )
        .unwrap();
    assert!(ok, "the summoned pet's name, health and power all draw");

    // The dismiss, as `feed_pet_unit` sends it: the token clears, then `UNIT_PET`.
    s.set_unit("pet", None);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(!s.eval::<bool>("return PetFrame:IsVisible()").unwrap());
}

#[test]
fn the_frame_answers_only_the_events_that_name_its_own_unit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(s.eval::<bool>("return PetFrame:IsVisible()").unwrap());

    s.set_unit("pet", Some(pet("Grimjaw", 5, 45, 80, 0)));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    assert_eq!(
        s.eval::<f64>("return PetFrameHealthBar:GetValue()")
            .unwrap(),
        72.0,
        "a UNIT_HEALTH for \"player\" is not the pet's event"
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("pet".into())]);
    assert_eq!(
        s.eval::<f64>("return PetFrameHealthBar:GetValue()")
            .unwrap(),
        5.0
    );

    s.set_unit("pet", None);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("pet".into())]);
    assert!(
        s.eval::<bool>("return PetFrame:IsVisible()").unwrap(),
        "UNIT_PET names the OWNER — a \"pet\" arg1 is somebody else's event"
    );
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(!s.eval::<bool>("return PetFrame:IsVisible()").unwrap());
}

/// A powerless pet wears the `-NoMana` plate (PetFrame.lua:29-33), recut only on `UNIT_PET` or
/// OnShow (PetFrame.lua:46-49, PetFrame.xml:284-287). Its bar hides through `TextStatusBar`:
/// `SetMinMaxValues(0, 0)` (UnitFrame.lua:211-212) reaches the inherited OnValueChanged, which
/// hides a bar whose max is 0 (TextStatusBar.lua:34,55-57), so only a value change brings it back.
#[test]
fn a_powerless_pet_wears_the_no_mana_plate() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();

    const PLAIN: &str = "Interface\\TargetingFrame\\UI-SmallTargetingFrame";
    const NO_MANA: &str = "Interface\\TargetingFrame\\UI-SmallTargetingFrame-NoMana";

    // Focus (power type 2), a hunter's pet: the plain plate and the focus colour.
    s.set_unit("pet", Some(pet("Boar", 100, 60, 100, 2)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    let (visible, r, g, b) = s
        .eval::<(bool, f64, f64, f64)>(
            "local r, g, b = PetFrameManaBar:GetStatusBarColor() \
             return PetFrameManaBar:IsVisible(), r, g, b",
        )
        .unwrap();
    assert!(visible);
    assert_eq!((r, g, b), (1.0, 0.5, 0.25), "FOCUS, not the mana default");
    assert!(draws(&mut s, PLAIN), "the plate with a mana rail");
    assert!(!draws(&mut s, NO_MANA));

    // A skeleton, with no power: `UNIT_PET` recuts the plate.
    s.set_unit("pet", Some(pet("Skeleton", 100, 0, 0, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(
        !s.eval::<bool>("return PetFrameManaBar:IsVisible()")
            .unwrap(),
        "no power bar on the plate that has no rail for it"
    );
    assert!(draws(&mut s, NO_MANA), "…and the plate swaps with it");
    assert!(!draws(&mut s, PLAIN));
}

/// `PET_ATTACK_START`/`PET_ATTACK_STOP` show and hide the attack overlay and repaint nothing else
/// (PetFrame.lua:58-62).
#[test]
fn the_attack_overlay_follows_its_own_two_events() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(!s
        .eval::<bool>("return PetAttackModeTexture:IsVisible()")
        .unwrap());

    s.fire_event("PET_ATTACK_START", vec![]);
    assert!(s
        .eval::<bool>("return PetAttackModeTexture:IsVisible()")
        .unwrap());

    // `PetFrame_OnUpdate` ramps the alpha down from 1 on the opening leg; its constants are 0-255,
    // divided into the API's 0-1 (PetFrame.lua:68-87).
    s.tick(0.1);
    s.tick(0.1);
    let tint = drawn(&mut s)
        .into_iter()
        .find(|(p, _)| p.contains("UI-Player-AttackStatus"))
        .and_then(|(_, c)| c)
        .expect("the attack overlay draws while shown");
    assert!(
        (0.0..=1.0).contains(&tint[3]),
        "the pulse stays inside the alpha range (got {})",
        tint[3]
    );
    assert!(tint[3] < 1.0, "…and it has actually ramped off full");

    s.fire_event("PET_ATTACK_STOP", vec![]);
    assert!(!s
        .eval::<bool>("return PetAttackModeTexture:IsVisible()")
        .unwrap());
}

/// The row shows buffs despite its `PetFrameDebuffN` names: `RefreshBuffs(this, 1, "pet")` selects
/// `UnitBuff` (PetFrame.lua:54-57, BuffFrame.lua:277). Its `PartyBuffButtonTemplate` has an icon
/// and a border and no count (PartyFrameTemplates.xml:3-36).
#[test]
fn the_debuff_row_fills_from_the_pets_own_auras() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    s.set_auras(
        "pet",
        Some(vec![pet_buff(1000, "Rend", 1), pet_buff(1001, "Sunder", 3)]),
    );
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("pet".into())]);

    let ok: bool = s
        .eval(
            r#"
            return PetFrameDebuff1:IsVisible()
               and PetFrameDebuff2:IsVisible()
               and not PetFrameDebuff3:IsVisible()
               and not PetFrameDebuff4:IsVisible()
               -- The reference has no count region on this template at all — not a hidden one.
               and getglobal("PetFrameDebuff1Count") == nil
               and getglobal("PetFrameDebuff2Count") == nil
            "#,
        )
        .unwrap();
    assert!(ok, "two auras draw, the other two rows stay down");
    // The icons, in order: `RefreshBuffs` sets `UnitBuff`'s first return, in 1.12 the texture
    // path (BuffFrame.lua:277,288).
    assert!(
        draws(&mut s, "Interface\\Icons\\Spell_1000"),
        "the first buff's ICON draws — if this fails with the aura's NAME on the row instead, \
         `UnitBuff`'s first return is the Era signature's `name`, not 1.12's texture"
    );
    assert!(draws(&mut s, "Interface\\Icons\\Spell_1001"));

    s.set_auras("pet", Some(vec![]));
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("pet".into())]);
    assert!(!s
        .eval::<bool>("return PetFrameDebuff1:IsVisible()")
        .unwrap());
}

/// A plain left-click targets the pet (PetFrame.lua:119-126).
#[test]
fn left_clicking_the_pet_frame_targets_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    s.run("PetFrame_OnClick(\"LeftButton\")").unwrap();
    assert_eq!(
        s.take_selection_requests(),
        vec![SelectionRequest::Unit("pet".into())]
    );

    // The right button toggles `PetFrameDropDown` (PetFrame.lua:127-128), never a target.
    s.run("PetFrame_OnClick(\"RightButton\")").unwrap();
    assert!(s.take_selection_requests().is_empty());
}

/// `PetFrame_OnClick` tries a spell target, then an item drop, then a target
/// (PetFrame.lua:114-129). Every global it calls must exist, so food dropped on the pet queues a
/// drop instead of raising.
#[test]
fn every_leg_of_the_pet_frame_click_reaches_a_live_binding() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    for global in [
        "SpellIsTargeting",
        "CursorHasItem",
        "DropItemOnUnit",
        "SpellTargetUnit",
    ] {
        assert!(
            s.eval::<bool>(&format!("return _G[\"{global}\"] ~= nil"))
                .unwrap_or(false),
            "{global} is not registered — the pet frame's click calls it"
        );
    }

    // The middle leg: food picked up from the backpack, then a click on the pet.
    s.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::from([(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: 2287, // Haunch of Meat, pet food
                    count: 1,
                    ..Default::default()
                },
            )]),
        }),
    );
    s.fire_event("BAG_UPDATE", vec![ScriptValue::Int(0)]);
    s.run("PickupContainerItem(0, 1)").unwrap();
    assert!(
        s.eval::<bool>("return CursorHasItem()").unwrap(),
        "the food is on the cursor"
    );

    s.run("PetFrame_OnClick(\"LeftButton\")")
        .expect("the cursor-holds-an-item leg must not error");
    assert_eq!(
        s.take_drop_item_on_unit(),
        vec!["pet".to_string()],
        "a held item + a pet click queues the drop"
    );
    // The drop leg alone: the reference's if/elseif is exclusive.
    assert!(s.take_selection_requests().is_empty());
}

/// Declares the `GlobalStrings` keys the happiness tooltip reads.
fn declare_happiness_strings(s: &UiScript) {
    s.run(
        "PET_HAPPINESS1 = 'Unhappy' PET_HAPPINESS2 = 'Content' PET_HAPPINESS3 = 'Happy' \
         PET_DAMAGE_PERCENTAGE = 'Damage: %d%%' \
         LOSING_LOYALTY = 'Losing Loyalty' GAINING_LOYALTY = 'Gaining Loyalty'",
    )
    .unwrap();
}

fn stats(
    hunter: bool,
    happiness: Option<u32>,
    damage: f32,
    rate: f32,
) -> benilla_ui::script::PetStats {
    benilla_ui::script::PetStats {
        hunter_pet: hunter,
        happiness,
        damage_percentage: damage,
        loyalty_rate: rate,
        loyalty: Some("(Loyalty Level 6) Best Friend".into()),
        training_points: (170, 130),
        experience: (4200, 8000),
        // Family and diet stay default: the unit frame draws neither.
        ..benilla_ui::script::PetStats::default()
    }
}

/// The tooltip is `PET_HAPPINESS<n>` (PetFrame.lua:147), so it names the bucket that painted.
#[test]
fn the_happiness_icon_shows_per_bucket_and_hides_for_a_non_hunter_pet() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    declare_happiness_strings(&s);
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 2)));

    for (bucket, tip, rate, loyalty_line) in [
        (3u32, "Happy", 20.0f32, Some("Gaining Loyalty")),
        (2, "Content", 0.0, None),
        (1, "Unhappy", -10.0, Some("Losing Loyalty")),
    ] {
        s.set_pet_stats(true, stats(true, Some(bucket), 125.0, rate));
        s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
        assert!(
            s.eval::<bool>("return PetFrameHappiness:IsVisible()")
                .unwrap(),
            "bucket {bucket} must show the icon"
        );
        assert_eq!(
            s.eval::<String>("return PetFrameHappiness.tooltip")
                .unwrap(),
            tip,
            "bucket {bucket} took the wrong texcoord branch"
        );
        // The loyalty line follows the rate's sign and is nil at zero (PetFrame.lua:149-155).
        let line = s
            .eval::<Option<String>>("return PetFrameHappiness.tooltipLoyalty")
            .unwrap();
        assert_eq!(
            line.as_deref(),
            loyalty_line,
            "bucket {bucket} loyalty line"
        );
    }

    // A warlock's imp: `HasPetUI`'s second return is nil, so the icon hides (PetFrame.lua:134-137).
    s.set_pet_stats(true, stats(false, Some(3), 125.0, 20.0));
    s.fire_event("UNIT_HAPPINESS", vec![]);
    assert!(!s
        .eval::<bool>("return PetFrameHappiness:IsVisible()")
        .unwrap());
}

/// The frame hides on `not happiness`, and 0 is truthy in Lua: `GetPetHappiness` (`0x4be900`)
/// answers bucket 0 as a number, not a nil.
#[test]
fn happiness_bucket_zero_keeps_the_icon_showing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    declare_happiness_strings(&s);
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 2)));

    s.set_pet_stats(true, stats(true, Some(0), 100.0, 0.0));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(
        s.eval::<bool>("return PetFrameHappiness:IsVisible()")
            .unwrap(),
        "bucket 0 is a number, not the hide case"
    );

    // A nil does hide it.
    s.set_pet_stats(true, stats(true, None, 100.0, 0.0));
    s.fire_event("UNIT_HAPPINESS", vec![]);
    assert!(!s
        .eval::<bool>("return PetFrameHappiness:IsVisible()")
        .unwrap());
}

/// `UNIT_HAPPINESS` repaints the icon alone (PetFrame.lua:63-64).
#[test]
fn unit_happiness_repaints_only_the_icon() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    declare_happiness_strings(&s);
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 2)));
    s.set_pet_stats(true, stats(true, Some(1), 75.0, -10.0));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert_eq!(
        s.eval::<String>("return PetFrameHappiness.tooltip")
            .unwrap(),
        "Unhappy"
    );

    s.set_pet_stats(true, stats(true, Some(3), 125.0, 20.0));
    s.fire_event("UNIT_HAPPINESS", vec![]);
    assert_eq!(
        s.eval::<String>("return PetFrameHappiness.tooltip")
            .unwrap(),
        "Happy",
        "UNIT_HAPPINESS must re-cut the icon on its own"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The art draws over the bar fills, which only frame level can hold. The art frame is anonymous,
/// nested two deep (PetFrame.xml:52-124) where the bars are direct children (PetFrame.xml:125,150),
/// so it is reached as `PetFrameTexture:GetParent()`.
#[test]
fn the_pet_art_paints_over_the_bars() {
    benilla_formats::wow_data_or_skip!();
    let s = load_pet_frame();
    let level: (i64, i64, i64) = s
        .eval(
            "return PetFrameTexture:GetParent():GetFrameLevel(), \
                    PetFrameHealthBar:GetFrameLevel(), \
                    PetFrameManaBar:GetFrameLevel()",
        )
        .unwrap();
    let (art, health, mana) = level;
    assert_eq!(health, art - 1, "the health bar sits under the art");
    assert_eq!(mana, art - 1, "and so does the mana bar");
}
