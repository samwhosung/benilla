//! The merchant bindings (decision 0081 phase 4) — the Era-shaped vendor surface, the same two-way
//! seam as [`super::container`]/[`super::gossip`]: the app pushes a **merchant snapshot**
//! ([`UiScript::set_merchant`] — the vendor rows already resolved from the wire to name/icon/price
//! by the app's item stores), and the Lua `BuyMerchantItem`/`CloseMerchant` calls queue outbound
//! **intents** the app drains ([`UiScript::take_merchant_buys`] / [`UiScript::take_merchant_close`]).
//! The engine holds no vendor knowledge — a row is "a name, an icon path, a price, a stack size,
//! how many are left, and the tooltip's stat head" ([`ItemStatsHead`], read by
//! `BenillaGetMerchantItemStats`).
//!
//! ## The 5875 API shape
//!
//! 1.12's `GetMerchantItemInfo(index)` returns a flat **6-value** tuple the FrameXML reads
//! positionally — `name, texture, price, quantity, numAvailable, isUsable` (byte-verified,
//! `0x4fb150`; the ref's own `MerchantFrame_UpdateMerchantInfo` destructures exactly these six.
//! The `isPurchasable`/`extendedCost` extras an earlier draft carried are TBC-era, not 5875).
//! `index` is **1-based**; an invalid index answers the binding's fixed tuple
//! `(nil, nil, 0, 1, 0, 1)` — same shape as `GetBuybackItemInfo`'s (`0x4fb2be`). `numAvailable`
//! is `-1` for unlimited stock (the wire's `0xFFFF_FFFF` through `fild`, mapped by the app).
//! `isUsable` is `1`/`nil` (the client pushes a number or nil, never a boolean): the
//! [`super::item_stats::item_usable`] gate over the row's template — a template still in flight
//! reads usable, the getter's null-record skip (`0x4fb298`).
//!
//! Buying addresses the row by its **1-based list position** here (`BuyMerchantItem(index)`); the app
//! maps that position to the item *entry* the wire's `CMSG_BUY_ITEM` needs (buy is by entry, not the
//! vendor `muid` — decision 0081). vanilla's client-side `CloseMerchant()` sends no packet, so it
//! just flags the app to clear its local state (the gossip pattern).

use mlua::{Lua, MultiValue, Value};

use super::container::UiCursorMode;
use super::cursor::CursorPayload;
use super::Model;

/// One vendor row, resolved by the app from the wire `VendorItem` (decision 0081). Plain data —
/// 1-based order in the window is its position in [`MerchantState::items`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MerchantItem {
    /// The item name (`GetMerchantItemInfo`'s first return); `None` while the ask-once item-template
    /// query is still in flight (the API reports `nil`, the XML shows a placeholder).
    pub name: Option<String>,
    /// Icon texture path (`Interface\Icons\…`); `None` while the template answer is in flight.
    pub texture: Option<String>,
    /// Buy price in copper (already reputation-discounted server-side).
    pub price: u32,
    /// Stack size delivered per purchase (`item_template.buy_count`, the wire's `buy_count`).
    pub quantity: u32,
    /// Remaining stock, or `-1` for unlimited (the app maps the wire's `0xFFFF_FFFF`).
    pub num_available: i32,
    /// The item's template **entry** — what `CMSG_BUY_ITEM` addresses (the app maps the clicked
    /// 1-based row to this; the Lua side never sees it).
    pub item_id: u32,
    /// The stat head the hover tooltip renders, from the same ask-once template answer as `name`
    /// (`None` while it's in flight — the tooltip shows nothing it can't know yet). Still plain
    /// data: the engine renders whatever numbers the app resolved, it holds no item model.
    pub stats: Option<ItemStatsHead>,
    /// The row's full escaped `|cff…|Hitem:…|h[Name]|h|r` link (`GetMerchantItemLink`, decision
    /// 1059) — what the row click's ctrl/shift arms hand to `DressUpItemLink` /
    /// `ChatFrameEditBox:Insert` (`MerchantFrame.lua:303`/`:306`). `None` while the template answer
    /// is in flight (the link embeds the name), and `None` on a **buyback** row: 1.12 has no
    /// `GetBuybackItemLink` — the reference's buyback arm is an unmodified `BuybackItem(this:GetID())`
    /// with no ctrl/shift branch at all (`MerchantFrame.lua:358-361`), so nothing would read it.
    pub link: Option<String>,
    /// `Stackable` from the item's own template — what one slot can hold, `1` for an item that
    /// does not stack. `None` while the ask-once template query is in flight, like `name`.
    ///
    /// This is `GetMerchantItemMaxStack`'s answer and it exists for one caller: the reference's
    /// `MerchantItemButton_OnClick` asks it before a shift-click, and opens the stack-split
    /// spinner only when it is above 1 (`MerchantFrame.lua:313-326`, and again at :340 for the
    /// buy-in-bulk arm). The wire has carried it all along — `item_template.stackable`, parsed and
    /// then dropped, because nothing on the Lua side could ask.
    pub max_stack: Option<u32>,
}

/// An item template's tooltip stat head (the app's `ItemInfo` view, resolved per row): what the
/// vendor hover tooltip needs beyond name/icon/price. The real client's `GameTooltip:SetMerchantItem`
/// reads the same template fields C++-side; benilla feeds them to Lua through
/// `BenillaGetMerchantItemStats(index)` (no 1.12 Lua API carries damage/armor — tooltip content was
/// C++'s alone — so the feed is benilla-named, not Era-shaped).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ItemStatsHead {
    /// 0 poor … 6 artifact — colours the tooltip's name line.
    pub quality: u32,
    /// `InventoryType` — the tooltip's slot line ("Main Hand", "Chest", …); 0 = no slot line.
    pub inventory_type: u32,
    /// Item class (2 weapon, 4 armor, 6 projectile) — with `subclass`, the slot line's right column.
    pub class: u32,
    /// Item subclass within `class` (7 = sword, 1 = cloth, …).
    pub subclass: u32,
    /// Damage block 0 per-hit minimum (0 for non-weapons).
    pub dmg_min: f32,
    /// Damage block 0 per-hit maximum.
    pub dmg_max: f32,
    /// Damage block 0 school (0 physical, 1 Holy … 6 Arcane).
    pub dmg_type: u32,
    /// Attack delay in milliseconds ("Speed" = delay / 1000).
    pub delay_ms: u32,
    /// The armor line's value; 0 = no line.
    pub armor: u32,
    /// A shield's block line; 0 = no line.
    pub block: u32,
    /// `SellPrice` — what a vendor pays per unit (the bag tooltip's money row while a merchant is
    /// open; 0 = the "No sell price" line).
    pub sell_price: u32,
}

/// One open merchant window: the vendor's rows, the buyback slots, and the repair head. Pushed
/// whole by the app; `None` means no vendor is open (the window is closed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MerchantState {
    pub items: Vec<MerchantItem>,
    /// The buyback slots, oldest-first (index 1 = the oldest; the LAST entry is the most recent
    /// sale — the one the merchant page's single buyback slot shows, ref
    /// `GetBuybackItemInfo(GetNumBuybackItems())`). Resolved by the app from the player's
    /// VENDORBUYBACK descriptor fields + the item stores; `price` here is the buyback price
    /// field, `quantity` the stored item's stack count.
    pub buyback: Vec<MerchantItem>,
    /// Whether this vendor repairs (UNIT_NPC_FLAGS repair bit) — shows the repair buttons.
    pub can_repair: bool,
    /// The repair-all cost in copper the app computed (0 = nothing to repair → the repair-all
    /// button disables, ref MerchantFrame_OnShow).
    pub repair_all_cost: u32,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open merchant's stock snapshot.
    pub fn set_merchant(&mut self, state: Option<MerchantState>) {
        self.model_mut().merchant = state;
    }

    /// Drain the `(index, quantity)` buy intents queued by `BuyMerchantItem` since the last call.
    /// `index` is the 1-based row position; the app maps it to the item entry the wire needs.
    pub fn take_merchant_buys(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().merchant_buys)
    }

    /// Drain the `(bag, slot)` sells `PickupMerchantItem`'s cursor arm queued — the item the
    /// cursor was holding when it was dropped on the vendor window. The app resolves the concrete
    /// item guid from the pair and sends `CMSG_SELL_ITEM`, the same resolution the bag-click sell
    /// route already does.
    pub fn take_merchant_cursor_sells(&mut self) -> Vec<(i64, u32)> {
        std::mem::take(&mut self.model_mut().merchant_cursor_sells)
    }

    /// Drain the `(bag, slot, item entry)` buys a held vendor row queued by being dropped into a
    /// container slot — `CMSG_BUY_ITEM_IN_SLOT 0x1a3`.
    pub fn take_merchant_slot_buys(&mut self) -> Vec<(i64, u32, u32)> {
        std::mem::take(&mut self.model_mut().merchant_slot_buys)
    }

    /// Whether `CloseMerchant` was called since the last drain (and clear the flag). vanilla's
    /// client-side close sends no packet — the app just clears its local merchant state.
    pub fn take_merchant_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().merchant_close)
    }

    /// Drain the 1-based buyback-slot intents queued by `BuybackItem` since the last call. The app
    /// maps each to the wire's absolute buyback inventory slot.
    pub fn take_merchant_buybacks(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().merchant_buybacks)
    }

    /// Whether `RepairAllItems` was called since the last drain (and clear the flag). The app
    /// sends the repair-all wire message.
    pub fn take_repair_all(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().repair_all)
    }

    /// The client-side repair-mode latch (`ShowRepairCursor`/`HideRepairCursor`/`InRepairMode`):
    /// while set, a bag/equipment click means "repair this item" — the app reads this to route
    /// clicks and swap the hardware cursor.
    pub fn repair_mode(&self) -> bool {
        self.model_ref().repair_mode
    }
}

/// `1`/`nil` — how the client pushes a usable flag (`pushnumber(1.0)` / `pushnil`).
fn usable_value(usable: bool) -> Value {
    if usable {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// The invalid-index tuple both getters answer (`0x4fb2be` / the buyback equivalent):
/// `(nil, nil, 0, 1, 0, 1)` — still six values.
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

    // → the number of rows the open vendor has (0 when no vendor is open).
    g.set(
        "GetMerchantNumItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.merchant.as_ref().map_or(0, |m| m.items.len()) as i64)
        })?,
    )?;

    // GetMerchantItemInfo(index) → name, texture, price, quantity, numAvailable, isUsable — the
    // byte-verified 5875 6-tuple (0x4fb150; see the module doc). `index` is 1-based; an invalid
    // index answers the fixed tuple (nil, nil, 0, 1, 0, 1).
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

    // GetMerchantItemLink(index) → the row's full escaped `|cff…|Hitem:…|h[Name]|h|r` link | nil.
    // 1-based like GetMerchantItemInfo; nil out of range and nil while the row's template answer is
    // in flight (the link embeds the name). The reference's row click reads it for both LEFT-button
    // modifier arms — `DressUpItemLink(GetMerchantItemLink(this:GetID()))` (`MerchantFrame.lua:303`)
    // and `ChatFrameEditBox:Insert(...)` (`:306`); ours routes the second through
    // `BenillaChatEdit_InsertLink`, whose whole job is the nil this getter can answer. Merchant rows
    // only: see [`MerchantItem::link`] for why a buyback row carries none. Decision 1059.
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

    // GetMerchantItemMaxStack(index) → the stack size ONE slot can hold for that row's item, or
    // nil while its template answer is in flight / the index is out of range.
    //
    // Its only caller in 1.12 is the reference's own `MerchantItemButton_OnClick`, which asks it
    // twice — once on the right-click arm (`MerchantFrame.lua:313`) and once on shift-click
    // (`:340`) — and takes the plain single-buy path whenever the answer is `<= 1`, opening the
    // stack-split spinner otherwise. So the number decides whether a vendor click asks "how many?"
    // at all.
    //
    // The value was on the wire and parsed the whole time (`item_template.stackable`); nothing on
    // the Lua side could reach it until now, which is why the reference's merchant window listed
    // this as one of its two missing verbs.
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
    // dmgType, delayMs, armor, block — the tooltip stat head ([`ItemStatsHead`]), or nil while
    // the row's template answer is in flight / the index is out of range. Benilla-named: 1.12's Lua
    // API never carried these (item tooltip content was C++'s alone — `SetMerchantItem 0x534080`).
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

    // BuyMerchantItem(index [, quantity]) — queue the 1-based row + stack count (default 1); the app
    // maps the row to the item entry the wire addresses.
    g.set(
        "BuyMerchantItem",
        lua.create_function(|lua, (index, quantity): (u32, Option<u32>)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.merchant_buys.push((index, quantity.unwrap_or(1)));
            Ok(())
        })?,
    )?;

    // PickupMerchantItem(index) — **two verbs behind one name** (`0x4fb760`). `0x4fb787` calls
    // `GetCursorItem 0x494c60`, which answers for **mode 1 only**, and the result forks the whole
    // function:
    //
    //   * cursor holds a real bag item  → **SELL it** (`CMSG_SELL_ITEM 0x1a0`), then clear the
    //     cursor. The index is IGNORED on this arm and there is no merchant-open gate on it. This
    //     is what stock `MerchantFrame.xml:743`'s `<OnMouseUp>PickupMerchantItem(0)</OnMouseUp>`
    //     is for: dropping a bag item anywhere on the vendor window sells it.
    //   * otherwise                     → **grab** vendor row `index - 1` as cursor mode 5.
    //
    // Stock FrameXML never guards this call with `CursorHasItem()` — `MerchantFrame.lua:329`'s
    // `else` is the else of the ctrl/shift modifier chain. An unmodified left-click calls it
    // whatever the cursor holds, and the binding decides.
    //
    // **Every refusal is silent, and every refusal CLEARS THE CURSOR** — there is no `luaL_error`
    // anywhere in the range, unlike its neighbour `BuyMerchantItem`. The ladder, in order:
    // non-number argument (`0x4fb7cb`; a numeric *string* is accepted), `index < 1` (`0x4fb7e1`),
    // `index > count` (`0x4fb7e9`), no merchant open (`0x4fb7f7`), a dead row (`0x4fb80b`), and a
    // re-click on the row already held (`0x4fb818`, a toggle-off). Conversion is `_ftol` —
    // **truncate toward zero**, then `-1`, bounded signed.
    //
    // It does **not** test stock and does **not** test the item cache: a sold-out row grabs fine
    // and the buy still goes out, and the server refuses it. `numAvailable` is read exactly once
    // image-wide, by `GetMerchantItemInfo`, for display.
    g.set(
        "PickupMerchantItem",
        lua.create_function(|lua, index: Option<f64>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");

            // The sell fork, and it comes FIRST — before the argument is even looked at. The
            // clear is the SELL clear, not `ClearCursor`: it fires `CURSOR_UPDATE` and leaves the
            // source slot greyed until the server's inventory update
            // (`cursor::take_cursor_item_for_sale`, shared with the interact ladder's vendor arm,
            // which is the same `0x494b60` call — decision 1914).
            if let Some(item) = crate::script::cursor::take_cursor_item_for_sale(&mut model) {
                model.merchant_cursor_sells.push((item.bag, item.slot));
                return Ok(());
            }

            // From here every leg clears the cursor and answers nothing. Holding a mode-5 payload
            // and calling this again is the toggle-off, which is the same clear. It is the REAL
            // `ClearCursor(1,1)` (`0x4fb82d`/`0x4fb83f` on a refusal, `0x49510b` inside the grab
            // setter), not a bare payload drop: a spell
            // or action dropped on the vendor window fires `CURSOR_UPDATE` and hides the bar's
            // grid, and an armed gift wrap is cancelled (`0x5edf10`, `ClearCursor`'s opening act).
            let held = match &model.cursor {
                Some(CursorPayload::Merchant(m)) => Some(m.row),
                _ => None,
            };
            crate::script::cursor::clear_cursor(&mut model);

            let Some(index) = index else { return Ok(()) };
            // `_ftol` truncates toward zero; the bound is then applied signed, so a negative or
            // a zero index falls out here rather than wrapping into a row. NaN is written as its
            // own test rather than as a negated `>=` — same answer, and the intent is on the page.
            //
            // The upper bound is ours and it is load-bearing, not defensive: without it a Lua
            // index of `2^32 + 1` truncates on the cast to row 0 and would grab the FIRST row.
            // The reference cannot have that bug — its bound is against the vendor count, which
            // it applies before any narrowing.
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
            // The toggle-off: naming the row already on the cursor puts nothing back.
            if held == Some(row0) {
                return Ok(());
            }
            let payload = CursorPayload::Merchant(super::cursor::CursorMerchantItem {
                item_id: item.item_id,
                row: row0,
                texture: item.texture.clone(),
            });
            // The grab's own transition (`0x495159` `SignalEvent(CURSOR_UPDATE)`); mode 5 skips
            // the mode-7 `ACTIONBAR_SHOWGRID` branch, which the shared seam derives from the arm.
            model.cursor = Some(payload);
            crate::script::cursor::queue_cursor_update(&mut model);
            Ok(())
        })?,
    )?;

    // CloseMerchant() — client-side close (no packet, vanilla): flag it so the app clears its state.
    g.set(
        "CloseMerchant",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.merchant_close = true;
            Ok(())
        })?,
    )?;

    // → how many buyback slots are filled (ref GetNumBuybackItems; 0 with no vendor open).
    g.set(
        "GetNumBuybackItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.merchant.as_ref().map_or(0, |m| m.buyback.len()) as i64)
        })?,
    )?;

    // GetBuybackItemInfo(index) → name, texture, price, quantity, numAvailable, isUsable (the
    // same 6-tuple; 0x4fb310). `index` is 1-based
    // oldest-first; an invalid index answers the fixed tuple (nil, nil, 0, 1, 0, 1). isUsable is
    // the same 0x5ea930 gate over the sold item's template (0x4fb4f7) — yes, even a just-sold
    // item reds if the seller can't use it (a mule selling a wrong-class drop).
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

    // BenillaGetBuybackItemStats(index) — the buyback hover's tooltip stat head, same shape and
    // reason as BenillaGetMerchantItemStats.
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

    // BuybackItem(index) — queue the 1-based buyback slot; the app maps it to the wire's absolute
    // inventory slot.
    g.set(
        "BuybackItem",
        lua.create_function(|lua, index: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.merchant_buybacks.push(index);
            Ok(())
        })?,
    )?;

    // CanMerchantRepair() → 1/nil (the Era boolean shape) — whether the open vendor repairs.
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

    // GetRepairAllCost() → cost, canRepair — the app-computed repair-all total; canRepair is
    // "there is damage to pay for" (ref MerchantFrame_OnShow enables the button on it).
    g.set(
        "GetRepairAllCost",
        lua.create_function(|lua, ()| {
            let cost = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.merchant.as_ref().map_or(0, |m| m.repair_all_cost)
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(cost)),
                Value::Boolean(cost > 0),
            ]))
        })?,
    )?;

    // RepairAllItems() — flag the repair-all intent for the app to send.
    g.set(
        "RepairAllItems",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.repair_all = true;
            Ok(())
        })?,
    )?;

    // The repair-mode latch trio (ref MerchantRepairItemButton OnClick): ShowRepairCursor arms
    // "next item click repairs", HideRepairCursor disarms, InRepairMode() → 1/nil reads it. The
    // app reads UiScript::repair_mode to route clicks + swap the hardware cursor.
    g.set(
        "ShowRepairCursor",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .repair_mode = true;
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

    // ShowMerchantSellCursor(index) (5875 `0x4fbab0`, "buying from vendor") and
    // ShowBuybackSellCursor(index) (`0x4fbbb0`, "re-buying") — the vendor-item hover cursor.
    // Despite the "Sell" in the names, these arm the BUY cursor with
    // an affordability gate: player coin vs the row's price → Buy(3) if `coin >= price`, else the
    // grayed UnableBuy(23). The merchant frame's OnUpdate re-arms this every frame while an item is
    // hovered (Ctrl-hover swaps to `ShowInspectCursor` instead); OnLeave `ResetCursor`s it. An
    // unresolvable index leaves the cursor unchanged — the binary's every-fail-path `ret` with no
    // `CursorSetMode` (the in-flight item lock and base-mode-Point gates live app-side).
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

/// Arm the vendor-item hover cursor from a `price` picked out of the open merchant snapshot: Buy(3)
/// if the player can afford it, UnableBuy(23) otherwise. Shared by `ShowMerchantSellCursor` and
/// `ShowBuybackSellCursor` (which differ only in which price list they read). A `None` price (no
/// vendor open, or the 1-based index out of range) leaves the cursor untouched — the binary bails
/// without a `CursorSetMode` (`0x4fbab0`/`0x4fbbb0`).
fn arm_vendor_cursor(lua: &Lua, price_of: impl FnOnce(&MerchantState) -> Option<u32>) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    // The sell-cursor family bails on `IsTargeting` at its first instruction (`0x6e48a0`) — an
    // armed spell keeps its cast cursor.
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
    use crate::script::UiScript;

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
                    // A consumable's stat head: everything zero but the quality.
                    stats: Some(ItemStatsHead {
                        quality: 1,
                        ..Default::default()
                    }),
                    link: Some("|cffffffff|Hitem:159:0:0:0|h[Refreshing Spring Water]|h|r".into()),
                    max_stack: Some(20),
                },
                // An in-flight row: the vendor list arrived, the item-template answer hasn't.
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
        // No vendor open: count 0, info nil.
        assert_eq!(s.eval::<i64>("return GetMerchantNumItems()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetMerchantItemInfo(1) == nil")
            .unwrap());

        s.set_merchant(Some(stock()));
        assert_eq!(s.eval::<i64>("return GetMerchantNumItems()").unwrap(), 2);
        // Row 1 (resolved): the byte-verified 6-tuple (0x4fb150) — isUsable is 1, not a boolean.
        let (name, texture, price, quantity, num, usable) = s
            .eval::<(String, String, i64, i64, i64, i64)>("return GetMerchantItemInfo(1)")
            .unwrap();
        assert_eq!(name, "Refreshing Spring Water");
        assert_eq!(texture, "Interface\\Icons\\INV_Drink_18");
        assert_eq!((price, quantity, num), (25, 1, -1));
        assert_eq!(usable, 1);

        // Row 2 (in flight): name + texture nil, the rest still present — and usable (the
        // null-record skip: no template answer, nothing to judge).
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetMerchantItemInfo(2)\n\
                 return n == nil and t == nil and p == 1500 and u == 1",
            )
            .unwrap());
        // An invalid index answers the fixed 6-tuple (nil, nil, 0, 1, 0, 1) — 0x4fb2be.
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetMerchantItemInfo(9)\n\
                 return n == nil and t == nil and p == 0 and q == 1 and a == 0 and u == 1",
            )
            .unwrap());

        // GetMerchantItemLink: the resolved row's link; nil while the template is in flight and nil
        // out of range (the row click's ctrl/shift arms hand this straight on — decision 1059).
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

        // **GetMerchantItemMaxStack** — the one number that decides whether a vendor click asks
        // "how many?". The reference's `MerchantItemButton_OnClick` reads it twice
        // (`MerchantFrame.lua:313` on the right-click arm and `:340` on shift-click) and takes the
        // plain single-buy path whenever it is `<= 1`, opening the stack-split spinner otherwise.
        //
        // Nil while the row's template answer is in flight, and nil out of range — the same two
        // absences `GetMerchantItemLink` above has, and for the same reason: the value is the
        // template's `stackable`, which the wire has carried all along.
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

    /// The isUsable leg end-to-end: the gate reads the shared template store + the player req
    /// state, so a level-short player reds the row (nil), levelling past it whitens (1), and the
    /// same flag drives the buyback tuple.
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
            159, // row 1: the spring water becomes level-gated for the test
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
        // Row 2 has no template pushed → still usable, whatever the player state.
        assert!(s
            .eval::<bool>("local n, t, p, q, a, u = GetMerchantItemInfo(2)\nreturn u == 1")
            .unwrap());
        // Levelling past the requirement whitens both.
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
        // Row 1 becomes a sword so every stat column is distinguishable.
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
        // In flight (row 2) and out of range: nil.
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

    /// The buyback surface: count, the ref tuple (including the 5875 binding's exact empty tuple
    /// `(nil,nil,0,1,0,1)` for an invalid index — 0x4fb310), and the BuybackItem intent drain.
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

    /// The repair surface: CanMerchantRepair off the snapshot, GetRepairAllCost's (cost, canRepair)
    /// pair, the RepairAllItems intent, and the client-side repair-mode latch trio.
    #[test]
    fn repair_reads_intents_and_mode_latch() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return CanMerchantRepair() == nil").unwrap());

        let mut state = stock();
        state.can_repair = true;
        state.repair_all_cost = 1234;
        s.set_merchant(Some(state));
        assert!(s.eval::<bool>("return CanMerchantRepair() == 1").unwrap());
        assert!(s
            .eval::<bool>("local c, can = GetRepairAllCost()\nreturn c == 1234 and can == true",)
            .unwrap());

        s.run("RepairAllItems()").unwrap();
        assert!(s.take_repair_all());
        assert!(!s.take_repair_all(), "drained");

        // ShowRepairCursor arms, InRepairMode reads (1/nil), HideRepairCursor disarms.
        assert!(s.eval::<bool>("return InRepairMode() == nil").unwrap());
        s.run("ShowRepairCursor()").unwrap();
        assert!(s.repair_mode());
        assert!(s.eval::<bool>("return InRepairMode() == 1").unwrap());
        s.run("HideRepairCursor()").unwrap();
        assert!(!s.repair_mode());
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

    /// The vendor-item hover cursor (`ShowMerchantSellCursor`/`ShowBuybackSellCursor`, `0x4fbab0`/
    /// `0x4fbbb0`): Buy(3) when the player can afford the row, the grayed UnableBuy(23) when they
    /// can't; `ShowInspectCursor` arms the magnifier; `ResetCursor` clears the whole override; an
    /// unresolvable index (or no vendor open) leaves the cursor unchanged.
    #[test]
    fn vendor_hover_cursor_gates_on_affordability() {
        use crate::script::UiCursorMode;
        let mut s = UiScript::new().unwrap();

        // No vendor open: the Show* calls resolve nothing and leave the cursor untouched.
        s.run("ShowMerchantSellCursor(1)").unwrap();
        assert_eq!(s.ui_cursor(), None);

        let mut state = stock(); // row 1 price 25, row 2 price 1500
        state.buyback = vec![MerchantItem {
            price: 47,
            ..Default::default()
        }];
        s.set_merchant(Some(state));
        s.set_money(100); // affords row 1 (25) + buyback (47), not row 2 (1500)

        // Row 1: coin ≥ price → Buy(3).
        s.run("ShowMerchantSellCursor(1)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
        // Row 2: coin < price → the grayed UnableBuy(23).
        s.run("ShowMerchantSellCursor(2)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::UnableBuy));

        // The Ctrl-hover magnifier, then ResetCursor back to the base mode.
        s.run("ShowInspectCursor()").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Inspect));
        s.run("ResetCursor()").unwrap();
        assert_eq!(s.ui_cursor(), None);

        // Buyback affords too → Buy(3).
        s.run("ShowBuybackSellCursor(1)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));

        // An out-of-range index bails without touching the armed cursor (the binary's no-CursorSetMode
        // fail path) — the Buy from the buyback hover above survives.
        s.run("ShowMerchantSellCursor(99)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
    }
    /// `PickupMerchantItem` is TWO verbs, and which one runs is decided by the cursor, never by
    /// the caller — stock FrameXML never guards it (`0x4fb760`).
    #[test]
    fn pickup_merchant_item_grabs_a_row_as_mode_5() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));

        s.run("PickupMerchantItem(1)").unwrap();
        // Mode 5 is INVISIBLE to Lua. This is the load-bearing assertion in the whole file: all
        // thirteen stock `CursorHasItem()` gates stay closed while a vendor cursor is held.
        assert!(
            s.eval::<bool>("return not CursorHasItem()").unwrap(),
            "CursorHasItem is nil for mode 5"
        );
        assert!(s.eval::<bool>("return not CursorHasSpell()").unwrap());
        assert!(
            s.eval::<bool>("return GetCursorInfo() == nil").unwrap(),
            "GetCursorInfo reports nothing — no binding exposes mode 5"
        );

        // …but it IS held: a second call naming the same row is the toggle-off (`0x4fb818`), and
        // a third call re-grabs it. Nothing else can observe the difference from Lua, so the
        // toggle is what the test reads it through.
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupMerchantItem(1)").unwrap();
        assert!(s.take_merchant_slot_buys().is_empty(), "no drop yet");
    }

    /// Every refusal is silent AND clears the cursor — there is no `luaL_error` in the range.
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
        // A missing argument is the same silent clear, not an error.
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupMerchantItem()").unwrap();
        // A wrapping index must not become a valid row: `2^32 + 1` narrows to 0 on a naive cast
        // and would grab row 1. It refuses, and the cursor stays empty.
        s.run("PickupMerchantItem(4294967297)").unwrap();
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(
            s.take_merchant_slot_buys().is_empty(),
            "the wrapping index grabbed nothing"
        );
        // A numeric STRING is accepted (`_ftol` over a converted argument), and `1.9` truncates
        // toward zero to row 1 — not rounds to 2.
        s.run(r#"PickupMerchantItem("1")"#).unwrap();
        s.run("PickupMerchantItem(1.9)").unwrap();
        // …which the toggle proves: 1.9 named the row 1 already held, so it toggled off.
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
    }

    /// The sell arm: a bag item on the cursor makes this a sell, the index is ignored, and the
    /// cursor empties. This is what stock `MerchantFrame.xml:743`'s `PickupMerchantItem(0)` is —
    /// an index the grab arm would refuse outright.
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

    /// The drop is the buy. A held row placed into a bag slot queues `CMSG_BUY_ITEM_IN_SLOT`.
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

    /// A stale row is a refusal, not a wrong buy. The reference has a latent null deref on two of
    /// its three drop paths for exactly this case (a fresh `SMSG_LIST_INVENTORY` rewrites the
    /// count without clearing the cursor); we take the third path's behaviour on all of them.
    #[test]
    fn a_stale_vendor_row_drops_without_buying() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.run("PickupMerchantItem(2)").unwrap();
        // The vendor list is replaced while the cursor still holds row 2 of the old one.
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

    /// A vendor cursor is not a bar action and not an equip: `PlaceAction` refuses it and hands it
    /// back, exactly as it does the pet-action and stable-pet payloads.
    #[test]
    fn the_action_bar_refuses_a_vendor_row_and_keeps_it_held() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PlaceAction(1)").unwrap();
        // Still held, proven positively: dropping it in a bag afterwards still buys. An asserted
        // *absence* here would pass just as well if the bar had eaten the payload.
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

    /// The grab writes through the real cursor setter (`0x4950f0`): `CURSOR_UPDATE` at
    /// `0x495159`, and — mode 5 not being mode 7 — **no**
    /// `ACTIONBAR_SHOWGRID`: a vendor row cannot land on the bar, so its empty slots stay dark.
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
        // …and its toggle-off is a plain clear: one more CURSOR_UPDATE, and no HIDEGRID either.
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(cursor_event_counts(&mut s), (2, 0, 0));
    }

    /// Every non-sell leg opens with `ClearCursor(1,1)` (`0x4fb82d`/`0x4fb83f` on a refusal,
    /// `0x49510b` inside the grab), so a spell held over the
    /// vendor window and dropped there — stock `MerchantFrame.xml`'s `OnMouseUp`,
    /// `PickupMerchantItem(0)` — is a real clear: the bar's grid hides and `CURSOR_UPDATE` fires.
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

    /// `ClearCursor 0x495190` opens with the gift-wrap cancel (`0x5edf10`, decision 1934), and the
    /// grab's first act is that clear (`0x49510b`) — so taking a vendor row disarms a wrap.
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
