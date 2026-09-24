//! `Spell.dbc` joined with `SpellIcon.dbc`: a spell id resolves to the display record the action
//! bar, spellbook, tooltips and cast gates read.
//!
//! A `Spell.dbc` record is 173 fields, 692 bytes (the loader's header checks, `0x55007e`,
//! `0x5500bb`), and the client's `SpellRec` holds the row as stored: offset = column × 4, so
//! `SpellVisual` is `+0x1cc` (column 115) and the name `+0x1e0` (column 120). The client's reads
//! agree, such as Speed `+0x94` at `0x6e814b`, `Attributes` and `AttributesEx2` at `0x6e5922` and
//! `0x6e591c`, `castUI` at `0x4b29bf`. Most columns were also matched by value against the
//! vmangos `spell_template` of build 5875: only column 115 equals `spellVisual1` on 1309 rows.
//!
//! A localized string is eight locale dwords and a flags dword: name 120-128, subtext 129-137,
//! description 138-146, aura description 147-155, then `ManaCostPercentage` at 156. The
//! per-effect fields are `[3]` arrays in consecutive columns from `Effect` at 61.
//! `SpellIcon.dbc` is `ID`, `TextureFilename` (no extension).

mod cast_times;
mod dispel_types;
mod display;
mod duration;
mod forms;
mod radius;
mod ranges;
mod tokens;

pub use cast_times::{load_spell_cast_times, SpellCastTime, SpellCastTimeCatalog};
pub use dispel_types::{load_spell_dispel_types, SpellDispelTypes};
pub use display::{FormRefusal, LearnAnnouncement, OpenLock, SpellDisplay};
pub use duration::{load_spell_durations, SpellDuration, SpellDurationCatalog};
pub use forms::{load_shapeshift_forms, ShapeshiftForm};
mod immunity;
pub use immunity::{cc_exemption, grants_immunity, CcExemption};
pub use radius::{load_spell_radii, SpellRadius, SpellRadiusCatalog};
pub use ranges::{
    load_spell_ranges, min_max_range, SpellRange, SpellRangeCatalog, COMBAT_REACH_ADD,
    MELEE_RANGE_FLOOR, ON_NEXT_SWING_RANGE,
};
pub use tokens::{substitute, TokenContext};

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse, str_at, u32_at};

const SPELL: &str = "DBFilesClient\\Spell.dbc";

const SPELL_FIELDS: usize = 173;
/// `Category` (`+0x8`): the shared-cooldown `SpellCategory.dbc` id, such as potions 4.
const COL_CATEGORY: usize = 2;
const COL_CAST_UI: usize = 3;
/// `RecoveryTime` (`+0x4c`) and `CategoryRecoveryTime` (`+0x50`), in ms, read by `StartCooldown`
/// (`0x6e2c60`) and the `SMSG_SPELL_COOLDOWN` handler (`0x6e9460`).
const COL_RECOVERY_TIME: usize = 19;
const COL_CATEGORY_RECOVERY_TIME: usize = 20;
/// The three interrupt masks (`+0x54`, `+0x58`, `+0x5c`): cast, applied aura, running channel.
const COL_INTERRUPT_FLAGS: usize = 21;
const COL_AURA_INTERRUPT_FLAGS: usize = 22;
const COL_CHANNEL_INTERRUPT_FLAGS: usize = 23;
/// The cost triple (`powerType`, `manaCost`, `ManaCostPercentage`) `IsUsableAction` checks.
const COL_POWER_TYPE: usize = 31;
const COL_MANA_COST: usize = 32;
const COL_MANA_COST_PCT: usize = 156;
const COL_MANA_COST_PER_LEVEL: usize = 33;
const COL_MANA_PER_SECOND: usize = 34;
/// `rangeIndex` (`+0x90`), resolved by `GetMinMaxRange` (`0x6e3480`).
const COL_RANGE_INDEX: usize = 36;
/// `modalNextSpell` (`+0x98`): 52 of its 57 nonzero rows are hunter shots naming Auto Shot.
const COL_MODAL_NEXT_SPELL: usize = 38;
/// `StartRecoveryCategory` (`+0x274`) and `StartRecoveryTime` (`+0x278`): the global cooldown
/// `StartGlobalCooldown` (`0x6e2de0`) starts at the cast send (`0x6e58fb`).
const COL_START_RECOVERY_CATEGORY: usize = 157;
const COL_START_RECOVERY_TIME: usize = 158;
const COL_PREVENTION_TYPE: usize = 165;
const COL_SPELL_FAMILY_NAME: usize = 160;
const COL_SPELL_FAMILY_FLAGS_LOW: usize = 161;
/// `Targets` (`+0x34`): the seed the cast arm loads into its targeting word (`0x6e525a`).
const COL_TARGETS: usize = 13;
/// `EffectImplicitTargetA[0]` (`+0x148`), the key of the cast arm's 62-case switch (`0x6e5484`).
const COL_IMPLICIT_TARGET_A1: usize = 82;
/// `EffectImplicitTargetB[0]` (`+0x154`), walked beside A by the classifier `0x6ea280`.
const COL_IMPLICIT_TARGET_B1: usize = 85;
/// The usable walk's gate columns (`IsSpellUsableNow` `0x6e3d60`).
const COL_STANCES: usize = 11;
const COL_STANCES_NOT: usize = 12;
const COL_CASTER_AURA_STATE: usize = 16;
const COL_TARGET_AURA_STATE: usize = 17;
const COL_TOTEM_1: usize = 40;
const COL_REAGENT_1: usize = 42;
const COL_REAGENT_COUNT_1: usize = 50;
const COL_EQUIPPED_ITEM_CLASS: usize = 58;
const COL_EQUIPPED_ITEM_SUBCLASS_MASK: usize = 59;
/// `EquippedItemInventoryTypeMask` (`+0xf0`), read by the item-target gate `0x495d60`.
const COL_EQUIPPED_ITEM_INVENTORY_TYPE_MASK: usize = 60;
const COL_REQUIRES_SPELL_FOCUS: usize = 15;
const COL_DISPEL: usize = 4;
const COL_SCHOOL: usize = 1;
const COL_MECHANIC: usize = 5;
const COL_EFFECT_MECHANIC_1: usize = 79;
const COL_ATTRIBUTES: usize = 6;
const COL_ATTRIBUTES_EX: usize = 7;
const COL_ATTRIBUTES_EX2: usize = 8;
const COL_ATTRIBUTES_EX3: usize = 9;
const COL_SPEED: usize = 37;
const COL_EFFECT_1: usize = 61;
/// `EffectMiscValue[0]`, `61 + 15 × 3`: the 16th `[3]` array from `Effect`.
const COL_EFFECT_MISC_1: usize = 106;
/// `EffectTriggerSpell[0]`, `61 + 16 × 3`.
const COL_EFFECT_TRIGGER_1: usize = 109;
/// `SPELL_EFFECT_LEARN_SPELL`: teaches its `EffectTriggerSpell`, as a trainer's wrapper does.
pub const SPELL_EFFECT_LEARN_SPELL: u32 = 36;

/// One learn effect, in the slot order the trainer's state re-evaluator reads (`0x4d7d40`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnEffect {
    /// `SPELL_EFFECT_LEARN_SPELL`: teaches `EffectTriggerSpell`.
    Spell(u32),
    /// `SPELL_EFFECT_SKILL_STEP`: raises skill line `skill` (`EffectMiscValue`) to `step`,
    /// `EffectBasePoints + EffectDieSides`.
    SkillStep { skill: u32, step: u32 },
    /// `SPELL_EFFECT_LEARN_PET_SPELL`: teaches the pet `EffectTriggerSpell`.
    PetSpell(u32),
}
/// `SPELL_EFFECT_LEARN_PET_SPELL`: the trainer's icon accepts it or 36 in any slot (`0x4d8ff5`),
/// but [`SpellCatalog::learned_spell`] follows 36 only.
pub const SPELL_EFFECT_LEARN_PET_SPELL: u32 = 57;
/// `SPELL_EFFECT_SKILL_STEP`: raises a profession's cap; a tradeskill trainer's tree partitions
/// on it alone, in the wire spell's three slots (`0x4d77b6`).
pub const SPELL_EFFECT_SKILL_STEP: u32 = 44;

/// `SPELL_EFFECT_OPEN_LOCK`, which the GameObject interact-cast matches (`0x5f84a1`).
const SPELL_EFFECT_OPEN_LOCK: u32 = 0x21;
/// `baseLevel` (`+0x70`, not `spellLevel`), read by the effect-value walk at `0x6e3854`.
const COL_BASE_LEVEL: usize = 28;
/// `maxLevel` (`+0x6c`): 0 is uncapped, as on the profession openers, not a cap of 0.
const COL_MAX_LEVEL: usize = 27;
const COL_SPELL_LEVEL: usize = 29;
/// `EffectApplyAuraName[0]`, `61 + 10 × 3`; the stance bar admits by it (`0x4b2810`).
const COL_EFFECT_APPLY_AURA_1: usize = 91;
/// `SPELL_AURA_MOD_SHAPESHIFT`: its `EffectMiscValue` is the `SpellShapeshiftForm.dbc` id.
const SPELL_AURA_MOD_SHAPESHIFT: u32 = 36;
const COL_ICON_ID: usize = 117;
/// `ActiveIconID` (`+0x1d8`, read at `0x4b4754`): druid forms carry 122, the dismiss paw.
const COL_ACTIVE_ICON_ID: usize = 118;
/// `StanceBarOrder` (`+0x298`, signed): `0x4b2bb0` sorts ascending, -1 last, ties by spell id;
/// Stealth is -1.
const COL_STANCE_BAR_ORDER: usize = 166;
const COL_VISUAL_ID: usize = 115;
const COL_NAME_ENUS: usize = 120;
const COL_NAME_SUBTEXT_ENUS: usize = 129;
const COL_DESCRIPTION_ENUS: usize = 138;
const COL_AURA_DESCRIPTION_ENUS: usize = 147;

/// `DurationIndex` (`+0x78`): `Spell_C::GetSpellDuration` (`0x6ea000`) looks it up at `0xc0d828`.
const COL_DURATION_INDEX: usize = 30;
/// `CastingTimeIndex` (`+0x48`): `Spell_C::GetCastTime` (`0x6e3340`) looks it up at `0xc0d878`.
const COL_CASTING_TIME_INDEX: usize = 18;
const COL_PROC_CHANCE: usize = 25;

/// The per-effect `[3]` arrays, each constant slot 0; die sides and base points are signed.
const COL_EFFECT_DIE_SIDES_1: usize = 64;
const COL_EFFECT_BASE_DICE_1: usize = 67;
const COL_EFFECT_DICE_PER_LEVEL_1: usize = 70;
const COL_EFFECT_REAL_POINTS_PER_LEVEL_1: usize = 73;
const COL_EFFECT_BASE_POINTS_1: usize = 76;
const COL_EFFECT_RADIUS_INDEX_1: usize = 88;
const COL_EFFECT_AMPLITUDE_1: usize = 94;
const COL_EFFECT_MULTIPLE_VALUE_1: usize = 97;
const COL_EFFECT_CHAIN_TARGETS_1: usize = 100;
const COL_EFFECT_ITEM_TYPE_1: usize = 103;

/// `SPELL_ATTR3_NORMAL_RANGED_ATTACK`: damage floats melee white (`0x6128b0`).
const ATTR_EX3_NORMAL_RANGED_ATTACK: u32 = 0x8000;
/// `SPELL_ATTR_EX3_NO_CASTING_BAR_TEXT` (vmangos `SpellDefines.h:907`).
const ATTR_EX3_NO_CASTING_BAR_TEXT: u32 = 0x4;
/// `AttributesEx3` bit 13: `0x6e7595` tests it as `0x20` in the word's second byte.
const ATTR_EX3_NO_CHANNEL_BAR: u32 = 0x2000;
/// `AttributesEx` bit 29: the channel bar names the spell, not "Channeling" (`0x6e759a`).
const ATTR_EX_CHANNEL_BAR_OWN_NAME: u32 = 0x2000_0000;
/// `SPELL_ATTR_RANGED`: the spell uses the ranged slot.
const ATTR_RANGED: u32 = 0x2;
/// `Attributes` bit 9: target the equipped main hand. vmangos leaves it unnamed
/// (`SPELL_ATTR_UNK9`): the server only sees the resolved item guid.
const ATTR_TARGET_MAIN_HAND_ITEM: u32 = 0x200;
/// `SPELL_ATTR_EX2_AUTO_REPEAT`: Auto Shot and wand Shoot.
const ATTR_EX2_AUTO_REPEAT: u32 = 0x20;
const ATTR_EX2_DO_NOT_RESET_COMBAT_TIMERS: u32 = 0x20000;
/// `SPELL_ATTR_PASSIVE`: the spellbook grays the spell (`SpellBookFrame.lua:379-390`).
const ATTR_PASSIVE: u32 = 0x40;
/// `SPELL_ATTR_ABILITY`, read by the 1.12 client only to word the learn line (`0x4b29a9`).
const ATTR_ABILITY: u32 = 0x10;
/// `SPELL_ATTR_DO_NOT_DISPLAY`: languages, proficiencies, hidden racials, internal proc auras.
const ATTR_DO_NOT_DISPLAY: u32 = 0x80;
/// `SPELL_ATTR_IS_TRADESKILL`: a profession or recipe spell, kept out of the book; a trainer
/// service with it shows an item tooltip (`SetTrainerService` `0x5338b0` at `0x533a1b`).
pub const SPELL_ATTR_IS_TRADESKILL: u32 = 0x20;
/// `SPELL_ATTR_EX_NO_AURA_ICON`: hidden from the aura bar, as all three warrior stances are.
const ATTR_EX_NO_AURA_ICON: u32 = 0x1000_0000;

/// `SPELL_EFFECT_ATTACK`: only 6603 "Attack" has it, but the client tests the effect, not the id.
const SPELL_EFFECT_ATTACK: u32 = 78;

/// The tracking aura types, which no aura display shows (`IsAuraDisplayable` `0x519860`).
const TRACKING_AURA_TYPES: [u32; 3] = [44, 45, 151];

/// The on-next-swing pair `0x4` and `0x400`, always tested as one mask (`0x6e34fb`, `0x6e5200`).
const ATTR_ON_NEXT_SWING: u32 = 0x404;
/// `SPELL_ATTR_EX_INITIATES_COMBAT`: casting starts auto-attack, on the client (`0x6e5200`); the
/// server reads it only for pet AI (vmangos `Spell.cpp:4377`).
const ATTR_EX_INITIATES_COMBAT: u32 = 0x200;
/// `AttributesEx` `0x4` or `0x40`: the two channeled variants, tested as one mask (`0x52ec27`).
const ATTR_EX_CHANNELED: u32 = 0x44;
/// `SPELL_ATTR_EX2_INITIATE_COMBAT_POST_CAST`: auto-attack starts at the spell's `SMSG_SPELL_GO`
/// (`0x6e83c0`), not at the send: the stealth openers, positional strikes and Judgement.
const ATTR_EX2_INITIATE_COMBAT_POST_CAST: u32 = 0x0010_0000;

/// `SPELL_ATTR_COOLDOWN_ON_EVENT`: held by the `SMSG_SPELL_COOLDOWN` handler (`0x6e9460`).
const ATTR_COOLDOWN_ON_EVENT: u32 = 0x0200_0000;

/// The usable walk's attribute gates (`0x6e3d60`): bit 23 castable while dead (`0x6e3dbd`), bit
/// 17 only while stealthed (`0x6e3ee3`, the creep flag), bit 16 not while shapeshifted, bit 28
/// only out of combat (`0x6e3f01`).
pub const ATTR_CASTABLE_WHILE_DEAD: u32 = 0x0080_0000;
pub const ATTR_ONLY_STEALTHED: u32 = 0x0002_0000;
const ATTR_NOT_SHAPESHIFT: u32 = 0x0001_0000;
pub const ATTR_NOT_IN_COMBAT: u32 = 0x1000_0000;
/// `SPELL_ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED`: waives the form requirement while unshifted.
const ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED: u32 = 0x0008_0000;
/// The combo-point consumers, `AttributesEx` bits 20 and 22 (vmangos `NeedsComboPoints`), read
/// by the usable walk at `0x6e3e7a`: the rogue and druid finishers, and Overpower.
const ATTR_EX_FINISHING_MOVE: u32 = 0x0050_0000;
/// `SPELL_EFFECT_TRADE_SKILL`: usable with no other gate (`0x6e3d99`), and `TryCast`
/// (`0x6e4b60`) opens the crafting window itself instead of sending the cast.
pub const SPELL_EFFECT_TRADE_SKILL: u32 = 47;
/// `SPELL_EFFECT_CREATE_ITEM`: a recipe's product, its `EffectItemType`.
pub const SPELL_EFFECT_CREATE_ITEM: u32 = 24;
/// `SPELL_EFFECT_SKINNING`: learning a spell with it as `Effect[0]` latches it at `0xb700e4`
/// (`0x4b2623`), and the Skin cursor requires that latch.
pub const SPELL_EFFECT_SKINNING: u32 = 95;
/// `SPELL_EFFECT_LANGUAGE`: its `EffectMiscValue` is the `Languages.dbc` id.
pub const SPELL_EFFECT_LANGUAGE: u32 = 39;

/// The permanent and temporary enchants, cast by the CraftFrame with `TARGET_FLAG_ITEM`.
pub const SPELL_EFFECT_ENCHANT_ITEM: u32 = 53;
pub const SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY: u32 = 54;

/// `SPELL_EFFECT_PROSPECTING`: handled by the client (`0x495f39`, `0x6e4cd4`, `0x6e3bdc`) but on
/// no 5875 spell, so the cast-fail reasons only its legs raise, `0x83`, `0x84` and `0x90`, never
/// show. The number is this build's own: `TryCast` pairs 99 with disenchant (`0x6e4ca2`) and 127
/// with prospect (`0x6e4cd4`).
pub const SPELL_EFFECT_PROSPECTING: u32 = 127;

/// `Spell.dbc` joined with `SpellIcon.dbc`, plus the learn-spell map: a trainer offers a learn
/// wrapper, and [`Self::learned_spell`] follows it to the ability it teaches.
pub struct SpellCatalog {
    spells: HashMap<u32, SpellDisplay>,
    learned_spell: HashMap<u32, u32>,
    /// Learn effects in slot order, for spells that have any.
    learn_effects: HashMap<u32, Vec<LearnEffect>>,
    /// Spell id to the `Languages.dbc` id its first effect declares.
    declared_language: HashMap<u32, u32>,
    dispel_types: SpellDispelTypes,
}

impl SpellCatalog {
    /// A catalog from explicit displays, for tests: no learn-spell map and no dispel table, so
    /// [`Self::dispel_name`] is always `None`.
    pub fn from_displays(spells: HashMap<u32, SpellDisplay>) -> Self {
        Self::from_displays_and_effects(spells, HashMap::new())
    }

    /// [`Self::from_displays`] with a learn-effect table, for the trainer tests.
    pub fn from_displays_and_effects(
        spells: HashMap<u32, SpellDisplay>,
        learn_effects: HashMap<u32, Vec<LearnEffect>>,
    ) -> Self {
        let learned_spell = learn_effects
            .iter()
            .filter_map(|(id, effects)| {
                effects.iter().find_map(|e| match e {
                    LearnEffect::Spell(taught) => Some((*id, *taught)),
                    _ => None,
                })
            })
            .collect();
        Self {
            spells,
            learned_spell,
            learn_effects,
            declared_language: HashMap::new(),
            dispel_types: SpellDispelTypes::default(),
        }
    }

    /// A spell's learn effects in slot order, empty for a plain ability.
    pub fn learn_effects(&self, id: u32) -> &[LearnEffect] {
        self.learn_effects.get(&id).map_or(&[], Vec::as_slice)
    }

    pub fn get(&self, id: u32) -> Option<&SpellDisplay> {
        self.spells.get(&id)
    }

    /// The dispel class's name, for the aura tooltip and the debuff border's `debuffType`; `None`
    /// when `SpellDispelType.dbc`'s `[+0x28]` gate withholds it.
    pub fn dispel_name(&self, display: &SpellDisplay) -> Option<&str> {
        self.dispel_types.name(display.dispel)
    }

    /// Every loaded spell, unordered.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &SpellDisplay)> + '_ {
        self.spells.iter().map(|(id, s)| (*id, s))
    }

    /// The ability a learn wrapper teaches, which resolves a trainer's wire id.
    pub fn learned_spell(&self, id: u32) -> Option<u32> {
        self.learned_spell.get(&id).copied()
    }

    /// The `Languages.dbc` id a spell declares: `EffectMiscValue_1` when `Effect_1` is 39. The
    /// client fills its language table (`0xb700ac`) on spell add (`0x4b25b0`), a later learn
    /// overwriting, so fold this over the known spells. Five language spells (813-817) declare
    /// Common (7): languages 8, 9, 10 and 12 are always garbled, and a warlock's Demon Tongue
    /// takes Common's entry. Both are 1.12.1 behaviour.
    pub fn declared_language(&self, spell: u32) -> Option<u32> {
        self.declared_language.get(&spell).copied()
    }

    /// Every `(spell, language)` pair a shipped spell declares.
    pub fn declared_languages(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.declared_language.iter().map(|(s, l)| (*s, *l))
    }

    pub fn len(&self) -> usize {
        self.spells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spells.is_empty()
    }
}

/// 173 fields, `u32` but for the floats (`Speed`, the two float effect arrays) and the four enUS
/// string heads; the signed columns stay `u32`, as [`i32_at`] reads the same bits.
fn spell_schema() -> Schema {
    let mut s = Schema::new("Spell");
    for i in 0..SPELL_FIELDS {
        if i == COL_NAME_ENUS {
            s.add_field(SchemaField::new("NameEnUs", FieldType::String));
        } else if i == COL_NAME_SUBTEXT_ENUS {
            s.add_field(SchemaField::new("NameSubtextEnUs", FieldType::String));
        } else if i == COL_DESCRIPTION_ENUS {
            s.add_field(SchemaField::new("DescriptionEnUs", FieldType::String));
        } else if i == COL_AURA_DESCRIPTION_ENUS {
            s.add_field(SchemaField::new("AuraDescriptionEnUs", FieldType::String));
        } else if i == COL_SPEED {
            s.add_field(SchemaField::new("Speed", FieldType::Float32));
        } else if (COL_EFFECT_REAL_POINTS_PER_LEVEL_1..COL_EFFECT_REAL_POINTS_PER_LEVEL_1 + 3)
            .contains(&i)
        {
            s.add_field(SchemaField::new(
                format!(
                    "EffectRealPointsPerLevel{}",
                    i - COL_EFFECT_REAL_POINTS_PER_LEVEL_1
                ),
                FieldType::Float32,
            ));
        } else if (COL_EFFECT_MULTIPLE_VALUE_1..COL_EFFECT_MULTIPLE_VALUE_1 + 3).contains(&i) {
            s.add_field(SchemaField::new(
                format!("EffectMultipleValue{}", i - COL_EFFECT_MULTIPLE_VALUE_1),
                FieldType::Float32,
            ));
        } else {
            s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
        }
    }
    s
}

/// Load the joined spell display catalog off the patch chain.
pub fn load_spell_catalog(chain: &mut Chain) -> Result<SpellCatalog> {
    let icons = crate::dbc::load_spell_icon_map(chain)?;
    let dispel_types = load_spell_dispel_types(chain)?;
    // Categories whose `SpellCategory.dbc` flags carry `0x2` match every cooldown query
    // (`GetCooldownInfo` `0x6e13e0`); only wand Shoot's 351 does.
    let wildcard_categories: std::collections::HashSet<u32> = {
        let bytes = chain
            .read_file("DBFilesClient\\SpellCategory.dbc")
            .context("reading SpellCategory.dbc")?;
        let mut s = benilla_dbc::Schema::new("SpellCategory");
        s.add_field(benilla_dbc::SchemaField::new(
            "ID",
            benilla_dbc::FieldType::UInt32,
        ));
        s.add_field(benilla_dbc::SchemaField::new(
            "Flags",
            benilla_dbc::FieldType::UInt32,
        ));
        let rs = parse(&bytes, s, "SpellCategory.dbc")?;
        rs.records()
            .iter()
            .filter(|r| u32_at(r, 1).unwrap_or(0) & 0x2 != 0)
            .filter_map(|r| u32_at(r, 0))
            .collect()
    };

    let spell_bytes = chain.read_file(SPELL).context("reading Spell.dbc")?;
    let spells_set = parse(&spell_bytes, spell_schema(), "Spell.dbc")?;
    let mut spells: HashMap<u32, SpellDisplay> = HashMap::new();
    let mut learned_spell: HashMap<u32, u32> = HashMap::new();
    let mut learn_effects: HashMap<u32, Vec<LearnEffect>> = HashMap::new();
    let mut declared_language: HashMap<u32, u32> = HashMap::new();
    for r in spells_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        // The learn-spell hop: the first LEARN_SPELL effect's trigger is the taught ability.
        for i in 0..3 {
            if u32_at(r, COL_EFFECT_1 + i) == Some(SPELL_EFFECT_LEARN_SPELL) {
                if let Some(taught) = u32_at(r, COL_EFFECT_TRIGGER_1 + i).filter(|&t| t != 0) {
                    learned_spell.entry(id).or_insert(taught);
                    break;
                }
            }
        }
        // Learn effects in slot order; a SKILL_STEP's step is base points plus die sides.
        let effects: Vec<LearnEffect> = (0..3)
            .filter_map(|i| {
                let trigger = || u32_at(r, COL_EFFECT_TRIGGER_1 + i).filter(|&t| t != 0);
                match u32_at(r, COL_EFFECT_1 + i)? {
                    SPELL_EFFECT_LEARN_SPELL => trigger().map(LearnEffect::Spell),
                    SPELL_EFFECT_LEARN_PET_SPELL => trigger().map(LearnEffect::PetSpell),
                    SPELL_EFFECT_SKILL_STEP => {
                        let skill = i32_at(r, COL_EFFECT_MISC_1 + i).filter(|&m| m > 0)? as u32;
                        let value = i32_at(r, COL_EFFECT_BASE_POINTS_1 + i).unwrap_or(0)
                            + i32_at(r, COL_EFFECT_DIE_SIDES_1 + i).unwrap_or(0);
                        Some(LearnEffect::SkillStep {
                            skill,
                            step: value.max(0) as u32,
                        })
                    }
                    _ => None,
                }
            })
            .collect();
        if !effects.is_empty() {
            learn_effects.insert(id, effects);
        }
        // The language declaration (`0x4b2656`) reads slot 0 only, `+0xf4` and `+0x1a8`, unlike
        // the learn hop's scan of all three.
        if u32_at(r, COL_EFFECT_1) == Some(SPELL_EFFECT_LANGUAGE) {
            if let Some(lang) = i32_at(r, COL_EFFECT_MISC_1).filter(|&l| l > 0) {
                declared_language.insert(id, lang as u32);
            }
        }
        let name = str_at(&spells_set, r, COL_NAME_ENUS).unwrap_or_default();
        let rank = str_at(&spells_set, r, COL_NAME_SUBTEXT_ENUS);
        let icon = u32_at(r, COL_ICON_ID)
            .filter(|&i| i != 0)
            .and_then(|i| icons.get(&i).cloned());
        let visual = u32_at(r, COL_VISUAL_ID).unwrap_or(0);
        let speed = f32_at(r, COL_SPEED).unwrap_or(0.0);
        let attributes = u32_at(r, COL_ATTRIBUTES).unwrap_or(0);
        // Read once; the shapeshift-form derivation below reuses `effect_apply_aura`.
        let effect_apply_aura: [u32; 3] =
            std::array::from_fn(|i| u32_at(r, COL_EFFECT_APPLY_AURA_1 + i).unwrap_or(0));
        let effect_trigger_spell: [u32; 3] =
            std::array::from_fn(|i| u32_at(r, COL_EFFECT_TRIGGER_1 + i).unwrap_or(0));
        spells.insert(
            id,
            SpellDisplay {
                name,
                rank,
                icon,
                visual,
                speed,
                attributes,
                attributes_ex: u32_at(r, COL_ATTRIBUTES_EX).unwrap_or(0),
                attributes_ex2: u32_at(r, COL_ATTRIBUTES_EX2).unwrap_or(0),
                attributes_ex3: u32_at(r, COL_ATTRIBUTES_EX3).unwrap_or(0),
                school: u32_at(r, COL_SCHOOL).unwrap_or(0),
                mechanic: u32_at(r, COL_MECHANIC).unwrap_or(0),
                effect_mechanic: std::array::from_fn(|i| {
                    u32_at(r, COL_EFFECT_MECHANIC_1 + i).unwrap_or(0)
                }),
                prevention_type: u32_at(r, COL_PREVENTION_TYPE).unwrap_or(0),
                spell_family: u32_at(r, COL_SPELL_FAMILY_NAME).unwrap_or(0),
                // Low dword first: the reference reads bit i at `[rec + 4 * (i >> 5) + 0x284]`.
                spell_family_flags: u64::from(u32_at(r, COL_SPELL_FAMILY_FLAGS_LOW).unwrap_or(0))
                    | u64::from(u32_at(r, COL_SPELL_FAMILY_FLAGS_LOW + 1).unwrap_or(0)) << 32,
                passive: attributes & ATTR_PASSIVE != 0,
                cast_ui: u32_at(r, COL_CAST_UI).unwrap_or(0),
                effects: [0, 1, 2].map(|i| u32_at(r, COL_EFFECT_1 + i).unwrap_or(0)),
                base_level: u32_at(r, COL_BASE_LEVEL).unwrap_or(0),
                max_level: u32_at(r, COL_MAX_LEVEL).unwrap_or(0),
                spell_level: u32_at(r, COL_SPELL_LEVEL).unwrap_or(0),
                // The first OPEN_LOCK effect, in any slot, and its `EffectMiscValue` LockType.
                open_lock: (0..3).find_map(|i| {
                    (u32_at(r, COL_EFFECT_1 + i)? == SPELL_EFFECT_OPEN_LOCK).then(|| OpenLock {
                        lock_type: u32_at(r, COL_EFFECT_MISC_1 + i).unwrap_or(0),
                        effect: i,
                    })
                }),
                dispel: u32_at(r, COL_DISPEL).unwrap_or(0),
                category: u32_at(r, COL_CATEGORY).unwrap_or(0),
                // Resolved at load, read off the armed record (`0x6e1563`).
                category_wildcard: u32_at(r, COL_CATEGORY)
                    .is_some_and(|c| wildcard_categories.contains(&c)),
                recovery_ms: u32_at(r, COL_RECOVERY_TIME).unwrap_or(0),
                interrupt_flags: u32_at(r, COL_INTERRUPT_FLAGS).unwrap_or(0),
                aura_interrupt_flags: u32_at(r, COL_AURA_INTERRUPT_FLAGS).unwrap_or(0),
                channel_interrupt_flags: u32_at(r, COL_CHANNEL_INTERRUPT_FLAGS).unwrap_or(0),
                category_recovery_ms: u32_at(r, COL_CATEGORY_RECOVERY_TIME).unwrap_or(0),
                start_recovery_category: u32_at(r, COL_START_RECOVERY_CATEGORY).unwrap_or(0),
                start_recovery_ms: u32_at(r, COL_START_RECOVERY_TIME).unwrap_or(0),
                power_type: u32_at(r, COL_POWER_TYPE).unwrap_or(0),
                mana_cost: u32_at(r, COL_MANA_COST).unwrap_or(0),
                mana_cost_pct: u32_at(r, COL_MANA_COST_PCT).unwrap_or(0),
                mana_cost_per_level: u32_at(r, COL_MANA_COST_PER_LEVEL).unwrap_or(0),
                mana_per_second: u32_at(r, COL_MANA_PER_SECOND).unwrap_or(0),
                range_index: u32_at(r, COL_RANGE_INDEX).unwrap_or(0),
                modal_next_spell: u32_at(r, COL_MODAL_NEXT_SPELL).unwrap_or(0),
                targets: u32_at(r, COL_TARGETS).unwrap_or(0),
                implicit_target_a1: u32_at(r, COL_IMPLICIT_TARGET_A1).unwrap_or(0),
                effect_implicit_target_a: std::array::from_fn(|i| {
                    u32_at(r, COL_IMPLICIT_TARGET_A1 + i).unwrap_or(0)
                }),
                effect_implicit_target_b: std::array::from_fn(|i| {
                    u32_at(r, COL_IMPLICIT_TARGET_B1 + i).unwrap_or(0)
                }),
                stances: u32_at(r, COL_STANCES).unwrap_or(0),
                stances_not: u32_at(r, COL_STANCES_NOT).unwrap_or(0),
                caster_aura_state: u32_at(r, COL_CASTER_AURA_STATE).unwrap_or(0),
                target_aura_state: u32_at(r, COL_TARGET_AURA_STATE).unwrap_or(0),
                totems: std::array::from_fn(|i| u32_at(r, COL_TOTEM_1 + i).unwrap_or(0)),
                reagents: std::array::from_fn(|i| {
                    (
                        u32_at(r, COL_REAGENT_1 + i).unwrap_or(0),
                        u32_at(r, COL_REAGENT_COUNT_1 + i).unwrap_or(0),
                    )
                }),
                equipped_item_class: u32_at(r, COL_EQUIPPED_ITEM_CLASS).unwrap_or(0) as i32,
                equipped_item_subclass_mask: u32_at(r, COL_EQUIPPED_ITEM_SUBCLASS_MASK)
                    .unwrap_or(0),
                equipped_item_inventory_type_mask: u32_at(r, COL_EQUIPPED_ITEM_INVENTORY_TYPE_MASK)
                    .unwrap_or(0),
                requires_spell_focus: u32_at(r, COL_REQUIRES_SPELL_FOCUS).unwrap_or(0),
                // The first MOD_SHAPESHIFT effect's `EffectMiscValue` is the form (`0x4b4690`).
                shapeshift_form: (0..3).find_map(|i| {
                    (effect_apply_aura[i] == SPELL_AURA_MOD_SHAPESHIFT)
                        .then(|| u32_at(r, COL_EFFECT_MISC_1 + i).unwrap_or(0))
                }),
                stance_bar_order: u32_at(r, COL_STANCE_BAR_ORDER).unwrap_or(0) as i32,
                active_icon_id: u32_at(r, COL_ACTIVE_ICON_ID).unwrap_or(0),
                active_icon: u32_at(r, COL_ACTIVE_ICON_ID)
                    .filter(|&i| i != 0)
                    .and_then(|i| icons.get(&i).cloned()),
                description: str_at(&spells_set, r, COL_DESCRIPTION_ENUS),
                aura_description: str_at(&spells_set, r, COL_AURA_DESCRIPTION_ENUS),
                duration_index: u32_at(r, COL_DURATION_INDEX).unwrap_or(0),
                casting_time_index: u32_at(r, COL_CASTING_TIME_INDEX).unwrap_or(0),
                proc_chance: u32_at(r, COL_PROC_CHANCE).unwrap_or(0),
                effect_base_points: std::array::from_fn(|i| {
                    i32_at(r, COL_EFFECT_BASE_POINTS_1 + i).unwrap_or(0)
                }),
                effect_die_sides: std::array::from_fn(|i| {
                    i32_at(r, COL_EFFECT_DIE_SIDES_1 + i).unwrap_or(0)
                }),
                effect_base_dice: std::array::from_fn(|i| {
                    i32_at(r, COL_EFFECT_BASE_DICE_1 + i).unwrap_or(0)
                }),
                effect_dice_per_level: std::array::from_fn(|i| {
                    i32_at(r, COL_EFFECT_DICE_PER_LEVEL_1 + i).unwrap_or(0)
                }),
                effect_real_points_per_level: std::array::from_fn(|i| {
                    f32_at(r, COL_EFFECT_REAL_POINTS_PER_LEVEL_1 + i).unwrap_or(0.0)
                }),
                effect_amplitude: std::array::from_fn(|i| {
                    u32_at(r, COL_EFFECT_AMPLITUDE_1 + i).unwrap_or(0)
                }),
                effect_apply_aura,
                effect_radius_index: std::array::from_fn(|i| {
                    u32_at(r, COL_EFFECT_RADIUS_INDEX_1 + i).unwrap_or(0)
                }),
                effect_chain_targets: std::array::from_fn(|i| {
                    u32_at(r, COL_EFFECT_CHAIN_TARGETS_1 + i).unwrap_or(0)
                }),
                effect_multiple_value: std::array::from_fn(|i| {
                    f32_at(r, COL_EFFECT_MULTIPLE_VALUE_1 + i).unwrap_or(0.0)
                }),
                effect_trigger_spell,
                effect_item_type: std::array::from_fn(|i| {
                    u32_at(r, COL_EFFECT_ITEM_TYPE_1 + i).unwrap_or(0)
                }),
                effect_misc_value: std::array::from_fn(|i| {
                    i32_at(r, COL_EFFECT_MISC_1 + i).unwrap_or(0)
                }),
            },
        );
    }
    Ok(SpellCatalog {
        spells,
        learned_spell,
        learn_effects,
        declared_language,
        dispel_types,
    })
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;

#[cfg(test)]
mod learn_effect_tests {
    use super::*;

    /// A trainer's wrapper carries the learn effect and the step `0x4d7d40` compares.
    #[test]
    fn the_openers_skill_steps_read_off_the_real_file() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_spell_catalog(&mut chain).expect("Spell.dbc");
        let blacksmithing = 164;
        for (wrapper, taught, step) in [
            (2020, 2018, 1),
            (2021, 3100, 2),
            (3539, 3538, 3),
            (9786, 9785, 4),
        ] {
            assert_eq!(
                cat.learn_effects(wrapper),
                &[
                    LearnEffect::Spell(taught),
                    LearnEffect::SkillStep {
                        skill: blacksmithing,
                        step
                    }
                ],
                "wrapper {wrapper}"
            );
        }
        // Mining's opener: base −1 + die 1 on the LEARN slot is not a step; the SKILL_STEP slot is.
        assert_eq!(
            cat.learn_effects(2581),
            &[
                LearnEffect::Spell(2575),
                LearnEffect::SkillStep {
                    skill: 186,
                    step: 1
                }
            ]
        );
        // A plain ability teaches nothing.
        assert!(cat.learn_effects(2383).is_empty(), "Find Herbs");
    }
}
