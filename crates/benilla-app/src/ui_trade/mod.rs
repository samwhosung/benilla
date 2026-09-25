//! The trade window: [`net`] folds the two status packets into [`TradeSession`], [`feed_trade`]
//! pushes it into the VM and fires the trade events, [`drain_trade`] sends the Lua intents, and
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
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::NpcSession;

mod net;

/// One side's offer: slots 0-5 change hands, slot 6 is the non-traded enchant slot.
#[derive(Default)]
struct TradeOffer {
    slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
    gold: u32,
    /// The spell on this side's enchant slot, 0 for none; stored, not shown yet.
    enchant_spell_id: u32,
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
    /// A `TRADE_REQUEST_CANCEL` owed to [`feed_trade`]. `CANCELED` signals it (`0x4bf832`)
    /// before it clears the window (`0x4bf842`), so it survives [`Self::close_window`].
    request_cancel: bool,
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

    /// `OPEN_WINDOW`: the feed's open edge fires `TRADE_SHOW`.
    pub(crate) fn open_window(&mut self) {
        self.open = true;
    }

    /// `SMSG_TRADE_STATUS_EXTENDED`: one side's new offer, which drops both accepts.
    pub(crate) fn set_offer(&mut self, ext: &TradeStatusExtended) {
        let side = if ext.their_window {
            &mut self.their
        } else {
            &mut self.our
        };
        side.slots = ext.slots;
        side.gold = ext.gold;
        side.enchant_spell_id = ext.enchant_spell_id;
        self.our_accept = false;
        self.their_accept = false;
    }

    /// Place our item in trade slot `id_1based` client-side: vmangos sends a placement to the
    /// partner only (`TradeData.cpp:81`), never back to the placer.
    pub(crate) fn place_own_item(&mut self, id_1based: u32, item: TradeItem) {
        if let Some(slot) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our.slots.get_mut(i as usize))
        {
            *slot = Some(item);
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

    /// Owe `TRADE_REQUEST_CANCEL`, the one Lua event the status dispatcher fires (`0x4bf832`, the
    /// sole `SignalEvent` in `[0x4bf720, 0x4bfa08)`).
    pub(crate) fn signal_request_cancel(&mut self) {
        self.request_cancel = true;
    }

    fn take_request_cancel(&mut self) -> bool {
        std::mem::take(&mut self.request_cancel)
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

    /// Close the window, the reference's `SetTradePartner(0, 0)` (`0x4bf4e0`), from `CANCELED`,
    /// `COMPLETE`, `CLOSE_WINDOW` or the local `CloseTrade` (`0x4bfdc0`): a full reset, a superset
    /// of [`Self::clear_pending`], keeping the two signals the reference raises before it,
    /// [`Self::request_cancel`] and [`Self::named_lines`].
    pub(crate) fn close_window(&mut self) {
        *self = TradeSession {
            request_cancel: self.request_cancel,
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
        }
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

/// The `"npc"` portrait points at the partner only while the window is open.
impl NpcSession for TradeSession {
    fn npc(&self) -> Option<u64> {
        self.open.then_some(self.partner).flatten()
    }

    fn close(&mut self) {
        self.close_window();
    }
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
        app.init_resource::<TradeSession>()
            .init_resource::<BlockTrades>()
            .add_systems(
                Update,
                (
                    // The feed runs after `UnitFeed`, whose item templates it reads, and before
                    // the input pass, with the drain after it, so each lands the same frame. No
                    // range guard: vmangos cancels a trade on distance (`Player.cpp:6117`).
                    feed_trade.after(crate::ui_unit::UnitFeed).in_set(UiFeed),
                    // Ahead of the feed, so an accepted partner shows the frame the window opens.
                    answer_trade_request.before(feed_trade),
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

    // Legs 4-7, a silent busy: a dead or ghost initiator (`0x605f30`, raw health 0 or the ghost
    // flag; `unit_reads_dead`, like `UnitIsDead` `0x517ac0`, would refuse a feigning hunter), a
    // cinematic (`[0xb4e310]`), lost player control (`[0xb4b3e4]`, which `0x4958e0` writes, not an
    // in-world flag) or an open auction house (`[0xb725f8]`, its auctioneer, not a pending trade).
    let dead_or_ghost = initiator_store
        .is_some_and(|s| s.0.unit_health().is_some_and(|hp| hp == 0) || s.0.player_is_ghost());
    if dead_or_ghost
        || cinematic.is_playing()
        || player.control_lost
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
        let Some(name) = names.resolve(initiator, &commands).map(str::to_string) else {
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
/// answers, the icon from `display_id` via `ItemDisplayInfo.dbc`; `enchantment` is not filled yet.
fn resolve_slot(
    item: &TradeItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> TradeSlotItem {
    let template = items.template(item.entry, 0, commands);
    let name = template.map(|t| t.name.clone());
    let quality = template.map(|t| t.quality);
    let texture = icons
        .and_then(|i| i.catalog.get(item.display_id))
        .and_then(|d| d.icon.clone());
    // Nil until the template answers: the link embeds the name and the quality colour.
    let link = match (name.as_deref(), quality) {
        (Some(n), Some(q)) => Some(crate::ui_items::item_link(item.entry, n, q)),
        _ => None,
    };
    TradeSlotItem {
        item_id: item.entry,
        name,
        texture,
        count: item.count.max(1),
        quality,
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
) -> Option<TradeItem> {
    let (guid, count) = crate::ui_items::slot_guid_count(store, bag, slot, objects);
    if guid == 0 {
        return None;
    }
    let entry = objects.object(guid).and_then(|f| f.object_entry())?;
    let display_id = items
        .template(entry, guid, commands)
        .map(|t| t.display_info_id)
        .unwrap_or(0);
    Some(TradeItem {
        entry,
        display_id,
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
    })
}

fn resolve_side(
    offer: &TradeOffer,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> TradeSideState {
    let mut slots: [Option<TradeSlotItem>; TRADE_SLOT_COUNT] = Default::default();
    for (i, s) in offer.slots.iter().enumerate() {
        slots[i] = s.map(|it| resolve_slot(&it, items, icons, commands));
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
    names: &NameCache,
    commands: &NetCommands,
) -> Option<TradeState> {
    if !trade.open {
        return None;
    }
    Some(TradeState {
        player: resolve_side(&trade.our, items, icons, commands),
        target: resolve_side(&trade.their, items, icons, commands),
        partner_name: trade
            .partner
            .and_then(|g| names.resolve(g, commands).map(str::to_string)),
    })
}

/// Push the trade into the VM and fire its events on a change, diffed against `Local` memory;
/// `TRADE_ACCEPT_UPDATE` fires after `TRADE_SHOW`, whose handler hides the highlights.
fn feed_trade(
    script: Option<NonSendMut<UiScript>>,
    mut trade: ResMut<TradeSession>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    // The `%s` lines' live-object guard, the same reads as the ladder's leg 3.
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    mut last: Local<crate::ui_script::VmMemo<Option<TradeState>>>,
    mut last_open: Local<crate::ui_script::VmMemo<bool>>,
    mut last_accept: Local<crate::ui_script::VmMemo<(bool, bool)>>,
    mut last_player_gold: Local<crate::ui_script::VmMemo<u32>>,
    mut last_their_gold: Local<crate::ui_script::VmMemo<u32>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_open = last_open.get(&script);
    let last_accept = last_accept.get(&script);
    let last_player_gold = last_player_gold.get(&script);
    let last_their_gold = last_their_gold.get(&script);

    let fresh = snapshot(&trade, &items, icons.as_deref(), &names, &commands);
    let opened = !*last_open && trade.is_open();
    let closed = *last_open && !trade.is_open();
    let changed = fresh != *last;
    if changed {
        script.set_trade(fresh.clone());
    }
    // The per-slot events the stock `TradeFrame.lua` repaints one slot on, arg1 the 1-based slot
    // (`0x4bf414`/`0x4bf452` target, `0x4bf487`/`0x4bfaef` player); the reference also fires the
    // full `TRADE_UPDATE` (`0x4c034f`).
    if changed && trade.is_open() && !opened {
        let empty = TradeState::default();
        let old = last.as_ref().unwrap_or(&empty);
        if let Some(new) = fresh.as_ref() {
            let changed_slots = |mine: &[Option<TradeSlotItem>],
                                 theirs: &[Option<TradeSlotItem>]| {
                mine.iter()
                    .zip(theirs.iter())
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .map(|(i, _)| ScriptValue::Int(i as i64 + 1))
                    .collect::<Vec<_>>()
            };
            for slot in changed_slots(&new.player.slots, &old.player.slots) {
                script.fire_event("TRADE_PLAYER_ITEM_CHANGED", vec![slot]);
            }
            for slot in changed_slots(&new.target.slots, &old.target.slots) {
                script.fire_event("TRADE_TARGET_ITEM_CHANGED", vec![slot]);
            }
        }
    }
    // The owed `%s` lines, resolved here where the name cache is; a miss waits for the query it
    // started. The catalog row, read by `ui_action::feed_actions`, decides where each lands.
    for line in trade.take_named_lines() {
        // The status arms print nothing once the player is no longer an object.
        if line.needs_live_object && streamed_player(&index, &stores, line.who).is_none() {
            continue;
        }
        match names.resolve(line.who, &commands).map(str::to_string) {
            Some(name) => errors.0.push(crate::ui_action::UiError::s(line.key, name)),
            // Not cached: `resolve` has asked, so park it and retry next frame.
            None => trade.owe_named_line(line),
        }
    }
    // `TRADE_REQUEST_CANCEL`, ahead of the `TRADE_CLOSED` the same arm fires: `0x4bf832` signals,
    // then `0x4bf842` calls `0x4bf4e0`, which fires `TRADE_CLOSED` at `0x4bf522`. Stock
    // `UIParent.lua:395` only hides the never-shown `TRADE` popup on it; addons may listen.
    if trade.take_request_cancel() {
        script.fire_event("TRADE_REQUEST_CANCEL", vec![]);
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
    } else if closed {
        script.fire_event("TRADE_CLOSED", vec![]);
    } else if trade.is_open() && changed {
        script.fire_event("TRADE_UPDATE", vec![]);
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
    if closed {
        *last_accept = (false, false);
    } else {
        *last_accept = accept;
    }

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

/// Drain the Lua intents into the trade `CMSG`s. `CloseTrade` clears the session, sending
/// `CMSG_CANCEL_TRADE` only while one is live.
fn drain_trade(
    script: Option<NonSendMut<UiScript>>,
    mut trade: ResMut<TradeSession>,
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
    // A money offer sends `CMSG_SET_TRADE_GOLD` while open; vmangos sends it to the partner only,
    // so our own gold is set here.
    if let Some(copper) = script.take_trade_money() {
        if trade.is_open() {
            trade.set_own_gold(copper);
            info!(target: "trade", "set gold: {copper} copper; sending CMSG_SET_TRADE_GOLD");
            let _ = commands.0.send(ClientCommand::SetTradeGold { copper });
        }
    }
    // Placements and clears, while open: the UI id is 1-based, the wire slot 0-based, and the
    // cursor's bag and slot map to the wire position.
    for (id, bag, slot) in script.take_trade_set_items() {
        if !trade.is_open() {
            continue;
        }
        let Some((wire_bag, wire_slot)) = crate::ui_items::wire_pos(bag, slot) else {
            info!(target: "trade", "set item: slot {id} <- bag {bag}/{slot} has no wire position — dropped");
            continue;
        };
        if let Some(trade_slot) = id.checked_sub(1).and_then(|n| u8::try_from(n).ok()) {
            // vmangos echoes a placement to the partner only, so our column is filled here.
            if let Some(item) = own_item_at(bag, slot, store, &objects, &items, &commands) {
                trade.place_own_item(id, item);
            }
            info!(target: "trade", "set item: slot {id} <- bag {bag}/{slot} (wire {wire_bag}/{wire_slot}); sending CMSG_SET_TRADE_ITEM");
            let _ = commands.0.send(ClientCommand::SetTradeItem {
                trade_slot,
                bag: wire_bag,
                slot: wire_slot,
            });
        }
    }
    for id in script.take_trade_clear_items() {
        if !trade.is_open() {
            continue;
        }
        if let Some(trade_slot) = id.checked_sub(1).and_then(|n| u8::try_from(n).ok()) {
            trade.clear_own_item(id);
            info!(target: "trade", "clear item: slot {id}; sending CMSG_CLEAR_TRADE_ITEM");
            let _ = commands
                .0
                .send(ClientCommand::ClearTradeItem { trade_slot });
        }
    }
    if script.take_trade_close() {
        if trade.is_open() || trade.partner.is_some() {
            let _ = commands.0.send(ClientCommand::CancelTrade);
        }
        trade.close_window();
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
        let _ = commands
            .0
            .send(ClientCommand::SetTradeGold { copper: trimmed });
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

    #[test]
    fn open_close_state_machine() {
        let mut s = TradeSession::default();
        assert!(!s.is_open());

        s.initiate(0x1234);
        assert_eq!(s.partner, Some(0x1234));
        assert!(!s.is_open());
        assert_eq!(s.npc(), None);

        s.open_window();
        assert!(s.is_open());
        assert_eq!(s.npc(), Some(0x1234));

        s.close_window();
        assert!(!s.is_open());
        assert_eq!(s.partner, None);
        assert_eq!(s.npc(), None);
    }

    #[test]
    fn an_incoming_request_is_not_yet_a_trade() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        assert_eq!(s.pending_request(), Some(0xABCD));
        assert_eq!(s.partner, None, "an unanswered request has no partner");
        assert!(!s.is_open(), "BEGIN_TRADE does not itself open the window");
        assert_eq!(s.npc(), None);
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

        s.place_own_item(2, wire_item(1234));
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
            .insert_resource(NetCommands(tx))
            .insert_resource(Selection {
                target: None,
                guid: Some(0x7), // high 16 bits 0: a player guid
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
            .insert_resource(names)
            .insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_trade);
        (app, rx)
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

        // A plain close, as `COMPLETE` and `CLOSE_WINDOW` do, signals nothing.
        app.world_mut()
            .resource_mut::<TradeSession>()
            .close_window();
        app.update();
        assert_eq!(
            cancels_seen(&mut app),
            0,
            "closing the window is not itself a cancel signal"
        );

        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.signal_request_cancel();
            trade.close_window();
        }
        app.update();
        assert_eq!(cancels_seen(&mut app), 1, "CANCELED signals it, once");
        app.update();
        assert_eq!(
            cancels_seen(&mut app),
            1,
            "and only once — the flag is taken"
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
            "0x7 is named in the cache but is not a streamed object: the status line is dropped,              the initiate line is not"
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
        // The `UNIT_FIELD_HEALTH` and `PLAYER_FLAGS` field ids, and `PLAYER_FLAGS_GHOST`.
        const HEALTH: u16 = 22;
        const PLAYER_FLAGS: u16 = 190;
        const GHOST: u32 = 0x10;
        for (label, fields) in [
            ("dead", vec![(HEALTH, 0u32)]),
            ("ghost", vec![(HEALTH, 1u32), (PLAYER_FLAGS, GHOST)]),
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
            .control_lost = !controlled;
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

    #[test]
    fn drain_money_ships_set_trade_gold_only_when_open() {
        use bevy::ecs::system::RunSystemOnce;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
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
        assert!(rx.try_recv().is_err(), "no CMSG with no open trade");

        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window();
        }
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("SetTradeMoney(12345)")
            .unwrap();
        app.world_mut().run_system_once(drain_trade).unwrap();
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::SetTradeGold { copper: 12_345 })
            ),
            "an open-window money offer ships CMSG_SET_TRADE_GOLD"
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
            .insert_resource(NetCommands(tx))
            .insert_resource(Selection::default());
        app.insert_non_send_resource(UiScript::new().unwrap());
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window();
        }

        // Our slot 1 filled, then an empty-cursor click on it.
        {
            let mut vm = app.world_mut().non_send_resource_mut::<UiScript>();
            let mut st = benilla_ui::script::TradeState::default();
            st.player.slots[0] = Some(benilla_ui::script::TradeSlotItem {
                item_id: 2589,
                count: 1,
                ..Default::default()
            });
            vm.set_trade(Some(st));
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
}
