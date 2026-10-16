//! The trade window: [`net`] folds the two status packets into [`TradeSession`], [`feed_trade`]
//! pushes it into the VM and fires the trade events, [`drain_trade`] sends the Lua intents,
//! [`run_record_callbacks`] signals `TRADE_UPDATE` when a record a getter asked for lands, and
//! [`answer_trade_request`] answers an incoming request.

use benilla_protocol::messages::{TradeItem, TradeStatusExtended, TRADE_SLOT_COUNT};
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, TradeSideState, TradeSlotItem, TradeState, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::target::Selection;
use crate::ui_party::GroupState;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed, UiInput};
use crate::ui_session::{CoreCloses, LatchApp, LatchedWindow, NpcWindow};

mod net;

/// One side's offer: slots 0-5 change hands, slot 6 is the non-traded enchant slot.
#[derive(Default)]
struct TradeOffer {
    slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
    gold: u32,
    /// The header's spell, `[0xc4bed8 + 4*side]` (`0x5d49fa`): the spell this side's player cast
    /// on the other side's enchant slot, 0 for none. Our own window's names the partner's slot 7,
    /// the partner's names ours (`0x4c02b7`, `0x4c05d1`).
    enchant_spell_id: u32,
    /// Our side only: each placed item's `ITEM_FIELD_FLAGS`, read off its object when it was
    /// placed. The slot's tooltip passes that live object to the builder (`0x5342ed`).
    object_flags: [u32; TRADE_SLOT_COUNT],
}

/// The trade session [`net`] fills and [`feed_trade`] reads, cleared on a close and on disconnect.
#[derive(Resource, Default)]
pub(crate) struct TradeSession {
    /// Set at our initiate or the ladder's accept, since `OPEN_WINDOW` carries no guid.
    partner: Option<u64>,
    /// An unanswered incoming `BEGIN_TRADE`, the initiator's guid; not yet a trade.
    request: Option<u64>,
    /// Our `CMSG_INITIATE_TRADE` is live: the reference's `[0xc4bec8]`, set only as it goes out
    /// (`0x5d4021`) and cleared by the status handler's tail (`0x5d4931`), which codes 1, 2, 4,
    /// 7, 9 and 22 skip, so it survives `OPEN_WINDOW` for the life of a trade we opened.
    initiated: bool,
    /// `OPEN_WINDOW` arrived; `partner` is set before it.
    open: bool,
    our: TradeOffer,
    their: TradeOffer,
    /// We pressed Trade: optimistic, dropped by `BACK_TO_TRADE`, an offer change or a close.
    our_accept: bool,
    /// The partner pressed Trade (`TRADE_ACCEPT`).
    their_accept: bool,
    /// Our 0-based slots `InvalidateTradeSlot` (`0x4bfbd0`) runs on, owed to [`feed_trade`].
    invalidated: Vec<u8>,
    /// The own-edit counter `[0xc4bea8]`: 1 at `OPEN_WINDOW` (`0x5d4825`), one more for each
    /// `CMSG_SET_TRADE_ITEM`, `CMSG_CLEAR_TRADE_ITEM` and `CMSG_SET_TRADE_GOLD` we send
    /// (`0x5d4442`, `0x5d44bc`, `0x5d45a5`, `0x5d463b`, `0x5d4712`). An own-side status whose
    /// sequence is below it is stale (`0x5d49ba`).
    own_edits: u32,
    /// Each side's spell as `0x4bf2b0` last compared it (`0xb71738`, `0xb7173c`), indexed by the
    /// header's side byte; zero for a fresh window (`0x4bf5cc`).
    spell_seen: [u32; 2],
    /// The enchant slot's change signals owed to [`feed_trade`], by side byte, in arrival order:
    /// side 0's is `TRADE_TARGET_ITEM_CHANGED(7)` (`0x4bf452`), side 1's
    /// `TRADE_PLAYER_ITEM_CHANGED(7)` (`0x4bf487`).
    enchant_changed: Vec<bool>,
    /// Owed lines whose `%s` names a player, parked until [`NameCache`] answers. The reference
    /// prints `ERR_INITIATE_TRADE_S` from its name cache in the sender (`0x5d4031`: `0x5d4042` on
    /// a hit, the query callback `0x5d4080` at `0x5d40ca` on a miss); the lines survive
    /// [`Self::close_window`], since that callback hangs off the name query, not the trade.
    named_lines: Vec<NamedLine>,
}

/// A line owed whose `%s` names a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NamedLine {
    /// The `GlobalStrings` key; the catalog names its surface.
    pub(crate) key: &'static str,
    /// Whose name fills the `%s`.
    pub(crate) who: u64,
    /// Print only while the player is streamed: the status arms read the live `CGUnit`
    /// (`0x609210`) after `0x468460(guid, TYPEMASK_PLAYER)`, while `ERR_INITIATE_TRADE_S` goes
    /// through the name cache `0x55f080` with no object test.
    pub(crate) needs_live_object: bool,
}

impl TradeSession {
    /// Our `CMSG_INITIATE_TRADE` goes out: a fresh session with the target as partner.
    pub(crate) fn initiate(&mut self, target: u64) {
        *self = TradeSession {
            partner: Some(target),
            initiated: true,
            named_lines: vec![NamedLine {
                key: "ERR_INITIATE_TRADE_S",
                who: target,
                needs_live_object: false,
            }],
            ..Default::default()
        };
    }

    /// `BEGIN_TRADE`: record the request beside any live session, unanswered;
    /// [`answer_trade_request`] replies, and only [`Self::begin`] resets the session.
    pub(crate) fn request(&mut self, initiator: u64) {
        self.request = Some(initiator);
    }

    pub(crate) fn pending_request(&self) -> Option<u64> {
        self.request
    }

    /// Spend the pending request alone: refusing an offer does not cancel the trade you are in.
    pub(crate) fn refuse_request(&mut self) {
        self.request = None;
    }

    /// Accept the pending request as a fresh session; the server then opens both windows.
    pub(crate) fn begin(&mut self, partner: u64) {
        *self = TradeSession {
            partner: Some(partner),
            ..Default::default()
        };
    }

    /// `OPEN_WINDOW`: the feed's open edge fires `TRADE_SHOW`; the own-edit counter starts at 1.
    pub(crate) fn open_window(&mut self) {
        self.open = true;
        self.own_edits = 1;
    }

    /// Send one of our offer edits and count it, as the reference's five senders do (item set
    /// `0x5d4380`, clear `0x5d4480`, money add `0x5d4500`, subtract `0x5d45f0`, set `0x5d4680`):
    /// the counter moves with the packet and with nothing else, so an edit that is refused
    /// before it goes out leaves it alone.
    pub(crate) fn send_edit(&mut self, commands: &NetCommands, command: ClientCommand) {
        self.own_edits = self.own_edits.wrapping_add(1);
        let _ = commands.0.send(command);
    }

    /// `SMSG_TRADE_STATUS_EXTENDED`: one side's new offer, which drops both accepts. A status for
    /// our own column below the own-edit counter is dropped whole (`0x5d49ba` returns before the
    /// store, the invalidations and `0x4bf2b0`'s accept reset): it answers an offer we have since
    /// edited.
    pub(crate) fn set_offer(&mut self, ext: &TradeStatusExtended) {
        if !ext.their_window && ext.sequence < self.own_edits {
            debug!(
                target: "trade",
                "stale own-side status dropped: sequence {} < {} edits",
                ext.sequence,
                self.own_edits,
            );
            return;
        }
        // Our side's slot that held an item and comes back empty is invalidated (`0x5d4ad3`,
        // `0x5d4b33`).
        if !ext.their_window {
            for (i, (old, new)) in self.our.slots.iter().zip(ext.slots.iter()).enumerate() {
                if old.is_some() && new.is_none() {
                    self.invalidated.push(i as u8);
                }
            }
        }
        let side = if ext.their_window {
            &mut self.their
        } else {
            &mut self.our
        };
        side.slots = ext.slots;
        side.gold = ext.gold;
        side.enchant_spell_id = ext.enchant_spell_id;
        // `0x4bf2b0`'s two compares after the slots (`0x4bf430`, `0x4bf464`): only this side's
        // spell can have moved.
        let seen = &mut self.spell_seen[usize::from(ext.their_window)];
        if *seen != ext.enchant_spell_id {
            *seen = ext.enchant_spell_id;
            self.enchant_changed.push(ext.their_window);
        }
        self.our_accept = false;
        self.their_accept = false;
    }

    /// Place our item in trade slot `id_1based` client-side: vmangos sends a placement to the
    /// partner only (`TradeData.cpp:81`), never back to the placer.
    pub(crate) fn place_own_item(&mut self, id_1based: u32, item: TradeItem, flags: u32) {
        if let Some(slot) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our.slots.get_mut(i as usize))
        {
            *slot = Some(item);
            self.our.object_flags[id_1based as usize - 1] = flags;
            self.our_offer_changed();
        }
    }

    /// Clear our trade slot `id_1based` client-side.
    pub(crate) fn clear_own_item(&mut self, id_1based: u32) {
        if let Some(slot) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our.slots.get_mut(i as usize))
        {
            *slot = None;
            self.our_offer_changed();
        }
    }

    /// Set our gold client-side: vmangos sends it to the partner only (`TradeData.cpp:118`).
    pub(crate) fn set_own_gold(&mut self, copper: u32) {
        self.our.gold = copper;
        self.our_offer_changed();
    }

    /// Any change to our offer drops both accepts, as vmangos does.
    fn our_offer_changed(&mut self) {
        self.our_accept = false;
        self.their_accept = false;
    }

    /// `TRADE_ACCEPT`: the partner pressed Trade.
    pub(crate) fn partner_accepted(&mut self) {
        self.their_accept = true;
    }

    /// `BACK_TO_TRADE`: an accept was withdrawn (an offer change, the partner's un-accept, or the
    /// 200 ms scam-delay bounce), so both drop.
    pub(crate) fn back_to_trade(&mut self) {
        self.our_accept = false;
        self.their_accept = false;
    }

    /// We pressed Trade: the optimistic local accept beside `CMSG_ACCEPT_TRADE`.
    pub(crate) fn accept(&mut self) {
        self.our_accept = true;
    }

    /// Our accept flag, the reference's `[0xb71730]`, which `0x4bf230` reads before it changes it
    /// or sends `CMSG_UNACCEPT_TRADE`.
    pub(crate) fn we_accepted(&self) -> bool {
        self.our_accept
    }

    /// `CancelTradeAccept`: the local side of `CMSG_UNACCEPT_TRADE`.
    pub(crate) fn unaccept(&mut self) {
        self.our_accept = false;
    }

    fn take_invalidated(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.invalidated)
    }

    fn take_enchant_changed(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.enchant_changed)
    }

    /// Park a line that names a player.
    pub(crate) fn owe_named_line(&mut self, line: NamedLine) {
        self.named_lines.push(line);
    }

    /// Take everything owed; the caller parks back what it could not name yet.
    fn take_named_lines(&mut self) -> Vec<NamedLine> {
        std::mem::take(&mut self.named_lines)
    }

    /// The name a `%s` status line carries, or `None` when the reference prints nothing: cases 0,
    /// 5 and 14 read the guid cell `[0xc4bed0]` under the latch `[0xc4bec8]`, both of which
    /// `InitiateTrade` (`0x5d3fb0`) sets, while the `BEGIN_TRADE` arm (`0x5d4815`) fills only the
    /// cell. So a refuser stays silent when vmangos echoes `IGNORE_YOU` to both sides
    /// (`TradeHandler.cpp:46`). The live-object guard rides [`NamedLine::needs_live_object`].
    pub(crate) fn line_names(&self) -> Option<u64> {
        self.initiated.then_some(self.partner).flatten()
    }

    /// The window's state cleared, as the close core does (`0x4bf516`): a full reset, a superset
    /// of [`Self::clear_pending`], keeping [`Self::named_lines`], which hang off the name query.
    pub(crate) fn close_window(&mut self) {
        *self = TradeSession {
            named_lines: std::mem::take(&mut self.named_lines),
            ..Default::default()
        };
    }

    /// The status handler's tail (`0x5d490a`): clear the initiate latch `[0xc4bec8]` and the
    /// pending guid `[0xc4bed0]`, on every code but 1, 2, 4, 7, 9 and 22, after the arm has read
    /// them. It never touches the window. `partner` stands for the pending guid only while the
    /// window is shut; once open it is the window partner `[0xb71728]`, which stays.
    pub(crate) fn clear_pending(&mut self) {
        self.initiated = false;
        self.request = None;
        if !self.open {
            self.partner = None;
        }
    }

    /// `TRADE_REJECTED` (case 9, `0x4bf821`): drop the partner's accept and nothing else; vmangos
    /// never sends it.
    pub(crate) fn partner_unaccepted(&mut self) {
        self.their_accept = false;
    }

    /// `ONLY_CONJURED` (case 22, `0x4bf9ec` → `0x4bfbd0`): empty the offending slot of our offer,
    /// or the gold for `0xff`; the feed's diff raises the repaint events. Deviation: an
    /// out-of-range slot is dropped, where the reference indexes its 7-slot mirror unchecked,
    /// because an out-of-bounds write is not a behaviour to reproduce.
    pub(crate) fn bounce_own_offer(&mut self, slot: u8) {
        const MONEY: u8 = 0xff;
        if slot == MONEY {
            self.our.gold = 0;
        } else if let Some(cell) = self.our.slots.get_mut(usize::from(slot)) {
            *cell = None;
            self.invalidated.push(slot);
        }
    }

    /// Both money offers zero (`0xc4be70`, `0xc4be74`), before the dispatcher, on the statuses that
    /// end a trade: cancel, complete, close-window and code 13 (`0x5d4884`-`0x5d48a2`,
    /// `0x5d48eb`-`0x5d48ff`).
    pub(crate) fn zero_gold(&mut self) {
        self.our.gold = 0;
        self.their.gold = 0;
    }

    /// Disconnect: a full reset, owed signals included; a session teardown is not a trade event.
    pub(crate) fn clear_session(&mut self) {
        *self = TradeSession::default();
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    // ── Test accessors, for `net`'s tests ──

    #[cfg(test)]
    pub(crate) fn take_named_lines_for_test(&mut self) -> Vec<NamedLine> {
        self.take_named_lines()
    }

    #[cfg(test)]
    pub(crate) fn partner_has_accepted(&self) -> bool {
        self.their_accept
    }

    /// Our own offer as `(gold, which slots are filled)`.
    #[cfg(test)]
    pub(crate) fn own_offer_for_test(&self) -> (u32, [bool; TRADE_SLOT_COUNT]) {
        (self.our.gold, self.our.slots.map(|s| s.is_some()))
    }
}

/// The partner's cell `[0xb71728]`, set only while the window is open; `CloseTrade` (`0x4bfdc0`)
/// enters the core.
impl LatchedWindow for TradeSession {
    const WINDOW: NpcWindow = NpcWindow::Trade;

    fn cell(&self) -> Option<u64> {
        self.open.then_some(self.partner).flatten()
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }

    fn take_close_verb(script: &mut UiScript) -> bool {
        script.take_trade_close()
    }
}

/// `OPEN_WINDOW`'s open, `0x4bf4e0` with the pending partner (`0x4bf7f8`): the latch first, with
/// no test of the cell (`0x4bf53e`), so the window a trade opens over, an NPC's or a trade's,
/// closes first; then the cell and a fresh offer (`0x4bf549`-`0x4bf5ea`). The feed fires
/// `TRADE_SHOW` (`0x4bf644`). With no partner the call is the close (`0x4bf4ed`).
pub(crate) fn open_window(world: &mut World) {
    let Some(partner) = world.resource::<TradeSession>().partner else {
        close_core(world);
        return;
    };
    crate::ui_session::register(world, partner);
    let mut trade = world.resource_mut::<TradeSession>();
    trade.partner = Some(partner);
    trade.open_window();
}

/// The close core, `0x4bf4e0` with a zero guid: with a window open, the latch release
/// (`0x4bf50a`), the cell cleared and `TRADE_CLOSED` (`0x4bf522`); then `CMSG_CANCEL_TRADE`
/// (`0x4bf527` → `0x5d4310`), open or not. Entered while the partner holds the latch, the
/// release's `0x493310` runs this once more inside it (`0x493365`), so a direct close fires
/// `TRADE_CLOSED` twice and sends the cancel twice.
pub(crate) fn close_core(world: &mut World) {
    drain_trade_touches(world);
    if let Some(partner) = world.resource::<TradeSession>().cell() {
        crate::ui_session::release(world, partner);
        world.resource_mut::<TradeSession>().close_window();
        crate::ui_session::note_closed(world, NpcWindow::Trade);
        // The cell clears and the signal fires (`0x4bf516`-`0x4bf522`); both offers stay for the
        // handlers' getters, and for the next open to clear.
        crate::ui_session::fire(world, |script| {
            script.close_trade_window();
            script.fire_event("TRADE_CLOSED", vec![]);
        });
    }
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net.0.send(ClientCommand::CancelTrade);
    }
}

/// The status dispatcher's three closing arms (`0x4bf720`): `CANCELED` signals
/// `TRADE_REQUEST_CANCEL` (`0x4bf832`), and each runs `UnlockTradeItems` (`0x4bf837`, `0x4bf859`,
/// `0x4bf87e`) before the close core (`0x4bf842`, `0x4bf867`, `0x4bf89c`).
pub(crate) fn server_close(world: &mut World, canceled: bool) {
    drain_trade_touches(world);
    crate::ui_session::fire(world, |script| {
        // The statuses zero both offers before any of it (`0x5d4884`-`0x5d48a2`).
        script.zero_trade_gold();
        if canceled {
            script.fire_event("TRADE_REQUEST_CANCEL", vec![]);
        }
        script.unlock_trade_items();
    });
    close_core(world);
}

/// The leave-world sweep's trade leg (`0x490adf` → `0x4bf220` → `0x5d4310`): `CMSG_CANCEL_TRADE`,
/// sent whether or not a trade is open; vmangos ignores one with no trade
/// (`TradeHandler.cpp:560-565` → `Player.cpp:11663`). The window itself stays until the sweep's
/// interaction close (`0x490b5f` → `0x493310` → `0x493365`) runs the core on the latched partner.
fn cancel_trade_on_leave_world(commands: Res<NetCommands>) {
    let _ = commands.0.send(ClientCommand::CancelTrade);
}

pub(crate) struct UiTradePlugin;

/// Mirrors the `BlockTrades` CVar into [`BlockTrades`].
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut block: ResMut<BlockTrades>) {
    if ev.is("BlockTrades") {
        block.0 = ev.flag();
    }
}

impl Plugin for UiTradePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        net::register(app);
        // A UI-only harness has no net stack; declaring a message twice is idempotent.
        app.add_message::<crate::item_watch::ItemNotice>();
        app.init_resource::<TradeSession>()
            .init_resource::<TradeTouchCursor>()
            .init_resource::<BlockTrades>()
            .add_latched_window::<TradeSession>()
            .add_sweep_leg(SweepLeg::Trade, cancel_trade_on_leave_world)
            .add_systems(
                Update,
                (
                    // The feed runs after `UnitFeed`, whose item templates it reads, and before
                    // the input pass, with the drain after it, so each lands the same frame. The
                    // latch's watchdog leashes the partner at 14 yd.
                    feed_trade.after(crate::ui_unit::UnitFeed).in_set(UiFeed),
                    // Each item handler runs the action bar's recount before the trade scan
                    // (`0x5d94e1` then `0x5d94f0`).
                    resignal_trade_items
                        .in_set(crate::ui_unit::UnitFeed)
                        .after(crate::ui_action::recount_item_actions),
                    // Ahead of the feed, so an accepted partner shows the frame the window opens.
                    answer_trade_request
                        .after(crate::player::ControlRestore)
                        .before(feed_trade),
                    // The coinage-change reflex's trade half, after the world's fields land.
                    trim_offer_to_purse
                        .after(crate::ui_unit::UnitFeed)
                        .before(feed_trade),
                    drain_trade.after(UiInput),
                ),
            );
    }
}

/// 1.12's `BlockTrades` CVar (`0x842fbc`), the Block Trades checkbox (`UIOptionsFrame.lua:11`).
/// A resource, not a [`TradeSession`] field, because a disconnect clears the session; read only
/// by [`answer_trade_request`]'s leg 8.
#[derive(Resource, Default)]
pub(crate) struct BlockTrades(pub(crate) bool);

/// `0x468460(guid, TYPEMASK_PLAYER)`: the guid's live object, `None` when it is not a streamed
/// player. Asked by the ladder's leg 3 (`0x4bf779`) and by the `%s` lines' second guard.
fn streamed_player<'a>(
    index: &GuidIndex,
    stores: &'a Query<&ObjectStore>,
    guid: u64,
) -> Option<&'a ObjectStore> {
    if !benilla_protocol::guid::is_player(guid) {
        return None;
    }
    index.0.get(&guid).and_then(|e| stores.get(*e).ok())
}

/// Answer an incoming trade request with the reference's eight-leg ladder (`0x4bf736`, case 1 of
/// `0x4bf720`, over `[0x4bf736, 0x4bf7f8)`); only leg 8 prints, so the order is behaviour. A
/// system, not a packet handler, because leg 8 may wait on a name query. There is no consent
/// step: 1.12 registers `TRADE_REQUEST` (event `0x11d`, slot `0xbe160c`) and never signals it,
/// so the stock `TRADE` popup never shows and a request that passes opens the window unasked.
fn answer_trade_request(
    mut trade: ResMut<TradeSession>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    social: Res<crate::ui_social::SocialState>,
    cinematic: Res<crate::cinematic::Cinematic>,
    player: Res<crate::player::Player>,
    auction: Res<crate::ui_auction::AuctionOpen>,
    block_trades: Res<BlockTrades>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    let Some(initiator) = trade.pending_request() else {
        return;
    };

    // Legs 1 and 3 send nothing: our own initiate is live (`[0xc4bec8]`), or the initiator is no
    // streamed player. Leg 3 is asked before leg 2 here, while the reference asks the ignore list
    // first (`0x4bf759` precedes `0x4bf779`); the two differ for an ignored, unstreamed initiator.
    let initiator_store = streamed_player(&index, &stores, initiator);
    if trade.initiated || initiator_store.is_none() {
        info!(
            target: "trade",
            "BEGIN_TRADE from {initiator:#x} dropped without a reply (own initiate in flight: {}, \
             initiator resolved: {})",
            trade.initiated,
            initiator_store.is_some(),
        );
        trade.refuse_request();
        return;
    }

    // Leg 2, the ignore list (`0x5ae5a0`): `CMSG_IGNORE_TRADE` (`0x4bf759` → `0x5d41c0`), read by
    // the initiator as "is ignoring you"; vmangos has no such check (`TradeHandler.cpp:567`).
    if social.is_ignored(initiator) {
        info!(target: "trade", "BEGIN_TRADE from an ignored {initiator:#x}; sending CMSG_IGNORE_TRADE");
        let _ = commands.0.send(ClientCommand::IgnoreTrade);
        trade.refuse_request();
        return;
    }

    // Legs 4-7, a silent busy: a dead or ghost initiator (`0x605f30`; `unit_reads_dead`, like
    // `UnitIsDead` `0x517ac0`, would refuse a feigning hunter), a cinematic (`[0xb4e310]`), lost
    // player control (`[0xb4b3e4]`, which `0x4958e0` writes, not an in-world flag) or an open
    // auction house (`[0xb725f8]`, its auctioneer, not a pending trade).
    let dead_or_ghost = initiator_store.is_some_and(|s| s.0.is_dead_or_ghost());
    if dead_or_ghost
        || cinematic.is_playing()
        || player.own_control_lost
        || auction.auctioneer.is_some()
    {
        info!(target: "trade", "BEGIN_TRADE from {initiator:#x} refused silently; sending CMSG_BUSY_TRADE");
        let _ = commands.0.send(ClientCommand::BusyTrade);
        trade.refuse_request();
        return;
    }

    // Leg 8, Block Trades, the one leg that names the initiator. The reference reads the live
    // `CGUnit` (`0x609210`); this may wait on a name query, with no timer, since vmangos cancels
    // the trade to us when the initiator leaves (`Player.cpp:11747`), which clears the request.
    if block_trades.0 {
        let Some(name) = names
            .resolve_player(initiator, &commands)
            .map(str::to_string)
        else {
            return;
        };
        info!(target: "trade", "BEGIN_TRADE from {name} refused by BlockTrades; sending CMSG_BUSY_TRADE");
        let _ = commands.0.send(ClientCommand::BusyTrade);
        trade.refuse_request();
        // `0x496720(0xbb, name)`: one `DisplayError`; catalog row `0xbb` makes it a chat line.
        errors
            .0
            .push(crate::ui_action::UiError::s("ERR_TRADE_BLOCKED_S", name));
        return;
    }

    // Nothing refuses it: accept, the bottom of the ladder.
    info!(target: "trade", "BEGIN_TRADE from {initiator:#x} accepted; sending CMSG_BEGIN_TRADE");
    trade.begin(initiator);
    let _ = commands.0.send(ClientCommand::BeginTrade);
}

/// A wire [`TradeItem`] as the Lua row: name and quality from the item-template cache once it
/// answers, the icon from `display_id` via `ItemDisplayInfo.dbc`; [`resolve_side`] names slot 7's
/// `enchantment`.
fn resolve_slot(
    item: &TradeItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    names: &NameCache,
    commands: &NetCommands,
) -> TradeSlotItem {
    let template = items.template(item.entry, 0, commands);
    let name = template.map(|t| t.name.clone());
    let quality = template.map(|t| t.quality);
    let inventory_type = template.map_or(0, |t| t.inventory_type);
    // The icon comes from the template's display, not the wire's (`0x5d88b0([rec+0x18])`,
    // `0x4c01f0`; `0x5d8af0` on our side, `0x4c0529`); a cached template whose display has no
    // icon falls to the question mark, and an unanswered one leaves it to the getter.
    let texture = template.map(|t| {
        icons
            .and_then(|i| i.catalog.get(t.display_info_id))
            .and_then(|d| d.icon.clone())
            .unwrap_or_else(|| crate::ui_action::MISSING_ITEM_ICON.to_string())
    });
    // Nil until the template answers: the link embeds the name and the quality colour.
    let link = match (name.as_deref(), quality) {
        (Some(n), Some(q)) => Some(crate::ui_items::item_link(item.entry, n, q)),
        _ => None,
    };
    // Only a wrapped gift asks for its giver, and a zero guid asks nothing (`0x52e288`,
    // `0x52e29d`).
    let gift_creator = (item.wrapped && item.gift_creator != 0)
        .then(|| names.resolve_player(item.gift_creator, commands))
        .flatten()
        .map(str::to_string);
    TradeSlotItem {
        item_id: item.entry,
        name,
        texture,
        count: item.count.max(1),
        quality,
        inventory_type,
        lock_id: item.lock_id,
        object_flags: None,
        wrapped: item.wrapped,
        gift_creator,
        enchantment: None,
        link,
    }
}

/// Our bag item at cursor-space `(bag, 1-based slot)` as a wire [`TradeItem`], for our own column
/// only; `None` for an empty or unstreamed slot. The item stays in the bag until the trade ends.
fn own_item_at(
    bag: i64,
    slot: u32,
    store: Option<&ObjectStore>,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> Option<(TradeItem, u32)> {
    let (guid, count) = crate::ui_items::slot_guid_count(store, bag, slot, objects);
    if guid == 0 {
        return None;
    }
    own_item(guid, count, objects, items, commands)
}

/// Our item object `guid` as a wire [`TradeItem`] of `count`, with its `ITEM_FIELD_FLAGS`.
fn own_item(
    guid: u64,
    count: u32,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> Option<(TradeItem, u32)> {
    let fields = objects.object(guid)?;
    let entry = fields.object_entry()?;
    let flags = fields.item_flags().unwrap_or(0);
    // The placed item's WRAPPED flag and giver, which its slot's tooltip reads off the object
    // (`0x52b789`, `0x52e32a`).
    let wrapped = flags & benilla_protocol::messages::ITEM_DYNFLAG_WRAPPED != 0;
    let display_id = items
        .template(entry, guid, commands)
        .map(|t| t.display_info_id)
        .unwrap_or(0);
    Some((
        TradeItem {
            entry,
            display_id,
            count,
            wrapped,
            gift_creator: fields.item_gift_creator().unwrap_or(0),
            perm_enchant: 0,
            creator: 0,
            charges: 0,
            suffix_factor: 0,
            random_prop_id: 0,
            lock_id: 0,
            max_durability: 0,
            durability: 0,
        },
        flags,
    ))
}

/// The enchant slot, the seventh: the only row with an enchantment.
const ENCHANT_SLOT: usize = TRADE_SLOT_COUNT - 1;

/// `enchant` is the name the column's slot 7 shows: the `Spell.dbc` name of the spell cast on it,
/// read for a filled slot only (`0x4c02b0`, `0x4c05c6`).
fn resolve_side(
    offer: &TradeOffer,
    ours: bool,
    enchant: Option<&str>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    names: &NameCache,
    commands: &NetCommands,
) -> TradeSideState {
    let mut slots: [Option<TradeSlotItem>; TRADE_SLOT_COUNT] = Default::default();
    for (i, s) in offer.slots.iter().enumerate() {
        slots[i] = s.map(|it| {
            let mut row = resolve_slot(&it, items, icons, names, commands);
            // Our items are live objects; the partner's are blocks (`0x5342ed`, `0x53456b`).
            row.object_flags = ours.then_some(offer.object_flags[i]);
            if i == ENCHANT_SLOT {
                row.enchantment = enchant.map(str::to_string);
            }
            row
        });
    }
    TradeSideState {
        slots,
        gold: offer.gold,
    }
}

fn snapshot(
    trade: &TradeSession,
    items: &Items,
    icons: Option<&ItemDisplays>,
    spells: Option<&crate::ui_action::Spells>,
    names: &NameCache,
    commands: &NetCommands,
) -> Option<TradeState> {
    if !trade.open {
        return None;
    }
    // A spell id with no positive `Spell.dbc` row names nothing (`0x4c02be`-`0x4c02d5`).
    let name = |id: u32| {
        (id != 0)
            .then(|| spells?.catalog.get(id))
            .flatten()
            .map(|d| d.name.as_str())
    };
    Some(TradeState {
        // The partner's window carries the spell they cast on our slot 7 (`0x4c05d1`, side 1).
        player: resolve_side(
            &trade.our,
            true,
            name(trade.their.enchant_spell_id),
            items,
            icons,
            names,
            commands,
        ),
        // Our window carries the spell we cast on theirs (`0x4c02b7`, side 0).
        target: resolve_side(
            &trade.their,
            false,
            name(trade.our.enchant_spell_id),
            items,
            icons,
            names,
            commands,
        ),
    })
}

/// Where the trade scan has read the item notices to, shared by its scheduled run and the close
/// paths' early one so no notice is scanned twice.
#[derive(Resource, Default)]
pub(crate) struct TradeTouchCursor(
    bevy::ecs::message::MessageCursor<crate::item_watch::ItemNotice>,
);

/// Run the trade scan ([`resignal_trade_items`]) on the notices not yet read, ahead of a close: the
/// destroys of the traded items arrive before the close in the packet stream, and their scan ran
/// at their turn in the reference.
fn drain_trade_touches(world: &mut World) {
    if world.get_resource::<TradeTouchCursor>().is_none()
        || world
            .get_resource::<Messages<crate::item_watch::ItemNotice>>()
            .is_none()
    {
        return;
    }
    let _ = world.run_system_cached(resignal_trade_items);
}

/// The item watchers' trade half, after the action bar's recount in each handler: an item of
/// ours that one of our slots holds re-signals the slot (`0x4bfb10`, `0x4bfb60` into `0x4bfa70`).
/// Gone, the slot clears with `CMSG_CLEAR_TRADE_ITEM` (`0x5d4480`), as a click's clear does;
/// restacked or enchanted, the same item is placed again with `CMSG_SET_TRADE_ITEM` from the
/// position it was placed from (`0x5d4380`), our session's offer left as it is. vmangos refuses an
/// item its trade already holds and cancels the trade (`TradeHandler.cpp:716-720`), as it does
/// for the reference's send. Neither scan asks whether a trade is open.
///
/// The scan reads the notices through [`TradeTouchCursor`], which the close paths share
/// ([`drain_trade_touches`]): vmangos sends the traded items' destroys before `TRADE_COMPLETE`
/// (`TradeHandler.cpp:469`, `Player.cpp:10710`, then `:526`), the destroy releases at once
/// (`0x4674cf` → `0x464920`), so the clears and `TRADE_PLAYER_ITEM_CHANGED` belong ahead of the
/// close, not a stage after it.
fn resignal_trade_items(
    script: Option<NonSendMut<UiScript>>,
    mut cursor: ResMut<TradeTouchCursor>,
    messages: Res<Messages<crate::item_watch::ItemNotice>>,
    mut trade: ResMut<TradeSession>,
    commands: Res<NetCommands>,
    objects: Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    names: Res<NameCache>,
) {
    use crate::item_watch::TradeTouch;
    let touches: Vec<TradeTouch> = cursor
        .0
        .read(&messages)
        .filter_map(crate::item_watch::ItemNotice::trade)
        .collect();
    let Some(mut script) = script else {
        return;
    };
    for touch in touches {
        match touch {
            TradeTouch::Gone(guid) => {
                let Some((slot, _, _)) = script.trade_slot_holding(guid) else {
                    continue;
                };
                info!(target: "trade", "item {guid:#x} gone from slot {}; sending CMSG_CLEAR_TRADE_ITEM", slot + 1);
                trade.send_edit(
                    &commands,
                    ClientCommand::ClearTradeItem { trade_slot: slot },
                );
                trade.clear_own_item(u32::from(slot) + 1);
                script.empty_trade_slot(slot);
            }
            TradeTouch::Changed(guid) => {
                let Some((slot, bag, bag_slot)) = script.trade_slot_holding(guid) else {
                    continue;
                };
                // The send refuses with no active player or no item object (`0x5d43af`,
                // `0x5d43da`), and then nothing is stored or signalled (`0x4bfab3`).
                let count = objects
                    .object(guid)
                    .and_then(|f| f.item_stack_count())
                    .unwrap_or(1);
                let (Some(_), Some((item, flags)), Some((wire_bag, wire_slot))) = (
                    objects.player(),
                    own_item(guid, count, &objects, &items, &commands),
                    crate::ui_items::wire_pos(bag, bag_slot),
                ) else {
                    continue;
                };
                info!(target: "trade", "item {guid:#x} in slot {} changed; sending CMSG_SET_TRADE_ITEM", slot + 1);
                trade.send_edit(
                    &commands,
                    ClientCommand::SetTradeItem {
                        trade_slot: slot,
                        bag: wire_bag,
                        slot: wire_slot,
                    },
                );
                let mut row = resolve_slot(&item, &items, icons.as_deref(), &names, &commands);
                row.object_flags = Some(flags);
                script.replace_trade_slot(slot, row);
            }
        }
    }
}

/// Push the trade into the VM and fire its events on a change, diffed against `Local` memory;
/// `TRADE_ACCEPT_UPDATE` fires after `TRADE_SHOW`, whose handler hides the highlights.
fn feed_trade(
    script: Option<NonSendMut<UiScript>>,
    mut trade: ResMut<TradeSession>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    // The `%s` lines' live-object guard, the same reads as the ladder's leg 3.
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    mut last: crate::ui_script::WindowMemo<Option<TradeState>>,
    mut last_open: crate::ui_script::WindowMemo<bool>,
    mut last_accept: crate::ui_script::WindowMemo<(bool, bool)>,
    mut last_player_gold: crate::ui_script::WindowMemo<u32>,
    mut last_their_gold: crate::ui_script::WindowMemo<u32>,
    mut closes: CoreCloses,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_open = last_open.get(&script);
    let last_accept = last_accept.get(&script);
    let last_player_gold = last_player_gold.get(&script);
    let last_their_gold = last_their_gold.get(&script);
    if closes.since_last(&script, NpcWindow::Trade) {
        *last = None;
        *last_open = false;
        *last_accept = (false, false);
        *last_player_gold = 0;
        *last_their_gold = 0;
    }

    let fresh = snapshot(
        &trade,
        &items,
        icons.as_deref(),
        spells.as_deref(),
        &names,
        &commands,
    );
    // A close is the core's, which shut the VM's window and left both offers in place; a
    // disconnect's clear tells it nothing.
    let opened = !*last_open && trade.is_open();
    if opened {
        // The open clears both offers and our item table before anything is placed (`0x4bf549`).
        script.open_trade_window();
        *last = None;
    }
    let changed = fresh != *last;
    // Our column as `ClickTradeButton` left it: its write signalled in place (`0x4bfaef`). A slot
    // whose source has not moved since the last push keeps the VM's value, so a click this
    // feed's store has not caught up with (or never will, when the bag item is not streamed) is
    // not rolled back.
    let held_player = script.trade_player_slots();
    let source_moved = |i: usize| {
        let new = fresh.as_ref().map(|f| &f.player.slots[i]);
        let old = last.as_ref().map(|l| &l.player.slots[i]);
        new != old
    };
    if changed {
        let mut pushed = fresh.clone();
        if let (Some(p), Some(held), Some(_)) = (pushed.as_mut(), held_player.as_ref(), &*last) {
            for (i, slot) in p.player.slots.iter_mut().enumerate() {
                if !source_moved(i) {
                    *slot = held[i].clone();
                }
            }
        }
        script.set_trade(pushed);
    }
    // The per-slot events the stock `TradeFrame.lua` repaints one slot on, arg1 the 1-based slot
    // (`0x4bf414`/`0x4bf452` target, `0x4bf487`/`0x4bfaef` player). An offer change signals
    // nothing else: `TRADE_UPDATE` has one producer, the getters' record callback
    // ([`run_record_callbacks`]).
    // `InvalidateTradeSlot` (`0x4bfbd0`) on our slots: the unlock and its own
    // `TRADE_PLAYER_ITEM_CHANGED`, which the diff below then skips.
    let invalidated = trade.take_invalidated();
    for &slot in &invalidated {
        script.invalidate_trade_slot(slot);
    }
    if changed && trade.is_open() && !opened {
        let empty = TradeState::default();
        let old = last.as_ref().unwrap_or(&empty);
        if let Some(new) = fresh.as_ref() {
            // The record compare (`0x4bf2c7`-`0x4bf378`) reads the wire's item fields, run only on a
            // status: never the enchant slot's spell, which signals on its own below, and never
            // what the template cache resolved (name, icon, quality, link), so a template
            // landing repaints through `TRADE_UPDATE` and signals no slot.
            let record =
                |s: &Option<TradeSlotItem>| s.as_ref().map(|s| (s.item_id, s.count, s.lock_id));
            let changed_slots = |mine: &[Option<TradeSlotItem>],
                                 theirs: &[Option<TradeSlotItem>]| {
                mine.iter()
                    .zip(theirs.iter())
                    .enumerate()
                    .filter(|(_, (a, b))| record(a) != record(b))
                    .map(|(i, _)| ScriptValue::Int(i as i64 + 1))
                    .collect::<Vec<_>>()
            };
            // A moved source fires only where it differs from the VM's column by the item each
            // slot holds, so the store catching up with a click, its richer row included, is no
            // change.
            let held = held_player.as_ref().unwrap_or(&old.player.slots);
            let item = |s: &Option<TradeSlotItem>| s.as_ref().map(|s| (s.item_id, s.count));
            let player_changed = new
                .player
                .slots
                .iter()
                .zip(held.iter())
                .enumerate()
                .filter(|(i, (a, b))| source_moved(*i) && item(a) != item(b))
                .map(|(i, _)| ScriptValue::Int(i as i64 + 1));
            for slot in player_changed {
                if matches!(slot, ScriptValue::Int(n) if invalidated.iter().any(|&i| i64::from(i) + 1 == n))
                {
                    continue;
                }
                script.fire_event("TRADE_PLAYER_ITEM_CHANGED", vec![slot]);
            }
            for slot in changed_slots(&new.target.slots, &old.target.slots) {
                script.fire_event("TRADE_TARGET_ITEM_CHANGED", vec![slot]);
            }
        }
    }
    // The enchant slot's spell, after the partner's slots (`0x4bf429` on): the spell we cast
    // repaints their slot 7, the one they cast repaints ours.
    for partner_window in trade.take_enchant_changed() {
        let event = if partner_window {
            "TRADE_PLAYER_ITEM_CHANGED"
        } else {
            "TRADE_TARGET_ITEM_CHANGED"
        };
        script.fire_event(event, vec![ScriptValue::Int(ENCHANT_SLOT as i64 + 1)]);
    }
    // The owed `%s` lines, resolved here where the name cache is; a miss waits for the query it
    // started. The catalog row, read by `ui_action::feed_actions`, decides where each lands.
    for line in trade.take_named_lines() {
        // The status arms print nothing once the player is no longer an object.
        if line.needs_live_object && streamed_player(&index, &stores, line.who).is_none() {
            continue;
        }
        match names
            .resolve_player(line.who, &commands)
            .map(str::to_string)
        {
            Some(name) => errors.0.push(crate::ui_action::UiError::s(line.key, name)),
            // Not cached: `resolve` has asked, so park it and retry next frame.
            None => trade.owe_named_line(line),
        }
    }
    if opened {
        // `SetTradePartner`'s open leg (`0x4bf4e0`): coins held on the cursor fold into the offer
        // first and fire the two money events locally; the send goes out with the money intent.
        if let Some(offer) = script.fold_cursor_money_into_trade() {
            trade.set_own_gold(offer);
            script.fire_event("PLAYER_TRADE_MONEY", vec![]);
            script.fire_event("PLAYER_MONEY", vec![]);
        }
        script.fire_event("TRADE_SHOW", vec![]);
    }

    // `TRADE_ACCEPT_UPDATE(mine, theirs)` on any change while open.
    let accept = (trade.our_accept, trade.their_accept);
    if trade.is_open() && accept != *last_accept {
        script.fire_event(
            "TRADE_ACCEPT_UPDATE",
            vec![
                ScriptValue::Int(i64::from(accept.0)),
                ScriptValue::Int(i64::from(accept.1)),
            ],
        );
    }
    *last_accept = if trade.is_open() {
        accept
    } else {
        (false, false)
    };

    // Our gold reaches the money input through `PLAYER_TRADE_MONEY` (`TradeFrame.xml:565`), never
    // `TRADE_UPDATE`, so a repaint never overwrites what the player is typing.
    let player_gold = trade.our.gold;
    if trade.is_open() && player_gold != *last_player_gold {
        script.fire_event("PLAYER_TRADE_MONEY", vec![]);
    }
    *last_player_gold = if trade.is_open() { player_gold } else { 0 };
    // The partner's gold: `TRADE_MONEY_CHANGED`, which the `TARGET_TRADE` money frame repaints on
    // (`MoneyFrame.lua:133`). The reference fires it at `0x4bf4d6`, beside `PLAYER_TRADE_MONEY` at
    // `0x4bf4ab`; which side each reports is inferred from `MoneyFrame.lua`, not traced.
    let their_gold = trade.their.gold;
    if trade.is_open() && their_gold != *last_their_gold {
        script.fire_event("TRADE_MONEY_CHANGED", vec![]);
    }
    *last_their_gold = if trade.is_open() { their_gold } else { 0 };

    *last = fresh;
    *last_open = trade.is_open();
}

/// `TRADE_UPDATE`, whose one producer is the callback `0x4c0340` (`0x4c034f`) that the item
/// getters register when they find a filled slot whose template the cache lacks (`0x4c017f`,
/// `0x4c0582` into `0x55ba30`). A hit never calls it. Each such getter call registers one
/// callback, with no dedupe (`0x55bbb3`), and each runs when the record lands, for a ready record
/// only; a refusal frees them silent. The asks the script queued are registered here, after its
/// tick, and the landed records ([`crate::item_watch::ItemNotice::TradeRecord`]) signal, after
/// the feed pushed the answered rows. The waiters are the getters' own asks, which a frameless VM
/// never makes.
fn run_record_callbacks(
    script: &mut UiScript,
    items: &Items,
    notices: &mut MessageReader<crate::item_watch::ItemNotice>,
) {
    for entry in script.take_trade_record_asks() {
        // An answered entry is never asked again, so nothing could run its callback.
        if items.template_cached(entry).is_none() && !items.template_answered_unknown(entry) {
            items.wait_for_trade_record(entry);
        }
    }
    for notice in notices.read() {
        if matches!(notice, crate::item_watch::ItemNotice::TradeRecord { .. }) {
            script.fire_event("TRADE_UPDATE", vec![]);
        }
    }
}

/// Drain the Lua intents into the trade `CMSG`s; `CloseTrade` runs the close core. The item
/// getters' record callbacks ([`run_record_callbacks`]) ride the same after-the-tick pass.
fn drain_trade(
    script: Option<NonSendMut<UiScript>>,
    mut notices: MessageReader<crate::item_watch::ItemNotice>,
    mut trade: ResMut<TradeSession>,
    mut world_commands: Commands,
    commands: Res<NetCommands>,
    selection: Res<Selection>,
    group: Res<GroupState>,
    objects: Objects,
    items: Res<Items>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
) {
    let Some(mut script) = script else {
        return;
    };
    run_record_callbacks(&mut script, &items, &mut notices);
    // Our bag contents, for the own-column item resolve.
    let store = self_q.iter().next();

    for token in script.take_trade_initiates() {
        match resolve_trade_target(&token, &selection, &group) {
            Some(guid) => {
                info!(target: "trade", "initiate: {token:?} -> {guid:#x}; sending CMSG_INITIATE_TRADE");
                trade.initiate(guid);
                let _ = commands
                    .0
                    .send(ClientCommand::InitiateTrade { target: guid });
            }
            None => {
                info!(target: "trade", "initiate: {token:?} did not resolve to a player guid — nothing sent");
            }
        }
    }

    // The accept verbs need an open window; close always clears, even a stale session.
    if script.take_trade_accept() && trade.is_open() {
        trade.accept();
        let _ = commands.0.send(ClientCommand::AcceptTrade);
    }
    if script.take_trade_unaccept() && trade.is_open() {
        trade.unaccept();
        let _ = commands.0.send(ClientCommand::UnacceptTrade);
    }
    // Every money offer sends `CMSG_SET_TRADE_GOLD`, each counted: two verbs in one tick (stock's
    // over-purse clamp re-enters `SetTradeMoney`, `TradeFrame.lua:162-173`) are two sends. No
    // sender tests the window (`0x5d4680`, `0x5d4500`, `0x5d45f0`); vmangos drops one with no
    // trade (`TradeHandler.cpp:731-735`). It sends the partner only, so our own gold is set here.
    for copper in script.take_trade_money() {
        trade.set_own_gold(copper);
        info!(target: "trade", "set gold: {copper} copper; sending CMSG_SET_TRADE_GOLD");
        trade.send_edit(&commands, ClientCommand::SetTradeGold { copper });
    }
    // Placements and clears, window or none (`0x4bfa70`, `0x5d4380`, `0x5d4480` test none;
    // vmangos returns silently, `TradeHandler.cpp:668-690`): the UI id is 1-based, the wire slot
    // 0-based, and the cursor's bag and slot map to the wire position.
    for (id, bag, slot) in script.take_trade_set_items() {
        let Some((wire_bag, wire_slot)) = crate::ui_items::wire_pos(bag, slot) else {
            info!(target: "trade", "set item: slot {id} <- bag {bag}/{slot} has no wire position — dropped");
            // Unsent, so the VM's slot gives the item back.
            script.return_unsent_trade_item(id);
            continue;
        };
        if let Some(trade_slot) = id.checked_sub(1).and_then(|n| u8::try_from(n).ok()) {
            // vmangos echoes a placement to the partner only, so our column is filled here.
            if let Some((item, flags)) = own_item_at(bag, slot, store, &objects, &items, &commands)
            {
                trade.place_own_item(id, item, flags);
            }
            info!(target: "trade", "set item: slot {id} <- bag {bag}/{slot} (wire {wire_bag}/{wire_slot}); sending CMSG_SET_TRADE_ITEM");
            trade.send_edit(
                &commands,
                ClientCommand::SetTradeItem {
                    trade_slot,
                    bag: wire_bag,
                    slot: wire_slot,
                },
            );
        }
    }
    for id in script.take_trade_clear_items() {
        if let Some(trade_slot) = id.checked_sub(1).and_then(|n| u8::try_from(n).ok()) {
            trade.clear_own_item(id);
            info!(target: "trade", "clear item: slot {id}; sending CMSG_CLEAR_TRADE_ITEM");
            trade.send_edit(&commands, ClientCommand::ClearTradeItem { trade_slot });
        }
    }
    // `CloseTrade` (`0x4bfdc0`) runs the core, whose cancel goes out open or not (`0x4bf527`).
    if script.take_trade_close() {
        crate::ui_session::queue(&mut world_commands, close_core);
    }
    // The `TRADE` popup's verbs: an empty `0x117` or `0x11C` and no local teardown; the status
    // reply drives the window.
    if script.take_trade_begin() {
        let _ = commands.0.send(ClientCommand::BeginTrade);
    }
    if script.take_trade_cancel() {
        let _ = commands.0.send(ClientCommand::CancelTrade);
    }
}

/// The coinage-change reflex (`0x5ddf30`): an offer above the purse is trimmed to the purse.
fn trimmed_offer(offer: u32, purse: u32) -> Option<u32> {
    (offer > purse).then_some(purse)
}

/// On each `PLAYER_FIELD_COINAGE` change with a trade open, re-send an offer past the purse as an
/// absolute `CMSG_SET_TRADE_GOLD` of the purse; the coin sound and `PLAYER_MONEY` fire elsewhere.
fn trim_offer_to_purse(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut trade: ResMut<TradeSession>,
    commands: Res<NetCommands>,
    mut prev: Local<Option<u32>>,
) {
    let Some(money) = self_q
        .iter()
        .next()
        .and_then(|store| store.0.player_money())
    else {
        *prev = None;
        return;
    };
    let old = prev.replace(money);
    if !matches!(old, Some(p) if p != money) || !trade.is_open() {
        return;
    }
    if let Some(trimmed) = trimmed_offer(trade.our.gold, money) {
        trade.set_own_gold(trimmed);
        trade.send_edit(&commands, ClientCommand::SetTradeGold { copper: trimmed });
    }
}

/// The player guid a UnitPopup token names, through [`crate::ui_unit::player_token_guid`].
fn resolve_trade_target(token: &str, selection: &Selection, group: &GroupState) -> Option<u64> {
    crate::ui_unit::player_token_guid(token, selection, group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{TradeStatus, TradeStatusExtended};

    fn wire_item(entry: u32) -> TradeItem {
        TradeItem {
            entry,
            display_id: 42,
            count: 3,
            wrapped: false,
            gift_creator: 0,
            perm_enchant: 0,
            creator: 0,
            charges: 0,
            suffix_factor: 0,
            random_prop_id: 0,
            lock_id: 0,
            max_durability: 0,
            durability: 0,
        }
    }

    /// The row carries the template's quality and InventoryType, which `GetTradePlayerItemInfo`
    /// gates its fourth value on; an unanswered template leaves both empty.
    #[test]
    fn a_trade_row_carries_the_templates_quality_and_inventory_type() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut items = Items::default();
        let mut sword = crate::items::test_template("Sword");
        sword.quality = 4;
        sword.inventory_type = 13;
        items.insert_template(1234, Some(sword));
        items.insert_template(1235, Some(crate::items::test_template("Linen Cloth")));

        let row = resolve_slot(
            &wire_item(1234),
            &items,
            None,
            &NameCache::default(),
            &commands,
        );
        assert_eq!((row.quality, row.inventory_type), (Some(4), 13));
        // The icon is the template's display's, the question mark when that row is missing, and
        // left to the getter while the template is unanswered.
        assert_eq!(
            row.texture.as_deref(),
            Some("Interface\\Icons\\INV_Misc_QuestionMark")
        );
        assert_eq!(
            resolve_slot(
                &wire_item(1236),
                &items,
                None,
                &NameCache::default(),
                &commands
            )
            .texture,
            None
        );
        let row = resolve_slot(
            &wire_item(1235),
            &items,
            None,
            &NameCache::default(),
            &commands,
        );
        assert_eq!((row.quality, row.inventory_type), (Some(1), 0));
        let row = resolve_slot(
            &wire_item(1236),
            &items,
            None,
            &NameCache::default(),
            &commands,
        );
        assert_eq!((row.quality, row.inventory_type), (None, 0));
    }

    /// A wrapped gift's giver rides both columns: the partner's block carries the wire's wrapped
    /// word and gift creator (`0x534559`, `0x534574`), our placed item's object the same; only a
    /// wrapped item asks for the name, a zero guid asks nothing, and a name in flight is `None`.
    #[test]
    fn a_wrapped_gifts_giver_reaches_both_columns() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut items = Items::default();
        items.insert_template(1234, Some(crate::items::test_template("Gift")));
        let mut names = NameCache::default();
        names.insert_player(0x7, "Santa".to_string(), None);
        let gift = |wrapped, giver| TradeItem {
            wrapped,
            gift_creator: giver,
            ..wire_item(1234)
        };
        let mut s = TradeSession::default();
        s.begin(0x9);
        s.open_window();
        s.place_own_item(1, gift(true, 0x7), 0x8);
        s.place_own_item(2, gift(false, 0x7), 0);
        s.place_own_item(3, gift(true, 0), 0x8);
        s.place_own_item(4, gift(true, 0x8), 0x8);
        s.their.slots[0] = Some(gift(true, 0x7));
        s.their.slots[1] = Some(gift(false, 0x7));
        let state = snapshot(&s, &items, None, None, &names, &commands).expect("open");
        let giver = |side: &TradeSideState, i: usize| {
            let row = side.slots[i].as_ref().expect("filled");
            (row.wrapped, row.gift_creator.clone())
        };
        assert_eq!(giver(&state.target, 0), (true, Some("Santa".into())));
        assert_eq!(
            giver(&state.target, 1),
            (false, None),
            "unwrapped asks nothing"
        );
        assert_eq!(giver(&state.player, 0), (true, Some("Santa".into())));
        assert_eq!(giver(&state.player, 1), (false, None));
        assert_eq!(
            giver(&state.player, 2),
            (true, None),
            "a zero guid names no one"
        );
        assert_eq!(giver(&state.player, 3), (true, None), "a name in flight");
        let asked: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(asked.as_slice(), [ClientCommand::NameQuery { guid: 0x8 }]),
            "only the wrapped, nonzero, uncached giver is asked: {asked:?}"
        );
    }

    #[test]
    fn open_close_state_machine() {
        let mut s = TradeSession::default();
        assert!(!s.is_open());

        s.initiate(0x1234);
        assert_eq!(s.partner, Some(0x1234));
        assert!(!s.is_open());
        assert_eq!(s.cell(), None);

        s.open_window();
        assert!(s.is_open());
        assert_eq!(s.cell(), Some(0x1234));

        s.close_window();
        assert!(!s.is_open());
        assert_eq!(s.partner, None);
        assert_eq!(s.cell(), None);
    }

    #[test]
    fn an_incoming_request_is_not_yet_a_trade() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        assert_eq!(s.pending_request(), Some(0xABCD));
        assert_eq!(s.partner, None, "an unanswered request has no partner");
        assert!(!s.is_open(), "BEGIN_TRADE does not itself open the window");
        assert_eq!(s.cell(), None);
    }

    #[test]
    fn accepting_a_request_promotes_the_initiator_to_partner() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        s.begin(0xABCD);
        assert_eq!(s.partner, Some(0xABCD));
        assert_eq!(s.pending_request(), None, "the request is spent");
        assert!(!s.is_open(), "the window waits for OPEN_WINDOW");
    }

    #[test]
    fn closing_clears_a_pending_request() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        s.close_window();
        assert_eq!(s.pending_request(), None);
    }

    #[test]
    fn a_request_leaves_a_live_trade_alone() {
        let mut s = TradeSession::default();
        s.initiate(0x1);
        s.open_window();
        s.partner_accepted();

        s.request(0x2);
        assert_eq!(s.pending_request(), Some(0x2));
        assert_eq!(s.partner, Some(0x1), "the live trade is untouched");
        assert!(s.is_open());
        assert!(s.their_accept);

        s.refuse_request();
        assert_eq!(s.pending_request(), None);
        assert_eq!(s.partner, Some(0x1), "refusing an offer is not cancelling");
        assert!(s.is_open());
    }

    #[test]
    fn extended_fills_the_right_side_and_resets_accepts() {
        let mut s = TradeSession::default();
        s.begin(0x1);
        s.open_window();
        s.partner_accepted();
        assert!(s.their_accept);

        let mut slots = [None; TRADE_SLOT_COUNT];
        slots[0] = Some(wire_item(2589));
        let ext = TradeStatusExtended {
            their_window: true,
            sequence: 7,
            gold: 500,
            enchant_spell_id: 0,
            slots,
        };
        s.set_offer(&ext);
        assert!(s.their.slots[0].is_some());
        assert_eq!(s.their.gold, 500);
        assert_eq!(s.our.gold, 0);
        assert!(!s.their_accept, "an offer change resets accepts");

        let our_ext = TradeStatusExtended {
            their_window: false,
            sequence: 7,
            gold: 999,
            enchant_spell_id: 0,
            slots: [None; TRADE_SLOT_COUNT],
        };
        s.set_offer(&our_ext);
        assert_eq!(s.our.gold, 999);
        assert_eq!(s.their.gold, 500, "the other side is untouched");
    }

    #[test]
    fn accept_glow_tracks_both_sides() {
        let mut s = TradeSession::default();
        s.begin(0x1);
        s.open_window();
        s.accept();
        s.partner_accepted();
        assert_eq!((s.our_accept, s.their_accept), (true, true));
        s.back_to_trade();
        assert_eq!((s.our_accept, s.their_accept), (false, false));
    }

    #[test]
    fn own_offer_is_tracked_optimistically() {
        let mut s = TradeSession::default();
        s.begin(0x7);
        s.open_window();
        s.accept();
        s.partner_accepted();
        assert_eq!((s.our_accept, s.their_accept), (true, true));

        s.place_own_item(2, wire_item(1234), 0);
        assert!(s.our.slots[1].is_some(), "our slot 2 shows the placed item");
        assert_eq!(
            (s.our_accept, s.their_accept),
            (false, false),
            "changing our offer resets both accepts"
        );

        s.set_own_gold(500);
        assert_eq!(s.our.gold, 500);

        s.clear_own_item(2);
        assert!(s.our.slots[1].is_none(), "clearing empties the slot");
    }

    #[test]
    fn resolve_target_reads_the_selection_and_roster() {
        // "player" is yourself, and there is no roster for "party3".
        let sel = Selection::default();
        let group = GroupState::default();
        assert_eq!(resolve_trade_target("player", &sel, &group), None);
        assert_eq!(resolve_trade_target("party3", &sel, &group), None);
    }

    #[test]
    fn status_codes_are_stable() {
        assert_eq!(TradeStatus::OpenWindow.code(), 2);
        assert_eq!(TradeStatus::Accept.code(), 4);
        assert_eq!(TradeStatus::BackToTrade.code(), 7);
    }

    #[test]
    fn drain_initiate_resolves_the_selection_and_ships_the_cmsg() {
        use bevy::ecs::system::RunSystemOnce;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .add_message::<crate::item_watch::ItemNotice>()
            .insert_resource(NetCommands(tx))
            .insert_resource(Selection {
                target: None,
                guid: Some(0x7), // high 16 bits 0: a player guid
                ..Default::default()
            });
        app.insert_non_send_resource(UiScript::new().unwrap());

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("InitiateTrade('target')")
            .unwrap();

        app.world_mut().run_system_once(drain_trade).unwrap();

        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::InitiateTrade { target: 0x7 })
            ),
            "the drain ships CMSG_INITIATE_TRADE against the resolved target guid"
        );
        assert_eq!(
            app.world().resource::<TradeSession>().partner,
            Some(0x7),
            "the initiator records the partner so OPEN_WINDOW can name it"
        );
    }

    /// An app running [`feed_trade`] with a live VM and `0x7` named in the cache.
    fn feed_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut names = NameCache::default();
        names.insert_player(0x7, "Grubbis".to_string(), None);
        app.init_resource::<TradeSession>()
            .init_resource::<Items>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<GuidIndex>()
            .init_resource::<TradeTouchCursor>()
            .add_message::<crate::item_watch::ItemNotice>()
            .insert_resource(names)
            .insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_latched_window::<TradeSession>()
            .add_systems(Update, feed_trade);
        net::register(&mut app);
        (app, rx)
    }

    /// The server's status `status`, through the handler table.
    fn status(app: &mut App, status: TradeStatus) {
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::TradeStatus { status }],
        );
    }

    /// A trade accepted with `partner` and its `OPEN_WINDOW`, the window holding the latch.
    fn open_with(app: &mut App, partner: u64) {
        app.world_mut()
            .resource_mut::<TradeSession>()
            .begin(partner);
        status(app, TradeStatus::OpenWindow);
    }

    /// The number of `TRADE_REQUEST_CANCEL`s a spy frame has seen.
    fn cancels_seen(app: &mut App) -> i64 {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<i64>("return BENILLA_TEST_TRADE_CANCELS or 0")
            .unwrap()
    }

    #[test]
    fn canceled_fires_trade_request_cancel_and_the_siblings_do_not() {
        let (mut app, _rx) = feed_app();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"
                local f = CreateFrame("Frame")
                f:RegisterEvent("TRADE_REQUEST_CANCEL")
                f:SetScript("OnEvent", function()
                    BENILLA_TEST_TRADE_CANCELS = (BENILLA_TEST_TRADE_CANCELS or 0) + 1
                end)
                "#,
            )
            .unwrap();
        app.update();
        assert_eq!(cancels_seen(&mut app), 0, "nothing has cancelled anything");

        // `COMPLETE` and `CLOSE_WINDOW` close without the signal.
        for closing in [
            TradeStatus::Complete,
            TradeStatus::CloseWindow {
                result: 0,
                item_limit_category: 0,
            },
        ] {
            open_with(&mut app, 0x7);
            app.update();
            status(&mut app, closing);
            app.update();
            assert_eq!(
                cancels_seen(&mut app),
                0,
                "closing the window is not itself a cancel signal"
            );
        }

        open_with(&mut app, 0x7);
        app.update();
        status(&mut app, TradeStatus::Canceled);
        assert_eq!(cancels_seen(&mut app), 1, "CANCELED signals it, once");
        app.update();
        assert_eq!(cancels_seen(&mut app), 1, "and only once");
    }

    /// `ClickTradeButton` signals its slot in place (`0x4bfaef`), so the store catching up with
    /// the click a frame later, its richer row included, fires nothing more; nor does the clear.
    #[test]
    fn a_trade_slot_click_is_not_announced_again_by_the_feed() {
        let (mut app, _rx) = feed_app();
        // The app's row reads the template; the bag record the click copies has none yet.
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(2589, Some(crate::items::test_template("Tough Jerky")));
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window();
        }
        app.update();
        app.update();
        let jerky = |count| TradeItem {
            entry: 2589,
            display_id: 0,
            count,
            wrapped: false,
            gift_creator: 0,
            perm_enchant: 0,
            creator: 0,
            charges: 0,
            suffix_factor: 0,
            random_prop_id: 0,
            lock_id: 0,
            max_durability: 0,
            durability: 0,
        };
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            let slot = benilla_ui::script::ContainerSlot {
                item_id: 2589,
                count: 3,
                ..Default::default()
            };
            script.set_container(
                0,
                Some(benilla_ui::script::ContainerState {
                    name: None,
                    num_slots: 16,
                    slots: [(1, slot)].into_iter().collect(),
                }),
            );
            script
                .run(
                    r#"
                    SLOT_EVENTS = 0
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("TRADE_PLAYER_ITEM_CHANGED")
                    f:SetScript("OnEvent", function() SLOT_EVENTS = SLOT_EVENTS + 1 end)
                    PickupContainerItem(0, 1)
                    ClickTradeButton(1)
                    "#,
                )
                .unwrap();
        }
        let seen = |app: &mut App| {
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return SLOT_EVENTS")
                .unwrap()
        };
        assert_eq!(seen(&mut app), 1, "the click's own signal, in place");
        app.world_mut()
            .resource_mut::<TradeSession>()
            .place_own_item(1, jerky(3), 0);
        app.update();
        assert_eq!(seen(&mut app), 1, "the feed re-announced the placement");

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("ClearCursor() ClickTradeButton(1)")
            .unwrap();
        assert_eq!(seen(&mut app), 2, "the clear's own signal, in place");
        app.world_mut()
            .resource_mut::<TradeSession>()
            .clear_own_item(1);
        app.update();
        assert_eq!(seen(&mut app), 2, "the feed re-announced the clear");
    }

    /// A click whose bag item the store cannot see (`own_item_at` finds nothing) never reaches
    /// our column's source, so another change to the trade must not push the stale empty slot
    /// over the VM's write, nor announce it.
    #[test]
    fn a_trade_slot_click_survives_a_push_of_the_stale_store() {
        let (mut app, _rx) = feed_app();
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window();
        }
        app.update();
        app.update();
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            let slot = benilla_ui::script::ContainerSlot {
                item_id: 2589,
                count: 3,
                link: Some("|cffffffff|Hitem:2589|h[Linen Cloth]|h|r".into()),
                ..Default::default()
            };
            script.set_container(
                0,
                Some(benilla_ui::script::ContainerState {
                    name: None,
                    num_slots: 16,
                    slots: [(1, slot)].into_iter().collect(),
                }),
            );
            script
                .run(
                    r#"
                    SLOT_EVENTS = 0
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("TRADE_PLAYER_ITEM_CHANGED")
                    f:SetScript("OnEvent", function() SLOT_EVENTS = SLOT_EVENTS + 1 end)
                    PickupContainerItem(0, 1)
                    ClickTradeButton(1)
                    "#,
                )
                .unwrap();
        }
        app.world_mut()
            .resource_mut::<TradeSession>()
            .set_own_gold(100);
        app.update();
        let (events, name): (i64, Option<String>) = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("return SLOT_EVENTS, (GetTradePlayerItemInfo(1))")
            .unwrap();
        assert_eq!(events, 1, "the stale slot was announced");
        assert_eq!(
            name.as_deref(),
            Some("Linen Cloth"),
            "the click was rolled back"
        );
    }

    /// A placement whose bag position has no wire form goes unsent, so the VM's slot gives the
    /// item back, unlocked, and repaints, as the reference's failed send never writes it
    /// (`0x4bfab1`).
    #[test]
    fn an_unsendable_placement_gives_the_item_back() {
        let (mut app, rx) = feed_app();
        app.init_resource::<Selection>()
            .init_resource::<GroupState>()
            .add_systems(Update, drain_trade.after(feed_trade));
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window();
        }
        app.update();
        app.update();
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            // Slot 30 of the backpack: no wire position.
            script.set_container(
                0,
                Some(benilla_ui::script::ContainerState {
                    name: None,
                    num_slots: 32,
                    slots: [(
                        30,
                        benilla_ui::script::ContainerSlot {
                            item_id: 2589,
                            guid: 0x4000_0000_0000_2589,
                            count: 3,
                            link: Some("|cffffffff|Hitem:2589|h[Linen Cloth]|h|r".into()),
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                }),
            );
            script
                .run(
                    r#"
                    SLOT_EVENTS = 0
                    local f = CreateFrame("Frame")
                    f:RegisterEvent("TRADE_PLAYER_ITEM_CHANGED")
                    f:SetScript("OnEvent", function() SLOT_EVENTS = SLOT_EVENTS + 1 end)
                    PickupContainerItem(0, 30)
                    ClickTradeButton(1)
                    "#,
                )
                .unwrap();
        }
        app.update();
        assert!(
            !rx.try_iter()
                .any(|c| matches!(c, ClientCommand::SetTradeItem { .. })),
            "nothing to send"
        );
        let script = app.world().non_send_resource::<UiScript>();
        let (events, name, locked): (i64, Option<String>, bool) = script
            .eval(
                "local _, _, l = GetContainerItemInfo(0, 30) \
                 return SLOT_EVENTS, (GetTradePlayerItemInfo(1)), l == 1",
            )
            .unwrap();
        assert_eq!(name, None, "the slot still shows the unsent item");
        assert!(!locked, "the item comes back unlocked");
        assert_eq!(events, 2, "the click's signal, then the give-back's");
        assert_eq!(
            app.world().resource::<TradeSession>().own_edits,
            1,
            "an edit that never went out is not counted (`0x5d4442` follows the send's checks)"
        );
    }

    /// The reference prints the line from its own sender, not on a reply; row `0xb9` is chat.
    #[test]
    fn initiating_a_trade_says_so() {
        let (mut app, _rx) = feed_app();
        app.world_mut().resource_mut::<TradeSession>().initiate(0x7);
        app.update();

        assert_eq!(
            app.world().resource::<crate::ui_action::UiErrorKeys>().0,
            vec![crate::ui_action::UiError::s(
                "ERR_INITIATE_TRADE_S",
                "Grubbis".to_string()
            )],
        );
        assert_eq!(
            benilla_ui::messages::kind_of("ERR_INITIATE_TRADE_S"),
            benilla_ui::messages::MsgKind::Chat,
        );
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::ui_action::UiErrorKeys>()
                .0
                .len(),
            1,
            "the debt is spent once, not re-raised every frame"
        );
    }

    #[test]
    fn a_naming_status_line_needs_a_live_object_and_the_initiate_line_does_not() {
        let (mut app, _rx) = feed_app();
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.initiate(0x7); // parks ERR_INITIATE_TRADE_S, needs_live_object: false
            trade.owe_named_line(NamedLine {
                key: "ERR_PLAYER_BUSY_S",
                who: 0x7,
                needs_live_object: true,
            });
        }
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::ui_action::UiErrorKeys>()
                .0
                .iter()
                .map(|e| e.key)
                .collect::<Vec<_>>(),
            vec!["ERR_INITIATE_TRADE_S"],
            "0x7 is cached, not a streamed object: the status line drops, the initiate line stays"
        );

        // Stream the player, ask again: now it prints.
        let e = app.world_mut().spawn(ObjectStore::default()).id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(0x7, e);
        app.world_mut()
            .resource_mut::<crate::ui_action::UiErrorKeys>()
            .0
            .clear();
        app.world_mut()
            .resource_mut::<TradeSession>()
            .owe_named_line(NamedLine {
                key: "ERR_PLAYER_BUSY_S",
                who: 0x7,
                needs_live_object: true,
            });
        app.update();
        assert_eq!(
            app.world().resource::<crate::ui_action::UiErrorKeys>().0,
            vec![crate::ui_action::UiError::s(
                "ERR_PLAYER_BUSY_S",
                "Grubbis".to_string()
            )],
        );
    }

    #[test]
    fn the_initiate_line_waits_for_the_name_and_survives_the_refusal() {
        let (mut app, _rx) = feed_app();
        app.world_mut()
            .resource_mut::<TradeSession>()
            .initiate(0x99); // no name cached
        app.update();
        assert!(
            app.world()
                .resource::<crate::ui_action::UiErrorKeys>()
                .0
                .is_empty(),
            "no name, no line — and never a line with an unfilled %s"
        );

        // The server refuses before the name lands; the debt survives the close.
        app.world_mut()
            .resource_mut::<TradeSession>()
            .close_window();
        app.world_mut().resource_mut::<NameCache>().insert_player(
            0x99,
            "Skarrid".to_string(),
            None,
        );
        app.update();
        assert_eq!(
            app.world().resource::<crate::ui_action::UiErrorKeys>().0,
            vec![crate::ui_action::UiError::s(
                "ERR_INITIATE_TRADE_S",
                "Skarrid".to_string()
            )],
        );
    }

    #[test]
    fn a_request_that_survives_the_ladder_is_accepted() {
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::BeginTrade)),
            "nothing refused it, so the client answers — the server opens both windows"
        );
        let trade = app.world().resource::<TradeSession>();
        assert_eq!(trade.partner, Some(0x7));
        assert_eq!(trade.pending_request(), None, "the request is spent");
        assert!(
            !trade.initiated,
            "a trade we did not start never sets the initiate latch"
        );
        assert!(
            app.world()
                .resource::<crate::ui_action::UiErrorKeys>()
                .0
                .is_empty(),
            "and an accepted request says nothing — only leg 8 speaks"
        );
    }

    #[test]
    fn block_trades_refuses_and_says_so() {
        let (mut app, rx) = request_app(true, false, true);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();

        assert!(matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)));
        assert_eq!(
            app.world().resource::<crate::ui_action::UiErrorKeys>().0,
            vec![crate::ui_action::UiError::s(
                "ERR_TRADE_BLOCKED_S",
                "Grubbis".to_string()
            )],
            "the refusal raises the reference's own `0x496720(0xbb, name)`"
        );
        assert_eq!(
            benilla_ui::messages::kind_of("ERR_TRADE_BLOCKED_S"),
            benilla_ui::messages::MsgKind::Chat,
        );
    }

    #[test]
    fn the_silent_legs_refuse_without_a_word() {
        for (cinematic, controlled) in [(true, true), (false, false)] {
            let (mut app, rx) = request_app(false, cinematic, controlled);
            app.world_mut().resource_mut::<TradeSession>().request(0x7);
            app.update();
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)),
                "cinematic={cinematic} controlled={controlled}: refused as busy"
            );
            assert!(
                app.world()
                    .resource::<crate::ui_action::UiErrorKeys>()
                    .0
                    .is_empty(),
                "cinematic={cinematic} controlled={controlled}: a silent leg says nothing"
            );
        }
    }

    /// The leg reads `[0xb4b3e4]` (`0x4bf79c`), written for our own character alone: a possessed
    /// creature stuck under fear leaves it set, and the request is answered.
    #[test]
    fn a_stuck_possessed_creature_does_not_make_us_busy() {
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .control_lost = true;
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::BeginTrade)),
            "auto-accepted, not busy"
        );
    }

    #[test]
    fn an_open_auction_house_refuses_the_request() {
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut()
            .resource_mut::<crate::ui_auction::AuctionOpen>()
            .auctioneer = Some(0x1234);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)));
    }

    #[test]
    fn an_ignored_initiator_is_refused_as_ignored() {
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut()
            .resource_mut::<crate::ui_social::SocialState>()
            .set_ignores_for_test(vec![0x7]);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::IgnoreTrade)),
            "the ignore leg does NOT answer busy"
        );
        assert!(app
            .world()
            .resource::<crate::ui_action::UiErrorKeys>()
            .0
            .is_empty());
    }

    #[test]
    fn a_dead_or_ghost_initiator_is_refused() {
        // The `UNIT_FIELD_HEALTH`, `UNIT_FIELD_MAXHEALTH` and `PLAYER_FLAGS` field ids, and
        // `PLAYER_FLAGS_GHOST`.
        const HEALTH: u16 = 22;
        const MAXHEALTH: u16 = 28;
        const PLAYER_FLAGS: u16 = 190;
        const GHOST: u32 = 0x10;
        for (label, fields) in [
            ("dead", vec![(HEALTH, 0u32), (MAXHEALTH, 100)]),
            (
                "ghost",
                vec![(HEALTH, 1u32), (MAXHEALTH, 100), (PLAYER_FLAGS, GHOST)],
            ),
        ] {
            let (mut app, rx) = request_app(false, false, true);
            let entity = app.world().resource::<GuidIndex>().0[&0x7];
            app.world_mut().entity_mut(entity).insert(ObjectStore(
                benilla_protocol::messages::ObjectFields::from_pairs(&fields),
            ));
            app.world_mut().resource_mut::<TradeSession>().request(0x7);
            app.update();
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)),
                "{label}: refused as busy"
            );
        }
    }

    #[test]
    fn two_legs_answer_with_no_packet_at_all() {
        let answered = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter().any(|c| {
                matches!(
                    c,
                    ClientCommand::BusyTrade
                        | ClientCommand::IgnoreTrade
                        | ClientCommand::BeginTrade
                        | ClientCommand::CancelTrade
                )
            })
        };

        // Leg 1: we asked somebody else first, and the request arrives over it.
        let (mut app, rx) = request_app(false, false, true);
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.initiate(0x9);
            trade.request(0x7);
        }
        app.update();
        assert!(
            !answered(&rx),
            "an initiate of our own in flight drops the request in silence"
        );
        let trade = app.world().resource::<TradeSession>();
        assert_eq!(
            trade.pending_request(),
            None,
            "…and spends it, so it is not retried every frame"
        );
        assert_eq!(trade.partner, Some(0x9), "…and our own trade survives it");

        // Leg 3: nothing of that guid is streamed.
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut().resource_mut::<GuidIndex>().0.remove(&0x7);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();
        assert!(
            !answered(&rx),
            "an unresolvable initiator drops the request in silence"
        );
    }

    fn request_app(
        block_trades: bool,
        cinematic: bool,
        controlled: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut names = NameCache::default();
        // Named, so leg 8 reaches its line rather than parking on a name query.
        names.insert_player(0x7, "Grubbis".to_string(), None);
        names.insert_player(0x8, "Skarrid".to_string(), None);
        app.init_resource::<TradeSession>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_social::SocialState>()
            .init_resource::<crate::ui_auction::AuctionOpen>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::player::Player>()
            .insert_resource(names)
            .insert_resource(NetCommands(tx))
            .insert_resource(BlockTrades(block_trades))
            .insert_resource(if cinematic {
                crate::cinematic::Cinematic::playing_for_test()
            } else {
                crate::cinematic::Cinematic::default()
            });
        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .own_control_lost = !controlled;
        // Streamed, since leg 3 drops a request from a guid that resolves to nothing.
        for guid in [0x7u64, 0x8] {
            let e = app.world_mut().spawn(ObjectStore::default()).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
        }
        app.add_systems(Update, answer_trade_request);
        (app, rx)
    }

    /// No sender tests the window (`0x5d4680`, `0x5d4500`, `0x5d45f0`, `0x4bfa70`): money, a
    /// placement and a clear ship with none open, and vmangos drops them (`TradeHandler.cpp:668-
    /// 735`). Two money verbs in one tick (stock's over-purse clamp re-enters `SetTradeMoney`,
    /// `TradeFrame.lua:162-173`) are two sends and two edits.
    #[test]
    fn the_drain_ships_every_edit_with_or_without_a_window() {
        use bevy::ecs::system::RunSystemOnce;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .add_message::<crate::item_watch::ItemNotice>()
            .insert_resource(NetCommands(tx))
            .insert_resource(Selection::default());
        let mut vm = UiScript::new().unwrap();
        vm.set_money(20_000); // the purse `SetTradeMoney` is gated on
        app.insert_non_send_resource(vm);

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("SetTradeMoney(500)")
            .unwrap();
        app.world_mut().run_system_once(drain_trade).unwrap();
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::SetTradeGold { copper: 500 })
            ),
            "CMSG_SET_TRADE_GOLD goes out with no window"
        );

        {
            let mut vm = app.world_mut().non_send_resource_mut::<UiScript>();
            vm.set_trade(Some(benilla_ui::script::TradeState::default()));
            let mut bag = benilla_ui::script::ContainerState::default();
            bag.slots.insert(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: 2589,
                    count: 1,
                    ..Default::default()
                },
            );
            vm.set_container(0, Some(bag));
            vm.run("PickupContainerItem(0, 1) ClickTradeButton(1) ClickTradeButton(1)")
                .unwrap();
        }
        app.world_mut().run_system_once(drain_trade).unwrap();
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::SetTradeItem { trade_slot: 0, .. },
                    ClientCommand::ClearTradeItem { trade_slot: 0 }
                ]
            ),
            "a placement and a clear ship with no window: {sent:?}"
        );
        let before = app.world().resource::<TradeSession>().own_edits;

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("SetTradeMoney(12345) SetTradeMoney(777)")
            .unwrap();
        app.world_mut().run_system_once(drain_trade).unwrap();
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::SetTradeGold { copper: 12_345 },
                    ClientCommand::SetTradeGold { copper: 777 }
                ]
            ),
            "both verbs go out: {sent:?}"
        );
        assert_eq!(
            app.world().resource::<TradeSession>().own_edits,
            before + 2,
            "and both count toward the stale-status check"
        );
    }

    #[test]
    fn drain_item_clear_ships_clear_trade_item_zero_based() {
        use bevy::ecs::system::RunSystemOnce;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .add_message::<crate::item_watch::ItemNotice>()
            .insert_resource(NetCommands(tx))
            .insert_resource(Selection::default());
        app.insert_non_send_resource(UiScript::new().unwrap());
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window();
        }

        // Our slot 1 filled by a click, then an empty-cursor click on it.
        {
            let mut vm = app.world_mut().non_send_resource_mut::<UiScript>();
            vm.set_trade(Some(benilla_ui::script::TradeState::default()));
            let mut bag = benilla_ui::script::ContainerState::default();
            bag.slots.insert(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: 2589,
                    count: 1,
                    ..Default::default()
                },
            );
            vm.set_container(0, Some(bag));
            vm.run("PickupContainerItem(0, 1) ClickTradeButton(1)")
                .unwrap();
            assert_eq!(vm.take_trade_set_items(), vec![(1, 0, 1)]);
            vm.run("ClickTradeButton(1)").unwrap();
        }
        app.world_mut().run_system_once(drain_trade).unwrap();
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::ClearTradeItem { trade_slot: 0 })
            ),
            "clearing our filled UI slot 1 ships CMSG_CLEAR_TRADE_ITEM for wire slot 0"
        );
    }

    /// Every offer edit the drain sends moves the own-edit counter: a placement, a clear and a
    /// money offer, one each (`0x5d4442`, `0x5d44bc`, `0x5d4712`); the open set it to 1.
    #[test]
    fn each_offer_edit_the_drain_sends_is_counted() {
        use bevy::ecs::system::RunSystemOnce;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .add_message::<crate::item_watch::ItemNotice>()
            .insert_resource(NetCommands(tx))
            .insert_resource(Selection::default());
        let mut vm = UiScript::new().unwrap();
        vm.set_money(20_000);
        app.insert_non_send_resource(vm);
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window();
            assert_eq!(trade.own_edits, 1, "the open starts the count at 1");
        }
        {
            let mut vm = app.world_mut().non_send_resource_mut::<UiScript>();
            vm.set_trade(Some(benilla_ui::script::TradeState::default()));
            let mut bag = benilla_ui::script::ContainerState::default();
            bag.slots.insert(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: 2589,
                    count: 1,
                    ..Default::default()
                },
            );
            vm.set_container(0, Some(bag));
            vm.run("PickupContainerItem(0, 1) ClickTradeButton(1) SetTradeMoney(500)")
                .unwrap();
        }
        app.world_mut().run_system_once(drain_trade).unwrap();
        assert_eq!(rx.try_iter().count(), 2, "the placement and the money");
        assert_eq!(app.world().resource::<TradeSession>().own_edits, 3);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("ClickTradeButton(1)")
            .unwrap();
        app.world_mut().run_system_once(drain_trade).unwrap();
        assert_eq!(rx.try_iter().count(), 1, "the clear");
        assert_eq!(app.world().resource::<TradeSession>().own_edits, 4);
    }

    /// `0x5d49ba`: an own-side status whose sequence is below the own-edit counter is dropped
    /// before the store, the slot invalidations and the accept reset. The counter is 1 at the
    /// open and one more per edit we send; vmangos writes 7, so the seventh edit makes its
    /// statuses stale. The partner's column is never dropped, and a new open starts over.
    #[test]
    fn a_stale_own_side_status_is_dropped_whole() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let ext = |their_window: bool, sequence: u32, gold: u32| TradeStatusExtended {
            their_window,
            sequence,
            gold,
            enchant_spell_id: 0,
            slots: [None; TRADE_SLOT_COUNT],
        };
        let mut s = TradeSession::default();
        s.begin(0x1);
        s.open_window();
        s.place_own_item(2, wire_item(1234), 0);
        s.accept();
        s.partner_accepted();
        let edit = |s: &mut TradeSession| {
            s.send_edit(&commands, ClientCommand::SetTradeGold { copper: 1 });
        };

        // Six edits make the counter 7: a status with sequence 7 is not below it.
        for _ in 0..6 {
            edit(&mut s);
        }
        s.set_offer(&ext(false, 7, 111));
        assert_eq!(
            s.our.gold, 111,
            "sequence 7 against a counter of 7 is applied"
        );
        assert!(s.our.slots[1].is_none() && s.take_invalidated() == [1]);

        // The seventh makes it 8: the same status is now stale.
        s.place_own_item(2, wire_item(1234), 0);
        s.accept();
        s.partner_accepted();
        edit(&mut s);
        s.set_offer(&ext(false, 7, 222));
        assert_eq!(s.our.gold, 111, "the stale status left our gold alone");
        assert!(s.our.slots[1].is_some(), "and the placement in flight");
        assert!(s.take_invalidated().is_empty(), "and invalidated nothing");
        assert!(s.we_accepted() && s.their_accept, "and both accepts");

        // The partner's column carries no sequence check.
        s.set_offer(&ext(true, 0, 333));
        assert_eq!(s.their.gold, 333);

        // A newer sequence passes, and a new open starts the count over.
        s.set_offer(&ext(false, 8, 444));
        assert_eq!(s.our.gold, 444);
        s.open_window();
        s.set_offer(&ext(false, 1, 555));
        assert_eq!(s.our.gold, 555);
    }

    /// An app with the trade window open and the backpack's slot 1 offered in trade slot 1, and
    /// a spy that lists the trade-close and lock events in order.
    fn offered_app() -> App {
        let (mut app, _rx) = feed_app();
        open_with(&mut app, 0x7);
        app.update();
        let mut vm = app.world_mut().non_send_resource_mut::<UiScript>();
        let mut bag = benilla_ui::script::ContainerState::default();
        bag.slots.insert(
            1,
            benilla_ui::script::ContainerSlot {
                item_id: 2589,
                guid: 0x4000_0000_0000_2589,
                count: 1,
                ..Default::default()
            },
        );
        vm.set_container(0, Some(bag));
        vm.run(
            r#"PickupContainerItem(0, 1) ClickTradeButton(1)
               SEEN = {}
               local f = CreateFrame("Frame")
               for _, e in { "TRADE_REQUEST_CANCEL", "ITEM_LOCK_CHANGED", "TRADE_CLOSED",
                             "TRADE_PLAYER_ITEM_CHANGED" } do
                   f:RegisterEvent(e)
               end
               f:SetScript("OnEvent", function() table.insert(SEEN, event) end)"#,
        )
        .unwrap();
        app
    }

    fn seen(app: &mut App) -> Vec<String> {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval("local s = SEEN SEEN = {} return s")
            .unwrap()
    }

    fn bag_locked(app: &mut App) -> bool {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval("local _, _, l = GetContainerItemInfo(0, 1) return l == 1")
            .unwrap()
    }

    /// `CANCELED` fires `TRADE_REQUEST_CANCEL` (`0x4bf832`), then unlocks the offered items
    /// (`0x4bf837`), then runs the close core while the partner holds the latch: `TRADE_CLOSED`
    /// twice (`0x4bf522`, once inside `0x493365`'s re-entry) and `CMSG_CANCEL_TRADE` after each
    /// (`0x4bf527`). A local `CloseTrade` (`0x4bfdc0`) runs the same core without unlocking, which
    /// waits for the server's `CANCELED`; that close finds no window and only cancels again.
    #[test]
    fn a_canceled_trade_unlocks_between_its_signals_and_a_local_close_does_not() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = offered_app();
        app.insert_resource(NetCommands(tx.clone()));
        status(&mut app, TradeStatus::Canceled);
        assert_eq!(
            seen(&mut app),
            [
                "TRADE_REQUEST_CANCEL",
                "ITEM_LOCK_CHANGED",
                "TRADE_CLOSED",
                "TRADE_CLOSED"
            ]
        );
        assert!(!bag_locked(&mut app));
        assert_eq!(cancels(&rx), 2, "the re-entered core's and the outer one's");
        assert_eq!(
            app.world()
                .resource::<crate::ui_session::InteractLatch>()
                .guid(),
            None
        );

        let mut app = offered_app();
        app.insert_resource(NetCommands(tx));
        // `CloseTrade` (`0x4bfdc0` → `0x4bf4e0(0, 0)`), as the drain runs it.
        close_core(app.world_mut());
        app.update();
        assert_eq!(seen(&mut app), ["TRADE_CLOSED", "TRADE_CLOSED"]);
        assert_eq!(cancels(&rx), 2);
        assert!(bag_locked(&mut app), "held until the server answers");
        status(&mut app, TradeStatus::Canceled);
        assert_eq!(
            seen(&mut app),
            ["TRADE_REQUEST_CANCEL", "ITEM_LOCK_CHANGED"]
        );
        assert!(!bag_locked(&mut app));
        assert_eq!(
            cancels(&rx),
            1,
            "the core's cancel goes out with no window open"
        );
    }

    /// Both offers live in tables only an open clears (`0x4bf549`-`0x4bf5ef`); a close signals
    /// `TRADE_CLOSED` (`0x4bf522`) and clears none, and the item getters test no open state
    /// (`0x4c015a`, `0x4c04b3`). So a `TRADE_CLOSED` handler reads the last offer of both sides,
    /// ours while its item object exists and the partner's from the record cache, and so does
    /// anything after, until the next open. The money is the exception: the statuses that end a
    /// trade zero both offers before the dispatcher (`0x5d4884`, `0x5d48eb`), and our getter
    /// also tests the partner cell (`0x4c0706`), so both answer 0 inside `TRADE_CLOSED`.
    #[test]
    fn both_offers_answer_inside_trade_closed_and_until_the_next_open() {
        for closing in [
            TradeStatus::Complete,
            TradeStatus::Canceled,
            TradeStatus::CloseWindow {
                result: 0,
                item_limit_category: 0,
            },
        ] {
            let mut app = offered_app();
            {
                let mut items = app.world_mut().resource_mut::<Items>();
                items.insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
                items.insert_template(1234, Some(crate::items::test_template("Iron Sword")));
            }
            let mut theirs = [None; TRADE_SLOT_COUNT];
            theirs[0] = Some(wire_item(1234));
            {
                let mut trade = app.world_mut().resource_mut::<TradeSession>();
                trade.place_own_item(1, wire_item(2589), 0);
                trade.set_own_gold(300);
                trade.set_offer(&TradeStatusExtended {
                    their_window: true,
                    sequence: 7,
                    gold: 500,
                    enchant_spell_id: 0,
                    slots: theirs,
                });
            }
            let reads = "return tostring((GetTradePlayerItemInfo(1))) .. '/' .. \
                         tostring((GetTradeTargetItemInfo(1))) .. '/' .. GetPlayerTradeMoney() \
                         .. '/' .. GetTargetTradeMoney()";
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run(&format!(
                    "CLOSED = {{}} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_CLOSED') \
                     f:SetScript('OnEvent', function() table.insert(CLOSED, (function() {reads} end)()) end)"
                ))
                .unwrap();
            app.update();
            let read = |app: &App| {
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<String>(reads)
                    .unwrap()
            };
            assert_eq!(read(&app), "Linen Cloth/Iron Sword/300/500", "open");

            status(&mut app, closing);
            app.update();
            let closed: Vec<String> = app
                .world()
                .non_send_resource::<UiScript>()
                .eval("return CLOSED")
                .unwrap();
            assert!(!closed.is_empty(), "TRADE_CLOSED fired after {closing:?}");
            assert!(
                closed.iter().all(|c| c == "Linen Cloth/Iron Sword/0/0"),
                "the handler read {closed:?} after {closing:?}"
            );
            assert_eq!(read(&app), "Linen Cloth/Iron Sword/0/0", "after the close");
            // `PickupTradeMoney` finds no offer to lift (`0x4c07ca`-`0x4c07d1`), so no coins
            // reach the cursor and nothing is queued.
            let mut vm = app.world_mut().non_send_resource_mut::<UiScript>();
            vm.run("PickupTradeMoney(100)").unwrap();
            assert!(vm.take_trade_money().is_empty());
            assert_eq!(vm.eval::<i64>("return GetCursorMoney()").unwrap(), 0);
            let holding = |app: &App| {
                app.world()
                    .non_send_resource::<UiScript>()
                    .trade_slot_holding(LINEN)
            };
            assert_eq!(holding(&app), Some((0, 0, 1)), "our item table outlives it");

            open_with(&mut app, 0x7);
            app.update();
            assert_eq!(read(&app), "nil/nil/0/0", "the open cleared both");
            assert_eq!(holding(&app), None, "and the item table");
        }
    }

    /// `TRADE_UPDATE` has one producer, the getters' record callback `0x4c0340` (`0x4c034f`): an
    /// offer or gold change signals its slot and money events and nothing else, and a getter
    /// that finds an unanswered template is called back once per call when the record lands,
    /// to a handler that now reads it answered. A getter that hits is never called back.
    #[test]
    fn trade_update_comes_only_from_a_landed_record_a_getter_asked_for() {
        use crate::item_watch::ItemNotice;
        let (mut app, _rx) = feed_app();
        app.add_systems(Update, drain_trade.after(feed_trade))
            .init_resource::<Selection>()
            .init_resource::<GroupState>();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"
                UPDATES, SLOTS, SEEN = 0, 0, {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("TRADE_UPDATE")
                f:RegisterEvent("TRADE_TARGET_ITEM_CHANGED")
                f:SetScript("OnEvent", function()
                    if event == "TRADE_UPDATE" then
                        UPDATES = UPDATES + 1
                        table.insert(SEEN, tostring((GetTradeTargetItemInfo(1))))
                    else
                        SLOTS = SLOTS + 1
                    end
                end)
                "#,
            )
            .unwrap();
        open_with(&mut app, 0x7);
        app.update();
        let counts = |app: &App| -> (i64, i64) {
            app.world()
                .non_send_resource::<UiScript>()
                .eval("return UPDATES, SLOTS")
                .unwrap()
        };
        let offer = |app: &mut App, gold: u32| {
            let mut theirs = [None; TRADE_SLOT_COUNT];
            theirs[0] = Some(wire_item(4306));
            app.world_mut()
                .resource_mut::<TradeSession>()
                .set_offer(&TradeStatusExtended {
                    their_window: true,
                    sequence: 7,
                    gold,
                    enchant_spell_id: 0,
                    slots: theirs,
                });
            app.update();
        };
        offer(&mut app, 0);
        offer(&mut app, 900);
        assert_eq!(
            counts(&app),
            (0, 1),
            "the slot event once, and no TRADE_UPDATE for the offer or the gold"
        );

        // Two getter calls on the unanswered slot register two callbacks (`0x55bbb3`).
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("GetTradeTargetItemInfo(1) GetTradeTargetItemInfo(1)")
            .unwrap();
        app.update();
        assert_eq!(counts(&app).0, 0, "nothing signals until the record lands");

        // The record lands as the object handlers write it: the cache, then one notice per waiter.
        let waiting = {
            let mut items = app.world_mut().resource_mut::<Items>();
            items.insert_template(4306, Some(crate::items::test_template("Iron Sword")));
            items.take_trade_waiters(4306)
        };
        assert_eq!(waiting, 2, "the drain registered one per call");
        app.world_mut()
            .resource_mut::<Messages<ItemNotice>>()
            .write_batch(vec![ItemNotice::TradeRecord { entry: 4306 }; waiting]);
        app.update();
        assert_eq!(
            counts(&app).0,
            2,
            "one TRADE_UPDATE per registered callback"
        );
        assert_eq!(
            counts(&app).1,
            1,
            "the landing changed the row, not the offer: no slot event (`0x4bf2b0` runs on a status)"
        );
        let seen: Vec<String> = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return SEEN")
            .unwrap();
        assert_eq!(
            seen,
            ["Iron Sword", "Iron Sword"],
            "the handler reads the answered row"
        );

        // Answered now, a getter asks nothing and nothing signals.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("GetTradeTargetItemInfo(1)")
            .unwrap();
        app.update();
        app.update();
        assert_eq!(counts(&app).0, 2);
        assert_eq!(
            app.world_mut()
                .resource_mut::<Items>()
                .take_trade_waiters(4306),
            0
        );
    }

    /// `ONLY_CONJURED` invalidates the slot (`0x4bf9fd` into `0x4bfbd0`): the item unlocks and
    /// `TRADE_PLAYER_ITEM_CHANGED` fires once, not again from the repaint diff.
    #[test]
    fn a_bounced_offer_unlocks_its_item_and_signals_the_slot_once() {
        let mut app = offered_app();
        // The column the app fills on the placement, as the drain would.
        app.world_mut()
            .resource_mut::<TradeSession>()
            .place_own_item(1, wire_item(2589), 0);
        app.update();
        seen(&mut app);
        app.world_mut()
            .resource_mut::<TradeSession>()
            .bounce_own_offer(0);
        app.update();
        assert_eq!(
            seen(&mut app),
            ["ITEM_LOCK_CHANGED", "TRADE_PLAYER_ITEM_CHANGED"]
        );
        assert!(!bag_locked(&mut app));

        // An own-side extended status that empties a filled slot does the same (`0x5d4b33`).
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("ClearCursor() PickupContainerItem(0, 1) ClickTradeButton(1)")
            .unwrap();
        app.world_mut()
            .resource_mut::<TradeSession>()
            .place_own_item(1, wire_item(2589), 0);
        app.update();
        seen(&mut app);
        app.world_mut()
            .resource_mut::<TradeSession>()
            .set_offer(&TradeStatusExtended {
                their_window: false,
                sequence: 7,
                gold: 0,
                enchant_spell_id: 0,
                slots: [None; TRADE_SLOT_COUNT],
            });
        app.update();
        assert_eq!(
            seen(&mut app),
            ["ITEM_LOCK_CHANGED", "TRADE_PLAYER_ITEM_CHANGED"]
        );
        assert!(!bag_locked(&mut app));
    }

    /// [`feed_app`] with the sweep's two trade legs.
    fn sweep_trade_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        use crate::ui_script::SweepApp;
        let (mut app, rx) = feed_app();
        app.add_sweep_leg(SweepLeg::Trade, cancel_trade_on_leave_world)
            .add_sweep_leg(SweepLeg::Interaction, crate::ui_session::dispatch);
        (app, rx)
    }

    fn cancels(rx: &crossbeam_channel::Receiver<ClientCommand>) -> usize {
        rx.try_iter()
            .filter(|c| matches!(c, ClientCommand::CancelTrade))
            .count()
    }

    const EVENTS: [&str; 3] = ["TRADE_SHOW", "TRADE_CLOSED", "PLAYER_LEAVING_WORLD"];

    /// A `/reload` with a trade open: the trade leg cancels (`0x490adf` → `0x5d4310`), and past
    /// `PLAYER_LEAVING_WORLD` the interaction close shuts the window in the dying VM and cancels
    /// again (`0x493365` → `0x4bf4e0`); the new VM opens nothing.
    #[test]
    fn a_reload_cancels_the_trade_closes_it_and_the_new_vm_opens_nothing() {
        use crate::ui_script::sweep_test::{reload_in_place, seen, seen_in, spied_vm};
        let (mut app, rx) = sweep_trade_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        open_with(&mut app, 0x7);
        app.update();
        assert_eq!(seen_in(&app), ["TRADE_SHOW"]);
        let _ = rx.try_iter().count();

        let dying = reload_in_place(&mut app, &EVENTS);
        assert_eq!(
            seen(&dying),
            ["TRADE_SHOW", "PLAYER_LEAVING_WORLD", "TRADE_CLOSED"]
        );
        assert_eq!(cancels(&rx), 2, "the leg's and the interaction close's");
        assert!(!app.world().resource::<TradeSession>().is_open());

        app.update();
        assert!(seen_in(&app).is_empty(), "the new VM has no trade to show");
    }

    /// With no trade, the leg still cancels and nothing closes; a worldport's surviving VM hears
    /// `TRADE_CLOSED` once when one was open.
    #[test]
    fn the_trade_leg_cancels_with_no_trade_and_a_worldport_closes_once() {
        use crate::ui_script::sweep_test::{seen_in, spied_vm, sweep_in_place};
        let (mut app, rx) = sweep_trade_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        sweep_in_place(&mut app);
        assert_eq!(
            cancels(&rx),
            1,
            "`0x5d4310` sends whether or not a trade is open"
        );
        assert_eq!(seen_in(&app), ["PLAYER_LEAVING_WORLD"]);

        open_with(&mut app, 0x7);
        app.update();
        sweep_in_place(&mut app);
        app.update();
        app.update();
        assert_eq!(
            seen_in(&app),
            [
                "PLAYER_LEAVING_WORLD",
                "TRADE_SHOW",
                "PLAYER_LEAVING_WORLD",
                "TRADE_CLOSED"
            ]
        );
    }

    /// The offered linen (backpack slot 1, our trade slot 1) as a live item object of ours, with
    /// our player, the re-signal system ahead of the feed, and the spy's list emptied.
    fn resignal_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
        use benilla_protocol::ObjectFields;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = offered_app();
        app.insert_resource(NetCommands(tx))
            .add_message::<crate::item_watch::ItemNotice>()
            .add_systems(Update, resignal_trade_items.before(feed_trade));
        let world = app.world_mut();
        crate::items::test_spawn_item(
            world,
            LINEN,
            ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 2589), (FIELD_ITEM_STACK_COUNT, 1)]),
            false,
        );
        world.spawn(crate::net::SelfPlayer);
        seen(&mut app);
        (app, rx)
    }

    /// The offered linen's guid in [`offered_app`].
    const LINEN: u64 = 0x4000_0000_0000_2589;

    fn notify(app: &mut App, notice: crate::item_watch::ItemNotice) {
        app.world_mut()
            .resource_mut::<Messages<crate::item_watch::ItemNotice>>()
            .write(notice);
        app.update();
    }

    /// `0x4bfb10` from the destroy: the slot holding the item clears on the wire and in the VM,
    /// and signals once; nothing unlocks, the lock having gone with the item.
    #[test]
    fn a_destroyed_offer_clears_its_slot_and_tells_the_server() {
        let (mut app, rx) = resignal_app();
        notify(
            &mut app,
            crate::item_watch::ItemNotice::Destroyed {
                guid: LINEN,
                entry: 2589,
            },
        );
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, ClientCommand::ClearTradeItem { trade_slot: 0 })),
            "CMSG_CLEAR_TRADE_ITEM for slot 1: {sent:?}"
        );
        assert_eq!(seen(&mut app), ["TRADE_PLAYER_ITEM_CHANGED"]);
        assert_eq!(
            app.world().resource::<TradeSession>().own_edits,
            2,
            "the clear is one of our edits (`0x5d44bc`)"
        );
        let name: Option<String> = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("return (GetTradePlayerItemInfo(1))")
            .unwrap();
        assert_eq!(name, None, "the slot is empty");
        // Another item's destroy, or the same one again, touches no slot.
        notify(
            &mut app,
            crate::item_watch::ItemNotice::Destroyed {
                guid: LINEN,
                entry: 2589,
            },
        );
        assert!(rx.try_iter().next().is_none());
        assert!(seen(&mut app).is_empty());
        assert_eq!(app.world().resource::<TradeSession>().own_edits, 2);
    }

    /// vmangos sends the traded items' destroys before `TRADE_COMPLETE`, and the destroy releases
    /// the item at its packet (`0x4674cf` → `0x464920`), so the slot clears and signals while the
    /// window is still open: by `TRADE_CLOSED` the slot is empty, once, and the later scheduled
    /// scan finds nothing left to read.
    #[test]
    fn a_traded_away_item_clears_its_slot_before_trade_closed() {
        let (mut app, rx) = resignal_app();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                "READS = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_CLOSED') \
                 f:SetScript('OnEvent', function() \
                     table.insert(READS, tostring((GetTradePlayerItemInfo(1)))) end)",
            )
            .unwrap();
        // The packets of one drain: the destroy's notice, then the status, with no frame between.
        app.world_mut()
            .resource_mut::<Messages<crate::item_watch::ItemNotice>>()
            .write(crate::item_watch::ItemNotice::Destroyed {
                guid: LINEN,
                entry: 2589,
            });
        status(&mut app, TradeStatus::Complete);
        assert_eq!(
            seen(&mut app),
            ["TRADE_PLAYER_ITEM_CHANGED", "TRADE_CLOSED", "TRADE_CLOSED"],
            "the clear signals ahead of the close"
        );
        let reads: Vec<String> = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return READS")
            .unwrap();
        assert_eq!(
            reads,
            ["nil", "nil"],
            "and the handler finds the slot empty"
        );
        app.update();
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert_eq!(
            sent.iter()
                .filter(|c| matches!(c, ClientCommand::ClearTradeItem { .. }))
                .count(),
            1,
            "scanned once: {sent:?}"
        );
        assert!(seen(&mut app).is_empty());
    }

    /// The placed item is a live object: its WRAPPED flag and `ITEM_FIELD_GIFTCREATOR` reach the
    /// slot when the watchers place it again (`0x52b789`, `0x52e32a`), the giver by name.
    #[test]
    fn a_wrapped_offer_placed_again_carries_its_giver() {
        use benilla_protocol::field::FIELD_ITEM_STACK_COUNT;
        use benilla_protocol::ObjectFields;
        let (mut app, _rx) = resignal_app();
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(0x77, "Santa".to_string(), None);
        let linen = app.world().resource::<GuidIndex>().0[&LINEN];
        // ITEM_FIELD_GIFTCREATOR is the guid at fields 12 and 13, ITEM_FIELD_FLAGS field 21.
        app.world_mut()
            .get_mut::<ObjectStore>(linen)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[
                (FIELD_ITEM_STACK_COUNT, 2),
                (12, 0x77),
                (13, 0),
                (21, 0x8),
            ]));
        notify(
            &mut app,
            crate::item_watch::ItemNotice::StackCount {
                guid: LINEN,
                entry: 2589,
            },
        );
        let slots = app
            .world()
            .non_send_resource::<UiScript>()
            .trade_player_slots()
            .expect("open trade");
        let row = slots[0].as_ref().expect("the slot is filled");
        assert_eq!(row.object_flags, Some(0x8));
        assert_eq!(
            (row.wrapped, row.gift_creator.as_deref()),
            (true, Some("Santa"))
        );
    }

    /// `0x4bfb60` from the stack and enchantment watchers: the same item placed again from its
    /// position, the slot showing the live count, and one signal; a placement of an item that
    /// is no longer an object sends and signals nothing (`0x5d43da`).
    #[test]
    fn a_restacked_offer_is_placed_again_with_its_live_count() {
        use benilla_protocol::field::FIELD_ITEM_STACK_COUNT;
        use benilla_protocol::ObjectFields;
        let (mut app, rx) = resignal_app();
        let linen = app.world().resource::<GuidIndex>().0[&LINEN];
        app.world_mut()
            .get_mut::<ObjectStore>(linen)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 7)]));
        notify(
            &mut app,
            crate::item_watch::ItemNotice::StackCount {
                guid: LINEN,
                entry: 2589,
            },
        );
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            sent.iter().any(|c| matches!(
                c,
                ClientCommand::SetTradeItem {
                    trade_slot: 0,
                    bag: 255,
                    slot: 23
                }
            )),
            "CMSG_SET_TRADE_ITEM from backpack slot 1: {sent:?}"
        );
        assert_eq!(seen(&mut app), ["TRADE_PLAYER_ITEM_CHANGED"]);
        let count: i64 = app
            .world_mut()
            .non_send_resource::<UiScript>()
            .eval("local _, _, n = GetTradePlayerItemInfo(1) return n")
            .unwrap();
        assert_eq!(count, 7, "the slot reads the live stack");
        assert_eq!(
            app.world().resource::<TradeSession>().own_edits,
            2,
            "the placement is one of our edits (`0x5d4442`)"
        );
        notify(
            &mut app,
            crate::item_watch::ItemNotice::Enchantment {
                guid: LINEN,
                entry: 2589,
            },
        );
        assert_eq!(rx.try_iter().count(), 1, "the enchantment places it again");
        assert_eq!(seen(&mut app), ["TRADE_PLAYER_ITEM_CHANGED"]);
        assert_eq!(app.world().resource::<TradeSession>().own_edits, 3);
        app.world_mut().resource_mut::<GuidIndex>().0.remove(&LINEN);
        notify(
            &mut app,
            crate::item_watch::ItemNotice::StackCount {
                guid: LINEN,
                entry: 2589,
            },
        );
        assert!(rx.try_iter().next().is_none(), "no item, no send");
        assert!(seen(&mut app).is_empty());
        assert_eq!(app.world().resource::<TradeSession>().own_edits, 3);
    }

    /// Each item handler refreshes the action bar before it scans the trade (`0x5d8798` before
    /// `0x5d87a7`, `0x5d94e1` before `0x5d94f0`, `0x5d9afd` before `0x5d9b0c`), and the slot's
    /// signal lands before the feed's push of the column.
    #[test]
    fn the_trade_re_signal_follows_the_bar_recount_and_precedes_the_feed() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        assert!(crate::test_support::runs_before(
            &mut app,
            crate::ui_action::recount_item_actions,
            resignal_trade_items
        ));
        assert!(crate::test_support::runs_before(
            &mut app,
            resignal_trade_items,
            feed_trade
        ));
    }

    /// Slot 7's spell per side (`0x4bf2b0`): the partner's window carries the spell cast on our
    /// slot 7, which repaints ours and names its enchantment; our window carries ours on theirs.
    /// A resend of the same spell signals nothing, and the record compare ignores the name.
    #[test]
    fn the_enchant_slots_spell_signals_its_side_and_names_the_enchantment() {
        const FIERY: u32 = 13898;
        let (mut app, _rx) = feed_app();
        app.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from(
                [(
                    FIERY,
                    benilla_formats::SpellDisplay {
                        name: "Enchant Weapon - Fiery Weapon".into(),
                        ..Default::default()
                    },
                )],
            )),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
        open_with(&mut app, 0x7);
        app.update();
        let mut slots = [None; TRADE_SLOT_COUNT];
        slots[ENCHANT_SLOT] = Some(wire_item(2589));
        let ext = |their_window: bool, spell: u32| TradeStatusExtended {
            their_window,
            sequence: 7,
            gold: 0,
            enchant_spell_id: spell,
            slots,
        };
        // Both columns hold an item in slot 7, as their windows say.
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.set_offer(&ext(true, 0));
            trade.set_offer(&ext(false, 0));
        }
        app.update();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"SEEN = {}
                   local f = CreateFrame("Frame")
                   f:RegisterEvent("TRADE_PLAYER_ITEM_CHANGED")
                   f:RegisterEvent("TRADE_TARGET_ITEM_CHANGED")
                   f:SetScript("OnEvent", function() table.insert(SEEN, event .. " " .. arg1) end)"#,
            )
            .unwrap();
        let enchantments = |app: &mut App| -> (Option<String>, Option<String>) {
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval(
                    "local _, _, _, _, p = GetTradePlayerItemInfo(7) \
                     local _, _, _, _, _, t = GetTradeTargetItemInfo(7) return p, t",
                )
                .unwrap()
        };
        app.world_mut()
            .resource_mut::<TradeSession>()
            .set_offer(&ext(true, FIERY));
        app.update();
        assert_eq!(seen(&mut app), ["TRADE_PLAYER_ITEM_CHANGED 7"]);
        assert_eq!(
            enchantments(&mut app),
            (Some("Enchant Weapon - Fiery Weapon".into()), None)
        );
        app.world_mut()
            .resource_mut::<TradeSession>()
            .set_offer(&ext(true, FIERY));
        app.update();
        assert!(seen(&mut app).is_empty(), "the same spell again");
        app.world_mut()
            .resource_mut::<TradeSession>()
            .set_offer(&ext(false, FIERY));
        app.update();
        assert_eq!(seen(&mut app), ["TRADE_TARGET_ITEM_CHANGED 7"]);
        assert_eq!(
            enchantments(&mut app),
            (
                Some("Enchant Weapon - Fiery Weapon".into()),
                Some("Enchant Weapon - Fiery Weapon".into())
            )
        );
        // Cleared on our side: the partner's slot 7 repaints and names nothing.
        app.world_mut()
            .resource_mut::<TradeSession>()
            .set_offer(&ext(false, 0));
        app.update();
        assert_eq!(seen(&mut app), ["TRADE_TARGET_ITEM_CHANGED 7"]);
        assert_eq!(enchantments(&mut app).1, None);
    }
}
