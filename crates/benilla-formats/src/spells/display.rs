//! `SpellDisplay` — one spell's display + usable-walk surface (the column pins its fields
//! read are documented on `spells/mod.rs`'s module header, which this file's every intra-doc
//! link points back to). Split out of `mod.rs` purely for file size — still `crate::spells`'s
//! own type, not a separate concern.

use super::*;

/// A spell's `SPELL_EFFECT_OPEN_LOCK` effect — which `LockType` it opens, and
/// which of the three effect slots carries it (so the value walk reads the right column of the
/// per-effect arrays).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenLock {
    /// `EffectMiscValue` — the `LockType.dbc` index this spell opens (pick-lock 1, herbalism 2,
    /// mining 3, "Quick Open" 10, "Open Kneeling" 13, blasting 16 …).
    pub lock_type: u32,
    /// Which of `Effect[0..3]` is the OPEN_LOCK one.
    pub effect: usize,
}

/// One spell's display identity.
pub struct SpellDisplay {
    pub name: String,
    /// `SpellNameSubtext` enUS (column 129, module docs) — "Rank N" for most leveled spells, a
    /// bare descriptor ("Racial", "Passive", …) for some, or absent for a handful of unranked
    /// spells. `None` when the column is empty ([`crate::dbc::str_at`]'s own empty-means-absent
    /// contract) — the spellbook slot then shows no second line at all.
    pub rank: Option<String>,
    /// The icon texture's MPQ path (`Interface\Icons\…`, extensionless as the DBC stores it);
    /// `None` when the spell's icon id is 0/unknown (render the fallback question mark).
    pub icon: Option<String>,
    /// `SpellVisual.dbc` id (column 115, module docs) — `0` = no visual chain (a silent cast).
    /// Resolve through [`crate::SpellVisualCatalog`] for the lifecycle-stage kits.
    pub visual: u32,
    /// The projectile travel speed, world units/sec (column 37, module docs). `0` = instant
    /// impact (decision 0099's `Speed==0` gate — no missile phase).
    pub speed: f32,
    /// `Attributes` (column 6, module docs) — consumed by [`Self::ranged_attack`] (`0x2`),
    /// [`Self::targets_main_hand_item`] (`0x200`), [`Self::in_spellbook`] (`0x20`/`0x80`),
    /// [`Self::cooldown_on_event`] (bit 25), and the aura-bar display filter
    /// [`Self::hidden_from_aura_bar`] (`0x80`).
    pub attributes: u32,
    /// **`School`** (column 1, `SpellRec+0x4`) — the spell's magic school as an INDEX, not a mask.
    /// The crowd-control exemption's school arm shifts it (`1 << School`) before testing it against
    /// an immunity effect's `EffectMiscValue`, which is a mask.
    pub school: u32,
    /// **`Mechanic`** (column 5, `SpellRec+0x14`) — the `SpellMechanic.dbc` id for the spell as a
    /// whole. Read by the crowd-control ladder twice over: the mechanic-immunity arm compares it,
    /// and it is the fallback value the scanner reports when a blocking aura's own
    /// `EffectMechanic` is 0 — which is what names the `0x8d` "Can't do that while %s" line.
    pub mechanic: u32,
    /// `AttributesEx` (column 7, `SpellRec+0x1c`) — only bit `0x10000000` is consumed
    /// ([`Self::hidden_from_aura_bar`]'s `SPELL_ATTR_EX_NO_AURA_ICON` half).
    pub attributes_ex: u32,
    /// `AttributesEx2` (column 8, module docs) — bit `0x20` is consumed by
    /// [`Self::ranged_attack`], bit 19 (`0x0008_0000`, allow-while-not-shapeshifted) by the form
    /// gate [`Self::form_refusal`]; the stance-bar admission bits (`0x2`/`0x10`) are read raw by
    /// `benilla::ui_shapeshift`.
    pub attributes_ex2: u32,
    /// `AttributesEx3` (column 9, `SpellRec+0x24`) — bit `0x8000` is consumed
    /// ([`Self::melee_white_damage`]), and bits `0x400` / `0x1000000` are the equipped-item
    /// search's **hand restriction** (main-hand-only / off-hand-only), which is where
    /// `0x5f0c50`'s slot mask comes from.
    pub attributes_ex3: u32,
    /// **`SpellFamilyName`** (column 160, `SpellRec+0x280`) — which class's talent tree may modify
    /// this spell. `0` on 18243 of the 22357 shipped rows (creature and world spells); the rest
    /// carry a `SpellFamilyNames` value, which for a player spell is its own class's.
    ///
    /// The first two of `GetSpellModifiers 0x6e6b30`'s three conjunct gates read it and nothing
    /// else: `!= 0` (`6e6b38`), then `== [0xcecaac]`, the local player's own class family
    /// (`6e6b46` — [`crate::ChrClasses::spell_family`]). A spell that fails either takes no
    /// modifier at all, which is how a mage's talent stays off a warrior's ability and off every
    /// item/creature spell in the file.
    pub spell_family: u32,
    /// **`SpellFamilyFlags`** (columns 161/162, `SpellRec+0x284`/`+0x288`) — the 64-bit bit-set
    /// naming which of its family's modifier rows this spell subscribes to, low dword first.
    ///
    /// `GetSpellModifiers` walks **all 64 bits** with no early break and SUMS one cell per set bit
    /// out of each table (`6e6b72`–`6e6ba6`), so a multi-bit spell accumulates several — and the
    /// high dword is genuinely live rather than unused width: measured on the shipped 5875 file,
    /// 1794 rows set exactly one bit, **322 set more than one**, and the highest bit index in the
    /// whole table is **35**. Frostbolt 116 sets 5/19/20/30, Cleanse 4987 sets 12 **and 33** (it
    /// needs both dwords), Cure Poison 526 sets 35 alone.
    pub spell_family_flags: u64,
    /// **`PreventionType`** (column 165, `SpellRec+0x294`) — which crowd-control flag refuses this
    /// spell **locally**, before any packet: `1` = silence, `2` = pacify, `0` = neither. The
    /// client's CC validator `0x6094f0` (called from `TryCast 0x6e4b60`, bailing at `0x6e4f42`)
    /// reads it per spell — so `UNIT_FLAG_SILENCED` does **not** stop everything, only the rows
    /// that declare `1`. The STUNNED arm above it carries no such gate and refuses every spell.
    ///
    /// **Column pinned twice**: the byte offset `0x294 / 4 = 165` on a 173-field,
    /// 692-byte record, and the shipped data itself — its neighbour 164 is `DmgClass` and takes
    /// four values where this takes three, which **Auto Shot (75)** separates decisively at
    /// `DmgClass = 3` (RANGED) with `PreventionType = 2`. Fireball 133 → 1, Heroic Strike 78 → 2,
    /// Attack 6603 → 0.
    pub prevention_type: u32,
    /// **`modalNextSpell`** (`Spell.dbc` column 38, `SpellRec + 0x98`) — the spell this one makes
    /// the client cast **by itself**, one server round-trip later, with no user input and no addon.
    /// `0` for all but 57 of the 22357 shipped rows.
    ///
    /// `HandleCastResult 0x6e7330` — the `SMSG_CAST_RESULT` (0x130) handler — reads it for the
    /// spell the reply names, **on success as well as failure** (`0x6e7356 cmp [ebp+0xf],0x2` /
    /// `0x6e735a jne` sends a non-failure straight to the same block the failure path falls into
    /// at `0x6e73eb`), provided the reply is for the in-flight cast (`0x6e7408` vs `[0xceca88]`).
    /// Then, at `0x6e7447`:
    ///
    /// - `0` → nothing happens (`0x6e744f je`);
    /// - `== [0xceac30]`, the **running** auto-repeat → the pending-cast record is re-armed and
    ///   nothing is cast (`0x6e745d`) — which is why a second sting does not restart Auto Shot or
    ///   reset its swing timer;
    /// - otherwise → the client **casts it** (`0x6e74aa call 0x6e5a90` → `TryCast`), at the null
    ///   target guid, through the ordinary ladder.
    ///
    /// **This is how a hunter starts shooting.** Every rank of every hunter shot — Serpent Sting,
    /// Arcane Shot, Multi-Shot, Concussive Shot, Aimed Shot, Viper/Scorpid Sting, Black Arrow,
    /// Distracting Shot — carries **75 (Auto Shot)** here, and Auto Shot's own column 38 is `0`, so
    /// the chain is exactly one hop and cannot loop. The only other non-zero values in the shipped
    /// file are three "(TEST) bow shot" rows → 59 and two `Minigun` rows → 23675 (self-referential,
    /// absorbed by the equal-branch).
    ///
    /// benilla. It **corrects** the reading in 0994 §4 — the client
    /// really does not start the repeat from the sting's *own* send, and then starts it from a
    /// *second cast it issues itself*.
    pub modal_next_spell: u32,
    /// `Attributes & 0x40` (`SPELL_ATTR_PASSIVE`, module docs) — the spellbook's gray-and-refuse
    /// gate (consumed by `benilla-ui/src/script/spellbook.rs`).
    pub passive: bool,
    /// `castUI` (column 3, `SpellRec+0xc`, module docs) — the third of the spellbook add-gate's
    /// three exclusions ([`Self::in_spellbook`]). Nonzero keeps the spell out of the book; reads
    /// 0 for every ordinary player spell.
    pub cast_ui: u32,
    /// `Effect[3]` (column 61 == [`COL_EFFECT_1`], module docs) — each effect's type. Slot 0 is what
    /// almost every consumer wants (the auto-attack gate [`Self::is_melee_auto_attack`], the
    /// tradeskill/enchant/duel classifications); the trainer's icon law is the one caller that scans
    /// all three, hunting a learn-wrapper effect in any slot (`0x4d8fed`'s
    /// three-slot loop). Carried
    /// as the `[T; 3]` array every other per-effect column already uses — it was a lone `effect_1`
    /// until that scan needed the siblings.
    pub effects: [u32; 3],
    /// This spell's `SPELL_EFFECT_OPEN_LOCK` effect, or `None` if it opens no lock. The GameObject
    /// interact-cast matches it against a lock slot across the player's known
    /// spells: "Opening" for keyless chests, "Mining"/"Herb Gathering"/"Pick Lock" for skill locks.
    /// The skill it *provides* is [`Self::open_lock_skill`].
    pub open_lock: Option<OpenLock>,
    /// `baseLevel` (column 28, `SpellRec+0x70`; **not** `spellLevel`, column 29 `+0x74`) — the
    /// level the effect values are
    /// quoted at: the effect-value walk subtracts it, floored at 0 (`0x6e3826`), and the
    /// cast-time scaling reads the same column (`0x6e3340`).
    pub base_level: u32,
    /// `maxLevel` (column 27, `SpellRec+0x6c`) — caps the skill-derived level term in
    /// [`Self::open_lock_skill`] at `maxLevel × 5` (`0x5ea6e3`); `0` = uncapped.
    pub max_level: u32,
    /// `spellLevel` (column 29, `SpellRec+0x74`) — the DBC's actual spellLevel. One consumer:
    /// the Beast Training rank comparator, whose recorded law names `+0x74` (`benilla-ui`
    /// `craft.rs`). Everything level-scaled here reads [`Self::base_level`] instead.
    pub spell_level: u32,
    /// `Dispel` (column 4, `SpellRec+0x10`) — the `SpellDispelType.dbc` id. The reference client
    /// reads exactly this field for `GetPlayerBuffDispelType` / `UnitDebuff`'s third return
    /// (byte-verified). Name it through [`crate::SpellCatalog::dispel_name`] — the
    /// id alone does not say whether the class is named (`SpellDispelType.dbc`'s `[+0x28]` gate).
    pub dispel: u32,
    /// `Category` (column 2, [`COL_CATEGORY`]) — the shared-cooldown category; `0` = none.
    pub category: u32,
    /// Whether [`Self::category`]'s `SpellCategory.dbc` row carries the flags-bit-`0x2`
    /// "matches every query" wildcard (`GetCooldownInfo 0x6e13e0`'s category leg) — resolved at
    /// catalog load. Only wand Shoot's category 351 in
    /// the 5875 data: its running swing cooldown sweeps EVERY button.
    pub category_wildcard: bool,
    /// `RecoveryTime` ms (column 19) — the spell's own cooldown.
    pub recovery_ms: u32,
    /// `InterruptFlags` ([`COL_INTERRUPT_FLAGS`]) — what breaks this spell's cast. Ordinary
    /// timed casts carry `0xf`; the movement bit (`0x1`, vmangos
    /// `SPELL_INTERRUPT_FLAG_MOVEMENT`) gates the cast bar's local self-cancel
    /// (`benilla::ui_cast`, where the client-side semantics are documented).
    pub interrupt_flags: u32,
    /// `AuraInterruptFlags` ([`COL_AURA_INTERRUPT_FLAGS`]) — what breaks this spell's applied
    /// aura (the food/drink "sit still" bits live here). The cast-initiation moving gate
    /// (`0x609de3`) reads its `0x18`
    /// (MOVING|TURNING) bits as one of the three "would movement matter" arms. `0` for most
    /// direct casts.
    pub aura_interrupt_flags: u32,
    /// `ChannelInterruptFlags` ([`COL_CHANNEL_INTERRUPT_FLAGS`]) — what breaks this spell's
    /// running channel; the bits live in the *aura*-interrupt space (vmangos tests it with
    /// `AURA_INTERRUPT_MOVING_CANCELS`), not `InterruptFlags`'. `0` for non-channels.
    pub channel_interrupt_flags: u32,
    /// `CategoryRecoveryTime` ms (column 20) — the category's shared cooldown.
    pub category_recovery_ms: u32,
    /// `StartRecoveryCategory` (column 157) — the GCD category (133 for ordinary spells).
    pub start_recovery_category: u32,
    /// `StartRecoveryTime` ms (column 158) — the GCD duration (1500 for ordinary spells; 0 = no GCD).
    pub start_recovery_ms: u32,
    /// `powerType` (column 31) — 0 mana, 1 rage, 3 energy (vmangos `Powers`); **health is the
    /// signed −2 stored as `0xFFFFFFFE`** (Life Tap, Bloodrage, Health Funnel), which is why every
    /// consumer treats "negative or ≥ 5" as the health lane (the ref's own fallback fork, 1074).
    pub power_type: u32,
    /// `manaCost` (column 32) — the flat cast cost in `power_type`'s unit.
    pub mana_cost: u32,
    /// `ManaCostPercentage` (column 156) — percent of base mana added to the flat cost (Judgement
    /// and kin); 0 for most spells.
    pub mana_cost_pct: u32,
    /// `manaCostPerlevel` (column 33) — the per-level cost term (`0x6e31b0`'s
    /// `(level − spellLevel) · perLevel`; 1074). 72 nonzero rows in the 5875 file, every one a
    /// creature-cast spell (Dark Offering's health lane among them) — dormant for player
    /// tooltips, modeled because the law and the data both carry it.
    pub mana_cost_per_level: u32,
    /// `manaPerSecond` (column 34) — the per-second maintenance component; the tooltip cost
    /// cell's `_PER_TIME` composite ("11 Health, plus 5 per sec" — Health Funnel; 1074). The
    /// sibling `manaPerSecondPerLevel` (35) is all-zero across the whole 5875 file
    /// ([`catalog_tests`]'s column scan — a verified negative) and stays unparsed.
    pub mana_per_second: u32,
    /// `rangeIndex` (column 36) — the `SpellRange.dbc` row ([`SpellRangeCatalog`]).
    pub range_index: u32,
    /// `Targets` (column 13, [`COL_TARGETS`]) — the `TARGET_FLAG_*` seed mask of the cast-arm's
    /// targeting flag_word. `0` for ordinary casts (the implicit-target switch supplies the bits).
    pub targets: u32,
    /// `EffectImplicitTargetA[0]` (column 82, [`COL_IMPLICIT_TARGET_A1`]) — the implicit-target
    /// enum the cast-arm's switch adjusts the flag_word by (and the usable walk's
    /// CanAttack/CanAssist fork inside the TargetAuraState leg keys on: 6 = enemy, 21 = friend).
    pub implicit_target_a1: u32,
    /// `EffectImplicitTargetA[3]` (columns 82–84, [`COL_IMPLICIT_TARGET_A1`]) and
    /// `EffectImplicitTargetB[3]` (columns 85–87, [`COL_IMPLICIT_TARGET_B1`]) — every effect's
    /// implicit-target pair, the input of [`Self::is_harmful`] (the client's hostility classifier
    /// walks all three slots, A then B). Slot 0 of A is also [`Self::implicit_target_a1`].
    pub effect_implicit_target_a: [u32; 3],
    pub effect_implicit_target_b: [u32; 3],
    /// `Stances` (column 11) — forms the spell is *explicitly* castable in, `1 << (form-1)` each
    /// (the form gate [`Self::usable_in_form`]). 0 = no form requirement of its own.
    pub stances: u32,
    /// `StancesNot` (column 12) — forms the spell is explicitly forbidden in.
    pub stances_not: u32,
    /// `CasterAuraState` (column 16) — required caster aura-state index (`1 << (n-1)` against
    /// `UNIT_FIELD_AURASTATE`); 0 = none. Revenge's defense state (1).
    pub caster_aura_state: u32,
    /// `TargetAuraState` (column 17) — required aura-state index on the CURRENT TARGET; 0 =
    /// none. The one target-dependent usable leg (`0x6e3f58`): Execute's healthless-20% (2).
    pub target_aura_state: u32,
    /// `Totem[2]` (columns 40-41) — tool items that must be PRESENT (not consumed): fishing
    /// pole-less fishing, blacksmith hammers. 0 = unused slot.
    pub totems: [u32; 2],
    /// `Reagent[8]`/`ReagentCount[8]` (columns 42-49/50-57) as (item entry, count) pairs —
    /// consumed materials. Entry 0 = unused slot.
    pub reagents: [(u32, u32); 8],
    /// `EquippedItemClass` (column 58, signed) — the item class a worn item must match (2 =
    /// weapon for Auto Shot's bow); `-1` = no requirement.
    pub equipped_item_class: i32,
    /// `EquippedItemSubClassMask` (column 59) — `1 << subclass` mask refining
    /// [`Self::equipped_item_class`] (`0x4000c` = bows|guns|crossbows).
    pub equipped_item_subclass_mask: u32,
    /// `EquippedItemInventoryTypeMask` (column 60, `SpellRec+0xf0`) — a `1 << InventoryType` mask,
    /// the third and last leg of the reference's item-target gate (`0x495d60` @ `495e4d`; the two
    /// before it are [`Self::equipped_item_class`]/[`Self::equipped_item_subclass_mask`] at
    /// `495e10`). `0` = no requirement. This is the leg that decides a Crude Scope goes on a gun
    /// and not a chestpiece; a mismatch is the client's own local "Invalid target" (`0x0a`).
    pub equipped_item_inventory_type_mask: u32,
    /// `RequiresSpellFocus` ([`COL_REQUIRES_SPELL_FOCUS`]) — the `SpellFocusObject.dbc` object
    /// that must be nearby (1 Anvil, 3 Forge, 4 Cooking Fire — [`crate::spell_focus`]); 0 = none.
    /// The crafting book's "Requires: …" line.
    pub requires_spell_focus: u32,
    /// The `SpellShapeshiftForm.dbc` form id this spell shifts into — the `EffectMiscValue` of
    /// its first `SPELL_AURA_MOD_SHAPESHIFT` apply-aura effect — or `None` for a non-form spell.
    /// The stance bar's admission + isActive keys (`0x4b2810`, `0x4b475c`).
    pub shapeshift_form: Option<u32>,
    /// `StanceBarOrder` ([`COL_STANCE_BAR_ORDER`], signed) — the stance bar's sort key
    /// (ascending, negative last, spell id tiebreak).
    pub stance_bar_order: i32,
    /// `ActiveIconID`'s resolved texture path ([`COL_ACTIVE_ICON_ID`]) — shown on the stance
    /// button while this form is active, when present (druid forms); `None` falls back to `icon`.
    pub active_icon: Option<String>,
    /// `ActiveIconID` raw ([`COL_ACTIVE_ICON_ID`]) — the plain-path active-action toggle's gate
    /// (`0x4e55f0`/`0x4b36f0` test the COLUMN, not the resolved texture): a nonzero id marks the
    /// spell press-again-to-cancel
    /// while its own aura is live (`benilla::ui_action::toggle`).
    pub active_icon_id: u32,
    /// `Description` enUS (column 138, module docs) — the tooltip body, raw `$`-token text
    /// un-substituted (decision 0274 P2's engine resolves the tokens; this is the source text).
    /// `None` when the column is empty.
    pub description: Option<String>,
    /// `AuraDescription` enUS (column 147, module docs) — the buff/debuff-icon tooltip text (a
    /// shorter restatement of the periodic/ongoing effect); `None` when empty (most direct-damage/
    /// direct-effect spells with no aura component, e.g. Fire Blast).
    pub aura_description: Option<String>,
    /// `DurationIndex` (column 30, module docs) — the [`SpellDurationCatalog`] row. `0` resolves to
    /// no row (an instant-hit spell with no periodic/aura tail).
    pub duration_index: u32,
    /// `CastingTimeIndex` (column 18, module docs) — the [`SpellCastTimeCatalog`] row. Row 1 is the
    /// universal instant sentinel (`0` ms).
    pub casting_time_index: u32,
    /// `ProcChance` (column 25, module docs) — percent chance to trigger on the proc event
    /// `procFlags` names (not read here); vmangos's own `101` sentinel means "always, no roll".
    pub proc_chance: u32,
    /// `EffectBasePoints[3]` (column 76, module docs, **signed**) — each effect's roll floor.
    /// `-1` is the weapon-damage/no-fixed-roll sentinel (Auto Shot, Feign Death).
    pub effect_base_points: [i32; 3],
    /// `EffectDieSides[3]` (column 64, module docs, **signed**) — each effect's die size; the roll
    /// is `base_points + 1 ..= base_points + die_sides` (`die_sides <= 1` collapses to a flat value).
    pub effect_die_sides: [i32; 3],
    /// `EffectBaseDice[3]` (column 67, module docs).
    pub effect_base_dice: [i32; 3],
    /// `EffectAmplitude[3]` ms (column 94, module docs) — a periodic effect's tick period; `0` =
    /// not periodic.
    pub effect_amplitude: [u32; 3],
    /// `EffectApplyAuraName[3]` (column 91 == [`COL_EFFECT_APPLY_AURA_1`]) — each effect's
    /// `SpellAuraDefines` aura-type enum; `0` = not an apply-aura effect. Slot 0 is also read via
    /// [`Self::shapeshift_form`]'s derivation.
    pub effect_apply_aura: [u32; 3],
    /// **`EffectMechanic[0..2]`** (columns 79–81, `SpellRec+0x13c`) — the per-effect
    /// `SpellMechanic.dbc` id. The mechanic-immunity arm accepts a match against *either* this or
    /// [`Self::mechanic`], and the scanner prefers this one when naming the blocking mechanic.
    pub effect_mechanic: [u32; 3],
    /// `EffectRadiusIndex[3]` (column 88, module docs) — each effect's `SpellRadius.dbc` row (not
    /// loaded by this crate yet); `0` = no radius (a single-target effect).
    pub effect_radius_index: [u32; 3],
    /// `EffectChainTarget[3]` (column 100, module docs) — extra chain/jump targets beyond the
    /// primary; `0` for the overwhelming majority of spells (thinnest-evidence pin — module docs).
    pub effect_chain_targets: [u32; 3],
    /// `EffectMultipleValue[3]` (column 97, module docs, f32) — a chain/multi-target falloff
    /// multiplier; `0.0` for most effects.
    pub effect_multiple_value: [f32; 3],
    /// `EffectTriggerSpell[3]` (column 109 == [`COL_EFFECT_TRIGGER_1`]) — each effect's triggered
    /// spell id (a proc, a learn-wrapper's taught ability, Frost Armor's `$6136` chill proc); `0` =
    /// no trigger.
    pub effect_trigger_spell: [u32; 3],
    /// `EffectItemType[3]` ([`COL_EFFECT_ITEM_TYPE_1`]) — the item entry a
    /// `SPELL_EFFECT_CREATE_ITEM` effect creates (the crafting book's product); `0` = none.
    pub effect_item_type: [u32; 3],
    /// `EffectMiscValue[3]` (column 106 == [`COL_EFFECT_MISC_1`], **signed**) — each effect's
    /// misc payload. For an effect-47 opener, slot 0 is the **window routing key** (`0x6e4bd7`:
    /// `!= 0` → the CraftFrame — Enchanting 3, Beast
    /// Training 1 — else the TradeSkillFrame). The OPEN_LOCK/shapeshift slots are already
    /// exposed derived ([`Self::open_lock`]/[`Self::shapeshift_form`]); this is the raw
    /// array.
    pub effect_misc_value: [i32; 3],
    /// `EffectDicePerLevel[3]` ([`COL_EFFECT_DICE_PER_LEVEL_1`]) — the integer per-level term of
    /// the effect-value walk (`0x6e3871`); `0` on every 5875 opener.
    pub effect_dice_per_level: [i32; 3],
    /// `EffectRealPointsPerLevel[3]` ([`COL_EFFECT_REAL_POINTS_PER_LEVEL_1`]) — the float
    /// per-level term (`0x6e3889`). `5.0` on Pick Lock / Mining / Herb Gathering, which is what
    /// makes their provided lock skill track the 5×level profession cap.
    pub effect_real_points_per_level: [f32; 3],
}

/// Hand-rolled so `equipped_item_class` defaults to the data's own "no requirement" (`-1`) —
/// a derived 0 would be a *real* class requirement, silently tripping the usable walk's
/// equipped-item leg on every synthetic/`..Default::default()` spell.
impl Default for SpellDisplay {
    fn default() -> Self {
        SpellDisplay {
            name: String::new(),
            rank: None,
            icon: None,
            visual: 0,
            speed: 0.0,
            attributes: 0,
            school: 0,
            mechanic: 0,
            attributes_ex: 0,
            attributes_ex2: 0,
            modal_next_spell: 0,
            attributes_ex3: 0,
            spell_family: 0,
            spell_family_flags: 0,
            prevention_type: 0,
            passive: false,
            cast_ui: 0,
            effects: [0, 0, 0],
            open_lock: None,
            base_level: 0,
            max_level: 0,
            spell_level: 0,
            dispel: 0,
            category: 0,
            category_wildcard: false,
            recovery_ms: 0,
            interrupt_flags: 0,
            aura_interrupt_flags: 0,
            channel_interrupt_flags: 0,
            category_recovery_ms: 0,
            start_recovery_category: 0,
            start_recovery_ms: 0,
            power_type: 0,
            mana_cost: 0,
            mana_cost_pct: 0,
            mana_cost_per_level: 0,
            mana_per_second: 0,
            range_index: 0,
            targets: 0,
            implicit_target_a1: 0,
            stances: 0,
            stances_not: 0,
            caster_aura_state: 0,
            target_aura_state: 0,
            totems: [0; 2],
            reagents: [(0, 0); 8],
            equipped_item_class: -1,
            equipped_item_subclass_mask: 0,
            equipped_item_inventory_type_mask: 0,
            requires_spell_focus: 0,
            shapeshift_form: None,
            stance_bar_order: 0,
            active_icon: None,
            active_icon_id: 0,
            description: None,
            aura_description: None,
            duration_index: 0,
            casting_time_index: 0,
            proc_chance: 0,
            effect_base_points: [0; 3],
            effect_die_sides: [0; 3],
            effect_base_dice: [0; 3],
            effect_dice_per_level: [0; 3],
            effect_real_points_per_level: [0.0; 3],
            effect_amplitude: [0; 3],
            effect_apply_aura: [0; 3],
            effect_implicit_target_a: [0; 3],
            effect_implicit_target_b: [0; 3],
            effect_mechanic: [0; 3],
            effect_radius_index: [0; 3],
            effect_chain_targets: [0; 3],
            effect_multiple_value: [0.0; 3],
            effect_trigger_spell: [0; 3],
            effect_item_type: [0; 3],
            effect_misc_value: [0; 3],
        }
    }
}

// The dispel-class NAME (the aura tooltip's right column, and the `debuffType` the debuff border
// tints by) is not derivable from [`SpellDisplay::dispel`] alone: `SpellDispelType.dbc` carries
// both the string and the `[+0x28]` gate that decides whether a class is named at all. It lives on
// the catalog that loads that file — [`crate::SpellCatalog::dispel_name`].

impl SpellDisplay {
    /// The `LockType` this spell opens, if any — [`Self::open_lock`]'s index alone.
    pub fn open_lock_type(&self) -> Option<u32> {
        self.open_lock.map(|o| o.lock_type)
    }

    /// The **skill value this spell provides** to a lock, given the player's skill in the
    /// spell's own line — the client's effect-value *min* (`0x6e3800`) put through its caller's
    /// rounding (`0x6e3760`: `round(2x ∓ 0.5) >> 1`), and the left-hand side of the lock
    /// resolver's satisfaction test (`0x5f850f`: `cmp eax,esi; jge`). `None` when the spell
    /// opens no lock.
    ///
    /// **The level term IS the player's skill** (overturning this doc's own earlier
    /// "never the player's skill block"
    /// gloss): `0x6e3800`'s level call (`0x6e384d → 0x6e3130`) resolves the CGPlayer
    /// vtable slot `+0xa8` = `0x5ea690`, which reads `PLAYER_SKILL_INFO` for the spell's
    /// SkillLineAbility line — value **plus** bonuses (`0x5ea56d`/`0x5ea578`/`0x5ea580`) —
    /// clamps it to `maxLevel × 5` (`0x5ea6e3`), and returns it `/5` (`0x6e3195`). So:
    /// `Δ = max(0, min(skill, maxLevel·5)/5 − baseLevel)`. Pick Lock 1804 at skill 300 → `4 +
    /// 1 + 5.0·(60−1)` = **300**; at skill 150 → **150**. Mining 2575 → exactly the skill. The
    /// flat spells are unchanged: Small Seaforium Charge 4056 → `149 + 1` = **150**, the
    /// "Opening" family flat 100.
    ///
    /// The old reading fed the **caster level** here — indistinguishable at skill cap (where
    /// `skill/5 == level`, which is how it survived every at-cap cross-check) and wrong
    /// everywhere else: a level-60 with 1 Mining satisfied a 300-skill vein. `skill_value = 0`
    /// (line unknown, or the player lacks it) is the fail-closed leg — the opener provides only
    /// its flat terms.
    pub fn open_lock_skill(&self, skill_value: u32) -> Option<i32> {
        let e = self.open_lock?.effect;
        // `0x5ea6e3`: skill capped at maxLevel·5 (0 = uncapped); `0x6e3195`: /5 (integer);
        // `0x6e3826`: baseLevel subtraction, floored at 0.
        let capped = if self.max_level > 0 {
            skill_value.min(self.max_level * 5)
        } else {
            skill_value
        };
        let delta = (capped / 5).saturating_sub(self.base_level) as f32;
        let v = self.effect_base_points[e] as f32
            + self.effect_base_dice[e] as f32
            + self.effect_dice_per_level[e] as f32 * delta
            + self.effect_real_points_per_level[e] * delta;
        // `0x6e3760`'s rounding verbatim: double, bias away from zero by a half, round-to-nearest
        // (x87 `fistp`), then arithmetic-halve.
        let doubled = if v >= 0.0 {
            v * 2.0 - 0.5
        } else {
            v * 2.0 + 0.5
        };
        Some((doubled.round_ties_even() as i32) >> 1)
    }

    /// The client's ranged-stance gate, verbatim: `AttributesEx2 & 0x20` (auto-repeat: Auto Shot,
    /// wand Shoot) **or** `Attributes & 0x2` (uses the ranged slot: every Shoot variant, Throw).
    /// (`SpellRec+0x20&0x20 || +0x18&0x2` — the test every ranged trigger
    /// runs: the `SMSG_SPELL_START` stance/ammo sites `0x6e78b6`/`0x6e78f3` and the local cast-send
    /// site `0x6e5930`).
    /// The client's spell-hostility classifier `Spell_C::GetSpellVisualState` (`0x6ea280`),
    /// reduced to its `== 2` answer — **"this spell targets enemies"** — which is the gate on the
    /// victim's **wound flinch after a spell impact** (the instant-hit loop `0x6e8bf0` @
    /// `0x6e8c7b`, and the reflect impact `0x6e8cb0` @ `0x6e8cf1`). Byte-read
    /// 2026-09-07 off `WoW.exe`: `Targets & 0x100` (the ally flag) ⇒ 1, never harmful; else
    /// `Targets & 0x80` (the enemy flag) ⇒ 2; else, for each of the three effects, A then B, an
    /// implicit target in the byte tables `0x6ea338` / `0x6ea378` (identical) marked `0` ⇒ 2 —
    /// the set is `{2, 6, 15, 16, 24, 28, 53, 54}`, exactly the ids the modern enum names
    /// `*_ENEMY` (nearby enemy, target enemy, the two enemy areas, the two enemy cones, the enemy
    /// dest, the channeled enemy area). The function's remaining passes decide 1 (helpful) vs 0
    /// and can never yield 2, so this predicate is the whole `== 2` truth.
    pub fn is_harmful(&self) -> bool {
        const ENEMY_TARGETS: [u32; 8] = [2, 6, 15, 16, 24, 28, 53, 54];
        if self.targets & 0x100 != 0 {
            return false;
        }
        if self.targets & 0x80 != 0 {
            return true;
        }
        (0..3).any(|i| {
            ENEMY_TARGETS.contains(&self.effect_implicit_target_a[i])
                || ENEMY_TARGETS.contains(&self.effect_implicit_target_b[i])
        })
    }

    pub fn ranged_attack(&self) -> bool {
        self.attributes_ex2 & ATTR_EX2_AUTO_REPEAT != 0 || self.attributes & ATTR_RANGED != 0
    }

    /// **This cast aims itself at the equipped main hand** — `Attributes & 0x200`
    /// ([`ATTR_TARGET_MAIN_HAND_ITEM`]), the client's own auto-pick for a weapon imbue.
    ///
    /// `ArmCast 0x6e5250` resolves the cast's candidate guid from ONE of three places, and this
    /// bit picks the first: for a **player** caster (`6e5361`: `[[caster+8]+8] >> 4 & 1`, the
    /// typemask-`0x10` test) carrying the bit (`6e5371: test ah,0x2`), the candidate is the
    /// player's inventory slot table entry 15 — `[player+0x1d3c][15]`, `EQUIPMENT_SLOT_MAINHAND`
    /// (`6e5385`–`6e538e`, the `+0x78/+0x7c` guid pair; the table is the
    /// `[player+0x1d38]` count / `[player+0x1d3c]` array, bounds-checked at `6e5376`). Only if
    /// the bit is clear does the walk fall to the explicit guid, then to the current selection
    /// (`6e5393`/`6e539f`).
    ///
    /// So a spell carrying it never raises the item-targeting cursor over an equipped weapon:
    /// the imbue binds and sends on the press. Exactly 22 rows of the shipped 5875 `Spell.dbc`
    /// carry the bit, four distinct names — **Rockbiter / Flametongue / Frostbrand / Windfury
    /// Weapon**, every rank of the shaman's weapon imbues and nothing else — and every one is
    /// `Targets == 0x10` with an `ENCHANT_ITEM_TEMPORARY` effect (censused against the real file
    /// in `catalog_tests.rs`, which also records why vmangos answers 28).
    pub fn targets_main_hand_item(&self) -> bool {
        self.attributes & ATTR_TARGET_MAIN_HAND_ITEM != 0
    }

    /// The auto-repeat attribute **alone** (`AttributesEx2 & 0x20`) — the narrower gate on the
    /// client's local auto-repeat state bit (`[+0xd58] & 0x200`, written only at the local
    /// cast-send `0x6e593b`), which drives the shooter's standing Load/Hold idle. `Throw` is
    /// ranged but not auto-repeat — it never sets this.
    pub fn auto_repeat(&self) -> bool {
        self.attributes_ex2 & ATTR_EX2_AUTO_REPEAT != 0
    }

    /// `AttributesEx3 & 0x400000` — `SPELL_ATTR_EX3_CASTING_CANCELS_AUTOREPEAT`: a *running*
    /// auto-repeat carrying this bit is cancelled when any new cast begins (the client's
    /// `0x60959e` test on the **cached** spell). Exactly
    /// ONE of the 22357 `Spell.dbc` records has it — wand Shoot 5019; Auto Shot (75) survives
    /// new casts, which is how hunter shot-weaving works at all.
    pub fn casting_cancels_autorepeat(&self) -> bool {
        self.attributes_ex3 & 0x0040_0000 != 0
    }

    /// The ranged-slot attribute **alone** (`Attributes & 0x2`) — the client's weapon-visual
    /// fallback gate (`0x60d46a`): a ranged-slot spell whose own `SpellVisual` resolves to nothing
    /// borrows the equipped ranged weapon's `ItemDisplayInfo` visual for its fire animation —
    /// how Throw/Auto Shot/wand Shoot animate at all (every one of them has `SpellVisual1 = 0`).
    pub fn ranged_slot(&self) -> bool {
        self.attributes & ATTR_RANGED != 0
    }

    /// `AttributesEx3 & 0x8000` — this spell's damage numbers render **melee-white**, not
    /// spell-gold: the second half of the combat-text color law's `B` bit (the
    /// emitter `0x6128b0` tests `sign(byte[SpellRec+0x25])` = bit 15 of the `+0x24` word; the
    /// "no record ⇒ melee" half lives at the call sites). On the
    /// real 5875 DBC the bit is set on exactly the ranged basic shots — Auto Shot 75, Shoot Bow
    /// 2480, Throw 2764, wand Shoot 5019 — whose damage ships as spell packets yet floats white
    /// (vmangos delivers them via `SMSG_SPELLNONMELEEDAMAGELOG`; the modern name is
    /// `SPELL_ATTR3_NORMAL_RANGED_ATTACK`).
    pub fn melee_white_damage(&self) -> bool {
        self.attributes_ex3 & ATTR_EX3_NORMAL_RANGED_ATTACK != 0
    }

    /// `AttributesEx3 & 0x4` — **the casting bar shows no name for this spell**
    /// ([`ATTR_EX3_NO_CASTING_BAR_TEXT`], `SPELL_ATTR_EX3_NO_CASTING_BAR_TEXT`). The cast-bar feed
    /// hands `SPELLCAST_START`/`SPELLCAST_CHANNEL_START` an empty name string when this is set, so
    /// the bar fills and sweeps with a blank label.
    ///
    /// Only the *name* is suppressed, and only on the bar: the spell still casts, still animates,
    /// and its failures still name it. Three shipped rows carry the bit; the one that reached a
    /// player is 22810 **"Opening - No Text"**, the internal opener for `LockType 13` ground
    /// containers, whose own name spells out what the attribute is for.
    pub fn no_casting_bar_text(&self) -> bool {
        self.attributes_ex3 & ATTR_EX3_NO_CASTING_BAR_TEXT != 0
    }

    /// `AttributesEx3 & 0x2000` — **this spell shows no channel bar at all**
    /// ([`ATTR_EX3_NO_CHANNEL_BAR`]). The channel handler `0x6e7550` tests it before it composes
    /// anything (`0x6e7595 test ch,0x20` → `jne` the return), so `SPELLCAST_CHANNEL_START` never
    /// fires and the frame is never shown.
    ///
    /// A **different** suppression from [`Self::no_casting_bar_text`]'s and a total one: that bit
    /// blanks the cast bar's *label* and still draws the bar; this one removes the event. The two
    /// live one nibble apart in the same column and are easy to conflate — the channel path never
    /// reads bit 2 at all (`0x6e7a2d` is the only bit-2 test on `SpellRec+0x24` image-wide).
    ///
    /// Both shipped rows are 24322/24323 "Blood Siphon", the Hakkar encounter's drain.
    pub fn no_channel_bar(&self) -> bool {
        self.attributes_ex3 & ATTR_EX3_NO_CHANNEL_BAR != 0
    }

    /// `AttributesEx & 0x2000_0000` — **the channel bar prints this spell's own name**
    /// ([`ATTR_EX_CHANNEL_BAR_OWN_NAME`]); cleared, it prints the GlobalStrings word `CHANNELING`.
    /// `0x6e759a test DWORD PTR [SpellRec+0x1c],0x20000000` — set takes `Name[locale]`
    /// (`0x6e75a9`), clear takes `FrameScript_GetText(0x870dd0 = "CHANNELING")` (`0x6e75bc`).
    ///
    /// This is the whole of the channel bar's naming law, and it is the reverse default of the
    /// cast bar's: a cast bar names its spell unless told not to, a channel bar says "Channeling"
    /// unless told to name it. Nine of the 323 channeled rows in the shipped 5875 file opt in —
    /// the four Fishing ranks, Cannibalize, Dream Vision, Using Control Console and the two
    /// (suppressed) Blood Siphons. Blizzard, Arcane Missiles, Mind Flay, Drain Life/Soul/Mana,
    /// Rain of Fire, Hurricane, Tranquility, Evocation and First Aid all read the generic word.
    pub fn channel_bar_own_name(&self) -> bool {
        self.attributes_ex & ATTR_EX_CHANNEL_BAR_OWN_NAME != 0
    }

    /// The ranged-shot cooldown pad's gate (`0x6e2b60` at `0x6e2c2c`–`0x6e2c47`): on the caster's
    /// own SPELL_GO self-insert, a
    /// `Attributes & 0x2` spell WITHOUT `AttributesEx2 & 0x20000`
    /// (`SPELL_ATTR_EX2_DO_NOT_RESET_COMBAT_TIMERS`) adds the caster's live
    /// `UNIT_FIELD_RANGEDATTACKTIME` to its category recovery — how the Throw / wand Shoot
    /// button sweeps the weapon's speed with `Spell.dbc` recovery all zero and no server packet
    /// (the category-0 Auto Shot inserts the same timer but no read can surface it).
    pub fn ranged_speed_cooldown(&self) -> bool {
        self.attributes & ATTR_RANGED != 0
            && self.attributes_ex2 & ATTR_EX2_DO_NOT_RESET_COMBAT_TIMERS == 0
    }

    /// Whether this spell appears in the spellbook — the client's book **add-gate**
    /// (`CGPlayer_C::AddSpell
    /// 0x5e9c20` → the classify+append `0x4b25b0`). A known spell is kept OUT of the book when any
    /// of three gates trips: `Attributes & 0x80` (`SPELL_ATTR_DO_NOT_DISPLAY` — every language,
    /// armor/weapon proficiency, and hidden racial passive), `Attributes & 0x20`
    /// (`SPELL_ATTR_IS_TRADESKILL` — professions divert to a name-only path, no book slot), or
    /// `castUI != 0`. Concrete: Fireball 133 (`Attributes 0x10000`, castUI 0) is shown;
    /// Language:Common 668 / Cloth 9078 / One-Handed Axes 196 (all `0xC0 = PASSIVE|DO_NOT_DISPLAY`)
    /// are hidden. Passive-but-shown spells exist (a passive is NOT itself a book gate); only the
    /// three bits above hide a spell.
    pub fn in_spellbook(&self) -> bool {
        self.attributes & (ATTR_DO_NOT_DISPLAY | SPELL_ATTR_IS_TRADESKILL) == 0 && self.cast_ui == 0
    }

    /// Whether this spell appears in the **pet's** book — a *different* add-gate from
    /// [`Self::in_spellbook`]'s, and deliberately narrower. `0x4b2f90`, the pet
    /// book's whole append routine, tests the record's existence and then exactly one bit:
    ///
    /// ```text
    /// 0x4b2fa8  mov dl, byte ptr [rec + 0x18]     ; Attributes, byte 0
    /// 0x4b2fab  test dl, dl
    /// 0x4b2fad  js  <drop>                        ; the SIGN of the byte = 0x80 = DO_NOT_DISPLAY
    /// 0x4b2fb4  [0xb6f098 + 4*count++] = spellId
    /// ```
    ///
    /// No `IS_TRADESKILL` leg and no `castUI` leg — the player book's other two gates simply are
    /// not there. Reusing `in_spellbook` here would be a guess that happens to agree on every
    /// vmangos pet list today and would diverge the moment one didn't.
    pub fn in_pet_book(&self) -> bool {
        self.attributes & ATTR_DO_NOT_DISPLAY == 0
    }

    /// The **melee auto-attack** — `Effect[0] == SPELL_EFFECT_ATTACK (78)`, the client's own
    /// effect-type trigger for the equipped-weapon icon substitution (resolvers
    /// `0x4b3f8a`/`0x4e59de`). In 1.12 the only spell
    /// carrying it is 6603 "Attack" (its `SpellIconID` → the `Temp` placeholder the client never
    /// shows, substituting the main-hand weapon icon instead).
    pub fn is_melee_auto_attack(&self) -> bool {
        self.effects[0] == SPELL_EFFECT_ATTACK
    }

    /// The spell **tooltip's** passive gate (`0x52eb15`–`0x52eb45`): the
    /// CastTime|Cooldown line is skipped whole for `Attributes & 0x40` **or**
    /// `Effect[0] ∈ {0x2f, 0x4e}` — `SPELL_EFFECT_TRADE_SKILL` (the profession book entries) and
    /// `SPELL_EFFECT_ATTACK` (6603 "Attack"). Wider than [`Self::passive`], which is the
    /// spellbook's own gray-and-refuse gate and must keep reading the attribute alone.
    ///
    /// The field at `[+0xf4]` is Effect[0], not an icon id
    /// (`0xf4/4 == 61` — the same `[SpellRec+0xf4]` the auto-attack resolvers
    /// compare against `0x4e`). Confirmed on the shipped data: 6603 "Attack" carries
    /// `Effect[0] = 78` with `Attributes = 0x10`, and the reference's spellbook hover shows it
    /// with NO cast-time line.
    pub fn tooltip_omits_cast_line(&self) -> bool {
        self.passive
            || matches!(
                self.effects[0],
                SPELL_EFFECT_TRADE_SKILL | SPELL_EFFECT_ATTACK
            )
    }

    /// The spell **tooltip's** range gate — the two attribute tests the builder runs BEFORE it
    /// ever calls `GetMinMaxRange 0x6e3480` (`0x52e9a5`: `Attributes & 0x404`, the on-next-swing
    /// pair; `0x52e9b2`: `AttributesEx3 & 0x40000000`), each jumping straight past the cell.
    /// The third absence case is not an attribute — it is a resolved `max <= 0` (`0x52e9ed`),
    /// which is what the 11 777 self-only rows produce and is by far the dominant one.
    ///
    /// So a Heroic Strike or a Backstab shows **no range cell at all** — not a melee wording.
    pub fn tooltip_omits_range_line(&self) -> bool {
        self.on_next_swing() || self.attributes_ex3 & 0x4000_0000 != 0
    }

    /// The cooldown getter's HEAD exclusion (`GetCooldownInfo 0x6e13e0` @ `6e1439`/`6e1442`):
    /// `Effect[0] ∈ {0x4e ATTACK, 0x2f TRADE_SKILL}`
    /// returns "no cooldown" unconditionally — the reason the Attack and profession buttons never
    /// show a pie AND their presses can never be cooldown-refused.
    pub fn cooldown_query_excluded(&self) -> bool {
        matches!(
            self.effects[0],
            SPELL_EFFECT_TRADE_SKILL | SPELL_EFFECT_ATTACK
        )
    }

    // The equipped-item requirement's NAME used to be decided here, by a
    // `single_equipped_subclass` that answered only for a one-bit mask — a placeholder standing in
    // for `0x6e2380`. `0x6e2380` reads only ItemSubClass.dbc,
    // behind an ItemSubClassMask.dbc group lookup that names a wide mask in one word. The whole
    // rule (both spellings, both call sites) lives with the vocabulary it reads:
    // [`crate::ItemSubClassCatalog::requirement_name`].

    /// The **ranged** icon-substitution gate — `Attributes & 0x2` AND `AttributesEx2 & 0x20`,
    /// both bits (decision 0231's deferred ranged case; the paired tests at `0x4b3f99`/`0x4b3f9f`
    /// and `0x4e5a2e`/`0x4e5a34` gate the
    /// call into the ranged helper `0x4e6990`). Auto Shot 75 and wand Shoot 5019 carry both;
    /// Throw 2764 carries only `0x2` and keeps its own icon.
    pub fn ranged_icon_substitution(&self) -> bool {
        self.attributes & ATTR_RANGED != 0 && self.attributes_ex2 & ATTR_EX2_AUTO_REPEAT != 0
    }

    /// `SPELL_ATTR_COOLDOWN_ON_EVENT` (Attributes bit 25) — the cooldown record is stored **on
    /// hold** (timers parked) until `SMSG_COOLDOWN_EVENT` starts it ([`ATTR_COOLDOWN_ON_EVENT`]).
    pub fn cooldown_on_event(&self) -> bool {
        self.attributes & ATTR_COOLDOWN_ON_EVENT != 0
    }

    /// A **combo-point consumer** (`AttributesEx` bits 20/22, [`ATTR_EX_FINISHING_MOVE`]) — the
    /// usable walk's combo-point gate (`0x6e3e7a`) greys it while the caster's combo-point byte
    /// is 0. The
    /// rogue/druid finishers, and Overpower.
    pub fn needs_combo_points(&self) -> bool {
        self.attributes_ex & ATTR_EX_FINISHING_MOVE != 0
    }

    /// The on-next-swing class (`Attributes & 0x404`, [`ATTR_ON_NEXT_SWING`]) — the spell queues
    /// on the server's melee slot and fires on the next swing instead of casting. The client's
    /// one mask for the class: the already-casting exemption (a queued one never blocks another
    /// cast), the queued-melee slot tracking, and the range law's self-cast short-circuit all
    /// test exactly these bits.
    pub fn on_next_swing(&self) -> bool {
        self.attributes & ATTR_ON_NEXT_SWING != 0
    }

    /// The tooltip cast cell's "Attack speed" arm — `Attributes & 0x2` ALONE (the byte test at
    /// `0x52ec1c`: `test al,0x2`; 1074), NOT [`Self::ranged_attack`]'s or-pair: Throw carries
    /// only `0x2` and still reads "Attack speed" in its cell.
    pub fn tooltip_on_next_ranged(&self) -> bool {
        self.attributes & ATTR_RANGED != 0
    }

    /// The tooltip cast cell's "Channeled" arm — `AttributesEx & 0x44` (`SPELL_ATTR_EX_CHANNELED`
    /// both variants), the byte test at `0x52ec27` (`test [rec+0x1c],0x44`; folded as 1074).
    /// Distinct from
    /// [`Self::channel_interrupt_flags`], which is the *running* channel's break mask.
    pub fn tooltip_channeled(&self) -> bool {
        self.attributes_ex & ATTR_EX_CHANNELED != 0
    }

    /// Whether *casting* this spell turns on the melee auto-attack **at the send** —
    /// `Spell_C::TryCast`'s post-send tail (`6e51b5`): fires the attack entry `0x6131a0` iff the
    /// cast committed+sent this call (`[0xcead5c] == spell_id`), the rec passes
    /// `[ebp-2] = 0x6e5200 && Ex2-bit20 CLEAR`, and no attack is already running (`0x60ecb0`).
    /// With `0x6e5200 = (Attr & 0x404) || (AttrEx & 0x200) || (AttrEx2 & 0x100000)`, the
    /// bit20-clear exclusion reduces the send-time law to exactly this: on-next-swing
    /// (the queued strike needs swings to fire at all) or [`ATTR_EX_INITIATES_COMBAT`]
    /// (Rend, Sunder Armor, Slam), and not [`ATTR_EX2_INITIATE_COMBAT_POST_CAST`] (whose
    /// GO-deferred start we don't build — dormant by data). **Path-independent**: action
    /// button, spellbook, and `CastSpellByName` all share the one `TryCast` tail — 1.12 has
    /// no macro-side opt-out.
    pub fn initiates_auto_attack(&self) -> bool {
        (self.attributes & ATTR_ON_NEXT_SWING != 0
            || self.attributes_ex & ATTR_EX_INITIATES_COMBAT != 0)
            && self.attributes_ex2 & ATTR_EX2_INITIATE_COMBAT_POST_CAST == 0
    }

    /// Whether *this spell's own `SMSG_SPELL_GO`* turns on the melee auto-attack — the **deferred**
    /// half of the same law, [`Self::initiates_auto_attack`]'s exact complement.
    /// `HandleSpellGo 0x6e7a70` @ `0x6e83c0` (re-read at the bytes for 1593):
    ///
    /// ```text
    /// 6e83c0  call 0x6e5230(rec); test al,al; jne 6e83da   ; AttrEx2 bit20 SET -> start
    /// 6e83cb  call 0x6e5200(rec); test al,al; je  6e8407   ; else the triad must hold
    /// 6e83d4  test byte[rec+0x54],0x8; je 6e8407           ; ...AND rec+0x54 bit3
    /// 6e83da  lea ecx,[esi+0xc48]; call 0x47bf60; or eax,edx; jne 6e8407  ; NOT already attacking
    /// 6e83e9  hitCount>0 ? guid = hits[0] : (0,0)          ; [ebp-0x20] count, [ebp-0x1c] array
    /// 6e8402  call 0x6131a0(ecx=caster, guid)              ; START MELEE AUTO-ATTACK
    /// ```
    ///
    /// **Only the bit20 leg is modelled**, deliberately — and the other one was measured when B280
    /// closed, rather than left to the argument below. `rec+0x54` is `Spell.dbc` **column
    /// 21, `InterruptFlags`** (VERIFIED position; bit 3's *semantics* stay INFERRED), which is
    /// [`Self::interrupt_flags`] — so we could build the leg today. Its live set in the shipped
    /// file is **8 rows, 3 names** (Slam, Shield Slam, Polymorphic Ray) and is **disjoint from the
    /// 36** bit20 rows, so every one of them passes `0x6e5200` with bit20 CLEAR and therefore
    /// started its attack at the *send* ([`Self::initiates_auto_attack`]); by their GO the
    /// reference's `[+0xc48]` is set and `0x6e83e7` refuses. Unreachable-in-effect, measured, not
    /// assumed. Modelling it would also be actively *wrong* here: our engaged mirror is the
    /// server-echoed `Engaged`, not a local lock, so a leg the reference silences with `[+0xc48]`
    /// would fire a second `CMSG_ATTACKSWING` on our side.
    ///
    /// The 36 spells that reach this and nothing else are the stealth openers and positional
    /// strikes — Backstab, Garrote, Ambush, Cheap Shot, Shred, Ravage, Pounce — plus Judgement:
    /// exactly the class whose attack must wait for the server to say the strike landed.
    pub fn initiates_auto_attack_at_go(&self) -> bool {
        self.attributes_ex2 & ATTR_EX2_INITIATE_COMBAT_POST_CAST != 0
    }

    /// Hidden from the player's buff bar — the cache-builder's **display filter**
    /// (`PlayerAuras_Update 0x4e4170`'s append pass, sites `0x4e42b6`–`0x4e42c8`). The `Attributes`
    /// clause is a **byte-width** read — `mov cl,[SpellRec+0x18]; test cl,cl; js` — so it tests
    /// bit `0x80` (`SPELL_ATTR_DO_NOT_DISPLAY`), NOT the dword sign bit
    /// (decision 0385 corrects 0268; vmangos
    /// corroborates: `SpellDefines.h:799` "not visible in spellbook or aura bar"). The other
    /// clause is `SpellRec+0x1c & 0x10000000` (`SPELL_ATTR_EX_NO_AURA_ICON`). The aura stays live
    /// on the wire and in `UNIT_FIELD_AURA` — the reference just never admits it to the display
    /// cache, so `GetPlayerBuff` cannot return it. Warrior stances (the `AttributesEx` clause) and
    /// internal proc auras like Defensive State 5302 (the `0x80` clause) are the everyday cases.
    pub fn hidden_from_aura_bar(&self) -> bool {
        self.attributes & ATTR_DO_NOT_DISPLAY != 0 || self.attributes_ex & ATTR_EX_NO_AURA_ICON != 0
    }

    /// A **tracking** spell — any of the three `EffectApplyAuraName` slots is
    /// `TRACK_CREATURES`/`TRACK_RESOURCES`/`TRACK_STEALTHED` ([`TRACKING_AURA_TYPES`], the
    /// byte-verified `{0x2c,0x2d,0x97}` test at `SpellRec+0x16c..+0x174`). A tracking aura is
    /// excluded from **every** aura display — the player-cache rebuild skips it *before* the
    /// insert (so the buff bar never shows Find Minerals), and `IsAuraDisplayable 0x519860`
    /// hides it from other units' rows the same way. Instead, the rebuild records the last
    /// visible tracking spell for `GetTrackingTexture` — the minimap's `MiniMapTrackingFrame`
    /// icon (its one consumer; `benilla::ui_aura` mirrors the whole law).
    pub fn tracking_aura(&self) -> bool {
        self.effect_apply_aura
            .iter()
            .any(|a| TRACKING_AURA_TYPES.contains(a))
    }

    /// The spell's `Stances` mask is an **allow-list, not a requirement** — `AttributesEx2`
    /// bit 19. Two independent consumers test exactly this bit: the form gate `0x612480` (which
    /// [`Self::form_refusal`] transcribes) and, separately, the spell tooltip's required-form
    /// line at `52f115` — the builder duplicates the test inline rather than
    /// calling the gate, so one predicate here keeps our two callers from drifting the way they
    /// did in 1483.
    ///
    /// 5875 leans on this hard, and reading `Stances != 0` as "requires a form" without it is
    /// simply wrong: every Shadowform-castable priest spell carries `Stances` bit 27 (Inner Fire,
    /// Psychic Scream, Shadow Word: Pain, Power Word: Shield…), every Spirit-of-Redemption-castable
    /// heal carries bit 31 (Flash Heal, Renew, Prayer of Healing), and the 52 balance-druid spells
    /// castable in Moonkin carry bit 30 (Wrath, Moonfire, Starfire, Thorns…) — the mask records
    /// "may ALSO be cast in that form". All 244 of them carry bit 19; **none** of the 274 spells
    /// that genuinely require a form (the warrior stances, the druid forms, the stealth openers)
    /// does. So this is the clean separator between the two readings of one column.
    pub fn form_mask_is_permissive(&self) -> bool {
        self.attributes_ex2 & ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED != 0
    }

    /// The shapeshift-form gate as a boolean — the usable walk's form leg (`0x6e3ec6`). See
    /// [`Self::form_refusal`], the reason-carrying transcription this wraps.
    pub fn usable_in_form(&self, form: u8, form_is_stance: bool) -> bool {
        self.form_refusal(form, form_is_stance).is_none()
    }

    /// The shapeshift-form gate — `0x612480` (reads the caster form, builds
    /// `1 << (form-1)`, tests **StancesNot** then **Stances**, then the Attributes-bit-16 /
    /// AttributesEx2-bit-19 composition), which both the usable walk (`0x6e3ec6`) and the TryCast
    /// requirement validator (`0x6094f0`, the leg right after the mounted block) run. The
    /// composition over the form's stance flag — and the reason split — is the vmangos
    /// corroboration (`SpellEntry::GetErrorAtShapeshiftedCast`, `SpellEntry.cpp` — anchored to
    /// exactly the byte-named fields): a *stance*-flagged form (warrior stances, stealth —
    /// `SpellShapeshiftForm.flags1 & 1`) does not count as "shapeshifted", so ordinary spells
    /// stay usable in it; a true shapeshift (cat, bear, Ghost Wolf) blocks NOT_SHAPESHIFT
    /// spells; and a form-requiring spell out of its form is refused unless AttributesEx2
    /// bit 19 waives the requirement. `None` = castable in this form.
    pub fn form_refusal(&self, form: u8, form_is_stance: bool) -> Option<FormRefusal> {
        let stance_bit = if form == 0 { 0 } else { 1u32 << (form - 1) };
        if self.stances_not & stance_bit != 0 {
            return Some(FormRefusal::NotShapeshift);
        }
        if self.stances & stance_bit != 0 {
            return None;
        }
        if form != 0 && !form_is_stance {
            // A true shapeshift: NOT_SHAPESHIFT spells are blocked (0x3d); a spell needing
            // some other form is too (0x56).
            if self.attributes & ATTR_NOT_SHAPESHIFT != 0 {
                Some(FormRefusal::NotShapeshift)
            } else if self.stances != 0 {
                Some(FormRefusal::OnlyShapeshift)
            } else {
                None
            }
        } else if self.stances != 0 && !self.form_mask_is_permissive() {
            // Unshifted (or a stance): a form-requiring spell is refused here unless waived.
            Some(FormRefusal::OnlyShapeshift)
        } else {
            None
        }
    }
    /// The spell's name with its rank subtext appended **the way the client composes it** —
    /// `"%s (%s)"` (the literal at `0x8468b0`, read out of the reference image) when [`Self::rank`]
    /// is a non-empty string, and the bare name when it is not.
    ///
    /// Two surfaces build this string and both reach the same literal, so it is one method rather
    /// than a copy each: the trainer window's prerequisite-ability list
    /// (`GetTrainerServiceAbilityReq 0x4d96e0`) and the
    /// learn announcement's argText (`0x4b2963`'s empty-subtext test, then either
    /// `0x4b2982 call 0x64a7f0` — `SStrPrintf(buf, 0x200, "%s (%s)", name, subtext)` — or
    /// `0x4b29a0 call 0x64a5a0`, the plain copy). The format is a property of the spell record,
    /// so it belongs with the record.
    pub fn ranked_name(&self) -> String {
        match self.rank.as_deref() {
            Some(rank) if !rank.is_empty() => format!("{} ({})", self.name, rank),
            _ => self.name.clone(),
        }
    }

    /// **Which line the client prints in chat when this spell is learned**, or `None` for one it
    /// learns silently.
    ///
    /// The announcement is the tail of the spell-added registrar `0x4b25b0` — the same function
    /// whose head sets the known-spell bit and whose gates [`Self::in_spellbook`] models — and it
    /// runs only when the registrar's `edx` flag is set. That flag is **not** an "announce" flag,
    /// though this is the leg benilla uses it for: at `0x4b2b4f` it gates the book re-sort +
    /// `SPELLS_CHANGED`, `LEARNED_SPELL_IN_TAB` and tutorial trigger 40 atomically with the chat
    /// line, which is why the login drain fires one batched `0x4b2fd0(1,0)` rather than one per
    /// spell. It is the **live-mutation** flag; decision 2246 names the legs still unbuilt. That flag is the whole
    /// reason logging in is silent while a trainer purchase is not: the `SMSG_INITIAL_SPELLS`
    /// drain replays the book through `AddSpell` with it **clear** (`0x5deaa4 push 0x1;
    /// 0x5deaa6 push 0x0` — arg3 is the flag `AddSpell` forwards as `edx` at `0x5e9c5c`), while
    /// `SMSG_LEARNED_SPELL`'s handler passes `1` (`0x5e61c0` -> `AddSpell(id, slot, 1, 1)`) and
    /// `SMSG_SUPERCEDED_SPELL`'s reaches the registrar through the supersede pair `0x4b2f50`,
    /// which hardcodes `mov edx,0x1` at `0x4b2f61`. **So a rank-up announces too** — and the
    /// unlearn half of that same pair (`0x4b2c50`) contains no `DisplayError` call at all, which
    /// is why a rank-up prints one line and not two, and why `SMSG_REMOVED_SPELL` prints nothing.
    ///
    /// The three-way itself is `0x4b2909`, reading `Attributes` (`SpellRec+0x18`) and nothing
    /// else:
    ///
    /// ```text
    /// 0x4b290f  test al,al / js  <out>      ; 0x80 DO_NOT_DISPLAY -> no line at all
    /// 0x4b2917  test al,0x20 / je <below>   ; 0x20 IS_TRADESKILL  -> id 0x39 ERR_LEARN_RECIPE_S
    /// 0x4b294a  and eax,0x10                ; 0x10 ABILITY
    /// 0x4b29a9  setne al / add eax,0x37     ; -> 0x37 ERR_LEARN_SPELL_S or 0x38 ERR_LEARN_ABILITY_S
    /// ```
    ///
    /// The recipe arm pushes the **bare** name (`0x4b292c`, `SpellRec+0x1e0`) and returns from the
    /// function outright; the other two push [`Self::ranked_name`]. Every one of the three ids is
    /// a `MsgKind::Chat` row carrying chat type `10` (`CHAT_MSG_SYSTEM`) in the message catalog,
    /// so all three are chat lines, never `UIErrorsFrame` toasts.
    ///
    /// Note what the block does **not** read: the spell's power type, its school, its class, and
    /// its `SPELL_ATTR_PASSIVE` bit are all irrelevant — a passive is announced like anything
    /// else unless it also carries `DO_NOT_DISPLAY`, which in the shipped data it usually does.
    /// Whether the client announces **unlearning** this spell — *"You have unlearned %s."*
    /// (`ERR_SPELL_UNLEARNED_S`, message id `0x14a`), with the bare localized name and no rank.
    ///
    /// A **different** gate set from [`Self::learn_announcement`]'s, in a different function, and
    /// not guessable from it — which is how decision 2243 came to claim, with a byte citation,
    /// that the reference never prints an unlearn line at all. It does. The claim came from
    /// bounding `RemoveSpell 0x5e9fe0` at the `ret 0x8` at `0x5ea28f`; that `ret` is a *block*
    /// end, and `0x5ea292` is a live branch target past it — the function really runs to
    /// `0x5ea2ba`, and the announce is in the part 2243 never read.
    ///
    /// Four gates, all silent, taken in the binary's order:
    ///
    /// ```text
    /// 0x5ea03a  sete cl                      ; announce := (suppress == 0)  — the caller's arg
    /// 0x5ea03d  test al,0x20 / je 0x5ea172   ; IS_TRADESKILL falls into the container path,
    /// 0x5ea170  xor ecx,ecx                  ;   whose join ZEROES the flag -> silent
    /// 0x5ea17a  jle 0x5ea292                 ; castUI > 0 takes the castUI-container walk and
    ///                                        ;   rejoins at 0x5ea245 -> silent (0x5ea292 has
    ///                                        ;   exactly ONE entry, this one)
    /// 0x5ea29b  js 0x5ea248                  ; DO_NOT_DISPLAY -> silent
    /// 0x5ea2ab  push 0x14a / call 0x496720   ; else the line, with SpellRec+0x1e0 (name only)
    /// ```
    ///
    /// `suppress` is the caller's, not the record's, so it is not modelled here: benilla's only
    /// producer is `SMSG_REMOVED_SPELL` (`0x5e43e3`, `suppress = 0`), and the rank-up path that
    /// passes `1` (`0x5e6392`) does not reach this function on our side at all.
    ///
    /// Note `castUI` gates the *unlearn* line and **not** the learn line — the learn block tests
    /// it only afterwards, at `0x4b29bf`, to decide the book slot. A spell with `castUI > 0`
    /// therefore announces when it is learned and says nothing when it is taken away. That
    /// asymmetry is the reference's, not an oversight here.
    pub fn announces_unlearn(&self) -> bool {
        self.attributes & SPELL_ATTR_IS_TRADESKILL == 0
            && self.cast_ui == 0
            && self.attributes & ATTR_DO_NOT_DISPLAY == 0
    }

    pub fn learn_announcement(&self) -> Option<LearnAnnouncement> {
        if self.attributes & ATTR_DO_NOT_DISPLAY != 0 {
            return None;
        }
        if self.attributes & SPELL_ATTR_IS_TRADESKILL != 0 {
            return Some(LearnAnnouncement::Recipe);
        }
        Some(if self.attributes & ATTR_ABILITY != 0 {
            LearnAnnouncement::Ability
        } else {
            LearnAnnouncement::Spell
        })
    }
}

/// A [`SpellDisplay::learn_announcement`] verdict — which of the three `ERR_LEARN_*` chat lines
/// the client prints when the spell is learned. The key each one names, and whether the argText
/// carries the rank, is the caller's to map (`benilla_app::spell::net`): the ids live in the
/// message catalog, which is a UI-layer table, not a `Spell.dbc` fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearnAnnouncement {
    /// Message id `0x37` — `ERR_LEARN_SPELL_S`, "You have learned a new spell: %s."
    Spell,
    /// Message id `0x38` — `ERR_LEARN_ABILITY_S`, "You have learned a new ability: %s."
    /// (`Attributes & 0x10`, `SPELL_ATTR_ABILITY`.)
    Ability,
    /// Message id `0x39` — `ERR_LEARN_RECIPE_S`, "You have learned how to create a new item: %s."
    /// (`Attributes & 0x20`, `SPELL_ATTR_IS_TRADESKILL`.) Its argText is the **bare** name: the
    /// reference's recipe arm never reaches the rank-subtext composer.
    Recipe,
}

/// A [`SpellDisplay::form_refusal`] verdict — which cast-fail reason `0x612480` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormRefusal {
    /// `SPELL_FAILED_NOT_SHAPESHIFT` (0x3d, "Can't do that while shapeshifted"): blocked by the
    /// current form — a StancesNot hit, or a NOT_SHAPESHIFT spell in a true shapeshift.
    NotShapeshift,
    /// `SPELL_FAILED_ONLY_SHAPESHIFT` (0x56, "Must be in %s"): the spell requires a form the
    /// caster is not in.
    OnlyShapeshift,
}

impl FormRefusal {
    /// The wire/cast-fail reason byte (the `SPELL_FAILED_*` table index).
    pub fn reason(self) -> u8 {
        match self {
            FormRefusal::NotShapeshift => 0x3d,
            FormRefusal::OnlyShapeshift => 0x56,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `0x6ea280 == 2` predicate on synthetic rows: the two `Targets` flags outrank the
    /// implicit-target walk (ally wins over everything, enemy wins next), and the walk reads all
    /// three effects' A and B slots against the byte tables' enemy set.
    #[test]
    fn is_harmful_follows_the_client_classifier() {
        let mut d = SpellDisplay::default();
        assert!(!d.is_harmful(), "an empty row targets nobody");
        d.targets = 0x80;
        assert!(d.is_harmful(), "the enemy target flag alone is harmful");
        d.targets = 0x80 | 0x100;
        assert!(!d.is_harmful(), "the ally flag wins over the enemy flag");
        d.targets = 0;
        d.effect_implicit_target_a = [6, 0, 0];
        assert!(d.is_harmful(), "TARGET_UNIT_TARGET_ENEMY in slot 0");
        d.effect_implicit_target_a = [21, 0, 0];
        assert!(!d.is_harmful(), "a single-friend target is not harmful");
        d.effect_implicit_target_b = [0, 16, 0];
        assert!(d.is_harmful(), "an enemy area in a B slot counts too");
        d.effect_implicit_target_b = [0, 0, 0];
        d.effect_implicit_target_a = [22, 0, 54];
        assert!(d.is_harmful(), "the enemy cone in the third effect");
        d.targets = 0x100;
        assert!(!d.is_harmful(), "the ally flag short-circuits the walk");
    }

    /// The learn announcement's three-way at `0x4b2909`, read off `Attributes` alone — and its
    /// one silent case. The bit order is the binary's: `DO_NOT_DISPLAY` is tested first and wins
    /// over a row that also declares `IS_TRADESKILL`, which in turn wins over `ABILITY`.
    #[test]
    fn learn_announcement_reads_the_three_attribute_bits_in_the_clients_order() {
        let mut d = SpellDisplay::default();
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Spell),
            "a plain row is a spell — Fireball 133 carries Attributes 0x10000"
        );
        d.attributes = 0x10;
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Ability),
            "SPELL_ATTR_ABILITY picks the ability wording"
        );
        d.attributes = 0x20;
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Recipe),
            "SPELL_ATTR_IS_TRADESKILL diverts to the recipe line"
        );
        d.attributes = 0x20 | 0x10;
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Recipe),
            "the tradeskill branch is taken before the ability bit is ever read"
        );
        d.attributes = 0x80;
        assert_eq!(
            d.learn_announcement(),
            None,
            "SPELL_ATTR_DO_NOT_DISPLAY is announced silently — every language and proficiency"
        );
        d.attributes = 0x80 | 0x20;
        assert_eq!(
            d.learn_announcement(),
            None,
            "…and the sign test at 0x4b290f runs before the tradeskill test at 0x4b2917"
        );
        d.attributes = 0x40;
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Spell),
            "PASSIVE is not read by the block at all — only DO_NOT_DISPLAY silences a spell"
        );
    }

    /// The unlearn line's four gates (`0x5e9fe0`'s block at `0x5ea292`) — a different set from
    /// the learn block's, in a different function. `castUI` is the one that is easy to miss: it
    /// silences the unlearn and does nothing to the learn.
    #[test]
    fn announces_unlearn_is_not_the_mirror_of_the_learn_gates() {
        let mut d = SpellDisplay::default();
        assert!(d.announces_unlearn(), "a plain row says it");
        d.attributes = 0x10;
        assert!(
            d.announces_unlearn(),
            "ABILITY is a learn-wording bit and nothing to this path"
        );
        d.attributes = 0x40;
        assert!(d.announces_unlearn(), "PASSIVE is not read here either");
        d.attributes = 0x20;
        assert!(
            !d.announces_unlearn(),
            "IS_TRADESKILL: 0x5ea170 zeroes the flag"
        );
        d.attributes = 0x80;
        assert!(!d.announces_unlearn(), "DO_NOT_DISPLAY: 0x5ea29b");
        d.attributes = 0;
        d.cast_ui = 1;
        assert!(
            !d.announces_unlearn(),
            "castUI > 0 never reaches 0x5ea292 — its single entry is 0x5ea17a jle"
        );
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Spell),
            "…while the SAME row still announces its learn: castUI is read after the message"
        );
    }

    /// `"%s (%s)"` (`0x8468b0`) versus the bare copy, keyed on the rank subtext being a non-empty
    /// string — the reference tests the first BYTE of the subtext (`0x4b2963 cmp BYTE PTR [eax],0x0`),
    /// so a present-but-empty column is the bare name, not `"Name ()"`.
    #[test]
    fn ranked_name_appends_the_subtext_only_when_there_is_one() {
        let mut d = SpellDisplay {
            name: "Fireball".to_string(),
            ..SpellDisplay::default()
        };
        assert_eq!(d.ranked_name(), "Fireball", "no subtext, no parentheses");
        d.rank = Some(String::new());
        assert_eq!(
            d.ranked_name(),
            "Fireball",
            "an empty subtext is no subtext"
        );
        d.rank = Some("Rank 2".to_string());
        assert_eq!(d.ranked_name(), "Fireball (Rank 2)");
    }
}
