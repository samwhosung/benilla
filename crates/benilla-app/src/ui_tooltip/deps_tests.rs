//! The feed's dependency classification against the real `Spell.dbc`.
//!
//! A pushed view is re-queued when [`Deps::hits`] finds an input it read among the [`Changes`]. A
//! spell missing a dependency would keep a stale tooltip, so the safety net builds every spell's
//! view, moves inputs, builds it again, and holds that a view which moved was re-queued. Two kinds
//! of move: each cause alone, through the real diff ([`Seen::changes_since`], the modifier and
//! reagent diffs), and, per view, every input the view's recorded reads leave out, all at once,
//! which must leave it exactly as it was.

use std::collections::BTreeSet;

use benilla_protocol::ObjectFields;
use benilla_ui::script::SpellTooltipView;

use super::spell_deps::{Changes, Deps, Reagents, Seen};
use super::spell_feed::{build_view, reagent_state, ViewCaster};
use super::tests::{real_spells, TestCtx};
use crate::net::{ObjectStore, Objects};
use crate::spell::usable::{worn_slots_read, WORN_SLOTS};
use crate::spell::{ModsDiff, SpellModifiers};
use crate::ui_action::Spells;
use crate::ui_items::TestObjects;

/// One side of each input group, so that moving an input is an xor: `false` is the empty side (no
/// bind point, form 0, no percentages, default reach, nothing worn, no skills, zero cells), `true`
/// the populated one.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
struct Inputs {
    home: bool,
    form: bool,
    /// A `Chance` bit each: block, dodge, parry, crit.
    avoid: u8,
    reach: bool,
    /// The worn slots holding an item that meets the view's requirement.
    worn: u32,
    /// Bit `i`: the skill line `lines[i]` at 300.
    lines: u128,
    /// The `SpellFamilyFlags` bits whose cells hold a modifier.
    bits: u64,
}

impl Inputs {
    const ALL: Inputs = Inputs {
        home: true,
        form: true,
        avoid: 0xf,
        reach: true,
        worn: WORN_SLOTS,
        lines: u128::MAX,
        bits: u64::MAX,
    };

    fn xor(self, o: Inputs) -> Inputs {
        Inputs {
            home: self.home ^ o.home,
            form: self.form ^ o.form,
            avoid: self.avoid ^ o.avoid,
            reach: self.reach ^ o.reach,
            worn: self.worn ^ o.worn,
            lines: self.lines ^ o.lines,
            bits: self.bits ^ o.bits,
        }
    }

    /// Each input in `self` alone, named: what to move to find which one moved a view.
    fn parts(self, layout: &Layout) -> Vec<(String, Inputs)> {
        let alone = Inputs::default();
        let mut parts = Vec::new();
        if self.home {
            parts.push((
                "the bind point".to_string(),
                Inputs {
                    home: true,
                    ..alone
                },
            ));
        }
        if self.form {
            parts.push((
                "the form".to_string(),
                Inputs {
                    form: true,
                    ..alone
                },
            ));
        }
        for (i, which) in ["block", "dodge", "parry", "crit"].iter().enumerate() {
            if self.avoid >> i & 1 == 1 {
                let only = Inputs {
                    avoid: 1 << i,
                    ..alone
                };
                parts.push((format!("the {which} percentage"), only));
            }
        }
        if self.reach {
            parts.push((
                "a combat reach".to_string(),
                Inputs {
                    reach: true,
                    ..alone
                },
            ));
        }
        for slot in (0..19).filter(|s| self.worn >> s & 1 == 1) {
            let only = Inputs {
                worn: 1 << slot,
                ..alone
            };
            parts.push((format!("worn slot {slot}"), only));
        }
        for (i, line) in layout.lines.iter().enumerate() {
            if self.lines >> i & 1 == 1 {
                parts.push((
                    format!("skill line {line}"),
                    Inputs {
                        lines: 1 << i,
                        ..alone
                    },
                ));
            }
        }
        for bit in (0..64).filter(|b| self.bits >> b & 1 == 1) {
            let only = Inputs {
                bits: 1 << bit,
                ..alone
            };
            parts.push((format!("the modifier cells of family-flag bit {bit}"), only));
        }
        parts
    }

    /// The inputs a view's recorded reads leave out.
    fn outside(deps: &Deps, layout: &Layout) -> Inputs {
        Inputs {
            home: !deps.home,
            form: !deps.form,
            avoid: !deps.avoidance & 0xf,
            reach: !deps.reach,
            worn: WORN_SLOTS & !deps.worn,
            lines: layout
                .lines
                .iter()
                .enumerate()
                .filter(|(_, line)| !deps.skill_lines.contains(line))
                .fold(0, |mask, (i, _)| mask | 1 << i),
            bits: !deps.mod_bits,
        }
    }
}

/// What a sweep holds fixed for a view: the form the form input puts on, the item kind worn
/// items are of, and the class family the tables gate on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Params {
    form: u8,
    kind: usize,
    family: u32,
}

/// The fixed shape of the scenarios.
struct Layout {
    /// The skill lines any spell belongs to, ascending; bit `i` of [`Inputs::lines`].
    lines: Vec<u32>,
    /// The `(class, subclass)` an item is of, per kind: what some spell's equipped-item search
    /// asks for.
    kinds: Vec<(u32, u32)>,
}

impl Layout {
    /// The item a slot holds in a scenario, one object per kind and slot.
    fn guid(kind: usize, slot: u32) -> u64 {
        0x1_0000 + ((kind as u64) << 8) + u64::from(slot)
    }

    fn entry(kind: usize) -> u32 {
        90_000 + kind as u32
    }

    /// The kind that meets `d`'s requirement: the lowest subclass its mask names.
    fn kind_of(&self, d: &benilla_formats::SpellDisplay) -> usize {
        let want = (
            d.equipped_item_class as u32,
            d.equipped_item_subclass_mask.trailing_zeros(),
        );
        self.kinds.iter().position(|&k| k == want).unwrap_or(0)
    }

    /// The parameters that put each input where it moves `d`'s view the most: the lowest form `d`
    /// names, the item kind it needs, its own family.
    fn params_of(&self, d: &benilla_formats::SpellDisplay) -> Params {
        Params {
            form: d.stances.trailing_zeros().min(31) as u8 + 1,
            kind: self.kind_of(d),
            family: d.spell_family.max(1),
        }
    }
}

/// A scenario made into the inputs a view is built from.
struct Mat {
    store: ObjectStore,
    mods: SpellModifiers,
    home: Option<&'static str>,
    target_reach: Option<f32>,
    form: u8,
}

fn mat(layout: &Layout, inputs: Inputs, p: Params) -> Mat {
    // Health, max health, level and base mana, which no input here moves.
    let mut pairs: Vec<(u16, u32)> = vec![(22, 3500), (28, 4000), (34, 60), (162, 1000)];
    let form = if inputs.form { p.form } else { 0 };
    if form != 0 {
        pairs.push((138, u32::from(form) << 16));
    }
    // Block, dodge, parry and crit percent.
    for (i, pct) in [7.5f32, 6.25, 5.5, 3.75].into_iter().enumerate() {
        if inputs.avoid >> i & 1 == 1 {
            pairs.push((1106 + i as u16, pct.to_bits()));
        }
    }
    if inputs.reach {
        pairs.push((130, 4.0f32.to_bits()));
    }
    let mut slot = 0;
    for (i, &line) in layout.lines.iter().enumerate() {
        if inputs.lines >> i & 1 == 1 {
            let base = 718 + 3 * slot;
            pairs.push((base, line));
            pairs.push((base + 1, 300 | 300 << 16));
            slot += 1;
        }
    }
    for s in (0..19).filter(|s| inputs.worn >> s & 1 == 1) {
        let guid = Layout::guid(p.kind, s);
        pairs.push((486 + 2 * s as u16, guid as u32));
        pairs.push((487 + 2 * s as u16, (guid >> 32) as u32));
    }
    let mut mods = SpellModifiers::default();
    mods.set_class_family(p.family);
    for bit in (0..64u8).filter(|b| inputs.bits >> b & 1 == 1) {
        for op in 0..29 {
            mods.set(true, bit, op, -7);
            mods.set(false, bit, op, 35);
        }
    }
    Mat {
        store: ObjectStore(ObjectFields::from_pairs(&pairs)),
        mods,
        home: inputs.home.then_some("Zzz"),
        // The two reaches move together: the caster's 1.5 to 4.0, and an auto-attack target at 3.0.
        target_reach: inputs.reach.then_some(3.0),
        form,
    }
}

/// What differs between two scenarios, through the feed's own diffs.
fn changes(before: &Mat, now: &Mat) -> Changes {
    Seen::of(Some(&now.store), now.home, now.target_reach)
        .changes_since(&Seen::of(
            Some(&before.store),
            before.home,
            before.target_reach,
        ))
        .with_mods(now.mods.diff(&before.mods))
}

/// The real data and the objects the scenarios put in the world.
struct Rig {
    spells: Spells,
    t: TestCtx,
    objs: TestObjects,
    ids: Vec<u32>,
    layout: Layout,
    /// The reagent entries any spell lists, ascending.
    reagents: Vec<u32>,
    families: Vec<u32>,
}

impl Rig {
    fn new() -> Option<Rig> {
        let spells = real_spells()?;
        let data = benilla_formats::wow_data_or_skip!(None);
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let skill_lines =
            benilla_formats::load_skill_line_catalog(&mut chain).expect("skill lines");
        let mut ids: Vec<u32> = spells.catalog.iter().map(|(id, _)| id).collect();
        ids.sort_unstable();
        let displays = || ids.iter().filter_map(|&id| spells.catalog.get(id));
        let lines: BTreeSet<u32> = ids
            .iter()
            .filter_map(|&id| skill_lines.spell_to_line(id))
            .collect();
        assert!(
            lines.len() <= 128,
            "the player's skill table holds 128 lines"
        );
        let mut layout = Layout {
            lines: lines.into_iter().collect(),
            kinds: vec![(2, 0)],
        };
        for d in displays().filter(|d| worn_slots_read(d) != 0) {
            let kind = (
                d.equipped_item_class as u32,
                d.equipped_item_subclass_mask.trailing_zeros(),
            );
            if !layout.kinds.contains(&kind) {
                layout.kinds.push(kind);
            }
        }
        let reagents: BTreeSet<u32> = displays()
            .flat_map(|d| d.reagents.iter().map(|&(entry, _)| entry))
            .filter(|&entry| entry != 0)
            .collect();
        let families: BTreeSet<u32> = displays()
            .map(|d| d.spell_family)
            .filter(|&f| f != 0)
            .collect();
        let mut t = TestCtx::new();
        t.skill_lines = Some(skill_lines);
        let mut objs = TestObjects::new();
        // One worn item per kind and slot, and each kind's template.
        for (kind, &(class, subclass)) in layout.kinds.iter().enumerate() {
            t.items.insert_template(
                Layout::entry(kind),
                Some(benilla_protocol::messages::ItemInfo {
                    class,
                    subclass,
                    ..crate::items::test_template("Worn")
                }),
            );
            for slot in 0..19 {
                objs.spawn(
                    Layout::guid(kind, slot),
                    ObjectFields::from_pairs(&[(3, Layout::entry(kind))]),
                );
            }
        }
        // One carried stack of five per reagent entry, for the scenarios that carry it.
        for (i, &entry) in reagents.iter().enumerate() {
            objs.spawn(
                0x2_0000 + i as u64,
                ObjectFields::from_pairs(&[(3, entry), (14, 5)]),
            );
        }
        Some(Rig {
            spells,
            t,
            objs,
            ids,
            layout,
            reagents: reagents.into_iter().collect(),
            families: families.into_iter().collect(),
        })
    }
}

fn build(
    t: &mut TestCtx,
    objects: &Objects,
    spells: &Spells,
    id: u32,
    m: &Mat,
) -> Option<(SpellTooltipView, Deps)> {
    t.spell_mods = m.mods.clone();
    let mut ctx = t.ctx_for(objects, m.form, None, Some(&m.store));
    ctx.home_area = m.home;
    ctx.attack_target_reach = m.target_reach;
    build_view(id, spells, &mut ctx)
}

/// A spell whose text names another spell's number (`$1234s1`), whose modifiers then answer to
/// that spell's family.
fn cross_references(d: &benilla_formats::SpellDisplay) -> bool {
    [d.description.as_deref(), d.aura_description.as_deref()]
        .into_iter()
        .flatten()
        .flat_map(|text| text.split('$').skip(1))
        .any(|token| {
            let after_scale = match token.split_once(';') {
                Some((scale, rest)) if scale.starts_with(['/', '*']) => rest,
                _ => token,
            };
            after_scale.starts_with(|c: char| c.is_ascii_digit())
        })
}

/// What a sweep found.
#[derive(Default)]
struct Sweep {
    /// The views that moved.
    moved: usize,
    /// The views the changes re-queue, moved or not.
    requeued: usize,
    /// One line per moved view its reads do not cover.
    misses: Vec<String>,
}

type Only<'a> = &'a dyn Fn(&benilla_formats::SpellDisplay) -> bool;
type ParamsOf<'a> = &'a dyn Fn(&Layout, &benilla_formats::SpellDisplay) -> Params;
type FlipOf<'a> = &'a dyn Fn(&Layout, &Deps) -> Inputs;

/// Build every view `only` names at `base`, move the inputs `flip` names for it, build it again,
/// and hold that a view which moved is one the changes re-queue.
fn sweep(rig: &mut Rig, base: Inputs, only: Only, params: ParamsOf, flip: FlipOf) -> Sweep {
    let Rig {
        spells,
        t,
        objs,
        ids,
        layout,
        ..
    } = rig;
    let objects = objs.get();
    let mut out = Sweep::default();
    let mut before: Option<(Params, Mat)> = None;
    let mut after: Option<((Params, Inputs), Mat, Changes)> = None;
    for &id in ids.iter() {
        let d = spells.catalog.get(id).expect("listed spell");
        if !only(d) {
            continue;
        }
        let p = params(layout, d);
        if before.as_ref().map(|(k, _)| k) != Some(&p) {
            before = Some((p, mat(layout, base, p)));
        }
        let (_, before) = before.as_ref().expect("set above");
        let Some((view, deps)) = build(t, &objects, spells, id, before) else {
            continue;
        };
        let moved = flip(layout, &deps);
        if after.as_ref().map(|(k, ..)| k) != Some(&(p, moved)) {
            let now = mat(layout, base.xor(moved), p);
            let c = changes(before, &now);
            after = Some(((p, moved), now, c));
        }
        let (_, now, changes) = after.as_ref().expect("set above");
        let (moved_view, _) = build(t, &objects, spells, id, now).expect("built before");
        let hit = deps.hits(changes);
        out.requeued += usize::from(hit);
        if moved_view == view {
            continue;
        }
        out.moved += 1;
        if hit {
            continue;
        }
        // Name what moved it: each input alone.
        let culprits: Vec<String> = moved
            .parts(layout)
            .into_iter()
            .filter(|(_, part)| {
                let alone = mat(layout, base.xor(*part), p);
                build(t, &objects, spells, id, &alone).is_some_and(|(v, _)| v != view)
            })
            .map(|(name, _)| name)
            .collect();
        out.misses.push(format!(
            "spell {id} {:?} moved with {}, which its reads {deps:?} do not cover",
            d.name,
            if culprits.is_empty() {
                "these inputs together".to_string()
            } else {
                culprits.join(", ")
            }
        ));
    }
    out
}

fn assert_covered(what: &str, sweep: &Sweep) {
    assert!(
        sweep.misses.is_empty(),
        "{what}: {} of {} moved views are not re-queued; first:\n{}",
        sweep.misses.len(),
        sweep.moved,
        sweep
            .misses
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn any(_: &benilla_formats::SpellDisplay) -> bool {
    true
}

fn own(layout: &Layout, d: &benilla_formats::SpellDisplay) -> Params {
    layout.params_of(d)
}

/// Every view that moves with an input is one the feed re-queues for that input: each cause alone
/// against the real spell set, and, per view, everything its reads leave out.
#[test]
fn every_view_that_moves_with_an_input_is_requeued_by_that_input() {
    let Some(mut rig) = Rig::new() else { return };
    let total = rig.ids.len();
    let mut report = Vec::new();
    let mut row = |what: &str, s: &Sweep| {
        report.push(format!(
            "{what:<28} moves {:>5}  re-queues {:>5} of {total}",
            s.moved, s.requeued
        ));
    };

    // Each cause alone, from the empty side to the populated one.
    let empty = Inputs::default();
    let cause = |inputs: Inputs| move |_: &Layout, _: &Deps| inputs;
    let s = sweep(
        &mut rig,
        empty,
        &any,
        &own,
        &cause(Inputs {
            home: true,
            ..empty
        }),
    );
    assert_covered("the bind point", &s);
    assert!(s.moved >= 1, "no view reads the bind point");
    row("home bind ($z)", &s);
    let s = sweep(
        &mut rig,
        empty,
        &any,
        &own,
        &cause(Inputs {
            form: true,
            ..empty
        }),
    );
    assert_covered("the form", &s);
    assert!(s.moved >= 100, "only {} views read the form", s.moved);
    row("form", &s);
    for (i, name) in ["block", "dodge", "parry", "crit"].into_iter().enumerate() {
        let inputs = Inputs {
            avoid: 1 << i,
            ..empty
        };
        let s = sweep(&mut rig, empty, &any, &own, &cause(inputs));
        assert_covered(name, &s);
        assert!(s.moved >= 1, "no view reads the {name} percentage");
        row(name, &s);
    }
    let s = sweep(
        &mut rig,
        empty,
        &any,
        &own,
        &cause(Inputs {
            reach: true,
            ..empty
        }),
    );
    assert_covered("the combat reaches", &s);
    assert!(s.moved >= 100, "only {} views read a reach", s.moved);
    row("combat reach", &s);
    let all_worn = Inputs {
        worn: WORN_SLOTS,
        ..empty
    };
    let s = sweep(&mut rig, empty, &any, &own, &cause(all_worn));
    assert_covered("the worn set", &s);
    assert!(s.moved >= 20, "only {} views read the worn set", s.moved);
    row("worn set (all slots)", &s);
    for slot in 0..19 {
        let inputs = Inputs {
            worn: 1 << slot,
            ..empty
        };
        let searching = |d: &benilla_formats::SpellDisplay| worn_slots_read(d) != 0;
        let s = sweep(&mut rig, empty, &searching, &own, &cause(inputs));
        assert_covered(&format!("worn slot {slot}"), &s);
    }
    let all_lines = Inputs {
        lines: u128::MAX,
        ..empty
    };
    let s = sweep(&mut rig, empty, &any, &own, &cause(all_lines));
    assert_covered("the skills", &s);
    assert!(s.moved >= 100, "only {} views read a skill", s.moved);
    row("skills (all lines)", &s);
    // The tables, per class family: the family gates which spells they reach.
    let families = rig.families.clone();
    let mut mods = Sweep::default();
    for &family in &families {
        let params = move |_: &Layout, _: &benilla_formats::SpellDisplay| Params {
            form: 1,
            kind: 0,
            family,
        };
        let inputs = Inputs {
            bits: u64::MAX,
            ..empty
        };
        let s = sweep(&mut rig, empty, &any, &params, &cause(inputs));
        assert_covered(&format!("the modifiers of family {family}"), &s);
        mods.moved += s.moved;
        mods.requeued += s.requeued;
    }
    assert!(
        mods.moved >= 500,
        "only {} views read the tables",
        mods.moved
    );
    row("modifier tables (all bits)", &mods);

    // Everything a view's reads leave out at once, from either side: it must not move. Moving the
    // inputs it does read instead moves it, so the moves are not inert.
    let outside = |layout: &Layout, deps: &Deps| Inputs::outside(deps, layout);
    let inside = |layout: &Layout, deps: &Deps| Inputs::ALL.xor(Inputs::outside(deps, layout));
    let s = sweep(&mut rig, empty, &any, &own, &inside);
    assert_covered("everything a view reads", &s);
    assert!(
        s.moved >= 3000,
        "only {} views moved with what they read",
        s.moved
    );
    row("everything a view reads", &s);
    for (name, base) in [("from empty", empty), ("from populated", Inputs::ALL)] {
        let s = sweep(&mut rig, base, &any, &own, &outside);
        assert_covered(&format!("everything a view does not read, {name}"), &s);
        // A cross-referenced spell's family owns its modifiers: try every family.
        for &family in &families {
            let params = move |layout: &Layout, d: &benilla_formats::SpellDisplay| Params {
                family,
                ..layout.params_of(d)
            };
            let s = sweep(&mut rig, base, &cross_references, &params, &outside);
            let what = format!("everything a view does not read, {name}, family {family}");
            assert_covered(&what, &s);
        }
    }
    eprintln!("{}", report.join("\n"));
}

/// The reagents line: a name streaming in re-queues every view that lists a reagent, and a count
/// moving re-queues only the spells that list the entry.
#[test]
fn a_reagent_moving_requeues_the_views_that_list_it() {
    let Some(mut rig) = Rig::new() else { return };
    let Rig {
        spells,
        t,
        objs,
        ids,
        reagents,
        ..
    } = &mut rig;
    let listing: Vec<u32> = ids
        .iter()
        .copied()
        .filter(|&id| {
            spells
                .catalog
                .get(id)
                .is_some_and(|d| d.reagents.iter().any(|&(entry, _)| entry != 0))
        })
        .collect();
    assert!(
        listing.len() >= 100,
        "{} spells list a reagent",
        listing.len()
    );
    let objects = objs.get();
    let states = |t: &mut TestCtx, store: &ObjectStore| -> Reagents {
        reagents
            .iter()
            .map(|&e| {
                let state = reagent_state(e, Some(store), &objects, &mut t.items, &t.commands);
                (e, state)
            })
            .collect()
    };
    let mods = SpellModifiers::default();
    let mat_of = |store: ObjectStore| Mat {
        store,
        mods: mods.clone(),
        home: None,
        target_reach: None,
        form: 0,
    };

    // The names stream in: every reagent line appears, and each view that has one is due.
    let empty = mat_of(ObjectStore(ObjectFields::from_pairs(&[(22, 100)])));
    let unnamed = states(t, &empty.store);
    let before: Vec<_> = listing
        .iter()
        .map(|&id| build(t, &objects, spells, id, &empty).expect("built"))
        .collect();
    for (i, &entry) in reagents.iter().enumerate() {
        let template = crate::items::test_template(&format!("Reagent {i}"));
        t.items.insert_template(entry, Some(template));
    }
    let named = states(t, &empty.store);
    let changes = Changes::default().with_reagents(&unnamed, &named);
    let mut moved = 0;
    for (&id, (view, deps)) in listing.iter().zip(&before) {
        let (now, _) = build(t, &objects, spells, id, &empty).expect("built");
        if now != *view {
            moved += 1;
            assert!(
                deps.hits(&changes),
                "spell {id} gained its reagent names unheeded: {deps:?}"
            );
        }
    }
    assert!(moved >= 100, "only {moved} views showed a reagent name");

    // A stack of each entry moves in, sixteen at a time: only the spells listing one are due.
    let plain: Vec<_> = listing
        .iter()
        .map(|&id| build(t, &objects, spells, id, &empty).expect("built"))
        .collect();
    let mut moved = 0;
    for (group, entries) in reagents.chunks(16).enumerate() {
        let mut pairs: Vec<(u16, u32)> = vec![(22, 100)];
        for slot in 0..entries.len() {
            let guid = 0x2_0000 + (group * 16 + slot) as u64;
            pairs.push((532 + 2 * slot as u16, guid as u32));
            pairs.push((533 + 2 * slot as u16, (guid >> 32) as u32));
        }
        let carrying = mat_of(ObjectStore(ObjectFields::from_pairs(&pairs)));
        let changes = Changes::default().with_reagents(&named, &states(t, &carrying.store));
        assert!(!changes.is_empty());
        for (&id, (view, deps)) in listing.iter().zip(&plain) {
            let (now, _) = build(t, &objects, spells, id, &carrying).expect("built");
            let lists = deps.reagents().iter().any(|e| entries.contains(e));
            assert_eq!(deps.hits(&changes), lists, "spell {id}: {deps:?}");
            if now != *view {
                moved += 1;
                assert!(lists, "spell {id} moved with a reagent it does not list");
            }
        }
    }
    assert!(moved >= 100, "only {moved} views showed a carried reagent");
    // Each view records exactly the entries its spell lists.
    for (&id, (_, deps)) in listing.iter().zip(&plain) {
        let d = spells.catalog.get(id).expect("listed");
        let mut want: Vec<u32> = d.reagents.iter().map(|r| r.0).filter(|&e| e != 0).collect();
        want.sort_unstable();
        want.dedup();
        let mut got = deps.reagents().to_vec();
        got.sort_unstable();
        assert_eq!(got, want, "spell {id}");
    }
}

/// A pet view reads the player's side only where the reference's builder does: the pet's form,
/// level and reaches are the pet's own inputs, and it never searches the player's worn set.
#[test]
fn a_pet_view_reads_no_player_form_worn_set_or_reach() {
    let Some(mut rig) = Rig::new() else { return };
    let Rig {
        spells,
        t,
        objs,
        ids,
        layout,
        ..
    } = &mut rig;
    let objects = objs.get();
    let pet = ObjectFields::from_pairs(&[(34, 30), (162, 900)]);
    let m = mat(
        layout,
        Inputs::default(),
        Params {
            form: 1,
            kind: 0,
            family: 1,
        },
    );
    let mut expands_home = 0;
    for &id in ids.iter() {
        t.spell_mods = m.mods.clone();
        let mut ctx = t.ctx_for(&objects, 0, None, Some(&m.store));
        ctx.caster = ViewCaster::Pet(Some(&pet));
        let Some((_, deps)) = build_view(id, spells, &mut ctx) else {
            continue;
        };
        assert!(
            !deps.form && !deps.reach && deps.worn == 0,
            "the pet view of spell {id} reads the player's side: {deps:?}"
        );
        expands_home += usize::from(deps.home);
    }
    assert!(expands_home >= 1, "no pet view expands `$z`");
}

/// [`ModsDiff`] names the row of the cell that moved, whichever table and op.
#[test]
fn the_modifier_diff_names_the_family_flag_bit_of_a_changed_cell() {
    let base = SpellModifiers::default();
    for (bit, op) in [(0u8, 0u8), (5, 14), (33, 28), (63, 28)] {
        for flat in [true, false] {
            let mut now = base.clone();
            now.set(flat, bit, op, 9);
            assert_eq!(
                now.diff(&base),
                ModsDiff {
                    bits: 1 << bit,
                    class_family: false
                },
                "bit {bit} op {op} flat {flat}"
            );
        }
    }
    let mut now = base.clone();
    now.set_class_family(3);
    assert_eq!(
        now.diff(&base),
        ModsDiff {
            bits: 0,
            class_family: true
        }
    );
    assert_eq!(base.diff(&base), ModsDiff::default());
}

/// A percentage's own bit, a worn slot's own bit, a skill line by its id and a player appearing,
/// through the diff the feed runs each frame.
#[test]
fn the_seen_diff_names_each_input_it_saw_move() {
    let player = |pairs: &[(u16, u32)]| ObjectStore(ObjectFields::from_pairs(pairs));
    let seen = |s: &ObjectStore| Seen::of(Some(s), None, None);
    let base = player(&[(22, 100)]);
    for (i, field) in [1106u16, 1107, 1108, 1109].into_iter().enumerate() {
        let now = player(&[(22, 100), (field, 4.5f32.to_bits())]);
        let c = seen(&now).changes_since(&seen(&base));
        assert_eq!(c.avoidance, 1 << i, "percentage field {field}");
        assert_eq!((c.worn, c.form, c.reach, c.home), (0, false, false, false));
    }
    let now = player(&[(22, 100), (486 + 2 * 15, 7)]);
    assert_eq!(seen(&now).changes_since(&seen(&base)).worn, 1 << 15);
    let now = player(&[(22, 100), (138, 1 << 16)]);
    assert!(seen(&now).changes_since(&seen(&base)).form);
    let now = player(&[(22, 100), (130, 3.0f32.to_bits())]);
    assert!(seen(&now).changes_since(&seen(&base)).reach);
    let engaged = Seen::of(Some(&base), None, Some(2.0));
    assert!(engaged.changes_since(&seen(&base)).reach);
    let bound = Seen::of(Some(&base), Some("Ironforge"), None);
    assert!(bound.changes_since(&seen(&base)).home);
    let gone = Seen::of(None, None, None);
    assert_eq!(gone.changes_since(&seen(&base)).worn, WORN_SLOTS);
    assert!(seen(&base).changes_since(&seen(&base)).is_empty());
    // A line's value moving names the line.
    let line = |value: u32| player(&[(22, 100), (718, 43), (719, value | 300 << 16)]);
    assert_eq!(
        seen(&line(40)).changes_since(&seen(&line(39))).skill_lines,
        [43]
    );
}
