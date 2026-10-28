//! The group loot rolls: `SMSG_LOOT_START_ROLL`, `SMSG_LOOT_ROLL`, `SMSG_LOOT_ROLL_WON` and
//! `SMSG_LOOT_ALL_PASSED` become the stock `GroupLootFrame`'s events and the roll chat lines, and
//! its `RollOnLoot` votes go back out as `CMSG_LOOT_ROLL`. The getters read the list as they run
//! ([`live`]), and every signal goes on the rolls' lane where the reference raises it
//! ([`lane`]): in the packet's handler, or in the item or name answer's that runs the callback a
//! lookup registered.
//!
//! A fresh roll's `START_LOOT_ROLL` waits for its item template, as in the reference: `0x61b310`
//! ends in the cache lookup `0x55ba30`, whose hit fires the event at once (`0x61b430`) and whose
//! miss fires it from the arrival callback `0x61b460`. `GroupLootFrame` paints once, from `OnShow`,
//! so an event ahead of the template paints a blank frame. Deviation: a negative answer, or none
//! within [`TEMPLATE_HOLD_MAX_MS`], opens the frame unresolved, because a frame that never opens
//! cannot even be passed on; the reference's callback fires on success only.
//!
//! At most [`MAX_OPEN`] rolls are open at once (`0x61b430` tests `[0xc4dda8]` against 4). A roll
//! whose template is in finds the slots full and waits, deferred, in arrival order; every close
//! (our vote `0x61bf46`, a win `0x61bc6b`, all-passed `0x61b74c`) walks the list oldest first
//! (`0x61b310` links each record at the tail, `0x61b384`) and opens deferred rolls while a slot is
//! free (`0x61b760`). Stock `GroupLootFrame_OpenNewFrame` has four frames and drops a fifth
//! silently, so an unqueued fifth could never be voted.
//!
//! Deviation: the open count is the number of rolls announced and not closed. The reference keeps
//! a counter that its server-close paths decrement for any roll whose closed byte is clear
//! (`0x61b6f8`, `0x61bc17`), a deferred roll that never counted up included, so it under-counts
//! and lets a fifth frame open; here a deferred roll's close changes nothing.
//!
//! A result's line, its `CANCEL_LOOT_ROLL` and the promotion after it wait for the result's item
//! record and, for a win, the winner's name (`0x61b96a` → `0x61b9a0` → `0x61b9bf` → `0x61b9e0`,
//! `0x61b5c0` → `0x61b640`); until then the roll stays listed. Deviation: a refused record or name
//! closes the roll unspoken. In 1.12.1 the failure callbacks (`0x61bc80`, `0x61bd50`, `0x61b7d0`)
//! free the record with no `CANCEL_LOOT_ROLL` and leave the open count unlowered, so its frame
//! stays up, empty and dead, until a `/reload`, and holds one of the four frames, so a later
//! fourth roll never shows; benilla closes the roll instead.
//!
//! A promoted roll's `arg2` is still the wire's countdown (`0x61b430` and `0x61b79a` push
//! `node+0x3c`, which nothing rewrites); only the bar is short, as `GetLootRollTimeLeft` counts
//! from the packet's arrival.
//!
//! `rollID` is a client-local serial; the wire addresses a roll by `(lootedTarget, itemSlot)`. Our
//! own vote closes our frame at once, as `RollOnLoot` (`0x61bdf0`) sends the CMSG and fires
//! `CANCEL_LOOT_ROLL` in one call; the closed roll stays listed until its result. On a
//! bind-on-pickup roll, Need or Greed only raises `CONFIRM_LOOT_ROLL` and `ConfirmLootRoll` sends,
//! the reference's C `RollOnLoot` gate; Pass is never gated.

use std::collections::VecDeque;
use std::time::Duration;

use benilla_protocol::messages::{roll_vote, LootAllPassed, LootRoll, LootRollWon, LootStartRoll};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::LootRollEntry;

use crate::entities::ItemDisplays;
use crate::items::{Items, RecordCallback, RollCatalogs};
use crate::lanes::{LaneApp, Stamp};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::Ask;

/// How long a fresh roll waits for its item template before `START_LOOT_ROLL` fires anyway: wall
/// clock, not frames, as the wait comes out of the roll's own minute.
const TEMPLATE_HOLD_MAX_MS: u64 = 2_000;

/// `item_template.bonding` bind on pickup (vmangos `ItemPrototype.h` `ItemBondingType`); only 1
/// gets `GroupLootFrame_OnShow`'s gold backdrop.
const BIND_WHEN_PICKED_UP: u32 = 1;

/// Rolls open at once (`0x61b435`, `0x61b765`); a fifth waits for a close.
const MAX_OPEN: usize = 4;

/// Where a roll stands between its packet and its frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// Its item template is not in, so `0x61b430` has not run on it: the lookup's callback
    /// `0x61b460` waits under the serial.
    Waiting(u32),
    /// `0x61b430` found four open: the node's queued byte `+0x39`, the oldest promoted first.
    Deferred,
    /// `START_LOOT_ROLL` has gone onto the lane.
    Open,
}

/// One group loot roll the engine lists (`0xc4dc00`).
struct ActiveRoll {
    /// The id Lua addresses the roll by (`+0x8`).
    roll_id: u32,
    looted_target: u64,
    item_slot: u32,
    item_id: u32,
    /// The drop's random-suffix roll: the reference's `SetLootRollItem` (`0x5364a0`) copies it
    /// into the tooltip's `+0x424` with no item object, so it is the hover's only enchant source.
    random_property_id: u32,
    /// The wire's `countdownTime`, kept whole: `START_LOOT_ROLL`'s `arg2` (`+0x3c`).
    countdown_ms: u32,
    /// The clock at the packet; the expiry `+0x28` is it plus the countdown (`0x61b404`).
    arrived: Duration,
    phase: Phase,
    /// The closed byte `+0x3a`, which our vote sets (`0x61bf32`); the roll stays listed.
    closed: bool,
}

impl ActiveRoll {
    /// Counted at `[0xc4dda8]`: announced and not closed.
    fn counted(&self) -> bool {
        self.phase == Phase::Open && !self.closed
    }

    /// The roll as `GetLootRollItemInfo`, `GetLootRollItemLink` and `GetLootRollTimeLeft` read it
    /// at the call (`0x4c3050`, `0x4c31f0`, `0x4c32d0`): the item looked up with no callback
    /// (`0x4c30d2`), its fields `None` or `false` until the template answers, and the expiry less
    /// the clock (`0x4c331c`).
    fn entry(
        &self,
        items: &Items,
        icons: Option<&ItemDisplays>,
        catalogs: RollCatalogs,
        now: Duration,
    ) -> LootRollEntry {
        let t = items.template(self.item_id, 0, Ask::Read);
        let texture = t
            .and_then(|t| icons?.catalog.get(t.display_info_id))
            .and_then(|d| d.icon.clone());
        let expires = self.arrived + Duration::from_millis(u64::from(self.countdown_ms));
        LootRollEntry {
            roll_id: self.roll_id,
            name: t.map(|t| catalogs.name(&t.name, self.random_property_id)),
            texture,
            quality: t.map(|t| t.quality),
            bind_on_pickup: t.is_some_and(|t| t.bonding == BIND_WHEN_PICKED_UP),
            // The 32-bit difference, unclamped: a past expiry wraps to about 4.29e9, which the
            // stock bar reads as its minimum (`LootFrame.lua:290-293`).
            time_left_ms: (expires.as_millis() as u32).wrapping_sub(now.as_millis() as u32),
            item_id: self.item_id,
            random_property_id: self.random_property_id,
            // `GetLootRollItemLink`, the chat lines' link; `None` until the template answers.
            link: t.map(|t| {
                crate::ui_items::item_link_full(
                    self.item_id,
                    0,
                    self.random_property_id,
                    0,
                    &catalogs.name(&t.name, self.random_property_id),
                    t.quality,
                )
            }),
            closed: self.closed,
        }
    }
}

/// Which announcement a line renders.
#[derive(Clone, Copy, Debug)]
enum RollLine {
    Announce(LootRoll),
    Won(LootRollWon),
    AllPassed(LootAllPassed),
}

impl RollLine {
    /// The line's item: every line embeds its link, so its record comes first.
    fn item_id(&self) -> u32 {
        match self {
            Self::Announce(p) => p.item_id,
            Self::Won(p) => p.item_id,
            Self::AllPassed(p) => p.item_id,
        }
    }

    /// The roller or the winner, whose name a line asks after its record.
    fn who(&self) -> Option<u64> {
        match self {
            Self::Announce(p) => Some(p.roller),
            Self::Won(p) => Some(p.winner),
            Self::AllPassed(_) => None,
        }
    }

    /// The roll a result closes once its line is shown.
    fn closes(&self) -> Option<(u64, u32)> {
        match self {
            Self::Announce(_) => None,
            Self::Won(p) => Some((p.looted_target, p.item_slot)),
            Self::AllPassed(p) => Some((p.looted_target, p.item_slot)),
        }
    }
}

/// What a line's lookup waits on: the item record's callback under its serial, or the name's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineWait {
    Item(u32),
    Name(u64),
}

/// One announcement whose record or name is in flight.
struct PendingLine {
    line: RollLine,
    wait: LineWait,
}

/// A line composed where its last lookup landed, its text read from the strings as it fires.
#[derive(Clone, Debug)]
pub(crate) struct ReadyLine {
    line: RollLine,
    name: Option<String>,
    is_self: bool,
    item: RollItem,
    /// `showLootSpam` as the composer read it (`0x61ba3a`, `0x61bafe`).
    detailed: bool,
}

/// What a lane item signals.
#[derive(Clone, Debug)]
pub(crate) enum RollEvent {
    /// `START_LOOT_ROLL(rollID, countdown)` (`0x61b452`, `0x61b7ac`).
    Start { roll_id: u32, countdown_ms: u32 },
    /// `CANCEL_LOOT_ROLL(rollID)` (`0x61b2c7`, `0x61b6f3`, `0x61bc12`).
    Cancel(u32),
    /// A roll line, added to chat as type 24 (`0x61c38c`, `0x61bbf8`, `0x61b6d9`).
    Line(ReadyLine),
}

/// Every group loot roll the engine lists, the lines whose lookups are in flight, and the rolls'
/// lane; cleared on disconnect.
#[derive(Resource, Default)]
pub(crate) struct LootRolls {
    /// Oldest first, as the list walks (`0x61b2a0`, `0x61b780`).
    active: Vec<ActiveRoll>,
    /// Never reused within a session, so a late packet for a closed roll cannot hit a fresh one.
    next_id: u32,
    /// The serial of the next item-record wait ([`RecordCallback::LootRoll`]).
    next_wait: u32,
    pending: Vec<PendingLine>,
    /// The lane: each signal stamped where it was raised.
    events: VecDeque<(Stamp, RollEvent)>,
}

/// What a roll's or a line's lookups read and ask through.
pub(crate) struct RollLookups<'a> {
    pub(crate) items: &'a Items,
    pub(crate) names: &'a NameCache,
    pub(crate) net: &'a NetCommands,
    pub(crate) self_guid: Option<u64>,
    pub(crate) catalogs: RollCatalogs<'a>,
    /// `showLootSpam` (`0xb4e2bc`), read as each line is composed (`0x61c0b9`, `0x61ba3a`,
    /// `0x61bafe`).
    pub(crate) detailed: bool,
}

impl LootRolls {
    /// Queue `event` stamped now, behind everything stamped before it.
    fn push(&mut self, event: RollEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    fn wait_serial(&mut self) -> u32 {
        self.next_wait = self.next_wait.wrapping_add(1);
        self.next_wait
    }

    /// `SMSG_LOOT_START_ROLL` (`0x61b310`): list the roll under a fresh id, then look its item up
    /// with the callback `0x61b460` (`0x61b40e`), a hit opening it at once (`0x61b41c`). A duplicate
    /// `(looted_target, item_slot)` is ignored, as vmangos can re-send every active roll on
    /// reconnect (`Group::SendLootStartRollsForPlayer`, under `SendLootRollUponReconnect`).
    pub(crate) fn start(&mut self, p: LootStartRoll, now: Duration, look: &RollLookups) {
        if self
            .active
            .iter()
            .any(|r| r.looted_target == p.looted_target && r.item_slot == p.item_slot)
        {
            debug!(
                "ui_loot_roll: duplicate start for {:#x} slot {} — ignored",
                p.looted_target, p.item_slot
            );
            return;
        }
        self.next_id += 1;
        let wait = self.wait_serial();
        self.active.push(ActiveRoll {
            roll_id: self.next_id,
            looted_target: p.looted_target,
            item_slot: p.item_slot,
            item_id: p.item_id,
            random_property_id: p.random_property_id,
            countdown_ms: p.countdown_ms,
            arrived: now,
            phase: Phase::Waiting(wait),
            closed: false,
        });
        if look
            .items
            .template(p.item_id, 0, Ask::Event(look.net))
            .is_some()
        {
            self.reach(self.active.len() - 1);
        } else {
            look.items.wait(p.item_id, RecordCallback::LootRoll(wait));
        }
    }

    /// Rolls counted open (`[0xc4dda8]`).
    fn open_count(&self) -> usize {
        self.active.iter().filter(|r| r.counted()).count()
    }

    /// `0x61b430` on roll `i`: a free slot opens it with its `START_LOOT_ROLL`, else it is
    /// deferred (`0x61b45b`).
    fn reach(&mut self, i: usize) {
        if self.open_count() < MAX_OPEN {
            self.open(i);
        } else {
            self.active[i].phase = Phase::Deferred;
        }
    }

    fn open(&mut self, i: usize) {
        let r = &mut self.active[i];
        r.phase = Phase::Open;
        let (roll_id, countdown_ms) = (r.roll_id, r.countdown_ms);
        debug!("ui_loot_roll: roll {roll_id} opened ({countdown_ms} ms)");
        self.push(RollEvent::Start {
            roll_id,
            countdown_ms,
        });
    }

    /// `0x61b760`: while a slot is free, open the oldest deferred roll. Run after every close.
    fn promote(&mut self) {
        for i in 0..self.active.len() {
            if self.open_count() >= MAX_OPEN {
                return;
            }
            if self.active[i].phase == Phase::Deferred {
                self.open(i);
            }
        }
    }

    /// The module doc's deviation: every roll still waiting on its template past the hold opens
    /// unresolved.
    fn expire_holds(&mut self, now: Duration) {
        for i in 0..self.active.len() {
            let r = &self.active[i];
            if matches!(r.phase, Phase::Waiting(_))
                && now.saturating_sub(r.arrived) >= Duration::from_millis(TEMPLATE_HOLD_MAX_MS)
            {
                debug!(
                    "ui_loot_roll: roll {} opening unresolved (template hold expired)",
                    r.roll_id
                );
                self.reach(i);
            }
        }
    }

    /// `SMSG_LOOT_ROLL` (`0x61bfa0`): the line, its record and the roller's name looked up first.
    pub(crate) fn announce(&mut self, p: LootRoll, look: &RollLookups) {
        self.look_up(RollLine::Announce(p), look);
    }

    /// `SMSG_LOOT_ROLL_WON` (`0x61b870`): the line, then the roll's close.
    pub(crate) fn won(&mut self, p: LootRollWon, look: &RollLookups) {
        self.look_up(RollLine::Won(p), look);
    }

    /// `SMSG_LOOT_ALL_PASSED` (`0x61b500`): the line, then the roll's close; the item stays
    /// lootable.
    pub(crate) fn all_passed(&mut self, p: LootAllPassed, look: &RollLookups) {
        self.look_up(RollLine::AllPassed(p), look);
    }

    /// A line's record lookup with its callback (`0x61c047`, `0x61b96a`, `0x61b5c0`): a hit goes
    /// on to the name, a miss waits for the answer.
    fn look_up(&mut self, line: RollLine, look: &RollLookups) {
        let entry = line.item_id();
        if look
            .items
            .template(entry, 0, Ask::Event(look.net))
            .is_some()
        {
            self.name_stage(line, look);
        } else {
            let wait = self.wait_serial();
            look.items.wait(entry, RecordCallback::LootRoll(wait));
            self.pending.push(PendingLine {
                line,
                wait: LineWait::Item(wait),
            });
        }
    }

    /// The name lookup with its callback (`0x61c08e`, `0x61b9bf`): every third-person line needs a
    /// name, and so does our own dice line (no self form); all-passed none.
    fn name_stage(&mut self, line: RollLine, look: &RollLookups) {
        let is_self = line.who().is_some() && line.who() == look.self_guid;
        let needs_name = match &line {
            RollLine::Announce(p) => is_dice(p) || !is_self,
            RollLine::Won(_) => !is_self,
            RollLine::AllPassed(_) => false,
        };
        match line.who() {
            Some(guid) if needs_name => {
                match look.names.resolve_player(guid, Ask::Event(look.net)) {
                    Some(name) => {
                        let name = name.to_string();
                        self.show(line, Some(name), look);
                    }
                    None => self.pending.push(PendingLine {
                        line,
                        wait: LineWait::Name(guid),
                    }),
                }
            }
            _ => self.show(line, None, look),
        }
    }

    /// The line's composer (`0x61c0b0`, `0x61b9e0`, `0x61b640`): its line onto the lane, then a
    /// result's close. With `showLootSpam` at 0 a vote or dice line is dropped (`0x61c0b9`).
    fn show(&mut self, line: RollLine, name: Option<String>, look: &RollLookups) {
        let Some(t) = look.items.template(line.item_id(), 0, Ask::Read) else {
            return self.refuse(line);
        };
        let (random_property_id, suffix_factor) = match &line {
            RollLine::Announce(p) => (p.random_property_id, p.suffix_factor),
            RollLine::Won(p) => (p.random_property_id, p.suffix_factor),
            RollLine::AllPassed(p) => (p.random_property_id, p.suffix_factor),
        };
        // The link names the rolled drop, suffix included, as the roll frame does.
        let item = RollItem {
            quality: t.quality,
            item_id: line.item_id(),
            random_property_id,
            suffix_factor,
            name: look.catalogs.name(&t.name, random_property_id),
        };
        if look.detailed || !matches!(line, RollLine::Announce(_)) {
            self.push(RollEvent::Line(ReadyLine {
                line,
                name,
                is_self: line.who().is_some() && line.who() == look.self_guid,
                item,
                detailed: look.detailed,
            }));
        }
        if let Some((guid, slot)) = line.closes() {
            self.close(guid, slot);
        }
    }

    /// A refused record or name: a vote or dice line is dropped, and a result closes its roll
    /// unspoken (the module doc's deviation).
    fn refuse(&mut self, line: RollLine) {
        if let Some((guid, slot)) = line.closes() {
            self.close(guid, slot);
        }
    }

    /// An item record's answer for the wait `wait`, found or refused: a waiting roll reaches
    /// `0x61b430` (`0x61b4ee`; a refusal too, the module doc's deviation), a line goes on to its
    /// name (`0x61c4b0` → `0x61c070`, `0x61bd50` → `0x61b9a0`) or is shown (`0x61b7d0` →
    /// `0x61b640`).
    pub(crate) fn item_answered(&mut self, wait: u32, found: bool, look: &RollLookups) {
        if let Some(i) = self
            .active
            .iter()
            .position(|r| r.phase == Phase::Waiting(wait))
        {
            if !found {
                debug!(
                    "ui_loot_roll: roll {} opening unresolved (the server does not know the entry)",
                    self.active[i].roll_id
                );
            }
            return self.reach(i);
        }
        let Some(i) = self
            .pending
            .iter()
            .position(|p| p.wait == LineWait::Item(wait))
        else {
            return;
        };
        let line = self.pending.remove(i).line;
        if found {
            self.name_stage(line, look);
        } else {
            self.refuse(line);
        }
    }

    /// The player-name answer for `guid`, an empty name a refusal: the lines waiting on it, in the
    /// order they asked, shown (`0x61c3e0`, `0x61bc80`).
    pub(crate) fn name_answered(&mut self, guid: u64, name: &str, look: &RollLookups) {
        while let Some(i) = self
            .pending
            .iter()
            .position(|p| p.wait == LineWait::Name(guid))
        {
            let line = self.pending.remove(i).line;
            if name.is_empty() {
                self.refuse(line);
            } else {
                self.show(line, Some(name.to_string()), look);
            }
        }
    }

    /// The result's close (`0x61b6de`-`0x61b74c`, `0x61bbfd`-`0x61bc6b`): a listed roll not yet
    /// closed signals its `CANCEL_LOOT_ROLL`, announced or not, as the closed byte `+0x3a` alone
    /// gates it; the roll is freed, and the oldest deferred roll promoted.
    fn close(&mut self, looted_target: u64, item_slot: u32) {
        let Some(i) = self
            .active
            .iter()
            .position(|r| r.looted_target == looted_target && r.item_slot == item_slot)
        else {
            return;
        };
        let r = self.active.remove(i);
        if !r.closed {
            debug!("ui_loot_roll: roll {} cancelled", r.roll_id);
            self.push(RollEvent::Cancel(r.roll_id));
        }
        self.promote();
    }

    /// Whether roll `roll_id` is listed and not closed.
    #[cfg(test)]
    pub(crate) fn holds(&self, roll_id: u32) -> bool {
        self.active
            .iter()
            .any(|r| r.roll_id == roll_id && !r.closed)
    }

    /// The listed, unclosed roll `roll_id`'s wire identity, the vote's send (`0x61bede`-
    /// `0x61bef1`); `None` for a stale click (`0x61be3f`).
    fn vote(&self, roll_id: u32) -> Option<(u64, u32)> {
        self.active
            .iter()
            .find(|r| r.roll_id == roll_id && !r.closed)
            .map(|r| (r.looted_target, r.item_slot))
    }

    /// The vote's tail after its `CANCEL_LOOT_ROLL`: the closed byte (`0x61bf32`), and the oldest
    /// deferred roll promoted (`0x61bf46`); the roll stays listed.
    fn close_voted(&mut self, roll_id: u32) {
        if let Some(r) = self
            .active
            .iter_mut()
            .find(|r| r.roll_id == roll_id && !r.closed)
        {
            r.closed = true;
            self.promote();
        }
    }

    /// `CancelAllLootRolls` (`0x61b290`), then `FreeAllLootRolls` (`0x61b240`): every roll neither
    /// deferred (the node's queued byte `+0x39`) nor closed signals its `CANCEL_LOOT_ROLL`, oldest
    /// first, announced or not; then every roll is gone and the open count is zero.
    pub(crate) fn cancel_all(&mut self) {
        for r in std::mem::take(&mut self.active) {
            if r.phase != Phase::Deferred && !r.closed {
                self.push(RollEvent::Cancel(r.roll_id));
            }
        }
    }

    /// Session teardown: no `CANCEL_LOOT_ROLL`, as the UI goes down with the session.
    pub(crate) fn clear(&mut self) {
        self.active.clear();
        self.pending.clear();
        self.events.clear();
    }
}

/// The group rolls' packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{LootRolls, RollReads};
    use crate::net::NetHandlerApp;

    /// Register the roll handlers, called from [`super::UiLootRollPlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::LootStartRoll, on_packet)
            .net_handler(K::LootRoll, on_packet)
            .net_handler(K::LootRollWon, on_packet)
            .net_handler(K::LootAllPassed, on_packet)
            .net_handler(K::PlayerName, on_player_name)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_packet(
        In(ev): In<SessionEvent>,
        mut rolls: ResMut<LootRolls>,
        loot: Option<ResMut<crate::ui_loot::LootState>>,
        reads: RollReads,
    ) {
        let look = reads.lookups();
        match ev {
            SessionEvent::LootStartRoll(p) => {
                debug!(
                    "net: loot roll opened on item {} ({:#x} slot {}), {} ms",
                    p.item_id, p.looted_target, p.item_slot, p.countdown_ms
                );
                rolls.start(p, reads.time.elapsed(), &look);
            }
            SessionEvent::LootRoll(p) => {
                debug!(
                    "net: loot roll announce — roller {:#x} number {} type {}",
                    p.roller, p.roll_number, p.roll_type
                );
                rolls.announce(p, &look);
            }
            SessionEvent::LootRollWon(p) => {
                debug!(
                    "net: loot roll won by {:#x} with {} (type {})",
                    p.winner, p.roll_number, p.roll_type
                );
                rolls.won(p, &look);
                resolved(loot, p.looted_target, p.item_slot);
            }
            SessionEvent::LootAllPassed(p) => {
                debug!("net: loot roll — everyone passed on item {}", p.item_id);
                rolls.all_passed(p, &look);
                resolved(loot, p.looted_target, p.item_slot);
            }
            _ => {}
        }
    }

    /// The window's row stops waiting on the roll (`ClearLootRollState 0x5ebcf0`), after the
    /// result's line when its record was in (`0x61b987`, `0x61b5dd`).
    fn resolved(loot: Option<ResMut<crate::ui_loot::LootState>>, guid: u64, slot: u32) {
        if let (Some(mut loot), Ok(slot)) = (loot, u8::try_from(slot)) {
            loot.roll_resolved(guid, slot);
        }
    }

    /// The name answer runs the line lookups' callbacks waiting on the guid (`0x61c3e0`,
    /// `0x61bc80`); an empty name is the answer for an unknown guid.
    fn on_player_name(In(ev): In<SessionEvent>, mut rolls: ResMut<LootRolls>, reads: RollReads) {
        if let SessionEvent::PlayerName { guid, name, .. } = ev {
            rolls.name_answered(guid, &name, &reads.lookups());
        }
    }

    fn on_session_end(In(_): In<SessionEvent>, mut rolls: ResMut<LootRolls>) {
        rolls.clear();
    }
}

/// What the roll handlers look up through, and the clock a start reads.
#[derive(SystemParam)]
pub(crate) struct RollReads<'w> {
    items: Res<'w, Items>,
    names: Res<'w, NameCache>,
    net: Res<'w, NetCommands>,
    self_guid: Res<'w, crate::net::SelfGuid>,
    config: Option<Res<'w, crate::ui_loot::LootConfig>>,
    catalogs: crate::items::Rolls<'w>,
    time: Res<'w, Time>,
}

impl RollReads<'_> {
    fn lookups(&self) -> RollLookups<'_> {
        RollLookups {
            items: &self.items,
            names: &self.names,
            net: &self.net,
            self_guid: self.self_guid.0,
            catalogs: self.catalogs.get(),
            detailed: self.config.as_ref().is_none_or(|c| c.show_loot_spam),
        }
    }
}

/// The rolls' half of the item answer's callbacks ([`crate::net::objects`]): the answer's handler
/// holds the records, the names and the socket.
#[derive(SystemParam)]
pub(crate) struct RollRecordAnswers<'w> {
    rolls: Option<ResMut<'w, LootRolls>>,
    config: Option<Res<'w, crate::ui_loot::LootConfig>>,
    catalogs: crate::items::Rolls<'w>,
}

impl RollRecordAnswers<'_> {
    /// [`LootRolls::item_answered`].
    pub(crate) fn item_answered(
        &mut self,
        wait: u32,
        found: bool,
        items: &Items,
        names: &NameCache,
        net: &NetCommands,
        self_guid: Option<u64>,
    ) {
        let Some(rolls) = self.rolls.as_mut() else {
            return;
        };
        let look = RollLookups {
            items,
            names,
            net,
            self_guid,
            catalogs: self.catalogs.get(),
            detailed: self.config.as_ref().is_none_or(|c| c.show_loot_spam),
        };
        rolls.item_answered(wait, found, &look);
    }
}

pub(crate) struct UiLootRollPlugin;

impl Plugin for UiLootRollPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<LootRolls>()
            .add_lane(
                "loot rolls",
                |world| world.get_resource::<LootRolls>()?.head(),
                lane::fire_rolls,
                None,
            )
            .add_systems(
                Update,
                lane::drop_roll_events
                    .in_set(crate::ui_script::UiFeed)
                    .run_if(not(crate::ui_script::ingame_ui_up)),
            )
            .add_systems(
                PreUpdate,
                (
                    // The world load's call at `0x401c2c`; the lane fires the cancels.
                    cancel_rolls_on_worldport
                        .in_set(crate::net::WorldportTimer::Rolls)
                        .run_if(crate::net::world_ported),
                    // Ahead of the frame's packets: a deadline is no packet's.
                    expire_roll_holds,
                ),
            );
    }
}

/// The module doc's deviation: a template the server has not answered within the hold opens its
/// roll on the wall clock, at the frame's start, ahead of its packets: a deadline is no packet's.
fn expire_roll_holds(mut rolls: ResMut<LootRolls>, time: Res<Time>) {
    if rolls
        .active
        .iter()
        .any(|r| matches!(r.phase, Phase::Waiting(_)))
    {
        rolls.expire_holds(time.elapsed());
    }
}

/// The world load's `CancelAllLootRolls` (`0x401c2c`): the rolls signal their cancels and go, at
/// the leg's place after its earlier effects.
fn cancel_rolls_on_worldport(mut rolls: ResMut<LootRolls>) {
    rolls.cancel_all();
}

/// Pick and fill an announcement: the key's template from the install's strings
/// (`FrameScript_GetText`, `0x703bf0`), filled with the pieces the reference pushes, or `None` for
/// no line, as for a key the strings lack.
///
/// A vote or dice line (`0x61c0b0`) branches on `roll_number` as a signed byte and the type: a
/// negative number with type 2 is a Greed vote (`0x61c162`-`0x61c16c`), any other negative one a
/// pass (`0x61c1ae`), a non-negative one with type 0 a Need vote (`0x61c1f5`), and anything else a
/// dice result, `ROLLED_GREED` for type 2 and `ROLLED_NEED` otherwise, with no self form
/// (`0x61c320`). A vote has a `_SELF` form for the active player (`0x61c144`-`0x61c159`); another's
/// takes the name first (`0x61c266`), and a dice line the number first and the name last
/// (`0x61c326`, `0x61c33d`).
///
/// `detailed` is `showLootSpam` (`0xb4e2bc`). At 0, vote and dice lines drop (`0x61c0b9`) and the
/// won line (`0x61b9e0`) takes `*_NO_SPAM_NEED` on `rollType == 1`, else `_GREED`, with the roll
/// number after the name (`0x61baa2`, `0x61bb6a`); all-passed is never gated (`0x61b640`).
fn format_line(
    line: &RollLine,
    name: Option<&str>,
    is_self: bool,
    item: &RollItem,
    detailed: bool,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use benilla_ui::strings::{fill, Arg};
    let color = format!("|c{}", crate::ui_items::quality_color(item.quality));
    let pieces = [
        Arg::S(&color),
        Arg::D(i64::from(item.item_id as i32)),
        Arg::D(0),
        Arg::D(i64::from(item.random_property_id as i32)),
        Arg::D(i64::from(item.suffix_factor as i32)),
        Arg::S(&item.name),
        Arg::S("|r"),
    ];
    let name = Arg::S(name.unwrap_or_default());
    let (key, args): (&str, Vec<Arg<'_>>) = match line {
        RollLine::Announce(_) if !detailed => return None,
        RollLine::Announce(p) => {
            let number = p.roll_number as i8;
            let vote = |own: &'static str, other: &'static str| {
                if is_self {
                    (own, pieces.to_vec())
                } else {
                    (other, [&[name][..], &pieces].concat())
                }
            };
            match (number < 0, p.roll_type) {
                (true, roll_vote::GREED) => vote("LOOT_ROLL_GREED_SELF", "LOOT_ROLL_GREED"),
                (true, _) => vote("LOOT_ROLL_PASSED_SELF", "LOOT_ROLL_PASSED"),
                _ if !is_dice(p) => vote("LOOT_ROLL_NEED_SELF", "LOOT_ROLL_NEED"),
                (false, roll_type) => (
                    if roll_type == roll_vote::GREED {
                        "LOOT_ROLL_ROLLED_GREED"
                    } else {
                        "LOOT_ROLL_ROLLED_NEED"
                    },
                    [&[Arg::D(i64::from(number))][..], &pieces, &[name]].concat(),
                ),
            }
        }
        RollLine::Won(_) if detailed => {
            if is_self {
                ("LOOT_ROLL_YOU_WON", pieces.to_vec())
            } else {
                ("LOOT_ROLL_WON", [&[name][..], &pieces].concat())
            }
        }
        RollLine::Won(p) => {
            let need = p.roll_type == roll_vote::NEED;
            let number = Arg::D(i64::from(p.roll_number as i8));
            match (is_self, need) {
                (true, true) => (
                    "LOOT_ROLL_YOU_WON_NO_SPAM_NEED",
                    [&[number][..], &pieces].concat(),
                ),
                (true, false) => (
                    "LOOT_ROLL_YOU_WON_NO_SPAM_GREED",
                    [&[number][..], &pieces].concat(),
                ),
                (false, true) => (
                    "LOOT_ROLL_WON_NO_SPAM_NEED",
                    [&[name, number][..], &pieces].concat(),
                ),
                (false, false) => (
                    "LOOT_ROLL_WON_NO_SPAM_GREED",
                    [&[name, number][..], &pieces].concat(),
                ),
            }
        }
        RollLine::AllPassed(_) => ("LOOT_ROLL_ALL_PASSED", pieces.to_vec()),
    };
    let text = fill(&get(key)?, &args);
    (!text.is_empty()).then_some(text)
}

/// Whether an announcement is a dice result: a roll number non-negative as a signed byte, with a
/// type other than 0, the Need vote's (`0x61c1ae`, `0x61c1f5`).
fn is_dice(p: &LootRoll) -> bool {
    p.roll_number as i8 >= 0 && p.roll_type != 0
}

/// The item pieces every roll line's template carries after its names: the quality's colour
/// escape (`0x52ad90`, at `0x61c116`), the item id, 0, the roll's two property words (`+0x20`,
/// then `+0x24`, `0x61c16e`-`0x61c171`), the item's
/// name with its suffix (`0x5d8b00`, at `0x61c13f`) and the `|r` reset (`0x844538`).
#[derive(Clone, Debug)]
struct RollItem {
    quality: u32,
    item_id: u32,
    random_property_id: u32,
    suffix_factor: u32,
    name: String,
}

/// The rolls' lane: what the roll handlers, the item and name answers' callbacks, the world
/// load and the vote's tail signal, each fired at its place among the other lanes' items
/// ([`crate::lanes`]). In the reference each signals inside itself (`0x61b430`, `0x61b760`,
/// `0x61b290`, the line composers' chat adds); the getters its listeners call read the list
/// through the host ([`super::live`]).
mod lane {
    use bevy::prelude::*;

    use super::{format_line, LootRolls, ReadyLine, RollEvent};
    use crate::lanes::Until;
    use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
    use benilla_ui::script::{ScriptValue, UiScript};

    /// The lane's firing: each item the drain admits, signalled with the host lent. It stops after
    /// an item whose firing stamped anything, so what that pushed fires next.
    pub(super) fn fire_rolls(
        In(until): In<Until>,
        mut vm: crate::script_calls::VmWith<ResMut<'static, LootRolls>>,
        chat: Res<ChatLog>,
    ) {
        if !vm.run(|script| script.is_some()) {
            return;
        }
        while let Some(at) = vm.own().head().filter(|&s| until.admits(s)) {
            let Some((_, event)) = vm.own().events.pop_front() else {
                return;
            };
            let ((), moved) = crate::lanes::firing(at, || {
                vm.run(|script| {
                    if let Some(script) = script {
                        fire(script, &chat, event);
                    }
                })
            });
            if moved {
                return;
            }
        }
    }

    fn fire(script: &mut UiScript, chat: &ChatLog, event: RollEvent) {
        match event {
            RollEvent::Start {
                roll_id,
                countdown_ms,
            } => script.fire_event(
                "START_LOOT_ROLL",
                vec![
                    ScriptValue::Int(i64::from(roll_id)),
                    ScriptValue::Int(i64::from(countdown_ms)),
                ],
            ),
            RollEvent::Cancel(roll_id) => script.fire_event(
                "CANCEL_LOOT_ROLL",
                vec![ScriptValue::Int(i64::from(roll_id))],
            ),
            // Added to chat inside the composer, as `0x49a870` shows it.
            RollEvent::Line(line) => {
                if let Some(text) = text(script, &line) {
                    chat.show_now(script, &ChatEvent::text_only(ChatEventKind::Loot, text));
                }
            }
        }
    }

    /// The line's template read through `FrameScript_GetText` (`0x703bf0`), filled.
    fn text(script: &UiScript, l: &ReadyLine) -> Option<String> {
        format_line(
            &l.line,
            l.name.as_deref(),
            l.is_self,
            &l.item,
            l.detailed,
            &|key| benilla_ui::strings::global(script.lua(), key),
        )
    }

    /// The lane's drop while no in-game interface is up to hear it: a signal is for the interface
    /// that was up.
    pub(super) fn drop_roll_events(mut rolls: ResMut<LootRolls>) {
        rolls.events.clear();
    }
}

/// The roll getters' reads at the call, the call host's half of
/// [`benilla_ui::script::LootRollQuery`].
mod live {
    use bevy::ecs::system::SystemParam;
    use bevy::prelude::*;

    use benilla_ui::script::live::LiveAsk;
    use benilla_ui::script::LootRollQuery;

    use super::LootRolls;
    use crate::entities::ItemDisplays;
    use crate::items::Items;

    /// What the roll getters read as they run: the list (`0xc4dc00`, by id through `0x61b2e0`),
    /// the item records, their icons and the clock; absent in a harness without them, where they
    /// read the VM's copy.
    #[derive(SystemParam)]
    pub(crate) struct LootRollLive<'w> {
        rolls: Option<Res<'w, LootRolls>>,
        items: Option<Res<'w, Items>>,
        icons: Option<Res<'w, ItemDisplays>>,
        catalogs: crate::items::Rolls<'w>,
        time: Option<Res<'w, Time>>,
    }

    impl LootRollLive<'_> {
        fn roll(&self, roll_id: u32) -> Option<Option<benilla_ui::script::LootRollEntry>> {
            let (rolls, items, time) = (
                self.rolls.as_deref()?,
                self.items.as_deref()?,
                self.time.as_deref()?,
            );
            Some(rolls.active.iter().find(|r| r.roll_id == roll_id).map(|r| {
                r.entry(
                    items,
                    self.icons.as_deref(),
                    self.catalogs.get(),
                    time.elapsed(),
                )
            }))
        }
    }

    impl crate::script_calls::live::LiveFamily for LootRollLive<'_> {
        fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
            ask.answer(|q: &LootRollQuery| self.roll(q.roll_id))
        }
    }
}

pub(crate) use live::LootRollLive;

/// The vote inside its calls ([`benilla_ui::script::LootRollCall`]).
#[derive(SystemParam)]
pub(crate) struct LootRollSteps<'w> {
    /// Absent in a world with no group loot, which drops the votes.
    rolls: Option<ResMut<'w, LootRolls>>,
    net: Res<'w, NetCommands>,
}

impl LootRollSteps<'_> {
    /// `0x61bdf0` past its gate: `CMSG_LOOT_ROLL` with the roll's wire identity (`0x61bf10`); the
    /// VM signals the vote's `CANCEL_LOOT_ROLL` after this returns (`0x61bf2a`). A stale id is
    /// dropped.
    pub(crate) fn vote(&mut self, roll_id: u32, roll_type: u8) {
        match self.rolls.as_deref().and_then(|r| r.vote(roll_id)) {
            Some((looted_target, item_slot)) => {
                debug!("ui_loot_roll: vote {roll_type} on roll {roll_id}");
                let _ = self.net.0.send(ClientCommand::LootRoll {
                    looted_target,
                    item_slot,
                    roll_type,
                });
            }
            None => debug!("ui_loot_roll: RollOnLoot({roll_id}) — no such roll, ignored"),
        }
    }

    /// The vote's tail after its `CANCEL_LOOT_ROLL` (`0x61bf32`-`0x61bf46`): the roll closed and
    /// the oldest deferred one promoted, its `START_LOOT_ROLL` onto the lane.
    pub(crate) fn close(&mut self, roll_id: u32) {
        if let Some(rolls) = self.rolls.as_deref_mut() {
            rolls.close_voted(roll_id);
        }
    }
}

#[cfg(test)]
mod line_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::UiScript;

    fn start(target: u64, slot: u32, item_id: u32) -> LootStartRoll {
        LootStartRoll {
            looted_target: target,
            item_slot: slot,
            item_id,
            random_property_id: 0,
            suffix_factor: 0,
            countdown_ms: 60_000,
        }
    }

    fn announce(roller: u64, roll_number: u8, roll_type: u8) -> LootRoll {
        LootRoll {
            looted_target: 0xAA,
            item_slot: 0,
            roller,
            item_id: 17182,
            random_property_id: 0,
            suffix_factor: 0,
            roll_number,
            roll_type,
        }
    }

    const LINK: &str = "|cffa335ee|Hitem:17182:0:0:0|h[Sulfuras]|h|r";

    /// [`LINK`]'s pieces.
    fn item() -> RollItem {
        RollItem {
            quality: 4,
            item_id: 17182,
            random_property_id: 0,
            suffix_factor: 0,
            name: "Sulfuras".into(),
        }
    }

    thread_local! {
        /// The install's `GlobalStrings.lua`, for the lines' enUS text.
        static STRINGS: UiScript = {
            let s = UiScript::new().expect("VM");
            crate::ui_script::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
            s
        };
    }

    /// [`format_line`] on [`item`], its templates read from the install's strings.
    fn line(l: &RollLine, name: Option<&str>, is_self: bool, detailed: bool) -> Option<String> {
        STRINGS.with(|s| {
            format_line(l, name, is_self, &item(), detailed, &|key| {
                benilla_ui::strings::global(s.lua(), key)
            })
        })
    }

    /// `(128, 2)` is a Greed vote and `(57, 2)` a Greed dice roll: `roll_type` alone cannot tell.
    #[test]
    fn line_table_covers_every_shape() {
        let _data = benilla_formats::wow_data_or_skip!();
        let other = Some("Bob");
        let me = Some("Aldric");
        // (packet, name, is_self, expected)
        let cases: &[(LootRoll, Option<&str>, bool, &str)] = &[
            // The three votes (`Group.cpp:970-990`), which have _SELF forms.
            (
                announce(1, 0, 0),
                other,
                false,
                "Bob has selected Need for: {L}",
            ),
            (
                announce(1, 0, 0),
                None,
                true,
                "You have selected Need for: {L}",
            ),
            (
                announce(1, 128, roll_vote::GREED),
                other,
                false,
                "Bob has selected Greed for: {L}",
            ),
            (
                announce(1, 128, roll_vote::GREED),
                None,
                true,
                "You have selected Greed for: {L}",
            ),
            (announce(1, 128, 128), other, false, "Bob passed on: {L}"),
            (announce(1, 128, 128), None, true, "You passed on: {L}"),
            // The dice results (`Group.cpp:1163,1214`): no _SELF form, so our own rolls name us.
            (
                announce(1, 57, roll_vote::NEED),
                other,
                false,
                "Need Roll - 57 for {L} by Bob",
            ),
            (
                announce(1, 57, roll_vote::NEED),
                me,
                true,
                "Need Roll - 57 for {L} by Aldric",
            ),
            (
                announce(1, 57, roll_vote::GREED),
                other,
                false,
                "Greed Roll - 57 for {L} by Bob",
            ),
            (
                announce(1, 57, roll_vote::GREED),
                me,
                true,
                "Greed Roll - 57 for {L} by Aldric",
            ),
        ];
        for (p, name, is_self, expect) in cases {
            let got = line(&RollLine::Announce(*p), *name, *is_self, true);
            assert_eq!(
                got,
                Some(expect.replace("{L}", LINK)),
                "roll_number {} roll_type {} name {name:?} is_self {is_self}",
                p.roll_number,
                p.roll_type
            );
        }
    }

    /// `LOOT_ROLL_ROLLED_NEED_SELF` is in `GlobalStrings.lua`, but no client code reads it.
    #[test]
    fn our_own_dice_roll_prints_third_person() {
        let _data = benilla_formats::wow_data_or_skip!();
        for roll_type in [roll_vote::NEED, roll_vote::GREED] {
            let got = line(
                &RollLine::Announce(announce(1, 57, roll_type)),
                Some("Aldric"),
                true, // is_self
                true,
            )
            .expect("detail on emits the dice line");
            assert!(
                got.ends_with("by Aldric"),
                "our own roll must name us third-person: {got}"
            );
            assert!(
                !got.contains("You roll"),
                "no _SELF dice form exists: {got}"
            );
        }
    }

    #[test]
    fn greed_vote_and_greed_roll_differ() {
        let _data = benilla_formats::wow_data_or_skip!();
        let vote = line(
            &RollLine::Announce(announce(1, 128, roll_vote::GREED)),
            Some("Bob"),
            false,
            true,
        )
        .unwrap();
        let dice = line(
            &RollLine::Announce(announce(1, 57, roll_vote::GREED)),
            Some("Bob"),
            false,
            true,
        )
        .unwrap();
        assert_ne!(vote, dice);
        assert!(vote.contains("has selected Greed"), "{vote}");
        assert!(dice.contains("Greed Roll - 57"), "{dice}");
    }

    #[test]
    fn resolution_lines() {
        let _data = benilla_formats::wow_data_or_skip!();
        let won = LootRollWon {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
            suffix_factor: 0,
            winner: 1,
            roll_number: 84,
            roll_type: roll_vote::NEED,
        };
        assert_eq!(
            line(&RollLine::Won(won), Some("Bob"), false, true),
            Some(format!("Bob won: {LINK}"))
        );
        // The client reads `LOOT_ROLL_YOU_WON`: the won line has a self form.
        assert_eq!(
            line(&RollLine::Won(won), None, true, true),
            Some(format!("You won: {LINK}"))
        );
        let passed = LootAllPassed {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
            suffix_factor: 0,
        };
        assert_eq!(
            line(&RollLine::AllPassed(passed), Some("Bob"), false, true),
            Some(format!("Everyone passed on: {LINK}"))
        );
    }

    /// `showLootSpam` 0 (the CVar `0xb4e2bc`).
    #[test]
    fn detail_off_suppresses_the_roll_lines_and_reshapes_the_winner() {
        let _data = benilla_formats::wow_data_or_skip!();
        for p in [
            announce(1, 128, roll_vote::NEED),
            announce(1, 128, roll_vote::GREED),
            announce(1, 128, roll_vote::PASS),
            announce(1, 57, roll_vote::NEED),
            announce(1, 57, roll_vote::GREED),
        ] {
            assert_eq!(
                line(&RollLine::Announce(p), Some("Bob"), false, false),
                None,
                "roll_number {} roll_type {}",
                p.roll_number,
                p.roll_type
            );
        }

        // The won line carries the roll number, and Need on `rollType == 1`.
        let mut won = LootRollWon {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
            suffix_factor: 0,
            winner: 1,
            roll_number: 84,
            roll_type: roll_vote::NEED,
        };
        assert_eq!(
            line(&RollLine::Won(won), Some("Bob"), false, false),
            Some(format!("Bob won: {LINK} |cff818181(Need - 84)|r"))
        );
        assert_eq!(
            line(&RollLine::Won(won), None, true, false),
            Some(format!("You won: {LINK} |cff818181(Need - 84)|r"))
        );
        won.roll_type = roll_vote::GREED;
        assert_eq!(
            line(&RollLine::Won(won), Some("Bob"), false, false),
            Some(format!("Bob won: {LINK} |cff818181(Greed - 84)|r"))
        );
        // Anything but Need reads Greed, even a Pass-typed win.
        won.roll_type = roll_vote::PASS;
        assert!(line(&RollLine::Won(won), None, true, false)
            .unwrap()
            .contains("(Greed - 84)"));

        // All-passed is never gated: `0x61b640` does not read the CVar.
        let passed = LootAllPassed {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
            suffix_factor: 0,
        };
        assert_eq!(
            line(&RollLine::AllPassed(passed), None, false, false),
            Some(format!("Everyone passed on: {LINK}"))
        );
    }

    // ── The list, its lookups and its lane ──

    const ROLLED: u32 = 17182;
    const WINNER: u64 = 0x51;

    /// The records, the names and the socket a roll's lookups go through.
    struct Fixture {
        items: Items,
        names: NameCache,
        net: NetCommands,
        rx: crossbeam_channel::Receiver<ClientCommand>,
        detailed: bool,
    }

    impl Fixture {
        /// [`ROLLED`] cached as Sulfuras and [`WINNER`] named Bob, unless `cold`.
        fn new(cold: bool) -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut f = Self {
                items: Items::default(),
                names: NameCache::default(),
                net: NetCommands(tx),
                rx,
                detailed: true,
            };
            if !cold {
                f.items
                    .insert_template(ROLLED, Some(crate::items::test_template("Sulfuras")));
                f.names.insert_player(WINNER, "Bob".into(), None);
            }
            f
        }

        fn look(&self) -> RollLookups<'_> {
            RollLookups {
                items: &self.items,
                names: &self.names,
                net: &self.net,
                self_guid: Some(0x7),
                catalogs: RollCatalogs::NONE,
                detailed: self.detailed,
            }
        }

        /// The record of `entry` answers, found or refused, and runs the rolls' callbacks waiting
        /// on it in their order.
        fn answer(&mut self, rolls: &mut LootRolls, entry: u32, found: bool) {
            self.items.insert_template(
                entry,
                found.then(|| crate::items::test_template("Sulfuras")),
            );
            for cb in self.items.take_callbacks(entry) {
                if let RecordCallback::LootRoll(wait) = cb {
                    rolls.item_answered(wait, found, &self.look());
                }
            }
        }

        fn sent(&self) -> Vec<ClientCommand> {
            self.rx.try_iter().collect()
        }
    }

    fn at(slot: u32) -> LootStartRoll {
        let mut p = start(0xAA, slot, ROLLED);
        p.countdown_ms = 60_000 + slot + 1;
        p
    }

    fn won_at(slot: u32) -> LootRollWon {
        LootRollWon {
            looted_target: 0xAA,
            item_slot: slot,
            item_id: ROLLED,
            random_property_id: 0,
            suffix_factor: 0,
            winner: WINNER,
            roll_number: 84,
            roll_type: roll_vote::NEED,
        }
    }

    fn passed_at(slot: u32) -> LootAllPassed {
        LootAllPassed {
            looted_target: 0xAA,
            item_slot: slot,
            item_id: ROLLED,
            random_property_id: 0,
            suffix_factor: 0,
        }
    }

    /// The lane taken, in order: `S<id>:<arg2>` a start, `C<id>` a cancel, `L:<key>` a line.
    fn lane(rolls: &mut LootRolls) -> Vec<String> {
        rolls
            .events
            .drain(..)
            .map(|(_, e)| match e {
                RollEvent::Start {
                    roll_id,
                    countdown_ms,
                } => format!("S{roll_id}:{countdown_ms}"),
                RollEvent::Cancel(roll_id) => format!("C{roll_id}"),
                RollEvent::Line(l) => format!(
                    "L:{}{}",
                    format_line(
                        &l.line,
                        l.name.as_deref(),
                        l.is_self,
                        &l.item,
                        l.detailed,
                        &|k| Some(k.to_string()),
                    )
                    .unwrap_or_default(),
                    l.name.unwrap_or_default()
                ),
            })
            .collect()
    }

    /// `n` rolls started on a cached record, ids 1..=n, slot id-1, countdown 60 s + id.
    fn opened(f: &Fixture, n: u32) -> LootRolls {
        let mut r = LootRolls::default();
        for slot in 0..n {
            r.start(at(slot), Duration::ZERO, &f.look());
        }
        r
    }

    /// `0x61b310` lists the roll under the next serial and looks its item up with the callback
    /// `0x61b460`; a miss asks and opens nothing.
    #[test]
    fn a_start_lists_the_roll_and_waits_for_its_record() {
        let f = Fixture::new(true);
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, ROLLED), Duration::ZERO, &f.look());
        r.start(start(0xAA, 1, 4306), Duration::ZERO, &f.look());
        assert_eq!(
            r.active.iter().map(|r| r.roll_id).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(r
            .active
            .iter()
            .all(|r| matches!(r.phase, Phase::Waiting(_))));
        assert!(lane(&mut r).is_empty(), "the packet does not announce");
        assert_eq!(crate::query_cache::queries(&f.rx).len(), 2);
    }

    /// `0x61b310`'s hit path: the roll opens inside its packet (`0x61b41c`).
    #[test]
    fn a_cached_record_opens_the_roll_at_its_packet() {
        let f = Fixture::new(false);
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, ROLLED), Duration::ZERO, &f.look());
        assert_eq!(lane(&mut r), ["S1:60000"]);
    }

    /// vmangos can re-send every active roll on reconnect (`SendLootStartRollsForPlayer`).
    #[test]
    fn duplicate_start_is_ignored() {
        let f = Fixture::new(false);
        let mut r = opened(&f, 1);
        r.start(at(0), Duration::ZERO, &f.look());
        assert_eq!(r.active.len(), 1);
        assert_eq!(r.next_id, 1, "and the duplicate burns no id");
    }

    /// The record's answer runs the start's callback (`0x61b460` → `0x61b430`); a refusal opens it
    /// too, unresolved (the module doc's deviation), and once only.
    #[test]
    fn the_records_answer_opens_a_waiting_roll() {
        for found in [true, false] {
            let mut f = Fixture::new(true);
            let mut r = LootRolls::default();
            r.start(start(0xAA, 0, ROLLED), Duration::ZERO, &f.look());
            f.answer(&mut r, ROLLED, found);
            assert_eq!(lane(&mut r), ["S1:60000"], "found {found}");
            f.answer(&mut r, ROLLED, found);
            assert!(lane(&mut r).is_empty(), "found {found}: and only once");
        }
    }

    /// The module doc's deviation: a record unanswered past the hold opens the roll, and its late
    /// answer opens nothing more.
    #[test]
    fn an_unanswered_record_opens_the_roll_at_the_deadline() {
        let mut f = Fixture::new(true);
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, ROLLED), Duration::ZERO, &f.look());
        r.expire_holds(Duration::from_millis(TEMPLATE_HOLD_MAX_MS - 1));
        assert!(lane(&mut r).is_empty(), "still holding");
        r.expire_holds(Duration::from_millis(TEMPLATE_HOLD_MAX_MS));
        assert_eq!(lane(&mut r), ["S1:60000"]);
        f.answer(&mut r, ROLLED, true);
        r.expire_holds(Duration::from_secs(9));
        assert!(lane(&mut r).is_empty());
    }

    /// A result's line, then its roll's `CANCEL_LOOT_ROLL` (`0x61bbf8`, `0x61bc12`; `0x61b6d9`,
    /// `0x61b6f3`), each for the matching roll only.
    #[test]
    fn a_result_shows_its_line_then_cancels_the_matching_roll() {
        let f = Fixture::new(false);
        let mut r = opened(&f, 2);
        lane(&mut r);
        r.won(won_at(0), &f.look());
        assert_eq!(lane(&mut r), ["L:LOOT_ROLL_WONBob", "C1"]);
        assert_eq!(r.active.len(), 1);
        r.all_passed(passed_at(1), &f.look());
        assert_eq!(lane(&mut r), ["L:LOOT_ROLL_ALL_PASSED", "C2"]);
        assert!(r.active.is_empty());
        r.won(won_at(0), &f.look());
        assert_eq!(
            lane(&mut r),
            ["L:LOOT_ROLL_WONBob"],
            "a roll no longer listed cancels nothing"
        );
    }

    /// `0x61b96a` → `0x61b9a0` → `0x61b9bf`: the win's line, its cancel and the promotion wait for
    /// the record and then the winner's name, the roll listed meanwhile. The record's answer runs
    /// the start's callback and then the line's, in the order they asked (`0x61b460`, then
    /// `0x61bd50`), and the name's answer shows the line (`0x61bc80`).
    #[test]
    fn a_win_waits_for_its_record_and_the_winners_name() {
        let mut f = Fixture::new(true);
        let mut r = LootRolls::default();
        r.start(at(0), Duration::ZERO, &f.look());
        r.won(won_at(0), &f.look());
        assert!(lane(&mut r).is_empty());
        f.sent();
        f.answer(&mut r, ROLLED, true);
        assert_eq!(lane(&mut r), ["S1:60001"], "the name is asked next");
        assert!(f
            .sent()
            .iter()
            .any(|c| matches!(c, ClientCommand::NameQuery { guid } if *guid == WINNER)));
        assert!(r.holds(1), "listed while its line waits");
        r.name_answered(0x99, "Other", &f.look());
        assert!(lane(&mut r).is_empty());
        r.name_answered(WINNER, "Bob", &f.look());
        assert_eq!(lane(&mut r), ["L:LOOT_ROLL_WONBob", "C1"]);
        assert!(!r.holds(1));
    }

    /// The module doc's deviation: a refused record or winner's name closes the roll unspoken.
    #[test]
    fn a_refused_record_or_name_closes_the_roll_unspoken() {
        let mut f = Fixture::new(true);
        let mut r = LootRolls::default();
        r.start(at(0), Duration::ZERO, &f.look());
        r.start(at(1), Duration::ZERO, &f.look());
        f.answer(&mut r, ROLLED, true);
        lane(&mut r);
        r.won(won_at(0), &f.look());
        r.name_answered(WINNER, "", &f.look());
        assert_eq!(lane(&mut r), ["C1"]);
        r.all_passed(passed_at(1), &f.look());
        assert_eq!(lane(&mut r), ["L:LOOT_ROLL_ALL_PASSED", "C2"]);
        let mut f = Fixture::new(true);
        let mut r = LootRolls::default();
        r.start(at(0), Duration::ZERO, &f.look());
        r.won(won_at(0), &f.look());
        f.answer(&mut r, ROLLED, false);
        assert_eq!(lane(&mut r), ["S1:60001", "C1"]);
    }

    /// With `showLootSpam` at 0 a vote line is looked up and dropped at its composer
    /// (`0x61c0b9`); a win still speaks, its `_NO_SPAM_` form.
    #[test]
    fn detail_off_drops_the_vote_lines_at_their_composer() {
        let mut f = Fixture::new(true);
        f.detailed = false;
        let mut r = LootRolls::default();
        r.announce(announce(WINNER, 0, 0), &f.look());
        assert_eq!(
            crate::query_cache::queries(&f.rx).len(),
            1,
            "its record is asked"
        );
        f.answer(&mut r, ROLLED, true);
        r.name_answered(WINNER, "Bob", &f.look());
        assert!(lane(&mut r).is_empty());
        assert!(r.pending.is_empty());
        f.names.insert_player(WINNER, "Bob".into(), None);
        r.won(won_at(0), &f.look());
        assert_eq!(lane(&mut r), ["L:LOOT_ROLL_WON_NO_SPAM_NEEDBob"]);
    }

    /// Our vote's tail closes the roll (`0x61bf32`), which stays listed, its getters answering,
    /// and votes again on nothing; its result then signals no cancel.
    #[test]
    fn a_voted_roll_stays_listed_until_its_result() {
        let f = Fixture::new(false);
        let mut r = opened(&f, 1);
        lane(&mut r);
        assert_eq!(r.vote(1), Some((0xAA, 0)));
        r.close_voted(1);
        assert_eq!(r.vote(1), None);
        assert_eq!(r.vote(99), None);
        assert_eq!(r.active.len(), 1);
        let e = r.active[0].entry(&f.items, None, RollCatalogs::NONE, Duration::from_secs(1));
        assert!(e.closed);
        assert_eq!(e.time_left_ms, 60_001 - 1_000);
        r.won(won_at(0), &f.look());
        assert_eq!(lane(&mut r), ["L:LOOT_ROLL_WONBob"]);
        assert!(r.active.is_empty());
    }

    #[test]
    fn ids_are_never_reused() {
        let f = Fixture::new(false);
        let mut r = opened(&f, 1);
        r.won(won_at(0), &f.look());
        r.start(at(1), Duration::ZERO, &f.look());
        assert_eq!(r.active[0].roll_id, 2);
    }

    // ── The four-roll queue (`0x61b430`, `0x61b760`) ──

    /// `0x61b430` opens a roll only while `[0xc4dda8] < 4`; the fifth is marked queued and fires
    /// nothing.
    #[test]
    fn five_rolls_announce_four() {
        let f = Fixture::new(false);
        let mut r = opened(&f, 5);
        assert_eq!(
            lane(&mut r),
            ["S1:60001", "S2:60002", "S3:60003", "S4:60004"]
        );
        assert_eq!(r.open_count(), 4);
        assert_eq!(r.active[4].phase, Phase::Deferred);
    }

    /// `CancelAllLootRolls` (`0x61b290`) signals every roll neither queued nor closed, oldest
    /// first, and `0x61b240` frees them all: the deferred fifth and a voted roll go with no
    /// cancel, and a roll started after opens on a fresh id.
    #[test]
    fn cancel_all_cancels_the_open_rolls_and_frees_every_one() {
        let f = Fixture::new(false);
        let mut r = opened(&f, 5);
        r.close_voted(2);
        lane(&mut r);
        r.cancel_all();
        assert_eq!(lane(&mut r), ["C1", "C3", "C4", "C5"]);
        assert!(r.active.is_empty());
        let f = Fixture::new(false);
        let mut r = opened(&f, 5);
        lane(&mut r);
        r.cancel_all();
        assert_eq!(
            lane(&mut r),
            ["C1", "C2", "C3", "C4"],
            "the deferred fifth has no frame to cancel"
        );
        r.start(at(9), Duration::ZERO, &f.look());
        assert_eq!(lane(&mut r), ["S6:60010"]);
    }

    /// Every close calls `0x61b760`: our vote (`0x61bf46`), a win (`0x61bc6b`) and all-passed
    /// (`0x61b74c`) each open the oldest deferred roll, with its own wire countdown, after the
    /// closed one's `CANCEL`.
    #[test]
    fn every_close_path_promotes_the_oldest_deferred_roll() {
        type Close = fn(&mut LootRolls, &RollLookups);
        let closers: [(&str, Close, &[&str]); 3] = [
            ("vote", |r, _| r.close_voted(2), &["S5:60005"]),
            (
                "won",
                |r, look| r.won(won_at(1), look),
                &["L:LOOT_ROLL_WONBob", "C2", "S5:60005"],
            ),
            (
                "all passed",
                |r, look| r.all_passed(passed_at(1), look),
                &["L:LOOT_ROLL_ALL_PASSED", "C2", "S5:60005"],
            ),
        ];
        for (name, close, expect) in closers {
            let f = Fixture::new(false);
            let mut r = opened(&f, 6);
            lane(&mut r);
            close(&mut r, &f.look());
            assert_eq!(lane(&mut r), expect, "{name}");
        }
    }

    /// Queued rolls open oldest first, one per freed slot.
    #[test]
    fn deferred_rolls_open_oldest_first() {
        let f = Fixture::new(false);
        let mut r = opened(&f, 6);
        lane(&mut r);
        r.won(won_at(3), &f.look());
        assert_eq!(lane(&mut r).last().map(String::as_str), Some("S5:60005"));
        r.won(won_at(0), &f.look());
        assert_eq!(lane(&mut r).last().map(String::as_str), Some("S6:60006"));
    }

    /// A deferred roll never counted up, so closing it frees nothing: the reference's
    /// `0x61b6f8`/`0x61bc17` decrement anyway and let a fifth frame open (the module doc's
    /// deviation). It still signals its `CANCEL_LOOT_ROLL`.
    #[test]
    fn closing_a_deferred_roll_leaves_the_count_right() {
        let f = Fixture::new(false);
        type Close = fn(&mut LootRolls, &RollLookups);
        for close in [
            (|r, look| r.all_passed(passed_at(4), look)) as Close,
            |r, look| r.won(won_at(4), look),
            |r, _| r.close_voted(5),
        ] {
            let mut r = opened(&f, 5);
            lane(&mut r);
            close(&mut r, &f.look());
            assert_eq!(r.open_count(), 4);
            r.start(at(9), Duration::ZERO, &f.look());
            assert!(
                !lane(&mut r).iter().any(|e| e.starts_with('S')),
                "the sixth packet opens nothing"
            );
        }
    }

    /// `0x61b430` runs when the record is in, not at the packet: a roll still waiting holds no
    /// slot, so rolls behind it open ahead of it; once in, it queues behind four open.
    #[test]
    fn a_waiting_roll_holds_no_slot() {
        let mut f = Fixture::new(false);
        let mut r = LootRolls::default();
        let mut first = start(0xAA, 9, 4306);
        first.countdown_ms = 70_000;
        r.start(first, Duration::ZERO, &f.look());
        for slot in 1..=4 {
            r.start(at(slot), Duration::ZERO, &f.look());
        }
        assert_eq!(
            lane(&mut r),
            ["S2:60002", "S3:60003", "S4:60004", "S5:60005"]
        );
        f.answer(&mut r, 4306, true);
        assert!(lane(&mut r).is_empty(), "four are open: it queues");
        r.won(won_at(1), &f.look());
        assert_eq!(lane(&mut r), ["L:LOOT_ROLL_WONBob", "C2", "S1:70000"]);
    }

    /// The getters read the expiry less the clock at the call (`0x4c331c`), and a promoted roll's
    /// bar counts from its packet, its `arg2` the wire's countdown whole (`0x61b79a`).
    #[test]
    fn a_promoted_roll_reads_its_bar_from_the_packet() {
        let f = Fixture::new(false);
        let mut r = opened(&f, 5);
        lane(&mut r);
        r.all_passed(passed_at(0), &f.look());
        assert_eq!(lane(&mut r).last().map(String::as_str), Some("S5:60005"));
        let fifth = r.active.iter().find(|r| r.roll_id == 5).unwrap();
        let e = fifth.entry(&f.items, None, RollCatalogs::NONE, Duration::from_secs(30));
        assert_eq!(e.time_left_ms, 60_005 - 30_000);
        assert_eq!(e.name.as_deref(), Some("Sulfuras"));
        let late = fifth.entry(&f.items, None, RollCatalogs::NONE, Duration::from_secs(99));
        assert_eq!(
            late.time_left_ms,
            60_005u32.wrapping_sub(99_000),
            "a past expiry wraps, unclamped"
        );
    }
}
