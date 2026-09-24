//! The shipped pet frame, driven end to end (decision 0990) — `UnitFrames.xml`'s
//! `PetFrame` over synthetic `"pet"` snapshots and the events the app's feed fires.
//!
//! The frame's whole job is to be right about **which unit an event names**, and that is what most
//! of these test: `UNIT_PET` names the OWNER (`arg1 == "player"`, `0x4bc84f`), every other `UNIT_*`
//! names the pet itself, and a frame that mixes the two repaints off the player's health.

use benilla_ui::script::{
    AuraState, QuadContent, ScriptValue, SelectionRequest, UiScript, UnitState,
};

use super::test_ui::load_ui as load_xml;

/// The pet frame's production load prefix — it is parented to `PlayerFrame`, so the whole
/// unit-frame file comes with it, and the tooltip/dropdown kit its hover and its neighbours need.
///
/// **A player is seated, and that is load-bearing rather than tidy.** `PetFrame` really is a CHILD
/// of `PlayerFrame` — our `UnitFrames.xml:1792` says `parent="PlayerFrame"` and the reference's
/// `PetFrame.xml:4` says the same — and `UnitFrame_Update` hides a frame whose unit does not exist.
/// So a fixture with a pet and no player gives `PlayerFrame:Hide()`, and a hidden parent means an
/// invisible pet frame. That is the reference's own structure working correctly; a pet without a
/// player is a state the game cannot be in.
///
/// (It did not matter until the loader learned the `parent=` attribute, which it had been ignoring:
/// before that `PetFrame` was a top-level frame and nothing could hide it from above.)
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

/// A pet snapshot. `max_power == 0` is the powerless pet (a skeleton) — the leg that swaps the art.
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
        // Every token the app's feed pushes is connected (`ui_unit.rs`, "the stated
        // `is_connected` gap, closed"). It matters to the reference's own bar: stock
        // `UnitFrameManaBar_Update` (UnitFrame.lua:213) greys a DISCONNECTED unit's bar to
        // (0.5,0.5,0.5) and only reaches `UnitFrame_UpdateManaType`'s power colour otherwise, so a
        // fixture that leaves this false paints every bar grey and tests nothing about power type.
        is_connected: true,
        ..UnitState::default()
    }
}

/// Every texture path the UI actually **draws** this frame, and its vertex tint.
///
/// The engine ships `SetTexture`/`SetVertexColor` and no getters (the reference's `GetTexture`
/// exists but nothing in the shipped XML reads it), so these assertions go through the render path
/// the unit-frame tests already use — which is the honest question anyway: not what the region was
/// told, but what comes out the other end.
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

/// Is exactly this texture drawn? Exact, not `contains` — `UI-SmallTargetingFrame` is a prefix of
/// `UI-SmallTargetingFrame-NoMana`, and the whole point of the art-swap test is telling them apart.
fn draws(s: &mut UiScript, path: &str) -> bool {
    drawn(s).iter().any(|(p, _)| p == path)
}

/// One aura on the pet's row. **Helpful**, because the row the pet frame fills is a BUFF row —
/// see [`the_debuff_row_fills_from_the_pets_own_auras`] for the reference's own `showBuffs = 1`.
fn pet_buff(spell_id: u32, name: &str, count: u8) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        count,
        debuff_type: None,
        // No unit but yourself carries a duration on the 1.12 wire (decision 0257 B6).
        duration: 0.0,
        expiration_time: 0.0,
        helpful: true,
        cancelable: false,
        until_cancelled: false,
        channeled: false,
    }
}

/// Summon → the frame appears with the pet's name, health and power; dismiss → it hides again.
/// `UNIT_PET` is the only wire either edge has, which is why it carries them both.
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

    // The dismiss: the token clears and UNIT_PET fires, exactly as `feed_pet_unit` does it.
    s.set_unit("pet", None);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(!s.eval::<bool>("return PetFrame:IsVisible()").unwrap());
}

/// **The two argument gates.** `UNIT_PET` names the OWNER, so `arg1 == "pet"` on it is not our
/// event; every other `UNIT_*` names the pet, so `arg1 == "player"` on those is not ours either.
/// A frame that has these backwards paints the player's health onto the pet's bar — which is why
/// this drives each event with the WRONG token and asserts nothing moved.
#[test]
fn the_frame_answers_only_the_events_that_name_its_own_unit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(s.eval::<bool>("return PetFrame:IsVisible()").unwrap());

    // A health change on the PLAYER must not touch the pet's bar.
    s.set_unit("pet", Some(pet("Grimjaw", 5, 45, 80, 0)));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    assert_eq!(
        s.eval::<f64>("return PetFrameHealthBar:GetValue()")
            .unwrap(),
        72.0,
        "a UNIT_HEALTH for \"player\" is not the pet's event"
    );
    // …and the same event naming the pet does.
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("pet".into())]);
    assert_eq!(
        s.eval::<f64>("return PetFrameHealthBar:GetValue()")
            .unwrap(),
        5.0
    );

    // UNIT_PET is the mirror image: it names the owner, so a "pet" arg1 is not ours.
    s.set_unit("pet", None);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("pet".into())]);
    assert!(
        s.eval::<bool>("return PetFrame:IsVisible()").unwrap(),
        "UNIT_PET names the OWNER — a \"pet\" arg1 is somebody else's event"
    );
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(!s.eval::<bool>("return PetFrame:IsVisible()").unwrap());
}

/// The reference's art swap (`PetFrame.lua:29-33`): a pet with no power wears the `-NoMana`
/// plate, which has no mana rail to leave empty — and the bar hides with it. A hunter's FOCUS pet
/// takes the plain plate and the focus colour, which is the case the mana-coloured default would
/// get wrong.
///
/// **Two things about the reference's wiring that our transcription did not have.**
///
/// *The plate's only wire is `UNIT_PET`.* `PetFrame_Update` — the sole caller of the
/// `UnitManaMax("pet") == 0` fork — runs from `PetFrame_OnEvent`'s `UNIT_PET`/`arg1 == "player"`
/// arm and from the frame's `OnShow` (`PetFrame.lua:46-49`, `PetFrame.xml:284-287`), and from
/// nothing else. This fixture used to poke `UNIT_MAXPOWER`, which the stock frame does not
/// register at all — nor could it, since the bar's own registrations are 1.12's
/// `UNIT_MANA`/`UNIT_MAXMANA`/… (`UnitFrame.lua:190-200`) rather than our Era pair. Swapping one
/// pet for a differently-powered one is a `UNIT_PET` on the wire, which is the honest edge here.
///
/// *Nothing in `PetFrame.lua` hides the bar.* The hide comes out of `TextStatusBar`: the bar
/// inherits that template's `<OnValueChanged>` (`TextStatusBar.xml:32-34` — `PetFrame.xml:161-169`
/// overrides only OnLoad/OnEvent), so `UnitFrameManaBar_Update`'s `SetMinMaxValues(0, 0)`
/// (`UnitFrame.lua:211-212`) reaches `TextStatusBar_UpdateTextString`, whose `valueMax > 0` else
/// branch is `textStatusBar:Hide()` (`TextStatusBar.lua:34/55-57`). Same visible answer as our
/// transcription's explicit `Hide()`, by a different and much longer road — which is worth knowing,
/// because it means the bar comes back only through a value change, never through `PetFrame.lua`.
#[test]
fn a_powerless_pet_wears_the_no_mana_plate() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();

    const PLAIN: &str = "Interface\\TargetingFrame\\UI-SmallTargetingFrame";
    const NO_MANA: &str = "Interface\\TargetingFrame\\UI-SmallTargetingFrame-NoMana";

    // Focus (power type 2) — a hunter's pet: plain art, orange bar.
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

    // A skeleton: no power at all. `UNIT_PET` names the OWNER — see the doc above for why it, and
    // not a power event, is what re-cuts the plate.
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

/// `PET_ATTACK_START`/`PET_ATTACK_STOP` show and hide the flashing attack overlay, and neither
/// repaints the rest of the frame (they are the ref's own early returns).
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

    // The ref's OnUpdate ramps the overlay's tint alpha down from 1 on the opening (sign −1) leg.
    // Two ticks so the ramp has visibly moved, and the range guard catches the classic transcription
    // slip: the ref's constants are 0..255 and the API takes 0..1.
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

/// The four `$parentDebuff` buttons fill from the pet's own auras in order and the unused ones
/// hide. `UNIT_AURA("pet")` is the wire (`PetFrame.lua:54-57`).
///
/// **Two things the reference does that our deleted transcription did not**, both worth stating
/// because the frames' NAMES argue for the opposite on both counts:
///
/// 1. **The row shows BUFFS, not debuffs**, despite every frame in it being called
///    `PetFrameDebuffN`. `PetFrame.lua:37`/`:56` are the only two `RefreshBuffs` calls in the whole
///    of 1.12's FrameXML that pass `showBuffs = 1`, and that argument selects
///    `UnitBuff(unit, i, SHOW_CASTABLE_BUFFS)` at `BuffFrame.lua:277`. (Every other caller —
///    `PartyMemberFrame.lua:60`/`:176`/`:259`, `TargetFrame.lua:538` — passes `0`, which is the
///    `UnitDebuff` arm at `:280`.) Our `BenillaPetFrame_UpdateDebuffs` read `UnitDebuff("pet", i)`,
///    so a hunter whose pet was buffed saw an empty row and a poisoned pet saw icons the reference
///    would not show.
/// 2. **There is no stack count.** `RefreshBuffs` sets an icon and a border and nothing else
///    (`BuffFrame.lua:287-301`), and `PartyBuffButtonTemplate` — the template all four inherit
///    (`PetFrame.xml:173`/`:188`/`:203`/`:218`) — declares only `$parentIcon` and `$parentBorder`
///    (`PartyFrameTemplates.xml:3-36`). `PetFrameDebuff2Count` is not a frame in the reference's
///    house at all; the old assertion was reading a FontString our transcription invented.
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
    // …and the icons are the pet's own auras, in the pet's own order. `RefreshBuffs` paints them
    // with `debuffIcon:SetTexture(debuff)` where `debuff` is `UnitBuff`'s FIRST return
    // (`BuffFrame.lua:277`/`:288`) — which on the 1.12 wire is the texture path
    // (`TargetFrame.lua:269-272` uses it the same way, and `:287-303` reads the debuff triple as
    // `texture, applications, dispelType`).
    assert!(
        draws(&mut s, "Interface\\Icons\\Spell_1000"),
        "the first buff's ICON draws — if this fails with the aura's NAME on the row instead, \
         `UnitBuff`'s first return is the Era signature's `name`, not 1.12's texture"
    );
    assert!(draws(&mut s, "Interface\\Icons\\Spell_1001"));

    // They clear when the auras do.
    s.set_auras("pet", Some(vec![]));
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("pet".into())]);
    assert!(!s
        .eval::<bool>("return PetFrameDebuff1:IsVisible()")
        .unwrap());
}

/// Left-clicking the frame targets the pet — the ref's `PetFrame_OnClick` bottom leg, and the one
/// the app answers in `target::click::target_unit_requests`' `"pet"` arm.
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

    // The right button is the deferred PET menu — it must do nothing at all, not target.
    s.run("PetFrame_OnClick(\"RightButton\")").unwrap();
    assert!(s.take_selection_requests().is_empty());
}

/// **All three legs of `PetFrame_OnClick` survive the click** — B208's "dropping food from the bag
/// onto the pet doesn't feed" (decision 1055).
///
/// The handler transcribed the reference's three legs correctly from the day it shipped, but two of
/// the globals it calls — `DropItemOnUnit` and `SpellTargetUnit` — were never registered, so the
/// middle leg called a nil value and the whole handler errored out. Nothing fed, and nothing said
/// why. The bottom-leg test above could not catch it: it only ever exercises the `else`.
///
/// So this walks the legs the reference's own order picks between them (`SpellIsTargeting()` first,
/// then `CursorHasItem()`, then plain target — `PetFrame.lua:114-129`), and asserts each *runs*.
/// The feed itself is the app's half, gated and sent in `ui_action::drop_item`; what the VM owes is
/// a queued token and no error.
#[test]
fn every_leg_of_the_pet_frame_click_reaches_a_live_binding() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    // The three globals the handler reaches for must all exist. Named individually so a failure
    // says which one went missing rather than just "attempt to call a nil value".
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

    // The middle leg, for real: put food in the backpack, pick it up the way a player does, then
    // click the pet. Before the fix this click errored instead of queueing anything.
    s.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::from([(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: 2287, // Haunch of Meat — pet food
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
    // …and it is the DROP leg, not the target leg — the reference's if/elseif is exclusive.
    assert!(s.take_selection_requests().is_empty());
}

/// The GlobalStrings the happiness tooltip resolves by key. Loaded from the MPQ in production; the
/// test declares the ones it reads, the pet-bar tests' convention. (That the real keys exist in the
/// shipped file is asserted separately, by name, in `ui_pet_stats`.)
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
        // The unit frame draws neither the family word (the paper doll's level line) nor the diet
        // (its tooltip) — decision 1062. Left at their defaults on purpose: the happiness icon
        // these tests drive must not start depending on them.
        ..benilla_ui::script::PetStats::default()
    }
}

/// The happiness icon end to end through the real XML (decision 1005): shown for a hunter pet,
/// re-cut per bucket, hidden for anything that is not one.
///
/// The tooltip key is the assertion rather than the texcoords because it is what proves **which
/// branch ran** — `PET_HAPPINESS2` can only come from the `happiness == 2` arm.
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
        // The loyalty line is chosen by the SIGN of the rate — and is absent at exactly zero.
        let line = s
            .eval::<Option<String>>("return PetFrameHappiness.tooltipLoyalty")
            .unwrap();
        assert_eq!(
            line.as_deref(),
            loyalty_line,
            "bucket {bucket} loyalty line"
        );
    }

    // A warlock's imp: `HasPetUI`'s second return is nil, so the icon hides however happy the
    // first return looks.
    s.set_pet_stats(true, stats(false, Some(3), 125.0, 20.0));
    s.fire_event("UNIT_HAPPINESS", vec![]);
    assert!(!s
        .eval::<bool>("return PetFrameHappiness:IsVisible()")
        .unwrap());
}

/// **Bucket 0 keeps the icon up.** The reference hides on `not happiness`, and `0` is truthy in
/// Lua — so a client that folded bucket 0 into nil would hide a frame the reference shows. This is
/// the `GetPetHappiness 0x4be900` trap, tested where it would actually bite: in the frame.
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

    // …whereas a genuine nil — the gate failure — does hide it.
    s.set_pet_stats(true, stats(true, None, 100.0, 0.0));
    s.fire_event("UNIT_HAPPINESS", vec![]);
    assert!(!s
        .eval::<bool>("return PetFrameHappiness:IsVisible()")
        .unwrap());
}

/// `UNIT_HAPPINESS` repaints the icon **alone** — the reference's own arm returns before the
/// frame-wide update, so a happiness tick must not be a full repaint.
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

    // Feed it up a bucket and fire ONLY the happiness event.
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

/// The layering law (decision 0884), the pet frame's copy of the player/target/party tests: the
/// frame art must draw OVER the bar fills, which only the frame LEVEL can hold — the draw layer is
/// bucket-wide. A "simplification" back to declaration order puts the bars on top as pasted slabs.
///
/// **The art frame has no name in the reference, so it is reached through its texture.** Our
/// deleted transcription gave it one (`PetFrameTextureFrame`); stock `PetFrame.xml:52-124` builds
/// it as two ANONYMOUS `<Frame setAllPoints="true">` wrappers, one inside the other, with
/// `PetFrameTexture` in the inner one's BACKGROUND layer. That nesting is how the reference buys
/// the level: `PetFrame` is 2, its direct children (both bars, `PetFrame.xml:125`/`:150`) are 3,
/// and the doubly-nested art frame is 4. The law is unchanged — the art is one level above the
/// bars — only the way to name the frame is, so this asks the texture for its parent.
///
/// It is also *why* the reference needs no `PetFrameManaBar:Hide()` of its own for the
/// [`a_powerless_pet_wears_the_no_mana_plate`] case: the `-NoMana` plate is painted a level ABOVE
/// the mana bar and simply covers the rail.
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
