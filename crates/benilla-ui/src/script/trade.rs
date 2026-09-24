//! The trade bindings: the app pushes both sides of the open trade ([`UiScript::set_trade`]), and
//! the trade verbs queue intents it drains. Slots are 1-based, 1..=7, the seventh the non-traded
//! enchant slot; an empty or out-of-range slot answers nil. The partner rides the `"npc"` unit,
//! which the app points at them.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{self, CursorPayload};
use super::Model;

/// Slots per side, six traded and the non-traded enchant slot: vmangos `TRADE_SLOT_COUNT`
/// (`TradeData.h`), kept equal to the protocol crate's, which this crate does not depend on.
pub const TRADE_SLOTS: usize = 7;

/// One filled trade slot, resolved by the app; an empty slot is `None`, never a zeroed `Some`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSlotItem {
    /// The item entry, the key for `isUsable` and the tooltip.
    pub item_id: u32,
    /// `None` while the item template is in flight.
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    /// `None` while the template is in flight; only the target side's info returns it
    /// (`TradeFrame.lua:84`).
    pub quality: Option<u32>,
    /// The name of the enchant being applied to slot 7, which stock shows in green
    /// (`TradeFrame.lua:62`); the app always sends `None`.
    pub enchantment: Option<String>,
    /// The escaped item link the link verbs answer; `None` while the template is in flight, since
    /// it embeds the name and the quality.
    pub link: Option<String>,
}

/// One side's offer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSideState {
    /// Index 0 is trade slot 1, index 6 the enchant slot.
    pub slots: [Option<TradeSlotItem>; TRADE_SLOTS],
    /// In copper.
    pub gold: u32,
}

/// The open trade, pushed whole by the app. The accept highlight is not here: it rides the
/// `TRADE_ACCEPT_UPDATE` event (`TradeFrame.lua:35`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeState {
    /// Our offer.
    pub player: TradeSideState,
    /// The partner's offer, the wire's `their_window` snapshot.
    pub target: TradeSideState,
    /// The partner's name for `GetTradePartnerName()`, `None` while in flight.
    pub partner_name: Option<String>,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open trade window's snapshot.
    pub fn set_trade(&mut self, state: Option<TradeState>) {
        self.model_mut().trade = state;
    }

    /// Drain the tokens `InitiateTrade` queued; the app sends `CMSG_INITIATE_TRADE` for each.
    pub fn take_trade_initiates(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().trade_initiates)
    }

    /// Whether `AcceptTrade` was called since the last drain: `CMSG_ACCEPT_TRADE`.
    pub fn take_trade_accept(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_accept)
    }

    /// Whether `CancelTradeAccept` was called since the last drain: `CMSG_UNACCEPT_TRADE`.
    pub fn take_trade_unaccept(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_unaccept)
    }

    /// Whether `CloseTrade`, the window's OnHide verb, was called since the last drain; the app
    /// sends `CMSG_CANCEL_TRADE` and ends its session.
    pub fn take_trade_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_close)
    }

    /// Whether `BeginTrade()` was called since the last drain: an empty `CMSG_BEGIN_TRADE`.
    pub fn take_trade_begin(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_begin)
    }

    /// Whether `CancelTrade()` was called since the last drain: the bare `CMSG_CANCEL_TRADE`.
    pub fn take_trade_cancel(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trade_cancel)
    }

    /// The copper `SetTradeMoney` last offered since the last drain: `CMSG_SET_TRADE_GOLD`.
    pub fn take_trade_money(&mut self) -> Option<u32> {
        std::mem::take(&mut self.model_mut().trade_set_money)
    }

    /// The `(trade_id, bag, slot)` placements `ClickTradeButton` queued, for `CMSG_SET_TRADE_ITEM`:
    /// `trade_id` 1..=7, `bag` and `slot` in the cursor's space.
    pub fn take_trade_set_items(&mut self) -> Vec<(u32, i64, u32)> {
        std::mem::take(&mut self.model_mut().trade_set_items)
    }

    /// The 1-based slots `ClickTradeButton` queued to clear, for `CMSG_CLEAR_TRADE_ITEM`.
    pub fn take_trade_clear_items(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().trade_clear_items)
    }
}

/// The slot at a 1-based index on the side `pick` selects.
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

fn gold(model: &Model, pick: impl Fn(&TradeState) -> &TradeSideState) -> u32 {
    model.trade.as_ref().map(pick).map_or(0, |side| side.gold)
}

/// `ClickTradeButton(id)` on our slot (`TradeFrame.xml:135`): a held bag item is queued for
/// `CMSG_SET_TRADE_ITEM` and the cursor cleared, the app filling our column itself since vmangos
/// echoes a placement only to the partner (`TradeData.cpp:81`); an empty cursor on a filled slot
/// queues a clear. Any other payload stays on the cursor.
fn click_trade_button(model: &mut Model, id: u32) {
    // Coins on the cursor go into the offer, the index unread (`0x4bfe34`).
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

    // GetTradePlayerItemInfo(id): the five values `TradeFrame.lua:55` reads.
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

    // The link verbs, for a slot's ctrl and shift clicks (`TradeFrame.xml:129`, `:95`).
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

    // GetTradeTargetItemInfo(id): the six values `TradeFrame.lua:84` reads.
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

    // `GetTradePartnerName()` is not a 1.12 global; the stock header reads `UnitName("NPC")`
    // (`TradeFrame.lua:43`).
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

    // InitiateTrade(unit): the unit menu's TRADE row.
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
    // BeginTrade() and CancelTrade() (`0x48aa60`, `0x48aa70`): no arguments, no returns, no gate,
    // and an empty `0x117` or `0x11C`. They are the `TRADE` popup's Yes and No
    // (`StaticPopup.lua:517-522`), which never shows in 1.12 as nothing signals `TRADE_REQUEST`.
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

    // SetTradeMoney(copper): the money input's callback (`TradeFrame.lua:173`).
    g.set(
        "SetTradeMoney",
        lua.create_function(|lua, copper: Value| {
            // `0x4c0820`: a non-number raises; the low dword is the whole offer, sent only when the
            // purse covers it, and a refusal is silent.
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

    g.set(
        "ClickTradeButton",
        lua.create_function(|lua, id: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            click_trade_button(&mut model, id);
            Ok(())
        })?,
    )?;

    // ClickTargetTradeButton(id) (`TradeFrame.xml:101`): the partner's column takes nothing.
    g.set(
        "ClickTargetTradeButton",
        lua.create_function(|lua, _id: u32| {
            // Coins on the cursor still go into our offer, the index unread (`0x4c00a3`).
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
        // A slot whose template is in flight has no link.
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
        // The in-flight slot, an empty one, and one past the seven.
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
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(2) == nil")
            .unwrap());

        // The target side adds quality as the fourth return.
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

        // An offer is sent only when the purse covers it.
        s.set_money(20_000);
        s.run("SetTradeMoney(1 * 10000 + 23 * 100 + 45)").unwrap();
        assert_eq!(s.take_trade_money(), Some(12_345));
        assert_eq!(s.take_trade_money(), None, "drained");
        // The low dword of -5 is 0xFFFFFFFB, past any purse: refused silently.
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

        // A held backpack item dropped on our slot 2 queues (2, 0, 3) and clears the cursor.
        s.model_mut().cursor = Some(item(0, 3));
        s.run("ClickTradeButton(2)").unwrap();
        assert_eq!(s.take_trade_set_items(), vec![(2, 0, 3)]);
        assert!(
            s.eval::<bool>("return not CursorHasItem()").unwrap(),
            "the drop clears the cursor"
        );

        // An empty cursor clears a filled slot and does nothing on an empty one.
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

        // The partner's column queues nothing and leaves the cursor alone.
        s.model_mut().cursor = Some(item(0, 3));
        s.run("ClickTargetTradeButton(1)").unwrap();
        assert!(s.take_trade_set_items().is_empty());
        assert!(
            s.eval::<bool>("return CursorHasItem()").unwrap(),
            "a partner-side click leaves the cursor untouched"
        );

        // A spell payload is refused and stays on the cursor.
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
