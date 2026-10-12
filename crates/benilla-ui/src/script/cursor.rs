//! The cursor payload: the client's payload-mode global `[0xb4d900]` as a typed enum, and the drag
//! gestures ([`drag`]) that change it. The payload changes through two primitives, as in the
//! reference: [`clear`], `ClearCursor(unlock, signal)` with its arm's events and its shared tail,
//! and [`put`], a setter's install and its own tail. Every setter runs its inner clear first
//! ([`set_cursor`] from a Lua verb, one segment each, so the clear's handlers read the emptied
//! cursor), and every verb that drops a payload clears through [`clear`].

use mlua::Lua;

use super::binding_abi::flag;
use super::tick::verb;
use super::{Model, ScriptValue};

mod bag_verbs;
mod bar;
mod doll;
mod drag;
pub(crate) mod money;
mod pet;

pub(crate) use bar::place_action;
pub(crate) use drag::{
    abandon_drag, maybe_start_drag, press_drag, take_drag, DragGesture, DragRelease,
};

/// Sentinel bag id for the player's equipped slots, whose `slot` is a 1-based inventory slot id
/// (`GetInventorySlotInfo`'s, HeadSlot = 1). Never a Lua value, and disjoint from every real bag
/// id, -1 the bank and -2 the keyring included. Ammo (slot 0) is outside it.
pub const EQUIPMENT_BAG: i64 = -100;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The payload
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// What the cursor carries, by the client's payload mode `[0xb4d900]`: 1 item, 2 money, 3 spell,
/// 4 pet action, 5 vendor row, 8 macro, 10 stabled pet; `Action` is the bar-slot pickup.
#[derive(Clone, Debug, PartialEq)]
pub enum CursorPayload {
    Item(CursorItem),
    Spell(CursorSpell),
    Action(CursorAction),
    Macro(CursorMacro),
    PetAction(CursorPetAction),
    /// Mode 5, a vendor row. Not an `Item`: `CursorHasItem` (`0x4895d0`) is true for modes 1 and 9
    /// only, so all thirteen stock `CursorHasItem()` gates stay shut while one is held.
    Merchant(CursorMerchantItem),
    /// Mode 10, a pet taken from the stable; its grab `0x495010` is the only writer of mode 10.
    StablePet(CursorStablePet),
    /// Mode 2, coins taken off a money frame.
    Money(CursorMoney),
}

/// Copper held on the cursor, mode 2 (`[0xb4e2f0]`, set by `PickupPlayerMoney`, `0x48abc0`). The
/// purse is not debited; the stock money frame subtracts `GetCursorMoney()` (`MoneyFrame.lua:17`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorMoney {
    pub copper: u32,
}

/// A vendor row held on the cursor, mode 5, set by `PickupMerchantItem` (`0x4fb760`). A drop on a
/// bag slot is the buy (`CMSG_BUY_ITEM_IN_SLOT`, `0x1a3`); in the reference a drop on the world,
/// ESC or the vendor closing clears it with no packet. No registered binding reports mode 5 to Lua.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorMerchantItem {
    /// The row's item entry (`0xb4b41c`), which the buy names.
    pub item_id: u32,
    /// The 0-based vendor row (`0xb4b420` = `trunc(arg) - 1`): a second `PickupMerchantItem` on the
    /// same row drops the grab.
    pub row: u32,
    /// The icon at the mouse, resolved from the display id the reference stores (`0xb4d8ec`).
    pub texture: Option<String>,
}

/// A pet held on the cursor from the stable window, mode 10, set by the grab `0x495010`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorStablePet {
    /// `[0xb4e300]`, in `PickupStablePet`'s index space: 0 the summoned pet, 1..=2 a stable slot.
    pub slot: u8,
    /// The family icon at the mouse, never empty: the grab refuses a pet without one (`0x495010`).
    pub texture: String,
}

/// An item held on the cursor: where it was picked from, so the next click routes the swap, and
/// the icon at the mouse. The server's field updates do the actual move.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorItem {
    /// The source bag id in the Lua API's numbering, or [`EQUIPMENT_BAG`].
    pub bag: i64,
    /// The 1-based source slot.
    pub slot: u32,
    pub item_id: u32,
    pub texture: Option<String>,
    pub link: Option<String>,
    /// `Some(n)` for a split of `n`, `None` for the whole stack.
    pub count: Option<u32>,
    /// Quality (0..6) at pickup, for a world drop's `DELETE_ITEM_CONFIRM`.
    pub quality: Option<u32>,
    /// The 1-based inventory slot ids the item can be equipped into, empty if none: the paper
    /// doll's place and `CursorCanGoInSlot` read it.
    pub equip_slots: Vec<u8>,
    /// Whether `PlaceAction` accepts it, its only item filter.
    pub bar_placeable: bool,
}

/// How many items the cursor holds. A whole-stack pickup has `count: None`, so the size is read
/// from the source slot, which a pickup locks but does not empty; 1 if that slot is unreadable.
pub(super) fn held_count(model: &super::Model, item: &CursorItem) -> u32 {
    if let Some(n) = item.count {
        return n.max(1);
    }
    model
        .containers
        .get(&item.bag)
        .and_then(|c| c.slots.get(&item.slot))
        .filter(|s| s.item_id == item.item_id)
        .map_or(1, |s| s.count.max(1))
}

/// A spell payload, mode 3, from `PickupSpell` on the player's spellbook.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorSpell {
    pub book_slot: u32,
    pub book_type: String,
    pub spell_id: u32,
    pub texture: Option<String>,
    /// `Attributes & 0x40` (`SPELL_ATTR_PASSIVE`): `PlaceAction` refuses a passive (`0x4e63ad`).
    pub passive: bool,
}

/// An action-bar payload from `PickupAction`: the source slot, the action's type byte as
/// `CMSG_SET_ACTION_BUTTON` packs it (spell, macro or item) and its id.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorAction {
    pub src_slot: u32,
    pub kind: u8,
    pub action: u32,
    pub texture: Option<String>,
}

/// A pet-bar payload, mode 4, from `PickupPetAction` or the pet spellbook: the packed action word
/// copied verbatim into `[0xb4e2f8]` (`0x494f0c`) and dropped undecoded (`0x4bce00`). Pet bar
/// only: `PlaceAction` refuses it (`0x4e62e0`).
#[derive(Clone, Debug, PartialEq)]
pub struct CursorPetAction {
    /// The 1-based pet bar slot it came from, 0 from the spellbook.
    pub src_slot: u32,
    /// The packed action word, verbatim.
    pub packed: u32,
    /// `Attributes & 0x40` of a spell word, tested at the drop, not the pickup (`0x4bc9f8`).
    pub passive: bool,
    pub texture: Option<String>,
}

/// A macro payload from `PickupMacro`, mode 8: the bare macro id (`[0xb4e2fc]`), which
/// `PlaceAction` accepts (`0x4e62e0`). It has no source slot, so a refused place leaves it held.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorMacro {
    /// The 1-based macro index: 1..=18 account, 19..=36 character.
    pub index: u32,
    pub texture: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The two primitives: the clear and the set
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `ClearCursor`'s two arguments (`0x495190`): `ecx`, whether a held item's lock lifts in the
/// item arm (`0x4951c2`), and `edx`, whether held coins signal `PLAYER_MONEY` (`0x49523f`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Clear {
    pub(crate) unlock: bool,
    pub(crate) signal_money: bool,
}

impl Clear {
    /// `ClearCursor(1,1)`: Lua's `ClearCursor()` (`0x4895b0`), every cancel, and the inner clear
    /// of each setter but `SetCursorItem`, which passes its own `edx` on (`0x494bb4`).
    pub(crate) const BOTH: Clear = Clear {
        unlock: true,
        signal_money: true,
    };
    /// `ClearCursor(0,1)`: an item send's clear, the item keeping its lock for the server's answer.
    pub(crate) const KEEP_LOCK: Clear = Clear {
        unlock: false,
        signal_money: true,
    };
    /// `ClearCursor(1,0)`: the trade window's money fold, which repaints by its own events.
    pub(crate) const NO_MONEY_EVENT: Clear = Clear {
        unlock: true,
        signal_money: false,
    };
}

/// `ClearCursor(unlock, signal)` (`0x495190`), the one way a payload leaves the cursor. An armed
/// gift wrap is cancelled first (`0x495197`). Then the payload's arm: an item's unlock signals
/// `ITEM_LOCK_CHANGED` when `unlock` (`0x4951d9`, `0x495455`), coins `PLAYER_MONEY` when `signal`
/// (`0x495248`). Then the shared tail, on every call, an empty cursor included: a grid a setter
/// showed hides, the pet bar's for a held pet action and the action bar's otherwise (`0x4952ac`,
/// `0x4952b3`), and `CURSOR_UPDATE` fires on the emptied cursor (`0x4952cc`).
pub(crate) fn clear(model: &mut Model, how: Clear) {
    // `0x5edf10`: the wrapping paper unlocks and the cursor mode returns to the base.
    if model.pending_wrap.take().is_some() {
        raise_lock_changed(model);
        model.ui_cursor = None;
        model.ui_cursor_dirty = true;
    }
    // Coins go back to nothing, as the purse was never debited; a vendor row clears with no
    // packet (`0x49525f`).
    let held = model.cursor.take();
    match &held {
        Some(CursorPayload::Item(_)) if how.unlock => raise_lock_changed(model),
        Some(CursorPayload::Money(_)) if how.signal_money => money::raise_player_money(model),
        _ => {}
    }
    if std::mem::take(&mut model.cursor_grid) {
        if matches!(held, Some(CursorPayload::PetAction(_))) {
            model.raise("PET_BAR_HIDEGRID", Vec::new());
        } else {
            model.raise("ACTIONBAR_HIDEGRID", Vec::new());
        }
    }
    model.raise("CURSOR_UPDATE", Vec::new());
}

/// `ClearCursor(1,1)`, Lua's `ClearCursor()` (`0x4895b0`).
pub(crate) fn clear_cursor(model: &mut Model) {
    clear(model, Clear::BOTH);
}

/// A setter past its inner clear: `payload` goes on the cursor, then that setter's own tail.
/// `SetCursorItem` (`0x494b60`) shows the action bar's grid for an item the bar takes
/// (`0x494c3b`) and fires `CURSOR_UPDATE` (`0x494c54`); the row grab (`0x4950f0`) shows it for a
/// bar item, mode 7 (`0x49513a`), and fires `CURSOR_UPDATE` (`0x49515e`). The spell (`0x494dd1`)
/// and macro (`0x494fd7`) setters end on the action bar's grid, the pet-action setter on the pet
/// bar's (`0x494f28`), the money setter on `PLAYER_MONEY` (`0x494d0a`) and the stable grab on
/// nothing (`0x4950c5`), so no `CURSOR_UPDATE` follows theirs. A bar pickup takes the setter of
/// its kind (`0x4e6130`): the grab for an item, the spell or macro setter otherwise.
pub(crate) fn put(model: &mut Model, payload: CursorPayload) {
    use super::action::ACTION_KIND_ITEM;
    /// What a setter fires past its set.
    enum Tail {
        /// `SetCursorItem` and the row grab: the action bar's grid if asked, then `CURSOR_UPDATE`.
        Update { grid: bool },
        /// The spell and macro setters.
        BarGrid,
        /// The pet-action setter.
        PetGrid,
        /// The money setter.
        Money,
        /// The stable grab.
        Nothing,
    }
    let tail = match &payload {
        CursorPayload::Item(item) => Tail::Update {
            grid: item.bar_placeable,
        },
        CursorPayload::Action(a) if a.kind == ACTION_KIND_ITEM => Tail::Update { grid: true },
        CursorPayload::Merchant(_) => Tail::Update { grid: false },
        CursorPayload::Action(_) | CursorPayload::Spell(_) | CursorPayload::Macro(_) => {
            Tail::BarGrid
        }
        CursorPayload::PetAction(_) => Tail::PetGrid,
        CursorPayload::Money(_) => Tail::Money,
        CursorPayload::StablePet(_) => Tail::Nothing,
    };
    model.cursor = Some(payload);
    match tail {
        Tail::Update { grid } => {
            if grid {
                model.raise("ACTIONBAR_SHOWGRID", Vec::new());
                model.cursor_grid = true;
            }
            model.raise("CURSOR_UPDATE", Vec::new());
        }
        Tail::BarGrid => {
            model.raise("ACTIONBAR_SHOWGRID", Vec::new());
            model.cursor_grid = true;
        }
        Tail::PetGrid => {
            model.raise("PET_BAR_SHOWGRID", Vec::new());
            model.cursor_grid = true;
        }
        Tail::Money => money::raise_player_money(model),
        Tail::Nothing => {}
    }
}

/// A cursor setter from a Lua verb. Its inner `ClearCursor(unlock, 1)` runs as one segment, so
/// that clear's handlers read the emptied cursor as the reference's do; then `payload`, read
/// past the clear, goes on with its tail as a second ([`put`]). `None` leaves the cursor clear,
/// as `SetCursorItem` with a zero guid (`0x494bc5`) or a stable grab whose slot fails its check
/// past the clear (`0x49505e`). Only `SetCursorItem` passes `unlock` through; every other setter
/// clears with `ClearCursor(1,1)`. Returns whether a payload went on.
pub(crate) fn set_cursor(
    lua: &Lua,
    unlock: bool,
    payload: impl FnOnce(&Model) -> Option<CursorPayload>,
) -> bool {
    verb(lua, |model| {
        clear(
            model,
            Clear {
                unlock,
                signal_money: true,
            },
        );
    });
    verb(lua, |model| match payload(model) {
        Some(payload) => {
            put(model, payload);
            true
        }
        None => false,
    })
}

/// An item pickup: `SetCursorItem(1, …)`, then `LockItem` (`0x4953e0`), whose
/// `ITEM_LOCK_CHANGED` (`0x495415`) follows the set's `CURSOR_UPDATE`, as the bag (`0x4f9de9`,
/// `0x4f9df0`), doll (`0x4c7838`) and bag-slot (`0x4c7be2`) pickups and the split (`0x4fa0aa`) do.
pub(crate) fn pick_up_item(lua: &Lua, item: CursorItem) {
    set_cursor(lua, true, |_| Some(CursorPayload::Item(item)));
    verb(lua, raise_lock_changed);
}

/// Raise `ITEM_LOCK_CHANGED` for a slot that a pickup locks or a put-down frees. It carries no
/// arguments: the lock (`0x495415`) and the unlock (`0x495455`) are plain `SignalEvent`s, and the
/// stock buttons re-read every slot's lock.
pub(crate) fn raise_lock_changed(model: &mut Model) {
    model.raise("ITEM_LOCK_CHANGED", Vec::new());
}

/// Whether the client itself holds the item at `(bag, slot)` locked, the reference's `item+0x314`
/// bit 0 that `LockItem` (`0x4953e0`) sets: while the item is on the cursor, is an armed gift
/// wrap's paper (`0x5edeb5`), or sits in the mail or auction attach slot, whose attach clears the
/// cursor with `SetCursorItem(0, …)` (`0x4ae0d2`, `0x4ce56a`) and so keeps the pickup's lock until
/// the item leaves the slot (`0x495420` at `0x4acdd6`, `0x4cd653`). The engine has no item object
/// to carry the bit, so it is read from the state that holds the item and lifts when the item
/// leaves it. The app's pending operations are the slot's `locked`.
pub(crate) fn client_locked(model: &Model, bag: i64, slot: u32) -> bool {
    let at = |item: &CursorItem| item.bag == bag && item.slot == slot;
    matches!(&model.cursor, Some(CursorPayload::Item(item)) if at(item))
        || model
            .pending_wrap
            .as_ref()
            .is_some_and(|w| w.bag == bag && w.slot == slot)
        || model.mail_send_item.as_ref().is_some_and(at)
        || model.auction_sell_item.as_ref().is_some_and(at)
}

/// Raise `DELETE_ITEM_CONFIRM(name, quality)` for an item dropped on the world. The payload stays
/// held: the popup's `DeleteCursorItem` or `ClearCursor` clears it.
fn raise_delete_item_confirm(model: &mut Model, item: &CursorItem) {
    let name = item_link_name(item.link.as_deref());
    let quality = item.quality.unwrap_or(0);
    model.raise(
        "DELETE_ITEM_CONFIRM",
        vec![ScriptValue::Str(name), ScriptValue::Int(i64::from(quality))],
    );
}

/// An item link's name, from its `|h[Name]|h` segment; empty for no link or another shape.
pub(super) fn item_link_name(link: Option<&str>) -> String {
    link.and_then(|l| l.split_once('['))
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(name, _)| name.to_string())
        .unwrap_or_default()
}

/// The sell fork's clear, `SetCursorItem(0, …)` with a zero guid, so its inner `ClearCursor(0,1)`
/// alone (`0x4fb7ac`, `0x5df5fc`): `PickupMerchantItem` (`0x4fb760`) and the interact ladder's
/// vendor arm (`0x5df5d0`). Unlike [`clear_cursor`] it lifts no lock, so the source slot stays
/// grey until the server removes the item. Any other payload is no sale and stays held
/// (`GetCursorItem`, `0x494c60`).
pub(crate) fn take_cursor_item_for_sale(model: &mut Model) -> Option<CursorItem> {
    let Some(CursorPayload::Item(item)) = &model.cursor else {
        return None;
    };
    let item = item.clone();
    clear(model, Clear::KEEP_LOCK);
    Some(item)
}

/// What is under the cursor in the world this frame: the reference's click-time pick state
/// `[this+0x350]` (`0x481f60`). The app feeds it; tests and captures leave it `Nothing`.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum WorldPick {
    /// State 0: no world hit (the sky).
    #[default]
    Nothing,
    /// State 1: terrain, a WMO or a doodad, but no unit or GameObject.
    Terrain,
    /// State 2: a unit or GameObject.
    Object,
}

/// A left click on the world with a payload held, press and release both over the world and no
/// drag (`0x495300`); returns whether it was consumed. Over an `Object` (`0x492ce0`) nothing
/// drops and the click selects. Over `Terrain` (`0x492c90`) or `Nothing` (`0x492d30`) an item
/// raises `DELETE_ITEM_CONFIRM(name, quality)` (`0x5e0320`) and stays held for the popup to clear;
/// a spell, action, macro or pet action clears. Coins, a vendor row and a stabled pet stay held
/// here, where the reference puts a vendor row down on every leg and clears any non-item payload
/// over `Nothing` (`0x492e04`).
///
/// Deviation: on `Terrain` the reference keeps a spell, action, macro or pet action on the left
/// button; we clear them, because otherwise a spell has no left-click way off the cursor.
pub(crate) fn world_drop_click(model: &mut Model) -> bool {
    match (&model.cursor, model.world_pick) {
        (Some(CursorPayload::Item(item)), WorldPick::Terrain | WorldPick::Nothing) => {
            let item = item.clone();
            raise_delete_item_confirm(model, &item);
            true
        }
        (
            Some(
                CursorPayload::Spell(_)
                | CursorPayload::Action(_)
                | CursorPayload::Macro(_)
                | CursorPayload::PetAction(_),
            ),
            WorldPick::Terrain | WorldPick::Nothing,
        ) => {
            clear_cursor(model);
            true
        }
        _ => false,
    }
}

/// `SplitContainerItem(bag, slot, count)` (`0x4f9f70`): an unlocked stack's `count` goes on the
/// cursor. Past its gates the binding clears with `ClearCursor(1,1)` (`0x4fa078`), whatever is
/// held, then picks up ([`pick_up_item`], `0x4fa0aa`), so its handlers see two clears and the
/// set. A count of the whole stack or more is a plain pickup, since vmangos refuses to split a
/// whole stack or more (`Player.cpp:11139`, `:11147`). Returns whether anything was picked up.
pub(crate) fn split_container_item(lua: &Lua, bag: i64, slot: u32, count: i64) -> bool {
    let item = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let Some(s) = model.containers.get(&bag).and_then(|c| c.slots.get(&slot)) else {
            return false;
        };
        if count < 1 || s.item_id == 0 || s.locked || client_locked(&model, bag, slot) {
            return false;
        }
        let n = count.min(i64::from(s.count)) as u32;
        let whole = n >= s.count;
        CursorItem {
            bag,
            slot,
            item_id: s.item_id,
            texture: s.texture.clone(),
            link: s.link.clone(),
            quality: s.quality,
            count: if whole { None } else { Some(n) },
            bar_placeable: s.bar_placeable,
            equip_slots: s.equip_slots.clone(),
        }
    };
    verb(lua, clear_cursor);
    pick_up_item(lua, item);
    true
}

/// `DeleteCursorItem()` (`0x4897b0`), the delete popup's accept: a held item queues its
/// `(bag, slot, count)` destroy, count 0 for the whole stack, then `ClearCursor(1,1)`
/// (`0x4898af`) and `LockItem` on it again (`0x4898bc`) for the server's answer; anything else is
/// a no-op.
pub(crate) fn delete_cursor_item(model: &mut Model) {
    let Some(CursorPayload::Item(item)) = &model.cursor else {
        return;
    };
    let item = item.clone();
    model
        .container_destroys
        .push((item.bag, item.slot, item.count.unwrap_or(0)));
    clear_cursor(model);
    raise_lock_changed(model);
}

/// An attach slot's click, `ClickSendMailItemButton` (`0x4adef0`) and
/// `ClickAuctionSellItemButton` (`0x4ce310`) alike: `slot` names the attachment. The held item,
/// or none, is compared with the attachment (`0x4ae07a`, `0x4ce512`): the same, nothing on both
/// sides included, is a plain `ClearCursor(1,1)` (`0x4ae08b`, `0x4ce523`), which drops any other
/// payload. Otherwise the held item becomes the attachment and `SetCursorItem(0, …)` puts the old
/// one back on the cursor (`0x4ae0d4`, `0x4ce56c`): its inner `ClearCursor(0,1)` lifts no lock,
/// so the newly attached item stays locked ([`client_locked`]), and an empty old attachment
/// leaves the cursor clear. Returns whether the attachment changed, after which the caller's
/// slot signals its own update.
pub(crate) fn click_attach_slot(
    lua: &Lua,
    slot: impl Fn(&mut Model) -> &mut Option<CursorItem>,
) -> bool {
    let (held, old) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let held = match &model.cursor {
            Some(CursorPayload::Item(item)) => Some(item.clone()),
            _ => None,
        };
        (held, slot(&mut model).clone())
    };
    let key = |item: &Option<CursorItem>| item.as_ref().map(|i| (i.bag, i.slot));
    if key(&held) == key(&old) {
        verb(lua, clear_cursor);
        return false;
    }
    verb(lua, |model| *slot(model) = held);
    set_cursor(lua, false, |_| old.map(CursorPayload::Item));
    true
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// UiScript accessors
// ─────────────────────────────────────────────────────────────────────────────────────────────

impl super::UiScript {
    /// The held item, if the payload is one.
    pub fn cursor_item(&self) -> Option<CursorItem> {
        match self.model_ref().cursor.clone() {
            Some(CursorPayload::Item(item)) => Some(item),
            _ => None,
        }
    }

    /// The cursor payload, any arm.
    pub fn cursor_payload(&self) -> Option<CursorPayload> {
        self.model_ref().cursor.clone()
    }

    /// Feed the world pick under the cursor this frame ([`WorldPick`]).
    pub fn set_world_pick(&mut self, pick: WorldPick) {
        self.model_mut().world_pick = pick;
    }

    /// [`take_cursor_item_for_sale`], for the app's interact ladder.
    pub fn take_cursor_item_for_sale(&mut self) -> Option<CursorItem> {
        take_cursor_item_for_sale(&mut self.model_mut())
    }

    /// `ClearCursor()` from Rust, which the app calls on a right-click over empty world: both
    /// reference legs there (`0x492c90`, `0x492d30`) call `ClearCursor(1, 1)` (`0x495190`).
    pub fn clear_cursor_payload(&mut self) {
        clear_cursor(&mut self.model_mut());
    }

    /// Test-only: set the payload directly, bypassing the Lua bindings.
    #[cfg(test)]
    pub(crate) fn set_cursor_for_test(&mut self, payload: CursorPayload) {
        self.model_mut().cursor = Some(payload);
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The targeting cursor's item pick and the enchant confirms
// ─────────────────────────────────────────────────────────────────────────────────────────────
impl super::UiScript {
    /// Arm or disarm the targeting cursor's item half (`TargetingWantsItem`, `0x6e6330`): while
    /// armed, a bag or paper-doll click queues its `(bag, slot)` as a pick instead of picking up,
    /// as the reference's two pickups do (`0x4f9b30` at `0x4f9c54`, `0x4c7300` at `0x4c76df`).
    pub fn set_item_pick_armed(&mut self, armed: bool) {
        self.model_mut().item_pick_armed = armed;
    }

    /// Push the guids of the units for which the armed word's unit checks all pass.
    /// `SpellCanTargetUnit` resolves its token to a guid, as `0x515970` does, and asks this set,
    /// so a chain such as `"party1target"` reads the unit it ends on.
    pub fn set_spell_targetable_units(&mut self, guids: impl IntoIterator<Item = u64>) {
        self.model_mut().spell_targetable_units = guids.into_iter().collect();
    }

    /// Drain the `(bag, slot)` picks since the last call; a paper-doll pick reports as
    /// [`EQUIPMENT_BAG`] and its 1-based inventory slot.
    pub fn take_item_picks(&mut self) -> Vec<(i64, u32)> {
        std::mem::take(&mut self.model_mut().item_picks)
    }

    /// Drain the enchant-confirm answers since the last call; both answer the pick the app parked.
    pub fn take_enchant_confirms(&mut self) -> Vec<EnchantConfirm> {
        std::mem::take(&mut self.model_mut().enchant_confirms)
    }
}

/// A Yes on an enchant-apply confirm (`StaticPopup.lua:1245`, `:1257`); the app holds the item
/// guid it answers for (the reference's `0xb4e3c0`/`0xb4e3c4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnchantConfirm {
    /// `BindEnchant()` (`0x48d2e0`): reruns the enchant-apply gate `0x495d60` as already
    /// confirmed, so it may raise the replace popup next rather than cast.
    Bind,
    /// `ReplaceEnchant()` (`0x48d300`): binds the parked guid through `0x6e5b40`, skipping the
    /// gate. There is no replace packet: the popup only gates the same cast.
    Replace,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Lua globals
// ─────────────────────────────────────────────────────────────────────────────────────────────

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `BindEnchant()`, `ReplaceEnchant()`: queue an intent for the app; nothing is sent from here.
    for (name, answer) in [
        ("BindEnchant", EnchantConfirm::Bind),
        ("ReplaceEnchant", EnchantConfirm::Replace),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.enchant_confirms.push(answer);
                Ok(())
            })?,
        )?;
    }

    g.set(
        "CursorHasItem",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(matches!(model.cursor, Some(CursorPayload::Item(_)))))
        })?,
    )?;
    g.set(
        "CursorHasSpell",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(matches!(model.cursor, Some(CursorPayload::Spell(_)))))
        })?,
    )?;
    // Deviation: returns nothing. The handler ends `mov eax,1` with nothing pushed (`0x4895bc`), so
    // Lua 5.0 hands back the stack top, the last argument or the function itself: an accident of
    // the handler, not an API, and copying a stack-top leak would copy a bug.
    g.set(
        "ClearCursor",
        lua.create_function(|lua, ()| {
            super::tick::verb(lua, clear_cursor);
            Ok(())
        })?,
    )?;

    g.set(
        "SplitContainerItem",
        lua.create_function(|lua, (bag, slot, count): (i64, u32, i64)| {
            split_container_item(lua, bag, slot, count);
            Ok(())
        })?,
    )?;
    g.set(
        "DeleteCursorItem",
        lua.create_function(|lua, ()| {
            super::tick::verb(lua, delete_cursor_item);
            Ok(())
        })?,
    )?;

    doll::install(lua)?;
    bag_verbs::install(lua)?;
    bar::install(lua)?;
    pet::install(lua)?;
    money::install(lua)?;

    Ok(())
}

/// Test-only: a `CursorKind()` global naming the payload a handler reads (`none`, `item`,
/// `spell`, `action`, `macro`, `pet`, `row`, `stable`, `money`), since 1.12's Lua has no getter for
/// most modes.
#[cfg(test)]
pub(crate) fn install_kind_probe(s: &super::UiScript) {
    let kind = s
        .lua
        .create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match &model.cursor {
                None => "none",
                Some(CursorPayload::Item(_)) => "item",
                Some(CursorPayload::Spell(_)) => "spell",
                Some(CursorPayload::Action(_)) => "action",
                Some(CursorPayload::Macro(_)) => "macro",
                Some(CursorPayload::PetAction(_)) => "pet",
                Some(CursorPayload::Merchant(_)) => "row",
                Some(CursorPayload::StablePet(_)) => "stable",
                Some(CursorPayload::Money(_)) => "money",
            })
        })
        .unwrap();
    s.lua.globals().set("CursorKind", kind).unwrap();
}

/// Test-only: a backpack with two items, 117 in slot 1 and 118 in slot 2, both bar-placeable.
#[cfg(test)]
pub(crate) fn two_item_bag() -> super::container::ContainerState {
    let item = |item_id: u32, name: &str| super::container::ContainerSlot {
        item_id,
        count: 1,
        quality: Some(1),
        bar_placeable: true,
        texture: Some(format!("Interface\\Icons\\INV_{item_id}")),
        link: Some(format!("|cffffffff|Hitem:{item_id}|h[{name}]|h|r")),
        ..Default::default()
    };
    super::container::ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots: [
            (1, item(117, "Tough Jerky")),
            (2, item(118, "Minor Healing Potion")),
        ]
        .into_iter()
        .collect(),
    }
}

/// Test-only: a Lua expression reading `GetContainerItemInfo(0, slot)`'s `locked`.
#[cfg(test)]
pub(crate) fn bag_lock_probe(slot: u32) -> String {
    format!("(function() local _, _, l = GetContainerItemInfo(0, {slot}) return l end)()")
}

#[cfg(test)]
mod tests {
    use super::{split_container_item, CursorAction, CursorItem, CursorPayload, CursorSpell};
    use crate::script::tick::heard;
    use crate::script::UiScript;

    /// A backpack holding a five-stack in slot 1.
    fn one_item_backpack() -> crate::script::container::ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            crate::script::container::ContainerSlot {
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
                count: 5,
                quality: Some(3),
                inventory_type: 0,
                item_id: 117,
                link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
                locked: false,
                equip_slots: Vec::new(),
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        crate::script::container::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    /// `SetCursorItem(0, …)` (`0x494b60`) skips `ClearCursor`'s unlock (`0x495420`) and takes only
    /// a held item.
    #[test]
    fn the_sell_clear_keeps_the_source_slot_locked() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));

        // The pickup's events fire inside it, so the queue holds only the host's sell clear.
        s.eval::<()>("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some(), "the pickup did not take");
        assert!(s.model_ref().pending_events.is_empty());

        let sold = s.take_cursor_item_for_sale().expect("mode 1 is a sale");
        assert_eq!((sold.bag, sold.slot), (0, 1));
        assert!(
            s.cursor_payload().is_none(),
            "the cursor kept the sold item"
        );

        let fired: Vec<String> = s
            .model_ref()
            .pending_events
            .iter()
            .map(|(e, _)| e.clone())
            .collect();
        assert!(
            fired.iter().any(|e| e == "CURSOR_UPDATE"),
            "the sell clear must fire CURSOR_UPDATE — got {fired:?}"
        );
        assert!(
            !fired.iter().any(|e| e == "ITEM_LOCK_CHANGED"),
            "the sell clear must NOT un-lock the source slot (that is `ClearCursor`'s job, not \
             `SetCursorItem(0, …)`'s) — got {fired:?}"
        );

        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
            passive: false,
        }));
        assert!(
            s.take_cursor_item_for_sale().is_none(),
            "a spell is not a sale"
        );
        assert!(
            matches!(s.cursor_payload(), Some(CursorPayload::Spell(_))),
            "asking for a sale dropped a non-item payload"
        );
    }

    /// `CursorHasItem` (`0x4895d0`) and `CursorHasSpell` (`0x489600`) push the number 1
    /// (`0x4895ea`, `0x489609`) or nil (`0x4895df`, `0x48961b`) and return one value.
    #[test]
    fn the_has_checks_answer_per_arm() {
        let mut s = UiScript::new().unwrap();
        // `arg.n` counts the returns (`select` is 5.1's, not a 1.12 global).
        let ask = |s: &mut UiScript, f: &str| {
            s.eval::<String>(&format!(
                "local function c(...) return arg.n .. ':' .. type(arg[1]) .. ':' .. tostring(arg[1]) end \
                 return c({f}())"
            ))
            .unwrap()
        };
        const YES: &str = "1:number:1";
        const NO: &str = "1:nil:nil";

        assert_eq!(ask(&mut s, "CursorHasItem"), NO);
        assert_eq!(ask(&mut s, "CursorHasSpell"), NO);

        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: Some("|Hitem:117|h[Tough Jerky]|h".into()),
            count: None,
            quality: Some(3),
            equip_slots: Vec::new(),
        }));
        assert_eq!(ask(&mut s, "CursorHasItem"), YES);
        assert_eq!(ask(&mut s, "CursorHasSpell"), NO);

        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 3,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
        }));
        assert_eq!(ask(&mut s, "CursorHasItem"), NO);
        assert_eq!(ask(&mut s, "CursorHasSpell"), YES);

        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 12,
            kind: 0,
            action: 133,
            texture: None,
        }));
        assert_eq!(ask(&mut s, "CursorHasItem"), NO);
        assert_eq!(ask(&mut s, "CursorHasSpell"), NO);
    }

    /// Deviation pin: the reference hands back its stack top (`0x4895bc`); `ClearCursor` returns
    /// nothing, with an empty cursor and a held payload alike.
    #[test]
    fn clear_cursor_returns_no_values() {
        let mut s = UiScript::new().unwrap();
        let count = "return (function(...) return arg.n end)(ClearCursor())";
        assert_eq!(s.eval::<i64>(count).unwrap(), 0);
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 1,
            texture: None,
        }));
        assert_eq!(s.eval::<i64>(count).unwrap(), 0);
        assert!(s.cursor_payload().is_none());
    }

    #[test]
    fn clear_cursor_widens_to_any_arm() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 1,
            texture: None,
        }));
        assert!(s.cursor_payload().is_some());
        s.run("ClearCursor()").unwrap();
        assert!(s.cursor_payload().is_none());
    }

    /// A pickup's events have run by the verb's next Lua line, each handler reading the cursor as
    /// the reference's does: `SetCursorItem`'s inner clear on the empty cursor (`0x4952cc`), then
    /// the grid before `CURSOR_UPDATE` (`0x494c3b`, `0x494c54`), then the lock (`0x495415`). The
    /// same-slot click that puts it back is `ClearCursor(1,1)` (`0x4f9e0e`): the unlock
    /// (`0x495455`), the grid hide (`0x4952b3`), then `CURSOR_UPDATE` (`0x4952cc`).
    #[test]
    fn a_pickup_fires_its_events_before_it_returns() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        heard::listen(
            &s,
            &[
                "ACTIONBAR_SHOWGRID",
                "ACTIONBAR_HIDEGRID",
                "CURSOR_UPDATE",
                "ITEM_LOCK_CHANGED",
            ],
            Some("CursorHasItem()"),
        );
        let seen: Vec<String> = s
            .eval("PickupContainerItem(0, 1) local h = HEARD HEARD = {} return h")
            .unwrap();
        assert_eq!(
            seen,
            [
                "CURSOR_UPDATE nil",
                "ACTIONBAR_SHOWGRID 1",
                "CURSOR_UPDATE 1",
                "ITEM_LOCK_CHANGED 1"
            ]
        );
        let seen: Vec<String> = s
            .eval("PickupContainerItem(0, 1) local h = HEARD HEARD = {} return h")
            .unwrap();
        assert_eq!(
            seen,
            [
                "ITEM_LOCK_CHANGED nil",
                "ACTIONBAR_HIDEGRID nil",
                "CURSOR_UPDATE nil"
            ]
        );
        assert!(s.model_ref().pending_events.is_empty());
    }

    /// The bag place arm (`0x4f9e1c`-`0x4f9eb8`): a whole stack locks an occupied destination
    /// first (`0x4f9e85`), then the swap's clear (`0x5e0fe7`) and the binding's own (`0x4f9eb8`)
    /// each fire `CURSOR_UPDATE`; an empty destination signals no lock (`0x4953ed`); a split's
    /// one clear (`0x4f9e4f`) signals no lock either.
    #[test]
    fn the_bag_place_arm_locks_the_destination_before_its_two_clears() {
        let mut s = UiScript::new().unwrap();
        let mut bag = one_item_backpack();
        let mut other = bag.slots[&1].clone();
        other.item_id = 118;
        bag.slots.insert(2, other);
        s.set_container(0, Some(bag));
        heard::listen(
            &s,
            &["ITEM_LOCK_CHANGED", "ACTIONBAR_HIDEGRID", "CURSOR_UPDATE"],
            None,
        );
        let place = |s: &UiScript, from: u32, to: u32| {
            s.run(&format!("PickupContainerItem(0, {from})")).unwrap();
            heard::take(s);
            s.run(&format!("PickupContainerItem(0, {to})")).unwrap();
            heard::take(s)
        };
        assert_eq!(
            place(&s, 1, 2),
            [
                "ITEM_LOCK_CHANGED",
                "ACTIONBAR_HIDEGRID",
                "CURSOR_UPDATE",
                "CURSOR_UPDATE"
            ],
            "a swap onto an occupied slot"
        );
        assert_eq!(
            place(&s, 1, 3),
            ["ACTIONBAR_HIDEGRID", "CURSOR_UPDATE", "CURSOR_UPDATE"],
            "a move onto an empty slot"
        );
        s.run("SplitContainerItem(0, 1, 2)").unwrap();
        heard::take(&s);
        s.run("PickupContainerItem(0, 4)").unwrap();
        assert_eq!(
            heard::take(&s),
            ["ACTIONBAR_HIDEGRID", "CURSOR_UPDATE"],
            "a split"
        );
    }

    /// The clear's shared tail (`0x49529b`) signals on every call: with nothing held, one
    /// `CURSOR_UPDATE` and no grid event.
    #[test]
    fn clear_cursor_with_nothing_held_still_signals() {
        let s = UiScript::new().unwrap();
        heard::listen(
            &s,
            &["CURSOR_UPDATE", "ACTIONBAR_SHOWGRID", "ACTIONBAR_HIDEGRID"],
            None,
        );
        s.run("ClearCursor()").unwrap();
        assert_eq!(heard::take(&s), ["CURSOR_UPDATE"]);
    }

    /// Clearing a spell on terrain is the deviation documented on `world_drop_click`.
    #[test]
    fn world_drop_terrain_and_nothing_both_clear_a_spell_payload() {
        use super::{world_drop_click, WorldPick};
        let mut s = UiScript::new().unwrap();
        for pick in [WorldPick::Terrain, WorldPick::Nothing] {
            s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
                passive: false,
                book_slot: 3,
                book_type: "spell".into(),
                spell_id: 133,
                texture: None,
            }));
            s.model_mut().world_pick = pick;
            assert!(
                world_drop_click(&mut s.model_mut()),
                "{pick:?}: the dismiss consumes the click"
            );
            assert!(
                s.cursor_payload().is_none(),
                "{pick:?}: the spell clears silently"
            );
        }

        let item = CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        });
        s.set_cursor_for_test(item.clone());
        s.model_mut().world_pick = WorldPick::Terrain;
        assert!(
            world_drop_click(&mut s.model_mut()),
            "terrain: the item pops the popup"
        );
        assert!(s.cursor_item().is_some(), "and stays held");
        s.model_mut().world_pick = WorldPick::Object;
        assert!(
            !world_drop_click(&mut s.model_mut()),
            "object: nothing drops at all"
        );
        assert!(s.cursor_item().is_some());
    }

    #[test]
    fn delete_cursor_item_queues_destroy_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("DeleteCursorItem()").unwrap();
        assert!(s.cursor_item().is_none(), "the payload clears");
        assert_eq!(s.take_container_destroys(), vec![(0, 1, 0)]);
        assert!(s.take_container_destroys().is_empty(), "drained");
    }

    #[test]
    fn delete_cursor_item_is_a_no_op_off_the_item_arm() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0,
            action: 1,
            texture: None,
        }));
        s.run("DeleteCursorItem()").unwrap();
        assert!(
            s.cursor_payload().is_some(),
            "non-item payload is untouched"
        );
        assert!(s.take_container_destroys().is_empty());
    }

    #[test]
    fn split_of_the_whole_stack_degrades_to_a_plain_pickup() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack())); // slot 1: a 5-stack
        assert!(split_container_item(&s.lua, 0, 1, 5));
        let held = s.cursor_item().expect("picked up");
        assert_eq!(held.count, None, "count>=stack ⇒ whole-stack pickup");
    }

    /// `ClearCursor(1,1)`'s coin arm signals `PLAYER_MONEY` (`0x495248`) before the tail's
    /// `CURSOR_UPDATE` (`0x4952cc`), both handlers reading the coins gone, so a money frame
    /// repaints the purse whole: Lua's `ClearCursor()` (`0x4895b0`) and a bag right-click's clear
    /// (`0x4fa198`) alike.
    #[test]
    fn a_clear_with_coins_held_signals_player_money() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        s.set_container(0, Some(one_item_backpack()));
        heard::listen(
            &s,
            &["PLAYER_MONEY", "CURSOR_UPDATE"],
            Some("GetCursorMoney()"),
        );
        for clear in ["ClearCursor()", "UseContainerItem(0, 1)"] {
            s.run("PickupPlayerMoney(10)").unwrap();
            heard::take(&s);
            s.run(clear).unwrap();
            assert_eq!(
                heard::take(&s),
                ["PLAYER_MONEY 0", "CURSOR_UPDATE 0"],
                "{clear}"
            );
        }
    }

    /// `PickupPlayerMoney`'s setter clears with `ClearCursor(1,1)` (`0x494ce0`): a held item
    /// unlocks (`0x4951d9`) and an armed gift wrap is cancelled (`0x495197`) before the coins go
    /// on and signal `PLAYER_MONEY` (`0x494d0a`).
    #[test]
    fn a_money_pickup_clears_a_held_item_and_an_armed_wrap() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        s.set_container(0, Some(one_item_backpack()));
        super::install_kind_probe(&s);
        s.run("PickupContainerItem(0, 1)").unwrap();
        heard::listen(
            &s,
            &[
                "ITEM_LOCK_CHANGED",
                "ACTIONBAR_SHOWGRID",
                "ACTIONBAR_HIDEGRID",
                "CURSOR_UPDATE",
                "PLAYER_MONEY",
            ],
            Some("CursorKind()"),
        );
        s.run("PickupPlayerMoney(10)").unwrap();
        assert_eq!(
            heard::take(&s),
            [
                "ITEM_LOCK_CHANGED none",
                "ACTIONBAR_HIDEGRID none",
                "CURSOR_UPDATE none",
                "PLAYER_MONEY money"
            ]
        );

        s.run("ClearCursor()").unwrap();
        s.model_mut().pending_wrap =
            Some(crate::script::container::PendingWrap { bag: 0, slot: 1 });
        heard::take(&s);
        s.run("PickupPlayerMoney(10)").unwrap();
        assert!(
            s.model_ref().pending_wrap.is_none(),
            "the wrap is cancelled"
        );
        assert_eq!(
            heard::take(&s),
            [
                "ITEM_LOCK_CHANGED none",
                "CURSOR_UPDATE none",
                "PLAYER_MONEY money"
            ]
        );
    }

    /// The split clears with `ClearCursor(1,1)` (`0x4fa078`) whatever is held, then picks up
    /// through `SetCursorItem`, whose inner clear signals again (`0x494bb6`), and `LockItem`.
    #[test]
    fn a_split_clears_what_is_held_then_picks_up() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        super::install_kind_probe(&s);
        s.set_cursor_for_test(CursorPayload::Macro(super::CursorMacro {
            index: 1,
            texture: None,
        }));
        heard::listen(
            &s,
            &["CURSOR_UPDATE", "ITEM_LOCK_CHANGED"],
            Some("CursorKind()"),
        );
        s.run("SplitContainerItem(0, 1, 2)").unwrap();
        assert_eq!(
            heard::take(&s),
            [
                "CURSOR_UPDATE none",
                "CURSOR_UPDATE none",
                "CURSOR_UPDATE item",
                "ITEM_LOCK_CHANGED item"
            ]
        );
        assert_eq!(s.cursor_item().and_then(|i| i.count), Some(2));
    }
}
