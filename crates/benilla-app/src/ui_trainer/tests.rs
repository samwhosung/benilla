//! The trainer feed's tests — the two byte-transcribed laws ([`super::law`]), the wire→service
//! resolution, and the snapshot.

use super::*;
use benilla_formats::{
    ItemDisplay, ItemDisplayCatalog, SpellDisplay, SPELL_ATTR_IS_TRADESKILL,
    SPELL_EFFECT_CREATE_ITEM, SPELL_EFFECT_LEARN_PET_SPELL, SPELL_EFFECT_LEARN_SPELL,
};
use benilla_protocol::messages::trainer_spell_state;
use benilla_ui::script::TrainerTooltip;
use std::collections::HashMap;

/// An empty spell catalog (`Spell.dbc` not resolved) — the resolver's name/icon lookups miss.
fn empty_catalog() -> SpellCatalog {
    SpellCatalog::from_displays(HashMap::new())
}

/// The feed-side trio [`service_icon`] needs — the shared seam, since the tradeskill and craft
/// icon laws terminate in the same ask-once cache. Gate 3's "template in flight" arm is the
/// default; the tests that want a landed template seed one.
use crate::items::TestDeps as Deps;

/// The three group-header labels [`super::law::service_group`] reads, spelled **unlike** the
/// shipped ones on purpose: `TRADESKILL_SERVICE_STEP`/`_LEARN` and
/// `KNOWN_TALENTS_HEADER` are single-value keys, so an assertion on their enUS wording would pass
/// for any lookup at all. Naming the key in the value makes each assertion say *which* lookup the
/// arm made. The real table is checked by
/// [`the_group_header_keys_resolve_in_the_real_global_strings`].
fn probe_strings(key: &str) -> Option<String> {
    match key {
        "TRADESKILL_SERVICE_STEP" => Some("<STEP>".into()),
        "TRADESKILL_SERVICE_LEARN" => Some("<LEARN>".into()),
        "KNOWN_TALENTS_HEADER" => Some("<KNOWN>".into()),
        _ => None,
    }
}

/// [`resolve_service`] with the icon trio defaulted — the shape the gate/state tests want.
fn resolve_service(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    known: &BTreeSet<u32>,
) -> TrainerService {
    let deps = Deps::new();
    super::resolve_service(
        wire,
        trainer_type,
        spells,
        skill_lines,
        known,
        None,
        &deps.items,
        &deps.commands,
        &probe_strings,
    )
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

/// [`snapshot`] with the icon trio defaulted.
fn snap(open: &TrainerOpen, spells: &SpellCatalog) -> Option<TrainerState> {
    let deps = Deps::new();
    snapshot(
        open,
        spells,
        None,
        &BTreeSet::new(),
        None,
        &deps.items,
        &deps.commands,
        &probe_strings,
    )
}

/// The icon law's fixture (`GetTrainerServiceIcon 0x4d8f50`'s shape, synthetic so
/// every gate is reachable): wrapper 100 teaches 200 via a slot-0 `LEARN_SPELL`; 200 creates
/// item 777; item 777's display 5 carries the "real" art. Each spell has a DISTINCT icon so a
/// wrong arm is named by the assertion, not merely unequal.
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

/// The product item's template + `ItemDisplayInfo` row, landed.
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

/// The trainer icon law, arm by arm. The director's report — a blue `Spell_Shadow_SealOfKings`
/// crown where the sword belongs — was BOTH halves of this wrong: we substituted nothing, and we
/// fell back to the taught spell's icon, which the client never paints at a trainer on any path.
#[test]
fn trainer_icon_substitutes_the_product_only_at_a_tradeskill_trainer() {
    let spells = icon_catalog();

    // Gate 1+2+3 all pass, template landed → the created ITEM's icon.
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 100, true).as_deref(),
        Some("ITEM")
    );

    // Gate 1 fails (a class trainer): the WIRE spell's own icon — never the taught spell's.
    // This is the corrected fallback; the old code returned "TAUGHT" here.
    assert_eq!(
        icon_of(&spells, 0, 100, true).as_deref(),
        Some("WRAPPER"),
        "a class trainer serves the wrapper's own icon, not the taught spell's"
    );

    // Gate 2 fails (no learn-wrapper effect in any slot) → the wire spell's own icon. Spell 200
    // creates an item, so only the missing wrapper effect can be keeping the substitution off.
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 200, true).as_deref(),
        Some("TAUGHT"),
        "200 IS the wire spell here, so its own icon is the right answer"
    );

    // A spell with no record at all → nil (every failure funnels there, 0x4d911c).
    assert_eq!(icon_of(&spells, TRAINER_TYPE_TRADESKILL, 999, true), None);
}

/// Gate 3: a wrapper whose taught spell creates NO item falls back to the wire icon — the
/// class/pet-trainer population, which is why the fallback arm has to be right.
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

/// The wrapper scan covers all three effect slots and accepts `LEARN_PET_SPELL` (57) as well as
/// `LEARN_SPELL` (36) — the paired `cmp ecx,0x24` / `cmp ecx,0x39` at `0x4d8ff5`/`0x4d8ffa`.
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

/// The in-flight case: gate 3 passes but the item template is not cached, so the icon reads
/// `None` **and the template gets asked for exactly once**. The real client pushes Lua nil here
/// and repaints from the cache callback (`0x4d9140` → `TRAINER_UPDATE`); our equivalent is the
/// feed's own snapshot diff, which re-fires once the answer changes the texture.
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

    // A second frame before the answer lands must not re-ask (the ask-once cache).
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

/// The director's exact case on the real shipped `Spell.dbc` (`GetTrainerServiceIcon 0x4d8f50`):
/// spell 2756 is the wrapper a Blacksmithing trainer sends, 2739 the recipe it teaches, 2847 the
/// sword. At a tradeskill trainer the law must reach for item 2847; at a class trainer it must
/// serve 2756's own icon. Skips without client data.
#[test]
fn trainer_icon_on_real_data_reaches_for_the_crafted_item() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    // The two textures a wrong law produces, straight off the shipped DBC.
    let wrapper_icon = spells.get(2756).unwrap().icon.clone();
    let recipe_icon = spells.get(2739).unwrap().icon.clone();
    assert_eq!(
        recipe_icon.as_deref(),
        Some("Interface\\Icons\\Spell_Shadow_SealOfKings"),
        "the blue crown the director saw IS the recipe spell's own icon"
    );

    // A tradeskill trainer: gate 3 fires for item 2847 and the icon waits on the template —
    // crucially it is NOT the crown.
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

/// The learn-spell hop end-to-end on real 5875 data: a warrior trainer sends the
/// LEARN wrapper 1605 ("learn Heroic Strike"), which is not in SkillLineAbility — resolve_service
/// must hop through the taught spell (78) to group it under Arms (26) and show its name, while the
/// BUY id stays the wrapper (1605) the server expects. This is the exact failure that emptied the
/// tree. Skips without client data.
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

/// The **display name is the WIRE spell's**, not the taught spell's (refuting 0247's
/// display half). 1605/78 above cannot tell the two laws apart — both are "Heroic Strike", as are
/// 65.9 % of the shipped learn wrappers, which is how the wrong hop survived. The profession-learn
/// wrappers are the visible 34.1 %: on real 5875 data spell **2020 is "Apprentice Blacksmith" with
/// no rank**, where the spell it teaches (2018) is "Blacksmithing"/"Apprentice" — so this is the row
/// the director watches change. Skips without client data.
#[test]
fn display_name_is_the_wire_spell_not_the_taught_one() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");
    let skills = benilla_formats::load_skill_line_catalog(&mut chain).expect("load skill lines");

    // Dane Lindgren's learn row, exactly as the wire carries it (reqLevel 5, no skill gate).
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

    // And the group law that puts it first: 2020 carries Effect 44 SKILL_STEP where a recipe
    // wrapper does not. Both read off the real `Spell.dbc` rather than a fixture; the header
    // itself is a GlobalStrings key, so the probe table names which one was looked up.
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
    // An unexpected value is treated as gated (safe default), never "learnable".
    assert_eq!(category(99), TrainerServiceCategory::Unavailable);
}

#[test]
fn resolve_reads_cost_state_and_gates_with_no_catalog() {
    // Empty catalog (Spell.dbc not resolved): name/subtext/icon nil, the wire fields still land,
    // and the skill-req name falls back rather than dropping the gate.
    let spells = empty_catalog();
    let mut w = wire(2018, trainer_spell_state::RED, 1000, 20, 164);
    w.req_spells = [78, 0, 0];
    // Empty known set: the player knows nothing → the ability gate reads unmet on its own terms.
    let svc = resolve_service(&w, TRAINER_TYPE_TRADESKILL, &spells, None, &BTreeSet::new());
    assert_eq!(svc.spell_id, 2018);
    assert!(svc.name.is_none(), "no catalog → name in flight");
    assert_eq!(svc.cost, 1000);
    assert_eq!(svc.category, TrainerServiceCategory::Unavailable);
    assert_eq!(svc.level_req, 20);
    // Unavailable service → the SKILL gate reads unmet (coarse, from the category — no per-gate
    // wire bit); the ABILITY gate reads unmet because the empty known set doesn't contain spell 78
    // (per-gate, not from the category). No catalog → the ability name falls back to "Spell 78".
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
    // No spell catalog at a TRADESKILL trainer → the SKILL_STEP predicate reads false and the row
    // falls to the `TRADESKILL_SERVICE_LEARN` group. Nothing is ever dropped at type 2 (the partition is total), so
    // the "unresolved → 0" arm belongs to the skill-line types, not this one.
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

/// A prerequisite ability's met/unmet is per-gate — whether the player KNOWS that spell — and is
/// decoupled from the service's overall category (`GetTrainerServiceAbilityReq 0x4d96e0`).
/// The director's exact case: an UNAVAILABLE spell (gated by level) whose already-learned prev-rank
/// prerequisite must still read met (white), not red. Deterministic (no client data needed).
#[test]
fn ability_req_met_tracks_known_spells_not_the_service_category() {
    let spells = empty_catalog();
    let mut w = wire(845, trainer_spell_state::RED, 100, 20, 0); // unavailable (gated by level)
    w.req_spells = [78, 0, 0]; // requires Heroic Strike (78)

    // Player doesn't know 78 → the prerequisite is unmet (red).
    let unknown = resolve_service(&w, 0, &spells, None, &BTreeSet::new());
    assert_eq!(unknown.category, TrainerServiceCategory::Unavailable);
    assert!(!unknown.ability_reqs[0].met, "prereq unknown → unmet");

    // Player knows 78 → the prerequisite is met (white) EVEN THOUGH the service stays unavailable.
    let known: BTreeSet<u32> = [78].into_iter().collect();
    let learned = resolve_service(&w, 0, &spells, None, &known);
    assert_eq!(learned.category, TrainerServiceCategory::Unavailable);
    assert!(
        learned.ability_reqs[0].met,
        "prereq known → met, decoupled from the unavailable service"
    );
}

/// The prerequisite name carries its rank the way the client does — `"Name (Rank)"` — resolved on
/// real 5875 data: Heroic Strike (78) is Rank 1, so a service requiring it shows "Heroic Strike
/// (Rank 1)", met iff the player knows 78. Skips without client data.
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
    // Not known → same name, but unmet (red).
    let svc = resolve_service(&w, 0, &spells, None, &BTreeSet::new());
    assert!(!svc.ability_reqs[0].met);
    assert_eq!(svc.ability_reqs[0].name, "Heroic Strike (Rank 1)");
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

/// The plain taught ability — no tradeskill bit, so the SPELL route.
fn taught_ability() -> SpellDisplay {
    SpellDisplay {
        name: "Heroic Strike".into(),
        ..Default::default()
    }
}

/// A taught RECIPE — the tradeskill bit set, `Effect[0] == CREATE_ITEM`, product in every slot
/// so a wrong-slot read is named by the assertion rather than reading zero.
fn taught_recipe() -> SpellDisplay {
    SpellDisplay {
        name: "Copper Shortsword".into(),
        attributes: SPELL_ATTR_IS_TRADESKILL,
        effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
        effect_item_type: [2847, 3333, 4444],
        ..Default::default()
    }
}

/// The ITEM arm: at a trainer, a taught spell carrying the tradeskill bit is described by its
/// CREATED ITEM's tooltip — the director's own case, 2756 → 2739 → item 2847.
#[test]
fn tooltip_takes_the_item_arm_when_the_taught_spell_is_a_recipe() {
    let spells = SpellCatalog::from_displays(HashMap::from([
        (2756, wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 2739)),
        (2739, taught_recipe()),
    ]));
    assert_eq!(service_tooltip(2756, &spells), TrainerTooltip::Item(2847));
}

/// The SPELL arm hops: a class trainer's service describes the TAUGHT ability, not the wrapper.
/// This is the half that disagrees with the icon, which paints the WIRE spell's art — the two
/// are different records on the same row, by design.
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
    // The icon law, same row, same catalog: the WIRE spell's own art. Pinned together so the
    // disagreement is visible in one place and can't be "fixed" into agreement.
    let deps = Deps::new();
    assert_eq!(
        super::service_icon(100, 0, &spells, None, &deps.items, &deps.commands),
        Some("WRAPPER".into()),
    );
}

/// `altCaster` is set by exactly one thing: the matched slot being `LEARN_PET_SPELL`. It gates
/// both the totems and the reagents blocks in the builder.
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

/// The scan is STRICTER than the icon's: a learn slot whose trigger doesn't resolve advances to
/// the NEXT slot rather than abandoning the substitution. Slot 0 triggers a spell that isn't in
/// the catalog; slot 1 triggers a real one, and slot 1 is what must win.
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

/// The item SLOT quirk (`0x52e610`'s own redirect at `0x52e6d2`): `Attributes & 0x20` alone
/// decides item-vs-spell. `Effect[0] == 24` only picks WHICH slot the item id comes from — the
/// matched wrapper slot `i` when it holds, slot 0 when it doesn't. Here the wrapper's learn
/// effect is in slot 1, so a recipe reads `EffectItemType[1]` while a non-`CREATE_ITEM`
/// tradeskill spell reads `EffectItemType[0]`.
#[test]
fn tooltip_item_slot_follows_the_effect_gate_not_the_attribute_gate() {
    // The learn effect in slot 1, slot 0 empty. Built per-use: SpellDisplay is not Clone.
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

    // Same wrapper, a taught spell that carries the bit WITHOUT Effect[0]==24: still the item
    // arm (the bit alone decides), but off slot 0.
    let mut odd = taught_recipe();
    odd.effects[0] = 6;
    let spells = SpellCatalog::from_displays(HashMap::from([(100, slot1_wrapper()), (200, odd)]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Item(2847),
        "the bit alone still routes to the item builder, from slot 0"
    );
}

/// No learn wrapper at all (a direct ability, or `Spell.dbc` not loaded): the WIRE spell — the
/// only path that describes the wrapper.
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
    // Catalog miss (before Spell.dbc lands) — same answer, no panic.
    assert_eq!(
        service_tooltip(100, &empty_catalog()),
        TrainerTooltip::Spell {
            spell_id: 100,
            alt_caster: false,
        },
    );
}

/// **Every list packet begins a window session.** The reset the reference's list builder does
/// per packet (`0x4d75d9`) is only ever observable at a window *opening*, because
/// the reference never receives a list packet with the window already up: a purchase, a level, a
/// skill point repaint the open window through the state re-evaluator (`0x4d7d40`,
/// [`super::reeval`]), never through a second list. Since 2333 benilla is the
/// same — the post-buy re-request that B256's "refresh" mark existed to tell apart is gone — so
/// the law is one line: a list packet opens, and opening resets.
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

    // And a trigger with no window open is nothing at all (`0x4d7d46`'s early-out).
    open.clear();
    open.trigger_re_derive();
    assert!(!open.re_derive);
}

mod re_derive {
    //! The state re-evaluator's clauses ([`super::super::reeval`]), one test per leg of
    //! `0x4d7d40`, on synthetic catalogs.
    use super::super::reeval::{re_derive, PetView, PlayerView, SkillSlot};
    use super::*;
    use benilla_formats::{LearnEffect, SlaInfo, SpellDisplay};
    use std::collections::HashMap;

    const BLACKSMITHING: u32 = 164;
    /// Wrapper 2020 teaches 2018 and steps Blacksmithing to 1 — the real openers' shape.
    const OPENER: u32 = 2020;
    const TAUGHT: u32 = 2018;
    /// A class ability wrapper: rank 1 teaches 100, rank 2 teaches 101 (100 → 101 by
    /// `forward_spellid`), no skill step.
    const RANK1_WRAPPER: u32 = 500;
    const RANK1: u32 = 100;
    const RANK2: u32 = 101;
    /// A pet wrapper: teaches the pet 300.
    const PET_WRAPPER: u32 = 700;
    const PET_SPELL: u32 = 300;

    /// A rank-2 wrapper: teaches RANK2 (whose previous rank is RANK1).
    const RANK2_WRAPPER: u32 = 501;

    fn catalog() -> SpellCatalog {
        // Every wrapper has a record — the admission gate (`0x4d7dcd`) skips a row without one.
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
        // The WIRE state is RED on purpose: every case below must ignore it (`0x4d7dec`).
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
        // A recipe-shaped service: a plain learn wrapper with a skill requirement. (An OPENER
        // would read gray here — the player already holds the step it teaches.)
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
        // Met (value + permanent bonus reaches it): the leg leaves the state alone.
        p.skills[0].value_plus_perm = 75;
        assert_eq!(state(&w, 0, &p), trainer_spell_state::GREEN);
    }

    /// The reference's own wart (`0x4d8082`): the skill scan's not-found exit jumps past the
    /// compare, so a line the player lacks entirely writes nothing — green, where the server sends
    /// red.
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
        // Trainer type 1 too: RANK1 is not the rank before TAUGHT, so this is a plain miss, not
        // the hidden state (`0x4d83a4`'s extra conjunct — `a_missing_previous_rank_…` pins it).
        assert_eq!(state(&w, 1, &player()), trainer_spell_state::RED);
        // Known — or known at a higher rank — satisfies it.
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
        // The row's level binds the player too (`0x4d8239`); keep the player clear of it so the
        // pet legs are what decides.
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
        // Known by the PET, not the player: gray.
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
        // A known service with an unmet skill line and an unmet level is GRAY: legs 4 and 5 run
        // only while the state is still 0.
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

    /// The admission gate (`0x4d7dcd`): a wire spell with no `Spell.dbc` record is skipped
    /// whole — its wire state stands, whatever the player has.
    #[test]
    fn a_row_without_a_spell_record_keeps_its_wire_state() {
        let w = wire(999_999, trainer_spell_state::RED, 10, 1, 0);
        assert_eq!(state(&w, 0, &player()), trainer_spell_state::RED);
        let w = wire(999_999, trainer_spell_state::GRAY, 10, 1, 0);
        assert_eq!(state(&w, 0, &player()), trainer_spell_state::GRAY);
    }

    /// `0x4d7e3e`: at a type-1 trainer, a rank the player has already passed is hidden — and it
    /// is counted as a learn effect but never as a known one, so it cannot come back as gray.
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

    /// `0x4d83a4`: the type-1 hidden state on the required-ability leg needs the missing ability
    /// to be exactly the rank before what the row teaches; any other missing ability is plain
    /// unavailable, and so is every miss at every other type.
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

    /// The pet-spell legs are early exits: nothing after a missing or under-level pet is
    /// visited (`0x4d803c`/`0x4d7fac` leave the loop), and a pet that KNOWS the spell is never
    /// level-tested at all.
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

    /// The required-ability leg runs on the PET's book when the row resolved a pet, and its
    /// failure is unavailable at every trainer type — the type-1 hidden state is the player
    /// variant's alone.
    #[test]
    fn a_pet_rows_required_abilities_are_the_pets_and_never_hide() {
        let mut w = service(PET_WRAPPER);
        w.req_spells = [RANK1, 0, 0];
        let mut p = player();
        p.level = 20;
        p.known.insert(RANK1); // the PLAYER knows it — irrelevant for a pet row
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

/// The three group-header keys against the player's REAL `GlobalStrings.lua` — the guard the
/// synthetic [`probe_strings`] table needs beside it, since a typo'd key yields an empty header
/// rather than a wrong one. Skips without client data.
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
