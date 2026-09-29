//! The spell-view cell tests, against the real 5875 data, and the feed's place in the schedule.

use super::spell_feed::{build_view, feed_spell_tooltips, ViewCaster, ViewCtx};
use super::*;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};

/// The view alone, for the cell tests; the feed keeps what it read beside it.
fn spell_tooltip_view(
    spell_id: u32,
    spells: &Spells,
    vctx: &mut ViewCtx,
) -> Option<benilla_ui::script::SpellTooltipView> {
    build_view(spell_id, spells, vctx).map(|(view, _)| view)
}

/// A view context with no player state, the DBC-only half of the builder.
pub(super) struct TestCtx {
    pub(super) items: Items,
    pub(super) commands: NetCommands,
    _rx: crossbeam_channel::Receiver<crate::net::ClientCommand>,
    /// The builder's lookup over the shipped `GlobalStrings.lua`: a stub would pass on wording
    /// the client never shows.
    get: Box<Getter>,
    /// Empty by default; modifier tests populate it explicitly.
    pub(super) spell_mods: crate::spell::SpellModifiers,
    /// Absent by default: every spell's skill level reads 0.
    pub(super) skill_lines: Option<benilla_formats::SkillLineCatalog>,
}

type Getter = dyn Fn(&str) -> Option<String>;

impl TestCtx {
    pub(super) fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let vm = benilla_ui::script::UiScript::new().expect("VM");
        crate::ui_script::load_ui_for_test(&vm, "Interface\\FrameXML\\GlobalStrings.lua");
        Self {
            items: Items::default(),
            commands: NetCommands(tx),
            _rx: rx,
            get: Box::new(move |key| benilla_ui::strings::global(vm.lua(), key)),
            spell_mods: crate::spell::SpellModifiers::default(),
            skill_lines: None,
        }
    }

    fn ctx<'a, 'w, 's>(
        &'a mut self,
        objects: &'a Objects<'w, 's>,
        form: u8,
        sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
    ) -> ViewCtx<'a, 'w, 's> {
        self.ctx_for(objects, form, sub_classes, None)
    }

    fn ctx_engaged<'a, 'w, 's>(
        &'a mut self,
        objects: &'a Objects<'w, 's>,
        store: Option<&'a ObjectStore>,
        target_reach: f32,
    ) -> ViewCtx<'a, 'w, 's> {
        let mut ctx = self.ctx_for(objects, 0, None, store);
        ctx.attack_target_reach = Some(target_reach);
        ctx
    }

    pub(super) fn ctx_for<'a, 'w, 's>(
        &'a mut self,
        objects: &'a Objects<'w, 's>,
        form: u8,
        sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
        store: Option<&'a ObjectStore>,
    ) -> ViewCtx<'a, 'w, 's> {
        ViewCtx {
            home_area: None,
            form,
            store,
            caster: ViewCaster::Player,
            combat_reach: store.map_or(1.5, |s| s.0.unit_combat_reach()),
            attack_target_reach: None,
            objects,
            items: &mut self.items,
            commands: &self.commands,
            sub_classes,
            skill_lines: self.skill_lines.as_ref(),
            spell_mods: &self.spell_mods,
            get: self.get.as_ref(),
        }
    }

    /// The pet view's context: the player `store`, and `pet` the unit the selector reads.
    fn pet_ctx<'a, 'w, 's>(
        &'a mut self,
        objects: &'a Objects<'w, 's>,
        store: Option<&'a ObjectStore>,
        pet: Option<&'a benilla_protocol::ObjectFields>,
    ) -> ViewCtx<'a, 'w, 's> {
        let mut ctx = self.ctx_for(objects, 0, None, store);
        ctx.caster = ViewCaster::Pet(pet);
        ctx
    }
}

/// The 5875 spell data the view builder reads; `None` skips where the install is absent.
pub(super) fn real_spells() -> Option<Spells> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    Some(Spells {
        catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
        forms: benilla_formats::load_shapeshift_forms(&mut chain).expect("forms"),
        ranges: benilla_formats::load_spell_ranges(&mut chain).expect("ranges"),
        cast_times: benilla_formats::load_spell_cast_times(&mut chain).expect("cast times"),
        durations: benilla_formats::load_spell_durations(&mut chain).expect("durations"),
        radii: benilla_formats::load_spell_radii(&mut chain).expect("radii"),
    })
}

/// A unit's fields: level (34) and base mana (162).
fn unit(level: u32, base_mana: u32) -> benilla_protocol::ObjectFields {
    benilla_protocol::ObjectFields::from_pairs(&[(22u16, 100u32), (34, level), (162, base_mana)])
}

/// The pet bar's views scale by the pet's level (`0x6e3130` with the selector: the charm or
/// summon's `[vtbl+0xa8]`, `0x60cd80`, level × 5 capped at `maxLevel × 5`): Voidwalker Sacrifice
/// rank 1 (7812: 305, 2.3 a level from 16, cap 22) reads 318 at 22 and at 30, and the player's own
/// view of it, in no line of the player's, reads the floor.
#[test]
fn a_pet_view_scales_sacrifice_by_the_pets_level() {
    let Some(spells) = real_spells() else { return };
    let mut t = TestCtx::new();
    let mut objs = no_objects();
    let objects = objs.get();
    let player = ObjectStore(unit(60, 1373));
    let absorb = |v: benilla_ui::script::SpellTooltipView, n: u32| {
        assert!(
            v.description.contains(&format!("absorb {n} damage")),
            "{n}: {}",
            v.description
        );
    };
    for (level, n) in [(16, 305), (22, 318), (30, 318)] {
        let pet = unit(level, 0);
        let v = spell_tooltip_view(
            7812,
            &spells,
            &mut t.pet_ctx(&objects, Some(&player), Some(&pet)),
        )
        .expect("Sacrifice view");
        absorb(v, n);
    }
    let v = spell_tooltip_view(
        7812,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&player)),
    )
    .expect("Sacrifice view");
    absorb(v, 305);
    // No pet unit reads level 0 (`0x6e31a4`), which floors at the rank's base.
    let v = spell_tooltip_view(7812, &spells, &mut t.pet_ctx(&objects, Some(&player), None))
        .expect("Sacrifice view");
    absorb(v, 305);
}

/// Imp Blood Pact rank 1 (6307: 0.1 a level from 4, cap 14) reads 3 on a level-14 imp's bar.
#[test]
fn a_pet_view_scales_blood_pact_by_the_pets_level() {
    let Some(spells) = real_spells() else { return };
    let mut t = TestCtx::new();
    let mut objs = no_objects();
    let objects = objs.get();
    let player = ObjectStore(unit(60, 1373));
    let pet = unit(14, 0);
    let v = spell_tooltip_view(
        6307,
        &spells,
        &mut t.pet_ctx(&objects, Some(&player), Some(&pet)),
    )
    .expect("Blood Pact view");
    assert!(v.description.contains("Stamina by 3."), "{}", v.description);
    let v = spell_tooltip_view(
        6307,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&player)),
    )
    .expect("Blood Pact view");
    assert!(v.description.contains("Stamina by 2."), "{}", v.description);
}

/// Seduction (6358, 24% of base mana) on the pet bar costs 24% of the succubus's
/// `UNIT_FIELD_BASE_MANA`, as `0x6e31b0` calls `0x612c50` on the pet (`6e327d`): 449 of 1874, not
/// 329 of the warlock's 1373. No pet unit is `GetPowerCost`'s -1 (`0x6e3233`), no cost cell.
#[test]
fn a_pet_view_costs_seduction_from_the_pets_base_mana() {
    let Some(spells) = real_spells() else { return };
    let mut t = TestCtx::new();
    let mut objs = no_objects();
    let objects = objs.get();
    let player = ObjectStore(unit(60, 1373));
    let pet = unit(60, 1874);
    let v = spell_tooltip_view(
        6358,
        &spells,
        &mut t.pet_ctx(&objects, Some(&player), Some(&pet)),
    )
    .expect("Seduction view");
    assert_eq!(v.cost.as_deref(), Some("449 Mana"));
    let v = spell_tooltip_view(
        6358,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&player)),
    )
    .expect("Seduction view");
    assert_eq!(v.cost.as_deref(), Some("329 Mana"), "the player's own view");
    let v = spell_tooltip_view(6358, &spells, &mut t.pet_ctx(&objects, Some(&player), None))
        .expect("Seduction view");
    assert_eq!(v.cost, None);
}

/// An object index with nothing streamed: no worn item, and every reagent count 0.
fn no_objects() -> crate::ui_items::TestObjects {
    crate::ui_items::TestObjects::new()
}

fn empty_player() -> ObjectStore {
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
        22u16, 100u32,
    )]))
}

/// Battle Shout rank 1's `$s1` (14 + 1d1, 0.5 a level from 1, cap 11) scales by the player's
/// skill in its line over 5 (`0x6e3130`), not the character level: a class line sits at level
/// × 5 (vmangos `Player::UpdateSkillsForLevel`), so levels 1, 11 and 60 read 15, 20 and 20.
#[test]
fn battle_shout_description_scales_by_the_skill_level() {
    use benilla_protocol::messages::FIELD_PLAYER_SKILL_INFO_1_1;
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = Spells {
        catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
        forms: benilla_formats::load_shapeshift_forms(&mut chain).expect("forms"),
        ranges: benilla_formats::load_spell_ranges(&mut chain).expect("ranges"),
        cast_times: benilla_formats::load_spell_cast_times(&mut chain).expect("cast times"),
        durations: benilla_formats::load_spell_durations(&mut chain).expect("durations"),
        radii: benilla_formats::load_spell_radii(&mut chain).expect("radii"),
    };
    let skill_lines = benilla_formats::load_skill_line_catalog(&mut chain).expect("skill lines");
    let line = skill_lines
        .spell_to_line(6673)
        .expect("Battle Shout's line");
    let mut t = TestCtx::new();
    t.skill_lines = Some(skill_lines);
    let mut objs = no_objects();
    let objects = objs.get();
    // (character level, skill value, attack power); the last is a level 60 at skill 5.
    for (level, skill, ap) in [(1, 5, 15), (11, 55, 20), (60, 300, 20), (60, 5, 15)] {
        let store = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (34, level),
            (FIELD_PLAYER_SKILL_INFO_1_1, line),
            (FIELD_PLAYER_SKILL_INFO_1_1 + 1, skill | 300 << 16),
        ]));
        let view = spell_tooltip_view(
            6673,
            &spells,
            &mut t.ctx_for(&objects, 0, None, Some(&store)),
        )
        .expect("Battle Shout view");
        assert!(
            view.description.contains(&format!("by {ap}.")),
            "level {level}, skill {skill}: {}",
            view.description
        );
    }
}

/// Fireball rank 1 (133) end to end: description 138, cast index 18 (1500 ms), duration 30.
#[test]
fn fireball_view_on_real_data() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = Spells {
        catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
        forms: benilla_formats::load_shapeshift_forms(&mut chain).expect("SpellShapeshiftForm.dbc"),
        ranges: benilla_formats::load_spell_ranges(&mut chain).expect("SpellRange.dbc"),
        cast_times: benilla_formats::load_spell_cast_times(&mut chain).expect("SpellCastTimes.dbc"),
        durations: benilla_formats::load_spell_durations(&mut chain).expect("SpellDuration.dbc"),
        radii: benilla_formats::load_spell_radii(&mut chain).expect("SpellRadius.dbc"),
    };
    let mut t = TestCtx::new();
    let mut objs = no_objects();
    let objects = objs.get();
    let v = spell_tooltip_view(133, &spells, &mut t.ctx(&objects, 0, None)).expect("Fireball view");
    assert_eq!(v.name, "Fireball");
    assert_eq!(v.rank.as_deref(), Some("Rank 1"));
    assert_eq!(v.cost.as_deref(), Some("30 Mana"));
    assert_eq!(v.range.as_deref(), Some("35 yd range"));
    assert_eq!(v.cast_time.as_deref(), Some("1.5 sec cast"));
    assert_eq!(
        v.cooldown, None,
        "Fireball has no recovery in either column"
    );
    assert_eq!(v.requires_form, None);
    assert!(
        v.description.starts_with("Hurls a fiery ball that causes"),
        "got: {}",
        v.description
    );
    assert!(
        v.description.contains(" to ") && v.description.contains("Fire damage"),
        "the $s range substituted: {}",
        v.description
    );
    assert!(
        !v.description.contains('$'),
        "no unsubstituted tokens: {}",
        v.description
    );

    // Charge rank 1 (100): range row 95 = {8, 25}, a cooldown only in the category column
    // (15000), and the Stances form line (0x10000 is form 17).
    let v = spell_tooltip_view(100, &spells, &mut t.ctx(&objects, 0, None)).expect("Charge view");
    assert_eq!(v.name, "Charge");
    assert_eq!(v.rank.as_deref(), Some("Rank 1"));
    assert_eq!(v.cost, None, "Charge costs nothing (it generates rage)");
    assert_eq!(v.range.as_deref(), Some("8-25 yd range"));
    assert_eq!(v.cast_time.as_deref(), Some("Instant"));
    assert_eq!(v.cooldown.as_deref(), Some("15 sec cooldown"));
    assert_eq!(v.requires_form.as_deref(), Some("Requires Battle Stance"));
    assert!(!v.form_met, "form 0 (unshifted) does not satisfy the mask");
    assert_eq!(
        v.description,
        "Charge an enemy, generate 9 rage, and stun it for 1 sec.  Cannot be used in combat."
    );
    let v = spell_tooltip_view(100, &spells, &mut t.ctx(&objects, 17, None)).expect("Charge view");
    assert!(v.form_met, "form 17 = Battle Stance satisfies the mask");

    // A permissive Stances mask (`AttributesEx2` bit 19) prints no line, though its forms have
    // names: bit 27 is form 28, bit 31 form 32.
    for (id, name, mask) in [
        (588u32, "Inner Fire", 0x0800_0000u32),
        (8122, "Psychic Scream", 0x0800_0000),
        (2061, "Flash Heal", 0x8000_0000),
        (5176, "Wrath", 0x4000_0000),
    ] {
        let d = spells.catalog.get(id).expect(name);
        assert_eq!(d.stances, mask, "{name} carries the permissive mask");
        assert!(
            d.form_mask_is_permissive(),
            "{name} carries AttributesEx2 b19"
        );
        let v = spell_tooltip_view(id, &spells, &mut t.ctx(&objects, 0, None)).expect(name);
        assert_eq!(v.requires_form, None, "{name} demands no form");
    }
}

/// Improved Devotion Aura's +25% on op 8 reaches the spell's description, 55 armor to 68; the
/// aura text expands as the aura tooltip `0x52f880` expands it, with the points' modifiers off
/// (`52f940`), so it keeps 55.
#[test]
fn improved_devotion_aura_updates_the_description_but_not_the_aura_text() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let mut spells = Spells::empty_for_tests();
    spells.catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
    spells.ranges = benilla_formats::load_spell_ranges(&mut chain).expect("SpellRange.dbc");
    spells.durations =
        benilla_formats::load_spell_durations(&mut chain).expect("SpellDuration.dbc");
    spells.radii = benilla_formats::load_spell_radii(&mut chain).expect("SpellRadius.dbc");
    let devotion = spells.catalog.get(465).expect("Devotion Aura rank 1");
    let bit = devotion.spell_family_flags.trailing_zeros() as u8;
    let mut t = TestCtx::new();
    let mut objs = no_objects();
    let objects = objs.get();
    let base = spell_tooltip_view(465, &spells, &mut t.ctx(&objects, 0, None)).unwrap();
    assert!(base.description.contains("55 additional armor"));
    assert!(base.aura_description.contains("55"));

    t.spell_mods.set_class_family(devotion.spell_family);
    t.spell_mods.set(false, bit, 8, 25);
    let improved = spell_tooltip_view(465, &spells, &mut t.ctx(&objects, 0, None)).unwrap();
    assert!(improved.description.contains("68 additional armor"));
    assert_eq!(improved.aura_description, base.aura_description);
}

/// Fire Blast rank 1's 8 s is its category recovery, its own recovery 0; Improved Fire Blast's
/// flat op 11 shortens the category value, and the cell shows the larger column (`0x52eada`).
#[test]
fn improved_fire_blast_shortens_the_cooldown_cell() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let mut spells = Spells::empty_for_tests();
    spells.catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
    let fire_blast = spells.catalog.get(2136).expect("Fire Blast rank 1");
    assert_eq!(
        (fire_blast.recovery_ms, fire_blast.category_recovery_ms),
        (0, 8_000)
    );
    let bit = fire_blast.spell_family_flags.trailing_zeros() as u8;
    let mut t = TestCtx::new();
    let mut objs = no_objects();
    let objects = objs.get();
    let base = spell_tooltip_view(2136, &spells, &mut t.ctx(&objects, 0, None)).unwrap();
    assert_eq!(base.cooldown.as_deref(), Some("8 sec cooldown"));

    t.spell_mods.set_class_family(fire_blast.spell_family);
    t.spell_mods
        .set(true, bit, crate::spell::OP_COOLDOWN, -1_500);
    let improved = spell_tooltip_view(2136, &spells, &mut t.ctx(&objects, 0, None)).unwrap();
    assert_eq!(improved.cooldown.as_deref(), Some("6.5 sec cooldown"));
}

/// The cast cell reads `GetCastTime(1)` (`52eb4b`): op 10 applies and nothing clamps, so a
/// modifier past the whole cast time reaches the negative "Instant cast" arm (`0x52ebce`), where
/// a clamped zero would take the no-mana "Instant" (`0x52ec4b`).
#[test]
fn the_cast_cell_takes_op_10_unclamped() {
    // The cells fill the install's `GlobalStrings.lua`.
    let _data = benilla_formats::wow_data_or_skip!();
    let mut spells = Spells::empty_for_tests();
    let rage_cast = benilla_formats::SpellDisplay {
        name: "Rage Cast".into(),
        casting_time_index: 5,
        power_type: 1,
        spell_family: 4,
        spell_family_flags: 1,
        ..Default::default()
    };
    spells.catalog =
        benilla_formats::SpellCatalog::from_displays([(900_001, rage_cast)].into_iter().collect());
    spells.cast_times = benilla_formats::SpellCastTimeCatalog::from_rows([(
        5,
        benilla_formats::SpellCastTime {
            base_ms: 1500,
            per_level_ms: 0,
            minimum_ms: 1500,
        },
    )]);
    let mut objs = no_objects();
    let objects = objs.get();
    let cell = |flat: i32| {
        let mut t = TestCtx::new();
        t.spell_mods.set_class_family(4);
        t.spell_mods.set(true, 0, crate::spell::OP_CAST_TIME, flat);
        spell_tooltip_view(900_001, &spells, &mut t.ctx(&objects, 0, None))
            .unwrap()
            .cast_time
    };
    assert_eq!(cell(0).as_deref(), Some("1.5 sec cast"));
    assert_eq!(cell(-500).as_deref(), Some("1 sec cast"));
    assert_eq!(cell(-1500).as_deref(), Some("Instant"), "exactly zero");
    assert_eq!(cell(-2000).as_deref(), Some("Instant cast"), "below zero");
}

#[test]
fn cost_and_cast_cells_on_real_data() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = Spells {
        catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
        forms: benilla_formats::load_shapeshift_forms(&mut chain).expect("SpellShapeshiftForm.dbc"),
        ranges: benilla_formats::load_spell_ranges(&mut chain).expect("SpellRange.dbc"),
        cast_times: benilla_formats::load_spell_cast_times(&mut chain).expect("SpellCastTimes.dbc"),
        durations: benilla_formats::load_spell_durations(&mut chain).expect("SpellDuration.dbc"),
        radii: benilla_formats::load_spell_radii(&mut chain).expect("SpellRadius.dbc"),
    };
    let mut t = TestCtx::new();
    let mut objs = no_objects();
    let objects = objs.get();
    // A level-60 store: max health 4000, base mana 1000 (fields: health 22, max health 28,
    // level 34, base mana 162).
    let store = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (22u16, 3500u32),
        (28, 4000),
        (34, 60),
        (162, 1000),
    ]));

    // Bloodrage (2687): 20% of max health, a flat number through the health fallback; bare
    // "Instant" on a non-mana type.
    let v = spell_tooltip_view(
        2687,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Bloodrage view");
    assert_eq!(v.cost.as_deref(), Some("800 Health"), "20% of 4000");
    assert_eq!(v.cast_time.as_deref(), Some("Instant"));

    // Life Tap (1454): the 5875 data has no cost columns for any rank, so the cell is empty.
    let v = spell_tooltip_view(
        1454,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Life Tap view");
    assert_eq!(v.cost, None, "1.12 Life Tap has no cost cell");
    assert_eq!(
        v.cast_time.as_deref(),
        Some("Instant"),
        "health type, not mana"
    );

    // Health Funnel (755): the `_PER_TIME` form in health, and a channeled cast cell.
    let v = spell_tooltip_view(
        755,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Health Funnel view");
    assert_eq!(v.cost.as_deref(), Some("11 Health, plus 5 per sec"));
    assert_eq!(v.cast_time.as_deref(), Some("Channeled"));

    // Judgement (20271): 6% of base mana resolves to a flat number; a DBC-only view has only
    // the flat cost, none here.
    let v = spell_tooltip_view(
        20271,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Judgement view");
    assert_eq!(v.cost.as_deref(), Some("60 Mana"), "6% of base mana 1000");
    let v =
        spell_tooltip_view(20271, &spells, &mut t.ctx(&objects, 0, None)).expect("Judgement view");
    assert_eq!(v.cost, None, "a DBC-only view cannot resolve a pct cost");

    // Heroic Strike (78): rage is wire 150 ÷ 10, and "Next melee" is the cast cell.
    let v = spell_tooltip_view(78, &spells, &mut t.ctx_for(&objects, 0, None, Some(&store)))
        .expect("Heroic Strike view");
    assert_eq!(v.cost.as_deref(), Some("15 Rage"));
    assert_eq!(v.cast_time.as_deref(), Some("Next melee"));

    // Throw (2764) and Auto Shot (75) read "Attack speed" off `Attributes & 0x2` alone; Attack
    // (6603), `Effect[0]` ATTACK, omits the line (`0x52eb3c`).
    let v = spell_tooltip_view(
        2764,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Throw view");
    assert_eq!(v.cast_time.as_deref(), Some("Attack speed"));
    let v = spell_tooltip_view(75, &spells, &mut t.ctx_for(&objects, 0, None, Some(&store)))
        .expect("Auto Shot view");
    assert_eq!(v.cast_time.as_deref(), Some("Attack speed"));
    let v = spell_tooltip_view(
        6603,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Attack view");
    assert_eq!(v.cast_time, None, "ATTACK Effect[0] omits the line");

    // Mind Flay (15407): a channeled mana spell.
    let v = spell_tooltip_view(
        15407,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Mind Flay view");
    assert_eq!(v.cost.as_deref(), Some("45 Mana"));
    assert_eq!(v.cast_time.as_deref(), Some("Channeled"));
}

/// The range cell (`0x52e9a2`-`0x52ea8c`): the melee row prints a number off the reaches, an
/// authored row its own, and the on-next-swing and self rows nothing.
#[test]
fn range_cell_on_real_data() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = Spells {
        catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
        forms: benilla_formats::load_shapeshift_forms(&mut chain).expect("SpellShapeshiftForm.dbc"),
        ranges: benilla_formats::load_spell_ranges(&mut chain).expect("SpellRange.dbc"),
        cast_times: benilla_formats::load_spell_cast_times(&mut chain).expect("SpellCastTimes.dbc"),
        durations: benilla_formats::load_spell_durations(&mut chain).expect("SpellDuration.dbc"),
        radii: benilla_formats::load_spell_radii(&mut chain).expect("SpellRadius.dbc"),
    };
    let mut t = TestCtx::new();
    let mut objs = no_objects();
    let objects = objs.get();
    // A default-reach player: `UNIT_FIELD_COMBATREACH` (130) unset reads the descriptor's 1.5.
    let store = empty_player();

    // Sinister Strike (1752) is on row 2 "Combat Range", the one row with flags bit 0:
    // 1.5 + 1.5 + 1.3333334 = 4.333 is under the 5.0 floor.
    let d = spells.catalog.get(1752).expect("Sinister Strike 1752");
    assert_eq!(d.range_index, 2, "the melee row");
    assert!(spells.ranges.get(2).expect("row 2").is_melee());
    let v = spell_tooltip_view(
        1752,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Sinister Strike view");
    assert_eq!(v.range.as_deref(), Some("5 yd range"));

    // A 4.0 reach: 4.0 + 4.0 + 1.3333334 = 9.333, rounded to 9.
    let big = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
        130u16,
        4.0f32.to_bits(),
    )]));
    let v = spell_tooltip_view(1752, &spells, &mut t.ctx_for(&objects, 0, None, Some(&big)))
        .expect("Sinister Strike view");
    assert_eq!(v.range.as_deref(), Some("9 yd range"));

    // The second reach is the auto-attack target's: 1.5 + 4.0 + 1.3333334 = 6.833, rounded to 7.
    let v = spell_tooltip_view(
        1752,
        &spells,
        &mut t.ctx_engaged(&objects, Some(&store), 4.0),
    )
    .expect("Sinister Strike view");
    assert_eq!(v.range.as_deref(), Some("7 yd range"));

    // An authored row: Fireball's 0-35.
    let v = spell_tooltip_view(
        133,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Fireball view");
    assert_eq!(v.range.as_deref(), Some("35 yd range"));

    // The tooltip passes a null target (`0x52e9c2`), so the auto-attack target pads no ranged row:
    // Fireball still reads its 35 while you swing at a 1.5-reach unit, and Charge its 8-25.
    let v = spell_tooltip_view(
        133,
        &spells,
        &mut t.ctx_engaged(&objects, Some(&store), 1.5),
    )
    .expect("Fireball view");
    assert_eq!(v.range.as_deref(), Some("35 yd range"));
    let v = spell_tooltip_view(
        100,
        &spells,
        &mut t.ctx_engaged(&objects, Some(&store), 1.5),
    )
    .expect("Charge view");
    assert_eq!(v.range.as_deref(), Some("8-25 yd range"));

    // An authored pair, `"%d-%d"`: Charge's 8-25, which the reach does not change.
    let v = spell_tooltip_view(100, &spells, &mut t.ctx_for(&objects, 0, None, Some(&big)))
        .expect("Charge view");
    assert_eq!(v.range.as_deref(), Some("8-25 yd range"));

    // No cell: Heroic Strike (78)'s on-next-swing attributes skip it before the resolver, and
    // Bloodrage (2687)'s self row resolves max 0.
    let v = spell_tooltip_view(78, &spells, &mut t.ctx_for(&objects, 0, None, Some(&store)))
        .expect("Heroic Strike view");
    assert_eq!(v.range, None, "Attributes & 0x404 → no range cell");
    let v = spell_tooltip_view(
        2687,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Bloodrage view");
    assert_eq!(v.range, None, "the self row resolves max 0");
}

/// The required-item, chance-to-X and reagents lines on the real data.
#[test]
fn the_required_item_chance_and_reagent_lines_on_real_data() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = Spells {
        catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
        forms: benilla_formats::load_shapeshift_forms(&mut chain).expect("SpellShapeshiftForm.dbc"),
        ranges: benilla_formats::load_spell_ranges(&mut chain).expect("SpellRange.dbc"),
        cast_times: benilla_formats::load_spell_cast_times(&mut chain).expect("SpellCastTimes.dbc"),
        durations: benilla_formats::load_spell_durations(&mut chain).expect("SpellDuration.dbc"),
        radii: benilla_formats::load_spell_radii(&mut chain).expect("SpellRadius.dbc"),
    };
    let subs = benilla_formats::load_item_sub_classes(&mut chain).expect("ItemSubClass.dbc");
    let mut t = TestCtx::new();
    let mut objs = no_objects();
    let objects = objs.get();
    let store = empty_player();

    // 1. The wand Shoot (5019, class 2, subclass bit 19): "Requires Wands", red with no wand
    // worn; the same row gives the cast-fail line its singular "Wand".
    assert_eq!(subs.name(2, 19), Some("Wands"), "the verbose plural");
    assert_eq!(subs.display_name(2, 19), Some("Wand"), "the singular");
    let v = spell_tooltip_view(
        5019,
        &spells,
        &mut t.ctx_for(&objects, 0, Some(&subs), Some(&store)),
    )
    .expect("Shoot view");
    assert_eq!(v.requires_item.as_deref(), Some("Requires Wands"));
    assert!(!v.item_met, "nothing worn satisfies class 2 / bit 19 → red");
    // A multi-bit mask is named by `ItemSubClassMask.dbc` (`0x52eef0`, `0x6e2380`): Parry's
    // 0x2a5f3 is the eleven melee subclasses, one name.
    let parry = spells.catalog.get(3127).expect("Parry 3127");
    assert!(parry.equipped_item_subclass_mask.count_ones() > 1);
    let v = spell_tooltip_view(
        3127,
        &spells,
        &mut t.ctx_for(&objects, 0, Some(&subs), Some(&store)),
    )
    .expect("Parry view");
    assert_eq!(v.requires_item.as_deref(), Some("Requires Melee Weapon"));

    // 2. Attack (6603): `Effect[0] == 78` omits the cast line (`0x52eb15`), though
    // `Attributes & 0x40` is clear.
    let d = spells.catalog.get(6603).expect("Attack 6603");
    assert_eq!(d.effects[0], 78, "SPELL_EFFECT_ATTACK");
    assert!(!d.passive, "6603 carries Attributes 0x10, not 0x40");
    let v = spell_tooltip_view(6603, &spells, &mut t.ctx(&objects, 0, None)).expect("Attack view");
    assert_eq!(v.cast_time, None, "the law's Effect[0] gate");
    // The chance line `Effect[0]` selects (`0x52f5b1`): ATTACK skips the passive gate. No
    // descriptor, no line; the wire's values are already percents.
    assert_eq!(v.chance, None, "no player streamed yet");
    let rated = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (22u16, 100u32),
        (1109u16, 2.62f32.to_bits()), // PLAYER_CRIT_PERCENTAGE
        (1107u16, 5.5f32.to_bits()),  // PLAYER_DODGE_PERCENTAGE
    ]));
    let v = spell_tooltip_view(
        6603,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&rated)),
    )
    .expect("Attack view");
    assert_eq!(v.chance.as_deref(), Some("2.62% chance to crit"));
    // Dodge (81) is passive and reads its own field.
    let dodge = spells.catalog.get(81).expect("Dodge 81");
    assert_eq!(dodge.effects[0], 20, "SPELL_EFFECT_DODGE");
    assert!(dodge.passive, "81 carries Attributes 0x40");
    let v = spell_tooltip_view(81, &spells, &mut t.ctx_for(&objects, 0, None, Some(&rated)))
        .expect("Dodge view");
    assert_eq!(v.chance.as_deref(), Some("5.50% chance to dodge"));
    // None of the four effects: no line.
    let v = spell_tooltip_view(
        133,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&rated)),
    )
    .expect("Fireball view");
    assert_eq!(v.chance, None);

    // 3. Slow Fall (130): "Reagents: Light Feather", red while unowned; the name comes from the
    // item cache, seeded here as the server would.
    let d = spells.catalog.get(130).expect("Slow Fall 130");
    assert_eq!(d.reagents[0], (17056, 1), "Light Feather ×1");
    let v = spell_tooltip_view(
        130,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Slow Fall view");
    assert_eq!(
        v.reagents, None,
        "the template hasn't landed: the line waits rather than printing an id"
    );
    t.items
        .insert_template(17056, Some(crate::items::test_template("Light Feather")));
    let v = spell_tooltip_view(
        130,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Slow Fall view");
    assert_eq!(
        v.reagents.as_deref(),
        Some("Reagents: |cffff2020Light Feather|r"),
        "count 1 prints no (N); unowned wraps in the builder's inline red"
    );
}

/// Red only when the resolver finds no opener; a key, a known skill, no requirement and no
/// `Lock.dbc` row all read green.
#[test]
fn the_locked_line_greens_when_the_lock_can_be_opened() {
    use crate::target::lock::LockOutcome;

    // The Scarlet Key in hand at the Armory Door.
    assert_eq!(
        locked_line_tint(Some(LockOutcome::OpenByKey(7146))),
        TooltipTint::LockOpen,
        "holding the key must read green"
    );
    // The same door without the key.
    assert_eq!(
        locked_line_tint(Some(LockOutcome::Unmet)),
        TooltipTint::Red,
        "no key still reads red"
    );
    // A known skill opener: green, where the reference ramps by margin (`locked_line_tint`).
    assert_eq!(
        locked_line_tint(Some(LockOutcome::OpenBySpell(2575))),
        TooltipTint::LockOpen
    );
    // A lock row that imposes nothing, and a flag-locked object with no row at all.
    assert_eq!(
        locked_line_tint(Some(LockOutcome::Unlocked)),
        TooltipTint::LockOpen
    );
    assert_eq!(locked_line_tint(None), TooltipTint::LockOpen);
}

/// A trainer list that lands this frame is hoverable in this frame's tick: the trainer feed derives
/// the subjects the spell feed pushes.
#[test]
fn the_spell_feed_runs_after_the_trainer_feed() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    assert!(crate::test_support::runs_before(
        &mut app,
        crate::ui_trainer::feed_trainer,
        feed_spell_tooltips
    ));
}

const ME: u64 = 0x77;
const WOLF: u64 = 0xF130_0000_4500_0001;
const BOAR: u64 = 0xF130_0000_4600_0002;

/// The world-hover driver over a bare VM holding `"player"` (us) and `"target"` (the wolf), each
/// with its guid, as the unit feed pushes them; returns the app, the wolf and the boar.
fn mouseover_app() -> (App, Entity, Entity) {
    use benilla_protocol::ObjectFields;

    let mut app = App::new();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx))
        .insert_resource(crate::ui_script::UiScaleCvar(1.0))
        .init_resource::<Hovered>()
        .init_resource::<HoveredObject>()
        .init_resource::<NameCache>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::go_templates::GameObjectTemplates>()
        .init_resource::<Items>()
        .init_resource::<PlayerActions>()
        .add_systems(Update, drive_mouseover_tooltip);
    app.world_mut()
        .spawn((SelfPlayer, ObjectStore(ObjectFields::default())));
    let mut unit = || {
        app.world_mut()
            .spawn(ObjectStore(ObjectFields::default()))
            .id()
    };
    let (wolf, boar) = (unit(), unit());

    let mut script = UiScript::new().unwrap();
    for (token, guid) in [("player", ME), ("target", WOLF)] {
        script.set_unit(
            token,
            Some(UnitState {
                exists: true,
                has_object: true,
                guid,
                ..Default::default()
            }),
        );
    }
    app.insert_non_send_resource(script);
    (app, wolf, boar)
}

/// Set this frame's pick and run the driver.
fn hover(app: &mut App, pick: Hovered, object: HoveredObject) {
    *app.world_mut().resource_mut::<Hovered>() = pick;
    *app.world_mut().resource_mut::<HoveredObject>() = object;
    app.update();
}

fn hover_unit(app: &mut App, entity: Entity, guid: u64) {
    let pick = Hovered {
        target: Some(entity),
        guid: Some(guid),
        distance: 10.0,
        ..Default::default()
    };
    hover(app, pick, HoveredObject::default());
}

fn eval(app: &mut App, lua: &str) -> Option<i64> {
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<Option<i64>>(lua)
        .unwrap()
}

/// The hover pushes `"mouseover"` with the hovered guid, the pair `0x492890` writes to
/// `0xb4e2c8`/`0xb4e2cc`: `UnitIsUnit` (`0x516070`) resolves both tokens through `0x515970` and
/// compares guids, so hovering the target answers 1 and hovering anyone else nil.
#[test]
fn the_mouseover_token_carries_the_hovered_guid() {
    let (mut app, wolf, boar) = mouseover_app();
    let is_unit = |app: &mut App, other: &str| {
        eval(
            app,
            &format!(r#"return UnitIsUnit("mouseover", "{other}")"#),
        )
    };

    hover_unit(&mut app, wolf, WOLF);
    assert_eq!(is_unit(&mut app, "target"), Some(1), "hovering the target");
    assert_eq!(is_unit(&mut app, "player"), None, "the target is not us");

    hover_unit(&mut app, boar, BOAR);
    assert_eq!(
        is_unit(&mut app, "target"),
        None,
        "another unit is not the target"
    );
    assert_eq!(
        is_unit(&mut app, "mouseover"),
        Some(1),
        "the token is itself"
    );
}

/// Once no unit wins the pick, `"mouseover"` names nobody: the publisher `0x492890` zeroes the pair
/// (`0x4928e8`, `0x4928f2`) and writes a null, corpse or GameObject guid, which the resolver
/// rejects as a unit (`0x515bca mov ecx,8`, `0x515bd9 je`). Empty ground, a corpse and a nearer
/// GameObject each clear it, in the frame the hover leaves the unit.
#[test]
fn the_mouseover_token_clears_when_no_unit_is_hovered() {
    use benilla_protocol::ObjectFields;

    let (mut app, wolf, _) = mouseover_app();
    let corpse = app
        .world_mut()
        .spawn(ObjectStore(ObjectFields::default()))
        .id();
    let chest = app
        .world_mut()
        .spawn(ObjectStore(ObjectFields::default()))
        .id();
    let named = |app: &mut App| {
        (
            eval(app, r#"return UnitExists("mouseover")"#),
            eval(app, r#"return UnitIsUnit("mouseover", "target")"#),
        )
    };
    let empty = Hovered::default();
    let on_corpse = Hovered {
        corpse: Some(corpse),
        corpse_guid: Some(0xF100_0000_0000_0003),
        distance: 10.0,
        ..Default::default()
    };
    let under_chest = Hovered {
        target: Some(wolf),
        guid: Some(WOLF),
        distance: 10.0,
        ..Default::default()
    };
    let chest_nearer = HoveredObject {
        target: Some(chest),
        guid: Some(0xF110_0000_0000_0004),
        distance: 5.0,
    };
    for (leave, object, what) in [
        (empty, HoveredObject::default(), "empty ground"),
        (on_corpse, HoveredObject::default(), "a corpse"),
        (under_chest, chest_nearer, "a nearer GameObject"),
    ] {
        hover_unit(&mut app, wolf, WOLF);
        assert_eq!(
            named(&mut app),
            (Some(1), Some(1)),
            "over the wolf, before {what}"
        );
        hover(&mut app, leave, object);
        assert_eq!(named(&mut app), (None, None), "after {what}");
    }
}

/// A quest panel that opens this frame is hoverable in its tick.
#[test]
fn the_spell_feed_runs_after_the_quest_feed() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    assert!(crate::test_support::runs_before(
        &mut app,
        crate::ui_quest::feed_quest,
        feed_spell_tooltips
    ));
}

/// The pet bar, Beast Training and target-of-target hovers are whole on the first hover.
#[test]
fn the_feed_pushes_the_spells_the_vm_holds_before_a_hover() {
    use benilla_ui::script::{
        AuraState, CraftRecipe, CraftState, CraftTooltip, PetActionView, TradeSkillDifficulty,
    };
    let spell = |name: &str, description: &str| benilla_formats::SpellDisplay {
        name: name.into(),
        description: Some(description.into()),
        ..Default::default()
    };
    let catalog = std::collections::HashMap::from([
        (3110, spell("Firebolt", "Deals Fire damage.")),
        (17253, spell("Bite", "Bite the enemy.")),
        (589, spell("Shadow Word: Pain", "Shadow damage over time.")),
    ]);
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(catalog),
        ..Spells::empty_for_tests()
    })
    .insert_resource(NetCommands(tx))
    .init_resource::<Items>()
    .init_resource::<crate::net::GuidIndex>()
    .init_resource::<crate::spell::SpellModifiers>()
    .add_systems(Update, feed_spell_tooltips);

    let mut script = UiScript::new().unwrap();
    script.set_pet_actions(
        true,
        true,
        true,
        vec![PetActionView {
            name: Some("Firebolt".into()),
            spell_id: Some(3110),
            ..Default::default()
        }],
    );
    script.set_craft(Some(CraftState {
        name: "Beast Training".into(),
        rank: 0,
        max_rank: 0,
        craft_type: 1,
        recipes: vec![CraftRecipe {
            spell_id: 24599,
            tooltip: CraftTooltip::Spell(17253),
            name: "Bite".into(),
            sub_name: String::new(),
            difficulty: TradeSkillDifficulty::Optimal,
            num_available: 1,
            icon: None,
            description: None,
            needs_item_target: false,
            reagents: vec![],
            tools: vec![],
            spell_level: 0,
        }],
    }));
    // We target a mob that targets party1, whose list the VM holds by guid.
    const ME: u64 = 0x10;
    const MOB: u64 = 0xF130_0000_0000_0001;
    const TOT: u64 = 0x21;
    script.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: ME,
        target: MOB,
        party: [TOT, 0, 0, 0],
        held: std::collections::HashMap::from([(ME, MOB), (MOB, TOT), (TOT, 0)]),
        ..Default::default()
    });
    script.set_unit_auras(
        TOT,
        Some(vec![AuraState {
            spell_id: 589,
            name: Some("Shadow Word: Pain".into()),
            ..Default::default()
        }]),
    );
    app.insert_non_send_resource(script);
    app.update();

    let script = app.world().non_send_resource::<UiScript>();
    script
        .run(
            r#"
            local a = CreateFrame("Button", "B"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
            CreateFrame("GameTooltip", "TT")
            local function lines()
                local t = {}
                for i = 1, TT:NumLines() do t[i] = getglobal("TTTextLeft" .. i):GetText() end
                return table.concat(t, " | ")
            end
            TT:SetOwner(B, "ANCHOR_RIGHT"); TT:SetPetAction(1); PET = lines()
            TT:SetOwner(B, "ANCHOR_RIGHT"); TT:SetCraftSpell(1); CRAFT = lines()
            TT:SetOwner(B, "ANCHOR_RIGHT"); TT:SetUnitDebuff("targettarget", 1); TOT = lines()
            "#,
        )
        .unwrap();
    assert_eq!(
        script.eval::<String>("return PET").unwrap(),
        "Firebolt | Deals Fire damage."
    );
    assert_eq!(
        script.eval::<String>("return CRAFT").unwrap(),
        "Bite | Bite the enemy."
    );
    assert_eq!(
        script.eval::<String>("return TOT").unwrap(),
        "Shadow Word: Pain | Shadow damage over time."
    );
}

/// The feed builds the pet's bar and book against the pet (the charm, else the summon), keeps the
/// player's book on the player, and rebuilds only the pet views when the pet's level moves.
#[test]
fn the_feed_builds_the_pet_views_against_the_pet_and_rebuilds_them_on_its_level() {
    use benilla_protocol::ObjectFields;
    use benilla_ui::script::{PetActionView, PetBookState, SpellBookState, SpellSlotView};
    const ME: u64 = 0x10;
    const IMP: u64 = 0xF140_0000_0000_0077;
    // `$s1` is 10 plus 1 a level, uncapped: the player, in no line of its own, reads 10.
    let pact = benilla_formats::SpellDisplay {
        id: 6307,
        name: "Blood Pact".into(),
        description: Some("Stamina by $s1.".into()),
        effect_base_points: [9, 0, 0],
        effect_base_dice: [1, 0, 0],
        effect_die_sides: [1, 0, 0],
        effect_real_points_per_level: [1.0, 0.0, 0.0],
        ..Default::default()
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(Spells {
        catalog: benilla_formats::SpellCatalog::from_displays([(6307, pact)].into()),
        ..Spells::empty_for_tests()
    })
    .insert_resource(NetCommands(tx))
    .init_resource::<Items>()
    .init_resource::<crate::net::GuidIndex>()
    .init_resource::<crate::spell::SpellModifiers>()
    .add_systems(Update, feed_spell_tooltips);
    // The player summons the imp (`UNIT_FIELD_SUMMON`, 8-9).
    let me = app
        .world_mut()
        .spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (8, IMP as u32),
                (9, (IMP >> 32) as u32),
                (34, 60),
            ])),
        ))
        .id();
    let imp = app
        .world_mut()
        .spawn(ObjectStore(ObjectFields::from_pairs(&[(34, 14)])))
        .id();
    let mut index = app.world_mut().resource_mut::<crate::net::GuidIndex>();
    index.0.insert(ME, me);
    index.0.insert(IMP, imp);

    let mut script = UiScript::new().unwrap();
    script.set_pet_actions(
        true,
        true,
        true,
        vec![PetActionView {
            name: Some("Blood Pact".into()),
            spell_id: Some(6307),
            ..Default::default()
        }],
    );
    let slot = SpellSlotView {
        spell_id: 6307,
        name: "Blood Pact".into(),
        ..Default::default()
    };
    script.set_pet_book(PetBookState {
        token: Some("DEMON".into()),
        slots: vec![slot.clone()],
    });
    script.set_spellbook(SpellBookState {
        tabs: Vec::new(),
        slots: vec![slot],
    });
    script
        .run(
            r#"
            local a = CreateFrame("Button", "B"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
            CreateFrame("GameTooltip", "TT")
            function DESC(set) TT:SetOwner(B, "ANCHOR_RIGHT"); set(); return TTTextLeft2:GetText() end
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    // The feed runs over the pet bar and book; the player's book's view is an ask on its hover.
    app.update();
    let descs = |app: &mut App| {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        let d = script
            .eval::<(String, String, Option<String>)>(
                r#"return DESC(function() TT:SetPetAction(1) end),
                    DESC(function() TT:SetSpell(1, "pet") end),
                    DESC(function() TT:SetSpell(1, "spell") end)"#,
            )
            .unwrap();
        assert!(script.take_errors().is_empty());
        d
    };
    let _ = descs(&mut app);
    app.update();
    assert_eq!(
        descs(&mut app),
        (
            "Stamina by 24.".to_string(),
            "Stamina by 24.".to_string(),
            Some("Stamina by 10.".to_string())
        ),
        "a level-14 imp's bar and book, and the player's own book"
    );
    // The imp dings: its views rebuild; the player's does not move.
    app.world_mut()
        .entity_mut(imp)
        .insert(ObjectStore(ObjectFields::from_pairs(&[(34, 15)])));
    app.update();
    assert_eq!(
        descs(&mut app),
        (
            "Stamina by 25.".to_string(),
            "Stamina by 25.".to_string(),
            Some("Stamina by 10.".to_string())
        )
    );
}
