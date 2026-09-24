//! Spell.dbc + SpellIcon.dbc loader — the spell **display** catalog the action bar reads
//! (decision 0068 slice 1): a spell id resolves to its name and its icon's BLP path.
//!
//! Layout — VERIFIED against build 5875 (empirical column derivation on the extracted files,
//! 2026-07-02): `Spell.dbc` is 22357 records × **173 fields** (692 B — matches the loader's own
//! header checks, `0x55007e`/`0x5500bb`). The two columns this catalog reads were pinned
//! by resolving known spells against the data: **SpellIconID = column 117** (78 "Heroic Strike" →
//! icon 856 `Ability_Rogue_Ambush`, 6673 "Battle Shout" → icon 456 `Ability_Warrior_BattleShout`;
//! both hit column 117 uniquely) and **SpellName enUS = column 120** (all probe spells resolve
//! their known name there and nowhere else — the head of the 8-locale + flags name block,
//! 120..128). `SpellIcon.dbc` is 1033 records × 2 fields: `ID(0), TextureFilename(1, str)` — paths
//! like `Interface\Icons\Ability_Rogue_Ambush` (no extension; the BLP loader appends it).
//!
//! **SpellVisualID = column 115 · projectile Speed (f32) = column 37** (decision 0107's phase-2
//! data plane) — pinned empirically, 2026-07-04, against the local vmangos `mangos.spell_template`
//! (`spellVisual1`/`speed`, build ≤ 5875) cross-checked column-by-column over the extracted
//! `Spell.dbc`: for every nonzero `spellVisual1` in `spell_template` (1309 rows sampled, ids
//! 1..2000), column 115 is the **only** u32 column that matches, and for every nonzero `speed`
//! (61 rows), column 37 is the only f32 column that matches — both unique across all 173 columns.
//! Column 37 also matches the `+0x94` Speed read at `0x6e814b` bit-for-bit (`0x94 / 4 == 37`); the
//! `+0x1e0` visual-id lead does **not** hold raw (`0x1e0 / 4 == 120`, already pinned above as the
//! name string), confirming the in-memory record isn't a raw row copy for that field — column 115
//! is the empirical column, not a translated offset. Spot-check (Fireball, entry 133):
//! `spell_template` gives `spellVisual1=67, speed=24.0`; `Spell.dbc` row 133 reads column 115 = 67,
//! column 37 = 24.0. `spellVisual2` (the DB's second visual column) is 0 for all but one spell in
//! the whole table — not read here. `speed == 0` means an instant-impact spell (decision 0099's
//! `Speed==0` gate — no missile phase); `visual == 0` means no `SpellVisual.dbc` row (silent cast).
//!
//! **Attributes = column 6 · AttributesEx2 = column 8** (decision 0099 phase 5's ranged-stance
//! gate) — pinned the same way, 2026-07-05: over every `spell_template` row with a nonzero
//! attribute word (2700 spells, ids ≤ 4000, each at its `MAX(build) ≤ 5875` row), column 6 is the
//! **only** column matching `attributes` (runner-up 49/2700) and column 8 the only one matching
//! `attributesEx2` — and both agree with the reads at `0x6e5922`/`0x6e591c` (`SpellRec+0x18` /
//! `+0x20`: `0x18/4 == 6`, `0x20/4 == 8`; the record layout is raw this early, before the
//! translated tail that broke the `+0x1e0` lead above). Only two bits are consumed — the client's
//! ranged gate, [`SpellDisplay::ranged_attack`]; `Attributes` bit `0x40` — `SPELL_ATTR_PASSIVE`
//! (vmangos `SpellDefines.h`) — for [`SpellDisplay::passive`]; and the
//! spellbook **add-gate** [`SpellDisplay::in_spellbook`] — `Attributes` bit `0x80`
//! (`SPELL_ATTR_DO_NOT_DISPLAY`), bit `0x20` (`SPELL_ATTR_IS_TRADESKILL`), and **castUI = column
//! 3** (`SpellRec+0xc`, `0xc/4 == 3` — raw this early like columns 6/8). castUI was pinned by the
//! same byte read that pinned the gate (`0x4b29bf`, in the classify+append `0x4b25b0`), not an
//! empirical column derivation; it reads 0 for every ordinary player spell
//! (Fireball 133), so it is exercised only as a gate that never trips on the shown set.
//!
//! **SpellNameSubtext enUS = column 129** — pinned empirically the same way as
//! the name column: NameEnUs's own locale block runs `120..127` (8 locales) `+128` (the name's
//! flags word), so the very next `LocalizedString` head at `129` was the candidate; confirmed
//! against six real spells spanning the whole rank range (Fireball 133/143/145 → "Rank 1"/"Rank
//! 2"/"Rank 3", Frost Armor 168 → "Rank 1", Corruption 172 → "Rank 1", Fire Blast 2136 → "Rank
//! 1") — every one resolves at column 129 and nowhere else in `118..146`. Notably even a spell's
//! FIRST rank carries the literal text "Rank 1" (the ref's `SpellButton_UpdateButton` shows
//! whatever `subSpellName` answers, unconditionally — no "hide Rank 1" special case, matching
//! `SpellBookFrame.lua:379-402`).
//!
//! **Description enUS = column 138 · AuraDescription enUS = column 147** (decision 0274 P2, the
//! tooltip arc) — the loc-block arithmetic predicts both exactly: NameSubtext's own 9-dword block
//! (129 enUS + 130..136 the other 7 locales + 137 flags) is immediately followed by a Description
//! block (138 enUS .. 146 flags), then an AuraDescription block (147 enUS .. 155 flags), landing
//! *exactly* on the already-pinned `ManaCostPercentage = 156` (129 + 9 + 9 + 9 == 156) — two
//! independently-verified anchors 27 columns apart with no room for anything else. Confirmed by
//! content, not just arithmetic: read against every `spell_template` row carrying description/
//! auraDescription text on the local vmangos DB (14278 spells), column 138 matches 13243/13258
//! (99.89%) and column 147 matches 4215/4225 (99.76%) of the nonempty rows verbatim (raw,
//! un-substituted `$`-token text) — the same noise floor the *already-shipped* name(120)/
//! nameSubtext(129) pins show against this same DB (14260/14278, 6015/6020: a handful of
//! vmangos-only QA/test spells diverge from the shipped client file, not a column slip). Spot
//! checks: Fireball 133 → "Hurls a fiery ball that causes $s1 Fire damage…"; Frost Armor 168's aura
//! text → "Increases Armor by $s1 and may slow attackers."; Fire Blast 2136 (a direct-damage spell
//! with no aura) → empty aura text.
//!
//! **DurationIndex = column 30 · CastingTimeIndex = column 18 · ProcChance = column 25** — pinned
//! empirically 2026-07-10 against the local vmangos `spell_template` (every entry at its own
//! `MAX(build) ≤ 5875` row, the module's established cross-check, ~22357 spells): each is the
//! *unique* column (of all 173) matching its named vmangos field on every nonzero-valued row —
//! `durationIndex` 11350/11350, `castingTimeIndex` 22354/22354, `procChance` 22129/22129, all
//! 100%. End-to-end confirmation through the new [`crate::SpellCastTimeCatalog`]/
//! [`crate::SpellDurationCatalog`]: Fireball 133 → CastingTimeIndex 16 → `SpellCastTimes.dbc` row
//! 16 → 1500 ms; Frost Armor 168 → DurationIndex 30 → `SpellDuration.dbc` row 30 → 1,800,000 ms
//! (30 min) — both match this arc's own independently-stated expectation. `[rec+0x48]` (== column
//! 18) is CastingTimeIndex, not *DurationIndex*: `Spell_C::GetCastTime` is `0x6e3340`
//! (`6e336e: mov eax,[edi+0x48]` —
//! CastingTimeIndex → `SpellCastTimes` recordsById `[0xc0d878]`, spell-mod op `0xa` =
//! SPELLMOD_CASTING_TIME), the real `Spell_C::GetDuration` is `0x6ea000` (`6ea016: mov
//! eax,[edi+0x78]` — DurationIndex → `SpellDuration` recordsById `[0xc0d828]`, spell-mod op `1` =
//! SPELLMOD_DURATION), and `0x6e31b0` is `GetPowerCost`, not "GetCastTime". The
//! in-memory SpellRec is a direct on-disk row image this deep (offset = column×4, anchored by
//! the verified neighbours `+0x4c`/col19 and `+0x90`/col36), so the byte reads and this
//! empirical map agree exactly: `0x48/4 == 18`, `0x78/4 == 30`.
//!
//! **Per-effect arrays** (each `[3]`, one slot per effect — pinned the same value-match way, same
//! cross-check population, 2026-07-10): `EffectDieSides` 64-66 (die roll, **signed**),
//! `EffectBaseDice` 67-69, `EffectBasePoints` 76-78 (the roll's floor, **signed** — Auto Shot 75 /
//! Feign Death 5384 both carry `-1`, a weapon-damage/no-fixed-roll sentinel), `EffectRadiusIndex`
//! 88-90 (`SpellRadius.dbc` row; Battle Shout 6673 carries 9, its "$a1 yards" party radius),
//! `EffectApplyAuraName` 91-93 (== the already-pinned [`COL_EFFECT_APPLY_AURA_1`]; 9612/9614,
//! 2488/2490, 595/595 — reused, not re-derived), `EffectAmplitude` 94-96 (periodic-tick ms;
//! Fireball 133's DoT tail ticks every 2000 ms), `EffectMultipleValue` 97-99 (f32),
//! `EffectChainTarget` 100-102 (100/101 at 113/10 nonzero samples, 100%; 102 has only *one*
//! nonzero row in the whole table — entry 26044, `1,1,1` across 100-102 — but lands exactly on the
//! consecutive-triple pattern every other array here confirms; flagged as the thinnest-evidence pin
//! of the batch), `EffectTriggerSpell` 109-111 (== the already-pinned [`COL_EFFECT_TRIGGER_1`];
//! Frost Armor 168's own description text names the exact spell its `EffectTriggerSpell[1]`
//! resolves to — `$6136s2%`/`$6136s1%`, `6136` read straight off column 110). All match 100% except
//! `Effect[0]`/`Effect[1]`/`EffectApplyAuraName[0]`/`EffectApplyAuraName[1]`, which match all but 2
//! rows each (the same 4 vmangos test entries — 11094/13043/11189/28332 — whose DB row diverges
//! from the shipped client file; not a column slip).
//!
//! Remaining columns (targeting/school-specific secondary fields, item-set data) stay dropped
//! until a consumer exists — the bar/book/tooltip want a face and a description, not a full spell
//! simulator (we render and speak the protocol; the server simulates).

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
/// `Category` (`SpellRec+0x8`, `0x8/4 == 2` — raw like columns 3/6/8): the shared-cooldown
/// category (`SpellCategory.dbc` id) — potions 4, Aimed Shot 2, … `0` = no category. Pinned
/// empirically 2026-07-10 against the vmangos `spell_template` rows (12 spells incl. Charge
/// cat 44, Taunt 82, Lay on Hands 56, Hunter's Mark 411, wand Shoot 351 — all match column 2).
const COL_CATEGORY: usize = 2;
const COL_CAST_UI: usize = 3;
/// `RecoveryTime` (`SpellRec+0x4c`, `0x4c/4 == 19`) / `CategoryRecoveryTime` (`+0x50`, 20) — the
/// spell's own cooldown ms and its category's shared cooldown ms, the two inputs of the client's
/// `StartCooldown 0x6e2c60` + the `SMSG_SPELL_COOLDOWN` handler `0x6e9460`.
/// Same 12-spell empirical pin as
/// [`COL_CATEGORY`] (Feign Death 30000 rec, Charge 15000 catRec, Lay on Hands 3600000 catRec).
const COL_RECOVERY_TIME: usize = 19;
const COL_CATEGORY_RECOVERY_TIME: usize = 20;
/// `InterruptFlags` (21, `SpellRec+0x54`) / `AuraInterruptFlags` (22, `SpellRec+0x58`) /
/// `ChannelInterruptFlags` (23, `SpellRec+0x5c`) — what breaks a cast / a live aura / a running
/// channel. Pinned empirically 2026-07-17 by the module's established cross-check (every vmangos
/// `spell_template` row at its `MAX(build) ≤ 5875`): column 21 is the **unique** column matching
/// `interruptFlags` on all 6389 nonzero rows, column 22 unique on all 582 nonzero
/// `auraInterruptFlags` rows, column 23 unique on all 343 nonzero `channelInterruptFlags` rows,
/// and the raw-row arithmetic agrees (offset = column×4, chain-locked between the verified
/// neighbors `+0x50`/col 20 and col 25). Spot checks: Fireball 133 interrupt `0xf`, channel 0;
/// Arcane Missiles 5143 channel `0x7c0c`; First Aid 746 interrupt 0, channel `0x3c0e`. Consumed
/// by the cast bar's local self-cancel (`benilla::ui_cast`) and the cast-initiation moving gate
/// (`benilla::ui_action`), where the bit semantics are documented.
const COL_INTERRUPT_FLAGS: usize = 21;
const COL_AURA_INTERRUPT_FLAGS: usize = 22;
const COL_CHANNEL_INTERRUPT_FLAGS: usize = 23;
/// `powerType` (31) / `manaCost` (32) / `ManaCostPercentage` (156) — the cast-cost triple
/// `IsUsableAction`'s not-enough-power verdict reads. Same empirical pin (Taunt/Charge pwr 1 =
/// rage; Pyroblast cost 125; pct pinned on its own nonzero rows 370/475/526/527/528 → 10/10/9/18/15).
const COL_POWER_TYPE: usize = 31;
const COL_MANA_COST: usize = 32;
const COL_MANA_COST_PCT: usize = 156;
/// `manaCostPerlevel` (33) / `manaPerSecond` (34) — the vmangos field order between `manaCost`
/// 32 and `manaPerSecondPerLevel` 35. The per-level column is `0x6e31b0`'s
/// `(level − spellLevel) · perLevel` term (72 nonzero rows, all creature spells); the
/// per-second column is the tooltip's `_PER_TIME` composite (Health Funnel 755 reads 5).
/// Column 35 is all-zero across the whole 5875 file (the catalog test's scan) and stays
/// unparsed. (1074)
const COL_MANA_COST_PER_LEVEL: usize = 33;
const COL_MANA_PER_SECOND: usize = 34;
/// `rangeIndex` (`SpellRec+0x90`, `0x90/4 == 36`) — the `SpellRange.dbc` row
/// `GetMinMaxRange 0x6e3480` resolves. Same empirical pin (Auto
/// Shot/Aimed Shot 114, Throw 74, Charge 95, Fireball 35).
const COL_RANGE_INDEX: usize = 36;
/// `modalNextSpell` — `Spell.dbc` **column 38** (`SpellRec + 0x98`): a spell the client casts **by
/// itself**, with no user input, when the server's `SMSG_CAST_RESULT` for this spell arrives. See
/// [`SpellDisplay::modal_next_spell`] for the whole law and the shipped-file census.
///
/// The column's position is chain-locked the same way the interrupt trio's is (offset = column×4,
/// between the verified `+0x90` = 36 rangeIndex and `+0xf4` = 61 Effect[0]); the name is the
/// community's. This repo's own reason to trust it is
/// the distribution: non-zero on 57 of 22357 rows, and 52 of those 57 name spell **75, Auto Shot**,
/// every one of them a hunter shot. A column that fell where this one falls by accident could not
/// look like that.
const COL_MODAL_NEXT_SPELL: usize = 38;
/// `StartRecoveryCategory` (`SpellRec+0x274`, `0x274/4 == 157`) / `StartRecoveryTime` (`+0x278`,
/// 158) — the **global-cooldown** pair `StartGlobalCooldown 0x6e2de0` reads at the local
/// cast-send (`0x6e58fb`). Category 133 / 1500 ms for ordinary
/// spells; 0/0 for the GCD-free (Attack, Auto Shot, wand Shoot). Same 12-spell empirical pin.
const COL_START_RECOVERY_CATEGORY: usize = 157;
const COL_START_RECOVERY_TIME: usize = 158;
/// `PreventionType` (`SpellRec+0x294`, `0x294/4 == 165`) — see [`SpellDisplay::prevention_type`]
/// for the two-way pin that separates it from its `DmgClass` neighbour at 164.
const COL_PREVENTION_TYPE: usize = 165;
/// `SpellFamilyName` (`SpellRec+0x280`, `0x280/4 == 160`) / `SpellFamilyFlags` low+high
/// (`+0x284`/`+0x288`, 161/162) — the talent spell-modifier gate and its row selector, read by
/// `GetSpellModifiers 0x6e6b30` at `6e6b38`/`6e6b46` and `6e6b83`.
/// See [`SpellDisplay::spell_family`] and
/// [`SpellDisplay::spell_family_flags`]; the shipped-file anchors are in
/// [`catalog_tests`].
const COL_SPELL_FAMILY_NAME: usize = 160;
const COL_SPELL_FAMILY_FLAGS_LOW: usize = 161;
/// `Targets` (`SpellRec+0x34`, `0x34/4 == 13`) — the wire `TARGET_FLAG_*` seed mask the cast-arm
/// loads into its targeting flag_word (`0x6e525a`). Empirical
/// pin against the binder's bit semantics: Resurrection 2006 = `0x8000` (corpse-ally bit 15),
/// Skinning 8613 = `0x402` (unit bit 1 + requires-explicit-selection bit 10), ground AoEs carry
/// `0x40` (dest location), enchant/poison rows `0x10` (item). `0` for ordinary casts.
const COL_TARGETS: usize = 13;
/// `EffectImplicitTargetA[0]` (`SpellRec+0x148`, `0x148/4 == 82`) — the implicit-target enum the
/// cast-arm's 62-case switch keys on (jump-table `0x6e5484`) to set/clear individual flag_word
/// bits. Empirical pin against the switch arms: Fireball 133 / Charge 100 = 6 (→ hostile bit 7),
/// Ice Armor 7302 / Feign Death 5384 = 1 (self — clears bit 10), Arcane Intellect 1459 / Lesser
/// Heal 2050 = 21 (→ assist bit 8), Battle Shout 6673 = 20 (party-area — a no-op arm).
const COL_IMPLICIT_TARGET_A1: usize = 82;
/// `EffectImplicitTargetB[0]` (`SpellRec+0x154`, `0x154/4 == 85`) — the second implicit-target
/// column, walked beside A by the hostility classifier `0x6ea280` ([`SpellDisplay::is_harmful`]).
/// Empirical pin: Frost Nova 122 carries A = 22 (caster coordinates) and B = 15 (src-area enemy)
/// — harmful through B alone.
const COL_IMPLICIT_TARGET_B1: usize = 85;
/// The usable-walk columns (`IsSpellUsableNow 0x6e3d60`'s gate table; column = SpellRec-offset/4).
/// Empirical pins on the real 5875 data: Claw 1082 Stances `0x1` (cat = form 1), Ambush 8676
/// Stances `0x20000000` (stealth = form 30) + EquippedItemClass 2 / SubClassMask `0x8000`
/// (dagger), Execute 5308 TargetAuraState 2 (healthless-20%) + Stances `0x50000`
/// (battle/berserker), Revenge 6572 CasterAuraState 1 (defense), Auto Shot 75 EquippedItemClass
/// 2 / SubClassMask `0x4000c` (bows/guns/crossbows), Slow Fall 130 Reagent[0] 17056 ×1
/// (Light Feather).
const COL_STANCES: usize = 11;
const COL_STANCES_NOT: usize = 12;
const COL_CASTER_AURA_STATE: usize = 16;
const COL_TARGET_AURA_STATE: usize = 17;
const COL_TOTEM_1: usize = 40;
const COL_REAGENT_1: usize = 42;
const COL_REAGENT_COUNT_1: usize = 50;
const COL_EQUIPPED_ITEM_CLASS: usize = 58;
const COL_EQUIPPED_ITEM_SUBCLASS_MASK: usize = 59;
/// `EquippedItemInventoryTypeMask` — chain-locked between `COL_EQUIPPED_ITEM_SUBCLASS_MASK`
/// (`SpellRec+0xec`) and `COL_EFFECT_1` (`+0xf4`), i.e. exactly the `+0xf0` the reference's
/// item-target gate reads at `0x495d60` @ `495e4d`.
const COL_EQUIPPED_ITEM_INVENTORY_TYPE_MASK: usize = 60;
/// `RequiresSpellFocus` (`SpellRec+0x3c`, `0x3c/4 == 15` — the run the pinned neighbors chain-lock:
/// `Stances` 11-12, `Targets` 13, `CasterAuraState` 16). A `SpellFocusObject.dbc` id that must be
/// nearby to cast (1 Anvil, 3 Forge, 4 Cooking Fire — [`crate::spell_focus`]); 0 = none. Verified
/// against live vmangos `spell_template` rows on the real 5875 file (catalog_tests).
const COL_REQUIRES_SPELL_FOCUS: usize = 15;
/// `Dispel` — the `SpellDispelType.dbc` id (`SpellRec+0x10`; the byte offset chain-locks it to
/// `COL_CAST_UI` at `+0xc` and `COL_ATTRIBUTES` at `+0x18`).
const COL_DISPEL: usize = 4;
/// `School` (`SpellRec+0x4`, `0x4/4 == 1`) — see [`SpellDisplay::school`].
const COL_SCHOOL: usize = 1;
/// `Mechanic` (`SpellRec+0x14`, `0x14/4 == 5`) — see [`SpellDisplay::mechanic`].
const COL_MECHANIC: usize = 5;
/// `EffectMechanic[0]` (`SpellRec+0x13c`, `0x13c/4 == 79`) — see [`SpellDisplay::effect_mechanic`].
const COL_EFFECT_MECHANIC_1: usize = 79;
const COL_ATTRIBUTES: usize = 6;
/// `AttributesEx` (`SpellRec+0x1c` — chain-locked between `COL_ATTRIBUTES` at `+0x18` and
/// `COL_ATTRIBUTES_EX2` at `+0x20`).
const COL_ATTRIBUTES_EX: usize = 7;
const COL_ATTRIBUTES_EX2: usize = 8;
/// `AttributesEx3` (`SpellRec+0x24` — the next word in the same chain-locked run). Consumed by
/// [`SpellDisplay::melee_white_damage`]'s bit 15.
const COL_ATTRIBUTES_EX3: usize = 9;
const COL_SPEED: usize = 37;
/// `Effect[0]` (`SpellRec+0xf4`, `0xf4/4 == 61` — raw this early, like columns 6/8/37): the
/// spell's first effect type. The action bar / spellbook auto-attack icon substitution keys on it:
/// `Effect[0] == SPELL_EFFECT_ATTACK (78)` is the melee auto-attack (resolvers
/// `0x4b3f8a` / `0x4e59de` both `cmp [SpellRec+0xf4], 0x4e`).
const COL_EFFECT_1: usize = 61;
/// `EffectMiscValue[0]` (column 106). The Spell.dbc effect block is 15 parallel `[3]` arrays from
/// `Effect@61`; `EffectMiscValue` is the 16th, at `61 + 15×3 = 106` — cross-checked by `SpellVisual`
/// landing at its known column 115 (`106 + 3×3`). For a `SPELL_EFFECT_OPEN_LOCK` effect this is the
/// `LockType` index the spell opens (mining = 3, herbalism = 2, per `Lock.dbc`).
const COL_EFFECT_MISC_1: usize = 106;
/// `EffectTriggerSpell[0]` — column 109 (`61 + 3×16`, past the 16 effect-array blocks: the three
/// `Effect`s at 61 are followed by die/base/aura/misc/etc. blocks, `EffectMiscValue` at 106 then
/// `EffectTriggerSpell` at 109, `SpellVisual` at 115). For a `SPELL_EFFECT_LEARN_SPELL` effect this is
/// the taught spell.
const COL_EFFECT_TRIGGER_1: usize = 109;
/// `SpellEffects` value `36` — `SPELL_EFFECT_LEARN_SPELL`: a trainer's learn wrapper carries it, and
/// its `EffectTriggerSpell` is the ability the player ends up with (decision 0247's taught spell).
pub const SPELL_EFFECT_LEARN_SPELL: u32 = 36;

/// One of a spell's three effect slots, as the trainer's state re-evaluator reads them
/// (`0x4d7d40`): the three effect
/// types that decide whether a service is already "known". Any other effect type is not a learn
/// effect and is skipped. Slot order is kept — the reference walks `+0xf4/+0xf8/+0xfc` in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnEffect {
    /// `SPELL_EFFECT_LEARN_SPELL` — teaches `EffectTriggerSpell`.
    Spell(u32),
    /// `SPELL_EFFECT_SKILL_STEP` — raises skill line `skill` (`EffectMiscValue`) to `step`. The
    /// step is the effect's value, `EffectBasePoints + EffectDieSides`: on the real build-5875
    /// file every profession opener carries `die = 1` and `base = step − 1` (Apprentice `0+1`,
    /// Journeyman `1+1`, Expert `2+1`, Artisan `3+1` — pinned by
    /// `the_openers_skill_steps_read_off_the_real_file`).
    SkillStep { skill: u32, step: u32 },
    /// `SPELL_EFFECT_LEARN_PET_SPELL` — teaches the player's pet `EffectTriggerSpell`.
    PetSpell(u32),
}
/// `SpellEffects` value `57` — `SPELL_EFFECT_LEARN_PET_SPELL`, the learn wrapper's pet twin. The
/// trainer's **icon** law accepts either in its three-slot wrapper scan (the paired
/// `cmp ecx,0x24` / `cmp ecx,0x39` at `0x4d8ff5`/`0x4d8ffa`). [`SpellCatalog::learned_spell`]'s
/// **display** hop deliberately stays 36-only — that is decision 0247's verified grouping hop, and
/// widening it is a separate question nothing has asked.
pub const SPELL_EFFECT_LEARN_PET_SPELL: u32 = 57;
/// `SpellEffects` value `44` — `SPELL_EFFECT_SKILL_STEP`: the effect that raises a profession's
/// *potential* (Apprentice → Journeyman → …). It is the whole of what a **tradeskill trainer's**
/// display tree partitions on — the builder tests the WIRE spell's three effect slots for it and
/// reads nothing else (`0x4d77b6`, decision 1124; benilla-app's `ui_trainer::law::service_group` is
/// the transcription).
pub const SPELL_EFFECT_SKILL_STEP: u32 = 44;

/// `SpellEffects` value `SPELL_EFFECT_OPEN_LOCK` — the lock-opening effect the GameObject
/// interact-cast matches (`0x5f84a1`, the client's
/// `cmp [SpellRec+0xf4], 0x21`). A spell carrying it opens the `LockType` its `EffectMiscValue` names.
const SPELL_EFFECT_OPEN_LOCK: u32 = 0x21;
/// `baseLevel` — column 28 (`SpellRec+0x70`), the level term the effect-value walk subtracts at
/// `0x6e3826`/`0x6e3854` and the cast-time scaling's base (`0x6e3340`). **Not** the DBC's
/// `spellLevel` (column 29, `+0x74`) — nothing here reads that one; the field carried that name
/// for a while.
/// Pinned by value on the extracted 5875 file: Pick Lock 1804 and Fireball rank 1 read `1`, the
/// professions' openers `0`.
const COL_BASE_LEVEL: usize = 28;
/// `maxLevel` — column 27 (`SpellRec+0x6c`), the cap on the skill-derived level term in an
/// opener's value: the player's skill is clamped to `maxLevel × 5` at `0x5ea6e3` before the
/// `/5` (`0x6e3195`); `0` = uncapped (the professions' openers read 0 here, and their values
/// would compute 0 under an unconditional clamp).
const COL_MAX_LEVEL: usize = 27;
/// `spellLevel` — column 29 (`SpellRec+0x74`), the DBC's actual spellLevel. Parsed for the ONE
/// consumer whose recorded law names `+0x74` (the Beast Training rank comparator,
/// `benilla-ui` `craft.rs`); before [`COL_BASE_LEVEL`]'s rename that consumer was silently fed
/// column 28.
const COL_SPELL_LEVEL: usize = 29;
/// `EffectApplyAuraName[0]` (column 91, `61 + 10×3` — tenth of the effect-`[3]` blocks; pinned on
/// the extracted 5875 file: every form spell — Battle Stance 2457, Bear 5487, Cat 768, Stealth
/// 1784, Moonkin 24858 — carries `36` here, and columns 94-96 don't). The stance bar's
/// admission key (`0x4b2810`): `== SPELL_AURA_MOD_SHAPESHIFT`.
const COL_EFFECT_APPLY_AURA_1: usize = 91;
/// `AuraType` value `36` — `SPELL_AURA_MOD_SHAPESHIFT` (vmangos `SpellAuraDefines.h`): the aura
/// that changes the form byte; its `EffectMiscValue` is the `SpellShapeshiftForm.dbc` form id.
const SPELL_AURA_MOD_SHAPESHIFT: u32 = 36;
const COL_ICON_ID: usize = 117;
/// `ActiveIconID` (column 118, `SpellRec+0x1d8` — right after `SpellIconID`, read at
/// `0x4b4754`): the icon shown while the spell's form is ACTIVE, when
/// nonzero (druid forms carry icon 122, the "dismiss form" paw; warrior stances carry 0).
const COL_ACTIVE_ICON_ID: usize = 118;
/// `StanceBarOrder` (column 166, `SpellRec+0x298`, SIGNED — the stance-bar sort comparator
/// `0x4b2bb0` orders ascending by it, `-1` last, spell
/// id as the tiebreak). Pinned on the extracted file: Battle 0 / Defensive 1 / Berserker 2,
/// Bear 0 / Aquatic 1 / Cat 2 / Travel 3 / Moonkin 4, Stealth −1.
const COL_STANCE_BAR_ORDER: usize = 166;
const COL_VISUAL_ID: usize = 115;
const COL_NAME_ENUS: usize = 120;
const COL_NAME_SUBTEXT_ENUS: usize = 129;
/// `Description` enUS (module docs — the loc-block arithmetic + content match).
const COL_DESCRIPTION_ENUS: usize = 138;
/// `AuraDescription` enUS (module docs).
const COL_AURA_DESCRIPTION_ENUS: usize = 147;

/// `DurationIndex` (`SpellRec+0x78`, `0x78/4 == 30` — module
/// docs: `Spell_C::GetDuration 0x6ea000`'s own read). `0` = no row (an instant-hit spell with no
/// periodic/aura tail — Fire Blast 2136).
const COL_DURATION_INDEX: usize = 30;
/// `CastingTimeIndex` (`SpellRec+0x48`, `0x48/4 == 18` — module
/// docs: `Spell_C::GetCastTime 0x6e3340`'s own read).
const COL_CASTING_TIME_INDEX: usize = 18;
/// `ProcChance` (module docs) — percent, `101` is vmangos's own "always proc, no roll" convention
/// on top of the DBC's raw 0-100 scale (carried through unexamined; not a client-side reinterpretation).
const COL_PROC_CHANCE: usize = 25;

/// The per-effect `[3]` arrays feeding the $-token tooltip engine (decision 0274 P2) — each column
/// is that family's slot-0 entry; slots 1/2 are `+1`/`+2` (module docs' consecutive-triple law,
/// confirmed for every array below). `EffectDieSides`/`EffectBasePoints` are **signed** (a roll's
/// floor can be negative, and `-1` is the weapon-damage/no-fixed-roll sentinel — Auto Shot,
/// Feign Death); the rest are their natural unsigned/float DBC type.
const COL_EFFECT_DIE_SIDES_1: usize = 64;
const COL_EFFECT_BASE_DICE_1: usize = 67;
/// `EffectDicePerLevel[0]` — column 70 (`SpellRec+0x118`), the integer per-level term of the
/// effect-value walk.
const COL_EFFECT_DICE_PER_LEVEL_1: usize = 70;
/// `EffectRealPointsPerLevel[0]` — column 73 (`SpellRec+0x124`), the **float** per-level term:
/// 5.0 on every profession opener, 0.6 on Fireball rank 1 — the two anchors that pin the column.
const COL_EFFECT_REAL_POINTS_PER_LEVEL_1: usize = 73;
const COL_EFFECT_BASE_POINTS_1: usize = 76;
const COL_EFFECT_RADIUS_INDEX_1: usize = 88;
const COL_EFFECT_AMPLITUDE_1: usize = 94;
const COL_EFFECT_MULTIPLE_VALUE_1: usize = 97;
const COL_EFFECT_CHAIN_TARGETS_1: usize = 100;
/// `EffectItemType[0]` — column 103, the `[3]` block between `EffectChainTarget` (100-102) and
/// `EffectMiscValue` (106-108) in the module docs' parallel-array walk. The item entry a
/// `SPELL_EFFECT_CREATE_ITEM` effect creates — the crafting book's product. Verified
/// against live vmangos rows on the real 5875 file (catalog_tests: 2963 → 2996 Bolt of Linen
/// Cloth, 2738 → 2845 Copper Axe).
const COL_EFFECT_ITEM_TYPE_1: usize = 103;
// `EffectApplyAuraName[0]` reuses [`COL_EFFECT_APPLY_AURA_1`] (91) and `EffectTriggerSpell[0]`
// reuses [`COL_EFFECT_TRIGGER_1`] (109) — both already pinned above for the shapeshift/learn-spell
// hops; the tooltip arc exposes the full `[3]` array off the same columns instead of re-deriving.

/// `AttributesEx3` bit `0x8000` — damage renders melee-white (`SPELL_ATTR3_NORMAL_RANGED_ATTACK`;
/// the combat-text emitter `0x6128b0`'s `B`-bit flip).
const ATTR_EX3_NORMAL_RANGED_ATTACK: u32 = 0x8000;
/// `AttributesEx3` bit `0x4` — **the cast bar shows no name for this spell**
/// (`SPELL_ATTR_EX3_NO_CASTING_BAR_TEXT`, vmangos `Spells/SpellDefines.h:907`).
///
/// Exactly three rows in the shipped 5875 file carry it, and one of them is *named after the bit*:
/// 6477 "Opening", **22810 "Opening - No Text"**, 26380 "zzOLDSummon Mouth Tentacle Visual". That
/// placeholder name is not a string benilla was ever meant to render — it is Blizzard's own note
/// to themselves about what this attribute does to the spell, and it reached a player's screen
/// because we printed the name unconditionally.
const ATTR_EX3_NO_CASTING_BAR_TEXT: u32 = 0x4;
/// `AttributesEx3` bit 13 (`0x2000`) — **this spell shows no CHANNEL bar at all**. The channel
/// handler's own suppressor, and a *total* one: `0x6e7595 test ch,0x20` on `[SpellRec+0x24]`
/// (`ch` = bits 8-15, so bit 13 — not [`ATTR_EX3_NO_CASTING_BAR_TEXT`]'s bit 2) jumps straight to
/// the return, so `SPELLCAST_CHANNEL_START` never fires. Exactly two rows in the shipped 5875 file
/// carry it, both 24322/24323 "Blood Siphon".
const ATTR_EX3_NO_CHANNEL_BAR: u32 = 0x2000;
/// `AttributesEx` bit 29 (`0x2000_0000`) — **the channel bar prints this spell's own name**
/// instead of the generic word. `0x6e759a test DWORD PTR [SpellRec+0x1c],0x20000000`: set ⇒
/// `Name[locale]`, clear ⇒ the GlobalStrings value of `CHANNELING`. Nine of the 323 channeled rows
/// in the shipped file set it — the four Fishing ranks (7620/7731/7732/18248), 20578 Cannibalize,
/// 11403 Dream Vision, 24937 Using Control Console and the two Blood Siphons (which the suppressor
/// above hides anyway). Every other channel — Blizzard, Arcane Missiles, Mind Flay, Drain Life,
/// Rain of Fire, Hurricane, Tranquility, Evocation, First Aid — reads "Channeling".
const ATTR_EX_CHANNEL_BAR_OWN_NAME: u32 = 0x2000_0000;
/// `Attributes` bit `0x2` — the "uses the ranged slot" attribute (mangos `SPELL_ATTR_RANGED`).
/// One half of the client's ranged-stance gate (module docs).
const ATTR_RANGED: u32 = 0x2;
/// `Attributes` bit 9 (`0x200`) — **this cast targets the equipped main hand, without asking**
/// (`SPELL_ATTR0_TARGET_MAINHAND_ITEM`). vmangos leaves the bit unnamed (`SPELL_ATTR_UNK9`)
/// because it is a purely *client-side* targeting instruction: the server only ever sees the
/// resolved item guid. The law is `ArmCast 0x6e5250`'s candidate leg (`6e536e`–`6e5391`), read by
/// [`SpellDisplay::targets_main_hand_item`].
const ATTR_TARGET_MAIN_HAND_ITEM: u32 = 0x200;
/// `AttributesEx2` bit `0x20` — the auto-repeat attribute (mangos `SPELL_ATTR_EX2_AUTO_REPEAT`);
/// the other half of the gate. Auto Shot and wand Shoot carry it.
const ATTR_EX2_AUTO_REPEAT: u32 = 0x20;
/// `AttributesEx2` bit `0x20000` — `SPELL_ATTR_EX2_DO_NOT_RESET_COMBAT_TIMERS` (vmangos
/// `SpellDefines.h`). Its absence on a ranged spell opts the cast into the ranged-speed cooldown
/// pad ([`SpellDisplay::ranged_speed_cooldown`]).
const ATTR_EX2_DO_NOT_RESET_COMBAT_TIMERS: u32 = 0x20000;
/// `Attributes` bit `0x40` — `SPELL_ATTR_PASSIVE` (vmangos `SpellDefines.h`'s `SpellAttributes`
/// enum, `0x00000040 // 6`): the spellbook's own passive gate ([`SpellDisplay::passive`]). The
/// ref's `SpellButton_UpdateButton` reads `IsSpellPassive` to gray the name/highlight art
/// (`SpellBookFrame.lua:379-390`) — a passive is real player state (a permanent weapon-skill/
/// stance-derived buff), never something the player casts, so the engine's `CastSpell` binding
/// (`benilla-ui/src/script/spellbook.rs`) refuses it outright rather than
/// sending a doomed cast the server would just reject.
const ATTR_PASSIVE: u32 = 0x40;
/// `Attributes` bit `0x10` — `SPELL_ATTR_ABILITY` (cmangos `SpellDefines.h`). The **only** thing
/// the 1.12 client reads it for is the learn announcement's wording: `0x4b29a9 setne al` /
/// `0x4b29b3 add eax,0x37` picks message id `0x37` `ERR_LEARN_SPELL_S` when the bit is clear and
/// `0x38` `ERR_LEARN_ABILITY_S` when it is set. See [`SpellDisplay::learn_announcement`].
const ATTR_ABILITY: u32 = 0x10;
/// `Attributes` bit `0x80` — `SPELL_ATTR_DO_NOT_DISPLAY` (cmangos `SpellDefines.h`: "Hidden in
/// Spellbook, Aura Icon, Combat Log"): THE spellbook add-gate AND the `Attributes`
/// half of the aura-bar display filter ([`SpellDisplay::hidden_from_aura_bar`] — the cache
/// builder's byte-width read at `0x4e42b6` tests exactly this bit). Every language, armor/weapon
/// proficiency, hidden racial passive, and internal proc aura (Defensive State 5302) carries it.
const ATTR_DO_NOT_DISPLAY: u32 = 0x80;
/// `Attributes` bit `0x20` — `SPELL_ATTR_IS_TRADESKILL` (cmangos `SpellDefines.h`): profession /
/// recipe spells the client diverts to a name-only path, never a book slot. See
/// [`SpellDisplay::in_spellbook`].
///
/// Public because four separate surfaces test this one bit and each had grown its own copy of the
/// literal: the spellbook's add-gate here, the TradeSkill and Craft admission filters, and — since
/// the trainer TOOLTIP law landed — the gate that decides whether a trainer service renders an ITEM
/// tooltip instead of a spell one (`SetTrainerService 0x5338b0` at `0x533a1b`, and again inside the
/// spell builder's own redirect at `0x52e6d2`).
pub const SPELL_ATTR_IS_TRADESKILL: u32 = 0x20;
/// `AttributesEx` bit `0x10000000` — vmangos `SPELL_ATTR_EX_NO_AURA_ICON` ("Client doesn't display
/// these spells in aura bar"): the other display-filter bit. All three warrior stances carry it
/// (extracted 5875 `Spell.dbc`: 2457 `AttributesEx 0x90000000`, 71/2458 `0x10000000`).
const ATTR_EX_NO_AURA_ICON: u32 = 0x1000_0000;

/// `SpellEffects` value `78` — `SPELL_EFFECT_ATTACK` (cmangos `SpellEffectDefines.h`): the melee
/// auto-attack effect. The only spell carrying it in 1.12 is 6603 "Attack", so the client's
/// effect-type trigger and a hardcoded-6603 trigger are behaviourally identical — but the client
/// checks the effect, so [`SpellDisplay::is_melee_auto_attack`] does too.
const SPELL_EFFECT_ATTACK: u32 = 78;

/// The three **tracking** aura types (`EffectApplyAuraName` values, vmangos `SpellAuraDefines.h`):
/// `SPELL_AURA_TRACK_CREATURES` 44, `SPELL_AURA_TRACK_RESOURCES` 45, `SPELL_AURA_TRACK_STEALTHED`
/// 151 — the client's own set `{0x2c,0x2d,0x97}`, tested against the three
/// `EffectApplyAuraName` dwords at `SpellRec+0x16c..+0x174` in BOTH aura display filters: the
/// player-cache rebuild's Pass 2 (`0x4e42xx`, which also records the matching spell for
/// `GetTrackingTexture`) and the shared `IsAuraDisplayable 0x519860` (`0x5198c2`–`0x5198d3`)
/// the `UnitBuff`/`UnitDebuff` walks call.
/// See [`SpellDisplay::tracking_aura`].
const TRACKING_AURA_TYPES: [u32; 3] = [44, 45, 151];

/// `Attributes` bits `0x4` + `0x400` — the two ON_NEXT_SWING attributes (vmangos
/// `SPELL_ATTR_ON_NEXT_SWING_NO_DAMAGE` / `SPELL_ATTR_ON_NEXT_SWING`). The client always tests
/// them as ONE mask (`SpellRec+0x18 & 0x404` — the already-casting exemption `6e4d97`, the
/// self-cast range short-circuit `0x6e34fb`, the attribute predicate `0x6e5200`):
/// a spell of this class doesn't cast — it queues on the server's melee slot
/// and fires on the caster's next swing (Heroic Strike 78 carries `0x4`, Raptor Strike 2973
/// `0x404`, Cleave 845 `0x4`). See [`SpellDisplay::on_next_swing`].
const ATTR_ON_NEXT_SWING: u32 = 0x404;
/// `AttributesEx` bit `0x200` — vmangos `SPELL_ATTR_EX_INITIATES_COMBAT` ("Enables Auto-Attack").
/// The server only reads it for pet AI (vmangos `Spell.cpp:4377`); the player-facing "casting
/// this starts my auto-attack" is client-side (one leg of predicate `0x6e5200`, read by
/// `TryCast`'s post-send tail `6e51b5` → the attack entry `0x6131a0`).
/// Rend/Sunder Armor/Slam/Sinister Strike carry it; Heroic
/// Strike and Charge do not.
const ATTR_EX_INITIATES_COMBAT: u32 = 0x200;
/// `AttributesEx` bits 0x4|0x40 — the two CHANNELED variants, tested as one mask by the tooltip's
/// cast cell (`0x52ec27`: `test [rec+0x1c],0x44` → "Channeled"; 1074).
const ATTR_EX_CHANNELED: u32 = 0x44;
/// `AttributesEx2` bit `0x100000` — vmangos `SPELL_ATTR_EX2_INITIATE_COMBAT_POST_CAST` ("Client
/// will send CMSG_ATTACK_SWING after SMSG_SPELL_GO"). The send-tail predicate
/// EXCLUDES it (`[ebp-2] = 0x6e5200 && Ex2-bit20 CLEAR`): a bit20 spell defers its attack-start
/// to the `SMSG_SPELL_GO` handler (`0x6e83c0`) instead of starting at send. Both halves are
/// built — [`SpellDisplay::initiates_auto_attack`] is the send-time one, and
/// [`SpellDisplay::initiates_auto_attack_at_go`] the deferred one.
///
/// This doc used to say "no spell benilla meets carries it, so the deferred path stays unbuilt".
/// That was a proof by absence over the ten warrior/rogue spells `catalog_tests.rs` happened to
/// list. The real 5875 `Spell.dbc` carries the bit on **36 rows** — every rank of Backstab,
/// Garrote, Ambush, Cheap Shot, Shred, Ravage and Pounce, plus Judgement 20271 — i.e. every
/// stealth opener and positional strike in the game, whose attack must wait for the server to
/// resolve it. The census is the test beside this now (bug B280's sweep).
const ATTR_EX2_INITIATE_COMBAT_POST_CAST: u32 = 0x0010_0000;

/// `Attributes` bit 25 (`0x0200_0000`) — `SPELL_ATTR_COOLDOWN_ON_EVENT` (vmangos; "disabled while
/// active"). The client's cooldown machinery stores such a spell's record **on hold** — timers
/// parked until `SMSG_COOLDOWN_EVENT` starts them (the `bl = (SpellRec+0x18 >> 0x19) & 1` read in
/// the `SMSG_SPELL_COOLDOWN` handler `0x6e9460`). Stealth,
/// Shield Wall — the "cooldown begins when the effect ends" family.
const ATTR_COOLDOWN_ON_EVENT: u32 = 0x0200_0000;

/// The usable-walk attribute gates (names = vmangos `SpellDefines.h`,
/// bit positions = the reads in `IsSpellUsableNow 0x6e3d60`):
/// bit 23 — castable while dead (the dead/ghost gate's waiver, `0x6e3dbd`);
/// bit 17 — only while stealthed, tested against `UNIT_FIELD_BYTES_1` byte 3's CREEP flag
/// (`[+0x110]+0x213 & 2` at `0x6e3ee3`; vmangos `UNIT_VIS_FLAGS_CREEP`, set by the stealth aura);
/// bit 16 — not while shapeshifted (a form-gate input, [`SpellDisplay::usable_in_form`]);
/// bit 28 — only out of combat, tested against `UNIT_FLAG_IN_COMBAT` (unit-flag b19, `0x6e3f01`).
pub const ATTR_CASTABLE_WHILE_DEAD: u32 = 0x0080_0000;
pub const ATTR_ONLY_STEALTHED: u32 = 0x0002_0000;
const ATTR_NOT_SHAPESHIFT: u32 = 0x0001_0000;
pub const ATTR_NOT_IN_COMBAT: u32 = 0x1000_0000;
/// `AttributesEx2` bit 19 — the form requirement is waived while unshifted (vmangos
/// `SPELL_ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED`; a form-gate input).
const ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED: u32 = 0x0008_0000;
/// The **combo-point consumers** — `AttributesEx` bits 20 and 22 (vmangos
/// `SPELL_ATTR_EX_FINISHING_MOVE_DAMAGE` / `_DURATION`, both commented "Uses combo points"; the
/// pair `SpellEntry::NeedsComboPoints` tests). The usable walk's combo-point gate reads exactly
/// this pair (`0x6e3e7a`) before consulting the caster's combo-point byte —
/// [`SpellDisplay::needs_combo_points`]. In 5875 the set is the six rogue/druid
/// finishers (Eviscerate, Expose Armor, Ferocious Bite, Kidney Shot, Rip, Rupture, Slice and
/// Dice) **plus Overpower**, whose own `AttributesEx` bit 30 vmangos names `COMBO_ON_BLOCK` and
/// annotates, in one word, "Overpower".
const ATTR_EX_FINISHING_MOVE: u32 = 0x0050_0000;
/// `SpellEffects` value `47` — `SPELL_EFFECT_TRADE_SKILL`: the usable walk's early-out
/// (`0x6e3d99`, `Effect[0]==0x2f` ⇒ usable, skipping every gate). Also the crafting book's open
/// key: `Spell_C::TryCast 0x6e4b60` branches on `Effect[0]==0x2f` and opens the window
/// client-side, never sending the cast.
pub const SPELL_EFFECT_TRADE_SKILL: u32 = 47;
/// `SpellEffects` value `24` — `SPELL_EFFECT_CREATE_ITEM` (vmangos `SharedDefines.h`): a recipe's
/// product effect; its `EffectItemType` slot is the created item entry.
pub const SPELL_EFFECT_CREATE_ITEM: u32 = 24;
/// `SpellEffects` value `95` — `SPELL_EFFECT_SKINNING` (vmangos `SharedDefines.h`,
/// `Spell::EffectSkinning`): the corpse-gathering cast the Skin cursor's right-click resolves
/// through (0437's gathering finish). Byte-confirmed as the client's own discriminator: the
/// spell-learn path latches a spell with this `Effect[0]` into `[0xb700e4]`
/// (`0x4b2623: cmp [esi+0xf4], 0x5f` — `0x5f == 95`), and the cursor's skin leg requires that latch.
pub const SPELL_EFFECT_SKINNING: u32 = 95;
/// `SpellEffects` value `39` — `SPELL_EFFECT_LANGUAGE`: the effect that makes a spell *be* a
/// language. Its `EffectMiscValue_1` is the `Languages.dbc` id, which is how a language reaches a
/// skill line at all — see [`SpellCatalog::language_spell`].
pub const SPELL_EFFECT_LANGUAGE: u32 = 39;

/// `SpellEffects` values `53`/`54` — `SPELL_EFFECT_ENCHANT_ITEM` (permanent) /
/// `SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY` (vmangos `SharedDefines.h`): the item-targeted craft
/// casts the CraftFrame sends with `TARGET_FLAG_ITEM` (0437 phase 3).
pub const SPELL_EFFECT_ENCHANT_ITEM: u32 = 53;
pub const SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY: u32 = 54;

/// `SpellEffects` value `127` — `SPELL_EFFECT_PROSPECTING`, and **no 5875 spell carries it**
/// (pinned by `real_prospecting_effect_is_absent_from_5875`). It is named here because the
/// absence is load-bearing rather than incidental: the item-target validator `0x495d60` has a
/// third leg past its two enchant ones, gated on exactly this value (`0x495f39: cmp [eax],0x7f`),
/// and that leg holds the only raise sites image-wide for the cast-failure reasons `0x84`
/// PROSPECT_NEED_MORE (`0x49614e`) and `0x90` MIN_SKILL (`0x496128`). The server sends neither
/// reason either, so on this build both messages are unreachable — which is why
/// `ui_action::cast_fail` models neither arm.
///
/// **The number is the binary's own, not a later build's enum ported backwards.** `TryCast
/// 0x6e4b60` carries a matched pair of `Effect[0]` gates two instructions apart:
/// `0x6e4ca2 cmp eax,0x63` (99) raises `SPELL_FAILED_DISENCHANT_WHILE_LOOTING` (`0x6e4cbb`), and
/// `0x6e4cd4 cmp [edx+0xf4],0x7f` raises `SPELL_FAILED_PROSPECT_WHILE_LOOTING` (`0x6e4cee`). That
/// pairing names 99 ↔ disenchant and 127 ↔ prospect in 5875's own hand — and makes `0x83` dead
/// on the same grounds as `0x84`/`0x90`, while its sibling `0x82` stays live (effect 99 ships).
///
/// The **code** is live and only the **data** is absent: 127 is handled in three reachable places
/// (`0x495f39`, `0x6e4cd4`, and the effect classifier `0x6e3b80`'s 128-entry case table at
/// `0x6e3bdc`, which maps it to the default arm). Nothing here is dead-stripped — 5875 ships
/// prospecting with no prospecting spell to reach it.
pub const SPELL_EFFECT_PROSPECTING: u32 = 127;

/// `Spell.dbc` × `SpellIcon.dbc`, joined: spell id → name + icon path, plus the **learn-spell map**
/// (a `SPELL_EFFECT_LEARN_SPELL` spell → the spell it teaches). Vanilla trainers offer a *learn*
/// spell — a thin wrapper whose only effect is "teach spell X" — not the ability itself; the ability
/// (with its skill line, its real icon) is the taught spell. [`Self::learned_spell`] follows that hop
/// so callers can resolve the ability from the trainer's wire id (decision 0247's `taughtSpell`).
pub struct SpellCatalog {
    spells: HashMap<u32, SpellDisplay>,
    learned_spell: HashMap<u32, u32>,
    /// Spell id → its learn effects in slot order ([`SpellCatalog::learn_effects`]); only spells
    /// that carry at least one have an entry.
    learn_effects: HashMap<u32, Vec<LearnEffect>>,
    /// Spell id → the `Languages.dbc` id its `Effect_1` declares
    /// ([`SpellCatalog::declared_language`]).
    declared_language: HashMap<u32, u32>,
    dispel_types: SpellDispelTypes,
}

impl SpellCatalog {
    /// Build a catalog from an explicit display map — for tests and synthetic fixtures. The live
    /// path is [`load_spell_catalog`]. Carries no learn-spell map (synthetic ids teach nothing) and
    /// no dispel table, so [`Self::dispel_name`] answers `None` for everything.
    pub fn from_displays(spells: HashMap<u32, SpellDisplay>) -> Self {
        Self::from_displays_and_effects(spells, HashMap::new())
    }

    /// [`Self::from_displays`] with an explicit learn-effect table — for the trainer tests, whose
    /// synthetic wrappers need to teach something.
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

    /// A spell's learn effects in slot order — empty for a plain ability. What the trainer's
    /// state re-evaluator walks.
    pub fn learn_effects(&self, id: u32) -> &[LearnEffect] {
        self.learn_effects.get(&id).map_or(&[], Vec::as_slice)
    }

    pub fn get(&self, id: u32) -> Option<&SpellDisplay> {
        self.spells.get(&id)
    }

    /// The name of a spell's dispel class — the aura tooltip's right column and the `debuffType`
    /// the debuff border tints by. `None` when the spell has no class or the class is one
    /// `SpellDispelType.dbc`'s `[+0x28]` gate withholds (see [`dispel_types`]).
    pub fn dispel_name(&self, display: &SpellDisplay) -> Option<&str> {
        self.dispel_types.name(display.dispel)
    }

    /// Every loaded spell `(id, display)`, unordered — the corpus instruments' walk
    /// (`benilla-extract partcensus` attributes effect models back to the spells that play them).
    pub fn iter(&self) -> impl Iterator<Item = (u32, &SpellDisplay)> + '_ {
        self.spells.iter().map(|(id, s)| (*id, s))
    }

    /// The spell a `SPELL_EFFECT_LEARN_SPELL` spell teaches (`None` if `id` teaches nothing — i.e. it
    /// is a plain ability, not a learn wrapper). The trainer's wire id is a learn spell; the ability
    /// (its skill line, its display) is what this resolves to (decision 0247's taught-spell hop).
    pub fn learned_spell(&self, id: u32) -> Option<u32> {
        self.learned_spell.get(&id).copied()
    }

    /// The language a spell **declares** — its `Effect_1 == 39` (`SPELL_EFFECT_LANGUAGE`) slot's
    /// `EffectMiscValue_1`, or `None` for a spell that is not a language.
    ///
    /// This is the direction the reference works in, and the direction matters. `0x4b25b0` runs on
    /// **spell add** and stores `[0xb700ac][EffectMiscValue_1] = spellId`, so the client's
    /// language→spell table only ever holds languages *this character has learned*, and a later
    /// learn overwrites an earlier one on the same language id.
    /// Exposing spell→language lets the caller
    /// fold that table over its own known-spell set and get the reference's answer; exposing
    /// language→spell over the whole DBC would not, and the shipped data is why:
    ///
    /// **Five of the fourteen language spells declare language 7 (Common), not their own.** In
    /// 5875's `Spell.dbc`, 813 Thalassian, 814 Draconic, 815 Demon Tongue, 816 Titan and 817 Old
    /// Tongue all carry `EffectMiscValue_1 = 7`, and four of the five are named "(NYI)". Two
    /// consequences a re-implementation must not smooth over:
    ///
    /// - **Languages 8, 9, 10 and 12 are unreachable** — no shipped spell declares them, so the
    ///   client's table never gets an entry and Demonic / Titan / Thalassian / Kalimag are *always*
    ///   fully garbled for every character. That is correct 1.12.1 behaviour, not a gap.
    /// - **A warlock's Demon Tongue (815) overwrites Common's entry**, so their Common is gated on
    ///   their Demon Tongue skill. Unobservable — every language skill a character holds is 300 —
    ///   but it is the reference's behaviour and falls out of the fold for free.
    ///
    /// Language 11 (Draconic) *is* reachable, via 25674 "Lesser Draconic (Language)", the one spell
    /// that declares it correctly.
    pub fn declared_language(&self, spell: u32) -> Option<u32> {
        self.declared_language.get(&spell).copied()
    }

    /// Every `(spell id, language id)` pair a shipped spell declares — for the fold above, and for
    /// the tests that pin the shipped anomaly.
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

/// Schema: 173 fields, all u32 except the ones we read as what they are (Speed/EffectMultipleValue
/// as f32; the name/name-subtext/description/aura-description block heads as strings; the rest of
/// each loc block stays u32 — unread). `EffectBasePoints`/`EffectDieSides` are genuinely signed but
/// read through [`i32_at`] regardless of the schema tag (a raw dword's bits don't change between
/// the `UInt32`/`Int32` variants — only which `_at` helper a caller reaches for), so they stay
/// untagged here like every other unread-as-a-type integer column.
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
    // The SpellCategory "matches every query" wildcard set (`GetCooldownInfo 0x6e13e0`'s category
    // leg, `6e1563`/`6e1567`: a category row whose Flags carry bit `0x2` contributes to ANY
    // queried spell). In the 5875 data exactly one row carries
    // it: category 351, wand Shoot's — the whole-bar wand-swing sweep (pinned in catalog_tests).
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
        // The learn-spell hop: a SPELL_EFFECT_LEARN_SPELL effect's EffectTriggerSpell
        // is the ability this spell teaches — the trainer offers the learn wrapper, the taught spell
        // carries the skill line + the real display. First LEARN effect wins.
        for i in 0..3 {
            if u32_at(r, COL_EFFECT_1 + i) == Some(SPELL_EFFECT_LEARN_SPELL) {
                if let Some(taught) = u32_at(r, COL_EFFECT_TRIGGER_1 + i).filter(|&t| t != 0) {
                    learned_spell.entry(id).or_insert(taught);
                    break;
                }
            }
        }
        // The learn effects in slot order, for the trainer's state re-evaluator (2333). A
        // SKILL_STEP's step is the effect's value: base points + die sides (see `LearnEffect`).
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
        // The language declaration (`0x4b2656`). **Effect slot 0 only**
        // — the reference dispatches on `Effect_1` alone (`[SpellRec+0xf4]`) and reads
        // `EffectMiscValue_1` (`+0x1a8`); it does not scan the other two slots the way the learn
        // hop above does.
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
        // The per-effect `[3]` arrays (module docs) — computed once so the shapeshift-form
        // derivation below can reuse `effect_apply_aura` instead of re-reading the columns.
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
                // Low dword first: bit `i >= 32` lives in column 162, which the reference reads as
                // `[edi + 4*(i>>5) + 0x284]` — the same little-endian pair this join makes.
                spell_family_flags: u64::from(u32_at(r, COL_SPELL_FAMILY_FLAGS_LOW).unwrap_or(0))
                    | u64::from(u32_at(r, COL_SPELL_FAMILY_FLAGS_LOW + 1).unwrap_or(0)) << 32,
                passive: attributes & ATTR_PASSIVE != 0,
                cast_ui: u32_at(r, COL_CAST_UI).unwrap_or(0),
                effects: [0, 1, 2].map(|i| u32_at(r, COL_EFFECT_1 + i).unwrap_or(0)),
                base_level: u32_at(r, COL_BASE_LEVEL).unwrap_or(0),
                max_level: u32_at(r, COL_MAX_LEVEL).unwrap_or(0),
                spell_level: u32_at(r, COL_SPELL_LEVEL).unwrap_or(0),
                // Scan the three effects for OPEN_LOCK and take that effect's LockType (its
                // EffectMiscValue) together with the value inputs the lock resolver compares
                // against the slot's requirement. Most openers carry it on
                // Effect[0], but the scan is cheap.
                open_lock: (0..3).find_map(|i| {
                    (u32_at(r, COL_EFFECT_1 + i)? == SPELL_EFFECT_OPEN_LOCK).then(|| OpenLock {
                        lock_type: u32_at(r, COL_EFFECT_MISC_1 + i).unwrap_or(0),
                        effect: i,
                    })
                }),
                dispel: u32_at(r, COL_DISPEL).unwrap_or(0),
                category: u32_at(r, COL_CATEGORY).unwrap_or(0),
                // The category row's flags-bit-0x2 "matches every query" mark (only wand Shoot's
                // 351 in the 5875 data) — resolved at load so the cooldown store's category leg
                // reads it off the record it armed (`0x6e1563`).
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
                // The stance-bar keys: the first MOD_SHAPESHIFT effect's MiscValue is the form id
                // (`0x4b4690`); order/active-icon read raw.
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

    /// The profession openers on the real build-5875 file: the WIRE spell (the wrapper a trainer
    /// lists) carries both the learn effect and the skill step, and the step is base points + die
    /// sides. What `0x4d7d40` compares the player's step word against (2333).
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
