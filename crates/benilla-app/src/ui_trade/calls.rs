//! The trade verbs' engine half, which the call host applies inside each call
//! ([`crate::script_calls::ScriptCallHost`]): each sends its packet and changes the trade's state
//! before the verb returns, as the reference's bindings do in their own bodies (`0x48a120`,
//! `0x4bf230`, `0x48aa60`, `0x48aa70`, the money senders `0x5d4500`, `0x5d45f0` and `0x5d4680`,
//! and `0x4bfa70`'s sends `0x5d4380` and `0x5d4480`). The enchant casts at the partner's slot 7
//! are the targeting cursor's ([`crate::spell::ScriptCursor::trade_enchant`]).

use benilla_ui::script::{ScriptValue, TradeCall, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{TradeSession, ENCHANT_SLOT};
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, Objects};
use crate::ui_action::{UiError, UiErrorKeys};

/// The trade session and what its verbs send through.
#[derive(SystemParam)]
pub(crate) struct TradeSteps<'w, 's> {
    /// Absent in a world with no trade window, which drops the calls.
    trade: Option<ResMut<'w, TradeSession>>,
    net: Res<'w, NetCommands>,
    errors: Res<'w, UiErrorKeys>,
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
}

impl TradeSteps<'_, '_> {
    /// Apply one trade call.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: TradeCall) {
        let Self {
            trade,
            net,
            errors,
            objects,
            items,
        } = self;
        let Some(trade) = trade.as_deref_mut() else {
            debug!("ui_trade: no trade window in this world; {call:?} dropped");
            return;
        };
        match call {
            // The dead test reads the active player's own object (`0x48a152`-`0x48a16e`), and
            // with none there is nothing to refuse.
            TradeCall::Initiate(guid) => {
                if objects
                    .player_fields()
                    .is_some_and(|f| f.is_dead_or_ghost())
                {
                    errors.push(UiError::key("ERR_PLAYER_DEAD"));
                    return;
                }
                info!(target: "trade", "initiate {guid:#x}; sending CMSG_INITIATE_TRADE");
                trade.initiate(guid, false);
                let _ = net.0.send(ClientCommand::InitiateTrade { target: guid });
            }
            // `0x4bf230`: its gate's tag 5 always passes (`0x4bf233`), an unchanged flag does
            // nothing (`0x4bf247`), and a change signals both flags, ours first (`0x4bf260`), then
            // sends (`0x4bf26d`, `0x4bf272`). Nothing tests an open window.
            TradeCall::Accept(flag) => {
                if !trade.set_our_accept(flag) {
                    return;
                }
                let (mine, theirs) = trade.accepts();
                script.fire_event(
                    "TRADE_ACCEPT_UPDATE",
                    vec![
                        ScriptValue::Int(i64::from(mine)),
                        ScriptValue::Int(i64::from(theirs)),
                    ],
                );
                let _ = net.0.send(if flag {
                    ClientCommand::AcceptTrade
                } else {
                    ClientCommand::UnacceptTrade
                });
            }
            // The `TRADE` popup's verbs: an empty `0x117` or `0x11C` and no local teardown; the
            // status reply drives the window.
            TradeCall::Begin => {
                let _ = net.0.send(ClientCommand::BeginTrade);
            }
            TradeCall::Cancel => {
                let _ = net.0.send(ClientCommand::CancelTrade);
            }
            // No money sender tests the window; vmangos drops one with no trade
            // (`TradeHandler.cpp:731-735`) and echoes ours to the partner only, so our own gold is
            // set here.
            TradeCall::Gold(copper) => {
                trade.set_own_gold(copper);
                info!(target: "trade", "set gold: {copper} copper; sending CMSG_SET_TRADE_GOLD");
                trade.send_edit(net, ClientCommand::SetTradeGold { copper });
            }
            TradeCall::SetItem {
                slot,
                guid,
                bag,
                bag_slot,
            } => {
                if !trade.send_placement(net, objects, items, slot, guid, bag, bag_slot) {
                    script.refuse_trade_send();
                }
            }
            TradeCall::ClearItem(slot) => {
                trade.clear_own_item(u32::from(slot) + 1);
                info!(target: "trade", "clear item: slot {}; sending CMSG_CLEAR_TRADE_ITEM", slot + 1);
                trade.send_edit(net, ClientCommand::ClearTradeItem { trade_slot: slot });
            }
            TradeCall::TargetEnchant | TradeCall::ReplaceEnchant => {
                debug!("ui_trade: {call:?} is the targeting cursor's, not the trade's");
            }
        }
    }

    /// The partner's slot 7 as the trade bind and its clash check read it: `Some` with the record's
    /// permanent enchant (`0xb715d8`) when it holds an item (`0xb7150c`, `0x496170`, `0x6e690c`).
    pub(crate) fn partner_enchant_slot(&self) -> Option<u32> {
        self.trade
            .as_deref()?
            .partner_slot(ENCHANT_SLOT)
            .map(|item| item.perm_enchant)
    }
}
