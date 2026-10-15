//! The money cursor, payload mode 2: coins taken off a money frame; the purse is never debited.

use mlua::{Lua, Value};

use super::{clear, clear_cursor, set_cursor, Clear, CursorMoney, CursorPayload};
use crate::script::binding_abi::{flag, number_arg};
use crate::script::tick::verb;
use crate::script::Model;

/// The coin icon for an amount of copper: `GetCoinIcon`'s table (`0x48d4e0`), thresholds signed.
/// The loot window's coin slot and the money cursor use it too.
pub fn coin_icon(copper: i64) -> &'static str {
    if copper < 10 {
        "Interface\\Icons\\INV_Misc_Coin_05"
    } else if copper < 100 {
        "Interface\\Icons\\INV_Misc_Coin_06"
    } else if copper < 1_000 {
        "Interface\\Icons\\INV_Misc_Coin_03"
    } else if copper < 10_000 {
        "Interface\\Icons\\INV_Misc_Coin_04"
    } else if copper < 100_000 {
        "Interface\\Icons\\INV_Misc_Coin_01"
    } else {
        "Interface\\Icons\\INV_Misc_Coin_02"
    }
}

/// Raise `PLAYER_MONEY`, which repaints the money frames less the held coins: the coin arm of
/// `ClearCursor` (`0x495248`) and the money setter's tail (`0x494d0a`).
pub(super) fn raise_player_money(model: &mut Model) {
    model.raise("PLAYER_MONEY", Vec::new());
}

pub(crate) fn cursor_money(model: &Model) -> u32 {
    match &model.cursor {
        Some(CursorPayload::Money(m)) => m.copper,
        _ => 0,
    }
}

/// Put coins on the cursor as `0x494cc0` does. Its inner `ClearCursor(1,1)` (`0x494cd9`) drops
/// whatever is held with that arm's events, an item's unlock or coins' `PLAYER_MONEY`, and fires
/// `CURSOR_UPDATE` (`0x4952cc`) on the empty cursor, so that handler sees no coins; then the coins
/// land and `PLAYER_MONEY` fires (`0x494d0a`), with no second `CURSOR_UPDATE`.
fn install_money(lua: &Lua, copper: u32) {
    set_cursor(lua, true, |_| {
        Some(CursorPayload::Money(CursorMoney { copper }))
    });
}

/// `PickupPlayerMoney` after the argument gate; each refusal is silent.
pub(crate) fn pickup_player_money(lua: &Lua, copper: u32) -> bool {
    let allowed = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        copper != 0 && model.player_control && u64::from(copper) <= model.money
    };
    if allowed {
        install_money(lua, copper);
    }
    allowed
}

/// `DropCursorMoney` (`0x48aba0`), `SetCursorMoney(0)`: its `ClearCursor(1,1)` fires the coin
/// arm's `PLAYER_MONEY`, then `CURSOR_UPDATE`, and no packet.
pub(crate) fn drop_cursor_money(model: &mut Model) -> bool {
    if !matches!(model.cursor, Some(CursorPayload::Money(_))) || !model.player_control {
        return false;
    }
    clear_cursor(model);
    true
}

/// `AddTradeMoney`, also run first by `ClickTradeButton`, `ClickTargetTradeButton` and the trade
/// window's open: the offer becomes offer plus cursor, if the purse covers it.
pub(crate) fn add_trade_money(model: &mut Model) -> bool {
    let copper = cursor_money(model);
    if copper == 0 {
        return false;
    }
    let offer = model.trade.as_ref().map_or(0, |t| t.player.gold);
    let total = u64::from(offer) + u64::from(copper);
    if total > model.money {
        return false;
    }
    let total = total as u32;
    model.trade_set_money.push(total);
    if let Some(t) = model.trade.as_mut() {
        t.player.gold = total;
    }
    // `ClearCursor(1,0)` (`0x4bfe4b`): no PLAYER_MONEY, the clear's own CURSOR_UPDATE.
    clear(model, Clear::NO_MONEY_EVENT);
    true
}

/// `PickupTradeMoney` after the argument gate: signed gates, then `CMSG_SET_TRADE_GOLD` (`0x11F`)
/// with the trimmed offer, then the coins onto the cursor.
pub(crate) fn pickup_trade_money(lua: &Lua, amount: i32) -> bool {
    let trimmed = verb(lua, |model| {
        let offer = model.trade.as_ref().map_or(0, |t| t.player.gold) as i32;
        if amount <= 0 || amount > offer {
            return false;
        }
        let left = (offer - amount) as u32;
        model.trade_set_money.push(left);
        if let Some(t) = model.trade.as_mut() {
            t.player.gold = left;
        }
        true
    });
    if trimmed {
        install_money(lua, amount as u32);
    }
    trimmed
}

impl crate::script::UiScript {
    /// The trade window's open (`SetTradePartner`, `0x4bf4e0`) folds held coins into the offer.
    pub fn fold_cursor_money_into_trade(&mut self) -> Option<u32> {
        let mut model = self.model_mut();
        add_trade_money(&mut model).then(|| model.trade.as_ref().map_or(0, |t| t.player.gold))
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "GetCursorMoney",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(cursor_money(&model)))
        })?,
    )?;
    g.set(
        "CursorHasMoney",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(matches!(model.cursor, Some(CursorPayload::Money(_)))))
        })?,
    )?;
    g.set(
        "PickupPlayerMoney",
        lua.create_function(|lua, amount: Value| {
            let n = number_arg(lua, amount, "Usage: PickupPlayerMoney(amount)")? as u32;
            pickup_player_money(lua, n);
            Ok(())
        })?,
    )?;
    g.set(
        "DropCursorMoney",
        lua.create_function(|lua, ()| {
            verb(lua, drop_cursor_money);
            Ok(())
        })?,
    )?;
    g.set(
        "AddTradeMoney",
        lua.create_function(|lua, ()| {
            verb(lua, add_trade_money);
            Ok(())
        })?,
    )?;
    g.set(
        "PickupTradeMoney",
        lua.create_function(|lua, amount: Value| {
            let n = number_arg(lua, amount, "Usage: PickupTradeMoney(amount)")?;
            pickup_trade_money(lua, n);
            Ok(())
        })?,
    )?;
    g.set(
        "GetCoinIcon",
        lua.create_function(|lua, amount: Value| {
            let n = number_arg(lua, amount, "Usage: GetCoinIcon(amount)")?;
            Ok(coin_icon(i64::from(n)))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::coin_icon;
    use crate::script::cursor::CursorPayload;
    use crate::script::tick::heard;
    use crate::script::UiScript;

    #[test]
    fn coins_ride_the_cursor_without_leaving_the_purse() {
        let mut s = UiScript::new().unwrap();
        s.set_money(12_345);
        assert!(s.eval::<bool>("return CursorHasMoney() == nil").unwrap());
        s.run("PickupPlayerMoney(2345)").unwrap();
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 2345);
        assert_eq!(s.eval::<i64>("return CursorHasMoney()").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return GetMoney() - GetCursorMoney()")
                .unwrap(),
            10_000
        );
        assert_eq!(s.eval::<i64>("return GetMoney()").unwrap(), 12_345);
        assert!(s.cursor_item().is_none(), "coins are not an item cursor");
        s.run("DropCursorMoney()").unwrap();
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetMoney()").unwrap(), 12_345);
    }

    #[test]
    fn the_pickup_gates_and_raises_as_the_reference_does() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        assert!(
            s.run("PickupPlayerMoney(nil)").is_err(),
            "a non-number raises"
        );
        assert!(s.run(r#"PickupPlayerMoney("x")"#).is_err());
        s.run(r#"PickupPlayerMoney(0) PickupPlayerMoney(1001) PickupPlayerMoney(-5)"#)
            .unwrap();
        assert_eq!(
            s.eval::<i64>("return GetCursorMoney()").unwrap(),
            0,
            "zero, more than the purse, and a negative (unsigned: huge) all refuse silently"
        );
        s.run(r#"PickupPlayerMoney("250")"#).unwrap();
        assert_eq!(
            s.eval::<i64>("return GetCursorMoney()").unwrap(),
            250,
            "a numeric string"
        );
        s.set_player_control(false);
        s.run("DropCursorMoney()").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetCursorMoney()").unwrap(),
            250,
            "neither pickup nor drop while control is lost"
        );
        s.set_player_control(true);
        s.run("DropCursorMoney()").unwrap();
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 0);
        s.run("DropCursorMoney()").unwrap();
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// The events fire inside each verb, in `0x494cc0`'s order: the inner clear's `CURSOR_UPDATE`
    /// reads an empty cursor, and only the install's `PLAYER_MONEY` reads the new coins.
    #[test]
    fn the_pickup_and_drop_fire_player_money_around_the_clear() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        heard::listen(
            &s,
            &["PLAYER_MONEY", "CURSOR_UPDATE"],
            Some("GetCursorMoney()"),
        );
        s.run("PickupPlayerMoney(10)").unwrap();
        assert_eq!(
            heard::take(&s),
            ["CURSOR_UPDATE 0", "PLAYER_MONEY 10"],
            "the inner clear's CURSOR_UPDATE, then the money's PLAYER_MONEY"
        );
        s.run("PickupPlayerMoney(20)").unwrap();
        assert_eq!(
            heard::take(&s),
            ["PLAYER_MONEY 0", "CURSOR_UPDATE 0", "PLAYER_MONEY 20"],
            "coins already held: the mode-2 arm's PLAYER_MONEY first"
        );
        s.run("DropCursorMoney()").unwrap();
        assert_eq!(heard::take(&s), ["PLAYER_MONEY 0", "CURSOR_UPDATE 0"]);
        assert!(s.model_ref().pending_events.is_empty());
    }

    /// A handler that calls another verb runs it to completion inside the dispatch, as the
    /// reference's synchronous `SignalEvent` nests: the mail verb's event lands between the
    /// pickup's two steps, and a pickup called from the install's `PLAYER_MONEY` runs both of its
    /// own steps before the outer verb returns. No step waits on a held model borrow.
    #[test]
    fn a_verb_called_from_a_handler_nests_inside_the_dispatch() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        let log: Vec<String> = s
            .eval(
                r#"LOG = {} NEST = 0
                   local f = CreateFrame("Frame")
                   f:RegisterEvent("CURSOR_UPDATE")
                   f:RegisterEvent("PLAYER_MONEY")
                   f:RegisterEvent("SEND_MAIL_MONEY_CHANGED")
                   f:SetScript("OnEvent", function()
                       table.insert(LOG, event .. " " .. GetCursorMoney() .. " " .. GetSendMailMoney())
                       if event == "CURSOR_UPDATE" and NEST == 0 then
                           NEST = 1 SetSendMailMoney(5)
                       elseif event == "PLAYER_MONEY" and NEST == 1 then
                           NEST = 2 PickupPlayerMoney(20)
                       end
                   end)
                   PickupPlayerMoney(10)
                   return LOG"#,
            )
            .unwrap();
        assert_eq!(
            log,
            [
                "CURSOR_UPDATE 0 0",
                "SEND_MAIL_MONEY_CHANGED 0 5",
                "PLAYER_MONEY 10 5",
                "PLAYER_MONEY 0 5",
                "CURSOR_UPDATE 0 5",
                "PLAYER_MONEY 20 5",
            ]
        );
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 20);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        assert!(s.model_ref().pending_events.is_empty());
    }

    #[test]
    fn the_trade_pair_moves_coins_between_the_cursor_and_the_offer() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        s.open_trade_window();
        s.set_trade(Some(crate::script::TradeState::default()));
        s.run("PickupPlayerMoney(300) AddTradeMoney()").unwrap();
        assert_eq!(s.take_trade_money(), [300], "an ABSOLUTE offer + cursor");
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(), 300);
        // The purse cannot cover offer + cursor: refused, the coins stay held.
        s.run("PickupPlayerMoney(800) AddTradeMoney()").unwrap();
        assert!(s.take_trade_money().is_empty());
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 800);
        s.run("DropCursorMoney()").unwrap();
        // Back off the offer: signed gates, the trimmed offer on the wire, coins on the cursor.
        assert!(s.run("PickupTradeMoney(nil)").is_err());
        s.run("PickupTradeMoney(0) PickupTradeMoney(301) PickupTradeMoney(-1)")
            .unwrap();
        assert!(s.take_trade_money().is_empty());
        s.run("PickupTradeMoney(100)").unwrap();
        assert_eq!(s.take_trade_money(), [200]);
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 100);
        assert!(matches!(s.cursor_payload(), Some(CursorPayload::Money(_))));
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    #[test]
    fn the_coin_icon_table() {
        assert!(s_icon(-3).ends_with("_05"));
        assert!(s_icon(9).ends_with("_05"));
        assert!(s_icon(10).ends_with("_06"));
        assert!(s_icon(999).ends_with("_03"));
        assert!(s_icon(1_000).ends_with("_04"));
        assert!(s_icon(99_999).ends_with("_01"));
        assert!(s_icon(100_000).ends_with("_02"));
        let s = UiScript::new().unwrap();
        assert!(s.run("GetCoinIcon(nil)").is_err(), "a non-number raises");
        assert_eq!(
            s.eval::<String>("return GetCoinIcon(12345)").unwrap(),
            "Interface\\Icons\\INV_Misc_Coin_01"
        );
    }

    fn s_icon(n: i64) -> &'static str {
        coin_icon(n)
    }
}
