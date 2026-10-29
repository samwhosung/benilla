//! The merchant bindings: the getters read the open vendor through the lent host at the call
//! ([`MerchantHeadQuery`] and its kin), and the Lua verbs act inside their calls through the host
//! ([`MerchantCall`]); `CloseMerchant` alone leaves its close to the interaction latch, which runs
//! the window's close core. A row is 1-based; the host maps it to the item entry `CMSG_BUY_ITEM`
//! needs.
//!
//! `GetMerchantItemInfo(index)` answers `name, texture, price, quantity, numAvailable, isUsable`
//! (`0x4fb150`), or `(nil, nil, 0, 1, 0, 1)` for an invalid index (`0x4fb2be`). Unlimited stock is
//! `numAvailable` -1; `isUsable` is 1 or nil, the [`super::item_stats::item_usable`] gate, and a
//! template still in flight reads usable (`0x4fb298`).

use mlua::{Lua, MultiValue, Value};

use super::live::{ask, LiveQuery};

use super::binding_abi::{self, flag, number_arg};
use super::container::UiCursorMode;
use super::cursor::CursorPayload;
use super::Model;

/// One vendor row, resolved by the app; its 1-based index is its place in [`MerchantState::items`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MerchantItem {
    /// The item name; `None` while its template is in flight, answered as nil.
    pub name: Option<String>,
    /// Icon path; `None` while the template is in flight.
    pub texture: Option<String>,
    /// Buy price in copper, already reputation-discounted by the server.
    pub price: u32,
    /// Stack size per purchase (`item_template.buy_count`).
    pub quantity: u32,
    /// Remaining stock, -1 for unlimited (the wire's `0xFFFF_FFFF`).
    pub num_available: i32,
    /// The template entry `CMSG_BUY_ITEM` addresses; Lua never sees it.
    pub item_id: u32,
    /// The hover tooltip's stat head; `None` while the template is in flight.
    pub stats: Option<ItemStatsHead>,
    /// `GetMerchantItemLink`'s answer; `None` while in flight and on a buyback row, as 1.12 has no
    /// `GetBuybackItemLink` and the buyback click takes no modifier (`MerchantFrame.lua:358-361`).
    pub link: Option<String>,
    /// The template's `stackable`, `GetMerchantItemMaxStack`'s answer for a row sold singly;
    /// `None` while in flight and on a buyback row.
    pub max_stack: Option<u32>,
}

/// An item template's tooltip stat head, resolved per row by the app, which `SetMerchantItem` and
/// `SetBuybackItem` draw while the VM's template is in flight. The reference reads the template in
/// C++ (`0x534080`); no 1.12 Lua API carries these.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ItemStatsHead {
    /// 0 poor to 6 artifact; colours the tooltip's name line.
    pub quality: u32,
    /// `InventoryType`, the tooltip's slot line ("Main Hand", "Chest"); 0 for none.
    pub inventory_type: u32,
    /// Item class (2 weapon, 4 armor, 6 projectile); with `subclass`, the slot line's right side.
    pub class: u32,
    /// Item subclass within `class` (7 = sword, 1 = cloth, …).
    pub subclass: u32,
    /// Damage block 0's per-hit minimum, 0 for a non-weapon.
    pub dmg_min: f32,
    /// Damage block 0's per-hit maximum.
    pub dmg_max: f32,
    /// Damage block 0's school (0 physical, 1 Holy to 6 Arcane).
    pub dmg_type: u32,
    /// Attack delay in milliseconds; the tooltip's "Speed" is delay / 1000.
    pub delay_ms: u32,
    /// The armor line's value; 0 for none.
    pub armor: u32,
    /// A shield's block line; 0 for none.
    pub block: u32,
    /// What a vendor pays per unit; 0 shows the tooltip's "No sell price" line.
    pub sell_price: u32,
}

/// Each damaged item's repair cost in copper after the vendor's discount (`0x4faf30`): the VM's
/// copy the tooltips read with no host to ask ([`RepairCostQuery`]), which only a test sets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepairCosts {
    /// Equipped items by live inventory id.
    pub equipped: std::collections::HashMap<u32, u32>,
    /// Container items by `(bag, slot)`: bags 0-4, the bank vault -1 and bank bags 5-10.
    pub bags: std::collections::HashMap<(i64, u32), u32>,
    /// The item's own cost before the discount (`0x5da330`), by item guid, for every item of
    /// [`Self::bags`]: what the merchant price takes off (`0x52e44a`).
    pub own: std::collections::HashMap<u64, u32>,
}

/// One open merchant window, pushed whole by the app; `None` means no vendor is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MerchantState {
    pub items: Vec<MerchantItem>,
    /// Buyback slots, oldest first; the last, the latest sale, is the one the merchant page shows
    /// (`MerchantFrame.lua:133`). From the player's VENDORBUYBACK fields: `price` is the buyback
    /// price, `quantity` the stored stack.
    pub buyback: Vec<MerchantItem>,
    /// Whether this vendor repairs (the `UNIT_NPC_FLAGS` repair bit).
    pub can_repair: bool,
}

/// The window as the getters read it at the call: whether the vendor cell `[0xbddfa0]` is set, the
/// row count `[0xbddfa8]` (`0x4fb130`), which the close zeroes (`0x4fae22`), the buy-back count
/// with a vendor (`0x4fc166`), and the open vendor's repair flag (`0x4fadb0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MerchantHead {
    pub open: bool,
    pub items: usize,
    pub buyback: usize,
    pub can_repair: bool,
}

/// The question: [`MerchantHead`] now.
pub struct MerchantHeadQuery;

impl LiveQuery for MerchantHeadQuery {
    type Answer = MerchantHead;
}

/// The open window's 0-based row (`0xbdd118 + 0x1c·i`), resolved off the item cache at the call;
/// `None` past the count.
pub struct MerchantRowQuery(pub usize);

impl LiveQuery for MerchantRowQuery {
    type Answer = Option<MerchantItem>;
}

/// The 0-based buy-back row of the list `0x4fafd0` keeps, oldest first (`0xbddf24`), which
/// `GetBuybackItemInfo` reads with no vendor test (`0x4fb310`); `None` past the count.
pub struct BuybackRowQuery(pub usize);

impl LiveQuery for BuybackRowQuery {
    type Answer = Option<MerchantItem>;
}

/// `GetRepairAllCost`'s sum and its damaged flag (`0x4fbd60`), `None` with no repairer open or
/// no player (`0x4fbd76`).
pub struct RepairAllCostQuery;

impl LiveQuery for RepairAllCostQuery {
    type Answer = Option<(u32, bool)>;
}

/// The repair cost the item tooltips return for the item a slot holds, read at the call
/// (`0x534948`, `0x5332e1`): `0x4faf30`, the object's own cost after the open vendor's discount,
/// 0 for no object. `bag` is the Lua container id, or [`super::EQUIPMENT_BAG`] with a doll live id.
pub struct RepairCostQuery {
    pub bag: i64,
    pub slot: u32,
}

impl LiveQuery for RepairCostQuery {
    type Answer = u32;
}

/// An item object's own repair cost before any discount (`0x5da330`), which the merchant price
/// takes off (`0x52e44a`); 0 for no such object.
pub struct ItemRepairCostQuery(pub u64);

impl LiveQuery for ItemRepairCostQuery {
    type Answer = u32;
}

/// The repair cost of the item at `bag`'s 1-based `slot` now, as [`RepairCostQuery`] reads it:
/// the host's, else the VM's copy, which only a test sets.
pub(crate) fn repair_cost_at(lua: &Lua, model: &Model, bag: i64, slot: u32) -> u32 {
    ask(lua, &RepairCostQuery { bag, slot }).unwrap_or_else(|| {
        let costs = &model.repair_costs;
        let cost = match bag {
            super::EQUIPMENT_BAG => costs.equipped.get(&slot),
            _ => costs.bags.get(&(bag, slot)),
        };
        cost.copied().unwrap_or(0)
    })
}

/// The item object `guid`'s own repair cost now, as [`ItemRepairCostQuery`] reads it.
pub(crate) fn item_repair_cost(lua: &Lua, model: &Model, guid: u64) -> u32 {
    ask(lua, &ItemRepairCostQuery(guid))
        .unwrap_or_else(|| model.repair_costs.own.get(&guid).copied().unwrap_or(0))
}

/// The window's head now: the host's, else the VM's copy, which only a test sets (every
/// production entry that runs Lua lends the host).
pub(crate) fn head(lua: &Lua, model: &Model) -> MerchantHead {
    ask(lua, &MerchantHeadQuery).unwrap_or_else(|| {
        model
            .merchant
            .as_ref()
            .map_or_else(MerchantHead::default, |m| MerchantHead {
                open: true,
                items: m.items.len(),
                buyback: m.buyback.len(),
                can_repair: m.can_repair,
            })
    })
}

/// The 0-based vendor row now, as [`head`] reads.
pub(crate) fn row(lua: &Lua, model: &Model, index: usize) -> Option<MerchantItem> {
    match ask(lua, &MerchantRowQuery(index)) {
        Some(row) => row,
        None => model.merchant.as_ref()?.items.get(index).cloned(),
    }
}

/// The 0-based buy-back row now, as [`head`] reads.
pub(crate) fn buyback_row(lua: &Lua, model: &Model, index: usize) -> Option<MerchantItem> {
    match ask(lua, &BuybackRowQuery(index)) {
        Some(row) => row,
        None => model.merchant.as_ref()?.buyback.get(index).cloned(),
    }
}

/// What a verb's model-side half reads of the vendor, read at its call before the model is
/// borrowed: whether the cell is set, and the entry of the vendor row the cursor holds, `None`
/// when no row is held or the list no longer holds its index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VendorAtCall {
    pub(crate) open: bool,
    pub(crate) held_entry: Option<u32>,
}

/// [`VendorAtCall`] now.
pub(crate) fn at_call(lua: &Lua) -> VendorAtCall {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let open = head(lua, &model).open;
    let held_entry = match &model.cursor {
        Some(super::cursor::CursorPayload::Merchant(held)) if open => {
            row(lua, &model, held.row as usize).map(|it| it.item_id)
        }
        _ => None,
    };
    VendorAtCall { open, held_entry }
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open vendor.
    pub fn set_merchant(&mut self, state: Option<MerchantState>) {
        self.model_mut().merchant = state;
    }

    /// Set the repair costs the item tooltip bindings return with no host to ask, for a test.
    pub fn set_repair_costs(&mut self, costs: RepairCosts) {
        self.model_mut().repair_costs = costs;
    }

    /// The `(row, quantity)` buys a harness's VM kept ([`Self::keep_calls`]), the row 1-based.
    pub fn take_merchant_buys(&mut self) -> Vec<(u32, u32)> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Merchant(MerchantCall::Buy { row, quantity }) => {
                Some((*row, *quantity))
            }
            _ => None,
        })
    }

    /// The merchant calls a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_merchant_calls(&mut self) -> Vec<MerchantCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Merchant(call) => Some(*call),
            _ => None,
        })
    }

    /// The `(bag, slot, entry)` of held rows a harness's VM kept dropped into slots
    /// ([`Self::keep_calls`], `CMSG_BUY_ITEM_IN_SLOT`).
    pub fn take_merchant_slot_buys(&mut self) -> Vec<(i64, u32, u32)> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Merchant(MerchantCall::BuyInSlot { bag, slot, entry }) => {
                Some((*bag, *slot, *entry))
            }
            _ => None,
        })
    }

    /// The 1-based buyback slots a harness's VM kept ([`Self::keep_calls`]) for `BuybackItem`.
    pub fn take_merchant_buybacks(&mut self) -> Vec<u32> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Merchant(MerchantCall::Buyback(index)) => Some(*index),
            _ => None,
        })
    }

    /// Whether a harness's VM kept ([`Self::keep_calls`]) a `RepairAllItems`.
    pub fn take_repair_all(&mut self) -> bool {
        !self
            .take_calls_where(|call| {
                matches!(call, super::ScriptCall::Merchant(MerchantCall::RepairAll)).then_some(())
            })
            .is_empty()
    }

    /// The repair-mode latch: while set, a bag or equipment click repairs the item.
    pub fn repair_mode(&self) -> bool {
        self.model_ref().repair_mode
    }

    /// Leave repair mode, as `HideRepairCursor` does, for the world's right mouse-down, which
    /// resets the Repair base mode to Point (`0x492c68`), and the merchant close's tail
    /// (`0x4fae4f`-`0x4fae68`).
    pub fn end_repair_mode(&mut self) {
        self.model_mut().repair_mode = false;
    }
}

/// A merchant verb past its binding's checks, which the host applies inside the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MerchantCall {
    /// `BuyMerchantItem(index, quantity)` (`0x4fb850`), after its cursor clear: with the row in
    /// range (`0x4fb904`-`0x4fb90e`), a vendor cell (`0x4fb91d`) and a row slot (`0x4fb92e`),
    /// `CMSG_BUY_ITEM` for the row's item (`0x4fb93c`).
    Buy {
        /// 1-based.
        row: u32,
        /// 1 to 255 (`0x4fb8bf`-`0x4fb8ff`).
        quantity: u32,
    },
    /// `PickupMerchantItem`'s sale (`0x4fb7a7` → `0x5e1d50`): `CMSG_SELL_ITEM` with the vendor
    /// cell as it stands, never tested, the cursor item's guid (`0x494c60`) and its split count
    /// (`[0xb4b40c]`, 0 for a whole stack).
    Sell { item_guid: u64, count: u8 },
    /// `BuybackItem(index)` (`0x4fb950`), 1-based into the buyback list.
    Buyback(u32),
    /// `RepairAllItems()` (`0x4fbfe0`) at a repairing vendor: the purse test, then
    /// `CMSG_REPAIR_ITEM` with guid 0.
    RepairAll,
    /// A held vendor row dropped into a bag or doll slot (`0x4f9f35`, `0x4c78fd`):
    /// `CMSG_BUY_ITEM_IN_SLOT` (`0x1a3`) for the row's item into Lua's `(bag, slot)`, count 1.
    BuyInSlot { bag: i64, slot: u32, entry: u32 },
    /// A held vendor row dropped on a bag slot or the backpack button (`0x4c7e66`-`0x4c7eaa`):
    /// `CMSG_BUY_ITEM_IN_SLOT` into the item in that bag slot, `None` the player's own for the
    /// backpack (`0x4c7c7d`), slot `0xFF` so the server picks, count 1.
    BuyIntoBag { bag_guid: Option<u64>, entry: u32 },
    /// `CloseMerchant()` (`0x4fb120`): the close core `0x4fadf0`.
    Close,
}

/// Hand `call` to the host, which applies it before this returns ([`super::calls::submit`]).
fn submit(lua: &Lua, call: MerchantCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Merchant(call))
}

/// 1 or nil, as the client pushes a usable flag (`pushnumber(1.0)` / `pushnil`).
fn usable_value(usable: bool) -> Value {
    if usable {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// Both getters' invalid-index six-tuple (`0x4fb2be`, and the buyback one).
fn invalid_tuple() -> Vec<Value> {
    vec![
        Value::Nil,
        Value::Nil,
        Value::Integer(0),
        Value::Integer(1),
        Value::Integer(0),
        Value::Integer(1),
    ]
}

/// The 0-based row `PickupMerchantItem(index)` names: `None` for a non-number (`0x4fb7cb`; a
/// numeric string passes) and `index < 1` (`0x4fb7e1`). `_ftol` truncates toward zero, then the
/// bound applies signed. The upper bound is load-bearing: `2^32 + 1` would narrow to row 0, where
/// the reference bounds against the vendor count before any narrowing.
fn grab_index(index: Option<f64>) -> Option<u32> {
    let row = index?.trunc();
    if row.is_nan() || row < 1.0 || row > f64::from(u32::MAX) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(row as u32 - 1)
}

/// The row `PickupMerchantItem` grabs, its 0-based index and row read at the call before its
/// clear ([`grab_index`]): `None` for each silent refusal, no index, `index > count`
/// (`0x4fb7e9`), no merchant (`0x4fb7f7`), a dead row (`0x4fb80b`), or the toggle-off: a held
/// preview whose entry is the row's (`0x4fb811`-`0x4fb818`). Neither stock nor the item cache is
/// tested: a sold-out row grabs, and the server refuses the buy.
fn merchant_grab(model: &Model, grab_row: Option<(u32, MerchantItem)>) -> Option<CursorPayload> {
    // The preview entry `[0xb4b41c]` (`0x495170`), set for modes 5, 7 and 9 alike.
    let held = match &model.cursor {
        Some(CursorPayload::Merchant(m)) => Some(m.item_id),
        Some(CursorPayload::Action(a)) => Some(a.action),
        Some(CursorPayload::Ammo(a)) => Some(a.item_id),
        _ => None,
    };
    let (row0, item) = grab_row?;
    if held == Some(item.item_id) {
        return None;
    }
    Some(CursorPayload::Merchant(super::cursor::CursorMerchantItem {
        item_id: item.item_id,
        row: row0,
        texture: item.texture.clone(),
    }))
}

/// Register the merchant globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetMerchantNumItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(head(lua, &model).items as i64)
        })?,
    )?;

    g.set(
        "GetMerchantItemInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetMerchantItemInfo(index)` (`0x84dc48`, `0x4fb16b`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetMerchantItemInfo(index)")?;
            let (item, usable) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let item = index.checked_sub(1).and_then(|n| row(lua, &model, n));
                let usable = item
                    .as_ref()
                    .is_none_or(|it| super::item_stats::item_usable_by_id(lua, &model, it.item_id));
                (item, usable)
            };
            let Some(it) = item else {
                return Ok(MultiValue::from_vec(invalid_tuple()));
            };
            // A row's record the cache misses is asked with the callback `0x4fada0`, the window's
            // wait counted (`0x4fb1d4` → `0x4faf1a`, `0x4faf23`), the query naming the vendor's
            // cell, which the host holds (`0x4faef6`-`0x4faf17`); entry 0 asks nothing
            // (`0x4faee9`).
            if it.name.is_none() && it.item_id != 0 {
                super::calls::ask(
                    lua,
                    super::RecordAsk::Item {
                        entry: it.item_id,
                        owner: 0,
                        callback: super::ItemCallback::Merchant,
                    },
                )?;
            }
            let name = match &it.name {
                Some(n) => Value::String(lua.create_string(n)?),
                None => Value::Nil,
            };
            // A string on every path: the wire display id has no icon row, or no row, so
            // `0x5d88b0` answers the question mark (`0x4fb214`, `0x5d8927`).
            let texture = Value::String(
                lua.create_string(
                    it.texture
                        .as_deref()
                        .unwrap_or(super::item_stats::MISSING_ITEM_ICON),
                )?,
            );
            Ok(MultiValue::from_vec(vec![
                name,
                texture,
                Value::Integer(i64::from(it.price)),
                Value::Integer(i64::from(it.quantity)),
                Value::Integer(i64::from(it.num_available)),
                usable_value(usable),
            ]))
        })?,
    )?;

    // GetMerchantItemLink(index): no values out of range, with no vendor and while in flight
    // (`0x4fb5dc`-`0x4fb627` → `0x4fb5bf`); both stock row-click arms read that as nil
    // (`MerchantFrame.lua:303`, `:306`).
    g.set(
        "GetMerchantItemLink",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetMerchantItemLink(index)` (`0x84dc8c`, `0x4fb5b1`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetMerchantItemLink(index)")?;
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                index
                    .checked_sub(1)
                    .and_then(|n| row(lua, &model, n))
                    .and_then(|it| it.link)
            };
            match link {
                Some(link) => Ok(MultiValue::from_vec(vec![Value::String(
                    lua.create_string(&link)?,
                )])),
                None => Ok(MultiValue::new()),
            }
        })?,
    )?;

    // GetMerchantItemMaxStack(index) (`0x4fb670`): the template's stack size, or 1 for a row sold
    // in bundles (`[row+0x18] > 1`) and for every miss: out of range, no vendor, a template in
    // flight (`0x4fb739`). A right shift-click, or a left one with the chat box closed, asks it and
    // opens the stack-split spinner only above 1 (`MerchantFrame.lua:313`, `:340`).
    g.set(
        "GetMerchantItemMaxStack",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetMerchantItemMaxStack(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // Signed, as 1.12.1 reads both.
            let max_stack = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|n| row(lua, &model, n))
                .filter(|it| it.quantity as i32 <= 1)
                .and_then(|it| it.max_stack)
                .map_or(1, |n| n as i32);
            Ok(i64::from(max_stack))
        })?,
    )?;

    g.set(
        "BuyMerchantItem",
        lua.create_function(|lua, (index, quantity): (Value, Value)| {
            // `is-number` else `Usage: BuyMerchantItem(index)` (`0x84dcd8`, `0x4fb861`).
            let index = binding_abi::index_arg32(lua, index, "Usage: BuyMerchantItem(index)")?;
            // `ClearCursor(1, 1)` before anything is read (`0x4fb89f`).
            super::tick::verb(lua, crate::script::cursor::clear_cursor);
            // The quantity is 1 unless `is-number` passes and `_ftol` gives 1 or more, and at most
            // 255 (`0x4fb8bf`-`0x4fb8ff`); a non-number never raises.
            let quantity = lua
                .coerce_number(quantity)?
                .map(binding_abi::ftol)
                .map_or(1, |q| q.clamp(1, 0xff)) as u32;
            submit(
                lua,
                MerchantCall::Buy {
                    row: index,
                    quantity,
                },
            )
        })?,
    )?;

    // PickupMerchantItem(index): two verbs in one (`0x4fb760`), forked at `0x4fb787` on
    // `GetCursorItem 0x494c60`, which answers for mode 1 only; stock never checks
    // `CursorHasItem()` first (`MerchantFrame.lua:329`). A bag item on the cursor is sold, index
    // ignored and no merchant-open gate, which stock `MerchantFrame.xml:743`'s
    // `PickupMerchantItem(0)` relies on; otherwise row `index - 1` is grabbed as cursor mode 5.
    // Every refusal ([`merchant_grab`]) is silent and clears the cursor.
    g.set(
        "PickupMerchantItem",
        lua.create_function(|lua, index: Value| {
            // `lua_isnumber` (`0x4fb7c4`) gates the row; a miss is `merchant_grab`'s refusal.
            let index = crate::script::binding_abi::is_number(lua, index);
            // The row a grab would take, read at the call before the model is borrowed.
            let grab_row = grab_index(index).and_then(|row0| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row(lua, &model, row0 as usize).map(|it| (row0, it))
            });
            // The sell fork comes first, before the argument is read: the sale goes out
            // (`0x4fb7a7`), handed over in place, then its clear, `ClearCursor(0,1)` (`0x4fb7b3`),
            // whose `CURSOR_UPDATE` follows the packet; the source slot stays greyed until the
            // server's inventory update. Every other leg reads the row, then clears.
            let grab = super::tick::verb(lua, |model| {
                let held = match &model.cursor {
                    Some(crate::script::cursor::CursorPayload::Item(item)) => Some(item.clone()),
                    _ => None,
                };
                match held {
                    Some(item) => {
                        model.merchant_call_in_verb(MerchantCall::Sell {
                            item_guid: item.guid,
                            count: item.count.unwrap_or(0) as u8,
                        });
                        crate::script::cursor::take_cursor_item_for_sale(model);
                        None
                    }
                    None => Some(merchant_grab(model, grab_row)),
                }
            });
            let Some(grab) = grab else {
                return Ok(());
            };

            // Every other leg clears with the real `ClearCursor(1,1)` (`0x4fb82d`/`0x4fb83f` on a
            // refusal, `0x495112` in the grab), not a bare drop: a held spell fires `CURSOR_UPDATE`
            // and hides the bar grid, and an armed gift wrap is cancelled (`0x5edf10`). Its events
            // run before the grab sets the row, so that `CURSOR_UPDATE` reads an empty cursor.
            // The grab is the setter (`0x4950f0`): its control test (`0x4950f3`), its clear, then
            // the row and its `CURSOR_UPDATE` (`0x49515e`); mode 5 skips the mode-7
            // `ACTIONBAR_SHOWGRID` branch. A refusal's clear is the binding's own, untested.
            match grab {
                Some(grab) => {
                    crate::script::cursor::set_cursor(
                        lua,
                        true,
                        crate::script::cursor::Setter::Controlled,
                        |_| Some(grab),
                    );
                }
                None => super::tick::verb(lua, crate::script::cursor::clear_cursor),
            }
            Ok(())
        })?,
    )?;

    // CloseMerchant() (`0x4fb120`): the close core `0x4fadf0` inside the call; no packet.
    g.set(
        "CloseMerchant",
        lua.create_function(|lua, ()| submit(lua, MerchantCall::Close))?,
    )?;

    g.set(
        "GetNumBuybackItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(head(lua, &model).buyback as i64)
        })?,
    )?;

    // GetBuybackItemInfo(index): the same six-tuple (`0x4fb310`), oldest first. isUsable is the
    // same `0x5ea930` gate over the sold item (`0x4fb4f7`): one its seller cannot use reads red.
    g.set(
        "GetBuybackItemInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetBuybackItemInfo(slot)` (`0x84dc6c`, `0x4fb33e`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetBuybackItemInfo(slot)")?;
            let (item, usable) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let item = index
                    .checked_sub(1)
                    .and_then(|n| buyback_row(lua, &model, n));
                let usable = item
                    .as_ref()
                    .is_none_or(|it| super::item_stats::item_usable_by_id(lua, &model, it.item_id));
                (item, usable)
            };
            let Some(it) = item else {
                return Ok(MultiValue::from_vec(invalid_tuple()));
            };
            // A template the cache lacks is asked with the callback `0x4fb570`, the query's guid 0
            // (`0x4fb3f0`-`0x4fb3f7`), and takes the miss (`0x4fb40c`, `0x4fb416`, `0x4fb515`); a
            // cached one reaches the icon through its
            // display (`0x4fb45e`, `0x5d8af0`), falling to the question mark when that row is
            // missing or bare.
            if it.name.is_none() {
                if it.item_id != 0 {
                    super::calls::ask(
                        lua,
                        super::RecordAsk::Item {
                            entry: it.item_id,
                            owner: 0,
                            callback: super::ItemCallback::Buyback,
                        },
                    )?;
                }
                return Ok(MultiValue::from_vec(invalid_tuple()));
            }
            let name = Value::String(lua.create_string(it.name.as_deref().unwrap_or_default())?);
            let texture = Value::String(
                lua.create_string(
                    it.texture
                        .as_deref()
                        .unwrap_or(super::item_stats::MISSING_ITEM_ICON),
                )?,
            );
            Ok(MultiValue::from_vec(vec![
                name,
                texture,
                Value::Integer(i64::from(it.price)),
                Value::Integer(i64::from(it.quantity)),
                Value::Integer(i64::from(it.num_available)),
                usable_value(usable),
            ]))
        })?,
    )?;

    g.set(
        "BuybackItem",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: BuybackItem(slot)` (`0x84dcf8`, `0x4fb968`).
            let index = binding_abi::index_arg32(lua, index, "Usage: BuybackItem(slot)")?;
            submit(lua, MerchantCall::Buyback(index))
        })?,
    )?;

    g.set(
        "CanMerchantRepair",
        lua.create_function(|lua, ()| {
            let can = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                head(lua, &model).can_repair
            };
            Ok(if can { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // GetRepairAllCost() → cost, canRepair: whether there is damage to pay for, which enables the
    // repair-all button (`MerchantFrame.lua:38`). `canRepair` is 1 or nil (`0x4fbfa8`-`0x4fbfce`).
    // With no open merchant that repairs (`0x4fadb0`) it answers the one value 0 (`0x4fbd76`).
    g.set(
        "GetRepairAllCost",
        lua.create_function(|lua, ()| {
            // With no host, the VM's copy names the repairer and no damage.
            let total = ask(lua, &RepairAllCostQuery).unwrap_or_else(|| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                head(lua, &model).can_repair.then_some((0, false))
            });
            let Some((cost, damaged)) = total else {
                return Ok(MultiValue::from_vec(vec![Value::Integer(0)]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(cost)),
                flag(damaged),
            ]))
        })?,
    )?;

    g.set(
        "RepairAllItems",
        lua.create_function(|lua, ()| {
            // `0x4fbff0`: nothing, not even the purse test, unless the open merchant repairs
            // (`0x4fadb0`, `UNIT_NPC_FLAGS` bit 14).
            let repairs = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                head(lua, &model).can_repair
            };
            if repairs {
                submit(lua, MerchantCall::RepairAll)?;
            }
            Ok(())
        })?,
    )?;

    g.set(
        "ShowRepairCursor",
        lua.create_function(|lua, ()| {
            // `0x4fbcc0`: a targeting spell or a vendor without repair service leaves the
            // cursor untouched. `ClearCursor(1,1)` returns any held item before mode 0x11 arms,
            // its events running first.
            {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                if super::action::spell_targeting(lua) || !head(lua, &model).can_repair {
                    return Ok(());
                }
            }
            super::tick::verb(lua, crate::script::cursor::clear_cursor);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.repair_mode = true;
            model.ui_cursor = None;
            model.ui_cursor_dirty = true;
            Ok(())
        })?,
    )?;
    g.set(
        "HideRepairCursor",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .repair_mode = false;
            Ok(())
        })?,
    )?;
    g.set(
        "InRepairMode",
        lua.create_function(|lua, ()| {
            let mode = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .repair_mode;
            Ok(if mode { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // ShowMerchantSellCursor(index) (`0x4fbab0`) and ShowBuybackSellCursor(index) (`0x4fbbb0`):
    // despite the names, the buy cursor, re-armed each frame an item is hovered
    // (`MerchantFrame.xml:726-733`). Every fail path returns without `CursorSetMode`; the
    // targeting and base-mode gates open each verb ([`sell_cursor_shut`]), and the in-flight item
    // lock lives app-side.
    g.set(
        "ShowMerchantSellCursor",
        lua.create_function(|lua, index: Value| {
            // The targeting and base-mode gates come first (`0x4fbab7`-`0x4fbacc`).
            if sell_cursor_shut(lua) {
                return Ok(());
            }
            // `is-number` else `Usage: ShowMerchantSellCursor(index)` (`0x84dd14`, `0x4fbadf`).
            let index = binding_abi::index_arg(lua, index, "Usage: ShowMerchantSellCursor(index)")?;
            arm_vendor_cursor(lua, |model| {
                row(lua, model, index.wrapping_sub(1)).map(|it| it.price)
            });
            Ok(())
        })?,
    )?;
    g.set(
        "ShowBuybackSellCursor",
        lua.create_function(|lua, index: Value| {
            // The targeting and base-mode gates come first (`0x4fbbb9`-`0x4fbbce`).
            if sell_cursor_shut(lua) {
                return Ok(());
            }
            // `is-number` else `Usage: ShowBuybackSellCursor(index)` (`0x84dd3c`, `0x4fbbe1`).
            let index = binding_abi::index_arg(lua, index, "Usage: ShowBuybackSellCursor(index)")?;
            arm_vendor_cursor(lua, |model| {
                buyback_row(lua, model, index.wrapping_sub(1)).map(|it| it.price)
            });
            Ok(())
        })?,
    )?;

    Ok(())
}

/// The gates every sell-cursor verb opens with, ahead of its argument reads: `IsSpellTargeting`
/// (`0x6e48a0`), then the base cursor mode (`0x523d10`) must be 1, which the repair cursor moves
/// (`0x4fbce3`). Either failing returns quietly; each is sticky, so a Buy would stamp it out.
pub(super) fn sell_cursor_shut(lua: &Lua) -> bool {
    super::action::spell_targeting(lua)
        || lua
            .app_data_ref::<Model>()
            .expect("model app_data")
            .repair_mode
}

/// Arm the vendor hover cursor from a row's price, read at the call: Buy(3) if the player's coinage
/// covers it, else the greyed UnableBuy(23); no price leaves the cursor untouched. The caller has
/// passed [`sell_cursor_shut`].
fn arm_vendor_cursor(lua: &Lua, price_of: impl FnOnce(&Model) -> Option<u32>) {
    let price = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        price_of(&model)
    };
    let Some(price) = price else {
        return;
    };
    let money = super::stat_read::read_self(lua, |own| own.money());
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    model.ui_cursor = Some(if money >= u64::from(price) {
        UiCursorMode::Buy
    } else {
        UiCursorMode::UnableBuy
    });
    model.ui_cursor_dirty = true;
}

#[cfg(test)]
mod tests {
    use super::{ItemStatsHead, MerchantItem, MerchantState};
    use crate::script::{ContainerSlot, ContainerState, UiScript};

    fn stock() -> MerchantState {
        MerchantState {
            items: vec![
                MerchantItem {
                    name: Some("Refreshing Spring Water".into()),
                    texture: Some("Interface\\Icons\\INV_Drink_18".into()),
                    price: 25,
                    quantity: 5,
                    num_available: -1, // unlimited
                    item_id: 159,
                    stats: Some(ItemStatsHead {
                        quality: 1,
                        ..Default::default()
                    }),
                    link: Some("|cffffffff|Hitem:159:0:0:0|h[Refreshing Spring Water]|h|r".into()),
                    max_stack: Some(20),
                },
                // In flight: the list arrived, its item template has not.
                MerchantItem {
                    name: None,
                    texture: None,
                    price: 1500,
                    quantity: 1,
                    num_available: 3,
                    item_id: 4540,
                    stats: None,
                    link: None,
                    max_stack: None, // in flight, like `name` above
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn merchant_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetMerchantNumItems()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetMerchantItemInfo(1) == nil")
            .unwrap());

        s.set_merchant(Some(stock()));
        assert_eq!(s.eval::<i64>("return GetMerchantNumItems()").unwrap(), 2);
        let (name, texture, price, quantity, num, usable) = s
            .eval::<(String, String, i64, i64, i64, i64)>("return GetMerchantItemInfo(1)")
            .unwrap();
        assert_eq!(name, "Refreshing Spring Water");
        assert_eq!(texture, "Interface\\Icons\\INV_Drink_18");
        assert_eq!((price, quantity, num), (25, 5, -1));
        assert_eq!(usable, 1);

        // In flight: name nil, the texture the question mark (the wire display has no icon row,
        // `0x4fb214`), and usable (the null-record skip).
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetMerchantItemInfo(2)\n\
                 return n == nil and t == 'Interface\\\\Icons\\\\INV_Misc_QuestionMark' \
                 and p == 1500 and u == 1",
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetMerchantItemInfo(9)\n\
                 return n == nil and t == nil and p == 0 and q == 1 and a == 0 and u == 1",
            )
            .unwrap());

        assert_eq!(
            s.eval::<String>("return GetMerchantItemLink(1)").unwrap(),
            "|cffffffff|Hitem:159:0:0:0|h[Refreshing Spring Water]|h|r"
        );
        // A miss returns no values (`0x4fb5bf`), not a nil.
        assert_eq!(s.arity("GetMerchantItemLink(1)").unwrap(), 1);
        assert_eq!(s.arity("GetMerchantItemLink(2)").unwrap(), 0, "in flight");
        assert_eq!(
            s.arity("GetMerchantItemLink(9)").unwrap(),
            0,
            "out of range"
        );
    }

    #[test]
    fn merchant_max_stack_is_one_for_bundles_and_misses() {
        let mut s = UiScript::new().unwrap();
        let max = |s: &mut UiScript, arg: &str| {
            s.eval::<i64>(&format!("return GetMerchantItemMaxStack({arg})"))
                .unwrap()
        };
        assert_eq!(max(&mut s, "1"), 1, "no vendor open");

        let mut state = stock();
        let single = |quantity: u32, max_stack: u32| MerchantItem {
            name: Some("Flask of Oil".into()),
            price: 100,
            quantity,
            num_available: -1,
            item_id: 814,
            max_stack: Some(max_stack),
            ..Default::default()
        };
        state.items.push(single(1, 20));
        state.items.push(single(1, 0));
        state.items.push(single(0x8000_0000, u32::MAX));
        s.set_merchant(Some(state));

        assert_eq!(max(&mut s, "1"), 1, "a bundle row takes no split");
        assert_eq!(max(&mut s, "2"), 1, "a template in flight");
        assert_eq!(max(&mut s, "3"), 20, "a single row answers its stack size");
        assert_eq!(max(&mut s, "4"), 0, "the stack size is not floored");
        assert_eq!(max(&mut s, "5"), -1, "both fields read signed");
        for out_of_range in ["0", "-1", "6", "9"] {
            assert_eq!(max(&mut s, out_of_range), 1, "index {out_of_range}");
        }
        assert_eq!(max(&mut s, "\"3\""), 20);
        assert_eq!(max(&mut s, "3.9"), 20);
        for bad in ["", "nil", "\"x\"", "{}"] {
            let err = s
                .eval::<i64>(&format!("return GetMerchantItemMaxStack({bad})"))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("Usage: GetMerchantItemMaxStack(index)"),
                "{bad:?}: {err}"
            );
        }
    }

    #[test]
    fn merchant_usable_tracks_the_item_gate() {
        use crate::script::{ItemTemplateView, PlayerReqState};
        let mut s = UiScript::new().unwrap();
        let mut state = stock();
        state.buyback = vec![MerchantItem {
            name: Some("Light Mail Gloves".into()),
            texture: Some("Interface\\Icons\\INV_Gauntlets_05".into()),
            price: 47,
            quantity: 1,
            item_id: 2418,
            ..Default::default()
        }];
        s.set_merchant(Some(state));
        s.set_item_template(
            159, // row 1, level-gated for the test
            ItemTemplateView {
                name: "Refreshing Spring Water".into(),
                required_level: 5,
                allowable_class: -1,
                allowable_race: -1,
                ..Default::default()
            },
        );
        s.set_item_template(
            2418,
            ItemTemplateView {
                name: "Light Mail Gloves".into(),
                required_level: 5,
                allowable_class: -1,
                allowable_race: -1,
                ..Default::default()
            },
        );
        let req = |level| PlayerReqState {
            level,
            class_id: 1,
            race_id: 1,
            ..Default::default()
        };
        s.set_player_req_state(req(4));
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetMerchantItemInfo(1)\nreturn u == nil and n ~= nil",
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetBuybackItemInfo(1)\nreturn u == nil and n ~= nil",
            )
            .unwrap());
        // Row 2 has no template, so it is usable whatever the player state.
        assert!(s
            .eval::<bool>("local n, t, p, q, a, u = GetMerchantItemInfo(2)\nreturn u == 1")
            .unwrap());
        s.set_player_req_state(req(5));
        assert!(s
            .eval::<bool>("local n, t, p, q, a, u = GetMerchantItemInfo(1)\nreturn u == 1")
            .unwrap());
        assert!(s
            .eval::<bool>("local n, t, p, q, a, u = GetBuybackItemInfo(1)\nreturn u == 1")
            .unwrap());
    }

    #[test]
    fn buy_merchant_item_calls_with_its_row_and_quantity() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));
        s.run("BuyMerchantItem(1)").unwrap(); // default quantity 1
        s.run("BuyMerchantItem(2, 5)").unwrap();
        assert_eq!(s.take_merchant_buys(), vec![(1, 1), (2, 5)]);
    }

    #[test]
    fn close_merchant_is_a_call() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));
        s.run("CloseMerchant()").unwrap();
        assert_eq!(s.take_merchant_calls(), [super::MerchantCall::Close]);
    }

    /// The question-mark fallback and the buyback miss: a vendor row whose wire display has no
    /// icon (or no row) answers the question mark (`0x5d88b0` at `0x4fb214`), a buyback row with a
    /// cached template and no icon does too (`0x5d8af0` at `0x4fb45e`), and one whose template is
    /// uncached takes the miss tuple nil, nil, 0, 1, 0, 1 (`0x4fb416`, `0x4fb515`).
    #[test]
    fn merchant_and_buyback_icons_fall_back_and_an_uncached_buyback_misses() {
        let mut s = UiScript::new().unwrap();
        let row = |name: Option<&str>| MerchantItem {
            name: name.map(str::to_string),
            texture: None,
            price: 10,
            quantity: 1,
            num_available: -1,
            item_id: 25,
            ..Default::default()
        };
        s.set_merchant(Some(MerchantState {
            items: vec![row(Some("Sword")), row(None)],
            buyback: vec![
                MerchantItem {
                    num_available: 0,
                    ..row(Some("Sold Sword"))
                },
                row(None),
            ],
            ..Default::default()
        }));
        let shape = |call: &str| -> String {
            s.eval(&format!(
                "local c = function(...) local o = {{}} for i = 1, arg.n do o[i] = tostring(arg[i]) end \
                 return table.concat(o, ',') end return c({call})"
            ))
            .unwrap()
        };
        let q = "Interface\\Icons\\INV_Misc_QuestionMark";
        assert_eq!(
            shape("GetMerchantItemInfo(1)"),
            format!("Sword,{q},10,1,-1,1")
        );
        assert_eq!(
            shape("GetMerchantItemInfo(2)"),
            format!("nil,{q},10,1,-1,1")
        );
        assert_eq!(
            shape("GetBuybackItemInfo(1)"),
            format!("Sold Sword,{q},10,1,0,1")
        );
        assert_eq!(shape("GetBuybackItemInfo(2)"), "nil,nil,0,1,0,1");
    }

    #[test]
    fn buyback_reads_and_calls() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        assert_eq!(s.eval::<i64>("return GetNumBuybackItems()").unwrap(), 0);
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetBuybackItemInfo(1)\n\
                 return n == nil and t == nil and p == 0 and q == 1 and a == 0 and u == 1",
            )
            .unwrap());

        let mut state = stock();
        state.buyback = vec![MerchantItem {
            name: Some("Worn Dagger".into()),
            texture: Some("Interface\\Icons\\INV_Weapon_ShortBlade_01".into()),
            price: 47,
            quantity: 1,
            stats: Some(ItemStatsHead {
                quality: 1,
                ..Default::default()
            }),
            ..Default::default()
        }];
        s.set_merchant(Some(state));
        assert_eq!(s.eval::<i64>("return GetNumBuybackItems()").unwrap(), 1);
        let (name, _tex, price): (String, String, i64) =
            s.eval("return GetBuybackItemInfo(1)").unwrap();
        assert_eq!((name.as_str(), price), ("Worn Dagger", 47));

        s.run("BuybackItem(1)").unwrap();
        assert_eq!(s.take_merchant_buybacks(), vec![1]);
    }

    /// A host answering `GetRepairAllCost` with a fixed pair, as the app's reads at the call.
    struct RepairHost(Option<(u32, bool)>);

    impl crate::script::CallHost for RepairHost {
        fn apply(
            &mut self,
            _: &mut UiScript,
            _: crate::script::ScriptCall,
        ) -> Option<crate::script::CallTail> {
            None
        }

        fn read_live(&mut self, ask: &mut crate::script::live::LiveAsk<'_>) -> bool {
            let total = self.0;
            ask.answer(|_: &super::RepairAllCostQuery| Some(total))
        }
    }

    /// Away from a vendor that repairs, the total reads 0, as `0x4fbd60` pushes there.
    #[test]
    fn the_repair_all_total_reads_zero_away_from_a_repairer() {
        let mut s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>("return GetRepairAllCost() == 0").unwrap(),
            "no vendor"
        );
        assert_eq!(
            s.arity("GetRepairAllCost()").unwrap(),
            1,
            "no vendor: `0x4fbd76`"
        );
        s.set_merchant(Some(stock()));
        assert!(
            s.eval::<bool>("return GetRepairAllCost() == 0").unwrap(),
            "a vendor that does not repair"
        );
        assert_eq!(
            s.arity("GetRepairAllCost()").unwrap(),
            1,
            "one value, 0, when `0x4fadb0` fails (`0x4fbd76`)"
        );
    }

    /// `RepairAllItems` returns before anything else unless the open merchant repairs
    /// (`0x4fadb0`, tested at `0x4fbfe9`–`0x4fbff0`): no vendor, or one without the repair flag,
    /// queues nothing.
    #[test]
    fn repair_all_queues_nothing_away_from_a_repairer() {
        let mut s = UiScript::new().unwrap();
        s.run("RepairAllItems()").unwrap();
        assert!(!s.take_repair_all(), "no vendor");
        s.set_merchant(Some(stock()));
        s.run("RepairAllItems()").unwrap();
        assert!(!s.take_repair_all(), "a vendor that does not repair");
    }

    #[test]
    fn repair_reads_calls_and_mode_latch() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        assert!(s.eval::<bool>("return CanMerchantRepair() == nil").unwrap());

        let mut state = stock();
        state.can_repair = true;
        s.set_merchant(Some(state));
        assert!(s.eval::<bool>("return CanMerchantRepair() == 1").unwrap());
        s.hosted(&mut RepairHost(Some((1234, true))), |s| {
            assert!(s
                .eval::<bool>("local c, can = GetRepairAllCost()\nreturn c == 1234 and can == 1",)
                .unwrap());
        });
        // No damage: `canRepair` is nil, not false (`0x4fbfce`).
        s.hosted(&mut RepairHost(Some((0, false))), |s| {
            assert!(s
                .eval::<bool>("local c, can = GetRepairAllCost()\nreturn c == 0 and can == nil",)
                .unwrap());
            assert_eq!(s.arity("GetRepairAllCost()").unwrap(), 2);
        });
        // The host's no-repairer answer is the one value 0 (`0x4fbd76`).
        s.hosted(&mut RepairHost(None), |s| {
            assert_eq!(s.arity("GetRepairAllCost()").unwrap(), 1);
        });

        s.run("RepairAllItems()").unwrap();
        assert!(s.take_repair_all());
        assert!(
            s.take_sounds().is_empty(),
            "the stock button's OnClick plays ITEM_REPAIR"
        );

        assert!(s.eval::<bool>("return InRepairMode() == nil").unwrap());
        s.run("ShowRepairCursor()").unwrap();
        assert!(s.repair_mode());
        assert!(s.eval::<bool>("return InRepairMode() == 1").unwrap());
        assert_eq!(s.ui_cursor(), None, "repair clears a stale hover override");
        s.run("HideRepairCursor()").unwrap();
        assert!(!s.repair_mode());
    }

    #[test]
    fn show_repair_cursor_checks_targeting_and_vendor_then_returns_held_item() {
        let mut s = UiScript::new().unwrap();
        let mut bag = ContainerState {
            num_slots: 1,
            ..Default::default()
        };
        bag.slots.insert(
            1,
            ContainerSlot {
                item_id: 117,
                count: 1,
                ..Default::default()
            },
        );
        s.set_container(0, Some(bag));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("ShowRepairCursor()").unwrap();
        assert!(!s.repair_mode(), "no vendor cannot arm repair");
        assert!(s.cursor_item().is_some(), "early return keeps held item");

        s.set_merchant(Some(stock()));
        s.run("ShowRepairCursor()").unwrap();
        assert!(!s.repair_mode(), "non-repair vendor cannot arm repair");
        assert!(s.cursor_item().is_some());

        let mut vendor = stock();
        vendor.can_repair = true;
        s.set_merchant(Some(vendor));
        s.set_spell_targeting(true);
        s.run("ShowRepairCursor()").unwrap();
        assert!(!s.repair_mode(), "targeting spell wins over repair");
        assert!(s.cursor_item().is_some());

        s.set_spell_targeting(false);
        // The clear's `CURSOR_UPDATE` (`0x4fbcd9`) runs before repair mode arms.
        crate::script::tick::heard::listen(&s, &["CURSOR_UPDATE"], Some("InRepairMode()"));
        s.run("ShowRepairCursor()").unwrap();
        assert_eq!(crate::script::tick::heard::take(&s), ["CURSOR_UPDATE nil"]);
        assert!(s.repair_mode());
        assert!(
            s.cursor_item().is_none(),
            "ClearCursor returned the held item"
        );
        assert!(s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return not locked")
            .unwrap());
        assert!(s.take_container_moves().is_empty());
    }

    /// The targeting arm writes the Cast base mode over Repair (`0x6e50b0`) and its end restores
    /// Point (`0x6e49f5`, `0x6e554c`), so arming ends repair mode for good. The idle feed pushes
    /// false every frame, which must leave repair mode standing.
    #[test]
    fn arming_spell_targeting_ends_repair_mode_and_disarming_does_not_restore_it() {
        let mut s = UiScript::new().unwrap();
        let mut vendor = stock();
        vendor.can_repair = true;
        s.set_merchant(Some(vendor));
        s.run("ShowRepairCursor()").unwrap();
        s.set_spell_targeting(false);
        assert!(s.repair_mode(), "the idle feed keeps repair mode");

        s.set_spell_targeting(true);
        assert!(!s.repair_mode(), "the targeting arm ends repair mode");
        assert!(s.eval::<bool>("return InRepairMode() == nil").unwrap());

        s.set_spell_targeting(false);
        assert!(
            !s.repair_mode(),
            "ending the targeting restores Point, not Repair"
        );
        assert!(s.eval::<bool>("return InRepairMode() == nil").unwrap());
    }

    #[test]
    fn clearing_the_merchant_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.set_merchant(None);
        assert_eq!(s.eval::<i64>("return GetMerchantNumItems()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetMerchantItemInfo(1) == nil")
            .unwrap());
    }

    #[test]
    fn vendor_hover_cursor_gates_on_affordability() {
        use crate::script::UiCursorMode;
        let mut s = UiScript::new().unwrap();

        s.run("ShowMerchantSellCursor(1)").unwrap();
        assert_eq!(s.ui_cursor(), None);

        let mut state = stock(); // row 1 price 25, row 2 price 1500
        state.buyback = vec![MerchantItem {
            price: 47,
            ..Default::default()
        }];
        s.set_merchant(Some(state));
        s.set_money(100); // affords row 1 (25) + buyback (47), not row 2 (1500)

        s.run("ShowMerchantSellCursor(1)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
        s.run("ShowMerchantSellCursor(2)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::UnableBuy));

        s.run("ShowInspectCursor()").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Inspect));
        s.run("ResetCursor()").unwrap();
        assert_eq!(s.ui_cursor(), None);

        s.run("ShowBuybackSellCursor(1)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));

        // Out of range leaves the armed cursor as it was.
        s.run("ShowMerchantSellCursor(99)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
    }

    /// The sell-cursor verbs test `IsSpellTargeting` and the base mode before any argument
    /// (`0x4fa469`-`0x4fa47a`, `0x4fbab7`-`0x4fbacc`, `0x4fbbb9`-`0x4fbbce`): under a targeting
    /// spell or the repair cursor a bad argument is quiet and a good one arms nothing.
    #[test]
    fn sell_cursor_verbs_gate_on_targeting_and_repair_before_their_arguments() {
        use crate::script::UiCursorMode;
        let mut s = UiScript::new().unwrap();
        let mut vendor = stock();
        vendor.can_repair = true;
        vendor.buyback = vec![MerchantItem {
            price: 1,
            ..Default::default()
        }];
        s.set_merchant(Some(vendor));
        s.set_money(100);
        let mut bag = ContainerState {
            num_slots: 1,
            ..Default::default()
        };
        bag.slots.insert(
            1,
            ContainerSlot {
                item_id: 117,
                count: 1,
                ..Default::default()
            },
        );
        s.set_container(0, Some(bag));
        let bad = [
            "ShowContainerSellCursor(nil, 1)",
            "ShowContainerSellCursor(0, {})",
            "ShowMerchantSellCursor(nil)",
            "ShowBuybackSellCursor('x')",
        ];
        let good = [
            "ShowContainerSellCursor(0, 1)",
            "ShowMerchantSellCursor(1)",
            "ShowBuybackSellCursor(1)",
        ];
        for arm in ["targeting", "repair"] {
            if arm == "targeting" {
                s.set_spell_targeting(true);
            } else {
                s.set_spell_targeting(false);
                s.run("ShowRepairCursor()").unwrap();
                assert!(s.repair_mode());
            }
            for call in bad.iter().chain(&good) {
                s.run(call)
                    .unwrap_or_else(|e| panic!("{call} under {arm}: {e}"));
                assert_eq!(s.ui_cursor(), None, "{call} under {arm}");
            }
        }
        s.run("HideRepairCursor()").unwrap();
        for call in good {
            s.run("ResetCursor()").unwrap();
            s.run(call).unwrap();
            assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy), "{call}");
        }
        for call in bad {
            assert!(s.run(call).is_err(), "{call} raises past the gates");
        }
    }

    #[test]
    fn pickup_merchant_item_grabs_a_row_as_mode_5() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));

        s.run("PickupMerchantItem(1)").unwrap();
        // Mode 5 is invisible to Lua: every stock `CursorHasItem()` gate stays closed.
        assert!(
            s.eval::<bool>("return not CursorHasItem()").unwrap(),
            "CursorHasItem is nil for mode 5"
        );
        assert!(s.eval::<bool>("return not CursorHasSpell()").unwrap());

        // Yet it is held: a second call toggles it off (`0x4fb818`), a third re-grabs.
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupMerchantItem(1)").unwrap();
        assert!(s.take_merchant_slot_buys().is_empty(), "no drop yet");
    }

    #[test]
    fn pickup_merchant_item_refuses_silently_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));

        for bad in [
            "PickupMerchantItem(0)",
            "PickupMerchantItem(-1)",
            "PickupMerchantItem(99)",
        ] {
            s.run("PickupMerchantItem(1)").unwrap(); // hold something first
            s.run(bad)
                .unwrap_or_else(|e| panic!("{bad} must not raise: {e}"));
            assert!(s.cursor_payload().is_none(), "{bad} cleared the cursor");
        }
        // A missing argument is the same silent clear.
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupMerchantItem()").unwrap();
        // `2^32 + 1` narrows to row 0 on a naive cast.
        s.run("PickupMerchantItem(4294967297)").unwrap();
        assert!(s.cursor_payload().is_none());
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(
            s.take_merchant_slot_buys().is_empty(),
            "the wrapping index grabbed nothing"
        );
        // A numeric string is accepted, and 1.9 truncates to the held row 1, toggling it off.
        s.run(r#"PickupMerchantItem("1")"#).unwrap();
        s.run("PickupMerchantItem(1.9)").unwrap();
        assert!(s.cursor_payload().is_none());
    }

    /// The grab is the preview setter, which needs control for mode 5 (`0x4950f3`): refused, it
    /// clears nothing, a held spell staying. A refusal's clear is the binding's own
    /// (`0x4fb83f`), which runs without control.
    #[test]
    fn pickup_merchant_item_grabs_only_with_control() {
        use crate::script::cursor::{CursorPayload, CursorSpell};
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));
        let spell = CursorPayload::Spell(CursorSpell {
            spell_id: 1,
            texture: None,
            passive: false,
        });
        s.set_cursor_for_test(spell.clone());
        s.set_player_control(false);
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(
            s.cursor_payload(),
            Some(spell),
            "the setter refused before its clear"
        );
        s.run("PickupMerchantItem(99)").unwrap();
        assert!(
            s.cursor_payload().is_none(),
            "the refusal clears all the same"
        );

        s.set_player_control(true);
        s.run("PickupMerchantItem(1)").unwrap();
        assert!(matches!(
            s.cursor_payload(),
            Some(CursorPayload::Merchant(_))
        ));
    }

    /// The toggle-off compares the held preview's entry, any of modes 5, 7 and 9, with the row's
    /// (`0x4fb811`-`0x4fb818`): held ammo or a bar item of row 1's entry drops, and grabs
    /// nothing; another row's grab replaces a held one.
    #[test]
    fn the_toggle_off_matches_any_held_preview_by_entry() {
        use crate::script::cursor::{CursorAction, CursorAmmo, CursorPayload};
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));
        let row = |s: &UiScript| match s.cursor_payload() {
            Some(CursorPayload::Merchant(m)) => Some(m.row),
            _ => None,
        };
        s.run("PickupMerchantItem(2) PickupMerchantItem(1)")
            .unwrap();
        assert_eq!(row(&s), Some(0), "row 1 replaces row 2");

        s.set_cursor_for_test(CursorPayload::Ammo(CursorAmmo {
            item_id: 159,
            texture: None,
        }));
        s.run("PickupMerchantItem(1)").unwrap();
        assert!(s.cursor_payload().is_none(), "the same entry toggles off");

        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0x80,
            action: 159,
            texture: None,
            count: 0,
            consumable: false,
            state: None,
        }));
        s.run("PickupMerchantItem(1)").unwrap();
        assert!(s.cursor_payload().is_none(), "a bar item of the entry too");
    }

    #[test]
    fn pickup_merchant_item_sells_what_the_cursor_holds() {
        use crate::script::cursor::{CursorItem, CursorPayload};
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));
        s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
            item_class: None,
            guid: 0x101,
            bag: 0,
            slot: 7,
            item_id: 4540,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
            bar_placeable: false,
            consumable: false,
        }));

        s.run("PickupMerchantItem(0)").unwrap();
        assert_eq!(
            s.take_merchant_calls(),
            [super::MerchantCall::Sell {
                item_guid: 0x101,
                count: 0
            }],
            "index 0 is ignored — the cursor decides"
        );
        assert!(s.eval::<bool>("return not CursorHasItem()").unwrap());
    }

    #[test]
    fn a_held_vendor_row_dropped_in_a_bag_is_a_buy() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert_eq!(
            s.take_merchant_slot_buys(),
            vec![(0, 3, 159)],
            "the row's item entry, aimed at the dropped-on slot"
        );
        assert!(
            s.cursor_payload().is_none(),
            "the cursor clears on the drop"
        );
    }

    /// Deviation: a stale row drops without buying on all three drop paths, because two of the
    /// reference's three dereference a null here (a fresh `SMSG_LIST_INVENTORY` rewrites the count
    /// without clearing the cursor); we take the third path's behaviour.
    #[test]
    fn a_stale_vendor_row_drops_without_buying() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));
        s.run("PickupMerchantItem(2)").unwrap();
        // The list is replaced while the cursor holds row 2 of the old one.
        let mut shorter = stock();
        shorter.items.truncate(1);
        s.set_merchant(Some(shorter));
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(
            s.take_merchant_slot_buys().is_empty(),
            "no buy goes out for a row that no longer resolves"
        );
        assert!(s.cursor_payload().is_none());
    }

    #[test]
    fn the_action_bar_refuses_a_vendor_row_and_keeps_it_held() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_merchant(Some(stock()));
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PlaceAction(1)").unwrap();
        // Still held: dropping it in a bag afterwards buys.
        s.run("PickupContainerItem(0, 5)").unwrap();
        assert_eq!(s.take_merchant_slot_buys(), vec![(0, 5, 159)]);
    }

    /// Count the cursor's three events from Lua, the way a stock handler would see them.
    fn listen_cursor_events(s: &mut UiScript) {
        s.run(
            r#"
            updates, shows, hides = 0, 0, 0
            local f = CreateFrame("Frame", "MerchantCursorListener")
            f:RegisterEvent("CURSOR_UPDATE")
            f:RegisterEvent("ACTIONBAR_SHOWGRID")
            f:RegisterEvent("ACTIONBAR_HIDEGRID")
            f:SetScript("OnEvent", function()
                if event == "CURSOR_UPDATE" then updates = updates + 1 end
                if event == "ACTIONBAR_SHOWGRID" then shows = shows + 1 end
                if event == "ACTIONBAR_HIDEGRID" then hides = hides + 1 end
            end)
            "#,
        )
        .unwrap();
    }

    /// `(CURSOR_UPDATE, ACTIONBAR_SHOWGRID, ACTIONBAR_HIDEGRID)` delivered so far.
    fn cursor_event_counts(s: &mut UiScript) -> (i64, i64, i64) {
        s.tick(0.01);
        let n = |g: &str| s.eval::<i64>(&format!("return {g}")).unwrap();
        (n("updates"), n("shows"), n("hides"))
    }

    /// The grab goes through the cursor setter (`0x4950f0`): its `ClearCursor(1,1)` (`0x495112`)
    /// signals `CURSOR_UPDATE` from the clear's tail, the set signals it again (`0x495159`), and no
    /// `ACTIONBAR_SHOWGRID` since mode 5 is not mode 7.
    #[test]
    fn a_vendor_grab_fires_cursor_update_but_not_the_bar_grid() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        listen_cursor_events(&mut s);
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(
            cursor_event_counts(&mut s),
            (2, 0, 0),
            "(CURSOR_UPDATE, ACTIONBAR_SHOWGRID, ACTIONBAR_HIDEGRID) after a vendor grab"
        );
        // The toggle-off is a plain clear: one more CURSOR_UPDATE, no HIDEGRID.
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(cursor_event_counts(&mut s), (3, 0, 0));
    }

    #[test]
    fn a_spell_dropped_on_the_vendor_is_a_real_clear() {
        use crate::script::cursor::{self, CursorPayload, CursorSpell};
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        listen_cursor_events(&mut s);
        // The spell setter's set and tail (`0x494dd1`): the grid, no `CURSOR_UPDATE`.
        cursor::put(
            &mut s.model_mut(),
            CursorPayload::Spell(CursorSpell {
                spell_id: 133,
                texture: None,
                passive: false,
            }),
        );
        assert_eq!(cursor_event_counts(&mut s), (0, 1, 0), "the spell pickup");

        s.run("PickupMerchantItem(0)").unwrap();
        assert!(s.cursor_payload().is_none());
        assert_eq!(
            cursor_event_counts(&mut s),
            (1, 1, 1),
            "(CURSOR_UPDATE, ACTIONBAR_SHOWGRID, ACTIONBAR_HIDEGRID) after the vendor drop"
        );
    }

    /// `ClearCursor 0x495190` opens with the gift-wrap cancel (`0x5edf10`), and the grab opens
    /// with that clear (`0x49510b`).
    #[test]
    fn a_vendor_grab_cancels_an_armed_gift_wrap() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.arm_gift_wrap(0, 1);
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(
            s.gift_wrap_armed(),
            None,
            "the grab's ClearCursor cancels the wrap"
        );
    }
}
