//! The player-to-player trade bindings (decision 0592 P1) — the two-sided trade-window surface, the
//! same push/intent seam as [`super::mail`]/[`super::merchant`]: the app pushes a **trade snapshot**
//! ([`UiScript::set_trade`] — both sides' slots already resolved from the wire to name/icon/quality
//! by the app's item-template + display caches, and the partner's name from the name cache) and the
//! Lua `InitiateTrade`/`AcceptTrade`/`CloseTrade`/… calls queue outbound **intents** the app drains.
//! The engine holds no trade knowledge — a slot is a resolved item view, a side is seven slots + a
//! gold amount, and the accept-glow is a fired event, never read state.
//!
//! ## The 5875 API shape (VERIFIED against the extracted `TradeFrame.lua`)
//!
//! `id` is **1-based** everywhere (slots 1..=7; slot 7 is the non-traded / enchant slot); an
//! out-of-range or empty slot answers `nil`. The read getters:
//! `GetTradePlayerItemInfo(id)` → `name, texture, numItems, isUsable, enchantment` (our own offer),
//! `GetTradeTargetItemInfo(id)` → `name, texture, numItems, quality, isUsable, enchantment` (the
//! partner's offer — the extra `quality` drives its slot's colour + the red not-usable tint),
//! `GetPlayerTradeMoney()`/`GetTargetTradeMoney()` → the two gold amounts in copper, and
//! `GetTradePartnerName()` → the partner's name (benilla's getter for the window header, in place of
//! the reference's `UnitName("NPC")`; the partner's *portrait* rides the `"npc"` token the app points
//! at the partner entity — decision 0592, [`crate::script`] has no engine-side unit for it).
//! `enchantment` is `nil` in P1 (the enchant-slot spell name is decision 0592 P3).
//!
//! The intents: `InitiateTrade(unit)` queues the right-click menu's trade offer (the app resolves the
//! unit token → guid → `CMSG_INITIATE_TRADE`); `AcceptTrade()`/`CancelTradeAccept()`/`CloseTrade()`
//! flag the accept / un-accept / cancel verbs the app maps to `CMSG_ACCEPT_TRADE` /
//! `CMSG_UNACCEPT_TRADE` / `CMSG_CANCEL_TRADE`. Setting items/gold onto the window (the drag-drop
//! slots + the money widget) is decision 0592 P2.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{self, CursorPayload};
use super::Model;

/// The seven trade slots per side (`TRADE_SLOT_COUNT`, vmangos `TradeData.h`) — six tradeable
/// (1..=6) plus the seventh non-traded / enchant slot. Mirrors the wire's fixed count without a
/// `benilla-protocol` dependency (this crate is engine-only).
pub const TRADE_SLOTS: usize = 7;

/// One resolved trade slot, as the app pushes it — the wire `TradeItem`'s
/// entry/display resolved through the shared item-template + display caches. Plain data; an empty
/// slot is `None` in [`TradeSideState::slots`], never a zeroed `Some`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSlotItem {
    /// The item template entry (`0` never stored — an empty slot is `None`) — the `isUsable` gate's
    /// key over the shared item-template store, and the slot tooltip's identity (decision 0592 P2).
    pub item_id: u32,
    /// The item name (from the template); `None` while the ask-once template answer is in flight.
    pub name: Option<String>,
    /// The item icon (from the wire `display_id` via `ItemDisplayInfo.dbc`); `None` while in flight.
    pub texture: Option<String>,
    /// The stack count in this slot (`>= 1` for a filled slot).
    pub count: u32,
    /// The item quality (0..=6); `None` while the template is in flight. Read only for the target
    /// side (the recipient slot's quality colour + red not-usable tint — `TradeFrame.lua` l.84).
    pub quality: Option<u32>,
    /// The enchant-slot spell name (slot 7 only) — `None` in P1 (the applied-spell path is
    /// decision 0592 P3).
    pub enchantment: Option<String>,
    /// The slot's full escaped `|cff…|Hitem:…|h[Name]|h|r` link — `GetTradePlayerItemLink` /
    /// `GetTradeTargetItemLink`'s answer, and `None` while the ask-once template answer is in
    /// flight (the link embeds the name and the quality colour).
    ///
    /// The same shape as [`super::merchant::MerchantItem::link`] and for the same reason: 1.12's
    /// own `TradeFrame.lua` hands it straight to `DressUpItemLink` on a ctrl-click and to
    /// `ChatFrameEditBox:Insert` on a shift-click, so it is a string the app already knows rather
    /// than anything the engine composes.
    pub link: Option<String>,
}

/// One side of the trade window — the seven slots (1-based in the API) plus the gold offered on
/// this side, in copper. `player` is our own offer, `target` the partner's ([`TradeState`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSideState {
    /// The seven slots (index 0 = trade slot 1 … index 6 = the enchant slot); `None` = empty.
    pub slots: [Option<TradeSlotItem>; TRADE_SLOTS],
    /// Gold offered on this side, in copper.
    pub gold: u32,
}

/// The open trade window's snapshot. Pushed whole by the app; `None` = no trade open (the window is
/// closed). The accept-glow state is **not** here — it rides the fired `TRADE_ACCEPT_UPDATE` event,
/// never a getter (the reference `TradeFrame_SetAcceptState`, `TradeFrame.lua` l.116).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeState {
    /// Our own offer (the wire's `their_window == false` snapshot).
    pub player: TradeSideState,
    /// The partner's offer (the wire's `their_window == true` snapshot).
    pub target: TradeSideState,
    /// The partner's name, resolved by the app through the name cache; `None` while in flight. The
    /// window header reads this (benilla's `GetTradePartnerName()` in place of `UnitName("NPC")`).
    pub partner_name: Option<String>,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open trade window's snapshot.
    pub fn set_trade(&mut self, state: Option<TradeState>) {
        self.model_mut().trade = state;
    }

    /// Drain the unit tokens `InitiateTrade` queued since the last drain — the app resolves each
    /// token → player guid and sends `CMSG_INITIATE_TRADE`.
    pub fn take_trade_initiates(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().trade_initiates)
    }

    /// Whether `AcceptTrade` was called since the last drain (and clear the flag) — the app maps it
    /// to `CMSG_ACCEPT_TRADE`.
    pub fn take_trade_accept(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_accept)
    }

    /// Whether `CancelTradeAccept` was called since the last drain — the app maps it to
    /// `CMSG_UNACCEPT_TRADE` (drop your accept, stay in the trade).
    pub fn take_trade_unaccept(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_unaccept)
    }

    /// Whether `CloseTrade` was called since the last drain — the app maps it to `CMSG_CANCEL_TRADE`
    /// and clears its local trade session (the window's OnHide close verb).
    pub fn take_trade_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_close)
    }

    /// Whether `BeginTrade()` was called since the last drain — `CMSG_BEGIN_TRADE`, empty
    /// (the TRADE dialog that calls it can never show in 1.12.1, so this is an
    /// addon's reach).
    pub fn take_trade_begin(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_begin)
    }

    /// Whether `CancelTrade()` was called since the last drain — the BARE `CMSG_CANCEL_TRADE`,
    /// where `CloseTrade` wraps the same opcode in the window's teardown.
    pub fn take_trade_cancel(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_cancel)
    }

    /// The copper amount `SetTradeMoney` last offered since the last drain (and clear it) — the app
    /// maps it to `CMSG_SET_TRADE_GOLD` (decision 0592 P2).
    pub fn take_trade_money(&mut self) -> Option<u32> {
        std::mem::take(&mut self.model_mut().trade_set_money)
    }

    /// The `(trade_id, bag, slot)` placements `ClickTradeButton` queued since the last drain — the app
    /// maps each `(bag, slot)` through its wire-position map to `CMSG_SET_TRADE_ITEM` (decision 0592 P2).
    /// `trade_id` is 1-based (1..=7); `bag`/`slot` are the engine's cursor space (0 backpack …).
    pub fn take_trade_set_items(&mut self) -> Vec<(u32, i64, u32)> {
        std::mem::take(&mut self.model_mut().trade_set_items)
    }

    /// The 1-based trade slot ids `ClickTradeButton` queued to clear (empty-cursor click on a filled
    /// slot) — the app maps each to `CMSG_CLEAR_TRADE_ITEM` (decision 0592 P2).
    pub fn take_trade_clear_items(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().trade_clear_items)
    }
}

/// Fetch a cloned slot for a 1-based index off one side, or `None` (out of range / empty / no trade
/// open). `pick` selects the side from the pushed [`TradeState`].
fn slot_at(
    model: &Model,
    index: usize,
    pick: impl Fn(&TradeState) -> &TradeSideState,
) -> Option<TradeSlotItem> {
    model
        .trade
        .as_ref()
        .map(pick)
        .and_then(|side| index.checked_sub(1).and_then(|n| side.slots.get(n)))
        .cloned()
        .flatten()
}

/// The gold on one side (copper), or `0` if no trade is open.
fn gold(model: &Model, pick: impl Fn(&TradeState) -> &TradeSideState) -> u32 {
    model.trade.as_ref().map(pick).map_or(0, |side| side.gold)
}

/// A click on OUR trade slot `id` (1-based) as a cursor drop target (decision 0592 P2, ref
/// `ClickTradeButton`, TradeFrame.xml l.135): a held bag item drops in — queue a `(id, bag, slot)`
/// placement the app maps to `CMSG_SET_TRADE_ITEM` — and the cursor is **cleared** (the 0218
/// plain-clear: no local held copy, the filled slot reads back from `SMSG_TRADE_STATUS_EXTENDED`). An
/// empty cursor on a filled slot queues a clear (`CMSG_CLEAR_TRADE_ITEM`; the item never left the bag,
/// the server just un-references it). A spell/action payload is refused, put back untouched.
fn click_trade_button(model: &mut Model, id: u32) {
    // The money arm runs first and never reads the index (`0x4bfe34`): coins on
    // the cursor go into the offer as `AddTradeMoney` puts them, and that is the whole click.
    if matches!(model.cursor, Some(CursorPayload::Money(_))) {
        cursor::money::add_trade_money(model);
        return;
    }
    match model.cursor.take() {
        Some(CursorPayload::Item(item)) => {
            let (bag, slot) = (item.bag, item.slot);
            model.trade_set_items.push((id, bag, slot));
            cursor::queue_cursor_update(model);
            cursor::queue_lock_changed(model, bag, slot);
        }
        None => {
            let filled = matches!(
                id.checked_sub(1).and_then(|i| {
                    model
                        .trade
                        .as_ref()
                        .and_then(|t| t.player.slots.get(i as usize))
                }),
                Some(Some(_))
            );
            if filled {
                model.trade_clear_items.push(id);
            }
        }
        Some(other) => model.cursor = Some(other),
    }
}

/// Register the trade globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetTradePlayerItemInfo(id) → name, texture, numItems, isUsable, enchantment (our own offer;
    // TradeFrame.lua l.55). An empty/out-of-range slot answers nil.
    g.set(
        "GetTradePlayerItemInfo",
        lua.create_function(|lua, id: usize| {
            let (slot, usable) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let slot = slot_at(&model, id, |t| &t.player);
                let usable = slot
                    .as_ref()
                    .is_none_or(|s| super::item_stats::item_usable_by_id(&model, s.item_id));
                (slot, usable)
            };
            let Some(s) = slot else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let opt_str = |v: &Option<String>| -> mlua::Result<Value> {
                Ok(match v {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&s.name)?,
                opt_str(&s.texture)?,
                Value::Integer(i64::from(s.count.max(1))),
                flag(usable),
                opt_str(&s.enchantment)?,
            ]))
        })?,
    )?;

    // GetTradePlayerItemLink(id) / GetTradeTargetItemLink(id) → the slot's escaped item link, or
    // nil for an empty slot / one whose template answer is still in flight.
    //
    // 1.12's `TradeFrame.lua` reaches for these on a modified click of a trade slot — ctrl hands
    // the link to `DressUpItemLink`, shift inserts it into the chat box — the same pair of arms
    // the merchant rows have, which is why this mirrors `GetMerchantItemLink` exactly rather than
    // composing anything here.
    for (name, side) in [
        ("GetTradePlayerItemLink", true),
        ("GetTradeTargetItemLink", false),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, id: usize| {
                let link = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let slot = if side {
                        slot_at(&model, id, |t| &t.player)
                    } else {
                        slot_at(&model, id, |t| &t.target)
                    };
                    slot.and_then(|s| s.link.clone())
                };
                match link {
                    Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
    }

    // GetTradeTargetItemInfo(id) → name, texture, numItems, quality, isUsable, enchantment (the
    // partner's offer; TradeFrame.lua l.84 — the extra `quality` is the recipient-only colour).
    g.set(
        "GetTradeTargetItemInfo",
        lua.create_function(|lua, id: usize| {
            let (slot, usable) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let slot = slot_at(&model, id, |t| &t.target);
                let usable = slot
                    .as_ref()
                    .is_none_or(|s| super::item_stats::item_usable_by_id(&model, s.item_id));
                (slot, usable)
            };
            let Some(s) = slot else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let opt_str = |v: &Option<String>| -> mlua::Result<Value> {
                Ok(match v {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            let quality = match s.quality {
                Some(q) => Value::Integer(i64::from(q)),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&s.name)?,
                opt_str(&s.texture)?,
                Value::Integer(i64::from(s.count.max(1))),
                quality,
                flag(usable),
                opt_str(&s.enchantment)?,
            ]))
        })?,
    )?;

    // GetPlayerTradeMoney() / GetTargetTradeMoney() → the two gold amounts, copper (TradeFrame.lua
    // l.165/l.585). 0 when no trade is open.
    g.set(
        "GetPlayerTradeMoney",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(gold(&model, |t| &t.player)))
        })?,
    )?;
    g.set(
        "GetTargetTradeMoney",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(gold(&model, |t| &t.target)))
        })?,
    )?;

    // GetTradePartnerName() → the partner's name (benilla's header getter, in place of the
    // reference's UnitName("NPC") — see the module doc). nil while in flight / no trade open.
    g.set(
        "GetTradePartnerName",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(
                match model.trade.as_ref().and_then(|t| t.partner_name.clone()) {
                    Some(n) => Value::String(lua.create_string(&n)?),
                    None => Value::Nil,
                },
            )
        })?,
    )?;

    // InitiateTrade(unit) — queue the right-click menu's trade offer against a unit token; the app
    // resolves it → player guid → CMSG_INITIATE_TRADE (the UnitPopup TRADE row, decision 0592 P1).
    g.set(
        "InitiateTrade",
        lua.create_function(|lua, unit: String| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_initiates
                .push(unit);
            Ok(())
        })?,
    )?;

    // AcceptTrade() / CancelTradeAccept() / CloseTrade() — the Trade / un-accept / cancel verbs the
    // app maps to CMSG_ACCEPT_TRADE / CMSG_UNACCEPT_TRADE / CMSG_CANCEL_TRADE (decision 0592 P1).
    g.set(
        "AcceptTrade",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_accept = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CancelTradeAccept",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_unaccept = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CloseTrade",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_close = true;
            Ok(())
        })?,
    )?;
    // BeginTrade() / CancelTrade() — the TRADE dialog's pair (`0x48aa60`/`0x48aa70`:
    // 0 args, 0 returns, no gate; `0x117` and `0x11C`, both empty). The dialog itself can never
    // show in 1.12.1 (TRADE_REQUEST is signalled by nothing), so both are an addon's reach;
    // CancelTrade is the bare packet, CloseTrade above the same opcode wrapped in the teardown.
    g.set(
        "BeginTrade",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_begin = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CancelTrade",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .trade_cancel = true;
            Ok(())
        })?,
    )?;

    // SetTradeMoney(copper) — offer this many copper on our side (the money input's value-changed
    // callback); the app maps it to CMSG_SET_TRADE_GOLD (decision 0592 P2). `i64` in, clamped, so a
    // fractional/negative Lua number can never panic the coercion.
    g.set(
        "SetTradeMoney",
        lua.create_function(|lua, copper: Value| {
            // `0x4c0820`: a non-number RAISES; the low dword goes on the wire as an
            // absolute offer, gated on the purse covering it — a refusal is silent at every level.
            let n = crate::script::binding_abi::number_arg(
                lua,
                copper,
                "Usage: SetTradeMoney(amount)",
            )? as u32;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if u64::from(n) > model.money {
                return Ok(());
            }
            model.trade_set_money = Some(n);
            Ok(())
        })?,
    )?;

    // ClickTradeButton(id) — drop a held bag item into / clear our trade slot `id` (1..=7); the app
    // maps it to CMSG_SET_TRADE_ITEM / CMSG_CLEAR_TRADE_ITEM (decision 0592 P2).
    g.set(
        "ClickTradeButton",
        lua.create_function(|lua, id: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            click_trade_button(&mut model, id);
            Ok(())
        })?,
    )?;

    // ClickTargetTradeButton(id) — the partner's column is read-only (their offer is server-driven);
    // inert in P2 (the shift-click item link is decision 0592 P3). Registered so the shared slot
    // handler can call it for the recipient side without erroring.
    g.set(
        "ClickTargetTradeButton",
        lua.create_function(|lua, _id: u32| {
            // The same money arm as `ClickTradeButton` (`0x4c00a3`): coins on the
            // cursor go into OUR offer whichever side's slot was clicked, the index unread.
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            cursor::money::add_trade_money(&mut model);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn item(item_id: u32, count: u32, quality: u32) -> TradeSlotItem {
        TradeSlotItem {
            item_id,
            name: Some("Linen Cloth".into()),
            texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
            count,
            quality: Some(quality),
            enchantment: None,
            link: Some(format!(
                "|cffffffff|Hitem:{item_id}:0:0:0|h[Linen Cloth]|h|r"
            )),
        }
    }

    fn state() -> TradeState {
        let mut player = TradeSideState {
            gold: 12_345,
            ..Default::default()
        };
        player.slots[0] = Some(item(2589, 5, 1));
        let mut target = TradeSideState {
            gold: 500,
            ..Default::default()
        };
        target.slots[0] = Some(item(4306, 1, 2));
        target.slots[6] = Some(item(6217, 1, 1)); // the enchant slot carries an item
        TradeState {
            player,
            target,
            partner_name: Some("Thrall".into()),
        }
    }

    /// **The two link verbs**, and the two absences they share with every other item-link reader
    /// here: an empty slot, and a slot whose ask-once template answer has not landed.
    ///
    /// 1.12's `TradeFrame.lua` reaches for these on a modified click of a trade slot — ctrl hands
    /// the link to `DressUpItemLink`, shift inserts it into the chat box. They were two of the four
    /// engine verbs the readiness probe reports against stock `TradeFrame.xml`.
    #[test]
    fn the_trade_slots_answer_their_item_links() {
        let mut s = UiScript::new().unwrap();
        // No trade open: both sides answer nil rather than raising.
        assert!(s
            .eval::<bool>("return GetTradePlayerItemLink(1) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradeTargetItemLink(1) == nil")
            .unwrap());

        let mut st = state();
        // A slot whose template is still in flight carries no link — the string embeds the name.
        st.target.slots[1] = Some(TradeSlotItem {
            item_id: 4306,
            name: None,
            texture: None,
            count: 1,
            quality: None,
            enchantment: None,
            link: None,
        });
        s.set_trade(Some(st));

        assert_eq!(
            s.eval::<String>("return GetTradePlayerItemLink(1)")
                .unwrap(),
            "|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r"
        );
        assert_eq!(
            s.eval::<String>("return GetTradeTargetItemLink(1)")
                .unwrap(),
            "|cffffffff|Hitem:4306:0:0:0|h[Linen Cloth]|h|r"
        );
        // …the in-flight slot, and an empty one, and one past the seven.
        assert!(s
            .eval::<bool>("return GetTradeTargetItemLink(2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradePlayerItemLink(7) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradePlayerItemLink(9) == nil")
            .unwrap());
    }

    #[test]
    fn player_and_target_item_info_read_the_reference_tuples() {
        let mut s = UiScript::new().unwrap();
        // No trade open → every slot is nil, money 0.
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(1) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(), 0);

        s.set_trade(Some(state()));

        // Player slot 1: name, texture, count 5, isUsable 1, enchantment nil.
        let (name, tex, count, usable): (String, String, i64, i64) = s
            .eval(
                "local n,t,c,u,e = GetTradePlayerItemInfo(1)\n\
                 return n,t,c,u",
            )
            .unwrap();
        assert_eq!((name.as_str(), count, usable), ("Linen Cloth", 5, 1));
        assert_eq!(tex, "Interface\\Icons\\INV_Fabric_Linen_01");
        assert!(s
            .eval::<bool>("local n,t,c,u,e = GetTradePlayerItemInfo(1)\nreturn e == nil")
            .unwrap());
        // An empty player slot → nil.
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(2) == nil")
            .unwrap());

        // Target slot 1 carries the extra quality (index 4 of the 6-tuple).
        let (name, _tex, count, quality, usable): (String, String, i64, i64, i64) =
            s.eval("return GetTradeTargetItemInfo(1)").unwrap();
        assert_eq!(
            (name.as_str(), count, quality, usable),
            ("Linen Cloth", 1, 2, 1)
        );
        // The enchant slot (7) is filled on the target side.
        assert!(s
            .eval::<bool>("return GetTradeTargetItemInfo(7) ~= nil")
            .unwrap());
    }

    #[test]
    fn money_and_partner_name_read_the_pushed_state() {
        let mut s = UiScript::new().unwrap();
        s.set_trade(Some(state()));
        assert_eq!(
            s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(),
            12_345
        );
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 500);
        assert_eq!(
            s.eval::<String>("return GetTradePartnerName()").unwrap(),
            "Thrall"
        );
    }

    #[test]
    fn clearing_the_trade_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_trade(Some(state()));
        s.set_trade(None);
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(1) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetTradePartnerName() == nil")
            .unwrap());
    }

    #[test]
    fn intents_queue_and_drain() {
        let mut s = UiScript::new().unwrap();
        s.run("InitiateTrade('target')").unwrap();
        s.run("InitiateTrade('party2')").unwrap();
        assert_eq!(s.take_trade_initiates(), vec!["target", "party2"]);
        assert!(s.take_trade_initiates().is_empty(), "drained");

        s.run("AcceptTrade()").unwrap();
        assert!(s.take_trade_accept());
        assert!(!s.take_trade_accept(), "drained");

        s.run("CancelTradeAccept()").unwrap();
        assert!(s.take_trade_unaccept());

        s.run("CloseTrade()").unwrap();
        assert!(s.take_trade_close());
        assert!(!s.take_trade_close(), "drained");

        // SetTradeMoney folds gold/silver/copper into a copper total the app ships as SET_TRADE_GOLD,
        // gated on the purse covering it (1965).
        s.set_money(20_000);
        s.run("SetTradeMoney(1 * 10000 + 23 * 100 + 45)").unwrap();
        assert_eq!(s.take_trade_money(), Some(12_345));
        assert_eq!(s.take_trade_money(), None, "drained");
        // The low dword of -5 is an unsigned 0xFFFFFFFB, past any purse: refused silently (1965).
        s.run("SetTradeMoney(-5)").unwrap();
        assert_eq!(s.take_trade_money(), None);
        s.run("SetTradeMoney(30000)").unwrap();
        assert_eq!(s.take_trade_money(), None, "more than the purse: silent");
        assert!(s.run("SetTradeMoney(nil)").is_err(), "a non-number raises");
    }

    #[test]
    fn click_trade_button_places_clears_and_refuses() {
        use crate::script::cursor::{CursorItem, CursorPayload};

        let item = |bag: i64, slot: u32| {
            CursorPayload::Item(CursorItem {
                bar_placeable: true,
                bag,
                slot,
                item_id: 2589,
                texture: None,
                link: None,
                count: None,
                quality: None,
                equip_slots: Vec::new(),
            })
        };

        let mut s = UiScript::new().unwrap();

        // Empty cursor, no trade → nothing queued.
        s.run("ClickTradeButton(1)").unwrap();
        assert!(s.take_trade_set_items().is_empty());
        assert!(s.take_trade_clear_items().is_empty());

        // A held backpack item (bag 0, slot 3) dropped on our slot 2 → a (2, 0, 3) placement, and the
        // cursor is cleared (0218 plain-clear — no local held copy).
        s.model_mut().cursor = Some(item(0, 3));
        s.run("ClickTradeButton(2)").unwrap();
        assert_eq!(s.take_trade_set_items(), vec![(2, 0, 3)]);
        assert!(
            s.eval::<bool>("return not CursorHasItem()").unwrap(),
            "the drop clears the cursor"
        );

        // Empty cursor on a FILLED player slot → clear it; on an empty slot → nothing.
        let mut st = TradeState::default();
        st.player.slots[0] = Some(TradeSlotItem {
            item_id: 2589,
            count: 1,
            ..Default::default()
        });
        s.set_trade(Some(st));
        s.run("ClickTradeButton(1)").unwrap();
        assert_eq!(s.take_trade_clear_items(), vec![1]);
        s.run("ClickTradeButton(4)").unwrap();
        assert!(
            s.take_trade_clear_items().is_empty(),
            "an empty slot clears nothing"
        );

        // The partner column is read-only — ClickTargetTradeButton never queues and never eats the cursor.
        s.model_mut().cursor = Some(item(0, 3));
        s.run("ClickTargetTradeButton(1)").unwrap();
        assert!(s.take_trade_set_items().is_empty());
        assert!(
            s.eval::<bool>("return CursorHasItem()").unwrap(),
            "a partner-side click leaves the cursor untouched"
        );

        // A spell payload is refused — put back untouched, nothing queued.
        s.model_mut().cursor = Some(CursorPayload::Spell(crate::script::cursor::CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
        }));
        s.run("ClickTradeButton(1)").unwrap();
        assert!(s.take_trade_set_items().is_empty());
        assert!(
            s.eval::<bool>("return CursorHasSpell()").unwrap(),
            "a spell cursor is refused, left in place"
        );
    }
}
