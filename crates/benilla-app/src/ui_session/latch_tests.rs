//! The latch over the real window cores: one window holds it, a new open closes the holder
//! first (`0x4930d0` → `0x4931c0` → `0x493310`), and a core entered directly while its window
//! holds the latch runs once more inside itself, so its `*_CLOSED` fires twice.

use bevy::prelude::*;

use super::{dispatch, register, InteractLatch, LatchApp};
use crate::net::NetCommands;
use crate::ui_bank::BankOpen;
use crate::ui_gossip::GossipState;
use crate::ui_merchant::MerchantOpen;
use crate::ui_quest::{QuestGiver, QuestView};
use crate::ui_script::sweep_test::{seen_in, spied_vm};
use crate::ui_stable::StableOpen;
use crate::ui_taxi::TaxiState;
use crate::ui_trainer::TrainerOpen;

const VENDOR: u64 = 0xF130_0000_0000_0001;
const BANKER: u64 = 0xF130_0000_0000_0002;
const NPC: u64 = 0xF130_0000_0000_0003;

const EVENTS: [&str; 7] = [
    "MERCHANT_CLOSED",
    "BANKFRAME_CLOSED",
    "GOSSIP_CLOSED",
    "QUEST_FINISHED",
    "TRAINER_CLOSED",
    "TAXIMAP_CLOSED",
    "PET_STABLE_CLOSED",
];

/// The windows on the latch with their cells, a wire, and a VM that records their closes.
fn harness() -> App {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(NetCommands(tx))
        .init_resource::<MerchantOpen>()
        .init_resource::<BankOpen>()
        .init_resource::<GossipState>()
        .init_resource::<QuestGiver>()
        .init_resource::<TrainerOpen>()
        .init_resource::<TaxiState>()
        .init_resource::<StableOpen>()
        .add_latched_window::<MerchantOpen>()
        .add_latched_window::<BankOpen>()
        .add_latched_window::<GossipState>()
        .add_latched_window::<QuestGiver>()
        .add_latched_window::<TrainerOpen>()
        .add_latched_window::<TaxiState>()
        .add_latched_window::<StableOpen>()
        .insert_non_send_resource(spied_vm(&EVENTS));
    app
}

fn latch(app: &App) -> Option<u64> {
    app.world().resource::<InteractLatch>().guid()
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

/// Each window's open as its packet handler runs it, on `npc`.
fn open(app: &mut App, window: super::NpcWindow, npc: u64) {
    use super::NpcWindow as W;
    let world = app.world_mut();
    match window {
        W::Merchant => crate::ui_merchant::open_merchant(world, npc, Vec::new()),
        W::Bank => crate::ui_bank::open_bank(world, npc),
        W::Gossip => {
            register(world, npc);
            world.resource_mut::<GossipState>().npc = Some(npc);
        }
        W::Quest => crate::ui_quest::open_panel(world, npc, greeting(npc)),
        W::Trainer => crate::ui_trainer::open_trainer(world, npc, 0, Vec::new(), String::new()),
        W::Taxi => crate::ui_taxi::open_map(world, npc, 1, Default::default()),
        W::Stable => crate::ui_stable::open_stable(world, npc, 0, Vec::new()),
        other => unreachable!("{other:?} is not in this harness"),
    }
}

/// Each window's close core, as its Lua verb or its server close enters it.
fn close(app: &mut App, window: super::NpcWindow) {
    use super::NpcWindow as W;
    let world = app.world_mut();
    match window {
        W::Merchant => crate::ui_merchant::close_core(world),
        W::Bank => crate::ui_bank::close_core(world),
        W::Gossip => crate::ui_gossip::close_core(world),
        W::Quest => crate::ui_quest::close_core(world, true),
        W::Trainer => crate::ui_trainer::close_core(world),
        W::Taxi => crate::ui_taxi::close_core(world),
        W::Stable => crate::ui_stable::close_core(world),
        other => unreachable!("{other:?} is not in this harness"),
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
    use super::NpcWindow as W;
    let windows = [
        W::Gossip,
        W::Quest,
        W::Merchant,
        W::Taxi,
        W::Trainer,
        W::Bank,
        W::Stable,
    ];
    for first in windows {
        for second in windows {
            for npc in [NPC, VENDOR] {
                let mut app = harness();
                open(&mut app, first, NPC);
                open(&mut app, second, npc);
                let world = app.world();
                let open_cells = [
                    world.resource::<GossipState>().npc,
                    world.resource::<QuestGiver>().npc,
                    world.resource::<MerchantOpen>().vendor,
                    world
                        .resource::<TaxiState>()
                        .open
                        .as_ref()
                        .map(|o| o.flightmaster),
                    world.resource::<TrainerOpen>().trainer,
                    world.resource::<BankOpen>().banker,
                    world.resource::<StableOpen>().npc,
                ]
                .into_iter()
                .flatten()
                .count();
                // The taxi map and the trainer and stable lists re-list in place on their own
                // NPC (`0x4db730`, `0x4d7459`, `0x4cadd6`). A map from another flight master
                // closes the open one through its core first (`0x4db73a`), while it still holds
                // the latch: twice. Every other pair is one close, through the dispatcher.
                let same = first == second;
                let expected = match (same, npc == NPC, first) {
                    (true, true, W::Taxi | W::Trainer | W::Stable) => 0,
                    (true, false, W::Taxi) => 2,
                    _ => 1,
                };
                let closes = seen_in(&app).len();
                assert_eq!(open_cells, 1, "{first:?} then {second:?} on {npc:#x}");
                assert_eq!(latch(&app), Some(npc), "{first:?} then {second:?}");
                assert_eq!(
                    closes,
                    expected,
                    "{first:?} then {second:?} on {npc:#x}: {:?}",
                    seen_in(&app)
                );
            }
        }
    }
}

/// A core entered directly (a Lua close verb, a server close, a leave-world leg) while its window
/// holds the latch releases it through `0x4931c0`, whose `0x493310` re-enters the same core while
/// the cell is still set: two `*_CLOSED`. Quest zeroes its cell first (`0x501201`): one.
#[test]
fn a_direct_close_of_the_latched_window_fires_its_event_twice_and_quest_once() {
    use super::NpcWindow as W;
    for (window, event, times) in [
        (W::Merchant, "MERCHANT_CLOSED", 2),
        (W::Bank, "BANKFRAME_CLOSED", 2),
        (W::Gossip, "GOSSIP_CLOSED", 2),
        (W::Trainer, "TRAINER_CLOSED", 2),
        (W::Taxi, "TAXIMAP_CLOSED", 2),
        (W::Stable, "PET_STABLE_CLOSED", 2),
        (W::Quest, "QUEST_FINISHED", 1),
    ] {
        let mut app = harness();
        open(&mut app, window, NPC);
        close(&mut app, window);
        assert_eq!(seen_in(&app), vec![event; times], "{window:?}");
        assert_eq!(latch(&app), None, "{window:?}");

        // Entered through `0x493310` (the watchdog, an open, the leave-world interaction close),
        // the latch is already clear: once.
        let mut app = harness();
        open(&mut app, window, NPC);
        dispatch(app.world_mut());
        assert_eq!(seen_in(&app), [event], "{window:?} through the dispatcher");
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
