//! The trade lane: what the trade's packet handlers and the name answer's callback signal, each
//! fired at its packet's place among the other lanes' items ([`crate::lanes`]). In the reference
//! each signals inside itself: the status handler's `PLAYER_MONEY` (`0x5d48a8`) and its
//! dispatcher's arms (`0x4bf720`): the open's `TRADE_SHOW` (`0x4bf644`), the accept flags'
//! `TRADE_ACCEPT_UPDATE` and our un-accept's send (`0x4bf260`, `0x4bf272`, `0x4bf29f`), the
//! refused slot's `0x4bfbd0` and the lines through `DisplayError` (`0x496720`); the extended
//! status's slot invalidations (`0x5d4ad3`, `0x5d4b33`) and its compare `0x4bf2b0` (`0x4bf414`,
//! `0x4bf452`, `0x4bf487`, `0x4bf4ab`, `0x4bf4d6`); and the initiate's name callback
//! (`0x5d40ca`). A handler queues its item here, stamped where it runs, and the getters its
//! listeners call read the trade through the host ([`super::live`]). A close run while the lane
//! holds an earlier item, or at a packet, queues its VM half here too ([`close_step`]), so it
//! undoes the VM's open after the open has run; its send goes out at once.

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::items::Items;
use crate::lanes::{LaneApp, Stamp, Until};
use crate::net::{ClientCommand, NetCommands, Objects};
use crate::ui_action::{MessageSink, UiError};

use super::TradeSession;

/// What an item signals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TradeEvent {
    /// The window's open, `0x4bf4e0` with a partner: the VM's offers, item table and cell
    /// (`0x4bf549`-`0x4bf5ef`), the open leg with the purse the handler found when a drop started
    /// the trade (`0x4bf5f1`), then `TRADE_SHOW` (`0x4bf644`).
    Show { partner: u64, purse: Option<u64> },
    /// `TRADE_ACCEPT_UPDATE(mine, theirs)` (`0x4bf260`, `0x4bf29f`).
    Accept(bool, bool),
    /// `CMSG_UNACCEPT_TRADE`, after its signal (`0x4bf272`).
    Unaccept,
    /// `InvalidateTradeSlot` (`0x4bfbd0`) on our 0-based slot: its item unlocked, the slot emptied
    /// and `TRADE_PLAYER_ITEM_CHANGED` (`0x4bfc3f`).
    Invalidate(u8),
    /// `TRADE_TARGET_ITEM_CHANGED` with the 1-based slot of the 0-based one here (`0x4bf414`,
    /// `0x4bf452`).
    TargetItem(u8),
    /// `TRADE_PLAYER_ITEM_CHANGED` with the 1-based slot of the 0-based one here (`0x4bf487`).
    PlayerItem(u8),
    /// Our offer's `PLAYER_TRADE_MONEY` then `PLAYER_MONEY` (`0x4bf4ab`, `0x4bfbea`).
    PlayerMoney,
    /// The partner's offer's `TRADE_MONEY_CHANGED` (`0x4bf4d6`).
    TargetMoney,
    /// The status handler's `PLAYER_MONEY` over both offers zeroed (`0x5d48a8`).
    Purse,
    /// A message line, by its catalog key and arguments (`0x496720`).
    Line(UiError),
    /// A closing status arm's start: both money offers zeroed (`0x5d4884`, `0x5d48eb`), for
    /// `CANCELED` its handler's `PLAYER_MONEY` (`0x5d48a8`) and its arm's `TRADE_REQUEST_CANCEL`
    /// (`0x4bf832`), then `UnlockTradeItems` (`0x4bf837`, `0x4bf859`, `0x4bf87e`).
    Unlock { canceled: bool },
    /// The close core's store of its `done` argument (`[0xb71748]`, `0x4bf4fd`).
    Done(bool),
    /// The close core's cell clear and `TRADE_CLOSED` (`0x4bf516`-`0x4bf522`).
    Closed,
    /// The close core's `CMSG_CANCEL_TRADE` (`0x4bf527` → `0x5d4310`).
    Cancel,
    /// A request's answer from the ladder (`0x4bf720`): `CMSG_BEGIN_TRADE`, `CMSG_BUSY_TRADE` or
    /// `CMSG_IGNORE_TRADE`, held behind a close still parked on the lane ([`TradeLane::answer`]).
    Answer(Answer),
}

/// A request's answer ([`TradeEvent::Answer`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Begin,
    Busy,
    Ignore,
}

/// The trade lane's queue.
#[derive(Resource, Default)]
pub(crate) struct TradeLane {
    events: VecDeque<(Stamp, TradeEvent)>,
}

impl TradeLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: TradeEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// Send a request's answer (`0x4bf720`): now, or behind a close's signals still parked here,
    /// whose listeners' `CloseTrade` sends its cancel inside the close in the reference
    /// (`0x4bf4fb` → `0x4bf527`), ahead of the next packet's answer.
    pub(crate) fn answer(&mut self, commands: &NetCommands, answer: Answer) {
        let closing = self.events.iter().any(|(_, e)| {
            matches!(
                e,
                TradeEvent::Closed | TradeEvent::Done(_) | TradeEvent::Unlock { .. }
            )
        });
        if closing {
            self.push(TradeEvent::Answer(answer));
        } else if let Some(command) = send_of(&TradeEvent::Answer(answer)) {
            let _ = commands.0.send(command);
        }
    }

    /// What the queued items signal, in order, taking them.
    #[cfg(test)]
    pub(crate) fn take(&mut self) -> Vec<TradeEvent> {
        self.events.drain(..).map(|(_, e)| e).collect()
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, TradeEvent)> {
        self.events.iter().cloned().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_trade_lane(app: &mut App) {
    app.init_resource::<TradeLane>()
        .add_lane(
            "trade",
            |world| world.get_resource::<TradeLane>()?.head(),
            fire_trade,
            None,
        )
        .add_systems(
            Update,
            drop_trade_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

/// The drop keeps the queued sends, which go out whether or not an interface hears the signals.
fn drop_trade_events(mut lane: ResMut<TradeLane>, commands: Option<Res<NetCommands>>) {
    for (_, event) in lane.events.drain(..) {
        if let (Some(command), Some(commands)) = (send_of(&event), commands.as_deref()) {
            let _ = commands.0.send(command);
        }
    }
}

/// The packet an item sends, for the two that only send.
fn send_of(event: &TradeEvent) -> Option<ClientCommand> {
    match event {
        TradeEvent::Unaccept => Some(ClientCommand::UnacceptTrade),
        TradeEvent::Cancel => Some(ClientCommand::CancelTrade),
        TradeEvent::Answer(Answer::Begin) => Some(ClientCommand::BeginTrade),
        TradeEvent::Answer(Answer::Busy) => Some(ClientCommand::BusyTrade),
        TradeEvent::Answer(Answer::Ignore) => Some(ClientCommand::IgnoreTrade),
        _ => None,
    }
}

/// What the open leg sends through and reads beside the session: the socket, the objects the
/// placement tests and the records it reads.
#[derive(bevy::ecs::system::SystemParam)]
struct OpenLeg<'w, 's> {
    commands: Res<'w, NetCommands>,
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_trade(
    In(until): In<Until>,
    mut vm: crate::script_calls::VmWith<ResMut<'static, TradeSession>>,
    mut lane: ResMut<TradeLane>,
    leg: OpenLeg,
    mut sink: MessageSink,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    while let Some(at) = lane.head().filter(|&s| until.admits(s)) {
        let Some((_, event)) = lane.events.pop_front() else {
            return;
        };
        let ((), moved) = crate::lanes::firing(at, || match event {
            TradeEvent::Show { partner, purse } => open(&mut vm, &leg, partner, purse),
            TradeEvent::Unaccept | TradeEvent::Cancel | TradeEvent::Answer(_) => {
                if let Some(command) = send_of(&event) {
                    let _ = leg.commands.0.send(command);
                }
            }
            event => vm.run(|script| {
                if let Some(script) = script {
                    fire(script, &mut sink, event);
                }
            }),
        });
        if moved {
            return;
        }
    }
}

/// Every item waiting, fired alone, for a test that drives the trade lane without the others.
#[cfg(test)]
pub(crate) fn fire_trade_lane(world: &mut World) {
    while let Some(head) = world.resource::<TradeLane>().head() {
        world
            .run_system_cached_with(fire_trade, Until::all())
            .expect("the trade lane fires");
        if world.resource::<TradeLane>().head() == Some(head) {
            return;
        }
    }
}

/// One item's signal.
fn fire(script: &mut UiScript, sink: &mut MessageSink, event: TradeEvent) {
    match event {
        TradeEvent::Accept(mine, theirs) => script.fire_event(
            "TRADE_ACCEPT_UPDATE",
            vec![
                ScriptValue::Int(i64::from(mine)),
                ScriptValue::Int(i64::from(theirs)),
            ],
        ),
        TradeEvent::Invalidate(i) => script.invalidate_trade_slot(i),
        TradeEvent::TargetItem(i) => script.fire_event(
            "TRADE_TARGET_ITEM_CHANGED",
            vec![ScriptValue::Int(i64::from(i) + 1)],
        ),
        TradeEvent::PlayerItem(i) => script.fire_event(
            "TRADE_PLAYER_ITEM_CHANGED",
            vec![ScriptValue::Int(i64::from(i) + 1)],
        ),
        // Our gold reaches the money input through `PLAYER_TRADE_MONEY` alone
        // (`TradeFrame.xml:565`), so a repaint never overwrites what the player is typing; the
        // partner's is the `TARGET_TRADE` money frame's `TRADE_MONEY_CHANGED` (`MoneyFrame.lua:133`).
        TradeEvent::PlayerMoney => {
            script.fire_event("PLAYER_TRADE_MONEY", vec![]);
            script.fire_event("PLAYER_MONEY", vec![]);
        }
        TradeEvent::TargetMoney => script.fire_event("TRADE_MONEY_CHANGED", vec![]),
        TradeEvent::Purse => {
            script.zero_trade_gold();
            script.fire_event("PLAYER_MONEY", vec![]);
        }
        TradeEvent::Line(line) => show_line(script, sink, &line),
        TradeEvent::Unlock { .. } | TradeEvent::Done(_) | TradeEvent::Closed => {
            crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Trade, || {
                close_vm(script, &event)
            })
        }
        TradeEvent::Show { .. }
        | TradeEvent::Unaccept
        | TradeEvent::Cancel
        | TradeEvent::Answer(_) => {}
    }
}

/// A line shown, and spoken, as its message row says, inside the handler that adds it.
fn show_line(script: &mut UiScript, sink: &mut MessageSink, line: &UiError) {
    let text = crate::ui_action::ui_error_text(line, &|key| {
        script.lua().globals().get::<String>(key).ok()
    });
    if let Some(text) = text {
        let shown = crate::ui_action::Shown::keyed(line.key, text);
        crate::ui_action::show_now(script, sink, "ui_trade", shown);
    }
}

/// [`show_line`] now, with the host lent.
fn line_now(In(line): In<UiError>, mut vm: crate::script_calls::HostedVm, mut sink: MessageSink) {
    vm.run(|script| {
        if let Some(script) = script {
            show_line(script, &mut sink, &line);
        }
    });
}

/// A close's VM half, at the handler or at its place on the lane.
fn close_vm(script: &mut UiScript, event: &TradeEvent) {
    match *event {
        TradeEvent::Unlock { canceled } => {
            script.zero_trade_gold();
            if canceled {
                script.fire_event("PLAYER_MONEY", vec![]);
                script.fire_event("TRADE_REQUEST_CANCEL", vec![]);
            }
            script.unlock_trade_items();
        }
        TradeEvent::Done(done) => script.set_trade_done(done),
        // The cell clears, so the item table's guids stop counting as offered (`0x4bfd20`); both
        // offers stay for the handlers' getters, and for the next open to clear.
        TradeEvent::Closed => {
            script.close_trade_window();
            script.fire_event("TRADE_CLOSED", vec![]);
        }
        _ => {}
    }
}

/// The open (`0x4bf549`-`0x4bf644`): the VM's offers, item table and cell; then, for a trade a drop
/// started, what the cursor holds moves into the offer, the coins tested against `purse`; then
/// `TRADE_SHOW`, whose handler hides the accept highlights.
fn open(
    vm: &mut crate::script_calls::VmWith<ResMut<'static, TradeSession>>,
    leg: &OpenLeg,
    partner: u64,
    purse: Option<u64>,
) {
    vm.run(|script| {
        if let Some(script) = script {
            script.open_trade_window(partner);
        }
    });
    // What the cursor holds, read with the host lent, as the leg reads the bags at the call.
    let held = purse.and_then(|purse| {
        vm.run(|script| {
            let script = script?;
            Some(match script.trade_open_money(purse) {
                Some(offer) => OpenHeld::Coins(offer),
                None => OpenHeld::Item(script.trade_open_item()?),
            })
        })
    });
    let item = vm.turn(|script, mut trade| {
        let script = script?;
        match held? {
            // Coins (`0x4bf5fe`-`0x4bf644`): `0x5d4500` sends the new offer, then the cursor
            // clears (`0x4bf61c`), the cache of our money `[0xb71740]` takes it (`0x4bf62b`), and
            // `PLAYER_TRADE_MONEY`, `PLAYER_MONEY` and `TRADE_SHOW` fire once each, then the leg
            // returns.
            OpenHeld::Coins(offer) => {
                trade.send_edit(&leg.commands, ClientCommand::SetTradeGold { copper: offer });
                script.trade_open_money_clear();
                trade.fold_held_coins(offer);
                script.fire_event("PLAYER_TRADE_MONEY", vec![]);
                script.fire_event("PLAYER_MONEY", vec![]);
                None
            }
            // An item (`0x4bf652`-`0x4bf6fa`): its send's answer is not read (`0x4bf6be`), then
            // the cursor clears and the slot is stored (`0x4bf6ca`-`0x4bf6fa`).
            OpenHeld::Item(held) => {
                let _ = trade.send_placement(
                    &leg.commands,
                    &leg.objects,
                    &leg.items,
                    held.slot,
                    held.guid,
                    held.bag,
                    held.bag_slot,
                );
                Some(held)
            }
        }
    });
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        if let Some(held) = item {
            script.trade_open_item_store(&held);
        }
        script.fire_event("TRADE_SHOW", vec![]);
    });
}

/// What the open leg moves into the offer.
enum OpenHeld {
    Coins(u32),
    Item(benilla_ui::script::TradeOpenItem),
}

/// Whether a close starting now waits behind the lane ([`crate::lanes::waits_behind`]): the open
/// it closes, or any earlier signal, has not fired yet.
pub(super) fn close_waits(world: &World) -> bool {
    crate::lanes::waits_behind(world.get_resource::<TradeLane>().and_then(TradeLane::head))
}

/// One step of a close: its VM half queued on the lane when the close `waits` ([`close_waits`]),
/// so the VM undoes its open after the open has run, its listeners re-entering the close as the
/// session it closed ([`crate::ui_session::in_parked_close`]); else run now, its signals fired with
/// the host lent. A send goes out now, inside the handler as the reference's `CMSG_CANCEL_TRADE`
/// does (`0x4bf527`), ahead of a later packet's `CMSG_BEGIN_TRADE`; only an open still on the
/// lane holds it back, as that open's own placement sends there (`0x4bf652`-`0x4bf6fa`) and the
/// cancel follows them.
pub(super) fn close_step(world: &mut World, waits: bool, event: TradeEvent) {
    let opening = world.get_resource::<TradeLane>().is_some_and(|lane| {
        lane.events
            .iter()
            .any(|(_, e)| matches!(e, TradeEvent::Show { .. }))
    });
    if waits && (send_of(&event).is_none() || opening) {
        if let Some(mut lane) = world.get_resource_mut::<TradeLane>() {
            lane.push(event);
            return;
        }
    }
    if let TradeEvent::Line(line) = event {
        if let Err(err) = world.run_system_cached_with(line_now, line) {
            debug!(target: "trade", "a close's line did not show: {err}");
        }
        return;
    }
    match send_of(&event) {
        Some(command) => {
            if let Some(net) = world.get_resource::<NetCommands>() {
                let _ = net.0.send(command);
            }
        }
        None => crate::ui_session::fire(world, |script| close_vm(script, &event)),
    }
}
