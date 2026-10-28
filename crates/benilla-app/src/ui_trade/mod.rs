//! The trade window: [`net`] folds the two status packets into [`TradeSession`] and queues what
//! they signal on the trade lane at their handlers ([`lane`]), whose listeners' getters read the
//! session through the host ([`live`]); the trade verbs act inside their calls ([`TradeSteps`]),
//! `CloseTrade`'s close core among them; the getters' record asks register their callbacks inside
//! their calls ([`ask_trade_record`]), whose answers signal `TRADE_UPDATE` on the item lane
//! ([`crate::item_lane`]); and an incoming request is answered inside its status
//! ([`answer_request`]).

use benilla_protocol::messages::{TradeItem, TradeStatusExtended, TRADE_SLOT_COUNT};
use bevy::prelude::*;

use benilla_ui::script::{ItemRead, TradeSlotItem, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects};
use crate::query_cache::Ask;
use crate::ui_action::{FillArg, UiError};
use crate::ui_script::{SweepApp, SweepLeg};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

mod calls;
mod lane;
mod live;
mod net;

#[cfg(test)]
mod call_tests;

pub(crate) use calls::TradeSteps;
pub(crate) use lane::{TradeEvent, TradeLane};
pub(crate) use live::TradeLive;

/// One side's offer: slots 0-5 change hands, slot 6 is the non-traded enchant slot. The records
/// are what the window's compare keeps (`0x4bf37e`) and the getters read, which only an open clears
/// (`0x4bf549`-`0x4bf5ef`); the gold is the trade's money cell (`[0xc4be70]`), which the ending
/// statuses zero too.
#[derive(Default)]
struct TradeOffer {
    slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
    gold: u32,
    /// The header's spell, `[0xc4bed8 + 4*side]` (`0x5d49fa`): the spell this side's player cast
    /// on the other side's enchant slot, 0 for none. Our own window's names the partner's slot 7,
    /// the partner's names ours (`0x4c02b7`, `0x4c05d1`).
    enchant_spell_id: u32,
}

/// The trade session [`net`] fills and the getters read, the offers kept over a close for the
/// handlers' getters and cleared at the next open; cleared whole on disconnect.
#[derive(Resource, Default)]
pub(crate) struct TradeSession {
    /// The cascade's reset of the window's tables, owed from its item's push
    /// ([`TradeSession::settle_world_reset`]).
    pub(crate) world_reset: crate::ui_world_enter::OwedReset,
    /// Set at our initiate or the ladder's accept, since `OPEN_WINDOW` carries no guid.
    partner: Option<u64>,
    /// Our `CMSG_INITIATE_TRADE` is live: the reference's `[0xc4bec8]`, set only as it goes out
    /// (`0x5d4021`) and cleared by the status handler's tail (`0x5d4931`), which codes 1, 2, 4,
    /// 7, 9 and 22 skip, so it survives `OPEN_WINDOW` for the life of a trade we opened.
    initiated: bool,
    /// The initiate's mode `[0xc4becc]` beside the pending flag (`0x5d402b`): 1 from the two
    /// cursor-drop starters (`DropItemOnUnit` at `0x48da62`, the world click at `0x49278e`), 0 from
    /// `InitiateTrade` (`0x48a188`). The window's open moves what the cursor holds into the offer
    /// only when both are set (`0x5d3f80`, read at `0x4bf5f1`).
    cursor_drop: bool,
    /// `OPEN_WINDOW` arrived; `partner` is set before it.
    open: bool,
    our: TradeOffer,
    their: TradeOffer,
    /// Our accept flag `[0xb71730]`: written by `0x4bf230` alone, and zeroed with no signal by the
    /// window's open (`0x4bf5c0`).
    our_accept: bool,
    /// The partner's `[0xb71734]`: written by `0x4bf280` alone, and zeroed by the open (`0x4bf5c6`).
    their_accept: bool,
    /// The own-edit counter `[0xc4bea8]`: 1 at `OPEN_WINDOW` (`0x5d4825`), one more for each
    /// `CMSG_SET_TRADE_ITEM`, `CMSG_CLEAR_TRADE_ITEM` and `CMSG_SET_TRADE_GOLD` we send
    /// (`0x5d4442`, `0x5d44bc`, `0x5d45a5`, `0x5d463b`, `0x5d4712`). An own-side status whose
    /// sequence is below it is stale (`0x5d49ba`).
    own_edits: u32,
    /// The partner's flag `[0xc4be7c]`, which `CMSG_ACCEPT_TRADE` carries (`0x5d4260`): 1 at
    /// `OPEN_WINDOW` (`0x5d4834`), then the second header dword of each partner-side extended
    /// status (`0x5d49d5`); a fresh client's is 0.
    partner_flag: u32,
    /// Each side's money as `0x4bf2b0` last compared it (`0xb71740`, `0xb71744`): zero for a
    /// fresh window (`0x4bf5d8`), our offer once the open folds held coins in (`0x4bf62b`), and
    /// our 0 when a status bounces our coins (`0x4bfbe0`).
    money_seen: [u32; 2],
    /// Each side's spell as `0x4bf2b0` last compared it (`0xb71738`, `0xb7173c`), indexed by the
    /// header's side byte; zero for a fresh window (`0x4bf5cc`).
    spell_seen: [u32; 2],
    /// The initiates whose partner's name missed the cache, one callback each (`0x5d4080`), by
    /// guid: they hang off the name query, not the trade, so a close keeps them.
    initiate_waits: Vec<u64>,
}

impl TradeSession {
    /// Our `CMSG_INITIATE_TRADE` is out (`0x5d3fb0`): a fresh session with the target as partner,
    /// the pending flag set and the starter's mode beside it (`0x5d4021`, `0x5d402b`). The offers
    /// stay for the next open to clear.
    pub(crate) fn initiate(&mut self, target: u64, cursor_drop: bool) {
        *self = TradeSession {
            partner: Some(target),
            initiated: true,
            cursor_drop,
            ..self.carried()
        };
    }

    /// `0x5d3fb0` whole: `CMSG_INITIATE_TRADE`, the session's cells, then the partner's name from
    /// the cache with the callback `0x5d4080` (`0x5d4031`): a held name shows
    /// `ERR_INITIATE_TRADE_S` at once (`0x5d4042`), a miss when the answer lands (`0x5d40ca`,
    /// [`TradeNameAnswers`]).
    pub(crate) fn initiate_send(
        &mut self,
        target: u64,
        cursor_drop: bool,
        net: &NetCommands,
        names: &NameCache,
        errors: &crate::ui_action::UiErrorKeys,
    ) {
        let _ = net.0.send(ClientCommand::InitiateTrade { target });
        self.initiate(target, cursor_drop);
        match names.resolve_player(target, Ask::Event(net)) {
            Some(name) => errors.push(UiError::s("ERR_INITIATE_TRADE_S", name)),
            None => self.initiate_waits.push(target),
        }
    }

    /// What a fresh session keeps of this one: both offers, which the getters read until the next
    /// open clears them, and the name callbacks, which hang off the name query.
    fn carried(&mut self) -> TradeSession {
        TradeSession {
            our: std::mem::take(&mut self.our),
            their: std::mem::take(&mut self.their),
            initiate_waits: std::mem::take(&mut self.initiate_waits),
            ..Default::default()
        }
    }

    /// Accept a request as a fresh session; the server then opens both windows.
    pub(crate) fn begin(&mut self, partner: u64) {
        *self = TradeSession {
            partner: Some(partner),
            ..self.carried()
        };
    }

    /// `OPEN_WINDOW`: the handler's cells (`0x5d4825`-`0x5d4864`: the own-edit counter and both
    /// flags 1, the records and money cleared, the spells 0), then the open's clears of the
    /// window's own (`0x4bf549`-`0x4bf5ea`): both accepts with no signal (`0x4bf5c0`, `0x4bf5c6`)
    /// and the compare's caches. The lane's open fires `TRADE_SHOW`.
    pub(crate) fn open_window(&mut self) {
        self.open = true;
        self.own_edits = 1;
        self.partner_flag = 1;
        self.our = TradeOffer::default();
        self.their = TradeOffer::default();
        self.money_seen = [0, 0];
        self.spell_seen = [0, 0];
        self.our_accept = false;
        self.their_accept = false;
    }

    /// `0x5d3f80`: our initiate is pending and came from a cursor drop, so the window's open moves
    /// what the cursor holds into the offer (`0x4bf5f1`).
    pub(crate) fn opens_with_held(&self) -> bool {
        self.initiated && self.cursor_drop
    }

    /// Send one of our offer edits and count it, as the reference's five senders do (item set
    /// `0x5d4380`, clear `0x5d4480`, money add `0x5d4500`, subtract `0x5d45f0`, set `0x5d4680`):
    /// the counter moves with the packet and with nothing else, so an edit that is refused
    /// before it goes out leaves it alone.
    pub(crate) fn send_edit(&mut self, commands: &NetCommands, command: ClientCommand) {
        self.own_edits = self.own_edits.wrapping_add(1);
        let _ = commands.0.send(command);
    }

    /// `SMSG_TRADE_STATUS_EXTENDED` (`0x5d4990`): one side's new offer, its signals queued on
    /// `lane` in the handler's order. A status for our own column below the own-edit counter is
    /// dropped whole (`0x5d49ba` returns before the store, the invalidations and `0x4bf2b0`): it
    /// answers an offer we have since edited. Our slot that held an item and comes back empty is
    /// invalidated as the records are stored (`0x5d4ad3`, `0x5d4b33`); then `0x4bf2b0` with the
    /// partner's records (`0x5d4b67`), whichever side the status was for: both accepts dropped
    /// (`0x4bf2b6`, `0x4bf2bd`), each partner slot whose record moved signalled (`0x4bf2ca`-
    /// `0x4bf414`), then the two spells (`0x4bf42b`, `0x4bf45f`) and the two money offers
    /// (`0x4bf48f`, `0x4bf4ba`), each stored before its signal.
    pub(crate) fn set_offer(&mut self, ext: &TradeStatusExtended, lane: &mut TradeLane) {
        if !ext.their_window && ext.sequence < self.own_edits {
            debug!(
                target: "trade",
                "stale own-side status dropped: sequence {} < {} edits",
                ext.sequence,
                self.own_edits,
            );
            return;
        }
        let shown = self.their.slots;
        if ext.their_window {
            self.partner_flag = ext.flag;
        } else {
            for (i, (old, new)) in self.our.slots.iter().zip(ext.slots.iter()).enumerate() {
                if old.is_some() && new.is_none() {
                    lane.push(TradeEvent::Invalidate(i as u8));
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
        self.back_to_trade(lane);
        // The compare reads every word of the record but the display (`+0x04`) and the unwritten
        // `+0x1c`.
        let record = |s: &Option<TradeItem>| s.map(|t| TradeItem { display_id: 0, ..t });
        for (i, (old, new)) in shown.iter().zip(self.their.slots.iter()).enumerate() {
            if record(old) != record(new) {
                lane.push(TradeEvent::TargetItem(i as u8));
            }
        }
        let enchant = ENCHANT_SLOT as u8;
        let spells = [
            (self.our.enchant_spell_id, TradeEvent::TargetItem(enchant)),
            (self.their.enchant_spell_id, TradeEvent::PlayerItem(enchant)),
        ];
        for ((spell, event), seen) in spells.into_iter().zip(self.spell_seen.iter_mut()) {
            if *seen != spell {
                *seen = spell;
                lane.push(event);
            }
        }
        let moneys = [
            (self.our.gold, TradeEvent::PlayerMoney),
            (self.their.gold, TradeEvent::TargetMoney),
        ];
        for ((gold, event), seen) in moneys.into_iter().zip(self.money_seen.iter_mut()) {
            if *seen != gold {
                *seen = gold;
                lane.push(event);
            }
        }
    }

    /// Place our item in trade slot `id_1based` client-side: vmangos sends a placement to the
    /// partner only (`TradeData.cpp:81`), never back to the placer.
    pub(crate) fn place_own_item(&mut self, id_1based: u32, item: TradeItem) {
        if let Some(slot) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our.slots.get_mut(i as usize))
        {
            *slot = Some(item);
        }
    }

    /// `0x5d4380` for our 0-based `slot`: `CMSG_SET_TRADE_ITEM` from the item's position, counted,
    /// refused with no active player or no object for `guid` (`0x5d43af`, `0x5d43da`), or for a
    /// position with no wire form. vmangos echoes a placement to the partner only
    /// (`TradeData.cpp:81`), so our own column is filled here. Whether it went.
    #[allow(clippy::too_many_arguments)] // the sender's own arguments
    pub(crate) fn send_placement(
        &mut self,
        commands: &NetCommands,
        objects: &Objects,
        items: &Items,
        slot: u8,
        guid: u64,
        bag: i64,
        bag_slot: u32,
    ) -> bool {
        let fields = objects.player().and(objects.object(guid));
        let (Some(fields), Some((wire_bag, wire_slot))) =
            (fields, crate::ui_items::wire_pos(bag, bag_slot))
        else {
            info!(target: "trade", "set item: slot {} <- bag {bag}/{bag_slot} not sent", slot + 1);
            return false;
        };
        let count = fields.item_stack_count().unwrap_or(1);
        if let Some(item) = own_item(guid, count, objects, items) {
            self.place_own_item(u32::from(slot) + 1, item);
        }
        info!(target: "trade", "set item: slot {} <- bag {bag}/{bag_slot} (wire {wire_bag}/{wire_slot}); sending CMSG_SET_TRADE_ITEM", slot + 1);
        self.send_edit(
            commands,
            ClientCommand::SetTradeItem {
                trade_slot: slot,
                bag: wire_bag,
                slot: wire_slot,
            },
        );
        true
    }

    /// The partner's record in 0-based `slot`, as the last status for their window left it.
    pub(crate) fn partner_slot(&self, slot: usize) -> Option<&TradeItem> {
        self.their.slots.get(slot)?.as_ref()
    }

    /// Clear our trade slot `id_1based` client-side.
    pub(crate) fn clear_own_item(&mut self, id_1based: u32) {
        if let Some(slot) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our.slots.get_mut(i as usize))
        {
            *slot = None;
        }
    }

    /// Set our gold client-side: vmangos sends it to the partner only (`TradeData.cpp:118`).
    pub(crate) fn set_own_gold(&mut self, copper: u32) {
        self.our.gold = copper;
    }

    /// `0x4bf230(0)` from a status: our flag down, which signals both flags (`0x4bf260`) and
    /// then sends `CMSG_UNACCEPT_TRADE` (`0x4bf272`), only when it was up (`0x4bf247`).
    fn unaccept_ours(&mut self, lane: &mut TradeLane) {
        if self.our_accept {
            self.our_accept = false;
            lane.push(TradeEvent::Accept(false, self.their_accept));
            lane.push(TradeEvent::Unaccept);
        }
    }

    /// `0x4bf280(flag)`: the partner's flag, which signals both flags when it moved (`0x4bf286`,
    /// `0x4bf29f`) and sends nothing.
    fn set_partner_accept(&mut self, flag: bool, lane: &mut TradeLane) {
        if self.their_accept != flag {
            self.their_accept = flag;
            lane.push(TradeEvent::Accept(self.our_accept, flag));
        }
    }

    /// `TRADE_ACCEPT` (case 4, `0x4bf810`): `0x4bf280(1)`.
    pub(crate) fn partner_accepted(&mut self, lane: &mut TradeLane) {
        self.set_partner_accept(true, lane);
    }

    /// `BACK_TO_TRADE` (case 7, `0x4bf81c`, `0x4bf823`), and every extended status before its
    /// records are compared (`0x4bf2b6`, `0x4bf2bd`): `0x4bf230(0)`, then `0x4bf280(0)`. vmangos
    /// sends it for any offer change, so our own edits drop the flags through it.
    pub(crate) fn back_to_trade(&mut self, lane: &mut TradeLane) {
        self.unaccept_ours(lane);
        self.set_partner_accept(false, lane);
    }

    /// Our flag up with no signal, for a test that seats an accept.
    #[cfg(test)]
    pub(crate) fn accept(&mut self) {
        self.our_accept = true;
    }

    #[cfg(test)]
    /// Our accept flag, the reference's `[0xb71730]`, which `0x4bf230` reads before it changes it
    /// or sends `CMSG_UNACCEPT_TRADE`.
    pub(crate) fn we_accepted(&self) -> bool {
        self.our_accept
    }

    /// `0x4bf230(flag)`'s store (`0x4bf25a`): whether our flag moved, which is when it signals and
    /// sends. The signal it makes is announced here, so no status repeats it.
    pub(crate) fn set_our_accept(&mut self, flag: bool) -> bool {
        if self.our_accept == flag {
            return false;
        }
        self.our_accept = flag;
        true
    }

    /// Both accept flags, ours first, as `TRADE_ACCEPT_UPDATE` carries them (`0x4bf24e`).
    pub(crate) fn accepts(&self) -> (bool, bool) {
        (self.our_accept, self.their_accept)
    }

    /// The name a `%s` status line carries, or `None` when the reference prints nothing: cases 0,
    /// 5 and 14 read the guid cell `[0xc4bed0]` under the latch `[0xc4bec8]`, both of which
    /// `InitiateTrade` (`0x5d3fb0`) sets, while the `BEGIN_TRADE` arm (`0x5d4815`) fills only the
    /// cell. So a refuser stays silent when vmangos echoes `IGNORE_YOU` to both sides
    /// (`TradeHandler.cpp:46`).
    pub(crate) fn line_names(&self) -> Option<u64> {
        self.initiated.then_some(self.partner).flatten()
    }

    /// The window's state cleared, as the close core does (`0x4bf516`): a full reset but the
    /// offers, which the handlers' getters read until the next open clears them, and the name
    /// callbacks, which hang off the name query.
    pub(crate) fn close_window(&mut self) {
        *self = self.carried();
    }

    /// The status handler's tail (`0x5d490a`): clear the initiate latch `[0xc4bec8]` and the
    /// pending guid `[0xc4bed0]`, on every code but 1, 2, 4, 7, 9 and 22, after the arm has read
    /// them. It never touches the window. `partner` stands for the pending guid only while the
    /// window is shut; once open it is the window partner `[0xb71728]`, which stays.
    pub(crate) fn clear_pending(&mut self) {
        self.initiated = false;
        if !self.open {
            self.partner = None;
        }
    }

    /// `TRADE_REJECTED` (case 9, `0x4bf821`): `0x4bf280(0)`; vmangos never sends it.
    pub(crate) fn partner_unaccepted(&mut self, lane: &mut TradeLane) {
        self.set_partner_accept(false, lane);
    }

    /// `ONLY_CONJURED` (case 22, `0x4bf9ec` → `0x4bfbd0`): empty the offending slot of our offer,
    /// or the gold for `0xff`, with its signals. Deviation: an out-of-range slot is dropped, where
    /// the reference indexes its 7-slot mirror unchecked, because an out-of-bounds write is not a
    /// behaviour to reproduce.
    pub(crate) fn bounce_own_offer(&mut self, slot: u8, lane: &mut TradeLane) {
        const MONEY: u8 = 0xff;
        if slot == MONEY {
            // `0x5d4864` zeroes the offer; `0x4bfbd0(0xff)` zeroes the cache and signals.
            self.our.gold = 0;
            self.money_seen[0] = 0;
            lane.push(TradeEvent::PlayerMoney);
        } else if let Some(cell) = self.our.slots.get_mut(usize::from(slot)) {
            *cell = None;
            lane.push(TradeEvent::Invalidate(slot));
        }
    }

    /// Both money offers zero (`0xc4be70`, `0xc4be74`), before the dispatcher, on the statuses that
    /// end a trade: cancel, complete, close-window and code 13 (`0x5d4884`-`0x5d48a2`,
    /// `0x5d48eb`-`0x5d48ff`).
    pub(crate) fn zero_gold(&mut self) {
        self.our.gold = 0;
        self.their.gold = 0;
    }

    /// The world-enter cascade's reset of the window's tables (`0x4bf0e0`): both offers' slots,
    /// the accepts and the compare's caches. The handler's cells (the pending initiate, the edit
    /// counter, the partner flag, the offered money and spells) are not the window's and stay; the
    /// sweep ahead of every cascade has closed the window. Owed from the cascade's push, it runs at
    /// the cascade's place or ahead of a later trade packet ([`crate::ui_world_enter::OwedReset`]).
    pub(crate) fn settle_world_reset(&mut self) {
        if !self.world_reset.take() {
            return;
        }
        self.our.slots = Default::default();
        self.their.slots = Default::default();
        self.our_accept = false;
        self.their_accept = false;
        self.money_seen = [0; 2];
        self.spell_seen = [0; 2];
    }

    /// Disconnect: a full reset, offers and waits included; a session teardown is not a trade
    /// event.
    pub(crate) fn clear_session(&mut self) {
        *self = TradeSession::default();
    }

    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// The open leg's coins: our offer `copper`, which the money cache takes (`0x4bf62b`).
    pub(crate) fn fold_held_coins(&mut self, copper: u32) {
        self.set_own_gold(copper);
        self.money_seen[0] = copper;
    }

    /// What `CMSG_ACCEPT_TRADE` carries ([`Self::partner_flag`]).
    pub(crate) fn partner_flag(&self) -> u32 {
        self.partner_flag
    }

    /// How many initiates waited on `guid`'s name, taking them.
    fn take_initiate_waits(&mut self, guid: u64) -> usize {
        let before = self.initiate_waits.len();
        self.initiate_waits.retain(|&g| g != guid);
        before - self.initiate_waits.len()
    }

    // ── Test accessors, for `net`'s tests ──

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
/// enters the core inside its call ([`crate::script_calls::ScriptCallHost`]).
impl LatchedWindow for TradeSession {
    const WINDOW: NpcWindow = NpcWindow::Trade;

    fn cell(&self) -> Option<u64> {
        self.open.then_some(self.partner).flatten()
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }
}

/// `OPEN_WINDOW`'s open, `0x4bf4e0` with the pending partner (`0x4bf7f8`): the latch first, with
/// no test of the cell (`0x4bf53e`), so the window a trade opens over, an NPC's or a trade's,
/// closes first; then the cell and a fresh offer (`0x4bf549`-`0x4bf5ea`), and the lane's open,
/// `purse` the coinage the handler found for a trade a drop started (`0x4bf5f1`), which fires
/// `TRADE_SHOW` (`0x4bf644`). With no partner the call is the close (`0x4bf4ed`).
pub(crate) fn open_window(world: &mut World, purse: Option<u64>) {
    let Some(partner) = world.resource::<TradeSession>().partner else {
        close_core(world);
        return;
    };
    crate::ui_session::register(world, partner);
    let mut trade = world.resource_mut::<TradeSession>();
    trade.partner = Some(partner);
    trade.open_window();
    if let Some(mut lane) = world.get_resource_mut::<TradeLane>() {
        lane.push(TradeEvent::Show { partner, purse });
    }
}

/// The trade reset in the cascade's place (`0x4909b5` → `0x4bf0e0`), which signals nothing,
/// unless a later trade packet ran it first: the engine's tables, then the VM's tables and partner
/// cell.
pub(crate) fn reset_on_world_enter(
    mut vm: crate::script_calls::VmWith<Option<ResMut<'static, TradeSession>>>,
) {
    if let Some(mut trade) = vm.own() {
        trade.settle_world_reset();
    }
    vm.run(|script| {
        if let Some(script) = script {
            script.reset_trade_tables();
        }
    });
}

/// The close core, `0x4bf4e0` with a zero guid: with a window open, the latch release
/// (`0x4bf50a`), the cell cleared and `TRADE_CLOSED` (`0x4bf522`); then `CMSG_CANCEL_TRADE`
/// (`0x4bf527` → `0x5d4310`), open or not. Entered while the partner holds the latch, the
/// release's `0x493310` runs this once more inside it (`0x493365`), so a direct close fires
/// `TRADE_CLOSED` twice and sends the cancel twice. The session closes and the cancel goes out
/// here; the VM's half waits behind what the lane still holds ([`lane::close_step`]).
pub(crate) fn close_core(world: &mut World) {
    let waits = lane::close_waits(world);
    close_core_done(world, waits, false);
}

/// [`close_core`] with its `done` argument, which only `TRADE_COMPLETE` passes as 1 (`0x4bf862`).
/// With a partner it is stored first (`[0xb71748]`, `0x4bf4fd`), so the release's inner run
/// stores 0 over it when the partner holds the latch.
fn close_core_done(world: &mut World, waits: bool, done: bool) {
    drain_trade_touches(world);
    if let Some(partner) = world.resource::<TradeSession>().cell() {
        lane::close_step(world, waits, TradeEvent::Done(done));
        crate::ui_session::release(world, partner);
        world.resource_mut::<TradeSession>().close_window();
        lane::close_step(world, waits, TradeEvent::Closed);
    }
    lane::close_step(world, waits, TradeEvent::Cancel);
}

/// The status dispatcher's three closing arms (`0x4bf720`): `CANCELED`'s handler signals
/// `PLAYER_MONEY` over the zeroed offers (`0x5d48a8`) and its arm `TRADE_REQUEST_CANCEL`
/// (`0x4bf832`), and each runs `UnlockTradeItems` (`0x4bf837`, `0x4bf859`, `0x4bf87e`); then
/// `CLOSE_WINDOW`'s line (`0x4bf88e`), the close core (`0x4bf842`, `0x4bf867`, `0x4bf89c`), which
/// `COMPLETE` enters with `done` 1 (`0x4bf862`), and the other two's line (`0x4bf847`,
/// `0x4bf871`). All of it waits behind what the lane still holds, the open it closes among them.
pub(crate) fn server_close(world: &mut World, close: net::Close) {
    let waits = lane::close_waits(world);
    drain_trade_touches(world);
    lane::close_step(
        world,
        waits,
        TradeEvent::Unlock {
            canceled: close.canceled,
        },
    );
    if let Some(line) = close.first {
        lane::close_step(world, waits, TradeEvent::Line(line));
    }
    close_core_done(world, waits, close.done);
    if let Some(line) = close.after {
        lane::close_step(world, waits, TradeEvent::Line(line));
    }
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
pub(crate) fn on_cvar(
    ev: &crate::cvars::CvarChanged,
    mut block: bevy::prelude::Mut<'_, BlockTrades>,
) {
    if ev.is("BlockTrades") {
        block.0 = ev.flag();
    }
}

impl Plugin for UiTradePlugin {
    fn build(&self, app: &mut App) {
        lane::add_trade_lane(app);
        net::register(app);
        app.init_resource::<TradeSession>()
            .init_resource::<BlockTrades>()
            .add_latched_window::<TradeSession>()
            .add_sweep_leg(SweepLeg::Trade, cancel_trade_on_leave_world);
    }
}

/// 1.12's `BlockTrades` CVar (`0x842fbc`), the Block Trades checkbox (`UIOptionsFrame.lua:11`).
/// A resource, not a [`TradeSession`] field, because a disconnect clears the session; read only
/// by [`answer_request`]'s leg 8.
#[derive(Resource, Default)]
pub(crate) struct BlockTrades(pub(crate) bool);

/// `0x468460(guid, TYPEMASK_PLAYER)`: the guid's live object, `None` when it is not a streamed
/// player. Asked by the ladder's leg 3 (`0x4bf779`) and by the `%s` lines' guard (`0x4bf8ff`,
/// `0x4bf94b`).
fn streamed_player<'a>(objects: &'a Objects, guid: u64) -> Option<&'a ObjectStore> {
    benilla_protocol::guid::is_player(guid)
        .then(|| objects.store(guid))
        .flatten()
}

/// What the trade status reads beside the session: the socket, the name cache, the objects, and
/// the ladder's tests; each absent in a harness without them reads as the test passing.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct StatusReads<'w, 's> {
    commands: Res<'w, NetCommands>,
    names: Res<'w, NameCache>,
    objects: Objects<'w, 's>,
    social: Option<Res<'w, crate::ui_social::SocialState>>,
    cinematic: Option<Res<'w, crate::cinematic::Cinematic>>,
    own_control: Option<Res<'w, crate::player::OwnControlLost>>,
    auction: Option<Res<'w, crate::ui_auction::AuctionOpen>>,
    block_trades: Option<Res<'w, BlockTrades>>,
}

impl StatusReads<'_, '_> {
    /// `GetUnitName` (`0x609210`) on the streamed player `guid`, as a line's `%s`: the cache's,
    /// asked with a callback on a miss, else `UNKNOWNOBJECT`; `None` when `guid` is no streamed
    /// player (`0x468460` with the player mask).
    fn unit_name(&self, guid: u64) -> Option<FillArg> {
        let store = streamed_player(&self.objects, guid)?;
        Some(
            match self
                .names
                .resolve_unit(guid, Some(store), Ask::Event(&self.commands))
            {
                Some(name) => FillArg::S(name.to_string()),
                None => FillArg::UnknownObject,
            },
        )
    }

    /// Our coinage, which the open leg's coins are tested against (`0x5d4570`).
    fn purse(&self) -> u64 {
        self.objects
            .player_fields()
            .and_then(|p| p.player_money())
            .map_or(0, u64::from)
    }
}

/// Answer an incoming trade request inside its status with the reference's ladder (`0x4bf736`,
/// case 1 of `0x4bf720`, over `[0x4bf736, 0x4bf7f8)`), whose order is behaviour. There is no
/// consent step: 1.12 registers `TRADE_REQUEST` (event `0x11d`, slot `0xbe160c`) and never signals
/// it, so the stock `TRADE` popup never shows and a request that passes opens the window unasked.
pub(crate) fn answer_request(
    initiator: u64,
    trade: &mut TradeSession,
    lane: &mut TradeLane,
    reads: &StatusReads,
) {
    let commands = &reads.commands;
    // Leg 1: our own initiate is live (`[0xc4bec8]`, `0x4bf736`), which sends nothing.
    if trade.initiated {
        info!(target: "trade", "BEGIN_TRADE from {initiator:#x} dropped: our own initiate is in flight");
        return;
    }
    // Leg 2, the ignore list (`0x5ae5a0` at `0x4bf750`): `CMSG_IGNORE_TRADE` (`0x4bf759` →
    // `0x5d41c0`), read by the initiator as "is ignoring you"; vmangos has no such check
    // (`TradeHandler.cpp:567`).
    if reads
        .social
        .as_deref()
        .is_some_and(|s| s.is_ignored(initiator))
    {
        info!(target: "trade", "BEGIN_TRADE from an ignored {initiator:#x}; sending CMSG_IGNORE_TRADE");
        lane.answer(commands, lane::Answer::Ignore);
        return;
    }
    // Leg 3: the initiator is no streamed player (`0x468460` with mask 0x10, `0x4bf779`), which
    // sends nothing.
    let Some(store) = streamed_player(&reads.objects, initiator) else {
        info!(target: "trade", "BEGIN_TRADE from {initiator:#x} dropped: no streamed player");
        return;
    };
    // Legs 4-7, a silent busy: a dead or ghost initiator (`0x605f30`; `unit_reads_dead`, like
    // `UnitIsDead` `0x517ac0`, would refuse a feigning hunter), a cinematic (`[0xb4e310]`), lost
    // player control (`[0xb4b3e4]` clear, which `0x4958e0` writes, not an in-world flag) or an
    // open auction house (`[0xb725f8]`, its auctioneer, not a pending trade).
    let busy = store.0.is_dead_or_ghost()
        || reads.cinematic.as_deref().is_some_and(|c| c.is_playing())
        || reads.own_control.as_deref().is_some_and(|c| c.0)
        || reads
            .auction
            .as_deref()
            .is_some_and(|a| a.auctioneer.is_some());
    if busy {
        info!(target: "trade", "BEGIN_TRADE from {initiator:#x} refused silently; sending CMSG_BUSY_TRADE");
        lane.answer(commands, lane::Answer::Busy);
        return;
    }
    // Leg 8, Block Trades (`0x4bf7bc`-`0x4bf7cb`), the one leg that names the initiator: busy,
    // then `0x496720(0xbb, name)` with `GetUnitName` (`0x4bf7d6`); catalog row `0xbb` makes it a
    // chat line.
    if reads.block_trades.as_deref().is_some_and(|b| b.0) {
        info!(target: "trade", "BEGIN_TRADE from {initiator:#x} refused by BlockTrades; sending CMSG_BUSY_TRADE");
        lane.answer(commands, lane::Answer::Busy);
        let name = reads.unit_name(initiator).unwrap_or(FillArg::UnknownObject);
        lane.push(TradeEvent::Line(UiError::args(
            "ERR_TRADE_BLOCKED_S",
            vec![name],
        )));
        return;
    }
    // Nothing refuses it: accept, the bottom of the ladder (`0x4bf7ee` → `0x5d40e0`).
    info!(target: "trade", "BEGIN_TRADE from {initiator:#x} accepted; sending CMSG_BEGIN_TRADE");
    trade.begin(initiator);
    lane.answer(commands, lane::Answer::Begin);
}

/// The initiates' name callbacks a player-name answer runs, a record or a refusal, after the cache
/// files it (`0x55f426`): each `0x5d4080` waiting on the name looks it up again with no callback
/// and shows `ERR_INITIATE_TRADE_S` for a found one (`0x5d40a0`-`0x5d40ca`), on the lane at the
/// answer's packet; a refusal shows nothing.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TradeNameAnswers<'w> {
    /// Each absent in a harness without the trade window.
    trade: Option<ResMut<'w, TradeSession>>,
    lane: Option<ResMut<'w, TradeLane>>,
}

impl TradeNameAnswers<'_> {
    /// `guid`'s answer just filed in `names`.
    pub(crate) fn answered(&mut self, guid: u64, names: &NameCache) {
        let (Some(trade), Some(lane)) = (self.trade.as_deref_mut(), self.lane.as_deref_mut())
        else {
            return;
        };
        let waits = trade.take_initiate_waits(guid);
        if let Some(name) = names.resolve_player(guid, Ask::Read) {
            for _ in 0..waits {
                lane.push(TradeEvent::Line(UiError::s("ERR_INITIATE_TRADE_S", name)));
            }
        }
    }
}

/// A wire [`TradeItem`] as the Lua row: name and quality from the item-template cache once it
/// answers, the icon from `display_id` via `ItemDisplayInfo.dbc`; the getters' read ([`live`])
/// names slot 7's `enchantment`. The template is only read: the getters' lookups ask for it
/// ([`ask_trade_record`]). The name joins the random property's suffix (`0x5d8b00`, at
/// `0x4c01b8` and `0x4c0405` for the partner's slots) and the link prints the record's permanent
/// enchant, random property id and suffix factor (`0x4c040a`-`0x4c0431`); our own slots' records
/// carry what `0x52ae00` reads off the object ([`own_item`]).
fn resolve_slot(
    item: &TradeItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    names: &NameCache,
    rolls: crate::items::RollCatalogs,
) -> TradeSlotItem {
    // What the getters read of it as they run, read here the same way.
    let roll = benilla_ui::script::ItemRoll {
        enchant: item.perm_enchant,
        property: item.random_prop_id,
        suffix_factor: item.suffix_factor,
    };
    let live = crate::ui_items::live::LiveItem::record(item.entry, roll, items, icons, rolls);
    let name = live.name();
    let quality = live.quality();
    let inventory_type = live.inventory_type();
    // A cached template whose display has no icon falls to the question mark, and an unanswered
    // one leaves it to the getter.
    let texture = live.cached().then(|| {
        live.texture()
            .unwrap_or_else(|| crate::ui_action::MISSING_ITEM_ICON.to_string())
    });
    // Nil until the template answers: the link embeds the name and the quality colour.
    let link = live.link();
    // Only a wrapped gift names its giver (`0x52e288`, `0x52e29d`). Only read: the slot tooltip's
    // build asks it in its call ([`benilla_ui::script::RecordAsk::ItemBuildName`]).
    let gift_creator = (item.wrapped && item.gift_creator != 0)
        .then(|| names.resolve_player(item.gift_creator, Ask::Read))
        .flatten()
        .map(str::to_string);
    // The maker, the partner's block `+0x410` (`0x53453e`) or our object's `ITEM_FIELD_CREATOR`
    // (`0x52e247`); only read, as the giver is.
    let creator = (item.creator != 0)
        .then(|| names.resolve_player(item.creator, Ask::Read))
        .flatten()
        .map(str::to_string);
    TradeSlotItem {
        uncached_link: None,
        item_id: item.entry,
        name,
        texture,
        count: item.count.max(1),
        quality,
        inventory_type,
        lock_id: item.lock_id,
        wrapped: item.wrapped,
        gift_creator,
        gift_creator_guid: if item.wrapped { item.gift_creator } else { 0 },
        creator,
        creator_guid: item.creator,
        enchantment: None,
        // The partner's block (`0x534595`, `0x5345c0`, `0x5345b4`).
        charges: item.charges,
        durability: (item.durability, item.max_durability),
        link,
        roll,
    }
}

/// [`resolve_slot`] for a slot of our own, which `GetTradePlayerItemLink` links off the item
/// object (`0x4c06bc` → `0x52ae00`) with its record cached or not: white (`0x52aec5`) and nameless
/// (`0x5d8b40`) without one.
fn resolve_own_slot(
    item: &TradeItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    names: &NameCache,
    rolls: crate::items::RollCatalogs,
) -> TradeSlotItem {
    let row = resolve_slot(item, items, icons, names, rolls);
    let uncached = items.template(item.entry, 0, Ask::Read).is_none();
    TradeSlotItem {
        uncached_link: uncached.then(|| {
            crate::ui_items::item_link_full(
                item.entry,
                item.perm_enchant,
                item.random_prop_id,
                item.suffix_factor,
                "",
                1,
            )
        }),
        ..row
    }
}

/// Our item object `guid` as a wire [`TradeItem`] of `count`.
fn own_item(guid: u64, count: u32, objects: &Objects, items: &Items) -> Option<TradeItem> {
    let fields = objects.object(guid)?;
    let entry = fields.object_entry()?;
    let flags = fields.item_flags().unwrap_or(0);
    // The placed item's WRAPPED flag and giver, which its slot's tooltip reads off the object
    // (`0x52b789`, `0x52e32a`).
    let wrapped = flags & benilla_protocol::messages::ITEM_DYNFLAG_WRAPPED != 0;
    // Read, as the placing click reads it (`0x4bfede`).
    let template = items.template(entry, guid, Ask::Read);
    let display_id = template.map(|t| t.display_info_id).unwrap_or(0);
    // What `GetTradePlayerItemLink` links (`0x4c06bc` → `0x52ae00`) and `GetTradePlayerItemInfo`
    // names (`0x4c04f5` → `0x5d8bc0`): the object's enchant, roll and seed, none when wrapped.
    let (perm_enchant, random_prop_id, suffix_factor) =
        crate::ui_items::object_link_numbers(fields, template);
    Some(TradeItem {
        entry,
        display_id,
        count,
        wrapped,
        gift_creator: fields.item_gift_creator().unwrap_or(0),
        perm_enchant,
        // The tooltip reads our object's maker (`0x52e247`).
        creator: fields.item_creator().unwrap_or(0),
        charges: 0,
        suffix_factor,
        random_prop_id,
        lock_id: 0,
        max_durability: 0,
        durability: 0,
    })
}

/// The enchant slot, the seventh: the only row with an enchantment.
const ENCHANT_SLOT: usize = TRADE_SLOT_COUNT - 1;

/// Run the trade half of every item watcher run still on the item lane, ahead of a close
/// ([`crate::item_lane::ItemLane::take_trade_touches`]): vmangos sends the traded items' destroys
/// before `TRADE_COMPLETE` (`TradeHandler.cpp:469`, `Player.cpp:10710`, then `:526`), the destroy
/// releases at once (`0x4674cf` → `0x464920`), so the clears and `TRADE_PLAYER_ITEM_CHANGED`
/// belong ahead of the close, not at the lanes' drain after it.
fn drain_trade_touches(world: &mut World) {
    let touches = world
        .get_resource_mut::<crate::item_lane::ItemLane>()
        .map(|mut lane| lane.take_trade_touches())
        .unwrap_or_default();
    if touches.is_empty() {
        return;
    }
    let _ = world.run_system_cached_with(scan_trade_touches, touches);
    crate::creature_anim::apply_unit_edits(world);
}

/// [`touch_trade`] on each of `touches`, in order, as one engine turn.
fn scan_trade_touches(
    In(touches): In<Vec<crate::item_watch::TradeTouch>>,
    mut vm: crate::script_calls::VmWith<ResMut<'static, TradeSession>>,
    reads: TouchReads,
) {
    vm.turn(|script, mut trade| {
        let Some(script) = script else {
            return;
        };
        for touch in touches {
            touch_trade(script, &mut trade, touch, &reads);
        }
    })
}

/// What the trade scan reads beside the session: the socket its sends go out on, the objects,
/// the item records and what a placed row's view reads.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TouchReads<'w, 's> {
    pub(crate) commands: Res<'w, NetCommands>,
    pub(crate) objects: Objects<'w, 's>,
    pub(crate) items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    names: Res<'w, NameCache>,
    rolls: crate::items::Rolls<'w>,
}

/// An item watcher's trade half, after the action bar's recount in its handler: an item of ours
/// that one of our slots holds re-signals the slot (`0x4bfb10`, `0x4bfb60` into `0x4bfa70`). Gone,
/// the slot clears with `CMSG_CLEAR_TRADE_ITEM` (`0x5d4480`), as a click's clear does; restacked
/// or enchanted, the same item is placed again with `CMSG_SET_TRADE_ITEM` from the position it was
/// placed from (`0x5d4380`), our session's offer left as it is. vmangos refuses an item its trade
/// already holds and cancels the trade (`TradeHandler.cpp:716-720`), as it does for the
/// reference's send. Neither scan asks whether a trade is open.
pub(crate) fn touch_trade(
    script: &mut UiScript,
    trade: &mut TradeSession,
    touch: crate::item_watch::TradeTouch,
    reads: &TouchReads,
) {
    use crate::item_watch::TradeTouch;
    let TouchReads {
        commands,
        objects,
        items,
        icons,
        names,
        rolls,
    } = reads;
    match touch {
        TradeTouch::Gone(guid) => {
            let Some((slot, _, _)) = script.trade_slot_holding(guid) else {
                return;
            };
            info!(target: "trade", "item {guid:#x} gone from slot {}; sending \
                                    CMSG_CLEAR_TRADE_ITEM", slot + 1);
            trade.send_edit(commands, ClientCommand::ClearTradeItem { trade_slot: slot });
            trade.clear_own_item(u32::from(slot) + 1);
            script.empty_trade_slot(slot);
        }
        TradeTouch::Changed(guid) => {
            let Some((slot, bag, bag_slot)) = script.trade_slot_holding(guid) else {
                return;
            };
            // The send refuses with no active player or no item object (`0x5d43af`,
            // `0x5d43da`), and then nothing is stored or signalled (`0x4bfab3`).
            let count = objects
                .object(guid)
                .and_then(|f| f.item_stack_count())
                .unwrap_or(1);
            let (Some(_), Some(item), Some((wire_bag, wire_slot))) = (
                objects.player(),
                own_item(guid, count, objects, items),
                crate::ui_items::wire_pos(bag, bag_slot),
            ) else {
                return;
            };
            info!(target: "trade", "item {guid:#x} in slot {} changed; sending \
                                    CMSG_SET_TRADE_ITEM", slot + 1);
            trade.send_edit(
                commands,
                ClientCommand::SetTradeItem {
                    trade_slot: slot,
                    bag: wire_bag,
                    slot: wire_slot,
                },
            );
            // Our column takes the item as placed, as a click's placement does.
            trade.place_own_item(u32::from(slot) + 1, item);
            let row = resolve_own_slot(&item, items, icons.as_deref(), names, rolls.get());
            script.replace_trade_slot(slot, row);
        }
    }
}

/// The callback behind `TRADE_UPDATE`, whose one producer is `0x4c0340` (`0x4c034f`), which an
/// item getter registers when it finds a filled slot whose template the cache lacks (`0x4c017f`,
/// `0x4c0582` into `0x55ba30`), inside its call ([`benilla_ui::script::RecordAsk::TradeItem`]).
/// A hit never calls it. Each such getter call registers one callback, with no dedupe
/// (`0x55bbb3`), and each runs when the record lands, for a ready record only; a refusal frees
/// them silent. The landed records signal on the item lane, at the answer's packet
/// ([`crate::item_watch::ItemNotice::TradeRecord`]).
pub(crate) fn ask_trade_record(items: &Items, commands: &NetCommands, entry: u32, owner: u64) {
    // The getter's lookup with the callback: a miss asks, a refused entry again, the slot's side
    // as the owner id.
    if items.template(entry, owner, Ask::Event(commands)).is_none() {
        items.wait_for_trade_record(entry);
    }
}

/// The trade half of `OnMoneyChanged` (`0x5ddf30`), the active player's coinage watcher, run inside
/// the packet that moved the purse: an own offer above the purse (`TradeGetPlayerMoney 0x5d4760`,
/// `0x5ddfab`) loses the difference (`TradeSubtractMoney 0x5d45f0`, `0x5ddfda`), which sends the new
/// total, the purse. A closed trade's offer is 0 (`0x5d4884`-`0x5d48a2`), so no open test is needed.
pub(crate) fn trim_offer_to_purse(trade: &mut TradeSession, purse: u32, commands: &NetCommands) {
    if trade.our.gold > purse {
        trade.set_own_gold(purse);
        trade.send_edit(commands, ClientCommand::SetTradeGold { copper: purse });
    }
}

#[cfg(test)]
mod tests;
