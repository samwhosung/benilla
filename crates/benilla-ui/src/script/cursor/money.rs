//! The money cursor, payload mode 2: coins taken off a money frame; the purse is never debited.
//! Deviation: a `CURSOR_UPDATE` handler already sees the coins, because events are queued; the
//! reference runs it inside the clear, before they land.

use mlua::{Lua, Value};

use super::{queue_cursor_update, CursorMoney, CursorPayload};
use crate::script::binding_abi::{flag, number_arg};
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

pub(crate) fn cursor_money(model: &Model) -> u32 {
    match &model.cursor {
        Some(CursorPayload::Money(m)) => m.copper,
        _ => 0,
    }
}

/// Queue `PLAYER_MONEY`, which repaints the money frames less the held coins.
fn queue_player_money(model: &mut Model) {
    model
        .pending_events
        .push(("PLAYER_MONEY".to_string(), Vec::new()));
}

/// Put coins on the cursor as `0x494cc0` does: the inner clear's `CURSOR_UPDATE`, after its own
/// `PLAYER_MONEY` if coins were already held, then the payload and `PLAYER_MONEY`.
fn install_money(model: &mut Model, copper: u32) {
    if matches!(model.cursor, Some(CursorPayload::Money(_))) {
        queue_player_money(model);
    }
    model.cursor = None;
    queue_cursor_update(model);
    model.cursor = Some(CursorPayload::Money(CursorMoney { copper }));
    queue_player_money(model);
}

/// `PickupPlayerMoney` after the argument gate; each refusal is silent.
pub(crate) fn pickup_player_money(model: &mut Model, copper: u32) -> bool {
    if copper == 0 || !model.player_control || u64::from(copper) > model.money {
        return false;
    }
    install_money(model, copper);
    true
}

/// `DropCursorMoney`: `PLAYER_MONEY`, then the clear's `CURSOR_UPDATE`, and no packet.
pub(crate) fn drop_cursor_money(model: &mut Model) -> bool {
    if !matches!(model.cursor, Some(CursorPayload::Money(_))) || !model.player_control {
        return false;
    }
    model.cursor = None;
    queue_player_money(model);
    queue_cursor_update(model);
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
    model.trade_set_money = Some(total);
    if let Some(t) = model.trade.as_mut() {
        t.player.gold = total;
    }
    // `ClearCursor(1, 0)`: no PLAYER_MONEY, the clear's own CURSOR_UPDATE.
    model.cursor = None;
    queue_cursor_update(model);
    true
}

/// `PickupTradeMoney` after the argument gate: signed gates, then `CMSG_SET_TRADE_GOLD` (`0x11F`)
/// with the trimmed offer, then the coins onto the cursor.
pub(crate) fn pickup_trade_money(model: &mut Model, amount: i32) -> bool {
    let offer = model.trade.as_ref().map_or(0, |t| t.player.gold) as i32;
    if amount <= 0 || amount > offer {
        return false;
    }
    let left = (offer - amount) as u32;
    model.trade_set_money = Some(left);
    if let Some(t) = model.trade.as_mut() {
        t.player.gold = left;
    }
    install_money(model, amount as u32);
    true
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
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            pickup_player_money(&mut model, n);
            Ok(())
        })?,
    )?;
    g.set(
        "DropCursorMoney",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            drop_cursor_money(&mut model);
            Ok(())
        })?,
    )?;
    g.set(
        "AddTradeMoney",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            add_trade_money(&mut model);
            Ok(())
        })?,
    )?;
    g.set(
        "PickupTradeMoney",
        lua.create_function(|lua, amount: Value| {
            let n = number_arg(lua, amount, "Usage: PickupTradeMoney(amount)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            pickup_trade_money(&mut model, n);
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
    use crate::script::UiScript;

    fn events(s: &mut UiScript) -> Vec<String> {
        s.run(
            r#"EV = {} F = F or CreateFrame("Frame") F:RegisterEvent("PLAYER_MONEY")
               F:RegisterEvent("CURSOR_UPDATE") F:SetScript("OnEvent", function() table.insert(EV, event) end)"#,
        )
        .unwrap();
        s.tick(0.0);
        s.eval::<Vec<String>>("return EV").unwrap()
    }

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

    #[test]
    fn the_pickup_and_drop_fire_player_money_around_the_clear() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        let _ = events(&mut s);
        s.run("PickupPlayerMoney(10)").unwrap();
        assert_eq!(
            events(&mut s),
            vec!["CURSOR_UPDATE", "PLAYER_MONEY"],
            "the inner clear's CURSOR_UPDATE, then the money's PLAYER_MONEY"
        );
        s.run("PickupPlayerMoney(20)").unwrap();
        assert_eq!(
            events(&mut s),
            vec!["PLAYER_MONEY", "CURSOR_UPDATE", "PLAYER_MONEY"],
            "coins already held: the mode-2 arm's PLAYER_MONEY first"
        );
        s.run("DropCursorMoney()").unwrap();
        assert_eq!(events(&mut s), vec!["PLAYER_MONEY", "CURSOR_UPDATE"]);
    }

    #[test]
    fn the_trade_pair_moves_coins_between_the_cursor_and_the_offer() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        s.set_trade(Some(crate::script::TradeState {
            partner_name: Some("Bob".into()),
            ..Default::default()
        }));
        s.run("PickupPlayerMoney(300) AddTradeMoney()").unwrap();
        assert_eq!(
            s.take_trade_money(),
            Some(300),
            "an ABSOLUTE offer + cursor"
        );
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(), 300);
        // The purse cannot cover offer + cursor: refused, the coins stay held.
        s.run("PickupPlayerMoney(800) AddTradeMoney()").unwrap();
        assert_eq!(s.take_trade_money(), None);
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 800);
        s.run("DropCursorMoney()").unwrap();
        // Back off the offer: signed gates, the trimmed offer on the wire, coins on the cursor.
        assert!(s.run("PickupTradeMoney(nil)").is_err());
        s.run("PickupTradeMoney(0) PickupTradeMoney(301) PickupTradeMoney(-1)")
            .unwrap();
        assert_eq!(s.take_trade_money(), None);
        s.run("PickupTradeMoney(100)").unwrap();
        assert_eq!(s.take_trade_money(), Some(200));
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
