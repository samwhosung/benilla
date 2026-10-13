//! The leave-world sweep over every leg benilla models, each registered by its window's own
//! plugin: every window open, one sweep, and the VM hears the closes in `0x490a80`'s call order,
//! `PLAYER_LEAVING_WORLD` after them, the interaction close after that and the cinematic's drop
//! last.

use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};

use super::sweep_test::{seen_in, spied_vm, sweep_in_place};

const CORPSE: u64 = 0xF130_0000_0000_0042;
const LETTER: u64 = 0x4000_0000_0000_0a11;
const GOSSIPER: u64 = 0xF130_0000_0000_0001;
/// A player sharing a quest, so the quest leg owes the decline.
const SHARER: u64 = 0x0000_0000_0000_0007;
const FLIGHT_MASTER: u64 = 0xF130_0000_0000_0002;
const TRAINER: u64 = 0xF130_0000_0000_0003;
const VENDOR: u64 = 0xF130_0000_0000_0004;
const PARTNER: u64 = 0x0000_0000_0000_0008;
const TABARD_VENDOR: u64 = 0xF130_0000_0000_0005;
const REGISTRAR: u64 = 0xF130_0000_0000_0006;
const MAILBOX: u64 = 0xF110_0000_0000_0B0C;
const AUCTIONEER: u64 = 0xF130_0000_0000_0009;
const STABLE_MASTER: u64 = 0xF130_0000_0000_000A;
const BANKER: u64 = 0xF130_0000_0000_000B;

/// Every event a leg can fire, in no particular order.
const EVENTS: [&str; 18] = [
    "LOOT_CLOSED",
    "ITEM_TEXT_CLOSED",
    "GOSSIP_CLOSED",
    "QUEST_FINISHED",
    "TAXIMAP_CLOSED",
    "TRAINER_CLOSED",
    "MERCHANT_CLOSED",
    "TRADE_CLOSED",
    "DUEL_FINISHED",
    "CLOSE_TABARD_FRAME",
    "GUILD_REGISTRAR_CLOSED",
    "MAIL_CLOSED",
    "AUCTION_HOUSE_CLOSED",
    "PET_STABLE_CLOSED",
    "PLAYER_LEAVING_WORLD",
    "BANKFRAME_CLOSED",
    "PETITION_CLOSED",
    "CINEMATIC_STOP",
];

/// The window plugins, which register the legs, over a world holding what the legs read.
fn legs_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.add_plugins((
        crate::ui_loot::UiLootPlugin,
        crate::ui_item_text::UiItemTextPlugin,
        crate::ui_gossip::UiGossipPlugin,
        crate::ui_quest::UiQuestPlugin,
        crate::ui_taxi::UiTaxiPlugin,
        crate::ui_trainer::UiTrainerPlugin,
        crate::ui_merchant::UiMerchantPlugin,
        crate::ui_trade::UiTradePlugin,
    ))
    .add_plugins((
        crate::ui_duel::UiDuelPlugin,
        crate::ui_tabard::TabardUiPlugin,
        crate::ui_petition::UiPetitionPlugin,
        crate::ui_mail::UiMailPlugin,
        crate::ui_auction::UiAuctionPlugin,
        crate::ui_stable::UiStablePlugin,
        crate::ui_bank::UiBankPlugin,
        crate::cinematic::CinematicPlugin,
    ))
    .insert_resource(NetCommands(tx))
    .init_resource::<crate::net::GuidIndex>()
    .init_resource::<crate::screen_fade::ScreenFade>()
    .init_resource::<crate::items::Items>()
    .add_message::<crate::sound::LootPickupSound>()
    .add_message::<crate::target::DeselectGuid>();
    (app, rx)
}

/// Open every window a leg closes.
fn open_everything(world: &mut World) {
    world
        .resource_mut::<crate::ui_loot::LootState>()
        .open(CORPSE, 1, 0, Vec::new());
    world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(CORPSE);
    world
        .resource_mut::<crate::ui_item_text::ItemTextOpen>()
        .open_letter(LETTER, 1);
    world.resource_mut::<crate::ui_gossip::GossipState>().npc = Some(GOSSIPER);
    world.resource_mut::<crate::ui_quest::QuestGiver>().npc = Some(SHARER);
    world
        .resource_mut::<crate::ui_taxi::TaxiState>()
        .open(FLIGHT_MASTER, 1, Default::default());
    world
        .resource_mut::<crate::ui_trainer::TrainerOpen>()
        .trainer = Some(TRAINER);
    world
        .resource_mut::<crate::ui_merchant::MerchantOpen>()
        .open(VENDOR, Vec::new());
    {
        let mut trade = world.resource_mut::<crate::ui_trade::TradeSession>();
        trade.begin(PARTNER);
        trade.open_window();
    }
    world.resource_mut::<crate::ui_duel::DuelState>().arbiter = 0xF130_0000_0000_00D1;
    world
        .resource_mut::<crate::ui_tabard::TabardOpen>()
        .open(TABARD_VENDOR);
    world
        .resource_mut::<crate::ui_petition::GuildRegistrarState>()
        .open_for_test(REGISTRAR);
    world
        .resource_mut::<crate::ui_mail::MailOpen>()
        .click(MAILBOX);
    world
        .resource_mut::<crate::ui_auction::AuctionOpen>()
        .open(AUCTIONEER, 1);
    world
        .resource_mut::<crate::ui_stable::StableOpen>()
        .open(STABLE_MASTER, 0, Vec::new());
    world
        .resource_mut::<crate::ui_bank::BankOpen>()
        .open(BANKER);
    world.insert_resource(crate::cinematic::Cinematic::playing_for_test());
}

/// One sweep closes every window in the reference's order: the legs from `0x490ab2` to
/// `0x490b2a`, `PLAYER_LEAVING_WORLD` (`0x490b4d`), then the interaction close (`0x490b5f`), whose
/// trade arm is tested before its bank arm, then the cinematic (`0x490b77`). The mail leg and the
/// duel leg are silent, and the charter window is no leg's.
#[test]
fn the_sweep_closes_every_window_in_the_references_order() {
    let (mut app, rx) = legs_app();
    app.insert_non_send_resource(spied_vm(&EVENTS));
    open_everything(app.world_mut());
    sweep_in_place(&mut app);
    assert_eq!(
        seen_in(&app),
        [
            "LOOT_CLOSED",
            "ITEM_TEXT_CLOSED",
            "GOSSIP_CLOSED",
            "QUEST_FINISHED",
            "TAXIMAP_CLOSED",
            "TRAINER_CLOSED",
            "MERCHANT_CLOSED",
            "CLOSE_TABARD_FRAME",
            "GUILD_REGISTRAR_CLOSED",
            "AUCTION_HOUSE_CLOSED",
            "PET_STABLE_CLOSED",
            "PLAYER_LEAVING_WORLD",
            "TRADE_CLOSED",
            "BANKFRAME_CLOSED",
            "CINEMATIC_STOP",
        ]
    );
    let sent: Vec<ClientCommand> = rx.try_iter().collect();
    assert!(
        matches!(
            sent.as_slice(),
            [
                ClientCommand::LootRelease { guid: CORPSE },
                ClientCommand::QuestPushResult { sharer: SHARER, .. },
                ClientCommand::CancelTrade,
                ClientCommand::CancelTrade,
            ]
        ),
        "the release (`0x48f2da`), the sharer's decline (`0x501173`), the trade leg's cancel \
         (`0x5d4310`) and the interaction close's (`0x4bf527`): {sent:?}"
    );

    let world = app.world();
    assert!(!world.resource::<crate::cinematic::Cinematic>().is_playing());
    assert_eq!(world.resource::<crate::ui_duel::DuelState>().arbiter, 0);
    assert_eq!(world.resource::<crate::ui_mail::MailOpen>().mailbox, None);
    assert!(!world.resource::<crate::ui_bank::BankOpen>().is_open());
}

/// The latch spent, a second departure in the same world sweeps nothing (`0x490a87`).
#[test]
fn a_second_departure_in_one_world_closes_nothing() {
    let (mut app, rx) = legs_app();
    app.insert_non_send_resource(spied_vm(&EVENTS));
    sweep_in_place(&mut app);
    let _ = rx.try_iter().count();
    open_everything(app.world_mut());
    assert!(!super::run_leave_world_sweep(app.world_mut()));
    assert_eq!(seen_in(&app), ["PLAYER_LEAVING_WORLD"]);
    assert_eq!(rx.try_iter().count(), 0);
}

/// The sweep's `PlaySound` scope (`0x490a94` to `0x490b64`) drops a leg handler's sound, and
/// closes before the cinematic's drop (`0x490b69`), whose handler is heard.
#[test]
fn the_sweep_drops_its_legs_sounds_and_not_the_cinematics() {
    let (mut app, _rx) = legs_app();
    let vm = spied_vm(&EVENTS);
    vm.run(
        "local f = CreateFrame('Frame') f:RegisterEvent('LOOT_CLOSED') \
         f:RegisterEvent('CINEMATIC_STOP') \
         f:SetScript('OnEvent', function() PlaySound(event) end)",
    )
    .expect("sound frame");
    app.insert_non_send_resource(vm);
    open_everything(app.world_mut());
    sweep_in_place(&mut app);
    let played = app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>()
        .take_sounds();
    assert_eq!(
        played,
        [benilla_ui::script::SoundRequest::KitName(
            "CINEMATIC_STOP".into()
        )]
    );
}
