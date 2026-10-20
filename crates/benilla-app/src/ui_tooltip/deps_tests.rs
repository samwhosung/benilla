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
use benilla_ui::script::{AuraKey, AuraTooltipView, SpellTooltipView};

use super::spell_deps::{Changes, Deps, RangeSeen, Reagents, Seen};
use super::spell_feed::{
    build_aura_view, build_view, reagent_state, PetInputs, ViewCaster, ViewCtx,
};
use super::tests::TestCtx;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects};
use crate::spell::usable::{
    slot_item_cached, worn_slots_read, SlotItem, EQUIPMENT_MASK, EQUIPMENT_SLOTS,
};
use crate::spell::{ModsDiff, SpellModifiers, UnitField};
use crate::ui_action::{real_spells, Spells};
use crate::ui_items::TestObjects;
use benilla_formats::{RangeUnit, UnitMotion};

/// One side of each input group, so that moving an input is an xor: `false` is the empty side (no
/// bind point, gender 0, form 0, no percentages, default reach, no auto-attack target, both units
/// standing, nothing worn, no skills, zero cells), `true` the populated one.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
struct Inputs {
    home: bool,
    /// The player's gender byte, `UNIT_FIELD_BYTES_0` byte 2.
    gender: bool,
    form: bool,
    /// A `Chance` bit each: block, dodge, parry, crit.
    avoid: u8,
    /// The caster's reach and, with a target engaged, its reach.
    reach: bool,
    /// An auto-attack target: the melee range cell's second unit.
    engaged: bool,
    /// The caster and the auto-attack target run: with a target engaged, the moving bonus.
    pace: bool,
    /// The equipment slots holding an item that meets the view's requirement.
    worn: u32,
    /// The disarm flag.
    disarm: bool,
    /// Every reputation slot at Exalted.
    rep: bool,
    /// A [`UnitField`] bit each: base mana, base health, ranged attack time, cast speed, the cost
    /// modifiers and the cost multipliers.
    unit: u16,
    /// Bit `i`: the skill line `lines[i]` at 300.
    lines: u128,
    /// The `SpellFamilyFlags` bits whose cells hold a modifier.
    bits: u64,
    /// A run of the player's fields set to other values ([`Poke`]).
    poke: Option<Poke>,
}

/// A run of at most 64 of the player's fields from `first`, each set to [`POKE`] whatever it
/// holds, but those whose bit of `skip` is set. A field in `keep` flips instead every bit outside
/// its mask and holds the scenario's value inside it, so a record of a byte or a bit of a field
/// (the gender byte of 36, the form byte of 138, the disarm bit of 46) is judged at that byte or
/// bit, its siblings moved.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
struct Poke {
    first: u16,
    count: u16,
    skip: u64,
    keep: [(u16, u32); 3],
}

impl Poke {
    fn run(first: u16, count: u16) -> Self {
        Self {
            first,
            count,
            ..Self::default()
        }
    }

    /// The poked fields, each with the mask it keeps (0 for none).
    fn fields(self) -> impl Iterator<Item = (u16, u32)> {
        (self.first..self.first + self.count)
            .filter(move |f| self.skip >> (f - self.first) & 1 == 0)
            .map(move |f| {
                let kept = self.keep.iter().find(|&&(k, m)| k == f && m != 0);
                (f, kept.map_or(0, |&(_, m)| m))
            })
    }
}

/// What a poked field is set to: 2.5 as a float, and a large number as an integer.
const POKE: u32 = 0x4020_0000;

impl Inputs {
    const ALL: Inputs = Inputs {
        home: true,
        gender: true,
        form: true,
        avoid: 0xf,
        reach: true,
        engaged: true,
        pace: true,
        worn: EQUIPMENT_MASK,
        disarm: true,
        rep: true,
        unit: UNIT_MASK,
        lines: u128::MAX,
        bits: u64::MAX,
        poke: None,
    };

    fn xor(self, o: Inputs) -> Inputs {
        Inputs {
            home: self.home ^ o.home,
            gender: self.gender ^ o.gender,
            form: self.form ^ o.form,
            avoid: self.avoid ^ o.avoid,
            reach: self.reach ^ o.reach,
            engaged: self.engaged ^ o.engaged,
            pace: self.pace ^ o.pace,
            worn: self.worn ^ o.worn,
            disarm: self.disarm ^ o.disarm,
            rep: self.rep ^ o.rep,
            unit: self.unit ^ o.unit,
            lines: self.lines ^ o.lines,
            bits: self.bits ^ o.bits,
            poke: self.poke.xor(o.poke),
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
        if self.gender {
            parts.push((
                "the gender byte".to_string(),
                Inputs {
                    gender: true,
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
        if self.engaged {
            parts.push((
                "the auto-attack target".to_string(),
                Inputs {
                    engaged: true,
                    ..alone
                },
            ));
        }
        if self.pace {
            parts.push((
                "the units' run".to_string(),
                Inputs {
                    pace: true,
                    ..alone
                },
            ));
        }
        for slot in (0..EQUIPMENT_SLOTS).filter(|s| self.worn >> s & 1 == 1) {
            let only = Inputs {
                worn: 1 << slot,
                ..alone
            };
            parts.push((format!("worn slot {slot}"), only));
        }
        if self.disarm {
            parts.push((
                "the disarm flag".to_string(),
                Inputs {
                    disarm: true,
                    ..alone
                },
            ));
        }
        if self.rep {
            parts.push(("the standings".to_string(), Inputs { rep: true, ..alone }));
        }
        for (bit, name) in UNIT_NAMES.iter().enumerate() {
            if self.unit >> bit & 1 == 1 {
                let only = Inputs {
                    unit: 1 << bit,
                    ..alone
                };
                parts.push((format!("the player's {name}"), only));
            }
        }
        if let Some(poke) = self.poke {
            for (field, mask) in poke.fields() {
                let only = Inputs {
                    poke: Some(Poke {
                        keep: [(field, mask), (0, 0), (0, 0)],
                        ..Poke::run(field, 1)
                    }),
                    ..alone
                };
                let bits = if mask == 0 {
                    String::new()
                } else {
                    format!(" outside {mask:#010x}")
                };
                parts.push((format!("the player's field {field}{bits}"), only));
            }
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
            gender: !deps.gender,
            form: !deps.form,
            avoid: !deps.avoidance & 0xf,
            reach: !deps.range_units,
            engaged: !deps.range_units,
            pace: !deps.range_units,
            worn: EQUIPMENT_MASK & !deps.worn,
            disarm: !deps.disarm,
            rep: !deps.reputation,
            unit: !deps.unit & UNIT_MASK,
            lines: layout
                .lines
                .iter()
                .enumerate()
                .filter(|(_, line)| !deps.skill_lines.contains(line))
                .fold(0, |mask, (i, _)| mask | 1 << i),
            bits: !deps.mod_bits,
            poke: None,
        }
    }
}

/// The [`UnitField`] names, by bit.
const UNIT_NAMES: [&str; 6] = [
    "base mana",
    "base health",
    "ranged attack time",
    "cast speed",
    "power cost modifiers",
    "power cost multipliers",
];

/// Every [`UnitField`] bit.
const UNIT_MASK: u16 = (1 << UNIT_NAMES.len()) - 1;

/// What a sweep holds fixed for a view: the form the form input puts on, the item kind worn
/// items are of, and the class family the tables gate on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
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
    reputations: crate::net::Reputations,
    mods: SpellModifiers,
    home: Option<&'static str>,
    /// The caster as the range cell reads it: the store's reach, and running or not.
    caster: RangeUnit,
    /// The auto-attack target, when one is engaged.
    attack_target: Option<RangeUnit>,
    form: u8,
}

fn mat(layout: &Layout, inputs: Inputs, p: Params) -> Mat {
    let unit = |field: UnitField| inputs.unit & field.bit() != 0;
    // Health and the level, fixed, and the unit fields at one value on the empty side and another on
    // the populated one.
    let mut pairs: Vec<(u16, u32)> = vec![
        (22, 3500),
        (34, 60),
        (
            162,
            if unit(UnitField::BaseMana) {
                2100
            } else {
                1000
            },
        ),
        (
            163,
            if unit(UnitField::BaseHealth) {
                1900
            } else {
                1689
            },
        ),
        (
            128,
            if unit(UnitField::RangedTime) {
                3000
            } else {
                2000
            },
        ),
        // `UNIT_MOD_CAST_SPEED`: a player's 1.0, and Curse of Tongues' 1.5.
        (
            145,
            if unit(UnitField::CastSpeed) {
                1.5f32
            } else {
                1.0
            }
            .to_bits(),
        ),
    ];
    // Every school's cost term: 25 more, and a third off.
    for school in 0..7 {
        if unit(UnitField::PowerCostModifier) {
            pairs.push((173 + school, 25));
        }
        if unit(UnitField::PowerCostMultiplier) {
            pairs.push((180 + school, (-1.0f32 / 3.0).to_bits()));
        }
    }
    if inputs.disarm {
        pairs.push((46, 0x0020_0000));
    }
    if inputs.gender {
        pairs.push((36, 1 << 16));
    }
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
    let motion = if inputs.pace {
        UnitMotion {
            flags: 1,
            speed: 7.0,
            walk_speed: 2.5,
        }
    } else {
        UnitMotion::default()
    };
    for (i, &line) in layout.lines.iter().enumerate() {
        if let Some(base) = skill_fields(layout, inputs, line) {
            debug_assert!(inputs.lines >> i & 1 == 1);
            pairs.push((base, line));
            pairs.push((base + 1, 300 | 300 << 16));
        }
    }
    for s in (0..EQUIPMENT_SLOTS.into()).filter(|s| inputs.worn >> s & 1 == 1) {
        let guid = Layout::guid(p.kind, s);
        pairs.push((486 + 2 * s as u16, guid as u32));
        pairs.push((487 + 2 * s as u16, (guid >> 32) as u32));
    }
    if let Some(poke) = inputs.poke {
        for (field, mask) in poke.fields() {
            let value = if mask == 0 {
                POKE
            } else {
                let now = pairs
                    .iter()
                    .rev()
                    .find(|&&(f, _)| f == field)
                    .map_or(0, |&(_, v)| v);
                (now & mask) | (!now & !mask)
            };
            pairs.push((field, value));
        }
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
        reputations: crate::net::Reputations::from_slots(vec![
            (
                0,
                if inputs.rep { 50_000 } else { 0 }
            );
            64
        ]),
        mods,
        home: inputs.home.then_some("Zzz"),
        // The two reaches move together: the caster's 1.5 to 4.0, and an auto-attack target's 4.0
        // to 6.0, which needs one engaged to read. The target's start clears the melee row's 5.0
        // floor beside the caster's 1.5, so its presence moves the range cell.
        caster: RangeUnit {
            reach: if inputs.reach { 4.0 } else { 1.5 },
            motion,
            ..RangeUnit::still(1.5)
        },
        attack_target: inputs.engaged.then_some(RangeUnit {
            reach: if inputs.reach { 6.0 } else { 4.0 },
            motion,
            ..RangeUnit::still(1.5)
        }),
        form,
    }
}

/// The first of the three skill-table fields (`PLAYER_SKILL_INFO_1_1`, 718, on) that hold `line`
/// in the scenario `inputs` makes, the present lines packed from slot 0 in layout order.
fn skill_fields(layout: &Layout, inputs: Inputs, line: u32) -> Option<u16> {
    let i = layout.lines.iter().position(|&l| l == line)?;
    if inputs.lines >> i & 1 == 0 {
        return None;
    }
    let slot = (inputs.lines & ((1u128 << i) - 1)).count_ones() as u16;
    Some(718 + 3 * slot)
}

/// The player's unit fields each [`UnitField`] bit is, by bit: base mana, base health, ranged
/// attack time, cast speed, and the seven schools' cost modifiers and multipliers.
const UNIT_FIELD_INDICES: [&[u16]; 6] = [
    &[162],
    &[163],
    &[128],
    &[145],
    &[173, 174, 175, 176, 177, 178, 179],
    &[180, 181, 182, 183, 184, 185, 186],
];

/// A whole field's mask in [`recorded_fields`].
const WHOLE: u32 = u32::MAX;

/// The player fields a view's records watch in the scenario `inputs` makes, each with the bits
/// watched: the ones a poke is covered by. The gender and the form are byte 2 of fields 36 and
/// 138, the disarm flag bit 21 of 46, the standings' race and class bytes 0 and 1 of 36; every
/// other record watches its fields whole.
fn recorded_fields(layout: &Layout, inputs: Inputs, deps: &Deps) -> Vec<(u16, u32)> {
    let mut fields: Vec<(u16, u32)> = UNIT_FIELD_INDICES
        .iter()
        .enumerate()
        .filter(|(bit, _)| deps.unit >> bit & 1 == 1)
        .flat_map(|(_, fields)| fields.iter().map(|&f| (f, WHOLE)))
        .collect();
    for (read, field, mask) in [
        (deps.gender, 36, 0x00ff_0000),
        (deps.form, 138, 0x00ff_0000),
        (deps.disarm, 46, 0x0020_0000),
        (deps.reputation, 36, 0x0000_ffff),
        (deps.range_units, 130, WHOLE),
    ] {
        if read {
            fields.push((field, mask));
        }
    }
    fields.extend(
        (0..4)
            .filter(|i| deps.avoidance >> i & 1 == 1)
            .map(|i| (1106 + i, WHOLE)),
    );
    for slot in (0..u16::from(EQUIPMENT_SLOTS)).filter(|s| deps.worn >> s & 1 == 1) {
        fields.extend([(486 + 2 * slot, WHOLE), (487 + 2 * slot, WHOLE)]);
    }
    for &line in &deps.skill_lines {
        if let Some(base) = skill_fields(layout, inputs, line) {
            fields.extend([base, base + 1, base + 2].map(|f| (f, WHOLE)));
        }
    }
    fields
}

/// What differs between two scenarios, through the feed's own diffs.
fn changes(before: &Mat, now: &Mat, objects: &Objects, items: &Items) -> Changes {
    let seen = |m: &Mat| {
        let range = RangeSeen::of(&m.caster, m.attack_target.as_ref());
        Seen::of(Some(&m.store), m.home, range, objects, items).with_reputations(&m.reputations)
    };
    seen(now)
        .changes_since(&seen(before))
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
    /// The aura builder's level and stack when the sweeps build aura views; `None` builds the
    /// spell views.
    aura: Option<(u8, u8)>,
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
        // The bag items the tools and reagents lines name.
        let reagents: BTreeSet<u32> = displays()
            .flat_map(|d| d.reagents.iter().map(|&(entry, _)| entry).chain(d.totems))
            .filter(|&entry| entry != 0)
            .collect();
        let families: BTreeSet<u32> = displays()
            .map(|d| d.spell_family)
            .filter(|&f| f != 0)
            .collect();
        let mut t = TestCtx::new();
        t.skill_lines = Some(skill_lines);
        t.factions = Some(benilla_formats::load_faction_catalog(&mut chain).expect("factions"));
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
            for slot in 0..u32::from(EQUIPMENT_SLOTS) {
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
            aura: None,
        })
    }
}

/// A built view, the spell builder's or the aura builder's.
#[derive(Debug, PartialEq)]
enum Built {
    Spell(Box<SpellTooltipView>),
    Aura(AuraTooltipView),
}

fn build(
    t: &mut TestCtx,
    objects: &Objects,
    spells: &Spells,
    aura: Option<(u8, u8)>,
    id: u32,
    m: &Mat,
) -> Option<(Built, Deps)> {
    t.spell_mods = m.mods.clone();
    t.reputations = m.reputations.clone();
    let mut ctx = t.ctx_for(objects, m.form, None, Some(&m.store));
    ctx.home_area = m.home;
    ctx.range_caster = m.caster;
    ctx.attack_target = m.attack_target;
    match aura {
        None => build_view(id, spells, &mut ctx).map(|(v, d)| (Built::Spell(Box::new(v)), d)),
        Some((level, stack)) => {
            let key = AuraKey {
                spell_id: id,
                level,
                stack,
            };
            build_aura_view(key, spells, &ctx).map(|(v, d)| (Built::Aura(v), d))
        }
    }
}

/// A spell whose text names another spell's number (`$1234s1`), whose modifiers then answer to
/// that spell's family. The spell builder's text and the aura builder's are both read.
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
        aura,
        ..
    } = rig;
    let aura = *aura;
    let objects = objs.get();
    let mut out = Sweep::default();
    let mut before: Option<(Params, Mat)> = None;
    let mut after: Option<((Params, Inputs), Mat, Changes)> = None;
    // Views with the same parameters run together, so a scenario is made once for them.
    let mut order: Vec<(Params, u32)> = ids
        .iter()
        .filter_map(|&id| {
            let d = spells.catalog.get(id).expect("listed spell");
            only(d).then(|| (params(layout, d), id))
        })
        .collect();
    order.sort_unstable_by_key(|&(p, id)| (p, id));
    for (p, id) in order {
        let d = spells.catalog.get(id).expect("listed spell");
        if before.as_ref().map(|(k, _)| k) != Some(&p) {
            before = Some((p, mat(layout, base, p)));
        }
        let (_, before) = before.as_ref().expect("set above");
        let Some((view, deps)) = build(t, &objects, spells, aura, id, before) else {
            continue;
        };
        let moved = flip(layout, &deps);
        if after.as_ref().map(|(k, ..)| k) != Some(&(p, moved)) {
            let now = mat(layout, base.xor(moved), p);
            let c = changes(before, &now, &objects, &t.items);
            after = Some(((p, moved), now, c));
        }
        let (_, now, changes) = after.as_ref().expect("set above");
        let (moved_view, _) = build(t, &objects, spells, aura, id, now).expect("built before");
        let hit = deps.hits(changes);
        out.requeued += usize::from(hit);
        if moved_view == view {
            continue;
        }
        out.moved += 1;
        // A poked run is judged field by field: one the records cover can hit the change for its
        // neighbours, so the run poked again without the recorded fields, and with only the
        // unrecorded bits of a field recorded in part, must not move the view.
        let moved = match moved.poke {
            Some(poke) if hit => {
                let mut rest = poke;
                for (field, mask) in recorded_fields(layout, base, &deps) {
                    if !(poke.first..poke.first + poke.count).contains(&field) {
                        continue;
                    }
                    if mask == WHOLE {
                        rest.skip |= 1 << (field - poke.first);
                        continue;
                    }
                    // A field recorded in part by two records keeps both parts.
                    let at = rest
                        .keep
                        .iter()
                        .position(|&(f, m)| f == field && m != 0)
                        .or_else(|| rest.keep.iter().position(|&(_, m)| m == 0))
                        .expect("at most three fields recorded in part");
                    rest.keep[at] = (field, rest.keep[at].1 | mask);
                }
                let rest = Inputs {
                    poke: Some(rest),
                    ..moved
                };
                let alone = mat(layout, base.xor(rest), p);
                if build(t, &objects, spells, aura, id, &alone).is_some_and(|(v, _)| v == view) {
                    continue;
                }
                rest
            }
            _ if hit => continue,
            _ => moved,
        };
        // Name what moved it: each input alone.
        let culprits: Vec<String> = moved
            .parts(layout)
            .into_iter()
            .filter(|(_, part)| {
                let alone = mat(layout, base.xor(*part), p);
                build(t, &objects, spells, aura, id, &alone).is_some_and(|(v, _)| v != view)
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
            gender: true,
            ..empty
        }),
    );
    assert_covered("the gender byte", &s);
    assert!(s.moved >= 50, "only {} views read the gender byte", s.moved);
    row("gender byte ($g)", &s);
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
    // The melee range cell's second unit, engaged; and, with it engaged, both units running.
    let engaged = Inputs {
        engaged: true,
        ..empty
    };
    let s = sweep(&mut rig, empty, &any, &own, &cause(engaged));
    assert_covered("the auto-attack target", &s);
    assert!(s.moved >= 100, "only {} views read the target", s.moved);
    row("auto-attack target", &s);
    let pace = Inputs {
        pace: true,
        ..empty
    };
    let s = sweep(&mut rig, engaged, &any, &own, &cause(pace));
    assert_covered("the units' run", &s);
    assert!(s.moved >= 100, "only {} views read the run", s.moved);
    row("both units running", &s);
    // With no target engaged the melee arm has no unit for the bonus: nothing moves.
    let s = sweep(&mut rig, empty, &any, &own, &cause(pace));
    assert_eq!(s.moved, 0, "the moving bonus needs a unit to read");
    let all_worn = Inputs {
        worn: EQUIPMENT_MASK,
        ..empty
    };
    let s = sweep(&mut rig, empty, &any, &own, &cause(all_worn));
    assert_covered("the worn set", &s);
    assert!(s.moved >= 20, "only {} views read the worn set", s.moved);
    row("worn set (all slots)", &s);
    for slot in 0..EQUIPMENT_SLOTS {
        let inputs = Inputs {
            worn: 1 << slot,
            ..empty
        };
        let searching = |d: &benilla_formats::SpellDisplay| worn_slots_read(d) != 0;
        let s = sweep(&mut rig, empty, &searching, &own, &cause(inputs));
        assert_covered(&format!("worn slot {slot}"), &s);
    }
    // The player's unit fields, one at a time, and the disarm flag with weapons in both hands.
    for (bit, name) in UNIT_NAMES.iter().enumerate() {
        let inputs = Inputs {
            unit: 1 << bit,
            ..empty
        };
        let s = sweep(&mut rig, empty, &any, &own, &cause(inputs));
        assert_covered(&format!("the player's {name}"), &s);
        assert!(s.moved >= 1, "no view reads the player's {name}");
        row(name, &s);
    }
    let armed = Inputs {
        worn: EQUIPMENT_MASK,
        ..empty
    };
    let disarm = Inputs {
        disarm: true,
        ..empty
    };
    let s = sweep(&mut rig, armed, &any, &own, &cause(disarm));
    assert_covered("the disarm flag", &s);
    assert!(s.moved >= 20, "only {} views read the disarm flag", s.moved);
    row("disarm flag (armed)", &s);
    // The standings: the reputation line of the one row with a faction, 6994.
    let rep = Inputs { rep: true, ..empty };
    let s = sweep(&mut rig, empty, &any, &own, &cause(rep));
    assert_covered("the standings", &s);
    assert_eq!(s.moved, 1, "the one reputation line reads a standing");
    row("standings", &s);
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

/// The aura builder's views (`0x52f880`) answer to their reads as the spell views do: at level 0,
/// where the skill line stands in for the caster's level, and at a caster's level and stack,
/// everything an aura view reads moves it and nothing else does.
#[test]
fn every_aura_view_that_moves_with_an_input_is_requeued_by_that_input() {
    let Some(mut rig) = Rig::new() else { return };
    let empty = Inputs::default();
    let outside = |layout: &Layout, deps: &Deps| Inputs::outside(deps, layout);
    let inside = |layout: &Layout, deps: &Deps| Inputs::ALL.xor(Inputs::outside(deps, layout));
    for (key, floor) in [((0, 1), 250), ((30, 3), 150)] {
        rig.aura = Some(key);
        let s = sweep(&mut rig, empty, &any, &own, &inside);
        assert_covered(&format!("everything an aura view reads at {key:?}"), &s);
        assert!(
            s.moved >= floor,
            "only {} aura views at {key:?} moved with what they read",
            s.moved
        );
        for base in [empty, Inputs::ALL] {
            let s = sweep(&mut rig, base, &any, &own, &outside);
            assert_covered(
                &format!("everything an aura view at {key:?} does not read"),
                &s,
            );
        }
    }
}

/// The tools and reagents lines: a name streaming in re-queues every view that lists the item,
/// and a count moving re-queues only the spells that list the entry.
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
            spells.catalog.get(id).is_some_and(|d| {
                d.reagents.iter().any(|&(entry, _)| entry != 0) || d.totems.iter().any(|&e| e != 0)
            })
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
                let state = reagent_state(e, Some(store), &objects, &t.items);
                (e, state)
            })
            .collect()
    };
    let mods = SpellModifiers::default();
    let mat_of = |store: ObjectStore| Mat {
        store,
        reputations: Default::default(),
        mods: mods.clone(),
        home: None,
        caster: RangeUnit::still(1.5),
        attack_target: None,
        form: 0,
    };

    // The names stream in: every reagent line appears, and each view that has one is due.
    let empty = mat_of(ObjectStore(ObjectFields::from_pairs(&[(22, 100)])));
    let unnamed = states(t, &empty.store);
    let before: Vec<_> = listing
        .iter()
        .map(|&id| build(t, &objects, spells, None, id, &empty).expect("built"))
        .collect();
    for (i, &entry) in reagents.iter().enumerate() {
        let template = crate::items::test_template(&format!("Reagent {i}"));
        t.items.insert_template(entry, Some(template));
    }
    let named = states(t, &empty.store);
    let changes = Changes::default().with_reagents(&unnamed, &named);
    let mut moved = 0;
    for (&id, (view, deps)) in listing.iter().zip(&before) {
        let (now, _) = build(t, &objects, spells, None, id, &empty).expect("built");
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
        .map(|&id| build(t, &objects, spells, None, id, &empty).expect("built"))
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
            let (now, _) = build(t, &objects, spells, None, id, &carrying).expect("built");
            let lists = deps.reagents().iter().any(|e| entries.contains(e));
            assert_eq!(deps.hits(&changes), lists, "spell {id}: {deps:?}");
            if now != *view {
                moved += 1;
                assert!(lists, "spell {id} moved with a reagent it does not list");
            }
        }
    }
    assert!(moved >= 100, "only {moved} views showed a carried reagent");
    // Each view records exactly the entries its spell lists, its tools among them.
    for (&id, (_, deps)) in listing.iter().zip(&plain) {
        let d = spells.catalog.get(id).expect("listed");
        let mut want: Vec<u32> = d
            .reagents
            .iter()
            .map(|r| r.0)
            .chain(d.totems)
            .filter(|&e| e != 0)
            .collect();
        want.sort_unstable();
        want.dedup();
        let mut got = deps.reagents().to_vec();
        got.sort_unstable();
        assert_eq!(got, want, "spell {id}");
    }
}

/// A pet view reads the player's side only where the builder does: of the unit fields, only the
/// ranged attack time. Its form, level, cost bases and reaches are the pet's own, and it never
/// searches the player's equipment.
#[test]
fn a_pet_view_reads_no_player_form_equipment_reach_or_unit_field_but_the_ranged_time() {
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
    let (mut expands_home, mut ranged) = (0, 0);
    for &id in ids.iter() {
        t.spell_mods = m.mods.clone();
        let mut ctx = t.ctx_for(&objects, 0, None, Some(&m.store));
        ctx.caster = ViewCaster::Pet(Some(&pet));
        let Some((_, deps)) = build_view(id, spells, &mut ctx) else {
            continue;
        };
        assert!(
            !deps.form
                && !deps.range_units
                && deps.worn == 0
                && !deps.disarm
                && deps.unit & !UnitField::RangedTime.bit() == 0,
            "the pet view of spell {id} reads the player's side: {deps:?}"
        );
        expands_home += usize::from(deps.home);
        ranged += deps.unit;
    }
    assert!(expands_home >= 1, "no pet view expands `$z`");
    assert!(ranged >= 1, "no pet view reads the ranged attack time");
}

/// A pet view's cost reads `0x612c50` on the pet (`6e327b`), so the pet's base mana and base health
/// are pet inputs, and its max health and max pools, which no cost reads, are not.
#[test]
fn the_pets_cost_bases_are_its_inputs_and_its_max_pools_are_not() {
    let unit = RangeUnit::still(1.5);
    let inputs = |pairs: &[(u16, u32)]| {
        PetInputs::of(
            Some(1),
            Some(&ObjectFields::from_pairs(pairs)),
            RangeSeen::of(&unit, None),
        )
    };
    let base = inputs(&[(34, 30), (162, 900), (163, 600)]);
    assert!(
        inputs(&[(34, 30), (162, 901), (163, 600)]) != base,
        "base mana"
    );
    assert!(
        inputs(&[(34, 30), (162, 900), (163, 601)]) != base,
        "base health"
    );
    let pools = [
        (34, 30),
        (162, 900),
        (163, 600),
        (28, 4000),
        (29, 2000),
        (32, 100),
    ];
    assert!(inputs(&pools) == base, "max health and max pools");
}

/// Every field of the pet's unit that moves a pet view is one of the pet inputs the feed rebuilds
/// the pet views on. The fields go 48 at a time, each judged on its own: a batch poked without the
/// pet inputs' fields must leave every view as it was, and a view it moves is narrowed to the
/// fields that move it.
#[test]
fn no_pet_field_moves_a_pet_view_unwatched() {
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
    let player = mat(
        layout,
        Inputs::ALL,
        Params {
            form: 1,
            kind: 0,
            family: 1,
        },
    );
    // Level, reach, cast speed, base mana, base health.
    let base_pairs: Vec<(u16, u32)> = vec![
        (34, 30),
        (130, 2.0f32.to_bits()),
        (145, 1.0f32.to_bits()),
        (162, 900),
        (163, 600),
    ];
    let pet_with = |poke: &mut dyn Iterator<Item = u16>| {
        let mut pairs = base_pairs.clone();
        pairs.extend(poke.map(|field| (field, POKE)));
        ObjectFields::from_pairs(&pairs)
    };
    // The fields `PetInputs` reads: level, reach, cast speed, form, base mana, base health, and
    // the seven schools' cost modifiers and multipliers.
    const PET_INPUT_FIELDS: [u16; 20] = [
        34, 130, 145, 138, 162, 163, 173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184,
        185, 186,
    ];
    let build_one = |t: &mut TestCtx, id: u32, pet: &ObjectFields| {
        t.spell_mods = player.mods.clone();
        let mut ctx = t.ctx_for(
            &objects,
            pet.unit_shapeshift_form(),
            None,
            Some(&player.store),
        );
        ctx.caster = ViewCaster::Pet(Some(pet));
        ctx.range_caster = RangeUnit::still(pet.unit_combat_reach());
        build_view(id, spells, &mut ctx).map(|(view, _)| view)
    };
    // The feed reads the pet's reach off its entity's store, which these fields stand in for.
    let pet_inputs = |pet: &ObjectFields| {
        let unit = RangeUnit::still(pet.unit_combat_reach());
        PetInputs::of(None, Some(pet), RangeSeen::of(&unit, None))
    };
    let base = pet_with(&mut std::iter::empty());
    for field in PET_INPUT_FIELDS {
        assert!(
            pet_inputs(&pet_with(&mut std::iter::once(field))) != pet_inputs(&base),
            "field {field} is a pet input"
        );
    }
    let before: Vec<_> = ids.iter().map(|&id| build_one(t, id, &base)).collect();
    let (mut moved, mut misses) = (0, Vec::new());
    for first in (0..192u16).step_by(48) {
        let pet = pet_with(&mut (first..first + 48));
        let unwatched = || (first..first + 48).filter(|f| !PET_INPUT_FIELDS.contains(f));
        let rest = pet_with(&mut unwatched());
        assert!(
            pet_inputs(&rest) == pet_inputs(&base),
            "fields {first}..: the pet inputs are these fields"
        );
        for (&id, before) in ids.iter().zip(&before) {
            if *before == build_one(t, id, &pet) {
                continue;
            }
            moved += 1;
            if *before == build_one(t, id, &rest) {
                continue;
            }
            let culprits: Vec<u16> = unwatched()
                .filter(|&field| {
                    *before != build_one(t, id, &pet_with(&mut std::iter::once(field)))
                })
                .collect();
            let name = spells.catalog.get(id).map(|d| d.name.as_str());
            misses.push(format!(
                "pet view of spell {id} {name:?} moved with its unit field(s) {culprits:?}"
            ));
        }
    }
    assert!(
        misses.is_empty(),
        "{} unwatched:\n{}",
        misses.len(),
        misses.join("\n")
    );
    assert!(moved >= 100, "only {moved} pet views moved with their unit");
}

/// Every field of the player's unit and player blocks that moves a view is one the feed watches:
/// the fields go 64 at a time, from a populated player with and without the disarm flag up, and a
/// view that moves without a change hitting it is narrowed to the fields that move it.
#[test]
fn no_player_field_moves_a_view_unwatched() {
    let Some(mut rig) = Rig::new() else { return };
    let mut moved = 0;
    for base in [
        Inputs::ALL,
        Inputs {
            disarm: false,
            ..Inputs::ALL
        },
    ] {
        for first in (0..1400u16).step_by(64) {
            let poke = Some(Poke::run(first, 64.min(1400 - first)));
            let flip = move |_: &Layout, _: &Deps| Inputs {
                poke,
                ..Inputs::default()
            };
            let s = sweep(&mut rig, base, &any, &own, &flip);
            assert_covered(&format!("the player's fields {first}.."), &s);
            moved += s.moved;
        }
    }
    assert!(
        moved >= 2000,
        "only {moved} views moved with a player field"
    );
}

/// The equipped-item search's answer is a function of what the feed watches: for spells of each
/// hand restriction and armor, across every state of the main hand, the off hand and the chest
/// (empty, a guid with no object, an object with and without a template of each kind, whole,
/// worn, broken or without durability) and the disarm flag, two configurations that the watched
/// state does not tell apart give the view the same `item_met`.
#[test]
fn the_search_answers_the_same_wherever_the_watched_state_is_the_same() {
    use std::collections::HashMap;

    // (class, subclass, flags): a sword, a mace's class with a shield's, a sword by another
    // subclass, a deprecated sword, a shield.
    const TEMPLATES: [(u32, u32, u32); 5] =
        [(2, 7, 0), (4, 7, 0), (2, 3, 0), (2, 7, 0x10), (4, 6, 0)];
    // (durability, max durability): none, whole, worn but not broken, broken, no maximum.
    const DURABILITY: [Option<(u32, u32)>; 5] = [
        None,
        Some((100, 100)),
        Some((50, 100)),
        Some((0, 100)),
        Some((0, 0)),
    ];
    #[derive(Clone, Copy, Debug)]
    enum Held {
        Empty,
        GuidOnly,
        Object {
            template: Option<usize>,
            durability: usize,
        },
    }
    let all: Vec<Held> = [Held::Empty, Held::GuidOnly]
        .into_iter()
        .chain((0..5).flat_map(|d| {
            (0..=TEMPLATES.len()).map(move |t| Held::Object {
                template: t.checked_sub(1),
                durability: d,
            })
        }))
        .collect();
    let chest: Vec<Held> = [Held::Empty, Held::GuidOnly]
        .into_iter()
        .chain([None, Some(0), Some(3), Some(4)].into_iter().flat_map(|t| {
            [1, 3].into_iter().map(move |durability| Held::Object {
                template: t,
                durability,
            })
        }))
        .collect();
    let guid = |slot: u64, i: usize| 0x9_0000 + slot * 0x100 + i as u64;
    let entry = |template: Option<usize>| 1000 + template.map_or(0, |t| 1 + t as u32);

    let mut objs = TestObjects::new();
    let mut items = Items::default();
    for (t, &(class, subclass, flags)) in TEMPLATES.iter().enumerate() {
        items.insert_template(
            entry(Some(t)),
            Some(benilla_protocol::messages::ItemInfo {
                class,
                subclass,
                flags,
                ..crate::items::test_template("Item")
            }),
        );
    }
    for (slot, states) in [(15u64, &all), (16, &all), (3, &chest)] {
        for (i, held) in states.iter().enumerate() {
            if let Held::Object {
                template,
                durability,
            } = *held
            {
                let mut pairs = vec![(3u16, entry(template))];
                if let Some((now, max)) = DURABILITY[durability] {
                    pairs.extend([(46, now), (47, max)]);
                }
                objs.spawn(guid(slot, i), ObjectFields::from_pairs(&pairs));
            }
        }
    }
    let objects = objs.get();

    let spec = |id: u32, class: i32, mask: u32, ex3: u32| {
        (
            id,
            benilla_formats::SpellDisplay {
                id,
                name: format!("Spec {id}"),
                equipped_item_class: class,
                equipped_item_subclass_mask: mask,
                attributes_ex3: ex3,
                ..Default::default()
            },
        )
    };
    let ids = [1u32, 2, 3, 4, 5];
    let specs = [
        spec(1, 2, 1 << 7, 0),
        spec(2, 2, 1 << 7, 0x400),
        spec(3, 2, 1 << 7, 0x0100_0000),
        spec(4, 4, 1 << 6, 0),
        spec(5, 2, 1 << 7 | 1 << 3, 0),
    ];
    let spells = Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(specs.into_iter().collect()),
        ..Spells::empty_for_tests()
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    let commands = NetCommands(tx);
    let mods = SpellModifiers::default();

    // (spell, the watched state of the slots it reads, the disarm flag if it reads it) to its
    // `item_met`, and the verdicts each spell reached.
    let mut answers: HashMap<(u32, Vec<SlotItem>, bool), bool> = HashMap::new();
    let mut verdicts: HashMap<u32, [usize; 2]> = HashMap::new();
    for (i15, h15) in all.iter().enumerate() {
        for (i16, h16) in all.iter().enumerate() {
            for (i3, h3) in chest.iter().enumerate() {
                for disarmed in [false, true] {
                    let mut pairs: Vec<(u16, u32)> = Vec::new();
                    for (slot, i, held) in [(15u64, i15, h15), (16, i16, h16), (3, i3, h3)] {
                        let g = match held {
                            Held::Empty => continue,
                            Held::GuidOnly => 0x8_0000 + slot,
                            Held::Object { .. } => guid(slot, i),
                        };
                        pairs.push((486 + 2 * slot as u16, g as u32));
                        pairs.push((487 + 2 * slot as u16, (g >> 32) as u32));
                    }
                    if disarmed {
                        pairs.push((46, 0x0020_0000));
                    }
                    let store = ObjectStore(ObjectFields::from_pairs(&pairs));
                    for id in ids {
                        let mut vctx = ViewCtx {
                            home_area: None,
                            form: 0,
                            store: Some(&store),
                            caster: ViewCaster::Player,
                            range_caster: RangeUnit::still(1.5),
                            attack_target: None,
                            objects: &objects,
                            items: &mut items,
                            commands: &commands,
                            sub_classes: None,
                            skill_lines: None,
                            spell_mods: &mods,
                            factions: None,
                            reputations: None,
                        };
                        let (view, deps) = build_view(id, &spells, &mut vctx).expect("a spec");
                        assert!(
                            deps.disarm && deps.worn != 0,
                            "spec {id} searches: {deps:?}"
                        );
                        let slots = (0..EQUIPMENT_SLOTS)
                            .filter(|&slot| deps.worn >> slot & 1 == 1)
                            .map(|slot| slot_item_cached(&store, slot, &objects, &items))
                            .collect();
                        let key = (id, slots, deps.disarm && crate::items::is_disarmed(&store));
                        let seen = *answers.entry(key).or_insert(view.item_met);
                        assert_eq!(
                            seen, view.item_met,
                            "spec {id}: main {h15:?}, off {h16:?}, chest {h3:?}, disarmed {disarmed}"
                        );
                        verdicts.entry(id).or_default()[usize::from(view.item_met)] += 1;
                    }
                }
            }
        }
    }
    for (id, [without, with]) in verdicts {
        assert!(
            without > 100 && with > 100,
            "spec {id} met {with} times, unmet {without}"
        );
    }
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

/// A percentage's own bit, a worn slot's own bit, a skill line by its id, each unit field by its
/// own bit, the gender byte and a player appearing, through the diff the feed runs each frame.
#[test]
fn the_seen_diff_names_each_input_it_saw_move() {
    let mut objs = TestObjects::new();
    let objects = objs.get();
    let items = Items::default();
    let player = |pairs: &[(u16, u32)]| ObjectStore(ObjectFields::from_pairs(pairs));
    // The feed reads the caster's reach off its store, and passes the units it read.
    let seen_with = |s: &ObjectStore, caster_motion: UnitMotion, attack: Option<&RangeUnit>| {
        let caster = RangeUnit {
            motion: caster_motion,
            ..RangeUnit::still(s.0.unit_combat_reach())
        };
        Seen::of(
            Some(s),
            None,
            RangeSeen::of(&caster, attack),
            &objects,
            &items,
        )
    };
    let seen = |s: &ObjectStore| seen_with(s, UnitMotion::default(), None);
    let base = player(&[(22, 100)]);
    for (i, field) in [1106u16, 1107, 1108, 1109].into_iter().enumerate() {
        let now = player(&[(22, 100), (field, 4.5f32.to_bits())]);
        let c = seen(&now).changes_since(&seen(&base));
        assert_eq!(c.avoidance, 1 << i, "percentage field {field}");
        assert_eq!(
            (c.worn, c.form, c.range_units, c.home),
            (0, false, false, false)
        );
        assert_eq!((c.unit, c.disarm), (0, false));
    }
    // A guid the object index does not hold is an unresolved item: the slot moved.
    let now = player(&[(22, 100), (486 + 2 * 15, 7)]);
    assert_eq!(seen(&now).changes_since(&seen(&base)).worn, 1 << 15);
    let now = player(&[(22, 100), (486 + 2 * 22, 7)]);
    assert_eq!(
        seen(&now).changes_since(&seen(&base)).worn,
        1 << 22,
        "a bag"
    );
    let now = player(&[(22, 100), (138, 1 << 16)]);
    assert!(seen(&now).changes_since(&seen(&base)).form);
    let now = player(&[(22, 100), (130, 3.0f32.to_bits())]);
    assert!(seen(&now).changes_since(&seen(&base)).range_units);
    // The auto-attack target: engaged, its reach, then each unit starting to run on its own.
    let target = RangeUnit::still(2.0);
    let engaged = seen_with(&base, UnitMotion::default(), Some(&target));
    assert!(engaged.changes_since(&seen(&base)).range_units);
    let target_reach = RangeUnit::still(3.0);
    let wider = seen_with(&base, UnitMotion::default(), Some(&target_reach));
    assert!(wider.changes_since(&engaged).range_units);
    let running = UnitMotion {
        flags: 1,
        speed: 7.0,
        walk_speed: 2.5,
    };
    let target_runs = RangeUnit {
        motion: running,
        ..target
    };
    // The moving bonus needs both units running with a target engaged: a run of either one alone
    // moves nothing a melee row shows.
    let caster_runs = seen_with(&base, running, Some(&target));
    assert!(caster_runs.changes_since(&engaged).is_empty());
    let target_run = seen_with(&base, UnitMotion::default(), Some(&target_runs));
    assert!(target_run.changes_since(&engaged).is_empty());
    let both_run = seen_with(&base, running, Some(&target_runs));
    assert!(
        both_run.changes_since(&caster_runs).range_units,
        "the target starts running beside a running caster"
    );
    assert!(
        both_run.changes_since(&target_run).range_units,
        "the caster starts running beside a running target"
    );
    // With no target engaged there is no unit for the bonus.
    let alone = seen_with(&base, running, None);
    assert!(alone.changes_since(&seen(&base)).is_empty());
    // A walk is not a run: the answer a melee row follows does not move.
    let walking = UnitMotion {
        flags: 1,
        speed: 2.5,
        walk_speed: 2.5,
    };
    let caster_walk = seen_with(&base, walking, Some(&target_runs));
    assert!(caster_walk.changes_since(&target_run).is_empty());
    assert!(both_run.changes_since(&caster_walk).range_units);
    let bound = Seen::of(
        Some(&base),
        Some("Ironforge"),
        RangeSeen::default(),
        &objects,
        &items,
    );
    assert!(bound.changes_since(&seen(&base)).home);
    // A player going or coming moves every view, which read nothing of one that was not there.
    let gone = Seen::of(None, None, RangeSeen::default(), &objects, &items);
    assert_eq!(gone.changes_since(&seen(&base)).worn, EQUIPMENT_MASK);
    assert!(gone.changes_since(&seen(&base)).everything);
    assert!(seen(&base).changes_since(&gone).everything);
    assert!(!gone.changes_since(&gone).everything);
    assert!(seen(&base).changes_since(&seen(&base)).is_empty());
    // The cast speed, which only the cast cell reads.
    let now = player(&[(22, 100), (145, 1.5f32.to_bits())]);
    assert_eq!(
        seen(&now).changes_since(&seen(&base)).unit,
        UnitField::CastSpeed.bit()
    );
    // The disarm flag, and no other flag.
    let now = player(&[(22, 100), (46, 0x0020_0000)]);
    let c = seen(&now).changes_since(&seen(&base));
    assert!(c.disarm);
    let now = player(&[(22, 100), (46, 0x0000_0008)]);
    assert!(seen(&now).changes_since(&seen(&base)).is_empty());
    // The gender byte, `UNIT_FIELD_BYTES_0` byte 2; the race and class bytes, under the
    // standings, which ride on their base; and the power type byte, under none.
    let bytes_0 = |bytes: u32| player(&[(22, 100), (36, bytes)]);
    let c = seen(&bytes_0(1 << 16)).changes_since(&seen(&base));
    assert!(c.gender && !c.reputation);
    let c = seen(&bytes_0(4 | 1 << 8)).changes_since(&seen(&base));
    assert!(c.reputation && !c.gender);
    assert!(seen(&bytes_0(1 << 24))
        .changes_since(&seen(&base))
        .is_empty());
    // Each unit field on its own bit: base mana 162, base health 163, ranged attack time 128, a
    // school's cost modifier 173-179 and multiplier 180-186; the level 34, which the per-level
    // terms read through the skill instead (`0x5ea690`), max health 28 and the max pools 29-33
    // on none.
    let unit = [
        (162u16, UnitField::BaseMana.bit()),
        (163, UnitField::BaseHealth.bit()),
        (128, UnitField::RangedTime.bit()),
        (176, UnitField::PowerCostModifier.bit()),
        (186, UnitField::PowerCostMultiplier.bit()),
        (34, 0),
        (28, 0),
        (29, 0),
        (33, 0),
    ];
    for (field, bit) in unit {
        let now = player(&[(22, 100), (field, 77)]);
        let c = seen(&now).changes_since(&seen(&base));
        assert_eq!(c.unit, bit, "unit field {field}");
    }
    // A line's value moving names the line.
    let line = |value: u32| player(&[(22, 100), (718, 43), (719, value | 300 << 16)]);
    assert_eq!(
        seen(&line(40)).changes_since(&seen(&line(39))).skill_lines,
        [43]
    );
}
