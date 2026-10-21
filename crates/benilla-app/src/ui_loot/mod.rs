//! The app side of the loot window. The net handlers change [`LootState`] at their packets, each
//! change a step that [`feed_loot`] shows the VM in wire order, with the item pushes queued
//! between them: the window's rows, `LOOT_SLOT_CLEARED`, `UPDATE_MASTER_LOOT_LIST` and
//! `LOOT_CLOSED`, and at a response the copier's end, `LOOT_OPENED` or the auto-loot sweep. The
//! loot verbs and a row's click apply inside their calls ([`calls::LootSteps::apply`]), and
//! [`drain_loot`] takes the closes a movement and the precast's end owe.

use benilla_protocol::messages::{ItemPushResult, LootItem, BAG_PLAYER_INVENTORY};
use bevy::prelude::*;

use benilla_ui::script::{LootRow, LootState as LootSnapshot, ScriptValue, UiScript};

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

#[cfg(test)]
mod call_tests;
mod calls;
mod net;
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
/// 1.12's `GOLD`, `SILVER` and `COPPER` (`GlobalStrings.lua:2025`, `:3465`, `:865`).
const GOLD_WORD: &str = "Gold";
const SILVER_WORD: &str = "Silver";
const COPPER_WORD: &str = "Copper";
/// Frames a push waits for its item template before it is dropped, about 2 s at 60 fps.
const RECEIVE_MAX_TRIES: u16 = 120;
/// The slots `OnItemPush` treats as the keyring (`0x491bc3`/`0x491bc8`, 0x51..=0x70): the
/// descriptor's full 32 positions, wider than the 16 vmangos uses from `KEYRING_SLOT_START` 81.
const PUSH_KEYRING_SLOTS: std::ops::RangeInclusive<u32> = 81..=112;

/// A push waiting on its item template. `OnItemPush 0x491a60` defers both of its outputs the same
/// way: on a cache miss (`0x491a93`) it re-enters from the cache callback `0x491ee0`.
#[derive(Clone)]
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
    tries: u16,
    /// How many of the frame's window steps ([`LootState::steps`]) came before it: each packet's
    /// events fire inside its dispatch (`0x537aa0`), so `ITEM_PUSH` reads the window the packets
    /// before it left.
    at: usize,
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
    /// The window's master-loot candidates (`SMSG_LOOT_MASTER_LIST`), in wire order.
    master_candidates: Vec<u64>,
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

    /// The candidate slots as `0x61c550` lays them out, split on the raid roster's count
    /// (`0x61c577`), not the group type: wire order with no roster held (`0x61c5b9`), a raid of one
    /// included; with one, each guid in the first free slot of its subgroup's block of five
    /// (`0x61c5c9`-`0x61c637`), holes kept for `GroupLootDropDown_Initialize`'s "Group N" submenus
    /// (`LootFrame.lua:197-213`). The feed and the give share it, so an index means one candidate.
    fn placed_candidates(&self, group: &GroupState) -> Vec<Option<u64>> {
        if !group.raid_held() {
            return self.master_candidates.iter().copied().map(Some).collect();
        }
        let mut slots: Vec<Option<u64>> = vec![None; MASTER_LOOT_CANDIDATE_SLOTS];
        for &guid in &self.master_candidates {
            // A guid missing from the roster is ours: `SMSG_GROUP_LIST` lists only the others,
            // and `Group::MasterLoot` includes us.
            let flags = group
                .members
                .iter()
                .find(|m| m.guid == guid)
                .map_or(group.own_flags, |m| m.flags);
            let base = usize::from(flags & GROUP_MEMBER_SUBGROUP) * MEMBERS_PER_RAID_GROUP;
            let end = (base + MEMBERS_PER_RAID_GROUP).min(MASTER_LOOT_CANDIDATE_SLOTS);
            if let Some(free) = (base..end).find(|&i| slots[i].is_none()) {
                slots[free] = Some(guid);
            }
        }
        while slots.last().is_some_and(Option::is_none) {
            slots.pop(); // trailing empties read as nil either way
        }
        slots
    }

    /// The candidate at a 1-based menu index, through [`Window::placed_candidates`].
    fn master_candidate(&self, index_1based: u32, group: &GroupState) -> Option<u64> {
        self.placed_candidates(group)
            .get(index_1based.checked_sub(1)? as usize)
            .copied()
            .flatten()
    }
}

/// A change to the window the feed has yet to show, with the window it left.
#[derive(Clone, Debug)]
enum Step {
    /// A removal (`0x4c1f00`, `0x4c2750`) or a close (`0x4c1cb0(0)`).
    Changed(Window),
    /// The coin row's clear with no window open (`0x4c2750`), which still signals
    /// `LOOT_SLOT_CLEARED` with its row (`0x4c276f`).
    SlotCleared(u32),
    /// The unlocks of a close a packet asked for (`0x48f2bd`, `0x4c1d36`), signalled in their
    /// place: after the removal's `LOOT_SLOT_CLEARED`, before the close's `LOOT_CLOSED`. Each is
    /// flagged when it needs a live item object.
    Unlocks(Vec<(u64, bool)>),
    /// A response's copier (`0x4c1cb0`): the cells its teardown leaves, the ended session's with
    /// the source zeroed (`0x4c1da0`), then the window it fills.
    Opened { ended: Window, window: Window },
}

/// The loot session: the window, the steps the feed has yet to show, the push queue, which
/// outlives the window, and what the loot calls owe.
#[derive(Resource, Default)]
pub(crate) struct LootState {
    window: Window,
    receives: Vec<PendingReceive>,
    /// The window's changes since the feed last ran, in wire order, each shown as the window it
    /// left, with the pushes queued between them ([`PendingReceive::at`]).
    steps: Vec<Step>,
    /// The row a `LOOT_BIND_CONFIRM` is open for, the reference's `[0x847cec]`: set after the
    /// signal's handlers (`0x4c2917`, `0x4c21dc`), cleared by the confirmed send (`0x4c281a`) and
    /// by the next response (`0x4c1df5`), not by a close.
    /// Deviation: it is the display row, where the reference's is the item index plus one
    /// (`0x4c2885`), so an addon's `GetLootSlotInfo(arg1)` reads the row the dialog is for.
    pending_bind_confirm: Option<u32>,
    /// The list staged for the next open: vmangos sends it from inside `Player::SendLoot`
    /// (`Player.cpp:8080`), just ahead of its response. Staging keeps it out of later windows.
    pending_master_candidates: Vec<u64>,
    /// A close with `dl` clear found a disenchant window with rows left and owes its sweep
    /// (`0x48f264` → `0x4c1fa0`), run by [`feed_loot`], which holds the VM.
    sweep_owed: bool,
    /// The `UnlockItem` (`0x495420`) calls the loot paths owe, run ahead of the inventory feeds
    /// ([`crate::ui_items::locks::apply_item_releases`]); a window's clear keeps them. The flag
    /// marks a window source's, owed only while the guid resolves as a live item object.
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
}

impl LootState {
    /// The copier (`0x4c1cb0`) at a response: it ends the session in progress, then fills the
    /// window; quest items ride the same rows at `slot = items.len() + i`.
    pub(crate) fn open(&mut self, source: u64, loot_type: u8, gold: u32, items: Vec<LootItem>) {
        // The ending unlocks an item source (`0x5ebc35` → `0x48f3a0` → `0x4c1d36`) and clears
        // the money line's flag (`0x4c1d95`).
        if let Some(old) = self.window.source {
            self.unlock_item_source(old);
            self.coin_line_shown = false;
        }
        let ended = Window {
            source: None,
            ..self.window.clone()
        };
        self.window = Window {
            source: Some(source),
            gold,
            coin_slot: gold > 0, // `0x4c1de9`
            items,
            taken: Vec::new(),
            loot_type, // `0x4c1dd6`
            master_candidates: std::mem::take(&mut self.pending_master_candidates),
        };
        self.pending_bind_confirm = None; // `0x4c1df5`
        self.sweep_owed = false;
        self.copier_due = true;
        self.steps.push(Step::Opened {
            ended,
            window: self.window.clone(),
        });
    }

    /// Stages a list for the next open; an open window takes it at once, as a refresh.
    pub(crate) fn set_master_candidates(&mut self, candidates: Vec<u64>) {
        if self.window.source.is_some() {
            self.window.master_candidates.clone_from(&candidates);
        }
        self.pending_master_candidates = candidates;
    }

    /// `ClearLootRow` (`0x4c1f00`): the row with the wire slot becomes a gap, whether or not a
    /// window is open; whether one did.
    pub(crate) fn remove_slot(&mut self, wire_slot: u8) -> bool {
        let w = &mut self.window;
        if !w.items.iter().any(|it| it.slot == wire_slot) || w.taken.contains(&wire_slot) {
            return false; // `0x4c1f22`
        }
        w.taken.push(wire_slot);
        self.steps.push(Step::Changed(self.window.clone()));
        true
    }

    /// `ClearLootCoinRow` (`0x4c2750`): money above 0 goes to -1, the coin row a gap, whether a
    /// window is open or not; whether it was there (`0x4c2757`).
    pub(crate) fn clear_money(&mut self) -> bool {
        if !self.window.has_coin() {
            return false;
        }
        self.take_coin();
        self.steps.push(match self.window.source {
            Some(_) => Step::Changed(self.window.clone()),
            None => Step::SlotCleared(1),
        });
        true
    }

    /// The money cell to -1 (`0x4c2858`, `0x4c2765`): the coin row stays a gap.
    fn take_coin(&mut self) {
        self.window.gold = 0;
    }

    /// Queues a push the net handler has already checked is ours.
    pub(crate) fn push_receive(&mut self, p: &ItemPushResult) {
        self.receives.push(PendingReceive {
            created: p.created,
            entry: p.item_entry,
            count: p.count,
            from_npc: p.from_npc,
            random_property_id: p.random_property_id,
            suffix_factor: p.suffix_factor,
            in_chat: p.show_in_chat,
            container: push_container(p.bag_slot, p.item_slot),
            tries: 0,
            at: self.steps.len(),
        });
    }

    /// The close's teardown (`0x4c1cb0(0)`): the source goes (`0x4c1da0`) and the rows stay; the
    /// feed signals `LOOT_CLOSED` for a window it showed. Queued pushes outlive it.
    pub(crate) fn clear(&mut self) {
        if self.window.source.take().is_some() {
            self.steps.push(Step::Changed(self.window.clone()));
            self.coin_line_shown = false; // `0x4c1d95`
        }
        self.copier_due = false;
        self.sweep_owed = false;
        self.pending_master_candidates.clear();
    }

    /// Session end: drops the window, the queued pushes and the owed unlocks.
    pub(crate) fn clear_session(&mut self) {
        *self = LootState::default();
    }

    /// Owe `UnlockItem` on `guid`, which fires its `ITEM_LOCK_CHANGED` whether or not the guid
    /// names an item we hold (`0x495450`); a zero guid owes nothing (`0x49542d`).
    pub(crate) fn unlock_item(&mut self, guid: u64) {
        if guid != 0 {
            self.item_unlocks.push((guid, false));
        }
    }

    /// Owe the window teardown's `UnlockItem` on its source, which runs only when the source
    /// resolves as a live item object (`0x48f299`/`0x48f2ab`, `0x4c1cdf`/`0x4c1d23`): checked
    /// where the unlocks run, against the objects.
    pub(crate) fn unlock_item_source(&mut self, guid: u64) {
        if benilla_protocol::guid::is_item(guid) {
            self.item_unlocks.push((guid, true));
        }
    }

    /// How many unlocks are owed, a mark for [`Self::defer_close_unlocks`].
    pub(crate) fn owed_unlocks(&self) -> usize {
        self.item_unlocks.len()
    }

    /// Signal the unlocks a close owed past `mark` in their place in the steps, ahead of its
    /// teardown's step: for a close inside a packet, which the feed shows.
    pub(crate) fn defer_close_unlocks(&mut self, mark: usize) {
        let unlocks = self
            .item_unlocks
            .split_off(mark.min(self.item_unlocks.len()));
        if unlocks.is_empty() {
            return;
        }
        let at = self.steps.len().saturating_sub(1);
        self.steps.insert(at, Step::Unlocks(unlocks));
    }

    /// Whether any unlock is owed.
    pub(crate) fn owes_unlocks(&self) -> bool {
        !self.item_unlocks.is_empty()
    }

    /// The owed unlocks, in order, each flagged when it needs a live item object.
    pub(crate) fn take_item_unlocks(&mut self) -> Vec<(u64, bool)> {
        std::mem::take(&mut self.item_unlocks)
    }

    /// Queued pushes, for the net handler's self-gate test.
    #[cfg(test)]
    pub(crate) fn pending_receive_count(&self) -> usize {
        self.receives.len()
    }

    /// The open loot's guid; the chest lid watcher in [`crate::go_anim`] reads it too.
    pub(crate) fn source(&self) -> Option<u64> {
        self.window.source
    }

    /// `LootHasContents` (`0x4c2a70`): the source resolves as any object (`0x4c2a8c`, mask 1) and
    /// has money above 0 or a row left.
    fn has_contents(&self, objects: &crate::net::Objects) -> bool {
        self.window
            .source
            .is_some_and(|source| objects.entity(source).is_some())
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
    /// [`templates_outstanding`] has it.
    fn records_pending(&self, items: &Items) -> bool {
        let w = &self.window;
        w.items.iter().any(|it| {
            !w.taken.contains(&it.slot)
                && items.template(it.item_id, 0, Ask::Read).is_none()
                && !items.template_answered_unknown(it.item_id)
        })
    }

    /// Owe the hold's sweep to the feed, from a close with no VM at hand.
    pub(crate) fn owe_sweep(&mut self) {
        self.sweep_owed = true;
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

/// The loot window's close owed this frame, `0x48f200(1, 1, 0)` while a loot target is latched
/// (`0x6126b0`). Two writers call it: the movement-start guard `0x60e990` (`0x60e9dd`), called by
/// every movement-start emitter, turning included, but not by mouse-look facing; and the local
/// stand-state apply for a change to any state but 0 (`0x5f07d5`). The precast end's release
/// owes it too while the player moves (`0x60d10b`, [`LootCloseAtRest`] at rest). No loot target reaches the
/// range gate `0x493230`, so there is no distance leash. vmangos also releases on movement
/// (`MovementHandler.cpp:1104`), but the close is the client's. A loot response that lands while
/// we move sets it too (`0x5ebc3a`).
#[derive(Resource, Default)]
pub(crate) struct LootMoveStart(pub(crate) bool);

/// The loot window's close owed this frame with `dl` clear, `0x48f200(1, 0, 0)`: the precast
/// end's release for the active player at rest (`0x60d0ee`-`0x60d10b`, its flags `& 0x200f`
/// clear), which closes an item's loot and sweeps a disenchant window with rows left, where the
/// move-start close keeps both.
#[derive(Resource, Default)]
pub(crate) struct LootCloseAtRest(pub(crate) bool);

pub(crate) struct UiLootPlugin;

/// Applies a loot CVar change.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut loot: ResMut<LootConfig>) {
    match ev.key().as_str() {
        "autolootdefault" => loot.auto_loot = ev.flag(),
        "showlootspam" => loot.show_loot_spam = ev.flag(),
        _ => {}
    }
}

impl Plugin for UiLootPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.add_observer(on_cvar);
        app.init_resource::<LootState>()
            .init_resource::<LootConfig>()
            .init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<LootMoveStart>()
            .init_resource::<LootCloseAtRest>()
            .init_resource::<AutoLootByte>()
            .add_sweep_leg(SweepLeg::Loot, close_loot_on_leave_world)
            .add_systems(
                Update,
                (
                    // Feed before the input pass and drain after it: an open shows the same
                    // frame. The drain's deselect lands before the selection writers read it, and
                    // their teardown's close finds this frame's window.
                    feed_loot.in_set(UiFeed),
                    drain_loot
                        .in_set(crate::ui_script::UiDrain)
                        .before(crate::target::TargetUpdate),
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

/// The coin row's name: each nonzero denomination as "<n> <Word>", joined by spaces. The
/// reference's composer `0x6c6260` emits the same "%d %s" parts but separates them with a newline
/// (`0x835144`).
fn format_money(copper: u32) -> String {
    let (g, s, c) = (copper / 10000, (copper % 10000) / 100, copper % 100);
    let mut parts: Vec<String> = Vec::new();
    if g > 0 {
        parts.push(format!("{g} {GOLD_WORD}"));
    }
    if s > 0 {
        parts.push(format!("{s} {SILVER_WORD}"));
    }
    if c > 0 || parts.is_empty() {
        parts.push(format!("{c} {COPPER_WORD}"));
    }
    parts.join(" ")
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
        item_id: item.item_id,
        link,
        // The raw id, as the client's loot record keeps it; the tooltip resolves it (`0x52b7bf`).
        random_property_id: item.random_property_id,
    }
}

/// Whether a row still waits on its item template: the reference's pending-query counter
/// `[0xb71b44]`. Deviation: a negative answer releases the row, where the reference's callback
/// `0x4c2ac0` returns on a failed query and the window never opens, because an entry the server
/// cannot describe would otherwise keep the loot shut for good.
fn templates_outstanding(items: &Items, snap: &LootSnapshot) -> bool {
    snap.rows.iter().flatten().any(|r| {
        !r.is_coin
            && r.item_id != 0
            && r.name.is_none()
            && !items.template_answered_unknown(r.item_id)
    })
}

/// The Lua snapshot: one entry per slot of the fixed layout, coin first, a looted slot `None`.
fn snapshot(
    window: &Window,
    items: &Items,
    icons: Option<&ItemDisplays>,
    ask: Ask<'_>,
    rolls: RollCatalogs,
    who: Candidates,
) -> Option<LootSnapshot> {
    window.source?;
    let mut rows = Vec::with_capacity(window.items.len() + 1);
    if window.coin_slot {
        rows.push(window.has_coin().then(|| LootRow {
            name: Some(format_money(window.gold)),
            texture: Some(coin_icon(window.gold).into()),
            // 0: `0x4c22e0`'s coin guard returns before the record read (`0x4c22fd`).
            quantity: 0,
            quality: Some(COIN_QUALITY),
            is_coin: true,
            item_id: 0,
            // No item behind it: a modified click finds nil.
            link: None,
            random_property_id: 0,
        }));
    }
    for it in &window.items {
        rows.push(
            (!window.taken.contains(&it.slot)).then(|| resolve_item(it, items, icons, ask, rolls)),
        );
    }
    Some(LootSnapshot {
        rows,
        has_money: window.coin_slot,
        master_candidates: who.names_for(window),
        // The feed's, which holds the object manager.
        source_unit: false,
    })
}

/// A candidate's name sources: the roster for everyone else, the name cache for our own guid,
/// which vmangos lists as a candidate (`Group.cpp:928-937`) but never sends as a member.
#[derive(Clone, Copy)]
struct Candidates<'a> {
    group: &'a GroupState,
    names: &'a NameCache,
}

impl Candidates<'_> {
    /// The candidate slots as names, an empty slot and an unresolved name both `None`: the
    /// binding `0x4c2f10` pushes nil on a name-cache miss (`0x4c2f91`), and the cache callback
    /// `0x4c2fb0` fires `UPDATE_MASTER_LOOT_LIST` when the name lands.
    fn names_for(&self, window: &Window) -> Vec<Option<String>> {
        window
            .placed_candidates(self.group)
            .into_iter()
            .map(|slot| slot.and_then(|guid| self.name(guid)))
            .collect()
    }

    fn name(&self, guid: u64) -> Option<String> {
        self.group
            .members
            .iter()
            .find(|m| m.guid == guid)
            .map(|m| m.name.clone())
            .or_else(|| {
                self.names
                    .resolve_unit(guid, None, Ask::Read)
                    .map(str::to_string)
            })
            .filter(|n| !n.is_empty())
    }
}

/// `OnItemPush`'s self line (`0x491bfb`): its `%s` is an item link (`0x52adb0`, at `0x491c43`),
/// so a count after it takes the line's own colour. The strings are `GlobalStrings.lua:2599-2605`,
/// with no space before `x%d`; created wins over received (`0x491c04`-`0x491c1b`).
fn receive_line(r: &PendingReceive, name: &str, quality: u32) -> String {
    let verb = if r.created {
        "You create"
    } else if r.from_npc {
        "You receive item"
    } else {
        "You receive loot"
    };
    let link = crate::ui_items::item_link_full(
        r.entry,
        0,
        r.random_property_id,
        r.suffix_factor,
        name,
        quality,
    );
    if r.count > 1 {
        format!("{verb}: {link}x{}.", r.count)
    } else {
        format!("{verb}: {link}.")
    }
}

/// Emits each push whose template has landed, in `OnItemPush`'s order: `ITEM_PUSH` first
/// (`0x491be8`), then the chat line if the wire asked for it. Deviation: a push is dropped after
/// [`RECEIVE_MAX_TRIES`] frames, where the reference waits on the item-cache callback with no
/// timeout, because an entry the server never describes would otherwise stay queued.
fn drain_receives(
    pending: Vec<PendingReceive>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    chat: &crate::ui_chat::ChatLog,
    script: &mut UiScript,
    rolls: RollCatalogs,
) -> Vec<PendingReceive> {
    let mut still = Vec::new();
    for mut r in pending {
        // The push's lookup with the callback `0x491ee0` (`0x491b05`); its retries read.
        let ask = if r.tries == 0 {
            Ask::Event(commands)
        } else {
            Ask::Read
        };
        let resolved = items
            .template(r.entry, 0, ask)
            .map(|t| (t.name.clone(), t.quality, t.display_info_id));
        match resolved {
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
                if r.in_chat {
                    chat.push_event(crate::ui_chat::ChatEvent::text_only(
                        crate::ui_chat::ChatEventKind::Loot,
                        receive_line(&r, &rolls.name(&name, r.random_property_id), quality),
                    ));
                }
            }
            None => {
                r.tries += 1;
                if r.tries < RECEIVE_MAX_TRIES {
                    still.push(r);
                }
            }
        }
    }
    still
}

/// The object manager as `SetLootPortrait` asks it (`0x468460`, typemask 8): whether the loot
/// source is a unit object we hold.
#[derive(bevy::ecs::system::SystemParam)]
struct LootSourceObjects<'w, 's> {
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
}

impl LootSourceObjects<'_, '_> {
    fn is_unit(&self, guid: u64) -> bool {
        self.index
            .as_ref()
            .and_then(|index| index.0.get(&guid))
            .and_then(|&e| self.stores.get(e).ok())
            .is_some_and(|s| s.is_unit())
    }
}

/// Shows the frame's window steps in wire order, each a turn whose handlers run as it ends, with
/// the pushes queued between them fired in their places: each packet's events fire inside its
/// dispatch (`0x537aa0`), so `ITEM_PUSH` (`0x491be8`) reads the window the packets before it left.
/// A response's step ends with its copier's end. Then the window as it stands: a hold's owed
/// sweep, a copier's end a late item record let through, and the names and candidates that
/// resolved since.
fn feed_loot(
    mut vm: VmWith<(LootSteps<'static, 'static>, ResMut<'static, LootMoveStart>)>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    chat: Res<crate::ui_chat::ChatLog>,
    // The random-suffix catalogs: a loot slot has no item object to carry the roll.
    props: Option<Res<crate::items::RandomProperties>>,
    enchants: Option<Res<crate::items::Enchants>>,
    group: Res<GroupState>,
    names: Res<NameCache>,
    objects: LootSourceObjects,
) {
    let rolls = RollCatalogs {
        props: props.as_deref(),
        enchants: enchants.as_deref(),
    };
    let who = Candidates {
        group: &group,
        names: &names,
    };
    let view = |w: &Window, ask: Ask<'_>| {
        snapshot(w, &items, icons.as_deref(), ask, rolls, who).map(|mut snap| {
            snap.source_unit = w.source.is_some_and(|g| objects.is_unit(g));
            snap
        })
    };
    let waits = |snap: &Option<LootSnapshot>| {
        snap.as_ref()
            .is_some_and(|snap| templates_outstanding(&items, snap))
    };
    let pushes = PushInputs {
        items: &items,
        icons: icons.as_deref(),
        commands: &commands,
        chat: &chat,
        rolls,
    };
    let (receives, steps) = {
        let (mut s, _) = vm.own();
        (
            std::mem::take(&mut s.loot.receives),
            std::mem::take(&mut s.loot.steps),
        )
    };
    let mut between: Vec<Vec<PendingReceive>> = (0..=steps.len()).map(|_| Vec::new()).collect();
    for r in receives {
        let k = r.at.min(steps.len());
        between[k].push(r);
    }
    let last_open = steps.iter().rposition(|s| matches!(s, Step::Opened { .. }));
    // A step of a session a later response replaced runs on the cells it left, as its packet's
    // dispatch ran it before the next arrived; the steps after the last response, on the window.
    let replaced = |k: usize| last_open.is_some_and(|last| k < last);
    let mut between = between.into_iter();
    let mut still = emit_pushes(&mut vm, between.next().unwrap_or_default(), &pushes);
    for ((k, step), after) in steps.into_iter().enumerate().zip(between) {
        match step {
            Step::Changed(w) => {
                let live = replaced(k).then(|| swap_in_cells(&mut vm, w.clone()));
                vm.turn(|script, _| {
                    if let Some(script) = script {
                        show(script, view(&w, Ask::Read));
                    }
                });
                if let Some(live) = live {
                    restore_live(&mut vm, live);
                }
            }
            Step::SlotCleared(row) => vm.turn(|script, _| {
                if let Some(script) = script {
                    script.fire_event("LOOT_SLOT_CLEARED", vec![ScriptValue::Int(i64::from(row))]);
                }
            }),
            Step::Unlocks(unlocks) => vm.turn(|script, (steps, _)| {
                if let Some(script) = script {
                    fire_unlocks(script, unlocks, &steps.objects);
                }
            }),
            Step::Opened { ended, window: w } => {
                // The copier ends the session the VM shows (`0x4c1ccb`-`0x4c1dac`): its
                // `LOOT_CLOSED` handlers run on the cells the teardown leaves, the source zeroed,
                // before the new source is stored (`0x4c1dc2`), so their `CloseLoot` finds no
                // window (`0x48f23f`). The stash clear comes after them (`0x4c1df5`).
                let live = swap_in_cells(&mut vm, ended);
                vm.turn(|script, _| {
                    if let Some(script) = script.filter(|s| s.shown_loot().is_some()) {
                        show(script, None);
                    }
                });
                restore_live(&mut vm, live);
                vm.own().0.loot.pending_bind_confirm = None;
                let waiting = vm.turn(|script, _| {
                    let Some(script) = script else {
                        return true;
                    };
                    // Then the type cell (`0x4c1dd6`), the window, and each row's record asked
                    // for (`0x4c1e77`).
                    script.set_loot_type(w.loot_type);
                    let snap = view(&w, Ask::Event(&commands));
                    let waiting = waits(&snap);
                    show(script, snap);
                    waiting
                });
                if !waiting {
                    if Some(k) != last_open {
                        let moving = vm.own().1 .0;
                        end_superseded_open(&mut vm, w, moving);
                    } else if vm.own().0.loot.copier_due {
                        let moving = std::mem::take(&mut vm.own().1 .0);
                        end_open(&mut vm, moving);
                    }
                }
            }
        }
        still.extend(emit_pushes(&mut vm, after, &pushes));
    }
    if vm.own().0.loot.sweep_owed {
        vm.turn(|script, (mut steps, _)| {
            steps.loot.sweep_owed = false;
            if let (Some(script), true) = (script, steps.loot.source().is_some()) {
                steps.sweep(script, false);
            }
        });
        run_then(&mut vm);
    }
    let late = vm.turn(|script, (steps, _)| {
        let Some(script) = script else {
            return false;
        };
        // A new VM's type cell too (`/reload`): the C cell outlives the interface.
        script.set_loot_type(steps.loot.window.loot_type);
        let snap = view(&steps.loot.window, Ask::Read);
        let ready = steps.loot.copier_due && !waits(&snap);
        show(script, snap);
        ready
    });
    if late {
        let moving = std::mem::take(&mut vm.own().1 .0);
        end_open(&mut vm, moving);
    }
    // What waits on its template fires ahead of the next frame's steps.
    let (mut s, _) = vm.own();
    for r in &mut still {
        r.at = 0;
    }
    still.append(&mut s.loot.receives);
    s.loot.receives = still;
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
    restore_live(vm, live);
    let (mut s, _) = vm.own();
    let loot = &mut *s.loot;
    loot.copier_due = true;
    loot.coin_line_shown = false;
    loot.sweep_owed = false;
    loot.pending_bind_confirm = None;
}

/// The live window and the steps' mark, set aside while a step runs on the cells it left.
struct Live {
    window: Window,
    mark: usize,
}

/// Put `cells`, the loot cells as a step left them, in the window's place for that step's turn.
fn swap_in_cells<X: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<(LootSteps<'static, 'static>, X)>,
    cells: Window,
) -> Live {
    let (mut s, _) = vm.own();
    Live {
        window: std::mem::replace(&mut s.loot.window, cells),
        mark: s.loot.steps.len(),
    }
}

/// Put the live window back: the later copier rewrote every cell (`0x4c1dc2` on), so what the
/// step's handlers changed in them goes, with the steps their calls queued.
fn restore_live<X: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<(LootSteps<'static, 'static>, X)>,
    live: Live,
) {
    let (mut s, _) = vm.own();
    s.loot.window = live.window;
    s.loot.steps.truncate(live.mark);
}

/// A response's end, once its window is filled: the copier's end ([`LootSteps::copier_end`]) and
/// what its signals owe, then the response's tail (`0x5ebc3a`-`0x5ebc51`): a window opened while
/// we move (`moving`, [`LootMoveStart`]) takes the move-start close in the same handler, so it
/// never paints.
fn end_open<X: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<(LootSteps<'static, 'static>, X)>,
    moving: bool,
) {
    vm.turn(|script, (mut steps, _)| {
        steps.loot.copier_due = false;
        if let Some(script) = script {
            steps.copier_end(script);
        }
    });
    run_then(vm);
    if !moving {
        return;
    }
    vm.turn(|script, (mut steps, _)| {
        let closed = close_on_move_start(&mut steps.loot, &mut steps.latch, &steps.net);
        if closed.is_some() {
            steps.dead.after_close(closed);
            if let Some(script) = script {
                show(script, None);
            }
        }
    });
}

/// Run what the steps owe once their signals' handlers have run ([`calls::Then`]), each a turn
/// of its own, innermost first.
fn run_then<X: bevy::ecs::system::SystemParam + 'static>(
    vm: &mut VmWith<(LootSteps<'static, 'static>, X)>,
) {
    loop {
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

/// Push `fresh` over the window the VM shows and signal what went: `LOOT_SLOT_CLEARED` for each
/// row, with its position plus one (`0x4c1f77`, `0x4c276f`), `UPDATE_MASTER_LOOT_LIST` for a new
/// candidate list (`LootFrame.lua:62-64`), and `LOOT_CLOSED` for the window (`0x4c1dac`). A window
/// that appears signals nothing: its copier's end does.
fn show(script: &mut UiScript, fresh: Option<LootSnapshot>) {
    let before = script.shown_loot();
    if before == fresh {
        return;
    }
    script.set_loot(fresh.clone());
    match (before, fresh) {
        (Some(before), Some(after)) => {
            for (i, was) in before.rows.iter().enumerate() {
                let gone = was.is_some() && after.rows.get(i).is_none_or(Option::is_none);
                if gone {
                    script.fire_event("LOOT_SLOT_CLEARED", vec![ScriptValue::Int(i as i64 + 1)]);
                }
            }
            if before.master_candidates != after.master_candidates {
                script.fire_event("UPDATE_MASTER_LOOT_LIST", vec![]);
            }
        }
        (Some(_), None) => script.fire_event("LOOT_CLOSED", vec![]),
        (None, _) => {}
    }
}

/// What a push's firing reads.
struct PushInputs<'a> {
    items: &'a Items,
    icons: Option<&'a ItemDisplays>,
    commands: &'a NetCommands,
    chat: &'a crate::ui_chat::ChatLog,
    rolls: RollCatalogs<'a>,
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
        ),
        None => batch,
    })
}

/// [`feed_loot`] as a one-shot, for a test that drives the window from outside the module.
#[cfg(test)]
pub(crate) fn feed_loot_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    seat_loot_steps(world);
    world
        .run_system_once(feed_loot)
        .expect("the loot feed runs as a one-shot system");
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
    world.init_resource::<Messages<crate::target::DeselectGuid>>();
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
/// `0x48f2da`) and the frame closes (`0x48f33d`). Returns the closed source for
/// [`DeadUnitDeselect`], the close's last step.
pub(crate) fn close_interaction(
    loot: &mut LootState,
    latch: &mut LootLatch,
    release: Option<&NetCommands>,
) -> Option<u64> {
    let source = loot.source()?;
    latch.clear_for(source);
    if let Some(commands) = release {
        let _ = commands.0.send(ClientCommand::LootRelease { guid: source });
    }
    // An item source unlocks twice, each with its `ITEM_LOCK_CHANGED`: the close's own
    // `UnlockItem` (`0x48f2bd`), then the frame teardown's (`0x4c1d36`). A move-start close, the
    // one with `dl` set, returns before both and never reaches here with an item.
    loot.unlock_item_source(source);
    loot.unlock_item_source(source);
    loot.clear();
    Some(source)
}

/// The leave-world sweep's loot leg (`0x490ab2` → `0x4c1ca0` → `0x48f200(cl=1, dl=0, 0)`): the
/// shared close with its `CMSG_LOOT_RELEASE` (`0x48f2da`), told to the VM the sweep runs in, the
/// dying one on a `/reload` ([`tell_loot_closed`]). The dead-unit deselect follows, as the
/// close's last step. A disenchant window with contents is swept instead
/// ([`LootState::disenchant_hold`]).
pub(crate) fn close_loot_on_leave_world(mut vm: VmWith<(LootSteps<'static, 'static>, ())>) {
    let closed = vm.turn(|script, (mut steps, _)| {
        if steps.loot.disenchant_hold(&steps.objects) {
            match script {
                Some(script) => steps.sweep(script, false),
                None => steps.loot.owe_sweep(),
            }
            return None;
        }
        let closed = close_interaction(&mut steps.loot, &mut steps.latch, Some(&steps.net))?;
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
        if let Some(script) = script {
            announce_loot_closed(script);
        }
        steps.dead.after_close(closed);
    })
}

/// Tell the VM in the world that a close the feed will not see closed the window: the UI
/// shutdown's deselect closing the loot on its target (`0x490bdf` → `0x493910` → `0x48f200`).
pub(crate) fn tell_loot_closed(
    mut vm: VmWith<ResMut<'static, LootState>>,
    objects: crate::net::Objects,
) {
    vm.turn(|script, mut loot| {
        if let Some(script) = script {
            unlock_closed_source(script, &mut loot, &objects);
        }
    });
    vm.turn(|script, _| {
        if let Some(script) = script {
            announce_loot_closed(script);
        }
    })
}

/// The VM's half of a close that runs where the feed cannot report it, first: an item source's
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

/// The close's teardown in the VM: the window goes and `LOOT_CLOSED` signals (`0x4c1dac`).
fn announce_loot_closed(script: &mut UiScript) {
    script.set_loot(None);
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
    asks: MessageWriter<'w, crate::target::DeselectGuid>,
}

impl DeadUnitDeselect<'_, '_> {
    pub(crate) fn after_close(&mut self, closed: Option<u64>) {
        let Some(source) = closed else {
            return;
        };
        let dead_unit = self
            .index
            .0
            .get(&source)
            .and_then(|&e| self.objects.get(e).ok())
            .is_some_and(|(net_entity, store)| {
                matches!(
                    net_entity.kind,
                    benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player
                ) && (store.0.unit_health().unwrap_or(0) as i32) <= 0 // signed, `jg` at `0x48f360`
            });
        if dead_unit {
            self.asks.write(crate::target::DeselectGuid(source));
        }
    }
}

/// `CloseInteraction 0x48f200(cl=1, dl=1, 0)` on a movement start. A disenchant window with rows
/// left (`0x48f24a`) and an item's loot (`0x48f2ab`) survive; anything else takes the shared close.
fn close_on_move_start(
    loot: &mut LootState,
    latch: &mut LootLatch,
    commands: &NetCommands,
) -> Option<u64> {
    use benilla_protocol::guid;
    use benilla_protocol::messages::loot_type;
    let source = loot.source()?;
    if loot.window.loot_type == loot_type::DISENCHANTING && !loot.window.is_empty() {
        return None;
    }
    if guid::is_item(source) {
        return None;
    }
    debug!("ui_loot: movement started with {source:#x} open — release");
    close_interaction(loot, latch, Some(commands))
}

/// The closes the drain takes: the movement start's and the precast end's at rest.
type OwedCloses = (
    ResMut<'static, LootMoveStart>,
    ResMut<'static, LootCloseAtRest>,
);

/// The closes the frame owes from outside the VM, after the input pass: the movement start's
/// ([`LootMoveStart`]) and the precast end's at rest ([`LootCloseAtRest`]), which sweeps a
/// disenchant window with rows left instead. The feed signals `LOOT_CLOSED` for them.
fn drain_loot(mut vm: VmWith<(LootSteps<'static, 'static>, OwedCloses)>) {
    vm.turn(|script, (mut steps, (mut move_start, mut at_rest))| {
        if std::mem::take(&mut move_start.0) {
            let closed = close_on_move_start(&mut steps.loot, &mut steps.latch, &steps.net);
            steps.dead.after_close(closed);
        }
        // After the move-start close, which it subsumes: whichever runs first, a window either
        // keeps is the one this takes.
        if std::mem::take(&mut at_rest.0) {
            if steps.loot.disenchant_hold(&steps.objects) {
                match script {
                    Some(script) => steps.sweep(script, false),
                    None => steps.loot.owe_sweep(),
                }
            } else {
                let closed = close_interaction(&mut steps.loot, &mut steps.latch, Some(&steps.net));
                steps.dead.after_close(closed);
            }
        }
    });
    run_then(&mut vm);
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

    fn nobody() -> (GroupState, NameCache) {
        (GroupState::default(), NameCache::default())
    }

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

    /// A bare world with what the loot systems read and write, the feed's other inputs at rest.
    fn bare_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::LootPickupSound>()
            .add_message::<crate::target::DeselectGuid>()
            .init_resource::<LootState>()
            .init_resource::<LootLatch>()
            .init_resource::<LootMoveStart>()
            .init_resource::<LootCloseAtRest>()
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

    /// `SetLootPortrait` answers 1 only while the loot source is a unit object we hold
    /// (`0x4c2bd1`-`0x4c2be9`): a creature's corpse, never a chest or a source out of view.
    #[test]
    fn the_loot_portrait_follows_a_unit_source_only() {
        use benilla_protocol::ObjectFields;
        for (unit, expected) in [(true, true), (false, false)] {
            let (mut app, _rx) = bare_app();
            app.add_systems(bevy::prelude::Update, feed_loot);
            // OBJECT_FIELD_TYPE (2): TYPEMASK_OBJECT | TYPEMASK_UNIT for a creature, | 0x20 for a
            // game object.
            let kind = if unit { 0x9 } else { 0x21 };
            let e = app
                .world_mut()
                .spawn(crate::net::ObjectStore(ObjectFields::from_pairs(&[(
                    2, kind,
                )])))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(0x42, e);
            app.world_mut().resource_mut::<LootState>().open(
                0x42,
                loot_type::CORPSE,
                5,
                Vec::new(),
            );
            let script = UiScript::new().unwrap();
            script
                .run(r#"CreateFrame("Frame", "LootHost"):CreateTexture("LootTex")"#)
                .unwrap();
            app.insert_non_send_resource(script);
            app.update();
            let answered = app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<bool>("return SetLootPortrait(LootTex) == 1")
                .unwrap();
            assert_eq!(answered, expected, "unit source: {unit}");
        }
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
            app.add_systems(bevy::prelude::Update, feed_loot);
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

            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(TOUGH_JERKY, answer.clone());
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
    /// interaction stored set, its `LOOT_BIND_CONFIRM` rows noted, then `lua` run; the feed has not
    /// run.
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
        app.world_mut().run_system_once(feed_loot).unwrap();

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
        app.world_mut().run_system_once(feed_loot).unwrap();
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

    #[test]
    fn the_candidate_list_arrives_before_its_window_and_does_not_outlive_it() {
        let mut loot = LootState::default();
        // A default GroupState is a party (`group_type` 0): the flat placement path.
        let party = GroupState::default();

        loot.set_master_candidates(vec![0xA, 0xB, 0xC]);
        assert_eq!(
            loot.window.master_candidate(1, &party),
            None,
            "no window, no candidates"
        );

        loot.open(0x42, loot_type::CORPSE, 0, vec![master_item(0, 117, 1)]);
        assert_eq!(loot.window.master_candidate(1, &party), Some(0xA));
        assert_eq!(loot.window.master_candidate(3, &party), Some(0xC));
        assert_eq!(
            loot.window.master_candidate(4, &party),
            None,
            "past the end"
        );
        assert_eq!(
            loot.window.master_candidate(0, &party),
            None,
            "0 is not a Lua index"
        );

        // A refresh while the window is up applies in place.
        loot.set_master_candidates(vec![0xA, 0xB]);
        assert_eq!(loot.window.master_candidate(2, &party), Some(0xB));
        assert_eq!(
            loot.window.master_candidate(3, &party),
            None,
            "the list shrank"
        );

        loot.clear();
        loot.open(0x43, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert_eq!(
            loot.window.master_candidate(1, &party),
            None,
            "a fresh window starts empty"
        );
    }

    /// `0x61c550`'s raid placement: each guid in the first free slot of its subgroup's block.
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
            own_raid_row: Some(0xA),
            ..GroupState::default()
        };
        raid.members = vec![
            member(0xB, "Cairne", 0),
            member(0xC, "Vol", 2),
            member(0xD, "Sylvanas", 2),
        ];

        let mut loot = LootState::default();
        // The wire order interleaves the subgroups; placement ignores it.
        loot.set_master_candidates(vec![0xC, 0xA, 0xD, 0xB]);
        loot.open(0x42, loot_type::CORPSE, 0, vec![master_item(0, 117, 1)]);

        // Subgroup 0 fills slots 1-2 (us at 0xA, then Cairne); subgroup 2 fills 11-12, in the
        // order the wire listed them.
        assert_eq!(
            loot.window.master_candidate(1, &raid),
            Some(0xA),
            "us, group 1"
        );
        assert_eq!(
            loot.window.master_candidate(2, &raid),
            Some(0xB),
            "Cairne, group 1"
        );
        for empty in [3, 4, 5, 6, 7, 8, 9, 10] {
            assert_eq!(
                loot.window.master_candidate(empty, &raid),
                None,
                "slot {empty} belongs to an empty group"
            );
        }
        assert_eq!(
            loot.window.master_candidate(11, &raid),
            Some(0xC),
            "Vol, group 3"
        );
        assert_eq!(
            loot.window.master_candidate(12, &raid),
            Some(0xD),
            "Sylvanas, group 3"
        );
        assert_eq!(loot.window.master_candidate(13, &raid), None);

        // In a party the same list is flat, in wire order.
        let party = GroupState {
            members: raid.members.clone(),
            ..GroupState::default()
        };
        assert_eq!(loot.window.master_candidate(1, &party), Some(0xC));
        assert_eq!(loot.window.master_candidate(4, &party), Some(0xB));
        assert_eq!(loot.window.master_candidate(5, &party), None);

        // A raid of one holds no roster (`0x5e6e71`), so the count reads 0 and the list is flat,
        // whatever subgroup we sit in.
        let alone = GroupState {
            group_type: crate::ui_party::GROUPTYPE_RAID,
            own_flags: 2,
            ..GroupState::default()
        };
        let mut solo = LootState::default();
        solo.set_master_candidates(vec![0xA]);
        solo.open(0x42, loot_type::CORPSE, 0, vec![master_item(0, 117, 1)]);
        assert_eq!(
            solo.window.master_candidate(1, &alone),
            Some(0xA),
            "a raid of one"
        );
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
        loot.push_receive(&push(entry, count, from_npc, created));
        loot.receives.pop().expect("queued")
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
        loot.push_receive(&push(117, 1, false, false));
        loot.clear();
        assert!(loot.source().is_none());
        assert_eq!(loot.receives.len(), 1, "a receive line outlives the window");
        loot.clear_session();
        assert!(loot.receives.is_empty(), "disconnect drops receive lines");
    }

    /// The count after the link's `|r` takes the line's own colour.
    #[test]
    fn receive_line_carries_a_quality_colored_item_link() {
        // Common, single: `LOOT_ITEM_SELF`.
        assert_eq!(
            receive_line(&pending(2589, 1, false, false), "Linen Cloth", 1),
            "You receive loot: |cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r."
        );
        // Poor, stacked: `LOOT_ITEM_SELF_MULTIPLE`, with no space before `x2`.
        assert_eq!(
            receive_line(&pending(7092, 2, false, false), "Chipped Claw", 0),
            "You receive loot: |cff9d9d9d|Hitem:7092:0:0:0|h[Chipped Claw]|h|rx2."
        );
        // The (created, received) pair picks the verb, created winning over received.
        assert_eq!(
            receive_line(&pending(4306, 1, true, false), "Silk Cloth", 2),
            "You receive item: |cff1eff00|Hitem:4306:0:0:0|h[Silk Cloth]|h|r."
        );
        assert_eq!(
            receive_line(&pending(2320, 4, true, true), "Coarse Thread", 1),
            "You create: |cffffffff|Hitem:2320:0:0:0|h[Coarse Thread]|h|rx4."
        );
    }

    /// `|Hitem:id:0:randomPropertyId:suffixFactor|h`, `0x52adb0`'s argument order.
    #[test]
    fn receive_line_carries_the_wire_random_property_fields() {
        let mut loot = LootState::default();
        loot.push_receive(&ItemPushResult {
            random_property_id: 862,
            suffix_factor: 1234,
            ..push(15268, 1, false, false)
        });
        let r = loot.receives.pop().expect("queued");
        assert_eq!(
            receive_line(&r, "Bloodrazor", 3),
            "You receive loot: |cff0070dd|Hitem:15268:0:862:1234|h[Bloodrazor]|h|r."
        );
    }

    /// `0x5d8b00` joins the suffix by `ITEM_SUFFIX_TEMPLATE`, for the row (`0x4c2550`) and tooltip.
    #[test]
    fn a_rolled_drop_reads_its_suffix_in_the_row_the_link_and_the_lines() {
        use benilla_formats::{RandomProperty, RandomPropertyCatalog};
        // "of the Monkey" (row 584 of the shipped table): Agility +7, Stamina +7.
        let props = crate::items::RandomProperties(RandomPropertyCatalog::from_rows(
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
        assert_eq!(
            receive_line(&pending(1, 1, false, false), "Odd Thing", 9),
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

    #[test]
    fn money_formats_with_real_words_dropping_zero_denominations() {
        assert_eq!(format_money(0), "0 Copper");
        assert_eq!(format_money(4), "4 Copper");
        assert_eq!(format_money(25), "25 Copper");
        assert_eq!(format_money(10025), "1 Gold 25 Copper");
        assert_eq!(format_money(12_345), "1 Gold 23 Silver 45 Copper");
        assert_eq!(format_money(10_000), "1 Gold");
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

    #[test]
    fn coin_row_uses_real_words_and_the_ladders_icon() {
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let (grp, nm) = nobody();
        let mut loot = LootState::default();
        // A pure-copper drop: name reads "4 Copper", icon is the copper coin pile (_05).
        loot.open(0x42, loot_type::CORPSE, 4, vec![]);
        let snap = snapshot(
            &loot.window,
            &items,
            None,
            Ask::Event(&commands),
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .expect("open");
        assert_eq!(snap.rows.len(), 1, "coin row only");
        let coin = snap.rows[0].as_ref().expect("coin row present");
        assert!(coin.is_coin);
        assert_eq!(coin.name.as_deref(), Some("4 Copper"));
        assert_eq!(coin.texture.as_deref(), Some(coin_icon(4)));
    }

    #[test]
    fn snapshot_prepends_coin_and_resolves_items() {
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let (grp, nm) = nobody();
        let mut loot = LootState::default();
        assert!(snapshot(
            &loot.window,
            &items,
            None,
            Ask::Event(&commands),
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .is_none());
        loot.open(0x42, loot_type::CORPSE, 12_345, vec![item(0, 117, 3)]);
        let snap = snapshot(
            &loot.window,
            &items,
            None,
            Ask::Event(&commands),
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .expect("open");
        assert_eq!(snap.rows.len(), 2, "coin + one item");
        let coin = snap.rows[0].as_ref().expect("coin row present");
        assert!(coin.is_coin);
        assert_eq!(coin.name.as_deref(), Some("1 Gold 23 Silver 45 Copper"));
        assert_eq!(coin.texture.as_deref(), Some(coin_icon(12_345)));
        // The item's name is nil while its template is in flight; its quantity is present.
        let row = snap.rows[1].as_ref().expect("item row present");
        assert!(!row.is_coin);
        assert!(row.name.is_none());
        assert_eq!(row.quantity, 3);

        // Looting the coin turns row 1 into a gap; the item keeps its position.
        loot.clear_money();
        let snap = snapshot(
            &loot.window,
            &items,
            None,
            Ask::Event(&commands),
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .expect("still open");
        assert_eq!(snap.rows.len(), 2, "the layout keeps both slots");
        assert!(snap.rows[0].is_none(), "the looted coin slot is a gap");
        assert!(snap.rows[1].is_some(), "the item stays at position 2");

        loot.remove_slot(0);
        let snap = snapshot(
            &loot.window,
            &items,
            None,
            Ask::Event(&commands),
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .expect("still open");
        assert_eq!(snap.rows, vec![None, None]);
        assert!(snap.has_money, "the taken coin's cell is -1, not 0");
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

    /// `drain_loot` scheduled, with [`CORPSE`] streamed at health 0 and [`LIVE`] at 100.
    fn close_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut app, rx) = bare_app();
        app.add_systems(Update, drain_loot);
        for (guid, health) in [(CORPSE, 0), (LIVE, 100)] {
            stream(&mut app, guid, EntityKind::Unit, &[(F_UNIT_HEALTH, health)]);
        }
        // The disenchanted item, which the object manager holds while its window is open.
        stream(&mut app, ESSENCE_SOURCE, EntityKind::Other, &[(2, 0x3)]);
        (app, rx)
    }

    /// The deselects asked for since the last call.
    fn deselects(app: &mut App) -> Vec<u64> {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>()
            .drain()
            .map(|d| d.0)
            .collect()
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
            app.world_mut().resource_mut::<LootMoveStart>().0 = true;
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
    /// move-start close keeps both. Owed beside a move-start close it decides, as the two commute.
    #[test]
    fn a_close_at_rest_takes_an_items_loot_and_sweeps_a_disenchant_hold() {
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;
        let (mut app, rx) = close_app();
        let step = |app: &mut App, move_start: bool| {
            app.world_mut().resource_mut::<LootCloseAtRest>().0 = true;
            app.world_mut().resource_mut::<LootMoveStart>().0 = move_start;
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
                    .take_item_unlocks(),
                vec![(LOCKBOX, true), (LOCKBOX, true)],
                "the close's unlock and the teardown's"
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
        assert!(
            loot.sweep_owed,
            "and owes its sweep to the feed, with no VM here"
        );
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

    /// The feed and the sweep's loot leg over an empty world, the feed's other inputs at rest.
    fn sweep_loot_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        use crate::ui_script::SweepApp;
        let (mut app, rx) = bare_app();
        app.add_sweep_leg(SweepLeg::Loot, close_loot_on_leave_world)
            .add_systems(Update, feed_loot);
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

    /// The response's tail closes a window that opened on the run after the copier's open and
    /// sweep (`0x5ebc3a`-`0x5ebc51`), so `LOOT_OPENED`'s handlers read the open window, its coin
    /// row included, and `LOOT_CLOSED`'s the closed one.
    #[test]
    fn a_window_opened_on_the_run_is_open_to_its_loot_opened_handlers() {
        let (mut app, _rx) = sweep_loot_app();
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("LOOT_OPENED")
                f:RegisterEvent("LOOT_CLOSED")
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, event .. ":" .. GetNumLootItems())
                end)"#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let world = app.world_mut();
        world
            .resource_mut::<LootState>()
            .open(CORPSE, 1, 5, Vec::new());
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        world.resource_mut::<LootMoveStart>().0 = true;
        app.update();
        let seen: String = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return table.concat(SEEN, ',')")
            .unwrap();
        assert_eq!(seen, "LOOT_OPENED:1,LOOT_CLOSED:0");
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

    /// A worldport sweeps the VM that stays: `LOOT_CLOSED` once, from the leg, and the feed adds
    /// none after it.
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

    /// Every close but the sweep's and the reload's (`CloseLoot`, the release response, the
    /// auto-release, the at-rest close) owes an item source's two unlocks (`0x48f2bd`, `0x4c1d36`)
    /// to `apply_item_releases` and leaves `LOOT_CLOSED` to the feed. Both unlocks come before the
    /// teardown signals it (`0x4c1dac`), so the releases run first and each `ITEM_LOCK_CHANGED`
    /// handler reads the window open. The VM pass holds the order by registration
    /// (`crate::vm_pass`), so a move that flips it fails here.
    #[test]
    fn the_owed_unlocks_run_before_the_feeds_loot_closed() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        assert!(crate::test_support::runs_before(
            &mut app,
            crate::ui_items::locks::apply_item_releases,
            feed_loot,
        ));
    }

    /// The sweep's loot leg on a lockbox: `CloseInteraction`'s unlock (`0x48f2bd`) and the
    /// teardown's (`0x4c1d36`) come before the teardown zeroes the source (`0x4c1cb0`) and signals
    /// `LOOT_CLOSED` (`0x4c1dac`), so each unlock's `ITEM_LOCK_CHANGED` handlers read the window
    /// still open, and `LOOT_CLOSED`'s the closed one.
    #[test]
    fn a_closing_lockboxs_unlock_handlers_read_the_window_open() {
        use crate::ui_script::sweep_test::sweep_in_place;
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;
        let (mut app, _rx) = sweep_loot_app();
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("LOOT_OPENED")
                f:RegisterEvent("ITEM_LOCK_CHANGED")
                f:RegisterEvent("LOOT_CLOSED")
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, event .. ":" .. GetNumLootItems())
                end)"#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        // OBJECT_FIELD_TYPE (2): TYPEMASK_OBJECT | TYPEMASK_ITEM.
        let e = app
            .world_mut()
            .spawn(crate::net::ObjectStore(ObjectFields::from_pairs(&[(
                2, 0x3,
            )])))
            .id();
        let world = app.world_mut();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(LOCKBOX, e);
        world
            .resource_mut::<Items>()
            .insert_template(117, Some(crate::items::test_template("Linen Cloth")));
        world.resource_mut::<LootState>().open(
            LOCKBOX,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        world.resource_mut::<LootLatch>().0 = Some(LOCKBOX);
        app.update();
        sweep_in_place(&mut app);
        let seen: String = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return table.concat(SEEN, ',')")
            .unwrap();
        assert_eq!(
            seen,
            "LOOT_OPENED:1,ITEM_LOCK_CHANGED:1,ITEM_LOCK_CHANGED:1,LOOT_CLOSED:0"
        );
    }

    /// `OnItemPush` signals `ITEM_PUSH` in the push's handler (`0x491be8`), which touches no loot
    /// window, so its handlers read the window as it stood before the frame's loot is pushed.
    #[test]
    fn an_item_pushs_handlers_read_the_loot_before_the_frames_push() {
        let (mut app, _rx) = sweep_loot_app();
        app.insert_resource(ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(std::collections::HashMap::from([
                (
                    5,
                    benilla_formats::ItemDisplay {
                        icon: Some("ITEM".into()),
                        ..Default::default()
                    },
                ),
            ])),
        ));
        let mut cloth = crate::items::test_template("Linen Cloth");
        cloth.display_info_id = 5;
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(2589, Some(cloth));
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"SEEN = ""
                local f = CreateFrame("Frame")
                f:RegisterEvent("ITEM_PUSH")
                f:SetScript("OnEvent", function() SEEN = SEEN .. GetNumLootItems() end)"#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let mut loot = app.world_mut().resource_mut::<LootState>();
        loot.push_receive(&push(2589, 1, false, false));
        loot.open(CORPSE, loot_type::CORPSE, 0, vec![item(0, 2589, 1)]);
        app.update();
        let s = app.world().non_send_resource::<UiScript>();
        assert_eq!(s.eval::<String>("return SEEN").unwrap(), "0");
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 1);
    }

    /// A loot app whose two cloths have icons, and a VM noting `LOOT_OPENED`, each
    /// `LOOT_SLOT_CLEARED` row and, at each `ITEM_PUSH`, which of rows 1 and 2 still hold an item.
    fn push_order_app() -> App {
        let (mut app, _rx) = sweep_loot_app();
        app.insert_resource(ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(std::collections::HashMap::from([
                (
                    5,
                    benilla_formats::ItemDisplay {
                        icon: Some("ITEM".into()),
                        ..Default::default()
                    },
                ),
            ])),
        ));
        for (entry, name) in [(LINEN, "Linen Cloth"), (WOOL, "Wool Cloth")] {
            let mut cloth = crate::items::test_template(name);
            cloth.display_info_id = 5;
            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(entry, Some(cloth));
        }
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("LOOT_OPENED")
                f:RegisterEvent("LOOT_SLOT_CLEARED")
                f:RegisterEvent("ITEM_PUSH")
                f:SetScript("OnEvent", function()
                    if event == "LOOT_OPENED" then
                        tinsert(SEEN, "OPENED")
                    elseif event == "LOOT_SLOT_CLEARED" then
                        tinsert(SEEN, "CLEARED" .. arg1)
                    else
                        local rows = ""
                        for i = 1, 2 do rows = rows .. ((GetLootSlotInfo(i)) and 1 or 0) end
                        tinsert(SEEN, "PUSH" .. rows)
                    end
                end)"#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    const LINEN: u32 = 2589;
    const WOOL: u32 = 2592;

    fn seen_order(app: &App) -> String {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<String>("return table.concat(SEEN, ',')")
            .unwrap()
    }

    /// Each packet's events fire inside its dispatch: `SMSG_LOOT_REMOVED` clears its row
    /// (`0x5ebeb0` → `0x4c1f00`) ahead of the `SMSG_ITEM_PUSH_RESULT` vmangos sends after it
    /// (`LootHandler.cpp:217`, `:230`), so each `ITEM_PUSH` reads the rows removed before it gone
    /// and the rows removed after it still there.
    #[test]
    fn each_item_push_reads_the_rows_removed_before_it() {
        let mut app = push_order_app();
        app.world_mut().resource_mut::<LootState>().open(
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, LINEN, 1), item(1, WOOL, 1)],
        );
        app.update();
        assert_eq!(seen_order(&app), "OPENED");
        let mut loot = app.world_mut().resource_mut::<LootState>();
        loot.remove_slot(0);
        loot.push_receive(&push(LINEN, 1, false, false));
        loot.remove_slot(1);
        loot.push_receive(&push(WOOL, 1, false, false));
        app.update();
        assert_eq!(seen_order(&app), "OPENED,CLEARED1,PUSH01,CLEARED2,PUSH00");
    }

    /// A push after the response in the same frame fires after the response's `LOOT_OPENED`
    /// (`0x4c2ac0`), so its handlers read the window open.
    #[test]
    fn an_item_push_after_the_response_reads_the_window_open() {
        let mut app = push_order_app();
        let mut loot = app.world_mut().resource_mut::<LootState>();
        loot.open(CORPSE, loot_type::CORPSE, 0, vec![item(0, LINEN, 1)]);
        loot.push_receive(&push(WOOL, 1, false, false));
        app.update();
        assert_eq!(seen_order(&app), "OPENED,PUSH10");
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
