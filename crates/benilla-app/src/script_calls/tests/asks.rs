//! The getters' lookups with a callback, each through the whole client's host: a getter that
//! misses asks the cache inside its call, so the query goes out ahead of the chunk's next verb
//! (`RandomRoll`, `0x48c832`), as the reference's lookup sends it on the miss (`0x556aa0`).

use benilla_ui::script::{TradeSideState, TradeSlotItem, TradeState, UiScript, UnitState};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::systems::{client, eval};
use crate::net::ClientCommand;

/// What went out, in order, of the kinds these chunks send.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { min, .. } => format!("roll {min}"),
                ClientCommand::NameQuery { guid } => format!("name {guid:#x}"),
                ClientCommand::ItemQuery { entry, .. } => format!("item {entry}"),
                ClientCommand::GuildQuery { guild_id } => format!("guild {guild_id}"),
                _ => return None,
            })
        })
        .collect()
}

fn vm(app: &mut App) -> Mut<'_, UiScript> {
    app.world_mut().non_send_resource_mut::<UiScript>()
}

/// `UnitName` on a unit with no object reads the player-name cache by guid with a callback
/// (`0x5171eb`), which sends `CMSG_NAME_QUERY` on the miss inside the call, once while it is in
/// flight.
#[test]
fn unit_name_asks_a_missed_name_inside_its_call() {
    let (mut app, rx) = client();
    vm(&mut app).set_unit(
        "target",
        Some(UnitState {
            guid: 0x77,
            ..Default::default()
        }),
    );
    eval::<()>(
        &mut app,
        r#"UnitName("target") RandomRoll(1, 2) UnitName("target")"#,
    );
    assert_eq!(sent(&rx), ["name 0x77", "roll 1"]);
}

/// A trade getter that finds a filled slot whose template the cache lacks asks the item cache
/// with the callback `0x4c0340` inside its call (`0x4c017f`, `0x4c0582`), so the query goes out
/// ahead of the chunk's next verb, and the callback waits for the record.
#[test]
fn a_trade_getter_asks_a_missed_template_inside_its_call() {
    let (mut app, rx) = client();
    let mut target = TradeSideState::default();
    target.slots[0] = Some(TradeSlotItem {
        item_id: 4306,
        count: 1,
        ..Default::default()
    });
    let mut player = TradeSideState::default();
    player.slots[1] = Some(TradeSlotItem {
        item_id: 2592,
        count: 1,
        ..Default::default()
    });
    vm(&mut app).set_trade(Some(TradeState { player, target }));
    eval::<()>(
        &mut app,
        "GetTradeTargetItemInfo(1) RandomRoll(1, 2) GetTradePlayerItemInfo(2)",
    );
    assert_eq!(sent(&rx), ["item 4306", "roll 1", "item 2592"]);
}

/// `GetGuildInfo` on a player whose guild the cache lacks makes the lookup with its callback
/// inside its call (`0x4c93d2`): `CMSG_GUILD_QUERY` goes out ahead of the chunk's next verb, once
/// while it is in flight.
#[test]
fn get_guild_info_asks_a_missed_guild_inside_its_call() {
    let (mut app, rx) = client();
    vm(&mut app).set_unit(
        "target",
        Some(UnitState {
            exists: true,
            has_object: true,
            is_player: true,
            guid: 0x77,
            guild_id: 9,
            ..Default::default()
        }),
    );
    eval::<()>(
        &mut app,
        r#"GetGuildInfo("target") RandomRoll(1, 2) GetGuildInfo("target")"#,
    );
    assert_eq!(sent(&rx), ["guild 9", "roll 1"]);
}
