//! [`super::feed_action_state`] through a real Lua VM, and its helpers.

use super::*;
use benilla_formats::SpellDisplay;

/// A profession opener (`Effect[0]` 47) names the window `IsCurrentAction` checks it against
/// (`0x4e54fe`): a nonzero `EffectMiscValue[0]` the craft of that type (Enchanting 3), zero
/// the trade skill of its `SkillLineAbility` line (Tailoring 3908, line 197); any other spell
/// none.
#[test]
fn a_profession_opener_names_its_window() {
    use benilla_ui::script::Profession;
    let lines = crate::ui_spellbook::SkillLines {
        catalog: benilla_formats::SkillLineCatalog::from_spell_lines([(3908, 197)]),
    };
    let opener = |misc| SpellDisplay {
        effects: [benilla_formats::SPELL_EFFECT_TRADE_SKILL, 0, 0],
        effect_misc_value: [misc, 0, 0],
        ..Default::default()
    };
    assert_eq!(
        profession(7411, &opener(3), Some(&lines)),
        Some(Profession::Craft(3))
    );
    assert_eq!(
        profession(3908, &opener(0), Some(&lines)),
        Some(Profession::TradeSkill(197))
    );
    assert_eq!(profession(3908, &opener(0), None), None, "no rows loaded");
    assert_eq!(
        profession(133, &SpellDisplay::default(), Some(&lines)),
        None
    );
}

/// `[rec+0x564]`'s three values (`0x4e5a50`): a live spell, a bare macro, and grey.
#[test]
fn a_macro_slot_resolves_through_its_bound_spell() {
    use crate::ui_macro::BoundSpell;
    use benilla_protocol::messages::ACTION_KIND_MACRO;

    let mut bound = crate::ui_macro::MacroBoundSpells::default();
    bound.0.insert(3, BoundSpell::Spell(133)); // macro 3 casts Fireball
    bound.0.insert(4, BoundSpell::None); // macro 4 is `.spawn 16032`
    bound.0.insert(5, BoundSpell::Unresolved); // macro 5 is `/cast Pyroblast`, unknown

    assert_eq!(
        resolve_through_macro(ACTION_KIND_MACRO, 3, &bound),
        SlotResolve::Action(ACTION_KIND_SPELL, 133),
        "from here down the macro IS the Fireball slot"
    );
    assert_eq!(
        resolve_through_macro(ACTION_KIND_MACRO, 4, &bound),
        SlotResolve::BareMacro,
        "a macro that casts nothing is usable, with no cooldown, no range"
    );
    assert_eq!(
        resolve_through_macro(ACTION_KIND_MACRO, 5, &bound),
        SlotResolve::Dead,
        "a /cast of an unknown spell is the reference's -1: grey"
    );
    assert_eq!(
        resolve_through_macro(ACTION_KIND_MACRO, 6, &bound),
        SlotResolve::Dead,
        "a slot whose macro no longer exists: grey"
    );
    assert_eq!(
        resolve_through_macro(ACTION_KIND_SPELL, 133, &bound),
        SlotResolve::Action(ACTION_KIND_SPELL, 133)
    );
    assert_eq!(
        resolve_through_macro(ACTION_KIND_ITEM, 117, &bound),
        SlotResolve::Action(ACTION_KIND_ITEM, 117)
    );
}

#[test]
fn the_feed_pushes_a_bare_macro_as_usable() {
    use crate::ui_macro::{BoundSpell, MacroBoundSpells};
    use benilla_protocol::messages::{ActionButton, ACTION_KIND_MACRO};

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = PlayerActions::default();
    for (slot, index) in [(0u8, 1u32), (1, 2), (2, 3)] {
        actions.buttons.insert(
            slot,
            ActionButton {
                slot,
                action: index,
                kind: ACTION_KIND_MACRO,
            },
        );
    }
    // Macro 1 is `.spawn 16032`, 2 an unknown `/cast Pyroblast`, and 3 does not exist.
    let mut bound = MacroBoundSpells::default();
    bound.0.insert(1, BoundSpell::None);
    bound.0.insert(2, BoundSpell::Unresolved);
    app.insert_resource(actions)
        .insert_resource(bound)
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<Selection>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<Items>()
        .insert_resource(NetCommands(tx));
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.add_systems(Update, feed_action_state);
    app.update();

    let script = app.world().non_send_resource::<UiScript>();
    let usable = |action: u32| {
        script
            .eval::<bool>(&format!(
                "return (IsUsableAction({action})) and true or false"
            ))
            .unwrap()
    };
    assert!(usable(1), "a macro that casts nothing is a usable button");
    assert!(
        !usable(2),
        "a /cast of an unknown spell is grey (the reference's -1)"
    );
    assert!(!usable(3), "a slot whose macro no longer exists is grey");
    assert!(
        !script
            .eval::<bool>("local _, oom = IsUsableAction(2) return oom and true or false")
            .unwrap(),
        "grey, not the out-of-power blue: notEnoughMana stays 0 on the spell-less leg"
    );
}

/// `IsActionInRange` (`0x4e56f0`) hands `GetMinMaxRange` the selected unit, so a ranged row
/// pads by that unit's reach and the player's alone: a Fireball (0-35) button reads in range
/// out to 38 yards from a 1.5-reach target, whoever the player swings at, and `ActionHasRange`
/// (`0x4e5810`, a null target) says the button has a range.
#[test]
fn a_ranged_button_pads_its_range_by_the_selected_target_alone() {
    use benilla_protocol::messages::ActionButton;
    use benilla_protocol::ObjectFields;

    const FIREBALL: u32 = 133;
    const SWUNG_AT: u64 = 0xA1;
    const SELECTED: u64 = 0xB2;
    /// `UNIT_FIELD_COMBATREACH`.
    const REACH: u16 = 130;

    let verdict = |distance: f32| {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: FIREBALL,
                kind: ACTION_KIND_SPELL,
            },
        );
        let fireball = SpellDisplay {
            range_index: 4,
            ..Default::default()
        };
        let row = benilla_formats::SpellRange {
            min: 0.0,
            max: 35.0,
            flags: 0,
        };
        app.insert_resource(actions)
            .insert_resource(crate::ui_macro::MacroBoundSpells::default())
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(FIREBALL, fireball)].into_iter().collect(),
                ),
                forms: Default::default(),
                ranges: benilla_formats::SpellRangeCatalog::from_rows(
                    [(4, row)].into_iter().collect(),
                ),
                cast_times: Default::default(),
                durations: Default::default(),
                radii: Default::default(),
            })
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx));
        // The player swings at a 4.0-reach unit, close by.
        let swung_at = app
            .world_mut()
            .spawn((
                Transform::from_xyz(2.0, 0.0, 0.0),
                ObjectStore(ObjectFields::from_pairs(&[
                    (22, 100),
                    (REACH, 4.0f32.to_bits()),
                ])),
            ))
            .id();
        app.world_mut().spawn((
            SelfPlayer,
            Engaged(SWUNG_AT),
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[(22, 100), (23, 500)])),
        ));
        // The selected unit keeps the descriptor's default 1.5 reach.
        let selected = app
            .world_mut()
            .spawn((
                Transform::from_xyz(distance, 0.0, 0.0),
                ObjectStore(ObjectFields::from_pairs(&[(22, 100)])),
            ))
            .id();
        let mut index = app.world_mut().resource_mut::<crate::net::GuidIndex>();
        index.0.insert(SWUNG_AT, swung_at);
        index.0.insert(SELECTED, selected);
        app.insert_resource(Selection {
            target: Some(selected),
            guid: Some(SELECTED),
            last: None,
        });
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_action_state);
        app.update();
        let script = app.world().non_send_resource::<UiScript>();
        assert!(script
            .eval::<bool>("return ActionHasRange(1) == 1")
            .unwrap());
        script
            .eval::<i64>("return IsActionInRange(1) or -1")
            .unwrap()
    };

    // 35 + the caster's 1.5 + the selected unit's 1.5 = 38.
    assert_eq!(verdict(37.5), 1, "inside the padded max");
    assert_eq!(
        verdict(38.5),
        0,
        "past it: the swing's 4.0 reach pads nothing"
    );
}

/// `IsActionInRange` hands `GetMinMaxRange` the selected unit, so a melee button reads in
/// range out to 7.667 yards while the player and the selected mob both run, and to the 5.0
/// floor otherwise (`0x6e3648`-`0x6e36a2`).
#[test]
fn a_melee_button_reaches_the_moving_bonus_while_both_units_run() {
    use benilla_protocol::messages::ActionButton;
    use benilla_protocol::ObjectFields;

    const HAMSTRING: u32 = 1715;
    const SELECTED: u64 = 0xB2;

    let verdict = |caster_runs: bool, mob_runs: bool, distance: f32| {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: HAMSTRING,
                kind: ACTION_KIND_SPELL,
            },
        );
        let melee = benilla_formats::SpellRange {
            min: 0.0,
            max: 5.0,
            flags: 1,
        };
        app.insert_resource(actions)
            .insert_resource(crate::ui_macro::MacroBoundSpells::default())
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(
                        HAMSTRING,
                        SpellDisplay {
                            range_index: 2,
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                ),
                forms: Default::default(),
                ranges: benilla_formats::SpellRangeCatalog::from_rows(
                    [(2, melee)].into_iter().collect(),
                ),
                cast_times: Default::default(),
                durations: Default::default(),
                radii: Default::default(),
            })
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<Items>()
            .insert_resource(crate::player::Player::with_move_flags(if caster_runs {
                crate::creature_anim::move_flags::FORWARD
            } else {
                0
            }))
            .insert_resource(NetCommands(tx));
        let speeds = crate::net::UnitSpeeds(benilla_protocol::MoveSpeeds {
            walk: 2.5,
            run: 7.0,
            run_back: 4.5,
            swim: 4.7,
            swim_back: 2.5,
            turn_rate: 3.1,
        });
        app.world_mut().spawn((
            SelfPlayer,
            crate::net::Embodied,
            speeds,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[(22, 100), (23, 500)])),
        ));
        // A mob at the descriptor's default 1.5 reach; a live spline at 8 yd/s is its run.
        let mob = app
            .world_mut()
            .spawn((
                speeds,
                Transform::from_xyz(distance, 0.0, 0.0),
                ObjectStore(ObjectFields::from_pairs(&[(22, 100)])),
            ))
            .id();
        if mob_runs {
            app.world_mut().entity_mut(mob).insert(crate::net::Spline {
                points: vec![[distance, 0.0, 0.0], [distance + 16.0, 0.0, 0.0]],
                start: Instant::now(),
                duration: std::time::Duration::from_secs(2),
                id: 1,
                grounded: true,
                run_mode: true,
                deck: None,
            });
        }
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(SELECTED, mob);
        app.insert_resource(Selection {
            target: Some(mob),
            guid: Some(SELECTED),
            last: None,
        });
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_action_state);
        app.update();
        let script = app.world().non_send_resource::<UiScript>();
        script
            .eval::<i64>("return IsActionInRange(1) or -1")
            .unwrap()
    };

    // 1.5 + 1.5 + 1.3333 is under the 5.0 floor; the bonus goes on the floor: 7.667.
    assert_eq!(verdict(true, true, 7.5), 1, "both running: inside 7.667");
    assert_eq!(verdict(true, true, 8.0), 0, "past it");
    assert_eq!(verdict(false, true, 7.5), 0, "the player stands");
    assert_eq!(verdict(true, false, 7.5), 0, "the mob stands");
    assert_eq!(verdict(false, false, 4.9), 1, "the floor");
}

/// An item slot's on-use spell takes the `0x6e3d60` walk (`0x4e5050`), and every shipped
/// Food/Drink spell has `Attributes` bit 28, `ATTR_NOT_IN_COMBAT` (the walk's leg 8).
#[test]
fn food_on_the_bar_greys_while_the_player_is_in_combat() {
    use benilla_protocol::messages::{ActionButton, ItemUseSpell};
    use benilla_protocol::ObjectFields;

    // One ON_USE block casting 433 "Food", whose shipped `Attributes` are `0x18000100`.
    const FOOD_ITEM: u32 = 1487;
    const FOOD_SPELL: u32 = 433;
    // Raw indices: `ITEM_FIELD_STACK_COUNT`, and `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1.
    const STACK: u16 = 14;
    const PACK_SLOT_1: u16 = 532;

    let lit = |in_combat: bool| {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: FOOD_ITEM,
                kind: ACTION_KIND_ITEM,
            },
        );
        let mut items = Items::default();
        items.insert_template(
            FOOD_ITEM,
            Some(benilla_protocol::messages::ItemInfo {
                use_spell: Some(ItemUseSpell {
                    spell_id: FOOD_SPELL,
                    cooldown_ms: -1,
                    category: 0,
                    category_cooldown_ms: -1,
                }),
                ..crate::items::test_template("Conjured Muffin")
            }),
        );
        let food = SpellDisplay {
            attributes: 0x1800_0100,
            ..Default::default()
        };
        app.insert_resource(actions)
            .insert_resource(crate::ui_macro::MacroBoundSpells::default())
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(FOOD_SPELL, food)].into_iter().collect(),
                ),
                forms: Default::default(),
                ranges: Default::default(),
                cast_times: Default::default(),
                durations: Default::default(),
                radii: Default::default(),
            })
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Selection>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .insert_resource(items)
            .insert_resource(NetCommands(tx));
        crate::items::test_spawn_item(
            app.world_mut(),
            0xF0,
            ObjectFields::from_pairs(&[(3, FOOD_ITEM), (STACK, 10)]),
            false,
        );
        // The player: alive, with the ten muffins in backpack slot 1.
        let flags = (1u32 << 3)
            | if in_combat {
                crate::player::UNIT_FLAG_IN_COMBAT
            } else {
                0
            };
        app.world_mut().spawn((
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[
                (22, 100),
                (23, 500),
                (46, flags),
                (PACK_SLOT_1, 0xF0),
            ])),
        ));
        // The slot as the identity feed pushed it, with the ten muffins it counted.
        let mut script = UiScript::new().unwrap();
        script.set_action(
            1,
            Some(benilla_ui::script::ActionSlot {
                kind: ACTION_KIND_ITEM,
                action: FOOD_ITEM,
                count: 10,
                ..Default::default()
            }),
        );
        app.insert_non_send_resource(script);
        app.add_systems(Update, feed_action_state);
        app.update();
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<bool>("return (IsUsableAction(1)) and true or false")
            .unwrap()
    };

    assert!(lit(false), "out of combat the muffins are full-colour");
    assert!(
        !lit(true),
        "in combat the food's on-use spell fails leg 8 and the button greys"
    );
}

/// `0x4e5050` reads the slot's cached count (`[0xbc6390]`, the one `GetActionCount` answers),
/// not the bags: a zero there greys the slot while a copy sits in the pack, and a count there
/// lights it with nothing in the bags, as for a worn copy the recount stored.
#[test]
fn an_item_slot_is_held_by_its_stored_count_not_the_bags() {
    use benilla_protocol::messages::ActionButton;
    use benilla_protocol::ObjectFields;

    const ITEM: u32 = 2512;
    // `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1.
    const PACK_SLOT_1: u16 = 532;

    let lit = |stored: u32, in_pack: bool| {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: ITEM,
                kind: ACTION_KIND_ITEM,
            },
        );
        let mut items = Items::default();
        items.insert_template(ITEM, Some(crate::items::test_template("Rough Arrow")));
        app.insert_resource(actions)
            .insert_resource(crate::ui_macro::MacroBoundSpells::default())
            .insert_resource(Spells::empty_for_tests())
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Selection>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .insert_resource(items)
            .insert_resource(NetCommands(tx));
        crate::items::test_spawn_item(
            app.world_mut(),
            0xF0,
            ObjectFields::from_pairs(&[(3, ITEM), (14, 10)]),
            false,
        );
        let mut store = vec![(22, 100), (23, 500), (46, 1 << 3)];
        if in_pack {
            store.push((PACK_SLOT_1, 0xF0));
        }
        app.world_mut().spawn((
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&store)),
        ));
        let mut script = UiScript::new().unwrap();
        script.set_action(
            1,
            Some(benilla_ui::script::ActionSlot {
                kind: ACTION_KIND_ITEM,
                action: ITEM,
                count: stored,
                ..Default::default()
            }),
        );
        app.insert_non_send_resource(script);
        app.add_systems(Update, feed_action_state);
        app.update();
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<bool>("return (IsUsableAction(1)) and true or false")
            .unwrap()
    };

    assert!(lit(3, false), "a stored count lights it, bags or none");
    assert!(
        !lit(0, true),
        "a stored zero greys it with a copy in the pack"
    );
}

/// `IsEquippedAction` asks `0x622270` at mask 1 (`0x4e5380`), the walker over the worn slots,
/// which skips a broken item (`0x6224f4`-`0x62250e`): a worn weapon at durability 0 draws no
/// border, a whole one does, and a copy in the pack is not worn. The slot's count comes from
/// the recount (`0x4e6d20`, mask 0), and the usable gate reads that count (`0x4e50ab`): a worn
/// whole copy lights the slot, a broken one counts 0 and greys it.
#[test]
fn a_broken_worn_weapon_draws_no_equipped_border() {
    use benilla_protocol::messages::ActionButton;
    use benilla_protocol::ObjectFields;

    const SWORD: u32 = 2025;
    let equipped = |seat_field: u16, durability: u32| {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: SWORD,
                kind: ACTION_KIND_ITEM,
            },
        );
        let mut items = Items::default();
        items.insert_template(SWORD, Some(crate::items::test_template("Sword")));
        app.insert_resource(actions)
            .insert_resource(crate::ui_macro::MacroBoundSpells::default())
            .insert_resource(Spells::empty_for_tests())
            .add_message::<crate::item_watch::ItemNotice>()
            .add_message::<crate::net::WorldEnterCascadeMessage>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Selection>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .insert_resource(items)
            .insert_resource(NetCommands(tx));
        // The sword: flags 0, durability, maximum 40.
        crate::items::test_spawn_item(
            app.world_mut(),
            0xF0,
            ObjectFields::from_pairs(&[(3, SWORD), (14, 1), (46, durability), (47, 40)]),
            false,
        );
        app.world_mut().spawn((
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[
                (22, 100),
                (23, 500),
                (46, 1 << 3),
                (seat_field, 0xF0),
            ])),
        ));
        let mut script = UiScript::new().unwrap();
        script.set_action(
            1,
            Some(benilla_ui::script::ActionSlot {
                kind: ACTION_KIND_ITEM,
                action: SWORD,
                count: 1,
                ..Default::default()
            }),
        );
        app.insert_non_send_resource(script);
        app.add_systems(Update, feed_action_state);
        app.update();
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<bool>("return (IsEquippedAction(1)) and true or false")
            .unwrap()
    };
    // Player slot 15 is the main hand, 23 the first backpack seat.
    assert!(equipped(486 + 2 * 15, 12), "a whole worn sword");
    assert!(!equipped(486 + 2 * 15, 0), "a broken one");
    assert!(
        !equipped(486 + 2 * 23, 12),
        "a copy in the pack is not worn"
    );
}

/// `SMSG_CLIENT_CONTROL_UPDATE` clears `[0xb4b3e4]` (`0x4958e0`), and `IsUsableAction` answers
/// nil for every spell button and spell-less macro but the spell whose immunity covers the fear
/// (`0x6e3dc3`-`0x6e3e4c`, `0x4e50f4`); control back, they all light.
#[test]
fn lost_control_greys_the_bar_but_the_immunity_spell() {
    use benilla_protocol::messages::{ActionButton, ACTION_KIND_MACRO};
    use benilla_protocol::ObjectFields;

    const FIREBALL: u32 = 133;
    const FEAR_AURA: u32 = 5782;
    const WILL: u32 = 7744;
    /// `UNIT_FIELD_FLAGS` bit 23, fleeing.
    const FLEEING: u32 = 0x0080_0000;

    let mut fear = SpellDisplay {
        mechanic: 5,
        ..Default::default()
    };
    fear.effects[0] = 6;
    fear.effect_apply_aura[0] = 7;
    let mut will = SpellDisplay {
        attributes_ex: 0x8000,
        ..Default::default()
    };
    will.effects[0] = 6;
    will.effect_apply_aura[0] = 77;
    will.effect_misc_value[0] = 5;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = PlayerActions::default();
    for (slot, action, kind) in [
        (0u8, FIREBALL, ACTION_KIND_SPELL),
        (1, WILL, ACTION_KIND_SPELL),
        (2, 1, ACTION_KIND_MACRO),
    ] {
        actions
            .buttons
            .insert(slot, ActionButton { slot, action, kind });
    }
    let mut bound = crate::ui_macro::MacroBoundSpells::default();
    bound.0.insert(1, crate::ui_macro::BoundSpell::None);
    app.insert_resource(actions)
        .insert_resource(bound)
        .insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    (FIREBALL, SpellDisplay::default()),
                    (WILL, will),
                    (FEAR_AURA, fear),
                ]
                .into_iter()
                .collect(),
            ),
            ..Spells::empty_for_tests()
        })
        .init_resource::<crate::player::Player>()
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<Selection>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<Items>()
        .insert_resource(NetCommands(tx));
    app.world_mut().spawn((
        SelfPlayer,
        Transform::default(),
        ObjectStore(ObjectFields::from_pairs(&[
            (22, 100),
            (23, 500),
            (46, (1 << 3) | FLEEING),
            (47, FEAR_AURA),
        ])),
    ));
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"
            USABLE_EVENTS = 0
            local f = CreateFrame("Frame")
            f:RegisterEvent("ACTIONBAR_UPDATE_USABLE")
            f:SetScript("OnEvent", function() USABLE_EVENTS = USABLE_EVENTS + 1 end)
        "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.add_systems(Update, feed_action_state);
    let events = |app: &mut App| -> i64 {
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.resolve();
        s.eval::<i64>("local n = USABLE_EVENTS; USABLE_EVENTS = 0; return n")
            .unwrap()
    };

    let lit = |app: &App| -> Vec<bool> {
        let script = app.world().non_send_resource::<UiScript>();
        (1..=3)
            .map(|n| {
                script
                    .eval::<bool>(&format!("return (IsUsableAction({n})) and true or false"))
                    .unwrap()
            })
            .collect()
    };

    app.update();
    assert_eq!(
        lit(&app),
        [true, true, true],
        "in control, the whole bar is lit"
    );
    events(&mut app); // the first push's own signal

    app.world_mut()
        .resource_mut::<crate::player::Player>()
        .own_control_lost = true;
    app.update();
    assert_eq!(
        lit(&app),
        [false, true, false],
        "only the fear immunity stays lit"
    );
    assert_eq!(events(&mut app), 1, "the cache change signals the bar once");

    app.world_mut()
        .resource_mut::<crate::player::Player>()
        .own_control_lost = false;
    app.update();
    assert_eq!(lit(&app), [true, true, true], "control back, all lit again");
    assert_eq!(events(&mut app), 1);
}

/// A held action (Stealth) reaches the VM as the held marker, and `GetActionCooldown` answers
/// the poll's own now (`0x6e1517`) with no feed change between polls.
#[test]
fn a_held_action_polls_answer_their_own_now() {
    use benilla_protocol::messages::ActionButton;
    use benilla_protocol::ObjectFields;

    const STEALTH: u32 = 1784;
    let stealth = || SpellDisplay {
        attributes: 0x0200_0000,
        recovery_ms: 10_000,
        ..Default::default()
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = PlayerActions::default();
    actions.buttons.insert(
        0,
        ActionButton {
            slot: 0,
            action: STEALTH,
            kind: ACTION_KIND_SPELL,
        },
    );
    app.insert_resource(actions)
        .insert_resource(crate::ui_macro::MacroBoundSpells::default())
        .insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(STEALTH, stealth())].into_iter().collect(),
            ),
            ..Spells::empty_for_tests()
        })
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<Selection>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<Items>()
        .insert_resource(NetCommands(tx));
    app.world_mut().spawn((
        SelfPlayer,
        Transform::default(),
        ObjectStore(ObjectFields::from_pairs(&[(22, 100), (23, 500)])),
    ));
    app.world_mut().resource_mut::<Cooldowns>().start_spell(
        STEALTH,
        &stealth(),
        0,
        Instant::now(),
        None,
    );
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.add_systems(Update, feed_action_state);

    let read = |app: &App| {
        let script = app.world().non_send_resource::<UiScript>();
        let (start, duration, enable) = script
            .eval::<(f64, f64, i64)>("return GetActionCooldown(1)")
            .unwrap();
        (start, duration, enable, script.now())
    };
    app.update();
    let first = read(&app);
    assert_eq!((first.1, first.2), (0.001, 0), "(now, 0.001, 0)");
    assert_eq!(first.0, first.3);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .tick(0.02);
    let second = read(&app);
    assert_eq!(second.0, second.3, "the poll answers its own now");
    assert!((second.0 - first.0 - 0.02).abs() < 1e-9, "20 ms apart");
}
