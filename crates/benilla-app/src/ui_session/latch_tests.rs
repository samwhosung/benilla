//! The latch over the real window cores: one window holds it, a new open closes the holder
//! first (`0x4930d0` → `0x4931c0` → `0x493310`), and a core entered directly while its window
//! holds the latch runs once more inside itself, so its `*_CLOSED` fires twice.

use bevy::prelude::*;

use super::{dispatch, InteractLatch, LatchApp, LatchedWindow, NpcWindow as W};
use crate::net::NetCommands;
use crate::ui_auction::AuctionOpen;
use crate::ui_bank::BankOpen;
use crate::ui_gossip::GossipState;
use crate::ui_item_text::ItemTextOpen;
use crate::ui_mail::MailOpen;
use crate::ui_merchant::MerchantOpen;
use crate::ui_petition::GuildRegistrarState;
use crate::ui_quest::{QuestGiver, QuestView};
use crate::ui_script::sweep_test::{seen_in, spied_vm};
use crate::ui_stable::StableOpen;
use crate::ui_tabard::TabardOpen;
use crate::ui_taxi::TaxiState;
use crate::ui_trade::TradeSession;
use crate::ui_trainer::TrainerOpen;

const VENDOR: u64 = 0xF130_0000_0000_0001;
const BANKER: u64 = 0xF130_0000_0000_0002;
const NPC: u64 = 0xF130_0000_0000_0003;

/// The windows this harness opens, in `0x493310`'s order: every window on the latch.
const WINDOWS: [W; 13] = [
    W::Trade,
    W::ItemText,
    W::Gossip,
    W::Quest,
    W::Merchant,
    W::Taxi,
    W::Trainer,
    W::Bank,
    W::Registrar,
    W::Tabard,
    W::Mail,
    W::Auction,
    W::Stable,
];

/// Each window's close event.
fn event(window: W) -> &'static str {
    match window {
        W::Trade => "TRADE_CLOSED",
        W::ItemText => "ITEM_TEXT_CLOSED",
        W::Gossip => "GOSSIP_CLOSED",
        W::Quest => "QUEST_FINISHED",
        W::Merchant => "MERCHANT_CLOSED",
        W::Taxi => "TAXIMAP_CLOSED",
        W::Trainer => "TRAINER_CLOSED",
        W::Bank => "BANKFRAME_CLOSED",
        W::Registrar => "GUILD_REGISTRAR_CLOSED",
        W::Tabard => "CLOSE_TABARD_FRAME",
        W::Mail => "MAIL_CLOSED",
        W::Auction => "AUCTION_HOUSE_CLOSED",
        W::Stable => "PET_STABLE_CLOSED",
    }
}

/// The windows on the latch with their cells, a wire, and a VM that records their closes. Every
/// unit guid's entry is 0, which a type-9 template makes a book, so any NPC here can be read.
fn harness() -> App {
    harness_with_wire().0
}

fn harness_with_wire() -> (App, crossbeam_channel::Receiver<crate::net::ClientCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut books = crate::go_templates::GameObjectTemplates::default();
    let mut data = [0i32; 24];
    data[0] = 1;
    books.insert(0, 9, "Book".into(), &data);
    app.insert_resource(NetCommands(tx))
        .insert_resource(books)
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::items::Items>()
        .init_resource::<TradeSession>()
        .init_resource::<ItemTextOpen>()
        .add_latched_window::<TradeSession>()
        .add_latched_window::<ItemTextOpen>()
        .init_resource::<MerchantOpen>()
        .init_resource::<BankOpen>()
        .init_resource::<GossipState>()
        .init_resource::<QuestGiver>()
        .init_resource::<TrainerOpen>()
        .init_resource::<TaxiState>()
        .init_resource::<StableOpen>()
        .init_resource::<GuildRegistrarState>()
        .init_resource::<TabardOpen>()
        .init_resource::<MailOpen>()
        .init_resource::<AuctionOpen>()
        .add_latched_window::<MerchantOpen>()
        .add_latched_window::<BankOpen>()
        .add_latched_window::<GossipState>()
        .add_latched_window::<QuestGiver>()
        .add_latched_window::<TrainerOpen>()
        .add_latched_window::<TaxiState>()
        .add_latched_window::<StableOpen>()
        .add_latched_window::<GuildRegistrarState>()
        .add_latched_window::<TabardOpen>()
        .add_latched_window::<MailOpen>()
        .add_latched_window::<AuctionOpen>()
        .insert_non_send_resource(spied_vm(&WINDOWS.map(event)));
    (app, rx)
}

fn latch(app: &App) -> Option<u64> {
    app.world().resource::<InteractLatch>().guid()
}

/// The guid in each window's cell, in [`WINDOWS`]' order.
fn cells(app: &App) -> [Option<u64>; 13] {
    let world = app.world();
    let cell = |window: W| match window {
        W::Trade => world.resource::<TradeSession>().cell(),
        W::ItemText => world.resource::<ItemTextOpen>().object(),
        W::Gossip => world.resource::<GossipState>().npc,
        W::Quest => world.resource::<QuestGiver>().npc,
        W::Merchant => world.resource::<MerchantOpen>().vendor,
        W::Taxi => world
            .resource::<TaxiState>()
            .open
            .as_ref()
            .map(|o| o.flightmaster),
        W::Trainer => world.resource::<TrainerOpen>().trainer,
        W::Bank => world.resource::<BankOpen>().banker,
        W::Registrar => world.resource::<GuildRegistrarState>().npc(),
        W::Tabard => world.resource::<TabardOpen>().npc(),
        W::Mail => world.resource::<MailOpen>().mailbox,
        W::Auction => world.resource::<AuctionOpen>().auctioneer,
        W::Stable => world.resource::<StableOpen>().npc,
    };
    WINDOWS.map(cell)
}

fn greeting(npc: u64) -> QuestView {
    QuestView::Greeting(benilla_protocol::messages::QuestGiverList {
        npc,
        greeting: String::new(),
        emote_delay: 0,
        emote: 0,
        quests: Vec::new(),
    })
}

/// Each window's open as its packet handler or click runs it, on `npc`.
fn open(app: &mut App, window: W, npc: u64) {
    let world = app.world_mut();
    match window {
        W::Trade => {
            world.resource_mut::<TradeSession>().begin(npc);
            crate::ui_trade::open_window(world);
        }
        W::ItemText => crate::ui_item_text::read_now(world, npc),
        W::Merchant => crate::ui_merchant::open_merchant(world, npc, Vec::new()),
        W::Bank => crate::ui_bank::open_bank(world, npc),
        W::Gossip => crate::ui_gossip::open_menu(world, npc, 0, Vec::new(), Vec::new()),
        W::Quest => crate::ui_quest::open_panel(world, npc, greeting(npc)),
        W::Trainer => crate::ui_trainer::open_trainer(world, npc, 0, Vec::new(), String::new()),
        W::Taxi => crate::ui_taxi::open_map(world, npc, 1, Default::default()),
        W::Stable => crate::ui_stable::open_stable(world, npc, 0, Vec::new()),
        W::Registrar => {
            use crate::target::cursor_mode::npc_flags::{PETITIONER, TABARDDESIGNER};
            let list = benilla_protocol::messages::PetitionShowList {
                npc,
                entries: vec![benilla_protocol::messages::PetitionShowListEntry {
                    index: 1,
                    charter_entry: benilla_protocol::messages::CHARTER_ITEM_ENTRY,
                    charter_display_id: benilla_protocol::messages::CHARTER_DISPLAY_ID,
                    charter_cost: 1000,
                    entry_flags: 1,
                }],
            };
            crate::ui_petition::open_registrar(world, &list, Some(PETITIONER | TABARDDESIGNER));
        }
        W::Tabard => crate::ui_tabard::open_tabard(world, npc),
        W::Mail => crate::ui_mail::open_mailbox(world, npc),
        W::Auction => crate::ui_auction::open_auction(world, npc, 1),
    }
}

/// Each window's close core, as its Lua verb or its server close enters it.
fn close(app: &mut App, window: W) {
    let world = app.world_mut();
    match window {
        W::Trade => crate::ui_trade::close_core(world),
        W::ItemText => crate::ui_item_text::close_core(world),
        W::Merchant => crate::ui_merchant::close_core(world),
        W::Bank => crate::ui_bank::close_core(world),
        W::Gossip => crate::ui_gossip::close_core(world),
        W::Quest => crate::ui_quest::close_core(world, true),
        W::Trainer => crate::ui_trainer::close_core(world),
        W::Taxi => crate::ui_taxi::close_core(world),
        W::Stable => crate::ui_stable::close_core(world),
        W::Registrar => crate::ui_petition::close_core(world),
        W::Tabard => crate::ui_tabard::close_core(world),
        W::Mail => crate::ui_mail::close_core(world),
        W::Auction => crate::ui_auction::close_core(world),
    }
}

/// The issue's case: a vendor's list closes the open bank before the merchant opens, and a
/// banker's open closes the open vendor. Each close is entered through `0x493310`, which clears
/// the latch first, so its event fires once.
#[test]
fn opening_a_bank_or_a_vendor_closes_the_other_first() {
    let mut app = harness();
    crate::ui_bank::open_bank(app.world_mut(), BANKER);
    assert_eq!(latch(&app), Some(BANKER));

    crate::ui_merchant::open_merchant(app.world_mut(), VENDOR, Vec::new());
    assert_eq!(seen_in(&app), ["BANKFRAME_CLOSED"]);
    assert!(!app.world().resource::<BankOpen>().is_open());
    assert_eq!(app.world().resource::<MerchantOpen>().vendor, Some(VENDOR));
    assert_eq!(latch(&app), Some(VENDOR));

    crate::ui_bank::open_bank(app.world_mut(), BANKER);
    assert_eq!(seen_in(&app), ["BANKFRAME_CLOSED", "MERCHANT_CLOSED"]);
    assert_eq!(app.world().resource::<MerchantOpen>().vendor, None);
    assert!(app.world().resource::<BankOpen>().is_open());
    assert_eq!(latch(&app), Some(BANKER));
}

/// Every window on the latch closes whichever window holds it when it opens, on another NPC or on
/// the same one, so at most one cell is ever set.
#[test]
fn every_open_closes_the_holder_so_one_window_is_open() {
    for first in WINDOWS {
        for second in WINDOWS {
            // One trade over another needs a second `OPEN_WINDOW` while the first stands, which
            // no server sends; benilla's session keeps one partner for the pending guid and the
            // window's, so it cannot stage one.
            if first == W::Trade && second == W::Trade {
                continue;
            }
            for npc in [NPC, VENDOR] {
                let mut app = harness();
                open(&mut app, first, NPC);
                open(&mut app, second, npc);
                let what = format!("{first:?} then {second:?} on {npc:#x}: {:?}", seen_in(&app));
                let open_cells: Vec<W> = WINDOWS
                    .into_iter()
                    .zip(cells(&app))
                    .filter_map(|(w, c)| c.map(|_| w))
                    .collect();
                let same_npc = npc == NPC;
                // Reading the open object again is the toggle (`0x4e330c`): the release closes
                // the latched reader inside it, then the read fires its own close, and nothing
                // is latched.
                if first == W::ItemText && second == W::ItemText && same_npc {
                    assert_eq!(seen_in(&app), [event(first); 2], "{what}");
                    assert_eq!(open_cells, [], "{what}");
                    assert_eq!(latch(&app), None, "{what}");
                    continue;
                }
                assert_eq!(latch(&app), Some(npc), "{what}");
                // The reader writes its cell at the read (`0x4e3364`) and registers only once
                // the text resolves (`0x4e349a`): on the guid a later arm's window holds,
                // `0x493310` finds the reader's own cell first and closes it, the holder stays,
                // and `ITEM_TEXT_BEGIN` fires anyway (`0x4e34a4`). No book shares a guid with an
                // NPC window, so only the matrix stages it.
                if second == W::ItemText && same_npc && first > W::ItemText {
                    assert_eq!(seen_in(&app), [event(second)], "{what}");
                    assert_eq!(open_cells, [first], "{what}");
                    continue;
                }
                // The designer writes its cell before it registers (`0x4f5845`, `0x4f5882`): on the
                // NPC whose window holds the latch, `0x493310` finds the first cell holding it,
                // the designer's own when the holder comes after it in the test order.
                let tabard_first = second == W::Tabard && same_npc && first >= W::Tabard;
                if tabard_first {
                    assert_eq!(seen_in(&app), ["CLOSE_TABARD_FRAME"], "{what}");
                    let held: Vec<W> = if first == W::Tabard {
                        vec![]
                    } else {
                        vec![first]
                    };
                    assert_eq!(open_cells, held, "{what}");
                    continue;
                }
                assert_eq!(open_cells, [second], "{what}");
                // A menu, the taxi map and the trainer and stable lists re-list in place on their
                // own NPC (`0x4e1fb8`, `0x4db730`, `0x4d7459`, `0x4cadd6`). A map from another flight master
                // closes the open one through its core first (`0x4db73a`), while it still holds
                // the latch: twice. A designer from another vendor overwrites the cell before it
                // registers, so `0x493310` finds no cell holding the old vendor: none. Every other
                // pair is one close, through the dispatcher.
                let expected = match (first == second, same_npc, first) {
                    (true, true, W::Gossip | W::Taxi | W::Trainer | W::Stable) => vec![],
                    (true, false, W::Taxi) => vec![event(first); 2],
                    (true, false, W::Tabard) => vec![],
                    _ => vec![event(first)],
                };
                assert_eq!(seen_in(&app), expected, "{what}");
            }
        }
    }
}

/// A core entered directly (a Lua close verb, a server close, a leave-world leg) while its window
/// holds the latch releases it through `0x4931c0`, whose `0x493310` re-enters the same core while
/// the cell is still set: two `*_CLOSED`. Quest zeroes its cell first (`0x501201`): one.
#[test]
fn a_direct_close_of_the_latched_window_fires_its_event_twice_and_quest_once() {
    for window in WINDOWS {
        let times = if window == W::Quest { 1 } else { 2 };
        let mut app = harness();
        open(&mut app, window, NPC);
        close(&mut app, window);
        assert_eq!(seen_in(&app), vec![event(window); times], "{window:?}");
        assert_eq!(latch(&app), None, "{window:?}");
        assert_eq!(cells(&app), [None; 13], "{window:?}");

        // Entered through `0x493310` (the watchdog, an open, the leave-world interaction close),
        // the latch is already clear: once.
        let mut app = harness();
        open(&mut app, window, NPC);
        dispatch(app.world_mut());
        assert_eq!(
            seen_in(&app),
            [event(window)],
            "{window:?} through the dispatcher"
        );
    }
}

/// The stock frames answer `*_CLOSED` with `HideUIPanel`, whose `OnHide` calls the close verb.
/// The verb runs inside the fire, as the reference's does. From the X button (the frame already
/// hidden, its `OnHide` calling the verb) every core fires twice. From a walk-away the merchant's
/// verb finds its cell already clear (`0x4fae0e` clears before `0x4fae2c` fires), while the bank's
/// and the trainer's fire before they clear (`0x4f84df`, `0x4d7468`), so their verb fires again.
#[test]
fn a_stock_on_hide_close_inside_the_fire_runs_as_the_reference() {
    for (window, event, verb, walk_away_times) in [
        (
            super::NpcWindow::Merchant,
            "MERCHANT_CLOSED",
            "CloseMerchant",
            1,
        ),
        (
            super::NpcWindow::Bank,
            "BANKFRAME_CLOSED",
            "CloseBankFrame",
            2,
        ),
        (
            super::NpcWindow::Trainer,
            "TRAINER_CLOSED",
            "CloseTrainer",
            2,
        ),
    ] {
        for walk_away in [false, true] {
            let mut app = harness();
            app.world()
                .non_send_resource::<benilla_ui::script::UiScript>()
                .run(&format!(
                    "SHOWN = true local f = CreateFrame('Frame') f:RegisterEvent('{event}') \
                     f:SetScript('OnEvent', function() \
                       if SHOWN then SHOWN = false {verb}() end end)"
                ))
                .unwrap();
            open(&mut app, window, NPC);
            if walk_away {
                dispatch(app.world_mut());
            } else {
                // The X button: `HideUIPanel` hides the frame, and `OnHide` calls the verb.
                app.world()
                    .non_send_resource::<benilla_ui::script::UiScript>()
                    .run("SHOWN = false")
                    .unwrap();
                close(&mut app, window);
            }
            let times = if walk_away { walk_away_times } else { 2 };
            assert_eq!(
                seen_in(&app),
                vec![event; times],
                "{window:?}, walk-away {walk_away}"
            );
            assert_eq!(latch(&app), None);
            assert!(app
                .world()
                .non_send_resource::<benilla_ui::script::UiScript>()
                .errors()
                .is_empty());
        }
    }
}

/// A trade holds the latch like an NPC window: a vendor's list closes it through the trade arm
/// (`0x493365`), `TRADE_CLOSED` once and `CMSG_CANCEL_TRADE` once (`0x4bf527`); a trade's open
/// closes the vendor and sends nothing of its own.
#[test]
fn a_trade_and_an_npc_window_close_each_other() {
    use crate::net::ClientCommand;
    const PARTNER: u64 = 0x0000_0000_0000_0008;
    let (mut app, rx) = harness_with_wire();
    app.world_mut()
        .resource_mut::<TradeSession>()
        .begin(PARTNER);
    crate::ui_trade::open_window(app.world_mut());
    assert_eq!(latch(&app), Some(PARTNER));
    assert!(rx.try_iter().next().is_none(), "the open sends nothing");

    crate::ui_merchant::open_merchant(app.world_mut(), VENDOR, Vec::new());
    assert_eq!(seen_in(&app), ["TRADE_CLOSED"]);
    let sent: Vec<ClientCommand> = rx.try_iter().collect();
    assert!(
        matches!(sent.as_slice(), [ClientCommand::CancelTrade]),
        "{sent:?}"
    );
    assert!(!app.world().resource::<TradeSession>().is_open());
    assert_eq!(latch(&app), Some(VENDOR));

    app.world_mut()
        .resource_mut::<TradeSession>()
        .begin(PARTNER);
    crate::ui_trade::open_window(app.world_mut());
    assert_eq!(seen_in(&app), ["TRADE_CLOSED", "MERCHANT_CLOSED"]);
    assert!(
        rx.try_iter().next().is_none(),
        "the vendor's close sends nothing"
    );
    assert_eq!(latch(&app), Some(PARTNER));

    // `CloseTrade` while latched: two closes, each with its cancel.
    crate::ui_trade::close_core(app.world_mut());
    assert_eq!(
        seen_in(&app),
        [
            "TRADE_CLOSED",
            "MERCHANT_CLOSED",
            "TRADE_CLOSED",
            "TRADE_CLOSED"
        ]
    );
    let sent: Vec<ClientCommand> = rx.try_iter().collect();
    assert!(
        matches!(
            sent.as_slice(),
            [ClientCommand::CancelTrade, ClientCommand::CancelTrade]
        ),
        "{sent:?}"
    );
}

/// A book read during a trade registers at `0x4e349a`, whose `0x493310` takes the dispatcher's
/// trade arm (`0x493365`): `TRADE_CLOSED` and one `CMSG_CANCEL_TRADE` (`0x4bf527`), then
/// `ITEM_TEXT_BEGIN` (`0x4e34a4`).
#[test]
fn a_book_over_a_trade_cancels_it_before_the_begin() {
    use crate::net::ClientCommand;
    const PARTNER: u64 = 0x0000_0000_0000_0008;
    let (mut app, rx) = harness_with_wire();
    app.insert_non_send_resource(spied_vm(&["TRADE_CLOSED", "ITEM_TEXT_BEGIN"]));
    app.world_mut()
        .resource_mut::<TradeSession>()
        .begin(PARTNER);
    crate::ui_trade::open_window(app.world_mut());
    crate::ui_item_text::read_now(app.world_mut(), NPC);
    assert_eq!(seen_in(&app), ["TRADE_CLOSED", "ITEM_TEXT_BEGIN"]);
    let sent: Vec<ClientCommand> = rx.try_iter().collect();
    assert!(
        matches!(sent.as_slice(), [ClientCommand::CancelTrade]),
        "{sent:?}"
    );
    assert!(!app.world().resource::<TradeSession>().is_open());
    assert_eq!(latch(&app), Some(NPC));
}

/// A book read over a vendor closes the vendor before the reader registers (`0x4e349a`), and a
/// vendor's list over a begun book closes the book once.
#[test]
fn a_book_and_an_npc_window_close_each_other() {
    let mut app = harness();
    crate::ui_merchant::open_merchant(app.world_mut(), VENDOR, Vec::new());
    crate::ui_item_text::read_now(app.world_mut(), NPC);
    assert_eq!(seen_in(&app), ["MERCHANT_CLOSED"]);
    assert_eq!(latch(&app), Some(NPC));

    crate::ui_merchant::open_merchant(app.world_mut(), VENDOR, Vec::new());
    assert_eq!(seen_in(&app), ["MERCHANT_CLOSED", "ITEM_TEXT_CLOSED"]);
    assert_eq!(app.world().resource::<ItemTextOpen>().object(), None);
}
