//! The character-window stats + equipment seam (decision 0208 §3) — the paper doll's data feed.
//!
//! Same engine-free shape as [`super::unit`]: the app pushes a **combat-stats snapshot** per unit
//! that has one ([`super::UiScript::set_player_combat_stats`] /
//! [`super::UiScript::set_pet_combat_stats`]) and an **inventory-slot snapshot**
//! ([`super::UiScript::set_inventory_slots`]) each frame they change, and the stat/slot globals
//! here read that plain data.
//!
//! **The stat family is unit-parameterised, as the reference's is** (decision 1057): every
//! `PaperDollFrame_Set*(unit, prefix)` helper the character sheet calls with `"player"`, the pet
//! sheet calls with `"pet"` (ref `PetPaperDollFrame.lua:73-81`), through these very bindings. So
//! `UnitStat`/`UnitResistance`/`UnitArmor`/… route on the token — `"player"` and `"pet"` each read
//! their own pushed snapshot, and **every other token (and a snapshot the app has not pushed yet)
//! serves the absent shape**: zeros, `percent` 1.0. A pet's descriptor carries the UNIT half and no
//! PLAYER block at all, which is why its buff decompositions read `0` and the ref's own pet sheet
//! shows plain white numbers.
//!
//! (Until 1057 the router was a hard `token == "player"` test documented as "the faithful
//! player-only gate". It was neither: it was "no consumer yet". The reference passes `"pet"` into
//! the same bindings, and the gate was simply never exercised.)
//!
//! **The absent shape for a third unit is the right ANSWER for `UnitStat` and a known GAP for the
//! resistance pair — and the reason it used to be filed as blanket-faithful was wrong.** The binary
//! does not gate slots 1/2 on SELF at all: `UnitStat`'s are NULL + typemask bit 3 only, and it
//! returns whatever the client's copy of `UNIT_FIELD_STAT0+i` holds (`UnitStat 0x518600`). What
//! makes our zeros agree there is the *server's* visibility, not a client gate: `UNIT_FIELD_STAT*`
//! is PRIVATE + OWNER_ONLY, and the only owner-visible units a 1.12 unit token can name are the two
//! we already serve — so a stranger's copy is zero on the reference too. `UNIT_FIELD_RESISTANCES`
//! carries a third flag, **`SPECIAL_INFO`**, which vmangos grants to the caster of
//! `SPELL_AURA_EMPATHY` — its own comment reads `// Beast Lore` (`Player.cpp:2603-2610`,
//! `Object.cpp:1065-1067`). So with Beast Lore up on a beast, the reference's
//! `UnitResistance("target", i)` / `UnitArmor("target")` return that creature's real numbers
//! through their non-SELF leg, and ours return zeros. That is a real, reachable divergence, unfed
//! rather than decided: the app pushes snapshots for two tokens only.
//! `unit_combat_stats` already works over any store, so the missing piece is a third push.
//!
//! **Return shapes differ BY FAMILY, and the reference Lua's own asymmetry is the tell**
//! (decision 1397 — reading one and assuming the other is how 0208 got the stat row wrong).
//! `UnitStat` serves the **raw** `UNIT_FIELD_STAT` twice — once as-is, once clamped at zero — and
//! leaves the subtraction to `PaperDollFrame_SetStats`, which writes `(stat - posBuff - negBuff)`
//! itself. `UnitResistance`/`UnitArmor` serve a **decomposed** first return, because the engine
//! helper `0x5efcd0` does that subtraction for them; their callers use it directly.
//!
//! The negative deltas are **negative-or-zero** where the wire can carry them at all, matching the
//! ref's `negBuff < 0` tests — but see 1397 for how little of that survives a given server: these
//! four arrays are stored float and narrowed to int by `Object::BuildValuesUpdate`, and that cast
//! saturates a negative to `0` on an arm64 host while wrapping it correctly on x86.

use mlua::{Lua, Value};

use super::binding_abi::flag;
use super::{binding_abi, Model};

/// The 1.12 weapon-subclass → `SkillLine.dbc` id table, transcribed from vmangos
/// `ItemPrototype::GetProficiencySkill`'s `item_weapon_skills` (`Objects/Item.cpp:700-707`;
/// subclass ids `Objects/ItemPrototype.h:190-213`, skill ids `SharedDefines.h:951-1038`) — item
/// class 2 (weapon) subclasses `0..=20`. `None` = no proficiency skill backs the subclass (the
/// obsolete/exotic/misc rows) or out of range. The app resolves the paper doll's weapon-skill
/// line ("Both Hands"/"Ranged") through this, falling back to [`SKILL_UNARMED`] with no weapon
/// (vmangos `Player::GetBaseWeaponSkillValue`, `Objects/Player.cpp:20175-20186`).
pub fn weapon_subclass_skill(subclass: u32) -> Option<u32> {
    const TABLE: [u32; 21] = [
        44,  // 0 axe → SKILL_AXES
        172, // 1 two-hand axe → SKILL_2H_AXES
        45,  // 2 bow → SKILL_BOWS
        46,  // 3 gun → SKILL_GUNS
        54,  // 4 mace → SKILL_MACES
        160, // 5 two-hand mace → SKILL_2H_MACES
        229, // 6 polearm → SKILL_POLEARMS
        43,  // 7 sword → SKILL_SWORDS
        55,  // 8 two-hand sword → SKILL_2H_SWORDS
        0,   // 9 obsolete
        136, // 10 staff → SKILL_STAVES
        0,   // 11 exotic
        0,   // 12 exotic2
        162, // 13 fist weapon → SKILL_UNARMED (vmangos's own row — fists ride the unarmed skill)
        0,   // 14 misc
        173, // 15 dagger → SKILL_DAGGERS
        176, // 16 thrown → SKILL_THROWN
        253, // 17 spear → SKILL_ASSASSINATION (vmangos's own row)
        226, // 18 crossbow → SKILL_CROSSBOWS
        228, // 19 wand → SKILL_WANDS
        356, // 20 fishing pole → SKILL_FISHING
    ];
    TABLE.get(subclass as usize).copied().filter(|&s| s != 0)
}

/// `SKILL_UNARMED` (162, vmangos `SharedDefines.h:987`) — the melee weapon-skill line's fallback
/// when no weapon is equipped (`Player::GetBaseWeaponSkillValue`, `Objects/Player.cpp:20184`).
pub const SKILL_UNARMED: u32 = 162;

/// `SKILL_DEFENSE` — the Defense skill line `UnitDefense` reports. **Read out of the shipped
/// 1.12.1 `SkillLine.dbc` this session**, not remembered: the file's 123 records carry exactly one
/// row whose enUS `displayName` (column 3, the `SkillLinefmt` layout in
/// `benilla_formats::skill_lines`) is `"Defense"`, and it is id **95**, category 6 (Weapon Skills).
/// Corroborated by vmangos `SharedDefines.h:961` (`SKILL_DEFENSE = 95`).
pub const SKILL_DEFENSE: u32 = 95;

/// One unit's combat-stats snapshot behind a paper doll's stat pane — plain data the app derives
/// from that unit's descriptor accessors ([`UnitStat`]/[`UnitResistance`]/… read it). Arrays are
/// in field order: stats 0..4 = Str/Agi/Stam/Int/Spi (`UNIT_FIELD_STAT0..4`), schools 0..6 with
/// `[0]` = armor/physical. The `*_neg` values are **negative-or-zero** (the stored wire sign; see
/// the module doc).
///
/// Two units carry one: the player and (decision 1057) the pet. A **creature's** descriptor has no
/// PLAYER block, so every field sourced from one — the stat/resistance buff splits, the
/// damage-done mods, the skill pairs — keeps its default for a pet, which is exactly the ref pet
/// sheet's plain white numbers with no buff decomposition in the tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitCombatStats {
    /// Effective (post-buff) primary stats (`UNIT_FIELD_STAT0..4`).
    pub stats: [i32; 5],
    /// Positive stat buff deltas (`PLAYER_FIELD_POSSTAT0..4`, ≥ 0).
    pub stat_pos: [i32; 5],
    /// Negative stat buff deltas (`PLAYER_FIELD_NEGSTAT0..4`, ≤ 0).
    pub stat_neg: [i32; 5],
    /// Effective resistances (`UNIT_FIELD_RESISTANCES`, `[0]` = armor).
    pub resistances: [i32; 7],
    /// Positive resistance buffs (`PLAYER_FIELD_RESISTANCEBUFFMODSPOSITIVE`, ≥ 0).
    pub resistance_pos: [i32; 7],
    /// Negative resistance buffs (`PLAYER_FIELD_RESISTANCEBUFFMODSNEGATIVE`, ≤ 0).
    pub resistance_neg: [i32; 7],
    /// Mainhand damage range (`UNIT_FIELD_MINDAMAGE`/`MAXDAMAGE`).
    pub min_damage: f32,
    pub max_damage: f32,
    /// Offhand damage range (`UNIT_FIELD_MINOFFHANDDAMAGE`/`MAXOFFHANDDAMAGE`).
    pub min_offhand_damage: f32,
    pub max_offhand_damage: f32,
    /// Physical damage-done bonuses (`PLAYER_FIELD_MOD_DAMAGE_DONE_POS[0]` / `_NEG[0]`, neg ≤ 0).
    pub physical_bonus_pos: i32,
    pub physical_bonus_neg: i32,
    /// The physical damage-done multiplier (`PLAYER_FIELD_MOD_DAMAGE_DONE_PCT[0]`, a true float —
    /// the app fills `1.0` while the field hasn't streamed; the default here is `1.0` too).
    pub damage_percent: f32,
    /// Attack speeds in ms (`UNIT_FIELD_BASEATTACKTIME[0..2]`).
    pub main_attack_time_ms: u32,
    pub offhand_attack_time_ms: u32,
    /// Whether an offhand **weapon** is equipped — gates `UnitAttackSpeed`'s second return (the
    /// app decides from the inv slots; the offhand fields stream regardless).
    pub has_offhand: bool,
    /// Melee AP + its split mods (`UNIT_FIELD_ATTACK_POWER` / `_MODS`, neg ≤ 0).
    pub attack_power: i32,
    pub attack_power_pos: i32,
    pub attack_power_neg: i32,
    /// Ranged AP + its split mods.
    pub ranged_attack_power: i32,
    pub ranged_attack_power_pos: i32,
    pub ranged_attack_power_neg: i32,
    /// Ranged attack speed in ms (`UNIT_FIELD_RANGEDATTACKTIME`).
    pub ranged_attack_time_ms: u32,
    /// Ranged damage range (`UNIT_FIELD_MINRANGEDDAMAGE`/`MAXRANGEDDAMAGE`).
    pub ranged_min_damage: f32,
    pub ranged_max_damage: f32,
    /// The equipped main-hand weapon's skill line as **`(value + PERM bonus, TEMP bonus)`** — the
    /// app resolves WHICH skill via [`weapon_subclass_skill`] (unarmed = [`SKILL_UNARMED`]) and
    /// reads the pair from `PLAYER_SKILL_INFO`; `UnitAttackBothHands` serves it verbatim.
    ///
    /// **The permanent half belongs to the BASE, not to the modifier** (`0x5ea460`; corrected here
    /// by decision 1812). Every one of these four pairs comes out of that one reader, so all four
    /// carry the split.
    pub main_weapon_skill: (i32, i32),
    /// The OFF hand's, read the same way. `UnitAttackBothHands` pushes both hands (`0x518810`
    /// calls `[vtbl+0xb0]` twice, hand 0 then hand 1), and an empty or non-weapon off hand is
    /// Unarmed exactly as the main hand is.
    pub offhand_weapon_skill: (i32, i32),
    /// The ranged weapon's skill pair (`UnitRangedAttack`), same split.
    ///
    /// **Player-only, and by a different gate than its two neighbours.** `UnitRangedAttack`
    /// `0x518b90` is a DIRECT call gated on the PLAYER typemask alone — no vtable slot and **no
    /// `CGUnit_C` fallback body**. So where
    /// `UnitDefense` and `UnitAttackBothHands` pass a pet through SELF-OR-MINE and answer
    /// `level * 5`, this one answers `(0, 0)` for a pet and for every other player too. Mirroring
    /// [`cgunit_skill`] here would be wrong.
    pub ranged_weapon_skill: (i32, i32),
    /// The [`SKILL_DEFENSE`] line's pair, in the same `(value + perm, temp)` split as the three
    /// above and out of the same reader; `UnitDefense` serves it verbatim.
    ///
    /// **The player's only.** A creature has no skill block, and the client does not read one for
    /// it: `UnitDefense` forks on the resolved unit's vtable and gives a non-player
    /// `UNIT_FIELD_LEVEL * 5` instead ([`cgunit_skill`]), so the pet feed never fills this and the
    /// binding never reads it for a pet.
    pub defense_skill: (i32, i32),
    /// Whether a wand is equipped (`HasWandEquipped` — the ref swaps the ranged-attack action for
    /// wand Shoot on it).
    pub has_wand: bool,
    /// `GetDodgeChance()` / `GetParryChance()` / `GetBlockChance()` — the player's avoidance
    /// percentages (`PLAYER_DODGE_PERCENTAGE` and its two siblings), already a percent on the
    /// wire: `2.62` is "2.62%".
    ///
    /// **The player's only, and no unit argument.** All three bindings take zero arguments and
    /// return exactly one number (`reference/1.12-shapes.tsv`: `GetDodgeChance 0x516f00`,
    /// `GetBlockChance 0x516f60`, `GetParryChance 0x516fc0` — argc 0, arity 1, kind number, all
    /// `exact`), so there is no pet leg to mirror and no absent answer to model: a field that has
    /// not streamed reads 0, which is what the wire's own default is.
    ///
    /// The shipped 1.12 FrameXML never calls them — the character sheet's own defense block is
    /// `UnitDefense` — so this trio is addon-facing surface, and the corpus asks for it: four
    /// vanilla addons read `GetDodgeChance` (FuBar_TankPointsFu, FuBar_DakSmak, Outfitter,
    /// BetterCharacterStats) and three read each of the other two.
    pub dodge_percent: f32,
    pub parry_percent: f32,
    pub block_percent: f32,
}

impl Default for UnitCombatStats {
    /// All-zeros except `damage_percent` = `1.0` — the multiplicative identity, so the absent
    /// shape never feeds the ref's `bonus / percent` math a division by zero.
    fn default() -> Self {
        UnitCombatStats {
            stats: [0; 5],
            stat_pos: [0; 5],
            stat_neg: [0; 5],
            resistances: [0; 7],
            resistance_pos: [0; 7],
            resistance_neg: [0; 7],
            min_damage: 0.0,
            max_damage: 0.0,
            min_offhand_damage: 0.0,
            max_offhand_damage: 0.0,
            physical_bonus_pos: 0,
            physical_bonus_neg: 0,
            damage_percent: 1.0,
            main_attack_time_ms: 0,
            offhand_attack_time_ms: 0,
            has_offhand: false,
            attack_power: 0,
            attack_power_pos: 0,
            attack_power_neg: 0,
            ranged_attack_power: 0,
            ranged_attack_power_pos: 0,
            ranged_attack_power_neg: 0,
            ranged_attack_time_ms: 0,
            ranged_min_damage: 0.0,
            ranged_max_damage: 0.0,
            main_weapon_skill: (0, 0),
            offhand_weapon_skill: (0, 0),
            ranged_weapon_skill: (0, 0),
            defense_skill: (0, 0),
            has_wand: false,
            dodge_percent: 0.0,
            parry_percent: 0.0,
            block_percent: 0.0,
        }
    }
}

/// One resolved equipment/ammo slot view (the `GetInventoryItem*` family reads it), resolved by
/// the app like [`super::container::ContainerSlot`]: icon from the display catalog, count/quality
/// from the item object + template. Plain data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InvSlotView {
    /// The item's template entry (`GetInventoryItemID`).
    pub item_id: u32,
    /// The doll twin of [`super::container::ContainerSlot::bar_placeable`] — an equipped item
    /// dragged straight to the bar goes through the same filter.
    pub bar_placeable: bool,
    /// Icon texture path (`Interface\Icons\…`); `None` while the template answer is in flight.
    pub icon: Option<String>,
    /// `ITEM_FIELD_STACK_COUNT` — what `GetInventoryItemCount 0x4c8680` pushes for an ordinary
    /// item (1 for equipment; the ammo slot's own leg puts the carried total here).
    pub count: u32,
    /// What that same function pushes when the item is a **CONTAINER** (`OBJECT_FIELD_TYPE`'s
    /// `TYPEMASK_CONTAINER` bit — `0x4c87a6`), which is a different number entirely: the sum of
    /// its contents' stack counts when its `(class, subclass)` row carries `ItemSubClass.dbc`'s
    /// `DisplayFlags & 0x4` (`0x4c881a`, set on Soul Bag and the quivers only), and **0**
    /// otherwise. `None` = not a container, and [`Self::count`] answers.
    ///
    /// The slot-dependent half of the law is NOT here — it is at the binding, where the reference
    /// keeps it: a container whose 0-based slot is past `0x16` (everything but the four equipped
    /// bag slots — the bank's six bag slots included) short-circuits to 0 at `0x4c87af` before any
    /// of this is read.
    pub contents_count: Option<u32>,
    /// Item quality 0..6 (`GetInventoryItemQuality`).
    pub quality: i32,
    /// The item's name, when known (tooltip/link consumers).
    pub name: Option<String>,
    /// The instance's live durability `(current, max)` — the equipped-item tooltip's
    /// "Durability X / Y" line (see [`super::container::ContainerSlot::durability`]).
    pub durability: Option<(u32, u32)>,
    /// `ITEM_FIELD_FLAGS` (wire field 21) — the alert/broken laws read two bits (`0x4c7ee0`):
    /// `0x08` wrapped (a gift — never alerts, never broken),
    /// `0x10` force-red (alert status 4 regardless of durability).
    pub flags: u32,
    /// `0x5da2c0` — **the instance is runtime-bound**: `ITEM_FIELD_FLAGS & 1` (soulbound), or a
    /// live enchant slot naming a `SpellItemEnchantment` row that binds. App-resolved off the raw
    /// descriptor (the doll twin of [`super::container::ContainerSlot::already_bound`]); the
    /// tooltip's bind line overrides to **Soulbound** on it (B310).
    pub already_bound: bool,
    /// An `|Hitem:…|h[Name]|h` link once the name is known — the doll twin of
    /// `ContainerSlot::link`; carried onto the cursor payload so a world-drop `DELETE_ITEM_CONFIRM`
    /// off an equipped item can report its name (decision 0208 phase 1b).
    pub link: Option<String>,
    /// Whether an outstanding pending op covers `(EQUIPMENT_BAG, this slot)` — the app's
    /// `PendingItemOps` feed (decision 0216 §4/0218 §3), fed by `ui_char.rs::feed_char` the same
    /// way `ContainerSlot::locked` is. `IsInventoryItemLocked` ORs this with the payload-held-here
    /// check.
    pub locked: bool,
    /// The 1-based live-API inventory slot ids this item could be EQUIPPED into (empty = not
    /// equippable; an equipped ring's own two finger slots, say) — decision 0208 phase 1b's "the
    /// fit rule", resolved app-side via `ui_items::find_equip_slot`.
    pub equip_slots: Vec<u8>,
    /// The RESOLVED `ITEM_FIELD_CREATOR` name — the equipped-item tooltip's green
    /// "<Made by %s>" line (see [`super::container::ContainerSlot::creator`], the bag twin).
    pub creator: Option<String>,
    /// The instance's resolved enchant slots, in slot order — the doll twin of
    /// [`super::container::ContainerSlot::enchants`] (decisions 0915/0920). Our own equipped item
    /// reads all 7 slots off its streamed item object; an INSPECTED player's record carries 7 too
    /// and the reference renders all 7 from it — but a 1.12 server fills only PERM and TEMP, so in
    /// practice that is what an inspect hover shows.
    pub enchants: Vec<super::EnchantView>,
    /// **The instance's remaining LIFETIME in milliseconds** — the doll twin of
    /// [`super::container::ContainerSlot::duration_ms`]. `None` = no timer. In practice an
    /// equipped duration item is rare (the holiday masks are the shipped case), but the line law
    /// is one law and the doll hover runs the same builder. Decision 1933.
    pub duration_ms: Option<u64>,
}

/// The inventory-slot snapshot: index 0 = ammo, 1..=19 the equipment slots, 20..=23 the four
/// equipped-bag icons (all the client's `GetInventorySlotInfo` slot ids — decision 0216 slice 2's
/// bag bar). `None` = an empty slot.
pub const INVENTORY_SLOT_COUNT: usize = 24;
pub type InventorySlots = [Option<InvSlotView>; INVENTORY_SLOT_COUNT];

/// The six BANK-BAG slots' snapshot, in button order (live-API ids 64..=69 —
/// `BankButtonIDToInvSlotID(1..6, isBag)`). `None` = the slot is empty (or not purchased).
///
/// **A separate dense array rather than a wider [`InventorySlots`]**, because the live-id space
/// between them is not ours to fill: 24..=39 is the backpack and 40..=63 the vault, and both of
/// those the model already holds as CONTAINERS — [`Model::inv_slot`]'s bank band maps them rather
/// than storing them twice. Growing the doll array to 70 would leave a 40-entry hole whose
/// emptiness means "look somewhere else", which is exactly the kind of implicit second rule that
/// lets a tooltip disagree with the icon under it. Three bands, three named sources.
///
/// Fed by the same system as the doll snapshot ([`super::UiScript::set_bank_bag_slots`]): these
/// are player-descriptor inventory slots that exist whether or not the bank window is open, not
/// part of the bank *window*'s state.
pub const BANK_BAG_SLOT_COUNT: usize = 6;
pub type BankBagSlots = [Option<InvSlotView>; BANK_BAG_SLOT_COUNT];

/// The client's own `PaperDollItemFrame.dbc` rows (re-verified against the 1.12.1 (build 5875)
/// MPQ this session, byte-exact — 36 records, 3 `u32` fields each: `SlotName` offset,
/// `SlotTexture` offset, `SlotID`): `(slotName, slotId, empty-slot art suffix)` for all 36 rows —
/// the 20 equipment+ammo rows, `Bag0Slot`..`Bag3Slot` (ids **20..23**, every one pointing at the
/// same `interface\paperdoll\UI-PaperDoll-Slot-Bag.blp`, matching the reference
/// `CharacterBag0Slot` bag-bar buttons' `GetID()`s), and `Bag1`..`Bag12` (ids 64..75 — the
/// bank-bag band; see the note over those rows). The oddballs: `BackSlot` shows the **Chest** art
/// and `AmmoSlot` the **Ranged** art (both confirmed by their `SlotTexture` offset pointing at
/// that other row's string, not a fresh one).
const SLOT_INFO: [(&str, i64, &str); 36] = [
    ("AmmoSlot", 0, "Ranged"),
    ("HeadSlot", 1, "Head"),
    ("NeckSlot", 2, "Neck"),
    ("ShoulderSlot", 3, "Shoulder"),
    ("ShirtSlot", 4, "Shirt"),
    ("ChestSlot", 5, "Chest"),
    ("WaistSlot", 6, "Waist"),
    ("LegsSlot", 7, "Legs"),
    ("FeetSlot", 8, "Feet"),
    ("WristSlot", 9, "Wrists"),
    ("HandsSlot", 10, "Hands"),
    ("Finger0Slot", 11, "Finger"),
    ("Finger1Slot", 12, "Finger"),
    ("Trinket0Slot", 13, "Trinket"),
    ("Trinket1Slot", 14, "Trinket"),
    ("BackSlot", 15, "Chest"),
    ("MainHandSlot", 16, "MainHand"),
    ("SecondaryHandSlot", 17, "SecondaryHand"),
    ("RangedSlot", 18, "Ranged"),
    ("TabardSlot", 19, "Tabard"),
    ("Bag0Slot", 20, "Bag"),
    ("Bag1Slot", 21, "Bag"),
    ("Bag2Slot", 22, "Bag"),
    ("Bag3Slot", 23, "Bag"),
    // The twelve rows this table was short of. The shipped DBC has **36 records, not 24**, and
    // `Bag1`..`Bag12` at SlotNumbers 64..75 are in the very table this binding scans — 64..69 is
    // exactly the bank-bag band `ContainerIDToInventoryID` produces. This file used to call them
    // "a different, unrelated numbering — not `GetInventorySlotInfo` names, out of scope"; that is
    // refuted, and the scan reaches them like any other row.
    //
    // All twelve share ONE string-block offset with `Bag0Slot`..`Bag3Slot` — sixteen rows, one
    // string (`+1001`). Read at the offset rather than resolved from the row name, because that is
    // how this column works: 36 rows carry only 17 distinct offsets, which is why `BackSlot` shows
    // the Chest art and `AmmoSlot` the Ranged art rather than art of their own.
    ("Bag1", 64, "Bag"),
    ("Bag2", 65, "Bag"),
    ("Bag3", 66, "Bag"),
    ("Bag4", 67, "Bag"),
    ("Bag5", 68, "Bag"),
    ("Bag6", 69, "Bag"),
    ("Bag7", 70, "Bag"),
    ("Bag8", 71, "Bag"),
    ("Bag9", 72, "Bag"),
    ("Bag10", 73, "Bag"),
    ("Bag11", 74, "Bag"),
    ("Bag12", 75, "Bag"),
];

/// The durability-alert regions in the client's own slot table (`0x806eb8` — 12 entries): alert
/// index 1..=11 → the live-id equipment slot (Head,
/// Shoulders, Chest, Waist, Legs, Feet, Wrists, Hands, Weapon, Shield, Ranged — the ref
/// FrameXML's `INVENTORY_ALERT_STATUS_SLOTS` order), index 12 → the client's low-ammo region
/// (slot -1 in its table; FrameXML never reads it, the binding answers it). Our live-id 0 IS
/// the ammo view, so the 12th entry maps there.
const ALERT_SLOTS: [usize; 12] = [1, 3, 5, 6, 7, 8, 9, 10, 16, 17, 18, 0];

/// The broken classification the doll tint (`GetInventoryItemBroken`) and alert status 4 share
/// (the recompute `0x4c7ee0`, byte-verified): a wrapped item (`flags & 0x08`) is never broken;
/// `flags & 0x10` forces broken regardless of durability; else broken ⇔ tracks durability
/// (max > 0) at 0.
fn slot_is_broken(v: &InvSlotView) -> bool {
    if v.item_id == 0 || v.flags & 0x08 != 0 {
        return false;
    }
    v.flags & 0x10 != 0 || matches!(v.durability, Some((0, max)) if max > 0)
}

/// One region's `GetInventoryAlertStatus` value — the recompute `0x4c7ee0`'s per-item
/// classification (the enum's own name is `INV_ALERT_STATUS`): `4` (red) = broken per
/// [`slot_is_broken`]; `3` (yellow) = tracks
/// durability and **`1..=5` points left — an ABSOLUTE count, no percentage, maxDurability is
/// only the tracks-durability gate** (`cmp [D+0xa0],5`); `0` otherwise. Statuses 1/2 are the
/// temp-weapon-enchant alerts (present/expiring-in-30s) — no enchant feed yet, and the 1.12
/// FrameXML disables their colors anyway; a named deferral.
fn alert_status(slot: &Option<InvSlotView>) -> u8 {
    let Some(v) = slot.as_ref().filter(|v| v.item_id != 0) else {
        return 0;
    };
    if v.flags & 0x10 != 0 {
        return 4;
    }
    if v.flags & 0x08 != 0 {
        return 0;
    }
    match v.durability {
        Some((0, max)) if max > 0 => 4,
        Some((1..=5, max)) if max > 0 => 3,
        _ => 0,
    }
}

/// The 12th region's low-ammo status (`0x4c7ee0`'s slot −1 arm): an equipped ammo item whose
/// carried count is `<= 20` reads `3`; no ammo (or plenty) reads `0`. The ammo view's count is
/// already the bag-summed carried total.
fn ammo_alert_status(slot: &Option<InvSlotView>) -> u8 {
    match slot.as_ref().filter(|v| v.item_id != 0) {
        Some(v) if v.count <= 20 => 3,
        _ => 0,
    }
}

impl super::UiScript {
    /// Push (or clear, with `None`) the player's combat-stats snapshot — the app calls this each
    /// frame any of the backing descriptor fields changed (decision 0208 §3).
    pub fn set_player_combat_stats(&mut self, stats: Option<UnitCombatStats>) {
        self.model_mut().player_combat_stats = stats;
    }

    /// The `"pet"` twin (decision 1057) — `None` whenever there is no pet, which is what makes
    /// every `Unit*("pet")` fall back to the absent shape the moment one is dismissed.
    pub fn set_pet_combat_stats(&mut self, stats: Option<UnitCombatStats>) {
        self.model_mut().pet_combat_stats = stats;
    }

    /// Push the equipment/ammo slot snapshot (index 0 = ammo, 1..=19 equipment). Recomputes the
    /// 11 durability-alert region statuses off the live pairs; a change fires
    /// `UPDATE_INVENTORY_ALERTS` — the DurabilityFrame's own repaint signal (the real engine
    /// computes these statuses itself and fires the same event).
    pub fn set_inventory_slots(&mut self, slots: InventorySlots) {
        {
            let mut model = self.model_mut();
            let alerts: [u8; 12] = std::array::from_fn(|i| {
                if i == 11 {
                    ammo_alert_status(&slots[ALERT_SLOTS[i]])
                } else {
                    alert_status(&slots[ALERT_SLOTS[i]])
                }
            });
            model.inventory_slots = slots;
            model.inventory_alerts = alerts;
        }
        // Unconditionally, the client's own shape (VERIFIED `0x4c7ee0`: the event fires at the
        // tail of EVERY recompute, never diffed against prior state) — SetAlerts re-derives the
        // whole frame from the statuses each time, so glyph choices (the off-hand
        // shield/off-weapon swap) can never go stale. The app pushes only on real slot-view
        // change, so this stays quiet between changes.
        self.fire_event("UPDATE_INVENTORY_ALERTS", vec![]);
    }

    /// Push the six bank-bag slots ([`BankBagSlots`] — live ids 64..=69).
    ///
    /// **The repaint signal is `PLAYERBANKSLOTS_CHANGED`, and nothing else will do** (1771). The
    /// reference's bank buttons register exactly `BANKFRAME_OPENED`, `PLAYERBANKSLOTS_CHANGED`,
    /// `ITEM_LOCK_CHANGED` and `CURSOR_UPDATE` (`BankFrameBaseButton_OnLoad`); of those, only the
    /// first two reach `BankFrameItemButton_OnUpdate`, the icon/count/lock repaint. That function
    /// is NOT an `OnUpdate` handler despite the name — `BankItemButtonTemplate`'s `<OnUpdate>` is
    /// `CursorOnUpdate()`, and the repaint is only ever reached from
    /// `BankFrameItemButton_OnEvent`. `UNIT_INVENTORY_CHANGED`, which rides
    /// [`Self::set_inventory_slots`] beside this push, is not registered by these buttons at all.
    /// So `ui_char::feed_char` fires `PLAYERBANKSLOTS_CHANGED` for every bag slot whose ITEM
    /// changed, and leaves the lock to `ITEM_LOCK_CHANGED`.
    pub fn set_bank_bag_slots(&mut self, slots: BankBagSlots) {
        self.model_mut().bank_bag_slots = slots;
    }

    /// Drain the inventory-slot ids `UseInventoryItem` queued (decision 0208 phase 1b) — the app
    /// resolves each to the equipped item's guid and sends `CMSG_USE_ITEM` (bag 255 + the
    /// 0-based wire slot).
    pub fn take_inventory_uses(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().inventory_uses)
    }
}

/// Read a unit's combat-stats snapshot under a short model borrow, routed by token: `"player"` and
/// `"pet"` each read their own pushed snapshot, anything else — and a snapshot the app has not
/// pushed yet — reads the default (the absent shape: zeros, `damage_percent` 1.0). See the module
/// doc for why exactly these two tokens and no more.
fn with_unit_stats<T>(
    lua: &Lua,
    token: &Option<String>,
    f: impl FnOnce(&UnitCombatStats) -> T,
) -> T {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let absent = UnitCombatStats::default();
    let pushed = match token.as_deref() {
        Some("player") => model.player_combat_stats.as_ref(),
        Some("pet") => model.pet_combat_stats.as_ref(),
        _ => None,
    };
    f(pushed.unwrap_or(&absent))
}

/// The post-processing both skill-shaped bindings share: **`if (mod + base) < 0 then mod = -base`**
/// — so the pair can never sum below zero (`0x519298` for `UnitDefense`, `0x5188a7` per hand for
/// `UnitAttackBothHands`). A debuff deeper than
/// the skill itself reads as "reduced to 0", never as a negative total.
fn skill_clamped((base, modifier): (i32, i32)) -> (i64, i64) {
    let base = i64::from(base);
    let modifier = i64::from(modifier);
    (base, if modifier + base < 0 { -base } else { modifier })
}

/// A **non-player** unit's answer to both skill-shaped questions — `UnitDefense` and
/// `UnitAttackBothHands` — which is `UNIT_FIELD_LEVEL * 5`, flat.
///
/// It is one helper because it is one function in the client: both bindings dispatch through the
/// resolved unit's vtable, and a `CGUnit_C` lands on a three-line body that multiplies the level by
/// five and writes 0 to the modifier (`0x613680` / `0x6136b0`). Nothing here is skill data — a
/// creature has no skill
/// block at all, which is exactly why the client substitutes a formula.
fn cgunit_skill(model: &Model, token: &str) -> i64 {
    model.unit(token).map_or(0, |u| i64::from(u.level)) * 5
}

/// Read inventory slot `slot` under a short borrow, cloned out so the caller holds no borrow.
/// `None` = empty / out of range / a token with no equipment source.
///
/// **Unit-keyed, and that is the reference's own shape** (decision 0631): the engine answers
/// `GetInventoryItemTexture(unit, slot)` for any unit whose item data it holds, which is why the
/// inspect paper doll reuses the very same bindings rather than a parallel `GetInspectItem*`
/// family. Two sources, by token:
///
/// - `"player"` → the self feed ([`Model::inventory_slots`]), resolved from our PRIVATE
///   `PLAYER_FIELD_INV_SLOT_*` guids → item objects → templates. Carries counts, durability,
///   locks — everything an owned item object knows.
/// - the **inspected** token → [`Model::inspect`], resolved from the target's PUBLIC
///   `PLAYER_VISIBLE_ITEM_*` entries. No item objects exist for a foreign player (their inventory
///   guids are server-private), so those views carry entry/icon/name/quality and nothing else —
///   which is exactly why the reference's inspect window shows no stack counts or durability.
fn player_inv_slot(lua: &Lua, token: &Option<String>, slot: i64) -> Option<InvSlotView> {
    let token = token.as_deref()?;
    let idx = usize::try_from(slot).ok()?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model.inv_slot(token, idx)
}

/// The live-API inventory ids the BANK occupies — `BankButtonIDToInvSlotID`'s own two bands
/// (`super::bank`): the 24 vault slots at 40..=63, the six bank-bag slots at 64..=69.
const BANK_INV_SLOTS: std::ops::RangeInclusive<usize> = 40..=63;
const BANK_BAG_INV_SLOTS: std::ops::RangeInclusive<usize> = 64..=69;

/// The container id the vault's own slots live under — `ui_items`' constant of the same name,
/// restated here because this file is the other end of the same map.
const BANK_CONTAINER: i64 = -1;

impl Model {
    /// The equipment slot `token` exposes at live-API id `slot`, or `None`. The one place the
    /// source routing is decided — shared by the `GetInventoryItem*` getters and
    /// `GameTooltip:SetInventoryItem`, so a tooltip can never disagree with the icon under it.
    ///
    /// **THREE sources now, and the third is a VIEW, not a store** (decision 1751's bank swap).
    /// The reference's bank paints every one of its slots through the *inventory* API —
    /// `BankFrameItemButton_OnUpdate` reads `GetInventoryItemTexture("player", BankButtonIDToInv
    /// SlotID(id))` (BankFrame.lua:35) — while benilla feeds the very same items as *containers*
    /// (`-1` for the vault, `5..10` for the bank bags), which is how our own bank window read
    /// them. Those are two names for one set of descriptor fields, so the fix is a map, not a
    /// second copy: a read in the bank band is answered from the container snapshot. Duplicating
    /// the items into a wider `inventory_slots` array would put two truths in the model and let
    /// a tooltip disagree with the icon under it, which is the one thing this function exists to
    /// prevent.
    pub(super) fn inv_slot(&self, token: &str, slot: usize) -> Option<InvSlotView> {
        if token.eq_ignore_ascii_case("player") {
            if let Some(view) = self.bank_inv_slot(slot) {
                return Some(view);
            }
            return self.inventory_slots.get(slot)?.clone();
        }
        self.inspect
            .as_ref()
            .filter(|v| v.unit == token)?
            .slots
            .get(slot)?
            .clone()
    }

    /// The bank band's answer, or `None` for any id outside it — see [`Model::inv_slot`].
    fn bank_inv_slot(&self, slot: usize) -> Option<InvSlotView> {
        if BANK_INV_SLOTS.contains(&slot) {
            let vault = self.containers.get(&BANK_CONTAINER)?;
            let n = (slot - BANK_INV_SLOTS.start() + 1) as u32;
            return vault.slots.get(&n).map(InvSlotView::from_container_slot);
        }
        if BANK_BAG_INV_SLOTS.contains(&slot) {
            // A bank BAG is not a slot in a container — it IS one, so there is no container slot
            // to map and this band is a real store ([`BankBagSlots`], fed off the player
            // descriptor's own `PLAYER_FIELD_BANK_BAG_SLOT_*` guids by the same system that feeds
            // the doll). It has to be the whole item, not just its icon: the reference picks a
            // bank bag up with `PickupBagFromSlot(BankButtonIDToInvSlotID(i, 1))` and describes it
            // with `GameTooltip:SetInventoryItem("player", …)`, both of which read this band.
            let i = slot - BANK_BAG_INV_SLOTS.start();
            return self.bank_bag_slots.get(i)?.clone();
        }
        None
    }
}

/// The shared inventory-slot reader's whitelist (`0x4c8520`), on the **0-based** value it hands
/// back (`0x4c8546 dec eax`) — Lua ids are one higher. Anything outside it raises the calling
/// binding's own "Invalid inventory slot in …", which is not the same thing as an empty answer.
///
/// The bands are the reader's own, kept apart rather than merged into one 39..=68 range because
/// they are different stores: the ammo pseudo-slot, the doll (equipment + the four equipped bag
/// icons), the bank's 24 vault slots, its 6 bag slots, and the keyring. Note what is NOT here —
/// the backpack's own 16 item slots (0-based 23..=38) and buyback (69..=80): both are addressed
/// through the container API instead, and a macro handing either to an inventory binding raises.
fn inventory_slot_reader_accepts(slot0: i32) -> bool {
    slot0 == -1                            // Lua 0      the ammo leg
        || (0x00..=0x16).contains(&slot0)  // Lua 1..=23   the doll + the four equipped bags
        || (0x27..=0x3e).contains(&slot0)  // Lua 40..=63  the bank vault
        || (0x3f..=0x44).contains(&slot0)  // Lua 64..=69  the bank bag slots
        || (0x51..=0x70).contains(&slot0) // Lua 82..=113 the keyring
}

/// The display name inside an item link — the text between `|h[` and `]|h`.
fn link_item_name(link: &str) -> Option<String> {
    let (_, rest) = link.split_once("|h[")?;
    let (name, _) = rest.split_once("]|h")?;
    Some(name.to_string())
}

impl InvSlotView {
    /// The same item, seen through the inventory API instead of the container API — the map
    /// [`Model::inv_slot`]'s bank band is built on. Every field is the same descriptor field under
    /// the other name; what a `ContainerSlot` has and this does not (`equip_slots`, `cooldown`,
    /// `readable`) has no inventory-API reader, and what this has and a container slot does not
    /// (`flags`) is not carried by the container feed.
    fn from_container_slot(slot: &super::container::ContainerSlot) -> Self {
        InvSlotView {
            item_id: slot.item_id,
            bar_placeable: slot.bar_placeable,
            icon: slot.texture.clone(),
            count: slot.count,
            quality: slot.quality.map_or(0, |q| q as i32),
            // A container slot carries no `name` of its own — the container API never asks for one
            // — but its `link` is composed FROM the name (`|Hitem:…|h[Name]|h`), so the one field
            // the inventory API adds is recoverable rather than absent.
            name: slot.link.as_deref().and_then(link_item_name),
            durability: slot.durability,
            already_bound: slot.already_bound,
            link: slot.link.clone(),
            locked: slot.locked,
            creator: slot.creator.clone(),
            enchants: slot.enchants.clone(),
            // Every band this mapper serves — the bank vault, Lua 40..=63 — sits past the
            // binding's `0x16` short-circuit, so a container met here answers 0 whatever is inside
            // it and only the `Some`/`None` distinction carries information. "Names Bag0Slot as a
            // place it could be worn" is the same set as TYPEMASK_CONTAINER (`INVTYPE_BAG` is the
            // only inventory type `FindEquipSlot` maps there — `cursor::bag_verbs::is_container`
            // states the equivalence in full), and it is what a container slot carries.
            contents_count: slot.equip_slots.contains(&20).then_some(0),
            ..Default::default()
        }
    }
}

/// The four reference `.data` literals the two indexed stat bindings raise with — read out of the
/// shipped 5875 image, verbatim, and NOT paraphrased (the house rule `GetInventorySlotInfo` sets:
/// an addon may compare or key on the message text).
///
/// **Each binding has TWO raises with different strings, and the string pool interleaves them so
/// that "the nearest `Usage:`" picks the WRONG binding's.** The layout is
/// `Usage: UnitResistance…` (`0x8510fc`) · `Invalid resistance index…` (`0x85112c`) ·
/// `Usage: UnitStat…` (`0x851158`) · `Invalid stat index…` (`0x85117c`) — so the `Usage:` two
/// padding bytes after `UnitResistance`'s index message belongs to `UnitStat`. Each literal is
/// fixed by the `push <VA>` that names it (`0x5187d6`/`0x5187ed` for `UnitStat`,
/// `0x5185cb`/`0x5185e2` for `UnitResistance`), never by proximity. This is the same trap
/// `GetInventorySlotInfo`'s transcription records, one degree worse.
///
/// The **index** arm is these two — no `Usage:` prefix, and the offending index is *not*
/// interpolated (`luaL_error` is called cdecl with exactly two dwords at all four sites: the `L`
/// and the message, no varargs, so the literal is never a format string).
const STAT_INDEX_ERROR: &str = "Invalid stat index in UnitStat";
const RESISTANCE_INDEX_ERROR: &str = "Invalid resistance index in UnitResistance";
/// The **argument-type** arm — a separate, earlier raise, reached only by a value that is neither
/// numeric nor a numeric string (the guards are coercion-aware: `UnitStat("player", "3")` serves
/// stat 3). [`binding_abi`] owns that contract.
const STAT_USAGE: &str = "Usage: UnitStat(\"unit\", statIndex)";
const RESISTANCE_USAGE: &str = "Usage: UnitResistance(\"unit\", resistanceIndex)";

/// Register the paper-doll stat/slot globals (decision 0208 §3).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // UnitStat("player", i /*1..=5*/) → (stat, effectiveStat, posBuff, negBuff).
    //
    // **The first two returns are the same field, UNDECOMPOSED** — `UNIT_FIELD_STAT0+i` raw
    // (`fild`, `0x518689`), then that same value clamped at zero (`0x5186a3`–`0x5186b7`:
    // `setl cl; dec ecx; and ecx,eax`). Slots 3/4 are `PLAYER_FIELD_POSSTAT0+i` (`0x518712`) and
    // `NEGSTAT0+i` (`0x518772`), each behind a SELF gate.
    //
    // This served `effective − pos − neg` as the first return until decision 1397. Subtracting is
    // the ref Lua's *own* job — `PaperDollFrame_SetStats` writes the tooltip's base as
    // `(stat - posBuff - negBuff)` — so a pre-subtracted `stat` deducts the buff twice. It was
    // invisible only because pos/neg were stuck at 0 by the field-decode bug 1397 fixes; the two
    // are one repair.
    //
    // **Note the contrast with the two resistance bindings below**, whose first return really *is*
    // the decomposed base (`0x5efcd0`: `*arg2 = raw - pos - neg`). The reference reads the two
    // families differently, and the ref Lua's own asymmetry — `SetStats` subtracts, `SetResistances`
    // and `PaperDollFormatStat` do not — is the tell.
    //
    // **An out-of-range index RAISES; it does not answer zeros** (see [`STAT_INDEX_ERROR`]).
    g.set(
        "UnitStat",
        lua.create_function(|lua, (token, i): (Value, Value)| {
            let token = binding_abi::string_arg(lua, token, STAT_USAGE)?;
            let idx = binding_abi::number_arg(lua, i, STAT_USAGE)? - 1;
            if !(0..5).contains(&idx) {
                return Err(mlua::Error::RuntimeError(STAT_INDEX_ERROR.into()));
            }
            let idx = idx as usize;
            Ok(with_unit_stats(lua, &Some(token), |s| {
                let (raw, pos, neg) = (s.stats[idx], s.stat_pos[idx], s.stat_neg[idx]);
                (
                    i64::from(raw),
                    i64::from(raw.max(0)),
                    i64::from(pos),
                    i64::from(neg),
                )
            }))
        })?,
    )?;

    // UnitResistance("player", school /*0..=6*/) → (base, resistance, positive, negative) — the
    // decomposition helper `0x5efcd0` per school ([0] = armor): `base = raw − pos − neg`, computed
    // BEFORE the clamp, and `resistance = max(raw, 0)`. A school cursed below zero therefore reads
    // a *displayed* 0 with the real (negative) total still folded out of `base`.
    // Out-of-range raises too, with its own string ([`RESISTANCE_INDEX_ERROR`]).
    g.set(
        "UnitResistance",
        lua.create_function(|lua, (token, school): (Value, Value)| {
            let token = binding_abi::string_arg(lua, token, RESISTANCE_USAGE)?;
            let school = binding_abi::number_arg(lua, school, RESISTANCE_USAGE)?;
            if !(0..7).contains(&school) {
                return Err(mlua::Error::RuntimeError(RESISTANCE_INDEX_ERROR.into()));
            }
            let idx = school as usize;
            Ok(with_unit_stats(lua, &Some(token), |s| {
                let (raw, pos, neg) = (
                    s.resistances[idx],
                    s.resistance_pos[idx],
                    s.resistance_neg[idx],
                );
                (
                    i64::from(raw - pos - neg),
                    i64::from(raw.max(0)),
                    i64::from(pos),
                    i64::from(neg),
                )
            }))
        })?,
    )?;

    // UnitArmor("player") → (base, effectiveArmor, armor, posBuff, negBuff) — school 0 through the
    // same `0x5efcd0`, whose 3rd and 4th out-params are both the clamped total (the ref reads
    // effectiveArmor and armor equivalently, and both are zeroed together when raw < 0).
    g.set(
        "UnitArmor",
        lua.create_function(|lua, token: Option<String>| {
            Ok(with_unit_stats(lua, &token, |s| {
                let (raw, pos, neg) = (s.resistances[0], s.resistance_pos[0], s.resistance_neg[0]);
                (
                    i64::from(raw - pos - neg),
                    i64::from(raw.max(0)),
                    i64::from(raw.max(0)),
                    i64::from(pos),
                    i64::from(neg),
                )
            }))
        })?,
    )?;

    // UnitDamage("player") → (minDamage, maxDamage, minOffHandDamage, maxOffHandDamage,
    // physicalBonusPos, physicalBonusNeg, percent) — the damage fields verbatim plus the
    // school-0 MOD_DAMAGE_DONE decomposition (percent 1.0 when absent; decision 0208 "to
    // confirm" flags the buffed decomposition as inferred).
    g.set(
        "UnitDamage",
        lua.create_function(|lua, token: Option<String>| {
            Ok(with_unit_stats(lua, &token, |s| {
                (
                    f64::from(s.min_damage),
                    f64::from(s.max_damage),
                    f64::from(s.min_offhand_damage),
                    f64::from(s.max_offhand_damage),
                    i64::from(s.physical_bonus_pos),
                    i64::from(s.physical_bonus_neg),
                    f64::from(s.damage_percent),
                )
            }))
        })?,
    )?;

    // UnitAttackSpeed("player") → (mainSpeed, offhandSpeed|nil) in seconds (BASEATTACKTIME /
    // 1000). Offhand is nil unless an offhand weapon is equipped — the snapshot's has_offhand,
    // the app's call (the field itself streams regardless).
    g.set(
        "UnitAttackSpeed",
        lua.create_function(|lua, token: Option<String>| {
            Ok(with_unit_stats(lua, &token, |s| {
                (
                    f64::from(s.main_attack_time_ms) / 1000.0,
                    s.has_offhand
                        .then_some(f64::from(s.offhand_attack_time_ms) / 1000.0),
                )
            }))
        })?,
    )?;

    // UnitAttackPower("player") → (base, posBuff, negBuff): base = UNIT_FIELD_ATTACK_POWER, the
    // buffs the MODS field's split signed halves (neg ≤ 0 — StatSystem.cpp:335-336).
    g.set(
        "UnitAttackPower",
        lua.create_function(|lua, token: Option<String>| {
            Ok(with_unit_stats(lua, &token, |s| {
                (
                    i64::from(s.attack_power),
                    i64::from(s.attack_power_pos),
                    i64::from(s.attack_power_neg),
                )
            }))
        })?,
    )?;

    // UnitRangedAttackPower("player") → (base, posBuff, negBuff) — the ranged twin.
    g.set(
        "UnitRangedAttackPower",
        lua.create_function(|lua, token: Option<String>| {
            Ok(with_unit_stats(lua, &token, |s| {
                (
                    i64::from(s.ranged_attack_power),
                    i64::from(s.ranged_attack_power_pos),
                    i64::from(s.ranged_attack_power_neg),
                )
            }))
        })?,
    )?;

    // UnitAttackBothHands("player") → (mainBase, mainMod, offBase, offMod) — **FOUR** returns, one
    // pair per hand: each hand's weapon-skill line, value + its temp+perm bonus, resolved app-side
    // via the weapon_subclass_skill table (unarmed = SKILL_UNARMED for an empty or non-weapon
    // hand).
    //
    // **It answered two until decision 1810**, which is 1793's shape at its quietest: `0x518810`
    // calls `[vtbl+0xb0]` TWICE, hand 0 then hand 1, and pushes `(base0, mod0, base1, mod1)`.
    // The reference's own
    // `PaperDollFrame_SetAttackBothHands` destructures only the first two — its next line is
    // `-- FIXME: The offhand stats aren't displayed yet.` — so putting the stock character sheet on
    // the chain could never have exposed this. An addon reading four gets two nils.
    //
    // A non-player unit takes the same virtual fork `UnitDefense` does (`0x6136b0`) and lands on
    // the identical `level * 5` — and that one **ignores its hand index outright**, so both hands
    // answer the same pair. The pet sheet's Attack row reads 300 at level 60, like its Defense row.
    g.set(
        "UnitAttackBothHands",
        lua.create_function(|lua, token: Option<String>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match token.as_deref() {
                Some("player") => model
                    .player_combat_stats
                    .as_ref()
                    .map_or((0, 0, 0, 0), |s| {
                        let (mb, mm) = skill_clamped(s.main_weapon_skill);
                        let (ob, om) = skill_clamped(s.offhand_weapon_skill);
                        (mb, mm, ob, om)
                    }),
                Some("pet") => {
                    let base = cgunit_skill(&model, "pet");
                    (base, 0, base, 0)
                }
                _ => (0, 0, 0, 0),
            })
        })?,
    )?;

    // UnitRangedAttack("player") → (base, modifier) — the ranged weapon's skill pair.
    g.set(
        "UnitRangedAttack",
        lua.create_function(|lua, token: Option<String>| {
            Ok(with_unit_stats(lua, &token, |s| {
                (
                    i64::from(s.ranged_weapon_skill.0),
                    i64::from(s.ranged_weapon_skill.1),
                )
            }))
        })?,
    )?;

    // UnitDefense(unit) → (base, modifier). The ref reads exactly two numbers and folds the
    // modifier's sign itself (PaperDollFrame.lua:259-271: modifier > 0 → a green posBuff, < 0 → a
    // red negBuff).
    //
    // **The fork is a VIRTUAL CALL on the resolved unit, not a token test** (correcting decision
    // 1057's INTERIM):
    // `[vtbl+0xac]` is either `CGPlayer_C 0x5eda20` — resolve the Defense SkillLine (`0x6de040`)
    // and read `PLAYER_SKILL_INFO` — or `CGUnit_C 0x613680`, which is three lines:
    // `*out1 = UNIT_FIELD_LEVEL * 5; *out2 = 0`. **A level-60 pet shows 300**,
    // not the 0 that 1057 shipped. The outer gate is SELF **or**
    // `UNIT_FIELD_SUMMONEDBY == my guid`, which our pet passes and no other token we serve does —
    // so `"player"` takes the skill leg, `"pet"` the level leg (a warlock's minion included: it is
    // summoned by us too), and everything else the gate-failure zeros.
    g.set(
        "UnitDefense",
        lua.create_function(|lua, token: Option<String>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match token.as_deref() {
                Some("player") => model
                    .player_combat_stats
                    .as_ref()
                    .map_or((0, 0), |s| skill_clamped(s.defense_skill)),
                Some("pet") => (cgunit_skill(&model, "pet"), 0),
                _ => (0, 0),
            })
        })?,
    )?;

    // GetDodgeChance() / GetParryChance() / GetBlockChance() → one number each, no unit argument
    // (`reference/1.12-shapes.tsv`, all three `exact` on argc, arity and kind). Player-implicit,
    // like `HasWandEquipped` below; a field that has not streamed reads 0, which is the wire's own
    // default rather than an absence to model.
    for (name, pick) in [
        ("GetDodgeChance", 0usize),
        ("GetParryChance", 1),
        ("GetBlockChance", 2),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(model
                    .player_combat_stats
                    .as_ref()
                    .map_or(0.0, |s| match pick {
                        0 => f64::from(s.dodge_percent),
                        1 => f64::from(s.parry_percent),
                        _ => f64::from(s.block_percent),
                    }))
            })?,
        )?;
    }

    // UnitRangedDamage("player") → (speed, minDamage, maxDamage, physicalBonusPos,
    // physicalBonusNeg, percent) — RANGEDATTACKTIME/1000 + the ranged damage range + the same
    // school-0 mods as UnitDamage.
    g.set(
        "UnitRangedDamage",
        lua.create_function(|lua, token: Option<String>| {
            Ok(with_unit_stats(lua, &token, |s| {
                (
                    f64::from(s.ranged_attack_time_ms) / 1000.0,
                    f64::from(s.ranged_min_damage),
                    f64::from(s.ranged_max_damage),
                    i64::from(s.physical_bonus_pos),
                    i64::from(s.physical_bonus_neg),
                    f64::from(s.damage_percent),
                )
            }))
        })?,
    )?;

    // HasWandEquipped() → 1/nil (the ref's ranged block swaps "Shoot" in on it). No unit arg —
    // the live global is player-implicit.
    g.set(
        "HasWandEquipped",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(
                model
                    .player_combat_stats
                    .as_ref()
                    .is_some_and(|s| s.has_wand),
            ))
        })?,
    )?;

    // GetInventoryItemID("player", slot) → itemId | nil (slot per GetInventorySlotInfo: 0 ammo,
    // 1..=19 equipment). Player-token-only like the stat block — the INV_SLOT guids are PRIVATE.
    g.set(
        "GetInventoryItemID",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot) {
                Some(v) if v.item_id != 0 => Ok(Value::Integer(i64::from(v.item_id))),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetInventoryItemTexture("player", slot) → icon path | nil (nil while the template answer is
    // in flight — the button shows the empty-slot art until the push lands).
    g.set(
        "GetInventoryItemTexture",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot).and_then(|v| v.icon) {
                Some(icon) => Ok(Value::String(lua.create_string(&icon)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetInventoryItemLink(unit, slot) → the full escaped `|cff…|Hitem:…|h[Name]|h|r` | nil (nil
    // while the template answer is in flight — the link is built from the item's name + quality,
    // and neither is known until it lands). Unit-keyed like its siblings, so the reference's paper
    // doll and its inspect twin both reach it from one binding:
    // `DressUpItemLink(GetInventoryItemLink("player", this:GetID()))` (PaperDollFrame.lua:650) and
    // the shift-click `ChatFrameEditBox:Insert(...)` beside it (l.653). Decision 1059.
    g.set(
        "GetInventoryItemLink",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot).and_then(|v| v.link) {
                Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // The binding's own two `.data` literals, read out of the shipped 5875 image and NOT
    // paraphrased (the house rule `GetInventorySlotInfo` set: an addon may compare on the text).
    // Neither carries a period or a newline.
    //
    //   `0x8489b4`  arg 1 fails `is-number-or-string 0x6f3510`
    //   `0x848984`  arg 2 fails `lua_isnumber`, OR names a slot outside the reader's whitelist
    //
    // (`0x848b54` is the sibling `Invalid inventory slot in IsInventoryItemLocked`, unclaimed
    // here: that binding's own argument reader has not been carved, and guessing it is exactly
    // what this block exists to stop.)
    const USAGE_GET_INVENTORY_ITEM_COUNT: &str = "Usage: GetInventoryItemCount(unit, slot)";
    const INVALID_SLOT_GET_INVENTORY_ITEM_COUNT: &str =
        "Invalid inventory slot in GetInventoryItemCount";
    // GetInventoryItemCount("player", slot) — CARVED (`0x4c8680`), and it is nothing like the
    // shape every secondary source describes. Four answers, in the reference's own order:
    //
    //  · an EMPTY slot pushes **1**, not 0 (`0x4c8797`). Both FrameXML callers gate on
    //    `GetInventoryItemTexture` first, which is why nobody ever noticed.
    //  · a non-container pushes `ITEM_FIELD_STACK_COUNT`.
    //  · a CONTAINER (`OBJECT_FIELD_TYPE & 4`, `0x4c87a6`) in a slot whose 0-based id is **past
    //    `0x16`** pushes a literal 0 (`0x4c87af cmp …,0x16` / `jg 0x4c8813`) — before any lookup.
    //    The bank's six bag slots (Lua 64..69 → 0-based 63..68) are all in that band, which is the
    //    whole reason `BankFrameBag1` shows no digit in the real client: `SetItemButtonCount`'s
    //    `isBag and count > 0` arm never fires.
    //  · a CONTAINER in one of the four equipped bag slots pushes the `ItemSubClass.dbc`-gated
    //    sum of its contents — see [`InvSlotView::contents_count`].
    g.set(
        "GetInventoryItemCount",
        lua.create_function(|lua, (unit, slot): (Value, Value)| {
            let token = super::binding_abi::string_arg(lua, unit, USAGE_GET_INVENTORY_ITEM_COUNT)?;
            let slot0 =
                super::binding_abi::number_arg(lua, slot, INVALID_SLOT_GET_INVENTORY_ITEM_COUNT)?;
            let slot0 = slot0.wrapping_sub(1);
            if !inventory_slot_reader_accepts(slot0) {
                return Err(mlua::Error::RuntimeError(
                    INVALID_SLOT_GET_INVENTORY_ITEM_COUNT.into(),
                ));
            }
            let Some(v) = player_inv_slot(lua, &Some(token), i64::from(slot0) + 1) else {
                return Ok(1i64);
            };
            Ok(match v.contents_count {
                None => i64::from(v.count),
                Some(_) if slot0 > 0x16 => 0,
                Some(n) => i64::from(n),
            })
        })?,
    )?;

    // GetInventoryItemQuality("player", slot) → 0..6 | nil (nil for an empty slot — the wiki
    // shape; the ref keys quality borders on it).
    g.set(
        "GetInventoryItemQuality",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot) {
                Some(v) if v.item_id != 0 => Ok(Value::Integer(i64::from(v.quality))),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetInventoryItemBroken("player", slot) → 1 | nil — the shared broken classification
    // ([`slot_is_broken`]: durability 0 with a max, or the force-red flag bit; wrapped never).
    // The ref's PaperDollItemSlotButton_Update keys the red slot tint on it
    // (PaperDollFrame.lua:670-676).
    g.set(
        "GetInventoryItemBroken",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot) {
                Some(v) if slot_is_broken(&v) => Ok(Value::Integer(1)),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetInventoryItemCooldown(unit, slot) → (start, duration, enable), the EQUIPPED twin of
    // `GetContainerItemCooldown` (container.rs) and the same `GetTime`-clock convention.
    //
    // **It answers "no cooldown" and that is an absent FEED, not a pretended one.** benilla has no
    // equipped-item cooldown source yet — the same gap `tooltip_item`'s `SetInventoryItem` already
    // records where it leaves `hasCooldown` nil — so there is nothing to report and `(0, 0, 1)` is
    // exactly what the container twin answers for a slot with no record. It is bound rather than
    // left absent because the SOURCED `PaperDollFrame.lua` calls it UNCONDITIONALLY, once per
    // `PaperDollItemSlotButton_Update`: without it every button built from
    // `PaperDollItemSlotButtonTemplate` or `BagSlotButtonTemplate` raises inside its own OnLoad,
    // which is how pfUI's bag bar would meet it. A raise there is not the loud-and-correct kind
    // (1203) — it is a raise on a verb whose honest answer we already know.
    //
    // When an equipped-cooldown feed lands, this reads it the way the container twin reads
    // `container_cooldowns`, and the CooldownFrame the doll slots already carry starts sweeping
    // with no caller change.
    g.set(
        "GetInventoryItemCooldown",
        lua.create_function(|_, (_token, _slot): (Option<String>, i64)| {
            Ok((0.0f64, 0.0f64, 1i64))
        })?,
    )?;

    // GetInventoryAlertStatus(index) → the region's alert status (`INV_ALERT_STATUS`: 0 none,
    // 3 damaged/low-ammo, 4 broken; 1/2 = temp-enchant alerts, unfed) — DurabilityFrame's
    // armor-guy read. Valid 1..=12 (the client's own 12-entry table; index 12 = low ammo, which
    // the 1.12 FrameXML never reads); out-of-range reads 0.
    g.set(
        "GetInventoryAlertStatus",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(if (1..=12).contains(&index) {
                i64::from(model.inventory_alerts[index - 1])
            } else {
                0
            })
        })?,
    )?;

    // OffhandHasWeapon() → 1 | nil — whether the off-hand slot holds a WEAPON (item class 2)
    // rather than a shield/held-in-off-hand; DurabilityFrame.lua swaps the shield glyph for the
    // off-weapon glyph on it. Read off the equipped slot view + the template store.
    g.set(
        "OffhandHasWeapon",
        lua.create_function(|lua, (): ()| {
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            let is_weapon = model.inventory_slots[17]
                .as_ref()
                .filter(|v| v.item_id != 0)
                .and_then(|v| model.item_templates.get(&v.item_id))
                .is_some_and(|t| t.class == 2);
            Ok(is_weapon.then_some(1i64))
        })?,
    )?;

    // GetInventorySlotInfo(slotName) → (slotId, textureName, checkRelic) — the client's own
    // PaperDollItemFrame.dbc rows (SLOT_INFO above). An unknown name is a Lua error, the client's
    // own behavior.
    //
    // `checkRelic` is the number 1 for the ranged slot and nil elsewhere, unconditionally — it is
    // a property of the SLOT, not of the player. It pairs with `UnitHasRelicSlot("player")`,
    // which is the class half, and stock reads the two as a conjunction
    // (`PaperDollFrame.lua:680`/`:744`). This comment used to say `UnitHasRelicSlot` is a
    // later-era concept, always false in vanilla; it is not — decision 1796.
    g.set(
        "GetInventorySlotInfo",
        lua.create_function(|lua, name: String| {
            // **The name match is CASE-INSENSITIVE, and that was the whole bug.** `0x4c8215` calls
            // `0x64a4c0` -> `0x414310`, the CRT `_strnicmp`, whose comparison folds BOTH operands
            // (`0x414352 add ah,dh` / `0x41435c add al,dh`, with `dh = 0x20` and the A-Z bounds in
            // `bh`/`bl`) before the byte compare — ASCII folding, which is exactly
            // `eq_ignore_ascii_case`. Its locale-aware arm folds too, so the verdict does not
            // depend on one. `maxlen` is `0x7fffffff` and the loop stops at the first NUL on either
            // side, so it is a FULL-STRING compare, not a prefix. The non-circular control is that
            // the image also ships a byte-identical case-SENSITIVE wrapper (`0x64a480` ->
            // `0x40de80`, `rep cmpsb`, no fold) and this binding does not call it.
            //
            // Two 1.12-era corpus addons died at session start on this and both worked on the real
            // client: `FuBar_AmmoFu` passes `"ammoSlot"` and `FuBar_PoisonFu` `"MAINHANDSLOT"`.
            // The 36 shipped names stay pairwise distinct after folding, so first-match-wins cannot
            // become ambiguous.
            let Some((_, id, art)) = SLOT_INFO
                .iter()
                .find(|(n, _, _)| n.eq_ignore_ascii_case(&name))
            else {
                // The raise is faithful — there is no nil path, and `0x4c823c xor eax,eax; ret` is
                // dead code after `luaL_error` longjmps. The message is the reference's own
                // (`.rdata 0x848894`): no `Usage:` prefix, and the offending name is NOT
                // interpolated. The `Usage:` literals either side of it in the string pool belong
                // to `KeyRingButtonIDToInvSlotID` and `GetInventoryItemTexture` — the adjacency
                // trap.
                return Err(mlua::Error::runtime(
                    "Invalid inventory slot in GetInventorySlotInfo",
                ));
            };
            Ok((
                *id,
                // The DBC string, VERBATIM. `0x4c825b` pushes `[esi+4]` straight through
                // (`0x6f3890`, `repne scasb` for the length) with no normalisation anywhere, and
                // the stored bytes are `interface\paperdoll\UI-PaperDoll-Slot-Bag.blp` — a
                // LOWERCASE directory, and the `.blp` extension present. Only the
                // `UI-PaperDoll-Slot-` leaf is mixed case. Texture *loading* would not care (the
                // asset VFS folds case), but this is a Lua-visible string: anything that compares
                // it, keys a table by it or concatenates it sees these bytes.
                Value::String(
                    lua.create_string(format!(
                        "interface\\paperdoll\\UI-PaperDoll-Slot-{art}.blp"
                    ))?,
                ),
                // `checkRelic` — the NUMBER 1, never a boolean, and only for `RangedSlot`
                // (`0x4c8263 dec ecx; cmp ecx,0x11`, i.e. SlotNumber 18); nil for every other slot.
                // It shipped as a constant `false` here, which is falsey like nil but is the wrong
                // type for a caller that compares it to 1, and wrong outright for the ranged slot.
                if *id == 18 {
                    Value::Integer(1)
                } else {
                    Value::Nil
                },
            ))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        weapon_subclass_skill, RESISTANCE_INDEX_ERROR, SKILL_UNARMED, STAT_INDEX_ERROR, STAT_USAGE,
    };
    use crate::script::{InvSlotView, UiScript, UnitCombatStats};

    /// A filled snapshot exercising every field the bindings read.
    fn stats() -> UnitCombatStats {
        UnitCombatStats {
            stats: [25, 20, 22, 10, 11],
            stat_pos: [4, 0, 0, 0, 0],
            stat_neg: [0, -2, 0, 0, 0],
            resistances: [150, 0, 20, 0, 0, 0, -5],
            resistance_pos: [30, 0, 25, 0, 0, 0, 0],
            resistance_neg: [-10, 0, -5, 0, 0, 0, -5],
            min_damage: 12.5,
            max_damage: 19.5,
            min_offhand_damage: 5.0,
            max_offhand_damage: 9.0,
            physical_bonus_pos: 25,
            physical_bonus_neg: -3,
            damage_percent: 1.1,
            main_attack_time_ms: 2900,
            offhand_attack_time_ms: 1500,
            has_offhand: false,
            attack_power: 78,
            attack_power_pos: 30,
            attack_power_neg: -10,
            ranged_attack_power: 52,
            ranged_attack_power_pos: 0,
            ranged_attack_power_neg: -3,
            ranged_attack_time_ms: 2800,
            ranged_min_damage: 31.0,
            ranged_max_damage: 47.0,
            main_weapon_skill: (25, 2),
            offhand_weapon_skill: (20, 0),
            ranged_weapon_skill: (18, 0),
            defense_skill: (55, 4),
            has_wand: false,
            dodge_percent: 0.0,
            parry_percent: 0.0,
            block_percent: 0.0,
        }
    }

    /// A pet's snapshot: the UNIT half filled, every PLAYER-block-sourced field at its default —
    /// what `ui_char::unit_combat_stats` really produces over a creature's descriptor.
    fn pet_stats() -> UnitCombatStats {
        UnitCombatStats {
            stats: [63, 45, 68, 32, 42],
            resistances: [1810, 0, 15, 0, 0, 0, 0],
            min_damage: 30.5,
            max_damage: 44.5,
            main_attack_time_ms: 2000,
            attack_power: 178,
            attack_power_pos: 12,
            attack_power_neg: -4,
            ..Default::default()
        }
    }

    #[test]
    fn weapon_skill_table_matches_vmangos_item_cpp() {
        // The populated rows (Item.cpp:700-707).
        assert_eq!(weapon_subclass_skill(0), Some(44)); // axe
        assert_eq!(weapon_subclass_skill(1), Some(172)); // 2h axe
        assert_eq!(weapon_subclass_skill(2), Some(45)); // bow
        assert_eq!(weapon_subclass_skill(3), Some(46)); // gun
        assert_eq!(weapon_subclass_skill(4), Some(54)); // mace
        assert_eq!(weapon_subclass_skill(5), Some(160)); // 2h mace
        assert_eq!(weapon_subclass_skill(6), Some(229)); // polearm
        assert_eq!(weapon_subclass_skill(7), Some(43)); // sword
        assert_eq!(weapon_subclass_skill(8), Some(55)); // 2h sword
        assert_eq!(weapon_subclass_skill(10), Some(136)); // staff
        assert_eq!(weapon_subclass_skill(13), Some(SKILL_UNARMED)); // fist
        assert_eq!(weapon_subclass_skill(15), Some(173)); // dagger
        assert_eq!(weapon_subclass_skill(16), Some(176)); // thrown
        assert_eq!(weapon_subclass_skill(17), Some(253)); // spear
        assert_eq!(weapon_subclass_skill(18), Some(226)); // crossbow
        assert_eq!(weapon_subclass_skill(19), Some(228)); // wand
        assert_eq!(weapon_subclass_skill(20), Some(356)); // fishing pole
                                                          // The skill-less rows + out of range.
        for sub in [9u32, 11, 12, 14, 21, 100] {
            assert_eq!(weapon_subclass_skill(sub), None, "subclass {sub}");
        }
    }

    /// **The three avoidance verbs are player-implicit and answer ONE number each.**
    ///
    /// `reference/1.12-shapes.tsv` has all three at argc 0 / arity 1 / kind number, every column
    /// `exact` (`GetDodgeChance 0x516f00`, `GetBlockChance 0x516f60`, `GetParryChance 0x516fc0`),
    /// so an argument is ignored rather than resolved and there is no pet leg. The absent
    /// snapshot answers 0 — the wire's own default for a field that has not streamed, and the
    /// only answer a one-number arity can give.
    ///
    /// The three values below are exactly representable in `f32`, so the widening to Lua's double
    /// is lossless and the assertions can be equalities rather than epsilons.
    #[test]
    fn the_avoidance_verbs_are_player_implicit_and_answer_one_number() {
        let mut s = UiScript::new().unwrap();
        // Before anything streams: a number, not nil — the arity is exact.
        assert_eq!(s.eval::<f64>("return GetDodgeChance()").unwrap(), 0.0);
        s.set_player_combat_stats(Some(UnitCombatStats {
            dodge_percent: 5.25,
            parry_percent: 3.5,
            block_percent: 2.75,
            ..stats()
        }));
        assert_eq!(s.eval::<f64>("return GetDodgeChance()").unwrap(), 5.25);
        assert_eq!(s.eval::<f64>("return GetParryChance()").unwrap(), 3.5);
        assert_eq!(s.eval::<f64>("return GetBlockChance()").unwrap(), 2.75);
        // Exactly one return, and a unit token is not a parameter — an addon passing one gets the
        // player's number, which is what a zero-argc binding does with a stack it never reads.
        assert_eq!(
            s.eval::<i64>("local a, b = GetDodgeChance() return b == nil and 1 or 2")
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<f64>(r#"return GetDodgeChance("target")"#).unwrap(),
            5.25
        );
    }

    #[test]
    fn unit_stat_serves_the_raw_field_twice_and_the_buff_split() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(stats()));
        // Str: the raw field 25 in BOTH of the first two slots, then the +4/0 split. The ref Lua
        // does the subtraction itself (`stat - posBuff - negBuff` = 21 for the tooltip's base) —
        // this binding must not pre-subtract, or the buff is deducted twice.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 1)"#)
                .unwrap(),
            (25, 25, 4, 0)
        );
        // Agi: raw 20 with a −2 debuff (the ref tests negBuff < 0 to pick red over green).
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 2)"#)
                .unwrap(),
            (20, 20, 0, -2)
        );
        // Only the SECOND return is clamped: a stat driven below zero keeps its sign in slot 1.
        s.set_player_combat_stats(Some(UnitCombatStats {
            stats: [-3, 20, 22, 10, 11],
            ..stats()
        }));
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 1)"#)
                .unwrap(),
            (-3, 0, 4, 0)
        );
        s.set_player_combat_stats(Some(stats()));
        // A non-player token serves the absent zeros (no other unit streams these fields).
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("target", 1)"#)
                .unwrap(),
            (0, 0, 0, 0)
        );
    }

    /// The index arm **raises**, and the truncation that decides which side of the range an
    /// argument lands on happens FIRST. Both ends converge on one raise (`0x51865a js` and
    /// `0x518663 jge` → `0x5187d6`), and `_ftol` chops toward zero rather than flooring, so `1.9`
    /// is a valid `1` while `0.5` and any negative are not.
    #[test]
    fn an_out_of_range_stat_index_raises_the_references_own_string() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(stats()));
        for arg in ["6", "0", "-3", "0.5", "-0.5"] {
            let err = s
                .eval::<(i64, i64, i64, i64)>(&format!(r#"return UnitStat("player", {arg})"#))
                .unwrap_err();
            let msg = format!("{err}");
            assert!(
                msg.contains(STAT_INDEX_ERROR),
                "UnitStat(\"player\", {arg}) must raise {STAT_INDEX_ERROR:?}, got {msg:?}"
            );
            // The index arm carries no `Usage:` prefix, and the pool's interleaving makes the
            // WRONG binding's usage line the nearest one — so assert we took neither.
            assert!(!msg.contains("Usage:"), "no Usage: prefix on the index arm");
        }
        // Truncation toward zero, ahead of the range test: 1.9 → 1 → stat 0, the valid answer.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 1.9)"#)
                .unwrap(),
            (25, 25, 4, 0)
        );
        // The guard is coercion-aware — a numeric STRING is a number here.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", "2")"#)
                .unwrap(),
            (20, 20, 0, -2)
        );
        // Neither numeric nor a numeric string takes the OTHER raise, with the other string.
        for arg in ["nil", "{}", "true", r#""abc""#] {
            let msg = format!(
                "{}",
                s.eval::<(i64, i64, i64, i64)>(&format!(r#"return UnitStat("player", {arg})"#))
                    .unwrap_err()
            );
            assert!(
                msg.contains(STAT_USAGE) && !msg.contains(STAT_INDEX_ERROR),
                "UnitStat(\"player\", {arg}) must take the Usage arm, got {msg:?}"
            );
        }
        // A missing unit token fails the same way (`0x6f3510` reports NULL past the top as
        // neither number nor string).
        assert!(format!(
            "{}",
            s.eval::<i64>(r#"return UnitStat(nil, 1)"#).unwrap_err()
        )
        .contains(STAT_USAGE));
    }

    #[test]
    fn unit_resistance_and_armor_decompose_school_zero() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(stats()));
        // Armor (school 0): 150 total, +30/−10 → base 130.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitResistance("player", 0)"#)
                .unwrap(),
            (130, 150, 30, -10)
        );
        // Fire (school 2): 20 total, +25/−5 → base 0.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitResistance("player", 2)"#)
                .unwrap(),
            (0, 20, 25, -5)
        );
        // A school cursed below zero (arcane: −5 total, all from the debuff): the DISPLAYED total
        // is clamped to 0 (`0x5efcd0`'s tail), while `base` keeps the pre-clamp decomposition.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitResistance("player", 6)"#)
                .unwrap(),
            (0, 0, 0, -5)
        );
        // UnitArmor is school 0 with the five-return shape (effectiveArmor = armor = the total).
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("player")"#)
                .unwrap(),
            (130, 150, 150, 30, -10)
        );
        // A non-player token: zeros. (UnitArmor takes no index and so has no index raise.)
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("target")"#)
                .unwrap(),
            (0, 0, 0, 0, 0)
        );
        // Out of range RAISES, with UnitResistance's own string — not UnitStat's, which is the
        // literal sitting two padding bytes away in the reference's string pool.
        for arg in ["7", "-1"] {
            let msg = format!(
                "{}",
                s.eval::<(i64, i64, i64, i64)>(&format!(
                    r#"return UnitResistance("player", {arg})"#
                ))
                .unwrap_err()
            );
            assert!(
                msg.contains(RESISTANCE_INDEX_ERROR) && !msg.contains("UnitStat"),
                "UnitResistance(\"player\", {arg}) must raise {RESISTANCE_INDEX_ERROR:?}, got {msg:?}"
            );
        }
        // School 0 is in range and is armor — the low end is inclusive, unlike UnitStat's.
        assert!(s
            .eval::<(i64, i64, i64, i64)>(r#"return UnitResistance("player", 0)"#)
            .is_ok());
    }

    #[test]
    fn unit_damage_and_attack_speed_read_the_snapshot() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(stats()));
        assert_eq!(
            s.eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("player")"#)
                .unwrap(),
            (12.5, 19.5, 5.0, 9.0, 25, -3, f64::from(1.1f32))
        );
        // No offhand equipped → second return nil.
        assert!(s
            .eval::<bool>(r#"local m, o = UnitAttackSpeed("player") return m == 2.9 and o == nil"#)
            .unwrap());
        // With an offhand: both speeds, in seconds.
        s.set_player_combat_stats(Some(UnitCombatStats {
            has_offhand: true,
            ..stats()
        }));
        assert_eq!(
            s.eval::<(f64, f64)>(r#"return UnitAttackSpeed("player")"#)
                .unwrap(),
            (2.9, 1.5)
        );
        // Absent snapshot / non-player token: zeros, percent 1.0 (the div-safe identity).
        assert_eq!(
            s.eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("target")"#)
                .unwrap(),
            (0.0, 0.0, 0.0, 0.0, 0, 0, 1.0)
        );
        let fresh = UiScript::new().unwrap();
        assert_eq!(
            fresh
                .eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("player")"#)
                .unwrap(),
            (0.0, 0.0, 0.0, 0.0, 0, 0, 1.0)
        );
    }

    #[test]
    fn attack_power_and_weapon_skill_bindings_serve_the_pairs() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(stats()));
        assert_eq!(
            s.eval::<(i64, i64, i64)>(r#"return UnitAttackPower("player")"#)
                .unwrap(),
            (78, 30, -10)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64)>(r#"return UnitRangedAttackPower("player")"#)
                .unwrap(),
            (52, 0, -3)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitAttackBothHands("player")"#)
                .unwrap(),
            (25, 2, 20, 0),
            "FOUR values, one pair per hand"
        );
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitRangedAttack("player")"#)
                .unwrap(),
            (18, 0)
        );
        assert_eq!(
            s.eval::<(f64, f64, f64, i64, i64, f64)>(r#"return UnitRangedDamage("player")"#)
                .unwrap(),
            (2.8, 31.0, 47.0, 25, -3, f64::from(1.1f32))
        );
        // Non-player: the absent zeros throughout.
        assert_eq!(
            s.eval::<(i64, i64, i64)>(r#"return UnitAttackPower("target")"#)
                .unwrap(),
            (0, 0, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitAttackBothHands("target")"#)
                .unwrap(),
            (0, 0, 0, 0)
        );
    }

    /// **`UnitRangedAttack` does NOT take the pet fork its two neighbours take** (decision 1812).
    ///
    /// `UnitDefense` `0x519200` and `UnitAttackBothHands` `0x518810` gate on SELF-OR-MINE and then
    /// dispatch through the resolved unit's vtable, so a pet passes and lands on `CGUnit_C`'s
    /// `level * 5`. `UnitRangedAttack` `0x518b90` is a **direct call** gated on the PLAYER typemask
    /// alone, with no vtable slot and **no `CGUnit_C` fallback body at all** — `0x612b40`,
    /// `0x5edae0` and `0x5ea460` appear as a dword zero times image-wide, against a positive
    /// control of one hit each for the four addresses that ARE in a vtable.
    ///
    /// So a pet's ranged attack is `(0, 0)`, not `level * 5`. The whole reason this is a test and
    /// not a comment is that the three verbs look interchangeable from the FrameXML side, and
    /// mirroring the defense handling here would be a plausible, silent, wrong number.
    #[test]
    fn ranged_attack_gives_a_pet_zeros_where_defense_gives_it_level_times_five() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(stats()));
        s.set_pet_combat_stats(Some(pet_stats()));
        s.set_unit(
            "pet",
            Some(super::super::UnitState {
                exists: true,
                level: 60,
                ..Default::default()
            }),
        );
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitDefense("pet")"#)
                .unwrap(),
            (300, 0),
            "the vtable fork: level * 5"
        );
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitRangedAttack("pet")"#)
                .unwrap(),
            (0, 0),
            "no fork, no fallback body — a pet fails the PLAYER typemask and gets zeros"
        );
        // …and so does another PLAYER, which is the same gate seen from the other side.
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitRangedAttack("target")"#)
                .unwrap(),
            (0, 0)
        );
    }

    /// **`UnitAttackBothHands` pushes FOUR values, one pair per hand — and it pushed two until
    /// decision 1810.**
    ///
    /// The quietest shape decision 1793 describes: `0x518810` calls `[vtbl+0xb0]` twice, hand 0
    /// then hand 1, and pushes `(base0, mod0, base1, mod1)`. The reference's own
    /// `PaperDollFrame_SetAttackBothHands` destructures only the first two — its very next line is
    /// `-- FIXME: The offhand stats aren't displayed yet.` — so no amount of running the stock
    /// character sheet could have exposed it. An addon reading four got two nils.
    ///
    /// The pet leg is here too because it is the same call: `CGUnit_C 0x6136b0` takes a hand index
    /// and **never reads it**, so both hands answer the identical `level * 5`.
    #[test]
    fn attack_both_hands_answers_a_pair_per_hand() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(UnitCombatStats {
            main_weapon_skill: (300, 5),
            offhand_weapon_skill: (275, -12),
            ..stats()
        }));
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitAttackBothHands("player")"#)
                .unwrap(),
            (300, 5, 275, -12),
            "each hand's own skill line, in hand order"
        );
        // …and the per-hand clamp the binding applies after each virtual call
        // (`0x5188a7`: `if (out2 + out1 < 0) out2 = -out1`), which `UnitDefense` shares.
        s.set_player_combat_stats(Some(UnitCombatStats {
            main_weapon_skill: (40, -100),
            offhand_weapon_skill: (40, -40),
            defense_skill: (30, -90),
            ..stats()
        }));
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitAttackBothHands("player")"#)
                .unwrap(),
            (40, -40, 40, -40),
            "a debuff deeper than the skill reads as reduced-to-0, never a negative total"
        );
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitDefense("player")"#)
                .unwrap(),
            (30, -30),
            "the same clamp, the same law"
        );
    }

    /// **The pet routing, end to end** (decision 1057) — the thing no build gate can catch: a
    /// pushed pet snapshot really is what `Unit*("pet")` answers, while `"player"` still answers
    /// the player's and a third token still answers the absent shape. The failure this guards is
    /// silent: a mis-routed reader shows a pet sheet full of the *player's* numbers, or of zeros,
    /// and nothing errors.
    #[test]
    fn the_pet_token_reads_the_pet_snapshot_and_only_it() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(stats()));
        s.set_pet_combat_stats(Some(pet_stats()));

        // Stamina (index 3): the pet's 68 with no buff split, the player's 22 with its own.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("pet", 3)"#)
                .unwrap(),
            (68, 68, 0, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 3)"#)
                .unwrap(),
            (22, 22, 0, 0)
        );
        // Str, where the two genuinely differ AND the player carries a buff: the pet must not
        // inherit either number.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("pet", 1)"#)
                .unwrap(),
            (63, 63, 0, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 1)"#)
                .unwrap(),
            (25, 25, 4, 0)
        );
        // Resistances: fire (school 2) — 15 for the pet, 20 (+25/−5) for the player.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitResistance("pet", 2)"#)
                .unwrap(),
            (15, 15, 0, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitResistance("player", 2)"#)
                .unwrap(),
            (0, 20, 25, -5)
        );
        // UnitArmor (school 0): the pet's 1810 undecomposed, the player's 150 split.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("pet")"#)
                .unwrap(),
            (1810, 1810, 1810, 0, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("player")"#)
                .unwrap(),
            (130, 150, 150, 30, -10)
        );
        // The rest of the family routes too — and `percent` stays the divide-safe 1.0 for the pet
        // (the ref Lua divides the damage range by it).
        assert_eq!(
            s.eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("pet")"#)
                .unwrap(),
            (30.5, 44.5, 0.0, 0.0, 0, 0, 1.0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64)>(r#"return UnitAttackPower("pet")"#)
                .unwrap(),
            (178, 12, -4)
        );
        // No offhand ⇒ the second return is nil, the same gate as the player's.
        assert!(s
            .eval::<bool>(r#"local m, o = UnitAttackSpeed("pet") return m == 2.0 and o == nil"#)
            .unwrap());

        // A third token is still the absent shape…
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("target", 1)"#)
                .unwrap(),
            (0, 0, 0, 0)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("target")"#)
                .unwrap(),
            (0, 0, 0, 0, 0)
        );
        // …and so is `"pet"` once the pet is dismissed (the feed pushes None).
        s.set_pet_combat_stats(None);
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("pet", 1)"#)
                .unwrap(),
            (0, 0, 0, 0)
        );
        assert_eq!(
            s.eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("pet")"#)
                .unwrap(),
            (0.0, 0.0, 0.0, 0.0, 0, 0, 1.0)
        );
        // The player's is untouched by any of it.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 1)"#)
                .unwrap(),
            (25, 25, 4, 0)
        );
    }

    /// `UnitDefense` always answers two numbers (the ref reads `local base, modifier`), and **the
    /// two legs are different functions, not one function with missing data**: the player's is the
    /// skill pair, a pet's is `level * 5` with a flat 0 modifier (the vtable fork,
    /// `0x5eda20`/`0x613680`). `UnitAttackBothHands` takes the same fork and must agree with it —
    /// that agreement is the point, since a mismatch is exactly what a snapshot-only
    /// implementation would produce.
    #[test]
    fn unit_defense_forks_the_player_skill_from_a_pets_level_times_five() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(stats()));
        s.set_pet_combat_stats(Some(pet_stats()));
        s.set_unit(
            "pet",
            Some(super::super::UnitState {
                exists: true,
                level: 60,
                ..Default::default()
            }),
        );
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitDefense("player")"#)
                .unwrap(),
            (55, 4)
        );
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitDefense("pet")"#)
                .unwrap(),
            (300, 0),
            "a level-60 pet: level * 5, modifier flat 0"
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitAttackBothHands("pet")"#)
                .unwrap(),
            (300, 0, 300, 0),
            "the Attack row takes the same fork — and ignores its hand index, so both pairs match"
        );
        // The pet's snapshot carries neither pair, which is the whole point: the numbers above
        // cannot have come from it.
        assert_eq!(pet_stats().defense_skill, (0, 0));
        assert_eq!(pet_stats().main_weapon_skill, (0, 0));
        // A pet the level feed has not reached yet reads 0 rather than a stale or invented number.
        s.set_unit("pet", None);
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitDefense("pet")"#)
                .unwrap(),
            (0, 0)
        );
        // A token that fails the client's SELF-or-SUMMONEDBY gate gets the failure zeros, not
        // some other unit's level.
        s.set_unit(
            "target",
            Some(super::super::UnitState {
                exists: true,
                level: 63,
                ..Default::default()
            }),
        );
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitDefense("target")"#)
                .unwrap(),
            (0, 0)
        );
        // A negative modifier survives the round trip — the ref branches on `modifier < 0` to
        // paint the number red.
        s.set_player_combat_stats(Some(UnitCombatStats {
            defense_skill: (300, -25),
            ..stats()
        }));
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitDefense("player")"#)
                .unwrap(),
            (300, -25)
        );
    }

    #[test]
    fn has_wand_equipped_reads_the_flag() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.eval::<bool>("return HasWandEquipped()").unwrap());
        s.set_player_combat_stats(Some(UnitCombatStats {
            has_wand: true,
            ..stats()
        }));
        assert!(s.eval::<bool>("return HasWandEquipped()").unwrap());
    }

    /// **`GetInventoryItemCount`'s container fork** (`0x4c8680`). Four answers, and three of them
    /// are not the stack count:
    ///
    /// · an equipped QUIVER (its `ItemSubClass.dbc` row has `DisplayFlags & 0x4`) answers the sum
    ///   of its arrows — the number that shows on the bag bar;
    /// · an equipped PLAIN BAG answers **0**, which is why the bar shows it no digit even though
    ///   `BagSlotButtonTemplate` sets `isBag = 1` and `SetItemButtonCount` would print any
    ///   positive number;
    /// · the SAME plain bag in a BANK BAG slot answers 0 too, but for a different reason and
    ///   earlier: `cmp …,0x16 / jg` short-circuits every container past the four equipped bag
    ///   slots before the DBC is ever consulted. This is the digit the director saw on
    ///   `BankFrameBag1` (decision 1771's sibling), and asserting it here rather than only through
    ///   the window is the point — the window can only ever show that the two agree.
    #[test]
    fn get_inventory_item_count_forks_on_the_container_bit_and_the_slot() {
        let mut s = UiScript::new().unwrap();
        let bag = |contents: Option<u32>| {
            Some(InvSlotView {
                item_id: 4496,
                count: 1,
                contents_count: contents,
                ..Default::default()
            })
        };
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[20] = bag(Some(162)); // a quiver: the gate is set, the sum is its arrows
        slots[21] = bag(Some(0)); // a plain bag: the gate is clear
        slots[22] = Some(InvSlotView {
            item_id: 2263,
            count: 7,
            ..Default::default()
        }); // not a container at all
        s.set_inventory_slots(slots);
        let mut bank: crate::script::BankBagSlots = Default::default();
        bank[0] = bag(Some(162)); // the very same quiver, in a bank bag slot
        s.set_bank_bag_slots(bank);

        let count = |slot: i64| {
            s.eval::<i64>(&format!(
                r#"return GetInventoryItemCount("player", {slot})"#
            ))
            .unwrap()
        };
        assert_eq!(count(20), 162, "a quiver counts what is inside it");
        assert_eq!(
            count(21),
            0,
            "a plain bag counts nothing — not its own stack"
        );
        assert_eq!(count(22), 7, "an ordinary item is still its stack count");
        assert_eq!(
            count(64),
            0,
            "0-based 63 is past 0x16: every container short-circuits, quiver or not"
        );

        // The two `.data` literals, and the fact that both arms RAISE rather than answer. A slot
        // outside the shared reader's whitelist is not an empty slot: the backpack's own item
        // slots (Lua 24..=39) are addressed through the container API, and handing one here
        // abandons the statement.
        let err = |code: &str| s.run(code).unwrap_err().to_string();
        assert!(
            err(r#"GetInventoryItemCount("player", 30)"#)
                .contains("Invalid inventory slot in GetInventoryItemCount"),
            "the backpack band raises"
        );
        assert!(
            err(r#"GetInventoryItemCount("player", 200)"#)
                .contains("Invalid inventory slot in GetInventoryItemCount"),
            "and so does anything past the keyring"
        );
        assert!(
            err(r#"GetInventoryItemCount("player", {})"#)
                .contains("Invalid inventory slot in GetInventoryItemCount"),
            "a non-number slot takes the SAME string, not the Usage one"
        );
        assert!(
            err("GetInventoryItemCount({}, 1)")
                .contains("Usage: GetInventoryItemCount(unit, slot)"),
            "…and only a bad unit takes the Usage string"
        );
        assert_eq!(
            count(0),
            1,
            "the ammo pseudo-slot (0-based -1) is in the whitelist, so it ANSWERS — and with no \
             ammo seated the answer is the empty one, which is 1"
        );
    }

    #[test]
    fn inventory_item_bindings_serve_occupied_empty_and_absent_shapes() {
        let mut s = UiScript::new().unwrap();
        let mut slots: crate::script::InventorySlots = Default::default();
        // Head (slot 1) occupied; ammo (slot 0) with a bag-summed count; the rest empty.
        slots[1] = Some(InvSlotView {
            item_id: 2263,
            icon: Some("Interface\\Icons\\INV_Misc_Bandana_01".into()),
            count: 1,
            quality: 2,
            name: Some("Brawler's Harness".into()),
            ..Default::default()
        });
        slots[0] = Some(InvSlotView {
            item_id: 2512,
            icon: Some("Interface\\Icons\\INV_Ammo_Arrow_01".into()),
            count: 200,
            quality: 1,
            name: Some("Rough Arrow".into()),
            ..Default::default()
        });
        s.set_inventory_slots(slots);

        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemID("player", 1)"#)
                .unwrap(),
            2263
        );
        assert_eq!(
            s.eval::<String>(r#"return GetInventoryItemTexture("player", 1)"#)
                .unwrap(),
            "Interface\\Icons\\INV_Misc_Bandana_01"
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemCount("player", 1)"#)
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemQuality("player", 1)"#)
                .unwrap(),
            2
        );
        // The ammo slot (0) reads through the same family.
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemID("player", 0)"#)
                .unwrap(),
            2512
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemCount("player", 0)"#)
                .unwrap(),
            200
        );
        // An empty slot: nil id/texture/quality — and **count 1, not 0** (`0x4c8797`). Every
        // secondary source says 0; the image pushes the literal 1, and both FrameXML callers gate
        // on `GetInventoryItemTexture` first, which is why nobody ever caught it.
        assert!(s
            .eval::<bool>(r#"return GetInventoryItemID("player", 5) == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return GetInventoryItemTexture("player", 5) == nil"#)
            .unwrap());
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemCount("player", 5)"#)
                .unwrap(),
            1
        );
        assert!(s
            .eval::<bool>(r#"return GetInventoryItemQuality("player", 5) == nil"#)
            .unwrap());
        // A non-player token: the empty shape (the INV_SLOT fields are PRIVATE — player only).
        assert!(s
            .eval::<bool>(r#"return GetInventoryItemID("target", 1) == nil"#)
            .unwrap());
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemCount("target", 1)"#)
                .unwrap(),
            1,
            "no item behind the token is the empty answer, and the empty answer is 1"
        );
        // Out-of-range slots: the empty shape, no error.
        assert!(s
            .eval::<bool>(r#"return GetInventoryItemID("player", 25) == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return GetInventoryItemID("player", -1) == nil"#)
            .unwrap());
    }

    #[test]
    fn get_inventory_slot_info_serves_the_dbc_rows() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<(i64, String)>(r#"return GetInventorySlotInfo("HeadSlot")"#)
                .unwrap(),
            (1, "interface\\paperdoll\\UI-PaperDoll-Slot-Head.blp".into())
        );
        // The DBC's oddballs: BackSlot shows the Chest art, AmmoSlot the Ranged art.
        assert_eq!(
            s.eval::<(i64, String)>(r#"return GetInventorySlotInfo("BackSlot")"#)
                .unwrap(),
            (
                15,
                "interface\\paperdoll\\UI-PaperDoll-Slot-Chest".to_string() + ".blp"
            )
        );
        assert_eq!(
            s.eval::<(i64, String)>(r#"return GetInventorySlotInfo("AmmoSlot")"#)
                .unwrap(),
            (
                0,
                "interface\\paperdoll\\UI-PaperDoll-Slot-Ranged.blp".into()
            )
        );
        // The bag rows: the four equipped-bag icons at 20..23 AND `Bag1`..`Bag12` at 64..75, which
        // this table was short of until the DBC was re-read (36 records, not 24). All SIXTEEN share
        // one string-block offset, which is why they answer the same art.
        const BAG_ART: &str = "interface\\paperdoll\\UI-PaperDoll-Slot-Bag.blp";
        for (name, id) in [
            ("Bag0Slot", 20),
            ("Bag1Slot", 21),
            ("Bag2Slot", 22),
            ("Bag3Slot", 23),
        ] {
            assert_eq!(
                s.eval::<(i64, String)>(&format!(r#"return GetInventorySlotInfo("{name}")"#))
                    .unwrap(),
                (id, BAG_ART.into()),
                "{name}"
            );
        }
        for n in 1..=12i64 {
            assert_eq!(
                s.eval::<(i64, String)>(&format!(r#"return GetInventorySlotInfo("Bag{n}")"#))
                    .unwrap(),
                (63 + n, BAG_ART.into()),
                "Bag{n}"
            );
        }
        assert_eq!(
            s.eval::<(i64, String)>(r#"return GetInventorySlotInfo("SecondaryHandSlot")"#)
                .unwrap(),
            (
                17,
                "interface\\paperdoll\\UI-PaperDoll-Slot-SecondaryHand.blp".into()
            )
        );
        // An unknown slot name is a Lua error (the client's own behavior).
        assert!(s
            .eval::<i64>(r#"return GetInventorySlotInfo("NoSuchSlot")"#)
            .is_err());
    }

    /// The character and pet paper dolls' facings are **the panes' own `SetRotation` state**, read
    /// back by name — not the two benilla-named scalars they were until decision 1751.
    ///
    /// The two tabs are two `<PlayerModel>`s that can sit at different facings, which is the whole
    /// reason this is per-pane at all: turning one must leave the other alone. It used to be
    /// guaranteed by there being two fields on `Model`; now it is guaranteed by there being two
    /// frames, which is also what the reference relies on (`Model_RotateLeft(model)` takes the
    /// pane).
    #[test]
    fn each_model_pane_carries_its_own_facing_for_the_app_to_sample() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.model_pane_facing("CharacterModelFrame"),
            0.0,
            "a pane whose file has not loaded"
        );
        s.run(
            r#"
            CreateFrame("PlayerModel", "CharacterModelFrame")
            CreateFrame("PlayerModel", "PetModelFrame")
        "#,
        )
        .unwrap();
        // The reference's own default, as `Model_OnLoad` writes it.
        s.run("CharacterModelFrame:SetRotation(0.61)").unwrap();
        assert_eq!(s.model_pane_facing("CharacterModelFrame"), 0.61);
        // Persistent, not a drain — two reads see the same value.
        assert_eq!(s.model_pane_facing("CharacterModelFrame"), 0.61);
        assert_eq!(
            s.model_pane_facing("PetModelFrame"),
            0.0,
            "the pet pane did not move"
        );

        s.run("PetModelFrame:SetRotation(1.2)").unwrap();
        assert_eq!(s.model_pane_facing("PetModelFrame"), 1.2);
        assert_eq!(
            s.model_pane_facing("CharacterModelFrame"),
            0.61,
            "…and neither did the character pane"
        );

        s.run("CharacterModelFrame:SetRotation(-0.5)").unwrap();
        assert_eq!(s.model_pane_facing("CharacterModelFrame"), -0.5);

        // A name nothing declares reads 0.0 rather than raising — the app samples this every
        // frame, including before the window's file has loaded.
        assert_eq!(s.model_pane_facing("NoSuchModelFrame"), 0.0);
        // …and so does a name that IS declared but is not a model pane, which is the shape a
        // renamed window would take.
        s.run(r#"CreateFrame("Frame", "NotAPane")"#).unwrap();
        assert!(s.model_pane("NotAPane").is_none());
    }
}
