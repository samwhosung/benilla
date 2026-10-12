//! The trainer feed's tests: the laws, service resolution, the snapshot and the re-evaluator.

use super::*;
use benilla_formats::{
    ItemDisplay, ItemDisplayCatalog, SpellDisplay, SPELL_ATTR_IS_TRADESKILL,
    SPELL_EFFECT_CREATE_ITEM, SPELL_EFFECT_LEARN_PET_SPELL, SPELL_EFFECT_LEARN_SPELL,
};
use benilla_protocol::messages::trainer_spell_state;
use benilla_ui::script::TrainerTooltip;
use std::collections::HashMap;

/// An empty spell catalog, as before `Spell.dbc` loads.
fn empty_catalog() -> SpellCatalog {
    SpellCatalog::from_displays(HashMap::new())
}

/// What [`service_icon`] needs; a template is in flight unless a test seeds one.
use crate::items::TestDeps as Deps;

/// Header labels that name their key, so each assertion says which lookup the arm made.
fn probe_strings(key: &str) -> Option<String> {
    match key {
        "TRADESKILL_SERVICE_STEP" => Some("<STEP>".into()),
        "TRADESKILL_SERVICE_LEARN" => Some("<LEARN>".into()),
        "KNOWN_TALENTS_HEADER" => Some("<KNOWN>".into()),
        _ => None,
    }
}

/// Run `f` with the description contexts over the test catalogs: the casters' skill values
/// (`level × 5` for a pet) and the bind point `$z` names.
fn with_text<R>(
    spells: &SpellCatalog,
    player_skill: u32,
    pet_skill: u32,
    home: Option<&str>,
    f: impl FnOnce(&ServiceText) -> R,
) -> R {
    let durations = benilla_formats::SpellDurationCatalog::default();
    let radii = benilla_formats::SpellRadiusCatalog::default();
    let lookup = |id: u32| spells.get(id);
    let skill_of = |_: u32| player_skill;
    let pet_of = |_: u32| pet_skill;
    let ctx = TokenContext {
        durations: &durations,
        radii: &radii,
        ranges: None,
        skill: &skill_of,
        lookup: &lookup,
        mods: None,
        unmodified_points: false,
        gender: &|| 0,
        home_area: &|| home,
        global: &probe_strings,
        printf: &crate::ui_script::token_printf,
    };
    let pet_ctx = TokenContext {
        skill: &pet_of,
        ..ctx
    };
    f(&ServiceText {
        player: &ctx,
        pet: &pet_ctx,
    })
}

/// [`resolve_service`] with no icon sources.
fn resolve_service(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    known: &BTreeSet<u32>,
) -> TrainerService {
    let deps = Deps::new();
    with_text(spells, 0, 0, None, |text| {
        super::resolve_service(
            wire,
            trainer_type,
            spells,
            skill_lines,
            known,
            None,
            &deps.items,
            &deps.commands,
            text,
            &probe_strings,
        )
    })
}

fn wire(spell: u32, state: u8, cost: u32, req_level: u8, req_skill: u32) -> TrainerSpell {
    TrainerSpell {
        spell,
        state,
        cost,
        can_learn_primary_prof: false,
        is_primary_prof_first_rank: false,
        req_level,
        req_skill,
        req_skill_value: if req_skill != 0 { 100 } else { 0 },
        req_spells: [0, 0, 0],
    }
}

/// [`snapshot`] with no icon sources.
fn snap(open: &TrainerOpen, spells: &SpellCatalog) -> Option<TrainerState> {
    let deps = Deps::new();
    with_text(spells, 0, 0, None, |text| {
        snapshot(
            open,
            spells,
            None,
            &BTreeSet::new(),
            None,
            &deps.items,
            &deps.commands,
            text,
            &probe_strings,
        )
    })
}

/// Wrapper 100 teaches 200 by a slot-0 `LEARN_SPELL`, 200 creates item 777, and 777's display 5
/// has the item art; every icon is distinct, so a wrong arm is named by the assertion.
fn icon_catalog() -> SpellCatalog {
    let wrapper = SpellDisplay {
        name: "Copper Shortsword".into(),
        icon: Some("WRAPPER".into()),
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        name: "Copper Shortsword".into(),
        icon: Some("TAUGHT".into()),
        effect_item_type: [777, 0, 0],
        ..Default::default()
    };
    SpellCatalog::from_displays(HashMap::from([(100, wrapper), (200, taught)]))
}

/// The product item's template and `ItemDisplayInfo` row, landed.
fn landed_item(deps: &mut Deps) -> ItemDisplays {
    let mut t = crate::items::test_template("Copper Shortsword");
    t.display_info_id = 5;
    deps.items.insert_template(777, Some(t));
    ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::from([(
        5,
        ItemDisplay {
            icon: Some("ITEM".into()),
            ..Default::default()
        },
    )])))
}

fn icon_of(
    spells: &SpellCatalog,
    trainer_type: u32,
    wire_spell: u32,
    land: bool,
) -> Option<String> {
    let mut deps = Deps::new();
    let icons = land.then(|| landed_item(&mut deps));
    service_icon(
        wire_spell,
        trainer_type,
        spells,
        icons.as_ref(),
        &deps.items,
        &deps.commands,
    )
}

#[test]
fn trainer_icon_substitutes_the_product_only_at_a_tradeskill_trainer() {
    let spells = icon_catalog();

    // All three gates pass and the template landed: the created item's icon.
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 100, true).as_deref(),
        Some("ITEM")
    );

    // Gate 1 fails (a class trainer): the wire spell's own icon, never the taught spell's.
    assert_eq!(
        icon_of(&spells, 0, 100, true).as_deref(),
        Some("WRAPPER"),
        "a class trainer serves the wrapper's own icon, not the taught spell's"
    );

    // Gate 2 fails: 200 creates an item but has no learn effect, so its own icon.
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 200, true).as_deref(),
        Some("TAUGHT"),
        "200 IS the wire spell here, so its own icon is the right answer"
    );

    // A spell with no record: nil, where every failure ends (`0x4d911c`).
    assert_eq!(icon_of(&spells, TRAINER_TYPE_TRADESKILL, 999, true), None);
}

#[test]
fn trainer_icon_falls_back_when_the_taught_spell_makes_no_item() {
    let wrapper = SpellDisplay {
        icon: Some("WRAPPER".into()),
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        icon: Some("TAUGHT".into()),
        ..Default::default() // effect_item_type all zero
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(100, wrapper), (200, taught)]));
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 100, true).as_deref(),
        Some("WRAPPER")
    );
}

/// `LEARN_PET_SPELL` (57) counts like `LEARN_SPELL` (36), in any slot (`0x4d8ff5`/`0x4d8ffa`).
#[test]
fn trainer_icon_scans_every_effect_slot_for_either_learn_effect() {
    let wrapper = SpellDisplay {
        icon: Some("WRAPPER".into()),
        effects: [0, SPELL_EFFECT_LEARN_PET_SPELL, 0],
        effect_trigger_spell: [0, 200, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        effect_item_type: [777, 0, 0],
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(100, wrapper), (200, taught)]));
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 100, true).as_deref(),
        Some("ITEM"),
        "a pet-learn effect in slot 1 substitutes just the same"
    );
}

/// The client pushes nil here and repaints from the cache callback (`0x4d9140`).
#[test]
fn trainer_icon_is_nil_until_the_product_template_lands_and_asks_once() {
    let spells = icon_catalog();
    let deps = Deps::new();
    let icons = ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::new()));

    let first = service_icon(
        100,
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&icons),
        &deps.items,
        &deps.commands,
    );
    assert_eq!(first, None, "no template yet → nil, not the wrapper's icon");
    assert_eq!(
        deps.queried_entries(),
        vec![777],
        "and the created item's template was asked for"
    );

    let _ = service_icon(
        100,
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&icons),
        &deps.items,
        &deps.commands,
    );
    assert!(
        deps.queried_entries().is_empty(),
        "asked once, not per frame"
    );
}

/// On the shipped `Spell.dbc`: 2756 is the wrapper a Blacksmithing trainer sends, 2739 the recipe
/// it teaches, 2847 the sword it makes. Skips without client data.
#[test]
fn trainer_icon_on_real_data_reaches_for_the_crafted_item() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    // The two textures a wrong law produces.
    let wrapper_icon = spells.get(2756).unwrap().icon.clone();
    let recipe_icon = spells.get(2739).unwrap().icon.clone();
    assert_eq!(
        recipe_icon.as_deref(),
        Some("Interface\\Icons\\Spell_Shadow_SealOfKings"),
        "the blue crown on the row IS the recipe spell's own icon"
    );

    // A tradeskill trainer: gate 3 fires for item 2847 and the icon waits on its template.
    let deps = Deps::new();
    let icons = ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::new()));
    let icon = service_icon(
        2756,
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&icons),
        &deps.items,
        &deps.commands,
    );
    assert_eq!(
        icon, None,
        "waiting on the item template, not painting the crown"
    );
    assert_eq!(
        deps.queried_entries(),
        vec![2847],
        "the law reached for the crafted sword's template"
    );

    // A class trainer with the same wire spell: the wrapper's own icon, not the recipe's.
    let deps = Deps::new();
    let class_icon = service_icon(2756, 0, &spells, None, &deps.items, &deps.commands);
    assert_eq!(class_icon, wrapper_icon);
    assert_ne!(
        class_icon, recipe_icon,
        "the taught spell's icon is never painted at a trainer"
    );
}

/// On real data: the warrior wrapper 1605 is not in `SkillLineAbility`, so grouping hops to the
/// taught 78 (Arms, 26) while the buy id stays 1605. Skips without client data.
#[test]
fn resolve_hops_the_learn_wrapper_to_the_taught_ability() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");
    let skills = benilla_formats::load_skill_line_catalog(&mut chain).expect("load skill lines");

    let svc = resolve_service(
        &wire(1605, trainer_spell_state::GREEN, 10, 1, 0),
        0,
        &spells,
        Some(&skills),
        &BTreeSet::new(),
    );
    assert_eq!(svc.spell_id, 1605, "the buy id stays the wire wrapper");
    assert_eq!(
        svc.group_key, 26,
        "grouped under the taught ability's Arms line"
    );
    assert_eq!(svc.group_name, "Arms");
    assert_eq!(
        svc.name.as_deref(),
        Some("Heroic Strike"),
        "the WIRE spell's own name — which here happens to match the taught one"
    );
}

/// Only a profession-learn row tells the two apart: 2020 is "Apprentice Blacksmith" with no rank,
/// teaching 2018 "Blacksmithing"/"Apprentice". Skips without client data.
#[test]
fn display_name_is_the_wire_spell_not_the_taught_one() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");
    let skills = benilla_formats::load_skill_line_catalog(&mut chain).expect("load skill lines");

    // A Blacksmithing trainer's learn row as the wire carries it (reqLevel 5, no skill gate).
    let svc = resolve_service(
        &wire(2020, trainer_spell_state::GREEN, 9, 5, 0),
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&skills),
        &BTreeSet::new(),
    );
    assert_eq!(svc.name.as_deref(), Some("Apprentice Blacksmith"));
    assert_eq!(svc.subtext.as_deref(), None);
    assert_ne!(
        spells.get(2020).map(|d| d.name.as_str()),
        spells.get(2018).map(|d| d.name.as_str()),
        "the two names really do differ on the shipped data — the test is not vacuous"
    );

    // The group law puts it first: 2020 carries Effect 44 `SKILL_STEP`, a recipe wrapper does not.
    assert_eq!(svc.group_key, 1);
    assert_eq!(svc.group_name, "<STEP>");
    let recipe = resolve_service(
        &wire(2743, trainer_spell_state::RED, 50, 0, 164),
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&skills),
        &BTreeSet::new(),
    );
    assert_eq!(recipe.group_key, 2);
    assert_eq!(recipe.group_name, "<LEARN>");
    assert_eq!(recipe.name.as_deref(), Some("Copper Chain Pants"));
}

#[test]
fn category_maps_the_wire_state_byte() {
    assert_eq!(
        category(trainer_spell_state::GREEN),
        TrainerServiceCategory::Available
    );
    assert_eq!(
        category(trainer_spell_state::RED),
        TrainerServiceCategory::Unavailable
    );
    assert_eq!(
        category(trainer_spell_state::GRAY),
        TrainerServiceCategory::Used
    );
    // An unexpected value is gated, never learnable.
    assert_eq!(category(99), TrainerServiceCategory::Unavailable);
}

#[test]
fn resolve_reads_cost_state_and_gates_with_no_catalog() {
    // No catalog: names and icon nil, the wire fields intact, the skill name a fallback.
    let spells = empty_catalog();
    let mut w = wire(2018, trainer_spell_state::RED, 1000, 20, 164);
    w.req_spells = [78, 0, 0];
    let svc = resolve_service(&w, TRAINER_TYPE_TRADESKILL, &spells, None, &BTreeSet::new());
    assert_eq!(svc.spell_id, 2018);
    assert!(svc.name.is_none(), "no catalog → name in flight");
    assert_eq!(svc.cost, 1000);
    assert_eq!(svc.category, TrainerServiceCategory::Unavailable);
    assert_eq!(svc.level_req, 20);
    // The skill gate follows the category; the ability gate is unmet as the player lacks 78.
    assert_eq!(
        svc.skill_req,
        Some(TrainerSkillReq {
            name: "Skill 164".to_string(),
            rank: 100,
            met: false,
        })
    );
    assert_eq!(
        svc.ability_reqs,
        vec![TrainerAbilityReq {
            name: "Spell 78".to_string(),
            met: false,
        }]
    );
    assert!(svc.is_trade_skill, "trainer_type 2 → tradeskill");
    assert!(svc.prof_first_rank == w.is_primary_prof_first_rank);
    // No catalog at a tradeskill trainer: the learn group, as type 2 never drops a row.
    assert_eq!(svc.group_key, 2);
    assert_eq!(svc.group_name, "<LEARN>");
    // No skill gate when req_skill is 0.
    let plain = resolve_service(
        &wire(78, trainer_spell_state::GREEN, 50, 5, 0),
        0,
        &spells,
        None,
        &BTreeSet::new(),
    );
    assert_eq!(plain.skill_req, None);
    assert!(plain.ability_reqs.is_empty());
    assert!(!plain.is_trade_skill);
}

/// `GetTrainerServiceAbilityReq` (`0x4d96e0`): a level-gated service's known prerequisite still
/// reads met.
#[test]
fn ability_req_met_tracks_known_spells_not_the_service_category() {
    let spells = empty_catalog();
    let mut w = wire(845, trainer_spell_state::RED, 100, 20, 0); // unavailable (gated by level)
    w.req_spells = [78, 0, 0]; // requires Heroic Strike (78)

    let unknown = resolve_service(&w, 0, &spells, None, &BTreeSet::new());
    assert_eq!(unknown.category, TrainerServiceCategory::Unavailable);
    assert!(!unknown.ability_reqs[0].met, "prereq unknown → unmet");

    let known: BTreeSet<u32> = [78].into_iter().collect();
    let learned = resolve_service(&w, 0, &spells, None, &known);
    assert_eq!(learned.category, TrainerServiceCategory::Unavailable);
    assert!(
        learned.ability_reqs[0].met,
        "prereq known → met, decoupled from the unavailable service"
    );
}

/// On real data the prerequisite reads as the client's `"%s (%s)"`: 78 is Heroic Strike Rank 1.
/// Skips without client data.
#[test]
fn ability_req_shows_the_required_rank_on_real_data() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    let mut w = wire(846, trainer_spell_state::RED, 100, 20, 0);
    w.req_spells = [78, 0, 0]; // Heroic Strike Rank 1

    let known: BTreeSet<u32> = [78].into_iter().collect();
    let svc = resolve_service(&w, 0, &spells, None, &known);
    assert_eq!(
        svc.ability_reqs,
        vec![TrainerAbilityReq {
            name: "Heroic Strike (Rank 1)".to_string(),
            met: true,
        }],
        "the prereq shows its rank and reads met because the player knows it"
    );
    let svc = resolve_service(&w, 0, &spells, None, &BTreeSet::new());
    assert!(!svc.ability_reqs[0].met);
    assert_eq!(svc.ability_reqs[0].name, "Heroic Strike (Rank 1)");
}

/// `GetTrainerServiceDescription 0x4d9b40`: a class trainer's plain ability row expands the wire
/// spell's own `Spell.dbc` Description (`0x4d9c32`).
#[test]
fn service_description_expands_the_wire_spells_own_text() {
    let d = SpellDisplay {
        description: Some("Increases melee damage by $s1.".into()),
        // A flat 11: base points are stored one under the total, on one one-sided die.
        effect_base_points: [10, 0, 0],
        effect_base_dice: [1, 0, 0],
        effect_die_sides: [1, 0, 0],
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(78, d)]));
    let deps = Deps::new();
    with_text(&spells, 0, 0, None, |text| {
        assert_eq!(
            service_description(
                &wire(78, trainer_spell_state::GREEN, 10, 1, 0),
                0,
                &spells,
                &deps.items,
                &deps.commands,
                text,
            ),
            Some("Increases melee damage by 11.".into())
        );
    });
}

/// No text to show is `None`, the nil `0x4d9d90` pushes: the wire spell has no record
/// (`0x4d9bd7`), or no arm found text and its own is empty (`0x4d9d44`, `0x4d9d49`).
#[test]
fn service_description_is_none_without_a_record_or_text() {
    let empty = SpellDisplay {
        description: Some(String::new()),
        ..Default::default()
    };
    let bare = SpellDisplay::default();
    let spells = SpellCatalog::from_displays(HashMap::from([(1, empty), (2, bare)]));
    let deps = Deps::new();
    with_text(&spells, 0, 0, None, |text| {
        for spell in [1, 2, 3] {
            assert_eq!(
                service_description(
                    &wire(spell, trainer_spell_state::GREEN, 10, 1, 0),
                    0,
                    &spells,
                    &deps.items,
                    &deps.commands,
                    text,
                ),
                None,
                "spell {spell}"
            );
        }
    });
}

/// A learn wrapper has no text of its own, so the taught spell's Description is expanded
/// (`0x4d9c6f`); the description is the taught record's, not the wrapper's.
#[test]
fn service_description_hops_to_the_taught_spell() {
    let wrapper = SpellDisplay {
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        name: "Growl".into(),
        description: Some("Causes $s1 threat.".into()),
        effect_base_points: [9, 0, 0],
        effect_base_dice: [1, 0, 0],
        effect_die_sides: [1, 0, 0],
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(100, wrapper), (200, taught)]));
    let deps = Deps::new();
    with_text(&spells, 0, 0, None, |text| {
        assert_eq!(
            service_description(
                &wire(100, trainer_spell_state::GREEN, 10, 1, 0),
                0,
                &spells,
                &deps.items,
                &deps.commands,
                text,
            ),
            Some("Causes 10 threat.".into())
        );
    });
}

/// The third arm: a tradeskill recipe has no text of its own, so the created item's Description is
/// returned verbatim — never expanded (`0x4d9cd0`-`0x4d9d22`).
#[test]
fn service_description_takes_the_recipes_product_text_verbatim() {
    let wrapper = SpellDisplay {
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let recipe = SpellDisplay {
        attributes: SPELL_ATTR_IS_TRADESKILL,
        effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
        effect_item_type: [777, 0, 0],
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(100, wrapper), (200, recipe)]));
    let mut deps = Deps::new();
    let mut item = crate::items::test_template("Copper Shortsword");
    item.description = "Keeps $s1 for later.".into();
    deps.items.insert_template(777, Some(item));
    with_text(&spells, 0, 0, None, |text| {
        assert_eq!(
            service_description(
                &wire(100, trainer_spell_state::GREEN, 10, 1, 0),
                TRAINER_TYPE_TRADESKILL,
                &spells,
                &deps.items,
                &deps.commands,
                text,
            ),
            Some("Keeps $s1 for later.".into()),
            "the product's text is the item's own, tokens and all"
        );
    });
}

/// A used row at a mount trainer skips its own text (`0x4d9c29`); the same row at a class trainer
/// shows it.
#[test]
fn service_description_skips_the_wire_text_for_a_used_mount_row() {
    let d = SpellDisplay {
        description: Some("Summons a mount.".into()),
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        description: Some("Riding skill.".into()),
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(100, d), (200, taught)]));
    let deps = Deps::new();
    let description = |trainer_type, state| {
        with_text(&spells, 0, 0, None, |text| {
            service_description(
                &wire(100, state, 10, 1, 0),
                trainer_type,
                &spells,
                &deps.items,
                &deps.commands,
                text,
            )
        })
    };
    assert_eq!(
        description(0, trainer_spell_state::GREEN),
        Some("Summons a mount.".into())
    );
    assert_eq!(
        description(TRAINER_TYPE_MOUNT, trainer_spell_state::GRAY),
        Some("Riding skill.".into())
    );
}

/// The caster selector: a `LEARN_PET_SPELL` wrapper expands against the pet (`0x6e3130`'s nonzero
/// selector), the same taught spell under a plain `LEARN_SPELL` against the player.
#[test]
fn service_description_expands_against_the_pet_for_a_learn_pet_row() {
    // One point per level over `baseLevel` 1: level 0 reads 10, a level-2 pet reads 11.
    let taught = SpellDisplay {
        description: Some("Causes $s1 threat.".into()),
        base_level: 1,
        effect_base_points: [9, 0, 0],
        effect_base_dice: [1, 0, 0],
        effect_die_sides: [1, 0, 0],
        effect_dice_per_level: [1, 0, 0],
        ..Default::default()
    };
    let pet_wrapper = SpellDisplay {
        effects: [SPELL_EFFECT_LEARN_PET_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let plain_wrapper = SpellDisplay {
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([
        (100, pet_wrapper),
        (101, plain_wrapper),
        (200, taught),
    ]));
    let deps = Deps::new();
    // A level-2 pet (skill 10) against a player with no skill in the taught spell's line at all.
    with_text(&spells, 0, 10, None, |text| {
        assert_eq!(
            service_description(
                &wire(100, trainer_spell_state::GREEN, 10, 1, 0),
                0,
                &spells,
                &deps.items,
                &deps.commands,
                text,
            ),
            Some("Causes 11 threat.".into()),
            "the pet's level drives the taught spell's points"
        );
        assert_eq!(
            service_description(
                &wire(101, trainer_spell_state::GREEN, 10, 1, 0),
                0,
                &spells,
                &deps.items,
                &deps.commands,
                text,
            ),
            Some("Causes 10 threat.".into()),
            "the same taught spell under LEARN_SPELL reads the player's"
        );
    });
}

/// The `$z` token: the bind point's area name (`AreaTable.dbc`), and raw where no bind point has
/// arrived, as the item and tooltip feeds' `$z` behaves.
#[test]
fn service_description_expands_z_to_the_bind_area() {
    let d = SpellDisplay {
        description: Some("Returns you to $z.".into()),
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(556, d)]));
    let deps = Deps::new();
    for (home, want) in [
        (Some("Razor Hill"), "Returns you to Razor Hill."),
        (None, "Returns you to $z."),
    ] {
        with_text(&spells, 0, 0, home, |text| {
            assert_eq!(
                service_description(
                    &wire(556, trainer_spell_state::GREEN, 4000, 30, 0),
                    0,
                    &spells,
                    &deps.items,
                    &deps.commands,
                    text,
                ),
                Some(want.to_string())
            );
        });
    }
}

/// On the shipped `Spell.dbc`: the warrior wrapper 1605 teaches 78 Heroic Strike, so the row's
/// description is the taught ability's text, not the wrapper's empty one. Skips without client
/// data.
#[test]
fn service_description_on_real_data_expands_the_taught_ability() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    let svc = resolve_service(
        &wire(1605, trainer_spell_state::GREEN, 10, 1, 0),
        0,
        &spells,
        None,
        &BTreeSet::new(),
    );
    assert!(
        svc.description
            .as_deref()
            .unwrap_or_default()
            .starts_with("A strong attack that increases melee damage by "),
        "the taught 78's description, not the wrapper's empty one: {:?}",
        svc.description
    );
    assert!(
        !svc.description.as_deref().unwrap_or_default().contains('$'),
        "every token resolved: {:?}",
        svc.description
    );
}

/// On the shipped `Spell.dbc`: 1352 is the shaman trainer's Astral Recall wrapper, teaching 556,
/// whose text is the one trainer-taught description carrying `$z`. Skips without client data.
#[test]
fn service_description_on_real_data_reads_astral_recalls_bind_area() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    let deps = Deps::new();
    let svc = with_text(&spells, 0, 0, Some("Razor Hill"), |text| {
        super::resolve_service(
            &wire(1352, trainer_spell_state::GREEN, 4000, 30, 0),
            0,
            &spells,
            None,
            &BTreeSet::new(),
            None,
            &deps.items,
            &deps.commands,
            text,
            &probe_strings,
        )
    });
    assert_eq!(
        svc.description.as_deref(),
        Some(
            "Yanks the caster through the twisting nether back to Razor Hill.  Speak to an \
             Innkeeper in a different place to change your home location."
        )
    );
}

/// On the shipped `Spell.dbc`: 2020 is the "Apprentice Blacksmith" wrapper a Blacksmithing trainer
/// lists, teaching 2018 "Blacksmithing" — the profession-learn row whose text is the taught
/// profession's. Skips without client data.
#[test]
fn service_description_on_real_data_reads_the_profession_learn_row() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    // The wrapper carries no text of its own, so the taught profession's is the only one there is.
    assert_eq!(
        spells.get(2020).and_then(|d| d.description.as_deref()),
        None
    );
    let svc = resolve_service(
        &wire(2020, trainer_spell_state::GREEN, 9, 5, 0),
        TRAINER_TYPE_TRADESKILL,
        &spells,
        None,
        &BTreeSet::new(),
    );
    assert_eq!(
        svc.description.as_deref(),
        Some(
            "Allows a Blacksmith to make basic weapons and armor up to a maximum potential skill \
             of 75.  Requires stone and metal found with the mining skill."
        )
    );
}

/// On the shipped `Spell.dbc`: 7820 is the recipe wrapper a Blacksmithing trainer lists, 7818 the
/// recipe it teaches and 6338 the Silver Rod it makes, so the row's text is that item's
/// description, read out of the item cache (`0x4d9cd0`-`0x4d9d22`). The product's record arrives
/// with the item query, so the template seeded below, with vmangos's text, stands in for it.
/// Skips without client data.
#[test]
fn service_description_on_real_data_reaches_for_the_recipes_product() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    // Neither the wrapper nor the recipe carries text of its own, so the item is the only source.
    assert_eq!(
        spells.get(7820).and_then(|d| d.description.as_deref()),
        None
    );
    assert_eq!(
        spells.get(7818).and_then(|d| d.description.as_deref()),
        None
    );

    // The law alone, never through `resolve_service`: the icon law asks for the same template
    // first, and one ask per pending entry reaches the cache.
    let describe = |deps: &Deps| {
        with_text(&spells, 0, 0, None, |text| {
            service_description(
                &wire(7820, trainer_spell_state::GREEN, 90, 0, 164),
                TRAINER_TYPE_TRADESKILL,
                &spells,
                &deps.items,
                &deps.commands,
                text,
            )
        })
    };

    let deps = Deps::new();
    assert_eq!(
        describe(&deps),
        None,
        "nothing to show until the product's template lands"
    );
    assert_eq!(
        deps.queried_entries(),
        vec![6338],
        "the description law reached for the crafted rod's template"
    );

    // With the product's record present, its description is the row's text, verbatim.
    let mut product = crate::items::test_template("Silver Rod");
    product.description = "Needed by Enchanters.".into();
    let landed = {
        let mut deps = Deps::new();
        deps.items.insert_template(6338, Some(product));
        deps
    };
    assert_eq!(describe(&landed), Some("Needed by Enchanters.".into()));
}

#[test]
fn snapshot_is_none_when_closed_and_lists_services_when_open() {
    let spells = empty_catalog();
    let mut open = TrainerOpen::default();
    assert!(snap(&open, &spells).is_none());

    open.open(
        0x42,
        0,
        vec![
            wire(78, trainer_spell_state::GREEN, 100, 10, 0),
            wire(79, trainer_spell_state::GRAY, 200, 0, 0),
        ],
        "Learn from me.".into(),
    );
    let state = snap(&open, &spells).expect("open → Some");
    assert_eq!(state.greeting, "Learn from me.");
    assert_eq!(state.services.len(), 2);
    assert_eq!(
        state.services[0].category,
        TrainerServiceCategory::Available
    );
    assert_eq!(state.services[1].category, TrainerServiceCategory::Used);

    open.clear();
    assert!(snap(&open, &spells).is_none());
    assert_eq!(open.trainer, None);
}
/// A wrapper spell whose slot `slot` carries `effect` and triggers `trigger`.
fn wrapper(effect: u32, slot: usize, trigger: u32) -> SpellDisplay {
    let mut d = SpellDisplay {
        name: "Learn Something".into(),
        icon: Some("WRAPPER".into()),
        ..Default::default()
    };
    d.effects[slot] = effect;
    d.effect_trigger_spell[slot] = trigger;
    d
}

/// A plain taught ability: no tradeskill bit, so the spell route.
fn taught_ability() -> SpellDisplay {
    SpellDisplay {
        name: "Heroic Strike".into(),
        ..Default::default()
    }
}

/// A taught recipe: the tradeskill bit, `Effect[0] == CREATE_ITEM`, and a distinct product in
/// every slot so a wrong-slot read is named by the assertion.
fn taught_recipe() -> SpellDisplay {
    SpellDisplay {
        name: "Copper Shortsword".into(),
        attributes: SPELL_ATTR_IS_TRADESKILL,
        effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
        effect_item_type: [2847, 3333, 4444],
        ..Default::default()
    }
}

/// A taught spell with the tradeskill bit shows its created item's tooltip: 2756 → 2739 → 2847.
#[test]
fn tooltip_takes_the_item_arm_when_the_taught_spell_is_a_recipe() {
    let spells = SpellCatalog::from_displays(HashMap::from([
        (2756, wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 2739)),
        (2739, taught_recipe()),
    ]));
    assert_eq!(service_tooltip(2756, &spells), TrainerTooltip::Item(2847));
}

#[test]
fn tooltip_hops_to_the_taught_spell_where_the_icon_pins_the_wire() {
    let spells = SpellCatalog::from_displays(HashMap::from([
        (100, wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 200)),
        (200, taught_ability()),
    ]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Spell {
            spell_id: 200,
            alt_caster: false,
        },
    );
    // The icon law on the same row paints the wire spell's art.
    let deps = Deps::new();
    assert_eq!(
        super::service_icon(100, 0, &spells, None, &deps.items, &deps.commands),
        Some("WRAPPER".into()),
    );
}

/// `altCaster` gates the builder's totem and reagent blocks.
#[test]
fn tooltip_sets_alt_caster_only_for_a_pet_learn_wrapper() {
    let spells = SpellCatalog::from_displays(HashMap::from([
        (100, wrapper(SPELL_EFFECT_LEARN_PET_SPELL, 0, 200)),
        (200, taught_ability()),
    ]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Spell {
            spell_id: 200,
            alt_caster: true,
        },
    );
}

/// Slot 0's trigger is not in the catalog, so slot 1 wins.
#[test]
fn tooltip_scan_advances_past_an_unresolvable_trigger() {
    let mut w = wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 999_999);
    w.effects[1] = SPELL_EFFECT_LEARN_SPELL;
    w.effect_trigger_spell[1] = 200;
    let spells = SpellCatalog::from_displays(HashMap::from([(100, w), (200, taught_ability())]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Spell {
            spell_id: 200,
            alt_caster: false,
        },
    );
}

/// `Attributes & 0x20` alone picks the item arm (`0x52e610`'s redirect at `0x52e6d2`);
/// `Effect[0] == 24` picks the matched slot, otherwise slot 0.
#[test]
fn tooltip_item_slot_follows_the_effect_gate_not_the_attribute_gate() {
    // The learn effect in slot 1, slot 0 empty. Built per use: SpellDisplay is not Clone.
    let slot1_wrapper = || {
        let mut w = wrapper(SPELL_EFFECT_LEARN_SPELL, 1, 200);
        w.effects[0] = 0;
        w
    };
    let spells = SpellCatalog::from_displays(HashMap::from([
        (100, slot1_wrapper()),
        (200, taught_recipe()),
    ]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Item(3333),
        "Effect[0]==CREATE_ITEM -> EffectItemType[matched slot]"
    );

    // The bit without Effect[0]==24: still the item arm, off slot 0.
    let mut odd = taught_recipe();
    odd.effects[0] = 6;
    let spells = SpellCatalog::from_displays(HashMap::from([(100, slot1_wrapper()), (200, odd)]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Item(2847),
        "the bit alone still routes to the item builder, from slot 0"
    );
}

/// No learn effect, or no `Spell.dbc`: the wire spell.
#[test]
fn tooltip_falls_back_to_the_wire_spell() {
    let spells = SpellCatalog::from_displays(HashMap::from([(100, taught_ability())]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Spell {
            spell_id: 100,
            alt_caster: false,
        },
    );
    assert_eq!(
        service_tooltip(100, &empty_catalog()),
        TrainerTooltip::Spell {
            spell_id: 100,
            alt_caster: false,
        },
    );
}

/// The builder resets per list packet (`0x4d75d9`), and an open window repaints through the
/// re-evaluator (`0x4d7d40`), never a second list, so every list opens and resets.
#[test]
fn every_list_packet_begins_a_window_session() {
    const TRAINER: u64 = 0xabc;
    let list = || vec![wire(1, trainer_spell_state::GREEN, 10, 1, 0)];
    let mut open = TrainerOpen::default();

    open.open(TRAINER, 0, list(), "Greetings".into());
    assert!(open.fresh_list, "a first list opens the window: reset");
    open.fresh_list = false; // the feed consumes it

    open.open(TRAINER, 0, list(), "Greetings".into());
    assert!(
        open.fresh_list,
        "the same trainer's list again is a session start again"
    );

    // A pending re-derivation never survives a list: the packet carries the server's own states.
    open.fresh_list = false;
    open.trigger_re_derive();
    assert!(open.re_derive);
    open.open(TRAINER, 0, list(), "Greetings".into());
    assert!(
        !open.re_derive,
        "a fresh list is the server's states — nothing pending against it"
    );

    // A trigger with no window open does nothing (`0x4d7d46`'s early-out).
    open.clear();
    open.trigger_re_derive();
    assert!(!open.re_derive);
}

mod re_derive {
    //! The re-evaluator, one test per leg of `0x4d7d40`, on synthetic catalogs.
    use super::super::reeval::{re_derive, PetView, PlayerView, SkillSlot};
    use super::*;
    use benilla_formats::{LearnEffect, SlaInfo, SpellDisplay};
    use std::collections::HashMap;

    const BLACKSMITHING: u32 = 164;
    /// Wrapper 2020 teaches 2018 and steps Blacksmithing to 1, the real openers' shape.
    const OPENER: u32 = 2020;
    const TAUGHT: u32 = 2018;
    /// A class ability's rank-1 wrapper: teaches 100, whose next rank (`forward_spellid`) is 101.
    const RANK1_WRAPPER: u32 = 500;
    const RANK1: u32 = 100;
    const RANK2: u32 = 101;
    /// A pet wrapper: teaches the pet 300.
    const PET_WRAPPER: u32 = 700;
    const PET_SPELL: u32 = 300;

    /// A rank-2 wrapper: teaches RANK2 (whose previous rank is RANK1).
    const RANK2_WRAPPER: u32 = 501;

    fn catalog() -> SpellCatalog {
        // Every wrapper has a record: the admission gate (`0x4d7dcd`) skips a row without one.
        let displays = [OPENER, RANK1_WRAPPER, RANK2_WRAPPER, PET_WRAPPER]
            .into_iter()
            .map(|id| (id, SpellDisplay::default()))
            .collect();
        SpellCatalog::from_displays_and_effects(
            displays,
            HashMap::from([
                (
                    OPENER,
                    vec![
                        LearnEffect::Spell(TAUGHT),
                        LearnEffect::SkillStep {
                            skill: BLACKSMITHING,
                            step: 1,
                        },
                    ],
                ),
                (RANK1_WRAPPER, vec![LearnEffect::Spell(RANK1)]),
                (RANK2_WRAPPER, vec![LearnEffect::Spell(RANK2)]),
                (PET_WRAPPER, vec![LearnEffect::PetSpell(PET_SPELL)]),
            ]),
        )
    }

    fn skill_lines() -> SkillLineCatalog {
        let row = |skill_id, forward| SlaInfo {
            skill_id,
            req_skill_value: 0,
            forward_spell_id: forward,
            trivial_low: 0,
            trivial_high: 0,
        };
        SkillLineCatalog::from_abilities([(RANK1, row(1, RANK2)), (RANK2, row(1, 0))])
    }

    fn service(spell: u32) -> TrainerSpell {
        // The wire state is RED on purpose: every case below must ignore it (`0x4d7dec`).
        let mut w = wire(spell, trainer_spell_state::RED, 10, 1, 0);
        w.req_skill_value = 0;
        w
    }

    fn player() -> PlayerView {
        PlayerView {
            known: BTreeSet::new(),
            level: 10,
            skills: Vec::new(),
            pet: None,
        }
    }

    fn state(wire: &TrainerSpell, trainer_type: u32, player: &PlayerView) -> u8 {
        re_derive(wire, trainer_type, &catalog(), &skill_lines(), player)
    }

    #[test]
    fn the_wire_state_is_discarded_and_a_met_service_reads_green() {
        assert_eq!(
            state(&service(RANK1_WRAPPER), 0, &player()),
            trainer_spell_state::GREEN
        );
    }

    #[test]
    fn every_learn_effect_known_reads_gray() {
        let mut p = player();
        p.known.insert(RANK1);
        assert_eq!(
            state(&service(RANK1_WRAPPER), 0, &p),
            trainer_spell_state::GRAY
        );
    }

    #[test]
    fn a_higher_known_rank_counts_as_known() {
        // Rank 2 in the book, rank 1 offered: `KnownHigherRank` (0x60c8d0) says known.
        let mut p = player();
        p.known.insert(RANK2);
        assert_eq!(
            state(&service(RANK1_WRAPPER), 0, &p),
            trainer_spell_state::GRAY
        );
    }

    #[test]
    fn a_skill_step_the_player_already_holds_reads_gray() {
        let mut p = player();
        p.skills.push(SkillSlot {
            skill_id: BLACKSMITHING,
            step: 1,
            value_plus_perm: 1,
        });
        assert_eq!(state(&service(OPENER), 0, &p), trainer_spell_state::GRAY);
    }

    #[test]
    fn a_skill_line_below_the_requirement_reads_red() {
        // A recipe-shaped service: a plain learn wrapper with a skill requirement.
        let mut w = service(RANK1_WRAPPER);
        w.req_skill = BLACKSMITHING;
        w.req_skill_value = 75;
        let mut p = player();
        p.skills.push(SkillSlot {
            skill_id: BLACKSMITHING,
            step: 1,
            value_plus_perm: 40,
        });
        assert_eq!(state(&w, 0, &p), trainer_spell_state::RED);
        // Met (value plus permanent bonus reaches it): the leg leaves the state alone.
        p.skills[0].value_plus_perm = 75;
        assert_eq!(state(&w, 0, &p), trainer_spell_state::GREEN);
    }

    /// The reference's own quirk (`0x4d8082`): a line the player lacks entirely writes nothing,
    /// so green where the server sends red.
    #[test]
    fn a_skill_line_the_player_lacks_entirely_reads_green() {
        let mut w = service(RANK1_WRAPPER);
        w.req_skill = BLACKSMITHING;
        w.req_skill_value = 75;
        assert_eq!(state(&w, 0, &player()), trainer_spell_state::GREEN);
    }

    #[test]
    fn an_unknown_required_ability_reads_red_or_hidden_at_type_one() {
        // The opener (its taught spell unknown, no skill slot held) with a prerequisite ability.
        let mut w = service(OPENER);
        w.req_spells = [RANK1, 0, 0];
        assert_eq!(state(&w, 0, &player()), trainer_spell_state::RED);
        assert_eq!(state(&w, 2, &player()), trainer_spell_state::RED);
        // Type 1: RANK1 is not the rank before TAUGHT, so a plain miss, not hidden (`0x4d83a4`).
        assert_eq!(state(&w, 1, &player()), trainer_spell_state::RED);
        // Known, or known at a higher rank, satisfies it.
        let mut p = player();
        p.known.insert(RANK2);
        assert_eq!(state(&w, 0, &p), trainer_spell_state::GREEN);
        let mut p = player();
        p.known.insert(RANK1);
        assert_eq!(state(&w, 1, &p), trainer_spell_state::GREEN);
    }

    #[test]
    fn a_level_requirement_above_one_gates_on_the_players_level() {
        let mut w = service(RANK1_WRAPPER);
        w.req_level = 20;
        assert_eq!(state(&w, 0, &player()), trainer_spell_state::RED);
        let mut p = player();
        p.level = 20;
        assert_eq!(state(&w, 0, &p), trainer_spell_state::GREEN);
        // `req_level <= 1` is not a gate at all (`row[+0x14] > 1`).
        w.req_level = 1;
        let mut p = player();
        p.level = 0;
        assert_eq!(state(&w, 0, &p), trainer_spell_state::GREEN);
    }

    #[test]
    fn a_pet_spell_needs_a_pet_of_the_rows_level_and_reads_off_the_pets_book() {
        let mut w = service(PET_WRAPPER);
        w.req_level = 12;
        // The row's level binds the player too (`0x4d8239`), so the player clears it.
        let player = || PlayerView {
            level: 20,
            ..player()
        };
        assert_eq!(state(&w, 3, &player()), trainer_spell_state::RED, "no pet");
        let mut p = player();
        p.pet = Some(PetView {
            level: 11,
            known: BTreeSet::new(),
        });
        assert_eq!(
            state(&w, 3, &p),
            trainer_spell_state::RED,
            "pet below the row's level"
        );
        p.pet = Some(PetView {
            level: 12,
            known: BTreeSet::new(),
        });
        assert_eq!(state(&w, 3, &p), trainer_spell_state::GREEN);
        // Known by the pet, not the player: gray.
        p.pet = Some(PetView {
            level: 12,
            known: BTreeSet::from([PET_SPELL]),
        });
        assert_eq!(state(&w, 3, &p), trainer_spell_state::GRAY);
        p.known.insert(PET_SPELL);
        p.pet = Some(PetView {
            level: 12,
            known: BTreeSet::new(),
        });
        assert_eq!(
            state(&w, 3, &p),
            trainer_spell_state::GREEN,
            "the player's own book is not the pet's"
        );
    }

    #[test]
    fn the_legs_run_in_the_references_order() {
        // A known service with an unmet skill line and level is gray: legs 4 and 5 run only while
        // the state is still 0.
        let mut w = service(OPENER);
        w.req_skill = BLACKSMITHING;
        w.req_skill_value = 300;
        w.req_level = 60;
        let mut p = player();
        p.known.insert(TAUGHT);
        p.skills.push(SkillSlot {
            skill_id: BLACKSMITHING,
            step: 1,
            value_plus_perm: 1,
        });
        assert_eq!(state(&w, 2, &p), trainer_spell_state::GRAY);
    }

    /// The admission gate (`0x4d7dcd`).
    #[test]
    fn a_row_without_a_spell_record_keeps_its_wire_state() {
        let w = wire(999_999, trainer_spell_state::RED, 10, 1, 0);
        assert_eq!(state(&w, 0, &player()), trainer_spell_state::RED);
        let w = wire(999_999, trainer_spell_state::GRAY, 10, 1, 0);
        assert_eq!(state(&w, 0, &player()), trainer_spell_state::GRAY);
    }

    /// `0x4d7e3e`: the hidden row counts as a learn effect, never a known one, so it stays hidden.
    #[test]
    fn a_higher_rank_known_at_a_type_one_trainer_hides_the_row() {
        let mut p = player();
        p.known.insert(RANK2);
        assert_eq!(state(&service(RANK1_WRAPPER), 1, &p), 3);
        assert_eq!(
            state(&service(RANK1_WRAPPER), 0, &p),
            trainer_spell_state::GRAY
        );
        // Knowing exactly the taught rank is not "higher": gray at every type.
        let mut p = player();
        p.known.insert(RANK1);
        assert_eq!(
            state(&service(RANK1_WRAPPER), 1, &p),
            trainer_spell_state::GRAY
        );
    }

    /// `0x4d83a4`: hidden only when the missing ability is the rank before what the row teaches;
    /// any other miss, and every miss at another type, is unavailable.
    #[test]
    fn a_missing_previous_rank_at_a_type_one_trainer_hides_the_row() {
        let mut w = service(RANK2_WRAPPER);
        w.req_spells = [RANK1, 0, 0];
        assert_eq!(
            state(&w, 1, &player()),
            3,
            "the missing ability is the previous rank"
        );
        assert_eq!(state(&w, 0, &player()), trainer_spell_state::RED);
        let mut w = service(RANK2_WRAPPER);
        w.req_spells = [RANK1 + 500, 0, 0];
        assert_eq!(
            state(&w, 1, &player()),
            trainer_spell_state::RED,
            "an unrelated ability"
        );
    }

    /// A missing or under-level pet leaves the loop (`0x4d803c`/`0x4d7fac`), and a pet that knows
    /// the spell is never level-tested.
    #[test]
    fn the_pet_legs_leave_the_loop_and_a_knowing_pet_skips_the_level_test() {
        let spells = SpellCatalog::from_displays_and_effects(
            HashMap::from([(PET_WRAPPER, SpellDisplay::default())]),
            HashMap::from([(
                PET_WRAPPER,
                vec![
                    LearnEffect::PetSpell(PET_SPELL),
                    LearnEffect::SkillStep {
                        skill: BLACKSMITHING,
                        step: 1,
                    },
                ],
            )]),
        );
        let mut w = service(PET_WRAPPER);
        w.req_level = 12;
        let mut p = player();
        p.level = 20;
        p.skills.push(SkillSlot {
            skill_id: BLACKSMITHING,
            step: 1,
            value_plus_perm: 1,
        });
        // No pet: the step effect after it is never reached, so the row stays unavailable.
        assert_eq!(
            re_derive(&w, 3, &spells, &skill_lines(), &p),
            trainer_spell_state::RED
        );
        // A pet that knows the spell, below the row's level: known, not level-tested.
        p.pet = Some(PetView {
            level: 5,
            known: BTreeSet::from([PET_SPELL]),
        });
        assert_eq!(
            re_derive(&w, 3, &spells, &skill_lines(), &p),
            trainer_spell_state::GRAY
        );
    }

    #[test]
    fn a_pet_rows_required_abilities_are_the_pets_and_never_hide() {
        let mut w = service(PET_WRAPPER);
        w.req_spells = [RANK1, 0, 0];
        let mut p = player();
        p.level = 20;
        p.known.insert(RANK1); // the player knows it, which a pet row ignores
        p.pet = Some(PetView {
            level: 20,
            known: BTreeSet::new(),
        });
        assert_eq!(state(&w, 3, &p), trainer_spell_state::RED);
        assert_eq!(
            state(&w, 1, &p),
            trainer_spell_state::RED,
            "no hidden state on the pet path"
        );
        p.pet = Some(PetView {
            level: 20,
            known: BTreeSet::from([RANK2]),
        });
        assert_eq!(
            state(&w, 3, &p),
            trainer_spell_state::GREEN,
            "the pet knows a higher rank"
        );
    }

    #[test]
    fn re_derive_all_rewrites_every_row_in_place() {
        let mut list = vec![service(RANK1_WRAPPER), service(OPENER)];
        let mut p = player();
        p.known.insert(RANK1);
        super::super::reeval::re_derive_all(&mut list, 0, &catalog(), &skill_lines(), &p);
        assert_eq!(list[0].state, trainer_spell_state::GRAY);
        assert_eq!(list[1].state, trainer_spell_state::GREEN);
    }
}

/// A mistyped key yields an empty header, which [`probe_strings`] cannot catch. Skips without
/// client data.
#[test]
fn the_group_header_keys_resolve_in_the_real_global_strings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let vm = benilla_ui::script::UiScript::new().expect("VM");
    vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
    for key in [
        "TRADESKILL_SERVICE_STEP",
        "TRADESKILL_SERVICE_LEARN",
        "KNOWN_TALENTS_HEADER",
    ] {
        let text = vm.lua().globals().get::<String>(key).unwrap_or_default();
        assert!(!text.is_empty(), "{key} missing");
    }
}
