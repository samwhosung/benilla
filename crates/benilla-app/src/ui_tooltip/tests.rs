//! The spell-view cell tests, against the REAL 5875 data (the module doc's line laws; moved
//! out of `mod.rs` whole at the 1000-line seam — same tests, file module).

use super::*;
use crate::ui_action::Spells;

/// A view context with no player state — the DBC-only half of the builder (the shape the
/// pre-0616 test used). `sub_classes` is threaded in by the caller when the case needs it.
struct TestCtx {
    items: Items,
    commands: NetCommands,
    _rx: crossbeam_channel::Receiver<crate::net::ClientCommand>,
    /// The two lookups the builder resolves through, over the **shipped** `GlobalStrings.lua` in
    /// a VM this harness owns.
    ///
    /// Every cell this builder composes is a key resolved at runtime (decision 2045), so a test
    /// that asserts a rendered cell has to grade it against the player's own table — a stub would
    /// pass on wording the client never shows, which is the trap 2052 named when it moved the
    /// glue tests onto the loader's own assembly.
    get: Box<Getter>,
    text: Box<Filler>,
    /// Empty tables, which is the un-talented character every cell here is graded as: the cost
    /// cell's modifier hop must be the identity when nothing has been sent.
    spell_mods: crate::spell::SpellModifiers,
}

/// The two lookup shapes, named so the harness's fields read.
type Getter = dyn Fn(&str) -> Option<String>;
type Filler = dyn Fn(&str, &[i64]) -> Option<String>;

impl TestCtx {
    fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let vm = std::rc::Rc::new(benilla_ui::script::UiScript::new().expect("VM"));
        crate::ui_script::load_ui_for_test(&vm, "Interface\\FrameXML\\GlobalStrings.lua");
        let (for_get, for_text) = (vm.clone(), vm);
        Self {
            items: Items::default(),
            commands: NetCommands(tx),
            _rx: rx,
            get: Box::new(move |key| benilla_ui::strings::global(for_get.lua(), key)),
            spell_mods: crate::spell::SpellModifiers::default(),
            text: Box::new(move |key, args: &[i64]| {
                let template = benilla_ui::strings::global(for_text.lua(), key)?;
                let args: Vec<_> = args
                    .iter()
                    .map(|n| benilla_ui::strings::Arg::D(*n))
                    .collect();
                Some(benilla_ui::strings::fill(&template, &args))
            }),
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

    /// The same context with an auto-attack target engaged — the melee range arm's second
    /// reach, which `0x6e3480` resolves out of `[caster+0xc48]` rather than taking as an
    /// argument.
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

    fn ctx_for<'a, 'w, 's>(
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
            combat_reach: store.map_or(1.5, |s| s.0.unit_combat_reach()),
            // The tests drive the melee arm through `combat_reach` alone; the engaged-target
            // reach has its own case in `range_cell_on_real_data`.
            attack_target_reach: None,
            objects,
            items: &mut self.items,
            commands: &self.commands,
            sub_classes,
            spell_mods: &self.spell_mods,
            get: self.get.as_ref(),
            text: self.text.as_ref(),
        }
    }
}

/// The object index the possession cells resolve through — nothing streamed, which is what
/// every case here is graded against: the worn-item search finds no instance and each reagent's
/// carried count reads 0 (decision 2334). Held beside the [`TestCtx`] because the lookup borrows
/// the world it reads.
fn no_objects() -> crate::ui_items::TestObjects {
    crate::ui_items::TestObjects::new()
}

/// A player descriptor with nothing worn and nothing in the bags — the "owns none of it"
/// pole of both possession tests.
fn empty_player() -> ObjectStore {
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
        22u16, 100u32,
    )]))
}

/// The full spell-tooltip view off the REAL 5875 data — Fireball rank 1 (133) end to end:
/// the pinned columns (description 138, cast index 18→1500 ms, duration 30), the token
/// engine's byte formulas, and the view's verified cell shapes. Skips without client data.
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

    // Charge rank 1 (100) — the director's reference shot, end to end: the dual-bound range
    // row (SpellRange 95 = {8, 25}), the CATEGORY-column cooldown (recoveryTime 0 /
    // categoryRecoveryTime 15000), and the Stances-mask form line (0x10000 → form 17).
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

    // 1483 — a PERMISSIVE Stances mask prints no line at all. 5875 overloads the column: with
    // `AttributesEx2` bit 19 set the bits say "may ALSO be cast in these forms", so reading them
    // as a requirement invented a red "Requires Shadowform" on Inner Fire / Psychic Scream and
    // "Requires Spirit of Redemption" on Flash Heal. Bit 27 = form 28, bit 31 = form 32 — the
    // names resolve, which is exactly why the bug looked plausible.
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

/// The cost and cast cells' full law on the REAL 5875 data (decision 1074, B192): the health
/// fallback and pct resolution (Bloodrage), the empty Life Tap cell (1.12 carries no cost
/// columns for it — the trade lives in the description), the `_PER_TIME` composite (Health
/// Funnel), the resolved pct-of-base-mana with no percentage line (Judgement — the B152
/// reframe), and the cast ladder's attr arms (Next melee / Attack speed / Channeled) with
/// the mana-keyed Instant fork. Skips without client data.
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
    // A level-60 warrior-shaped store: max health 4000, base mana 1000 (field indices are
    // the protocol crate's: health 22, maxhealth 28, level 34, base mana 162).
    let store = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (22u16, 3500u32),
        (28, 4000),
        (34, 60),
        (162, 1000),
    ]));

    // Bloodrage (2687): pct-ONLY health cost — 20% of MAX health resolves to a flat number
    // through the health fallback; never a percentage line. Instant on a non-mana type reads
    // bare "Instant" whatever it costs.
    let v = spell_tooltip_view(
        2687,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Bloodrage view");
    assert_eq!(v.cost.as_deref(), Some("800 Health"), "20% of 4000");
    assert_eq!(v.cast_time.as_deref(), Some("Instant"));

    // Life Tap (1454): the 5875 file carries NO cost columns for any rank — the cell is
    // empty, exactly the reference render (the printed health cost is 2.x's change).
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

    // Health Funnel (755): the `_PER_TIME` composite in the health lane, and the channeled
    // cast cell.
    let v = spell_tooltip_view(
        755,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Health Funnel view");
    assert_eq!(v.cost.as_deref(), Some("11 Health, plus 5 per sec"));
    assert_eq!(v.cast_time.as_deref(), Some("Channeled"));

    // Judgement (20271): pct-of-base-mana resolves to its flat number — the line B152
    // reported as "% of base mana" never exists on the reference. DBC-only (no store)
    // degrades to the flat cost: none here.
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

    // Heroic Strike (78): the cost cell keeps its rage (wire 150 ÷ 10) and "Next melee"
    // moves to the CAST cell where the ref's ladder puts it.
    let v = spell_tooltip_view(78, &spells, &mut t.ctx_for(&objects, 0, None, Some(&store)))
        .expect("Heroic Strike view");
    assert_eq!(v.cost.as_deref(), Some("15 Rage"));
    assert_eq!(v.cast_time.as_deref(), Some("Next melee"));

    // Throw (2764) and Auto Shot (75): the ranged bit reads "Attack speed" — and for Throw
    // the bit is ALONE (`Attributes & 0x2`, not the auto-repeat or-pair). Melee Attack
    // (6603) is the cast line's skip at `0x52eb3c`: Effect[0] == ATTACK omits the whole line.
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

    // Mind Flay (15407): a channeled MANA spell — the cost cell and the channeled cell
    // together.
    let v = spell_tooltip_view(
        15407,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Mind Flay view");
    assert_eq!(v.cost.as_deref(), Some("45 Mana"));
    assert_eq!(v.cast_time.as_deref(), Some("Channeled"));
}

/// The RANGE cell's whole law (`[0x52e9a2, 0x52ea8c)`) on the REAL 5875 data: the melee family
/// renders through `SPELL_RANGE` off the caster's own combat reach — the
/// invented "Melee Range" decision 2080 named is gone — the authored rows print their own numbers
/// through the same key, and the on-next-swing class and the self-only rows print no cell at all.
/// Skips without client data.
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

    // Sinister Strike (1752) sits on SpellRange row 2 "Combat Range" — the ONE shipped row with
    // flags bit 0. 1.5 + 1.5 + 1.3333334 = 4.333 does not clear the 5.0 floor, so the cell reads
    // the floor: the melee family's normal render is a NUMBER, not a word.
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

    // …and it MOVES with the caster's reach, which is the whole reason the cell needs a store:
    // 4.0 + 4.0 + 1.3333334 = 9.333 clears the floor and `fistp` rounds it to 9.
    let big = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
        130u16,
        4.0f32.to_bits(),
    )]));
    let v = spell_tooltip_view(1752, &spells, &mut t.ctx_for(&objects, 0, None, Some(&big)))
        .expect("Sinister Strike view");
    assert_eq!(v.range.as_deref(), Some("9 yd range"));

    // …and the second reach is the AUTO-ATTACK target's, not the caster's doubled: swinging at
    // a 4.0-reach mob with a default 1.5 body reads 1.5 + 4.0 + 1.3333334 = 6.833 -> 7.
    let v = spell_tooltip_view(
        1752,
        &spells,
        &mut t.ctx_engaged(&objects, Some(&store), 4.0),
    )
    .expect("Sinister Strike view");
    assert_eq!(v.range.as_deref(), Some("7 yd range"));

    // An authored single-number row: Fireball's 0–35.
    let v = spell_tooltip_view(
        133,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&store)),
    )
    .expect("Fireball view");
    assert_eq!(v.range.as_deref(), Some("35 yd range"));

    // An authored PAIR (the `"%d-%d"` nested fill): Charge's 8–25, unpadded — the tooltip's own
    // `GetMinMaxRange` call passes `target = NULL`, so the reach never joins these two.
    let v = spell_tooltip_view(100, &spells, &mut t.ctx_for(&objects, 0, None, Some(&big)))
        .expect("Charge view");
    assert_eq!(v.range.as_deref(), Some("8-25 yd range"));

    // The two absences. Heroic Strike (78) carries the on-next-swing pair, which jumps the cell
    // before the resolver runs — it does NOT read "Melee Range", and it does not read 5 yd
    // either. Bloodrage (2687) sits on the self row, whose resolved max is 0.
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

/// The three lines the 2026-07-25 reference captures pinned (decision 0620), each against the
/// REAL 5875 data. Skips without client data.
#[test]
fn the_pinned_c6_lines_on_real_data() {
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

    // 1 · The wand Shoot (5019, class 2 / submask bit 19) — "Requires Wands", red with no
    // wand worn. The same row feeds the cast-fail line's SINGULAR "Wand" (see `cast_fail`).
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
    // A multi-bit mask is named by ItemSubClassMask.dbc, not skipped (`0x52eef0` — we
    // printed nothing here until `0x6e2380` was decoded): Parry's 0x2a5f3 is exactly the eleven
    // melee subclasses, which that table names in one word.
    let parry = spells.catalog.get(3127).expect("Parry 3127");
    assert!(parry.equipped_item_subclass_mask.count_ones() > 1);
    let v = spell_tooltip_view(
        3127,
        &spells,
        &mut t.ctx_for(&objects, 0, Some(&subs), Some(&store)),
    )
    .expect("Parry view");
    assert_eq!(v.requires_item.as_deref(), Some("Requires Melee Weapon"));

    // 2 · Attack (6603) — `Effect[0] == 78` omits the cast|cooldown line WHOLE, even though
    // `Attributes & 0x40` is clear. Before the cast-line gate `0x52eb15` widened, this read
    // "Instant".
    let d = spells.catalog.get(6603).expect("Attack 6603");
    assert_eq!(d.effects[0], 78, "SPELL_EFFECT_ATTACK");
    assert!(!d.passive, "6603 carries Attributes 0x10, not 0x40");
    let v = spell_tooltip_view(6603, &spells, &mut t.ctx(&objects, 0, None)).expect("Attack view");
    assert_eq!(v.cast_time, None, "the law's Effect[0] gate");
    // …and the chance line the same Effect[0] selects (`[0x52f5b1, 0x52f697)`). ATTACK
    // BYPASSES the passive gate, which is the whole reason a non-passive Attack shows a crit
    // line at all. No descriptor = no line; the percentages are already percents on the wire.
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
    // A spell naming none of the four effects has no line at all.
    let v = spell_tooltip_view(
        133,
        &spells,
        &mut t.ctx_for(&objects, 0, None, Some(&rated)),
    )
    .expect("Fireball view");
    assert_eq!(v.chance, None);

    // 3 · Slow Fall (130) — "Reagents: Light Feather", inline-red while unowned (no store =
    // owns nothing). The name rides the ask-once item cache, seeded here as the server would.
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

/// The "Locked" line's colour law (decision 0770) — the director's report: a door they held
/// the key for read RED, where the reference reads green.
///
/// The builder's own shape is "red unless the resolver found an opener", and *every* kind of
/// opener lands on the same green — so the mapping is a one-way test on `Unmet`, not a
/// per-arm table. A flag-locked object with no `Lock.dbc` row is the reference's
/// no-requirement arm and is green too: the flag alone never means "you can't".
#[test]
fn the_locked_line_greens_when_the_lock_can_be_opened() {
    use crate::target::lock::LockOutcome;

    // The report: the Scarlet Key in hand, the Armory Door in front of you.
    assert_eq!(
        locked_line_tint(Some(LockOutcome::OpenByKey(7146))),
        TooltipTint::LockOpen,
        "holding the key must read green"
    );
    // The same door without the key — unchanged, and the control for the fix.
    assert_eq!(
        locked_line_tint(Some(LockOutcome::Unmet)),
        TooltipTint::Red,
        "no key still reads red"
    );
    // A skill opener you know: green. (The reference would ramp this by margin; green is that
    // ramp's comfortable rung — see `locked_line_tint`'s note.)
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
