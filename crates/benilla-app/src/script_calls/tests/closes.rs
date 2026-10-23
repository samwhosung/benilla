//! The NPC windows' Lua close verbs through the whole client's host: each runs its window's close
//! core inside its call, the latch's release (`0x4931c0`) and the core's signal included, so the
//! chunk's next line reads the window closed and the latch clear, and a core entered while its
//! window holds the latch runs once more inside the release (`0x493310`), its event twice.

use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::systems::{client, eval, listen, NPC};
use crate::net::ClientCommand;
use crate::ui_session::latch_tests::{latch, open};
use crate::ui_session::NpcWindow as W;

/// Each window this piece closes inside its verb: its close verb and the event its core signals.
const CLOSES: [(W, &str, &str); 10] = [
    (W::Trade, "CloseTrade", "TRADE_CLOSED"),
    (W::Gossip, "CloseGossip", "GOSSIP_CLOSED"),
    (W::Quest, "CloseQuest", "QUEST_FINISHED"),
    (W::Merchant, "CloseMerchant", "MERCHANT_CLOSED"),
    (W::Taxi, "CloseTaxiMap", "TAXIMAP_CLOSED"),
    (W::Trainer, "CloseTrainer", "TRAINER_CLOSED"),
    (W::Bank, "CloseBankFrame", "BANKFRAME_CLOSED"),
    (W::Mail, "CloseMail", "MAIL_CLOSED"),
    (W::Auction, "CloseAuctionHouse", "AUCTION_HOUSE_CLOSED"),
    (W::Stable, "ClosePetStables", "PET_STABLE_CLOSED"),
];

/// What went out of the kinds these chunks send, in order.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<&'static str> {
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { .. } => "roll",
                ClientCommand::CancelTrade => "cancel trade",
                ClientCommand::QueryNextMailTime => "next mail time",
                _ => return None,
            })
        })
        .collect()
}

/// Whether `window`'s cell is still set.
fn is_open(app: &App, window: W) -> bool {
    let world = app.world();
    match window {
        W::Trade => world.resource::<crate::ui_trade::TradeSession>().is_open(),
        W::Gossip => world.resource::<crate::ui_gossip::GossipState>().npc.is_some(),
        W::Quest => world.resource::<crate::ui_quest::QuestGiver>().npc.is_some(),
        W::Merchant => world
            .resource::<crate::ui_merchant::MerchantOpen>()
            .vendor
            .is_some(),
        W::Taxi => world.resource::<crate::ui_taxi::TaxiState>().open.is_some(),
        W::Trainer => world
            .resource::<crate::ui_trainer::TrainerOpen>()
            .trainer
            .is_some(),
        W::Bank => world.resource::<crate::ui_bank::BankOpen>().is_open(),
        W::Mail => world
            .resource::<crate::ui_mail::MailOpen>()
            .mailbox
            .is_some(),
        W::Auction => world
            .resource::<crate::ui_auction::AuctionOpen>()
            .auctioneer
            .is_some(),
        W::Stable => world.resource::<crate::ui_stable::StableOpen>().npc.is_some(),
        _ => unreachable!("{window:?} is not closed here"),
    }
}

/// Each close verb runs its core inside its call while its window holds the latch: the release
/// re-enters the core (`0x4931c0` → `0x493310`), so the event fires twice before the verb returns,
/// once for quest, whose core zeroes its cell before the release (`0x501201`). The next line reads
/// the count, and the window and the latch are clear before anything after the chunk runs. Each
/// core: `0x4bf4e0`, `0x4e1fa0`, `0x501130`, `0x4fadf0`, `0x4dc0f0`, `0x4d7440`, `0x4f84d0`,
/// `0x4acd50`, `0x4cd5f0`, `0x4cae10`.
#[test]
fn each_close_verb_signals_twice_inside_its_call_while_latched() {
    for (window, verb, event) in CLOSES {
        let (mut app, rx) = client();
        open(&mut app, window, NPC);
        assert_eq!(latch(&app), Some(NPC), "{window:?}");
        listen(&mut app, &[event], "");
        let n: i64 = eval(
            &mut app,
            &format!("{verb}() local n = N RandomRoll(1, 2) return n"),
        );
        let times = if window == W::Quest { 1 } else { 2 };
        assert_eq!(n, times, "{window:?}");
        assert_eq!(latch(&app), None, "{window:?}: the latch released inside the call");
        assert!(!is_open(&app, window), "{window:?}: the cell cleared inside the call");
        let wire = sent(&rx);
        let expected: &[&str] = match window {
            // Each run of the core sends its cancel, open or not (`0x4bf527`).
            W::Trade => &["cancel trade", "cancel trade", "roll"],
            _ => &["roll"],
        };
        assert_eq!(wire, expected, "{window:?}");
    }
}

/// A listener's send at a close's signal goes out where its signal ran, ahead of what the core
/// does next: `TRADE_CLOSED` (`0x4bf522`) runs its listeners before the cancel (`0x4bf527`), in
/// each run of the core.
#[test]
fn a_close_listener_sends_ahead_of_the_core_rest() {
    let (mut app, rx) = client();
    open(&mut app, W::Trade, NPC);
    listen(&mut app, &["TRADE_CLOSED"], "RandomRoll(1, 2)");
    eval::<()>(&mut app, "CloseTrade()");
    assert_eq!(
        sent(&rx),
        ["roll", "cancel trade", "roll", "cancel trade"]
    );
}

/// The stock frames answer `*_CLOSED` with `HideUIPanel`, whose `OnHide` calls the close verb.
/// The verb runs inside the fire, as the reference's does. From the X button (the frame already
/// hidden, its `OnHide` calling the verb) every core fires twice. From a walk-away the merchant's
/// verb finds its cell already clear (`0x4fae0e` clears before `0x4fae2c` fires), while the bank's
/// and the trainer's fire before they clear (`0x4f84df`, `0x4d7468`), so their verb fires again.
#[test]
fn a_stock_on_hide_close_inside_the_fire_runs_as_the_reference() {
    for (window, event, verb, walk_away_times) in [
        (W::Merchant, "MERCHANT_CLOSED", "CloseMerchant", 1),
        (W::Bank, "BANKFRAME_CLOSED", "CloseBankFrame", 2),
        (W::Trainer, "TRAINER_CLOSED", "CloseTrainer", 2),
        (W::Auction, "AUCTION_HOUSE_CLOSED", "CloseAuctionHouse", 2),
    ] {
        for walk_away in [false, true] {
            let (mut app, _rx) = client();
            eval::<()>(&mut app, "SHOWN = true");
            listen(
                &mut app,
                &[event],
                &format!("if SHOWN then SHOWN = false {verb}() end"),
            );
            open(&mut app, window, NPC);
            if walk_away {
                crate::ui_session::dispatch(app.world_mut());
            } else {
                // The X button: `HideUIPanel` hides the frame, and `OnHide` calls the verb.
                eval::<()>(&mut app, &format!("SHOWN = false {verb}()"));
            }
            let times = if walk_away { walk_away_times } else { 2 };
            assert_eq!(
                eval::<i64>(&mut app, "return N"),
                times,
                "{window:?}, walk-away {walk_away}"
            );
            assert_eq!(latch(&app), None);
            assert!(app.world().non_send_resource::<UiScript>().errors().is_empty());
        }
    }
}

/// `CloseTradeSkill()` (`0x4fd7f0` → `0x4fc870`) and `CloseCraft()` (`0x4f6c00` → `0x4f5fe0`)
/// zero the open marker inside their calls (`[0xbde040]`, `[0xbdcfb8]`), so the engine's book is
/// closed before the next line.
#[test]
fn the_profession_closes_zero_the_open_marker_inside_their_calls() {
    let (mut app, rx) = client();
    app.world_mut()
        .resource_mut::<crate::ui_tradeskill::TradeSkillOpen>()
        .line = Some(197);
    app.world_mut()
        .resource_mut::<crate::ui_craft::CraftOpen>()
        .line = Some(333);
    eval::<()>(&mut app, "CloseTradeSkill() CloseCraft() RandomRoll(1, 2)");
    assert_eq!(
        app.world()
            .resource::<crate::ui_tradeskill::TradeSkillOpen>()
            .line,
        None
    );
    assert_eq!(app.world().resource::<crate::ui_craft::CraftOpen>().line, None);
    assert_eq!(sent(&rx), ["roll"]);
}
