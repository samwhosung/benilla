//! The paper doll's data feed: the stat and equipment-slot globals. The stat getters read the
//! unit's object as the call runs, through the lent host ([`super::stat_read`]), and with no host
//! to ask, the combat-stats snapshots the app pushes for the active player and with each unit; the
//! slot getters read the inventory-slot snapshot and its items live ([`super::item_read`]).
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

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::item_read::{read_item, read_live};
use super::stat_read::{read_own_stats, read_unit_stats, CombatRead, UnitCombatStats};
use super::{binding_abi, Model};

/// The `SkillLine.dbc` id behind weapon subclass `0..=20` (vmangos `Item.cpp:700-707`), `None`
/// for the obsolete, exotic and misc rows; with no weapon the skill is [`SKILL_UNARMED`]
/// (`Player.cpp:20175-20186`).
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

/// The melee skill with no weapon equipped (vmangos `SharedDefines.h:987`, `Player.cpp:20184`).
pub const SKILL_UNARMED: u32 = 162;

/// The `SkillLine.dbc` Defense row `UnitDefense` reports (vmangos `SharedDefines.h:961`).
pub const SKILL_DEFENSE: u32 = 95;

/// One equipment or ammo slot as the `GetInventoryItem*` family reads it, resolved by the app like
/// [`super::container::ContainerSlot`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InvSlotView {
    /// The item's template entry, the id inside `GetInventoryItemLink`'s link.
    pub item_id: u32,
    /// Whether the item may go on an action bar, by the same filter as a bag slot's.
    pub bar_placeable: bool,
    /// The action bar's count gate, as a bag slot's ([`super::container::ContainerSlot`]).
    pub consumable: bool,
    /// Icon texture path (`Interface\Icons\…`); `None` while the template answer is in flight.
    pub icon: Option<String>,
    /// `ITEM_FIELD_STACK_COUNT`, or for the ammo slot the bag-summed carried total.
    pub count: u32,
    /// For a container (`TYPEMASK_CONTAINER`, `0x4c87a6`), what `GetInventoryItemCount` pushes:
    /// its contents' summed stacks when its `ItemSubClass.dbc` row has `DisplayFlags & 0x4` (Soul
    /// Bag and quivers, `0x4c881a`), else 0; past 0-based slot `0x16` the binding answers 0 first.
    pub contents_count: Option<u32>,
    /// Item quality 0..6.
    pub quality: i32,
    /// The record's inventory type (`+0x2c`), `None` while the record is not cached: what
    /// `GetInventoryItemQuality` tests before the quality (`0x4c8a11`, `0x4c8a16`).
    pub inventory_type: Option<u32>,
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
    /// The template's bonding (`[rec+0x194]`), 0 while unresolved: the equip arms ask on 2.
    pub bonding: u32,
    /// For a container, how many of its seats hold an item, as
    /// [`super::container::ContainerSlot::occupied_seats`].
    pub occupied_seats: Option<u32>,
    /// The on-use spell the dispatcher reads (`0x5d8c80`), 0 for none or while unresolved.
    pub use_spell: u32,
    /// The template's block 0 names a spell (`+0x11c`, any trigger): `GetInventoryItemCooldown`
    /// runs no query without one (`0x4c8b47`).
    pub spell_0: bool,
    /// The worn object's `ITEM_FIELD_SPELL_CHARGES`, per spell block (`flds+0x28`), which its
    /// tooltip's charges lines read (`0x52d9e9`); zeros for a visible-item record.
    pub spell_charges: [i32; 5],
    /// The arm `UseInventoryItem` takes in the dispatcher, with no equip fork; `link` is set once
    /// the template is cached, which the dispatcher needs (`0x5d8d5c`).
    pub use_fork: super::container::UseFork,
    /// An `|Hitem:…|h[Name]|h` link once the name is known.
    pub link: Option<String>,
    /// The link `0x52ae00` builds of an own item object whose record the cache lacks, white and
    /// nameless (`0x52aec5`, `0x5d8b40`), which `GetInventoryItemLink` answers (`0x4c8ce2`); `None`
    /// once [`Self::link`] is set, and for another player's visible item, which links nothing
    /// without its record (`0x4c8d3d`).
    pub uncached_link: Option<String>,
    /// The item object's guid, 0 while unresolved: `IsInventoryItemLocked` reads the client's lock
    /// on it.
    pub guid: u64,
    /// For a slot with no item object (the ammo slot, an inspected player's visible item), the
    /// roll its record shows with: the name's suffix and the link's numbers.
    pub roll: super::ItemRoll,
    /// The 1-based live-API slots this item could be equipped into; empty if none.
    pub equip_slots: Vec<u8>,
    /// The resolved name of [`Self::creator_guid`], the tooltip's "<Made by %s>" line.
    pub creator: Option<String>,
    /// The resolved `ITEM_FIELD_GIFTCREATOR` name, the tooltip's `ITEM_WRAPPED_BY` line.
    pub gift_creator: Option<String>,
    /// The `ITEM_FIELD_CREATOR` guid, or an inspected slot's `PLAYER_VISIBLE_ITEM_<slot>_CREATOR`
    /// (`0x5333cd`), 0 for none: with no [`Self::creator`] yet, the name the tooltip waits for.
    pub creator_guid: u64,
    /// The `ITEM_FIELD_GIFTCREATOR` guid, 0 for none, as [`Self::creator_guid`].
    pub gift_creator_guid: u64,
    /// The resolved enchant slots in slot order. An inspected player's record has all 7 too, but
    /// vmangos fills only PERM and TEMP (`Player.cpp:10594-10595`).
    pub enchants: Vec<super::EnchantView>,
    /// The instance's remaining lifetime in ms; `None` without a timer.
    pub duration_ms: Option<u64>,
    /// `ITEM_FIELD_DURATION`, as [`super::container::ContainerSlot::field_duration`].
    pub field_duration: u32,
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
fn slot_is_broken(v: &InvSlotView, durability: Option<(u32, u32)>) -> bool {
    if v.item_id == 0 || v.flags & 0x08 != 0 {
        return false;
    }
    v.flags & 0x10 != 0 || matches!(durability, Some((0, max)) if max > 0)
}

/// What `0x4c7ee0` reads of one worn item: `ITEM_FIELD_FLAGS` (`+0x3c`), `DURABILITY` (`+0xa0`)
/// and `MAXDURABILITY` (`+0xa4`); for the temporary-enchant arm, the record's flags (the `+0x110`
/// table's slot 0, `0x4c8039`, 0 while the record is not cached), the temporary enchantment's id
/// (`ITEM_FIELD_ENCHANTMENT` slot 1, `+0x4c`, `0x4c8043`) and its milliseconds left
/// (`0x5d9d00(1)`, `0x4c805b`: the deadline less now, 0 when unset or run out).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AlertItem {
    pub flags: u32,
    pub durability: u32,
    pub max_durability: u32,
    pub record_flags: u32,
    pub temp_enchant: u32,
    pub temp_enchant_left_ms: u32,
}

/// `0x4c7ee0`'s reads: the item `GetItemInSlot` (`0x6228a0`) finds in each of the 11 worn regions,
/// in `0x806eb8`'s order, and, while `PLAYER_AMMO_ID` is set, the carried count of that ammunition
/// (`0x622130(id, 0)`, `0x4c7f73`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AlertInputs {
    pub worn: [Option<AlertItem>; 11],
    pub ammo: Option<u32>,
}

/// The live question of the alert recompute: its reads as of here, `None` while no active player
/// is held, when the recompute does nothing (`0x4c7f06`).
pub struct InventoryAlertInputs;

impl super::live::LiveQuery for InventoryAlertInputs {
    type Answer = Option<AlertInputs>;
}

/// One worn region's status (`0x4c7faa`-`0x4c806d`): 4 when force-red (`0x10`); else, unless
/// wrapped (`0x08`) or without a maximum, 4 at durability 0 and 3 with 1..=5 points left (an
/// absolute count, not a fraction); else the temporary-enchant arm (`0x4c8035`): with record flag
/// `0x2000` clear (`0x4c803b`) and a temporary enchantment (`0x4c8046`), 1, or 2 with 30000 ms or
/// fewer left (`0x4c8060`, unsigned); else 0.
fn alert_status(item: Option<AlertItem>) -> u8 {
    let Some(v) = item else {
        return 0;
    };
    if v.flags & 0x10 != 0 {
        return 4;
    }
    if v.flags & 0x08 == 0 && v.max_durability != 0 {
        match v.durability {
            0 => return 4,
            1..=5 => return 3,
            _ => {}
        }
    }
    if v.record_flags & 0x2000 != 0 || v.temp_enchant == 0 {
        return 0;
    }
    if v.temp_enchant_left_ms <= 30_000 {
        2
    } else {
        1
    }
}

/// Each region's status off `inputs`, in `0x806eb8`'s order, with the tutorial its arm fires: a
/// region broken by durability fires Broken Items (`0x4c8002`) and a damaged one Damaged Items
/// (`0x4c802e`); the force-red arm (`0x4c7fb0`), the temporary-enchant arm and the ammo arm
/// (`0x4c7f78`: 3 at 20 or fewer) fire nothing.
fn alert_statuses(inputs: &AlertInputs) -> [(u8, Option<u32>); 12] {
    let mut out = [(0u8, None); 12];
    for (i, &item) in inputs.worn.iter().enumerate() {
        let status = alert_status(item);
        let forced = item.is_some_and(|v| v.flags & 0x10 != 0);
        let tutorial = match status {
            4 if !forced => Some(TUTORIAL_BROKEN_ITEMS),
            3 => Some(TUTORIAL_DAMAGED_ITEMS),
            _ => None,
        };
        out[i] = (status, tutorial);
    }
    out[11].0 = match inputs.ammo {
        Some(count) if count <= 20 => 3,
        _ => 0,
    };
    out
}

/// The doll snapshot's reads, for a VM with no host to ask: each worn region's view, and the ammo
/// view's count.
fn snapshot_alert_inputs(model: &Model) -> AlertInputs {
    let slots = &model.inventory_slots;
    let worn = std::array::from_fn(|i| {
        slots[ALERT_SLOTS[i]]
            .as_ref()
            .filter(|v| v.item_id != 0)
            .map(|v| {
                let (durability, max_durability) = v.durability.unwrap_or((0, 0));
                // The snapshot holds no temporary enchantment: its arm reads none.
                AlertItem {
                    flags: v.flags,
                    durability,
                    max_durability,
                    ..AlertItem::default()
                }
            })
    });
    let ammo = slots[ALERT_SLOTS[11]]
        .as_ref()
        .filter(|v| v.item_id != 0)
        .map(|v| v.count);
    AlertInputs { worn, ammo }
}

/// The recompute's reads through the lent host, as of here, else off the doll snapshot; `None`
/// while no active player is held.
fn alert_inputs(lua: &Lua) -> Option<AlertInputs> {
    match super::live::ask(lua, &InventoryAlertInputs) {
        Some(inputs) => inputs,
        None => {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Some(snapshot_alert_inputs(&model))
        }
    }
}

/// `0x4c7ee0`'s pass over the stored statuses (`0xb71f60`), region by region: each set to 0 and
/// recomputed (`0x4c7f1b`), its tutorial fired at once (`0x4b5390` with delay 0), so a
/// `TUTORIAL_TRIGGER` listener reads the regions before it recomputed and those after it as the
/// last pass left them; the caller signals. The caller holds no borrow of the model.
fn store_inventory_alerts(lua: &Lua, inputs: &AlertInputs) {
    for (i, (status, tutorial)) in alert_statuses(inputs).into_iter().enumerate() {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .inventory_alerts[i] = status;
        if let Some(id) = tutorial {
            super::tutorial::trigger_in_place(lua, id);
        }
    }
}

/// The alert recompute in a verb's call (`UpdateInventoryAlertStatus`, `0x4c9560`): the statuses
/// stored and `UPDATE_INVENTORY_ALERTS` raised at the pass's end (`0x4c8085`); nothing while no
/// active player is held.
pub(super) fn recompute_inventory_alerts_in_verb(lua: &Lua) {
    let Some(inputs) = alert_inputs(lua) else {
        return;
    };
    store_inventory_alerts(lua, &inputs);
    super::tick::verb(lua, |model| {
        model.raise("UPDATE_INVENTORY_ALERTS", Vec::new());
    });
}

impl super::UiScript {
    /// Push, or clear with `None`, the player's combat-stats snapshot.
    pub fn set_player_combat_stats(&mut self, stats: Option<UnitCombatStats>) {
        self.model_mut().player_combat_stats = stats;
    }

    /// The ammunition entries the ammo slot's texture and quality getters looked up and missed
    /// that a harness's VM kept ([`Self::keep_calls`]), one per call.
    pub fn take_ammo_record_asks(&mut self) -> Vec<u32> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::RecordAsk(super::RecordAsk::AmmoItem { entry, .. }) => Some(*entry),
            _ => None,
        })
    }

    /// Push the doll snapshot. The alert statuses are the recompute's alone
    /// ([`Self::recompute_inventory_alerts`]).
    pub fn set_inventory_slots(&mut self, slots: InventorySlots) {
        self.model_mut().inventory_slots = slots;
    }

    /// The alert recompute `0x4c7ee0`, where a field watcher, the world entry's set-up
    /// (`0x4c6d30`) or the verb runs it: its reads as of here through the lent host
    /// ([`InventoryAlertInputs`]), else off the doll snapshot; the 12 statuses stored, their
    /// tutorials, and `UPDATE_INVENTORY_ALERTS` on every pass, whether or not a status changed
    /// (`0x4c8085`). Nothing while no active player is held (`0x4c7f06`).
    pub fn recompute_inventory_alerts(&mut self) {
        let lua = self.lua().clone();
        let Some(inputs) = alert_inputs(&lua) else {
            return;
        };
        store_inventory_alerts(&lua, &inputs);
        self.fire_event("UPDATE_INVENTORY_ALERTS", vec![]);
    }

    /// Push the six bank-bag slots. The caller fires `PLAYERBANKSLOTS_CHANGED` for a changed item:
    /// the stock bank buttons repaint only on it and `BANKFRAME_OPENED` (`BankFrame.lua:206-212`).
    pub fn set_bank_bag_slots(&mut self, slots: BankBagSlots) {
        self.model_mut().bank_bag_slots = slots;
    }

    /// Take the `UseInventoryItem` calls out of the call stream; the app sends each as
    /// `CMSG_USE_ITEM`, bag 255 and the 0-based slot.
    pub fn take_inventory_uses(&mut self) -> Vec<u32> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::UseInventoryItem { slot, .. } => Some(*slot),
            _ => None,
        })
    }
}

/// The two alert tutorials' 0-based numbers.
const TUTORIAL_DAMAGED_ITEMS: u32 = 0x23;
const TUTORIAL_BROKEN_ITEMS: u32 = 0x24;

/// The unit a stat getter asks after `GetUnitFromToken` (`0x515940`), the gates' inputs beside
/// its fields; `None` is no object, whose answer is each function's zeros.
struct Asked<'a> {
    /// The fields read: the unit's object through the lent host as the call runs, the active
    /// player's with its PLAYER block and equipment; with no host to ask, the active player's own
    /// snapshot, else the unit's ([`super::UnitState::combat`]).
    fields: &'a dyn CombatRead,
    /// SELF: the object's GUID is the active player's (`0x468550`).
    me: bool,
    /// PLAYER: the type word's player bit (`shr ecx,4; test cl,1`).
    player: bool,
    /// SELF or `UNIT_FIELD_SUMMONEDBY` the active player's GUID (`0x519272`, `0x518881`), with
    /// no zero guard.
    me_or_mine: bool,
    /// `UNIT_FIELD_LEVEL`, which a creature's skill body multiplies by 5 (`0x613680`).
    level: u32,
}

/// [`Asked`] for `token`, after the resolver's raise for a token it does not know (`0x515c14`).
/// The `"player"` token's own push is the active player whatever its guid reads, as the resolver
/// names it. The token resolves through the snapshots; the fields, the level and SUMMONEDBY are
/// then read off the object the lent host holds ([`read_unit_stats`]), as the reference's
/// getters read the object they resolved, and with no host or no object, off the snapshots.
fn asked<T>(lua: &Lua, token: &str, f: impl FnOnce(Option<Asked<'_>>) -> T) -> mlua::Result<T> {
    super::unit::check_unit_token(&Some(token.to_string()))?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let player_guid = model.unit_guids.player;
    let player_push = model.pushed_unit("player");
    let Some(u) = super::unit::view(lua, &model, token).filter(|u| u.has_object) else {
        return Ok(f(None));
    };
    let u: &super::UnitState = &u;
    let me =
        player_push.is_some_and(|p| std::ptr::eq(p, u)) || (u.guid != 0 && u.guid == player_guid);
    let player = me || u.is_player;
    let mut f = Some(f);
    let live = read_unit_stats(lua, u.guid, |object, fields| {
        f.take().map(|f| {
            f(Some(Asked {
                fields,
                me,
                player,
                me_or_mine: me || object.summoned_by() == player_guid,
                level: object.level(),
            }))
        })
    })
    .flatten();
    if let Some(out) = live {
        return Ok(out);
    }
    let fields: &dyn CombatRead = match &model.player_combat_stats {
        Some(own) if me => own,
        _ => &u.combat,
    };
    Ok(f.take().expect("an unread `f`")(Some(Asked {
        fields,
        me,
        player,
        me_or_mine: me || u.summoned_by == player_guid,
        level: u.level,
    })))
}

/// [`asked`] behind the `Usage:` gate the one-argument getters open with (`0x6f3510`).
fn asked_arg<T>(
    lua: &Lua,
    token: Value,
    usage: &'static str,
    f: impl FnOnce(Option<Asked<'_>>) -> T,
) -> mlua::Result<T> {
    let token = binding_abi::string_arg(lua, token, usage)?;
    asked(lua, &token, f)
}

/// A swing time in seconds: the field, an integer, times the single 0.001 at `0x801360`
/// (`0x518eb8`, `0x51898f`), at the x87's double precision.
fn swing_seconds(ms: u32) -> f64 {
    f64::from(ms) * f64::from(0.001f32)
}

/// One attack power value as `UnitAttackPower` pushes it: `(1 + m) · x` at the x87's double
/// precision, stored as a single, then `fistp`'d to nearest even (`0x518fe2`-`0x518ffd`).
fn scaled_attack_power(m: f32, x: i32) -> i64 {
    (((f64::from(m) + 1.0) * f64::from(x)) as f32).round_ties_even() as i64
}

/// The damage-bonus triple `UnitDamage` and `UnitRangedDamage` push after the damage: the active
/// player's own; another player's the zero constant (`0x518cf7`, `0x518d7a`); a non-player's 0, 0
/// and 1.0 (`0x518dbc`-`0x518dd2`, `0x518b1d`-`0x518b33`).
fn damage_bonus(a: &Asked<'_>, own: (i32, i32, f32)) -> (i64, i64, f64) {
    match (a.player, a.me) {
        (false, _) => (0, 0, 1.0),
        (true, false) => (0, 0, 0.0),
        (true, true) => (i64::from(own.0), i64::from(own.1), f64::from(own.2)),
    }
}

/// The clamp `UnitDefense` (`0x519298`) and each hand of `UnitAttackBothHands` (`0x5188a7`) apply:
/// a modifier below `-base` becomes `-base`, so the total never reads negative.
fn skill_clamped((base, modifier): (i32, i32)) -> (i64, i64) {
    let base = i64::from(base);
    let modifier = i64::from(modifier);
    (base, if modifier + base < 0 { -base } else { modifier })
}

/// The skill pair `UnitDefense` and each hand of `UnitAttackBothHands` ask through the vtable
/// (`[vtbl+0xac]`, `[vtbl+0xb0]`) behind SELF-or-mine: the active player's skill line
/// (`0x5eda20`), a creature's level times 5 and 0 (`0x613680`, `0x6136b0`); anyone else's zeros.
fn vtable_skill(a: Option<&Asked<'_>>, own: (i32, i32)) -> (i32, i32) {
    match a {
        Some(a) if a.me => own,
        Some(a) if a.me_or_mine => ((a.level as i32).wrapping_mul(5), 0),
        _ => (0, 0),
    }
}

/// `token`'s inventory slot, unit-keyed as in the reference so the inspect doll reuses these
/// bindings: `"player"` reads our own items, the inspected unit its public
/// `PLAYER_VISIBLE_ITEM_*` entries, which carry no counts or durability.
fn player_inv_slot(lua: &Lua, token: &Option<String>, slot: i64) -> Option<InvSlotView> {
    let token = token.as_deref()?;
    let idx = usize::try_from(slot).ok()?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let reader = inventory_reader(lua, &model, token);
    model.inv_slot_as(lua, reader?, idx)
}

/// [`player_inv_slot`] for the getters that read the unit's item table (`0x6228a0`), which only
/// the active player's holds: another player's slot is empty.
fn own_inv_slot(lua: &Lua, token: &Option<String>, slot: i64) -> Option<InvSlotView> {
    let own = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        inventory_reader(lua, &model, token.as_deref()?) == Some(InvReader::Own)
    };
    own.then(|| player_inv_slot(lua, token, slot)).flatten()
}

/// The ammo slot's record lookup with the callback `0x4c84f0` (`0x4c83bc`, `0x4c89cf`): a view of
/// the active player's ammunition whose record the cache lacks asks inside the call and registers
/// one callback per call.
fn ask_ammo_record(
    lua: &Lua,
    token: &Option<String>,
    slot: i64,
    view: Option<&InvSlotView>,
) -> mlua::Result<()> {
    let Some(v) = view.filter(|v| slot == 0 && v.item_id != 0 && v.name.is_none()) else {
        return Ok(());
    };
    // The ammo leg reads the active player's field alone (`0x4c8382`), and passes the unit's
    // guid, the active player's, as the owner id (`0x4c83ad`-`0x4c83c4`).
    let owner = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let own = token
            .as_deref()
            .and_then(|t| inventory_reader(lua, &model, t))
            == Some(InvReader::Own);
        own.then_some(model.unit_guids.player)
    };
    let Some(owner) = owner else {
        return Ok(());
    };
    super::calls::ask(
        lua,
        super::RecordAsk::AmmoItem {
            entry: v.item_id,
            owner,
        },
    )
}

/// The bank's live-API ids, `BankButtonIDToInvSlotID`'s two bands: 24 vault slots, 6 bag slots.
const BANK_INV_SLOTS: std::ops::RangeInclusive<usize> = 40..=63;
const BANK_BAG_INV_SLOTS: std::ops::RangeInclusive<usize> = 64..=69;

/// The vault's container id; must match `ui_items::BANK_CONTAINER` in the app.
pub(super) const BANK_CONTAINER: i64 = -1;

/// The keyring's live-API ids: `KeyRingButtonIDToInvSlotID` adds `0x51` to a 1-based button id
/// (`0x4c818b`), and the slot reader takes 0-based 81..=112 (`0x4c8567`).
pub(super) const KEYRING_INV_SLOTS: std::ops::RangeInclusive<usize> = 82..=113;

/// The keyring's container id (`KEYRING_CONTAINER`, `MainMenuBarBagButtons.lua:1`); must match
/// `ui_items::KEYRING_CONTAINER` in the app, which feeds the keys as that container.
pub(super) const KEYRING_CONTAINER: i64 = -2;

/// Whose items a slot getter reads after the token's resolve (`0x515940`) and
/// `CanReadUnitInventory` (`0x4944a0`): the active player's own table, or another player's
/// visible items, a player it can cooperate with within 10 yards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InvReader {
    Own,
    Visible(u64),
}

/// [`InvReader`] for `token`, `None` for a unit the getters may not read or no unit at all, the
/// token resolved at the call ([`super::unit::guid_of`]). `"player"` is the active player whatever
/// its guid reads, and none while no active player is held (`0x4944c9`), as in a
/// `PLAYER_LOGOUT` handler, which runs after the objects are freed.
pub(super) fn inventory_reader(lua: &Lua, model: &Model, token: &str) -> Option<InvReader> {
    if token.eq_ignore_ascii_case("player") {
        return active_player_held(lua).then_some(InvReader::Own);
    }
    let guid = super::unit::guid_of(lua, model, token).ok().flatten()?;
    if guid == model.unit_guids.player {
        return active_player_held(lua).then_some(InvReader::Own);
    }
    let reach = super::inspect::reach_of(lua, model, guid)?;
    (reach.inspectable && super::inspect::in_inspect_range(&reach))
        .then_some(InvReader::Visible(guid))
}

/// Whether an active player is held now (`0x4944af` → `0x468460`): through the lent host, the
/// VM's copy standing in without one. Borrows no [`Model`], so a caller may hold it.
pub(super) fn active_player_held(lua: &Lua) -> bool {
    let mut held = None;
    super::host::read_host(lua, |h| h.read_self(&mut |own| held = Some(own.held())));
    held.unwrap_or(true)
}

impl Model {
    /// The item `reader`'s unit exposes at live-API id `slot`, the one routing the
    /// `GetInventoryItem*` getters and `GameTooltip:SetInventoryItem` share. Stock paints the bank
    /// through this API (`BankFrame.lua:35`) and hovers a key through it
    /// (`ContainerFrame.lua:616-619`), so the bank and keyring bands are answered from the
    /// container slots. Another player answers its visible items alone, slots 1-19
    /// (`0x5f0d60`), read through the lent host ([`super::inspect::visible_slot`]).
    pub(super) fn inv_slot_as(
        &self,
        lua: &Lua,
        reader: InvReader,
        slot: usize,
    ) -> Option<InvSlotView> {
        match reader {
            InvReader::Own => own_inv_view(lua, self, slot),
            InvReader::Visible(guid) => (1..=19)
                .contains(&slot)
                .then(|| super::inspect::visible_slot(lua, self, guid, slot))
                .flatten(),
        }
    }

    /// The player's repair cost at live id `slot` now (`0x5332e1`), the vault and keyring bands by
    /// their container slots.
    pub(super) fn inv_repair_cost(&self, lua: &mlua::Lua, slot: usize) -> u32 {
        let (bag, n) = if BANK_INV_SLOTS.contains(&slot) {
            (BANK_CONTAINER, slot - BANK_INV_SLOTS.start() + 1)
        } else if KEYRING_INV_SLOTS.contains(&slot) {
            (KEYRING_CONTAINER, slot - KEYRING_INV_SLOTS.start() + 1)
        } else {
            (super::EQUIPMENT_BAG, slot)
        };
        let Ok(n) = u32::try_from(n) else {
            return 0;
        };
        super::merchant::repair_cost_at(lua, self, bag, n)
    }
}

/// The active player's item at live-API id `slot`. The bank bands are the player list's slots 39
/// to 68, which `GetItemInSlot` (`0x6228a0`) answers empty while no banker is open (`0x6228ad`-
/// `0x6228cf`, the banker guid `[0xbdd038]`), and every getter here reads through it (`0x4c8449`,
/// `0x4c8cd3`, `SetInventoryItem` at `0x53314e`).
pub(super) fn own_inv_view(lua: &Lua, model: &Model, slot: usize) -> Option<InvSlotView> {
    let bank_band = BANK_INV_SLOTS.contains(&slot) || BANK_BAG_INV_SLOTS.contains(&slot);
    if bank_band && model.bank.is_none() {
        return None;
    }
    player_slot_object(lua, model, slot)
}

/// The item object at the active player's live-API id `slot`, with no bank gate: what the client
/// finds by the held item's guid (the object manager, not `GetItemInSlot`), so an item held from
/// a bank slot is still found after the banker closes. For the readers that stand in for that
/// guid lookup; a slot getter reads [`Model::inv_slot_as`].
pub(crate) fn player_slot_object(lua: &Lua, model: &Model, slot: usize) -> Option<InvSlotView> {
    if let Some(s) = inv_container_slot(lua, model, slot) {
        return Some(InvSlotView::from_container_slot(&s));
    }
    if BANK_INV_SLOTS.contains(&slot) || KEYRING_INV_SLOTS.contains(&slot) {
        return None;
    }
    if BANK_BAG_INV_SLOTS.contains(&slot) {
        // A bank bag is a container, not a slot in one, so this band is its own store; stock
        // picks it up and describes it through this API.
        let i = slot - BANK_BAG_INV_SLOTS.start();
        return model.bank_bag_slots.get(i)?.clone();
    }
    model.inventory_slots.get(slot)?.clone()
}

/// The bag-slot item object behind the player's live id `slot` in the vault and keyring bands,
/// which `SetInventoryItem` resolves through the player's slot list as it does a worn item
/// (`0x53314e` → `0x6228a0`), as it stands now; `None` for every other band.
pub(super) fn inv_container_slot(
    lua: &Lua,
    model: &Model,
    slot: usize,
) -> Option<super::container::ContainerSlot> {
    let (bag, first) = if BANK_INV_SLOTS.contains(&slot) {
        (BANK_CONTAINER, *BANK_INV_SLOTS.start())
    } else if KEYRING_INV_SLOTS.contains(&slot) {
        (KEYRING_CONTAINER, *KEYRING_INV_SLOTS.start())
    } else {
        return None;
    };
    let n = (slot - first + 1) as u32;
    super::held::held_slot(lua, model, bag, n)
}

/// The use cooldown `(start, duration, enabled)` in `GetTime` seconds of the player's item at
/// live id `slot`: the vault and keyring bands by their container slots, the doll's from the worn
/// copy a harness seats.
pub(super) fn inv_slot_cooldown(lua: &Lua, model: &Model, slot: usize) -> Option<(f64, f64, bool)> {
    if BANK_INV_SLOTS.contains(&slot) || KEYRING_INV_SLOTS.contains(&slot) {
        inv_container_slot(lua, model, slot)
            .as_ref()
            .and_then(super::container::slot_cooldown)
    } else {
        model.worn_cooldowns.get(&slot).copied()
    }
}

/// The inventory-slot reader's whitelist (`0x4c8520`) on its 0-based slot (`0x4c8546`); outside
/// it the binding raises "Invalid inventory slot in …". The backpack's item slots (23..=38) and
/// buyback (69..=80) are container-API slots, not in it.
pub(super) fn inventory_slot_reader_accepts(slot0: i32) -> bool {
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
    /// The item the getters read as they run: the slot's object, else, for the ammo slot and an
    /// inspected player's visible item, its record with the slot's roll.
    pub(crate) fn item_key(&self) -> Option<super::ItemKey> {
        if self.guid != 0 {
            Some(super::ItemKey::Object(self.guid))
        } else {
            (self.item_id != 0).then_some(super::ItemKey::Record {
                entry: self.item_id,
                roll: self.roll,
            })
        }
    }

    /// An item object whose record the cache does not hold yet, which the app pushes with its
    /// entry and no name: `SetInventoryItem`'s cooldown query on it asks the record with the
    /// callback `0x6e2f70` (`0x533188` → `0x5331ec`, `0x6e2efb`).
    pub fn record_pending(&self) -> bool {
        self.item_id != 0 && self.name.is_none()
    }

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
            bonding: slot.bonding,
            occupied_seats: slot.occupied_seats,
            use_spell: slot.use_spell,
            spell_0: slot.spell_0,
            spell_charges: slot.spell_charges,
            link: slot.link.clone(),
            uncached_link: slot.uncached_link.clone(),
            guid: slot.guid,
            creator: slot.creator.clone(),
            gift_creator: slot.gift_creator.clone(),
            creator_guid: slot.creator_guid,
            gift_creator_guid: slot.gift_creator_guid,
            enchants: slot.enchants.clone(),
            // The item's own flags (UNLOCKED, WRAPPED) and expiry, which the tooltip builder reads.
            flags: slot.flags,
            duration_ms: slot.duration_ms,
            field_duration: slot.field_duration,
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
            // Both arguments' gates, then the resolver (`0x51863f`), then the index (`0x51865a`).
            let token = binding_abi::string_arg(lua, token, STAT_USAGE)?;
            let idx = binding_abi::number_arg(lua, i, STAT_USAGE)? - 1;
            super::unit::check_unit_token(&Some(token.clone()))?;
            if !(0..5).contains(&idx) {
                return Err(mlua::Error::RuntimeError(STAT_INDEX_ERROR.into()));
            }
            let idx = idx as usize;
            asked(lua, &token, |a| {
                let Some(a) = a else {
                    return (0, 0, 0, 0);
                };
                let s = a.fields;
                let raw = s.stats(idx);
                let (pos, neg) = if a.me {
                    (s.stat_pos(idx), s.stat_neg(idx))
                } else {
                    (0, 0)
                };
                (
                    i64::from(raw),
                    i64::from(raw.max(0)),
                    i64::from(pos),
                    i64::from(neg),
                )
            })
        })?,
    )?;

    // UnitResistance(unit, 0..=6) → (base, resistance, positive, negative). The active player's
    // through `0x5efcd0`: `base = raw - pos - neg` before the clamp, `resistance = max(raw, 0)`;
    // any other unit's the field raised to 0 twice and no buffs (`0x518562`-`0x51857c`).
    g.set(
        "UnitResistance",
        lua.create_function(|lua, (token, school): (Value, Value)| {
            // Both arguments' gates, then the resolver (`0x5184df`), then the index (`0x5184fd`).
            let token = binding_abi::string_arg(lua, token, RESISTANCE_USAGE)?;
            let school = binding_abi::number_arg(lua, school, RESISTANCE_USAGE)?;
            super::unit::check_unit_token(&Some(token.clone()))?;
            if !(0..7).contains(&school) {
                return Err(mlua::Error::RuntimeError(RESISTANCE_INDEX_ERROR.into()));
            }
            let idx = school as usize;
            asked(lua, &token, |a| {
                let Some(a) = a else {
                    return (0, 0, 0, 0);
                };
                let s = a.fields;
                let raw = s.resistances(idx);
                if !a.me {
                    let shown = i64::from(raw.max(0));
                    return (shown, shown, 0, 0);
                }
                let (pos, neg) = (s.resistance_pos(idx), s.resistance_neg(idx));
                (
                    i64::from(raw - pos - neg),
                    i64::from(raw.max(0)),
                    i64::from(pos),
                    i64::from(neg),
                )
            })
        })?,
    )?;

    // UnitArmor(unit) → (base, effectiveArmor, armor, posBuff, negBuff): the armor school through
    // `0x5efcd0` for the active player, effectiveArmor and armor both its clamped total; any
    // other unit's the field raised to 0 three times and no buffs (`0x51937e`-`0x5193b0`).
    g.set(
        "UnitArmor",
        lua.create_function(|lua, token: Value| {
            asked_arg(lua, token, r#"Usage: UnitArmor("unit")"#, |a| {
                let Some(a) = a else {
                    return (0, 0, 0, 0, 0);
                };
                let s = a.fields;
                let raw = s.resistances(0);
                let shown = i64::from(raw.max(0));
                if !a.me {
                    return (shown, shown, shown, 0, 0);
                }
                let (pos, neg) = (s.resistance_pos(0), s.resistance_neg(0));
                (
                    i64::from(raw - pos - neg),
                    shown,
                    shown,
                    i64::from(pos),
                    i64::from(neg),
                )
            })
        })?,
    )?;

    // UnitDamage(unit) → (minDamage, maxDamage, minOffHandDamage, maxOffHandDamage,
    // physicalBonusPos, physicalBonusNeg, percent): the four damage fields of any unit
    // (`0x518c72`-`0x518cc9`), then [`damage_bonus`] at the physical school; seven zeros with no
    // object (`0x518deb`).
    g.set(
        "UnitDamage",
        lua.create_function(|lua, token: Value| {
            asked_arg(lua, token, r#"Usage: UnitDamage("unit")"#, |a| {
                let Some(a) = a else {
                    return (0.0, 0.0, 0.0, 0.0, 0, 0, 0.0);
                };
                let s = a.fields;
                let (pos, neg, percent) = damage_bonus(
                    &a,
                    (
                        s.physical_bonus_pos(),
                        s.physical_bonus_neg(),
                        s.damage_percent(),
                    ),
                );
                (
                    f64::from(s.min_damage()),
                    f64::from(s.max_damage()),
                    f64::from(s.min_offhand_damage()),
                    f64::from(s.max_offhand_damage()),
                    pos,
                    neg,
                    percent,
                )
            })
        })?,
    )?;

    // UnitAttackSpeed(unit) → (mainSpeed, offhandSpeed) in seconds (`0x518ea0`, field 126, for
    // any unit); the off hand only for a player whose slot-16 item is a weapon (`0x518ecf`,
    // `0x518f19`), which only the active player's descriptor names, else nil; `0, nil` with no
    // object (`0x518f55`).
    g.set(
        "UnitAttackSpeed",
        lua.create_function(|lua, token: Value| {
            asked_arg(lua, token, r#"Usage: UnitAttackSpeed("unit")"#, |a| {
                let Some(a) = a else {
                    return (0.0, None);
                };
                let s = a.fields;
                (
                    swing_seconds(s.main_attack_time_ms()),
                    (a.player && s.has_offhand())
                        .then(|| swing_seconds(s.offhand_attack_time_ms())),
                )
            })
        })?,
    )?;

    // UnitAttackPower(unit) → (base, posBuff, negBuff): `UNIT_FIELD_ATTACK_POWER` and the signed
    // halves of `_MODS` (vmangos `StatSystem.cpp:335-336`), each scaled by `1 + _MULTIPLIER` and
    // rounded alone (`0x518fe2`, `0x519019`, `0x519051`), for any unit (the type word's unit bit,
    // `0x518fd3`); the server sends them to the owner alone.
    g.set(
        "UnitAttackPower",
        lua.create_function(|lua, token: Value| {
            asked_arg(lua, token, r#"Usage: UnitAttackPower("unit")"#, |a| {
                a.map_or((0, 0, 0), |a| {
                    let s = a.fields;
                    let m = s.attack_power_multiplier();
                    (
                        scaled_attack_power(m, s.attack_power()),
                        scaled_attack_power(m, s.attack_power_pos()),
                        scaled_attack_power(m, s.attack_power_neg()),
                    )
                })
            })
        })?,
    )?;

    // UnitRangedAttackPower(unit) → (base, posBuff, negBuff), from the ranged fields scaled the
    // same way, under `UnitAttackPower`'s usage text (`0x5190d7`).
    g.set(
        "UnitRangedAttackPower",
        lua.create_function(|lua, token: Value| {
            asked_arg(lua, token, r#"Usage: UnitAttackPower("unit")"#, |a| {
                a.map_or((0, 0, 0), |a| {
                    let s = a.fields;
                    let m = s.ranged_attack_power_multiplier();
                    (
                        scaled_attack_power(m, s.ranged_attack_power()),
                        scaled_attack_power(m, s.ranged_attack_power_pos()),
                        scaled_attack_power(m, s.ranged_attack_power_neg()),
                    )
                })
            })
        })?,
    )?;

    // UnitAttackBothHands(unit) → (mainBase, mainMod, offBase, offMod): `0x518810` calls
    // `[vtbl+0xb0]` for hand 0 then hand 1 ([`vtable_skill`]). Stock reads only the first pair
    // (`PaperDollFrame.lua:398-399`). A creature's body (`0x6136b0`) ignores the hand, so both
    // pairs are `(level * 5, 0)`.
    g.set(
        "UnitAttackBothHands",
        lua.create_function(|lua, token: Value| {
            asked_arg(lua, token, r#"Usage: UnitAttackBothHands("unit")"#, |a| {
                let own = a.as_ref().map_or(((0, 0), (0, 0)), |a| {
                    (
                        a.fields.main_weapon_skill(),
                        a.fields.offhand_weapon_skill(),
                    )
                });
                let (mb, mm) = skill_clamped(vtable_skill(a.as_ref(), own.0));
                let (ob, om) = skill_clamped(vtable_skill(a.as_ref(), own.1));
                (mb, mm, ob, om)
            })
        })?,
    )?;

    // UnitRangedAttack(unit) → (base, modifier), the active player's ranged weapon skill pair
    // (`0x518be5`, `0x612b40`), behind PLAYER then SELF; any other unit `0, 0`.
    g.set(
        "UnitRangedAttack",
        lua.create_function(|lua, token: Value| {
            asked_arg(
                lua,
                token,
                r#"Usage: UnitRangedAttack("unit")"#,
                |a| match a {
                    Some(a) if a.player && a.me => (
                        i64::from(a.fields.ranged_weapon_skill().0),
                        i64::from(a.fields.ranged_weapon_skill().1),
                    ),
                    _ => (0, 0),
                },
            )
        })?,
    )?;

    // UnitDefense(unit) → (base, modifier); stock splits the modifier's sign itself
    // (`PaperDollFrame.lua:259-271`). SELF or `UNIT_FIELD_SUMMONEDBY` us, then `[vtbl+0xac]`:
    // `CGPlayer_C` `0x5eda20` reads the Defense skill (`0x6de040`), `CGUnit_C` `0x613680` answers
    // level * 5 and 0 ([`vtable_skill`]); the clamp runs on every arm (`0x519298`).
    g.set(
        "UnitDefense",
        lua.create_function(|lua, token: Value| {
            asked_arg(lua, token, r#"Usage: UnitDefense("unit")"#, |a| {
                let own = a.as_ref().map_or((0, 0), |a| a.fields.defense_skill());
                skill_clamped(vtable_skill(a.as_ref(), own))
            })
        })?,
    )?;

    // GetDodgeChance(), GetParryChance(), GetBlockChance() → one number each, no unit argument
    // (`0x516f00`, `0x516fc0`, `0x516f60`), off the active player's field block as the call runs; 0
    // before the field streams.
    for (name, pick) in [
        ("GetDodgeChance", 0usize),
        ("GetParryChance", 1),
        ("GetBlockChance", 2),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                Ok(read_own_stats(lua, |s| {
                    s.map_or(0.0, |s| match pick {
                        0 => f64::from(s.dodge_percent()),
                        1 => f64::from(s.parry_percent()),
                        _ => f64::from(s.block_percent()),
                    })
                }))
            })?,
        )?;
    }

    // UnitRangedDamage(unit) → (speed, minDamage, maxDamage, physicalBonusPos, physicalBonusNeg,
    // percent): the ranged speed and damage fields of any unit (`0x518977`-`0x5189c8`), then
    // [`damage_bonus`] at the ranged weapon's school (`0x5189e1`-`0x518b0e`); six zeros with no
    // object (`0x518b3a`).
    g.set(
        "UnitRangedDamage",
        lua.create_function(|lua, token: Value| {
            asked_arg(lua, token, r#"Usage: UnitRangedDamage("unit")"#, |a| {
                let Some(a) = a else {
                    return (0.0, 0.0, 0.0, 0, 0, 0.0);
                };
                let s = a.fields;
                let (pos, neg, percent) = damage_bonus(
                    &a,
                    (
                        s.ranged_bonus_pos(),
                        s.ranged_bonus_neg(),
                        s.ranged_percent(),
                    ),
                );
                (
                    swing_seconds(s.ranged_attack_time_ms()),
                    f64::from(s.ranged_min_damage()),
                    f64::from(s.ranged_max_damage()),
                    pos,
                    neg,
                    percent,
                )
            })
        })?,
    )?;

    // HasWandEquipped() → 1 or nil, for the player: the ranged slot's item and its record as
    // the call runs (`0x4c99c2`-`0x4c9a04`).
    g.set(
        "HasWandEquipped",
        lua.create_function(|lua, ()| {
            Ok(flag(read_own_stats(lua, |s| {
                s.is_some_and(|s| s.has_wand())
            })))
        })?,
    )?;

    // The getters' shared opening: `is-number-or-string` on the unit (`0x6f3510`) else `usage`,
    // then the slot reader `0x4c8520` else `invalid`, then the token's resolve, which raises for
    // one it does not know (`0x4c8328` → `0x515c14`); the 1-based slot back for the lookup.
    fn inventory_item_args(
        lua: &Lua,
        (unit, slot): (Value, Value),
        usage: &'static str,
        invalid: &'static str,
    ) -> mlua::Result<(Option<String>, i64)> {
        let token = super::binding_abi::string_arg(lua, unit, usage)?;
        let slot0 = super::inventory_verbs::inventory_slot_arg(lua, slot, invalid)?;
        let token = Some(token);
        super::unit::check_unit_token(&token)?;
        Ok((token, i64::from(slot0) + 1))
    }

    // GetInventoryItemTexture(unit, slot) → icon path, or nil until the item's template arrives.
    g.set(
        "GetInventoryItemTexture",
        lua.create_function(|lua, (unit, slot): (Value, Value)| {
            // `Usage: GetInventoryItemTexture(unit, slot)` (`0x8488f8`) unless arg 1 is a number or
            // string, `Invalid inventory slot in GetInventoryItemTexture` (`0x8488c4`) unless the
            // shared reader passes arg 2 (`0x4c82f8`).
            let (token, slot) = inventory_item_args(
                lua,
                (unit, slot),
                "Usage: GetInventoryItemTexture(unit, slot)",
                "Invalid inventory slot in GetInventoryItemTexture",
            )?;
            let view = player_inv_slot(lua, &token, slot);
            // The slot's item and its record as the call runs.
            let read = view.as_ref().map(|v| {
                read_item(lua, v.item_key(), v, |_, item| {
                    (item.cached(), item.texture())
                })
            });
            // Only while the player carries some (`0x4c83a0`, `CountInventoryItemsByEntry`).
            if read.as_ref().is_some_and(|(cached, _)| !cached) {
                ask_ammo_record(lua, &token, slot, view.as_ref().filter(|v| v.count > 0))?;
            }
            match read.and_then(|(_, icon)| icon) {
                Some(icon) => Ok(Value::String(lua.create_string(&icon)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetInventoryItemLink(unit, slot) → `|cff…|Hitem:…|h[Name]|h|r` (`0x4c8c10`). Every no-item
    // exit pushes nothing (`0x4c8c56`): the reach gate, an empty slot, and a visible item whose
    // record is not cached (`0x4c8d3d`); an own item links without its record (`0x4c8ce2`).
    g.set(
        "GetInventoryItemLink",
        lua.create_function(|lua, (unit, slot): (Value, Value)| {
            // `Usage: GetInventoryItemLink(unit, slot)` (`0x848ad0`) unless arg 1 is a number or
            // string, `Invalid inventory slot in GetInventoryItemLink` (`0x848aa0`) unless the
            // shared reader passes arg 2 (`0x4c8c68`).
            let token = super::binding_abi::string_arg(
                lua,
                unit,
                "Usage: GetInventoryItemLink(unit, slot)",
            )?;
            let slot0 = super::inventory_verbs::inventory_slot_arg(
                lua,
                slot,
                "Invalid inventory slot in GetInventoryItemLink",
            )?;
            // The ammo slot answers no values ahead of the token's resolve (`0x4c8c8a`).
            if slot0 == -1 {
                return Ok(MultiValue::new());
            }
            let token = Some(token);
            super::unit::check_unit_token(&token)?;
            let link = player_inv_slot(lua, &token, i64::from(slot0) + 1).and_then(|v| {
                read_item(lua, v.item_key(), &v, |_, item| {
                    item.link().or_else(|| item.uncached_link())
                })
            });
            Ok(match link {
                Some(link) => MultiValue::from_vec(vec![Value::String(lua.create_string(&link)?)]),
                None => MultiValue::new(),
            })
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
            let token = Some(token);
            super::unit::check_unit_token(&token)?;
            // The item table's slot (`0x6228a0`), empty for another player: 1 (`0x4c8797`).
            let Some(v) = own_inv_slot(lua, &token, i64::from(slot0) + 1) else {
                return Ok(1i64);
            };
            Ok(match v.contents_count {
                // A held object's stack as the call runs; the ammo slot's carried total is its
                // view's.
                None if v.guid != 0 => {
                    let count = read_item(lua, v.item_key(), &v, |v, item| {
                        item.stack_count().unwrap_or(v.count).max(1)
                    });
                    i64::from(count)
                }
                None => i64::from(v.count),
                Some(_) if slot0 > 0x16 => 0,
                Some(n) => i64::from(n),
            })
        })?,
    )?;

    // GetInventoryItemQuality(unit, slot) → the record's quality, -1 for a record of inventory
    // type 0 (`0x4c8a22`), nil for an empty slot or an uncached record (`0x4c8a11`).
    g.set(
        "GetInventoryItemQuality",
        lua.create_function(|lua, (unit, slot): (Value, Value)| {
            // `Usage: GetInventoryItemQuality(unit, slot)` (`0x848a14`) unless arg 1 is a number or
            // string, `Invalid inventory slot in GetInventoryItemQuality` (`0x8489e0`) unless the
            // shared reader passes arg 2 (`0x4c8925`).
            let (token, slot) = inventory_item_args(
                lua,
                (unit, slot),
                "Usage: GetInventoryItemQuality(unit, slot)",
                "Invalid inventory slot in GetInventoryItemQuality",
            )?;
            // The item table's slot (`0x4c89d6` → `0x6228a0`), empty for another player.
            let view = own_inv_slot(lua, &token, slot).filter(|v| v.item_id != 0);
            // The slot's record as the call runs.
            let read = view.as_ref().map(|v| {
                read_item(lua, v.item_key(), v, |_, item| {
                    item.quality().map(|q| (item.inventory_type(), q))
                })
            });
            if read.as_ref().is_some_and(Option::is_none) {
                ask_ammo_record(lua, &token, slot, view.as_ref())?;
            }
            Ok(match read.flatten() {
                Some((0, _)) => Value::Integer(-1),
                Some((_, quality)) => Value::Integer(i64::from(quality)),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetInventoryItemBroken(unit, slot) → 1 or nil, per `slot_is_broken`.
    g.set(
        "GetInventoryItemBroken",
        lua.create_function(|lua, (unit, slot): (Value, Value)| {
            // `Usage: GetInventoryItemBroken(unit, slot)` (`0x848958`) unless arg 1 is a number or
            // string, `Invalid inventory slot in GetInventoryItemBroken` (`0x848924`) unless the
            // shared reader passes arg 2 (`0x4c85ca`).
            let (token, slot) = inventory_item_args(
                lua,
                (unit, slot),
                "Usage: GetInventoryItemBroken(unit, slot)",
                "Invalid inventory slot in GetInventoryItemBroken",
            )?;
            // The object's durability as the call runs.
            let broken = own_inv_slot(lua, &token, slot).is_some_and(|v| {
                read_item(lua, v.item_key(), &v, |v, item| {
                    slot_is_broken(v, item.durability())
                })
            });
            Ok(if broken {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // The reference's verbatim strings (`0x848a74`, `0x848a40`): arg 1 neither number nor string,
    // arg 2 not a number or not a whitelisted slot.
    const USAGE_GET_INVENTORY_ITEM_COOLDOWN: &str = "Usage: GetInventoryItemCooldown(unit, slot)";
    const INVALID_SLOT_GET_INVENTORY_ITEM_COOLDOWN: &str =
        "Invalid inventory slot in GetInventoryItemCooldown";
    // GetInventoryItemCooldown(unit, slot) (`0x4c8a60`) → (start, duration, enable) on the
    // `GetTime` clock, three zeros for every case but one (`0x4c8bdf`): slot -1 (the ammo leg), a
    // unit that is not the active player (a player in range passes `0x4944a0`, but only the active
    // player's table holds items, `0x6228a0`), an empty slot, a template with no block-0 spell
    // (`+0x11c`, `0x4c8b47`) and a charged one with none left (`0x4c8b55`-`0x4c8b6a`). The rest
    // push the item query `0x6e2ed0`, scaled by 0.001 (`0x4c8b9e`, `0x4c8bbd`), with its enable
    // as it is.
    g.set(
        "GetInventoryItemCooldown",
        lua.create_function(|lua, (unit, slot): (Value, Value)| {
            let token =
                super::binding_abi::string_arg(lua, unit, USAGE_GET_INVENTORY_ITEM_COOLDOWN)?;
            let slot0 = super::binding_abi::number_arg(
                lua,
                slot,
                INVALID_SLOT_GET_INVENTORY_ITEM_COOLDOWN,
            )?
            .wrapping_sub(1);
            if !inventory_slot_reader_accepts(slot0) {
                return Err(mlua::Error::RuntimeError(
                    INVALID_SLOT_GET_INVENTORY_ITEM_COOLDOWN.into(),
                ));
            }
            // The ammo slot answers three zeros ahead of the token's resolve (`0x4c8ad4`).
            if slot0 == -1 {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                return Ok(super::container::item_cooldown_triple(None, model.now));
            }
            super::unit::check_unit_token(&Some(token.clone()))?;
            let (asked, snapshot, now) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let live = usize::try_from(slot0 + 1).ok();
                let own = inventory_reader(lua, &model, &token) == Some(InvReader::Own);
                let asked = live.filter(|_| own).filter(|&live| {
                    crate::script::char_stats::own_inv_view(lua, &model, live)
                        .is_some_and(|v| v.item_id != 0 && v.spell_0)
                });
                let snapshot = asked.and_then(|live| {
                    crate::script::char_stats::inv_slot_cooldown(lua, &model, live)
                });
                (asked, snapshot, model.now)
            };
            // A worn item's query reads the list inside the call (`0x4c8b86` → `0x6e2ed0`),
            // through the lent host; the bank and keyring bands answer off their snapshots.
            let live = asked
                .filter(|live| (1..=19).contains(live))
                .and_then(|live| u8::try_from(live - 1).ok())
                .and_then(|slot0| {
                    super::cooldown_read::read_live(lua, super::CooldownQuery::Worn(slot0))
                });
            let answer = match asked {
                Some(_) => live.unwrap_or(snapshot),
                None => None,
            };
            Ok(super::container::item_cooldown_triple(answer, now))
        })?,
    )?;

    // GetInventoryAlertStatus(index) → the status of region 1..=12: 0 none, 3 damaged or low ammo,
    // 4 broken.
    g.set(
        "GetInventoryAlertStatus",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetInventoryAlertStatus(index)` (`0x848cbc`, `0x4c94c5`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: GetInventoryAlertStatus(index)")?;
            let model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(if (1..=12).contains(&index) {
                i64::from(model.inventory_alerts[index - 1])
            } else {
                0
            })
        })?,
    )?;

    // OffhandHasWeapon() → 1 or nil: whether the off hand holds a weapon (item class 2). The
    // active player is looked up first (`0x4c9578`, `0x4c9589` → `0x468460`) and nil is pushed with
    // none held (`je 0x4c95c4` at `0x4c9590`), as in a `PLAYER_LOGOUT` handler.
    g.set(
        "OffhandHasWeapon",
        lua.create_function(|lua, (): ()| {
            if !active_player_held(lua) {
                return Ok(None);
            }
            let (held, snapshot_class) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let held = model.inventory_slots[17].clone().filter(|v| v.item_id != 0);
                let class = held
                    .as_ref()
                    .and_then(|v| model.item_templates.get(&v.item_id))
                    .map(|t| t.class);
                (held, class)
            };
            // The off hand's record as the call runs; off the snapshot, the VM's record copy.
            let is_weapon = held.is_some_and(|v| {
                let live = read_live(lua, v.item_key(), |item| item.class());
                live.unwrap_or(snapshot_class) == Some(2)
            });
            Ok(is_weapon.then_some(1i64))
        })?,
    )?;

    // `0x848894`, verbatim.
    const INVALID_SLOT: &str = "Invalid inventory slot in GetInventorySlotInfo";
    // GetInventorySlotInfo(slotName) → (slotId, textureName, checkRelic) from `SLOT_INFO`.
    // `checkRelic` is a property of the slot; stock pairs it with `UnitHasRelicSlot("player")`
    // (`PaperDollFrame.lua:680`, `:744`).
    g.set(
        "GetInventorySlotInfo",
        lua.create_function(|lua, name: Value| {
            // A non-string misses `is-number-or-string` and takes the unknown name's raise
            // (`0x4c81ca` → `0x4c822c`).
            let name = crate::script::binding_abi::string_arg(lua, name, INVALID_SLOT)?;
            // A full-string ASCII case-insensitive match: `0x4c8215` calls `_strnicmp` (`0x64a4c0`
            // -> `0x414310`) with no length limit. The 36 names stay distinct after folding.
            let Some((_, id, art)) = SLOT_INFO
                .iter()
                .find(|(n, _, _)| n.eq_ignore_ascii_case(&name))
            else {
                // An unknown name raises, never nil.
                return Err(mlua::Error::runtime(INVALID_SLOT));
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
        weapon_subclass_skill, AlertInputs, AlertItem, InventoryAlertInputs,
        RESISTANCE_INDEX_ERROR, SKILL_UNARMED, STAT_INDEX_ERROR, STAT_USAGE,
    };
    use crate::script::{InvSlotView, UiScript, UnitCombatStats, UnitGuids, UnitState};

    const ME: u64 = 0x10;
    const PET: u64 = 0xF140_0000_0000_0077;
    const MOB: u64 = 0xF130_0000_0000_0001;
    /// Another player.
    const BOB: u64 = 0x20;

    /// A VM with the active player seated: its snapshot under `"player"` and its guid in the
    /// resolver's stand-in.
    fn seated() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                has_object: true,
                guid: ME,
                is_player: true,
                ..Default::default()
            }),
        );
        s.set_unit_guids(&UnitGuids {
            player: ME,
            ..Default::default()
        });
        s
    }

    /// A held unit that is not the player, with its own fields.
    fn held(guid: u64, level: u32, combat: UnitCombatStats) -> UnitState {
        UnitState {
            exists: true,
            has_object: true,
            guid,
            level,
            combat,
            ..Default::default()
        }
    }

    /// Our level-60 pet, which we summoned.
    fn pet_unit(combat: UnitCombatStats) -> UnitState {
        UnitState {
            summoned_by: ME,
            ..held(PET, 60, combat)
        }
    }

    /// A swing time in seconds as the getters scale it: the integer times the single 0.001
    /// (`0x518eb8`, `0x51898f`), at double precision.
    fn speed(ms: u32) -> f64 {
        f64::from(ms) * f64::from(0.001f32)
    }

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
            ranged_bonus_pos: 40,
            ranged_bonus_neg: -6,
            ranged_percent: 1.2,
            main_attack_time_ms: 2900,
            offhand_attack_time_ms: 1500,
            has_offhand: false,
            attack_power: 78,
            attack_power_pos: 30,
            attack_power_neg: -10,
            attack_power_multiplier: 0.0,
            ranged_attack_power: 52,
            ranged_attack_power_pos: 0,
            ranged_attack_power_neg: -3,
            ranged_attack_power_multiplier: 0.0,
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
        let mut s = seated();
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
        let mut s = seated();
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
        let mut s = seated();
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
        let mut s = seated();
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
        let mut s = seated();
        s.set_player_combat_stats(Some(stats()));
        assert_eq!(
            s.eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("player")"#)
                .unwrap(),
            (12.5, 19.5, 5.0, 9.0, 25, -3, f64::from(1.1f32))
        );
        // No off-hand weapon: the second return is nil.
        assert_eq!(
            s.eval::<(f64, Option<f64>)>(r#"return UnitAttackSpeed("player")"#)
                .unwrap(),
            (speed(2900), None)
        );
        s.set_player_combat_stats(Some(UnitCombatStats {
            has_offhand: true,
            ..stats()
        }));
        assert_eq!(
            s.eval::<(f64, f64)>(r#"return UnitAttackSpeed("player")"#)
                .unwrap(),
            (speed(2900), speed(1500))
        );
        // No object, a token naming nobody or the player before its object: seven zeros, the
        // percent among them (`0x518deb`).
        assert_eq!(
            s.eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("target")"#)
                .unwrap(),
            (0.0, 0.0, 0.0, 0.0, 0, 0, 0.0)
        );
        let fresh = UiScript::new().unwrap();
        assert_eq!(
            fresh
                .eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("player")"#)
                .unwrap(),
            (0.0, 0.0, 0.0, 0.0, 0, 0, 0.0)
        );
    }

    #[test]
    fn attack_power_and_weapon_skill_bindings_serve_the_pairs() {
        let mut s = seated();
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
        // The bonus at the ranged weapon's school, not the physical one.
        assert_eq!(
            s.eval::<(f64, f64, f64, i64, i64, f64)>(r#"return UnitRangedDamage("player")"#)
                .unwrap(),
            (speed(2800), 31.0, 47.0, 40, -6, f64::from(1.2f32))
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
        let mut s = seated();
        s.set_player_combat_stats(Some(stats()));
        s.set_unit("pet", Some(pet_unit(pet_stats())));
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
        let mut s = seated();
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
        let mut s = seated();
        s.set_player_combat_stats(Some(stats()));
        s.set_unit("pet", Some(pet_unit(pet_stats())));

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
        assert_eq!(
            s.eval::<(f64, Option<f64>)>(r#"return UnitAttackSpeed("pet")"#)
                .unwrap(),
            (speed(2000), None)
        );

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
        s.set_unit("pet", None);
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(r#"return UnitStat("pet", 1)"#)
                .unwrap(),
            (0, 0, 0, 0)
        );
        assert_eq!(
            s.eval::<(f64, f64, f64, f64, i64, i64, f64)>(r#"return UnitDamage("pet")"#)
                .unwrap(),
            (0.0, 0.0, 0.0, 0.0, 0, 0, 0.0)
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
        let mut s = seated();
        s.set_player_combat_stats(Some(stats()));
        s.set_unit("pet", Some(pet_unit(pet_stats())));
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
            Some(UnitState {
                summoned_by: BOB,
                ..held(MOB, 63, UnitCombatStats::default())
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

    /// The stat getters resolve any token to its object (`0x515940`) and read that object's fields
    /// behind each function's gates (benilla-re `ui/unitstats`, "Who may be asked"): SELF is the
    /// object's GUID against the active player's (`0x468550`), so a chain that ends on us reads our
    /// own block; SUMMONEDBY us (`0x519272`) gives the level times 5; PLAYER picks the damage bonus
    /// arm (`0x518cd4`, `0x5189d5`); no object gives zeros, the percent among them.
    #[test]
    fn the_stat_getters_answer_for_any_token_by_its_unit() {
        let mut s = seated();
        s.set_player_combat_stats(Some(stats()));
        s.set_unit_guids(&UnitGuids {
            player: ME,
            target: MOB,
            held: std::collections::HashMap::from([(MOB, ME), (ME, MOB)]),
            ..Default::default()
        });
        // A target's public base attack time (`0x518ea0`, field 126) and nothing owner-only.
        let mob = UnitCombatStats {
            main_attack_time_ms: 2000,
            min_damage: 50.0,
            max_damage: 70.0,
            resistances: [900, 0, -5, 0, 0, 0, 0],
            ..Default::default()
        };
        s.set_unit("target", Some(held(MOB, 50, mob.clone())));
        // The chain `targettarget` ends on us.
        s.set_unit_by_guid(
            ME,
            Some(UnitState {
                is_player: true,
                ..held(ME, 60, UnitCombatStats::default())
            }),
        );
        let eval = |s: &UiScript, chunk: &str| -> String {
            s.eval::<mlua::MultiValue>(&format!("return {chunk}"))
                .map(|v| {
                    v.iter()
                        .map(|v| match v {
                            mlua::Value::Nil => "nil".to_string(),
                            mlua::Value::Number(n) => n.to_string(),
                            mlua::Value::Integer(n) => n.to_string(),
                            other => format!("{other:?}"),
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_else(|e| panic!("{chunk}: {e}"))
        };
        assert_eq!(
            eval(&s, r#"UnitAttackSpeed("target")"#),
            format!("{} nil", speed(2000))
        );
        assert_eq!(
            eval(&s, r#"UnitDamage("target")"#),
            "50 70 0 0 0 0 1",
            "a creature: the bonus arm's 0, 0 and 1.0"
        );
        assert_eq!(
            eval(&s, r#"UnitResistance("target", 2)"#),
            "0 0 0 0",
            "not the player: the field raised to 0 twice (`0x518562`-`0x51857c`)"
        );
        assert_eq!(eval(&s, r#"UnitArmor("target")"#), "900 900 900 0 0");
        assert_eq!(eval(&s, r#"UnitDefense("target")"#), "0 0");
        // Us by a chain: our own block, the buffs and the skills included.
        assert_eq!(eval(&s, r#"UnitStat("targettarget", 1)"#), "25 25 4 0");
        assert_eq!(eval(&s, r#"UnitAttackPower("targettarget")"#), "78 30 -10");
        assert_eq!(eval(&s, r#"UnitDefense("targettarget")"#), "55 4");
        assert_eq!(eval(&s, r#"UnitRangedAttack("targettarget")"#), "18 0");
        // A target that is our pet: its own fields, and our summon's level times 5.
        s.set_unit("target", Some(pet_unit(pet_stats())));
        assert_eq!(eval(&s, r#"UnitAttackPower("target")"#), "178 12 -4");
        assert_eq!(eval(&s, r#"UnitDefense("target")"#), "300 0");
        assert_eq!(eval(&s, r#"UnitAttackBothHands("target")"#), "300 0 300 0");
        assert_eq!(eval(&s, r#"UnitRangedAttack("target")"#), "0 0");
        // Another player: its fields, the bonus arm's 0, 0 and 0.0, no off hand.
        s.set_unit(
            "target",
            Some(UnitState {
                is_player: true,
                ..held(BOB, 60, mob)
            }),
        );
        assert_eq!(eval(&s, r#"UnitDamage("target")"#), "50 70 0 0 0 0 0");
        assert_eq!(eval(&s, r#"UnitRangedDamage("target")"#), "0 0 0 0 0 0");
        // A party member out of view: no object, zeros.
        s.set_unit(
            "party1",
            Some(UnitState {
                exists: true,
                guid: BOB,
                level: 60,
                ..Default::default()
            }),
        );
        assert_eq!(eval(&s, r#"UnitAttackSpeed("party1")"#), "0 nil");
        assert_eq!(eval(&s, r#"UnitStat("party1", 1)"#), "0 0 0 0");
        // Every one raises `Usage:` for a non-string (`0x6f3510`) and the resolver's error for an
        // unknown token.
        for (getter, usage) in [
            ("UnitArmor", r#"Usage: UnitArmor("unit")"#),
            ("UnitDamage", r#"Usage: UnitDamage("unit")"#),
            ("UnitRangedDamage", r#"Usage: UnitRangedDamage("unit")"#),
            ("UnitAttackSpeed", r#"Usage: UnitAttackSpeed("unit")"#),
            ("UnitAttackPower", r#"Usage: UnitAttackPower("unit")"#),
            ("UnitRangedAttackPower", r#"Usage: UnitAttackPower("unit")"#),
            ("UnitDefense", r#"Usage: UnitDefense("unit")"#),
            (
                "UnitAttackBothHands",
                r#"Usage: UnitAttackBothHands("unit")"#,
            ),
            ("UnitRangedAttack", r#"Usage: UnitRangedAttack("unit")"#),
        ] {
            let err = s.run(&format!("{getter}()")).unwrap_err().to_string();
            assert!(err.contains(usage), "{getter}(): {err}");
            let err = s
                .run(&format!("{getter}(\"bogus\")"))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("Unknown unit name"),
                "{getter}(\"bogus\"): {err}"
            );
        }
    }

    #[test]
    fn has_wand_equipped_reads_the_flag() {
        let mut s = seated();
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
        let mut s = seated();
        // The bank bands read through `GetItemInSlot`, open only at a banker.
        s.set_bank(Some(crate::script::BankState::default()));
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

    /// The item id in `GetInventoryItemLink("player", slot)`'s link, 1.12's only road to it.
    fn link_id(slot: &str) -> String {
        format!(
            r#"local _, _, id = string.find(GetInventoryItemLink("player", {slot}), "item:(%d+)")
               return tonumber(id)"#
        )
    }

    /// The slot getters resolve their token (`0x4c8328`) and read through
    /// `CanReadUnitInventory` (`0x4944a0`): any token naming the active player reads its own
    /// table; another player, a cooperative one within 10 yards, its visible items, slots 1-19
    /// (`0x4c8420` → `0x5f0d60`), whose quality the item table answers, empty (`0x4c89d6`); a token
    /// the resolver does not know raises (`0x515c14`).
    #[test]
    fn the_slot_getters_resolve_their_token_and_read_through_the_reach_gate() {
        use crate::script::UnitReach;
        use std::collections::HashMap;
        const MATE: u64 = 0x20;
        const FAR: u64 = 0x30;
        let mut s = seated();
        let helm = |id: u32, icon: &str| InvSlotView {
            item_id: id,
            icon: Some(icon.into()),
            count: 1,
            quality: 3,
            name: Some(format!("Helm {id}")),
            link: Some(format!("|cff0070dd|Hitem:{id}:0:0:0|h[Helm {id}]|h|r")),
            ..Default::default()
        };
        let mut own: crate::script::InventorySlots = Default::default();
        own[1] = Some(helm(1, "Own"));
        s.set_inventory_slots(own);
        for (guid, id) in [(MATE, 2), (FAR, 3)] {
            let mut slots: crate::script::InventorySlots = Default::default();
            slots[1] = Some(helm(id, &format!("Visible{id}")));
            s.set_visible_equipment(guid, Some(slots));
        }
        let guids = |target: u64| UnitGuids {
            player: ME,
            target,
            party: [MATE, FAR, 0, 0],
            held: HashMap::from([(ME, 0), (MATE, 0), (FAR, 0)]),
            ..Default::default()
        };
        s.set_unit_guids(&guids(MATE));
        let reach = |inspectable: bool| {
            HashMap::from([
                (
                    ME,
                    UnitReach {
                        dist_sq: 0.0,
                        inspectable: true,
                    },
                ),
                (
                    MATE,
                    UnitReach {
                        dist_sq: 16.0,
                        inspectable,
                    },
                ),
                (
                    FAR,
                    UnitReach {
                        dist_sq: 400.0,
                        inspectable: true,
                    },
                ),
            ])
        };
        s.set_unit_reach(reach(true));
        let texture = |s: &UiScript, token: &str| {
            s.eval::<Option<String>>(&format!("return GetInventoryItemTexture({token:?}, 1)"))
                .unwrap()
        };
        assert_eq!(texture(&s, "target").as_deref(), Some("Visible2"));
        assert_eq!(texture(&s, "PARTY1").as_deref(), Some("Visible2"));
        assert_eq!(texture(&s, "player").as_deref(), Some("Own"));
        assert_eq!(texture(&s, "party2"), None, "20 yards off");
        assert_eq!(
            s.eval::<Option<String>>(r#"return GetInventoryItemLink("target", 1)"#)
                .unwrap(),
            Some("|cff0070dd|Hitem:2:0:0:0|h[Helm 2]|h|r".into())
        );
        assert_eq!(
            s.eval::<Option<i64>>(r#"return GetInventoryItemQuality("target", 1)"#)
                .unwrap(),
            None,
            "another player's item table holds nothing"
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemCount("target", 1)"#)
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<Option<String>>(r#"return GetInventoryItemTexture("target", 0)"#)
                .unwrap(),
            None,
            "the ammo slot is the active player's alone"
        );
        s.set_unit_reach(reach(false));
        assert_eq!(
            texture(&s, "target"),
            None,
            "a player we cannot cooperate with"
        );
        s.set_unit_guids(&guids(ME));
        assert_eq!(
            texture(&s, "target").as_deref(),
            Some("Own"),
            "a token naming us reads our table"
        );
        for call in [
            r#"GetInventoryItemTexture("npctarget", 1)"#,
            r#"GetInventoryItemLink("bogus", 1)"#,
            r#"GetInventoryItemQuality("bogus", 1)"#,
            r#"GetInventoryItemCount("bogus", 1)"#,
            r#"GetInventoryItemBroken("bogus", 1)"#,
            r#"GetInventoryItemCooldown("bogus", 1)"#,
        ] {
            assert!(s.run(call).is_err(), "{call}");
        }
    }

    /// Every no-item exit of `GetInventoryItemLink` jumps to `0x4c8c56` (`xor eax,eax`) and pushes
    /// nothing, never a nil: the reach gate (`0x4c8cae`), an empty own slot (`0x4c8cda`) and a
    /// visible item whose record the cache lacks (`0x4c8d3d`).
    #[test]
    fn the_inventory_link_answers_no_values_without_an_item() {
        use crate::script::UnitReach;
        use std::collections::HashMap;
        const MATE: u64 = 0x20;
        const FAR: u64 = 0x30;
        let mut s = seated();
        let mut own: crate::script::InventorySlots = Default::default();
        own[1] = Some(InvSlotView {
            item_id: 1,
            name: Some("Helm".into()),
            link: Some("|cffffffff|Hitem:1:0:0:0|h[Helm]|h|r".into()),
            ..Default::default()
        });
        s.set_inventory_slots(own);
        // The mate's helm, its record not cached: no name and no link.
        let mut visible: crate::script::InventorySlots = Default::default();
        visible[1] = Some(InvSlotView {
            item_id: 2,
            count: 1,
            ..Default::default()
        });
        s.set_visible_equipment(MATE, Some(visible));
        s.set_unit_guids(&UnitGuids {
            player: ME,
            target: MATE,
            party: [FAR, 0, 0, 0],
            held: HashMap::from([(ME, 0), (MATE, 0), (FAR, 0)]),
            ..Default::default()
        });
        let near = UnitReach {
            dist_sq: 16.0,
            inspectable: true,
        };
        let far = UnitReach {
            dist_sq: 400.0,
            inspectable: true,
        };
        s.set_unit_reach(HashMap::from([(MATE, near), (FAR, far)]));
        let count = |s: &UiScript, call: &str| s.arity(call).unwrap();
        assert_eq!(count(&s, r#"GetInventoryItemLink("player", 1)"#), 1);
        for call in [
            r#"GetInventoryItemLink("player", 2)"#,
            r#"GetInventoryItemLink("player", 20)"#,
            r#"GetInventoryItemLink("target", 1)"#,
            r#"GetInventoryItemLink("target", 2)"#,
            r#"GetInventoryItemLink("party1", 1)"#,
        ] {
            assert_eq!(count(&s, call), 0, "{call}");
        }
    }

    /// The ammo slot's texture and quality getters look the record up with the callback
    /// `0x4c84f0` (`0x4c83bc`, `0x4c89cf`): each call that misses registers one, the texture's
    /// only while the player carries some (`0x4c83a0`, `CountInventoryItemsByEntry`). A cached
    /// record, another getter or another slot registers none.
    #[test]
    fn the_ammo_slot_getters_register_a_record_callback_on_a_miss() {
        let mut s = seated();
        s.keep_calls();
        let mut slots: crate::script::InventorySlots = Default::default();
        let arrows = |count: u32, name: Option<&str>| InvSlotView {
            item_id: 2512,
            count,
            name: name.map(str::to_string),
            ..Default::default()
        };
        slots[0] = Some(arrows(200, None));
        slots[1] = Some(InvSlotView {
            item_id: 2263,
            count: 1,
            ..Default::default()
        });
        s.set_inventory_slots(slots.clone());
        s.run(
            r#"GetInventoryItemTexture("player", 0); GetInventoryItemQuality("player", 0)
               GetInventoryItemQuality("player", 0); GetInventoryItemLink("player", 0)
               GetInventoryItemTexture("player", 1); GetInventoryItemQuality("player", 1)"#,
        )
        .unwrap();
        assert_eq!(s.take_ammo_record_asks(), [2512, 2512, 2512]);
        assert!(s.take_ammo_record_asks().is_empty(), "taken");
        slots[0] = Some(arrows(0, None));
        s.set_inventory_slots(slots.clone());
        s.run(r#"GetInventoryItemTexture("player", 0); GetInventoryItemQuality("player", 0)"#)
            .unwrap();
        assert_eq!(
            s.take_ammo_record_asks(),
            [2512],
            "none carried: the quality's alone"
        );
        slots[0] = Some(arrows(200, Some("Rough Arrow")));
        s.set_inventory_slots(slots);
        s.run(r#"GetInventoryItemTexture("player", 0); GetInventoryItemQuality("player", 0)"#)
            .unwrap();
        assert!(s.take_ammo_record_asks().is_empty(), "the record is cached");
    }

    /// `GetInventoryItemLink` and `GetInventoryItemCooldown` answer the ammo slot ahead of the
    /// token's resolve (`0x4c8c8a`, `0x4c8ad4`): no values from the link and three zeros from the
    /// cooldown, for any token, one the resolver does not know included.
    #[test]
    fn the_link_and_cooldown_answer_the_ammo_slot_before_the_token() {
        let mut s = seated();
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[0] = Some(InvSlotView {
            item_id: 2512,
            count: 200,
            name: Some("Rough Arrow".into()),
            link: Some("|cffffffff|Hitem:2512:0:0:0|h[Rough Arrow]|h|r".into()),
            ..Default::default()
        });
        s.set_inventory_slots(slots);
        for token in ["player", "bogus"] {
            assert_eq!(
                s.eval::<i64>(&format!(
                    "local function n(...) return arg.n end \
                     return n(GetInventoryItemLink({token:?}, 0))"
                ))
                .unwrap(),
                0,
                "{token}"
            );
            assert_eq!(
                s.eval::<(f64, f64, f64)>(&format!(
                    "return GetInventoryItemCooldown({token:?}, 0)"
                ))
                .unwrap(),
                (0.0, 0.0, 0.0),
                "{token}"
            );
        }
    }

    #[test]
    fn inventory_item_bindings_serve_occupied_empty_and_absent_shapes() {
        let mut s = seated();
        let mut slots: crate::script::InventorySlots = Default::default();
        // Head (slot 1) occupied; ammo (slot 0) with a bag-summed count; the rest empty.
        slots[1] = Some(InvSlotView {
            item_id: 2263,
            icon: Some("Interface\\Icons\\INV_Misc_Bandana_01".into()),
            count: 1,
            quality: 2,
            inventory_type: Some(1),
            name: Some("Brawler's Harness".into()),
            link: Some("|cff1eff00|Hitem:2263:0:0:0|h[Brawler's Harness]|h|r".into()),
            ..Default::default()
        });
        slots[0] = Some(InvSlotView {
            item_id: 2512,
            icon: Some("Interface\\Icons\\INV_Ammo_Arrow_01".into()),
            count: 200,
            quality: 1,
            name: Some("Rough Arrow".into()),
            link: Some("|cffffffff|Hitem:2512:0:0:0|h[Rough Arrow]|h|r".into()),
            ..Default::default()
        });
        s.set_inventory_slots(slots);

        assert_eq!(s.eval::<i64>(&link_id("1")).unwrap(), 2263);
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
        // The ammo slot (0) reads through the same family, but for its link (`0x4c8c8a`).
        assert!(s
            .eval::<bool>(r#"return GetInventoryItemLink("player", 0) == nil"#)
            .unwrap());
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemCount("player", 0)"#)
                .unwrap(),
            200
        );
        // An empty slot: nil id, texture and quality, and count 1 (`0x4c8797`).
        assert!(s
            .eval::<bool>(r#"return GetInventoryItemLink("player", 5) == nil"#)
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
            .eval::<bool>(r#"return GetInventoryItemLink("target", 1) == nil"#)
            .unwrap());
        assert_eq!(
            s.eval::<i64>(r#"return GetInventoryItemCount("target", 1)"#)
                .unwrap(),
            1,
            "no item behind the token is the empty answer, and the empty answer is 1"
        );
        // Slots outside the reader's whitelist raise (`0x4c8520`, `0x848aa0`).
        for slot in [25, -1] {
            let e = s
                .run(&format!(r#"GetInventoryItemLink("player", {slot})"#))
                .unwrap_err()
                .to_string();
            assert!(
                e.contains("Invalid inventory slot in GetInventoryItemLink"),
                "{e}"
            );
        }
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

    /// A vault or keyring item read through the inventory API keeps its `ITEM_FIELD_DURATION`
    /// (`[item+0x114]+0x24`), the field the auction sell slot refuses on (`0x4ce411`).
    #[test]
    fn a_container_slot_seen_as_an_inventory_slot_keeps_its_field_duration() {
        use crate::script::container::{ContainerSlot, ContainerState};
        let mut s = seated();
        // The bank bands read through `GetItemInSlot`, open only at a banker.
        s.set_bank(Some(crate::script::BankState::default()));
        for (bag, live_id) in [(super::BANK_CONTAINER, 40), (super::KEYRING_CONTAINER, 82)] {
            let mut state = ContainerState {
                num_slots: 24,
                ..Default::default()
            };
            state.slots.insert(
                1,
                ContainerSlot {
                    item_id: 7000,
                    guid: 0x700,
                    count: 1,
                    field_duration: 3_600_000,
                    ..Default::default()
                },
            );
            s.set_container(bag, Some(state));
            let view =
                crate::script::char_stats::own_inv_view(s.lua(), &s.model_ref(), live_id).unwrap();
            assert_eq!(view.item_id, 7000, "bag {bag}");
            assert_eq!(view.field_duration, 3_600_000, "bag {bag}");
        }
    }

    /// The alert recompute triggers Broken Items for a slot broken at durability 0 (`0x4c7fee`-
    /// `0x4c8002`) and Damaged Items for 1 to 5 points left (`0x4c801b`-`0x4c802e`); a force-red
    /// slot (`0x10`, `0x4c7fb0`) is broken with no tutorial, and the ammo arm has none.
    #[test]
    fn the_alert_recompute_triggers_broken_and_damaged_items() {
        let mut s = UiScript::new().unwrap();
        let worn = |durability: (u32, u32), flags: u32| {
            Some(InvSlotView {
                item_id: 2361,
                durability: Some(durability),
                flags,
                ..Default::default()
            })
        };
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[1] = worn((10, 50), 0x10);
        slots[0] = Some(InvSlotView {
            item_id: 2512,
            count: 3,
            ..Default::default()
        });
        s.set_inventory_slots(slots.clone());
        s.recompute_inventory_alerts();
        assert!(
            s.take_tutorial_triggers().is_empty(),
            "force-red and low ammo"
        );
        slots[3] = worn((4, 50), 0);
        s.set_inventory_slots(slots.clone());
        s.recompute_inventory_alerts();
        assert_eq!(s.take_tutorial_triggers(), [0x23], "damaged");
        slots[3] = worn((0, 50), 0);
        s.set_inventory_slots(slots);
        s.recompute_inventory_alerts();
        assert_eq!(s.take_tutorial_triggers(), [0x24], "broken");
    }

    /// The recompute reads the lent host as it runs, never the doll snapshot, and signals
    /// `UPDATE_INVENTORY_ALERTS` on every pass whether or not a status moved (`0x4c8085`); with no
    /// active player it stores and signals nothing (`0x4c7f06`). So does the verb (`0x4c9560`).
    #[test]
    fn the_recompute_reads_the_host_and_signals_every_pass() {
        use crate::script::live::LiveAsk;
        use crate::script::{CallHost, CallTail, ScriptCall};
        struct Host(Option<AlertInputs>);
        impl CallHost for Host {
            fn apply(&mut self, _: &mut UiScript, _: ScriptCall) -> Option<CallTail> {
                None
            }
            fn read_live(&mut self, ask: &mut LiveAsk<'_>) -> bool {
                let inputs = self.0;
                ask.answer(|_: &InventoryAlertInputs| Some(inputs))
            }
        }
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"Heard = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("UPDATE_INVENTORY_ALERTS")
               f:SetScript("OnEvent", function()
                   table.insert(Heard, GetInventoryAlertStatus(1) .. GetInventoryAlertStatus(12))
               end)"#,
        )
        .unwrap();
        // The snapshot's helm is whole; the host's is broken.
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[1] = Some(InvSlotView {
            item_id: 7366,
            durability: Some((40, 40)),
            ..Default::default()
        });
        s.set_inventory_slots(slots);
        let mut worn = [None; 11];
        worn[0] = Some(AlertItem {
            durability: 0,
            max_durability: 40,
            ..AlertItem::default()
        });
        let mut host = Host(Some(AlertInputs {
            worn,
            ammo: Some(20),
        }));
        s.hosted(&mut host, |s| {
            s.recompute_inventory_alerts();
            s.recompute_inventory_alerts();
            s.run("UpdateInventoryAlertStatus()").unwrap();
        });
        let mut none = Host(None);
        s.hosted(&mut none, |s| {
            s.recompute_inventory_alerts();
            s.run("UpdateInventoryAlertStatus()").unwrap();
        });
        assert_eq!(
            s.eval::<Vec<String>>("return Heard").unwrap(),
            ["43", "43", "43"],
            "the host's statuses, once a pass, none with no player"
        );
    }

    /// The temporary-enchant arm (`0x4c8035`-`0x4c806d`): a worn item no durability arm claims
    /// reads 1 with a temporary enchantment, 2 at 30000 ms or fewer left, the boundary included
    /// and an unset or run-out deadline (0) too; a charter record's (`0x2000`) or no enchantment
    /// reads 0, and a damaged or broken item keeps its 3 or 4.
    #[test]
    fn the_temporary_enchant_arm_reads_one_or_two() {
        let status = |item: AlertItem| {
            let mut worn = [None; 11];
            worn[8] = Some(item);
            super::alert_statuses(&AlertInputs { worn, ammo: None })[8].0
        };
        let enchanted = |left: u32| AlertItem {
            durability: 40,
            max_durability: 40,
            temp_enchant: 7,
            temp_enchant_left_ms: left,
            ..AlertItem::default()
        };
        assert_eq!(status(enchanted(30_001)), 1);
        assert_eq!(status(enchanted(30_000)), 2);
        assert_eq!(status(enchanted(0)), 2, "unset or run out");
        assert_eq!(
            status(AlertItem {
                flags: 0x08,
                ..enchanted(60_000)
            }),
            1,
            "wrapped skips the durability arms"
        );
        assert_eq!(
            status(AlertItem {
                max_durability: 0,
                ..enchanted(60_000)
            }),
            1
        );
        assert_eq!(
            status(AlertItem {
                record_flags: 0x2000,
                ..enchanted(60_000)
            }),
            0
        );
        assert_eq!(
            status(AlertItem {
                temp_enchant: 0,
                ..enchanted(10)
            }),
            0
        );
        assert_eq!(
            status(AlertItem {
                durability: 5,
                ..enchanted(10)
            }),
            3
        );
        assert_eq!(
            status(AlertItem {
                flags: 0x10,
                ..enchanted(10)
            }),
            4
        );
    }
}
