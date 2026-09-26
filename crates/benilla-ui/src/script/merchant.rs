//! The merchant bindings: the app pushes the open vendor ([`UiScript::set_merchant`]), and the
//! Lua verbs queue intents it drains. A row is 1-based; the app maps it to the item entry
//! `CMSG_BUY_ITEM` needs.
//!
//! `GetMerchantItemInfo(index)` answers `name, texture, price, quantity, numAvailable, isUsable`
//! (`0x4fb150`), or `(nil, nil, 0, 1, 0, 1)` for an invalid index (`0x4fb2be`). Unlimited stock is
//! `numAvailable` -1; `isUsable` is 1 or nil, the [`super::item_stats::item_usable`] gate, and a
//! template still in flight reads usable (`0x4fb298`).

use mlua::{Lua, MultiValue, Value};

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
    /// `GetMerchantItemMaxStack`: the template's `stackable`, 1 if it does not stack; `None` while
    /// in flight.
    pub max_stack: Option<u32>,
}

/// An item template's tooltip stat head, resolved per row by the app. The reference's
/// `GameTooltip:SetMerchantItem` reads these in C++; no 1.12 Lua API carries them.
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

impl super::UiScript {
    /// Push (or clear, with `None`) the open vendor. Clearing it leaves repair mode, as the
    /// merchant-close handler restores the Point base mode (`0x4fadf0`).
    pub fn set_merchant(&mut self, state: Option<MerchantState>) {
        let closing = state.is_none();
        self.model_mut().merchant = state;
        if closing {
            self.model_mut().repair_mode = false;
        }
    }

    /// Push `GetRepairAllCost`'s total in copper, swept by the app ahead of the events that read it.
    pub fn set_repair_all_cost(&mut self, copper: u32) {
        self.model_mut().repair_all_cost = copper;
    }

    /// Drain the `(row, quantity)` buys `BuyMerchantItem` queued, the row 1-based.
    pub fn take_merchant_buys(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().merchant_buys)
    }

    /// Drain the `(bag, slot)` of each cursor item dropped on the vendor; the app sells it.
    pub fn take_merchant_cursor_sells(&mut self) -> Vec<(i64, u32)> {
        std::mem::take(&mut self.model_mut().merchant_cursor_sells)
    }

    /// Drain the `(bag, slot, entry)` of held rows dropped into bags (`CMSG_BUY_ITEM_IN_SLOT`).
    pub fn take_merchant_slot_buys(&mut self) -> Vec<(i64, u32, u32)> {
        std::mem::take(&mut self.model_mut().merchant_slot_buys)
    }

    /// Whether `CloseMerchant` was called since the last drain; the close sends no packet.
    pub fn take_merchant_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().merchant_close)
    }

    /// Drain the 1-based buyback slots `BuybackItem` queued.
    pub fn take_merchant_buybacks(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().merchant_buybacks)
    }

    /// Whether `RepairAllItems` was called since the last drain.
    pub fn take_repair_all(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().repair_all)
    }

    /// The repair-mode latch: while set, a bag or equipment click repairs the item.
    pub fn repair_mode(&self) -> bool {
        self.model_ref().repair_mode
    }
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

/// Register the merchant globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetMerchantNumItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.merchant.as_ref().map_or(0, |m| m.items.len()) as i64)
        })?,
    )?;

    g.set(
        "GetMerchantItemInfo",
        lua.create_function(|lua, index: usize| {
            let (item, usable) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let item = model
                    .merchant
                    .as_ref()
                    .and_then(|m| index.checked_sub(1).and_then(|n| m.items.get(n)))
                    .cloned();
                let usable = item
                    .as_ref()
                    .is_none_or(|it| super::item_stats::item_usable_by_id(&model, it.item_id));
                (item, usable)
            };
            let Some(it) = item else {
                return Ok(MultiValue::from_vec(invalid_tuple()));
            };
            let name = match &it.name {
                Some(n) => Value::String(lua.create_string(n)?),
                None => Value::Nil,
            };
            let texture = match &it.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
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

    // GetMerchantItemLink(index): nil out of range and while in flight; both stock row-click
    // arms accept a nil (`MerchantFrame.lua:303`, `:306`).
    g.set(
        "GetMerchantItemLink",
        lua.create_function(|lua, index: usize| {
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .merchant
                    .as_ref()
                    .and_then(|m| index.checked_sub(1).and_then(|n| m.items.get(n)))
                    .and_then(|it| it.link.clone())
            };
            match link {
                Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetMerchantItemMaxStack(index): nil while in flight or out of range. A right shift-click, or
    // a left one with the chat box closed, asks it and opens the stack-split spinner only above 1
    // (`MerchantFrame.lua:313`, `:340`).
    g.set(
        "GetMerchantItemMaxStack",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model
                .merchant
                .as_ref()
                .and_then(|m| index.checked_sub(1).and_then(|n| m.items.get(n)))
                .and_then(|it| it.max_stack)
                .map_or(Value::Nil, |n| Value::Integer(i64::from(n))))
        })?,
    )?;

    // BenillaGetMerchantItemStats(index) → quality, invType, class, subclass, dmgMin, dmgMax,
    // dmgType, delayMs, armor, block, or nil. Not a 1.12 verb: the reference's tooltip reads the
    // template in C++ (`SetMerchantItem 0x534080`).
    g.set(
        "BenillaGetMerchantItemStats",
        lua.create_function(|lua, index: usize| {
            let stats = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .merchant
                    .as_ref()
                    .and_then(|m| index.checked_sub(1).and_then(|n| m.items.get(n)))
                    .and_then(|it| it.stats)
            };
            let Some(s) = stats else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(s.quality)),
                Value::Integer(i64::from(s.inventory_type)),
                Value::Integer(i64::from(s.class)),
                Value::Integer(i64::from(s.subclass)),
                Value::Number(f64::from(s.dmg_min)),
                Value::Number(f64::from(s.dmg_max)),
                Value::Integer(i64::from(s.dmg_type)),
                Value::Integer(i64::from(s.delay_ms)),
                Value::Integer(i64::from(s.armor)),
                Value::Integer(i64::from(s.block)),
            ]))
        })?,
    )?;

    g.set(
        "BuyMerchantItem",
        lua.create_function(|lua, (index, quantity): (u32, Option<u32>)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.merchant_buys.push((index, quantity.unwrap_or(1)));
            Ok(())
        })?,
    )?;

    // PickupMerchantItem(index): two verbs in one (`0x4fb760`), forked at `0x4fb787` on
    // `GetCursorItem 0x494c60`, which answers for mode 1 only; stock never checks
    // `CursorHasItem()` first (`MerchantFrame.lua:329`). A bag item on the cursor is sold, index
    // ignored and no merchant-open gate, which stock `MerchantFrame.xml:743`'s
    // `PickupMerchantItem(0)` relies on; otherwise row `index - 1` is grabbed as cursor mode 5.
    //
    // Every refusal is silent and clears the cursor: a non-number (`0x4fb7cb`; a numeric string
    // passes), `index < 1` (`0x4fb7e1`), `index > count` (`0x4fb7e9`), no merchant (`0x4fb7f7`), a
    // dead row (`0x4fb80b`), or the held row again (`0x4fb818`, the toggle-off). Neither stock nor
    // the item cache is tested: a sold-out row grabs, and the server refuses the buy.
    g.set(
        "PickupMerchantItem",
        lua.create_function(|lua, index: Option<f64>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");

            // The sell fork comes first, before the argument is read. Its clear is the sale's, not
            // `ClearCursor`: `CURSOR_UPDATE` fires and the source slot stays greyed until the
            // server's inventory update (`0x494b60`).
            if let Some(item) = crate::script::cursor::take_cursor_item_for_sale(&mut model) {
                model.merchant_cursor_sells.push((item.bag, item.slot));
                return Ok(());
            }

            // Every other leg clears with the real `ClearCursor(1,1)` (`0x4fb82d`/`0x4fb83f` on a
            // refusal, `0x49510b` in the grab), not a bare drop: a held spell fires `CURSOR_UPDATE`
            // and hides the bar grid, and an armed gift wrap is cancelled (`0x5edf10`).
            let held = match &model.cursor {
                Some(CursorPayload::Merchant(m)) => Some(m.row),
                _ => None,
            };
            crate::script::cursor::clear_cursor(&mut model);

            let Some(index) = index else { return Ok(()) };
            // `_ftol` truncates toward zero, then the bound applies signed. The upper bound is
            // load-bearing: `2^32 + 1` would narrow to row 0, where the reference bounds against
            // the vendor count before any narrowing.
            let row = index.trunc();
            if row.is_nan() || row < 1.0 || row > f64::from(u32::MAX) {
                return Ok(());
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let row0 = row as u32 - 1;
            let Some(merchant) = model.merchant.as_ref() else {
                return Ok(());
            };
            let Some(item) = merchant.items.get(row0 as usize) else {
                return Ok(());
            };
            // The toggle-off: naming the held row puts nothing back.
            if held == Some(row0) {
                return Ok(());
            }
            let payload = CursorPayload::Merchant(super::cursor::CursorMerchantItem {
                item_id: item.item_id,
                row: row0,
                texture: item.texture.clone(),
            });
            // The grab's `SignalEvent(CURSOR_UPDATE)` (`0x495159`); mode 5 skips the mode-7
            // `ACTIONBAR_SHOWGRID` branch.
            model.cursor = Some(payload);
            crate::script::cursor::queue_cursor_update(&mut model);
            Ok(())
        })?,
    )?;

    g.set(
        "CloseMerchant",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.merchant_close = true;
            Ok(())
        })?,
    )?;

    g.set(
        "GetNumBuybackItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.merchant.as_ref().map_or(0, |m| m.buyback.len()) as i64)
        })?,
    )?;

    // GetBuybackItemInfo(index): the same six-tuple (`0x4fb310`), oldest first. isUsable is the
    // same `0x5ea930` gate over the sold item (`0x4fb4f7`): one its seller cannot use reads red.
    g.set(
        "GetBuybackItemInfo",
        lua.create_function(|lua, index: usize| {
            let (item, usable) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let item = model
                    .merchant
                    .as_ref()
                    .and_then(|m| index.checked_sub(1).and_then(|n| m.buyback.get(n)))
                    .cloned();
                let usable = item
                    .as_ref()
                    .is_none_or(|it| super::item_stats::item_usable_by_id(&model, it.item_id));
                (item, usable)
            };
            let Some(it) = item else {
                return Ok(MultiValue::from_vec(invalid_tuple()));
            };
            let name = match &it.name {
                Some(n) => Value::String(lua.create_string(n)?),
                None => Value::Nil,
            };
            let texture = match &it.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
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

    // BenillaGetBuybackItemStats(index): the buyback hover's stat head, as above.
    g.set(
        "BenillaGetBuybackItemStats",
        lua.create_function(|lua, index: usize| {
            let stats = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .merchant
                    .as_ref()
                    .and_then(|m| index.checked_sub(1).and_then(|n| m.buyback.get(n)))
                    .and_then(|it| it.stats)
            };
            let Some(s) = stats else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(s.quality)),
                Value::Integer(i64::from(s.inventory_type)),
                Value::Integer(i64::from(s.class)),
                Value::Integer(i64::from(s.subclass)),
                Value::Number(f64::from(s.dmg_min)),
                Value::Number(f64::from(s.dmg_max)),
                Value::Integer(i64::from(s.dmg_type)),
                Value::Integer(i64::from(s.delay_ms)),
                Value::Integer(i64::from(s.armor)),
                Value::Integer(i64::from(s.block)),
            ]))
        })?,
    )?;

    g.set(
        "BuybackItem",
        lua.create_function(|lua, index: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.merchant_buybacks.push(index);
            Ok(())
        })?,
    )?;

    g.set(
        "CanMerchantRepair",
        lua.create_function(|lua, ()| {
            let can = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.merchant.as_ref().is_some_and(|m| m.can_repair)
            };
            Ok(if can { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // GetRepairAllCost() → cost, canRepair: whether there is damage to pay for, which enables the
    // repair-all button (`MerchantFrame.lua:38`); 0 away from a vendor that repairs (`0x4fbd60`).
    g.set(
        "GetRepairAllCost",
        lua.create_function(|lua, ()| {
            let cost = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                if model.merchant.as_ref().is_some_and(|m| m.can_repair) {
                    model.repair_all_cost
                } else {
                    0
                }
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(cost)),
                Value::Boolean(cost > 0),
            ]))
        })?,
    )?;

    g.set(
        "RepairAllItems",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.repair_all = true;
            Ok(())
        })?,
    )?;

    g.set(
        "ShowRepairCursor",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `0x4fbcc0`: a targeting spell or a vendor without repair service leaves the
            // cursor untouched. `ClearCursor(1,1)` returns any held item before mode 0x11 arms.
            if model.spell_targeting || !model.merchant.as_ref().is_some_and(|m| m.can_repair) {
                return Ok(());
            }
            crate::script::cursor::clear_cursor(&mut model);
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
    // in-flight item lock and base-mode Point gates live app-side.
    g.set(
        "ShowMerchantSellCursor",
        lua.create_function(|lua, index: usize| {
            arm_vendor_cursor(lua, |m| {
                m.items.get(index.wrapping_sub(1)).map(|it| it.price)
            });
            Ok(())
        })?,
    )?;
    g.set(
        "ShowBuybackSellCursor",
        lua.create_function(|lua, index: usize| {
            arm_vendor_cursor(lua, |m| {
                m.buyback.get(index.wrapping_sub(1)).map(|it| it.price)
            });
            Ok(())
        })?,
    )?;

    Ok(())
}

/// Arm the vendor hover cursor from a row's price: Buy(3) if the player can afford it, else the
/// greyed UnableBuy(23); no price leaves the cursor untouched.
fn arm_vendor_cursor(lua: &Lua, price_of: impl FnOnce(&MerchantState) -> Option<u32>) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    // The family bails on `IsTargeting` first (`0x6e48a0`): an armed spell keeps its cursor.
    if model.spell_targeting {
        return;
    }
    let Some(price) = model.merchant.as_ref().and_then(price_of) else {
        return;
    };
    model.ui_cursor = Some(if model.money >= u64::from(price) {
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
                    quantity: 1,
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
        assert_eq!((price, quantity, num), (25, 1, -1));
        assert_eq!(usable, 1);

        // In flight: name and texture nil, and usable (the null-record skip).
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetMerchantItemInfo(2)\n\
                 return n == nil and t == nil and p == 1500 and u == 1",
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
        assert!(s
            .eval::<bool>("return GetMerchantItemLink(2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetMerchantItemLink(9) == nil")
            .unwrap());

        assert_eq!(
            s.eval::<i64>("return GetMerchantItemMaxStack(1)").unwrap(),
            20
        );
        assert!(s
            .eval::<bool>("return GetMerchantItemMaxStack(2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetMerchantItemMaxStack(9) == nil")
            .unwrap());
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
    fn merchant_stats_feed_reads_the_tooltip_head() {
        let mut s = UiScript::new().unwrap();
        let mut stock = stock();
        // A sword, so every stat column is distinct.
        stock.items[0].stats = Some(ItemStatsHead {
            quality: 2,
            inventory_type: 21,
            class: 2,
            subclass: 7,
            dmg_min: 5.0,
            dmg_max: 9.0,
            dmg_type: 2,
            delay_ms: 2600,
            armor: 0,
            block: 0,
            sell_price: 0,
        });
        s.set_merchant(Some(stock));
        let (quality, inv, class, sub, dmin, dmax, dtype, delay, armor, block) = s
            .eval::<(i64, i64, i64, i64, f64, f64, i64, i64, i64, i64)>(
                "return BenillaGetMerchantItemStats(1)",
            )
            .unwrap();
        assert_eq!((quality, inv, class, sub), (2, 21, 2, 7));
        assert_eq!((dmin, dmax, dtype, delay), (5.0, 9.0, 2, 2600));
        assert_eq!((armor, block), (0, 0));
        assert!(s
            .eval::<bool>("return BenillaGetMerchantItemStats(2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return BenillaGetMerchantItemStats(9) == nil")
            .unwrap());
    }

    #[test]
    fn buy_merchant_item_queues_intents() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.run("BuyMerchantItem(1)").unwrap(); // default quantity 1
        s.run("BuyMerchantItem(2, 5)").unwrap();
        assert_eq!(s.take_merchant_buys(), vec![(1, 1), (2, 5)]);
        assert!(s.take_merchant_buys().is_empty(), "drained");
    }

    #[test]
    fn close_merchant_flags_the_intent() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        assert!(!s.take_merchant_close());
        s.run("CloseMerchant()").unwrap();
        assert!(s.take_merchant_close());
        assert!(!s.take_merchant_close(), "drained");
    }

    #[test]
    fn buyback_reads_and_intents() {
        let mut s = UiScript::new().unwrap();
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
        assert!(s
            .eval::<bool>("return BenillaGetBuybackItemStats(1) ~= nil")
            .unwrap());

        s.run("BuybackItem(1)").unwrap();
        assert_eq!(s.take_merchant_buybacks(), vec![1]);
        assert!(s.take_merchant_buybacks().is_empty(), "drained");
    }

    /// Away from a vendor that repairs, the total reads 0, as `0x4fbd60` pushes there.
    #[test]
    fn the_repair_all_total_reads_zero_away_from_a_repairer() {
        let mut s = UiScript::new().unwrap();
        s.set_repair_all_cost(1234);
        assert!(
            s.eval::<bool>("return GetRepairAllCost() == 0").unwrap(),
            "no vendor"
        );
        s.set_merchant(Some(stock()));
        assert!(
            s.eval::<bool>("return GetRepairAllCost() == 0").unwrap(),
            "a vendor that does not repair"
        );
    }

    #[test]
    fn repair_reads_intents_and_mode_latch() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return CanMerchantRepair() == nil").unwrap());

        let mut state = stock();
        state.can_repair = true;
        s.set_merchant(Some(state));
        s.set_repair_all_cost(1234);
        assert!(s.eval::<bool>("return CanMerchantRepair() == 1").unwrap());
        assert!(s
            .eval::<bool>("local c, can = GetRepairAllCost()\nreturn c == 1234 and can == true",)
            .unwrap());

        s.run("RepairAllItems()").unwrap();
        assert!(s.take_repair_all());
        assert!(!s.take_repair_all(), "drained");
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

        s.run("ShowRepairCursor()").unwrap();
        s.set_merchant(None);
        assert!(
            !s.repair_mode(),
            "closing the merchant hides the repair cursor"
        );
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
        s.run("ShowRepairCursor()").unwrap();
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
    #[test]
    fn pickup_merchant_item_grabs_a_row_as_mode_5() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));

        s.run("PickupMerchantItem(1)").unwrap();
        // Mode 5 is invisible to Lua: every stock `CursorHasItem()` gate stays closed.
        assert!(
            s.eval::<bool>("return not CursorHasItem()").unwrap(),
            "CursorHasItem is nil for mode 5"
        );
        assert!(s.eval::<bool>("return not CursorHasSpell()").unwrap());
        assert!(
            s.eval::<bool>("return GetCursorInfo() == nil").unwrap(),
            "GetCursorInfo reports nothing — no binding exposes mode 5"
        );

        // Yet it is held: a second call toggles it off (`0x4fb818`), a third re-grabs.
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupMerchantItem(1)").unwrap();
        assert!(s.take_merchant_slot_buys().is_empty(), "no drop yet");
    }

    #[test]
    fn pickup_merchant_item_refuses_silently_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));

        for bad in [
            "PickupMerchantItem(0)",
            "PickupMerchantItem(-1)",
            "PickupMerchantItem(99)",
        ] {
            s.run("PickupMerchantItem(1)").unwrap(); // hold something first
            s.run(bad)
                .unwrap_or_else(|e| panic!("{bad} must not raise: {e}"));
            assert!(
                s.eval::<bool>("return GetCursorInfo() == nil").unwrap(),
                "{bad} cleared the cursor"
            );
        }
        // A missing argument is the same silent clear.
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupMerchantItem()").unwrap();
        // `2^32 + 1` narrows to row 0 on a naive cast.
        s.run("PickupMerchantItem(4294967297)").unwrap();
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(
            s.take_merchant_slot_buys().is_empty(),
            "the wrapping index grabbed nothing"
        );
        // A numeric string is accepted, and 1.9 truncates to the held row 1, toggling it off.
        s.run(r#"PickupMerchantItem("1")"#).unwrap();
        s.run("PickupMerchantItem(1.9)").unwrap();
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
    }

    #[test]
    fn pickup_merchant_item_sells_what_the_cursor_holds() {
        use crate::script::cursor::{CursorItem, CursorPayload};
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
            bag: 0,
            slot: 7,
            item_id: 4540,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
            bar_placeable: false,
        }));

        s.run("PickupMerchantItem(0)").unwrap();
        assert_eq!(
            s.take_merchant_cursor_sells(),
            vec![(0, 7)],
            "index 0 is ignored — the cursor decides"
        );
        assert!(s.eval::<bool>("return not CursorHasItem()").unwrap());
        assert!(s.take_merchant_cursor_sells().is_empty(), "drained");
    }

    #[test]
    fn a_held_vendor_row_dropped_in_a_bag_is_a_buy() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert_eq!(
            s.take_merchant_slot_buys(),
            vec![(0, 3, 159)],
            "the row's item entry, aimed at the dropped-on slot"
        );
        assert!(
            s.eval::<bool>("return GetCursorInfo() == nil").unwrap(),
            "the cursor clears on the drop"
        );
    }

    /// Deviation: a stale row drops without buying on all three drop paths, because two of the
    /// reference's three dereference a null here (a fresh `SMSG_LIST_INVENTORY` rewrites the count
    /// without clearing the cursor); we take the third path's behaviour.
    #[test]
    fn a_stale_vendor_row_drops_without_buying() {
        let mut s = UiScript::new().unwrap();
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
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
    }

    #[test]
    fn the_action_bar_refuses_a_vendor_row_and_keeps_it_held() {
        let mut s = UiScript::new().unwrap();
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

    /// The grab goes through the cursor setter (`0x4950f0`): `CURSOR_UPDATE` at `0x495159`, and
    /// no `ACTIONBAR_SHOWGRID` since mode 5 is not mode 7.
    #[test]
    fn a_vendor_grab_fires_cursor_update_but_not_the_bar_grid() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        listen_cursor_events(&mut s);
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(
            cursor_event_counts(&mut s),
            (1, 0, 0),
            "(CURSOR_UPDATE, ACTIONBAR_SHOWGRID, ACTIONBAR_HIDEGRID) after a vendor grab"
        );
        // The toggle-off is a plain clear: one more CURSOR_UPDATE, no HIDEGRID.
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(cursor_event_counts(&mut s), (2, 0, 0));
    }

    #[test]
    fn a_spell_dropped_on_the_vendor_is_a_real_clear() {
        use crate::script::cursor::{self, CursorPayload, CursorSpell};
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        listen_cursor_events(&mut s);
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
            passive: false,
        }));
        cursor::queue_cursor_update(&mut s.model_mut()); // the pickup's own transition
        assert_eq!(cursor_event_counts(&mut s), (1, 1, 0), "the spell pickup");

        s.run("PickupMerchantItem(0)").unwrap();
        assert!(s.cursor_payload().is_none());
        assert_eq!(
            cursor_event_counts(&mut s),
            (2, 1, 1),
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
