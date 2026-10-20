//! The 1.12 bag verbs, all bare globals. The app pushes each bag's resolved snapshot and drains
//! the intents the verbs queue; the engine holds no item knowledge. Bag ids are the 1.12 client's
//! (0 the backpack, 1-4 the worn bags, -1 the bank's own slots, 5-10 the bank bags, -2 the
//! keyring); slots are 1-based.

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::{self, flag, number_arg};
use super::cursor::{self, CursorItem, CursorPayload};
use super::protected::{self, Tag};
use super::Model;

/// The petition lines an item tooltip prints under the name (`0x854d7c..0x854dd0`): the title and
/// the creator, keyed `GUILD_CHARTER_*` for a charter, else `PETITION_*`, then the signature
/// count.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionSlotView {
    /// The record's charter bit, also `GetPetitionInfo`'s first return.
    pub is_charter: bool,
    /// The proposed guild's (or petition's) name.
    pub title: String,
    /// The owner's name from the name cache; `None`, and no line, while the query is in flight.
    pub owner: Option<String>,
    /// The signatures, the item object's second enchantment dword (`flds+0x44`, `0x52ba51`).
    pub signatures: u32,
}

/// One occupied bag slot, resolved by the app from the item object and its template.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerSlot {
    /// Icon texture path (`Interface\Icons\…`); `None` while the template answer is in flight.
    pub texture: Option<String>,
    pub count: u32,
    /// Item quality 0..6; `None` while unresolved. `GetContainerItemInfo` reports it only when
    /// `inventory_type` is nonzero (`0x4f9794`), else -1.
    pub quality: Option<u32>,
    /// The template's InventoryType; 0 while unresolved and for an item that cannot be equipped.
    pub inventory_type: u32,
    /// The template's bonding (`[rec+0x194]`), 0 while unresolved: the equip arms ask on 2.
    pub bonding: u32,
    /// The on-use spell the dispatcher reads (`0x5d8c80`): the first template block with trigger 0,
    /// 0 for none or while unresolved.
    pub use_spell: u32,
    /// The template's class (`[rec+0x0]`, read by `0x5d9f30`), `None` while unresolved: a
    /// projectile (6) auto-equips by entry and unlocks as it goes (`0x5e1704`).
    pub item_class: Option<u32>,
    /// The item's template entry; 0 while unresolved.
    pub item_id: u32,
    /// An `|Hitem:…|h[Name]|h` link once the name is known.
    pub link: Option<String>,
    /// The item object's guid, 0 while unresolved: what the client's lock (`item+0x314`) and
    /// every lock read key on.
    pub guid: u64,
    /// The 1-based inventory slots the item equips into, from `inventoryType`; empty if none.
    pub equip_slots: Vec<u8>,
    /// Whether the item may go on an action bar: `PlaceAction`'s only item filter (`0x4e62e0`), an
    /// on-use spell or equippable.
    pub bar_placeable: bool,
    /// The action bar's count gate (`IsConsumableAction`, `0x4e5250`): ammo, thrown, or an on-use
    /// spell with negative charges.
    pub consumable: bool,
    /// Current and max durability; `None` when the max is 0 or the item object has not arrived.
    pub durability: Option<(u32, u32)>,
    /// The item query's answer (`GetItemCooldown`, `0x6e2ed0`), `(start_ms on the GetTime clock,
    /// duration_ms, enabled)`. `None` is the all-zero answer, enable included: a template with no
    /// on-use spell, a charged one with none left, or one still in flight. A cold on-use spell is
    /// `Some((0, 0, true))`.
    pub cooldown: Option<(i64, u32, bool)>,
    /// The template's block 0 names a spell (`+0x11c`, any trigger): `GetInventoryItemCooldown`
    /// runs no query without one (`0x4c8b47`).
    pub spell_0: bool,
    /// `ITEM_FIELD_SPELL_CHARGES`, the charges left on each spell block (`flds+0x28`): the
    /// tooltip's charges lines read them (`0x52d9e9`), and the merchant price scales by block 0's
    /// (`0x52e43d`).
    pub spell_charges: [i32; 5],
    /// The instance carries `ITEM_FIELD_ITEM_TEXT_ID` (a mail copy): `GetContainerItemInfo`'s
    /// `readable`, which shows the Inspect cursor (`ContainerFrame.lua:638`).
    pub readable: bool,
    /// The `ITEM_FIELD_CREATOR` name for the "Written by" or "<Made by>" line, once resolved.
    pub creator: Option<String>,
    /// The `ITEM_FIELD_GIFTCREATOR` name for a wrapped item's `ITEM_WRAPPED_BY` line, once
    /// resolved; only a wrapped item asks for it, as the gift arm does (`0x52e29d`).
    pub gift_creator: Option<String>,
    /// The `ITEM_FIELD_CREATOR` guid, 0 for none: with no [`Self::creator`] yet, the name the
    /// tooltip waits for.
    pub creator_guid: u64,
    /// The `ITEM_FIELD_GIFTCREATOR` guid, 0 for none, as [`Self::creator_guid`].
    pub gift_creator_guid: u64,
    /// `ITEM_FIELD_FLAGS`; the tooltip reads UNLOCKED `0x4` and WRAPPED `0x8`.
    pub flags: u32,
    /// Bound at runtime (`0x5da2c0`): `ITEM_FIELD_FLAGS & 1`, or an enchant whose
    /// `SpellItemEnchantment` row binds. The tooltip's bind line then says Soulbound.
    pub already_bound: bool,
    /// For a container (`TYPEMASK_CONTAINER`), how many of its seats hold an item, which
    /// `ClickTradeButton` counts (`0x4bff50`); `None` for any other item.
    pub occupied_seats: Option<u32>,
    /// The enchant slots in slot order, named from `SpellItemEnchantment.dbc`; empty when none.
    pub enchants: Vec<super::EnchantView>,
    /// A timed item's lifetime left in ms (`SMSG_ITEM_TIME_UPDATE`), floored to the second.
    pub duration_ms: Option<u64>,
    /// A signable charter's petition, printed between the name and the green `ITEM_SIGNABLE`
    /// line; `None` until the record the hover queries arrives.
    pub petition: Option<PetitionSlotView>,
    /// The arm a right-click takes past the bank and vendor forks, which the app reads off the
    /// item object and its template so `UseContainerItem` can take the arm's lock in the call.
    pub use_fork: UseFork,
}

impl ContainerSlot {
    /// The whole stack of this slot as a cursor payload, as a pickup sets it.
    pub(crate) fn cursor_item(&self, bag: i64, slot: u32) -> CursorItem {
        CursorItem {
            item_class: self.item_class,
            guid: self.guid,
            bag,
            slot,
            item_id: self.item_id,
            texture: self.texture.clone(),
            link: self.link.clone(),
            quality: self.quality,
            count: None,
            bar_placeable: self.bar_placeable,
            consumable: self.consumable,
            equip_slots: self.equip_slots.clone(),
        }
    }
}

/// `UseContainerItem`'s arm past its bank and vendor forks: the equip fork (`0x4fa3b9`), else the
/// use dispatcher `0x5d8d00`'s arms in its order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UseFork {
    /// An equippable template (`0x5d9ff0`) whose `StartQuest` is 0 (`0x4fa3bd`) auto-equips.
    Equip,
    /// A wrapped gift unwraps with `CMSG_OPEN_ITEM` (`0x5d8d92`), taking no lock.
    Unwrap,
    /// Wrapping paper arms the wrap (`0x5edea0`), which locks the paper.
    WrapPaper,
    /// A quest-starter offers its quest (`0x5d8dd2`).
    Quest,
    /// A template `PageText` book opens the reader (`0x5d8e4c`).
    Read,
    /// A mail letter, the instance's `ITEM_FIELD_ITEM_TEXT_ID`, opens the reader.
    Letter,
    /// A lootable template sends `CMSG_OPEN_ITEM` (`0x5d8f7c`), locking the item first
    /// (`0x5edcd9`).
    Open,
    /// The plain use, and a slot whose template is still in flight, which `0x5d9ff0` reads as
    /// not equippable.
    #[default]
    Plain,
}

/// The arm a `UseContainerItem` call took, in `0x4fa0e0`'s order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseArm {
    /// A bank-vault or bank-bag item withdraws (`0x4fa305`), the bank flag the slot resolve sets
    /// for bags -1 and 5..10 (`0x4f98b3`, `0x4f98fc`).
    Withdraw,
    /// Any other item deposits while a banker is open (`[0xbdd038]`, `0x4fa33e`).
    Deposit,
    /// With a vendor open (`[0xbddfa0]`) the whole stack sells (`0x4fa378`).
    Sell,
    /// The item's own fork.
    Item(UseFork),
}

/// One `UseContainerItem` call past its gates: the slot, the item it resolved (`0x4fa1a6`) and
/// the arm it took, whose lock the call has already taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerUse {
    pub bag: i64,
    pub slot: u32,
    pub guid: u64,
    pub arm: UseArm,
    /// The equip arm raised the bind-on-equip question and sent nothing: the item stays on the
    /// cursor, locked, under the popup (`0x5e16c6`, `0x5e16ce`).
    pub asks: bool,
    /// `onSelf`: the use targets the player in place of the selection (`0x4fa17d`-`0x4fa18e`).
    pub on_self: bool,
    /// What the protected-action gate's tag 2 allowed the call: an item's on-use spell casts
    /// through `TryCastSpell`, behind it (`0x4fa430` → `0x5d8d00` → `0x5d9258` → `0x6e5a90`).
    pub may_cast: bool,
}

/// One enchant slot as the tooltip renders it (`0x52c9f9`..`0x52ca23`); the app resolves the row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnchantView {
    /// `ITEM_FIELD_ENCHANTMENT` slot: 0 permanent, 1 temporary, 2-6 the random suffix; only 0 and
    /// 1 are coloured.
    pub slot: u8,
    /// The id was negative: `abs(id)` names the row, and slots 0 and 1 paint red, not green
    /// (`0x52ca29`..`0x52ca49`).
    pub negative: bool,
    /// The `SpellItemEnchantment` row's name, copied unformatted (`0x52ca8b`..`0x52caa1`).
    pub name: String,
    /// The slot's charges; nonzero appends " (N Charges)".
    pub charges: u32,
    /// Ms left on a temporary enchant, from `SMSG_ITEM_ENCHANT_TIME_UPDATE`, never the item's own
    /// duration field; `Some` prints the countdown form.
    pub remaining_ms: Option<u64>,
}

/// One `ItemRandomProperties` row, an item's random-suffix roll. The app pushes the whole table at
/// load, and a tooltip source supplies only the id, as in the reference.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RandomPropertyView {
    /// The suffix (`"of the Monkey"`), joined on by `ITEM_SUFFIX_TEMPLATE` (`0x5d8b00`), for the
    /// names the app does not compose itself.
    pub suffix: String,
    /// The roll's five enchants, named, in slots 2-6 (`0x52b7e0`..`0x52b7fb`); always white.
    pub enchants: Vec<EnchantView>,
}

/// One bag's snapshot: its name, capacity, and occupied slots (1-based).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerState {
    /// `GetBagName` for bags 1-10 (the bag item's own name); the backpack, the bank and the
    /// keyring have none.
    pub name: Option<String>,
    pub num_slots: u32,
    pub slots: std::collections::HashMap<u32, ContainerSlot>,
}

/// One queued move from `(src_bag, src_slot)` to `(dst_bag, dst_slot)`. `count` is `None` for a
/// whole stack (a move, a swap, or a merge the server tops up), `Some(n)` for a split placement
/// (`CMSG_SPLIT_ITEM`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerMove {
    pub src_bag: i64,
    pub src_slot: u32,
    pub dst_bag: i64,
    pub dst_slot: u32,
    pub count: Option<u32>,
}

/// One queued auto-store (`PutItemInBag`, `PutItemInBackpack`): the item at `(src_bag, src_slot)`
/// into container `dst_bag`, with no slot: `CMSG_AUTOSTORE_BAG_ITEM` (0x10B) carries none and
/// `CMSG_SPLIT_ITEM` (0x10E) sends `0xFF`, so the server picks. `Some(n)` in `count` is a split,
/// the fork the reference makes on `[0xb4b40c]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BagAutoStore {
    pub src_bag: i64,
    pub src_slot: u32,
    /// The container id: 0 the backpack, 1-4 a worn bag, 5-10 a bank bag.
    pub dst_bag: i64,
    pub count: Option<u32>,
}

/// What `GetContainerItemCooldown` and `GetInventoryItemCooldown` push for the item query's answer
/// (`0x6e2ed0`), `(start, duration, enable)` in `GetTime` seconds. No answer is three zeros, the
/// enable included (`0x4f9ae9`, `0x4c8bdf`). An answer past its end reads cold with enable 1: the
/// query ran and found nothing left, so a re-feed cannot replay the finish flash. A held one
/// (`enable` 0) answers the call's own now (`0x6e1517`).
pub(super) fn item_cooldown_triple(query: Option<(f64, f64, bool)>, now: f64) -> (f64, f64, i64) {
    match query {
        None => (0.0, 0.0, 0),
        Some((start, duration, enabled)) if start + duration > now || !enabled => (
            crate::script::clock::held_start(start, enabled, now),
            duration,
            i64::from(enabled),
        ),
        Some(_) => (0.0, 0.0, 1),
    }
}

impl super::UiScript {
    /// Drain the auto-stores `PutItemInBag`/`PutItemInBackpack` queued since the last call.
    pub fn take_bag_autostores(&mut self) -> Vec<BagAutoStore> {
        std::mem::take(&mut self.model_mut().bag_autostores)
    }

    /// Push one bag's snapshot, or remove it with `None`; cooldowns arrive in ms on the `GetTime`
    /// clock and are stored in seconds.
    pub fn set_container(&mut self, bag: i64, state: Option<ContainerState>) {
        let mut model = self.model_mut();
        model.container_cooldowns.retain(|&(b, _), _| b != bag);
        match state {
            Some(s) => {
                for (&slot, cs) in &s.slots {
                    let Some((start_ms, duration_ms, enabled)) = cs.cooldown else {
                        continue;
                    };
                    let abs = (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    );
                    model.container_cooldowns.insert((bag, slot), abs);
                }
                model.containers.insert(bag, s);
            }
            None => {
                model.containers.remove(&bag);
            }
        }
    }

    /// Push the use cooldowns of the worn items, `(start_ms on the GetTime clock, duration_ms,
    /// enabled)` by live-API slot id, stored in seconds like a bag slot's. No event fires: a
    /// cooldown moves no inventory slot.
    pub fn set_worn_cooldowns(
        &mut self,
        cooldowns: std::collections::HashMap<usize, (i64, u32, bool)>,
    ) {
        self.model_mut().worn_cooldowns = cooldowns
            .into_iter()
            .map(|(slot, (start_ms, duration_ms, enabled))| {
                (
                    slot,
                    (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    ),
                )
            })
            .collect();
    }

    /// Push `HasKey()`'s answer: whether the player owns a `BagFamily` key item anywhere.
    pub fn set_has_key(&mut self, has_key: bool) {
        self.model_mut().has_key = has_key;
    }

    /// Take the `UseContainerItem` calls out of the call stream, as `(bag, slot)`.
    pub fn take_container_uses(&mut self) -> Vec<(i64, u32)> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::UseContainerItem(u) => Some((u.bag, u.slot)),
            _ => None,
        })
    }

    /// Drain the pick/place/swap moves queued by `PickupContainerItem` since the last call.
    pub fn take_container_moves(&mut self) -> Vec<ContainerMove> {
        std::mem::take(&mut self.model_mut().container_moves)
    }

    /// Drain the `(bag, slot)` sources `AutoEquipCursorItem` queued, for `CMSG_AUTOEQUIP_ITEM`.
    pub fn take_container_autoequips(&mut self) -> Vec<(i64, u32)> {
        std::mem::take(&mut self.model_mut().container_autoequips)
    }

    /// Drain the `(bag, slot, guid)` of each `AutoEquipCursorItem` that raised the bind-on-equip
    /// question: the app files the pending record and the event, the item staying held.
    pub fn take_container_equip_asks(&mut self) -> Vec<(i64, u32, u64)> {
        std::mem::take(&mut self.model_mut().container_equip_asks)
    }

    /// Drain the `(bag, slot)` clicks repair mode queued, for `CMSG_REPAIR_ITEM`.
    pub fn take_container_repairs(&mut self) -> Vec<(i64, u32)> {
        std::mem::take(&mut self.model_mut().container_repairs)
    }

    /// Drain the `(bag, slot, count)` destroys `DeleteCursorItem` queued, a 0 count being the whole
    /// stack, for `CMSG_DESTROYITEM`.
    pub fn take_container_destroys(&mut self) -> Vec<(i64, u32, u32)> {
        std::mem::take(&mut self.model_mut().container_destroys)
    }

    /// Arm a gift wrap with the paper at `(bag, slot)`, as `0x5edea0` does: the paper locks, the
    /// cursor takes mode 2 (the cast art, table `0x853b88`), and nothing is sent. The next
    /// left-click on a container slot completes it.
    pub fn arm_gift_wrap(&mut self, bag: i64, slot: u32) {
        arm_gift_wrap(&mut self.model_mut(), bag, slot);
    }

    /// The armed gift wrap, if any; the app reads it to hold the cursor's base at Cast.
    pub fn gift_wrap_armed(&self) -> Option<PendingWrap> {
        self.model_ref().pending_wrap
    }

    /// Drain the `(giftBag, giftSlot, itemBag, itemSlot)` wraps queued, for `CMSG_WRAP_ITEM`.
    pub fn take_container_wraps(&mut self) -> Vec<(i64, u32, i64, u32)> {
        std::mem::take(&mut self.model_mut().container_wraps)
    }

    /// The mode the last FrameXML cursor call set, `None` after a `ResetCursor`; the app acts on
    /// [`Self::take_cursor_write`] instead.
    pub fn ui_cursor(&self) -> Option<UiCursorMode> {
        self.model_ref().ui_cursor
    }

    /// The path [`UiCursorMode::Custom`] shows: the name `SetCursor` loaded it by, as the script
    /// gave it (`0xbe2c70`).
    pub fn custom_cursor_name(&self) -> String {
        self.model_ref().custom_cursor_name.clone()
    }

    /// Drain the pending cursor write: `Some(mode)` a set, `Some(None)` a reset, `None` no call,
    /// which must leave the cursor as it was, so an armed spell keeps its cast cursor over a
    /// spellbook button.
    #[allow(clippy::option_option)]
    pub fn take_cursor_write(&mut self) -> Option<Option<UiCursorMode>> {
        let mut model = self.model_mut();
        std::mem::take(&mut model.ui_cursor_dirty).then_some(model.ui_cursor)
    }
}

/// The armed gift wrap's paper. The reference keeps it beside the cursor mode, not as a cursor
/// payload, so `CursorHasItem()` stays false while a wrap is armed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PendingWrap {
    /// The paper's container id.
    pub bag: i64,
    /// The paper's 1-based slot within that container.
    pub slot: u32,
    /// The paper's guid, locked while armed (`0x5edeb5`).
    pub guid: u64,
}

/// A displayed-cursor override from the FrameXML cursor verbs, the reference's one displayed mode
/// (`0xbe2c2c`), until `ResetCursor`; the app maps each to its `Interface\Cursor` art.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiCursorMode {
    /// Buy (3): selling a bag item, or a merchant or buyback item the player can afford.
    Buy,
    /// UnableBuy (23): a merchant or buyback item the player cannot afford.
    UnableBuy,
    /// Inspect (7): the magnifier, `ShowInspectCursor`.
    Inspect,
    /// Point (1), `SetCursor("POINT_CURSOR")`.
    Point,
    /// Cast (2), `SetCursor("CAST_CURSOR")`: a unit frame the armed spell can target.
    Cast,
    /// UnableCast (22), `SetCursor("CAST_ERROR_CURSOR")`: a unit frame the armed spell cannot
    /// target, and leaving one while still targeting.
    CastError,
    /// Attack (4), `SetCursor("ATTACK_CURSOR")`.
    Attack,
    /// UnablePoint (21), `SetCursor("POINT_ERROR_CURSOR")`.
    UnablePoint,
    /// UnableAttack (24), `SetCursor("ATTACK_ERROR_CURSOR")`.
    UnableAttack,
    /// Custom (41): the texture `SetCursor` loaded by any other name (`0x523c40`), its image
    /// [`super::UiScript::custom_cursor_name`].
    Custom,
}

/// `CursorSetCustom` (`0x523c40`): the name as it stands is a texture path (`0x449610`, `.blp` put
/// on, `0x523c86`) that must be 32×32 (`0x523cb9`, `0x523cbd`); it becomes mode 41 (`0x523cc9`),
/// its image the slot-41 picture (`0xc0bd80`), and its name is kept (`0x523cf5`, 259 bytes and a
/// NUL), answering true. A load failure or another size answers false and changes nothing. The
/// same name, ignoring case, while mode 41 shows answers true at once (`0x523c46`-`0x523c6d`);
/// here it also re-asserts the mode, since the world classifier, the other writer of the shown
/// mode, lives in the host.
fn custom_cursor(model: &mut super::Model, name: &str) -> bool {
    if model.ui_cursor == Some(UiCursorMode::Custom)
        && model.custom_cursor_name.eq_ignore_ascii_case(name)
    {
        model.ui_cursor_dirty = true;
        return true;
    }
    let size = model
        .texture_size_probe
        .as_ref()
        .and_then(|probe| probe(name));
    if size != Some((32, 32)) {
        return false;
    }
    model.ui_cursor = Some(UiCursorMode::Custom);
    model.ui_cursor_dirty = true;
    let mut end = name.len().min(0x103);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    model.custom_cursor_name = name[..end].to_string();
    true
}

/// `UseContainerItem` past its clear. An empty slot or a locked item ends it, repair mode
/// included (`0x4fa1c7`, `0x4fa1cd`); then repair mode repairs (`0x4fa1da`). Every other arm takes
/// its lock here, in the call, so a later call reads it as the reference's does: the withdraw and
/// deposit (`0x4fa312`, `0x4fa34b`), the sale (`0x4fa37f`), the open (`0x5edcd9`) and the wrap
/// paper (`0x5edea0`). The equip arm is not done here: it returns its item, which the call puts on
/// the cursor ([`use_equip_arm`]). The app sends each arm, from the call left in `call`.
fn use_cleared(
    model: &mut super::Model,
    bag: i64,
    slot: u32,
    on_self: bool,
    may_cast: bool,
    call: &mut Option<super::ScriptCall>,
) -> Option<CursorItem> {
    let s = model
        .containers
        .get(&bag)
        .and_then(|c| c.slots.get(&slot))
        .filter(|s| s.item_id != 0 && !cursor::item_locked(model, s.guid))?;
    let (guid, fork, item_id, use_spell) = (s.guid, s.use_fork, s.item_id, s.use_spell);
    let equip = (fork == UseFork::Equip).then(|| s.cursor_item(bag, slot));
    if model.repair_mode {
        model.container_repairs.push((bag, slot));
        return None;
    }
    let arm = if bag == -1 || (5..=10).contains(&bag) {
        UseArm::Withdraw
    } else if model.bank.is_some() {
        UseArm::Deposit
    } else if model.merchant.is_some() {
        UseArm::Sell
    } else {
        UseArm::Item(fork)
    };
    if let (UseArm::Item(UseFork::Equip), Some(item)) = (arm, equip) {
        // Without control the equip arm returns before it locks or sends, unless on a taxi
        // (`0x4fa3ce`-`0x4fa3ec`).
        return cursor::equip::has_cursor_control(model).then_some(item);
    }
    // The use dispatcher's gates (`0x5d8d00`), each showing its line and ending the call before
    // the arm's lock (`0x5edcd9`): the requirement check ahead of the read, letter, open and
    // plain arms, the control gate ahead of the open and plain ones.
    if let UseArm::Item(fork) = arm {
        let known = model
            .containers
            .get(&bag)
            .and_then(|c| c.slots.get(&slot))
            .is_some_and(|s| s.item_class.is_some());
        if let Some(key) =
            super::use_gates::item_use_refusal(model, fork, item_id, use_spell, known)
        {
            model.push_ui_error(key);
            return None;
        }
    }
    *call = Some(super::ScriptCall::UseContainerItem(ContainerUse {
        bag,
        slot,
        guid,
        arm,
        asks: false,
        on_self,
        may_cast,
    }));
    match arm {
        UseArm::Withdraw | UseArm::Deposit | UseArm::Sell | UseArm::Item(UseFork::Open) => {
            cursor::lock_item(model, guid);
        }
        UseArm::Item(UseFork::WrapPaper) => arm_gift_wrap(model, bag, slot),
        UseArm::Item(_) => {}
    }
    None
}

/// The equip arm (`0x4fa3b9`): `0x5e1840` puts the item on the cursor and runs
/// `AutoEquipCursorItem` (`0x4fa3ff`), then the item locks unless it is a projectile
/// (`0x4fa40b`, `0x4fa418`), whose clear unlocked it instead (`0x5e1711`). A bind-on-equip item
/// sends nothing: it stays held and locked under the question.
fn use_equip_arm(lua: &Lua, bag: i64, slot: u32, item: CursorItem, on_self: bool, may_cast: bool) {
    let guid = item.guid;
    let projectile = item.item_class == Some(cursor::ITEM_CLASS_PROJECTILE);
    let step = cursor::equip::auto_equip_from(lua, item, false);
    super::tick::verb(lua, |model| {
        if !projectile {
            cursor::lock_item(model, guid);
        }
    });
    // An item the verb refused (in the trade window) sends nothing, but still locks (`0x4fa418`).
    if matches!(
        step,
        cursor::equip::AutoEquip::Sent { .. } | cursor::equip::AutoEquip::Asked { .. }
    ) {
        super::calls::submit(
            lua,
            super::ScriptCall::UseContainerItem(ContainerUse {
                bag,
                slot,
                guid,
                arm: UseArm::Item(UseFork::Equip),
                asks: matches!(step, cursor::equip::AutoEquip::Asked { .. }),
                on_self,
                may_cast,
            }),
        );
    }
}

/// The wrap arm `0x5edea0`: the paper at `(bag, slot)` locks, the cursor takes mode 2 (the cast
/// art, table `0x853b88`), and nothing is sent.
fn arm_gift_wrap(model: &mut super::Model, bag: i64, slot: u32) {
    let guid = model
        .containers
        .get(&bag)
        .and_then(|c| c.slots.get(&slot))
        .map_or(0, |s| s.guid);
    model.pending_wrap = Some(PendingWrap { bag, slot, guid });
    model.ui_cursor = Some(UiCursorMode::Cast);
    model.ui_cursor_dirty = true;
    cursor::lock_item(model, guid);
}

/// `PickupContainerItem`'s gesture (`0x4f9b30`):
/// - an empty cursor, or one holding a spell, macro, pet action, stabled pet or coins, picks up a
///   resolved item whose slot is not locked, the pickup's clear dropping that payload;
/// - a click on the held item's own slot cancels;
/// - a whole stack placed anywhere else queues the move and clears the cursor; a swap lands the
///   displaced item in the source slot, never on the cursor (no `SetCursorItem`, `0x5e0c40`);
/// - a split carry places onto an empty or same-item slot, and stays held over another item;
/// - a vendor row buys into the slot, and an item off the bar is cleared.
///
/// Returns the item a pickup puts on the cursor, which the binding sets past this segment
/// ([`cursor::pick_up_item`]), so the setter's inner clear signals on its own.
fn pickup_container_item(
    model: &mut super::Model,
    bag: i64,
    slot: u32,
    may_cast: bool,
) -> Option<CursorItem> {
    // An armed gift wrap completes here, before the cursor is read. `PickupContainerItem` is the
    // only caller of the wrap sender `0x5edfc0`, so an equipped item cannot be wrapped.
    if let Some(wrap) = model.pending_wrap {
        // An empty or unresolved target leaves the wrap armed, as in the reference.
        if !model
            .containers
            .get(&bag)
            .and_then(|c| c.slots.get(&slot))
            .is_some_and(|s| s.item_id != 0)
        {
            return None;
        }
        // A vanished paper also bails, the wrap armed and the paper locked (`0x5edfef`).
        if !model
            .containers
            .get(&wrap.bag)
            .and_then(|c| c.slots.get(&wrap.slot))
            .is_some_and(|s| s.item_id != 0)
        {
            return None;
        }
        // No eligibility test: the server refuses with an `ERR_CANT_WRAP_*` reason
        // (`SMSG_INVENTORY_CHANGE_FAILURE` 43-48), and a local gate would eat that error line.
        model.pending_wrap = None;
        model.container_wraps.push((wrap.bag, wrap.slot, bag, slot));
        // The paper unlocks and the cursor resets right after the send (`0x5eded0` at
        // `0x5ee0aa`), not on the answer; the target never locks.
        cursor::unlock_item(model, wrap.guid);
        model.ui_cursor = None;
        model.ui_cursor_dirty = true;
        return None;
    }
    // Only a held item (`0x4f9c38`) or a held row, a vendor's or the bar's (`0x4f9c43`), takes the
    // click first. Past them an empty slot does nothing (`0x4f9c4e`), an armed item-targeting
    // spell binds the item (`0x4f9c54`) and repair mode repairs it (`0x4f9c7b`), any other payload
    // staying held; the app's repair drain runs the arm's `ITEM_REPAIR` and send. Else the item is
    // picked up, its setter's clear dropping whatever else is held (`0x4f9de9`).
    let placing = matches!(
        model.cursor,
        Some(CursorPayload::Item(_) | CursorPayload::Merchant(_) | CursorPayload::Action(_))
    );
    if !placing && (model.item_pick_armed || model.repair_mode) {
        let occupied = model
            .containers
            .get(&bag)
            .and_then(|c| c.slots.get(&slot))
            .is_some_and(|s| s.item_id != 0);
        if occupied && model.item_pick_armed {
            model.item_picks.push(cursor::ItemPick {
                bag,
                slot,
                may_cast,
            });
        } else if occupied {
            model.container_repairs.push((bag, slot));
        }
        return None;
    }
    match model.cursor.clone() {
        // The lock gate reads the item's own bit (`0x4f9dc4`), the app's or the client's.
        None
        | Some(
            CursorPayload::Spell(_)
            | CursorPayload::Macro(_)
            | CursorPayload::PetAction(_)
            | CursorPayload::StablePet(_)
            | CursorPayload::Money(_),
        ) => model
            .containers
            .get(&bag)
            .and_then(|c| c.slots.get(&slot))
            .filter(|s| s.item_id != 0 && !cursor::item_locked(model, s.guid))
            .map(|s| s.cursor_item(bag, slot)),
        // The cancel is `ClearCursor(1,1)` (`0x4f9e0e`): the unlock, then `CURSOR_UPDATE`.
        Some(CursorPayload::Item(held)) if held.bag == bag && held.slot == slot => {
            cursor::clear_cursor(model);
            None
        }
        Some(CursorPayload::Item(held)) => {
            // Cloned so the moves below can borrow `model`; an unresolved slot reads as empty.
            let dest = model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .cloned()
                .filter(|d| d.item_id != 0);
            match (held.count, dest) {
                // A split carry cannot swap with a different item: it stays held.
                (Some(_), Some(d)) if d.item_id != held.item_id => {}
                // A split places and clears (`0x5e1210`, then `ClearCursor(0,1)` at `0x4f9e4f`):
                // the grid hide and `CURSOR_UPDATE`, and no lock event.
                (Some(n), _) => {
                    queue_move(model, &held, bag, slot, Some(n));
                    cursor::clear(model, cursor::Clear::KEEP_LOCK);
                }
                // A whole stack locks the destination's item first (`0x4f9e85`; an empty slot's
                // zero guid signals nothing, `0x4953ed`), then the swap send clears
                // (`ClearCursor(0,1)` at `0x5e0fe7`: the grid hide, `CURSOR_UPDATE`) and the
                // binding clears again (`0x4f9eb8`), a second `CURSOR_UPDATE` on the empty cursor.
                // The source keeps its lock in both: the clears pass no unlock.
                // A locked occupant refuses the whole stack, the payload kept (`0x4f9e76`); the
                // split arm above never reads it.
                (None, Some(d)) if cursor::item_locked(model, d.guid) => {}
                (None, dest) => {
                    queue_move(model, &held, bag, slot, None);
                    cursor::lock_item(model, dest.map_or(0, |d| d.guid));
                    cursor::clear(model, cursor::Clear::KEEP_LOCK);
                    cursor::clear(model, cursor::Clear::KEEP_LOCK);
                }
            }
            None
        }
        // Mode 5, a held vendor row, buys into this slot whatever it holds (`0x4f9efa` tests
        // `[0xb4d900] == 5`): `CMSG_BUY_ITEM_IN_SLOT` (0x1a3) with count 1, and the cursor clears
        // either way; with no vendor open the row stays held (`0x4f9f0f`). Deviation: a row that
        // no longer resolves is refused, as `0x4c7300` does, because this path dereferences it
        // unchecked (`0x4f9f35`), a latent null deref.
        Some(CursorPayload::Merchant(_)) if model.merchant.is_none() => None,
        Some(CursorPayload::Merchant(held)) => {
            let entry = model
                .merchant
                .as_ref()
                .and_then(|m| m.items.get(held.row as usize))
                .filter(|it| it.item_id == held.item_id)
                .map(|it| it.item_id);
            if let Some(entry) = entry {
                model.merchant_slot_buys.push((bag, slot, entry));
            }
            cursor::clear_cursor(model);
            None
        }
        // Mode 7, an item off the bar, is a held row too, and only cleared (`0x4f9f41`).
        Some(CursorPayload::Action(_)) => {
            cursor::clear_cursor(model);
            None
        }
    }
}

/// Queue one move, the shared tail of every clearing branch above.
fn queue_move(
    model: &mut super::Model,
    held: &CursorItem,
    dst_bag: i64,
    dst_slot: u32,
    count: Option<u32>,
) {
    model.container_moves.push(ContainerMove {
        src_bag: held.bag,
        src_slot: held.slot,
        dst_bag,
        dst_slot,
        count,
    });
}

/// The `(bag, slot)` the three container getters resolve (`0x4f9820`): both are numbers (else the
/// binding's usage error), the slot is `trunc(n) - 1` and fails the resolver when negative
/// (`0x4f990d`), so a zero or negative slot is no slot at all. `None` is that failure. The slot
/// is returned 1-based, as the snapshot keys it.
fn container_key(
    lua: &Lua,
    bag: Value,
    slot: Value,
    usage: &'static str,
) -> mlua::Result<Option<(i64, u32)>> {
    let bag = number_arg(lua, bag, usage)?;
    let slot = number_arg(lua, slot, usage)?;
    Ok((slot >= 1).then_some((i64::from(bag), slot as u32)))
}

/// Register the container verbs as bare globals, as `reference/lua_globals.tsv` lists them;
/// `C_Container` is a later client's namespace, and an addon that finds it assumes that client.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // ContainerIDToInventoryID(containerID) (`0x4f94e0`): `id - 1`, then +20 below 4 else +60, so
    // the keyring -2 → 17, the backpack 0 → 19, bags 1-4 → 20-23 and bank bags 5-10 → 64-69, with
    // no special case and no range check.
    lua.globals().set(
        "ContainerIDToInventoryID",
        lua.create_function(|lua, id: Value| {
            let id = super::binding_abi::number_arg(
                lua,
                id,
                "Usage: ContainerIDToInventoryID(containerID)",
            )?;
            // The reference's steps and signed compare (`0x4f9524 jl`), wrapping at the `i32` edge.
            let t = id.wrapping_sub(1);
            Ok(i64::from(if t < 4 {
                t.wrapping_add(20)
            } else {
                t.wrapping_add(60)
            }))
        })?,
    )?;

    // SetBagPortaitTexture(texture, containerID) (`0x4fa4f0`, the reference's spelling): 0 and
    // below return before the clear (`0x4fa5b9`), 1-10 clear and then set the bag's icon, 11 and
    // up raise. The reference also needs the bank open for 5-10, which is not checked here.
    lua.globals().set(
        "SetBagPortaitTexture",
        lua.create_function(|lua, (region, container): (Table, Value)| {
            // `is-number` else `Usage: SetBagPortaitTexture(texture, slot)` (`0x84d9c4`,
            // `0x4fa58e`).
            let container = i64::from(binding_abi::number_arg(
                lua,
                container,
                "Usage: SetBagPortaitTexture(texture, slot)",
            )?);
            if container >= 11 {
                return Err(mlua::Error::runtime("Invalid slot in SetBagPortaitTexture"));
            }
            if container <= 0 {
                return Ok(());
            }
            // `ContainerIDToInventoryID`'s arithmetic, inlined: an addon may replace that global,
            // and the reference's portrait read does not go through Lua.
            let t = container.wrapping_sub(1);
            let inv = if t < 4 {
                t.wrapping_add(20)
            } else {
                t.wrapping_add(60)
            };
            let path = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                usize::try_from(inv)
                    .ok()
                    .and_then(|slot| model.inv_slot("player", slot))
                    .and_then(|v| v.icon.clone())
            };
            let rh = crate::script::region::region_handle_of(lua, &region)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // An empty slot's `None` is the answer: the reference clears first, then sets.
            let set = path.map_or(super::texel::TexSet::None, super::texel::TexSet::Portrait);
            model.set_texel(rh, set);
            let data = model.region_data.entry(rh).or_default();
            data.circular = true;
            data.portrait_unit = None;
            model.note_art(rh);
            Ok(())
        })?,
    )?;

    lua.globals().set(
        "GetContainerNumSlots",
        lua.create_function(|lua, bag: Value| {
            // `is-number` else `Usage: GetContainerNumSlots(index)` (`0x84d83c`, `0x4f9577`).
            let bag = i64::from(binding_abi::number_arg(
                lua,
                bag,
                "Usage: GetContainerNumSlots(index)",
            )?);
            // The backpack, the bank and bags 1-10 only; the keyring and anything else answer 0
            // (`0x4f95c4`, `0x4f95de`, `0x4f95fb`).
            if !(-1..=10).contains(&bag) {
                return Ok(0);
            }
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.containers.get(&bag).map_or(0, |c| c.num_slots))
        })?,
    )?;

    lua.globals().set(
        "GetBagName",
        lua.create_function(|lua, bag: Value| {
            // `is-number` else `Usage: GetBagName(index)` (`0x84da04`, `0x4fa69a`).
            let bag = i64::from(binding_abi::number_arg(
                lua,
                bag,
                "Usage: GetBagName(index)",
            )?);
            // Nil with no active player (`0x4fa6cf`), `BACKPACK_TOOLTIP` for the backpack
            // (`0x4fa6f0`), the bag item's name for 1-10, nil for the bank, the keyring and the
            // rest (`0x4fa712`, `0x4fa717`).
            let name = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                match model.containers.get(&bag) {
                    Some(_) if bag == 0 => {
                        Some(super::glue_core::global_text(lua, "BACKPACK_TOOLTIP"))
                    }
                    Some(c) if (1..=10).contains(&bag) => c.name.clone(),
                    _ => None,
                }
            };
            match name {
                Some(n) => Ok(Value::String(lua.create_string(&n)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetContainerItemCooldown(bag, slot) (`0x4f99b0`) → start, duration, enable on the `GetTime`
    // clock, as `GetActionCooldown`; enable 0 is a held cooldown. No resolved slot or no item in it
    // pushes three zeros, the enable included (`0x4f9ae9`), and so does the slot's own answer when
    // it is all zero ([`item_cooldown_triple`]). Unlike the worn getter it asks no use-spell test
    // of the template (`0x4f9a48`).
    lua.globals().set(
        "GetContainerItemCooldown",
        lua.create_function(|lua, (bag, slot): (Value, Value)| {
            let now = crate::script::clock::now(lua);
            let key = container_key(
                lua,
                bag,
                slot,
                "Usage: GetContainerItemCooldown(index, slot)",
            )?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let held = key.is_some_and(|k| {
                model
                    .containers
                    .get(&k.0)
                    .is_some_and(|c| c.slots.contains_key(&k.1))
            });
            Ok(if held {
                item_cooldown_triple(
                    key.and_then(|k| model.container_cooldowns.get(&k).copied()),
                    now,
                )
            } else {
                (0.0, 0.0, 0)
            })
        })?,
    )?;

    lua.globals().set(
        "GetContainerItemLink",
        lua.create_function(|lua, (bag, slot): (Value, Value)| {
            let key = container_key(lua, bag, slot, "Usage: GetContainerItemLink(index, slot)")?;
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                key.and_then(|(bag, slot)| {
                    model
                        .containers
                        .get(&bag)
                        .and_then(|c| c.slots.get(&slot))
                        .and_then(|s| s.link.clone())
                })
            };
            // Nothing at all on every miss: the resolver failing, no item object or no template
            // name (`0x4f996b`, `0x4f9979`, `0x4f99a5`).
            match link {
                Some(l) => Ok(MultiValue::from_vec(vec![Value::String(
                    lua.create_string(&l)?,
                )])),
                None => Ok(MultiValue::new()),
            }
        })?,
    )?;

    // GetContainerItemInfo(bag, slot) (`0x4f9670`) → texture, itemCount, locked, quality,
    // readable, the five `ContainerFrame.lua:241` reads; no values for an empty or unknown slot.
    // The texture is the question mark while the template is unknown.
    // `locked` and `readable` are 1 or nil (`0x4f975d`/`0x4f976b`, `0x4f97e6`/`0x4f97d3`).
    lua.globals().set(
        "GetContainerItemInfo",
        lua.create_function(|lua, (bag, slot): (Value, Value)| {
            let Some((bag, slot)) =
                container_key(lua, bag, slot, "Usage: GetContainerItemInfo(index, slot)")?
            else {
                // The resolver failed (`0x4f990d`): nothing is pushed (`0x4f9810`).
                return Ok(mlua::MultiValue::new());
            };
            let (info, locked) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let info = model
                    .containers
                    .get(&bag)
                    .and_then(|c| c.slots.get(&slot))
                    .cloned();
                let locked = info
                    .as_ref()
                    .is_some_and(|s| cursor::item_locked(&model, s.guid));
                (info, locked)
            };
            let Some(s) = info else {
                return Ok(mlua::MultiValue::new());
            };
            Ok(mlua::MultiValue::from_vec(vec![
                // A string on every path: the question mark while the template is unknown
                // (`0x5d8af0`).
                Value::String(
                    lua.create_string(
                        s.texture
                            .as_deref()
                            .unwrap_or(super::item_stats::MISSING_ITEM_ICON),
                    )?,
                ),
                Value::Integer(i64::from(s.count)),
                // The item's lock bit (`0x4f9752`, [`cursor::item_locked`]).
                flag(locked),
                // A number on every path: -1 unless the template is cached and equippable
                // (`0x4f9794`).
                Value::Integer(super::item_stats::gated_quality(
                    s.quality,
                    s.inventory_type,
                )),
                flag(s.readable),
            ]))
        })?,
    )?;

    lua.globals().set(
        "UseContainerItem",
        lua.create_function(|lua, (bag, slot, rest): (Value, Value, mlua::MultiValue)| {
            // `is-number` on args 1 and 2 else `Usage: UseContainerItem(index, slot[, onSelf])`
            // (`0x84d940`, `0x4fa440`).
            let bag = i64::from(binding_abi::number_arg(
                lua,
                bag,
                "Usage: UseContainerItem(index, slot[, onSelf])",
            )?);
            let slot = binding_abi::index_arg32(
                lua,
                slot,
                "Usage: UseContainerItem(index, slot[, onSelf])",
            )?;
            // `onSelf` through the client's flag coercion (`0x4fa17d`, `0x6f1c10`, default 0).
            let on_self = super::binding_abi::bool_or_default(rest.front(), false);
            // Every right-click clears the cursor before it reads the slot (`0x4fa198`): a held
            // payload goes back, never placed, and an armed gift wrap is cancelled. The clear's
            // events run first, as the reference's do before it reads the slot.
            super::tick::verb(lua, cursor::clear_cursor);
            // The item's on-use spell casts through `TryCastSpell`, behind the gate's tag 2.
            let may_cast = protected::gate(lua, Tag::CAST);
            let mut call = None;
            let equip = super::tick::verb(lua, |model| {
                use_cleared(model, bag, slot, on_self, may_cast, &mut call)
            });
            if let Some(call) = call {
                super::calls::submit(lua, call);
            }
            if let Some(item) = equip {
                use_equip_arm(lua, bag, slot, item, on_self, may_cast);
            }
            Ok(())
        })?,
    )?;

    // PickupContainerItem(bag, slot) (`0x4f9b30`): pick up, or place and swap; no return value on
    // any path.
    lua.globals().set(
        "PickupContainerItem",
        lua.create_function(|lua, (bag, slot): (Value, Value)| {
            // `is-number` on args 1 and 2 else `Usage: PickupContainerItem(index, slot)`
            // (`0x84d8e8`, `0x4f9f56`).
            let bag = i64::from(binding_abi::number_arg(
                lua,
                bag,
                "Usage: PickupContainerItem(index, slot)",
            )?);
            let slot =
                binding_abi::index_arg32(lua, slot, "Usage: PickupContainerItem(index, slot)")?;
            // An armed item-targeting spell binds the item through `BindSpellTargetObject`, behind
            // the gate's tag 2.
            let may_cast = protected::gate(lua, Tag::CAST);
            if let Some(item) = super::tick::verb(lua, |model| {
                pickup_container_item(model, bag, slot, may_cast)
            }) {
                cursor::pick_up_item(lua, item);
            }
            Ok(())
        })?,
    )?;

    // ShowContainerSellCursor(bag, slot) (`0x4fa460`): Buy (3) over an occupied, unlocked slot
    // (`0x4fa4bf`), with no price check, so selling never greys; an empty or locked slot leaves
    // the cursor alone.
    lua.globals().set(
        "ShowContainerSellCursor",
        lua.create_function(|lua, (bag, slot): (Value, Value)| {
            // `is-number` on args 1 and 2 else `Usage: ShowContainerSellCursor(index, slot)`
            // (`0x84d970`, `0x4fa4d9`).
            let bag = i64::from(binding_abi::number_arg(
                lua,
                bag,
                "Usage: ShowContainerSellCursor(index, slot)",
            )?);
            let slot =
                binding_abi::index_arg32(lua, slot, "Usage: ShowContainerSellCursor(index, slot)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let occupied = model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .is_some_and(|s| !cursor::item_locked(&model, s.guid));
            // An armed spell or repair cursor suppresses it first; each is sticky, so a Buy here
            // would stamp out the active base cursor.
            if occupied && !model.spell_targeting && !model.repair_mode {
                model.ui_cursor = Some(UiCursorMode::Buy);
                model.ui_cursor_dirty = true;
            }
            Ok(())
        })?,
    )?;
    // ShowInspectCursor() (`0x48ac60`): Inspect (7), unconditionally.
    lua.globals().set(
        "ShowInspectCursor",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.ui_cursor = Some(UiCursorMode::Inspect);
            model.ui_cursor_dirty = true;
            Ok(())
        })?,
    )?;
    // HasKey() (`0x48ae90`) → the number 1 if the player owns a key (`BagFamily` 9) anywhere, else
    // nil, never a boolean; the app does the search.
    lua.globals().set(
        "HasKey",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if model.has_key {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;
    // SetCursor(name) (`0x489490`) returns nothing for nil, after the reset to the base mode
    // (`0x489518`); anything `lua_tostring` cannot convert, no argument included, raises its usage
    // line (`0x489537`). A number converts to its text. The name matches one of eight pairs
    // ignoring case (`0x48955b`), POINT, CAST, BUY and ATTACK as modes 1-4 and each `*_ERROR` 20
    // higher, and answers 1 (`0x48958f` → `0x489575`). Any other name is a custom cursor
    // (`0x48956c`, [`custom_cursor`]): 1 when it loads, else nil with the cursor unchanged
    // (`0x48959c`).
    lua.globals().set(
        "SetCursor",
        lua.create_function(|lua, args: MultiValue| {
            let first = args.into_iter().next();
            if matches!(first, Some(Value::Nil)) {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.ui_cursor = None;
                model.ui_cursor_dirty = true;
                return Ok(MultiValue::new());
            }
            let name = first
                .as_ref()
                .and_then(|v| super::binding_abi::optional_string(lua, v))
                .ok_or_else(|| mlua::Error::runtime("Usage: SetCursor(\"cursor\" or nil)"))?;
            const NAMED: [(&str, UiCursorMode); 8] = [
                ("POINT_CURSOR", UiCursorMode::Point),
                ("CAST_CURSOR", UiCursorMode::Cast),
                ("BUY_CURSOR", UiCursorMode::Buy),
                ("ATTACK_CURSOR", UiCursorMode::Attack),
                ("POINT_ERROR_CURSOR", UiCursorMode::UnablePoint),
                ("CAST_ERROR_CURSOR", UiCursorMode::CastError),
                ("BUY_ERROR_CURSOR", UiCursorMode::UnableBuy),
                ("ATTACK_ERROR_CURSOR", UiCursorMode::UnableAttack),
            ];
            let Some(&(_, mode)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) else {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let loaded = custom_cursor(&mut model, &name);
                return Ok(MultiValue::from_vec(vec![if loaded {
                    Value::Integer(1)
                } else {
                    Value::Nil
                }]));
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.ui_cursor = Some(mode);
            model.ui_cursor_dirty = true;
            Ok(MultiValue::from_vec(vec![Value::Integer(1)]))
        })?,
    )?;
    // ResetCursor() (`0x48ac70` → `0x523d30`): back to the base mode, whatever set the override.
    lua.globals().set(
        "ResetCursor",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.ui_cursor = None;
            model.ui_cursor_dirty = true;
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ContainerMove, ContainerSlot, ContainerState, PendingWrap, UiCursorMode};
    use crate::script::cursor::split_container_item;
    use crate::script::{MerchantItem, MerchantState, UiScript};

    /// `SetCursor` (`0x489490`): nil resets and returns nothing, no argument or a value with no
    /// text raises the usage line, the eight names match without case and answer 1, and any other
    /// name answers nil and leaves the cursor alone.
    #[test]
    fn set_cursor_takes_the_eight_names_and_raises_its_usage_line() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("SetCursor(\"attack_cursor\")").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return SetCursor(\"attack_cursor\")")
                .unwrap(),
            1
        );
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Attack));
        for (name, mode) in [
            ("POINT_CURSOR", UiCursorMode::Point),
            ("Point_Error_Cursor", UiCursorMode::UnablePoint),
            ("CAST_CURSOR", UiCursorMode::Cast),
            ("cast_error_cursor", UiCursorMode::CastError),
            ("BUY_CURSOR", UiCursorMode::Buy),
            ("BUY_ERROR_CURSOR", UiCursorMode::UnableBuy),
            ("ATTACK_ERROR_CURSOR", UiCursorMode::UnableAttack),
        ] {
            s.run(&format!("R = SetCursor(\"{name}\")")).unwrap();
            assert_eq!(s.eval::<i64>("return R").unwrap(), 1, "{name}");
            assert_eq!(s.ui_cursor(), Some(mode), "{name}");
        }
        // Another name is a custom cursor; this one does not load: nil, the cursor as it was.
        assert_eq!(s.arity("SetCursor(\"Nope\")").unwrap(), 1);
        assert!(s.eval::<bool>("return SetCursor(\"Nope\") == nil").unwrap());
        assert!(s.eval::<bool>("return SetCursor(5) == nil").unwrap());
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::UnableAttack));
        // Nil is the reset, with no value back.
        assert_eq!(s.arity("SetCursor(nil)").unwrap(), 0);
        assert_eq!(s.ui_cursor(), None);
        for bad in ["SetCursor()", "SetCursor({})", "SetCursor(true)"] {
            let err = s.run(bad).unwrap_err().to_string();
            assert!(
                err.contains("Usage: SetCursor(\"cursor\" or nil)"),
                "{bad}: {err}"
            );
        }
        assert_eq!(s.ui_cursor(), None, "a raise changes nothing");
    }

    /// Any other name is a texture path (`0x523c40` → `0x449610`, no folder put on): a 32×32 one
    /// becomes mode 41 under its name and answers 1; a missing one or another size answers nil and
    /// leaves the cursor and the name as they were; the shown name again answers 1 at once.
    #[test]
    fn set_cursor_loads_any_other_name_as_a_32_by_32_texture() {
        let mut s = UiScript::new().unwrap();
        s.set_texture_size_probe(Box::new(|path| match path {
            "Interface\\AddOns\\Mine\\Sword" | "5" => Some((32, 32)),
            "Interface\\Icons\\Big" => Some((64, 64)),
            _ => None,
        }));
        let set = |s: &UiScript, name: &str| {
            s.eval::<Option<i64>>(&format!("return SetCursor({name})"))
                .unwrap()
        };
        assert_eq!(set(&s, r#""Interface\\AddOns\\Mine\\Sword""#), Some(1));
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Custom));
        assert_eq!(s.custom_cursor_name(), "Interface\\AddOns\\Mine\\Sword");
        assert_eq!(s.take_cursor_write(), Some(Some(UiCursorMode::Custom)));
        // The shown name, ignoring case, answers 1 without a load.
        assert_eq!(set(&s, r#""INTERFACE\\ADDONS\\MINE\\SWORD""#), Some(1));
        assert_eq!(s.custom_cursor_name(), "Interface\\AddOns\\Mine\\Sword");
        s.run("SetCursor(\"ATTACK_CURSOR\")").unwrap();
        for bad in [
            r#""Interface\\Icons\\Big""#,
            r#""Interface\\Cursor\\Nothing""#,
        ] {
            assert_eq!(set(&s, bad), None, "{bad}");
            assert_eq!(s.ui_cursor(), Some(UiCursorMode::Attack), "{bad}");
            assert_eq!(s.custom_cursor_name(), "Interface\\AddOns\\Mine\\Sword");
        }
        // A number is its text (`0x6f36b4`), a path like any other.
        assert_eq!(set(&s, "5"), Some(1));
        assert_eq!(s.custom_cursor_name(), "5");
    }

    fn backpack() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                use_fork: Default::default(),
                item_class: None,
                duration_ms: None,
                petition: None,
                already_bound: false,
                occupied_seats: None,
                bonding: 0,
                use_spell: 0,
                bar_placeable: true,
                consumable: false,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
                count: 5,
                quality: Some(1),
                inventory_type: 0,
                item_id: 117,
                link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
                guid: 0x10a,
                equip_slots: Vec::new(),
                cooldown: None,
                spell_0: false,
                spell_charges: [0; 5],
                readable: false,
                creator: None,
                gift_creator: None,
                creator_guid: 0,
                gift_creator_guid: 0,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        // An in-flight slot: the create arrived, the template answer hasn't.
        slots.insert(3, ContainerSlot::default());
        // A readable letter, a mail copy with item text.
        slots.insert(
            4,
            ContainerSlot {
                item_id: 8383,
                count: 1,
                readable: true,
                ..Default::default()
            },
        );
        ContainerState {
            name: None,
            num_slots: 16,
            slots,
        }
    }

    #[test]
    fn an_armed_gift_wrap_spends_on_the_next_container_click() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.arm_gift_wrap(0, 1);
        assert_eq!(
            s.gift_wrap_armed(),
            Some(PendingWrap {
                bag: 0,
                slot: 1,
                guid: 0x10a
            })
        );
        assert!(s.take_container_wraps().is_empty(), "arming sends nothing");
        assert_eq!(s.take_cursor_write(), Some(Some(UiCursorMode::Cast)));
        assert!(
            s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the paper dims while the wrap stands"
        );
        assert!(
            !s.eval::<bool>("return CursorHasItem()").unwrap(),
            "the wrap writes no payload, so every stock CursorHasItem() gate stays shut"
        );
        // A click on an empty slot leaves the wrap armed.
        s.eval::<()>("PickupContainerItem(0, 9)").unwrap();
        assert_eq!(
            s.gift_wrap_armed(),
            Some(PendingWrap {
                bag: 0,
                slot: 1,
                guid: 0x10a
            })
        );
        assert!(s.take_container_wraps().is_empty());
        // A click on a real item spends it: the paper's pair leads, as the wire does.
        s.eval::<()>("PickupContainerItem(0, 4)").unwrap();
        assert_eq!(s.take_container_wraps(), vec![(0, 1, 0, 4)]);
        assert_eq!(s.gift_wrap_armed(), None);
        assert_eq!(
            s.take_cursor_write(),
            Some(None),
            "the cursor resets on the send, not on the server's answer"
        );
        assert!(
            s.cursor_payload().is_none(),
            "the completing click is spent on the wrap, not on picking the target up"
        );
        assert!(
            !s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the paper unlocks on the send (`0x5ededd`)"
        );
    }

    /// `ClearCursor` (`0x495190`) tests for an armed wrap first, whatever its arguments.
    #[test]
    fn clear_cursor_cancels_an_armed_gift_wrap() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.arm_gift_wrap(0, 1);
        let _ = s.take_cursor_write();
        s.eval::<()>("ClearCursor()").unwrap();
        assert_eq!(s.gift_wrap_armed(), None);
        assert_eq!(s.take_cursor_write(), Some(None), "and the mode goes back");
        assert!(
            !s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the paper unlocks with the cancel — the lock never waited on a server"
        );
        s.eval::<()>("PickupContainerItem(0, 1)").unwrap();
        assert!(s.take_container_wraps().is_empty());
        assert!(s.cursor_payload().is_some(), "a plain pickup, as before");
    }

    #[test]
    fn container_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetContainerItemInfo(0, 1) == nil")
            .unwrap());

        s.set_container(0, Some(backpack()));
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 16);
        s.run("BACKPACK_TOOLTIP = 'Rucksack'").unwrap();
        assert_eq!(
            s.eval::<String>("return GetBagName(0)").unwrap(),
            "Rucksack",
            "the backpack is named by its global, not by the snapshot (`0x4fa6f0`)"
        );
        // The five values `ContainerFrame.lua:241` reads, in its order.
        let (icon, count, quality) = s
            .eval::<(String, i64, i64)>(
                "local texture, itemCount, locked, quality = GetContainerItemInfo(0, 1)\n\
                 return texture, itemCount, quality",
            )
            .unwrap();
        assert_eq!(icon, "Interface\\Icons\\INV_Misc_Food_16");
        // Food has InventoryType 0, so its quality is the -1 sentinel (`0x4f9794`).
        assert_eq!((count, quality), (5, -1));
        assert_eq!(
            s.eval::<i64>(
                "local _, _, id = string.find(GetContainerItemLink(0, 1), 'item:(%d+)') \
                 return tonumber(id)"
            )
            .unwrap(),
            117
        );
        // readable: the letter in 4, not the jerky in 1.
        assert!(!s
            .eval::<bool>("local _, _, _, _, readable = GetContainerItemInfo(0, 1) return readable")
            .unwrap());
        assert!(s
            .eval::<bool>("local _, _, _, _, readable = GetContainerItemInfo(0, 4) return readable")
            .unwrap());

        // An in-flight slot: the question mark, the -1 quality sentinel, itemCount real.
        assert!(s
            .eval::<bool>(
                "local texture, itemCount, locked, quality = GetContainerItemInfo(0, 3)\n\
                 return texture == 'Interface\\\\Icons\\\\INV_Misc_QuestionMark' \
                 and quality == -1 and itemCount ~= nil",
            )
            .unwrap());
        assert!(s
            .eval::<bool>("return GetContainerItemLink(0, 3) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetContainerItemInfo(0, 2) == nil")
            .unwrap());
    }

    /// The keyring, the bank and an out-of-range id answer no slots and no name, however the
    /// snapshot holds them (`0x4f95fb`, `0x4fa712`); the keyring is sized by `GetKeyRingSize`.
    #[test]
    fn keyring_and_bank_answer_no_slots_or_name() {
        let mut s = UiScript::new().unwrap();
        let named = |name: Option<&str>, num_slots| ContainerState {
            name: name.map(Into::into),
            num_slots,
            slots: std::collections::HashMap::new(),
        };
        s.set_container(-2, Some(named(Some("Keyring"), 8)));
        s.set_container(-1, Some(named(Some("Bank"), 24)));
        s.set_container(4, Some(named(Some("Linen Bag"), 6)));
        s.set_container(11, Some(named(Some("Stray"), 6)));
        let slots = |bag: i64| {
            s.eval::<i64>(&format!("return GetContainerNumSlots({bag})"))
                .unwrap()
        };
        assert_eq!(slots(-2), 0, "the keyring is no container to this verb");
        assert_eq!(slots(-1), 24);
        assert_eq!(slots(4), 6);
        assert_eq!(slots(11), 0);
        for bag in [-2, -1, 11] {
            assert!(
                s.eval::<bool>(&format!("return GetBagName({bag}) == nil"))
                    .unwrap(),
                "GetBagName({bag})"
            );
        }
        assert_eq!(
            s.eval::<String>("return GetBagName(4)").unwrap(),
            "Linen Bag"
        );
        assert!(
            s.eval::<bool>("return GetBagName(0) == nil").unwrap(),
            "no backpack, no active player"
        );
    }

    /// The fourth value is the template's quality only when the template is cached and its
    /// InventoryType is nonzero (`0x4f9794`, `0x4f979b`), else the number -1 (`0x4f97a6`): a potion,
    /// a reagent or conjured food answers -1 with its template in hand, as does an item whose
    /// template has not arrived. Never nil.
    #[test]
    fn container_item_info_quality_is_gated_on_the_inventory_type() {
        let mut s = UiScript::new().unwrap();
        let slot = |quality: Option<u32>, inventory_type: u32| ContainerSlot {
            count: 1,
            quality,
            inventory_type,
            item_id: 25,
            ..Default::default()
        };
        let mut slots = std::collections::HashMap::new();
        // An equippable epic, two InventoryType 0 items of quality 1 and 3, and an item whose
        // template is still in flight.
        slots.insert(1, slot(Some(4), 13));
        slots.insert(2, slot(Some(1), 0));
        slots.insert(3, slot(Some(3), 0));
        slots.insert(4, slot(None, 0));
        s.set_container(
            0,
            Some(ContainerState {
                slots,
                ..Default::default()
            }),
        );
        for (index, want) in [
            (1, "number:4"),
            (2, "number:-1"),
            (3, "number:-1"),
            (4, "number:-1"),
        ] {
            assert_eq!(
                s.eval::<String>(&format!(
                    "local c = function(...) return arg.n .. ':' .. type(arg[4]) .. ':' .. \
                     tostring(arg[4]) end return c(GetContainerItemInfo(0, {index}))"
                ))
                .unwrap(),
                format!("5:{want}"),
                "slot {index}"
            );
        }
    }

    /// The texture is a string on every path: while the template is unknown, `0x5d8af0` falls back
    /// to `Interface\Icons\INV_Misc_QuestionMark` (`0x5d88b0`, `0x847fe4`), not nil.
    #[test]
    fn container_item_info_texture_falls_back_to_the_question_mark() {
        let mut s = UiScript::new().unwrap();
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                count: 1,
                item_id: 25,
                ..Default::default()
            },
        );
        slots.insert(
            2,
            ContainerSlot {
                count: 1,
                item_id: 26,
                texture: Some("Interface\\Icons\\INV_Sword_01".into()),
                ..Default::default()
            },
        );
        s.set_container(
            0,
            Some(ContainerState {
                slots,
                ..Default::default()
            }),
        );
        for (index, want) in [
            (1, "string:Interface\\Icons\\INV_Misc_QuestionMark"),
            (2, "string:Interface\\Icons\\INV_Sword_01"),
        ] {
            assert_eq!(
                s.eval::<String>(&format!(
                    "local c = function(...) return type(arg[1]) .. ':' .. tostring(arg[1]) end \
                     return c(GetContainerItemInfo(0, {index}))"
                ))
                .unwrap(),
                want,
                "slot {index}"
            );
        }
    }

    /// `GetContainerItemInfo` (`0x4f9670`) pushes `locked` and `readable` as the number 1
    /// (`0x4f975d`, `0x4f97e6`) or nil (`0x4f976b`, `0x4f97d3`), five values in all.
    #[test]
    fn container_item_info_flags_are_one_or_nil() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        // `arg.n` counts the returns (`select` is 5.1's, not a 1.12 global).
        let shape = |s: &mut UiScript, slot: u32| {
            s.eval::<String>(&format!(
                "local function c(...) return arg.n .. ':' .. type(arg[3]) .. ':' .. tostring(arg[3]) \
                 .. ':' .. type(arg[5]) .. ':' .. tostring(arg[5]) end \
                 return c(GetContainerItemInfo(0, {slot}))"
            ))
            .unwrap()
        };
        assert_eq!(shape(&mut s, 1), "5:nil:nil:nil:nil", "a plain item");
        assert_eq!(
            shape(&mut s, 4),
            "5:nil:nil:number:1",
            "the readable letter"
        );
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(
            shape(&mut s, 1),
            "5:number:1:nil:nil",
            "the held item reads locked"
        );
    }

    /// The item's on-use spell casts through `TryCastSpell`, behind the gate's tag 2 (`0x4fa430` →
    /// `0x5d8d00` → `0x5d9258` → `0x6e5a90` → `0x6e4bc1`). The call keeps its other arms, so it is
    /// queued whatever the gate said, carrying the answer; an armed item-targeting spell's pick
    /// carries it the same way (`0x4f9c54` → `0x495d60` → `0x6e5b9e`).
    #[test]
    fn container_uses_and_picks_carry_what_the_gate_allowed() {
        let may_cast = |s: &mut UiScript| -> Vec<bool> {
            s.take_script_calls()
                .into_iter()
                .map(|c| c.may_cast())
                .collect()
        };
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("UseContainerItem(0, 1)").unwrap();
        s.run(r#"RunScript("UseContainerItem(0, 1)")"#).unwrap();
        s.run_binding(r#"RunScript("UseContainerItem(0, 1)")"#)
            .unwrap();
        assert_eq!(
            may_cast(&mut s),
            [true, false, true],
            "the client's own, a macro outside an input event, a macro inside a key's body"
        );

        s.set_item_pick_armed(true);
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.run(r#"RunScript("PickupContainerItem(0, 1)")"#).unwrap();
        s.run_binding(r#"RunScript("PickupContainerItem(0, 1)")"#)
            .unwrap();
        assert_eq!(
            s.take_item_picks()
                .iter()
                .map(|p| (p.bag, p.slot, p.may_cast))
                .collect::<Vec<_>>(),
            [(0, 1, true), (0, 1, false), (0, 1, true)]
        );
    }

    #[test]
    fn use_container_item_queues_intents() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("UseContainerItem(0, 1)").unwrap();
        s.run("UseContainerItem(0, 4, 'target')").unwrap();
        assert_eq!(s.take_container_uses(), vec![(0, 1), (0, 4)]);
        assert!(s.take_container_uses().is_empty(), "drained");
    }

    /// The arm is `0x4fa0e0`'s, in its order, and its lock is taken in the call: a bank item
    /// withdraws whether or not a banker is open (the bank flag, `0x4f98b3`), then an open banker
    /// deposits ahead of an open vendor's sale; an item with no lock-taking fork (a plain use)
    /// leaves it unlocked.
    #[test]
    fn use_container_item_takes_its_arm_and_the_arms_lock_in_the_call() {
        let mut s = UiScript::new().unwrap();
        let arm = |s: &mut UiScript, bag: i64| {
            s.run(&format!("UseContainerItem({bag}, 1)")).unwrap();
            let calls = s.take_script_calls();
            let [crate::script::ScriptCall::UseContainerItem(u)] = calls.as_slice() else {
                panic!("{calls:?}");
            };
            let locked = s.item_locked(u.guid);
            s.unlock_item(u.guid);
            (u.arm, locked)
        };
        s.set_container(0, Some(backpack()));
        s.set_container(-1, Some(backpack()));
        assert_eq!(
            arm(&mut s, 0),
            (super::UseArm::Item(super::UseFork::Plain), false)
        );
        assert_eq!(
            arm(&mut s, -1),
            (super::UseArm::Withdraw, true),
            "no banker open"
        );
        s.set_merchant(Some(super::super::MerchantState::default()));
        assert_eq!(arm(&mut s, 0), (super::UseArm::Sell, true));
        s.set_bank(Some(super::super::BankState::default()));
        assert_eq!(
            arm(&mut s, 0),
            (super::UseArm::Deposit, true),
            "the banker first"
        );
        assert_eq!(arm(&mut s, -1), (super::UseArm::Withdraw, true));
    }

    /// One backpack slot of `fork`, its item entry 871.
    fn forked_bag(fork: super::UseFork) -> ContainerState {
        ContainerState {
            name: None,
            num_slots: 16,
            slots: [(
                1,
                ContainerSlot {
                    guid: 0x11,
                    item_id: 871,
                    count: 1,
                    item_class: Some(0),
                    use_fork: fork,
                    equip_slots: vec![1],
                    ..Default::default()
                },
            )]
            .into(),
        }
    }

    /// The equip arm returns without control of the character, before it locks or sends
    /// (`0x4fa3ce`-`0x4fa3ec`), unless the player is on a taxi.
    #[test]
    fn the_equip_arm_needs_control_or_a_taxi() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(forked_bag(super::UseFork::Equip)));
        s.set_player_control(false);
        s.run("UseContainerItem(0, 1)").unwrap();
        assert!(s.take_script_calls().is_empty(), "no send");
        assert!(!s.item_locked(0x11), "no lock");
        assert!(s.cursor_item().is_none(), "nothing set on the cursor");

        s.set_on_taxi(true);
        s.run("UseContainerItem(0, 1)").unwrap();
        assert_eq!(s.take_script_calls().len(), 1, "a taxi passenger equips");
        assert!(s.item_locked(0x11));

        s.unlock_item(0x11);
        s.set_on_taxi(false);
        s.set_player_control(true);
        s.run("UseContainerItem(0, 1)").unwrap();
        assert_eq!(s.take_script_calls().len(), 1, "and so does a free player");
    }

    /// The dispatcher's requirement check (`0x5d8e0e`) and control gate (`0x5d8eaa`, error
    /// `0x7f`) end the call before the open arm's lock (`0x5edcd9`), each with its line, the
    /// requirement first.
    #[test]
    fn the_open_arm_runs_the_requirement_and_control_gates_before_its_lock() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(forked_bag(super::UseFork::Open)));

        s.set_player_control(false);
        s.run("UseContainerItem(0, 1)").unwrap();
        assert_eq!(s.take_ui_errors(), vec!["ERR_CLIENT_LOCKED_OUT"]);
        assert!(s.take_script_calls().is_empty());
        assert!(!s.item_locked(0x11));

        // A level-60 item for a level-10 player: `ERR_CANT_USE_ITEM` (`0x9c`), ahead of the
        // control gate.
        s.set_player_req_state(crate::script::PlayerReqState {
            level: 10,
            class_id: 1,
            race_id: 1,
            ..Default::default()
        });
        let template = |class, subclass, required_level| crate::script::ItemTemplateView {
            class,
            subclass,
            required_level,
            allowable_class: -1,
            allowable_race: -1,
            ..Default::default()
        };
        s.set_item_template(871, template(0, 0, 60));
        s.run("UseContainerItem(0, 1)").unwrap();
        assert_eq!(s.take_ui_errors(), vec!["ERR_CANT_USE_ITEM"]);
        assert!(!s.item_locked(0x11));

        // Control back, the requirement met: the open locks and goes out.
        s.set_player_control(true);
        s.set_item_template(871, template(0, 0, 5));
        s.run("UseContainerItem(0, 1)").unwrap();
        assert!(s.take_ui_errors().is_empty());
        assert_eq!(s.take_script_calls().len(), 1);
        assert!(s.item_locked(0x11));
    }

    /// The dispatcher's order (`0x5d8d00`): the gift, wrapping paper and quest-starter arms run
    /// before any gate (`0x5d8d92`, `0x5d8dcc`), the requirement check (`0x5d8e0e`) guards the
    /// read, letter, open and plain arms, and the control gate (`0x5d8eaa`) only the open and
    /// plain ones.
    #[test]
    fn each_gate_guards_the_arms_the_dispatcher_runs_after_it() {
        use super::UseFork::{Equip, Letter, Open, Plain, Quest, Read, Unwrap, WrapPaper};
        let refusals = |fork, level_ok: bool, control: bool| {
            let mut s = UiScript::new().unwrap();
            s.set_container(0, Some(forked_bag(fork)));
            s.set_player_control(control);
            s.set_player_req_state(crate::script::PlayerReqState {
                level: 10,
                class_id: 1,
                race_id: 1,
                ..Default::default()
            });
            s.set_item_template(
                871,
                crate::script::ItemTemplateView {
                    required_level: if level_ok { 5 } else { 60 },
                    allowable_class: -1,
                    allowable_race: -1,
                    ..Default::default()
                },
            );
            s.run("UseContainerItem(0, 1)").unwrap();
            s.take_ui_errors()
        };
        for fork in [Read, Letter, Open, Plain] {
            assert_eq!(
                refusals(fork, false, true),
                vec!["ERR_CANT_USE_ITEM"],
                "{fork:?}: the requirement"
            );
        }
        for fork in [Unwrap, WrapPaper, Quest, Equip] {
            assert!(
                refusals(fork, false, true).is_empty(),
                "{fork:?}: ahead of the requirement"
            );
        }
        for fork in [Read, Letter] {
            assert!(
                refusals(fork, true, false).is_empty(),
                "{fork:?}: ahead of the control gate"
            );
        }
        for fork in [Open, Plain] {
            assert_eq!(
                refusals(fork, true, false),
                vec!["ERR_CLIENT_LOCKED_OUT"],
                "{fork:?}: the control gate"
            );
        }
    }

    /// Without control the item goes only if its on-use spell is one the app fed as passing
    /// (`0x5d8ec0`-`0x5d8f6a`); an item with no on-use spell is refused (`0x5d8ee7`), and an
    /// uncached template ends the use silently (`0x5d8d5c`).
    #[test]
    fn a_bypass_spell_passes_the_control_gate_and_a_plain_item_does_not() {
        let mut slot = forked_bag(super::UseFork::Plain);
        slot.slots.get_mut(&1).unwrap().use_spell = 7744;
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(slot.clone()));
        s.set_player_control(false);

        s.run("UseContainerItem(0, 1)").unwrap();
        assert_eq!(s.take_ui_errors(), vec!["ERR_CLIENT_LOCKED_OUT"]);
        assert!(s.take_script_calls().is_empty());

        s.set_control_bypass([7744].into());
        s.run("UseContainerItem(0, 1)").unwrap();
        assert!(s.take_ui_errors().is_empty());
        assert_eq!(s.take_script_calls().len(), 1, "the trinket goes");

        slot.slots.get_mut(&1).unwrap().use_spell = 0;
        s.set_container(0, Some(slot.clone()));
        s.run("UseContainerItem(0, 1)").unwrap();
        assert_eq!(
            s.take_ui_errors(),
            vec!["ERR_CLIENT_LOCKED_OUT"],
            "no spell"
        );

        slot.slots.get_mut(&1).unwrap().item_class = None;
        s.set_container(0, Some(slot));
        s.run("UseContainerItem(0, 1)").unwrap();
        assert!(s.take_ui_errors().is_empty(), "no cached template: silent");
    }

    /// The proficiency leg's own line is `ERR_PROFICIENCY_NEEDED` (`0x7`, `0x5ea9b3`).
    #[test]
    fn a_proficiency_refusal_names_its_own_line() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(forked_bag(super::UseFork::Open)));
        s.set_player_req_state(crate::script::PlayerReqState {
            level: 10,
            class_id: 1,
            race_id: 1,
            proficiency: [(4, 0b10)].into(),
            ..Default::default()
        });
        s.set_item_template(
            871,
            crate::script::ItemTemplateView {
                class: 4,
                subclass: 0,
                allowable_class: -1,
                allowable_race: -1,
                ..Default::default()
            },
        );
        s.run("UseContainerItem(0, 1)").unwrap();
        assert_eq!(s.take_ui_errors(), vec!["ERR_PROFICIENCY_NEEDED"]);
        assert!(!s.item_locked(0x11));
    }

    /// An empty slot or a locked item ends `UseContainerItem` past its clear (`0x4fa1c7`,
    /// `0x4fa1cd`), repair mode included: nothing is used or repaired.
    #[test]
    fn use_container_item_skips_an_empty_slot_and_a_locked_item() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.lock_item(0x10a); // slot 1, a send's lock
        s.run("UseContainerItem(0, 1) UseContainerItem(0, 3)")
            .unwrap();
        assert!(s.take_container_uses().is_empty(), "locked, then empty");
        s.model_mut().repair_mode = true;
        s.run("UseContainerItem(0, 1)").unwrap();
        assert!(
            s.take_container_repairs().is_empty(),
            "a locked item is not repaired"
        );
        s.unlock_item(0x10a);
        s.model_mut().repair_mode = false;
        s.run("UseContainerItem(0, 1)").unwrap();
        assert_eq!(s.take_container_uses(), vec![(0, 1)]);
    }

    /// A right-click clears the cursor before it reads the slot (`0x4fa198`): a held item goes
    /// back to its slot, unlocked, and the clicked item is still used.
    #[test]
    fn right_click_puts_a_held_item_back_then_uses_the_clicked_one() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("UseContainerItem(0, 4)").unwrap();
        assert!(s.cursor_payload().is_none(), "the held item went back");
        assert!(
            !s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "its slot unlocks"
        );
        assert!(s.take_container_moves().is_empty(), "never placed");
        assert_eq!(s.take_container_uses(), vec![(0, 4)]);
    }

    /// The same clear cancels an armed gift wrap (`0x495190` opens with `0x5edf10`), and the
    /// clicked item is used, not wrapped.
    #[test]
    fn right_click_cancels_an_armed_gift_wrap_then_uses_the_clicked_one() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.arm_gift_wrap(0, 1);
        s.run("UseContainerItem(0, 4)").unwrap();
        assert_eq!(s.gift_wrap_armed(), None);
        assert!(s.take_container_wraps().is_empty(), "nothing wrapped");
        assert_eq!(s.take_container_uses(), vec![(0, 4)]);
    }

    #[test]
    fn pickup_then_place_swaps_and_clears_cursor() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack())); // slot 1 = Tough Jerky (resolved), slot 3 = in-flight
        assert!(s.cursor_item().is_none());
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());

        s.run("PickupContainerItem(0, 1)").unwrap();
        let held = s.cursor_item().expect("cursor holds the picked item");
        assert_eq!((held.bag, held.slot, held.item_id), (0, 1, 117));
        assert_eq!(
            held.texture.as_deref(),
            Some("Interface\\Icons\\INV_Misc_Food_16")
        );
        assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap());
        assert!(s.take_container_moves().is_empty());

        assert_eq!(s.cursor_item().map(|c| c.item_id), Some(117));

        s.run("PickupContainerItem(0, 5)").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 5,
                count: None,
            }]
        );
        assert!(
            s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the send's clear keeps the source locked for the server's answer"
        );
    }

    /// The reference's place runs no `SetCursorItem`: the displaced item is never held.
    #[test]
    fn pickup_place_onto_occupied_different_item_swaps_and_clears() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack(); // slot 1 = item 117 (A)
        state.slots.insert(
            5,
            ContainerSlot {
                use_fork: Default::default(),
                item_class: None,
                duration_ms: None,
                petition: None,
                already_bound: false,
                occupied_seats: None,
                bonding: 0,
                use_spell: 0,
                bar_placeable: true,
                consumable: false,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
                count: 1,
                quality: Some(2),
                inventory_type: 0,
                item_id: 200, // item B, a different id
                link: Some("|Hitem:200|h[Shiny Gem]|h".into()),
                guid: 0x10b,
                equip_slots: Vec::new(),
                cooldown: None,
                spell_0: false,
                spell_charges: [0; 5],
                readable: false,
                creator: None,
                gift_creator: None,
                creator_guid: 0,
                gift_creator_guid: 0,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        s.set_container(0, Some(state));

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(s.cursor_item().unwrap().item_id, 117);

        s.run("PickupContainerItem(0, 5)").unwrap();
        assert!(
            s.cursor_item().is_none(),
            "a swap clears the cursor — the displaced item never hops on"
        );
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 5,
                count: None,
            }]
        );
        assert!(
            s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the send's clear keeps the source locked for the server's answer"
        );
    }

    /// A same-item placement is a plain move: the server merges the stacks.
    #[test]
    fn pickup_place_onto_same_item_merges_and_clears() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack();
        state.slots.insert(
            5,
            ContainerSlot {
                durability: None,
                item_id: 117, // same id as slot 1
                count: 2,
                ..Default::default()
            },
        );
        s.set_container(0, Some(state));

        s.run("PickupContainerItem(0, 1)").unwrap();
        s.run("PickupContainerItem(0, 5)").unwrap();
        assert!(
            s.cursor_item().is_none(),
            "same-item merge clears the cursor"
        );
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 5,
                count: None,
            }]
        );
    }

    /// A whole stack placed onto a locked occupant is refused, held and unsent (`0x4f9e76`); a
    /// split onto the same stack goes (`0x4f9e1c` reads no lock).
    #[test]
    fn a_whole_stack_onto_a_locked_occupant_is_refused_and_a_split_is_not() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack(); // slot 1: a 5-stack of 117, guid 0x10a
        state.slots.insert(
            5,
            ContainerSlot {
                item_id: 117,
                count: 2,
                guid: 0x55,
                ..Default::default()
            },
        );
        s.set_container(0, Some(state));
        s.lock_item(0x55);
        s.run("PickupContainerItem(0, 1) PickupContainerItem(0, 5)")
            .unwrap();
        assert!(s.cursor_item().is_some(), "kept");
        assert!(s.take_container_moves().is_empty());
        s.run("ClearCursor() SplitContainerItem(0, 1, 2) PickupContainerItem(0, 5)")
            .unwrap();
        assert!(s.cursor_item().is_none());
        assert_eq!(s.take_container_moves().len(), 1, "the split merges");
    }

    #[test]
    fn pickup_refuses_a_locked_slot() {
        let mut s = UiScript::new().unwrap();
        let state = backpack();
        let guid = state.slots[&1].guid;
        s.set_container(0, Some(state));
        s.lock_item(guid);

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_none());
    }

    #[test]
    fn split_container_item_carries_a_partial_stack() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack(); // slot 1: a 5-stack of item 117
        state.slots.insert(
            7,
            ContainerSlot {
                durability: None,
                item_id: 117,
                count: 1,
                ..Default::default()
            },
        );
        state.slots.insert(
            9,
            ContainerSlot {
                durability: None,
                item_id: 999, // a different item
                count: 1,
                ..Default::default()
            },
        );
        s.set_container(0, Some(state));

        assert!(split_container_item(&s.lua, 0, 1, 3));
        let held = s.cursor_item().expect("the split carry");
        assert_eq!(
            (held.bag, held.slot, held.item_id, held.count),
            (0, 1, 117, Some(3))
        );

        s.run("PickupContainerItem(0, 2)").unwrap();
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 2,
                count: Some(3),
            }]
        );

        // The source stays locked until the server's answer, its stack-count write (`0x5d9400`).
        assert!(
            !split_container_item(&s.lua, 0, 1, 3),
            "locked for the answer"
        );
        s.unlock_item(0x10a);
        s.run("SplitContainerItem(0, 1, 3)").unwrap();
        s.run("PickupContainerItem(0, 9)").unwrap();
        let held = s.cursor_item().expect("kept — can't swap a partial stack");
        assert_eq!(held.count, Some(3));
        assert!(s.take_container_moves().is_empty());

        s.run("PickupContainerItem(0, 7)").unwrap();
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 7,
                count: Some(3),
            }]
        );
    }

    #[test]
    fn split_of_the_whole_stack_is_a_plain_pickup_and_an_oversized_count_is_refused() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack())); // slot 1: a 5-stack
        assert!(split_container_item(&s.lua, 0, 1, 5));
        assert_eq!(s.cursor_item().unwrap().count, None);
        // A count above the stack, or below 1, is refused before the clear (`0x4fa06f`,
        // `0x4fa064`): a held macro stays held.
        s.run("ClearCursor()").unwrap();
        let macro_payload =
            crate::script::cursor::CursorPayload::Macro(crate::script::cursor::CursorMacro {
                id: 1,
                texture: None,
            });
        s.set_cursor_for_test(macro_payload.clone());
        for count in [6, 99, 0, -1] {
            assert!(!split_container_item(&s.lua, 0, 1, count), "{count}");
        }
        assert_eq!(s.cursor_payload(), Some(macro_payload));
        // Past the gates the split stops spell targeting (`0x4fa07d`).
        s.run("ClearCursor()").unwrap();
        s.set_spell_targeting(true);
        assert!(split_container_item(&s.lua, 0, 1, 2));
        assert!(s.take_stop_targeting());
    }

    #[test]
    fn pickup_same_slot_cancels_no_move() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
    }

    #[test]
    fn pickup_empty_slot_holds_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        // Slot 2 is empty and slot 3 unresolved: neither can be picked up.
        s.run("PickupContainerItem(0, 2)").unwrap();
        assert!(s.cursor_item().is_none());
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(s.cursor_item().is_none());
    }

    #[test]
    fn clear_cursor_drops_the_held_item() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());
        s.run("ClearCursor()").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
    }

    /// A locked item shows no sell cursor (`0x4fa4bf`).
    #[test]
    fn a_locked_item_shows_no_sell_cursor() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.lock_item(0x10a);
        s.run("ShowContainerSellCursor(0, 1)").unwrap();
        assert_eq!(s.ui_cursor(), None);
        s.unlock_item(0x10a);
        s.run("ShowContainerSellCursor(0, 1)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
    }

    #[test]
    fn repair_cursor_wins_over_a_sellable_bag_hover() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_merchant(Some(MerchantState {
            can_repair: true,
            ..Default::default()
        }));

        s.run("ShowContainerSellCursor(0, 1)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));

        s.run("ShowRepairCursor()").unwrap();
        assert_eq!(
            s.ui_cursor(),
            None,
            "repair replaces the existing Buy hover"
        );
        s.run("ShowContainerSellCursor(0, 1)").unwrap();
        assert_eq!(
            s.ui_cursor(),
            None,
            "a bag hover keeps the repair base cursor"
        );
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(
            s.take_container_repairs(),
            vec![(0, 1), (0, 1)],
            "repair mode sticks across clicks"
        );
    }

    #[test]
    fn right_click_repairs_bag_item_instead_of_selling_it() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_merchant(Some(MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        s.run("ShowRepairCursor() UseContainerItem(0, 1) UseContainerItem(0, 1)")
            .unwrap();

        assert_eq!(s.take_container_repairs(), vec![(0, 1), (0, 1)]);
        assert!(s.take_container_uses().is_empty(), "no sell/use intent");
    }

    #[test]
    fn right_click_in_repair_mode_puts_a_held_item_back_then_repairs() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_merchant(Some(MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());
        // A held item and repair mode together, as after a cursor-mode change.
        s.model_mut().repair_mode = true;

        s.run("UseContainerItem(0, 4)").unwrap();
        assert!(
            s.cursor_payload().is_none(),
            "the held item went back (`0x4fa198`)"
        );
        assert!(s.take_container_moves().is_empty(), "never placed");
        assert_eq!(s.take_container_repairs(), vec![(0, 4)]);

        s.model_mut().repair_mode = false;
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.model_mut().repair_mode = true;
        s.run("UseContainerItem(0, 2)").unwrap();
        assert!(
            s.cursor_payload().is_none(),
            "an empty slot still clears the cursor"
        );
        assert!(
            s.take_container_repairs().is_empty(),
            "and repairs nothing (`0x4fa1a6`)"
        );
    }

    #[test]
    fn held_vendor_row_buys_into_bag_before_repair_mode() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_merchant(Some(MerchantState {
            items: vec![MerchantItem {
                item_id: 159,
                ..Default::default()
            }],
            can_repair: true,
            ..Default::default()
        }));
        s.run("ShowRepairCursor() PickupMerchantItem(1)").unwrap();
        assert!(
            s.repair_mode(),
            "vendor grab preserves the repair base mode"
        );
        assert!(s.cursor_payload().is_some());

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(s.take_merchant_slot_buys(), vec![(0, 1, 159)]);
        assert!(s.cursor_payload().is_none());
        assert!(s.take_container_repairs().is_empty());
        assert!(s.take_sounds().is_empty());

        // The right-click clears the vendor row with no packet and repairs instead.
        s.run("PickupMerchantItem(1) UseContainerItem(0, 1)")
            .unwrap();
        assert!(s.take_merchant_slot_buys().is_empty());
        assert!(s.cursor_payload().is_none());
        assert_eq!(s.take_container_repairs(), vec![(0, 1)]);
        assert!(s.take_container_uses().is_empty());
    }

    #[test]
    fn held_item_places_before_repair_mode() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());
        // A held item and repair base mode can coexist after a cursor-mode change.
        s.model_mut().repair_mode = true;

        s.run("PickupContainerItem(0, 2)").unwrap();
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 2,
                count: None,
            }]
        );
        assert!(s.cursor_payload().is_none());
        assert!(s.take_container_repairs().is_empty());
        assert!(s.take_sounds().is_empty());
    }

    #[test]
    fn repair_mode_repairs_under_held_coins_and_keeps_them() {
        use crate::script::cursor::{CursorMoney, CursorPayload};
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.model_mut().repair_mode = true;
        let coins = CursorPayload::Money(CursorMoney { copper: 50 });
        s.model_mut().cursor = Some(coins.clone());

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(
            s.take_container_repairs(),
            vec![(0, 1)],
            "coins pass both payload tests (`0x4f9c38`, `0x4f9c43`)"
        );
        assert_eq!(s.cursor_payload(), Some(coins), "and stay held");
        s.run("PickupContainerItem(0, 2)").unwrap();
        assert!(
            s.take_container_repairs().is_empty(),
            "an empty slot does nothing (`0x4f9c4e`)"
        );
    }

    #[test]
    fn removing_a_bag_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_container(2, Some(backpack()));
        s.set_container(2, None);
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(2)").unwrap(), 0);
    }

    #[test]
    fn container_item_cooldown_reads_the_gettime_triple() {
        let mut s = UiScript::new().unwrap();
        s.tick(100.0); // an arbitrary clock epoch
        let mut state = backpack();
        // Slot 1's item is 4 s into a 10 s use-cooldown: started at GetTime 96.
        state.slots.get_mut(&1).unwrap().cooldown = Some((96_000, 10_000, true));
        // Slot 4's on-use spell is cold: the query answers enabled.
        state.slots.get_mut(&4).unwrap().cooldown = Some((0, 0, true));
        s.set_container(0, Some(state));

        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 1)\n\
                 return s == 96 and d == 10 and e == 1"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 4)\n\
                 return s == 0 and d == 0 and e == 1"
            )
            .unwrap());
        // An item the query answered cold (`Some`) reads enabled; one it left at its zeros
        // (`None`: no on-use spell, a spent charged template) reads enable 0 (`0x6e2f34`).
        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 3)\n\
                 return s == 0 and d == 0 and e == 0"
            )
            .unwrap());
        // Past expiry, the same stored triple reads cold.
        s.tick(7.0);
        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 1)\n\
                 return s == 0 and d == 0 and e == 1"
            )
            .unwrap());
        // A re-push without the cooldown clears the stored triple.
        s.set_container(0, Some(backpack()));
        assert!(s
            .eval::<bool>(
                "local s, d = GetContainerItemCooldown(0, 1)\n\
                 return s == 0 and d == 0"
            )
            .unwrap());
    }

    // ── `ContainerIDToInventoryID` (`0x4f94e0`) ─────────────────────────────────────

    #[test]
    fn container_id_to_inventory_id_is_two_lines_with_no_clamp() {
        let s = UiScript::new().unwrap();
        let slot = |id: &str| {
            s.eval::<i64>(&format!("return ContainerIDToInventoryID({id})"))
                .unwrap()
        };
        // The `id <= 4` line: the keyring, -1, the backpack, the four worn bags.
        assert_eq!(slot("-2"), 17, "keyring — an ordinary point, not a case");
        assert_eq!(slot("-1"), 18);
        assert_eq!(slot("0"), 19, "backpack — likewise");
        assert_eq!(
            (slot("1"), slot("2"), slot("3"), slot("4")),
            (20, 21, 22, 23)
        );
        // The `id >= 5` line: the bank bags.
        assert_eq!(slot("5"), 64, "the jump — +59, not +19");
        assert_eq!((slot("6"), slot("10")), (65, 69));
        // No range check, no clamp, and never nil: out of range is an ordinary number.
        assert_eq!(
            slot("99"),
            158,
            "the value `SetInventoryItem` would receive"
        );
        assert_eq!(slot("-100"), -81, "negatives run off the line too");
        assert_eq!(
            s.arity("ContainerIDToInventoryID(99)").unwrap(),
            1,
            "one value on every non-raising path"
        );
        // `0x40a2b0` truncates toward zero, a C cast, not `floor`.
        assert_eq!(slot("2.9"), 21, "2.9 -> 2");
        assert_eq!(slot("-2.9"), 17, "-2.9 -> -2, NOT -3");
    }

    /// A missing or non-number argument raises: the type test's error `0x6f4940` does not return.
    #[test]
    fn container_id_to_inventory_id_raises_on_a_bad_argument() {
        let s = UiScript::new().unwrap();
        for call in ["ContainerIDToInventoryID()", "ContainerIDToInventoryID({})"] {
            let err = s
                .eval::<mlua::Value>(&format!("return {call}"))
                .unwrap_err();
            assert!(
                format!("{err}").contains("Usage: ContainerIDToInventoryID(containerID)"),
                "{call} must raise, got {err}"
            );
        }
    }

    /// A bag click over a payload that is neither an item nor a held row picks the item up, its
    /// setter's clear dropping the payload (`0x4f9de9`); an item off the bar (mode 7) is only
    /// cleared (`0x4f9f41`); a vendor row with no vendor open stays held (`0x4f9f0f`).
    #[test]
    fn a_bag_click_over_another_payload_picks_up_or_clears() {
        use crate::script::cursor::{CursorAction, CursorMerchantItem, CursorPayload, CursorSpell};
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            spell_id: 133,
            texture: None,
            passive: false,
        }));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(s.cursor_item().map(|i| i.slot), Some(1));

        s.run("ClearCursor()").unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0x80,
            action: 4540,
            texture: None,
            count: 0,
            consumable: false,
            state: None,
        }));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_payload().is_none(), "the bar item is cleared");

        let row = CursorPayload::Merchant(CursorMerchantItem {
            item_id: 159,
            row: 0,
            texture: None,
        });
        s.set_cursor_for_test(row.clone());
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(s.cursor_payload(), Some(row), "no vendor: kept");
        assert!(s.take_merchant_slot_buys().is_empty());
    }

    /// A held bag item answers the call's own now, whatever was pushed (`0x6e1517`).
    #[test]
    fn a_held_container_item_answers_the_polls_now() {
        let mut s = UiScript::new().unwrap();
        s.tick(100.0);
        let mut state = backpack();
        state.slots.get_mut(&1).unwrap().cooldown = Some((0, 1, false));
        s.set_container(0, Some(state));
        let poll = |s: &UiScript| {
            s.eval::<(f64, f64, i32)>("return GetContainerItemCooldown(0, 1)")
                .unwrap()
        };
        assert_eq!(poll(&s), (100.0, 0.001, 0));
        s.tick(0.02);
        let (start, _, _) = poll(&s);
        assert_eq!(start, s.eval::<f64>("return GetTime()").unwrap());
        assert!((start - 100.02).abs() < 1e-9);
    }

    /// `UseContainerItem`'s use and equip arms hand their call to the lent host inside the call.
    #[test]
    fn container_uses_apply_in_the_call_with_a_host_lent() {
        use crate::script::ScriptCall;
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        let used =
            crate::script::host::test_support::applied_in_call(&mut s, "UseContainerItem(0, 1)");
        let used_slot = |u: &super::ContainerUse| (u.bag, u.slot) == (0, 1);
        assert!(
            matches!(used.as_slice(), [ScriptCall::UseContainerItem(u)] if used_slot(u)),
            "{used:?}"
        );
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(forked_bag(super::UseFork::Equip)));
        let equipped =
            crate::script::host::test_support::applied_in_call(&mut s, "UseContainerItem(0, 1)");
        let equip_arm = super::UseArm::Item(super::UseFork::Equip);
        assert!(
            matches!(equipped.as_slice(), [ScriptCall::UseContainerItem(u)] if u.arm == equip_arm),
            "{equipped:?}"
        );
    }
}
