//! The paper doll's data feed: the stat and equipment-slot globals, read from the combat-stats
//! snapshots the app pushes for `"player"` and `"pet"` and from its inventory-slot snapshot.
//!
//! As in the reference, the pet sheet calls the same `PaperDollFrame_Set*` helpers with `"pet"`
//! (`PetPaperDollFrame.lua:75-81`). Any other token, or a snapshot not yet pushed, serves the
//! absent shape: zeros, `percent` 1.0. A pet has no PLAYER block, so its buff splits read 0.
//!
//! For a third unit, zeros are right for `UnitStat` (`0x518600`): `UNIT_FIELD_STAT*` is PRIVATE +
//! OWNER_ONLY, so the reference holds zeros too. Not built: vmangos also sends a beast's
//! `UNIT_FIELD_RESISTANCES` to the caster of its Beast Lore (`Object.cpp:1065-1067`,
//! `Player.cpp:2603-2610`), and the reference's `UnitResistance` and `UnitArmor` then answer them.
//!
//! vmangos narrows the four buff-split arrays with `uint32(float)` (`Object.cpp:759-763`), so a
//! negative delta arrives as 0 from an arm64 server and intact from x86.

use mlua::{Lua, Value};

use super::binding_abi::flag;
use super::{binding_abi, Model};

/// The `SkillLine.dbc` id behind weapon subclass `0..=20` (vmangos `Item.cpp:700-707`), `None`
/// for the obsolete, exotic and misc rows; with no weapon the skill is [`SKILL_UNARMED`]
/// (`Player.cpp:20144-20155`).
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
        162, // 13 fist weapon → SKILL_UNARMED (vmangos's own row: fists use the unarmed skill)
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

/// The melee skill with no weapon equipped (vmangos `SharedDefines.h:987`, `Player.cpp:20153`).
pub const SKILL_UNARMED: u32 = 162;

/// The `SkillLine.dbc` Defense row `UnitDefense` reports (vmangos `SharedDefines.h:961`).
pub const SKILL_DEFENSE: u32 = 95;

/// One unit's combat stats behind a paper doll, from its descriptor. Arrays are in field order:
/// stats Str/Agi/Sta/Int/Spi, schools with `[0]` armor. A pet leaves the PLAYER-block fields (buff
/// splits, damage-done mods, skill pairs) at their defaults.
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
    /// The physical damage multiplier (`PLAYER_FIELD_MOD_DAMAGE_DONE_PCT[0]`), 1.0 until streamed.
    pub damage_percent: f32,
    /// Attack speeds in ms (`UNIT_FIELD_BASEATTACKTIME[0..2]`).
    pub main_attack_time_ms: u32,
    pub offhand_attack_time_ms: u32,
    /// Whether an off-hand weapon is equipped, which gates `UnitAttackSpeed`'s second return.
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
    /// The main hand's skill line (picked by [`weapon_subclass_skill`]) as `(value + perm, temp)`,
    /// the split the reference's skill reader `0x5ea460` makes for all four skill pairs.
    pub main_weapon_skill: (i32, i32),
    /// The off hand's pair, read the same way; an empty or non-weapon off hand is Unarmed.
    pub offhand_weapon_skill: (i32, i32),
    /// The ranged weapon's pair, the same split. Player only: `UnitRangedAttack` (`0x518b90`)
    /// has no creature fallback, so a pet answers `(0, 0)` where `UnitDefense` answers level * 5.
    pub ranged_weapon_skill: (i32, i32),
    /// The [`SKILL_DEFENSE`] pair, the same split. The player's only: `UnitDefense` gives a
    /// non-player level * 5 instead (`cgunit_skill`), so the pet feed leaves it unset.
    pub defense_skill: (i32, i32),
    /// Whether a wand is equipped (`HasWandEquipped`).
    pub has_wand: bool,
    /// Avoidance chances, already percents on the wire (`PLAYER_DODGE_PERCENTAGE` and siblings).
    pub dodge_percent: f32,
    pub parry_percent: f32,
    pub block_percent: f32,
}

impl Default for UnitCombatStats {
    /// All zeros but `damage_percent` 1.0, which stock divides by (`PaperDollFrame.lua:298`).
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

/// One equipment or ammo slot as the `GetInventoryItem*` family reads it, resolved by the app like
/// [`super::container::ContainerSlot`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InvSlotView {
    /// The item's template entry (`GetInventoryItemID`).
    pub item_id: u32,
    /// Whether the item may go on an action bar, by the same filter as a bag slot's.
    pub bar_placeable: bool,
    /// Icon texture path (`Interface\Icons\…`); `None` while the template answer is in flight.
    pub icon: Option<String>,
    /// `ITEM_FIELD_STACK_COUNT`, or for the ammo slot the bag-summed carried total.
    pub count: u32,
    /// For a container (`TYPEMASK_CONTAINER`, `0x4c87a6`), what `GetInventoryItemCount` pushes:
    /// its contents' summed stacks when its `ItemSubClass.dbc` row has `DisplayFlags & 0x4` (Soul
    /// Bag and quivers, `0x4c881a`), else 0; past 0-based slot `0x16` the binding answers 0 first.
    pub contents_count: Option<u32>,
    /// Item quality 0..6 (`GetInventoryItemQuality`).
    pub quality: i32,
    /// The item's name, once known.
    pub name: Option<String>,
    /// Live durability `(current, max)`, the tooltip's "Durability X / Y" line.
    pub durability: Option<(u32, u32)>,
    /// `ITEM_FIELD_FLAGS`; the alert recompute `0x4c7ee0` reads two bits: `0x10` force-red
    /// (status 4) and `0x08` wrapped (no durability alert).
    pub flags: u32,
    /// Bound at runtime (`0x5da2c0`): `ITEM_FIELD_FLAGS & 1`, or an enchant whose
    /// `SpellItemEnchantment` row binds. The tooltip's bind line then says Soulbound.
    pub already_bound: bool,
    /// An `|Hitem:…|h[Name]|h` link once the name is known.
    pub link: Option<String>,
    /// Whether a pending item operation covers this slot (`IsInventoryItemLocked`).
    pub locked: bool,
    /// The 1-based live-API slots this item could be equipped into; empty if none.
    pub equip_slots: Vec<u8>,
    /// The resolved `ITEM_FIELD_CREATOR` name, the tooltip's "<Made by %s>" line.
    pub creator: Option<String>,
    /// The resolved enchant slots in slot order. An inspected player's record has all 7 too, but
    /// vmangos fills only PERM and TEMP (`Player.cpp:10518-10519`).
    pub enchants: Vec<super::EnchantView>,
    /// The instance's remaining lifetime in ms; `None` without a timer.
    pub duration_ms: Option<u64>,
}

/// The doll snapshot: 0 ammo, 1..=19 equipment, 20..=23 the equipped bags, as
/// `GetInventorySlotInfo` numbers them; `None` is an empty slot.
pub const INVENTORY_SLOT_COUNT: usize = 24;
pub type InventorySlots = [Option<InvSlotView>; INVENTORY_SLOT_COUNT];

/// The six bank-bag slots in button order, live-API ids 64..=69; `None` is empty or unbought.
/// They are player-descriptor slots, present whether or not the bank window is open.
pub const BANK_BAG_SLOT_COUNT: usize = 6;
pub type BankBagSlots = [Option<InvSlotView>; BANK_BAG_SLOT_COUNT];

/// `PaperDollItemFrame.dbc`'s 36 rows as `(SlotName, SlotID, art suffix)`. The texture column
/// shares strings between rows, so `BackSlot` shows the Chest art and `AmmoSlot` the Ranged.
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
    // The bank-bag band is 64..69; all sixteen bag rows share one texture string.
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

/// The client's 12 durability-alert regions (`0x806eb8`) as live slot ids: stock's 11
/// `INVENTORY_ALERT_STATUS_SLOTS` (`DurabilityFrame.lua:2-12`), then low ammo, which the client
/// table holds as slot -1 and we as the ammo view, slot 0.
const ALERT_SLOTS: [usize; 12] = [1, 3, 5, 6, 7, 8, 9, 10, 16, 17, 18, 0];

/// Broken for the doll's red tint, from `0x4c7ee0`'s bits: never when wrapped (`0x08`), else when
/// force-red (`0x10`) or at durability 0 of a nonzero max.
fn slot_is_broken(v: &InvSlotView) -> bool {
    if v.item_id == 0 || v.flags & 0x08 != 0 {
        return false;
    }
    v.flags & 0x10 != 0 || matches!(v.durability, Some((0, max)) if max > 0)
}

/// One region's `GetInventoryAlertStatus` (`0x4c7ee0`): 4 when force-red or broken, 3 with 1..=5
/// durability points left (an absolute count, not a percentage), else 0. Statuses 1 and 2, the
/// temporary-enchant alerts, are not fed; stock gives them no color (`DurabilityFrame.lua:18-19`).
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

/// The low-ammo region (`0x4c7ee0`'s slot -1 arm): 3 when the carried ammo is 20 or fewer, else 0.
fn ammo_alert_status(slot: &Option<InvSlotView>) -> u8 {
    match slot.as_ref().filter(|v| v.item_id != 0) {
        Some(v) if v.count <= 20 => 3,
        _ => 0,
    }
}

impl super::UiScript {
    /// Push, or clear with `None`, the player's combat-stats snapshot.
    pub fn set_player_combat_stats(&mut self, stats: Option<UnitCombatStats>) {
        self.model_mut().player_combat_stats = stats;
    }

    /// The pet's snapshot; `None` without a pet, so `Unit*("pet")` reads the absent shape.
    pub fn set_pet_combat_stats(&mut self, stats: Option<UnitCombatStats>) {
        self.model_mut().pet_combat_stats = stats;
    }

    /// Push the doll snapshot, recompute the 12 alert statuses and fire `UPDATE_INVENTORY_ALERTS`.
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
        // Every recompute fires it, never diffed against the last (`0x4c7ee0`); the app pushes
        // only on a real change.
        self.fire_event("UPDATE_INVENTORY_ALERTS", vec![]);
    }

    /// Push the six bank-bag slots. The caller fires `PLAYERBANKSLOTS_CHANGED` for a changed item:
    /// the stock bank buttons repaint only on it and `BANKFRAME_OPENED` (`BankFrame.lua:206-212`).
    pub fn set_bank_bag_slots(&mut self, slots: BankBagSlots) {
        self.model_mut().bank_bag_slots = slots;
    }

    /// Drain the slot ids `UseInventoryItem` queued; the app sends each as `CMSG_USE_ITEM`, bag
    /// 255 and the 0-based slot.
    pub fn take_inventory_uses(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().inventory_uses)
    }
}

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

/// The clamp `UnitDefense` (`0x519298`) and each hand of `UnitAttackBothHands` (`0x5188a7`) apply:
/// a modifier below `-base` becomes `-base`, so the total never reads negative.
fn skill_clamped((base, modifier): (i32, i32)) -> (i64, i64) {
    let base = i64::from(base);
    let modifier = i64::from(modifier);
    (base, if modifier + base < 0 { -base } else { modifier })
}

/// A non-player's `UnitDefense` and `UnitAttackBothHands` answer: `CGUnit_C`'s vtable bodies
/// (`0x613680`, `0x6136b0`) return level * 5 with a 0 modifier; a creature has no skill block.
fn cgunit_skill(model: &Model, token: &str) -> i64 {
    model.unit(token).map_or(0, |u| i64::from(u.level)) * 5
}

/// `token`'s inventory slot, unit-keyed as in the reference so the inspect doll reuses these
/// bindings: `"player"` reads our own items, the inspected unit its public
/// `PLAYER_VISIBLE_ITEM_*` entries, which carry no counts or durability.
fn player_inv_slot(lua: &Lua, token: &Option<String>, slot: i64) -> Option<InvSlotView> {
    let token = token.as_deref()?;
    let idx = usize::try_from(slot).ok()?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model.inv_slot(token, idx)
}

/// The bank's live-API ids, `BankButtonIDToInvSlotID`'s two bands: 24 vault slots, 6 bag slots.
const BANK_INV_SLOTS: std::ops::RangeInclusive<usize> = 40..=63;
const BANK_BAG_INV_SLOTS: std::ops::RangeInclusive<usize> = 64..=69;

/// The vault's container id; must match `ui_items::BANK_CONTAINER` in the app.
const BANK_CONTAINER: i64 = -1;

impl Model {
    /// The item `token` exposes at live-API id `slot`, the one routing the `GetInventoryItem*`
    /// getters and `GameTooltip:SetInventoryItem` share. Stock paints the bank through this API
    /// (`BankFrame.lua:35`), so the bank band is answered from the container snapshot.
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

    fn bank_inv_slot(&self, slot: usize) -> Option<InvSlotView> {
        if BANK_INV_SLOTS.contains(&slot) {
            let vault = self.containers.get(&BANK_CONTAINER)?;
            let n = (slot - BANK_INV_SLOTS.start() + 1) as u32;
            return vault.slots.get(&n).map(InvSlotView::from_container_slot);
        }
        if BANK_BAG_INV_SLOTS.contains(&slot) {
            // A bank bag is a container, not a slot in one, so this band is its own store; stock
            // picks it up and describes it through this API.
            let i = slot - BANK_BAG_INV_SLOTS.start();
            return self.bank_bag_slots.get(i)?.clone();
        }
        None
    }
}

/// The inventory-slot reader's whitelist (`0x4c8520`) on its 0-based slot (`0x4c8546`); outside
/// it the binding raises "Invalid inventory slot in …". The backpack's item slots (23..=38) and
/// buyback (69..=80) are container-API slots, not in it.
fn inventory_slot_reader_accepts(slot0: i32) -> bool {
    slot0 == -1                            // Lua 0      the ammo leg
        || (0x00..=0x16).contains(&slot0)  // Lua 1..=23   the doll + the four equipped bags
        || (0x27..=0x3e).contains(&slot0)  // Lua 40..=63  the bank vault
        || (0x3f..=0x44).contains(&slot0)  // Lua 64..=69  the bank bag slots
        || (0x51..=0x70).contains(&slot0) // Lua 82..=113 the keyring
}

fn link_item_name(link: &str) -> Option<String> {
    let (_, rest) = link.split_once("|h[")?;
    let (name, _) = rest.split_once("]|h")?;
    Some(name.to_string())
}

impl InvSlotView {
    /// A container slot seen through the inventory API, for `Model::inv_slot`'s bank band.
    fn from_container_slot(slot: &super::container::ContainerSlot) -> Self {
        InvSlotView {
            item_id: slot.item_id,
            bar_placeable: slot.bar_placeable,
            icon: slot.texture.clone(),
            count: slot.count,
            quality: slot.quality.map_or(0, |q| q as i32),
            // A container slot has no name, but its link is built from it.
            name: slot.link.as_deref().and_then(link_item_name),
            durability: slot.durability,
            already_bound: slot.already_bound,
            link: slot.link.clone(),
            locked: slot.locked,
            creator: slot.creator.clone(),
            enchants: slot.enchants.clone(),
            // Slot 20 (`Bag0Slot`) among `equip_slots` marks a container (`INVTYPE_BAG`). The bank
            // vault is past the `0x16` short-circuit, so only `Some` versus `None` matters.
            contents_count: slot.equip_slots.contains(&20).then_some(0),
            ..Default::default()
        }
    }
}

/// The reference's verbatim index-arm strings, pushed at `0x5187d6` (`0x85117c`) and `0x5185cb`
/// (`0x85112c`): no `Usage:` prefix, the index not interpolated. The pool interleaves the two
/// bindings' strings, so each is fixed by its `push`, never by adjacency.
const STAT_INDEX_ERROR: &str = "Invalid stat index in UnitStat";
const RESISTANCE_INDEX_ERROR: &str = "Invalid resistance index in UnitResistance";
/// The argument-type arm, pushed at `0x5187ed` (`0x851158`) and `0x5185e2` (`0x8510fc`), for a
/// value neither a number nor a numeric string ([`binding_abi`]).
const STAT_USAGE: &str = "Usage: UnitStat(\"unit\", statIndex)";
const RESISTANCE_USAGE: &str = "Usage: UnitResistance(\"unit\", resistanceIndex)";

/// Register the paper-doll stat/slot globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // UnitStat(unit, 1..=5) → (stat, effectiveStat, posBuff, negBuff): `UNIT_FIELD_STAT0+i` raw
    // (`0x518689`), then clamped at zero (`0x5186a3`-`0x5186b7`), then `POSSTAT0+i` and
    // `NEGSTAT0+i` behind a SELF gate (`0x518712`, `0x518772`). Unlike `UnitResistance` it is not
    // decomposed: stock subtracts the buffs itself (`PaperDollFrame.lua:152`). A bad index raises.
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

    // UnitResistance(unit, 0..=6) → (base, resistance, positive, negative) through `0x5efcd0`,
    // school 0 armor: `base = raw - pos - neg` before the clamp, `resistance = max(raw, 0)`.
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

    // UnitArmor(unit) → (base, effectiveArmor, armor, posBuff, negBuff): school 0 through
    // `0x5efcd0`, effectiveArmor and armor both its clamped total.
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

    // UnitDamage(unit) → (minDamage, maxDamage, minOffHandDamage, maxOffHandDamage,
    // physicalBonusPos, physicalBonusNeg, percent): the damage fields verbatim, then school 0's
    // `MOD_DAMAGE_DONE` split, which is inferred, not traced in the reference.
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

    // UnitAttackSpeed(unit) → (mainSpeed, offhandSpeed) in seconds; nil offhand without a weapon.
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

    // UnitAttackPower(unit) → (base, posBuff, negBuff): `UNIT_FIELD_ATTACK_POWER` and the signed
    // halves of `_MODS` (vmangos `StatSystem.cpp:335-336`).
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

    // UnitRangedAttackPower(unit) → (base, posBuff, negBuff), from the ranged fields.
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

    // UnitAttackBothHands(unit) → (mainBase, mainMod, offBase, offMod): `0x518810` calls
    // `[vtbl+0xb0]` for hand 0 then hand 1. Stock reads only the first pair
    // (`PaperDollFrame.lua:398-399`). A pet's `CGUnit_C` body (`0x6136b0`) ignores the hand, so
    // both pairs are `(level * 5, 0)`.
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

    // UnitRangedAttack(unit) → (base, modifier), the ranged weapon's skill pair.
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

    // UnitDefense(unit) → (base, modifier); stock splits the modifier's sign itself
    // (`PaperDollFrame.lua:259-271`). The gate is SELF or `UNIT_FIELD_SUMMONEDBY` = us, then
    // `[vtbl+0xac]`: `CGPlayer_C` `0x5eda20` reads the Defense skill (`0x6de040`), `CGUnit_C`
    // `0x613680` answers level * 5 and 0. Any other token gets the gate's zeros.
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

    // GetDodgeChance(), GetParryChance(), GetBlockChance() → one number each, no unit argument
    // (`0x516f00`, `0x516fc0`, `0x516f60`); 0 before the field streams.
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

    // UnitRangedDamage(unit) → (speed, minDamage, maxDamage, physicalBonusPos, physicalBonusNeg,
    // percent), with `UnitDamage`'s school-0 mods.
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

    // HasWandEquipped() → 1 or nil, for the player.
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

    // GetInventoryItemID(unit, slot) → itemId or nil.
    g.set(
        "GetInventoryItemID",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot) {
                Some(v) if v.item_id != 0 => Ok(Value::Integer(i64::from(v.item_id))),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetInventoryItemTexture(unit, slot) → icon path, or nil until the item's template arrives.
    g.set(
        "GetInventoryItemTexture",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot).and_then(|v| v.icon) {
                Some(icon) => Ok(Value::String(lua.create_string(&icon)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetInventoryItemLink(unit, slot) → `|cff…|Hitem:…|h[Name]|h|r`, or nil until the template
    // arrives with the name and quality.
    g.set(
        "GetInventoryItemLink",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot).and_then(|v| v.link) {
                Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // The reference's verbatim strings: `0x8489b4` when arg 1 is neither number nor string
    // (`0x6f3510`), `0x848984` when arg 2 is not a number or not a whitelisted slot.
    const USAGE_GET_INVENTORY_ITEM_COUNT: &str = "Usage: GetInventoryItemCount(unit, slot)";
    const INVALID_SLOT_GET_INVENTORY_ITEM_COUNT: &str =
        "Invalid inventory slot in GetInventoryItemCount";
    // GetInventoryItemCount(unit, slot) (`0x4c8680`): an empty slot pushes 1 (`0x4c8797`), a
    // non-container its stack count, a container past 0-based slot `0x16` 0 before any lookup
    // (`0x4c87af`, so a bank bag shows no digit), and an equipped bag its `contents_count`.
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

    // GetInventoryItemQuality(unit, slot) → 0..6, or nil for an empty slot (the wiki's shape).
    g.set(
        "GetInventoryItemQuality",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot) {
                Some(v) if v.item_id != 0 => Ok(Value::Integer(i64::from(v.quality))),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetInventoryItemBroken(unit, slot) → 1 or nil, per `slot_is_broken`.
    g.set(
        "GetInventoryItemBroken",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            match player_inv_slot(lua, &token, slot) {
                Some(v) if slot_is_broken(&v) => Ok(Value::Integer(1)),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetInventoryItemCooldown(unit, slot) → (start, duration, enable). Not fed: with no
    // equipped-item cooldown source it answers `(0, 0, 1)`, no cooldown, as the container verb
    // does; stock calls it on every slot update (`PaperDollFrame.lua:691`).
    g.set(
        "GetInventoryItemCooldown",
        lua.create_function(|_, (_token, _slot): (Option<String>, i64)| {
            Ok((0.0f64, 0.0f64, 1i64))
        })?,
    )?;

    // GetInventoryAlertStatus(index) → the status of region 1..=12: 0 none, 3 damaged or low ammo,
    // 4 broken.
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

    // OffhandHasWeapon() → 1 or nil: whether the off hand holds a weapon (item class 2).
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

    // GetInventorySlotInfo(slotName) → (slotId, textureName, checkRelic) from `SLOT_INFO`.
    // `checkRelic` is a property of the slot; stock pairs it with `UnitHasRelicSlot("player")`
    // (`PaperDollFrame.lua:680`, `:744`).
    g.set(
        "GetInventorySlotInfo",
        lua.create_function(|lua, name: String| {
            // A full-string ASCII case-insensitive match: `0x4c8215` calls `_strnicmp` (`0x64a4c0`
            // -> `0x414310`) with no length limit. The 36 names stay distinct after folding.
            let Some((_, id, art)) = SLOT_INFO
                .iter()
                .find(|(n, _, _)| n.eq_ignore_ascii_case(&name))
            else {
                // An unknown name raises, never nil; the string is `0x848894`'s, verbatim.
                return Err(mlua::Error::runtime(
                    "Invalid inventory slot in GetInventorySlotInfo",
                ));
            };
            Ok((
                *id,
                // The DBC's bytes verbatim, unnormalised (`0x4c825b`): a lowercase directory and
                // the `.blp` extension, visible to any Lua that compares the string.
                Value::String(
                    lua.create_string(format!(
                        "interface\\paperdoll\\UI-PaperDoll-Slot-{art}.blp"
                    ))?,
                ),
                // `checkRelic`: the number 1 for `RangedSlot` only (`0x4c8263`), not a boolean.
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

    /// A pet's snapshot: the UNIT fields filled, the PLAYER-block ones at their defaults.
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
        for sub in [9u32, 11, 12, 14, 21, 100] {
            assert_eq!(weapon_subclass_skill(sub), None, "subclass {sub}");
        }
    }

    /// The values are exact in `f32`, so the assertions compare equal.
    #[test]
    fn the_avoidance_verbs_are_player_implicit_and_answer_one_number() {
        let mut s = UiScript::new().unwrap();
        // Before anything streams: 0, not nil.
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
        // One return, and a unit argument is ignored.
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
        // Str: raw 25 in both first slots; stock subtracts the +4 itself.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 1)"#)
                .unwrap(),
            (25, 25, 4, 0)
        );
        // Agi: raw 20 with a −2 debuff.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 2)"#)
                .unwrap(),
            (20, 20, 0, -2)
        );
        // Only the second return is clamped.
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
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("target", 1)"#)
                .unwrap(),
            (0, 0, 0, 0)
        );
    }

    /// The index truncates toward zero (`_ftol`) ahead of the range test (`0x51865a`,
    /// `0x518663`): 1.9 is stat 1, while 0.5 and any negative raise.
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
            assert!(!msg.contains("Usage:"), "no Usage: prefix on the index arm");
        }
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", 1.9)"#)
                .unwrap(),
            (25, 25, 4, 0)
        );
        // A numeric string is a number.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("player", "2")"#)
                .unwrap(),
            (20, 20, 0, -2)
        );
        // Anything else takes the Usage arm.
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
        // A missing unit fails the same way: `0x6f3510` sees neither number nor string.
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
        // Arcane cursed to −5: the displayed total clamps to 0, `base` keeps the pre-clamp split.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitResistance("player", 6)"#)
                .unwrap(),
            (0, 0, 0, -5)
        );
        // UnitArmor is school 0, effectiveArmor = armor = the total.
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("player")"#)
                .unwrap(),
            (130, 150, 150, 30, -10)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, i64)>(r#"return UnitArmor("target")"#)
                .unwrap(),
            (0, 0, 0, 0, 0)
        );
        // Out of range raises `UnitResistance`'s own string, not the adjacent `UnitStat` one.
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
        // School 0 is in range, unlike `UnitStat`'s index 0.
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
        // No off-hand weapon: the second return is nil.
        assert!(s
            .eval::<bool>(r#"local m, o = UnitAttackSpeed("player") return m == 2.9 and o == nil"#)
            .unwrap());
        s.set_player_combat_stats(Some(UnitCombatStats {
            has_offhand: true,
            ..stats()
        }));
        assert_eq!(
            s.eval::<(f64, f64)>(r#"return UnitAttackSpeed("player")"#)
                .unwrap(),
            (2.9, 1.5)
        );
        // Another token, or no snapshot: zeros and percent 1.0.
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

    /// `UnitDefense` (`0x519200`) and `UnitAttackBothHands` (`0x518810`) fork through the unit's
    /// vtable to a creature body; `UnitRangedAttack` (`0x518b90`) is a direct call gated on the
    /// PLAYER typemask (`0x612b40`, `0x5edae0`, `0x5ea460` are in no vtable), so a pet gets zeros.
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
        // Another player gets zeros too.
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitRangedAttack("target")"#)
                .unwrap(),
            (0, 0)
        );
    }

    /// `0x518810` pushes `(base0, mod0, base1, mod1)`, one pair per hand.
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
        // The per-hand clamp (`0x5188a7`), which `UnitDefense` shares.
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

    #[test]
    fn the_pet_token_reads_the_pet_snapshot_and_only_it() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(stats()));
        s.set_pet_combat_stats(Some(pet_stats()));

        // Stamina: the pet's 68, the player's 22.
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
        // Str, where the player carries a buff the pet must not inherit.
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
        // Fire resistance: the pet's 15, the player's 20 (+25/−5).
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
        // The rest routes too; the pet's `percent` stays 1.0.
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
        assert!(s
            .eval::<bool>(r#"local m, o = UnitAttackSpeed("pet") return m == 2.0 and o == nil"#)
            .unwrap());

        // A third token is still the absent shape.
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
        // So is `"pet"` once the pet is dismissed (the feed pushes `None`).
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

    /// The vtable fork (`0x5eda20`, `0x613680`): the player's skill pair, a pet's level * 5 with a
    /// 0 modifier; `UnitAttackBothHands` takes the same fork.
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
        // The pet's snapshot carries neither pair; the numbers above come from its level.
        assert_eq!(pet_stats().defense_skill, (0, 0));
        assert_eq!(pet_stats().main_weapon_skill, (0, 0));
        // A pet with no level fed yet reads 0.
        s.set_unit("pet", None);
        assert_eq!(
            s.eval::<(i64, i64)>(r#"return UnitDefense("pet")"#)
                .unwrap(),
            (0, 0)
        );
        // A token failing the SELF-or-SUMMONEDBY gate gets zeros, not its level.
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
        // A negative modifier survives; stock paints it red.
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

    /// `0x4c8680`'s container fork: an equipped quiver counts its arrows, a plain bag 0, and any
    /// container in a bank bag slot 0 before the DBC is read.
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

        // Both arms raise. The backpack's item slots (Lua 24..=39) are outside the whitelist.
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
        // An empty slot: nil id, texture and quality, and count 1 (`0x4c8797`).
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
        // A token with no items behind it: the empty shape.
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
        // The bag rows, 20..23 and 64..75, share one texture string.
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
        assert!(s
            .eval::<i64>(r#"return GetInventorySlotInfo("NoSuchSlot")"#)
            .is_err());
    }

    /// Each paper doll's facing is its pane's own `SetRotation` state, so turning one leaves the
    /// other alone, as stock `Model_RotateLeft(model)` expects.
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
        // Stock's default, as `Model_OnLoad` writes it (`UIParent.lua:1422`).
        s.run("CharacterModelFrame:SetRotation(0.61)").unwrap();
        assert_eq!(s.model_pane_facing("CharacterModelFrame"), 0.61);
        // Persistent, not a drain.
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

        // An undeclared name reads 0.0: the app samples this before the window loads.
        assert_eq!(s.model_pane_facing("NoSuchModelFrame"), 0.0);
        // A declared frame that is not a model pane has no pane.
        s.run(r#"CreateFrame("Frame", "NotAPane")"#).unwrap();
        assert!(s.model_pane("NotAPane").is_none());
    }
}
