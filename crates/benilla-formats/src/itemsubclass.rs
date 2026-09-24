//! ItemSubClass.dbc — per `(class, subclass)`: the alternate-proficiency fields and the
//! display gate the item tooltip's slot|type line reads (the builder `0x52b650`, the
//! `0xc0db90` row cache).
//!
//! The builder consumes exactly three fields beyond the key: **prerequisiteProficiency@2 /
//! postrequisiteProficiency@3** (−1 = none; a weapon whose own subclass bit is missing from the
//! player's proficiency mask is still usable when the alternate's bit is set — the slot cell's
//! red instead of the type cell's), and **displayFlags@5** bit 0 (suppress the type name — the
//! "Miscellaneous" family: rings, trinkets, shirts never print an armor type).
//!
//! Record layout (no id column; keyed by class+subclass, 28 fields): class@0, subClass@1,
//! prerequisiteProficiency@2, postrequisiteProficiency@3, flags@4, displayFlags@5,
//! weaponParrySeq@6, weaponReadySeq@7, weaponAttackSeq@8, weaponSwingSize@9, displayName
//! 8+1 @10..18, verboseName 8+1 @19..27 — the offsets the builder reads (`[row+2]`, `[row+3]`,
//! byte of `[row+5]`, `[row+locale+10]`) land on exactly this shape.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, u32_at};
use crate::Chain;

const ITEM_SUB_CLASS: &str = "DBFilesClient\\ItemSubClass.dbc";
/// The group-name table read *before* [`ITEM_SUB_CLASS`] whenever a whole subclass **mask** needs a
/// name — see [`ItemSubClassCatalog::requirement_name`]. 11 fields (verified by loading the shipped
/// file): `ClassID@0`, `Mask@1`, `Name_Lang@2..9`, `NameFlags@10`.
const ITEM_SUB_CLASS_MASK: &str = "DBFilesClient\\ItemSubClassMask.dbc";

/// One row's tooltip-relevant fields.
#[derive(Debug, Clone, Copy)]
pub struct ItemSubClassInfo {
    /// Alternate subclasses whose proficiency also permits use (−1 = none). The builder's
    /// short-circuit: prerequisite wins when present; postrequisite is only consulted when
    /// prerequisite is −1.
    pub prerequisite_proficiency: i32,
    pub postrequisite_proficiency: i32,
    /// `Flags@4` (`[row+0x10]`), raw. Its consumers each own the bit they read — e.g. `0x200`,
    /// the auction house's "offers the inventory-slot rows" gate (`0x4cfb63`).
    pub flags: u32,
    /// Bit 0 = never print the type name on the slot|type line.
    pub display_flags: u32,
    /// `WeaponSwingSize@9` — the swinging weight, 0 light · 1 medium · 2 heavy, and the sole
    /// input to the connecting swing's whoosh ([`crate::WeaponSwingCatalog`]). Meaningful on
    /// class 2 only; the shipped weapon rows put daggers and fist weapons at light, every
    /// two-hander plus polearms, staves and spears at heavy, and everything else at medium.
    pub weapon_swing_size: u32,
}

/// ItemSubClass.dbc keyed by `(class, subclass)`.
pub struct ItemSubClassCatalog {
    rows: HashMap<(u32, u32), ItemSubClassInfo>,
    /// The crafting book's header vocabulary: the resolved display name, by the
    /// client's own byte law in the recipe-list build `0x4fca20` — **VerboseName**
    /// (`row + locale·4 + 0x4c`, enUS column 19) when non-empty, else **DisplayName** (`+0x28`,
    /// column 10). "One-Handed Swords" over "Sword"; plain "Cloth" where no verbose form exists.
    names: HashMap<(u32, u32), String>,
    /// **DisplayName** alone (column 10) — the SINGULAR spelling. The two are not
    /// interchangeable, and the reference picks between them by call site: the cast-fail line
    /// `SPELL_FAILED_EQUIPPED_ITEM_CLASS` reads "Must have a **Wand** equipped" where the spell
    /// tooltip's requirement line reads "Requires **Wands**" — both pinned by reference captures
    /// of the same spell (Shoot 5019: class 2, submask bit 19).
    display_names: HashMap<(u32, u32), String>,
    /// Every `(class, subclass)` key in **file order** — the order the reference walks the rows in,
    /// which decides both the comma-join's order and which row counts as "first"
    /// ([`Self::requirement_name`] / [`Self::requirement_display_name`]).
    order: Vec<(u32, u32)>,
    /// `ItemSubClassMask.dbc`: `(classId, mask, name)` — the group names that stand in for a whole
    /// mask ("Melee Weapon" for the eleven melee weapon subclasses). Only 3 rows ship. Folded in
    /// here rather than given its own catalog because it is stage 1 of one lookup, not a second
    /// vocabulary.
    mask_groups: Vec<(u32, u32, String)>,
}

impl ItemSubClassCatalog {
    /// The alternate proficiency subclass for `(class, subclass)` — the builder's exact
    /// sentinel walk: prerequisite if not −1, else postrequisite if not −1, else `None`.
    pub fn proficiency_alt(&self, class: u32, subclass: u32) -> Option<u32> {
        let r = self.rows.get(&(class, subclass))?;
        [r.prerequisite_proficiency, r.postrequisite_proficiency]
            .into_iter()
            .find(|&v| v != -1)
            .map(|v| v as u32)
    }

    /// The swinging weight for `(class, subclass)` — `WeaponSwingSize`, the reference's
    /// `0x623870` return: it reads the equipped item's class/subclass, looks the pair up in this
    /// table and hands `[row+0x24]` straight to the swing-sound play. `None` for an unknown pair,
    /// which is the reference's "not a weapon" answer — it returns *false* there and plays
    /// nothing rather than defaulting to a weight.
    pub fn weapon_swing_size(&self, class: u32, subclass: u32) -> Option<u32> {
        Some(self.rows.get(&(class, subclass))?.weapon_swing_size)
    }

    /// The subclass display name (verbose-first, `0x4fca20`'s byte law) — the
    /// crafting book's group header text; `None` for an unknown key.
    pub fn name(&self, class: u32, subclass: u32) -> Option<&str> {
        self.names.get(&(class, subclass)).map(String::as_str)
    }

    /// The SINGULAR subclass name (DisplayName only — [`Self::display_names`]); `None` for an
    /// unknown key or an empty column.
    pub fn display_name(&self, class: u32, subclass: u32) -> Option<&str> {
        self.display_names
            .get(&(class, subclass))
            .map(String::as_str)
    }

    /// What a spell's equipped-item requirement is *called*, for `(class, subclass_mask)` — the
    /// **plural/verbose** spelling the spell tooltip prints ("Requires Wands", "Requires Melee
    /// Weapon"). `None` when nothing names it.
    ///
    /// The reference's law is two-stage (`0x6e2380` and its two call sites), and both stages
    /// matter for a mask with several bits set:
    ///
    /// 1. `ItemSubClassMask.dbc` on **exact whole-mask equality** — not "any bit", the entire mask.
    ///    Three rows ship: `{2, 0x2a5f3, "Melee Weapon"}`, `{4, 0x60, "Shield"}`,
    ///    `{2, 0x4000c, "Ranged Weapon"}`. This is why Parry (`0x2a5f3` = eleven melee subclasses)
    ///    prints one group name rather than eleven weapon types.
    /// 2. Otherwise every subclass the mask names, comma-joined in file order, each preferring
    ///    VerboseName with a DisplayName fallback ([`Self::name`]).
    ///
    /// Only when stage 2 also finds nothing is the line absent. Note `0x6e2380` reads **only**
    /// ItemSubClass.dbc — there is no ItemClass.dbc fallback on either path.
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

    /// The **singular** spelling of the same requirement — what the `SPELL_FAILED_EQUIPPED_ITEM_CLASS`
    /// cast-fail line says ("Must have a Wand equipped"). Same stage 1, but stage 2 takes only the
    /// FIRST matching subclass and its DisplayName: never a join, never VerboseName.
    pub fn requirement_display_name(&self, class: u32, mask: u32) -> Option<String> {
        self.mask_group(class, mask)
            .map(str::to_string)
            .or_else(|| {
                self.masked(class, mask)
                    .find_map(|key| self.display_names.get(&key))
                    .cloned()
            })
    }

    /// Stage 1: the `ItemSubClassMask.dbc` group name for an EXACT `(class, mask)` pair.
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

    /// This subclass's raw `Flags@4` (`0` for an unknown key) — see [`ItemSubClassInfo::flags`].
    pub fn flags(&self, class: u32, subclass: u32) -> u32 {
        self.rows.get(&(class, subclass)).map_or(0, |r| r.flags)
    }

    /// This subclass's raw `DisplayFlags` (`0` for an unknown key). Bit 0 is [`Self::hides_name`];
    /// bit 1 marks a subclass the auction house's category filter does not offer; **bit 2 (`0x4`)
    /// is the "this bag counts what is inside it" gate** — `GetInventoryItemCount` (`0x4c881a`–
    /// `0x4c8826`) reads it before summing an equipped bag's contents, and in the shipped 1.12.1
    /// file it is set on Soul Bag (1/1) and the whole Quiver class (11/0..3) and nowhere else.
    /// The bits are served raw rather than as named predicates because each consumer owns the
    /// meaning of the one it reads — the auction law lives in the auction module, not here.
    pub fn display_flags(&self, class: u32, subclass: u32) -> u32 {
        self.rows
            .get(&(class, subclass))
            .map_or(0, |r| r.display_flags)
    }

    /// Every subclass id defined for `class`, in **file order** — the order the reference's own
    /// linear scans walk (the auction house's `0x4cf9c0`/`0x4ce980` count the Nth matching row).
    /// The shipped table is small (72 rows total), so the scan costs nothing and the alternative —
    /// assuming subclass ids are dense from 0 — is false for several classes.
    pub fn subclasses_of(&self, class: u32) -> Vec<u32> {
        self.order
            .iter()
            .filter(|(c, _)| *c == class)
            .map(|&(_, s)| s)
            .collect()
    }

    /// Number of rows (for logging/diagnostics).
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether no rows loaded.
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

/// Load ItemSubClass.dbc — plus the ItemSubClassMask.dbc group names, which are stage 1 of the same
/// lookup ([`ItemSubClassCatalog::requirement_name`]) — from the patch chain.
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
        // VerboseName enUS (col 19) first, DisplayName enUS (col 10) fallback — the struct doc's
        // byte law. Empty both → no name row (the header renders blank, faithfully unlikely).
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

    /// The header-name law of `0x4fca20` on the real 5875 file: verbose-first,
    /// display fallback. Skips without client data.
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

    /// The two-stage equipped-item requirement law (`0x52eea7`–`0x52f10a`) against the real 5875
    /// DBCs —
    /// including the multi-bit case we used to give up on. Skips without client data.
    #[test]
    fn real_requirement_names_take_the_group_before_the_join() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_sub_classes(&mut chain).expect("load the two DBCs");

        // Stage 1 — the whole mask has a name of its own. All three shipped rows.
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
        // The group name is the same string in the singular arm — the stages share stage 1.
        assert_eq!(
            cat.requirement_display_name(2, 0x0002_a5f3).as_deref(),
            Some("Melee Weapon")
        );

        // Stage 2, one bit — the two spellings diverge (Shoot 5019: class 2, bit 19).
        assert_eq!(cat.requirement_name(2, 1 << 19).as_deref(), Some("Wands"));
        assert_eq!(
            cat.requirement_display_name(2, 1 << 19).as_deref(),
            Some("Wand")
        );

        // Stage 2, several bits with no group row — joined for the tooltip, first-only for the
        // cast-fail line. Daggers (15) + fist weapons (13) is not a shipped group.
        let mask = (1 << 13) | (1 << 15);
        assert_eq!(
            cat.requirement_name(2, mask).as_deref(),
            Some("Fist Weapons, Daggers")
        );
        assert_eq!(
            cat.requirement_display_name(2, mask).as_deref(),
            Some("Fist Weapon")
        );

        // Class 6 PROJECTILE — the lookup the cast-fail `0x31` NEED_EXOTIC_AMMO arm makes
        // (`0x6e1e5e: mov ecx,0x6`, mask `1 << arg`), so "Requires exotic ammo: %s" names the
        // ammo type. Only two of the five shipped rows are live in 1.12; the other three carry
        // an `(OBSOLETE)` suffix in the data itself, and one is pinned below so a schema slip
        // that silently shifted the class would be visible rather than merely returning `None`.
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
        // Past the shipped rows — the arm's own decline (`0x6e1e6a: je 0x6e21d8`, the shared
        // default), which is where an absent or wild wire word lands.
        assert_eq!(cat.requirement_display_name(6, 1 << 5), None);
        assert_eq!(cat.requirement_display_name(6, 1 << 31), None);

        // Nothing to name.
        assert_eq!(cat.requirement_name(2, 0), None);
        assert_eq!(cat.requirement_name(99, 1), None);
    }

    /// Data-gated on the real 5875 DBC. Prints the live prereq/postreq and displayFlags rows so
    /// a schema slip is visible, and pins the known shape. Skips without client data.
    #[test]
    fn item_sub_classes_load_from_the_chain() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_sub_classes(&mut chain).expect("ItemSubClass.dbc loads");
        assert!(!cat.is_empty());
        // The live alt pairs come in prerequisite/postrequisite couples per weapon family:
        // 2H Axe (2,1) ← 1H Axe via prerequisite, 1H Mace (2,4) → 2H Mace via POSTrequisite
        // (the sentinel short-circuit's second leg), 2H Sword (2,8) ← 1H Sword, and Shield
        // (4,6) ← Buckler. Print the full list so a schema slip is visible.
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
        // Daggers stand alone — no other proficiency softens a dagger's red.
        assert_eq!(cat.proficiency_alt(2, 15), None);
        // displayFlags bit 0: Miscellaneous armor (rings/trinkets/shirts) hides its type name;
        // ordinary armor and weapons show theirs.
        assert!(cat.hides_name(4, 0));
        assert!(!cat.hides_name(4, 1), "Cloth prints");
        assert!(!cat.hides_name(2, 7), "Sword prints");
    }
}
