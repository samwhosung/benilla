//! The auction verbs' engine half, which the call host applies inside each call
//! ([`crate::script_calls::ScriptCallHost`]): the gates on the auctioneer and the session's flags,
//! the refusals' lines and the packets, all before the verb returns, as the reference's bindings
//! do them in their own bodies (`0x4ce770`, `0x4ce980`, `0x4cec80`, `0x4cecd0`, `0x4cf610`,
//! `0x4d0260`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::messages::auction_filter;
use benilla_ui::script::{cancel_fee, AuctionBid, AuctionCall, AuctionQuery, BidVerdict, UiScript};

use super::{AuctionOpen, QUERY_THROTTLE_SECS};
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::Ask;
use crate::ui_action::{show_messages, ui_error_text, MessageSink, Shown, UiError};
use crate::ui_items::{count_of, InventoryScope};

/// The state the auction verbs read and write: the house's session, the socket, the player's
/// purse and bags, and where a refusal's line goes.
#[derive(SystemParam)]
pub(crate) struct AuctionVerbs<'w, 's> {
    /// Absent in a world with no auction house, which drops the calls.
    auction: Option<ResMut<'w, AuctionOpen>>,
    commands: Res<'w, NetCommands>,
    /// The browse throttle's clock ([`AuctionOpen::query_gate`]).
    time: Res<'w, Time>,
    items: Res<'w, Items>,
    objects: crate::net::Objects<'w, 's>,
    sink: MessageSink<'w>,
}

impl AuctionVerbs<'_, '_> {
    /// Apply one auction call.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: AuctionCall) {
        let Self {
            auction,
            commands,
            time,
            items,
            objects,
            sink,
        } = self;
        let Some(auction) = auction.as_deref_mut() else {
            debug!("ui_auction: no auction house in this world; {call:?} dropped");
            return;
        };
        match call {
            AuctionCall::Query(query) => {
                send_query(auction, commands, time.elapsed_secs_f64(), script, query);
            }
            // The binding returns with no auctioneer or a clear flag (`0x4cec8e`, `0x4cec97`),
            // and its sender tests the flag again (`0x4cd686`).
            AuctionCall::OwnerList { offset } => {
                if auction.auctioneer.is_some() {
                    if let Some(request) = auction.owner_list(offset) {
                        let _ = commands.0.send(request);
                    }
                }
            }
            AuctionCall::BidderList { offset } => {
                if auction.auctioneer.is_some() {
                    if let Some(request) = auction.bidder_list(offset) {
                        let _ = commands.0.send(request);
                    }
                }
            }
            AuctionCall::Bid(bid) => {
                let refusal = place_bid(auction, commands, items, objects, &bid);
                if let Some(key) = refusal {
                    show_error(script, sink, key);
                }
            }
            AuctionCall::Cancel { auction_id, bid } => {
                // No auctioneer, nothing (`0x4d02a5`).
                let Some(auctioneer) = auction.auctioneer else {
                    return;
                };
                // With a bid the player pays 5% of it; no player is the error at once
                // (`0x4d0301`).
                if bid != 0 {
                    let money = objects
                        .player_fields()
                        .map(|p| p.player_money().unwrap_or(0));
                    if money.is_none_or(|m| m < cancel_fee(bid)) {
                        show_error(script, sink, "ERR_NOT_ENOUGH_MONEY");
                        return;
                    }
                }
                let _ = commands.0.send(ClientCommand::AuctionRemoveItem {
                    auctioneer,
                    auction_id,
                });
            }
            AuctionCall::Start(start) => {
                // No auctioneer, nothing, ahead of the item's checks (`0x4ce7da`).
                let Some(auctioneer) = auction.auctioneer else {
                    return;
                };
                match start {
                    Err(key) => show_error(script, sink, key),
                    Ok(req) => {
                        let _ = commands.0.send(ClientCommand::AuctionSellItem {
                            auctioneer,
                            item_guid: req.item_guid,
                            bid: req.min_bid,
                            buyout: req.buyout,
                            etime_minutes: req.duration,
                        });
                        // The sell slot's clear follows the send whatever the server answers
                        // (`0x4ce92a` → `0x4cd640`): the unlock and `NEW_AUCTION_UPDATE` fire in
                        // the call.
                        script.clear_auction_sell_item();
                    }
                }
            }
        }
    }
}

/// `QueryAuctionItems` past its arguments: with an auctioneer (`0x4ce99c`) and the throttle open
/// (`0x4ce9a2`-`0x4ce9c0`), the packet, then the throttle armed five seconds out (`0x4cec42`-
/// `0x4cec4c`), which `CanSendAuctionQuery` reads on the script's next line.
fn send_query(
    auction: &mut AuctionOpen,
    commands: &NetCommands,
    now: f64,
    script: &mut UiScript,
    q: AuctionQuery,
) {
    let Some(auctioneer) = auction.auctioneer else {
        return;
    };
    if auction.query_gate.is_some_and(|gate| now < gate) {
        return;
    }
    let _ = commands.0.send(ClientCommand::AuctionListItems {
        auctioneer,
        list_from: q.list_from,
        searched_name: q.name,
        level_min: q.min_level,
        level_max: q.max_level,
        slot_id: q.inv_type.unwrap_or(auction_filter::ANY),
        // Already ids: the binding maps menu positions as `0x4ce980` does.
        main_category: q.class.unwrap_or(auction_filter::ANY),
        sub_category: q.sub_class.unwrap_or(auction_filter::ANY),
        quality: q.quality.unwrap_or(auction_filter::ANY),
        usable: u8::from(q.usable_only),
    });
    auction.query_gate = Some(now + QUERY_THROTTLE_SECS);
    auction.wire.browse_sent += 1;
    script.set_auction_can_query(false);
}

/// `PlaceAuctionBid` past its row: nothing with no player or no auctioneer (`0x4cf740`,
/// `0x4cf75c`), else [`AuctionBid::verdict`] and its send, the bid remembered (`0x4cf91e`). The
/// refusal's line, if any.
fn place_bid(
    auction: &mut AuctionOpen,
    commands: &NetCommands,
    items: &Items,
    objects: &crate::net::Objects,
    bid: &AuctionBid,
) -> Option<&'static str> {
    let player = objects.player_fields()?;
    let auctioneer = auction.auctioneer?;
    let money = player.player_money().unwrap_or(0);
    // The record with no callback (`0x4cf879`), and the carried count with scope `0x4f`
    // (`0x4cf892`), equipment, bags, backpack, bank and keyring.
    let unique = || {
        let max = items.template(bid.item_id, 0, Ask::Read)?.max_count as i32;
        let carried = count_of(player, objects, bid.item_id, InventoryScope::QUEST_ITEMS);
        Some((max, carried as i32))
    };
    match bid.verdict(money, unique) {
        BidVerdict::Send(amount) => {
            let _ = commands.0.send(ClientCommand::AuctionPlaceBid {
                auctioneer,
                auction_id: bid.auction_id,
                price: amount,
            });
            auction.last_bid = amount;
            None
        }
        BidVerdict::Refuse(key) => Some(key),
        BidVerdict::Drop => None,
    }
}

/// A refusal's red line, shown inside the call under its input events (`0x496720`).
fn show_error(script: &mut UiScript, sink: &mut MessageSink, key: &'static str) {
    let input = script.input_events();
    let line = {
        let get = |k: &str| script.lua().globals().get::<String>(k).ok();
        ui_error_text(&UiError::key(key), &get).map(|text| Shown::keyed(key, text).under(input))
    };
    show_messages(script, sink, "ui_auction", line);
}
