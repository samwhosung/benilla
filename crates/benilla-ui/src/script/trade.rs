//! The trade bindings: the app pushes both sides of the open trade ([`UiScript::set_trade`]), and
//! the trade verbs queue intents it drains. Slots are 1-based, 1..=7, the seventh the non-traded
//! enchant slot; an empty or out-of-range slot answers nil. The partner rides the `"npc"` unit,
//! which the app points at them.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{self, CursorPayload};
use super::Model;
use crate::script::binding_abi::Idx;

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
    /// `None` while the template is in flight. `GetTradePlayerItemInfo` and
    /// `GetTradeTargetItemInfo` report it only when `inventory_type` is nonzero (`0x4c059c`,
    /// `0x4c0241`), else -1.
    pub quality: Option<u32>,
    /// The template's InventoryType; 0 while the template is in flight and for an item that cannot
    /// be equipped.
    pub inventory_type: u32,
    /// The wire's `lock_id`: the trade target's tooltip draws LOCKED off it (`0x534563`,
    /// `0x52bdd2`). 0 for none; the player's side ignores it.
    pub lock_id: u32,
    /// The player's own side only: the placed item's `ITEM_FIELD_FLAGS`. Its presence means the
    /// slot holds a live item object, which the tooltip builder's LOCKED gate reads (`0x5342ed`);
    /// the partner's side has none.
    pub object_flags: Option<u32>,
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
    match model.cursor.clone() {
        Some(CursorPayload::Item(item)) => {
            let (bag, slot) = (item.bag, item.slot);
            model.trade_set_items.push((id, bag, slot));
            // The reference passes `SetCursorItem` the send's failure as its unlock (`0x4c005f`),
            // so a sent item's clear is `ClearCursor(0,1)`. The lock event reports the engine's
            // lock lifting with the cursor ([`cursor::client_locked`]), which holds no trade slot.
            cursor::clear(model, cursor::Clear::KEEP_LOCK);
            cursor::raise_lock_changed(model);
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
        Some(_) => {}
    }
}

/// Register the trade globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetTradePlayerItemInfo(id): name, texture, numItems, quality, enchantment; an empty slot is
    // nil, nil, 0, 0, nil (`0x4c0613`), and a filled one's texture is a string. The fourth is the quality, -1 unless the template is cached
    // and equippable (`0x4c059c`), never a usable flag; `TradeFrame.lua:55` names it `isUsable` and
    // never reads it.
    g.set(
        "GetTradePlayerItemInfo",
        lua.create_function(|lua, Idx(id): Idx| {
            let slot = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                slot_at(&model, id, |t| &t.player)
            };
            let Some(s) = slot else {
                return Ok(super::binding_abi::tuple(&[
                    None,
                    None,
                    Some(0),
                    Some(0),
                    None,
                ]));
            };
            let opt_str = |v: &Option<String>| -> mlua::Result<Value> {
                Ok(match v {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&s.name)?,
                // The question mark while the template is unknown (`0x4c0529`).
                Value::String(
                    lua.create_string(
                        s.texture
                            .as_deref()
                            .unwrap_or(super::item_stats::MISSING_ITEM_ICON),
                    )?,
                ),
                Value::Integer(i64::from(s.count.max(1))),
                Value::Integer(super::item_stats::gated_quality(
                    s.quality,
                    s.inventory_type,
                )),
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
            lua.create_function(move |lua, Idx(id): Idx| {
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

    // GetTradeTargetItemInfo(id): the six values `TradeFrame.lua:84` reads; an empty slot is
    // nil, nil, 0, 0, nil, nil (`0x4c02f6`). The fourth is the quality through the same gate as
    // the player's (`0x4c0241`).
    g.set(
        "GetTradeTargetItemInfo",
        lua.create_function(|lua, Idx(id): Idx| {
            let (slot, usable) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let slot = slot_at(&model, id, |t| &t.target);
                let usable = slot
                    .as_ref()
                    .is_none_or(|s| super::item_stats::item_usable_by_id(&model, s.item_id));
                (slot, usable)
            };
            // A template the cache lacks takes the miss too (`0x4c019b`, `0x4c02f6`).
            let Some(s) = slot.filter(|s| s.name.is_some()) else {
                return Ok(super::binding_abi::tuple(&[
                    None,
                    None,
                    Some(0),
                    Some(0),
                    None,
                    None,
                ]));
            };
            let opt_str = |v: &Option<String>| -> mlua::Result<Value> {
                Ok(match v {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&s.name)?,
                Value::String(
                    lua.create_string(
                        s.texture
                            .as_deref()
                            .unwrap_or(super::item_stats::MISSING_ITEM_ICON),
                    )?,
                ),
                Value::Integer(i64::from(s.count.max(1))),
                Value::Integer(super::item_stats::gated_quality(
                    s.quality,
                    s.inventory_type,
                )),
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
            super::tick::verb(lua, |model| click_trade_button(model, id));
            Ok(())
        })?,
    )?;

    // ClickTargetTradeButton(id) (`TradeFrame.xml:101`): the partner's column takes nothing.
    g.set(
        "ClickTargetTradeButton",
        lua.create_function(|lua, _id: u32| {
            // Coins on the cursor still go into our offer, the index unread (`0x4c00a3`).
            super::tick::verb(lua, cursor::money::add_trade_money);
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
            inventory_type: 0,
            lock_id: 0,
            object_flags: None,
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
        TradeState { player, target }
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
            inventory_type: 0,
            lock_id: 0,
            object_flags: None,
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

        // Linen cloth has InventoryType 0, so the fourth value is the -1 sentinel.
        let (name, tex, count, quality): (String, String, i64, i64) = s
            .eval(
                "local n,t,c,q,e = GetTradePlayerItemInfo(1)\n\
                 return n,t,c,q",
            )
            .unwrap();
        assert_eq!((name.as_str(), count, quality), ("Linen Cloth", 5, -1));
        assert_eq!(tex, "Interface\\Icons\\INV_Fabric_Linen_01");
        assert!(s
            .eval::<bool>("local n,t,c,q,e = GetTradePlayerItemInfo(1)\nreturn e == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(2) == nil")
            .unwrap());

        // The target side adds the enchant and the usable flag; the fourth is the quality, -1 for
        // linen cloth (InventoryType 0).
        let (name, _tex, count, quality, usable): (String, String, i64, i64, i64) =
            s.eval("return GetTradeTargetItemInfo(1)").unwrap();
        assert_eq!(
            (name.as_str(), count, quality, usable),
            ("Linen Cloth", 1, -1, 1)
        );
        // The enchant slot (7) is filled on the target side.
        assert!(s
            .eval::<bool>("return GetTradeTargetItemInfo(7) ~= nil")
            .unwrap());
    }

    /// The fourth value is the quality, through the item-info gate (`0x4c059c`..`0x4c05c1`): the
    /// template's quality for a cached, equippable item, else the number -1; five values on every
    /// slot, the fourth never nil and never a usable flag.
    #[test]
    fn player_item_quality_is_gated_on_the_inventory_type() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        let mut weapon = item(1234, 1, 4);
        weapon.inventory_type = 13;
        st.player.slots[1] = Some(weapon);
        // Equippable, but its template is still in flight.
        let mut pending = item(1235, 1, 0);
        pending.quality = None;
        pending.inventory_type = 0;
        st.player.slots[2] = Some(pending);
        s.set_trade(Some(st));
        for (slot, want) in [(1, "5:number:-1"), (2, "5:number:4"), (3, "5:number:-1")] {
            assert_eq!(
                s.eval::<String>(&format!(
                    "local c = function(...) return arg.n .. ':' .. type(arg[4]) .. ':' .. \
                     tostring(arg[4]) end return c(GetTradePlayerItemInfo({slot}))"
                ))
                .unwrap(),
                want,
                "slot {slot}"
            );
        }
    }

    /// The target side's quality goes through the same gate, and an empty slot pushes the six-value
    /// miss tuple (`0x4c02f6`): nil, nil, 0, 0, nil, nil.
    #[test]
    fn target_item_info_gates_its_quality_and_pushes_six_values_on_a_miss() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        let mut weapon = item(1234, 1, 3);
        weapon.inventory_type = 13;
        st.target.slots[1] = Some(weapon);
        s.set_trade(Some(st));
        let shape = |s: &UiScript, slot: u32| -> String {
            s.eval(&format!(
                "local c = function(...) local o = {{}} for i = 1, arg.n do o[i] = tostring(arg[i]) end \
                 return table.concat(o, ',') end return c(GetTradeTargetItemInfo({slot}))"
            ))
            .unwrap()
        };
        assert_eq!(
            shape(&s, 2),
            "Linen Cloth,Interface\\Icons\\INV_Fabric_Linen_01,1,3,1,nil"
        );
        assert_eq!(shape(&s, 3), "nil,nil,0,0,nil,nil");
        // A row whose template is still in flight takes the miss too (`0x4c019b`, `0x4c02f6`).
        let mut st = state();
        st.target.slots[3] = Some(TradeSlotItem {
            item_id: 4306,
            count: 1,
            ..Default::default()
        });
        s.set_trade(Some(st));
        assert_eq!(shape(&s, 4), "nil,nil,0,0,nil,nil");
        assert_eq!(shape(&s, 0), "nil,nil,0,0,nil,nil");
        assert_eq!(shape(&s, 9), "nil,nil,0,0,nil,nil");
    }

    /// The texture is a string for a filled slot: `0x5d8af0` falls back to the question mark while
    /// the template is unknown.
    #[test]
    fn player_item_texture_falls_back_to_the_question_mark() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        let mut pending = item(1235, 1, 0);
        pending.texture = None;
        st.player.slots[1] = Some(pending);
        s.set_trade(Some(st));
        for (slot, want) in [
            (1, "Interface\\Icons\\INV_Fabric_Linen_01"),
            (2, "Interface\\Icons\\INV_Misc_QuestionMark"),
        ] {
            assert_eq!(
                s.eval::<String>(&format!(
                    "local n, t = GetTradePlayerItemInfo({slot}) return type(t) .. ':' .. t"
                ))
                .unwrap(),
                format!("string:{want}"),
                "slot {slot}"
            );
        }
    }

    #[test]
    fn money_reads_the_pushed_state() {
        let mut s = UiScript::new().unwrap();
        s.set_trade(Some(state()));
        assert_eq!(
            s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(),
            12_345
        );
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 500);
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
