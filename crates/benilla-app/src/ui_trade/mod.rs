//! The trade window: [`net`] folds the two status packets into [`TradeSession`], [`feed_trade`]
//! pushes it into the VM and fires the trade events, the trade verbs act inside their calls
//! ([`TradeSteps`]), [`drain_trade`] runs `CloseTrade`'s close core,
//! [`run_record_callbacks`] signals `TRADE_UPDATE` when a record a getter asked for lands, and
//! [`answer_trade_request`] answers an incoming request.

use benilla_protocol::messages::{TradeItem, TradeStatusExtended, TRADE_SLOT_COUNT};
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, TradeSideState, TradeSlotItem, TradeState, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{CoreCloses, LatchApp, LatchedWindow, NpcWindow};

mod calls;
mod net;

#[cfg(test)]
mod call_tests;

pub(crate) use calls::TradeSteps;

/// One side's offer: slots 0-5 change hands, slot 6 is the non-traded enchant slot.
#[derive(Default)]
struct TradeOffer {
    slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
    gold: u32,
    /// The header's spell, `[0xc4bed8 + 4*side]` (`0x5d49fa`): the spell this side's player cast
    /// on the other side's enchant slot, 0 for none. Our own window's names the partner's slot 7,
    /// the partner's names ours (`0x4c02b7`, `0x4c05d1`).
    enchant_spell_id: u32,
}

/// One step a status's accept drop left for [`feed_trade`] ([`TradeSession::back_to_trade`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcceptStep {
    /// `TRADE_ACCEPT_UPDATE(mine, theirs)`.
    Signal(bool, bool),
    /// `CMSG_UNACCEPT_TRADE`.
    SendUnaccept,
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
    /// What a status's `0x4bf230(0)` and `0x4bf280` did, in order, for [`feed_trade`] to replay:
    /// each moved flag's signal, and our un-accept's send after its signal (`0x4bf260`, `0x4bf272`).
    accept_steps: Vec<AcceptStep>,
    /// Our 0-based slots `InvalidateTradeSlot` (`0x4bfbd0`) runs on, owed to [`feed_trade`].
    invalidated: Vec<u8>,
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
    /// The money signals owed to [`feed_trade`], in order: `false` ours, `PLAYER_TRADE_MONEY` then
    /// `PLAYER_MONEY` (`0x4bf4ab`, `0x4bfbea`), `true` the partner's, `TRADE_MONEY_CHANGED`
    /// (`0x4bf4d6`).
    money_changed: Vec<bool>,
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
    /// How many of `named_lines`, from the front, a feed has read: the rest were raised since,
    /// each by a handler that looked the name up with a callback.
    named_lines_asked: usize,
    /// The pending request's ladder has looked the initiator's name up (leg 8, `0x609210`): the
    /// frames that wait on it read.
    request_asked: bool,
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
    /// Our `CMSG_INITIATE_TRADE` goes out (`0x5d3fb0`): a fresh session with the target as partner,
    /// the pending flag set and the starter's mode beside it (`0x5d4021`, `0x5d402b`).
    pub(crate) fn initiate(&mut self, target: u64, cursor_drop: bool) {
        *self = TradeSession {
            partner: Some(target),
            initiated: true,
            cursor_drop,
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
        self.request_asked = false;
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
    /// The open zeroes both accept flags with no signal (`0x4bf5c0`, `0x4bf5c6`).
    pub(crate) fn open_window(&mut self) {
        self.open = true;
        self.own_edits = 1;
        self.partner_flag = 1;
        self.money_seen = [0, 0];
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
        // `0x4bf2b0` first drops both accepts (`0x4bf2b6`, `0x4bf2bd`).
        self.back_to_trade();
        // Our side's slot that held an item and comes back empty is invalidated (`0x5d4ad3`,
        // `0x5d4b33`).
        if !ext.their_window {
            for (i, (old, new)) in self.our.slots.iter().zip(ext.slots.iter()).enumerate() {
                if old.is_some() && new.is_none() {
                    self.invalidated.push(i as u8);
                }
            }
        }
        if ext.their_window {
            self.partner_flag = ext.flag;
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
        // Then both money offers, whichever side the status was for: ours (`0x4bf48f`), then the
        // partner's (`0x4bf4ba`).
        for (partner, gold) in [(false, self.our.gold), (true, self.their.gold)] {
            let seen = &mut self.money_seen[usize::from(partner)];
            if *seen != gold {
                *seen = gold;
                self.money_changed.push(partner);
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
    fn unaccept_ours(&mut self) {
        if self.our_accept {
            self.our_accept = false;
            self.accept_steps
                .push(AcceptStep::Signal(false, self.their_accept));
            self.accept_steps.push(AcceptStep::SendUnaccept);
        }
    }

    /// `0x4bf280(flag)`: the partner's flag, which signals both flags when it moved (`0x4bf286`,
    /// `0x4bf29f`) and sends nothing.
    fn set_partner_accept(&mut self, flag: bool) {
        if self.their_accept != flag {
            self.their_accept = flag;
            self.accept_steps
                .push(AcceptStep::Signal(self.our_accept, flag));
        }
    }

    /// `TRADE_ACCEPT` (case 4, `0x4bf810`): `0x4bf280(1)`.
    pub(crate) fn partner_accepted(&mut self) {
        self.set_partner_accept(true);
    }

    /// `BACK_TO_TRADE` (case 7, `0x4bf81c`, `0x4bf823`), and every extended status before its
    /// records are compared (`0x4bf2b6`, `0x4bf2bd`): `0x4bf230(0)`, then `0x4bf280(0)`. vmangos
    /// sends it for any offer change, so our own edits drop the flags through it.
    pub(crate) fn back_to_trade(&mut self) {
        self.unaccept_ours();
        self.set_partner_accept(false);
    }

    /// The steps the statuses left, in order ([`Self::accept_steps`]).
    pub(crate) fn take_accept_steps(&mut self) -> Vec<AcceptStep> {
        std::mem::take(&mut self.accept_steps)
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
    /// sends. The signal it makes is announced here, so the feed does not repeat it.
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
            named_lines_asked: self.named_lines_asked,
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

    /// `TRADE_REJECTED` (case 9, `0x4bf821`): `0x4bf280(0)`; vmangos never sends it.
    pub(crate) fn partner_unaccepted(&mut self) {
        self.set_partner_accept(false);
    }

    /// `ONLY_CONJURED` (case 22, `0x4bf9ec` → `0x4bfbd0`): empty the offending slot of our offer,
    /// or the gold for `0xff`; the feed's diff raises the repaint events. Deviation: an
    /// out-of-range slot is dropped, where the reference indexes its 7-slot mirror unchecked,
    /// because an out-of-bounds write is not a behaviour to reproduce.
    pub(crate) fn bounce_own_offer(&mut self, slot: u8) {
        const MONEY: u8 = 0xff;
        if slot == MONEY {
            // `0x5d4864` zeroes the offer; `0x4bfbd0(0xff)` zeroes the cache and signals.
            self.our.gold = 0;
            self.money_seen[0] = 0;
            self.money_changed.push(false);
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

    /// The open leg's coins: our offer `copper`, which the money cache takes (`0x4bf62b`).
    pub(crate) fn fold_held_coins(&mut self, copper: u32) {
        self.set_own_gold(copper);
        self.money_seen[0] = copper;
    }

    /// The money signals owed, in order ([`Self::money_changed`]).
    pub(crate) fn take_money_changed(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.money_changed)
    }

    /// What `CMSG_ACCEPT_TRADE` carries ([`Self::partner_flag`]).
    pub(crate) fn partner_flag(&self) -> u32 {
        self.partner_flag
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
    close_core_done(world, false);
}

/// [`close_core`] with its `done` argument, which only `TRADE_COMPLETE` passes as 1 (`0x4bf862`).
/// With a partner it is stored first (`[0xb71748]`, `0x4bf4fd`), so the release's inner run
/// stores 0 over it when the partner holds the latch.
fn close_core_done(world: &mut World, done: bool) {
    drain_trade_touches(world);
    if let Some(partner) = world.resource::<TradeSession>().cell() {
        crate::ui_session::with_script(world, |script| script.set_trade_done(done));
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
/// `0x4bf87e`) before the close core (`0x4bf842`, `0x4bf867`, `0x4bf89c`), which `COMPLETE` enters
/// with `done` 1 (`0x4bf862`).
pub(crate) fn server_close(world: &mut World, canceled: bool, done: bool) {
    drain_trade_touches(world);
    crate::ui_session::fire(world, |script| {
        // The statuses zero both offers before any of it (`0x5d4884`-`0x5d48a2`), and
        // `CANCELED`'s handler arm signals `PLAYER_MONEY` there (`0x5d48a8`), ahead of the
        // dispatcher's `TRADE_REQUEST_CANCEL`.
        script.zero_trade_gold();
        if canceled {
            script.fire_event("PLAYER_MONEY", vec![]);
            script.fire_event("TRADE_REQUEST_CANCEL", vec![]);
        }
        script.unlock_trade_items();
    });
    close_core_done(world, done);
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
                    drain_trade.in_set(crate::ui_script::UiDrain),
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
    errors: Res<crate::ui_action::UiErrorKeys>,
    social: Res<crate::ui_social::SocialState>,
    cinematic: Res<crate::cinematic::Cinematic>,
    own_control: Res<crate::player::OwnControlLost>,
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
    if dead_or_ghost || cinematic.is_playing() || own_control.0 || auction.auctioneer.is_some() {
        info!(target: "trade", "BEGIN_TRADE from {initiator:#x} refused silently; sending CMSG_BUSY_TRADE");
        let _ = commands.0.send(ClientCommand::BusyTrade);
        trade.refuse_request();
        return;
    }

    // Leg 8, Block Trades, the one leg that names the initiator. The reference reads the live
    // `CGUnit` (`0x609210`); this may wait on a name query, with no timer, since vmangos cancels
    // the trade to us when the initiator leaves (`Player.cpp:11747`), which clears the request.
    if block_trades.0 {
        let ask = Ask::at(
            !std::mem::replace(&mut trade.request_asked, true),
            &commands,
        );
        let Some(name) = names.resolve_player(initiator, ask).map(str::to_string) else {
            return;
        };
        info!(target: "trade", "BEGIN_TRADE from {name} refused by BlockTrades; sending CMSG_BUSY_TRADE");
        let _ = commands.0.send(ClientCommand::BusyTrade);
        trade.refuse_request();
        // `0x496720(0xbb, name)`: one `DisplayError`; catalog row `0xbb` makes it a chat line.
        errors.push(crate::ui_action::UiError::s("ERR_TRADE_BLOCKED_S", name));
        return;
    }

    // Nothing refuses it: accept, the bottom of the ladder.
    info!(target: "trade", "BEGIN_TRADE from {initiator:#x} accepted; sending CMSG_BEGIN_TRADE");
    trade.begin(initiator);
    let _ = commands.0.send(ClientCommand::BeginTrade);
}

/// A wire [`TradeItem`] as the Lua row: name and quality from the item-template cache once it
/// answers, the icon from `display_id` via `ItemDisplayInfo.dbc`; [`resolve_side`] names slot 7's
/// `enchantment`. The template is only read: the getters' lookups ask for it
/// ([`run_record_callbacks`]). The name joins the random property's suffix (`0x5d8b00`, at
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
    let template = items.template(item.entry, 0, Ask::Read);
    let name = template.map(|t| rolls.name(&t.name, item.random_prop_id));
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
        (Some(n), Some(q)) => Some(crate::ui_items::item_link_full(
            item.entry,
            item.perm_enchant,
            item.random_prop_id,
            item.suffix_factor,
            n,
            q,
        )),
        _ => None,
    };
    // Only a wrapped gift names its giver (`0x52e288`, `0x52e29d`). Only read: the slot tooltip's
    // build asks it (`crate::ui_items::feed::ask_item_build_lookups`).
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
    let (perm_enchant, random_prop_id, suffix_factor) = template
        .map(|t| crate::ui_items::object_link_numbers(fields, t))
        .unwrap_or_default();
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

/// `enchant` is the name the column's slot 7 shows: the `Spell.dbc` name of the spell cast on it,
/// read for a filled slot only (`0x4c02b0`, `0x4c05c6`).
fn resolve_side(
    offer: &TradeOffer,
    enchant: Option<&str>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    names: &NameCache,
    rolls: crate::items::RollCatalogs,
) -> TradeSideState {
    let mut slots: [Option<TradeSlotItem>; TRADE_SLOT_COUNT] = Default::default();
    for (i, s) in offer.slots.iter().enumerate() {
        slots[i] = s.map(|it| {
            let mut row = resolve_slot(&it, items, icons, names, rolls);
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
    rolls: crate::items::RollCatalogs,
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
            name(trade.their.enchant_spell_id),
            items,
            icons,
            names,
            rolls,
        ),
        // Our window carries the spell we cast on theirs (`0x4c02b7`, side 0).
        target: resolve_side(
            &trade.their,
            name(trade.our.enchant_spell_id),
            items,
            icons,
            names,
            rolls,
        ),
    })
}

/// What [`snapshot`] resolves the rows through: the item templates, their icons, the spell names
/// slot 7 shows and the roll catalogs.
#[derive(bevy::ecs::system::SystemParam)]
struct TradeRows<'w> {
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    rolls: crate::items::Rolls<'w>,
}

impl TradeRows<'_> {
    /// [`snapshot`] over these.
    fn snapshot(&self, trade: &TradeSession, names: &NameCache) -> Option<TradeState> {
        snapshot(
            trade,
            &self.items,
            self.icons.as_deref(),
            self.spells.as_deref(),
            names,
            self.rolls.get(),
        )
    }
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
    let _ = crate::script_calls::run_cached(world, resignal_trade_items);
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
#[allow(clippy::too_many_arguments)] // the scan's reads
fn resignal_trade_items(
    mut vm: crate::script_calls::VmWith<ResMut<'static, TradeSession>>,
    mut cursor: ResMut<TradeTouchCursor>,
    messages: Res<Messages<crate::item_watch::ItemNotice>>,
    commands: Res<NetCommands>,
    objects: Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    names: Res<NameCache>,
    rolls: crate::items::Rolls,
) {
    let rolls = rolls.get();
    vm.turn(|script, mut trade| {
        use crate::item_watch::TradeTouch;
        let touches: Vec<TradeTouch> = cursor
            .0
            .read(&messages)
            .filter_map(crate::item_watch::ItemNotice::trade)
            .collect();
        let Some(script) = script else {
            return;
        };
        for touch in touches {
            match touch {
                TradeTouch::Gone(guid) => {
                    let Some((slot, _, _)) = script.trade_slot_holding(guid) else {
                        continue;
                    };
                    info!(target: "trade", "item {guid:#x} gone from slot {}; sending \
                                            CMSG_CLEAR_TRADE_ITEM", slot + 1);
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
                    let (Some(_), Some(item), Some((wire_bag, wire_slot))) = (
                        objects.player(),
                        own_item(guid, count, &objects, &items),
                        crate::ui_items::wire_pos(bag, bag_slot),
                    ) else {
                        continue;
                    };
                    info!(target: "trade", "item {guid:#x} in slot {} changed; sending \
                                            CMSG_SET_TRADE_ITEM", slot + 1);
                    trade.send_edit(
                        &commands,
                        ClientCommand::SetTradeItem {
                            trade_slot: slot,
                            bag: wire_bag,
                            slot: wire_slot,
                        },
                    );
                    let row = resolve_slot(&item, &items, icons.as_deref(), &names, rolls);
                    script.replace_trade_slot(slot, row);
                }
            }
        }
    })
}

/// Push the trade into the VM and fire its events on a change, diffed against `Local` memory;
/// `TRADE_ACCEPT_UPDATE` fires after `TRADE_SHOW`, whose handler hides the highlights. An engine
/// turn ([`crate::script_calls::VmWith::turn`]): it holds the session the trade calls write, so
/// the handlers of what it fires run as it ends, in order, with the host lent.
#[allow(clippy::too_many_arguments)] // the feed's reads and its memos
fn feed_trade(
    mut vm: crate::script_calls::VmWith<ResMut<'static, TradeSession>>,
    rows: TradeRows,
    objects: Objects,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    errors: Res<crate::ui_action::UiErrorKeys>,
    // The `%s` lines' live-object guard, the same reads as the ladder's leg 3.
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    mut last: crate::ui_script::WindowMemo<Option<TradeState>>,
    mut last_open: crate::ui_script::WindowMemo<bool>,
    mut closes: CoreCloses,
) {
    // The statuses' accept drops first, as `0x4bf2b0` runs them ahead of its compares: each
    // signal in place, its handlers' calls applying inside it, and our un-accept's send after
    // its signal (`0x4bf260`, `0x4bf272`).
    // Our slots an extended status emptied go first, as the handler runs `InvalidateTradeSlot`
    // (`0x4bfbd0`: the unlock and its own `TRADE_PLAYER_ITEM_CHANGED`) as it stores the records
    // (`0x5d4ad3`, `0x5d4b33`), before `0x4bf2b0` (`0x5d4b67`); the diff below then skips them.
    let (invalidated, steps) = {
        let mut trade = vm.own();
        (trade.take_invalidated(), trade.take_accept_steps())
    };
    if !invalidated.is_empty() || !steps.is_empty() {
        vm.run(|mut script| {
            if let Some(script) = script.as_deref_mut() {
                for &slot in &invalidated {
                    script.invalidate_trade_slot(slot);
                }
            }
            for step in steps {
                match step {
                    AcceptStep::Signal(mine, theirs) => {
                        if let Some(script) = script.as_deref_mut() {
                            script.fire_event(
                                "TRADE_ACCEPT_UPDATE",
                                vec![
                                    ScriptValue::Int(i64::from(mine)),
                                    ScriptValue::Int(i64::from(theirs)),
                                ],
                            );
                        }
                    }
                    AcceptStep::SendUnaccept => {
                        let _ = commands.0.send(ClientCommand::UnacceptTrade);
                    }
                }
            }
        });
    }
    vm.turn(|script, mut trade| {
        let Some(script) = script else {
            return;
        };
        let last = last.get(script);
        let last_open = last_open.get(script);
        if closes.since_last(script, NpcWindow::Trade) {
            *last = None;
            *last_open = false;
        }

        let fresh = rows.snapshot(&trade, &names);
        // A close is the core's, which shut the VM's window and left both offers in place; a
        // disconnect's clear tells it nothing.
        let opened = !*last_open && trade.is_open();
        if opened {
            // The open clears both offers and our item table before anything is placed
            // (`0x4bf549`).
            script.open_trade_window();
            *last = None;
        }
        let changed = fresh != *last;
        // Our column as `ClickTradeButton` left it: its write signalled in place (`0x4bfaef`). A
        // slot whose source has not moved since the last push keeps the VM's value, so a click this
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
            if let (Some(p), Some(held), Some(_)) = (pushed.as_mut(), held_player.as_ref(), &*last)
            {
                for (i, slot) in p.player.slots.iter_mut().enumerate() {
                    if !source_moved(i) {
                        *slot = held[i].clone();
                    }
                }
            }
            script.set_trade(pushed);
        }
        // The per-slot events the stock `TradeFrame.lua` repaints one slot on, arg1 the 1-based
        // slot (`0x4bf414`/`0x4bf452` target, `0x4bf487`/`0x4bfaef` player). An offer change
        // signals nothing else: `TRADE_UPDATE` has one producer, the getters' record callback
        // ([`run_record_callbacks`]). Our invalidated slots signalled ahead of this turn.
        if changed && trade.is_open() && !opened {
            let empty = TradeState::default();
            let old = last.as_ref().unwrap_or(&empty);
            if let Some(new) = fresh.as_ref() {
                // The record compare (`0x4bf2c7`-`0x4bf378`) reads the wire's item fields, run only
                // on a status: never the enchant slot's spell, which signals on its own below, and
                // never what the template cache resolved (name, icon, quality, link), so a template
                // landing repaints through `TRADE_UPDATE` and signals no slot.
                let record =
                    |s: &Option<TradeSlotItem>| s.as_ref().map(|s| (s.item_id, s.count, s.lock_id));
                let changed_slots =
                    |mine: &[Option<TradeSlotItem>], theirs: &[Option<TradeSlotItem>]| {
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
                let was_invalidated = |n: i64| invalidated.iter().any(|&i| i64::from(i) + 1 == n);
                for slot in player_changed {
                    if matches!(slot, ScriptValue::Int(n) if was_invalidated(n)) {
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
        // Then the money, as a status's compare (`0x4bf48f`-`0x4bf4d6`) or a bounce of our coins
        // (`0x4bfbd0(0xff)`) left it. Our gold reaches the money input through
        // `PLAYER_TRADE_MONEY` alone (`TradeFrame.xml:565`), so a repaint never overwrites what
        // the player is typing; the partner's is the `TARGET_TRADE` money frame's
        // `TRADE_MONEY_CHANGED` (`MoneyFrame.lua:133`).
        for partner in trade.take_money_changed() {
            if partner {
                script.fire_event("TRADE_MONEY_CHANGED", vec![]);
            } else {
                script.fire_event("PLAYER_TRADE_MONEY", vec![]);
                script.fire_event("PLAYER_MONEY", vec![]);
            }
        }
        // The owed `%s` lines, resolved here where the name cache is; a miss waits for the query it
        // started. The catalog row, read by `ui_action::feed_actions`, decides where each lands.
        // Taken in the frame before the interface is up too, deliberately: it marks only the ask,
        // which goes to the server, not the VM.
        let asked = std::mem::take(&mut trade.named_lines_asked);
        for (i, line) in trade.take_named_lines().into_iter().enumerate() {
            // The status arms print nothing once the player is no longer an object.
            if line.needs_live_object && streamed_player(&index, &stores, line.who).is_none() {
                continue;
            }
            match names
                .resolve_player(line.who, Ask::at(i >= asked, &commands))
                .map(str::to_string)
            {
                Some(name) => errors.push(crate::ui_action::UiError::s(line.key, name)),
                // Not cached: the line's lookup has asked, so park it and read it again next frame.
                None => trade.owe_named_line(line),
            }
        }
        trade.named_lines_asked = trade.named_lines.len();
        if opened {
            // `SetTradePartner`'s open leg (`0x4bf5f1`): only a trade we started by dropping
            // something on the partner moves what the cursor holds into the offer (`0x5d3f80`).
            if trade.opens_with_held() {
                if let Some(offer) = script.trade_open_money() {
                    // Coins (`0x4bf5fe`-`0x4bf644`): `0x5d4500` sends the new offer, the cache of
                    // our money `[0xb71740]` takes it (`0x4bf62b`), and `PLAYER_TRADE_MONEY`,
                    // `PLAYER_MONEY` and `TRADE_SHOW` fire once each, then the leg returns.
                    trade.fold_held_coins(offer);
                    trade.send_edit(&commands, ClientCommand::SetTradeGold { copper: offer });
                    script.fire_event("PLAYER_TRADE_MONEY", vec![]);
                    script.fire_event("PLAYER_MONEY", vec![]);
                } else if let Some(held) = script.trade_open_item() {
                    // An item (`0x4bf652`-`0x4bf6fa`): its send's answer is not read (`0x4bf6be`).
                    let _ = trade.send_placement(
                        &commands,
                        &objects,
                        &rows.items,
                        held.slot,
                        held.guid,
                        held.bag,
                        held.bag_slot,
                    );
                }
            }
            script.fire_event("TRADE_SHOW", vec![]);
        }

        *last = fresh;
        *last_open = trade.is_open();
    })
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
    commands: &NetCommands,
    notices: &mut MessageReader<crate::item_watch::ItemNotice>,
) {
    for entry in script.take_trade_record_asks() {
        // The getter's lookup with the callback: a miss asks, a refused entry again.
        if items.template(entry, 0, Ask::Event(commands)).is_none() {
            items.wait_for_trade_record(entry);
        }
    }
    for notice in notices.read() {
        if matches!(notice, crate::item_watch::ItemNotice::TradeRecord { .. }) {
            script.fire_event("TRADE_UPDATE", vec![]);
        }
    }
}

/// After the tick: `CloseTrade`'s close core, which the interaction latch runs (`0x4bfdd9` →
/// `0x4bf4e0(0, 0)`), and the item getters' record callbacks ([`run_record_callbacks`]). Every
/// other trade verb acts inside its call ([`TradeSteps`]).
fn drain_trade(
    mut hosted: crate::script_calls::HostedVm,
    mut notices: MessageReader<crate::item_watch::ItemNotice>,
    mut world_commands: Commands,
    commands: Res<NetCommands>,
    items: Res<Items>,
) {
    hosted.run(|script| {
        let Some(script) = script else {
            return;
        };
        run_record_callbacks(script, &items, &commands, &mut notices);
        // The core's cancel goes out open or not (`0x4bf527`).
        if script.take_trade_close() {
            crate::ui_session::queue(&mut world_commands, close_core);
        }
    })
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

#[cfg(test)]
mod tests;
