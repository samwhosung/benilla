//! `ItemVisuals.dbc` + `ItemVisualEffects.dbc` + `SpellItemEnchantment`'s visual column — the
//! **item / enchant glow chain** (decision 0805): the permanent weapon glows and the shaman/oil
//! enchant visuals, as up to five `Spells\Enchantments\*.mdx` effect models per item.
//!
//! ## The chain
//!
//! Two sources name an **ItemVisuals id**, and both land in the same consumer:
//!
//! - the item's own **intrinsic** visual — `ItemDisplayInfo` column 22
//!   ([`crate::ItemDisplay::item_visual`], record `+0x58`), and
//! - the **enchant's** visual — `SpellItemEnchantment` field 22 (record `+0x58`), read off the
//!   item's enchant slots.
//!
//! An ItemVisuals row is `{ id, effect[5] }`; each effect is an `ItemVisualEffects` id whose one
//! payload column is the effect model's `.mdx` path. So one id = **up to five glow models**, one
//! per slot, and the slot index *is* the M2 attachment id (0..4) they hang from on the item's own
//! model (`0x479700`'s `0x712f70(glow, item, attachId = loop index)`).
//!
//! ## Layout — VERIFIED against build 5875 (both tables dumped whole)
//!
//! | table | records | fields | record size | columns |
//! |---|---|---|---|---|
//! | `ItemVisuals` | 34 | 6 | 24 | id + 5 `ItemVisualEffects` ids |
//! | `ItemVisualEffects` | 35 | 2 | 8 | id + model `.mdx` path (string) |
//! | `SpellItemEnchantment` | 1460 | 24 | 96 | id · effect[3] · pointsMin[3] · pointsMax[3] · arg[3] · name[8]+mask · **ItemVisual (22)** · flags |
//!
//! The loaders' own `cmp fieldCount/recSize` asserts (`0x548760`/`0x548530`/`0x54f6e0`) pin the
//! same shapes.
//!
//! ## The skip rules are the client's, applied here at load
//!
//! `0x479700` bounds-checks **twice**, signed, and both checks matter on the shipped data:
//!
//! - the ItemVisuals id: `jl` on negative, `jg` on `> maxId`, then a null-row test. **Five of the
//!   365 visual-carrying displays carry `-1`** — they resolve to nothing.
//! - each of the 5 effect ids, the same way — and **ItemVisuals row 28 carries two out-of-range
//!   garbage dwords** (`90148992`, `455344256`) in slots 0 and 3, which the reference skips and so
//!   do we (id 28 renders only its slot-4 `Sparkle_A`).
//! - an **empty** path string (`cmpb $0,(%eax)`) is skipped before the load. Effect id 61 is
//!   `Spells\Enchantments\` — a directory, not empty, so the reference *tries* it and the load
//!   fails; no ItemVisuals row references it, so this is theory either way.
//!
//! Every one of those becomes a `None` slot in [`ItemVisualCatalog`], so a consumer sees only real
//! model paths and can never spawn a glow the reference wouldn't.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

const ITEM_VISUALS: &str = "DBFilesClient\\ItemVisuals.dbc";
const ITEM_VISUAL_EFFECTS: &str = "DBFilesClient\\ItemVisualEffects.dbc";
const SPELL_ITEM_ENCHANTMENT: &str = "DBFilesClient\\SpellItemEnchantment.dbc";

/// The five effect slots an ItemVisuals row carries — and, one-for-one, the M2 attachment ids
/// `0..4` on the item model each one hangs from.
pub const ITEM_VISUAL_SLOTS: usize = 5;

/// `ItemVisuals.dbc` joined with `ItemVisualEffects.dbc`: an ItemVisuals id → its five glow-model
/// paths (raw `.mdx` references, as the DBC stores them — the app's `m2_url` owns the `.m2` swap),
/// `None` where the slot is empty, out of range, or names an empty path.
pub struct ItemVisualCatalog {
    visuals: HashMap<u32, [Option<String>; ITEM_VISUAL_SLOTS]>,
}

impl ItemVisualCatalog {
    /// The five glow-model slots for an ItemVisuals id, or `None` when the id names no row.
    ///
    /// Takes the id **signed**, because that is how the client reads it: `0x479700` tests `jl`
    /// before its `maxId` compare, so `-1` (five shipped `ItemDisplayInfo` rows) and `0` name
    /// nothing. `> 0` with no row is equally nothing.
    pub fn effects(&self, visual_id: i32) -> Option<&[Option<String>; ITEM_VISUAL_SLOTS]> {
        (visual_id > 0).then(|| self.visuals.get(&(visual_id as u32)))?
    }

    /// Build from an explicit map — tests and synthetic fixtures.
    pub fn from_visuals(visuals: HashMap<u32, [Option<String>; ITEM_VISUAL_SLOTS]>) -> Self {
        ItemVisualCatalog { visuals }
    }

    pub fn len(&self) -> usize {
        self.visuals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visuals.is_empty()
    }
}

/// `SpellItemEnchantment.dbc`'s consumer columns, from one load of the one table: the **visual**
/// (field 22) an enchant glows with, the **name** (field 13, the enUS slot of the 1.12
/// localized-string block) the item tooltip prints for it, and its **`Flags`** (field 23 —
/// [`EnchantCatalog::binds_the_item`] and [`EnchantCatalog::tooltip_hides_name`]). Three lanes
/// read this table — the weapon-glow chain (decision 0805), the tooltip's enchant line (decision
/// 0915) and the item-bind confirms (decision 0928) — and one adapter serves all three: two
/// loaders over one DBC is how a schema drifts.
///
/// Sparse on every axis: 102 of the 1460 rows carry a visual, and a row without a name simply has
/// none. The rest of the table (effects, points, args) belongs to whoever needs it, not here.
pub struct EnchantCatalog {
    visuals: HashMap<u32, i32>,
    names: HashMap<u32, String>,
    /// `Flags` (field 23) for **every** row the table carries, `0` included — so this map's key
    /// set is also the reference's `enchantTable[id] != 0` ([`EnchantCatalog::has_row`]).
    flags: HashMap<u32, u32>,
}

/// `Flags & 0x1` — applying this enchant **binds the item to you**. Only two sites in the whole
/// binary read this bit, and both ask that one question: the enchant-apply gate `0x495d60` @
/// `495ea3` (`testb $0x1, 0x5c(%eax)` — the sole gate on the "Enchanting this item will bind it to
/// you." confirm, event 402) and `0x5da2c0` @ `5da313`, which walks an item's seven live enchant
/// slots asking whether it already carries one (and so has already been asked). 86 of the 1460
/// shipped rows: the shaman weapon imbues, every rogue poison, Firestone, and the Zul'Gurub /
/// Ahn'Qiraj head-and-leg enchants. The permanent profession enchants are NOT among them, and
/// neither are sharpening stones, weightstones, oils or scopes.
const FLAG_BINDS_THE_ITEM: u32 = 0x1;

/// `Flags & 0x2` — **suppress the tooltip line** for this enchant. The two item-tooltip enchant-line
/// printers both open with it and return outright when it is set (`6290e4` and `62923e`, each
/// `testb $0x2, 0x5c(...)` → `jne` straight to the function's `retl`), *before* they read the name
/// at `+0x34`. 12 shipped rows, and they are one coherent family: the totem-granted weapon imbues
/// (Flametongue Totem 1-4, Windfury Totem 1-3) plus the warlock Firestone 1-4 and Orb of Fire —
/// buffs whose source is already visible elsewhere on screen. The *replace* confirm reads the name
/// with no such gate (`4960d0`), so a suppressed enchant still names itself in that popup.
const FLAG_TOOLTIP_HIDES_NAME: u32 = 0x2;

impl EnchantCatalog {
    /// The ItemVisuals id an enchant glows with, or `None` for an enchant with no visual. The
    /// value is signed for the same reason as [`ItemVisualCatalog::effects`] — one shipped row
    /// carries `-1`.
    pub fn visual(&self, enchant_id: u32) -> Option<i32> {
        self.visuals.get(&enchant_id).copied()
    }

    /// The enchant's display name as the table stores it — `"Agility +15"`, `"Crusader"`,
    /// `"Stamina +7"`. `None` for an unknown id or a row with an empty name string.
    pub fn name(&self, enchant_id: u32) -> Option<&str> {
        self.names.get(&enchant_id).map(String::as_str)
    }

    /// [`FLAG_BINDS_THE_ITEM`] — whether applying this enchant binds the item to you, the whole
    /// predicate behind the reference's bind confirm (decision 0928). `false` for an unknown id,
    /// which is also the reference's answer: it looks the row up first and skips on a miss.
    ///
    /// `+0x5c` is field 23, and the field-13 name column chain-locks it: the replace confirm reads
    /// its two names at `4960d0`/`4960d4` as `0x34(%row,%locale,4)`, and `0x34/4 == 13` is exactly
    /// [`Self::name`]'s column — so the eight localized name slots run 13..20, `NameFlags` is 21,
    /// `ItemVisual` 22 (the glow lane's, and `5d9be1: movl 0x58(%eax)` confirms it), and `Flags`
    /// 23. vmangos's own `SpellItemEnchantmentEntry` agrees field-for-field.
    pub fn binds_the_item(&self, enchant_id: u32) -> bool {
        self.flag(enchant_id, FLAG_BINDS_THE_ITEM)
    }

    /// [`FLAG_TOOLTIP_HIDES_NAME`] — whether the item tooltip must print NO line for this enchant.
    /// The enchant-line lane gates on it (`crate::items::enchant_lines` in the app).
    pub fn tooltip_hides_name(&self, enchant_id: u32) -> bool {
        self.flag(enchant_id, FLAG_TOOLTIP_HIDES_NAME)
    }

    fn flag(&self, enchant_id: u32, bit: u32) -> bool {
        self.flags.get(&enchant_id).is_some_and(|f| f & bit != 0)
    }

    /// Whether the id names a real row at all — the reference's `testl %eax,%eax` after every one
    /// of its `enchantTable[id]` loads. Both confirms need it: an id that names nothing raises no
    /// popup and blocks no bind.
    pub fn has_row(&self, enchant_id: u32) -> bool {
        self.flags.contains_key(&enchant_id)
    }

    /// Build from explicit maps — tests and synthetic fixtures. `flags` is field 23 per row and
    /// doubles as the row census, so a fixture that only sets `names`/`visuals` must list its ids
    /// there too for [`Self::has_row`] to see them.
    pub fn from_rows(
        visuals: HashMap<u32, i32>,
        names: HashMap<u32, String>,
        flags: HashMap<u32, u32>,
    ) -> Self {
        EnchantCatalog {
            visuals,
            names,
            flags,
        }
    }

    /// Iterate `(enchant id, ItemVisuals id)` for the rows that carry one (order unspecified) —
    /// the cross-table join checks read it.
    pub fn iter_visuals(&self) -> impl Iterator<Item = (u32, i32)> + '_ {
        self.visuals.iter().map(|(k, v)| (*k, *v))
    }

    /// How many enchants carry a glow — the glow lane's startup census.
    pub fn visual_count(&self) -> usize {
        self.visuals.len()
    }

    /// How many enchants carry a printable name — the tooltip lane's.
    pub fn name_count(&self) -> usize {
        self.names.len()
    }
}

/// `ItemVisuals.dbc` — 6 fields / 24-byte records in build 5875.
pub(crate) fn item_visuals_schema() -> Schema {
    let mut s = Schema::new("ItemVisuals");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..ITEM_VISUAL_SLOTS {
        s.add_field(SchemaField::new(
            format!("Effect{i}"),
            FieldType::UInt32, // signed in use — see the module doc's skip rules
        ));
    }
    s
}

/// `ItemVisualEffects.dbc` — 2 fields / 8-byte records in build 5875.
pub(crate) fn item_visual_effects_schema() -> Schema {
    let mut s = Schema::new("ItemVisualEffects");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Model", FieldType::String));
    s
}

/// `SpellItemEnchantment.dbc` — 24 fields / 96-byte records in build 5875. Fields 13 (`Name0`)
/// and 22 (`ItemVisual`) are read; the rest are typed to keep the field-count check exact. The
/// eight `Name_Lang` slots + their mask are the 1.12 localized-string block (only `enUS`, the
/// first, is filled).
pub(crate) fn spell_item_enchantment_schema() -> Schema {
    let mut s = Schema::new("SpellItemEnchantment");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for group in ["Effect", "EffectPointsMin", "EffectPointsMax", "EffectArg"] {
        for i in 0..3 {
            s.add_field(SchemaField::new(format!("{group}{i}"), FieldType::UInt32));
        }
    }
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.add_field(SchemaField::new("ItemVisual", FieldType::UInt32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s
}

/// Load `ItemVisuals.dbc` + `ItemVisualEffects.dbc` off the patch chain and join them into an
/// [`ItemVisualCatalog`] — the client's per-slot skip rules (module doc) applied at load, so a
/// resolved slot is always a real model path.
pub fn load_item_visual_catalog(chain: &mut Chain) -> Result<ItemVisualCatalog> {
    let bytes = chain
        .read_file(ITEM_VISUAL_EFFECTS)
        .with_context(|| format!("reading {ITEM_VISUAL_EFFECTS}"))?;
    let rs = parse(&bytes, item_visual_effects_schema(), "ItemVisualEffects")?;
    // `str_at` already drops the empty string — the reference's own `cmpb $0,(%eax)` skip.
    let mut effects: HashMap<u32, String> = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(model)) = (u32_at(r, 0), str_at(&rs, r, 1)) else {
            continue;
        };
        effects.insert(id, model);
    }

    let bytes = chain
        .read_file(ITEM_VISUALS)
        .with_context(|| format!("reading {ITEM_VISUALS}"))?;
    let rs = parse(&bytes, item_visuals_schema(), "ItemVisuals")?;
    let mut visuals = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let slots = std::array::from_fn(|i| {
            let raw = u32_at(r, 1 + i).unwrap_or(0) as i32;
            (raw > 0)
                .then(|| effects.get(&(raw as u32)).cloned())
                .flatten()
        });
        visuals.insert(id, slots);
    }
    Ok(ItemVisualCatalog { visuals })
}

/// Load `SpellItemEnchantment.dbc`'s two consumer columns off the patch chain — one parse, both
/// lanes (see [`EnchantCatalog`]).
pub fn load_enchant_catalog(chain: &mut Chain) -> Result<EnchantCatalog> {
    let bytes = chain
        .read_file(SPELL_ITEM_ENCHANTMENT)
        .with_context(|| format!("reading {SPELL_ITEM_ENCHANTMENT}"))?;
    let rs = parse(
        &bytes,
        spell_item_enchantment_schema(),
        "SpellItemEnchantment",
    )?;
    let mut visuals = HashMap::new();
    let mut names = HashMap::new();
    let mut flags = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        // Every row lands here, `Flags == 0` included — the map's key set IS the row census.
        flags.insert(id, u32_at(r, 23).unwrap_or(0));
        let visual = u32_at(r, 22).unwrap_or(0) as i32;
        if visual != 0 {
            visuals.insert(id, visual);
        }
        // `str_at` drops the empty string, so a nameless row simply never lands.
        if let Some(name) = str_at(&rs, r, 13) {
            names.insert(id, name);
        }
    }
    Ok(EnchantCatalog {
        visuals,
        names,
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two glow tables as they actually ship, including both traps: row **28**'s two
    /// out-of-range garbage slots and the reference's per-slot skip.
    #[test]
    fn real_item_visuals_join_their_effect_models() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_visual_catalog(&mut chain).expect("load ItemVisuals");
        assert_eq!(cat.len(), 34, "34 ItemVisuals rows in build 5875");

        // The common shape: one effect repeated across all five attach slots.
        let all_blue = ["Spells\\Enchantments\\BlueGlow_Med.mdx"; ITEM_VISUAL_SLOTS];
        let got = cat.effects(2).expect("visual 2");
        assert_eq!(
            got.each_ref().map(|s| s.as_deref().unwrap_or("")),
            all_blue,
            "visual 2 glows on every slot"
        );

        // A single-slot shape — slot 3 only.
        let one = cat.effects(1).expect("visual 1");
        assert_eq!(
            one[3].as_deref(),
            Some("Spells\\Enchantments\\SkullBalls.mdx")
        );
        assert!(
            [0, 1, 2, 4].iter().all(|&i| one[i].is_none()),
            "visual 1 authors only slot 3"
        );

        // A mixed row: slot 0 differs from the other four.
        let rune = cat.effects(30).expect("visual 30");
        assert_eq!(
            rune[0].as_deref(),
            Some("Spells\\Enchantments\\Rune_Intellect.mdx")
        );
        assert_eq!(
            rune[4].as_deref(),
            Some("Spells\\Enchantments\\YellowGlow_Low.mdx")
        );

        // **The garbage row.** Slots 0 and 3 hold 90148992 / 455344256 — far past the effect
        // table's maxId (152) — and the reference's `jg` skips them.
        let junk = cat.effects(28).expect("visual 28");
        assert_eq!(
            junk.each_ref().map(|s| s.is_some()),
            [false, false, false, false, true],
            "only the in-range slot-4 effect survives row 28"
        );
        assert_eq!(
            junk[4].as_deref(),
            Some("Spells\\Enchantments\\Sparkle_A.mdx")
        );

        // The id gate: 0 and the shipped -1 name nothing.
        assert!(cat.effects(0).is_none());
        assert!(cat.effects(-1).is_none());
        assert!(cat.effects(9999).is_none(), "past maxId");
    }

    /// The display half of the join, on real data: 365 of the 29 604 displays carry a visual, five
    /// of them the unresolvable `-1`, and every other one resolves to at least one glow model.
    #[test]
    fn real_displays_carrying_a_visual_all_resolve() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let visuals = load_item_visual_catalog(&mut chain).expect("load ItemVisuals");
        let displays = crate::load_item_display_catalog(&mut chain).expect("load ItemDisplayInfo");

        let (mut carried, mut minus_one, mut resolved, mut models) = (0, 0, 0, 0);
        for (_, d) in displays.iter() {
            if d.item_visual == 0 {
                continue;
            }
            carried += 1;
            if d.item_visual == -1 {
                minus_one += 1;
            }
            match visuals.effects(d.item_visual) {
                Some(slots) => {
                    resolved += 1;
                    models += slots.iter().flatten().count();
                }
                None => assert_eq!(
                    d.item_visual, -1,
                    "the only unresolvable visual ids on the shipped table are -1"
                ),
            }
        }
        assert_eq!(carried, 365, "displays carrying a nonzero ItemVisuals id");
        assert_eq!(minus_one, 5, "…of which five are the skipped -1");
        assert_eq!(resolved, 360);
        assert_eq!(
            models, 1588,
            "glow-model instances the shipped displays add up to"
        );
    }

    /// The enchant half: `SpellItemEnchantment` field 22 lands in the ItemVisuals id space on
    /// every shipped row but one (`-1`) — the independent corroboration that column 22 (record
    /// `+0x58`) is the visual, and the three shaman weapon buffs to name it.
    #[test]
    fn real_enchant_visuals_resolve() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let visuals = load_item_visual_catalog(&mut chain).expect("load ItemVisuals");
        let enchants = load_enchant_catalog(&mut chain).expect("load SpellItemEnchantment");
        assert_eq!(enchants.visual_count(), 102, "enchants carrying a visual");

        let resolved = enchants
            .iter_visuals()
            .filter(|(_, v)| visuals.effects(*v).is_some())
            .count();
        assert_eq!(resolved, 101, "…all but the one -1 row resolve to a row");

        // Rockbiter 3 (enchant 1) → visual 61 → the slot-3 rock glow.
        let rockbiter = visuals
            .effects(enchants.visual(1).expect("enchant 1 has a visual"))
            .expect("visual 61");
        assert_eq!(
            rockbiter[3].as_deref(),
            Some("Spells\\Enchantments\\Shaman_Rock.mdx")
        );
        // A sharpening stone (enchant 13) → visual 28 → the garbage row's surviving sparkle.
        let sharpened = visuals
            .effects(enchants.visual(13).expect("enchant 13 has a visual"))
            .expect("visual 28");
        assert_eq!(
            sharpened[4].as_deref(),
            Some("Spells\\Enchantments\\Sparkle_A.mdx")
        );
        // A plain +stat enchant carries none (241 "Weapon Damage +2", 929 "Stamina +7").
        assert_eq!(enchants.visual(241), None);
        assert_eq!(enchants.visual(929), None);
    }

    /// The **name** column (field 13), on real data — the string the tooltip's enchant line
    /// prints (decision 0915). Pinned on the case that opened the lane plus one of each other
    /// shape, and on the two properties the consumer rests on: the name is stored in the table's
    /// own word order (`"Agility +15"`, NOT a reformat), and it is independent of the visual
    /// column — a glowing enchant and a plain +stat one both have one.
    #[test]
    fn real_enchant_names_read_off_the_table() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let enchants = load_enchant_catalog(&mut chain).expect("load SpellItemEnchantment");

        // 2564 = the permanent weapon enchant on the director's Hatchet of Sundered Bone: a name
        // AND a visual (125 → GreenGlow_Low), the two columns joined on one row.
        assert_eq!(enchants.name(2564), Some("Agility +15"));
        assert_eq!(enchants.visual(2564), Some(125));
        // Named, no glow — the +stat family.
        assert_eq!(enchants.name(241), Some("Weapon Damage +2"));
        assert_eq!(enchants.name(929), Some("Stamina +7"));
        // Glow, and a name that is a proper noun rather than a stat phrase.
        assert_eq!(enchants.name(1900), Some("Crusader"));
        // An id past the table names nothing at all.
        assert_eq!(enchants.name(999_999), None);
        assert!(
            enchants.name_count() > enchants.visual_count(),
            "far more enchants print a name than carry a glow"
        );
    }

    /// The `Flags` column (field 23) as it actually ships — the two bits the reference reads, and
    /// the shape of each one's set. Written the way the mapping was *settled*: the name column at
    /// field 13 is chain-locked by the binary reading it as `0x34(%row,%locale,4)`, `ItemVisual`
    /// at 22 by `5d9be1: movl 0x58(%eax)`, so `Flags` is 23 (`+0x5c`) — which is exactly what the
    /// two `testb $0x1,0x5c` / `testb $0x2,0x5c` sites read.
    ///
    /// A column slip fails this hard, because both sets are *characterized*, not just counted: bit
    /// 0's members are weapon imbues / poisons / raid enchants and bit 1's are exactly the twelve
    /// totem-and-Firestone rows. This also pins the negative that surprised us and is worth not
    /// re-learning: the permanent profession enchants (Agility +15, Crusader, Fiery Weapon) and
    /// every consumable-applied enchant (stones, oils, scopes) have `Flags == 0`, so the bind
    /// confirm does NOT fire for them.
    #[test]
    fn real_enchant_flags_column() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let e = load_enchant_catalog(&mut chain).expect("load SpellItemEnchantment");

        // Bit 0 — the bind confirm's gate. 86 rows: the shaman imbues, the rogue poisons, the
        // warlock Firestone, and the ZG/AQ head-and-leg enchants.
        for (id, name) in [
            (1u32, "Rockbiter 3"),
            (7, "Deadly Poison"),
            (283, "Windfury 1"),
            (2488, "+5 All Resistances"),
            (2606, "+30 Attack Power"),
        ] {
            assert!(e.binds_the_item(id), "{id} ({name}) must carry Flags & 1");
        }
        // The negative half — the one that makes this bit counter-intuitive.
        for (id, name) in [
            (2564u32, "Agility +15"),
            (1900, "Crusader"),
            (803, "Fiery Weapon"),
            (40, "Sharpened +2"),
            (2627, "Wizard Oil"),
            (33, "Scope (+3 Damage)"),
        ] {
            assert!(
                !e.binds_the_item(id),
                "{id} ({name}) must NOT carry Flags & 1"
            );
        }

        // Bit 1 — the tooltip's name suppression. Exactly twelve rows, all one family.
        let hidden: std::collections::BTreeSet<u32> =
            (0..3000).filter(|&id| e.tooltip_hides_name(id)).collect();
        assert_eq!(
            hidden,
            [124, 285, 303, 543, 563, 564, 1683, 1783, 1803, 1823, 1824, 1825]
                .into_iter()
                .collect(),
            "the totem-granted imbues (Flametongue/Windfury Totem), Orb of Fire and Firestone 1-4"
        );
        // Suppression is about the tooltip LINE, not the row: the names are still there, and the
        // replace confirm reads them ungated (`4960d0`).
        assert_eq!(e.name(124), Some("Flametongue Totem 1"));

        // The row census the confirms gate on — present for a real id, absent past the table.
        assert!(e.has_row(2564) && e.has_row(1) && !e.has_row(999_999));
    }
}
