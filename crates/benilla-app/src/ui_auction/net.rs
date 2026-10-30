//! The auction house's packet handlers: every `SMSG_AUCTION_*` is written into [`AuctionOpen`],
//! its requests and the wave sent as the handler goes, and what the reference's handler shows and
//! signals inside it is readied in its order onto the house lane ([`settle`], [`HouseLane`]): the
//! hello's show, each line, each row a bid patches with its list's update, each removal, each
//! page. The lanes' drain fires them at the packet's place among what the other packets raised
//! ([`crate::lanes`]). A command result that goes on past its line runs the rest where the line
//! fires ([`AfterLine`]), after the line's listeners.

use benilla_protocol::messages::{
    auction_action, auction_error, AuctionBidderNotification, AuctionCommandTail, AuctionListEntry,
    AuctionOwnerNotification,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{BIDDER, LIST, OWNER};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{AfterLine, AuctionMessage, AuctionOpen, HouseEffect, RowPatch};
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, NetHandlerApp, SelfPlayer};
use crate::query_cache::Ask;
use crate::script_calls::VmWith;
use crate::ui_action::{show_now, MessageSink};

/// One handler per `SMSG_AUCTION_*` kind, plus the session-end listener, and the house lane.
pub(super) fn register(app: &mut App) {
    use crate::lanes::LaneApp;
    use SessionEventKind as K;
    app.init_resource::<HouseLane>()
        .add_lane(
            "auction house",
            |world| world.get_resource::<HouseLane>()?.peek(),
            fire_house,
            None,
        )
        .add_systems(
            OnExit(crate::client_state::ClientState::InWorld),
            end_session_house.in_set(crate::ui_script::SessionEnd::Clears),
        );
    app.net_handler(K::AuctionHello, on_hello)
        .net_handler(K::AuctionCommandResult, on_command_result)
        .net_handler(K::AuctionListResult, on_list_result)
        .net_handler(K::AuctionOwnerListResult, on_owner_list_result)
        .net_handler(K::AuctionBidderListResult, on_bidder_list_result)
        .net_handler(K::AuctionBidderNotification, on_bidder_notification)
        .net_handler(K::AuctionOwnerNotification, on_owner_notification)
        .net_handler(K::AuctionRemovedNotification, on_removed_notification)
        .net_handler(K::Disconnected, on_session_end);
}

/// What a handler reads besides the house: the socket a result's requests go out on, the VM
/// clock's tick the bid notices' deadline floor counts from (`0x42c010`), and the player's guid a
/// placed bid writes into its row (`0x468550`).
#[derive(SystemParam)]
pub(super) struct HouseWire<'w> {
    commands: Option<Res<'w, NetCommands>>,
    clock: Option<Res<'w, crate::ui_script::UiClock>>,
    me: Option<Res<'w, crate::net::SelfGuid>>,
}

impl HouseWire<'_> {
    fn view(&self) -> Wire<'_> {
        Wire {
            commands: self.commands.as_deref(),
            now: self
                .clock
                .as_ref()
                .map_or(0, |c| benilla_ui::script::auction_tick_ms(c.ui_now)),
            me: self.me.as_ref().and_then(|m| m.0).unwrap_or(0),
        }
    }
}

/// [`HouseWire`] as the handlers read it.
#[derive(Clone, Copy)]
pub(super) struct Wire<'a> {
    pub(super) commands: Option<&'a NetCommands>,
    /// The tick now, on `GetTime`'s clock.
    pub(super) now: u32,
    pub(super) me: u64,
}

impl Wire<'_> {
    fn send(&self, command: ClientCommand) {
        if let Some(commands) = self.commands {
            let _ = commands.0.send(command);
        }
    }
}

/// What a handler holds beside the VM: the house, its wire, and what a line and a patched row read.
#[derive(SystemParam)]
pub(super) struct House<'w, 's> {
    auction: ResMut<'w, AuctionOpen>,
    wire: HouseWire<'w>,
    items: Option<Res<'w, Items>>,
    names: Option<Res<'w, NameCache>>,
    /// The steps the handler readies for the VM.
    lane: ResMut<'w, HouseLane>,
    /// The player's guid as the getters read it, so a patched row resolves as their read does.
    self_q: Query<'w, 's, &'static Guid, With<SelfPlayer>>,
    icons: Option<Res<'w, crate::entities::ItemDisplays>>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
}

/// A handler's VM and its [`House`].
type HouseVm<'w, 's> = VmWith<'w, 's, House<'static, 'static>>;

/// Run one handler's body on the house, then ready the VM's half of what it wrote ([`settle`]).
fn handle(vm: &mut HouseVm, body: impl FnOnce(&mut AuctionOpen, Wire)) {
    {
        let mut h = vm.own();
        let wire = h.wire.view();
        body(&mut h.auction, wire);
    }
    settle(vm);
}

/// One step of what a handler did that the VM sees, readied as the handler ran it, for the house
/// lane to apply at the packet's place: the handler's own reads (its tick, the rows it wrote) are
/// taken as it runs, and only the VM's writes and the signals wait.
pub(crate) enum HouseStep {
    /// The hello's VM half (`0x4cd5b4`-`0x4cd5e6`): the emptied lists, the house's deposit rate,
    /// the sell slot's clear (`0x4cd640`) and `AUCTION_HOUSE_SHOW`.
    Show {
        lists: [benilla_ui::script::AuctionListState; 3],
        sorts: [benilla_ui::auction_sort::AuctionSortStack; 3],
        deposit_percent: u32,
    },
    /// The close core's VM half, behind the step it follows ([`close_waits`]):
    /// `AUCTION_HOUSE_CLOSED` (`0x4cd602`), the session's clear in the VM and the sell slot's
    /// (`0x4cd62e` → `0x4cd640`).
    Closed,
    /// A line shown (`0x496720`), whose chat line fires inside it (`0x49a870`), worded with the
    /// VM's strings and `fill`; then the rest of its handler, if it goes on past it.
    Line {
        msg: AuctionMessage,
        fill: Option<String>,
        then: Option<AfterLine>,
    },
    /// A row a bid patched, then its list's update ([`UiScript::patch_auction_row`]).
    Patch {
        list: usize,
        page: u32,
        auction_id: u32,
        bid: benilla_ui::script::AuctionRowBid,
    },
    /// An auction gone from the lists holding it on those pages
    /// ([`UiScript::remove_auction_from`]).
    Remove {
        auction_id: u32,
        pages: [Option<u32>; 3],
    },
    /// A list's sort tail and its update, run where the last of its page's items answered
    /// (`0x4cca10`) ([`UiScript::sort_auction_list`]).
    Tail(usize),
    /// A list's update alone, where a name its getter missed landed (`0x4cf290`).
    Update(usize),
    /// `NEW_AUCTION_UPDATE`, where the record a sell slot getter missed landed (`0x4ce2f0`).
    SellItemUpdate,
    /// A list result's page, and its sort tail when every item was known
    /// ([`UiScript::set_auction_list`], [`UiScript::sort_auction_list`]).
    Page {
        list: usize,
        state: benilla_ui::script::AuctionListState,
        sorts: [benilla_ui::auction_sort::AuctionSortStack; 3],
        tail: bool,
    },
}

/// The house lane: the handlers' steps, each stamped as the handler readies it
/// ([`crate::lanes::stamp`]), fired in stamp order among the other lanes ([`fire_house`]); and
/// the names `GetAuctionItemInfo` missed, each with the list whose callback waits on it.
#[derive(Resource, Default)]
pub(crate) struct HouseLane {
    steps: std::collections::VecDeque<(crate::lanes::Stamp, HouseStep)>,
    waits: Vec<(u64, usize)>,
}

impl HouseLane {
    pub(super) fn push(&mut self, step: HouseStep) {
        let stamp = crate::lanes::stamp();
        let at = self.steps.partition_point(|(s, _)| *s < stamp);
        self.steps.insert(at, (stamp, step));
    }

    fn peek(&self) -> Option<crate::lanes::Stamp> {
        self.steps.front().map(|(s, _)| *s)
    }

    fn pop(&mut self, until: crate::lanes::Until) -> Option<(crate::lanes::Stamp, HouseStep)> {
        until
            .admits(self.peek()?)
            .then(|| self.steps.pop_front())
            .flatten()
    }

    /// Keep a name's wait for `list`, once per name and list (`0x55f080`'s dedupe flag 1). The
    /// list callbacks (`0x4cf290`, `0x4cf2b0`, `0x4cf2d0`) are engine code that only the name
    /// cache's cancel `0x55f870` removes, and no auction caller makes it, so a wait outlives the VM
    /// whose getter made it and signals the next one.
    pub(super) fn wait_for_name(&mut self, guid: u64, list: usize) {
        if !self.waits.contains(&(guid, list)) {
            self.waits.push((guid, list));
        }
    }

    /// The name of `guid` landed in the packet being handled: each list that waited on it hears
    /// its update here, in the order the waits were kept, with no pending test and sorting
    /// nothing (`0x4cf290`, `0x4cf2b0`, `0x4cf2d0`).
    pub(crate) fn name_landed(&mut self, guid: u64) {
        let mut landed = Vec::new();
        self.waits.retain(|&(g, list)| {
            if g == guid {
                landed.push(list);
            }
            g != guid
        });
        for list in landed {
            self.push(HouseStep::Update(list));
        }
    }

    /// Whether `named` now answers some wait's name.
    #[cfg(test)]
    pub(crate) fn any_named(&self, named: impl Fn(u64) -> bool) -> bool {
        self.waits.iter().any(|&(g, _)| named(g))
    }

    /// Everything waiting, dropped: the session's end, or a new VM's for the waits.
    pub(super) fn clear(&mut self) {
        self.steps.clear();
        self.waits.clear();
    }

    /// The lists whose tails wait, in order.
    #[cfg(test)]
    pub(crate) fn tails(&self) -> Vec<usize> {
        self.steps
            .iter()
            .filter_map(|(_, s)| match s {
                HouseStep::Tail(list) => Some(*list),
                _ => None,
            })
            .collect()
    }
}

/// What a handler wrote that the VM sees, in its order ([`HouseEffect`]), each readied onto the
/// house lane as the handler reaches it: a line, or one left waiting on its item's record; a
/// patched row's bid; a removal; a page and whether its tail runs. A request or the wave goes out
/// here, in its place among the handler's steps.
fn settle(vm: &mut HouseVm) {
    loop {
        let effects = std::mem::take(&mut vm.own().auction.effects);
        if effects.is_empty() {
            return;
        }
        settle_effects(vm, effects);
    }
}

/// [`settle`]'s one pass. A line whose handler goes on past it and that waits on its item's
/// record shows nothing now (`0x4ccfcc`), so the rest runs here, its effects settled in the next
/// pass.
fn settle_effects(vm: &mut HouseVm, effects: Vec<HouseEffect>) {
    for effect in effects {
        match effect {
            HouseEffect::Line(msg) => {
                show_line(&mut vm.own(), msg, None);
            }
            HouseEffect::LineThen(msg, then) => {
                let mut h = vm.own();
                if !show_line(&mut h, msg, Some(then)) {
                    let wire = h.wire.view();
                    after_line(then, &mut h.auction, wire);
                }
            }
            HouseEffect::Edit { list, auction_id } => {
                let mut h = vm.own();
                if let Some(step) = patch_step(&h, list, auction_id) {
                    h.lane.push(step);
                }
            }
            HouseEffect::Remove { auction_id, pages } => {
                vm.own().lane.push(HouseStep::Remove { auction_id, pages });
            }
            HouseEffect::Send(command) => vm.own().wire.view().send(command),
            HouseEffect::Page(list) => land_page(vm, list),
        }
    }
}

/// The VM's half of a bid patch on `list`'s row for `auction_id`, the row as the house now holds
/// it.
fn patch_step(h: &House, list: usize, auction_id: u32) -> Option<HouseStep> {
    let self_guid = h.self_q.iter().next().map(|g| g.0);
    let names = h.names.as_deref()?;
    let (page, bid) = h.auction.row_bid(list, auction_id, self_guid, names)?;
    Some(HouseStep::Patch {
        list,
        page,
        auction_id,
        bid,
    })
}

/// A list result's page as its handler writes it, each item looked up with the list's callback
/// (`0x4cc9bf`, `0x4ccbff`, `0x4cce4f`) and the tail run there when none missed
/// ([`AuctionOpen::land_page`]), readied onto the house lane, stamped with the tick it is read at
/// (`0x4cc865`): the VM's clock, or with none the host's copy of it.
fn land_page(vm: &mut HouseVm, list: usize) {
    vm.turn(|script, mut h| {
        let Some(items) = h.items.as_deref() else {
            return;
        };
        let Some(names) = h.names.as_deref() else {
            return;
        };
        let tick = script.map_or(h.wire.view().now, |s| s.auction_tick_ms());
        let ask = match h.wire.commands.as_deref() {
            Some(commands) => crate::query_cache::Ask::Event(commands),
            None => crate::query_cache::Ask::Read,
        };
        let tail = h.auction.land_page(list, tick, items, ask);
        let reads = super::ListReads {
            self_guid: h.self_q.iter().next().map(|g| g.0),
            items,
            icons: h.icons.as_deref(),
            names,
            rolls: crate::items::RollCatalogs {
                props: h.props.as_deref(),
                enchants: h.enchants.as_deref(),
            },
        };
        let state = h
            .auction
            .list_state(list, tick, &reads, crate::query_cache::Ask::Read);
        let sorts = h.auction.sorts.0;
        h.lane.push(HouseStep::Page {
            list,
            state,
            sorts,
            tail,
        });
    });
}

/// A line readied onto the house lane, its item's name filled as the cache holds it; one whose
/// item is not cached waits on the record, asked with the callback `0x4cd190` (`0x4ccfa2`,
/// `0x4cd41f`, `0x4cd51a`), and shows when it lands ([`message_answered`]). Whether it was
/// readied, with the rest of its handler past it.
fn show_line(h: &mut House, msg: AuctionMessage, then: Option<AfterLine>) -> bool {
    let fill = match msg.item {
        None => None,
        Some(entry) => {
            let Some(items) = h.items.as_deref() else {
                return false;
            };
            match items.template(entry, 0, Ask::Read) {
                Some(t) => Some(t.name.clone()),
                None => {
                    if let Some(commands) = h.wire.commands.as_deref() {
                        let _ = items.template(entry, 0, Ask::Event(commands));
                    }
                    items.wait(entry, crate::items::RecordCallback::AuctionMessage);
                    h.auction.messages.push(msg);
                    return false;
                }
            }
        }
    };
    h.lane.push(HouseStep::Line { msg, fill, then });
    true
}

/// The callback `0x4cd190` on the record of `entry`, run inside the answer's handler: the oldest
/// line waiting on it shows with the item's name (`0x4cd1b3`, `0x4cd1c3`), or, for a refusal, is
/// dropped unshown (`0x4cd199`).
pub(crate) fn message_answered(
    auction: &mut AuctionOpen,
    lane: &mut HouseLane,
    items: &Items,
    entry: u32,
) {
    let Some(i) = auction.messages.iter().position(|m| m.item == Some(entry)) else {
        return;
    };
    let msg = auction.messages.remove(i);
    if let Some(t) = items.template(entry, 0, Ask::Read) {
        lane.push(HouseStep::Line {
            msg,
            fill: Some(t.name.clone()),
            then: None,
        });
    }
}

/// The callback `0x4ce2f0` on a found record, run inside the answer's handler: the sell slot's
/// `NEW_AUCTION_UPDATE` (`0x4ce2fa`-`0x4ce2ff`).
pub(crate) fn sell_item_answered(lane: &mut HouseLane) {
    lane.push(HouseStep::SellItemUpdate);
}

/// Whether a close starting now waits behind the house lane ([`crate::lanes::waits_behind`]): the
/// open it closes, or any earlier step, has not fired yet.
pub(super) fn close_waits(world: &World) -> bool {
    crate::lanes::waits_behind(world.get_resource::<HouseLane>().and_then(HouseLane::peek))
}

/// The house lane's `fire_until` ([`crate::lanes`]): each step `until` admits applied to the VM
/// with the host lent, so the Lua its update runs makes its calls inside the verb; a line shown in
/// place, as `DisplayError` shows it (`0x496720`), then the rest of its handler, after its
/// listeners ([`continue_after_line`]). A tail or an update that an answer runs reads the list as
/// the cache now resolves it, as the reference's getters read the cache live. The lane stops after
/// a step whose firing stamped anything, so what it pushed fires next. With no VM the VM's half of
/// a step is dropped; a handler's rest still runs.
fn fire_house(In(until): In<crate::lanes::Until>, mut vm: HouseVm, mut sink: MessageSink) {
    while let Some((stamp, step)) = vm.own().lane.pop(until) {
        let ((), moved) = crate::lanes::firing(stamp, || {
            if let HouseStep::Tail(list) | HouseStep::Update(list) = step {
                resolve_list(&mut vm, list);
            }
            let then = match &step {
                HouseStep::Line { then, .. } => *then,
                _ => None,
            };
            let closing = matches!(step, HouseStep::Closed);
            let apply = || {
                vm.run(|script| {
                    if let Some(script) = script {
                        apply_step(script, &mut sink, step);
                    }
                })
            };
            if closing {
                // The session it closes is cleared, so a close core its listeners re-enter is that
                // session's re-entry with its cell still set (`0x4cd602` before `0x4cd613`).
                crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Auction, apply);
            } else {
                apply();
            }
            if let Some(then) = then {
                continue_after_line(&mut vm, then);
            }
        });
        if moved {
            return;
        }
    }
}

/// The rest of a command result past its line, where the line fired: its writes and sends read
/// the house as the line's listeners left it (`0x4cc4d1`, `0x4cc51c`, `0x4cc52d`, `0x4cc56b`), and
/// a patched row's update fires in place.
fn continue_after_line(vm: &mut HouseVm, then: AfterLine) {
    let effects = {
        let mut h = vm.own();
        let wire = h.wire.view();
        after_line(then, &mut h.auction, wire);
        std::mem::take(&mut h.auction.effects)
    };
    for effect in effects {
        match effect {
            HouseEffect::Send(command) => vm.own().wire.view().send(command),
            HouseEffect::Edit { list, auction_id } => {
                let step = patch_step(&vm.own(), list, auction_id);
                if let Some(HouseStep::Patch {
                    list,
                    page,
                    auction_id,
                    bid,
                }) = step
                {
                    vm.run(|script| {
                        if let Some(script) = script {
                            script.patch_auction_row(list, page, auction_id, bid);
                        }
                    });
                }
            }
            // The rest past a line removes nothing, lands no page and shows no other line.
            HouseEffect::Line(_)
            | HouseEffect::LineThen(..)
            | HouseEffect::Remove { .. }
            | HouseEffect::Page(_) => {}
        }
    }
}

/// `list` pushed again as the caches now resolve its rows, ahead of the update an answer runs.
fn resolve_list(vm: &mut HouseVm, list: usize) {
    vm.turn(|script, h| {
        let (Some(script), Some(items), Some(names)) =
            (script, h.items.as_deref(), h.names.as_deref())
        else {
            return;
        };
        let tick = script.auction_tick_ms();
        let reads = super::ListReads {
            self_guid: h.self_q.iter().next().map(|g| g.0),
            items,
            icons: h.icons.as_deref(),
            names,
            rolls: crate::items::RollCatalogs {
                props: h.props.as_deref(),
                enchants: h.enchants.as_deref(),
            },
        };
        let state = h.auction.list_state(list, tick, &reads, Ask::Read);
        script.set_auction_list(list, state, h.auction.sorts.0);
    });
}

/// One step's VM half.
fn apply_step(script: &mut benilla_ui::script::UiScript, sink: &mut MessageSink, step: HouseStep) {
    match step {
        HouseStep::Show {
            lists,
            sorts,
            deposit_percent,
        } => {
            for (list, state) in lists.into_iter().enumerate() {
                script.set_auction_list(list, state, sorts);
            }
            script.open_auction_session(deposit_percent);
            script.clear_auction_sell_item();
            script.fire_event("AUCTION_HOUSE_SHOW", vec![]);
        }
        HouseStep::Closed => {
            script.fire_event("AUCTION_HOUSE_CLOSED", vec![]);
            script.close_auction_session();
            script.clear_auction_sell_item();
        }
        HouseStep::Line { msg, fill, .. } => {
            if let Some(line) = msg.shown(script, fill) {
                show_now(script, sink, "ui_auction", line);
            }
        }
        HouseStep::Patch {
            list,
            page,
            auction_id,
            bid,
        } => script.patch_auction_row(list, page, auction_id, bid),
        HouseStep::Remove { auction_id, pages } => script.remove_auction_from(auction_id, pages),
        HouseStep::Tail(list) => script.sort_auction_list(list),
        HouseStep::SellItemUpdate => script.fire_event("NEW_AUCTION_UPDATE", vec![]),
        // Literal names: `reference_ui`'s producer test finds fire sites by literal.
        HouseStep::Update(list) => match list {
            LIST => script.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]),
            BIDDER => script.fire_event("AUCTION_BIDDER_LIST_UPDATE", vec![]),
            _ => script.fire_event("AUCTION_OWNED_LIST_UPDATE", vec![]),
        },
        HouseStep::Page {
            list,
            state,
            sorts,
            tail,
        } => {
            script.set_auction_list(list, state, sorts);
            if tail {
                script.sort_auction_list(list);
            }
        }
    }
}

/// The steps waiting, fired alone, for a test that drives the house without the other lanes.
#[cfg(test)]
pub(crate) fn fire_house_lane(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    let until = crate::lanes::Until::all();
    while world
        .resource::<HouseLane>()
        .peek()
        .is_some_and(|s| until.admits(s))
    {
        world.run_system_once_with(fire_house, until).unwrap();
    }
}

/// The `MSG_AUCTION_HELLO` reply opens the window, as in the reference's hello handler `0x4cc420`.
fn on_hello(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::AuctionHello {
        auctioneer,
        house_id,
    } = ev
    {
        commands.queue(move |world: &mut World| {
            super::open_auction(world, auctioneer, house_id);
        });
    }
}

fn on_command_result(In(ev): In<SessionEvent>, mut vm: HouseVm) {
    if let SessionEvent::AuctionCommandResult {
        auction_id,
        action,
        error,
        tail,
    } = ev
    {
        handle(&mut vm, |auction, _| {
            auction_command_result(auction_id, action, error, &tail, auction);
        });
    }
}

fn on_list_result(In(ev): In<SessionEvent>, mut vm: HouseVm) {
    if let SessionEvent::AuctionListResult {
        auctions,
        total_count,
    } = ev
    {
        handle(&mut vm, |auction, _| {
            auction_list_result(auctions, total_count, auction);
        });
    }
}

fn on_owner_list_result(In(ev): In<SessionEvent>, mut vm: HouseVm) {
    if let SessionEvent::AuctionOwnerListResult {
        auctions,
        total_count,
    } = ev
    {
        handle(&mut vm, |auction, _| {
            auction_owner_list_result(auctions, total_count, auction);
        });
    }
}

fn on_bidder_list_result(In(ev): In<SessionEvent>, mut vm: HouseVm) {
    if let SessionEvent::AuctionBidderListResult {
        auctions,
        total_count,
    } = ev
    {
        handle(&mut vm, |auction, _| {
            auction_bidder_list_result(auctions, total_count, auction);
        });
    }
}

fn on_bidder_notification(In(ev): In<SessionEvent>, mut vm: HouseVm) {
    if let SessionEvent::AuctionBidderNotification(n) = ev {
        handle(&mut vm, |auction, _| {
            auction_bidder_notification(&n, auction);
        });
    }
}

fn on_owner_notification(In(ev): In<SessionEvent>, mut vm: HouseVm) {
    if let SessionEvent::AuctionOwnerNotification(n) = ev {
        handle(&mut vm, |auction, wire| {
            auction_owner_notification(&n, auction, wire);
        });
    }
}

fn on_removed_notification(In(ev): In<SessionEvent>, mut vm: HouseVm) {
    if let SessionEvent::AuctionRemovedNotification {
        auction_id,
        item_entry,
        ..
    } = ev
    {
        handle(&mut vm, |auction, _| {
            auction_removed_notification(auction_id, item_entry, auction);
        });
    }
}

/// The window dies with the socket: the server re-validates the auctioneer on every command, so
/// a window kept across a reconnect would fail silently.
fn on_session_end(In(_): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    auction.clear_session();
}

/// The session's end drops the steps and the name waits no VM will hear, as the chat lane drops
/// its lines: the Lua state they were for is replaced at the session end.
fn end_session_house(mut lane: ResMut<HouseLane>) {
    lane.clear();
}

/// A list's callback on one of its rows' records, run in the answer's list, a record or a
/// refusal alike (`0x4cca10`, `0x4ccc60`, `0x4cceb0`): its counter down, and the sort tail at 0
/// (`0x4cca21`, `0x4ccc71`, `0x4ccec1`), whatever the session.
pub(crate) fn list_record_answered(auction: &mut AuctionOpen, lane: &mut HouseLane, list: usize) {
    if auction.list_record_answered(list) {
        lane.push(HouseStep::Tail(list));
    }
}

/// Any of the three list results, which share a record and differ only in their tab. A result is
/// written whatever the session, as its handler reads no auctioneer (`0x4cc7f0`, `0x4cca30`,
/// `0x4ccc80`): one in flight at the close lands in the lists the close left standing.
fn auction_list(
    which: usize,
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    auction.wire.list_results[which] += 1;
    auction.set_list(which, auctions, total_count);
    auction.effects.push(HouseEffect::Page(which));
}

fn auction_list_result(
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    auction_list(LIST, auctions, total_count, auction);
}

/// The owner page; a count other than the total arms the owner request (`0x4ccc2d`-`0x4ccc35`).
fn auction_owner_list_result(
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    if auctions.len() as u32 != total_count {
        auction.owner_armed = true;
    }
    auction_list(OWNER, auctions, total_count, auction);
}

/// The bidder page, its rows as the wire sends them (`0x4ccc80`), an auction listed twice kept
/// twice; a count other than the total arms the bidder request (`0x4cce7d`-`0x4cce85`).
fn auction_bidder_list_result(
    auctions: Vec<AuctionListEntry>,
    total_count: u32,
    auction: &mut AuctionOpen,
) {
    if auctions.len() as u32 != total_count {
        auction.bidder_armed = true;
    }
    auction_list(BIDDER, auctions, total_count, auction);
}

/// The inventory result the reference reads when the fourth dword is short: `0x4cc472` seeds it
/// with 40 and a short read leaves it (`0x418e30`).
const SHORT_INVENTORY_RESULT: u32 = 0x28;

/// `SMSG_AUCTION_COMMAND_RESULT` (`0x4cc460`), the verdict on a sell, a cancel or a bid, through
/// the jump table `0x4cc7b4`. `auction_id` is 0 on most failures.
fn auction_command_result(
    auction_id: u32,
    action: u32,
    error: u32,
    tail: &AuctionCommandTail,
    auction: &mut AuctionOpen,
) {
    // For the live probe: a success otherwise leaves no trace but its request.
    auction.wire.last_command = Some((auction_id, action, error));
    match error {
        auction_error::OK => match action {
            // Started (`0x4cc4c7`): its line, then the owner request armed and sent again at its
            // saved offset (`0x4cc4da`, `0x4cc4e0`), past the line's listeners.
            auction_action::STARTED => {
                auction.line_then(
                    AuctionMessage::chat("ERR_AUCTION_STARTED"),
                    AfterLine::Started,
                );
            }
            // Cancelled (`0x4cc648`): its line and the auction removed (`0x4cc658`).
            auction_action::REMOVED => {
                auction.line(AuctionMessage::chat("ERR_AUCTION_REMOVED"));
                auction.remove_auction(auction_id);
            }
            // Bid placed (`0x4cc507`): the increment read (`0x4cc502`), its line, then the rest
            // past the line's listeners ([`bid_placed`]).
            auction_action::BID_PLACED => {
                let increment = match tail {
                    AuctionCommandTail::BidPlaced { new_min_outbid } => Some(*new_min_outbid),
                    _ => None,
                };
                auction.line_then(
                    AuctionMessage::chat("ERR_AUCTION_BID_PLACED"),
                    AfterLine::BidPlaced {
                        auction_id,
                        increment,
                    },
                );
            }
            // Any other action shows nothing (`0x4cc642`).
            _ => {}
        },
        // `HIGHER_BID` (`0x4cc672`): with the window open the browse and bidder rows take the
        // packet's bidder, bid and increment, and every exit shows the line (`0x4cc76e`,
        // `0x4cc728`).
        auction_error::HIGHER_BID => {
            if let (
                Some(_),
                AuctionCommandTail::HigherBid {
                    new_bidder_guid,
                    new_bid,
                    new_min_outbid,
                },
            ) = (auction.auctioneer, tail)
            {
                let patch = RowPatch {
                    bid: *new_bid,
                    increment: Some(*new_min_outbid),
                    bidder: *new_bidder_guid,
                    floor_at: None,
                };
                auction.patch_row(LIST, auction_id, &patch);
                auction.patch_row(BIDDER, auction_id, &patch);
            }
            auction.line(AuctionMessage::error("ERR_AUCTION_HIGHER_BID"));
        }
        _ => {
            let inventory_result = match tail {
                AuctionCommandTail::Inventory { result } => *result,
                _ => SHORT_INVENTORY_RESULT,
            };
            if let Some(key) = command_error_key(error, inventory_result) {
                auction.line(AuctionMessage::error(key));
            }
        }
    }
}

/// The rest of a command result past its line ([`AfterLine`]), as the line's listeners left the
/// house.
fn after_line(then: AfterLine, auction: &mut AuctionOpen, wire: Wire) {
    match then {
        AfterLine::Started => {
            auction.owner_armed = true;
            let offset = auction.owner_offset;
            if let Some(request) = auction.owner_list(offset) {
                auction.effects.push(HouseEffect::Send(request));
            }
        }
        AfterLine::BidPlaced {
            auction_id,
            increment,
        } => bid_placed(auction_id, increment, auction, wire),
        AfterLine::Outbid {
            house_id,
            auction_id,
            bid,
            increment,
            bidder,
        } => {
            let patch = RowPatch {
                bid,
                increment: Some(increment),
                bidder,
                floor_at: Some(wire.now),
            };
            outbid(house_id, auction_id, patch, auction);
        }
    }
}

/// A placed bid past its line (`0x4cc511`): its outbid pair removed (`0x4cc517`), the bidder
/// request armed and sent again at its saved offset (`0x4cc522`, `0x4cc528`); with the window open
/// the browse row takes the bid last sent, the packet's increment, the player as bidder and a
/// deadline at least 90 s out (`0x4cc564`-`0x4cc5c0`); then, open or not, the wave
/// (`0x4cc5dd`-`0x4cc60c`).
fn bid_placed(auction_id: u32, increment: Option<u32>, auction: &mut AuctionOpen, wire: Wire) {
    auction.outbid_remove(auction_id);
    auction.bidder_armed = true;
    let offset = auction.bidder_offset;
    if let Some(request) = auction.bidder_list(offset) {
        auction.effects.push(HouseEffect::Send(request));
    }
    if auction.auctioneer.is_some() {
        let patch = RowPatch {
            bid: auction.last_bid,
            increment,
            bidder: wire.me,
            floor_at: Some(wire.now),
        };
        auction.patch_row(LIST, auction_id, &patch);
    }
    auction
        .effects
        .push(HouseEffect::Send(ClientCommand::Emote {
            emote: EMOTE_WAVE,
        }));
}

/// `EMOTE_ONESHOT_WAVE`, the placed bid's emote (`0x4cc5fc push 3`).
const EMOTE_WAVE: u32 = 3;

/// A failed command's GlobalStrings key, the message the jump table `0x4cc7b4` pushes to
/// `0x496720`: 1 maps the inventory result through `0x622630`, 3 `0x25`, 4 `0x17`, 7 `0x174`, 10
/// `0x173`, 13 `0x1be`, and 2, 6, 8, 9, 11, 12 and past 13 `0x172` (`0x4cc799`). `None` where the
/// display shows nothing.
fn command_error_key(error: u32, inventory_result: u32) -> Option<&'static str> {
    Some(match error {
        auction_error::INVENTORY => crate::ui_items::inventory_result_key(inventory_result)?,
        auction_error::NOT_ENOUGH_MONEY => "ERR_NOT_ENOUGH_MONEY", // `0x25`
        auction_error::ITEM_NOT_FOUND => "ERR_ITEM_NOT_FOUND",     // `0x17`
        auction_error::BID_INCREMENT => "ERR_AUCTION_BID_INCREMENT", // `0x174`
        auction_error::BID_OWN => "ERR_AUCTION_BID_OWN",           // `0x173`
        auction_error::RESTRICTED_ACCOUNT => "ERR_RESTRICTED_ACCOUNT", // `0x1be`
        _ => "ERR_AUCTION_DATABASE_ERROR",                         // `0x172`
    })
}

/// `SMSG_AUCTION_BIDDER_NOTIFICATION` (`0x4cced0`): a zero `bid_or_zero` means we won, else we
/// were outbid.
fn auction_bidder_notification(notice: &AuctionBidderNotification, auction: &mut AuctionOpen) {
    let won = notice.bid_or_zero == 0;
    let msg = AuctionMessage::chat_item(
        if won {
            "ERR_AUCTION_WON_S"
        } else {
            "ERR_AUCTION_OUTBID_S"
        },
        notice.item_entry,
    );
    // Won: the auction removed from the lists (`0x4cd173`).
    if won {
        auction.line(msg);
        auction.remove_auction(notice.auction_id);
        return;
    }
    // Outbid: past its line ([`outbid`]).
    auction.line_then(
        msg,
        AfterLine::Outbid {
            house_id: notice.house_id,
            auction_id: notice.auction_id,
            bid: notice.bid_or_zero,
            increment: notice.out_bid,
            bidder: notice.bidder_guid,
        },
    );
}

/// An outbid notice past its line: the pair kept whatever the house (`0x4ccff3`); in the house
/// open then (`0x4ccff8`, `0x4ccffe`) the bidder and browse rows take the bid, the increment, the
/// new bidder and the 90 s floor (`0x4cd033`-`0x4cd132`). A closed window's house is 0, which no
/// notice names.
fn outbid(house_id: u32, auction_id: u32, patch: RowPatch, auction: &mut AuctionOpen) {
    auction.outbid_append(house_id, auction_id);
    if house_id == auction.house_id {
        auction.patch_row(BIDDER, auction_id, &patch);
        auction.patch_row(LIST, auction_id, &patch);
    }
}

/// `SMSG_AUCTION_OWNER_NOTIFICATION` (`0x4cd1f0`): one of ours took a bid, sold or expired.
fn auction_owner_notification(
    notice: &AuctionOwnerNotification,
    auction: &mut AuctionOpen,
    wire: Wire,
) {
    // A bidder guid means a new bid, which the reference does not announce (no display call in
    // `[0x4cd25f, 0x4cd3bd)`): with the window open (`0x4cd26b`) the owner and browse rows take
    // it, with the 90 s floor (`0x4cd296`-`0x4cd3a8`).
    if notice.bidder_guid != 0 {
        if auction.auctioneer.is_some() {
            let patch = RowPatch {
                bid: notice.bid,
                increment: Some(notice.out_bid),
                bidder: notice.bidder_guid,
                floor_at: Some(wire.now),
            };
            auction.patch_row(OWNER, notice.auction_id, &patch);
            auction.patch_row(LIST, notice.auction_id, &patch);
        }
        return;
    }
    // A zero guid closes it, sold with a bid (`0x4cd43e`), and removes it (`0x4cd468`).
    auction.line(AuctionMessage::chat_item(
        if notice.bid != 0 {
            "ERR_AUCTION_SOLD_S"
        } else {
            "ERR_AUCTION_EXPIRED_S"
        },
        notice.item_entry,
    ));
    auction.remove_auction(notice.auction_id);
}

/// `SMSG_AUCTION_REMOVED_NOTIFICATION` (`0x4cd480`): the seller cancelled an auction we bid on;
/// it always prints `ERR_AUCTION_REMOVED_S` (`0x4cd540`) and removes the auction (`0x4cd550`).
fn auction_removed_notification(auction_id: u32, item_entry: u32, auction: &mut AuctionOpen) {
    auction.line(AuctionMessage::chat_item(
        "ERR_AUCTION_REMOVED_S",
        item_entry,
    ));
    auction.remove_auction(auction_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::messages::MsgKind;

    #[test]
    fn the_table_routes_the_hello_and_the_session_end_to_the_house() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AuctionOpen>();
        register(&mut app);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::AuctionHello {
                auctioneer: 0x10,
                house_id: 1,
            }],
        );
        assert_eq!(app.world().resource::<AuctionOpen>().auctioneer, Some(0x10));
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );
        assert_eq!(app.world().resource::<AuctionOpen>().auctioneer, None);
    }

    const ME: u64 = 0x77;
    const NOW: u32 = 1_000_000;
    /// Commands as their debug text, `ClientCommand` having no `PartialEq`.
    fn text(commands: impl IntoIterator<Item = ClientCommand>) -> Vec<String> {
        commands.into_iter().map(|c| format!("{c:?}")).collect()
    }
    const AUCTIONEER: u64 = 0x42;

    /// The socket and the tick a handler reads, the sends caught.
    struct Rig {
        commands: NetCommands,
        rx: crossbeam_channel::Receiver<ClientCommand>,
    }

    impl Rig {
        fn new() -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self {
                commands: NetCommands(tx),
                rx,
            }
        }

        fn wire(&self) -> Wire<'_> {
            Wire {
                commands: Some(&self.commands),
                now: NOW,
                me: ME,
            }
        }

        fn sent(&self) -> Vec<ClientCommand> {
            self.rx.try_iter().collect()
        }
    }

    /// What the handler left to settle, in order, as debug text.
    fn fx(auction: &AuctionOpen) -> Vec<String> {
        auction
            .effects
            .iter()
            .map(|e| match e {
                HouseEffect::Line(m) | HouseEffect::LineThen(m, _) => format!("Line({m:?})"),
                e => format!("{e:?}"),
            })
            .collect()
    }

    /// The rest of the handler past its line, as the line's firing runs it once its listeners
    /// have run ([`continue_after_line`]), in place of what the handler left to settle.
    fn rest(a: &mut AuctionOpen, rig: &Rig) {
        let then = a
            .effects
            .iter()
            .find_map(|e| match e {
                HouseEffect::LineThen(_, then) => Some(*then),
                _ => None,
            })
            .expect("a line the handler goes on past");
        a.effects.clear();
        after_line(then, a, rig.wire());
    }

    /// A chat line's effect, as [`fx`] prints it.
    fn line(key: &'static str) -> String {
        format!("Line({:?})", AuctionMessage::chat(key))
    }

    /// A send's effect, as [`fx`] prints it.
    fn send(command: ClientCommand) -> String {
        format!("Send({command:?})")
    }

    /// The lines the handler showed, in order.
    fn keys(auction: &AuctionOpen) -> Vec<(&'static str, MsgKind, Option<u32>)> {
        auction
            .effects
            .iter()
            .filter_map(|e| match e {
                HouseEffect::Line(m) | HouseEffect::LineThen(m, _) => {
                    Some((m.key, benilla_ui::messages::kind_of(m.key), m.item))
                }
                _ => None,
            })
            .collect()
    }

    fn owner(bidder_guid: u64, bid: u32) -> AuctionOwnerNotification {
        AuctionOwnerNotification {
            auction_id: 7,
            bid,
            out_bid: 5,
            bidder_guid,
            item_entry: 2589,
            random_property_id: 0,
        }
    }

    fn bidder(bid_or_zero: u32) -> AuctionBidderNotification {
        AuctionBidderNotification {
            house_id: 1,
            auction_id: 7,
            bidder_guid: 0x6C,
            bid_or_zero,
            out_bid: 5,
            item_entry: 2589,
            random_property_id: 0,
        }
    }

    /// A row of auction `auction_id`: 100c to start, no bid, an hour left.
    fn entry(auction_id: u32) -> AuctionListEntry {
        AuctionListEntry {
            auction_id,
            item_entry: 2589,
            perm_enchant: 0,
            random_property_id: 0,
            suffix_factor: 0,
            count: 1,
            spell_charges: 0,
            owner_guid: 0x55,
            start_bid: 100,
            min_increment: 0,
            buyout: 0,
            time_left_ms: 3_600_000,
            bidder_guid: 0,
            current_bid: 0,
        }
    }

    /// A house open at house 1 holding auctions 7 and 8 on all three lists, each read at `NOW`.
    fn house() -> AuctionOpen {
        let mut a = AuctionOpen::default();
        a.open(AUCTIONEER, 1);
        for list in [LIST, OWNER, BIDDER] {
            a.set_list(list, vec![entry(7), entry(8)], 2);
            a.lists[list].arrived_ms = Some(NOW);
        }
        a
    }

    fn row(a: &AuctionOpen, list: usize, auction_id: u32) -> Option<&AuctionListEntry> {
        a.lists[list]
            .entries
            .iter()
            .find(|e| e.auction_id == auction_id)
    }

    #[test]
    fn a_bid_on_your_auction_updates_the_row_and_says_nothing() {
        let rig = Rig::new();
        let mut a = AuctionOpen::default();
        auction_owner_notification(&owner(0x6C, 500), &mut a, rig.wire());
        assert!(
            keys(&a).is_empty(),
            "a live bid raises no message; `[0x4cd25f, 0x4cd3bd)` holds no display call"
        );

        // Sold: guid zeroed, and a price paid.
        let mut a = AuctionOpen::default();
        auction_owner_notification(&owner(0, 10_000), &mut a, rig.wire());
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_SOLD_S", MsgKind::Chat, Some(2589))]
        );

        // Expired: guid zeroed, and nobody paid anything.
        let mut a = AuctionOpen::default();
        auction_owner_notification(&owner(0, 0), &mut a, rig.wire());
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_EXPIRED_S", MsgKind::Chat, Some(2589))],
            "a zero bid on a closed auction is an expiry, not a sale"
        );
    }

    /// A bid on one of ours (`0x4cd25f`-`0x4cd3a8`): with the window open (`0x4cd26b`) the owner
    /// and browse rows take the notice's bid, increment and bidder and the 90 s floor; the bidder
    /// row and a closed window's rows stay.
    #[test]
    fn a_bid_on_your_auction_patches_the_owner_and_browse_rows() {
        let rig = Rig::new();
        let mut a = house();
        a.lists[OWNER].entries[0].time_left_ms = 1_000;
        auction_owner_notification(&owner(0x6C, 500), &mut a, rig.wire());
        for list in [OWNER, LIST] {
            let e = row(&a, list, 7).unwrap();
            assert_eq!(
                (e.current_bid, e.min_increment, e.bidder_guid),
                (500, 5, 0x6C)
            );
        }
        assert_eq!(row(&a, OWNER, 7).unwrap().time_left_ms, 90_000);
        assert_eq!(row(&a, LIST, 7).unwrap().time_left_ms, 3_600_000);
        assert_eq!(row(&a, BIDDER, 7).unwrap().current_bid, 0);

        let mut a = house();
        a.clear();
        auction_owner_notification(&owner(0x6C, 500), &mut a, rig.wire());
        assert_eq!(row(&a, LIST, 7).unwrap().current_bid, 0);
        assert!(rig.sent().is_empty());
    }

    #[test]
    fn a_zero_bid_field_means_you_won() {
        let mut a = AuctionOpen::default();
        auction_bidder_notification(&bidder(0), &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_WON_S", MsgKind::Chat, Some(2589))]
        );

        let mut a = AuctionOpen::default();
        auction_bidder_notification(&bidder(9_000), &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_OUTBID_S", MsgKind::Chat, Some(2589))]
        );
    }

    #[test]
    fn the_outcomes_are_chat_and_the_refusals_are_the_error_frame() {
        let tail = AuctionCommandTail::Empty;
        for (action, key) in [
            (auction_action::STARTED, "ERR_AUCTION_STARTED"),
            (auction_action::REMOVED, "ERR_AUCTION_REMOVED"),
            (auction_action::BID_PLACED, "ERR_AUCTION_BID_PLACED"),
        ] {
            let mut a = AuctionOpen::default();
            auction_command_result(7, action, auction_error::OK, &tail, &mut a);
            assert_eq!(
                keys(&a),
                vec![(key, MsgKind::Chat, None)],
                "a successful {action} says so in chat, with no item fill"
            );
        }

        let mut a = AuctionOpen::default();
        auction_removed_notification(7, 2589, &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_REMOVED_S", MsgKind::Chat, Some(2589))]
        );

        // A refusal, on the red frame.
        let mut a = AuctionOpen::default();
        auction_command_result(0, auction_action::BID_PLACED, 10, &tail, &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_BID_OWN", MsgKind::Error, None)]
        );
    }

    /// Error 5 (`0x4cc672`) always ends at `push 0x175`: with the window open it first patches the
    /// browse and bidder rows from the packet, and with the window closed (`0x4cc69f`) or the
    /// auction on no list (`0x4cc728`) it says the same.
    #[test]
    fn a_higher_bid_patches_the_rows_and_always_says_so() {
        let rig = Rig::new();
        let tail = AuctionCommandTail::HigherBid {
            new_bidder_guid: 0x6C,
            new_bid: 900,
            new_min_outbid: 45,
        };
        let higher = [("ERR_AUCTION_HIGHER_BID", MsgKind::Error, None)];
        let mut a = house();
        auction_command_result(7, auction_action::BID_PLACED, 5, &tail, &mut a);
        assert_eq!(keys(&a), higher);
        for list in [LIST, BIDDER] {
            let e = row(&a, list, 7).unwrap();
            assert_eq!(
                (
                    e.current_bid,
                    e.min_increment,
                    e.bidder_guid,
                    e.time_left_ms
                ),
                (900, 45, 0x6C, 3_600_000),
                "list {list}: the deadline stays"
            );
        }
        assert_eq!(
            row(&a, OWNER, 7).unwrap().current_bid,
            0,
            "not the owner row"
        );

        let mut a = house();
        auction_command_result(9, auction_action::BID_PLACED, 5, &tail, &mut a);
        assert_eq!(keys(&a), higher, "no row holds it");

        let mut a = house();
        a.clear();
        auction_command_result(7, auction_action::BID_PLACED, 5, &tail, &mut a);
        assert_eq!(keys(&a), higher, "the window closed");
        assert_eq!(
            row(&a, LIST, 7).unwrap().current_bid,
            0,
            "and nothing patched"
        );
        assert!(rig.sent().is_empty(), "a refusal sends nothing");
    }

    /// The jump table `0x4cc7b4`: 4 is `0x17` (`0x4cc78e`), 13 `0x1be` (`0x4cc792`), and 2, 6, 8,
    /// 9, 11, 12 and past 13 the default `0x172` (`0x4cc799`).
    #[test]
    fn each_refusal_shows_its_tables_line() {
        let shown = |error: u32, tail: AuctionCommandTail| {
            let mut a = AuctionOpen::default();
            auction_command_result(0, auction_action::BID_PLACED, error, &tail, &mut a);
            keys(&a).into_iter().map(|k| k.0).collect::<Vec<_>>()
        };
        let e = || AuctionCommandTail::Empty;
        assert_eq!(shown(3, e()), ["ERR_NOT_ENOUGH_MONEY"]);
        assert_eq!(shown(4, e()), ["ERR_ITEM_NOT_FOUND"]);
        assert_eq!(shown(7, e()), ["ERR_AUCTION_BID_INCREMENT"]);
        assert_eq!(shown(13, e()), ["ERR_RESTRICTED_ACCOUNT"]);
        for error in [2, 6, 8, 9, 11, 12, 14, 999] {
            assert_eq!(shown(error, e()), ["ERR_AUCTION_DATABASE_ERROR"], "{error}");
        }
    }

    /// Error 1 maps its fourth dword through `0x622630` (`0x4cc783`): 23 is `ERR_ITEM_NOT_FOUND`,
    /// 0 is the id the display refuses, and past 66, the whole dword tested, `ERR_BAG_FULL`. A
    /// short packet leaves the dword at its seed, 40 (`0x4cc472`).
    #[test]
    fn an_inventory_refusal_shows_the_inventory_results_line() {
        let shown = |tail: AuctionCommandTail| {
            let mut a = AuctionOpen::default();
            auction_command_result(0, auction_action::STARTED, 1, &tail, &mut a);
            keys(&a).into_iter().map(|k| k.0).collect::<Vec<_>>()
        };
        let inv = |result| AuctionCommandTail::Inventory { result };
        assert_eq!(shown(inv(23)), ["ERR_ITEM_NOT_FOUND"]);
        assert!(
            shown(inv(0)).is_empty(),
            "0x1d1 is past the display's table"
        );
        assert_eq!(shown(inv(67)), ["ERR_BAG_FULL"]);
        assert_eq!(shown(inv(256 + 23)), ["ERR_BAG_FULL"], "not narrowed to 23");
        assert_eq!(shown(inv(29)), ["ERR_NOT_ENOUGH_MONEY"]);
        assert_eq!(shown(AuctionCommandTail::Empty), ["ERR_INTERNAL_BAG_ERROR"]);
    }

    /// A placed bid (`0x4cc4f3`): its line (`0x4cc507`) and nothing else in the handler's body;
    /// then, past the line's listeners, its outbid pair gone (`0x4cc517`), the bidder request armed
    /// and sent at its saved offset (`0x4cc522`, `0x4cc528`), the browse row taking the bid last
    /// sent, the packet's increment, the player and a deadline at least 90 s out
    /// (`0x4cc564`-`0x4cc5c0`), and then `CMSG_EMOTE` 3 (`0x4cc5dd`-`0x4cc60c`).
    #[test]
    fn a_placed_bid_patches_the_browse_row_and_waves() {
        let rig = Rig::new();
        let mut a = house();
        a.lists[LIST].entries[0].time_left_ms = 30_000;
        a.outbid_append(1, 7);
        a.outbid_append(1, 9);
        a.bidder_armed = false;
        a.bidder_offset = 50;
        a.last_bid = 650;
        let tail = AuctionCommandTail::BidPlaced { new_min_outbid: 33 };
        auction_command_result(7, auction_action::BID_PLACED, 0, &tail, &mut a);
        let placed = line("ERR_AUCTION_BID_PLACED");
        assert_eq!(fx(&a), [placed], "the line, and the rest waits for it");
        assert_eq!(a.outbid.len(), 2, "nothing removed ahead of the line");
        assert_eq!(row(&a, LIST, 7).unwrap().current_bid, 0, "nothing patched");
        rest(&mut a, &rig);
        let e = row(&a, LIST, 7).unwrap();
        assert_eq!(
            (
                e.current_bid,
                e.min_increment,
                e.bidder_guid,
                e.time_left_ms
            ),
            (650, 33, ME, 90_000)
        );
        assert_eq!(
            row(&a, BIDDER, 7).unwrap().current_bid,
            0,
            "the browse row alone"
        );
        assert!(
            a.outbid.is_empty(),
            "auction 7's pair by the bid (`0x4cc517`), house 1's first left by the request \
             (`0x4cd808`)"
        );
        assert!(!a.bidder_armed, "the request went out");
        assert!(rig.sent().is_empty(), "the firing sends, in order");
        let wave = send(ClientCommand::Emote { emote: 3 });
        assert_eq!(
            fx(&a),
            [
                send(ClientCommand::AuctionListBidderItems {
                    auctioneer: AUCTIONEER,
                    list_from: 50,
                    auction_ids: vec![9],
                }),
                format!(
                    "{:?}",
                    HouseEffect::Edit {
                        list: LIST,
                        auction_id: 7
                    }
                ),
                wave.clone(),
            ]
        );

        // A deadline past the floor stays; with the window closed nothing is patched, and the
        // request and the wave still go.
        let mut a = house();
        a.last_bid = 650;
        auction_command_result(7, auction_action::BID_PLACED, 0, &tail, &mut a);
        rest(&mut a, &rig);
        assert_eq!(row(&a, LIST, 7).unwrap().time_left_ms, 3_600_000);
        let _ = rig.sent();
        a.clear();
        a.last_bid = 700;
        a.effects.clear();
        auction_command_result(8, auction_action::BID_PLACED, 0, &tail, &mut a);
        rest(&mut a, &rig);
        assert_eq!(row(&a, LIST, 8).unwrap().current_bid, 0);
        assert_eq!(
            fx(&a),
            [
                send(ClientCommand::AuctionListBidderItems {
                    auctioneer: 0,
                    list_from: 0,
                    auction_ids: vec![],
                }),
                wave,
            ],
            "the sender tests no auctioneer (`0x4cd726`)"
        );
    }

    /// A started auction shows its line (`0x4cc4c7`); then, past the line's listeners, it re-arms
    /// the owner request and sends it at the offset saved then (`0x4cc4d1`-`0x4cc4e0`).
    #[test]
    fn a_started_auction_asks_the_owner_list_again() {
        let rig = Rig::new();
        let mut a = house();
        a.owner_armed = false;
        a.owner_offset = 100;
        let tail = AuctionCommandTail::Empty;
        auction_command_result(7, auction_action::STARTED, 0, &tail, &mut a);
        assert_eq!(fx(&a), [line("ERR_AUCTION_STARTED")]);
        assert!(!a.owner_armed, "armed past the line");
        // A listener's request moved the saved offset.
        a.owner_offset = 150;
        rest(&mut a, &rig);
        assert_eq!(
            fx(&a),
            [send(ClientCommand::AuctionListOwnerItems {
                auctioneer: AUCTIONEER,
                list_from: 150,
            })]
        );
        assert!(rig.sent().is_empty(), "the firing sends, in order");
        assert!(!a.owner_armed);
    }

    /// The cancel reply, a win, a sale or expiry and the removed notice each show their line and
    /// then remove the auction from every list that holds it (`0x4cc658`, `0x4cd173`, `0x4cd468`,
    /// `0x4cd550`), with its page per list for the VM's copies, and send nothing; a closed
    /// window's lists change too.
    #[test]
    fn the_closing_notices_remove_the_auction_and_ask_nothing() {
        let rig = Rig::new();
        let remove = |f: &dyn Fn(&mut AuctionOpen)| {
            let mut a = house();
            a.clear();
            a.owner_armed = true;
            a.bidder_armed = true;
            let pages = a.lists.each_ref().map(|l| Some(l.page));
            f(&mut a);
            for list in [LIST, OWNER, BIDDER] {
                assert!(row(&a, list, 7).is_none(), "list {list}");
                assert!(row(&a, list, 8).is_some());
                assert_eq!(a.lists[list].total, 2, "the total stays");
            }
            assert!(matches!(a.effects[0], HouseEffect::Line(..)));
            assert_eq!(
                fx(&a)[1..],
                [format!(
                    "{:?}",
                    HouseEffect::Remove {
                        auction_id: 7,
                        pages
                    }
                )]
            );
        };
        let e = AuctionCommandTail::Empty;
        remove(&|a| auction_command_result(7, auction_action::REMOVED, 0, &e, a));
        remove(&|a| auction_bidder_notification(&bidder(0), a));
        remove(&|a| auction_owner_notification(&owner(0, 10_000), a, rig.wire()));
        remove(&|a| auction_owner_notification(&owner(0, 0), a, rig.wire()));
        remove(&|a| auction_removed_notification(7, 2589, a));
        assert!(rig.sent().is_empty(), "no list request");
    }

    /// Outbid (`0x4ccfed`-`0x4cd132`): the pair kept whatever the house (`0x4ccff3`); in the open
    /// house the bidder and browse rows take the notice's bid, increment and bidder and the 90 s
    /// floor, each past the line (`0x4ccfd5`); another house patches nothing.
    #[test]
    fn an_outbid_notice_keeps_the_pair_and_patches_the_open_houses_rows() {
        let rig = Rig::new();
        let mut a = house();
        a.lists[BIDDER].entries[0].time_left_ms = 1_000;
        auction_bidder_notification(&bidder(9_000), &mut a);
        assert!(a.outbid.is_empty(), "nothing kept ahead of the line");
        assert_eq!(row(&a, BIDDER, 7).unwrap().current_bid, 0, "nor patched");
        rest(&mut a, &rig);
        assert_eq!(a.outbid, [(1, 7)]);
        for list in [BIDDER, LIST] {
            let e = row(&a, list, 7).unwrap();
            assert_eq!(
                (e.current_bid, e.min_increment, e.bidder_guid),
                (9_000, 5, 0x6C)
            );
        }
        assert_eq!(row(&a, BIDDER, 7).unwrap().time_left_ms, 90_000);
        assert_eq!(row(&a, OWNER, 7).unwrap().current_bid, 0);

        let mut a = house();
        let mut other = bidder(9_000);
        other.house_id = 6;
        auction_bidder_notification(&other, &mut a);
        rest(&mut a, &rig);
        assert_eq!(a.outbid, [(6, 7)]);
        assert_eq!(row(&a, LIST, 7).unwrap().current_bid, 0);
        assert!(rig.sent().is_empty());
    }

    /// The bidder request (`0x4cd720`) sends the open house's outbid ids, then removes the first
    /// pair of that house alone: `0x4cdf90`'s shift reuses the scan's index and ends it.
    #[test]
    fn the_bidder_request_sends_the_houses_ids_and_drops_one_pair() {
        let mut a = house();
        for pair in [(1, 7), (2, 8), (1, 9), (1, 10)] {
            a.outbid_append(pair.0, pair.1);
        }
        assert_eq!(
            text(a.bidder_list(0)),
            text([ClientCommand::AuctionListBidderItems {
                auctioneer: AUCTIONEER,
                list_from: 0,
                auction_ids: vec![7, 9, 10],
            }])
        );
        assert_eq!(a.outbid, [(2, 8), (1, 9), (1, 10)]);
        assert!(a.bidder_list(0).is_none(), "the flag is spent");
    }

    /// The bidder page's rows stay as the wire sends them, a repeated auction twice: `0x4ccc80`
    /// reads each record in turn.
    #[test]
    fn the_bidder_page_keeps_a_repeated_auction() {
        let mut a = AuctionOpen::default();
        auction_bidder_list_result(vec![entry(7), entry(8), entry(7)], 3, &mut a);
        let ids: Vec<u32> = a.lists[BIDDER]
            .entries
            .iter()
            .map(|e| e.auction_id)
            .collect();
        assert_eq!(ids, [7, 8, 7]);
    }

    /// `0x4cde50`: on a full list the room doubles and the pair is written past the count it does
    /// not raise (`0x4cdf02`-`0x4cdf1c`), so the ninth is lost and the tenth lands.
    #[test]
    fn a_full_outbid_list_grows_and_loses_the_pair_that_filled_it() {
        let mut a = AuctionOpen::default();
        for auction_id in 1..=10 {
            a.outbid_append(1, auction_id);
        }
        let ids: Vec<u32> = a.outbid.iter().map(|&(_, id)| id).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7, 8, 10]);
    }

    /// An owner or bidder result whose count is not its total arms its request (`0x4ccc35`,
    /// `0x4cce85`); the browse result has no such test.
    #[test]
    fn a_partial_owner_or_bidder_page_arms_its_request() {
        let mut a = AuctionOpen::default();
        auction_owner_list_result(vec![entry(1)], 1, &mut a);
        auction_bidder_list_result(vec![entry(1), entry(1)], 2, &mut a);
        assert!(!a.owner_armed && !a.bidder_armed, "whole pages");
        auction_owner_list_result(vec![entry(1)], 60, &mut a);
        auction_bidder_list_result(vec![entry(1)], 60, &mut a);
        assert!(a.owner_armed && a.bidder_armed);
    }

    #[test]
    fn closing_the_window_does_not_swallow_a_pending_notice() {
        let rig = Rig::new();
        let mut a = AuctionOpen::default();
        a.open(0x42, 1);
        auction_owner_notification(&owner(0, 10_000), &mut a, rig.wire());
        a.clear();
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_SOLD_S", MsgKind::Chat, Some(2589))]
        );
    }
}
