//! The trainer window's tests: the laws, service resolution, the snapshot, the re-evaluator and the lane.

use super::*;
use crate::names::NameCache;
use benilla_formats::{
    ItemDisplay, ItemDisplayCatalog, SpellDisplay, TokenContext, SPELL_ATTR_IS_TRADESKILL,
    SPELL_EFFECT_CREATE_ITEM, SPELL_EFFECT_LEARN_PET_SPELL, SPELL_EFFECT_LEARN_SPELL,
    SPELL_EFFECT_SKILL_STEP,
};
use benilla_protocol::messages::trainer_spell_state;
use benilla_ui::script::{TrainerServiceCategory, TrainerTooltip};
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
        "INCREASE_POTENTIAL" => Some("Increases potential in %s by %d".into()),
        "HOME_INN" => Some("<INN>".into()),
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
    let skill_of = |d: &benilla_formats::SpellDisplay| d.skill_level(player_skill);
    let pet_of = |d: &benilla_formats::SpellDisplay| d.skill_level(pet_skill);
    let ctx = TokenContext {
        durations: &durations,
        radii: &radii,
        ranges: None,
        level: &skill_of,
        lookup: &lookup,
        mods: None,
        power_cost: &|_| -1,
        power_per_second: &|_| 0,
        unmodified_points: false,
        gender: &|| 0,
        home_area: &|| home,
        global: &probe_strings,
        printf: &crate::ui_script::token_printf,
    };
    let pet_ctx = TokenContext {
        level: &pet_of,
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
    resolve_service_skilled(wire, trainer_type, spells, skill_lines, known, Some(&[]))
}

/// [`resolve_service`] with the player's skill slots, `None` for no player object.
fn resolve_service_skilled(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    known: &BTreeSet<u32>,
    skills: Option<&[benilla_protocol::messages::PlayerSkillSlot]>,
) -> TrainerService {
    let deps = Deps::new();
    with_text(spells, 0, 0, None, |text| {
        super::resolve_service(
            wire,
            trainer_type,
            spells,
            skill_lines,
            &ReqBooks {
                player: known,
                pet: None,
                race_class: (1, 1),
                skills,
            },
            None,
            &deps.items,
            Ask::New(&deps.commands),
            text,
            benilla_ui::strings::Strings::Host(&probe_strings),
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
            &ReqBooks {
                player: &BTreeSet::new(),
                pet: None,
                race_class: (1, 1),
                skills: Some(&[]),
            },
            None,
            &deps.items,
            Ask::New(&deps.commands),
            text,
            benilla_ui::strings::Strings::Host(&probe_strings),
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
        Ask::New(&deps.commands),
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

/// A skill-step wire spell is flagged for `IsTrainerServiceSkillStep` (`0x4d9dd0`, effect 44 in
/// any slot) and carries `GetTrainerServiceStepIncrease`'s text (`0x4da030`): `INCREASE_POTENTIAL`
/// over the first skill-step effect's line name and five times its step. A spell that lacks the
/// effect, a step of 0 and a line with no row carry none; a service's group line name is the
/// `SkillLine.dbc` row of its key.
#[test]
fn a_skill_step_service_carries_its_flag_its_increase_text_and_its_line_name() {
    let step_spell = || SpellDisplay {
        name: "Journeyman".into(),
        effects: [0, SPELL_EFFECT_SKILL_STEP, 0],
        ..Default::default()
    };
    let plain = SpellDisplay {
        name: "Heroic Strike".into(),
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays_and_effects(
        HashMap::from([
            (7000, step_spell()),
            (7001, step_spell()),
            (7002, step_spell()),
            (78, plain),
        ]),
        HashMap::from([
            (
                7000,
                vec![LearnEffect::SkillStep {
                    skill: 164,
                    step: 150,
                }],
            ),
            (
                7001,
                vec![LearnEffect::SkillStep {
                    skill: 999,
                    step: 150,
                }],
            ),
            (
                7002,
                vec![LearnEffect::SkillStep {
                    skill: 164,
                    step: 0,
                }],
            ),
        ]),
    );
    let lines = SkillLineCatalog::from_spell_lines([(78, 26)])
        .with_line_names([(164, "Blacksmithing"), (26, "Arms")]);
    let make = |spell: u32, trainer_type: u32| {
        resolve_service(
            &wire(spell, trainer_state(), 100, 1, 0),
            trainer_type,
            &spells,
            Some(&lines),
            &BTreeSet::new(),
        )
    };
    fn trainer_state() -> u8 {
        trainer_spell_state::GREEN
    }
    let step = make(7000, TRAINER_TYPE_TRADESKILL);
    assert!(step.skill_step);
    assert_eq!(
        step.step_increase.as_deref(),
        Some("Increases potential in Blacksmithing by 750")
    );
    // A tradeskill trainer keys its tree 1 or 2, which `SkillLine.dbc` lacks here.
    assert_eq!(step.line_name, None);
    // The line has no row, or the step is 0: no text, but still a step row.
    for spell in [7001, 7002] {
        let s = make(spell, TRAINER_TYPE_TRADESKILL);
        assert!(s.skill_step, "{spell}");
        assert_eq!(s.step_increase, None, "{spell}");
    }
    // A plain ability: neither, and its group's line name is the `SkillLine.dbc` row of its key.
    let plain = make(78, 0);
    assert!(!plain.skill_step);
    assert_eq!(plain.step_increase, None);
    assert_eq!(plain.group_key, 26);
    assert_eq!(plain.line_name.as_deref(), Some("Arms"));
    // A spell with no record is no step.
    assert!(!make(31337, 0).skill_step);
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
        Ask::New(&deps.commands),
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
        Ask::New(&deps.commands),
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
        Ask::New(&deps.commands),
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
    let class_icon = service_icon(
        2756,
        0,
        &spells,
        None,
        &deps.items,
        Ask::New(&deps.commands),
    );
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
    assert_eq!(
        svc.subtext.as_deref(),
        Some(""),
        "a rankless spell's NameSubtext is \"\" (0x4d8b84)"
    );
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
    // The re-evaluator's 3 and any other byte name a bit no filter sets (`0x4d84ba`, `0x4da390`).
    assert_eq!(category(3), TrainerServiceCategory::Hidden);
    assert_eq!(category(10), TrainerServiceCategory::Hidden);
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
    // A skill line with no `SkillLine.dbc` row is no requirement (`0x4d94c5` → `0x4d95bc`); the
    // ability gate reads met, as no service spell record reaches the known test (`0x4d9842`), and
    // a requirement with no record answers nil (`0x4d97c8`, `0x4d98d1`). Every slot stays in place
    // (`0x4d97a5`).
    assert_eq!(svc.skill_req, None);
    assert_eq!(
        svc.ability_reqs,
        vec![
            TrainerAbilityReq {
                spell_id: 78,
                name: None,
                met: true,
            },
            TrainerAbilityReq {
                spell_id: 0,
                name: None,
                met: true,
            },
            TrainerAbilityReq {
                spell_id: 0,
                name: None,
                met: true,
            },
        ]
    );
    assert!(!svc.is_trade_skill, "no record: nil (`0x4d9fcf`)");
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
    assert!(plain
        .ability_reqs
        .iter()
        .all(|r| r.spell_id == 0 && r.name.is_none()));
    assert!(!plain.is_trade_skill);
}

/// `GetTrainerServiceAbilityReq` (`0x4d96e0`): a level-gated service's known prerequisite still
/// reads met.
#[test]
fn ability_req_met_tracks_known_spells_not_the_service_category() {
    // The service spell and the requirement have records, so the known test is reached
    // (`0x4d97c8`, `0x4d9842`).
    let spells = SpellCatalog::from_displays(HashMap::from([
        (845, taught_ability()),
        (78, taught_ability()),
    ]));
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
        vec![
            TrainerAbilityReq {
                spell_id: 78,
                name: Some("Heroic Strike (Rank 1)".to_string()),
                met: true,
            },
            TrainerAbilityReq {
                spell_id: 0,
                name: None,
                met: true,
            },
            TrainerAbilityReq {
                spell_id: 0,
                name: None,
                met: true,
            },
        ],
        "the prereq shows its rank and reads met because the player knows it"
    );
    let svc = resolve_service(&w, 0, &spells, None, &BTreeSet::new());
    assert!(!svc.ability_reqs[0].met);
    assert_eq!(
        svc.ability_reqs[0].name.as_deref(),
        Some("Heroic Strike (Rank 1)")
    );
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
                Ask::New(&deps.commands),
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
                    Ask::New(&deps.commands),
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
                Ask::New(&deps.commands),
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
                Ask::New(&deps.commands),
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
                Ask::New(&deps.commands),
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
                Ask::New(&deps.commands),
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
                Ask::New(&deps.commands),
                text,
            ),
            Some("Causes 10 threat.".into()),
            "the same taught spell under LEARN_SPELL reads the player's"
        );
    });
}

/// The `$z` token: the bind point's area name (`AreaTable.dbc`), and the `HOME_INN` text where no
/// bind point has arrived (`0x507f3a`), as the item and spell tooltips' `$z` behaves.
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
        (None, "Returns you to <INN>."),
    ] {
        with_text(&spells, 0, 0, home, |text| {
            assert_eq!(
                service_description(
                    &wire(556, trainer_spell_state::GREEN, 4000, 30, 0),
                    0,
                    &spells,
                    &deps.items,
                    Ask::New(&deps.commands),
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
            &ReqBooks {
                player: &BTreeSet::new(),
                pet: None,
                race_class: (1, 1),
                skills: Some(&[]),
            },
            None,
            &deps.items,
            Ask::New(&deps.commands),
            text,
            benilla_ui::strings::Strings::Host(&probe_strings),
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
                Ask::New(&deps.commands),
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

/// No list, no snapshot; a close zeroes only the cell (`0x4d747c`), the records staying until the
/// next list, and the session's end takes them.
#[test]
fn snapshot_is_none_before_a_list_and_keeps_the_records_through_a_close() {
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
    assert_eq!(open.trainer, None);
    assert_eq!(snap(&open, &spells).map(|s| s.services.len()), Some(2));
    open.clear_session();
    assert!(snap(&open, &spells).is_none());
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
        super::service_icon(100, 0, &spells, None, &deps.items, Ask::New(&deps.commands)),
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

/// Every list steps the window's list, which the VM's rows carry so a live read answers only for
/// its own; a pass is owed only with a window open (`0x4d7d46`'s early-out).
#[test]
fn every_list_packet_begins_a_window_session() {
    const TRAINER: u64 = 0xabc;
    let list = || vec![wire(1, trainer_spell_state::GREEN, 10, 1, 0)];
    let mut open = TrainerOpen::default();
    let mut lane = TrainerLane::default();

    open.open(TRAINER, 0, list(), "Greetings".into());
    let first = open.list;
    open.open(TRAINER, 0, list(), "Greetings".into());
    assert_ne!(
        open.list, first,
        "the same trainer's list again is a list again"
    );

    pass_owed(Some(&open), Some(&mut lane));
    assert_eq!(lane.stamped().len(), 1);
    open.clear();
    pass_owed(Some(&open), Some(&mut lane));
    assert_eq!(lane.stamped().len(), 1, "no window, no pass");
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

    /// A type-1 row re-derived to state 3 (`0x4d7e3e`) is listed under no filter: the filter map
    /// `0x4da390` names bits 0-2 only, and the finalizer hides a row whose state bit is clear
    /// (`0x4d84ba`). Its type string is still `"unavailable"` (`0x4d8ba0`).
    #[test]
    fn a_hidden_row_shows_under_no_filter() {
        let mut app = super::feed_app(catalog(), skill_lines());
        app.world_mut()
            .resource_mut::<PlayerActions>()
            .spells
            .insert(RANK2);
        super::list(&mut app, 0x10, 1, vec![service(RANK1_WRAPPER)]);
        assert_eq!(super::tick(&mut app), ["TRAINER_SHOW"]);
        let every_filter = "SetTrainerServiceTypeFilter('available', 1) \
                            SetTrainerServiceTypeFilter('unavailable', 1) \
                            SetTrainerServiceTypeFilter('used', 1) SEEN = {} \
                            return GetNumTrainerServices()";
        assert_eq!(
            super::lua_count(&mut app, every_filter),
            2,
            "the server's red row and its header"
        );
        super::trigger(&mut app);
        assert_eq!(super::tick(&mut app), ["TRAINER_UPDATE"]);
        assert_eq!(super::lua_count(&mut app, every_filter), 0);
        let kinds = app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<Vec<String>>(
            "local out = {} local i = 1 while true do local _, _, t = GetTrainerServiceInfo(i) \
             if t == nil then break end table.insert(out, t) i = i + 1 end return out",
            )
            .unwrap();
        assert!(kinds.contains(&"unavailable".to_string()), "{kinds:?}");
    }

    /// The list files a row the server sent known into the known group, key -1 (`0x4d77e8`), and
    /// the state pass writes -1 into every row it leaves known (`0x4d82b2`-`0x4d82c1`), never back:
    /// a known row re-derived to learnable stays under the known header until the next list.
    #[test]
    fn a_row_keyed_known_keeps_the_key_until_the_next_list() {
        let mut app = super::feed_app(catalog(), skill_lines());
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("KNOWN_TALENTS_HEADER = 'My Talents'")
            .unwrap();
        let mut known = service(RANK1_WRAPPER);
        known.state = trainer_spell_state::GRAY;
        let list = |app: &mut App, row: TrainerSpell| super::list(app, 0x10, 1, vec![row]);
        let header = |app: &mut App| -> String {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<String>(
                    "SetTrainerServiceTypeFilter('all') SEEN = {} \
                     return (GetTrainerServiceInfo(1))",
                )
                .unwrap()
        };
        list(&mut app, known);
        assert_eq!(super::tick(&mut app), ["TRAINER_SHOW"]);
        assert_eq!(header(&mut app), "My Talents");
        // RANK1 is not in the book: the pass reads it learnable, and the key stays -1.
        super::trigger(&mut app);
        assert_eq!(super::tick(&mut app), ["TRAINER_UPDATE"]);
        assert_eq!(header(&mut app), "My Talents");
        // The next list files it by its state again.
        list(&mut app, service(RANK1_WRAPPER));
        assert_eq!(super::tick(&mut app), ["TRAINER_SHOW"]);
        assert_eq!(header(&mut app), "Skill 1");
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

/// The list asks no product: the reference's list and its passes look no item up, and a product
/// is asked by the icon and description getters' calls alone (`0x4d9096`, `0x4d9d07`), inside
/// them. No list, no pass and no frame asks.
#[test]
fn the_list_asks_no_product() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = feed_app(
        icon_catalog(),
        benilla_formats::SkillLineCatalog::from_spell_lines([]),
    );
    app.insert_resource(NetCommands(tx));
    let list = |app: &mut App| {
        list(
            app,
            0xF130_0000_4500_0001,
            TRAINER_TYPE_TRADESKILL,
            vec![wire(100, trainer_spell_state::GREEN, 10, 1, 0)],
        );
    };
    let product_asks = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
        crate::query_cache::queries(rx)
            .iter()
            .filter(|c| matches!(c, ClientCommand::ItemQuery { entry: 777, .. }))
            .count()
    };
    list(&mut app);
    for _ in 0..3 {
        tick(&mut app);
    }
    assert_eq!(product_asks(&rx), 0, "the list and its drains");
    list(&mut app);
    trigger(&mut app);
    tick(&mut app);
    assert_eq!(product_asks(&rx), 0, "the next list and a pass");
}

/// [`resolve_service`] with the pet's book beside the player's, as `GetTrainerServiceAbilityReq`
/// asks it for a `LEARN_PET_SPELL` service (`0x4d9850`-`0x4d9882`).
fn resolve_with_pet(
    wire: &TrainerSpell,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    player: &BTreeSet<u32>,
    pet: Option<&BTreeSet<u32>>,
) -> TrainerService {
    let deps = Deps::new();
    with_text(spells, 0, 0, None, |text| {
        super::resolve_service(
            wire,
            0,
            spells,
            skill_lines,
            &ReqBooks {
                player,
                pet,
                race_class: (1, 1),
                skills: Some(&[]),
            },
            None,
            &deps.items,
            Ask::New(&deps.commands),
            text,
            benilla_ui::strings::Strings::Host(&probe_strings),
        )
    })
}

/// `IsTrainerServiceTradeSkill` (`0x4d9f70`) answers from the wire spell's effects, never the
/// trainer's type: 1 for a `LEARN_SPELL` (36) slot teaching a spell with `Attributes & 0x20`
/// (`0x4d9fe0`-`0x4d9ffe`). A profession rank at a tradeskill trainer teaches a plain spell, and a
/// rogue's poison at a class trainer teaches a 0x20 one; a wire spell with no record answers nil.
#[test]
fn trade_skill_answers_from_the_taught_spells_attribute_not_the_trainer_type() {
    let poison = SpellDisplay {
        name: "Instant Poison".into(),
        attributes: SPELL_ATTR_IS_TRADESKILL,
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([
        (2020, wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 2018)),
        (2018, taught_ability()),
        (8681, wrapper(SPELL_EFFECT_LEARN_SPELL, 1, 8680)),
        (8680, poison),
    ]));
    let rank = |spell, trainer_type| {
        resolve_service(
            &wire(spell, trainer_spell_state::GREEN, 10, 5, 0),
            trainer_type,
            &spells,
            None,
            &BTreeSet::new(),
        )
    };
    assert!(!rank(2020, TRAINER_TYPE_TRADESKILL).is_trade_skill);
    assert!(rank(8681, 0).is_trade_skill);
    assert!(!rank(9999, TRAINER_TYPE_TRADESKILL).is_trade_skill);
}

/// `GetTrainerServiceAbilityReq` (`0x4d96e0`) reads a requirement met when the player knows it
/// (`0x60c740`) or a later rank of it (`0x60c8d0`, the `SkillLineAbility` successor chain): a
/// learned Heroic Strike rank 2 lists rank 1, which left the book with the learn.
#[test]
fn ability_req_is_met_by_a_known_higher_rank() {
    const RANK1: u32 = 78;
    const RANK2: u32 = 284;
    let spells = SpellCatalog::from_displays(HashMap::from([
        (285, wrapper(SPELL_EFFECT_LEARN_SPELL, 0, RANK2)),
        (RANK1, taught_ability()),
        (RANK2, taught_ability()),
    ]));
    let row = |skill_id, forward| benilla_formats::SlaInfo {
        skill_id,
        req_skill_value: 0,
        forward_spell_id: forward,
        trivial_low: 0,
        trivial_high: 0,
    };
    let lines = SkillLineCatalog::from_abilities([(RANK1, row(26, RANK2)), (RANK2, row(26, 0))]);
    let mut w = wire(285, trainer_spell_state::GRAY, 10, 8, 0);
    w.req_spells = [RANK1, 0, 0];
    let known: BTreeSet<u32> = [RANK2].into_iter().collect();
    let svc = resolve_with_pet(&w, &spells, Some(&lines), &known, None);
    assert!(svc.ability_reqs[0].met, "rank 2 known: rank 1 is met");
    let svc = resolve_with_pet(&w, &spells, Some(&lines), &BTreeSet::new(), None);
    assert!(!svc.ability_reqs[0].met, "neither rank known");
}

/// The flag starts at 1 (`0x4d9761`) and only a resolved service spell reaches the tests
/// (`0x4d9821`-`0x4d9842`), so a wire spell with no record reads met whatever the book holds.
/// A `LEARN_PET_SPELL` (57) service asks the pet's book (`0x4d9850`-`0x4d9882`), and with no pet
/// the flag stays 1 (`0x4d9884`).
#[test]
fn ability_req_asks_the_pets_book_for_a_pet_spell_and_none_without_a_record() {
    let spells = SpellCatalog::from_displays(HashMap::from([
        (700, wrapper(SPELL_EFFECT_LEARN_PET_SPELL, 0, 301)),
        (300, taught_ability()),
    ]));
    let mut w = wire(700, trainer_spell_state::GREEN, 10, 1, 0);
    w.req_spells = [300, 0, 0];
    let player: BTreeSet<u32> = [300].into_iter().collect();
    let empty = BTreeSet::new();
    let met = |pet: Option<&BTreeSet<u32>>, player: &BTreeSet<u32>| {
        resolve_with_pet(&w, &spells, None, player, pet).ability_reqs[0].met
    };
    assert!(!met(Some(&empty), &player), "the pet lacks it: unmet");
    assert!(met(Some(&player), &empty), "the pet knows it: met");
    assert!(met(None, &empty), "no pet: met");

    let mut stray = wire(9999, trainer_spell_state::GREEN, 10, 1, 0);
    stray.req_spells = [300, 0, 0];
    assert!(
        resolve_with_pet(&stray, &spells, None, &empty, None).ability_reqs[0].met,
        "no record for the service spell: met"
    );
}

/// The rows sort only where the reference finalizes them, the list (`0x4d7b3b`) and the state
/// pass `0x4d7d40`, each from the order the last left (`0x4d857e`): an identical re-list lays them
/// out anew and the re-derive after it carries them, and a name that lands re-sorts nothing, so
/// rows the comparator ties (here four with no `Spell.dbc` record, `0x4d87cd`) keep their places.
#[test]
fn the_trainer_rows_sort_only_at_a_list_and_a_re_derive() {
    const TRAINER: u64 = 0x10;
    let mut app = feed_app(
        icon_catalog(),
        benilla_formats::SkillLineCatalog::from_spell_lines([]),
    );
    let list = |app: &mut App| {
        list(
            app,
            TRAINER,
            TRAINER_TYPE_TRADESKILL,
            (1..=4)
                .map(|cost| wire(300 + cost, trainer_spell_state::GREEN, cost, 1, 0))
                .collect(),
        );
    };
    let rows = |app: &mut App| costs(app, 5);
    list(&mut app);
    tick(&mut app);
    let first = rows(&mut app);
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_player(TRAINER, "Trainer".into(), None);
    tick(&mut app);
    assert_eq!(rows(&mut app), first, "a landed name re-sorts nothing");
    list(&mut app);
    tick(&mut app);
    assert_eq!(
        rows(&mut app),
        first,
        "an identical re-list lays the rows out anew"
    );
    trigger(&mut app);
    tick(&mut app);
    let carried = rows(&mut app);
    assert_ne!(
        carried, first,
        "the re-derive sorts from the order the list left"
    );
}

/// The trainer lane over a VM that records `TRAINER_SHOW` and `TRAINER_UPDATE` in `SEEN`, with
/// `spells` and `lines` loaded.
fn feed_app(spells: SpellCatalog, lines: SkillLineCatalog) -> App {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(NetCommands(tx))
        .init_resource::<TrainerOpen>()
        .init_resource::<PlayerActions>()
        .init_resource::<Items>()
        .init_resource::<NameCache>()
        .init_resource::<GuidIndex>()
        .init_resource::<crate::ui_pet::PetBar>()
        .init_resource::<crate::spell::SpellModifiers>()
        .insert_resource(Spells {
            catalog: spells,
            ..Spells::empty_for_tests()
        })
        .insert_resource(SkillLines { catalog: lines });
    lane::add_trainer_lane(&mut app);
    let mut script = UiScript::new().unwrap();
    script.keep_calls();
    script
        .run(
            "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('TRAINER_SHOW') \
             f:RegisterEvent('TRAINER_UPDATE') \
             f:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app
}

/// A list's handler on `trainer`: the window filled and its `TRAINER_SHOW` on the lane, as
/// [`open_trainer`] leaves them, the latch left out.
pub(super) fn list(app: &mut App, trainer: u64, trainer_type: u32, services: Vec<TrainerSpell>) {
    let world = app.world_mut();
    world
        .resource_mut::<TrainerOpen>()
        .open(trainer, trainer_type, services, String::new());
    world.resource_mut::<TrainerLane>().push(TrainerEvent::Show);
}

/// One of the state pass's callers ([`pass_owed`]).
pub(super) fn trigger(app: &mut App) {
    let world = app.world_mut();
    world.resource_scope(|world, mut lane: Mut<TrainerLane>| {
        pass_owed(world.get_resource::<TrainerOpen>(), Some(&mut lane));
    });
}

/// The lanes' drain, then the trainer events the VM saw.
fn tick(app: &mut App) -> Vec<String> {
    crate::lanes::fire_in_order(app.world_mut());
    let s = app.world_mut().non_send_resource_mut::<UiScript>();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let seen = s.eval::<Vec<String>>("return SEEN").unwrap();
    s.run("SEEN = {}").unwrap();
    seen
}

/// Run `chunk`, then make the item lookups its getters kept as the host makes them inside the
/// call ([`super::ask_item_record`]).
fn run_getters(app: &mut App, chunk: &str) {
    let entries = {
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.run(chunk).unwrap();
        s.take_item_asks(benilla_ui::script::ItemCallback::Trainer)
    };
    let world = app.world();
    for entry in entries {
        super::ask_item_record(
            world.resource::<Items>(),
            world.resource::<NetCommands>(),
            entry,
            0,
        );
    }
}

/// `entry`'s record lands, and each trainer callback its lookups left runs the state pass, as the
/// answer's handler runs them (`crate::net::objects`).
fn land(app: &mut App, entry: u32) {
    let callbacks = {
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            entry,
            Some(crate::items::test_template("Copper Shortsword")),
        );
        items.take_callbacks(entry)
    };
    for callback in callbacks {
        if callback == crate::items::RecordCallback::Trainer {
            trigger(app);
        }
    }
}

/// `chunk`'s number.
fn lua_count(app: &mut App, chunk: &str) -> i64 {
    let s = app.world_mut().non_send_resource_mut::<UiScript>();
    s.eval::<i64>(chunk).unwrap()
}

/// `chunk`'s list of numbers.
fn lua_numbers(app: &mut App, chunk: &str) -> Vec<i64> {
    let s = app.world_mut().non_send_resource_mut::<UiScript>();
    s.eval::<Vec<i64>>(chunk).unwrap()
}

/// The icon and description getters' product lookups pass the taught spell's id under
/// `0xb0000000` as the owner: the icon takes the record of the learn slot's trigger and reads its
/// id (`0x4d9050`-`0x4d9074`), the description the taught record's (`0x4d9c98`, `0x4d9cdf`), never
/// the wire spell's.
#[test]
fn the_product_lookups_pass_the_taught_spell_as_their_owner() {
    use benilla_ui::script::{spell_owner, ItemCallback, RecordAsk};
    let wire_spell = SpellDisplay {
        name: "Copper Shortsword".into(),
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        name: "Copper Shortsword".into(),
        attributes: SPELL_ATTR_IS_TRADESKILL,
        effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
        effect_item_type: [777, 0, 0],
        ..Default::default()
    };
    let mut app = feed_app(
        SpellCatalog::from_displays(HashMap::from([(100, wire_spell), (200, taught)])),
        benilla_formats::SkillLineCatalog::from_spell_lines([]),
    );
    list(
        &mut app,
        0x10,
        TRAINER_TYPE_TRADESKILL,
        vec![wire(100, trainer_spell_state::GREEN, 1, 1, 0)],
    );
    tick(&mut app);
    let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
    // Row 1 is the group's header.
    s.run("GetTrainerServiceIcon(2) GetTrainerServiceDescription(2)")
        .unwrap();
    let ask = RecordAsk::Item {
        entry: 777,
        owner: spell_owner(200),
        callback: ItemCallback::Trainer,
    };
    assert_eq!(s.take_record_asks(), [ask, ask]);
}

/// `TRAINER_SHOW` fires from the list builder alone (`0x4d7b73`), so the open trainer's second
/// list fires it again; `TRAINER_UPDATE` from the state pass (`0x4d83f9`) and the mask commits
/// (`0x4d8ca0`-`0x4d8ce0`) alone. The trainer's name landing fires neither.
#[test]
fn the_trainer_events_fire_only_from_their_producers() {
    const TRAINER: u64 = 0x10;
    let mut app = feed_app(
        icon_catalog(),
        benilla_formats::SkillLineCatalog::from_spell_lines([]),
    );
    let list = |app: &mut App| {
        list(
            app,
            TRAINER,
            TRAINER_TYPE_TRADESKILL,
            vec![wire(301, trainer_spell_state::GREEN, 1, 1, 0)],
        );
    };
    list(&mut app);
    assert_eq!(tick(&mut app), ["TRAINER_SHOW"]);
    app.world_mut()
        .resource_mut::<NameCache>()
        .insert_player(TRAINER, "Trainer".into(), None);
    assert!(tick(&mut app).is_empty(), "a landed name fires nothing");
    list(&mut app);
    assert_eq!(tick(&mut app), ["TRAINER_SHOW"], "every list shows");
    trigger(&mut app);
    assert_eq!(tick(&mut app), ["TRAINER_UPDATE"]);
    assert!(tick(&mut app).is_empty());
}

/// The rows' order, each by its cost (a header as 0), over the whole array.
fn costs(app: &mut App, rows: usize) -> Vec<i64> {
    lua_numbers(
        app,
        &format!(
            "local out = {{}} for i = 1, {rows} do out[i] = (GetTrainerServiceCost(i)) end \
             return out"
        ),
    )
}

/// An item record that a getter's lookup missed lands: the callback that lookup registered
/// (`0x4d9140` from `GetTrainerServiceIcon`, `0x4d907e`) runs the state pass `0x4d7d40`
/// (`0x4d914a`), which re-derives, sorts from the order the last pass left and fires
/// `TRAINER_UPDATE`. A record no getter call asked for runs nothing.
#[test]
fn an_item_record_a_getter_missed_runs_the_state_pass() {
    const TRAINER: u64 = 0x10;
    let open = |app: &mut App| {
        // Wrapper 100's product, item 777, is asked for its icon; the rest have no record, so
        // every row ties (`0x4d87cd`) and lands where the sort's history puts it.
        let mut services = vec![wire(100, trainer_spell_state::GREEN, 5, 1, 0)];
        services
            .extend((1..=4).map(|cost| wire(300 + cost, trainer_spell_state::GREEN, cost, 1, 0)));
        list(app, TRAINER, TRAINER_TYPE_TRADESKILL, services);
    };

    // No getter asked: the landing is silent and moves nothing.
    let mut app = feed_app(
        icon_catalog(),
        benilla_formats::SkillLineCatalog::from_spell_lines([]),
    );
    open(&mut app);
    assert_eq!(tick(&mut app), ["TRAINER_SHOW"]);
    let listed = costs(&mut app, 6);
    land(&mut app, 777);
    assert!(tick(&mut app).is_empty(), "no callback, no pass");
    assert_eq!(costs(&mut app, 6), listed);

    // The stock window's detail pane asks the row's icon: the landing runs the pass.
    let mut app = feed_app(
        icon_catalog(),
        benilla_formats::SkillLineCatalog::from_spell_lines([]),
    );
    open(&mut app);
    assert_eq!(tick(&mut app), ["TRAINER_SHOW"]);
    assert_eq!(costs(&mut app, 6), listed);
    run_getters(&mut app, "for i = 1, 6 do GetTrainerServiceIcon(i) end");
    assert!(tick(&mut app).is_empty(), "the record is still out");
    land(&mut app, 777);
    assert_eq!(tick(&mut app), ["TRAINER_UPDATE"]);
    assert_ne!(
        costs(&mut app, 6),
        listed,
        "the pass sorts from the order the list left"
    );
    assert!(tick(&mut app).is_empty(), "the callback is spent");
}

/// `0x4d7d40` runs, and fires `TRAINER_UPDATE` (`0x4d83f9`), once per trigger: two triggers in
/// one frame are two passes, and a record two getter calls missed is two callbacks (`0x4d907e`,
/// dedupe flag 0), so two passes when it lands.
#[test]
fn the_state_pass_runs_once_per_trigger_and_per_callback() {
    let mut app = feed_app(
        icon_catalog(),
        benilla_formats::SkillLineCatalog::from_spell_lines([]),
    );
    list(
        &mut app,
        0x10,
        TRAINER_TYPE_TRADESKILL,
        vec![wire(100, trainer_spell_state::GREEN, 5, 1, 0)],
    );
    assert_eq!(tick(&mut app), ["TRAINER_SHOW"]);
    trigger(&mut app);
    trigger(&mut app);
    assert_eq!(tick(&mut app), ["TRAINER_UPDATE", "TRAINER_UPDATE"]);
    assert!(tick(&mut app).is_empty());

    run_getters(
        &mut app,
        "for i = 1, 2 do GetTrainerServiceIcon(i) end GetTrainerServiceIcon(2)",
    );
    assert!(tick(&mut app).is_empty(), "the record is still out");
    land(&mut app, 777);
    assert_eq!(tick(&mut app), ["TRAINER_UPDATE", "TRAINER_UPDATE"]);
    assert!(tick(&mut app).is_empty(), "the callbacks are spent");
}

/// A pet-learn row's pet is the unit `UNIT_FIELD_SUMMON` names (`0x4d7f2d`, mask 8 at
/// `0x4d7f49`), its level its own (`0x4d7f99`); its book is the pet bar's only when the bar names
/// that unit (`0x60c77a`-`0x60c790`). No such unit is no pet (`0x4d7f5a` → `0x4d803c`).
#[test]
fn a_pet_rows_pet_is_the_summon_not_the_bar() {
    use benilla_protocol::messages::{PetActionEntry, PetSpells};
    use benilla_protocol::ObjectFields;
    const SUMMON: u64 = 0xF140_0000_0000_0001;
    const POSSESSED: u64 = 0xF130_0000_0000_0002;
    let unit = |level: u32| ObjectStore(ObjectFields::from_pairs(&[(2, 0x9), (34, level)]));
    let (summon, possessed) = (unit(10), unit(30));
    let store_of = |guid: u64| match guid {
        SUMMON => Some(&summon),
        POSSESSED => Some(&possessed),
        _ => None,
    };
    let bar = |guid: u64| PetSpells {
        pet_guid: guid,
        spells: vec![PetActionEntry {
            packed: (1 << 24) | 100,
        }],
        ..Default::default()
    };
    let view = |level: u32, known: &[u32]| reeval::PetView {
        level,
        known: known.iter().copied().collect(),
    };
    assert_eq!(
        pet_view(Some(SUMMON), store_of, &bar(POSSESSED)),
        Some(view(10, &[])),
        "the summon, whose book the bar does not hold"
    );
    assert_eq!(
        pet_view(Some(SUMMON), store_of, &bar(SUMMON)),
        Some(view(10, &[100]))
    );
    assert_eq!(
        pet_view(Some(SUMMON), store_of, &bar(0)),
        Some(view(10, &[]))
    );
    assert_eq!(
        pet_view(None, store_of, &bar(POSSESSED)),
        None,
        "a possessed unit's bar is no pet"
    );
    assert_eq!(
        pet_view(Some(0xF140_0000_0000_0099), store_of, &bar(0)),
        None
    );
}

fn skill_slot(
    skill_id: u16,
    value: u16,
    perm_bonus: i16,
) -> benilla_protocol::messages::PlayerSkillSlot {
    benilla_protocol::messages::PlayerSkillSlot {
        skill_id,
        step: 0,
        value,
        max: 300,
        temp_bonus: 0,
        perm_bonus,
    }
}

/// `GetTrainerServiceSkillReq`'s flag (`0x4d9570`) compares the player's rank plus the permanent
/// bonus, added only to a nonzero rank, with the required value: a row that is red for its level
/// still reports the skill met, and an unmet skill shows on a green row. No player, no slot for
/// the line, and an unknown line keep the default 1 or fail as the scan ends (`0x4d945a`,
/// `0x4d9575`, `0x4d95bc`).
#[test]
fn skill_req_met_compares_the_players_skill_not_the_rows_colour() {
    const ALCHEMY: u32 = 171;
    let spells = empty_catalog();
    let lines = SkillLineCatalog::from_spell_lines([]).with_line_names([(ALCHEMY, "Alchemy")]);
    let met = |state: u8, skills: Option<&[benilla_protocol::messages::PlayerSkillSlot]>| {
        let mut w = wire(2018, state, 100, 10, ALCHEMY);
        w.req_skill_value = 50;
        resolve_service_skilled(
            &w,
            TRAINER_TYPE_TRADESKILL,
            &spells,
            Some(&lines),
            &BTreeSet::new(),
            skills,
        )
        .skill_req
        .map(|r| r.met)
    };
    let red = trainer_spell_state::RED;
    let green = trainer_spell_state::GREEN;
    assert_eq!(
        met(red, Some(&[skill_slot(171, 50, 0)])),
        Some(true),
        "50 of 50, red for level"
    );
    assert_eq!(
        met(green, Some(&[skill_slot(171, 40, 9)])),
        Some(false),
        "49 of 50"
    );
    assert_eq!(
        met(green, Some(&[skill_slot(171, 40, 10)])),
        Some(true),
        "bonus counts"
    );
    assert_eq!(
        met(green, Some(&[skill_slot(171, 0, 80)])),
        Some(false),
        "no bonus on rank 0"
    );
    assert_eq!(
        met(green, Some(&[skill_slot(164, 300, 0)])),
        Some(false),
        "no slot for the line"
    );
    assert_eq!(met(red, None), None, "no player is the miss `nil, 0, 1`");
    // A zero value and a line with no row are the miss too (`0x4d94a0`, `0x4d94c5`).
    let skill_req = |req_skill: u32, value: u32| {
        let mut w = wire(2018, green, 100, 10, req_skill);
        w.req_skill_value = value;
        resolve_service_skilled(
            &w,
            TRAINER_TYPE_TRADESKILL,
            &spells,
            Some(&lines),
            &BTreeSet::new(),
            Some(&[]),
        )
        .skill_req
    };
    assert_eq!(skill_req(ALCHEMY, 0), None, "a zero value");
    assert_eq!(skill_req(0, 50), None, "a zero line");
    assert_eq!(skill_req(999, 50), None, "a line with no row");
    assert_eq!(
        skill_req(ALCHEMY, 50).map(|r| r.name),
        Some("Alchemy".into())
    );
}

/// `GetTrainerServiceSkillLine` (`0x4d9160`) names the line of the ability row of the spell the
/// first learn effect teaches (`0x4d91e6`), else of the sent spell, and falls on to the spell the
/// record's third `EffectMiscValue` numbers (`0x4d9261`); nil without a player or a match. It is
/// not the group's name, which at a tradeskill trainer is the learn header.
#[test]
fn skill_line_name_is_the_taught_spells_line_not_the_group() {
    let learn = |taught: u32| SpellDisplay {
        name: "Alchemy".into(),
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [taught, 0, 0],
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([
        (2018, learn(2259)),
        (2019, learn(31337)),
        (2259, taught_ability()),
        (78, taught_ability()),
        (
            5000,
            SpellDisplay {
                effect_misc_value: [0, 0, 78],
                ..taught_ability()
            },
        ),
    ]));
    let lines = SkillLineCatalog::from_spell_lines([(2259, 171), (78, 26)])
        .with_line_names([(171, "Alchemy"), (26, "Arms")]);
    let name = |spell: u32, trainer_type: u32, skills: Option<&[_]>| {
        resolve_service_skilled(
            &wire(spell, trainer_spell_state::GREEN, 100, 1, 0),
            trainer_type,
            &spells,
            Some(&lines),
            &BTreeSet::new(),
            skills,
        )
        .skill_line
    };
    let player = Some(&[][..]);
    assert_eq!(
        name(2018, TRAINER_TYPE_TRADESKILL, player).as_deref(),
        Some("Alchemy")
    );
    assert_eq!(name(2018, TRAINER_TYPE_TRADESKILL, None), None, "no player");
    assert_eq!(
        name(2019, 0, player),
        None,
        "a taught spell with no ability row"
    );
    assert_eq!(
        name(78, 0, player).as_deref(),
        Some("Arms"),
        "the sent spell's own row"
    );
    assert_eq!(
        name(5000, 0, player).as_deref(),
        Some("Arms"),
        "the third misc value's spell"
    );
    assert_eq!(name(31337, 0, player), None, "no record");
}
