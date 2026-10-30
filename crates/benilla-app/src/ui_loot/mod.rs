//! The app side of the loot window. The net handlers change [`LootState`] at their packets, and
//! what each signals is a step on the loot lane, stamped in its handler with the item pushes among
//! them, which [`fire_loot`] signals at the packet's place among the other lanes
//! ([`crate::lanes`]): a removal's `LOOT_SLOT_CLEARED` (`0x4c1f77`, `0x4c276f`), a close's
//! `LOOT_CLOSED` (`0x4c1dac`), at a response the ended window's `LOOT_CLOSED` and the copier's end,
//! `LOOT_OPENED` or the auto-loot sweep, a candidate name's `UPDATE_MASTER_LOOT_LIST` (`0x4c2fba`),
//! and each push's `ITEM_PUSH` and line. The getters read the session through the host
//! ([`live::LootLive`]). The loot verbs and a row's click apply inside their calls
//! ([`calls::LootSteps::apply`]), a precast end's close where the end applies
//! ([`close_at_precast_end`]), and a movement start's where its guard runs ([`MoveStartClose`]).

use benilla_protocol::messages::{ItemPushResult, LootItem, BAG_PLAYER_INVENTORY};
use bevy::prelude::*;

use benilla_ui::script::{LootRow, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::{Items, RollCatalogs};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::Ask;
use crate::script_calls::VmWith;
use crate::ui_items::KEYRING_CONTAINER;
use crate::ui_party::{GroupState, GROUP_MEMBER_SUBGROUP};
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};

pub(crate) use calls::{interaction_byte, LootSteps, OpenItem};
pub(crate) use live::LootLive;

#[cfg(test)]
mod call_tests;
mod calls;
mod live;
mod net;
#[cfg(test)]
mod receive_tests;
#[cfg(test)]
mod sweep_tests;

/// `ItemBondingType::BIND_WHEN_PICKED_UP`, the first conjunct of the `LOOT_BIND` confirm: the
/// client compares `item_template + 0x194` against 1.
const BIND_WHEN_PICKED_UP: u32 = 1;

/// The second conjunct: uncommon or better (`0x4c28fb cmp [tmpl+0x1c], 2`).
const BIND_CONFIRM_MIN_QUALITY: u32 = 2;

/// The coin row's quality, 0, so its text is grey: `0x4c23a0`'s coin guard returns 0 at
/// `0x4c23de`, and `LootFrame_Update` has no coin case (`LootFrame.lua:81-85`).
const COIN_QUALITY: u32 = 0;
/// The slots `OnItemPush` treats as the keyring (`0x491bc3`/`0x491bc8`, 0x51..=0x70): the
/// descriptor's full 32 positions, wider than the 16 vmangos uses from `KEYRING_SLOT_START` 81.
const PUSH_KEYRING_SLOTS: std::ops::RangeInclusive<u32> = 81..=112;

/// A push, on the lane at its packet or waiting on its item template. `OnItemPush 0x491a60` defers
/// both of its outputs the same way: on a cache miss (`0x491a93`) it re-enters from the cache
/// callback `0x491ee0`, at the answer, and a refused answer frees it (`0x491eea`-`0x491efb` →
/// `SMemFree 0x646430`) ([`LootState::push_answered`]), so every wait ends at its answer.
#[derive(Clone, Debug)]
struct PendingReceive {
    /// Crafted or conjured: "You create: …".
    created: bool,
    entry: u32,
    count: u32,
    from_npc: bool,
    random_property_id: u32,
    suffix_factor: u32,
    /// `showInChat` gates the line only: `ITEM_PUSH` fires (`0x491be8`) before the chat test
    /// (`0x491bf3`), so a silent push still animates.
    in_chat: bool,
    container: i64,
    /// Another group member's push, by guid; `None` for the active player's own.
    owner: Option<u64>,
}

/// `ITEM_PUSH`'s `arg1`, the reference's selector at `0x491bb5`-`0x491bd6`: a wire bag other than
/// 255 gives `bag + 1` (20..=23 for an equipped bag, the ids its bar buttons carry), a keyring slot
/// gives [`KEYRING_CONTAINER`], and anything else 0, the backpack.
fn push_container(bag: u8, slot: u32) -> i64 {
    if bag != BAG_PLAYER_INVENTORY {
        return i64::from(bag) + 1;
    }
    if PUSH_KEYRING_SLOTS.contains(&slot) {
        return KEYRING_CONTAINER;
    }
    0
}

/// The window's cells: the source `0xb71b48`, the money `0xb71ba0`, the type `0xb71bbc` and the
/// row array `0xb71968`. The copier fills them at a response (`0x4c1cb0`), a removal zeroes its
/// row in place (`0x4c1f59`), and a close zeroes only the source (`0x4c1da0`): the rows, the money
/// and the type stay until the next response, and the take dispatcher `0x4c2790` reads them
/// without asking for a source.
#[derive(Clone, Debug, Default, PartialEq)]
struct Window {
    /// The looted object; `None` when no loot is open.
    source: Option<u64>,
    /// The coin pile in copper, 0 once taken.
    gold: u32,
    /// The money cell is nonzero, so row 1 is the coin row: set at the response, kept once the
    /// coin is taken (-1).
    coin_slot: bool,
    /// The item rows in wire order; a taken row lands in `taken`, a gap in place.
    items: Vec<LootItem>,
    /// Wire slots taken by anyone (`SMSG_LOOT_REMOVED`), left as gaps.
    taken: Vec<u8>,
    /// The wire `loot_type`, the cell `0xb71bbc` (`0x4c2740`) a close leaves: fishing for
    /// `IsFishingLoot()`, and a disenchant window with rows left survives a close.
    loot_type: u8,
    /// The candidate array `0xc4dc38` as the last `SMSG_LOOT_MASTER_LIST` placed it
    /// ([`place_candidates`]), its one writer (`0x61c562`-`0x61c637`). It is no cell of the
    /// window: no response or close touches it, so each response carries it over, and it rides
    /// here so a step's listeners read it as their packet left it.
    master_candidates: Vec<Option<u64>>,
}

/// A clicked row resolved: the coin pile, or an item at its wire loot slot.
enum LootAction {
    Money,
    Item {
        wire_slot: u8,
        display_id: u32,
        item_id: u32,
        /// The take dispatcher branches on it (`0x4c28a9`).
        slot_type: u8,
    },
}

/// The client's candidate array: 40 slots at `0xc4dc38`, bound-checked by the getter `0x61c660`.
const MASTER_LOOT_CANDIDATE_SLOTS: usize = 40;
/// A raid subgroup's block within it (`0x61c609`).
const MEMBERS_PER_RAID_GROUP: usize = 5;

/// The candidate slots as `HandleLootMasterList 0x61c550` lays them out at its packet, split on
/// the raid roster's count as the packet finds it (`0x61c577`), not the group type: wire order
/// with no roster held (`0x61c5b9`), a raid of one included; with one, each guid in the first free
/// slot of its subgroup's block of five (`0x61c5c9`-`0x61c637`), holes kept for
/// `GroupLootDropDown_Initialize`'s "Group N" submenus (`LootFrame.lua:197-213`). The getter and the
/// give read the same array, so an index means one candidate.
fn place_candidates(candidates: &[u64], group: &GroupState) -> Vec<Option<u64>> {
    if !group.raid_held() {
        return candidates.iter().copied().map(Some).collect();
    }
    let mut slots: Vec<Option<u64>> = vec![None; MASTER_LOOT_CANDIDATE_SLOTS];
    for &guid in candidates {
        // The stored row's subgroup (`[row+8]`, `0x61c606`); a guid on no row below the count,
        // ours included, is placed nowhere (`0x61c5f9`).
        let Some(row) = group.raid.iter().find(|r| r.guid == guid) else {
            continue;
        };
        let base = usize::from(row.subgroup & GROUP_MEMBER_SUBGROUP) * MEMBERS_PER_RAID_GROUP;
        let end = (base + MEMBERS_PER_RAID_GROUP).min(MASTER_LOOT_CANDIDATE_SLOTS);
        if let Some(free) = (base..end).find(|&i| slots[i].is_none()) {
            slots[free] = Some(guid);
        }
    }
    while slots.last().is_some_and(Option::is_none) {
        slots.pop(); // trailing empties read as 0 either way
    }
    slots
}

impl Window {
    fn has_coin(&self) -> bool {
        self.gold > 0
    }

    /// No money above 0 and no row left: the second half of `LootHasContents` (`0x4c2a9c`-
    /// `0x4c2ab8`).
    fn is_empty(&self) -> bool {
        !self.has_coin() && self.items.iter().all(|it| self.taken.contains(&it.slot))
    }

    /// The display rows of the fixed layout, gaps included.
    fn row_count(&self) -> u32 {
        (self.items.len() + usize::from(self.coin_slot)) as u32
    }

    /// A 1-based display row of the fixed layout; a gap answers `None`, like the hidden button.
    fn action_at(&self, index_1based: u32) -> Option<LootAction> {
        let mut index = index_1based.checked_sub(1)? as usize; // 0-based layout position
        if self.coin_slot {
            if index == 0 {
                return self.has_coin().then_some(LootAction::Money);
            }
            index -= 1;
        }
        let it = self.items.get(index)?;
        (!self.taken.contains(&it.slot)).then_some(LootAction::Item {
            wire_slot: it.slot,
            display_id: it.display_info_id,
            item_id: it.item_id,
            slot_type: it.slot_type,
        })
    }

    /// The display row of the item with `wire_slot`: its array position plus one, plus one more
    /// while the money cell is nonzero (`0x4c1f60`-`0x4c1f6b`).
    fn row_of(&self, wire_slot: u8) -> Option<u32> {
        let at = self.items.iter().position(|it| it.slot == wire_slot)?;
        Some(at as u32 + 1 + u32::from(self.coin_slot))
    }

    /// The display row at a 0-based position of the fixed layout as the getters read it, every
    /// cache only read: `None` with no window or past the end, `Some(None)` for a cleared row, the
    /// coin row first while the money cell is set.
    fn row(
        &self,
        at: usize,
        items: &Items,
        icons: Option<&ItemDisplays>,
        rolls: RollCatalogs,
    ) -> Option<Option<LootRow>> {
        self.source?;
        let mut index = at;
        if self.coin_slot {
            if index == 0 {
                return Some(self.has_coin().then(|| coin_row(self.gold)));
            }
            index -= 1;
        }
        let it = self.items.get(index)?;
        Some(
            (!self.taken.contains(&it.slot))
                .then(|| resolve_item(it, items, icons, Ask::Read, rolls)),
        )
    }

    /// `GetNumLootItems` (`0x4c2260`): the last occupied row's position plus one (`0x4c2281`), plus
    /// one while the money cell is nonzero (`0x4c229b`); 0 with no window (`0x4c226d`).
    fn count(&self) -> usize {
        if self.source.is_none() {
            return 0;
        }
        let items = self
            .items
            .iter()
            .rposition(|it| !self.taken.contains(&it.slot))
            .map_or(0, |i| i + 1);
        items + usize::from(self.coin_slot)
    }

    /// Whether a row still waits on its item record: the reference's pending count `[0xb71b44]`
    /// ([`LootState::records_pending`]). Deviation: a negative answer releases the row, where the
    /// reference's callback `0x4c2ac0` returns on a failed query and the window never opens,
    /// because an entry the server cannot describe would otherwise keep the loot shut for good.
    fn records_pending(&self, items: &Items) -> bool {
        self.items.iter().any(|it| {
            !self.taken.contains(&it.slot)
                && items.template(it.item_id, 0, Ask::Read).is_none()
                && !items.template_answered_unknown(it.item_id)
        })
    }
}

/// The cells a response's handler writes over before its copier runs, as the session it replaces
/// left them: the latch (`0x5ebb60`), the bind stash (`[0x847cec]`, `0x4c1df5`) and whether a
/// copier's end waits on records (`0x4c1e77`). The steps that session's packets queued ahead of
/// the response run on these, as each packet's dispatch ran them before the response arrived.
/// The auto-loot byte is none of them: only `SetAutoLoot` writes it (`0x5df468`), never a
/// response, so every step reads it live.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cells {
    latch: Option<u64>,
    stash: Option<u32>,
    copier_due: bool,
    /// The session's window is still open (`0xb71b48` set), so the copier's teardown signals its
    /// `LOOT_CLOSED` (`0x4c1ccb`, `0x4c1dac`): set at the response, and kept as that session's
    /// steps leave it ([`restore_live`]), a listener's close among them.
    open: bool,
}

/// What a packet signals that the VM has yet to hear, in its order on the loot lane, each with the
/// window as its packet left it, which its listeners read through the host while it fires
/// ([`on_its_window`]).
#[derive(Clone, Debug)]
enum Step {
    /// A removal's `LOOT_SLOT_CLEARED` with its display row (`0x4c1f77`, `0x4c276f`), window or
    /// not, on the window it left.
    Cleared { row: u32, left: Window },
    /// A close's teardown's `LOOT_CLOSED` (`0x4c1cb0(0)`, `0x4c1dac`), on the window it left, the
    /// source zeroed.
    Closed(Window),
    /// `UPDATE_MASTER_LOOT_LIST`, the name callback `0x4c2fb0` a candidate's lookup registered,
    /// at its name's answer (`0x4c2fba`).
    CandidateNamed(Window),
    /// The unlocks of a close a packet asked for (`0x48f2bd`, `0x4c1d36`), signalled in their
    /// place: after the removal's `LOOT_SLOT_CLEARED`, before the close's `LOOT_CLOSED`. Each is
    /// flagged when it needs a live item object.
    Unlocks(Vec<(u64, bool)>, Window),
    /// A response's copier (`0x4c1cb0`): the cells its teardown leaves, the ended session's with
    /// the source zeroed (`0x4c1da0`), its `LOOT_CLOSED` while that window is open
    /// ([`Cells::open`]), then the window it fills, and whether its end ran inside its packet,
    /// every row's record held when its handler asked for each (`0x4c1e77`, `0x4c1ea6`); unknown
    /// for a window no handler asked for. `moving`: the response's tail owes the move-start close
    /// behind that end (`0x5ebc3a`-`0x5ebc51`).
    Opened {
        ended: Window,
        window: Window,
        ends: Option<bool>,
        prior: Cells,
        moving: bool,
    },
    /// The copier's end a response's pending records held, run by the cache callback at the last
    /// answer (`0x4c2ac0`, `0x4c2afc`), on its own session's window: a later response in the
    /// frame replaced the live cells.
    CopierEnd(Window),
    /// An inventory failure's tail (`0x5e3af3`-`0x5e3b20`): `LOOT_OPENED` again while a window
    /// is open (`0xb71b48`, as the failure's packet found it) and the active player's auto-loot
    /// byte is set, read at the failure's place.
    FailureTail(Window),
    /// The sweep a close with no VM owed a disenchant window with rows left (`0x48f264` →
    /// `0x4c1fa0`).
    Sweep,
    /// `SMSG_ITEM_PUSH_RESULT`'s `OnItemPush` (`0x491a60`): `ITEM_PUSH` and the line, reading the
    /// window the packets before it left.
    Push(PendingReceive, Window),
}

/// The loot session: the window, the loot lane, the pushes waiting on their records, which outlive
/// the window, and what the loot calls owe.
#[derive(Resource, Default)]
pub(crate) struct LootState {
    window: Window,
    /// The pushes waiting on their item templates, each queued again at its answer.
    waiting: Vec<PendingReceive>,
    /// The loot lane: what the packets signal that the VM has yet to hear, each stamped where it
    /// was raised ([`crate::lanes::stamp`]), each with the window its packet left.
    steps: std::collections::VecDeque<(crate::lanes::Stamp, Step)>,
    /// The row a `LOOT_BIND_CONFIRM` is open for, the reference's `[0x847cec]`: set after the
    /// signal's handlers (`0x4c2917`, `0x4c21dc`), cleared by the confirmed send (`0x4c281a`) and
    /// by the next response (`0x4c1df5`), not by a close.
    /// Deviation: it is the display row, where the reference's is the item index plus one
    /// (`0x4c2885`), so an addon's `GetLootSlotInfo(arg1)` reads the row the dialog is for.
    pending_bind_confirm: Option<u32>,
    /// The candidates `GetMasterLootCandidate` looked up and the name cache missed, each a
    /// registration of the callback `0x4c2fb0` (dedupe flag 0, `0x4c2f61`), spent by the name's
    /// answer ([`Self::name_answered`]).
    name_waits: Vec<u64>,
    /// A close with `dl` clear found a disenchant window with rows left and owes its sweep
    /// (`0x48f264` → `0x4c1fa0`), run by its step on the lane ([`Step::Sweep`]), which holds the
    /// VM; a response or a close since cancels it.
    sweep_owed: bool,
    /// The owed sweep's close ran in its item source's own deactivate, the item still in the live
    /// table (`0x46496f`): the sweep, which the reference runs inside that close (`0x48f264`),
    /// reads the source resolving at its end (`0x4c2228` → `0x4c2a8c`), though the release has
    /// unlinked it by the time the lane fires.
    sweep_source_live: bool,
    /// The `UnlockItem` (`0x495420`) calls a close owes on an item source that a call or a turn
    /// runs in place, after its line ([`close_interaction_in_place`]); a window's clear keeps
    /// them. The flag marks a window source's, owed only while the guid resolves as a live item
    /// object. Every other close and the copier's ending put theirs on the lane at their place
    /// ([`close_interaction`], [`Self::open`]), as a refusal's does ([`Self::unlock_item_here`]).
    item_unlocks: Vec<(u64, bool)>,
    /// The response's copier end, `LOOT_OPENED` or the sweep, waits on its item records: the
    /// copier returns with a fetch outstanding (`0x4c1ea6`) and the cache callback ends it at the
    /// last arrival (`0x4c2afc`).
    copier_due: bool,
    /// `0xb71bc4`: a sweep of this session has run (`0x4c221e`), so the next prints no money line
    /// (`0x4c2008`); the session's end clears it (`0x4c1d95`).
    coin_line_shown: bool,
    /// What the loot steps owe once their signals' handlers have run, innermost last
    /// ([`calls::Then`]).
    then: Vec<calls::Then>,
    /// The source of the window a step fires on, while a later packet's handler has closed that
    /// session: its release and latch went at that packet, its signals are queued behind
    /// ([`LootState::take_queued_close`]).
    closed_since: Option<u64>,
}

impl LootState {
    /// The copier (`0x4c1cb0`) at a response: it ends the session in progress, then fills the
    /// window; quest items ride the same rows at `slot = items.len() + i`.
    pub(crate) fn open(&mut self, source: u64, loot_type: u8, gold: u32, items: Vec<LootItem>) {
        // The ending unlocks an item source (`0x5ebc35` → `0x48f3a0` → `0x4c1d36`), on the lane
        // ahead of the step whose firing signals the ended session's `LOOT_CLOSED` (`0x4c1dac`),
        // and clears the money line's flag (`0x4c1d95`).
        if let Some(old) = self.window.source {
            self.queue_source_unlocks(old, 1, Source::CheckedAtTheFire);
            self.coin_line_shown = false;
        }
        let ended = Window {
            source: None,
            ..self.window.clone()
        };
        let prior = Cells {
            latch: self.window.source,
            stash: self.pending_bind_confirm,
            copier_due: self.copier_due,
            open: self.window.source.is_some(),
        };
        self.window = Window {
            source: Some(source),
            gold,
            coin_slot: gold > 0, // `0x4c1de9`
            items,
            taken: Vec::new(),
            loot_type, // `0x4c1dd6`
            master_candidates: std::mem::take(&mut self.window.master_candidates),
        };
        self.pending_bind_confirm = None; // `0x4c1df5`
        self.sweep_owed = false;
        self.sweep_source_live = false;
        self.copier_due = true;
        self.queue(Step::Opened {
            ended,
            window: self.window.clone(),
            ends: None,
            prior,
            moving: false,
        });
    }

    /// The copier returned with an item record outstanding (`0x4c1ea6`), its end left to the last
    /// answer (`0x4c2afc`).
    pub(crate) fn copier_waits(&self) -> bool {
        self.copier_due
    }

    /// The response's tail on the move (`0x5ebc3a`-`0x5ebc51`) after a copier that ended inside its
    /// packet: the move-start close behind that end, which the lane runs as the response's step
    /// fires ([`end_open`]), so the copier's sends and signals go first.
    pub(crate) fn close_after_the_copier(&mut self) {
        let opened = self.steps.iter_mut().rev().find_map(|(_, s)| match s {
            Step::Opened { moving, .. } => Some(moving),
            _ => None,
        });
        if let Some(moving) = opened {
            *moving = true;
        }
    }

    /// The latch as it stood before the response's handler wrote it (`0x5ebb60`), for the steps
    /// of the session it replaces.
    pub(crate) fn note_replaced_latch(&mut self, latch: Option<u64>) {
        if let Some(prior) = self.replaced_cells_from_back() {
            prior.latch = latch;
        }
    }

    /// The last queued response's cells, the one a handler just queued.
    fn replaced_cells_from_back(&mut self) -> Option<&mut Cells> {
        self.steps.iter_mut().rev().find_map(|(_, s)| match s {
            Step::Opened { prior, .. } => Some(prior),
            _ => None,
        })
    }

    /// The first queued response's cells: those of the session the steps ahead of it belong to.
    fn replaced_cells(&mut self) -> Option<&mut Cells> {
        self.steps.iter_mut().find_map(|(_, s)| match s {
            Step::Opened { prior, .. } => Some(prior),
            _ => None,
        })
    }

    /// The copier's asks at its packet (`0x4c1e77`): each row's record looked up with the callback
    /// `0x4c2ac0`, the copier returning with any outstanding (`0x4c1ea6`), so its end waits for
    /// the last answer ([`Step::CopierEnd`]) whatever lands before the lane fires it; with none
    /// outstanding it ends inside the packet, whatever a later packet does to the window.
    pub(crate) fn ask_rows(&mut self, items: &Items, commands: &NetCommands) {
        for it in &self.window.items {
            if items
                .template(it.item_id, 0, Ask::Event(commands))
                .is_none()
            {
                items.wait(it.item_id, crate::items::RecordCallback::LootRow);
            }
        }
        let ends = !self.records_pending(items);
        if ends {
            self.copier_due = false;
        }
        let opened = self.steps.iter_mut().rev().find_map(|(_, s)| match s {
            Step::Opened { ends, .. } => Some(ends),
            _ => None,
        });
        if let Some(verdict) = opened {
            *verdict = Some(ends);
        }
    }

    /// Queue `step` on the lane behind everything stamped before it.
    fn queue(&mut self, step: Step) {
        let stamp = crate::lanes::stamp();
        let at = self.steps.partition_point(|(s, _)| *s < stamp);
        self.steps.insert(at, (stamp, step));
    }

    /// The lane's head.
    fn peek(&self) -> Option<crate::lanes::Stamp> {
        self.steps.front().map(|(s, _)| *s)
    }

    /// Take the head if `until` admits it.
    fn pop(&mut self, until: crate::lanes::Until) -> Option<(crate::lanes::Stamp, Step)> {
        until
            .admits(self.peek()?)
            .then(|| self.steps.pop_front())
            .flatten()
    }

    /// Whether the session a step's window `at` shows open is open still where the step fires:
    /// the replaced session's cells' ([`Cells::open`]), else the live window's.
    fn session_open(&self, at: &Window) -> bool {
        let replaced = self.steps.iter().find_map(|(_, s)| match s {
            Step::Opened { prior, .. } => Some(prior.open),
            _ => None,
        });
        at.source.is_some() && replaced.unwrap_or(self.window.source == at.source)
    }

    /// The window a step fires on is of a session a later packet's handler closed.
    fn window_closed_since(&self) -> bool {
        self.closed_since.is_some() && self.closed_since == self.window.source
    }

    /// A close a call makes on a step's window whose session a later packet's handler closed
    /// ([`Self::closed_since`]): the reference made it here, before that packet arrived, whose
    /// close then found no window (`0x48f23f`). So it takes that close's unlocks and `LOOT_CLOSED`
    /// off the lane, queued ahead of the next response; whether they were still there, else they
    /// have signalled and this close finds no window either.
    fn take_queued_close(&mut self, source: u64) -> bool {
        let end = self
            .steps
            .iter()
            .position(|(_, s)| matches!(s, Step::Opened { .. }))
            .unwrap_or(self.steps.len());
        let Some(at) = self
            .steps
            .iter()
            .take(end)
            .position(|(_, s)| matches!(s, Step::Closed(_)))
        else {
            return false;
        };
        self.steps.remove(at);
        let unlocks = at.checked_sub(1).filter(|&u| {
            matches!(&self.steps[u].1, Step::Unlocks(us, _)
                if !us.is_empty() && us.iter().all(|&(g, _)| g == source))
        });
        if let Some(u) = unlocks {
            self.steps.remove(u);
        }
        true
    }

    /// Whether a later response's copier is queued: the steps ahead of it ran on the cells their
    /// session left, as each packet's dispatch ran them before the next arrived.
    fn replaced(&self) -> bool {
        self.steps
            .iter()
            .any(|(_, s)| matches!(s, Step::Opened { .. }))
    }

    /// A row's record answered, or was refused (`0x4c2ac0`; [`Window::records_pending`]'s
    /// deviation): at the last of the session's answers the copier ends (`0x4c2afc`), on the
    /// session's window as the answer finds it.
    pub(crate) fn row_answered(&mut self, items: &Items) {
        if self.copier_due && !self.records_pending(items) {
            self.copier_due = false;
            self.queue(Step::CopierEnd(self.window.clone()));
        }
    }

    /// A push's record answered (`0x491ee0`): the first push waiting on `entry` runs with its
    /// record held, and a refusal frees it unrun (`0x491ee8`, `0x491efb` → `SMemFree 0x646430`).
    pub(crate) fn push_answered(&mut self, items: &Items, entry: u32) {
        let Some(at) = self.waiting.iter().position(|r| r.entry == entry) else {
            return;
        };
        let r = self.waiting.remove(at);
        if items.template(entry, 0, Ask::Read).is_some() {
            self.queue(Step::Push(r, self.window.clone()));
        }
    }

    /// `HandleLootMasterList 0x61c550`: the array cleared (`0x61c567`) and each guid placed off
    /// the roster as the packet finds it ([`place_candidates`]). It signals nothing: the one
    /// `UPDATE_MASTER_LOOT_LIST` is a name lookup's callback (`0x4c2fb0`).
    pub(crate) fn set_master_candidates(&mut self, candidates: &[u64], group: &GroupState) {
        self.window.master_candidates = place_candidates(candidates, group);
    }

    /// `0x61c660(index)`: the candidate at a 0-based index of the array, `None` for a zero slot or
    /// an index of 40 or more (`0x61c660`-`0x61c669`).
    pub(crate) fn master_candidate(&self, index: usize) -> Option<u64> {
        if index >= MASTER_LOOT_CANDIDATE_SLOTS {
            return None;
        }
        self.window.master_candidates.get(index).copied().flatten()
    }

    /// `GetMasterLootCandidate`'s lookup on a name the cache misses: its callback `0x4c2fb0`
    /// waits on the guid, once per call (`0x4c2f61`).
    pub(crate) fn wait_for_name(&mut self, guid: u64) {
        self.name_waits.push(guid);
    }

    /// A player name's answer (`0x55f426`): each callback waiting on `guid` runs, and for a found
    /// name signals `UPDATE_MASTER_LOOT_LIST` (`0x4c2fb6`-`0x4c2fbf`), on the lane at the answer.
    pub(crate) fn name_answered(&mut self, guid: u64, found: bool) {
        let before = self.name_waits.len();
        self.name_waits.retain(|&g| g != guid);
        if found {
            for _ in self.name_waits.len()..before {
                self.queue(Step::CandidateNamed(self.window.clone()));
            }
        }
    }

    /// `ClearLootRow` (`0x4c1f00`): the row with the wire slot becomes a gap, whether or not a
    /// window is open, and `LOOT_SLOT_CLEARED` signals with its display row (`0x4c1f77`); whether
    /// one did.
    pub(crate) fn remove_slot(&mut self, wire_slot: u8) -> bool {
        let w = &mut self.window;
        if !w.items.iter().any(|it| it.slot == wire_slot) || w.taken.contains(&wire_slot) {
            return false; // `0x4c1f22`
        }
        w.taken.push(wire_slot);
        let Some(row) = self.window.row_of(wire_slot) else {
            return false;
        };
        self.queue(Step::Cleared {
            row,
            left: self.window.clone(),
        });
        true
    }

    /// `ClearLootCoinRow` (`0x4c2750`): money above 0 goes to -1, the coin row a gap, whether a
    /// window is open or not, and `LOOT_SLOT_CLEARED` signals with row 1 (`0x4c276f`); whether it
    /// was there (`0x4c2757`).
    pub(crate) fn clear_money(&mut self) -> bool {
        if !self.window.has_coin() {
            return false;
        }
        self.take_coin();
        self.queue(Step::Cleared {
            row: 1,
            left: self.window.clone(),
        });
        true
    }

    /// The money cell to -1 (`0x4c2858`, `0x4c2765`): the coin row stays a gap.
    fn take_coin(&mut self) {
        self.window.gold = 0;
    }

    /// Queues a push the net handler has already checked is ours, at its packet, or, when its
    /// record was not `held` there (`0x491a93`), keeps it for the record's answer.
    pub(crate) fn push_receive(&mut self, p: &ItemPushResult, held: bool) {
        self.enqueue_receive(p, None, held);
    }

    /// Queues another group member's push, for its line alone, as [`Self::push_receive`] does
    /// the active player's.
    pub(crate) fn push_receive_other(&mut self, p: &ItemPushResult, held: bool) {
        self.enqueue_receive(p, Some(p.player_guid), held);
    }

    fn enqueue_receive(&mut self, p: &ItemPushResult, owner: Option<u64>, held: bool) {
        let r = PendingReceive {
            created: p.created,
            entry: p.item_entry,
            count: p.count,
            from_npc: p.from_npc,
            random_property_id: p.random_property_id,
            suffix_factor: p.suffix_factor,
            in_chat: p.show_in_chat,
            container: push_container(p.bag_slot, p.item_slot),
            owner,
        };
        if held {
            self.queue(Step::Push(r, self.window.clone()));
        } else {
            self.waiting.push(r);
        }
    }

    /// The close's teardown (`0x4c1cb0(0)`): the source goes (`0x4c1da0`) and the rows stay, and
    /// the lane signals `LOOT_CLOSED` (`0x4c1dac`) at the close's place. Queued pushes outlive it.
    pub(crate) fn clear(&mut self) {
        if self.clear_here() {
            self.queue(Step::Closed(self.window.clone()));
        }
    }

    /// The teardown of a close in place ([`close_interaction_in_place`]), once its unlocks have
    /// signalled: the source zeroed, then `LOOT_CLOSED` (`0x4c1da0`, `0x4c1dac`).
    fn tear_down_here(&mut self, script: &mut UiScript) {
        if self.clear_here() {
            announce_loot_closed(script);
        }
    }

    /// [`Self::clear`] for a call or a turn that signals the teardown's `LOOT_CLOSED` in place
    /// itself; whether a window was open.
    fn clear_here(&mut self) -> bool {
        let open = self.window.source.take().is_some();
        if open {
            self.coin_line_shown = false; // `0x4c1d95`
        }
        self.copier_due = false;
        self.sweep_owed = false;
        self.sweep_source_live = false;
        open
    }

    /// Session end: drops the window, the queued pushes and the owed unlocks.
    pub(crate) fn clear_session(&mut self) {
        *self = LootState::default();
    }

    /// `UnlockItem` on `guid` inside a packet's handler, on the lane at the packet's place, its
    /// `ITEM_LOCK_CHANGED` firing whether or not the guid names an item we hold (`0x495450`); a
    /// zero guid unlocks nothing (`0x49542d`).
    pub(crate) fn unlock_item_here(&mut self, guid: u64) {
        if guid != 0 {
            self.queue(Step::Unlocks(vec![(guid, false)], self.window.clone()));
        }
    }

    /// The lane's queued signals dropped, as though they fired long ago: for a test that seats an
    /// open window ahead of a close that must find no open still queued ahead of it.
    #[cfg(test)]
    pub(crate) fn signals_fired(&mut self) {
        self.steps.clear();
    }

    /// The lane's queued unlock steps, taken unfired, in order, each flagged when it needs a live
    /// item object.
    #[cfg(test)]
    pub(crate) fn take_queued_unlocks(&mut self) -> Vec<(u64, bool)> {
        let mut out = Vec::new();
        self.steps.retain(|(_, s)| match s {
            Step::Unlocks(u, _) => {
                out.extend(u.iter().copied());
                false
            }
            _ => true,
        });
        out
    }

    /// The guids the lane's queued unlock steps will unlock, in order.
    #[cfg(test)]
    pub(crate) fn queued_unlocks(&self) -> Vec<u64> {
        self.steps
            .iter()
            .filter_map(|(_, s)| match s {
                Step::Unlocks(u, _) => Some(u.iter().map(|&(g, _)| g)),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// Owe the window teardown's `UnlockItem` on its source, which runs only when the source
    /// resolves as a live item object (`0x48f299`/`0x48f2ab`, `0x4c1cdf`/`0x4c1d23`): checked
    /// where the unlocks run, against the objects.
    pub(crate) fn unlock_item_source(&mut self, guid: u64) {
        if benilla_protocol::guid::is_item(guid) {
            self.item_unlocks.push((guid, true));
        }
    }

    /// `n` of the window teardown's `UnlockItem`s on its source, on the lane at the caller's
    /// place, ahead of what it queues next: each runs only while the source resolves as a live
    /// item object (`0x48f299`/`0x48f2ab`, `0x4c1cdf`/`0x4c1d23`), checked as the step fires
    /// unless the caller knows it does ([`Source`]).
    fn queue_source_unlocks(&mut self, guid: u64, n: usize, source: Source) {
        if benilla_protocol::guid::is_item(guid) {
            let checked = source == Source::CheckedAtTheFire;
            self.queue(Step::Unlocks(vec![(guid, checked); n], self.window.clone()));
        }
    }

    /// The owed unlocks, in order, each flagged when it needs a live item object.
    pub(crate) fn take_item_unlocks(&mut self) -> Vec<(u64, bool)> {
        std::mem::take(&mut self.item_unlocks)
    }

    /// The last push queued, taken off the lane.
    #[cfg(test)]
    fn take_push(&mut self) -> Option<PendingReceive> {
        let at = self
            .steps
            .iter()
            .rposition(|(_, s)| matches!(s, Step::Push(..)))?;
        match self.steps.remove(at)?.1 {
            Step::Push(r, _) => Some(r),
            _ => None,
        }
    }

    /// Queued and waiting pushes, for the net handler's self-gate test.
    #[cfg(test)]
    pub(crate) fn pending_receive_count(&self) -> usize {
        let queued = self
            .steps
            .iter()
            .filter(|(_, s)| matches!(s, Step::Push(..)))
            .count();
        queued + self.waiting.len()
    }

    /// The open loot's guid; the chest lid watcher in [`crate::go_anim`] reads it too.
    pub(crate) fn source(&self) -> Option<u64> {
        self.window.source
    }

    /// `ClearLootRollState` (`0x5ebcf0`), from `SMSG_LOOT_ALL_PASSED` (`0x61b5dd`) and
    /// `SMSG_LOOT_ROLL_WON` (`0x61b97c`): the row at `slot` of the open source `guid` goes back to
    /// the type 0 its click re-reads (`0x4c28a9` → `0x5ebce0`), so the freed item loots.
    pub(crate) fn roll_resolved(&mut self, guid: u64, slot: u8) {
        if self.window.source != Some(guid) {
            return;
        }
        if let Some(row) = self.window.items.iter_mut().find(|i| i.slot == slot) {
            row.slot_type = benilla_protocol::messages::slot_type::ALLOW_LOOT;
        }
    }

    /// `LootHasContents` (`0x4c2a70`): the source resolves as any object (`0x4c2a8c`, mask 1) and
    /// has money above 0 or a row left.
    fn has_contents(&self, objects: &crate::net::Objects) -> bool {
        self.window
            .source
            .is_some_and(|source| objects.entity(source).is_some())
            && !self.window.is_empty()
    }

    /// [`Self::has_contents`] at a sweep's end (`0x4c2228`), which reads the source resolving
    /// when its close found it live ([`Self::sweep_source_live`]), the fact spent.
    pub(super) fn sweep_has_contents(&mut self, objects: &crate::net::Objects) -> bool {
        let live = std::mem::take(&mut self.sweep_source_live);
        self.window
            .source
            .is_some_and(|source| live || objects.entity(source).is_some())
            && !self.window.is_empty()
    }

    /// `0x48f200`'s early-out with `dl` clear (`0x48f245`-`0x48f26f`): a disenchant window
    /// (`0x4c2740` == 4) with contents (`0x4c2a70`, its source resolving) is not closed but swept
    /// (`0x48f264` → `0x4c1fa0`), with no release, no latch clear and no `LOOT_CLOSED`. Every
    /// close with `dl` clear tests it; the move-start close, `dl` set, returns instead
    /// (`0x48f25a`).
    pub(crate) fn disenchant_hold(&self, objects: &crate::net::Objects) -> bool {
        self.window.loot_type == benilla_protocol::messages::loot_type::DISENCHANTING
            && self.has_contents(objects)
    }

    /// [`Self::disenchant_hold`] for the selection teardown's close, which holds no object manager:
    /// a set source stands in for one that resolves (`0x4c2a8c`).
    pub(crate) fn disenchant_holds(&self) -> bool {
        self.window.source.is_some()
            && self.window.loot_type == benilla_protocol::messages::loot_type::DISENCHANTING
            && !self.window.is_empty()
    }

    /// `0xb71b44` above 0: a row's item record is still being fetched, counted from the copier
    /// (`0x4c1e77`) until its callback (`0x4c2ac0`); a negative answer releases the row, as
    /// [`Window::records_pending`] has it. Deviation: a row cleared meanwhile stops counting, where
    /// `ClearLootRow` (`0x4c1f00`) cancels its callback (`0x4c1f52`) and never lowers the count
    /// (its writers are `0x4c1def`, `0x4c1e94` and `0x4c2af6` alone), so in 1.12.1 a group
    /// member's take of an uncached item before the answer leaves that corpse's window shut until
    /// the next loot response resets the count (`0x4c1def`), and ours opens.
    fn records_pending(&self, items: &Items) -> bool {
        self.window.records_pending(items)
    }

    /// Owe an inventory failure's tail to the lane, behind its line and unlocks, with whether a
    /// window is open at the failure's packet (`0x5e3aff`).
    pub(crate) fn owe_failure_tail(&mut self) {
        self.queue(Step::FailureTail(self.window.clone()));
    }

    /// Owe the hold's sweep to the lane, from a close with no VM at hand.
    pub(crate) fn owe_sweep(&mut self) {
        self.sweep_owed = true;
        self.queue(Step::Sweep);
    }
}

/// The active player's auto-loot byte (`+0x1d30`): the copier's end sweeps while it is set
/// (`0x4c1ec7`, `0x4c2b1d`), and the sweep clears it (`0x4c2219`). Each loot interaction stores it
/// through `SetAutoLoot` (`0x5df460`), from the keys held then ([`interaction_byte`]).
#[derive(Resource, Default)]
pub(crate) struct AutoLootByte(pub(crate) bool);

/// The loot CVars. `showLootSpam` is 1.12's Detailed Loot Information checkbox (`0xb4e2bc`,
/// registered at `0x48fd1c`, default `"1"`), read only by the loot-roll line composers.
/// Deviation: `autoLootDefault` is benilla's own option, 1.12 auto-looting only on a shift-click,
/// and off is 1.12's behaviour. On, every window auto-loots unless Shift is held: an interaction
/// stores the byte inverted by it ([`interaction_byte`]), and a pickpocket or disenchant window,
/// which no interaction stores the byte for, takes it at its copier's end from the Shift held
/// then.
#[derive(Resource)]
pub(crate) struct LootConfig {
    pub(crate) auto_loot: bool,
    pub(crate) show_loot_spam: bool,
}

impl Default for LootConfig {
    fn default() -> Self {
        Self {
            // Off leaves 1.12's shift-click as the only auto-loot.
            auto_loot: false,
            // The reference's registered `"1"`.
            show_loot_spam: true,
        }
    }
}

/// The loot-target latch, the reference's `[player+0x1d28]` guid: a loot session is open on this
/// object; kneeling is [`LootKneel`]'s question. Armed at the `CMSG_LOOT` send
/// (`0x5df253`/`0x5df40d`), at `SMSG_SPELL_GO` for an `OPEN_LOCK` on a chest (`0x6e831b`), by an
/// admitted response (`0x5eb900`), and at the `CMSG_OPEN_ITEM` send (`0x5edcc0`), which is what
/// admits vmangos' type-1 answer for the item.
///
/// Deviation: our close paths clear it guid-matched, where `CloseInteraction` clears it
/// unconditionally (`0x48f2c9`), so closing an old window keeps a newer `CMSG_LOOT`'s latch.
#[derive(Resource, Default)]
pub(crate) struct LootLatch(pub(crate) Option<u64>);

/// Whether the character kneels at the latched object, the reference's predicate B `0x612710`:
/// yes for a corpse or any GameObject but a fishing bobber, no for a live unit, an item or an
/// unresolved guid. Recomputed each frame between the net drain and the anim driver, as the
/// reference force-plays the pose at the arm itself (`0x5ed619`).
#[derive(Resource, Default)]
pub(crate) struct LootKneel(pub(crate) bool);

/// `GAMEOBJECT_TYPE_ID` 17, `FISHINGNODE`: the one GameObject type that does not kneel
/// (`0x612772`).
const GO_TYPE_FISHINGNODE: i32 = 17;

/// Recomputes [`LootKneel`]; `pub(crate)` so [`crate::creature_anim`]'s driver can order after it.
pub(crate) fn resolve_loot_kneel(
    latch: Res<LootLatch>,
    index: Res<crate::net::GuidIndex>,
    objects: Query<(&crate::net::NetEntity, &crate::net::ObjectStore)>,
    mut kneel: ResMut<LootKneel>,
) {
    let allowed = latch
        .0
        .and_then(|guid| index.0.get(&guid).copied())
        .and_then(|e| objects.get(e).ok())
        .is_some_and(|(net_entity, store)| match net_entity.kind {
            // `0x612764`: a GameObject kneels unless it is the bobber.
            benilla_protocol::EntityKind::GameObject => {
                store.0.gameobject_type_id() != GO_TYPE_FISHINGNODE
            }
            // `0x61278c`: a unit kneels once its health reads 0 off the descriptor, where an
            // unsent field is 0 too; not `unit_is_dead`, whose `max_health` guard it lacks.
            benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player => {
                store.0.unit_health().unwrap_or(0) == 0
            }
            // An item, which the bags stream, never kneels (`0x612797`).
            _ => false,
        });
    if kneel.0 != allowed {
        kneel.0 = allowed;
    }
}

impl LootLatch {
    /// Drops the latch only if it still names `guid`.
    pub(crate) fn clear_for(&mut self, guid: u64) {
        if self.0 == Some(guid) {
            self.0 = None;
        }
    }
}

pub(crate) struct UiLootPlugin;

/// Applies a loot CVar change.
pub(crate) fn on_cvar(
    ev: &crate::cvars::CvarChanged,
    mut loot: bevy::prelude::Mut<'_, LootConfig>,
) {
    match ev.key().as_str() {
        "autolootdefault" => loot.auto_loot = ev.flag(),
        "showlootspam" => loot.show_loot_spam = ev.flag(),
        _ => {}
    }
}

impl Plugin for UiLootPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        use crate::lanes::LaneApp;
        app.init_resource::<LootState>()
            .add_lane("loot", peek_loot, fire_loot, None)
            .init_resource::<LootConfig>()
            .init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<AutoLootByte>()
            .add_sweep_leg(SweepLeg::Loot, close_loot_on_leave_world)
            .add_systems(
                Update,
                (
                    drop_loot_steps
                        .in_set(UiFeed)
                        .run_if(not(crate::ui_script::ingame_ui_up)),
                    // After the net drain and the selection writers, which arm and drop the
                    // latch; the anim driver orders after this.
                    resolve_loot_kneel
                        .after(benilla_world::schedule::WorldStage::Net)
                        .after(crate::target::TargetUpdate)
                        .after(crate::vm_pass::VmPass::End),
                ),
            );
    }
}

/// The window's signals readied while no in-game interface is up, taken unsignalled: there is no
/// interface to hear them, and the one that comes up later reads the window as it stands through
/// the host. The pushes wait for it, as they wait for a VM.
fn drop_loot_steps(mut loot: ResMut<LootState>) {
    loot.steps.retain(|(_, s)| matches!(s, Step::Push(..)));
}

/// The coin row's icon: the reference's six-step ladder (`0x4c2460` → `0x6c62d0`), shared with
/// `GetCoinIcon` and the money cursor.
fn coin_icon(copper: u32) -> &'static str {
    benilla_ui::script::coin_icon(i64::from(copper))
}

/// One wire row for the Lua: the icon from the wire display id, the rest from the item template.
/// The link's suffix factor is 0, as the wire's `randomSuffix` always is (`LootMgr.cpp:842`).
fn resolve_item(
    item: &LootItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    ask: Ask<'_>,
    rolls: RollCatalogs,
) -> LootRow {
    let (name, quality, link) = match items.template(item.item_id, 0, ask) {
        Some(t) => {
            // The rolled name: the reference composes every display of it through `0x5d8b00`.
            let name = rolls.name(&t.name, item.random_property_id);
            (
                Some(name.clone()),
                Some(t.quality),
                Some(crate::ui_items::item_link_full(
                    item.item_id,
                    0,
                    item.random_property_id,
                    0,
                    &name,
                    t.quality,
                )),
            )
        }
        None => (None, None, None),
    };
    let texture = icons
        .and_then(|i| i.catalog.get(item.display_info_id))
        .and_then(|d| d.icon.clone());
    LootRow {
        name,
        texture,
        quantity: item.count,
        quality,
        is_coin: false,
        money: 0,
        item_id: item.item_id,
        link,
        // The raw id, as the client's loot record keeps it; the tooltip resolves it (`0x52b7bf`).
        random_property_id: item.random_property_id,
    }
}

/// The coin row as the getters read it while the money is above 0: its copper, which
/// `GetLootSlotInfo` spells out at the call (`0x4c2550`), and the ladder's icon.
fn coin_row(gold: u32) -> LootRow {
    LootRow {
        // Spelled out at `GetLootSlotInfo`'s call, from the money cell (`0x4c2550`).
        name: None,
        money: gold,
        texture: Some(coin_icon(gold).into()),
        // 0: `0x4c22e0`'s coin guard returns before the record read (`0x4c22fd`).
        quantity: 0,
        quality: Some(COIN_QUALITY),
        is_coin: true,
        item_id: 0,
        // No item behind it: a modified click finds nil.
        link: None,
        random_property_id: 0,
    }
}

/// `OnItemPush`'s line: the template read through `FrameScript_GetText`, filled with the item's
/// link (`0x52adb0`) and the count, `_MULTIPLE` for a count above 1 by a signed compare (`jg`);
/// `None`, for no line, when the strings lack it. The active player's own push (`0x491bfb`, read
/// at `0x491c25` and `0x491c81`) takes created over pushed by an NPC over looted
/// (`0x491c04`-`0x491c1b`); another member's (`0x491dbc`, read at `0x491dd5` and `0x491e2e`)
/// takes `CREATED_ITEM` or `LOOT_ITEM`, with `owner_name`, their `GetUnitName`, first
/// (`0x491dfe`, `0x491e5b`).
fn receive_line(
    r: &PendingReceive,
    name: &str,
    quality: u32,
    owner_name: Option<&str>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use benilla_ui::strings::{fill, Arg};
    let multiple = r.count as i32 > 1;
    let key = match (owner_name.is_some(), r.created, r.from_npc, multiple) {
        (false, true, _, false) => "LOOT_ITEM_CREATED_SELF",
        (false, true, _, true) => "LOOT_ITEM_CREATED_SELF_MULTIPLE",
        (false, false, true, false) => "LOOT_ITEM_PUSHED_SELF",
        (false, false, true, true) => "LOOT_ITEM_PUSHED_SELF_MULTIPLE",
        (false, false, false, false) => "LOOT_ITEM_SELF",
        (false, false, false, true) => "LOOT_ITEM_SELF_MULTIPLE",
        (true, true, _, false) => "CREATED_ITEM",
        (true, true, _, true) => "CREATED_ITEM_MULTIPLE",
        (true, false, _, false) => "LOOT_ITEM",
        (true, false, _, true) => "LOOT_ITEM_MULTIPLE",
    };
    let link = crate::ui_items::item_link_full(
        r.entry,
        0,
        r.random_property_id,
        r.suffix_factor,
        name,
        quality,
    );
    let count = Arg::D(i64::from(r.count as i32));
    let args = match owner_name {
        Some(owner) => vec![Arg::S(owner), Arg::S(&link), count],
        None => vec![Arg::S(&link), count],
    };
    let text = fill(&get(key)?, &args);
    (!text.is_empty()).then_some(text)
}

fn drain_receives(
    pending: Vec<PendingReceive>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    chat: &crate::ui_chat::ChatLog,
    script: &mut UiScript,
    rolls: RollCatalogs,
    owner: &dyn Fn(u64) -> Option<Option<String>>,
    received: &dyn Fn(&PendingReceive, &benilla_protocol::ItemInfo),
) -> Vec<PendingReceive> {
    let mut still = Vec::new();
    for r in pending {
        // The push's lookup with the callback `0x491ee0` (`0x491b05`).
        let resolved = items
            .template(r.entry, 0, Ask::Event(commands))
            .map(|t| (t.name.clone(), t.quality, t.display_info_id));
        match resolved {
            Some((name, quality, _)) if r.owner.is_some() => {
                // Another member's push: a pusher the client holds as a player (`0x491b2c`, mask
                // 0x10), named by `GetUnitName` with its `UNKNOWNOBJECT` miss; no `ITEM_PUSH`.
                let Some(owner) = r.owner.and_then(owner) else {
                    continue;
                };
                let owner = owner.unwrap_or_else(|| script.unknown_object_text());
                let line = r.in_chat.then(|| {
                    receive_line(
                        &r,
                        &rolls.name(&name, r.random_property_id),
                        quality,
                        Some(&owner),
                        &|key| benilla_ui::strings::global(script.lua(), key),
                    )
                });
                if let Some(text) = line.flatten() {
                    chat.push_event(crate::ui_chat::ChatEvent::text_only(
                        crate::ui_chat::ChatEventKind::Loot,
                        text,
                    ));
                }
            }
            Some((name, quality, display_id)) => {
                // Ungated by `in_chat`. `arg2` is the icon path the reference builds from the
                // display record (`0x491baa`).
                let icon = icons
                    .and_then(|i| i.catalog.get(display_id))
                    .and_then(|d| d.icon.clone());
                if let Some(icon) = icon {
                    debug!(
                        "ui_loot: ITEM_PUSH container {} icon {icon} (item {})",
                        r.container, r.entry
                    );
                    script.fire_event(
                        "ITEM_PUSH",
                        vec![ScriptValue::Int(r.container), ScriptValue::Str(icon)],
                    );
                }
                let line = r.in_chat.then(|| {
                    receive_line(
                        &r,
                        &rolls.name(&name, r.random_property_id),
                        quality,
                        None,
                        &|key| benilla_ui::strings::global(script.lua(), key),
                    )
                });
                if let Some(text) = line.flatten() {
                    chat.push_event(crate::ui_chat::ChatEvent::text_only(
                        crate::ui_chat::ChatEventKind::Loot,
                        text,
                    ));
                }
                // The tutorials after the line (`0x491cdd`, then `0x491cec`).
                if let Some(info) = items.template(r.entry, 0, Ask::Read) {
                    received(&r, info);
                }
            }
            // A record dropped since it was held (`0x491a93`): its callback waits on the answer.
            None => {
                items.wait(r.entry, crate::items::RecordCallback::Push);
                still.push(r);
            }
        }
    }
    still
}

/// The object manager as `SetLootPortrait` asks it (`0x468460`, typemask 8): whether the loot
/// source is a unit object we hold.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct LootSourceObjects<'w, 's> {
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
}

impl LootSourceObjects<'_, '_> {
    /// A player the object manager holds (`0x468460`, typemask 0x10, `0x491b2c`).
    fn player(&self, guid: u64) -> Option<&crate::net::ObjectStore> {
        self.index
            .as_ref()
            .and_then(|index| index.0.get(&guid))
            .and_then(|&e| self.stores.get(e).ok())
            .filter(|s| s.0.object_type_mask().unwrap_or(0) & 0x10 != 0)
    }

    fn is_unit(&self, guid: u64) -> bool {
        self.index
            .as_ref()
            .and_then(|index| index.0.get(&guid))
            .and_then(|&e| self.stores.get(e).ok())
            .is_some_and(|s| s.is_unit())
    }
}

/// What the loot lane's firing reads beside the session: the item records a response's copier
/// asks after and a push resolves through, its icons, the roll catalogs, and the socket, the chat,
/// the names and the object manager a push asks and speaks through.
#[derive(bevy::ecs::system::SystemParam)]
struct LootReads<'w, 's> {
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    commands: Res<'w, NetCommands>,
    chat: Res<'w, crate::ui_chat::ChatLog>,
    // The random-suffix catalogs: a loot slot has no item object to carry the roll.
    rolls: crate::items::Rolls<'w>,
    names: Res<'w, NameCache>,
    objects: LootSourceObjects<'w, 's>,
    // Our own pushes' tutorials and the class the ranged one reads (`0x491ebc`); absent in a
    // harness without the tutorials.
    tutorials: Option<Res<'w, crate::tutorial::Tutorials>>,
    me: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
}

impl LootReads<'_, '_> {
    /// The copier's asks for a window no handler asked for (`0x4c1e77`), each row's record with
    /// none held asked for; whether any is still pending.
    fn ask_rows(&self, w: &Window) -> bool {
        for it in w.items.iter().filter(|it| !w.taken.contains(&it.slot)) {
            let _ = self
                .items
                .template(it.item_id, 0, Ask::Event(&self.commands));
        }
        w.records_pending(&self.items)
    }

    fn pushes(&self) -> PushInputs<'_> {
        PushInputs {
            items: &self.items,
            icons: self.icons.as_deref(),
            commands: &self.commands,
            chat: &self.chat,
            rolls: self.rolls.get(),
            names: &self.names,
            objects: &self.objects,
            tutorials: self.tutorials.as_deref(),
            class: self.me.single().ok().and_then(|s| s.0.unit_class()),
        }
    }
}

/// The loot window's VM and session.
type LootVm<'w, 's> = VmWith<'w, 's, (LootSteps<'static, 'static>, ())>;

/// The loot lane's head ([`crate::lanes`]).
fn peek_loot(world: &World) -> Option<crate::lanes::Stamp> {
    world.get_resource::<LootState>()?.peek()
}

/// The loot lane's `fire_until`: each step `until` admits, a turn whose handlers run as it ends;
/// the lane stops after a step whose firing stamped anything, so what it pushed fires next, and at
/// a push with no VM to hear it, which waits there for one. Each packet's events fire inside its
/// dispatch (`0x537aa0`), so `ITEM_PUSH` (`0x491be8`) reads the window the packets before it left.
fn fire_loot(In(until): In<crate::lanes::Until>, mut vm: LootVm, reads: LootReads) {
    loop {
        let push_next = matches!(vm.own().0.loot.steps.front(), Some((_, Step::Push(..))));
        if push_next && !vm.run(|script| script.is_some()) {
            return;
        }
        let Some((stamp, step)) = vm.own().0.loot.pop(until) else {
            return;
        };
        let ((), moved) = crate::lanes::firing(stamp, || fire_step(&mut vm, &reads, step));
        if moved {
            return;
        }
    }
}

/// Every step waiting, fired alone, for a test that drives the loot window without the lanes.
#[cfg(test)]
pub(crate) fn fire_loot_lane(world: &mut World) {
    use crate::lanes::Until;
    while let Some(head) = world
        .resource::<LootState>()
        .peek()
        .filter(|&s| Until::all().admits(s))
    {
        world
            .run_system_cached_with(fire_loot, Until::all())
            .expect("the loot lane fires");
        if world.resource::<LootState>().peek() == Some(head) {
            return; // a push waiting for a VM
        }
    }
}

/// The teardown's `LOOT_CLOSED` (`0x4c1dac`) in a turn of its own.
fn signal_closed(vm: &mut LootVm) {
    vm.turn(|script, _| {
        if let Some(script) = script {
            announce_loot_closed(script);
        }
    });
}

/// Run `f`, a step's turns, on `w`, the window as the step's packet left it, which its listeners
/// read through the host, as the reference's ran inside that packet's dispatch before the next
/// arrived; then the live window back ([`restore_live`]).
fn on_its_window(vm: &mut LootVm, w: Window, f: impl FnOnce(&mut LootVm)) {
    let live = swap_in_cells(vm, w);
    f(vm);
    restore_live(vm, live, true);
}

/// One step, on the window its packet left ([`on_its_window`]); a step of a session a later
/// response replaced runs on all the cells that session left. A response's step ends with its
/// copier's end.
fn fire_step(vm: &mut LootVm, reads: &LootReads, step: Step) {
    match step {
        Step::Cleared { row, left } => on_its_window(vm, left, |vm| {
            vm.turn(|script, _| {
                if let Some(script) = script {
                    script.fire_event("LOOT_SLOT_CLEARED", vec![ScriptValue::Int(i64::from(row))]);
                }
            });
        }),
        Step::Closed(left) => on_its_window(vm, left, signal_closed),
        Step::CandidateNamed(at) => on_its_window(vm, at, |vm| {
            vm.turn(|script, _| {
                if let Some(script) = script {
                    script.fire_event("UPDATE_MASTER_LOOT_LIST", vec![]);
                }
            });
        }),
        Step::Unlocks(unlocks, at) => on_its_window(vm, at, |vm| {
            vm.turn(|script, (steps, _)| {
                if let Some(script) = script {
                    fire_unlocks(script, unlocks, &steps.objects);
                }
            });
        }),
        Step::Opened {
            ended,
            window: w,
            ends,
            prior,
            moving,
        } => {
            let replaced = vm.own().0.loot.replaced();
            // The copier ends the session in progress (`0x4c1ccb`-`0x4c1dac`): its
            // `LOOT_CLOSED` handlers run on the cells the teardown leaves, the source zeroed,
            // before the new source is stored (`0x4c1dc2`), so their `CloseLoot` finds no
            // window (`0x48f23f`). The stash is the ended session's until the clear after them
            // (`0x4c1df5`); the latch is already the response's (`0x5ebb60`).
            let live = swap_in_cells(vm, ended);
            vm.own().0.loot.pending_bind_confirm = prior.stash;
            if prior.open {
                signal_closed(vm);
            }
            vm.own().0.loot.pending_bind_confirm = None;
            restore_live(vm, live, false);
            let ends = vm.turn(|script, (mut steps, _)| {
                // With no VM the copier's end tells no one; the close behind it still runs.
                if script.is_none() {
                    return moving;
                }
                // A window no handler asked for (`0x4c1e77`) decides here, on the session as it
                // stands.
                ends.unwrap_or_else(|| {
                    let ends = steps.loot.copier_due && !reads.ask_rows(&w);
                    steps.loot.copier_due &= !ends;
                    ends
                })
            });
            if ends {
                end_session_open(vm, w, replaced, moving);
            }
        }
        // The response's own close ran in its handler (`0x5ebc51`), whatever the copier left.
        Step::CopierEnd(w) => {
            if vm.turn(|script, _| script.is_some()) {
                let replaced = vm.own().0.loot.replaced();
                end_session_open(vm, w, replaced, false);
            }
        }
        Step::FailureTail(at) => {
            let window_open = at.source.is_some();
            on_its_window(vm, at, |vm| {
                vm.turn(|script, (steps, _)| {
                    let open = window_open && steps.objects.player().is_some();
                    if let (Some(script), true) = (script, open && steps.byte.0) {
                        script.fire_event("LOOT_OPENED", vec![]);
                    }
                });
            });
        }
        Step::Sweep => {
            if !vm.own().0.loot.sweep_owed {
                return;
            }
            vm.turn(|script, (mut steps, _)| {
                steps.loot.sweep_owed = false;
                if let (Some(script), true) = (script, steps.loot.source().is_some()) {
                    steps.sweep(script, false);
                }
            });
            run_then(vm);
            // A sweep that stopped short of its end leaves the fact to no other.
            let (mut steps, _) = vm.own();
            steps.loot.sweep_source_live = false;
        }
        Step::Push(r, at) => on_its_window(vm, at, |vm| {
            let still = emit_pushes(vm, vec![r], &reads.pushes());
            vm.own().0.loot.waiting.extend(still);
        }),
    }
}

/// A copier's end on its session `own`, with the move-start close its response's tail owes behind
/// it when `moving`: one a later response in the frame replaced on the cells it left
/// ([`end_superseded_open`]), else on its window as its packet left it, what the end does to it
/// kept ([`on_its_window`]).
fn end_session_open(vm: &mut LootVm, own: Window, replaced: bool, moving: bool) {
    if replaced {
        end_superseded_open(vm, own, moving);
    } else {
        on_its_window(vm, own, |vm| end_open(vm, moving));
    }
}

/// The copier end of a response a later one in the same frame replaced, which the reference ran
/// inside its own packet's dispatch (`0x4c1ea8`-`0x4c1ee6`) before the next arrived: on its own
/// window, which the live cells take for the while, then the next copier's resets over what it
/// left (`0x4c1d95`, `0x4c1def`, `0x4c1df5`). What it closes it releases; its teardown's step is
/// the replacing window's to show.
fn end_superseded_open<X: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<(LootSteps<'static, 'static>, X)>,
    own: Window,
    moving: bool,
) {
    let live = swap_in_cells(vm, own);
    end_open(vm, moving);
    restore_live(vm, live, true);
    let (mut s, _) = vm.own();
    let loot = &mut *s.loot;
    loot.coin_line_shown = false;
    loot.sweep_owed = false;
    loot.sweep_source_live = false;
    loot.pending_bind_confirm = None;
}

/// The live window and the clock's mark, set aside while a step runs on the cells it left.
struct Live {
    window: Window,
    /// [`LootState::closed_since`] as it stood, for a step inside another's turn.
    closed_since: Option<u64>,
    mark: crate::lanes::Mark,
    /// The replaced session's other cells were swapped in ([`exchange_replaced_cells`]).
    cells: bool,
    /// The source of the window swapped in, as it went in.
    source: Option<u64>,
}

/// Put `cells`, the loot window as a step left it, in the window's place for that step's turn,
/// and, for a session a later response replaced, the latch, the stash and the copier's wait it
/// left ([`Cells`]).
fn swap_in_cells<X: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<(LootSteps<'static, 'static>, X)>,
    cells: Window,
) -> Live {
    let (mut s, _) = vm.own();
    let source = cells.source;
    let closed = source.filter(|_| !s.loot.session_open(&cells));
    Live {
        closed_since: std::mem::replace(&mut s.loot.closed_since, closed),
        window: std::mem::replace(&mut s.loot.window, cells),
        mark: crate::lanes::mark(),
        cells: exchange_replaced_cells(&mut s),
        source,
    }
}

/// Exchange the live [`Cells`] with those the first queued response holds for the session it
/// replaces; run twice it puts the live ones back and keeps what the turn left in the replaced
/// session's, for its next step. Whether a response is queued.
fn exchange_replaced_cells(s: &mut LootSteps) -> bool {
    let Some(mut cells) = s.loot.replaced_cells().copied() else {
        return false;
    };
    std::mem::swap(&mut s.latch.0, &mut cells.latch);
    std::mem::swap(&mut s.loot.pending_bind_confirm, &mut cells.stash);
    std::mem::swap(&mut s.loot.copier_due, &mut cells.copier_due);
    if let Some(prior) = s.loot.replaced_cells() {
        *prior = cells;
    }
    true
}

/// Put the live cells back. For a session a later response replaced, the later copier rewrote
/// every cell (`0x4c1dc2` on), so what the step's handlers changed in them goes, with the steps
/// their calls queued; for a step on that session's own window (`own`), a close its turn made, a
/// listener's `CloseLoot` or the emptied window's, is kept for the copier's teardown
/// ([`Cells::open`]). For the live session, the window the later packets left comes back,
/// and a close the turn made, the one change a listener's call makes to the window, stands on it.
fn restore_live<X: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<(LootSteps<'static, 'static>, X)>,
    live: Live,
    own: bool,
) {
    let (mut s, _) = vm.own();
    let open = s.loot.window.source.is_some();
    s.loot.window = live.window;
    s.loot.closed_since = live.closed_since;
    if !live.cells {
        if live.source.is_some() && s.loot.window.source != live.source {
            // A close since took the session, its teardown's flag clear (`0x4c1d95`) after
            // anything the turn's sweep set (`0x4c221e`).
            s.loot.coin_line_shown = false;
        }
        if live.source.is_some() && !open && s.loot.window.source == live.source {
            s.loot.window.source = None;
        }
        return;
    }
    if own && live.source.is_some() && !open {
        if let Some(prior) = s.loot.replaced_cells() {
            prior.open = false;
        }
    }
    exchange_replaced_cells(&mut s);
    s.loot.steps.retain(|(stamp, _)| !stamp.since(live.mark));
}

/// A response's end, once its window is filled: the copier's end ([`LootSteps::copier_end`]) and
/// what its signals owe, then the response's tail (`0x5ebc3a`-`0x5ebc51`): a window opened while
/// we move (`moving`, [`LootState::close_after_the_copier`]) takes the move-start close in the
/// same handler, so it never paints. A session a later packet's handler has closed
/// ([`LootState::closed_since`]) was released there, so its end sends nothing after that release:
/// no sweep and no second close.
fn end_open<X: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<(LootSteps<'static, 'static>, X)>,
    moving: bool,
) {
    vm.turn(|script, (mut steps, _)| {
        if let Some(script) = script {
            steps.copier_end(script);
        }
    });
    run_then(vm);
    if !moving || vm.own().0.loot.window_closed_since() {
        return;
    }
    vm.turn(|script, (mut steps, _)| {
        let closed = move_start_close(
            &mut steps.loot,
            &mut steps.latch,
            &steps.net,
            Some(&steps.items),
            Unlocks::InPlace,
        );
        if closed.is_some() {
            steps.dead.after_close(closed);
            match script {
                Some(script) => steps.loot.tear_down_here(script),
                None => {
                    steps.loot.clear_here();
                }
            }
        }
    });
}

/// Run what the steps owe once their signals' handlers have run ([`calls::Then`]), each a turn
/// of its own, innermost first.
fn run_then<X: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<(LootSteps<'static, 'static>, X)>,
) {
    // Read before it is taken, so a turn with nothing owed leaves the window unchanged.
    while !vm.own().0.loot.then.is_empty() {
        let Some(then) = vm.own().0.loot.then.pop() else {
            break;
        };
        vm.turn(|script, (mut steps, _)| {
            if let Some(script) = script {
                steps.then(script, then);
            }
        });
    }
}

/// What a push's firing reads.
struct PushInputs<'a> {
    items: &'a Items,
    icons: Option<&'a ItemDisplays>,
    commands: &'a NetCommands,
    chat: &'a crate::ui_chat::ChatLog,
    rolls: RollCatalogs<'a>,
    names: &'a NameCache,
    objects: &'a LootSourceObjects<'a, 'a>,
    tutorials: Option<&'a crate::tutorial::Tutorials>,
    class: Option<u8>,
}

/// Fire `batch` as one turn ([`drain_receives`]); what still waits on its template comes back.
fn emit_pushes<P: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<'_, '_, P>,
    batch: Vec<PendingReceive>,
    with: &PushInputs,
) -> Vec<PendingReceive> {
    if batch.is_empty() {
        return batch;
    }
    vm.turn(|script, _| match script {
        Some(script) => drain_receives(
            batch,
            with.items,
            with.icons,
            with.commands,
            with.chat,
            script,
            with.rolls,
            &|guid| {
                let store = with.objects.player(guid)?;
                Some(
                    with.names
                        .resolve_unit(guid, Some(store), Ask::Event(with.commands))
                        .map(str::to_string),
                )
            },
            &|r, info| {
                let Some(tutorials) = with.tutorials else {
                    return;
                };
                let keyring = r.container == KEYRING_CONTAINER;
                for id in crate::tutorial::item_tutorials(r.entry, keyring, info, with.class) {
                    tutorials.trigger_now(id);
                }
            },
        ),
        None => batch,
    })
}

/// The frame's loot lane, as the drain runs it, for a test that drives the window without the
/// lanes.
#[cfg(test)]
pub(crate) fn loot_frame(world: &mut World) {
    fire_loot_lane(world);
}

/// [`loot_frame`] over a seated world, for a test that drives the window from outside the module.
#[cfg(test)]
pub(crate) fn feed_loot_once(world: &mut World) {
    seat_loot_steps(world);
    loot_frame(world);
}

/// Seat what [`LootSteps`] reads that a harness's world need not hold; the commands are the
/// harness's own.
#[cfg(test)]
pub(crate) fn seat_loot_steps(world: &mut World) {
    use bevy::ecs::message::Messages;
    world.init_resource::<LootState>();
    world.init_resource::<LootLatch>();
    world.init_resource::<AutoLootByte>();
    world.init_resource::<LootConfig>();
    world.init_resource::<GroupState>();
    world.init_resource::<crate::ui_chat::ChatLog>();
    world.init_resource::<crate::ui_action::UiErrorKeys>();
    world.init_resource::<crate::net::GuidIndex>();
    world.init_resource::<Items>();
    world.init_resource::<Messages<crate::sound::LootPickupSound>>();
    world.init_resource::<crate::target::OwedSelects>();
}

/// Whether a take must raise `LOOT_BIND_CONFIRM` first (`0x4c28f2`/`0x4c28fb`): bind on pickup
/// and uncommon or better. An unresolved template answers false, as the reference's cache peek.
fn bind_confirm_required(items: &Items, item_id: u32) -> bool {
    items
        .template(item_id, 0, Ask::Read)
        .is_some_and(|t| t.bonding == BIND_WHEN_PICKED_UP && t.quality >= BIND_CONFIRM_MIN_QUALITY)
}

/// `CloseInteraction 0x48f200` past its early returns, the one close every loot close shares: the
/// latch drops (`0x48f2c9`), the release goes out when `release` is given (the `cl` argument,
/// `0x48f2da`) and the frame closes (`0x48f33d`). An item source's two unlocks and the teardown's
/// `LOOT_CLOSED` go on the lane at the close's place, so a packet's close signals them inside its
/// packet and an engine step's at its own, behind what the lane already holds. Returns the closed
/// source for [`DeadUnitDeselect`], the close's last step.
pub(crate) fn close_interaction(
    loot: &mut LootState,
    latch: &mut LootLatch,
    release: Option<&NetCommands>,
    items: Option<&Items>,
) -> Option<u64> {
    close_with(
        loot,
        latch,
        release,
        items,
        Unlocks::OnTheLane(Source::CheckedAtTheFire),
    )
}

/// `EndLootWindow 0x48f200(1, 0, 0)` from an engine step: nothing with no window (`0x48f23f`); a
/// disenchant window with rows left owes its sweep (`0x48f24f`, `0x48f264`); any other closes with
/// its release ([`close_interaction`]). The selection teardown's (`0x493974`) and our own death's
/// (`0x5e9a5c`).
pub(crate) fn end_loot_window(
    loot: &mut LootState,
    latch: &mut LootLatch,
    net: &NetCommands,
    items: Option<&Items>,
) {
    end_window_with(loot, latch, net, items, Source::CheckedAtTheFire);
}

/// [`end_loot_window`] from the item source's own deactivate (`0x5d869b`), which runs while the
/// item is still in the live table (`0x46496f`, ahead of its unlink after `0x464974`): both of
/// the close's `UnlockItem`s find it (`0x48f299`, `0x4c1cdf`), though the release has unlinked it
/// by the time the lane fires.
pub(crate) fn end_loot_window_on_its_item(
    loot: &mut LootState,
    latch: &mut LootLatch,
    net: &NetCommands,
    items: Option<&Items>,
) {
    end_window_with(loot, latch, net, items, Source::LiveAtTheClose);
}

fn end_window_with(
    loot: &mut LootState,
    latch: &mut LootLatch,
    net: &NetCommands,
    items: Option<&Items>,
    source: Source,
) {
    if loot.disenchant_holds() {
        loot.owe_sweep();
        loot.sweep_source_live = source == Source::LiveAtTheClose;
        return;
    }
    close_with(loot, latch, Some(net), items, Unlocks::OnTheLane(source));
}

/// [`close_interaction`] for a call or a turn that signals the unlocks and the teardown in place
/// itself, after the line the close's flag shows (`0x48f27b`): the unlocks wait in the session's
/// own list ([`unlock_closed_source`]), signalled with the window still open, and the teardown,
/// the source zeroed (`0x4c1da0`) and `LOOT_CLOSED` (`0x4c1dac`), is the caller's after them
/// ([`LootState::tear_down_here`]).
pub(crate) fn close_interaction_in_place(
    loot: &mut LootState,
    latch: &mut LootLatch,
    release: Option<&NetCommands>,
    items: Option<&Items>,
) -> Option<u64> {
    close_with(loot, latch, release, items, Unlocks::InPlace)
}

/// Where a close's two unlocks go.
#[derive(Clone, Copy)]
enum Unlocks {
    OnTheLane(Source),
    InPlace,
}

/// Where a window source's unlocks learn whether it resolves as a live item object.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    /// As the step fires.
    CheckedAtTheFire,
    /// It does at the close's place, its caller knows.
    LiveAtTheClose,
}

fn close_with(
    loot: &mut LootState,
    latch: &mut LootLatch,
    release: Option<&NetCommands>,
    items: Option<&Items>,
    unlocks: Unlocks,
) -> Option<u64> {
    let source = loot.source()?;
    latch.clear_for(source);
    // The teardown (`0x48f33d` → `0x4c1cb0`) cancels each row's pending fetch
    // (`0x4c1d40`-`0x4c1d68`), so a record's answer reaches no closed window.
    if let Some(items) = items {
        items.cancel_waits(crate::items::RecordCallback::LootRow);
    }
    if let Some(commands) = release {
        let _ = commands.0.send(ClientCommand::LootRelease { guid: source });
    }
    // An item source unlocks twice, each with its `ITEM_LOCK_CHANGED`: the close's own
    // `UnlockItem` (`0x48f2bd`), then the frame teardown's (`0x4c1d36`), both ahead of the
    // teardown's `LOOT_CLOSED` (`0x4c1dac`). A move-start close, the one with `dl` set, returns
    // before both and never reaches here with an item.
    match unlocks {
        Unlocks::OnTheLane(live) => {
            loot.queue_source_unlocks(source, 2, live);
            loot.clear();
        }
        Unlocks::InPlace => {
            loot.unlock_item_source(source);
            loot.unlock_item_source(source);
        }
    }
    Some(source)
}

/// The leave-world sweep's loot leg (`0x490ab2` → `0x4c1ca0` → `0x48f200(cl=1, dl=0, 0)`): the
/// shared close with its `CMSG_LOOT_RELEASE` (`0x48f2da`), told to the VM the sweep runs in, the
/// dying one on a `/reload` ([`tell_loot_closed`]). The dead-unit deselect follows, as the
/// close's last step. A disenchant window with contents is swept instead
/// ([`LootState::disenchant_hold`]). A sweep run while the lane still holds an earlier signal,
/// the open of the window it closes among them, closes the session at once and queues its unlocks,
/// its `LOOT_CLOSED` or its sweep behind them ([`crate::lanes::waits_behind`]), so no close fires
/// ahead of the open it closes.
pub(crate) fn close_loot_on_leave_world(mut vm: VmWith<(LootSteps<'static, 'static>, ())>) {
    if crate::lanes::waits_behind(vm.own().0.loot.peek()) {
        let (mut steps, _) = vm.own();
        if steps.loot.disenchant_hold(&steps.objects) {
            steps.loot.owe_sweep();
            return;
        }
        let closed = close_interaction(
            &mut steps.loot,
            &mut steps.latch,
            Some(&steps.net),
            Some(&steps.items),
        );
        steps.dead.after_close(closed);
        return;
    }
    let closed = vm.turn(|script, (mut steps, _)| {
        if steps.loot.disenchant_hold(&steps.objects) {
            match script {
                Some(script) => steps.sweep(script, false),
                None => steps.loot.owe_sweep(),
            }
            return None;
        }
        let closed = close_interaction_in_place(
            &mut steps.loot,
            &mut steps.latch,
            Some(&steps.net),
            Some(&steps.items),
        )?;
        if let Some(script) = script {
            unlock_closed_source(script, &mut steps.loot, &steps.objects);
        }
        Some(closed)
    });
    run_then(&mut vm);
    if closed.is_none() {
        return;
    }
    vm.turn(|script, (mut steps, _)| {
        match script {
            Some(script) => steps.loot.tear_down_here(script),
            None => {
                steps.loot.clear_here();
            }
        }
        steps.dead.after_close(closed);
    })
}

/// Tell the VM in the world that a close whose teardown signals nowhere else closed the window: the
/// UI shutdown's deselect closing the loot on its target (`0x490bdf` → `0x493910` → `0x48f200`).
/// That close is on the selected unit, never an item, so it owes no unlock (`0x48f2ab`).
pub(crate) fn tell_loot_closed(mut vm: VmWith<()>) {
    vm.turn(|script, _| {
        if let Some(script) = script {
            announce_loot_closed(script);
        }
    })
}

/// The VM's half of a close that signals in place, first: an item source's
/// two unlocks (`0x48f2bd`, `0x4c1d36`), each resolved as the close's own lookup (`0x48f29e`). Both
/// come before the teardown zeroes the source (`0x4c1cb0`), so their handlers read the window
/// open, and run as this turn ends, ahead of [`announce_loot_closed`].
fn unlock_closed_source(
    script: &mut UiScript,
    loot: &mut LootState,
    objects: &crate::net::Objects,
) {
    fire_unlocks(script, loot.take_item_unlocks(), objects);
}

/// `UnlockItem` (`0x495420`) on each guid, with its `ITEM_LOCK_CHANGED`; one flagged as a
/// window source's only while it resolves as a live item object.
fn fire_unlocks(script: &mut UiScript, unlocks: Vec<(u64, bool)>, objects: &crate::net::Objects) {
    for (guid, live) in unlocks {
        if !live || objects.object(guid).is_some_and(|f| f.is_item()) {
            script.unlock_item(guid);
        }
    }
}

/// The close's teardown's `LOOT_CLOSED` (`0x4c1dac`), signalled in place.
fn announce_loot_closed(script: &mut UiScript) {
    script.fire_event("LOOT_CLOSED", vec![]);
}

/// `CloseInteraction`'s last step (`0x48f34f`–`0x48f369`): a closed source that is a unit, a
/// player included, with `UNIT_FIELD_HEALTH` at or below 0 is deselected if it is still the
/// selection (`0x493910(guid, 1)`). It reads the health, never the loot type, so a skinned corpse
/// loses the selection and a live pickpocket target keeps it; an unstreamed source is left alone.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DeadUnitDeselect<'w, 's> {
    index: Res<'w, crate::net::GuidIndex>,
    objects: Query<
        'w,
        's,
        (
            &'static crate::net::NetEntity,
            &'static crate::net::ObjectStore,
        ),
    >,
    /// Where the deselect is owed ([`crate::target::OwedSelect::Deselect`]); absent in a harness
    /// without targets.
    owed: Option<ResMut<'w, crate::target::OwedSelects>>,
}

impl DeadUnitDeselect<'_, '_> {
    pub(crate) fn after_close(&mut self, closed: Option<u64>) {
        if let Some(source) = self.dead_unit(closed) {
            self.deselect(source);
        }
    }

    /// The closed source, when it is a dead unit.
    pub(super) fn dead_unit(&self, closed: Option<u64>) -> Option<u64> {
        let source = closed?;
        self.index
            .0
            .get(&source)
            .and_then(|&e| self.objects.get(e).ok())
            .is_some_and(|(net_entity, store)| {
                matches!(
                    net_entity.kind,
                    benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player
                ) && (store.0.unit_health().unwrap_or(0) as i32) <= 0 // signed, `jg` at `0x48f360`
            })
            .then_some(source)
    }

    /// Owe `0x493910(source, 1)`.
    pub(super) fn deselect(&mut self, source: u64) {
        if let Some(owed) = self.owed.as_deref_mut() {
            owed.deselect(source);
        }
    }

    /// Where a handler that owes its own selects owes them, beside this deselect.
    pub(crate) fn owed_selects(&mut self) -> Option<&mut crate::target::OwedSelects> {
        self.owed.as_deref_mut()
    }
}

/// `CloseInteraction 0x48f200(cl=1, dl=1, 0)` on a movement start. A disenchant window with rows
/// left (`0x48f24a`) and an item's loot (`0x48f2ab`) survive; anything else takes the shared close,
/// its teardown's `LOOT_CLOSED` on the lane.
fn close_on_move_start(
    loot: &mut LootState,
    latch: &mut LootLatch,
    commands: &NetCommands,
    items: Option<&Items>,
) -> Option<u64> {
    move_start_close(
        loot,
        latch,
        commands,
        items,
        Unlocks::OnTheLane(Source::CheckedAtTheFire),
    )
}

/// `0x48f200(1, 1, 0)` while a loot target is latched (`0x6126b0`), where its caller runs it: the
/// movement guard `OnMoveInput 0x60e990` with `keepsLoot` clear (`0x60e9ba`-`0x60e9dd`), which every
/// movement-start emitter calls, turning included, but not mouse-look facing, inside the start
/// (`0x60dc6a`, ahead of the mover's queue at `0x60dc7d`); and the local stand-state apply for a
/// change to any state but 0 (`0x5f07d5`). [`close_on_move_start`] and its dead source's deselect,
/// the engine half there, `LOOT_CLOSED` on the loot lane at that place. No loot target reaches the
/// range gate `0x493230`, so there is no distance leash. vmangos also releases on movement
/// (`MovementHandler.cpp:1104`), but the close is the client's. A loot response's own on the move
/// (`0x5ebc3a`-`0x5ebc51`) runs in its handler or behind its copier's end ([`end_open`]), and a
/// precast end's where the end applies ([`close_at_precast_end`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MoveStartClose<'w, 's> {
    /// Absent in a harness without the loot window, which closes nothing.
    loot: Option<ResMut<'w, LootState>>,
    latch: Option<ResMut<'w, LootLatch>>,
    net: Res<'w, NetCommands>,
    /// The record fetches the teardown cancels; absent in a harness without the item cache.
    items: Option<Res<'w, Items>>,
    dead: DeadUnitDeselect<'w, 's>,
}

impl MoveStartClose<'_, '_> {
    /// `0x48f200(1, 1, 0)` (`0x60e9dd`, `0x5f07d5`); nothing with no loot target latched
    /// ([`loot_target_held`]), whatever window is open.
    pub(crate) fn close(&mut self) {
        let (Some(loot), Some(latch)) = (self.loot.as_deref_mut(), self.latch.as_deref_mut())
        else {
            return;
        };
        if !loot_target_held(latch) {
            return;
        }
        let closed = close_on_move_start(loot, latch, &self.net, self.items.as_deref());
        self.dead.after_close(closed);
    }
}

/// `IsLooting 0x6126b0` for the active player: its loot guid `[+0x1d28]` is set (`0x6126bc`). The
/// movement guard (`0x60e9ce`) and the stand apply (`0x5f07c6`) skip their close on a clear one
/// (`je` past `0x60e9dd`, `0x5f07d5`), so a window a releasing refusal left open with the latch
/// dropped (`0x5ebac2`) stays open through a move or a sit.
pub(super) fn loot_target_held(latch: &LootLatch) -> bool {
    latch.0.is_some()
}

/// [`close_on_move_start`], its teardown's signals on the lane or in place by `unlocks`.
fn move_start_close(
    loot: &mut LootState,
    latch: &mut LootLatch,
    commands: &NetCommands,
    items: Option<&Items>,
    unlocks: Unlocks,
) -> Option<u64> {
    let source = loot.source()?;
    if move_start_keeps(loot, source) {
        return None;
    }
    debug!("ui_loot: movement started with {source:#x} open — release");
    close_with(loot, latch, Some(commands), items, unlocks)
}

/// What a movement start's close keeps, `dl` set: a disenchant window with rows left
/// (`0x48f24a`) and an item's loot (`0x48f2ab`).
pub(super) fn move_start_keeps(loot: &LootState, source: u64) -> bool {
    use benilla_protocol::guid;
    use benilla_protocol::messages::loot_type;
    loot.window.loot_type == loot_type::DISENCHANTING && !loot.window.is_empty()
        || guid::is_item(source)
}

/// `EndLootWindow 0x48f200(1, moving, 0)` as a precast end's tail runs it for the active player
/// (`0x60d0ee`-`0x60d10b`, `moving` its flags `& 0x200f`), wherever the end applies: inside the
/// packet whose handler ended the precast, its release sent there and its unlocks, its
/// `LOOT_CLOSED` and its dead source's deselect on their lanes at the end's place. Moving, the
/// move-start close, which keeps an item's loot and a disenchant window with rows left
/// ([`move_start_keeps`]); at rest the full one, which closes an item's loot with both unlocks and
/// owes a disenchant window with contents its sweep (`0x48f24f` → `0x4c2a70`, `0x48f264`), which
/// the loot lane runs. Nothing with no window open (`0x48f23f`), and nothing in a harness without
/// the loot window or the wire.
pub(crate) fn close_at_precast_end(world: &mut World, moving: bool) {
    if let Err(e) = world.run_system_cached_with(precast_end_close, moving) {
        debug!("ui_loot: the precast end's close did not run: {e}");
    }
}

fn precast_end_close(In(moving): In<bool>, mut close: PrecastEndClose) {
    close.close(moving);
}

/// What [`close_at_precast_end`] reads and writes.
#[derive(bevy::ecs::system::SystemParam)]
struct PrecastEndClose<'w, 's> {
    /// Absent in a harness without the loot window, which closes nothing.
    loot: Option<ResMut<'w, LootState>>,
    latch: Option<ResMut<'w, LootLatch>>,
    net: Res<'w, NetCommands>,
    /// The record fetches the teardown cancels; absent in a harness without the item cache.
    items: Option<Res<'w, Items>>,
    objects: crate::net::Objects<'w, 's>,
    dead: DeadUnitDeselect<'w, 's>,
}

impl PrecastEndClose<'_, '_> {
    fn close(&mut self, moving: bool) {
        let (Some(loot), Some(latch)) = (self.loot.as_deref_mut(), self.latch.as_deref_mut())
        else {
            return;
        };
        let items = self.items.as_deref();
        let closed = if moving {
            close_on_move_start(loot, latch, &self.net, items)
        } else if loot.disenchant_hold(&self.objects) {
            loot.owe_sweep();
            None
        } else {
            close_interaction(loot, latch, Some(&self.net), items)
        };
        self.dead.after_close(closed);
    }
}

/// [`precast_end_close`]'s declared access, for the guards that model what the unit edits touch
/// as they apply.
#[cfg(test)]
pub(crate) fn precast_end_close_access(world: &mut World) -> bevy::ecs::query::FilteredAccessSet {
    crate::lanes::system_access(world, precast_end_close)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_action::UiErrorKeys;
    use benilla_protocol::messages::loot_type;
    use benilla_protocol::messages::slot_type;
    use benilla_protocol::messages::GroupMemberEntry;
    use benilla_protocol::messages::ItemInfo;
    use benilla_protocol::messages::ObjectFields;
    use benilla_protocol::EntityKind;
    use benilla_protocol::SessionEvent;
    use benilla_ui::script::{CallHost, CallTail, ScriptCall};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::ecs::system::SystemState;

    /// Descriptor field indices the predicate-B table reads.
    const F_GO_TYPE_ID: u16 = 21;
    const F_UNIT_HEALTH: u16 = 22;

    /// Predicate B's answer for one latched object; `None` latches a guid that does not resolve.
    fn kneels_at(object: Option<(EntityKind, &[(u16, u32)])>) -> bool {
        const GUID: u64 = 0xF110_0000_0000_0042;
        let mut app = App::new();
        app.init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<crate::net::GuidIndex>()
            .add_systems(Update, resolve_loot_kneel);
        app.world_mut().resource_mut::<LootLatch>().0 = Some(GUID);
        if let Some((kind, fields)) = object {
            let e = app
                .world_mut()
                .spawn((
                    crate::net::NetEntity {
                        kind,
                        display_id: None,
                        scale: 1.0,
                    },
                    crate::net::ObjectStore(ObjectFields::from_pairs(fields)),
                ))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(GUID, e);
        }
        app.update();
        app.world().resource::<LootKneel>().0
    }

    #[test]
    fn predicate_b_decides_which_loot_targets_are_knelt_at() {
        // A GameObject other than the bobber: a chest (3), a fishing hole (25).
        assert!(kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 3)]
        ))));
        assert!(kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 25)]
        ))));
        // `0x612772`: type 17, `FISHINGNODE`.
        assert!(!kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 17)]
        ))));
        // `0x61278c`: a corpse kneels, a live (pickpocketed) target does not.
        assert!(kneels_at(Some((EntityKind::Unit, &[(F_UNIT_HEALTH, 0)]))));
        assert!(!kneels_at(Some((EntityKind::Unit, &[(F_UNIT_HEALTH, 1)]))));
        // `0x612732`: an unresolved guid, which an item latch is for us.
        assert!(!kneels_at(None));
    }

    #[test]
    fn a_cold_latch_never_kneels() {
        let mut app = App::new();
        app.init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<crate::net::GuidIndex>()
            .add_systems(Update, resolve_loot_kneel);
        app.update();
        assert!(!app.world().resource::<LootKneel>().0);
    }

    /// A bare world with what the loot systems read and write, the lane's other inputs at rest.
    fn bare_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::LootPickupSound>()
            .init_resource::<crate::target::OwedSelects>()
            .init_resource::<LootState>()
            .init_resource::<LootLatch>()
            .init_resource::<AutoLootByte>()
            .init_resource::<LootConfig>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<UiErrorKeys>()
            .insert_resource(NetCommands(tx));
        // Our player, whom the take and the sweep ask for (`0x4c27b6`, `0x4c1fd9`).
        app.world_mut().spawn(crate::net::SelfPlayer);
        (app, rx)
    }

    /// Stream `guid` as an object with `fields`, which the object manager then resolves.
    fn stream(app: &mut App, guid: u64, kind: EntityKind, fields: &[(u16, u32)]) {
        let e = app
            .world_mut()
            .spawn((
                crate::net::NetEntity {
                    kind,
                    display_id: None,
                    scale: 1.0,
                },
                crate::net::ObjectStore(ObjectFields::from_pairs(fields)),
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(guid, e);
    }

    /// A host over a bare loot world: the loot calls apply through [`LootSteps`], as the engine's
    /// host applies them; any other call is dropped.
    struct LootHost<'a>(&'a mut World);

    impl CallHost for LootHost<'_> {
        fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            let ScriptCall::Loot(call) = call else {
                return None;
            };
            let mut state = SystemState::<LootSteps>::new(self.0);
            let tail = state.get_mut(self.0).apply(script, call);
            state.apply(self.0);
            tail
        }

        /// The loot getters' reads, as the engine's host answers them ([`LootLive`]).
        fn read_live(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
            use crate::script_calls::live::LiveFamily;
            let mut state = SystemState::<LootLive>::new(self.0);
            state.get(self.0).answer(ask)
        }
    }

    /// Run `f` on the app's VM with a [`LootHost`] lent, as every entry that runs Lua lends one.
    fn hosted<R>(app: &mut App, f: impl FnOnce(&mut UiScript) -> R) -> R {
        let mut script = app
            .world_mut()
            .remove_non_send_resource::<UiScript>()
            .expect("a VM");
        let r = script.hosted(&mut LootHost(app.world_mut()), f);
        app.world_mut().insert_non_send_resource(script);
        r
    }

    /// Run `lua` with a [`LootHost`] lent.
    fn run_hosted(app: &mut App, lua: &str) {
        hosted(app, |s| s.run(lua)).expect("the chunk runs");
    }

    // ── The soulbind confirm ──────────────────────────────────────────────────
    //
    // vmangos `item_template` rows: Felstriker a BoP epic, Flurry Axe a BoE epic, Tough Jerky a
    // plain white; 9999 is a synthetic white BoP, the quality control.
    const FELSTRIKER: u32 = 12590;
    const FLURRY_AXE: u32 = 871;
    const TOUGH_JERKY: u32 = 117;
    const WHITE_BOP: u32 = 9999;
    /// Brutality Blade, a second BoP epic, for the sweep's one-dialog latch.
    const SECOND_BOP: u32 = 18832;

    /// A loot window over `rows` whose display rows `clicks` are clicked by the mouse, as a
    /// `LootButton` takes (`0x4c1820`), then `lua` run, each with the host lent.
    fn drain_with(
        gold: u32,
        rows: Vec<LootItem>,
        clicks: &[u32],
        lua: &str,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        drain_after(gold, rows, "", clicks, lua)
    }

    /// [`drain_with`] with `first` run ahead of the clicks.
    fn drain_after(
        gold: u32,
        rows: Vec<LootItem>,
        first: &str,
        clicks: &[u32],
        lua: &str,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut app, rx) = bare_app();
        let mut items = app.world_mut().resource_mut::<Items>();
        let mut tmpl = |entry: u32, name: &str, quality: u32, bonding: u32| {
            items.insert_template(
                entry,
                Some(ItemInfo {
                    quality,
                    bonding,
                    ..crate::items::test_template(name)
                }),
            );
        };
        tmpl(FELSTRIKER, "Felstriker", 4, 1);
        tmpl(FLURRY_AXE, "Flurry Axe", 4, 2);
        tmpl(TOUGH_JERKY, "Tough Jerky", 1, 0);
        tmpl(WHITE_BOP, "A White Soulbound Thing", 1, 1);

        app.world_mut()
            .resource_mut::<LootState>()
            .open(0x42, loot_type::CORPSE, gold, rows);
        app.world_mut().resource_mut::<LootLatch>().0 = Some(0x42);

        let mut script = UiScript::new().unwrap();
        script
            .run(
                "BIND_CONFIRMS = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"LOOT_BIND_CONFIRM\")\n\
                 f:SetScript(\"OnEvent\", function() tinsert(BIND_CONFIRMS, arg1) end)",
            )
            .unwrap();
        script.set_screen_size(1024.0, 768.0);
        app.insert_non_send_resource(script);
        run_hosted(&mut app, first);
        for &row in clicks {
            hosted(&mut app, |script| {
                script
                    .run(&format!(
                        "local b = CreateFrame(\"LootButton\", \"DrainRow\", UIParent)\n\
                         b:SetPoint(\"BOTTOMLEFT\", 100, 100) b:SetWidth(50) b:SetHeight(50)\n\
                         b:EnableMouse(true) b:Show() b:SetSlot({row})"
                    ))
                    .unwrap();
                script.resolve();
                script.mouse_button(125.0, 125.0, "LeftButton", true);
                script.mouse_button(125.0, 125.0, "LeftButton", false);
            });
        }
        run_hosted(&mut app, lua);
        (app, rx)
    }

    /// `0x4c2ac0` fires `LOOT_OPENED` once, at the last answer; here a negative one opens it too.
    #[test]
    fn the_window_waits_for_every_item_template() {
        for answer in [
            Some(ItemInfo {
                quality: 1,
                ..crate::items::test_template("Thin Cloth Gloves")
            }),
            None,
        ] {
            let (mut app, rx) = bare_app();
            app.add_systems(bevy::prelude::Update, loot_frame);
            app.world_mut().resource_mut::<LootState>().open(
                0x42,
                loot_type::CORPSE,
                0,
                vec![item(3, TOUGH_JERKY, 1)],
            );

            let script = UiScript::new().unwrap();
            script
                .run(
                    "OPENS = 0\n\
                     local f = CreateFrame(\"Frame\")\n\
                     f:RegisterEvent(\"LOOT_OPENED\")\n\
                     f:SetScript(\"OnEvent\", function() OPENS = OPENS + 1 end)",
                )
                .unwrap();
            app.insert_non_send_resource(script);

            let opens = |app: &mut App| {
                app.world_mut()
                    .non_send_resource_mut::<UiScript>()
                    .eval::<i64>("return OPENS")
                    .unwrap()
            };

            app.update();
            assert_eq!(
                opens(&mut app),
                0,
                "no LOOT_OPENED while a template is pending"
            );
            assert!(
                sent(&rx).iter().any(
                    |c| matches!(c, ClientCommand::ItemQuery { entry, .. } if *entry == TOUGH_JERKY)
                ),
                "and the template was asked for"
            );

            // A second pass changes nothing: the deferral is not a one-shot.
            app.update();
            assert_eq!(opens(&mut app), 0);

            // The answer as the item handler lands it: the record, then its callbacks.
            let world = app.world_mut();
            world
                .resource_mut::<Items>()
                .insert_template(TOUGH_JERKY, answer.clone());
            world.resource_scope(|world, mut loot: Mut<LootState>| {
                loot.row_answered(world.resource::<Items>())
            });
            app.update();
            assert_eq!(opens(&mut app), 1, "opened when the last template answered");
            app.update();
            assert_eq!(opens(&mut app), 1, "and only once");
        }
    }

    /// The rows `LOOT_BIND_CONFIRM` has named so far, in order.
    fn confirms(app: &mut App) -> Vec<i64> {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let n = script.eval::<i64>("return getn(BIND_CONFIRMS)").unwrap();
        (1..=n)
            .map(|i| {
                script
                    .eval::<i64>(&format!("return BIND_CONFIRMS[{i}]"))
                    .unwrap()
            })
            .collect()
    }

    fn sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<ClientCommand> {
        rx.try_iter().collect()
    }

    fn pending_confirm(app: &App) -> Option<u32> {
        app.world().resource::<LootState>().pending_bind_confirm
    }

    /// The click arm's deferral (`0x4c28f2`-`0x4c2920`).
    #[test]
    fn a_bop_row_confirms_instead_of_sending() {
        let (mut app, rx) = drain_with(0, vec![item(0, FELSTRIKER, 1)], &[1], "");
        assert!(sent(&rx).is_empty(), "the deferred take sends nothing");
        assert_eq!(pending_confirm(&app), Some(1), "and stashes the row");

        assert_eq!(
            confirms(&mut app),
            vec![1],
            "the event carries the row out, 1-based and display-side"
        );
    }

    /// The confirm arm (`0x4c27c0`).
    #[test]
    fn loot_slot_completes_the_pending_confirm_exactly_once() {
        let (mut app, rx) = drain_with(0, vec![item(7, FELSTRIKER, 1)], &[1], "LootSlot(1)");
        assert!(
            matches!(
                sent(&rx)[..],
                [ClientCommand::AutostoreLootItem { slot: 7 }]
            ),
            "the accept sends the row's wire slot, once"
        );
        assert_eq!(pending_confirm(&app), None, "and the stash clears");

        // A second accept over the same dialog: nothing left to complete.
        run_hosted(&mut app, "LootSlot(1)");
        assert!(sent(&rx).is_empty(), "a second accept sends nothing");
    }

    /// The click signals `LOOT_BIND_CONFIRM` (`0x4c290f`) before the stash takes the row
    /// (`0x4c2917`), and the confirm arm compares with the stash (`0x4c27c0`), which the open
    /// reset (`0x4c1df5`): a `LootSlot` inside the handler sends nothing, and one after it sends.
    #[test]
    fn a_bind_confirms_own_handler_cannot_accept_it() {
        let (mut app, rx) = drain_with(
            0,
            vec![item(7, FELSTRIKER, 1)],
            &[1],
            "local f = CreateFrame(\"Frame\")\n\
             f:RegisterEvent(\"LOOT_BIND_CONFIRM\")\n\
             f:SetScript(\"OnEvent\", function() LootSlot(arg1) end)",
        );
        assert!(sent(&rx).is_empty(), "the handler's accept finds no stash");
        assert_eq!(
            pending_confirm(&app),
            Some(1),
            "the stash takes the row after it"
        );
        run_hosted(&mut app, "LootSlot(1)");
        assert!(
            matches!(
                sent(&rx)[..],
                [ClientCommand::AutostoreLootItem { slot: 7 }]
            ),
            "an accept after the handler sends the row"
        );
    }

    /// The takes are answered in their call order: an accept made before the click finds no stash
    /// (`0x4c27c0`), which the click stores only when it runs (`0x4c2917`).
    #[test]
    fn an_accept_before_the_click_finds_no_stash() {
        let (app, rx) = drain_after(0, vec![item(7, FELSTRIKER, 1)], "LootSlot(1)", &[1], "");
        assert!(sent(&rx).is_empty(), "the early accept sends nothing");
        assert_eq!(
            pending_confirm(&app),
            Some(1),
            "and the click still stashes its row"
        );
    }

    /// The flag-1 arm opens with `cmp edi,[0x847cec]`, the pending row.
    #[test]
    fn loot_slot_on_an_ordinary_row_sends_nothing() {
        let (_app, rx) = drain_with(
            0,
            vec![item(0, TOUGH_JERKY, 1), item(1, FLURRY_AXE, 1)],
            &[],
            "LootSlot(1) LootSlot(2)",
        );
        assert!(
            sent(&rx).is_empty(),
            "no confirm is pending, so neither call reaches the wire"
        );
    }

    #[test]
    fn the_bind_gate_needs_both_conjuncts() {
        for (entry, why) in [
            (FLURRY_AXE, "bind-on-equip is not bind-on-pickup"),
            (WHITE_BOP, "quality 1 is below the floor of 2"),
            (TOUGH_JERKY, "neither"),
        ] {
            let (app, rx) = drain_with(0, vec![item(3, entry, 1)], &[1], "");
            assert!(
                matches!(
                    sent(&rx)[..],
                    [ClientCommand::AutostoreLootItem { slot: 3 }]
                ),
                "entry {entry} should be taken outright: {why}"
            );
            assert_eq!(
                pending_confirm(&app),
                None,
                "entry {entry}: no confirm ({why})"
            );
        }
    }

    /// The display row on both halves (the deviation on `LootState::pending_bind_confirm`).
    #[test]
    fn the_coin_row_does_not_shift_the_confirm_out_from_under_itself() {
        let (mut app, rx) = drain_with(
            120, // gold, so display row 1 is the coin and the item is row 2
            vec![item(4, FELSTRIKER, 1)],
            &[2],
            "",
        );
        assert_eq!(
            pending_confirm(&app),
            Some(2),
            "the DISPLAY row, coin included"
        );
        assert_eq!(confirms(&mut app), vec![2]);
        assert!(sent(&rx).is_empty());

        // The accept, with the same number, reaches the right wire slot.
        run_hosted(&mut app, "LootSlot(2)");
        assert!(matches!(
            sent(&rx)[..],
            [ClientCommand::AutostoreLootItem { slot: 4 }]
        ));
    }

    /// `0x4c27d7`: the continuation returns on an emptied record.
    #[test]
    fn an_accept_for_a_row_that_was_taken_away_sends_nothing() {
        let (mut app, rx) = drain_with(0, vec![item(2, FELSTRIKER, 1)], &[1], "");
        assert_eq!(pending_confirm(&app), Some(1));

        app.world_mut().resource_mut::<LootState>().remove_slot(2);
        run_hosted(&mut app, "LootSlot(1)");

        assert!(
            sent(&rx)
                .iter()
                .all(|c| !matches!(c, ClientCommand::AutostoreLootItem { .. })),
            "nothing is autostored for a row that is gone"
        );
    }

    /// The close leaves the stash (`0x48f200` and `0x4c1cb0(0)` never store to `0x847cec`); the
    /// next response's copier resets it (`0x4c1df5`).
    #[test]
    fn a_pending_confirm_lasts_until_the_next_response() {
        let (mut app, _rx) = drain_with(0, vec![item(0, FELSTRIKER, 1)], &[1], "");
        assert_eq!(pending_confirm(&app), Some(1));

        app.world_mut().resource_mut::<LootState>().clear();
        assert_eq!(pending_confirm(&app), Some(1), "the close keeps it");

        app.world_mut().resource_mut::<LootState>().open(
            0x43,
            loot_type::CORPSE,
            0,
            vec![item(0, TOUGH_JERKY, 1)],
        );
        assert_eq!(
            pending_confirm(&app),
            None,
            "and a fresh window opens with none"
        );
    }

    /// A window over two BoP epics and a white (wire slots 5/6/7) with the auto-loot byte the
    /// interaction stored set, its `LOOT_BIND_CONFIRM` rows noted, then `lua` run; the lane has not
    /// fired.
    fn auto_loot_app(lua: &str) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut app, rx) = bare_app();
        app.insert_resource(AutoLootByte(true));
        stream(&mut app, 0x42, EntityKind::Unit, &[]);

        let mut items = app.world_mut().resource_mut::<Items>();
        for (entry, name, quality, bonding) in [
            (FELSTRIKER, "Felstriker", 4, 1),
            (SECOND_BOP, "Brutality Blade", 4, 1),
            (TOUGH_JERKY, "Tough Jerky", 1, 0),
        ] {
            items.insert_template(
                entry,
                Some(ItemInfo {
                    quality,
                    bonding,
                    ..crate::items::test_template(name)
                }),
            );
        }

        // Wire slots 5/6/7 in display order: BoP epic, BoP epic, white.
        app.world_mut().resource_mut::<LootState>().open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![
                item(5, FELSTRIKER, 1),
                item(6, SECOND_BOP, 1),
                item(7, TOUGH_JERKY, 1),
            ],
        );
        let script = UiScript::new().unwrap();
        script
            .run(
                "BIND_CONFIRMS = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"LOOT_BIND_CONFIRM\")\n\
                 f:SetScript(\"OnEvent\", function() tinsert(BIND_CONFIRMS, arg1) end)",
            )
            .unwrap();
        script.run(lua).unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    /// The sweep's one-shot latch (`0x4c21c2`, `0x4c21e2`), over two BoP epics and a white.
    #[test]
    fn the_auto_loot_sweep_raises_exactly_one_bind_confirm() {
        let (mut app, rx) = auto_loot_app("");
        loot_frame(app.world_mut());

        assert_eq!(
            confirms(&mut app),
            vec![1],
            "one dialog, for the FIRST bind-on-pickup row only"
        );
        assert_eq!(pending_confirm(&app), Some(1));
        assert!(
            matches!(
                sent(&rx)[..],
                [ClientCommand::AutostoreLootItem { slot: 7 }]
            ),
            "the white is swept; neither blue is sent"
        );
    }

    /// The sweep stores the stash after its `LOOT_BIND_CONFIRM`'s listeners (`0x4c21dc` after
    /// `0x4c21d4`) and sends the rows after it; an accept then sends the row.
    #[test]
    fn a_swept_bind_confirm_is_accepted_after_the_sweep() {
        let (mut app, rx) = auto_loot_app("");
        loot_frame(app.world_mut());
        assert!(matches!(
            sent(&rx)[..],
            [ClientCommand::AutostoreLootItem { slot: 7 }]
        ));
        assert_eq!(pending_confirm(&app), Some(1));
        run_hosted(&mut app, "LootSlot(1)");
        assert!(
            matches!(
                sent(&rx)[..],
                [ClientCommand::AutostoreLootItem { slot: 5 }]
            ),
            "an accept after the sweep sends the row"
        );
    }

    fn item(slot: u8, entry: u32, count: u32) -> LootItem {
        LootItem {
            slot,
            item_id: entry,
            count,
            display_info_id: 1000 + entry,
            random_property_id: 0,
            slot_type: 0,
        }
    }

    /// The same row stamped `MASTER`, as vmangos sends every row under master loot.
    fn master_item(slot: u8, entry: u32, count: u32) -> LootItem {
        LootItem {
            slot_type: slot_type::MASTER,
            ..item(slot, entry, count)
        }
    }

    /// The candidate array is the list packet's alone (`0x61c562`-`0x61c637`, its one writer):
    /// it reads with no window, and no response or close touches it, so it serves every window
    /// until the next list, which clears it first (`0x61c567`). The getter reads 40 slots
    /// (`0x61c660`).
    #[test]
    fn the_candidate_list_is_the_list_packets_until_the_next() {
        let mut loot = LootState::default();
        // A default GroupState is a party (`group_type` 0): the flat placement path.
        let party = GroupState::default();

        loot.set_master_candidates(&[0xA, 0xB, 0xC], &party);
        assert_eq!(loot.master_candidate(0), Some(0xA), "no window needed");

        loot.open(0x42, loot_type::CORPSE, 0, vec![master_item(0, 117, 1)]);
        assert_eq!(loot.master_candidate(0), Some(0xA));
        assert_eq!(loot.master_candidate(2), Some(0xC));
        assert_eq!(loot.master_candidate(3), None, "past the end");

        // The next list replaces it whole.
        loot.set_master_candidates(&[0xA, 0xB], &party);
        assert_eq!(loot.master_candidate(1), Some(0xB));
        assert_eq!(loot.master_candidate(2), None, "the list shrank");

        loot.clear();
        loot.open(0x43, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert_eq!(
            loot.master_candidate(0),
            Some(0xA),
            "a close and a response leave the array"
        );

        let many: Vec<u64> = (1..=45).collect();
        loot.set_master_candidates(&many, &party);
        assert_eq!(loot.master_candidate(39), Some(40));
        assert_eq!(
            loot.master_candidate(40),
            None,
            "the getter's bound (`0x61c660`)"
        );
    }

    /// `0x61c550` places a candidate only off its row below the roster's count: a guid on no row
    /// is skipped (`0x61c5f9`), taking no slot, so the next candidate of that block files first.
    #[test]
    fn a_raid_candidate_on_no_row_is_placed_nowhere() {
        let mut raid = GroupState {
            group_type: crate::ui_party::GROUPTYPE_RAID,
            members: vec![GroupMemberEntry {
                name: "Cairne".into(),
                guid: 0xB,
                status: 0,
                flags: 0,
            }],
            ..GroupState::default()
        };
        raid.rebuild_raid(0xA);
        let mut loot = LootState::default();
        loot.set_master_candidates(&[0xE, 0xB, 0xA], &raid);
        let placed: Vec<Option<u64>> = (0..4).map(|i| loot.master_candidate(i)).collect();
        assert_eq!(placed, [Some(0xB), Some(0xA), None, None], "0xE has no row");
    }

    /// `0x61c550`'s raid placement: each guid in the first free slot of its subgroup's block, off
    /// the roster as the packet finds it.
    #[test]
    fn raid_candidates_file_into_their_own_subgroup_block() {
        let member = |guid: u64, name: &str, subgroup: u8| GroupMemberEntry {
            name: name.into(),
            guid,
            status: 0,
            flags: subgroup,
        };
        let mut raid = GroupState {
            group_type: crate::ui_party::GROUPTYPE_RAID,
            own_flags: 0, // we are in subgroup 0
            ..GroupState::default()
        };
        raid.members = vec![
            member(0xB, "Cairne", 0),
            member(0xC, "Vol", 2),
            member(0xD, "Sylvanas", 2),
        ];
        raid.rebuild_raid(0xA);

        let mut loot = LootState::default();
        // The wire order interleaves the subgroups; placement ignores it.
        loot.set_master_candidates(&[0xC, 0xA, 0xD, 0xB], &raid);

        // Subgroup 0 fills slots 1-2 (us at 0xA, then Cairne); subgroup 2 fills 11-12, in the
        // order the wire listed them.
        assert_eq!(loot.master_candidate(0), Some(0xA), "us, group 1");
        assert_eq!(loot.master_candidate(1), Some(0xB), "Cairne, group 1");
        for empty in 2..10 {
            assert_eq!(
                loot.master_candidate(empty),
                None,
                "slot {} belongs to an empty group",
                empty + 1
            );
        }
        assert_eq!(loot.master_candidate(10), Some(0xC), "Vol, group 3");
        assert_eq!(loot.master_candidate(11), Some(0xD), "Sylvanas, group 3");
        assert_eq!(loot.master_candidate(12), None);

        // In a party the same list is flat, in wire order.
        let party = GroupState {
            members: raid.members.clone(),
            ..GroupState::default()
        };
        loot.set_master_candidates(&[0xC, 0xA, 0xD, 0xB], &party);
        assert_eq!(loot.master_candidate(0), Some(0xC));
        assert_eq!(loot.master_candidate(3), Some(0xB));
        assert_eq!(loot.master_candidate(4), None);

        // A raid of one holds no roster (`0x5e6e71`), so the count reads 0 and the list is flat,
        // whatever subgroup we sit in.
        let alone = GroupState {
            group_type: crate::ui_party::GROUPTYPE_RAID,
            own_flags: 2,
            ..GroupState::default()
        };
        let mut solo = LootState::default();
        solo.set_master_candidates(&[0xA], &alone);
        assert_eq!(solo.master_candidate(0), Some(0xA), "a raid of one");
    }

    #[test]
    fn a_master_row_reports_its_slot_type() {
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![master_item(0, 117, 1), item(1, 2589, 5)],
        );
        assert!(matches!(
            loot.window.action_at(1),
            Some(LootAction::Item {
                wire_slot: 0,
                slot_type: slot_type::MASTER,
                ..
            })
        ));
        assert!(matches!(
            loot.window.action_at(2),
            Some(LootAction::Item {
                wire_slot: 1,
                slot_type: slot_type::ALLOW_LOOT,
                ..
            })
        ));
    }

    /// Our own push, shown in chat, with no random property.
    fn push(entry: u32, count: u32, from_npc: bool, created: bool) -> ItemPushResult {
        ItemPushResult {
            player_guid: 0x1,
            from_npc,
            created,
            show_in_chat: true,
            bag_slot: 0xFF,
            item_slot: 0,
            item_entry: entry,
            suffix_factor: 0,
            random_property_id: 0,
            count,
        }
    }

    fn pending(entry: u32, count: u32, from_npc: bool, created: bool) -> PendingReceive {
        let mut loot = LootState::default();
        loot.push_receive(&push(entry, count, from_npc, created), true);
        loot.take_push().expect("queued")
    }

    #[test]
    fn action_maps_coin_first_then_items_by_wire_slot() {
        let mut loot = LootState::default();
        assert!(loot.source().is_none());
        loot.open(
            0x42,
            loot_type::CORPSE,
            1234,
            vec![item(0, 117, 1), item(1, 2589, 5)],
        );
        assert!(loot.source().is_some());
        // Row 1 is the coin, rows 2 and 3 the items; the helper's display id is `1000 + entry`.
        assert!(matches!(loot.window.action_at(1), Some(LootAction::Money)));
        assert!(matches!(
            loot.window.action_at(2),
            Some(LootAction::Item {
                wire_slot: 0,
                display_id: 1117,
                ..
            })
        ));
        assert!(matches!(
            loot.window.action_at(3),
            Some(LootAction::Item {
                wire_slot: 1,
                display_id: 3589,
                ..
            })
        ));
        assert!(loot.window.action_at(4).is_none());
        assert!(loot.window.action_at(0).is_none());
    }

    #[test]
    fn action_maps_items_directly_when_no_coin() {
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(3, 117, 1), item(7, 2589, 5)],
        );
        assert!(matches!(
            loot.window.action_at(1),
            Some(LootAction::Item { wire_slot: 3, .. })
        ));
        assert!(matches!(
            loot.window.action_at(2),
            Some(LootAction::Item { wire_slot: 7, .. })
        ));
        assert!(loot.window.action_at(3).is_none());
    }

    #[test]
    fn remove_slot_leaves_a_gap_at_the_fixed_position() {
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1), item(1, 2589, 5), item(2, 4306, 2)],
        );
        loot.remove_slot(1);
        assert!(matches!(
            loot.window.action_at(1),
            Some(LootAction::Item { wire_slot: 0, .. })
        ));
        assert!(
            loot.window.action_at(2).is_none(),
            "the looted row is a gap"
        );
        assert!(matches!(
            loot.window.action_at(3),
            Some(LootAction::Item { wire_slot: 2, .. })
        ));
    }

    #[test]
    fn clear_money_leaves_the_coin_slot_as_a_gap() {
        let mut loot = LootState::default();
        loot.open(0x42, loot_type::CORPSE, 500, vec![item(0, 117, 1)]);
        assert!(loot.window.has_coin());
        assert!(matches!(loot.window.action_at(1), Some(LootAction::Money)));
        loot.clear_money();
        assert!(!loot.window.has_coin());
        assert!(
            loot.window.action_at(1).is_none(),
            "the looted coin slot is a gap"
        );
        assert!(matches!(
            loot.window.action_at(2),
            Some(LootAction::Item { wire_slot: 0, .. })
        ));
    }

    /// The close zeroes the source alone (`0x4c1da0`; `0x4c1cb0(0)` returns at `0x4c1db7` before the
    /// copier's clear at `0x4c1ddb`): the rows and the money stay for the take dispatcher, which
    /// asks for no source, until the next response replaces them.
    #[test]
    fn a_close_keeps_the_rows_until_the_next_response() {
        let mut loot = LootState::default();
        loot.open(0x42, loot_type::CORPSE, 500, vec![item(3, 117, 1)]);
        loot.clear();
        assert_eq!(loot.source(), None);
        assert!(matches!(loot.window.action_at(1), Some(LootAction::Money)));
        assert!(matches!(
            loot.window.action_at(2),
            Some(LootAction::Item { wire_slot: 3, .. })
        ));
        loot.open(0x43, loot_type::CORPSE, 0, vec![item(5, 117, 1)]);
        assert!(matches!(
            loot.window.action_at(1),
            Some(LootAction::Item { wire_slot: 5, .. })
        ));
    }

    #[test]
    fn latch_clears_guid_matched_only() {
        // Loot B requested while A was open: A's release response keeps B's latch.
        let mut latch = LootLatch(Some(0xB));
        latch.clear_for(0xA);
        assert_eq!(latch.0, Some(0xB), "a stale release leaves the new latch");
        latch.clear_for(0xB);
        assert_eq!(latch.0, None, "the matching release drops it");
    }

    /// `IsFishingLoot` reads the type cell (`0x4c2740` == 3), which each response's copier stores
    /// (`0x4c1dd6`) and the close leaves (`0x4c1cb0(0)` zeroes only the source).
    #[test]
    fn the_type_cell_follows_the_responses_and_outlives_the_close() {
        let mut loot = LootState::default();
        loot.open(0x42, loot_type::FISHING, 0, vec![item(0, 117, 1)]);
        assert_eq!(loot.window.loot_type, loot_type::FISHING);
        loot.clear();
        assert_eq!(loot.window.loot_type, loot_type::FISHING, "no window");
        loot.open(0x42, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert_eq!(loot.window.loot_type, loot_type::CORPSE);
    }

    #[test]
    fn clear_closes_but_keeps_receives() {
        let mut loot = LootState::default();
        loot.open(0x42, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        loot.push_receive(&push(117, 1, false, false), true);
        loot.clear();
        assert!(loot.source().is_none());
        assert_eq!(
            loot.pending_receive_count(),
            1,
            "a receive line outlives the window"
        );
        loot.clear_session();
        assert_eq!(
            loot.pending_receive_count(),
            0,
            "disconnect drops receive lines"
        );
    }

    thread_local! {
        /// The install's `GlobalStrings.lua`, for the receive lines' enUS text.
        static STRINGS: UiScript = {
            let s = UiScript::new().expect("VM");
            crate::ui_script::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
            s
        };
    }

    /// [`receive_line`] from the install's strings.
    fn receive_text(r: &PendingReceive, name: &str, quality: u32) -> String {
        STRINGS.with(|s| {
            receive_line(r, name, quality, None, &|key| {
                benilla_ui::strings::global(s.lua(), key)
            })
            .expect("a template for every receive line")
        })
    }

    /// The count after the link's `|r` takes the line's own colour.
    #[test]
    fn receive_line_carries_a_quality_colored_item_link() {
        let _data = benilla_formats::wow_data_or_skip!();
        // Common, single: `LOOT_ITEM_SELF`.
        assert_eq!(
            receive_text(&pending(2589, 1, false, false), "Linen Cloth", 1),
            "You receive loot: |cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r."
        );
        // Poor, stacked: `LOOT_ITEM_SELF_MULTIPLE`, with no space before `x2`.
        assert_eq!(
            receive_text(&pending(7092, 2, false, false), "Chipped Claw", 0),
            "You receive loot: |cff9d9d9d|Hitem:7092:0:0:0|h[Chipped Claw]|h|rx2."
        );
        // The (created, received) pair picks the verb, created winning over received.
        assert_eq!(
            receive_text(&pending(4306, 1, true, false), "Silk Cloth", 2),
            "You receive item: |cff1eff00|Hitem:4306:0:0:0|h[Silk Cloth]|h|r."
        );
        assert_eq!(
            receive_text(&pending(2320, 4, true, true), "Coarse Thread", 1),
            "You create: |cffffffff|Hitem:2320:0:0:0|h[Coarse Thread]|h|rx4."
        );
    }

    /// `|Hitem:id:0:randomPropertyId:suffixFactor|h`, `0x52adb0`'s argument order.
    #[test]
    fn receive_line_carries_the_wire_random_property_fields() {
        let _data = benilla_formats::wow_data_or_skip!();
        let mut loot = LootState::default();
        loot.push_receive(
            &ItemPushResult {
                random_property_id: 862,
                suffix_factor: 1234,
                ..push(15268, 1, false, false)
            },
            true,
        );
        let r = loot.take_push().expect("queued");
        assert_eq!(
            receive_text(&r, "Bloodrazor", 3),
            "You receive loot: |cff0070dd|Hitem:15268:0:862:1234|h[Bloodrazor]|h|r."
        );
    }

    /// `0x5d8b00` joins the suffix by `ITEM_SUFFIX_TEMPLATE`, for the row (`0x4c2550`) and tooltip.
    #[test]
    fn a_rolled_drop_reads_its_suffix_in_the_row_the_link_and_the_lines() {
        use benilla_formats::{RandomProperty, RandomPropertyCatalog};
        // "of the Monkey" (row 584 of the shipped table): Agility +7, Stamina +7.
        let props = crate::items::RandomProperties::new(RandomPropertyCatalog::from_rows(
            [(
                584,
                RandomProperty {
                    suffix: "of the Monkey".into(),
                    enchants: [74, 71, 0, 0, 0],
                },
            )]
            .into_iter()
            .collect(),
        ));
        let enchants = crate::items::Enchants(benilla_formats::EnchantCatalog::from_rows(
            std::collections::HashMap::new(),
            [
                (74, "Agility +7".to_string()),
                (71, "Stamina +7".to_string()),
            ]
            .into_iter()
            .collect(),
            [(74, 0), (71, 0)].into_iter().collect(),
        ));
        let rolls = RollCatalogs {
            props: Some(&props),
            enchants: Some(&enchants),
        };
        assert_eq!(rolls.name("Bloodrazor", 584), "Bloodrazor of the Monkey");
        assert_eq!(rolls.name("Bloodrazor", 0), "Bloodrazor");
        // The roll's lines land in slots 2..6, the suffix band the renderer draws white.
        let lines = rolls.lines(584);
        assert_eq!(
            lines
                .iter()
                .map(|l| (l.slot, l.name.as_str()))
                .collect::<Vec<_>>(),
            vec![(2, "Agility +7"), (3, "Stamina +7")],
        );
        assert!(rolls.lines(0).is_empty(), "no roll, no lines");
    }

    /// `0x52ad90` clamps a quality outside the table to 1, white.
    #[test]
    fn receive_line_clamps_an_out_of_range_quality_to_white() {
        let _data = benilla_formats::wow_data_or_skip!();
        assert_eq!(
            receive_text(&pending(1, 1, false, false), "Odd Thing", 9),
            "You receive loot: |cffffffff|Hitem:1:0:0:0|h[Odd Thing]|h|r."
        );
    }

    /// [`push_container`] against the selector at `0x491bb5`-`0x491bd6`, arm by arm.
    #[test]
    fn push_container_maps_the_wire_destination_onto_a_bag_bar_button() {
        // An equipped bag: wire 19..22 give `bag + 1`, 20..23, the ids its buttons carry.
        assert_eq!(push_container(19, 0), 20);
        assert_eq!(push_container(22, 5), 23);
        // `bag == 255` in the keyring window, 0x51..=0x70.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 81), KEYRING_CONTAINER);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 96), KEYRING_CONTAINER);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 112), KEYRING_CONTAINER);
        // Anywhere else is the backpack: its slots 23..38, and 80 and 113 just outside the keyring.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 23), 0);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 80), 0);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 113), 0);
        // A stack merge reports no slot (`0xFFFF_FFFF`): the backpack.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, u32::MAX), 0);
        // A bank bag (wire 63..68) lands on an id no bag-bar button carries.
        assert!(!(20..=23).contains(&push_container(63, 0)));
    }

    /// The coin row's name is `GetLootRowName`'s (`0x4c2550`): gold, silver and copper in that
    /// order, each non-zero one as `"%d %s%s"` (`0x847ffc`) with the word read from the
    /// `GOLD`/`SILVER`/`COPPER` global at the call (`0x6c6230`, `0x703bf0`) and `"\n"`
    /// (`0x835144`) after gold and silver, `""` (`0x882748`) after copper.
    #[test]
    fn the_coin_rows_name_puts_each_denomination_on_its_own_line_in_the_globals_words() {
        let items = Items::default();
        let mut script = UiScript::new().expect("VM");
        script
            .run(r#"GOLD = "Gold" SILVER = "Silver" COPPER = "Copper""#)
            .expect("the words");
        let name_of = |script: &mut UiScript, copper: u32| {
            let mut loot = LootState::default();
            loot.open(0x42, loot_type::CORPSE, copper, vec![]);
            script.set_loot(vm_copy(&loot.window, &items));
            script
                .eval::<String>("local _, name = GetLootSlotInfo(1) return name")
                .expect("the coin row")
        };
        let s = &mut script;
        assert_eq!(name_of(s, 12_345), "1 Gold\n23 Silver\n45 Copper");
        assert_eq!(name_of(s, 10_000), "1 Gold\n", "gold's newline stays");
        assert_eq!(name_of(s, 10_025), "1 Gold\n25 Copper");
        assert_eq!(name_of(s, 2_300), "23 Silver\n");
        assert_eq!(name_of(s, 4), "4 Copper");

        s.run(r#"GOLD = "Or" SILVER = "Argent" COPPER = "Cuivre""#)
            .expect("an addon's words");
        assert_eq!(
            name_of(s, 12_345),
            "1 Or\n23 Argent\n45 Cuivre",
            "the words are the globals' at the call"
        );
    }

    /// The reference's six-step ladder (`0x6c6307`-`0x6c6386`), both sides of every boundary.
    #[test]
    fn coin_icon_walks_the_references_six_step_ladder() {
        let icon = |n: u32| coin_icon(n).rsplit('\\').next().unwrap().to_string();
        assert_eq!(icon(0), "INV_Misc_Coin_05");
        assert_eq!(icon(9), "INV_Misc_Coin_05");
        assert_eq!(icon(10), "INV_Misc_Coin_06");
        assert_eq!(icon(99), "INV_Misc_Coin_06");
        assert_eq!(icon(100), "INV_Misc_Coin_03");
        assert_eq!(icon(999), "INV_Misc_Coin_03");
        assert_eq!(icon(1_000), "INV_Misc_Coin_04");
        assert_eq!(icon(9_999), "INV_Misc_Coin_04");
        assert_eq!(icon(10_000), "INV_Misc_Coin_01");
        assert_eq!(icon(99_999), "INV_Misc_Coin_01");
        assert_eq!(icon(100_000), "INV_Misc_Coin_02");
        assert_eq!(icon(u32::MAX), "INV_Misc_Coin_02");
    }

    /// The window's display rows as the getters read them ([`Window::row`]).
    fn rows(w: &Window, items: &Items) -> Option<Vec<Option<LootRow>>> {
        w.source?;
        let n = w.items.len() + usize::from(w.coin_slot);
        Some(
            (0..n)
                .map(|i| w.row(i, items, None, RollCatalogs::NONE).flatten())
                .collect(),
        )
    }

    /// The window as a VM's own copy, for a test that drives the getters with no host.
    fn vm_copy(w: &Window, items: &Items) -> Option<benilla_ui::script::LootState> {
        Some(benilla_ui::script::LootState {
            rows: rows(w, items)?,
            has_money: w.coin_slot,
            master_candidates: Vec::new(),
            source_unit: false,
        })
    }

    #[test]
    fn coin_row_carries_its_copper_and_the_ladders_icon() {
        let items = Items::default();
        let mut loot = LootState::default();
        // A pure-copper drop: its copper for the name, and the copper coin pile (_05).
        loot.open(0x42, loot_type::CORPSE, 4, vec![]);
        let rows = rows(&loot.window, &items).expect("open");
        assert_eq!(rows.len(), 1, "coin row only");
        let coin = rows[0].as_ref().expect("coin row present");
        assert!(coin.is_coin);
        assert_eq!(coin.money, 4, "the name is spelled out at the call");
        assert_eq!(coin.texture.as_deref(), Some(coin_icon(4)));
    }

    #[test]
    fn the_rows_put_the_coin_first_and_resolve_items() {
        let items = Items::default();
        let mut loot = LootState::default();
        assert!(rows(&loot.window, &items).is_none());
        assert_eq!(loot.window.count(), 0, "no window counts 0 (`0x4c226d`)");
        loot.open(0x42, loot_type::CORPSE, 12_345, vec![item(0, 117, 3)]);
        let snap = rows(&loot.window, &items).expect("open");
        assert_eq!(snap.len(), 2, "coin + one item");
        assert_eq!(loot.window.count(), 2);
        let coin = snap[0].as_ref().expect("coin row present");
        assert!(coin.is_coin);
        assert_eq!(coin.money, 12_345);
        assert_eq!(coin.texture.as_deref(), Some(coin_icon(12_345)));
        // The item's name is nil while its template is in flight; its quantity is present.
        let row = snap[1].as_ref().expect("item row present");
        assert!(!row.is_coin);
        assert!(row.name.is_none());
        assert_eq!(row.quantity, 3);

        // Looting the coin turns row 1 into a gap; the item keeps its position.
        loot.clear_money();
        let snap = rows(&loot.window, &items).expect("still open");
        assert_eq!(snap.len(), 2, "the layout keeps both slots");
        assert!(snap[0].is_none(), "the looted coin slot is a gap");
        assert!(snap[1].is_some(), "the item stays at position 2");
        assert_eq!(loot.window.count(), 2, "a taken coin's -1 still counts");

        loot.remove_slot(0);
        let snap = rows(&loot.window, &items).expect("still open");
        assert_eq!(snap, vec![None, None]);
        assert!(loot.window.coin_slot, "the taken coin's cell is -1, not 0");
        assert_eq!(loot.window.count(), 1, "only the money cell counts");
        assert!(loot.window.is_empty(), "nothing lootable left");
    }

    #[test]
    fn page_math_two_pages_of_three_and_two() {
        // Four rows a page, three when there are more than four (`LootFrame.lua:70-73`, `:112`).
        let num_items = 5u32;
        let per_page = if num_items > 4 { 3 } else { 4 };
        let pages = num_items.div_ceil(per_page);
        assert_eq!(per_page, 3);
        assert_eq!(pages, 2);
        let page1: Vec<u32> = (1..=per_page).filter(|&i| i <= num_items).collect();
        let page2: Vec<u32> = (per_page + 1..=2 * per_page)
            .filter(|&i| i <= num_items)
            .collect();
        assert_eq!(page1, vec![1, 2, 3]);
        assert_eq!(page2, vec![4, 5]);
        // Four items: one page of four.
        assert_eq!(if 4u32 > 4 { 3 } else { 4 }, 4);
    }

    /// A creature corpse and a live creature, both streamed.
    const CORPSE: u64 = 0xF130_0000_0000_0042;
    const LIVE: u64 = 0xF130_0000_0000_0043;

    /// [`bare_app`] with [`CORPSE`] streamed at health 0 and [`LIVE`] at 100.
    fn close_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut app, rx) = bare_app();
        for (guid, health) in [(CORPSE, 0), (LIVE, 100)] {
            stream(&mut app, guid, EntityKind::Unit, &[(F_UNIT_HEALTH, health)]);
        }
        // The disenchanted item, which the object manager holds while its window is open.
        stream(&mut app, ESSENCE_SOURCE, EntityKind::Other, &[(2, 0x3)]);
        (app, rx)
    }

    /// The deselects asked for since the last call.
    fn deselects(app: &mut App) -> Vec<u64> {
        crate::target::take_owed_deselects(app.world_mut())
    }

    /// A movement start's guard, `0x60e990` → `0x48f200(1, 1, 0)` (`0x60e9dd`), where it runs.
    fn move_start(app: &mut App) {
        app.world_mut()
            .run_system_once(|mut close: MoveStartClose| close.close())
            .expect("the close runs as a one-shot system");
    }

    /// Opens a window as an admitted response does: the rows and the latch.
    fn open_loot(app: &mut App, guid: u64, kind: u8, gold: u32, rows: Vec<LootItem>) {
        app.world_mut()
            .resource_mut::<LootState>()
            .open(guid, kind, gold, rows);
        app.world_mut().resource_mut::<LootLatch>().0 = Some(guid);
    }

    /// Every close reaches `CloseInteraction`'s last step (`0x48f34f`–`0x48f369`): `CloseLoot`
    /// (`0x4c2ed6`), the empty-window close after the last row (`0x4c1f8d`) or the coin
    /// (`0x4c2785`), each deselecting a dead unit and only a dead unit.
    #[test]
    fn every_loot_close_deselects_a_dead_unit() {
        let (mut app, rx) = close_app();
        let released = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<u64> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::LootRelease { guid } => Some(guid),
                    _ => None,
                })
                .collect()
        };
        app.insert_non_send_resource(UiScript::new().unwrap());
        let close_loot = |app: &mut App| run_hosted(app, "CloseLoot()");

        // `CloseLoot`, as the close button, Escape or any other hide reaches it.
        open_loot(
            &mut app,
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        close_loot(&mut app);
        assert_eq!(released(&rx), [CORPSE]);
        assert_eq!(app.world().resource::<LootLatch>().0, None);
        assert_eq!(
            deselects(&mut app),
            [CORPSE],
            "CloseLoot deselects the corpse"
        );

        // The last row taken.
        open_loot(
            &mut app,
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        app.world_mut()
            .run_system_once_with(net::on_removed, SessionEvent::LootRemoved { slot: 0 })
            .unwrap();
        assert_eq!(released(&rx), [CORPSE]);
        assert_eq!(
            deselects(&mut app),
            [CORPSE],
            "the last row's close deselects"
        );

        // The coin taken from a coin-only window.
        open_loot(&mut app, CORPSE, loot_type::CORPSE, 250, vec![]);
        app.world_mut()
            .run_system_once_with(net::on_clear_money, SessionEvent::LootClearMoney)
            .unwrap();
        assert_eq!(released(&rx), [CORPSE]);
        assert_eq!(deselects(&mut app), [CORPSE], "the coin's close deselects");

        // A live pickpocket target keeps the selection.
        open_loot(
            &mut app,
            LIVE,
            loot_type::PICKPOCKETING,
            0,
            vec![item(0, 117, 1)],
        );
        close_loot(&mut app);
        assert_eq!(released(&rx), [LIVE]);
        assert!(
            deselects(&mut app).is_empty(),
            "a live unit is not deselected"
        );

        // With nothing open, a second `CloseLoot` sends and asks nothing.
        close_loot(&mut app);
        assert!(released(&rx).is_empty());
        assert!(deselects(&mut app).is_empty());
    }

    /// `CloseLoot(1)`, stock `LootFrame.lua:18`'s answer to a window that cannot open: the close
    /// raises `ERR_LOOT_NO_UI` (`0x48f27b`) before its release, and only with loot open
    /// (`0x48f23f` returns first on none) and past the disenchant hold.
    #[test]
    fn close_loot_with_its_flag_set_shows_err_loot_no_ui() {
        let (mut app, rx) = close_app();
        app.insert_non_send_resource(UiScript::new().unwrap());
        let close_loot = |app: &mut App, chunk: &str| -> Vec<&'static str> {
            run_hosted(app, chunk);
            app.world_mut()
                .resource_mut::<UiErrorKeys>()
                .take()
                .into_iter()
                .map(|e| e.key)
                .collect()
        };
        open_loot(
            &mut app,
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        assert_eq!(close_loot(&mut app, "CloseLoot(1)"), ["ERR_LOOT_NO_UI"]);
        assert!(rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::LootRelease { guid } if guid == CORPSE)));
        assert!(
            close_loot(&mut app, "CloseLoot(1)").is_empty(),
            "nothing open, nothing shown"
        );
        open_loot(
            &mut app,
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        assert!(close_loot(&mut app, "CloseLoot()").is_empty());
        open_loot(
            &mut app,
            ESSENCE_SOURCE,
            loot_type::DISENCHANTING,
            0,
            vec![item(0, 117, 1)],
        );
        assert!(
            close_loot(&mut app, "CloseLoot(1)").is_empty(),
            "a disenchant hold returns before the error"
        );
    }

    /// `CloseInteraction` on a move start, with no server help and no VM.
    #[test]
    fn a_movement_start_closes_and_releases_the_open_loot() {
        const BOBBER: u64 = 0xF110_0000_0000_0011;
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;

        // No UiScript mounted: the move-start leg must not depend on the VM.
        let (mut app, rx) = close_app();
        let open = |app: &mut App, guid: u64, kind: u8, rows: Vec<LootItem>| {
            open_loot(app, guid, kind, 0, rows);
        };
        let step = |app: &mut App| {
            move_start(app);
            app.update();
        };

        open(&mut app, CORPSE, loot_type::CORPSE, vec![item(0, 117, 1)]);
        app.update();
        assert_eq!(app.world().resource::<LootState>().source(), Some(CORPSE));
        assert!(
            rx.try_recv().is_err(),
            "nothing goes out without a move start"
        );

        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert_eq!(app.world().resource::<LootLatch>().0, None);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE),
            "the move-start close owes the wire the release"
        );
        assert!(rx.try_recv().is_err(), "and sends it once");
        assert_eq!(deselects(&mut app), vec![CORPSE]);

        // Skinning comes as wire type 2, as pick-pocketing does (`Player.cpp:8122`); the
        // deselect reads the health (`0x48f35d`), so the skinned corpse is deselected ...
        open(
            &mut app,
            CORPSE,
            loot_type::PICKPOCKETING,
            vec![item(0, 117, 1)],
        );
        step(&mut app);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE));
        assert_eq!(
            deselects(&mut app),
            vec![CORPSE],
            "a skinned corpse loses it"
        );
        // ... and a live pickpocket target is not.
        open(
            &mut app,
            LIVE,
            loot_type::PICKPOCKETING,
            vec![item(0, 117, 1)],
        );
        step(&mut app);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == LIVE));
        assert!(deselects(&mut app).is_empty(), "a live target keeps it");

        // A fishing bobber is a GameObject: the same path, minus the unit deselect.
        open(&mut app, BOBBER, loot_type::FISHING, vec![]);
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == BOBBER));
        assert!(deselects(&mut app).is_empty());

        // A lockbox survives movement, unreleased.
        open(&mut app, LOCKBOX, loot_type::CORPSE, vec![item(0, 117, 1)]);
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), Some(LOCKBOX));
        assert!(rx.try_recv().is_err());
        app.world_mut().resource_mut::<LootState>().clear();

        // A disenchant with rows left survives; once emptied, the step closes it.
        open(
            &mut app,
            CORPSE,
            loot_type::DISENCHANTING,
            vec![item(0, 117, 1)],
        );
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), Some(CORPSE));
        assert!(rx.try_recv().is_err());
        app.world_mut().resource_mut::<LootState>().remove_slot(0);
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE));
    }

    /// The precast end's release at rest, `0x48f200(1, 0, 0)` (`0x60d10b`): with `dl` clear an
    /// item's loot closes with its release and both unlocks (`0x48f2b5` falls through to
    /// `0x48f2bd`), and a disenchant window with rows left owes its sweep (`0x48f264`), where the
    /// move-start close keeps both. Beside a move-start close after it, it decides alike, as the two
    /// commute.
    #[test]
    fn a_close_at_rest_takes_an_items_loot_and_sweeps_a_disenchant_hold() {
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;
        let (mut app, rx) = close_app();
        let step = |app: &mut App, and_move_start: bool| {
            close_at_precast_end(app.world_mut(), false);
            if and_move_start {
                move_start(app);
            }
            app.update();
        };
        for move_start in [false, true] {
            open_loot(
                &mut app,
                LOCKBOX,
                loot_type::CORPSE,
                0,
                vec![item(0, 117, 1)],
            );
            step(&mut app, move_start);
            assert_eq!(app.world().resource::<LootState>().source(), None);
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == LOCKBOX),
                "an item's loot is released (beside a move start: {move_start})"
            );
            assert!(rx.try_recv().is_err(), "once");
            assert_eq!(
                app.world_mut()
                    .resource_mut::<LootState>()
                    .take_queued_unlocks(),
                vec![(LOCKBOX, true), (LOCKBOX, true)],
                "the close's unlock and the teardown's, on the lane at the close"
            );
        }

        open_loot(
            &mut app,
            CORPSE,
            loot_type::DISENCHANTING,
            0,
            vec![item(0, 117, 1)],
        );
        step(&mut app, false);
        let loot = app.world().resource::<LootState>();
        assert_eq!(loot.source(), Some(CORPSE), "a disenchant hold stays open");
        assert!(loot.sweep_owed, "and owes its sweep to the lane");
        assert!(rx.try_recv().is_err(), "unreleased");
        app.world_mut().resource_mut::<LootState>().clear();
        deselects(&mut app);

        open_loot(
            &mut app,
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        step(&mut app, false);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE));
        assert_eq!(
            deselects(&mut app),
            vec![CORPSE],
            "the close's dead-unit deselect"
        );
    }

    /// The loot lane and the sweep's loot leg over an empty world, the lane's other inputs at rest.
    fn sweep_loot_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        use crate::ui_script::SweepApp;
        let (mut app, rx) = bare_app();
        app.add_sweep_leg(SweepLeg::Loot, close_loot_on_leave_world)
            .add_systems(Update, loot_frame);
        (app, rx)
    }

    /// The response's copier looks every row up with the callback `0x4c2ac0` (`0x4c1e77`); the
    /// frames the window waits through read, as the row getters do (`0x4c241d`). A refused row is
    /// asked by no frame after it, only by the next response.
    #[test]
    fn a_refused_loot_row_is_asked_by_the_next_response_never_by_a_frame() {
        let (mut app, rx) = sweep_loot_app();
        app.insert_non_send_resource(UiScript::new().unwrap());
        let row = LootItem {
            slot: 0,
            item_id: 955,
            count: 1,
            display_info_id: 0,
            random_property_id: 0,
            slot_type: slot_type::ALLOW_LOOT,
        };
        let open = |app: &mut App| {
            app.world_mut()
                .resource_mut::<LootState>()
                .open(CORPSE, 1, 0, vec![row]);
            app.update();
        };
        let asked =
            |rx: &crossbeam_channel::Receiver<ClientCommand>| crate::query_cache::queries(rx).len();
        open(&mut app);
        assert_eq!(asked(&rx), 1);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(955, None);
        for _ in 0..600 {
            app.update();
        }
        assert_eq!(asked(&rx), 0, "no frame asks a refusal");
        open(&mut app);
        assert_eq!(asked(&rx), 1, "the next response asks again");
    }

    const EVENTS: [&str; 2] = ["LOOT_OPENED", "LOOT_CLOSED"];

    /// A `/reload` with a window open: the sweep's loot leg (`0x490ab2` → `0x48f200(cl=1)`)
    /// releases it and tells the dying VM `LOOT_CLOSED`, and the new VM opens nothing.
    #[test]
    fn a_reload_releases_the_loot_and_the_new_vm_opens_nothing() {
        use crate::ui_script::sweep_test::{reload_in_place, seen, seen_in, spied_vm};
        let (mut app, rx) = sweep_loot_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        app.world_mut()
            .resource_mut::<LootState>()
            .open(CORPSE, 1, 0, Vec::new());
        app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
        app.update();
        assert_eq!(seen_in(&app), ["LOOT_OPENED"]);

        let dying = reload_in_place(&mut app, &EVENTS);
        assert_eq!(seen(&dying), ["LOOT_OPENED", "LOOT_CLOSED"]);
        assert_eq!(
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::LootRelease { guid } if *guid == CORPSE))
                .count(),
            1,
            "the leg's `CMSG_LOOT_RELEASE` (`0x48f2da`)"
        );
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert_eq!(app.world().resource::<LootLatch>().0, None);

        app.update();
        assert!(seen_in(&app).is_empty(), "the new VM has no window to open");
    }

    /// A worldport sweeps the VM that stays: `LOOT_CLOSED` once, from the leg, and nothing after
    /// it.
    #[test]
    fn a_worldport_closes_the_loot_once() {
        use crate::ui_script::sweep_test::{seen_in, spied_vm, sweep_in_place};
        let (mut app, _rx) = sweep_loot_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        app.world_mut()
            .resource_mut::<LootState>()
            .open(CORPSE, 1, 0, Vec::new());
        app.update();
        sweep_in_place(&mut app);
        app.update();
        app.update();
        assert_eq!(seen_in(&app), ["LOOT_OPENED", "LOOT_CLOSED"]);
    }

    /// A disenchanted item's guid (`HIGHGUID_ITEM`).
    const ESSENCE_SOURCE: u64 = 0x4000_0000_0000_0d15;

    /// `CloseLoot` (`0x4c2ec0` → `0x48f200(cl=1, dl=0)`) on a disenchant window with rows left
    /// sweeps it (`0x48f264` → `0x4c1fa0`): the rows go out, no release, the latch and the
    /// window stay.
    #[test]
    fn close_loot_on_a_disenchant_window_with_rows_left_sweeps_it() {
        let (mut app, rx) = close_app();
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.world_mut().resource_mut::<Items>().insert_template(
            117,
            Some(crate::items::test_template("Lesser Magic Essence")),
        );
        open_loot(
            &mut app,
            ESSENCE_SOURCE,
            loot_type::DISENCHANTING,
            0,
            vec![item(0, 117, 1)],
        );
        run_hosted(&mut app, "CloseLoot()");
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::AutostoreLootItem { slot: 0 }]
            ),
            "{sent:?}"
        );
        assert_eq!(
            app.world().resource::<LootState>().source(),
            Some(ESSENCE_SOURCE)
        );
        assert_eq!(app.world().resource::<LootLatch>().0, Some(ESSENCE_SOURCE));
    }

    /// The sweep's loot leg has `dl` clear too: a disenchant window with rows left is swept in
    /// the VM the sweep runs in, not closed, and no `LOOT_CLOSED` fires; the sweep's flag is the
    /// leg's 0 (`0x4c1ca0`), so its end signals `LOOT_OPENED` again with the rows left
    /// (`0x4c2240`).
    #[test]
    fn the_loot_leg_sweeps_a_disenchant_window_with_rows_left() {
        use crate::ui_script::sweep_test::{seen_in, spied_vm, sweep_in_place};
        let (mut app, rx) = sweep_loot_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        app.world_mut().resource_mut::<Items>().insert_template(
            117,
            Some(crate::items::test_template("Lesser Magic Essence")),
        );
        app.world_mut().resource_mut::<LootState>().open(
            ESSENCE_SOURCE,
            loot_type::DISENCHANTING,
            0,
            vec![item(0, 117, 1)],
        );
        app.world_mut().resource_mut::<LootLatch>().0 = Some(ESSENCE_SOURCE);
        stream(&mut app, ESSENCE_SOURCE, EntityKind::Other, &[(2, 0x3)]);
        app.update();
        let _ = rx.try_iter().count();
        sweep_in_place(&mut app);
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::AutostoreLootItem { slot: 0 }]
            ),
            "{sent:?}"
        );
        assert_eq!(seen_in(&app), ["LOOT_OPENED", "LOOT_OPENED"]);
        assert_eq!(
            app.world().resource::<LootState>().source(),
            Some(ESSENCE_SOURCE)
        );
    }
}
