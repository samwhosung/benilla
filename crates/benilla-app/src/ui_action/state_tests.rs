//! [`super::feed_action_state`] through a real Lua VM, and its helpers.

use super::*;
use crate::creature_anim::Engaged;
use crate::target::Selection;
use benilla_formats::SpellDisplay;
use benilla_ui::script::UiScript;

/// A player (`OBJECT_FIELD_TYPE` 0x19) under its own control (`UNIT_FIELD_FLAGS` 0x8), alive, with
/// mana: the relation functions' attacker.
const PLAYER: &[(u16, u32)] = &[(2, 0x19), (22, 100), (23, 500), (46, 0x8)];
/// A creature (`OBJECT_FIELD_TYPE` 0x9), alive: with no faction tables the player can attack it
/// and cannot assist it.
const MOB: &[(u16, u32)] = &[(2, 0x9), (22, 100)];

/// The player's guid.
const ME: u64 = 0x10;

/// What the feed reads beside each test's own inputs: the field and item notices, the player's
/// guid and the self-cast flag the cast targeting reads.
fn feed_inputs(app: &mut App) {
    app.add_message::<crate::net::FieldChanged>()
        .add_message::<crate::item_watch::ItemNotice>()
        .add_message::<crate::net::WorldEnterCascadeMessage>()
        .init_resource::<crate::ui_items::feed::BagUpdates>()
        .insert_resource(crate::net::SelfGuid(Some(ME)))
        .init_resource::<crate::spell::AutoSelfCast>()
        .init_resource::<Selection>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>();
}

/// The feed's two halves, around the unit snapshots as the plugin orders them.
fn add_action_feed(app: &mut App) {
    app.init_resource::<ActionEvents>().add_systems(
        Update,
        (
            feed_action_state.before(crate::ui_unit::UnitSnapshots),
            fire_action_events
                .in_set(crate::ui_action::CooldownEvents)
                .after(crate::ui_unit::UnitSnapshots),
        )
            .chain(),
    );
}

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
    use crate::ui_macro::{Bound, BoundSpell};
    use benilla_protocol::messages::ACTION_KIND_MACRO;

    let mut bound = crate::ui_macro::MacroBoundSpells::default();
    // Macro 3 casts Fireball, 4 is `.spawn 16032`, 5 is `/cast Pyroblast`, unknown, and 7 is
    // `/cast Seduction`, out of the pet's book; 6 does not exist. The first character macro casts
    // Fireball too.
    for (id, spell) in [
        (3, BoundSpell::Spell(133)),
        (4, BoundSpell::None),
        (5, BoundSpell::Unresolved),
        (0x100_0001, BoundSpell::Spell(133)),
    ] {
        bound.0.insert(id, Bound::from(spell));
    }
    bound.0.insert(
        7,
        Bound {
            spell: BoundSpell::Spell(6358),
            pet_book: true,
        },
    );
    // A record that lost a pet spell keeps the pet's book (`0x4b3a10` writes it on a match alone),
    // and the selector forks only a live spell: it is a bare macro still (`0x4e50ee`).
    bound.0.insert(
        8,
        Bound {
            spell: BoundSpell::None,
            pet_book: true,
        },
    );

    assert_eq!(
        resolve_through_macro(ACTION_KIND_MACRO, 7, &bound),
        SlotResolve::PetSpell(6358),
        "the pet's book routes the slot to the pet's legs"
    );
    assert_eq!(
        resolve_through_macro(ACTION_KIND_MACRO, 8, &bound),
        SlotResolve::BareMacro
    );
    assert_eq!(
        resolve_through_macro(0x41, 1, &bound),
        SlotResolve::Action(ACTION_KIND_SPELL, 133),
        "a character macro's id carries bit 24, so its slot's kind byte is 0x41"
    );
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
        SlotResolve::Unresolved,
        "a /cast of an unknown spell is the reference's -1: grey"
    );
    assert_eq!(
        resolve_through_macro(ACTION_KIND_MACRO, 6, &bound),
        SlotResolve::Missing,
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
    bound.0.insert(1, BoundSpell::None.into());
    bound.0.insert(2, BoundSpell::Unresolved.into());
    app.insert_resource(actions)
        .insert_resource(bound)
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::creature_anim::UnitEdits>()
        .add_message::<crate::player::StandStateRequest>()
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
    feed_inputs(&mut app);
    app.insert_non_send_resource(UiScript::new().unwrap());
    add_action_feed(&mut app);
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
        // Its effect aims at an enemy (6), a relation (`CanAttack`) that admits the selected
        // unit, so the pre-check compares (`0x6e45f1`).
        let fireball = SpellDisplay {
            range_index: 4,
            effect_implicit_target_a: [6, 0, 0],
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
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx));
        feed_inputs(&mut app);
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
            ObjectStore(ObjectFields::from_pairs(PLAYER)),
        ));
        // The selected unit keeps the descriptor's default 1.5 reach.
        let selected = app
            .world_mut()
            .spawn((
                Transform::from_xyz(distance, 0.0, 0.0),
                ObjectStore(ObjectFields::from_pairs(MOB)),
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
        add_action_feed(&mut app);
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
                            effect_implicit_target_a: [6, 0, 0],
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
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
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
        feed_inputs(&mut app);
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
            ObjectStore(ObjectFields::from_pairs(PLAYER)),
        ));
        // A mob at the descriptor's default 1.5 reach; a live spline at 8 yd/s is its run.
        let mob = app
            .world_mut()
            .spawn((
                speeds,
                Transform::from_xyz(distance, 0.0, 0.0),
                ObjectStore(ObjectFields::from_pairs(MOB)),
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
        add_action_feed(&mut app);
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
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
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
        feed_inputs(&mut app);
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
        add_action_feed(&mut app);
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
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
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
        feed_inputs(&mut app);
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
        add_action_feed(&mut app);
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
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
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
        feed_inputs(&mut app);
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
        add_action_feed(&mut app);
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

/// The bar's usable walk hands the ammo leg of `0x6e40e0` the `ItemSubClass.dbc` flags
/// (`0x6e4264`): a ranged weapon whose subclass row carries `0x10` (Thrown) fires without ammo,
/// so its shot lights with nothing loaded, and the same weapon under a row without the flag greys.
#[test]
fn the_bars_ammo_leg_reads_the_subclass_flags() {
    use benilla_protocol::messages::ItemInfo;
    use benilla_protocol::ObjectFields;
    // Player slot 17, the ranged seat.
    const INV_RANGED: u16 = 486 + 2 * 17;
    const KNIFE: u64 = 0xF0;
    const KNIFE_ENTRY: u32 = 2946;
    const THROW: u32 = 2764;

    let lit = |flags: u32| {
        let throw = SpellDisplay {
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << 16,
            ..Default::default()
        };
        let mut app = feed_app(
            &[(0, THROW, ACTION_KIND_SPELL)],
            catalog(vec![(THROW, throw)], &[]),
            5.0,
        );
        app.insert_resource(crate::ui_items::ItemSubClasses(
            benilla_formats::ItemSubClassCatalog::from_rows([(
                (2, 16),
                benilla_formats::ItemSubClassInfo {
                    prerequisite_proficiency: -1,
                    postrequisite_proficiency: -1,
                    flags,
                    display_flags: 0,
                    weapon_swing_size: 0,
                },
            )]),
        ));
        // A Thrown weapon whose record names an ammo type, and no ammo loaded or carried.
        app.world_mut().resource_mut::<Items>().insert_template(
            KNIFE_ENTRY,
            Some(ItemInfo {
                class: 2,
                subclass: 16,
                ammo_type: 2,
                ..crate::items::test_template("Throwing Knife")
            }),
        );
        crate::items::test_spawn_item(
            app.world_mut(),
            KNIFE,
            ObjectFields::from_pairs(&[(3, KNIFE_ENTRY)]),
            false,
        );
        let me = app.world().resource::<crate::net::GuidIndex>().0[&ME];
        app.world_mut()
            .get_mut::<ObjectStore>(me)
            .unwrap()
            .0
            .merge_diff(
                ObjectFields::from_pairs(&[(INV_RANGED, KNIFE as u32)]),
                |_, _, _| {},
            );
        app.update();
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<bool>("return (IsUsableAction(1)) and true or false")
            .unwrap()
    };
    assert!(lit(0x10), "a subclass that fires no ammo");
    assert!(!lit(0), "the same weapon, needing its ammo");
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
    bound.0.insert(1, crate::ui_macro::BoundSpell::None.into());
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
        .init_resource::<crate::player::OwnControlLost>()
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::creature_anim::UnitEdits>()
        .add_message::<crate::player::StandStateRequest>()
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
    feed_inputs(&mut app);
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
    add_action_feed(&mut app);
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
        .resource_mut::<crate::player::OwnControlLost>()
        .0 = true;
    app.update();
    assert_eq!(
        lit(&app),
        [false, true, false],
        "only the fear immunity stays lit"
    );
    assert_eq!(events(&mut app), 1, "the cache change signals the bar once");

    app.world_mut()
        .resource_mut::<crate::player::OwnControlLost>()
        .0 = false;
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
        .init_resource::<crate::creature_anim::UnitEdits>()
        .add_message::<crate::player::StandStateRequest>()
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
    feed_inputs(&mut app);
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
    add_action_feed(&mut app);

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

/// The second unit the producer and range tests stand around the player.
const OTHER: u64 = 0xB2;

/// The feed with `buttons` (wire slot, action, kind) on the bar and `spells` in the catalog: the
/// player [`PLAYER`] at the origin, guid [`ME`], and [`OTHER`] a [`MOB`] `x` yards east, nobody
/// selected.
fn feed_app(buttons: &[(u8, u32, u8)], spells: Spells, x: f32) -> App {
    use benilla_protocol::messages::ActionButton;
    use benilla_protocol::ObjectFields;
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = PlayerActions::default();
    for &(slot, action, kind) in buttons {
        actions
            .buttons
            .insert(slot, ActionButton { slot, action, kind });
    }
    app.insert_resource(actions)
        .insert_resource(spells)
        .init_resource::<crate::ui_macro::MacroBoundSpells>()
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::creature_anim::UnitEdits>()
        .add_message::<crate::player::StandStateRequest>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<crate::player::Player>()
        .init_resource::<crate::player::OwnControlLost>()
        .init_resource::<Items>()
        .insert_resource(NetCommands(tx));
    feed_inputs(&mut app);
    let world = app.world_mut();
    let me = world
        .spawn((
            SelfPlayer,
            crate::net::Guid(ME),
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(PLAYER)),
        ))
        .id();
    let other = world
        .spawn((
            crate::net::Guid(OTHER),
            Transform::from_xyz(x, 0.0, 0.0),
            ObjectStore(ObjectFields::from_pairs(MOB)),
        ))
        .id();
    let mut index = world.resource_mut::<crate::net::GuidIndex>();
    index.0.insert(ME, me);
    index.0.insert(OTHER, other);
    app.insert_non_send_resource(UiScript::new().unwrap());
    add_action_feed(&mut app);
    app
}

/// `spells`, each with its id, as the catalog, and `ranges` as the range table.
fn catalog(spells: Vec<(u32, SpellDisplay)>, ranges: &[(u32, f32, f32)]) -> Spells {
    Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(spells.into_iter().collect()),
        ranges: benilla_formats::SpellRangeCatalog::from_rows(
            ranges
                .iter()
                .map(|&(id, min, max)| (id, benilla_formats::SpellRange { min, max, flags: 0 }))
                .collect(),
        ),
        ..Spells::empty_for_tests()
    }
}

/// Selects `guid`'s unit, or clears the selection.
fn select(app: &mut App, guid: Option<u64>) {
    let target = guid.and_then(|g| {
        app.world()
            .resource::<crate::net::GuidIndex>()
            .0
            .get(&g)
            .copied()
    });
    app.insert_resource(Selection {
        target,
        guid,
        last: None,
    });
}

/// A recorder of `events` into the VM's `HEARD`, in the order they fire.
fn record(app: &mut App, events: &[&str]) {
    let registers: String = events
        .iter()
        .map(|e| format!("f:RegisterEvent('{e}') "))
        .collect();
    app.world()
        .non_send_resource::<UiScript>()
        .run(&format!(
            "HEARD = {{}} local f = CreateFrame('Frame') {registers} \
             f:SetScript('OnEvent', function() table.insert(HEARD, event) end)"
        ))
        .unwrap();
}

/// One frame of the feed, and what the recorder heard in it.
fn frame(app: &mut App) -> Vec<String> {
    app.update();
    let script = app.world().non_send_resource::<UiScript>();
    let heard = script.eval::<Vec<String>>("return HEARD").unwrap();
    script.run("HEARD = {}").unwrap();
    heard
}

/// One descriptor edge on `guid`, as a values block reports it.
fn edge(
    app: &mut App,
    guid: u64,
    kind: benilla_protocol::messages::ObjectType,
    index: u16,
    old: u32,
    new: u32,
) {
    let entity = app.world().resource::<crate::net::GuidIndex>().0[&guid];
    app.world_mut().write_message(crate::net::FieldChanged {
        entity,
        guid,
        kind,
        index,
        old,
        new,
    });
}

/// `IsActionInRange` (`0x4e56f0`) runs the pre-check `0x6e4440` on the selection, which compares
/// the distance only when a relation the spell's targets name admits the target, then answers 1
/// only for a first implicit target in the table at `0x4e57cc`. A Fireball (enemy, 6) is 0 past
/// its range; a heal (assist, 21) over an enemy the player cannot assist is never compared and
/// answers 1 far away; an area spell (all enemies around the caster, 22) is nil whatever the
/// distance; a unit with `UNIT_FIELD_FLAGS & 0x10000` passes untested (`0x6e4517`).
#[test]
fn is_action_in_range_compares_only_where_a_relation_admits_the_target() {
    use benilla_protocol::ObjectFields;
    const FIREBALL: u32 = 133;
    const HEAL: u32 = 2054;
    const NOVA: u32 = 122;
    let spell = |target: u32| SpellDisplay {
        range_index: 4,
        effect_implicit_target_a: [target, 0, 0],
        ..Default::default()
    };
    let answers = |x: f32, selected: Option<u64>, flags: u32| {
        let spells = catalog(
            vec![(FIREBALL, spell(6)), (HEAL, spell(21)), (NOVA, spell(22))],
            &[(4, 0.0, 30.0)],
        );
        let mut app = feed_app(
            &[
                (0, FIREBALL, ACTION_KIND_SPELL),
                (1, HEAL, ACTION_KIND_SPELL),
                (2, NOVA, ACTION_KIND_SPELL),
            ],
            spells,
            x,
        );
        let other = app.world().resource::<crate::net::GuidIndex>().0[&OTHER];
        app.world_mut()
            .entity_mut(other)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x9),
                (22, 100),
                (46, flags),
            ])));
        select(&mut app, selected);
        app.update();
        let script = app.world().non_send_resource::<UiScript>();
        (1..=3)
            .map(|n| {
                script
                    .eval::<Option<i64>>(&format!("return IsActionInRange({n})"))
                    .unwrap()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        answers(20.0, Some(OTHER), 0),
        [Some(1), Some(1), None],
        "in range"
    );
    assert_eq!(
        answers(60.0, Some(OTHER), 0),
        [Some(0), Some(1), None],
        "far: the enemy is compared, the heal's assist relation does not admit it"
    );
    assert_eq!(
        answers(60.0, Some(OTHER), 0x0001_0000),
        [Some(1), Some(1), None],
        "a unit flagged 0x10000 passes before any relation"
    );
    assert_eq!(
        answers(20.0, None, 0),
        [None, None, None],
        "nil without a target (`0x4e579c`)"
    );
    assert_eq!(
        answers(60.0, Some(ME), 0),
        [Some(1), Some(1), None],
        "the player selected: his own assist compares at distance 0, nobody attacks himself"
    );
}

/// `GetActionCooldown` (`0x4e6ed0`) leaves its enable at 0 (`0x4e6f0d`) unless the slot reaches a
/// cooldown query (`0x4e6ca0`), which writes 1 first (`0x6e1406`): a spell, known or not, an
/// unresolved macro's -1 (`0x6e2ea0`), an item with a cached on-use spell (`0x6e2ed0`). A macro that
/// casts nothing or is gone, and an item with no on-use spell or no record, never query.
#[test]
fn get_action_cooldown_enables_only_the_slots_that_reach_a_query() {
    use crate::ui_macro::BoundSpell;
    use benilla_protocol::messages::{ItemUseSpell, ACTION_KIND_MACRO};
    const HEARTHSTONE: u32 = 6948;
    const LINEN: u32 = 2589;
    const UNCACHED: u32 = 4306;
    let mut app = feed_app(
        &[
            (0, 133, ACTION_KIND_SPELL),
            (1, 999_999, ACTION_KIND_SPELL),
            (2, 1, ACTION_KIND_MACRO),
            (3, 2, ACTION_KIND_MACRO),
            (4, 3, ACTION_KIND_MACRO),
            (5, HEARTHSTONE, ACTION_KIND_ITEM),
            (6, LINEN, ACTION_KIND_ITEM),
            (7, UNCACHED, ACTION_KIND_ITEM),
        ],
        catalog(vec![(133, SpellDisplay::default())], &[]),
        5.0,
    );
    let mut bound = crate::ui_macro::MacroBoundSpells::default();
    bound.0.insert(1, BoundSpell::None.into());
    bound.0.insert(2, BoundSpell::Unresolved.into());
    app.insert_resource(bound);
    let mut items = app.world_mut().resource_mut::<Items>();
    items.insert_template(
        HEARTHSTONE,
        Some(benilla_protocol::messages::ItemInfo {
            use_spell: Some(ItemUseSpell {
                spell_id: 8690,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            }),
            ..crate::items::test_template("Hearthstone")
        }),
    );
    items.insert_template(LINEN, Some(crate::items::test_template("Linen Cloth")));
    app.update();
    let script = app.world().non_send_resource::<UiScript>();
    let enable = |n: u32| {
        script
            .eval::<i64>(&format!("local _, _, e = GetActionCooldown({n}) return e"))
            .unwrap()
    };
    assert_eq!(enable(1), 1, "a spell");
    assert_eq!(
        enable(2),
        1,
        "a spell id past the table: the query writes 1 first"
    );
    assert_eq!(enable(3), 0, "a macro that casts nothing");
    assert_eq!(enable(4), 1, "an unresolved /cast, the query's -1");
    assert_eq!(enable(5), 0, "a macro that is gone");
    assert_eq!(enable(6), 1, "an item with an on-use spell");
    assert_eq!(enable(7), 0, "an item with none");
    assert_eq!(enable(8), 0, "an item whose record has not arrived");
    assert_eq!(enable(9), 0, "an empty slot");
}

/// The feed seats an item slot's entry, which `IsCurrentAction`'s item arm holds against the
/// in-flight cast's item (`0x4e53fc`-`0x4e5428`).
#[test]
fn an_item_slot_is_current_while_its_items_cast_is_in_flight() {
    const HEARTHSTONE: u32 = 6948;
    let mut app = feed_app(
        &[(0, HEARTHSTONE, ACTION_KIND_ITEM)],
        Spells::empty_for_tests(),
        5.0,
    );
    app.update();
    let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
    script.set_cast_view(benilla_ui::script::CastView {
        slot: Some(8690),
        item: Some(HEARTHSTONE),
        ..Default::default()
    });
    assert!(script
        .eval::<bool>("return IsCurrentAction(1) == 1")
        .unwrap());
}

/// Every caller of the usable refresh `0x4b31c0` signals `SPELL_UPDATE_USABLE` (`0x4b3246`), with
/// `ACTIONBAR_UPDATE_USABLE` ahead of it only when a slot's pair changed: the active player's
/// health, power and aura state watchers (`0x604848`, `0x604876`, `0x6048b6`), the target's aura
/// state (`0x6048b6`), the combo points (`0x5ddff0`), the form (`0x5de011`), the unit flags' bits
/// 0x1c20000 and the combat bit 0x80000 (`0x5ff50e`, `0x5ff51b`), byte 3's bit 0x2 of
/// `UNIT_FIELD_BYTES_1` (`0x5ff83a`), the target's select and clear (`0x493892`, `0x493a69`), the
/// control flag (`0x4958fc`, `0x495910`), a held global cooldown's start (`0x6e2e85`) and the start
/// or removal of a spell's records (`0x6e3076`). One frame's callers signal it once.
#[test]
fn spell_update_usable_fires_from_every_usable_refresh_and_once_a_frame() {
    use benilla_protocol::messages::ObjectType::{Player, Unit};
    let mut app = feed_app(&[], Spells::empty_for_tests(), 5.0);
    record(
        &mut app,
        &["ACTIONBAR_UPDATE_USABLE", "SPELL_UPDATE_USABLE"],
    );
    const USABLE: [&str; 1] = ["SPELL_UPDATE_USABLE"];
    assert!(frame(&mut app).is_empty(), "a new VM hears no refresh");
    assert!(frame(&mut app).is_empty(), "nor a quiet frame");

    for (index, old, new, why) in [
        (22, 100, 90, "health"),
        (23, 500, 480, "mana"),
        (26, 100, 80, "energy"),
        (125, 0, 2, "aura state"),
        (46, 0x8, 0x8_0008, "the combat bit"),
        (46, 0x8, 0x2_0008, "a usable bit (pacified)"),
        (138, 0, 0x0001_0000, "the form byte"),
        (138, 0, 0x0200_0000, "byte 3's bit 0x2"),
        (1222, 0, 0x0100, "the combo points"),
    ] {
        edge(&mut app, ME, Player, index, old, new);
        assert_eq!(frame(&mut app), USABLE, "{why}");
    }
    for (index, old, new, why) in [
        (46, 0x8, 0x8 | 0x1000, "a unit-flag bit no refresh watches"),
        (138, 0, 0x0100_0000, "byte 3's bit 0x1"),
        (1222, 0, 0x08, "PLAYER_FIELD_BYTES byte 0"),
        (34, 1, 2, "the level"),
    ] {
        edge(&mut app, ME, Player, index, old, new);
        assert!(frame(&mut app).is_empty(), "{why}");
    }
    edge(&mut app, OTHER, Unit, 23, 500, 480);
    edge(&mut app, OTHER, Unit, 125, 0, 2);
    assert!(
        frame(&mut app).is_empty(),
        "another unit's power and aura state"
    );

    select(&mut app, Some(OTHER));
    assert_eq!(frame(&mut app), USABLE, "the select");
    assert!(frame(&mut app).is_empty(), "the same target again");
    edge(&mut app, OTHER, Unit, 125, 2, 0);
    assert_eq!(frame(&mut app), USABLE, "the target's aura state");
    select(&mut app, None);
    assert_eq!(frame(&mut app), USABLE, "the clear");

    app.world_mut()
        .resource_mut::<crate::player::OwnControlLost>()
        .0 = true;
    assert_eq!(frame(&mut app), USABLE, "control lost");

    let now = Instant::now();
    let gcd = |attributes: u32| SpellDisplay {
        attributes,
        start_recovery_category: 133,
        start_recovery_ms: 1500,
        ..Default::default()
    };
    let (held, plain) = (gcd(0x0200_0000), gcd(0));
    let mods = crate::spell::SpellModifiers::default();
    app.world_mut()
        .resource_mut::<Cooldowns>()
        .start_gcd(1, &plain, now, &mods);
    assert!(frame(&mut app).is_empty(), "a plain global cooldown");
    app.world_mut()
        .resource_mut::<Cooldowns>()
        .start_gcd(1784, &held, now, &mods);
    assert_eq!(frame(&mut app), USABLE, "a held one (`0x6e2e85`)");
    app.world_mut()
        .resource_mut::<Cooldowns>()
        .cooldown_event(1784, now);
    assert_eq!(frame(&mut app), USABLE, "its event");
    app.world_mut()
        .resource_mut::<Cooldowns>()
        .clear_spell(1784);
    assert_eq!(frame(&mut app), USABLE, "a clear");

    edge(&mut app, ME, Player, 22, 90, 80);
    edge(&mut app, ME, Player, 23, 480, 470);
    select(&mut app, Some(OTHER));
    app.world_mut()
        .resource_mut::<crate::player::OwnControlLost>()
        .0 = false;
    assert_eq!(
        frame(&mut app),
        USABLE,
        "four callers in one frame, one signal"
    );
}

/// A refresh that changes a slot's pair signals `ACTIONBAR_UPDATE_USABLE` (`0x4e5c53`) and then
/// `SPELL_UPDATE_USABLE` (`0x4b3246`), the pair `0x4b31c0` raises in that order.
#[test]
fn a_refresh_that_changes_a_slot_signals_the_bar_then_the_spells() {
    let mut app = feed_app(
        &[(0, 133, ACTION_KIND_SPELL)],
        catalog(vec![(133, SpellDisplay::default())], &[]),
        5.0,
    );
    record(
        &mut app,
        &["ACTIONBAR_UPDATE_USABLE", "SPELL_UPDATE_USABLE"],
    );
    frame(&mut app);
    app.world_mut()
        .resource_mut::<crate::player::OwnControlLost>()
        .0 = true;
    assert_eq!(
        frame(&mut app),
        ["ACTIONBAR_UPDATE_USABLE", "SPELL_UPDATE_USABLE"]
    );
}

/// The action feed beside the unit feed, with a recorder whose `SPELL_UPDATE_USABLE` line carries
/// what its handler read: `HasFullControl()`, `UnitExists("target")` and `UnitHealth("player")`.
fn usable_reader_app() -> App {
    let mut app = feed_app(
        &[(0, 133, ACTION_KIND_SPELL)],
        catalog(vec![(133, SpellDisplay::default())], &[]),
        5.0,
    );
    app.init_resource::<crate::names::NameCache>()
        .init_resource::<crate::ui_party::GroupState>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .init_resource::<crate::ui_guild::GuildState>();
    crate::ui_unit::add_unit_feed(&mut app);
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            "HEARD = {} local f = CreateFrame('Frame') \
             for _, e in ipairs({'PLAYER_CONTROL_LOST', 'PLAYER_CONTROL_GAINED', \
                 'SPELL_UPDATE_USABLE', 'PLAYER_TARGET_CHANGED'}) do f:RegisterEvent(e) end \
             f:SetScript('OnEvent', function() \
                 if event == 'SPELL_UPDATE_USABLE' then \
                     event = event .. ' control=' .. tostring(HasFullControl()) \
                         .. ' target=' .. tostring(UnitExists('target')) \
                         .. ' health=' .. tostring(UnitHealth('player')) \
                 end \
                 table.insert(HEARD, event) \
             end)",
        )
        .unwrap();
    frame(&mut app);
    app
}

/// Sets whether the player has lost control, as `SMSG_CLIENT_CONTROL_UPDATE` reports it.
fn set_control_lost(app: &mut App, lost: bool) {
    app.world_mut()
        .resource_mut::<crate::player::OwnControlLost>()
        .0 = lost;
}

/// `SetPlayerControl` (`0x4958e0`) stores the flag (`0x4958ea`) and signals
/// `PLAYER_CONTROL_LOST`/`GAINED` (`0x4958f7`, `0x49590b`) before its usable refresh (`0x4958fc`,
/// `0x495910`): the refresh's handlers hear it after the edge and read the stored flag.
#[test]
fn a_usable_handler_hears_the_control_edge_first_and_reads_its_flag() {
    let mut app = usable_reader_app();
    set_control_lost(&mut app, true);
    assert_eq!(
        frame(&mut app),
        [
            "PLAYER_CONTROL_LOST",
            "SPELL_UPDATE_USABLE control=nil target=nil health=100",
        ]
    );
    set_control_lost(&mut app, false);
    assert_eq!(
        frame(&mut app),
        [
            "PLAYER_CONTROL_GAINED",
            "SPELL_UPDATE_USABLE control=1 target=nil health=100",
        ]
    );
}

/// `SelectTarget` stores the target (`0x4936e2`) and refreshes (`0x493892`) before
/// `PLAYER_TARGET_CHANGED` (`0x49389c`), as a release does (`0x4939d3`, `0x493a69`,
/// `0x493a73`), and the health watch (`0x604848`) runs after its field is written: a usable
/// handler reads this frame's target and health, and the target's event comes after it.
#[test]
fn a_usable_handler_reads_this_frames_target_and_health() {
    use benilla_protocol::messages::ObjectType;
    let mut app = usable_reader_app();
    let me = app.world().resource::<crate::net::GuidIndex>().0[&ME];
    app.world_mut()
        .get_mut::<ObjectStore>(me)
        .unwrap()
        .0
        .merge_diff(
            benilla_protocol::ObjectFields::from_pairs(&[(22, 80)]),
            |_, _, _| {},
        );
    edge(&mut app, ME, ObjectType::Player, 22, 100, 80);
    select(&mut app, Some(OTHER));
    assert_eq!(
        frame(&mut app),
        [
            "SPELL_UPDATE_USABLE control=1 target=1 health=80",
            "PLAYER_TARGET_CHANGED",
        ]
    );
    select(&mut app, None);
    assert_eq!(
        frame(&mut app),
        [
            "SPELL_UPDATE_USABLE control=1 target=nil health=80",
            "PLAYER_TARGET_CHANGED",
        ]
    );
}

/// `BAG_UPDATE_COOLDOWN` (`0x4f93d0`) comes from the item legs alone: `SMSG_ITEM_COOLDOWN`
/// (`0x6e965b`), the player's start or removal of a spell's records (`0x6e3089`) and its list clear
/// (`0x6e9721`), and the spell charges watcher (`0x5d9587`). The global cooldown's start
/// (`0x6e2e77`) and `SMSG_SPELL_COOLDOWN` (`0x6e95b0`) flush the cooldown pair only, and a held
/// global cooldown adds the usable refresh (`0x6e2e85`). Each handler flushes whether or not it
/// matched a record. Each frame's events come in the reference's order: the pair (`0x4b31b0`),
/// the usable refresh (`0x4b31c0`), then the bags.
#[test]
fn bag_update_cooldown_fires_from_the_item_legs_alone() {
    const PAIR: [&str; 2] = ["ACTIONBAR_UPDATE_COOLDOWN", "SPELL_UPDATE_COOLDOWN"];
    let mut app = feed_app(&[], Spells::empty_for_tests(), 5.0);
    record(
        &mut app,
        &[
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "ACTIONBAR_UPDATE_USABLE",
            "SPELL_UPDATE_USABLE",
            "BAG_UPDATE_COOLDOWN",
        ],
    );
    assert!(
        !frame(&mut app).contains(&"BAG_UPDATE_COOLDOWN".to_string()),
        "a new VM's first frame is no item leg"
    );
    let now = Instant::now();
    let gcd = |attributes: u32| SpellDisplay {
        attributes,
        start_recovery_category: 133,
        start_recovery_ms: 1500,
        ..Default::default()
    };
    let fireball = gcd(0);
    let mods = crate::spell::SpellModifiers::default();
    fn cds(app: &mut App) -> Mut<'_, Cooldowns> {
        app.world_mut().resource_mut::<Cooldowns>()
    }

    cds(&mut app).start_gcd(133, &fireball, now, &mods);
    assert_eq!(frame(&mut app), PAIR, "a cast's global cooldown");
    cds(&mut app).apply_spell_cooldowns(
        [(133, 8000, Some(&fireball))],
        &mods,
        crate::spell::CategoryPad::NoPlayer,
        now,
    );
    assert_eq!(frame(&mut app), PAIR, "SMSG_SPELL_COOLDOWN");
    cds(&mut app).apply_wire_item_cooldown(6948, 8690, now);
    assert_eq!(
        frame(&mut app),
        [PAIR[0], PAIR[1], "BAG_UPDATE_COOLDOWN"],
        "SMSG_ITEM_COOLDOWN"
    );
    let full = [
        PAIR[0],
        PAIR[1],
        "SPELL_UPDATE_USABLE",
        "BAG_UPDATE_COOLDOWN",
    ];
    cds(&mut app).cooldown_event(1784, now);
    assert_eq!(frame(&mut app), full, "SMSG_COOLDOWN_EVENT, nothing parked");
    cds(&mut app).clear_spell(1784);
    assert_eq!(frame(&mut app), full, "SMSG_CLEAR_COOLDOWN, nothing held");
    cds(&mut app).wipe();
    assert_eq!(
        frame(&mut app),
        [PAIR[0], PAIR[1], "BAG_UPDATE_COOLDOWN"],
        "SMSG_COOLDOWN_CHEAT"
    );
    cds(&mut app).wipe();
    assert_eq!(
        frame(&mut app),
        [PAIR[0], PAIR[1], "BAG_UPDATE_COOLDOWN"],
        "the cheat on an empty list flushes too"
    );
    let stealth = gcd(0x0200_0000);
    cds(&mut app).start_gcd(1784, &stealth, now, &mods);
    assert_eq!(
        frame(&mut app),
        [PAIR[0], PAIR[1], "SPELL_UPDATE_USABLE"],
        "a held global cooldown"
    );
    app.world_mut()
        .write_message(crate::item_watch::ItemNotice::SpellCharges {
            guid: 0x4000_0000_0000_0042,
            entry: 5512,
        });
    assert_eq!(
        frame(&mut app),
        ["BAG_UPDATE_COOLDOWN"],
        "the spell charges watcher"
    );
    assert!(frame(&mut app).is_empty(), "and a quiet frame after");
}

/// `GetItemCooldown` (`0x6e2ed0`) asks a missing item record with the callback `0x6e2f70`, which on
/// the record's arrival queries again and, with the cooldown running, flushes the pair and the bags
/// (`0x6e2f89`, `0x6e2f8e`). An item slot whose record lands with nothing running flushes nothing.
#[test]
fn an_item_slots_record_landing_flushes_its_running_cooldown() {
    use benilla_protocol::messages::ItemUseSpell;
    const HEARTHSTONE: u32 = 6948;
    const HEARTH: u32 = 8690;
    const LINEN: u32 = 2589;
    let mut app = feed_app(
        &[
            (0, HEARTHSTONE, ACTION_KIND_ITEM),
            (1, LINEN, ACTION_KIND_ITEM),
        ],
        Spells::empty_for_tests(),
        5.0,
    );
    record(
        &mut app,
        &[
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "BAG_UPDATE_COOLDOWN",
        ],
    );
    app.world_mut()
        .resource_mut::<Cooldowns>()
        .apply_wire_item_cooldown(HEARTHSTONE, HEARTH, Instant::now());
    frame(&mut app);
    assert!(frame(&mut app).is_empty(), "the slot waits for its record");
    let hearthstone = benilla_protocol::messages::ItemInfo {
        use_spell: Some(ItemUseSpell {
            spell_id: HEARTH,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        }),
        ..crate::items::test_template("Hearthstone")
    };
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(HEARTHSTONE, Some(hearthstone));
    assert_eq!(
        frame(&mut app),
        [
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "BAG_UPDATE_COOLDOWN"
        ],
        "the record lands on a running cooldown"
    );
    assert!(frame(&mut app).is_empty(), "once");
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(LINEN, Some(crate::items::test_template("Linen Cloth")));
    assert!(
        frame(&mut app).is_empty(),
        "a record landing with nothing running"
    );
}

/// The three events a record callback `0x6e2f70` fires (`0x6e2f89`, `0x6e2f8e`).
const RECORD_FLUSH: [&str; 3] = [
    "ACTIONBAR_UPDATE_COOLDOWN",
    "SPELL_UPDATE_COOLDOWN",
    "BAG_UPDATE_COOLDOWN",
];

/// Hearthstone's record, its on-use spell 8690.
fn hearthstone_record() -> benilla_protocol::messages::ItemInfo {
    benilla_protocol::messages::ItemInfo {
        use_spell: Some(benilla_protocol::messages::ItemUseSpell {
            spell_id: 8690,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        }),
        ..crate::items::test_template("Hearthstone")
    }
}

/// The callback `0x6e2f70` is the item cache's, not the asking slot's: a record that lands after
/// the slot was cleared still flushes its running cooldown (`0x6e2f80`-`0x6e2f8e`).
#[test]
fn a_record_landing_after_its_slot_was_cleared_still_flushes() {
    const HEARTHSTONE: u32 = 6948;
    let mut app = feed_app(
        &[(0, HEARTHSTONE, ACTION_KIND_ITEM)],
        Spells::empty_for_tests(),
        5.0,
    );
    record(&mut app, &RECORD_FLUSH);
    app.world_mut()
        .resource_mut::<Cooldowns>()
        .apply_wire_item_cooldown(HEARTHSTONE, 8690, Instant::now());
    frame(&mut app);
    app.world_mut()
        .resource_mut::<PlayerActions>()
        .buttons
        .clear();
    assert!(
        frame(&mut app).is_empty(),
        "the slot is gone, its ask is not"
    );
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(HEARTHSTONE, Some(hearthstone_record()));
    assert_eq!(frame(&mut app), RECORD_FLUSH);
}

/// `GetContainerItemCooldown` on a bag slot whose record is missing asks it with the callback
/// `0x6e2f70` (`0x4f9a7f` → `0x6e2efb`), whose landing on a running cooldown fires the pair and
/// `BAG_UPDATE_COOLDOWN` (`0x6e2f89`, `0x6e2f8e`).
#[test]
fn a_bag_query_on_a_missing_record_flushes_when_the_record_lands() {
    use benilla_ui::script::{ContainerSlot, ContainerState};
    const HEARTHSTONE: u32 = 6948;
    let mut app = feed_app(&[], Spells::empty_for_tests(), 5.0);
    record(&mut app, &RECORD_FLUSH);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_container(
            0,
            Some(ContainerState {
                name: None,
                num_slots: 16,
                slots: std::collections::HashMap::from([(
                    1,
                    ContainerSlot {
                        item_id: HEARTHSTONE,
                        count: 1,
                        ..Default::default()
                    },
                )]),
            }),
        );
    app.world_mut()
        .resource_mut::<Cooldowns>()
        .apply_wire_item_cooldown(HEARTHSTONE, 8690, Instant::now());
    frame(&mut app);
    app.world()
        .non_send_resource::<UiScript>()
        .run("GetContainerItemCooldown(0, 1)")
        .unwrap();
    assert!(frame(&mut app).is_empty(), "asked, and waiting");
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(HEARTHSTONE, Some(hearthstone_record()));
    assert_eq!(frame(&mut app), RECORD_FLUSH);
    assert!(frame(&mut app).is_empty(), "once");
}

/// The feed's two halves sit around the unit snapshots, and its signals come ahead of the events
/// its producers precede. The push half seats the action states before the snapshots, which
/// `PLAYER_ENTERING_WORLD`'s handlers read. The fire half runs after the snapshots and the
/// control flag are this frame's: the target is stored (`0x4936e2`) and the control flag stored
/// and signalled (`0x4958ea`, `0x4958f7`) before the usable refresh (`0x493892`, `0x4958fc`).
/// It signals before `PLAYER_TARGET_CHANGED` (`0x493892` ahead of `0x49389c`), `UNIT_HEALTH` and
/// `UNIT_MANA` (the type watchers ahead of the object's bridge at one field, `0x46544a`),
/// `PLAYER_REGEN_DISABLED` (`0x5ff51b` ahead of `0x5ff533`), `PLAYER_COMBO_POINTS` (`0x5ddff0`)
/// and the form's cast change (`0x5de011` ahead of `0x5de016`). The bag feed runs ahead of the
/// push half, whose usable cache each `BAG_UPDATE` refreshes (`0x4f9374`), so the refresh lands in
/// the update's own frame. Read off the declared graph; skips where bevy carries no system names.
#[test]
fn the_feed_runs_around_the_snapshots_and_before_the_events_its_producers_precede() {
    use crate::game_plugins::schedule_tests::{census, headless_client, SyncPoints};
    use std::collections::HashSet;

    let mut app = headless_client();
    let c = census(&mut app, Update, SyncPoints::Declared);
    let named = |needle: &str| {
        let hits: Vec<_> = c
            .systems
            .iter()
            .filter(|(_, s)| s.name.ends_with(needle))
            .map(|(k, _)| *k)
            .collect();
        (!hits.is_empty()).then(|| {
            assert_eq!(hits.len(), 1, "one system named {needle}");
            hits[0]
        })
    };
    let Some(push) = named("::feed_action_state") else {
        eprintln!("skipped: this build carries no system names");
        return;
    };
    let mut after: HashMap<_, Vec<_>> = HashMap::new();
    for (a, b) in &c.dependencies {
        after.entry(*a).or_default().push(*b);
    }
    let later_than = |from| {
        let (mut seen, mut stack) = (HashSet::new(), vec![from]);
        while let Some(n) = stack.pop() {
            for m in after.get(&n).into_iter().flatten() {
                if seen.insert(*m) {
                    stack.push(*m);
                }
            }
        }
        seen
    };
    let key = |name: &str| named(name).unwrap_or_else(|| panic!("{name} is scheduled"));
    let fire = key("::fire_action_events");
    assert!(
        later_than(key("::ui_items::feed::feed_containers")).contains(&push),
        "the bag feed runs before the push half"
    );
    assert!(
        later_than(push).contains(&key("::ui_unit::feed_units")),
        "the push half runs before the unit snapshots"
    );
    for earlier in ["::ui_unit::feed_units", "::ui_unit::feed_player_control"] {
        assert!(
            later_than(key(earlier)).contains(&fire),
            "the fire half runs after {earlier}"
        );
    }
    let seen = later_than(fire);
    for later in [
        "::ui_unit::fire_unit_events",
        "::field_events::fire_field_notices",
        "::ui_cast::feed_cast_reports",
    ] {
        assert!(
            seen.contains(&key(later)),
            "the fire half runs before {later}"
        );
    }
}

/// Sets the player's mana to `mana` with no values block, so no watcher runs.
fn set_mana_quietly(app: &mut App, mana: u32) {
    let me = app.world().resource::<crate::net::GuidIndex>().0[&ME];
    let mut fields: Vec<(u16, u32)> = PLAYER.to_vec();
    fields.retain(|&(i, _)| i != 23);
    fields.push((23, mana));
    app.world_mut()
        .entity_mut(me)
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(
            &fields,
        )));
}

/// Placing, clearing or recounting a slot runs `0x4e58e0`, which stores the slot's new pair
/// (`0x4e590a`, `0x4e5914`) and signals only `ACTIONBAR_SLOT_CHANGED`: no usable event, and the
/// next refresh finds that pair already stored.
#[test]
fn placing_or_clearing_a_slot_stores_its_pair_with_no_usable_event() {
    use benilla_protocol::messages::ActionButton;
    let mut app = feed_app(
        &[(0, 133, ACTION_KIND_SPELL)],
        catalog(
            vec![
                (133, SpellDisplay::default()),
                (116, SpellDisplay::default()),
            ],
            &[],
        ),
        5.0,
    );
    app.world_mut()
        .resource_mut::<PlayerActions>()
        .spells
        .extend([133, 116]);
    record(
        &mut app,
        &["ACTIONBAR_UPDATE_USABLE", "SPELL_UPDATE_USABLE"],
    );
    assert!(frame(&mut app).is_empty(), "the bar as it stands at load");
    app.world_mut()
        .resource_mut::<PlayerActions>()
        .buttons
        .insert(
            1,
            ActionButton {
                slot: 1,
                action: 116,
                kind: ACTION_KIND_SPELL,
            },
        );
    assert!(frame(&mut app).is_empty(), "a placed spell");
    edge(
        &mut app,
        ME,
        benilla_protocol::messages::ObjectType::Player,
        22,
        100,
        90,
    );
    assert_eq!(
        frame(&mut app),
        ["SPELL_UPDATE_USABLE"],
        "a refresh finds the placed pair stored"
    );
    app.world_mut()
        .resource_mut::<PlayerActions>()
        .buttons
        .remove(&1);
    assert!(frame(&mut app).is_empty(), "a cleared slot");
}

/// `0x4e5c00` signals `ACTIONBAR_UPDATE_USABLE` alone for its callers other than `0x4b31c0`: a
/// reload's world entry against the cache the last VM left (`0x4e4f90`; the cache is zeroed only
/// by `InitializeGame`, `0x4e4f50`), each `BAG_UPDATE` (`0x4f9374`) and `EditMacro` (`0x4f132e`).
/// A pair that changed with no refresh signals nothing. Our player's arrival writes every slot
/// before its enter cascade refreshes (`0x5deafc`, `0x5deb60`), so it signals nothing; a reload
/// writes none (`0x495664` reaches no `0x5dea50`).
#[test]
fn the_cache_refreshes_alone_at_a_reload_a_bag_update_and_a_macro_edit() {
    const FROSTBOLT: u32 = 116;
    let costly = SpellDisplay {
        mana_cost: 400,
        ..Default::default()
    };
    let mut app = feed_app(
        &[(0, FROSTBOLT, ACTION_KIND_SPELL)],
        catalog(vec![(FROSTBOLT, costly)], &[]),
        5.0,
    );
    app.world_mut()
        .resource_mut::<PlayerActions>()
        .spells
        .insert(FROSTBOLT);
    const USABLE: [&str; 2] = ["ACTIONBAR_UPDATE_USABLE", "SPELL_UPDATE_USABLE"];
    record(&mut app, &USABLE);
    assert!(frame(&mut app).is_empty());

    set_mana_quietly(&mut app, 100);
    assert!(frame(&mut app).is_empty(), "no caller, no event");
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.world_mut()
        .write_message(crate::net::WorldEnterCascadeMessage);
    record(&mut app, &USABLE);
    assert_eq!(
        frame(&mut app),
        ["ACTIONBAR_UPDATE_USABLE"],
        "a reload's world entry finds the pair the last VM's cache held changed"
    );

    set_mana_quietly(&mut app, 500);
    assert!(frame(&mut app).is_empty());
    app.world_mut()
        .resource_mut::<crate::ui_items::feed::BagUpdates>()
        .0 += 1;
    assert_eq!(frame(&mut app), ["ACTIONBAR_UPDATE_USABLE"], "a BAG_UPDATE");

    set_mana_quietly(&mut app, 100);
    assert!(frame(&mut app).is_empty());
    app.world()
        .non_send_resource::<UiScript>()
        .run("CreateMacro('m', 1, '/say hi', 1) EditMacro(1, 'n')")
        .unwrap();
    assert_eq!(frame(&mut app), ["ACTIONBAR_UPDATE_USABLE"], "an EditMacro");

    set_mana_quietly(&mut app, 500);
    assert!(frame(&mut app).is_empty());
    arrive(&mut app, 500);
    assert!(
        frame(&mut app).is_empty(),
        "our player's arrival stores every slot ahead of its refresh"
    );
}

/// Our player created anew with `mana`, and the enter cascade its arrival runs.
fn arrive(app: &mut App, mana: u32) {
    let old = app.world().resource::<crate::net::GuidIndex>().0[&ME];
    app.world_mut().despawn(old);
    let mut fields: Vec<(u16, u32)> = PLAYER.to_vec();
    fields.retain(|&(i, _)| i != 23);
    fields.push((23, mana));
    let me = app
        .world_mut()
        .spawn((
            SelfPlayer,
            crate::net::Guid(ME),
            Transform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&fields)),
        ))
        .id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    app.world_mut()
        .write_message(crate::net::WorldEnterCascadeMessage);
}

/// The arrival's loop (`0x5deae2`-`0x5deb12`) writes each slot but a spell word the player does
/// not know (`0x60c740`, skipped at `0x5deaf5`), so that slot keeps the pair the cache held and the
/// cascade's refresh (`0x4e4f90`) finds it changed; the known spell beside it signals nothing.
#[test]
fn an_arrival_skips_a_spell_the_player_does_not_know() {
    const FROSTBOLT: u32 = 116;
    const BLIZZARD: u32 = 10;
    let costly = || SpellDisplay {
        mana_cost: 400,
        ..Default::default()
    };
    let mut app = feed_app(
        &[
            (0, FROSTBOLT, ACTION_KIND_SPELL),
            (1, BLIZZARD, ACTION_KIND_SPELL),
        ],
        catalog(vec![(FROSTBOLT, costly()), (BLIZZARD, costly())], &[]),
        5.0,
    );
    app.world_mut()
        .resource_mut::<PlayerActions>()
        .spells
        .insert(FROSTBOLT);
    record(&mut app, &["ACTIONBAR_UPDATE_USABLE"]);
    frame(&mut app);
    set_mana_quietly(&mut app, 100);
    assert!(frame(&mut app).is_empty(), "no caller, no event");
    arrive(&mut app, 100);
    assert_eq!(
        frame(&mut app),
        ["ACTIONBAR_UPDATE_USABLE"],
        "the unknown spell's slot kept its pair"
    );

    app.world_mut()
        .resource_mut::<PlayerActions>()
        .spells
        .insert(BLIZZARD);
    set_mana_quietly(&mut app, 500);
    assert!(frame(&mut app).is_empty());
    arrive(&mut app, 500);
    assert!(frame(&mut app).is_empty(), "both known: both stored");
}

/// A macro bound to a spell out of the pet's book (`[rec+0x568] != 0`, `0x4e5a50`) is read through
/// the pet: usable only past the pet gate `0x4bcf70` and when the spell's cost, from the player's
/// fields, is within the pet's power (`0x4e51b2`-`0x4e5216`, equal passes), its cooldown from the
/// pet's history (`0x6e2ea0` with the pet selector), and no range at all (`0x4e5759`, `0x4e584d`).
#[test]
fn a_macro_bound_to_a_pet_spell_reads_the_pets_gate_power_and_history() {
    use crate::ui_macro::{Bound, BoundSpell, MacroBoundSpells};
    use crate::ui_pet::PetBar;
    use benilla_protocol::ObjectFields;

    const PET: u64 = 0x2A;
    const SPELL_LOCK: u32 = 19647;
    const MACRO_BAR: u8 = 0x40;
    // `UNIT_FIELD_SUMMONEDBY`, `UNIT_FIELD_CREATEDBY`, health, power 1 and flags.
    let pet_fields = |power: u32, flags: u32| {
        ObjectStore(ObjectFields::from_pairs(&[
            (12, ME as u32),
            (14, ME as u32),
            (22, 100),
            (23, power),
            (46, flags),
        ]))
    };
    let lock = || SpellDisplay {
        mana_cost: 100,
        recovery_ms: 24_000,
        range_index: 4,
        effect_implicit_target_a: [6, 0, 0],
        ..Default::default()
    };
    // One fresh client per pet state, read after its first frame, which is a refresh of the usable
    // cache (`0x4e5c00`, the new VM's). `IsUsableAction` answers that cache (`0x4e7403` into
    // `0x4e5230`), and the power watch refreshes it only for the active player (`0x60486b`), so
    // what this asserts is what a refresh finds, not that the button follows the pet's power.
    let at_a_refresh = |power: u32, flags: u32| {
        let mut app = feed_app(
            &[(0, 1, MACRO_BAR)],
            catalog(vec![(SPELL_LOCK, lock())], &[(4, 0.0, 35.0)]),
            10.0,
        );
        app.world_mut().resource_mut::<MacroBoundSpells>().0.insert(
            1,
            Bound {
                spell: BoundSpell::Spell(SPELL_LOCK),
                pet_book: true,
            },
        );
        let pet = app
            .world_mut()
            .spawn((
                crate::net::Guid(PET),
                Transform::default(),
                pet_fields(power, flags),
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(PET, pet);
        let mut bar = PetBar::default();
        bar.spells.pet_guid = PET;
        bar.cooldowns
            .start_spell(SPELL_LOCK, &lock(), 0, Instant::now(), None);
        app.insert_resource(bar);
        // A hostile in range, where a player's own Spell Lock would read a range.
        select(&mut app, Some(OTHER));
        app.update();
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<(Option<i64>, Option<i64>, Option<i64>, Option<i64>, f64)>(
                "local u, oom = IsUsableAction(1) \
                 local _, d = GetActionCooldown(1) \
                 return u, oom, ActionHasRange(1), IsActionInRange(1), d",
            )
            .unwrap()
    };

    let (usable, oom, has_range, in_range, duration) = at_a_refresh(150, 0);
    assert_eq!(
        (usable, oom),
        (Some(1), None),
        "150 power against a cost of 100"
    );
    assert_eq!((has_range, in_range), (None, None), "no range for book 1");
    assert_eq!(
        duration, 24.0,
        "the pet's history, which the player's list lacks"
    );
    assert_eq!(at_a_refresh(100, 0).0, Some(1), "equal passes");
    let (usable, oom, ..) = at_a_refresh(99, 0);
    assert_eq!(
        (usable, oom),
        (None, Some(1)),
        "only the power gate sets notEnoughMana"
    );
    // Stunned (0x40000): the gate fails before the cost is read, so nothing is out of power.
    let (usable, oom, ..) = at_a_refresh(500, 0x4_0000);
    assert_eq!((usable, oom), (None, None));
}
