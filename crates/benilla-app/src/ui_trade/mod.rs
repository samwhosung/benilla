//! The app-side **trade feed** (decision 0592 P1) — the inward half of the trade seam around
//! [`benilla_ui::script`]'s `trade` module, the two-sided twin of [`crate::ui_mail`]'s mailbox seam.
//!
//! Trade is entirely server-driven: a right-click → **Trade** ([`crate::ui_trade`] via the UnitPopup
//! row) sends `CMSG_INITIATE_TRADE`; the packet handlers ([`net`]) then fold the two
//! status packets into [`TradeSession`] — `SMSG_TRADE_STATUS` drives the open/accept/close state
//! machine, `SMSG_TRADE_STATUS_EXTENDED` replaces one side's item/gold snapshot. Each frame
//! [`feed_trade`] resolves both sides' wire [`TradeItem`]s to Lua-facing
//! [`benilla_ui::script::TradeSlotItem`]s (name/quality via the ask-once item-template cache, icon via
//! the wire `display_id` + `ItemDisplayInfo.dbc`) and the partner's name (via the name cache), pushes
//! the snapshot ([`benilla_ui::script::UiScript::set_trade`]), and fires the events the reference Lua
//! drives — `TRADE_SHOW` on open, `TRADE_UPDATE` on any offer change, `TRADE_CLOSED` on close, and
//! `TRADE_ACCEPT_UPDATE(myState, theirState)` on an accept-glow change. [`drain_trade`] pulls the Lua
//! intents (`InitiateTrade`/`AcceptTrade`/`CancelTradeAccept`/`CloseTrade`) back out into the trade
//! `CMSG`s.
//!
//! **An incoming request is answered here, not on the wire**.
//! `SMSG_TRADE_STATUS(BEGIN_TRADE)` only *records* the request; [`answer_trade_request`] walks the
//! reference's own eight-leg ladder (`0x4bf736`) and either refuses it — on one of three different
//! opcodes, or with no packet at all — or accepts it, which is what the client did unconditionally
//! before. The split is what made the ladder possible when it was built: it reads the ignore
//! list, the cinematic state, player control, the auction house, the initiator's descriptor, an
//! outbound initiate of ours and the [`BlockTrades`] CVar, none of which could reach the drain's
//! dispatch match at Bevy's 16-`SystemParam` ceiling (decision 1725 left the cinematic leg unbuilt
//! for exactly that reason and said so). Since 2306 the status packet is this module's own
//! handler ([`net`]) and that ceiling is gone; what keeps the ladder a system of its own is that
//! its answer may **span frames**, which a packet handler cannot do.
//!
//! **There is no consent prompt, and that is faithful rather than missing.** 1.12.1 registers a
//! `TRADE_REQUEST` event (slot `0xbe160c`) and signals it from nowhere, so the `TRADE` StaticPopup
//! that would have asked is dead code in the real client. benilla wired that dialog up and took it
//! back out: the reference's answer to an unwanted trade is the ignore list and the
//! *Block Trades* checkbox, both of which are legs of the ladder below.
//!
//! **The partner's portrait** rides the shared `"npc"` booth token: [`TradeSession`] implements
//! [`NpcSession`] so [`crate::ui_session::feed_interact_npc`] points the `"npc"` portrait at the
//! partner's live entity while the window is open — the partner is a nearby player (within
//! `TRADE_DISTANCE`, always streamed), so the booth bakes their dressed look exactly as it bakes a
//! vendor's. Trade is deliberately **excluded** from the range-guard auto-close
//! ([`crate::ui_session::close_npc_session_out_of_range`]): unlike the NPC windows (a no-packet local
//! clear), a trade's out-of-range cancel is server-driven (vmangos sends `CANCELED` past
//! `TRADE_DISTANCE`), and the walk-away/death/disconnect unwinding is decision 0592 P3.
//!
//! **Scope (decision 0592 P1).** The window opens, mirrors both sides' items + gold live, and drives
//! the accept glow (the accept verb is a wire pass-through: click Trade → `CMSG_ACCEPT_TRADE`, and a
//! 200 ms scam-delay bounce comes back as `BACK_TO_TRADE` and drops the glow honestly). **Setting**
//! items/gold onto the window — the drag-drop slots + the editable money widget — is decision 0592 P2.

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

/// One side's offer as the wire delivered it — the seven slots (index 0 = trade slot 1 … index 6 =
/// the non-traded / enchant slot) plus the gold and the enchant-slot spell. Filled from
/// `SMSG_TRADE_STATUS_EXTENDED`; the feed resolves the [`TradeItem`]s to display rows.
#[derive(Default)]
struct TradeOffer {
    slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
    gold: u32,
    /// The spell applied to this side's enchant slot (`0` = none) — decision 0592 P3.
    enchant_spell_id: u32,
}

/// The open trade window's session, filled by the packet handlers ([`net`]) and read
/// by [`feed_trade`]. Holds the partner guid, the open flag, both sides' wire offers, and the two
/// accept flags. Cleared on cancel/complete/close and on disconnect.
///
/// `partner` is set the moment the trade begins (the initiator records its target at
/// `CMSG_INITIATE_TRADE` time, since the later `OPEN_WINDOW` carries no guid; the target records it
/// when the ladder *accepts* the request — see `request`); `open` flips on `OPEN_WINDOW`. `npc()`
/// is gated on `open` so the `"npc"` portrait bakes only while the window is up.
#[derive(Resource, Default)]
pub(crate) struct TradeSession {
    /// The trade partner's guid; `Some` from the initiate/accept onward, `None` = no trade.
    partner: Option<u64>,
    /// An incoming `BEGIN_TRADE` **this client has not answered yet** — the
    /// initiator's guid. Set by [`Self::request`] and cleared the moment
    /// [`answer_trade_request`] resolves it, either by promoting it to `partner`
    /// ([`Self::begin`]) or by refusing ([`Self::refuse_request`]). Mutually exclusive with
    /// `partner`: a request that has not been answered is not yet a trade.
    request: Option<u64>,
    /// **An outbound `CMSG_INITIATE_TRADE` of ours is live** — the reference's `[0xc4bec8]`, and
    /// leg 1 of [`answer_trade_request`]'s ladder, where it drops an incoming request without a
    /// reply of any kind.
    ///
    /// Its own field rather than `partner.is_some()` because the two are different sets, and the
    /// binary is explicit about which — a complete 5-site census of `[0xc4bec8]`: set at
    /// `0x5d4021` as `CMSG_INITIATE_TRADE` goes out, zeroed at module init (`0x5d478e`) and in the
    /// status handler's common tail (`0x5d4931`), read twice. So a trade we did not start has it
    /// clear for its whole life, while `partner` is set the moment we accept one.
    ///
    /// **It survives `OPEN_WINDOW`**, which is the half that is easy to get wrong and was worth
    /// asking about: the tail only clears on the statuses that set `esi`, and 1, 2 (`OPEN_WINDOW`),
    /// 4, 7, 9 and 22 do not — so the latch stands for the whole life of a trade *we* opened, and
    /// leg 1 drops an incoming request throughout it. Modelling it as "we asked and no window has
    /// opened yet" would have been strictly narrower, and wrong in exactly the window where it
    /// matters. Ours is cleared by the session reset, which is the same moment.
    initiated: bool,
    /// The window is up (`OPEN_WINDOW` arrived). `partner` can be `Some` before this (an initiate in
    /// flight the server hasn't opened yet).
    open: bool,
    /// Our own offer (the wire's `their_window == false` snapshot).
    our: TradeOffer,
    /// The partner's offer (the wire's `their_window == true` snapshot).
    their: TradeOffer,
    /// We pressed Trade (our accept glow) — client-tracked, optimistic like the reference's
    /// button-disable; a `BACK_TO_TRADE`/offer-change/close drops it.
    our_accept: bool,
    /// The partner pressed Trade (their accept glow) — set by `TRADE_STATUS_TRADE_ACCEPT`.
    their_accept: bool,
    /// **A `TRADE_REQUEST_CANCEL` is owed to the VM** — set by the `CANCELED` arm of
    /// [`net`]'s status handler, drained by [`feed_trade`] on the next frame.
    ///
    /// It is a *pending signal* rather than session state, so it is the one field
    /// [`Self::close`] carries across the reset: the reference's own `CANCELED` arm signals the
    /// event (`0x4bf832`) and *then* clears the window (`0x4bf842`), and a flag wiped by the
    /// clear could never be observed. The net drain cannot fire it itself — a `SessionEvent` arm
    /// has no `UiScript`, which is the same seam `answer_trade_request` exists for.
    request_cancel: bool,
    /// **Lines owed that name a player** — `(GlobalStrings key, whose name fills the `%s`)`,
    /// parked until [`NameCache`] can answer and drained by [`feed_trade`].
    ///
    /// The reference has no queue because it never waits: `ERR_INITIATE_TRADE_S` comes off the
    /// player-name cache inside the `CMSG_INITIATE_TRADE` sender (`0x5d4031`, printing at
    /// `0x5d4042` on a hit and from the query callback `0x5d4080` at `0x5d40ca` on a miss), and the
    /// status arms' `%s` lines read the name straight off the live `CGUnit` (`0x609210`). benilla
    /// may need a `CMSG_NAME_QUERY` round trip for either, which is a wait a wire decoder cannot
    /// do — the same seam [`answer_trade_request`]'s leg 8 exists for.
    ///
    /// A `Vec` rather than one slot because two can be owed at once: our own initiate parks a line
    /// and the server's refusal of that same initiate parks another, and the second must not eat
    /// the first.
    ///
    /// It survives [`Self::close_window`] for the reference's own reason: its callback hangs off
    /// the **name query**, not off the trade, so a refusal arriving before the name does not
    /// cancel the line. Nothing hangs on the wait — one query at worst, and in practice the target
    /// is the unit you right-clicked, whose name is already cached.
    named_lines: Vec<NamedLine>,
}

/// A line owed whose `%s` names a player — see [`TradeSession::named_lines`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NamedLine {
    /// The `GlobalStrings` key; the catalog names its surface.
    pub(crate) key: &'static str,
    /// Whose name fills the `%s`.
    pub(crate) who: u64,
    /// **The status arms' second guard**, and the difference between the two kinds of line here.
    ///
    /// Cases 0/5/14 take the name off the named player's **live `CGUnit`** (`0x609210`) after
    /// asking `0x468460(guid, TYPEMASK_PLAYER)` whether the object is still there — so if it is
    /// not, the reference prints **nothing**, and never waits. `ERR_INITIATE_TRADE_S` is the
    /// opposite shape: it goes through the player-name **cache** `0x55f080`, has no
    /// object test at all, and defers to the name query's callback when the cache misses.
    ///
    /// benilla has one name source for both, so the distinction has to be carried: `true` drops
    /// the line the moment the player stops being a streamed object, `false` waits indefinitely
    /// for the name. Collapsing them either way is wrong in one direction — a stale cached name
    /// printed for someone who is gone, or an initiate line silently dropped.
    pub(crate) needs_live_object: bool,
}

impl TradeSession {
    /// The initiator records its target the moment it sends `CMSG_INITIATE_TRADE` — `OPEN_WINDOW`
    /// carries no guid, so this is where the initiator's side learns the partner. Resets any stale
    /// prior offer.
    pub(crate) fn initiate(&mut self, target: u64) {
        *self = TradeSession {
            partner: Some(target),
            initiated: true,
            // The reference prints its line from inside the sender, so the owed line is parked in
            // the same breath as the packet — see [`Self::named_lines`].
            named_lines: vec![NamedLine {
                key: "ERR_INITIATE_TRADE_S",
                who: target,
                needs_live_object: false,
            }],
            ..Default::default()
        };
    }

    /// `BEGIN_TRADE(initiator guid)` — record the incoming request, **unanswered**. Nothing goes
    /// on the wire here: [`answer_trade_request`] owns the reply.
    ///
    /// Records it *beside* whatever session already exists rather than resetting — a request is
    /// not a trade, and a request arriving over a live one must not destroy it. That reset is
    /// [`Self::begin`]'s, at the moment the request is accepted and there really is a fresh trade.
    /// Leaving it here would also have made the "already trading" refusal leg dead code, since
    /// the ladder reads the very fields the reset had just cleared.
    pub(crate) fn request(&mut self, initiator: u64) {
        self.request = Some(initiator);
    }

    /// The incoming request awaiting an answer, if any.
    pub(crate) fn pending_request(&self) -> Option<u64> {
        self.request
    }

    /// Spend the pending request **without touching the rest of the session** — the refusal path's
    /// counterpart to [`Self::begin`]. It clears only the request for the same reason
    /// [`Self::request`] sets only the request: refusing an offer is not cancelling the trade you
    /// are in, and on a fresh session the two are indistinguishable anyway (there is nothing else
    /// to clear). The server's own `TRADE_STATUS_BUSY` reply closes what needs closing.
    pub(crate) fn refuse_request(&mut self) {
        self.request = None;
    }

    /// Accept the pending request: the initiator becomes the partner and the request is spent. The
    /// caller sends `CMSG_BEGIN_TRADE` in the same breath; the server answers both sides
    /// `OPEN_WINDOW`.
    pub(crate) fn begin(&mut self, partner: u64) {
        *self = TradeSession {
            partner: Some(partner),
            ..Default::default()
        };
    }

    /// `OPEN_WINDOW` — both windows go up; the feed's open edge fires `TRADE_SHOW`.
    pub(crate) fn open_window(&mut self) {
        self.open = true;
    }

    /// A fresh `SMSG_TRADE_STATUS_EXTENDED` for one side — replace that side's offer and reset **both**
    /// accepts (any offer change invalidates prior accepts, the server's own rule; on open the initial
    /// snapshot's reset is a no-op since accepts are already clear).
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

    /// Place our own item into trade slot `id_1based` **optimistically** (decision 0592 P2): vmangos
    /// echoes a `SET_TRADE_ITEM` only to the *partner* (`TradeData::SetItem` → `Update(true)`), never
    /// to the placer, so our own column is client-side — the drain resolves the bag item and fills
    /// `our` here, which the feed then paints exactly like the partner's server-driven side. Changing
    /// our offer resets both accepts, mirroring the server's `SetAccepted(false)` on both.
    pub(crate) fn place_own_item(&mut self, id_1based: u32, item: TradeItem) {
        if let Some(slot) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our.slots.get_mut(i as usize))
        {
            *slot = Some(item);
            self.our_offer_changed();
        }
    }

    /// Clear our own trade slot `id_1based` optimistically (the twin of [`Self::place_own_item`]).
    pub(crate) fn clear_own_item(&mut self, id_1based: u32) {
        if let Some(slot) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our.slots.get_mut(i as usize))
        {
            *slot = None;
            self.our_offer_changed();
        }
    }

    /// Set our own offered gold optimistically (the money input's value — the same client-side reason
    /// the items are; `SetMoney` also only echoes to the partner).
    pub(crate) fn set_own_gold(&mut self, copper: u32) {
        self.our.gold = copper;
        self.our_offer_changed();
    }

    /// Any change to our own offer resets both accepts, exactly as vmangos does server-side.
    fn our_offer_changed(&mut self) {
        self.our_accept = false;
        self.their_accept = false;
    }

    /// `TRADE_STATUS_TRADE_ACCEPT` — the partner pressed Trade.
    pub(crate) fn partner_accepted(&mut self) {
        self.their_accept = true;
    }

    /// `TRADE_STATUS_BACK_TO_TRADE` — an accept was withdrawn (a change, the partner's un-accept, or
    /// the 200 ms scam-delay bounce): drop both accepts.
    pub(crate) fn back_to_trade(&mut self) {
        self.our_accept = false;
        self.their_accept = false;
    }

    /// We pressed Trade (the drained `AcceptTrade` intent) — optimistic local accept beside the
    /// `CMSG_ACCEPT_TRADE` send.
    pub(crate) fn accept(&mut self) {
        self.our_accept = true;
    }

    /// Whether **our** accept flag is up — the reference's `[0xb71730]`, which `0x4bf230`'s
    /// idempotence check reads before it changes anything or sends `CMSG_UNACCEPT_TRADE`.
    pub(crate) fn we_accepted(&self) -> bool {
        self.our_accept
    }

    /// We dropped our accept (`CancelTradeAccept`) — local twin of the `CMSG_UNACCEPT_TRADE` send.
    pub(crate) fn unaccept(&mut self) {
        self.our_accept = false;
    }

    /// **Signal `TRADE_REQUEST_CANCEL`** — the one Lua event the reference's 23-case status
    /// dispatcher fires, from the top of its `CANCELED` arm (`0x4bf832`, the sole `SignalEvent`
    /// inside `[0x4bf720, 0x4bfa08)`). See [`Self::request_cancel`] for why it survives
    /// [`Self::close`].
    pub(crate) fn signal_request_cancel(&mut self) {
        self.request_cancel = true;
    }

    /// Take the owed `TRADE_REQUEST_CANCEL`, if any — [`feed_trade`]'s side of the flag.
    fn take_request_cancel(&mut self) -> bool {
        std::mem::take(&mut self.request_cancel)
    }

    /// Park a line that names a player — see [`Self::named_lines`].
    pub(crate) fn owe_named_line(&mut self, line: NamedLine) {
        self.named_lines.push(line);
    }

    /// Take everything owed — [`feed_trade`]'s side of [`Self::named_lines`]. The caller parks
    /// back whatever it could not name yet.
    fn take_named_lines(&mut self) -> Vec<NamedLine> {
        std::mem::take(&mut self.named_lines)
    }

    /// **The name a `%s` status line carries**, or `None` when the reference's own two guards
    /// stop it printing at all.
    ///
    /// Cases 0, 5 and 14 (`ERR_PLAYER_BUSY_S`, `ERR_IGNORING_YOU_S`) read the *network* handler's
    /// guid cell `[0xc4bed0]`/`[0xc4bed4]` — never `0x4bf4e0`'s window-partner pair
    /// `[0xb71728]`/`[0xb7172c]` — and each is guarded on `[0xc4bec8]`, the outbound-initiate
    /// latch that exactly one instruction image-wide writes (`0x5d4021`, inside `InitiateTrade`).
    /// **The cell holds different players on the two sides**, and that is the whole mechanism:
    /// `InitiateTrade 0x5d3fb0` writes the **target's** guid (`0x5d401c`/`0x5d4010`) and then the
    /// latch (`0x5d4021`) in the same breath, while the `BEGIN_TRADE` arm writes the
    /// **initiator's** (`0x5d4815`/`0x5d481a`) and touches no latch. So an initiator has both and
    /// prints, naming the player it clicked; a recipient has the cell but no latch and prints
    /// nothing. (`0x5d3f70`, the latch getter, has exactly three callers image-wide — the three
    /// name-bearing arms and nothing else.)
    ///
    /// That asymmetry is load-bearing, not incidental. vmangos's `HandleIgnoreTradeOpcode` calls
    /// `TradeCancel(sendback = true, …)`, which sends `IGNORE_YOU` to **both** sides — so the
    /// client that just refused a trade gets the status back at itself with the initiator's guid
    /// still in the cell. The latch, which a refuser never set, is what makes that arrive
    /// silently. A client that dropped the guard would print "%s is ignoring you." to the person
    /// who did the ignoring: inventing a quirk, not reproducing one.
    ///
    /// The reference's **second** guard is not a cell test — it is
    /// `0x468460(guid, TYPEMASK_PLAYER)`, "is that player still a resolvable object", because the
    /// arm reads the name off the live `CGUnit`. It rides the owed line as
    /// [`NamedLine::needs_live_object`], since benilla cannot answer it until it resolves the name.
    pub(crate) fn line_names(&self) -> Option<u64> {
        self.initiated.then_some(self.partner).flatten()
    }

    /// **Close the window** — the reference's `SetTradePartner(0, 0)` (`0x4bf4e0`), which zeroes
    /// the partner guid `[0xb71728]`/`[0xb7172c]` and its three 7-slot mirrors and fires
    /// `TRADE_CLOSED`. Reached from **exactly three** status arms — `CANCELED` (`0x4bf842`),
    /// `COMPLETE` (`0x4bf867`) and `CLOSE_WINDOW` (`0x4bf89c`) — plus the local close verb
    /// (`CloseTrade 0x4bfdc0` at `0x4bfdd9`) and the world-click select path.
    ///
    /// **This is NOT what the other fourteen closing-looking statuses do** — see
    /// [`Self::clear_pending`]. Conflating the two is the mistake this pair exists to prevent, and
    /// benilla made it: `close` used to be called from seventeen arms.
    ///
    /// All three statuses that reach it are also in the tail's clear set, and a full reset is a
    /// superset of [`Self::clear_pending`] — so the arm calls only this one.
    ///
    /// Clears everything except the two things in flight to the VM — [`Self::request_cancel`] and
    /// [`Self::named_lines`] — which are signals, not session state, and which the reference
    /// raises *before* it clears (`0x4bf832` precedes `0x4bf842`).
    pub(crate) fn close_window(&mut self) {
        *self = TradeSession {
            request_cancel: self.request_cancel,
            named_lines: std::mem::take(&mut self.named_lines),
            ..Default::default()
        };
    }

    /// **The status handler's own common tail** (`0x5d490a`) — clear the three *network* cells
    /// `[0xc4bec8]`, `[0xc4bed0]`, `[0xc4bed4]`: the outbound-initiate latch and the pending guid.
    /// It runs on every status that leaves `esi != 0`, which is all of them except
    /// `{1, 2, 4, 7, 9, 22}` — and it is **strictly after** the UI dispatcher call (`0x5d4931` vs
    /// `0x5d4923`), so an arm always reads the cells before they go.
    ///
    /// It does **not** touch the window. On vmangos every status that reaches this without also
    /// closing arrives with no window open (they are all initiate refusals), so the distinction
    /// has no observable there — but writing it as a close is how the code came to claim the
    /// reference tears the window down on `TARGET_STUNNED`, which it does not.
    ///
    /// `partner` clears only while the window is shut, because benilla fuses two of the
    /// reference's cells into that one field: before `OPEN_WINDOW` it stands in for the pending
    /// guid `[0xc4bed0]`/`[0xc4bed4]` (which this clears), and after it for the window partner
    /// `[0xb71728]`/`[0xb7172c]` (which this must not).
    pub(crate) fn clear_pending(&mut self) {
        self.initiated = false;
        self.request = None;
        if !self.open {
            self.partner = None;
        }
    }

    /// `TRADE_STATUS_TRADE_REJECTED` (case 9, `0x4bf821`) — drop **the partner's** accept and
    /// nothing else. It closes no window, prints no line and clears no cell (9 is one of the six
    /// codes that leave `esi == 0`); benilla used to close the whole session on it. vmangos never
    /// sends it, so this is unobservable there and is written the reference's way because there
    /// is now no reason for it to be written any other way (1764 left it open for want of one).
    pub(crate) fn partner_unaccepted(&mut self) {
        self.their_accept = false;
    }

    /// `TRADE_STATUS_ONLY_CONJURED` (case 22, `0x4bf9ec` → `0x4bfbd0`) — the placement is
    /// bounced, not the window: the offending slot is emptied out of **our** offer. `0xff` names
    /// the money instead of a slot.
    ///
    /// The reference indexes its 7-slot mirror with the wire byte and **does not bound-check it**;
    /// ours does, because reproducing an out-of-bounds write is not reproducing a behaviour. The
    /// repaint events (`TRADE_PLAYER_ITEM_CHANGED(slot + 1)`, `PLAYER_TRADE_MONEY`) fall out of
    /// the feed's own diff, which is where every other offer change already raises them.
    pub(crate) fn bounce_own_offer(&mut self, slot: u8) {
        const MONEY: u8 = 0xff;
        if slot == MONEY {
            self.our.gold = 0;
        } else if let Some(cell) = self.our.slots.get_mut(usize::from(slot)) {
            *cell = None;
        }
    }

    /// Disconnect: drop the open window (mirrors the merchant/mail session clears). Unlike
    /// [`Self::close_window`] this drops the pending [`Self::request_cancel`] and
    /// [`Self::named_lines`] too — there is no VM left to hear either, and a session teardown is
    /// not a trade event.
    pub(crate) fn clear_session(&mut self) {
        *self = TradeSession::default();
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    // ── Test windows onto the private state, for the sibling module that owns the status arm ──
    //    ([`net`], whose tests drive `trade_status` and then have to ask what
    //    it did). Reading them through named accessors rather than opening the fields keeps the
    //    session's invariants — `partner`/`request` mutually exclusive, `open` gating `npc()` —
    //    enforced in exactly one file.

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

/// Trade points the shared `"npc"` portrait booth at the partner while the window is open (the
/// partner is a live nearby player entity — see the module doc). It is **not** registered with the
/// range-guard auto-close: a trade's cancel is server-driven, not a local clear.
impl NpcSession for TradeSession {
    fn npc(&self) -> Option<u64> {
        self.open.then_some(self.partner).flatten()
    }

    fn close(&mut self) {
        self.close_window();
    }
}

pub(crate) struct UiTradePlugin;

/// Block Trades' change callback (1764, 2303): a flag.
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
                    // Feed before the input pass so an open/close/mirror is on screen the same
                    // frame; drain after it so a click's intent (initiate, accept, close) goes out
                    // the same frame (the ui_mail ordering exactly). After the UnitFeed set so the
                    // resolved item-template store is landed. No range-guard registration — a
                    // trade's cancel is server-driven (the module doc).
                    feed_trade.after(crate::ui_unit::UnitFeed).in_set(UiFeed),
                    // The incoming request's answer needs no VM at all — it is wire policy over
                    // engine state — so it sits ahead of the feed, and an accepted request's
                    // `partner` is on screen the same frame the window opens.
                    answer_trade_request.before(feed_trade),
                    // The coinage-change reflex's trade half (1965): after the world's fields land.
                    trim_offer_to_purse
                        .after(crate::ui_unit::UnitFeed)
                        .before(feed_trade),
                    drain_trade.after(UiInput),
                ),
            );
    }
}

/// **Block Trades** — 1.12's own `BlockTrades` CVar (`0x842fbc`), the *Block Trades* checkbox in
/// Basic Options' General box (`UIOptionsFrame.lua` l.11: `UIOptionsFrameCheckButtons`
/// `["BLOCK_TRADES"] = { index = 14, cvar = "BlockTrades" }`, tooltip
/// `OPTION_TOOLTIP_BLOCK_TRADES` = "Block all incoming trade requests.").
///
/// A knob resource rather than a field on [`TradeSession`], for the same reason
/// [`crate::ui_guild::GuildMemberNotify`] is one: it is a *setting*, and `TradeSession` is cleared
/// on every disconnect.
///
/// Its one reader is [`answer_trade_request`]'s last refusal leg — the only leg of that ladder
/// that speaks, which is why the silent ones are ordered ahead of it.
#[derive(Resource, Default)]
pub(crate) struct BlockTrades(pub(crate) bool);

/// **Answer an incoming trade request** — the arm the reference keeps inside `CGTradeInfo`'s
/// dispatcher (`0x4bf736`, case 1 of the 23-case `0x4bf720`), lifted out to where its inputs live.
///
/// The status handler only *records* the request ([`TradeSession::request`]); this system decides.
/// The split is the point: the answer can **span frames**, which a packet handler cannot do — the
/// `BlockTrades` line names the initiator, and that name may need a `CMSG_NAME_QUERY` first. (It
/// was first cut for a second reason that 2306 retired: the decision reads the ignore list, the
/// cinematic state, player control, the auction house, the initiator's own descriptor, an outbound
/// initiate of ours and a CVar, none of which could reach the drain's dispatch match at Bevy's
/// 16-`SystemParam` ceiling — exactly why decision 1725 left the cinematic leg unbuilt and said so.)
///
/// **The ladder is the reference's, in its order** — eight legs, walked contiguously over
/// `[0x4bf736, 0x4bf7f8)`. Three answer with something other than "busy", which is why this is a
/// ladder and not a boolean:
///
/// | # | condition | benilla reads | answer |
/// |---|---|---|---|
/// | 1 | our own `CMSG_INITIATE_TRADE` is in flight (`[0xc4bec8]`) | [`TradeSession::initiated`] | **nothing at all** |
/// | 2 | the initiator is on our **ignore list** (`0x5ae5a0`, by guid, cap 25) | [`crate::ui_social::SocialState::is_ignored`] | **`CMSG_IGNORE_TRADE`**, silent |
/// | 3 | the initiator resolves to no streamed **player** object (`0x468460(TYPEMASK_PLAYER)`) | [`GuidIndex`] + [`ObjectStore`] | **nothing at all** |
/// | 4 | the initiator is dead or a ghost (`0x605f30`) | raw health ≤ 0, or `PLAYER_FLAGS` ghost | `CMSG_BUSY_TRADE`, silent |
/// | 5 | a cinematic is playing (`[0xb4e310]`) | [`crate::cinematic::Cinematic`] | `CMSG_BUSY_TRADE`, silent |
/// | 6 | we have lost **player control** (`[0xb4b3e4]`) | `Player::control_lost` | `CMSG_BUSY_TRADE`, silent |
/// | 7 | an **auction house** window is open (`[0xb725f8]`/`[0xb725fc]`) | [`crate::ui_auction::AuctionOpen`] | `CMSG_BUSY_TRADE`, silent |
/// | 8 | the `BlockTrades` CVar is set (`0x842fbc`) | [`BlockTrades`] | `CMSG_BUSY_TRADE` **+ the one chat line** |
/// | — | nothing refuses it | — | **`CMSG_BEGIN_TRADE`**, and the server opens both windows |
///
/// Only leg 8 speaks, so the **order is behaviour**, not taste: a request during a cinematic is
/// refused silently because that leg returns before the message.
///
/// **There is no consent step, and that is the reference's answer, not an omission.** The 5875
/// client registers a `TRADE_REQUEST` event (id `0x11d`) and signals it from nowhere — a
/// whole-image census — so `StaticPopupDialogs["TRADE"]`, its `TRADE_WITH_QUESTION` text and its
/// `BeginTrade`/`CancelTrade` verbs are dead code there, and an incoming trade that survives the
/// ladder opens the window unasked. benilla briefly wired that dialog up and then took it back out:
/// a popup the real client never shows is a divergence every addon and every
/// player would feel, and the reference's own answer to an unwanted trade is leg 2 and leg 8 —
/// ignore them, or tick *Block Trades*.
///
/// Three legs are easy to get wrong: `[0xb4b3e4]` is player *control*, not "in world";
/// `[0xb725f8]` is the auction house, not a pending trade; and the ignore leg is easy to miss
/// entirely.
/// **`0x468460(guid, TYPEMASK_PLAYER)`** — the guid's live object, or `None` when it is not a
/// streamed player. The reference asks this in two places in this arc and benilla now asks it in
/// the same two: the incoming ladder's leg 3 (`0x4bf779`, which drops a request from a guid that
/// resolves to nothing) and the status arms' second guard on a `%s` line
/// ([`NamedLine::needs_live_object`]). One predicate, because it is one question.
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

    // ── Legs 1 and 3: the two that send NOTHING ─────────────────────────────────────────────────
    //
    // Leg 1 — our own `CMSG_INITIATE_TRADE` is in flight. See [`TradeSession::initiated`]: the
    // latch is "we asked somebody", it survives `OPEN_WINDOW`, and it is never set by a trade we
    // did not start.
    //
    // Leg 3 — the initiator does not resolve to a streamed **player**. The reference asks
    // `0x468460(TYPEMASK_PLAYER)` and returns on NULL; ours is the guid index plus the store, and
    // it fails in exactly the case the reference's does (nothing of that guid in the world).
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

    // ── Leg 2: the ignore list, and the ONLY leg that answers on a different opcode ─────────────
    //
    // The server has no ignore check for trade at all (vmangos `HandleInitiateTradeOpcode` reads
    // no social list), so this refusal is entirely the client's — the same shape as the duel
    // challenge's. `CMSG_IGNORE_TRADE` rather than busy is what makes the
    // initiator read "… is ignoring you" instead of "… is busy", and it is a single call site
    // image-wide (`0x4bf759` → `0x5d41c0`). Both sides key on the guid, and both cap at 25.
    if social.is_ignored(initiator) {
        info!(target: "trade", "BEGIN_TRADE from an ignored {initiator:#x}; sending CMSG_IGNORE_TRADE");
        let _ = commands.0.send(ClientCommand::IgnoreTrade);
        trade.refuse_request();
        return;
    }

    // ── Legs 4-7: the silent "busy" refusals. None needs a name, so none waits for one ──────────
    //
    // Leg 4 — the initiator is dead or a ghost. `0x605f30` is **raw** health ≤ 0 or the
    // `PLAYER_FLAGS` ghost bit; deliberately NOT `unit_reads_dead()`, which also takes the
    // dead-looking dynflag and would refuse a feigning hunter the reference trades with.
    // Leg 5 — a cinematic (`[0xb4e310]`): decision 1725 named this exact hole and left it for want
    // of a param slot here.
    // Leg 6 — player control lost (`[0xb4b3e4]`): NOT an in-world flag. The cell is written by
    // `0x4958e0`, which signals `PLAYER_CONTROL_GAINED`/`LOST` — the same wire fact
    // `SMSG_CLIENT_CONTROL_UPDATE` gives `Player::control_lost`.
    // Leg 7 — an auction house is open (`[0xb725f8]`/`[0xb725fc]`, one auctioneer guid split
    // across two cells, set and cleared with the AH window).
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

    // ── Leg 8 is the only one that names the initiator, so only it waits for the name cache ─────
    //
    // The reference reads it straight off the initiator's live `CGUnit` (`0x609210`) and does not
    // have to wait; benilla may need a `CMSG_NAME_QUERY` round trip first. Leg 3 above has already
    // established the initiator is streamed, which is exactly the case the reference has the name
    // for free, so in practice there is nothing to wait for.
    //
    // **The wait needs no timer, because the server already bounds it.** The only way a name never
    // arrives is a player the server can no longer find — and that is precisely the case where
    // vmangos's `CleanupsBeforeDelete` sends US `TRADE_STATUS_TRADE_CANCELED` regardless of the
    // leaver's own `sendback`, which clears the request through `close`. `NameCache` asks once per
    // guid per connection, so the frames spent here cost one query, not one per frame.
    //
    // It is also the one leg that could not live in the net drain even with a param to spare: the
    // answer may land a frame or two after the packet did.
    if block_trades.0 {
        let Some(name) = names.resolve(initiator, &commands).map(str::to_string) else {
            return;
        };
        info!(target: "trade", "BEGIN_TRADE from {name} refused by BlockTrades; sending CMSG_BUSY_TRADE");
        let _ = commands.0.send(ClientCommand::BusyTrade);
        trade.refuse_request();
        // `0x496720(0xbb, 0x609210(initiator))` — one `DisplayError` call, which is exactly what
        // [`crate::ui_action::UiErrorKeys`] models: the key names the row, the row names the
        // surface, and `ui_action::feed_actions` resolves the text out of the **player's own**
        // `GlobalStrings.lua` and routes it. Row `0xbb` carries `kind = 0`, chat type `0xa`, no
        // sound cue — a system chat line, not the red `UI_ERROR_MESSAGE` (`kind = 2`) — but no
        // line here says so, and that is the point: this arm no longer knows or decides.
        errors
            .0
            .push(crate::ui_action::UiError::s("ERR_TRADE_BLOCKED_S", name));
        return;
    }

    // Nothing refuses it: accept, exactly as the reference does at the bottom of the same ladder.
    info!(target: "trade", "BEGIN_TRADE from {initiator:#x} accepted; sending CMSG_BEGIN_TRADE");
    trade.begin(initiator);
    let _ = commands.0.send(ClientCommand::BeginTrade);
}

/// Resolve one wire [`TradeItem`] into the Lua-facing [`TradeSlotItem`]: name/quality through the
/// ask-once item-template cache, icon through the wire `display_id` + `ItemDisplayInfo.dbc`. `None`s
/// stay `None` while a template query is in flight — the slot shows the icon (from the wire) and fills
/// in the name/quality when the answer lands (the mail/merchant pattern). The enchant-slot spell name
/// is decision 0592 P3.
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
    // The same link the merchant rows carry, built by the same helper — nil until the template
    // answers, because the string embeds both the name and the quality colour.
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

/// Resolve the bag item at a cursor-space `(bag, 1-based slot)` into a wire [`TradeItem`] for our own
/// **optimistic** display (decision 0592 P2): entry from the descriptor, display id from the ask-once
/// item template (cached already — the item was showing in the bag), count from the slot. `None` for an
/// empty or not-yet-streamed slot. The item stays in the bag until the trade completes; this is only
/// what WE show in our own column, since vmangos never echoes a placement back to the placer.
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

/// Resolve one side's wire offer into the Lua-facing side state.
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

/// Build the Lua-facing snapshot from [`TradeSession`] — `None` when no trade window is open.
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

/// Push the current trade into the VM and fire the show/update/close + accept-glow events on a
/// transition or content change (an async item/name landing, a fresh offer, an accept flip). Diffed
/// against `Local` memory, exactly like the mail/merchant feeds. The accept-update fires **after** the
/// show/update block so an open frame's `TRADE_SHOW` (which hides the highlights) is followed by the
/// glow, not overwritten by it.
fn feed_trade(
    script: Option<NonSendMut<UiScript>>,
    mut trade: ResMut<TradeSession>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    // The status arms' second guard (`0x468460(guid, TYPEMASK_PLAYER)`) — see
    // [`NamedLine::needs_live_object`]. The same two reads `answer_trade_request`'s leg 3 makes.
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
    // The per-slot events the stock TradeFrame.lua repaints one slot on (`0x4bf414`/`0x4bf452`
    // TRADE_TARGET_ITEM_CHANGED, `0x4bf487`/`0x4bfaef` TRADE_PLAYER_ITEM_CHANGED, arg1 the 1-based
    // slot). The full TRADE_UPDATE below is kept:
    // the reference fires it too, from `0x4c034f`.
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
    // The owed lines that name a player ([`TradeSession::named_lines`]) — resolved here rather
    // than where they are raised, because this is where the name is: the raiser parks a guid,
    // this asks the cache, and a miss simply waits for the query it just started. Where each
    // lands is the message catalog's `+0x04`, read by `ui_action::feed_actions`; no line here
    // decides it (all three of these happen to be chat rows, and nothing depends on that).
    for line in trade.take_named_lines() {
        // The status arms read the name off the named player's live `CGUnit`, having first asked
        // whether that object is still there — and print nothing when it is not. Ours asks the
        // same question of the guid index and drops the line on a miss, rather than serving a
        // cached name for somebody who has gone.
        if line.needs_live_object && streamed_player(&index, &stores, line.who).is_none() {
            continue;
        }
        match names.resolve(line.who, &commands).map(str::to_string) {
            Some(name) => errors.0.push(crate::ui_action::UiError::s(line.key, name)),
            // Not cached yet: `resolve` has asked, so park the debt again and retry next frame.
            None => trade.owe_named_line(line),
        }
    }
    // `TRADE_REQUEST_CANCEL` — the `CANCELED` arm's own signal, and the ONLY Lua event the
    // reference's 23-case status dispatcher fires (`0x4bf832`; there is no other `SignalEvent` in
    // `[0x4bf720, 0x4bfa08)`). It goes out **ahead of** the
    // `TRADE_CLOSED` the same arm's window clear fires, which is the arm's own address order:
    // signal `0x4bf832`, then `0x4bf842 call 0x4bf4e0`, whose `0x4bf522` is `TRADE_CLOSED`.
    //
    // Nothing stock listens: 1.12's `UIParent.lua` hides the `TRADE` StaticPopup on it, and that
    // dialog can never be up (its `TRADE_REQUEST` is signalled from nowhere in the image —
    // decision 1764). It is owed to **addons**, which is reason enough — an event the reference
    // fires and we do not is a hole an addon falls into silently.
    if trade.take_request_cancel() {
        script.fire_event("TRADE_REQUEST_CANCEL", vec![]);
    }
    if opened {
        // `SetTradePartner 0x4bf4e0`'s open leg: coins held on the cursor fold
        // into the offer before anything else — the one leg that fires the two money events
        // locally, the send following through the money drain below.
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

    // The accept glow: fire on any change while open (the reference `TRADE_ACCEPT_UPDATE(my, his)`).
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

    // Our accepted gold echoed to the money input via a dedicated PLAYER_TRADE_MONEY (the reference's
    // own event, ref TradePlayerInputMoneyFrame l.565) — kept off the TRADE_UPDATE repaint so it never
    // overwrites what the player is mid-typing; the input's diff-guarded SetCopper reflects the value.
    let player_gold = trade.our.gold;
    if trade.is_open() && player_gold != *last_player_gold {
        script.fire_event("PLAYER_TRADE_MONEY", vec![]);
    }
    *last_player_gold = if trade.is_open() { player_gold } else { 0 };
    // The partner's gold: TRADE_MONEY_CHANGED, the event the stock money kit's TARGET_TRADE
    // frame repaints on (MoneyFrame.lua's OnEvent pairs the two exactly this way — ours on
    // PLAYER_TRADE_MONEY, theirs on TRADE_MONEY_CHANGED; the reference's fire site `0x4bf4d6`
    // sits beside PLAYER_TRADE_MONEY's `0x4bf4ab` in the status handler). Which side each site
    // reads is INFERRED from the consumer; flagged (1962).
    let their_gold = trade.their.gold;
    if trade.is_open() && their_gold != *last_their_gold {
        script.fire_event("TRADE_MONEY_CHANGED", vec![]);
    }
    *last_their_gold = if trade.is_open() { their_gold } else { 0 };

    *last = fresh;
    *last_open = trade.is_open();
}

/// Drain the Lua intents into the trade `CMSG`s (decision 0592 P1): `InitiateTrade(unit)` → resolve
/// the token → player guid → `CMSG_INITIATE_TRADE` (recording the target so `OPEN_WINDOW` can name
/// it); `AcceptTrade`/`CancelTradeAccept` → the accept/un-accept verbs (with the optimistic local
/// glow); `CloseTrade` → `CMSG_CANCEL_TRADE` + a local clear.
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
    // The self player's descriptor store (our bag contents) — for the optimistic own-item resolve.
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

    // The accept verbs only act on an open window (a stray click otherwise is dropped); close is
    // idempotent (honored even with no session, to clear a stale one).
    if script.take_trade_accept() && trade.is_open() {
        trade.accept();
        let _ = commands.0.send(ClientCommand::AcceptTrade);
    }
    if script.take_trade_unaccept() && trade.is_open() {
        trade.unaccept();
        let _ = commands.0.send(ClientCommand::UnacceptTrade);
    }
    // A money offer (the input's value-changed callback) → CMSG_SET_TRADE_GOLD, only while open; the
    // server echoes our new gold back, which the feed turns into PLAYER_TRADE_MONEY (decision 0592 P2).
    if let Some(copper) = script.take_trade_money() {
        if trade.is_open() {
            trade.set_own_gold(copper); // optimistic — the placer gets no server echo of its own gold
            info!(target: "trade", "set gold: {copper} copper; sending CMSG_SET_TRADE_GOLD");
            let _ = commands.0.send(ClientCommand::SetTradeGold { copper });
        }
    }
    // Item placements: a bag item dropped onto our slot → CMSG_SET_TRADE_ITEM (the cursor's engine-space
    // bag/slot resolved to the wire position); an empty-cursor clear → CMSG_CLEAR_TRADE_ITEM. The UI id
    // is 1-based, the wire slot 0-based. Only while a window is open (decision 0592 P2).
    for (id, bag, slot) in script.take_trade_set_items() {
        if !trade.is_open() {
            continue;
        }
        let Some((wire_bag, wire_slot)) = crate::ui_items::wire_pos(bag, slot) else {
            info!(target: "trade", "set item: slot {id} <- bag {bag}/{slot} has no wire position — dropped");
            continue;
        };
        if let Some(trade_slot) = id.checked_sub(1).and_then(|n| u8::try_from(n).ok()) {
            // Optimistic own display: vmangos echoes the placement only to the partner, so resolve the
            // bag item and fill our own column client-side (decision 0592 P2).
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
            trade.clear_own_item(id); // optimistic — un-reference our own slot client-side
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
    // The TRADE dialog's pair: `BeginTrade` is the empty `0x117`, `CancelTrade`
    // the bare `0x11C` — no teardown of ours, the server's status reply drives the window.
    if script.take_trade_begin() {
        let _ = commands.0.send(ClientCommand::BeginTrade);
    }
    if script.take_trade_cancel() {
        let _ = commands.0.send(ClientCommand::CancelTrade);
    }
}

/// The offer the purse can no longer cover, if any — the coinage-change reflex (`0x5ddf30`,
/// decision 1965): when the purse drops below the standing offer, the client trims the offer to
/// the purse and sends it.
fn trimmed_offer(offer: u32, purse: u32) -> Option<u32> {
    (offer > purse).then_some(purse)
}

/// The coinage-change reflex's trade half: on every change of `PLAYER_FIELD_COINAGE` with a trade
/// open, an offer past the purse is trimmed to it and re-sent as an absolute `CMSG_SET_TRADE_GOLD`
/// (the coin sound and `PLAYER_MONEY` are `sound/money.rs`'s and the aura feed's).
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

/// Resolve a UnitPopup unit token to the player guid to trade with (decision 0592 P1). The
/// resolution is not trade-specific — every UnitPopup verb against another player needs the same
/// token → player-guid step — so it lives in [`crate::ui_unit::player_token_guid`] and inspect
/// shares it. Kept as a named local for this module's own tests.
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

        // The initiator records the target before the window opens.
        s.initiate(0x1234);
        assert_eq!(s.partner, Some(0x1234));
        assert!(!s.is_open());
        // No portrait yet (window not open).
        assert_eq!(s.npc(), None);

        // OPEN_WINDOW → open; the "npc" portrait now points at the partner.
        s.open_window();
        assert!(s.is_open());
        assert_eq!(s.npc(), Some(0x1234));

        // A refusal / cancel closes and clears everything.
        s.close_window();
        assert!(!s.is_open());
        assert_eq!(s.partner, None);
        assert_eq!(s.npc(), None);
    }

    /// An incoming `BEGIN_TRADE` is a *request*, not a trade: nothing is a partner until it is
    /// answered, so the "npc" portrait, the snapshot and the already-in-a-trade refusal leg all
    /// read the same "no trade here" they read before it arrived.
    #[test]
    fn an_incoming_request_is_not_yet_a_trade() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        assert_eq!(s.pending_request(), Some(0xABCD));
        assert_eq!(s.partner, None, "an unanswered request has no partner");
        assert!(!s.is_open(), "BEGIN_TRADE does not itself open the window");
        assert_eq!(s.npc(), None);
    }

    /// Accepting the request promotes the initiator to partner and spends the request — the exact
    /// state the initiator's own side reaches through `initiate`.
    #[test]
    fn accepting_a_request_promotes_the_initiator_to_partner() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        s.begin(0xABCD);
        assert_eq!(s.partner, Some(0xABCD));
        assert_eq!(s.pending_request(), None, "the request is spent");
        assert!(!s.is_open(), "the window waits for OPEN_WINDOW");
    }

    /// Refusing (or any close) clears the request with everything else, so a refusal in flight can
    /// never be answered twice.
    #[test]
    fn closing_clears_a_pending_request() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        s.close_window();
        assert_eq!(s.pending_request(), None);
    }

    /// A `BEGIN_TRADE` arriving over a **live** trade leaves that trade completely alone — and so
    /// does refusing it. This is what makes the "already trading" leg of the ladder readable at
    /// all: it reads the fields a reset would have cleared a line earlier.
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

        // Their side (their_window = true) fills `their`; the offer change resets the accept.
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

        // Our side (their_window = false) fills `our`.
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
        // BACK_TO_TRADE drops both.
        s.back_to_trade();
        assert_eq!((s.our_accept, s.their_accept), (false, false));
    }

    /// Our own offer is tracked client-side (decision 0592 P2): vmangos never echoes a placement to the
    /// placer, so `place_own_item`/`clear_own_item`/`set_own_gold` fill `our` directly, and each resets
    /// both accepts exactly as the server does — the feed then paints `our` as the player column.
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
        // "player" (yourself) never resolves.
        let sel = Selection::default();
        let group = GroupState::default();
        assert_eq!(resolve_trade_target("player", &sel, &group), None);
        // An out-of-range party token resolves to nothing (no roster).
        assert_eq!(resolve_trade_target("party3", &sel, &group), None);
    }

    // A status-code smoke check so the enum stays wired the way the apply arm reads it.
    #[test]
    fn status_codes_are_stable() {
        assert_eq!(TradeStatus::OpenWindow.code(), 2);
        assert_eq!(TradeStatus::Accept.code(), 4);
        assert_eq!(TradeStatus::BackToTrade.code(), 7);
    }

    /// The `drain_trade` **system** end-to-end (decision 0592 P1): a queued `InitiateTrade("target")`
    /// intent plus a player [`Selection`] resolves to the target guid, records the partner, and ships
    /// `CMSG_INITIATE_TRADE`. The isolated pieces (`resolve_trade_target`, the Lua queue) are tested
    /// apart; this is the only test that runs the drain as a real Bevy system through the VM +
    /// `Selection` + `NetCommands` — the exact seam behind the director's "click Trade, nothing
    /// happens" report.
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
                guid: Some(0x7), // a player guid (high 0x0000 → is_player)
            });
        app.insert_non_send_resource(UiScript::new().unwrap());

        // The right-click menu queued this Lua-side (the menu hit-path test proves the click gets
        // this far); the drain is the untested downstream half.
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

    /// Build an app that can run [`feed_trade`] as a real system with a live VM — the seam the
    /// two lines below need, since both are resolved (not raised) by the feed.
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

    /// **`TRADE_REQUEST_CANCEL` fires, and only on `CANCELED`.** It is the one Lua event the
    /// reference's 23-case status dispatcher signals (`0x4bf832`, the sole `SignalEvent` inside
    /// `[0x4bf720, 0x4bfa08)`), and benilla fired it from nowhere: 1764 noticed the gap and left
    /// it, and the stock `UIParent.lua` has listened for it ever since without anything to hear.
    ///
    /// Nothing stock *acts* on it (it hides the `TRADE` StaticPopup, which can never be up — that
    /// dialog is dead code in the reference too), so the observable is the event itself, which is
    /// exactly what an addon sees.
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

        // A plain close — what `COMPLETE` and `CLOSE_WINDOW` do — signals nothing. (Which
        // statuses reach which of these two is `ui_trade::net`'s own test; this is the half
        // that turns a signal into an event.)
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

    /// **The outgoing line.** "You have requested to trade with %s." is printed by the reference
    /// from inside its own `CMSG_INITIATE_TRADE` sender (`0x5d4042` on a name-cache hit,
    /// `0x5d40ca` from the name-query callback on a miss), not from anything the
    /// server says back. benilla sent the packet in silence, so a trade that the server then
    /// refused produced no text at all, in either direction.
    ///
    /// Row `0xb9` is `kind = 0`, a **system chat line** — asserted off the catalog rather than
    /// written here, because the surface is the record's to name.
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

    /// **A `%s` status line is dropped when the player it names is no longer a streamed object**
    /// — the status arms' second guard (`0x468460(guid, TYPEMASK_PLAYER)`), which they have
    /// because they read the name off the live `CGUnit` and print nothing when there is not one.
    ///
    /// benilla's `NameCache` would happily serve a name for somebody who has despawned, so
    /// without the guard the line would still go out — naming a player who is gone. The
    /// *initiate* line is the deliberate opposite (it goes through the reference's name cache,
    /// which has no such test) and the second half of this test holds that difference in place:
    /// same session, same absent object, one line dropped and one still owed.
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

    /// A target whose name is not cached yet **waits** — the reference's deferred arm, which
    /// prints from the name-query callback. The line must not go out with a hole in it, and it
    /// must not be dropped: a refusal arriving first does not cancel it, because the reference
    /// hangs its callback off the name query and not off the trade.
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

    /// A request that survives every leg is **accepted unasked** — the bottom of the reference's
    /// own ladder, and the behaviour benilla briefly replaced with a consent dialog before taking
    /// it back out. The 5875 client never asks: it registers `TRADE_REQUEST` and
    /// signals it from nowhere.
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

    /// **Block Trades** refuses without asking, and it is the one refusal that speaks: the
    /// `ERR_TRADE_BLOCKED_S` line naming the initiator (`0x496720(0xbb, …)`). Catalog row `0xbb`
    /// is `kind = 0` — the **chat frame**, chat type `0xa` — so this is a system chat line and
    /// **not** the red `UI_ERROR_MESSAGE`, which is `kind = 2`.
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
        // …and the row it names is a CHAT row, which is the half a re-implementation gets wrong:
        // `DisplayError` is modelled elsewhere in benilla as the red toast, and is not one here.
        assert_eq!(
            benilla_ui::messages::kind_of("ERR_TRADE_BLOCKED_S"),
            benilla_ui::messages::MsgKind::Chat,
        );
    }

    /// Legs 5 and 6 — a cinematic (decision 1725's named hole) and lost player control — refuse
    /// with **no** line. Only leg 8 reaches the message, which is why the ladder's order is
    /// behaviour and not taste.
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

    /// Leg 7 — an open auction house refuses the request. `[0xb725f8]`/`[0xb725fc]` is the open
    /// auctioneer's guid, which is easy to mislabel as "a trade is already pending". benilla was
    /// one commit from building that wrong leg.
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

    /// Leg 2 — an ignored initiator is refused on a **different opcode**, silently. The server has
    /// no ignore check for trade at all (vmangos's `HandleInitiateTradeOpcode` reads no social
    /// list), so this refusal is entirely the client's; `IGNORE_TRADE` rather than `BUSY_TRADE` is
    /// what makes the initiator read "is ignoring you" instead of "is busy".
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

    /// Leg 4 — a dead or ghost initiator is refused as busy. The predicate is the reference's
    /// `0x605f30`: **raw** health ≤ 0, or the `PLAYER_FLAGS` ghost bit. Deliberately not
    /// `unit_reads_dead()` (`UnitIsDead 0x517ac0`), which also takes the dead-looking dynflag and
    /// would refuse a feigning hunter the reference trades with quite happily.
    #[test]
    fn a_dead_or_ghost_initiator_is_refused() {
        // `UNIT_FIELD_HEALTH` and `PLAYER_FLAGS`, spelled here the way `ui_unit`'s tests spell
        // their field ids — the protocol crate keeps them private.
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

    /// Legs 1 and 3 — the two that answer with **nothing at all**: our own initiate in flight
    /// (`[0xc4bec8]`), and an initiator that resolves to no streamed player object. Both drop the
    /// request without a packet, which is the reference's behaviour and not an oversight — it has
    /// nothing to say to a trade it cannot see. Leg 1 is also what replaced an "already trading"
    /// leg: the latch is set by the initiate sender and never by
    /// `OPEN_WINDOW`, so it is "we asked somebody", not "a window is up".
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

    /// The harness for [`answer_trade_request`]: an app carrying the eight pieces of state the
    /// ladder reads, and two streamed player objects to be asked by. **No VM** — the system needs
    /// none, which is the shape it settled into once the consent dialog came back out (decision
    /// 1764): the answer is wire policy over engine state, and the only reason it is not in the
    /// net drain is the parameter ceiling there.
    fn request_app(
        block_trades: bool,
        cinematic: bool,
        controlled: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut names = NameCache::default();
        // Two names, because several of these tests need a SECOND asker — and an unresolved name
        // parks leg 8 before it reaches the line, which would let a test pass for the wrong
        // reason (it did, once).
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
        // The two askers are streamed player objects — leg 3 drops a request from a guid that
        // resolves to nothing, so every test that expects to reach the ladder needs them present.
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

    /// Draining a `SetTradeMoney` offer ships `CMSG_SET_TRADE_GOLD` — but only while the window is
    /// open, so a stray value with no trade is dropped (decision 0592 P2).
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
        vm.set_money(20_000); // the engine's purse gate on SetTradeMoney (1965)
        app.insert_non_send_resource(vm);

        // No window yet → the offer is dropped.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("SetTradeMoney(500)")
            .unwrap();
        app.world_mut().run_system_once(drain_trade).unwrap();
        assert!(rx.try_recv().is_err(), "no CMSG with no open trade");

        // Open the window, then a money offer ships the gold CMSG.
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

    /// Draining an item CLEAR (an empty-cursor click on our filled slot) ships `CMSG_CLEAR_TRADE_ITEM`
    /// with the 1-based UI slot mapped to the 0-based wire slot (decision 0592 P2). The SET path's
    /// wire-position mapping rides `ui_items::wire_pos` (tested there) over the `(id, bag, slot)` intent
    /// the script test proves; here we drive the cursor-free clear end to end.
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

        // A pushed trade state with our slot 1 filled, then an empty-cursor click clears it.
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
