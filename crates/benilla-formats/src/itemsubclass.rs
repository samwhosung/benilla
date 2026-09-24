//! `ItemSubClass.dbc` by `(class, subclass)`, 28 fields with no id column: the alternate
//! proficiencies and the display gate the item tooltip's slot and type line reads (builder
//! `0x52b650`, row cache `0xc0db90`), and the subclass names requirements are spelled with.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, u32_at};
use crate::Chain;

const ITEM_SUB_CLASS: &str = "DBFilesClient\\ItemSubClass.dbc";
/// The group names, read before [`ITEM_SUB_CLASS`] when a whole subclass mask needs a name.
const ITEM_SUB_CLASS_MASK: &str = "DBFilesClient\\ItemSubClassMask.dbc";

/// One row's tooltip-relevant fields.
#[derive(Debug, Clone, Copy)]
pub struct ItemSubClassInfo {
    /// An alternate subclass whose proficiency also permits use, -1 for none.
    pub prerequisite_proficiency: i32,
    pub postrequisite_proficiency: i32,
    /// `Flags` (`[row+0x10]`), raw; `0x200` gates the auction house's inventory-slot rows
    /// (`0x4cfb63`).
    pub flags: u32,
    /// Bit 0: never print the type name on the slot and type line.
    pub display_flags: u32,
    /// The swing weight, 0 light, 1 medium, 2 heavy, the sole input to a connecting swing's whoosh;
    /// meaningful on class 2 only.
    pub weapon_swing_size: u32,
}

/// `ItemSubClass.dbc` by `(class, subclass)`.
pub struct ItemSubClassCatalog {
    rows: HashMap<(u32, u32), ItemSubClassInfo>,
    /// The crafting book's header names (`0x4fca20`): the verbose name (`+0x4c`, column 19) when
    /// non-empty, else the display name (`+0x28`, column 10).
    names: HashMap<(u32, u32), String>,
    /// The display name alone, the singular: the cast-fail line says "Must have a Wand equipped"
    /// where the spell tooltip says "Requires Wands".
    display_names: HashMap<(u32, u32), String>,
    /// Keys in file order, the reference's walk order, which sets the join's order and which row
    /// is first.
    order: Vec<(u32, u32)>,
    /// `ItemSubClassMask.dbc`'s `(class, mask, name)`: one name for a whole mask, "Melee Weapon".
    mask_groups: Vec<(u32, u32, String)>,
}

impl ItemSubClassCatalog {
    /// The alternate proficiency subclass, the builder's walk: the prerequisite, then the
    /// postrequisite. A weapon without its own proficiency is usable with the alternate's.
    pub fn proficiency_alt(&self, class: u32, subclass: u32) -> Option<u32> {
        let r = self.rows.get(&(class, subclass))?;
        [r.prerequisite_proficiency, r.postrequisite_proficiency]
            .into_iter()
            .find(|&v| v != -1)
            .map(|v| v as u32)
    }

    /// `WeaponSwingSize` (`[row+0x24]`, returned by `0x623870`). An unknown pair is the reference's
    /// "not a weapon", which plays nothing rather than defaulting to a weight.
    pub fn weapon_swing_size(&self, class: u32, subclass: u32) -> Option<u32> {
        Some(self.rows.get(&(class, subclass))?.weapon_swing_size)
    }

    /// The crafting book's header name, verbose first (`0x4fca20`).
    pub fn name(&self, class: u32, subclass: u32) -> Option<&str> {
        self.names.get(&(class, subclass)).map(String::as_str)
    }

    /// The singular subclass name, the display name alone.
    pub fn display_name(&self, class: u32, subclass: u32) -> Option<&str> {
        self.display_names
            .get(&(class, subclass))
            .map(String::as_str)
    }

    /// A spell's equipped-item requirement as the spell tooltip spells it, plural and verbose
    /// ("Requires Wands"). The reference's lookup (`0x6e2380`) has two stages:
    /// 1. `ItemSubClassMask.dbc` on exact whole-mask equality, so Parry's eleven melee subclasses
    ///    (`0x2a5f3`) print "Melee Weapon".
    /// 2. Otherwise every subclass the mask names, comma-joined in file order, by [`Self::name`].
    ///
    /// It reads only `ItemSubClass.dbc`, with no `ItemClass.dbc` fallback.
    pub fn requirement_name(&self, class: u32, mask: u32) -> Option<String> {
        self.mask_group(class, mask)
            .map(str::to_string)
            .or_else(|| {
                let joined = self
                    .masked(class, mask)
                    .filter_map(|key| self.names.get(&key))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                (!joined.is_empty()).then_some(joined)
            })
    }

    /// The singular spelling the `SPELL_FAILED_EQUIPPED_ITEM_CLASS` line uses: the same stage 1,
    /// then the first matching subclass's display name, never a join.
    pub fn requirement_display_name(&self, class: u32, mask: u32) -> Option<String> {
        self.mask_group(class, mask)
            .map(str::to_string)
            .or_else(|| {
                self.masked(class, mask)
                    .find_map(|key| self.display_names.get(&key))
                    .cloned()
            })
    }

    /// Stage 1: the group name for an exact `(class, mask)` pair.
    fn mask_group(&self, class: u32, mask: u32) -> Option<&str> {
        self.mask_groups
            .iter()
            .find(|(c, m, _)| *c == class && *m == mask)
            .map(|(_, _, name)| name.as_str())
    }

    /// The catalog's keys for `class` whose subclass bit is set in `mask`, in file order.
    fn masked(&self, class: u32, mask: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.order
            .iter()
            .copied()
            .filter(move |&(c, sub)| c == class && sub < 32 && mask & (1 << sub) != 0)
    }

    /// Whether the type name is suppressed (displayFlags bit 0).
    pub fn hides_name(&self, class: u32, subclass: u32) -> bool {
        self.rows
            .get(&(class, subclass))
            .is_some_and(|r| r.display_flags & 1 != 0)
    }

    /// The raw `Flags`, 0 for an unknown key.
    pub fn flags(&self, class: u32, subclass: u32) -> u32 {
        self.rows.get(&(class, subclass)).map_or(0, |r| r.flags)
    }

    /// The raw `DisplayFlags`, 0 for an unknown key. Bit 0 is [`Self::hides_name`]; bit 1 keeps a
    /// subclass out of the auction house's category filter; bit 2 makes `GetInventoryItemCount`
    /// count an equipped bag's contents (`0x4c881a`-`0x4c8826`), set on Soul Bag and every quiver.
    pub fn display_flags(&self, class: u32, subclass: u32) -> u32 {
        self.rows
            .get(&(class, subclass))
            .map_or(0, |r| r.display_flags)
    }

    /// Every subclass id of `class` in file order, the order in which the auction house's scans
    /// count the Nth matching row (`0x4cf9c0`, `0x4ce980`); ids are not dense from 0.
    pub fn subclasses_of(&self, class: u32) -> Vec<u32> {
        self.order
            .iter()
            .filter(|(c, _)| *c == class)
            .map(|&(_, s)| s)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn item_sub_class_schema() -> Schema {
    let mut s = Schema::new("ItemSubClass");
    s.add_field(SchemaField::new("Class", FieldType::UInt32));
    s.add_field(SchemaField::new("SubClass", FieldType::UInt32));
    s.add_field(SchemaField::new(
        "PrerequisiteProficiency",
        FieldType::Int32,
    ));
    s.add_field(SchemaField::new(
        "PostrequisiteProficiency",
        FieldType::Int32,
    ));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s.add_field(SchemaField::new("DisplayFlags", FieldType::UInt32));
    for name in ["ParrySeq", "ReadySeq", "AttackSeq", "SwingSize"] {
        s.add_field(SchemaField::new(format!("Weapon{name}"), FieldType::UInt32));
    }
    for i in 0..8 {
        s.add_field(SchemaField::new(
            format!("DisplayName{i}"),
            FieldType::String,
        ));
    }
    s.add_field(SchemaField::new("DisplayNameFlags", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(
            format!("VerboseName{i}"),
            FieldType::String,
        ));
    }
    s.add_field(SchemaField::new("VerboseNameFlags", FieldType::UInt32));
    s
}

fn item_sub_class_mask_schema() -> Schema {
    let mut s = Schema::new("ItemSubClassMask");
    s.add_field(SchemaField::new("ClassID", FieldType::UInt32));
    s.add_field(SchemaField::new("Mask", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// Load `ItemSubClass.dbc` and the `ItemSubClassMask.dbc` group names off the patch chain.
pub fn load_item_sub_classes(chain: &mut Chain) -> Result<ItemSubClassCatalog> {
    let bytes = chain
        .read_file(ITEM_SUB_CLASS)
        .with_context(|| format!("reading {ITEM_SUB_CLASS}"))?;
    let rs = parse(&bytes, item_sub_class_schema(), "ItemSubClass")?;
    let mask_bytes = chain
        .read_file(ITEM_SUB_CLASS_MASK)
        .with_context(|| format!("reading {ITEM_SUB_CLASS_MASK}"))?;
    let mask_rs = parse(
        &mask_bytes,
        item_sub_class_mask_schema(),
        "ItemSubClassMask",
    )?;
    let mut mask_groups = Vec::with_capacity(mask_rs.records().len());
    for r in mask_rs.records() {
        let (Some(class), Some(mask), Some(name)) = (
            u32_at(r, 0),
            u32_at(r, 1),
            crate::dbc::str_at(&mask_rs, r, 2).filter(|n| !n.is_empty()),
        ) else {
            continue;
        };
        mask_groups.push((class, mask, name));
    }
    let mut rows = HashMap::with_capacity(rs.records().len());
    let mut names = HashMap::with_capacity(rs.records().len());
    let mut display_names = HashMap::with_capacity(rs.records().len());
    let mut order = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(class), Some(subclass)) = (u32_at(r, 0), u32_at(r, 1)) else {
            continue;
        };
        order.push((class, subclass));
        rows.insert(
            (class, subclass),
            ItemSubClassInfo {
                prerequisite_proficiency: i32_at(r, 2).unwrap_or(-1),
                postrequisite_proficiency: i32_at(r, 3).unwrap_or(-1),
                flags: u32_at(r, 4).unwrap_or(0),
                display_flags: u32_at(r, 5).unwrap_or(0),
                weapon_swing_size: u32_at(r, 9).unwrap_or(0),
            },
        );
        // The enUS verbose name (column 19) first, the display name (column 10) as fallback.
        let display = crate::dbc::str_at(&rs, r, 10).filter(|n| !n.is_empty());
        let name = crate::dbc::str_at(&rs, r, 19)
            .filter(|n| !n.is_empty())
            .or_else(|| display.clone());
        if let Some(name) = name {
            names.insert((class, subclass), name);
        }
        if let Some(display) = display {
            display_names.insert((class, subclass), display);
        }
    }
    Ok(ItemSubClassCatalog {
        rows,
        names,
        display_names,
        order,
        mask_groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_subclass_names_resolve_verbose_first() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_sub_classes(&mut chain).expect("load ItemSubClass.dbc");
        assert_eq!(cat.name(2, 7), Some("One-Handed Swords"), "verbose wins");
        assert_eq!(cat.name(4, 1), Some("Cloth"));
        assert_eq!(
            cat.name(5, 0),
            Some("Reagent"),
            "display fallback when verbose empty"
        );
        assert_eq!(cat.name(0, 0), Some("Consumable"));
        assert_eq!(cat.name(99, 0), None);
    }

    /// The requirement lookup's two stages (`0x52eea7`-`0x52f10a`) on the shipped tables.
    #[test]
    fn real_requirement_names_take_the_group_before_the_join() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_sub_classes(&mut chain).expect("load the two DBCs");

        // Stage 1: the whole mask has a name; all three shipped rows.
        assert_eq!(
            cat.requirement_name(2, 0x0002_a5f3).as_deref(),
            Some("Melee Weapon"),
            "Parry's eleven melee subclasses are ONE group name, not a join"
        );
        assert_eq!(cat.requirement_name(4, 0x60).as_deref(), Some("Shield"));
        assert_eq!(
            cat.requirement_name(2, 0x0004_000c).as_deref(),
            Some("Ranged Weapon")
        );
        // The singular arm shares stage 1.
        assert_eq!(
            cat.requirement_display_name(2, 0x0002_a5f3).as_deref(),
            Some("Melee Weapon")
        );

        // Stage 2, one bit: the spellings diverge (Shoot 5019: class 2, bit 19).
        assert_eq!(cat.requirement_name(2, 1 << 19).as_deref(), Some("Wands"));
        assert_eq!(
            cat.requirement_display_name(2, 1 << 19).as_deref(),
            Some("Wand")
        );

        // Stage 2, fist weapons (13) and daggers (15), no shipped group: joined for the tooltip,
        // first only for the cast-fail line.
        let mask = (1 << 13) | (1 << 15);
        assert_eq!(
            cat.requirement_name(2, mask).as_deref(),
            Some("Fist Weapons, Daggers")
        );
        assert_eq!(
            cat.requirement_display_name(2, mask).as_deref(),
            Some("Fist Weapon")
        );

        // Class 6, projectile: the cast-fail `0x31` NEED_EXOTIC_AMMO arm's lookup (`0x6e1e5e`,
        // mask `1 << arg`). Three of the five shipped rows are `(OBSOLETE)` in the data itself.
        assert_eq!(
            cat.requirement_display_name(6, 1 << 2).as_deref(),
            Some("Arrow")
        );
        assert_eq!(
            cat.requirement_display_name(6, 1 << 3).as_deref(),
            Some("Bullet")
        );
        assert_eq!(
            cat.requirement_display_name(6, 1 << 4).as_deref(),
            Some("Thrown(OBSOLETE)")
        );
        // Past the shipped rows: the arm declines (`0x6e1e6a`, to the shared default `0x6e21d8`).
        assert_eq!(cat.requirement_display_name(6, 1 << 5), None);
        assert_eq!(cat.requirement_display_name(6, 1 << 31), None);

        assert_eq!(cat.requirement_name(2, 0), None);
        assert_eq!(cat.requirement_name(99, 1), None);
    }

    #[test]
    fn item_sub_classes_load_from_the_chain() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_sub_classes(&mut chain).expect("ItemSubClass.dbc loads");
        assert!(!cat.is_empty());
        // The live pairs: 2H Axe (2, 1) to 1H Axe, 1H Mace (2, 4) to 2H Mace by postrequisite,
        // 2H Sword (2, 8) to 1H Sword, Shield (4, 6) to Buckler.
        for ((c, sc), r) in {
            let mut v: Vec<_> = cat.rows.iter().map(|(&k, v)| (k, *v)).collect();
            v.sort_by_key(|&((c, sc), _)| (c, sc));
            v
        } {
            if r.prerequisite_proficiency != -1 || r.postrequisite_proficiency != -1 {
                eprintln!(
                    "alt: class {c} sub {sc} pre {} post {}",
                    r.prerequisite_proficiency, r.postrequisite_proficiency
                );
            }
        }
        assert_eq!(cat.proficiency_alt(2, 1), Some(0));
        assert_eq!(cat.proficiency_alt(2, 4), Some(5));
        assert_eq!(cat.proficiency_alt(2, 8), Some(7));
        assert_eq!(cat.proficiency_alt(4, 6), Some(5));
        assert_eq!(cat.proficiency_alt(2, 15), None);
        // Display flag bit 0: Miscellaneous armor hides its type name.
        assert!(cat.hides_name(4, 0));
        assert!(!cat.hides_name(4, 1), "Cloth prints");
        assert!(!cat.hides_name(2, 7), "Sword prints");
    }
}
