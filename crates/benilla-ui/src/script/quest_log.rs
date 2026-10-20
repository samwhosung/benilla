//! The quest log (`QuestLog.cpp`, `0x4de390`-`0x4e1a80`): the player's twenty `PLAYER_QUEST_LOG`
//! slots and the quest records the app mirrors from its cache, the display list the rebuild
//! (`0x4de510`) makes of them, and the verbs the stock `QuestLogFrame.lua` calls. A verb reads
//! what it needs at the call, as the reference's do: a row's title off its quest's record, its
//! state and counters off its slot, an objective's name off the creature or item cache and its
//! count off the bags. A record a verb misses is asked for with a callback that fires
//! `QUEST_LOG_UPDATE` when it lands (`0x4e07d0`, `0x4e0830`); the reference holds one per call that
//! missed (`0x4e05c9`, `0x4e0b9c`; `0x55bbb3`), benilla one per record
//! ([`super::UiScript::flush_quest_log_refires`]).
//!
//! `QUEST_LOG_UPDATE` fires where the reference fires it: at the rebuild's tail (`0x4de811`), which
//! the quest log's field watcher (`0x4de4d0`), the world entry (`0x4de430`) and the landing of the
//! last record the list awaits (`0x4de8b0`) run, with the active player in the world (`0x4de568`);
//! at a fold (`0x4ded30`); when an item of ours that a listed quest needs is set up, restacked or
//! destroyed (`0x4df7b0`); at each `SMSG_QUERY_TIME_RESPONSE` (`0x4def1b`); when a faction's rank
//! moves (`0x4d58f3` into `0x4de52f`); from `GetQuestTimers` when a timer that ran out latched
//! (`0x4e1554`); and from the lookups' callbacks.
//!
//! A timed quest whose time ran out takes its row's expiry latch (`0x4df4f0`) unless it can be
//! turned in: when its row is made (`0x4de6ca`), when `GetQuestTimers` reads it (`0x4e14f1`) and at
//! `SMSG_QUESTUPDATE_FAILEDTIMER` (`0x5e5cd9`). The log then reads it as failed, and every rebuild
//! clears the latch with the rows (`0x4de5e2`), latching again what is still expired.
//!
//! The engine owns the header folds, as the reference's mask does. The selection lives in the
//! engine, as `QuestLog_SetSelection` sets and re-reads it in one call (`QuestLogFrame.lua:318`,
//! `:342`). The abandon mark and the share hold the quest id selected at the click, as the
//! reference's mark does (`0x4dfb50` copies the selection `0xbb7480`, a quest id, to `0xbb7484`),
//! so a re-index before the popup's Yes cannot retarget them.
//!
//! `IsUnitOnQuest` reads a unit's quest-log window off its snapshot ([`super::UnitState`]).
//!
//! Not built: `GetAbandonQuestItems` answers nil; a GameObject objective's name, which wears the
//! placeholder.

use std::collections::{HashMap, HashSet};

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{self, flag, number_arg};
use super::quest::{QuestItemView, QuestRewardSpell};
use super::Model;
use crate::strings::{fill, global, Arg};

/// The quest slots, `PLAYER_QUEST_LOG_1_1` on, 12 bytes each (`0x4de73e`).
pub const QUEST_LOG_SLOTS: usize = 20;

/// The slot state byte's complete bit (`test byte [slot+7],1`, `0x4e02a2`).
const SLOT_COMPLETE: u8 = 0x01;
/// Its failed bit (`test byte [slot+7],2`, `0x4dfa61`).
const SLOT_FAILED: u8 = 0x02;

/// A GameObject objective's id carries the top bit (`0x4e0315`).
const GO_OBJECTIVE_BIT: u32 = 0x8000_0000;

/// Deviation: an in-flight name reads `...`, where the reference's reads `" "` (`0x82ee00`),
/// because a blank looks like a nameless objective.
const NAME_PLACEHOLDER: &str = "...";

/// One `PLAYER_QUEST_LOG` slot as the descriptor holds it: the quest id, the four 6-bit counters
/// and the state byte of the second dword, and the deadline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuestSlotView {
    pub quest_id: u32,
    pub counters: [u8; 4],
    /// Bit 0 complete, bit 1 failed.
    pub state: u8,
    /// The deadline in unix seconds (`time(nullptr) + limitTime`, vmangos `Player::AddQuest`), 0
    /// when untimed; vmangos sets a failed quest's to 1 (`Player.cpp:13430`).
    pub timer: u32,
}

/// One of a record's four objective quads (`+0x149c`, `+0x14ac`, `+0x14bc`, `+0x14cc`, the text
/// at `+0x14dc + 0x100·i`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuestObjectiveView {
    /// A creature entry, or a GameObject's with [`GO_OBJECTIVE_BIT`]; 0 for none.
    pub creature_or_go: u32,
    pub required_count: u32,
    pub item_id: u32,
    pub item_count: u32,
    /// The override text, which names the creature line (`0x4e03da`).
    pub text: String,
}

/// A quest's record (`SMSG_QUEST_QUERY_RESPONSE`, the cache `0xc0e1b0`) with what the app resolves
/// off its tables: the header and tag names, the text's macros and the reward spell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestRecordView {
    pub title: String,
    /// `GetQuestLogTitle`'s return 2, which colors the stock row.
    pub level: u32,
    /// The template's `ZoneOrSort`, the header group (`[template+0xc]`, `0x4de6de`).
    pub zone_or_sort: i32,
    /// The group's name: `AreaTable` above 0, `QuestSort` below, the reference's literal for 0
    /// (`0x84b4ec`), empty for an id neither names.
    pub header: String,
    /// The bare tag word (`Elite`, `Dungeon`, `Raid`, …) from the template's `Type` through
    /// `QuestInfo.dbc`; the stock row adds the parentheses (`QuestLogFrame.lua:195`). `None`,
    /// never `Some("")`, for no tag: the row branches on its presence (`QuestLogFrame.lua:194`).
    pub tag: Option<String>,
    /// `GetQuestLogPushable`: the template's `QUEST_FLAGS_SHARABLE` (`0x8`).
    pub pushable: bool,
    pub objectives: [QuestObjectiveView; 4],
    /// The item the quest hands out (`+0x34`), never a leaderboard line (`0x4e04fa`).
    pub src_item_id: u32,
    /// The template's flags `+0x38 & 6`: the turn-in test needs the slot's complete bit
    /// (`0x4df65b`, `0x4df6bd`).
    pub needs_complete_bit: bool,
    /// `(faction, standing)`: the least standing the turn-in test needs (`+0x14`, `+0x18`, `jl`
    /// at `0x4df6d6`), faction 0 for none.
    pub rep_min: (u32, i32),
    /// [`Self::rep_min`]'s faction name, `Faction.dbc`'s at the locale (`[row+locale*4+0x4c]`,
    /// `0x4e0743`); `None` with no row, whose reputation line is counted but reads three nils
    /// (`0x4e053b`).
    pub rep_faction_name: Option<String>,
    /// `(faction, standing)`: the most standing it allows (`+0x1c`, `+0x20`, `jg` at `0x4df6eb`).
    pub rep_max: (u32, i32),
    /// The template's end text (`+0x129c`), as the wire sent it: when not empty, the event line.
    pub end_text: String,
    /// `GetQuestLogRequiredMoney` and `GetQuestLogRewardMoney`, in copper.
    pub required_money: u32,
    pub reward_money: u32,
    /// `(item id, count)`, the ids that are set, in order (`+0x3c`, `+0x4c`).
    pub rewards: Vec<(u32, u32)>,
    pub choices: Vec<(u32, u32)>,
    /// `GetQuestLogQuestText`'s two returns, the macros expanded.
    pub description: String,
    pub objectives_text: String,
    pub reward_spell: Option<QuestRewardSpell>,
}

/// One row of the display list `0xbb71c0`: a header, or a quest and its slot (`+0x4`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct QuestLogRow {
    /// The quest's id, 0 on a header ([`0x4df150`'s answer]); watches, marks and shares key on
    /// it, as indexes shift.
    pub(crate) quest_id: u32,
    pub(crate) slot: u8,
    pub(crate) is_header: bool,
    /// A header's name.
    pub(crate) header: String,
    /// A header's `ZoneOrSort`, which the reference's row keeps where a quest row keeps its id
    /// (`0x4de713`), so the by-id scans can stop on it (`0x4df4c2`, `0x4df562`).
    pub(crate) zone_or_sort: i32,
    /// A collapsed header, whose quests the fold leaves out of the visible rows.
    pub(crate) collapsed: bool,
}

/// The display list in its order after the fold: the visible rows, then the quests collapsed
/// headers fold.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct QuestLogState {
    pub(crate) entries: Vec<QuestLogRow>,
    /// `GetNumQuestLogEntries`'s return 2: every quest, folded ones included, for the
    /// "Quests: N/20" count (`QuestLogFrame.lua:290`).
    pub(crate) num_quests: u32,
    /// The quests folded under a collapsed header, the tail of `entries`. Their watches and
    /// abandon marks hold, as the reference's watch prune (`0x4de7a7` to `0x4de80f`) and abandon
    /// search (`0xbb71c0`) cover every row, folded ones included.
    pub(crate) hidden_quest_ids: Vec<u32>,
}

/// The live inputs the verbs read, and the lookups in flight.
#[derive(Clone, Debug, Default)]
pub(crate) struct QuestLogStore {
    pub(crate) slots: [QuestSlotView; QUEST_LOG_SLOTS],
    /// The quest cache's mirror, by quest id.
    pub(crate) records: HashMap<u32, QuestRecordView>,
    /// The records the list awaits (`[0xbb7498]`): while above 0, a rebuild returns at once.
    pending: u32,
    /// The rebuild's callbacks (`0x4de8b0`) per awaited quest, one per rebuild that missed it.
    record_waits: HashMap<u32, u32>,
    /// The quests the rebuild missed, for the app to ask.
    record_asks: Vec<u32>,
    /// The item and creature records a verb missed and that have not landed.
    item_waits: HashSet<u32>,
    creature_waits: HashSet<u32>,
    creature_asks: Vec<u32>,
    /// The creature cache (`0xc0e354`) as far as the leaderboard asked it.
    creature_names: HashMap<u32, String>,
    /// The held count of each item a log quest needs (`0x622130(inventory, item, 8)`).
    held: HashMap<u32, u32>,
    /// Missed records that landed, each one `QUEST_LOG_UPDATE` at the next flush.
    due: u32,
    /// Whether the active player is in the world: without one the rebuild returns before its
    /// signal (`0x4de568`-`0x4de572`).
    player: bool,
    /// The quests whose row holds the expiry latch (`[row+0xc]`, `0x4df50d`), cleared with the
    /// rows by every rebuild (`0x4de5e2`).
    latched: HashSet<u32>,
    /// The quest-giver refreshes (`0x5eb070`) the latch asked for since the app last drained.
    giver_refreshes: u32,
}

impl QuestLogStore {
    fn slot(&self, slot: u8) -> Option<&QuestSlotView> {
        self.slots.get(usize::from(slot))
    }

    /// An item record landed: a verb that missed it falls due (`0x4e0830` signals
    /// `QUEST_LOG_UPDATE` on a ready record, `0x4e083a`), fired by the next flush.
    pub(crate) fn item_landed(&mut self, item_id: u32) {
        self.due += u32::from(self.item_waits.remove(&item_id));
    }
}

/// `0x4de510(1)`: the display list afresh from the slots, unless a record it awaits is still in
/// flight (`0x4de545`). A slot whose record is cached gets a row and its group a header; one whose
/// record is not gets none, counts as awaited and asks with the callback `0x4de8b0`, once per
/// rebuild (`0x4de65f`-`0x4de679`). Groups and rows are ordered as `0x4de8f0` and `0x4deac0` order
/// them, the fold re-seats the list and the watches of quests that left go (`0x4de7a7`). Whether
/// it ran: the caller fires `QUEST_LOG_UPDATE`, the tail's signal (`0x4de811`).
fn rebuild(model: &mut Model) -> bool {
    // `0x4de5f0`: the offset as it stands, 0 before the sync.
    let now = countdown_now(model);
    let store = &mut model.quest_store;
    if store.pending != 0 || !store.player {
        return false;
    }
    store.latched.clear();
    let mut quests: Vec<(u8, u32, &QuestRecordView)> = Vec::new();
    for (slot, s) in store.slots.iter().enumerate() {
        // `cmp edi,ecx; jle` (`0x4de642`): an id that is not positive is no quest.
        if (s.quest_id as i32) <= 0 {
            continue;
        }
        match store.records.get(&s.quest_id) {
            Some(record) => quests.push((slot as u8, s.quest_id, record)),
            None => {
                store.pending += 1;
                *store.record_waits.entry(s.quest_id).or_default() += 1;
                store.record_asks.push(s.quest_id);
            }
        }
    }
    // Groups by `ZoneOrSort` in slot order, then sorted (`0x4de751`, comparator `0x4de8f0`): id 0
    // first (`0x4de913`), then by name through the case-insensitive collator `0x64a4c0`, an unnamed
    // id as `""`. Rows (`0x4deac0`) go by level (`0x4decd9`), then title (`0x4decfb`), never by
    // slot; the reference's `qsort` is unstable, so a full tie has no order there, and ties break
    // on slot here.
    let mut groups: Vec<(i32, &str, Vec<usize>)> = Vec::new();
    for (i, (_, _, r)) in quests.iter().enumerate() {
        match groups.iter_mut().find(|(z, _, _)| *z == r.zone_or_sort) {
            Some((_, _, rows)) => rows.push(i),
            None => groups.push((r.zone_or_sort, r.header.as_str(), vec![i])),
        }
    }
    groups.sort_by_cached_key(|&(zos, name, _)| {
        if zos == 0 {
            (0u8, String::new())
        } else {
            (1, name.to_lowercase())
        }
    });
    let mut rows = Vec::with_capacity(quests.len() + groups.len());
    for (zone_or_sort, name, mut members) in groups {
        members.sort_by_cached_key(|&i| {
            let (slot, _, r) = quests[i];
            (r.level, r.title.to_lowercase(), slot)
        });
        rows.push(QuestLogRow {
            is_header: true,
            header: name.to_string(),
            zone_or_sort,
            ..Default::default()
        });
        rows.extend(members.into_iter().map(|i| QuestLogRow {
            quest_id: quests[i].1,
            slot: quests[i].0,
            ..Default::default()
        }));
    }
    let num_quests = quests.len() as u32;
    let ids: HashSet<u32> = quests.iter().map(|&(_, id, _)| id).collect();
    // A timed quest whose time ran out latches as its row is made (`0x4de6a4`-`0x4de6ca`).
    let expired: Vec<(u32, u8)> = quests
        .iter()
        .map(|&(slot, id, _)| (id, slot))
        .filter(|&(_, slot)| {
            store
                .slot(slot)
                .and_then(|s| seconds_left(s, now))
                .is_some_and(|left| left < 0)
        })
        .collect();
    model.quest_log_watched.retain(|id| ids.contains(id));
    model.quest_log.num_quests = num_quests;
    model.quest_log_rows = rows;
    for (id, slot) in expired {
        latch_expiry(model, id, slot);
    }
    fold(model);
    true
}

/// `0x4df4f0(row)`: unless the turn-in test passes, which a latched row fails (`0x4df64e`), the
/// row's latch is set (`0x4df50d`) and, with a player, the quest givers refreshed (`0x4df531`).
/// Whether it latched.
fn latch_expiry(model: &mut Model, quest_id: u32, slot: u8) -> bool {
    let store = &model.quest_store;
    let passes = !store.latched.contains(&quest_id)
        && match (store.records.get(&quest_id), store.slot(slot)) {
            (Some(record), Some(slot)) => turn_in_passes(model, record, slot),
            _ => false,
        };
    if passes {
        owe_turn_in_tutorial(model);
        return false;
    }
    let store = &mut model.quest_store;
    store.latched.insert(quest_id);
    if store.player {
        store.giver_refreshes += 1;
    }
    true
}

impl QuestLogStore {
    /// `0x4df480` on the quest's own row: whether it holds the latch.
    fn is_latched(&self, quest_id: u32) -> bool {
        self.latched.contains(&quest_id)
    }
}

/// The by-id scans of `0x4df4b0` and `0x4df550` (`0x4df4c2`, `0x4df562`): the first row whose
/// first dword is `id`, which on a header is its zone id (`0x4de713`), so a header whose zone id
/// equals a quest's id can stand before that quest's row and answer for it.
fn first_row_for(model: &Model, id: u32) -> Option<&QuestLogRow> {
    model.quest_log.entries.iter().find(|e| {
        if e.is_header {
            e.zone_or_sort as u32 == id
        } else {
            e.quest_id == id
        }
    })
}

/// The rows off every row and the collapse set, as the rebuild `0x4dea50` orders them: the
/// visible rows, then the quests a collapsed header folds, sorted past them, so
/// `GetNumQuestLogEntries` counts the visible ones (`[0xbb7490]`) while an index reaches every row
/// (`[0xbb7478]`, `0x4df99c`). The selection is the reference's quest id (`0xbb7480`), answered as
/// that quest's index wherever it now sits (`0x4df010`), 0 once it left the log; a header is never
/// a selection (`QuestLog_GetFirstSelectableQuest`).
pub(crate) fn fold(model: &mut Model) {
    let selected = (model.quest_log_selection as usize)
        .checked_sub(1)
        .and_then(|i| model.quest_log.entries.get(i))
        .filter(|e| !e.is_header)
        .map(|e| e.quest_id)
        .or_else(|| model.quest_log_selection_seed.take());
    let mut entries = Vec::with_capacity(model.quest_log_rows.len());
    let mut folded_rows = Vec::new();
    let mut folded = false;
    for row in &model.quest_log_rows {
        if row.is_header {
            folded = model.quest_log_collapsed.contains(&row.header);
            entries.push(QuestLogRow {
                collapsed: folded,
                ..row.clone()
            });
        } else if folded {
            folded_rows.push(row.clone());
        } else {
            entries.push(row.clone());
        }
    }
    model.quest_log.hidden_quest_ids = folded_rows.iter().map(|e| e.quest_id).collect();
    entries.extend(folded_rows);
    model.quest_log_selection = selected
        .and_then(|id| {
            entries
                .iter()
                .position(|e| !e.is_header && e.quest_id == id)
        })
        .map_or(0, |i| i as u32 + 1);
    model.quest_log.entries = entries;
}

/// Tutorial 0x21, "Quest Completion": a turn-in test that passes triggers it (`0x4df78d`-`0x4df792`
/// into `0x4b5390`) from either caller, the latch (`0x4df4fc`) or `GetQuestLogTitle`
/// (`0x4dfa8d`). The fire-once bank is the app's, which drains the ask.
fn owe_turn_in_tutorial(model: &mut Model) {
    const QUEST_COMPLETION: u32 = 0x21;
    if !model.tutorial_triggers.contains(&QUEST_COMPLETION) {
        model.tutorial_triggers.push(QUEST_COMPLETION);
    }
}

/// The turn-in test `0x4df580` on a quest row's record and slot: no player (`0x4df5e1`) or failed
/// (`0x4df642`) fails; a template whose flags `& 6` need the slot's complete bit (`0x4df6bd`); one
/// with neither those flags nor a reputation id nor any creature or item id is never complete
/// (`0x4df663`-`0x4df6b0`). Then the least and most standing (`0x4df6d6`, `0x4df6eb`), the coinage
/// a negative money asks (`0x4df72e`), each creature or object count (`0x4df74a`) and the held
/// count of each item id, the quest's own item included (`0x4df780`). The expiry latch
/// (`0x4df64e`) its callers read first.
fn turn_in_passes(model: &Model, record: &QuestRecordView, slot: &QuestSlotView) -> bool {
    if !model.quest_store.player || slot.state & SLOT_FAILED != 0 {
        return false;
    }
    let any_objective = record
        .objectives
        .iter()
        .any(|o| o.creature_or_go != 0 || o.item_id != 0);
    if !record.needs_complete_bit
        && record.rep_min.0 == 0
        && record.rep_max.0 == 0
        && !any_objective
    {
        return false;
    }
    if record.needs_complete_bit && slot.state & SLOT_COMPLETE == 0 {
        return false;
    }
    // `0x4d6370`: the base plus the wire standing, the base 0 where no race/class column fits,
    // and 0 for a faction with no reputation index (`0x4d6377`).
    let standing = |faction: u32| model.faction_standings.get(&faction).copied().unwrap_or(0);
    if record.rep_min.0 != 0 && standing(record.rep_min.0) < record.rep_min.1 {
        return false;
    }
    if record.rep_max.0 != 0 && standing(record.rep_max.0) > record.rep_max.1 {
        return false;
    }
    if model.money < u64::from(record.required_money) {
        return false;
    }
    let store = &model.quest_store;
    record.objectives.iter().enumerate().all(|(i, o)| {
        (o.creature_or_go == 0 || u32::from(slot.counters[i]) >= o.required_count)
            && (o.item_id == 0
                || store.held.get(&o.item_id).copied().unwrap_or(0) as i64
                    >= i64::from(o.item_count as i32))
    })
}

/// The visible rows' count, `GetNumQuestLogEntries`'s first return (`[0xbb7490]`).
fn visible_rows(model: &Model) -> usize {
    model.quest_log.entries.len() - model.quest_log.hidden_quest_ids.len()
}

/// `CollapseQuestHeader`/`ExpandQuestHeader` (`0x4e1730`, `0x4e1780` → `0x4ded30`): the header at
/// the 1-based visible index folds or opens; an index that is no header, 0 or past the rows
/// folds or opens every header (`0x4dedbf`). Either re-folds the list (`0x4dea50`) and signals
/// `QUEST_LOG_UPDATE` before the verb returns.
fn set_collapsed(model: &mut Model, index: u32, collapse: bool) {
    let header = (index as usize)
        .checked_sub(1)
        .and_then(|i| model.quest_log.entries.get(i))
        .filter(|e| e.is_header)
        .map(|e| e.header.clone());
    match (header, collapse) {
        (Some(title), true) => {
            model.quest_log_collapsed.insert(title);
        }
        (Some(title), false) => {
            model.quest_log_collapsed.remove(&title);
        }
        (None, true) => {
            let all: Vec<String> = model
                .quest_log_rows
                .iter()
                .filter(|e| e.is_header)
                .map(|e| e.header.clone())
                .collect();
            model.quest_log_collapsed.extend(all);
        }
        (None, false) => model.quest_log_collapsed.clear(),
    }
    fold(model);
    model.raise("QUEST_LOG_UPDATE", Vec::new());
}

impl Model {
    /// The display row at the 1-based `index`, visible or folded.
    fn quest_row(&self, index: usize) -> Option<&QuestLogRow> {
        index
            .checked_sub(1)
            .and_then(|n| self.quest_log.entries.get(n))
    }

    /// A quest row's record and slot; `None` on a header.
    fn quest_row_parts(&self, index: usize) -> Option<(&QuestRecordView, &QuestSlotView)> {
        let row = self.quest_row(index).filter(|r| !r.is_header)?;
        let store = &self.quest_store;
        Some((store.records.get(&row.quest_id)?, store.slot(row.slot)?))
    }

    /// The selected quest's record, read per call as the reference's detail verbs read the
    /// selection `0xbb7480` and look the quest up (`0x4e1130` to `0xc0e1b0`, `0x562a40`), so a
    /// select-then-read answers about the row just selected.
    pub(crate) fn selected_quest_record(&self) -> Option<&QuestRecordView> {
        self.quest_row_parts(self.quest_log_selection as usize)
            .map(|(record, _)| record)
    }

    /// An item's record with a callback (`0x55ba30` with `0x4e0830`): the name, quality and icon,
    /// or, on a miss, the ask and the record held as awaited.
    fn quest_item_lookup(&mut self, item_id: u32) -> Option<(String, u32, Option<String>)> {
        // A zero key returns null before anything else (`0x55ba42`).
        if item_id == 0 {
            return None;
        }
        match self.item_templates.get(&item_id) {
            Some(t) => Some((t.name.clone(), t.quality, t.icon.clone())),
            None => {
                self.item_stat_asks.insert(item_id);
                self.quest_store.item_waits.insert(item_id);
                None
            }
        }
    }

    /// A creature's name with the callback `0x4e07d0` (`0x4e0461`), or on a miss the ask, once,
    /// and the entry held as awaited.
    fn quest_creature_lookup(&mut self, entry: u32) -> Option<String> {
        let store = &mut self.quest_store;
        match store.creature_names.get(&entry) {
            Some(name) => Some(name.clone()),
            None => {
                if store.creature_waits.insert(entry) {
                    store.creature_asks.push(entry);
                }
                None
            }
        }
    }
}

/// One leaderboard line of a record, in `GetQuestLogLeaderBoard`'s order (`0x4e0110`): the event
/// line when the end text is not empty (`0x4e026c`-`0x4e0285`); a creature or GameObject pass over
/// the ids that are set, the array index counting the empty ones (`0x4e02d7`); an item pass over
/// the ids above 0 that are not the quest's own item (`0x4e04f4`-`0x4e04fd`), a quad carrying both
/// giving two lines; then the reputation line for a non-zero `+0x14` (`0x4e0510`).
/// `GetNumQuestLeaderBoards` counts the same lines (`0x4e009a`-`0x4e00e7`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Event,
    Creature(usize),
    Item(usize),
    Reputation,
}

fn lines(record: &QuestRecordView) -> impl Iterator<Item = Line> + '_ {
    let event = (!record.end_text.is_empty()).then_some(Line::Event);
    let creatures = record
        .objectives
        .iter()
        .enumerate()
        .filter(|(_, o)| o.creature_or_go != 0)
        .map(|(i, _)| Line::Creature(i));
    let items = record
        .objectives
        .iter()
        .enumerate()
        .filter(|(_, o)| (o.item_id as i32) > 0 && o.item_id != record.src_item_id)
        .map(|(i, _)| Line::Item(i));
    let reputation = (record.rep_min.0 != 0).then_some(Line::Reputation);
    event
        .into_iter()
        .chain(creatures)
        .chain(items)
        .chain(reputation)
}

/// The line `GetQuestLogLeaderBoard` answers for the 1-based `line`. The event, creature and item
/// lines each match their own index alone (`cmp eax,ecx` at `0x4e0285`, `cmp ecx,eax` at `0x4e02dd` and `0x4e0503`);
/// the reputation tail compares no index (`0x4e0510`), so it answers any index the others leave,
/// 0 and past the count included.
fn line_at(record: &QuestRecordView, line: usize) -> Option<Line> {
    line.checked_sub(1)
        .and_then(|n| lines(record).filter(|l| *l != Line::Reputation).nth(n))
        .or_else(|| (record.rep_min.0 != 0).then_some(Line::Reputation))
}

/// `0x4d63a0`'s rank of a standing, 0 Hated to 7 Exalted, which the reputation line also applies
/// to its required value inline (`0x4e0544`-`0x4e06a5`).
fn standing_rank(standing: i32) -> u8 {
    match standing {
        42000.. => 7,
        21000.. => 6,
        9000.. => 5,
        3000.. => 4,
        0.. => 3,
        -3000.. => 2,
        -6000.. => 1,
        _ => 0,
    }
}

/// What a line shows before its strings are read off the VM.
enum LineText {
    /// The text as it is, under no format key (`0x4e0289`).
    Raw(String),
    /// `key` filled with the name and the two counts.
    Count {
        key: &'static str,
        name: String,
        cur: u32,
        req: u32,
    },
    /// `QUEST_FACTION_NEEDED` filled with the faction's name, then the current and the required
    /// rank's label (`0x4e0732`-`0x4e0764`), each gendered for the player (`0x612bf0`).
    Faction {
        name: String,
        current: u8,
        required: u8,
        female: bool,
    },
}

struct LineParts {
    text: LineText,
    kind: &'static str,
    finished: bool,
}

impl LineParts {
    /// A counted line, finished when the count reaches the need (`jl 0x4e07bc` at `0x4e078d`).
    fn count(key: &'static str, name: String, cur: u32, req: u32, kind: &'static str) -> Self {
        Self {
            text: LineText::Count {
                key,
                name,
                cur,
                req,
            },
            kind,
            finished: cur >= req,
        }
    }
}

/// Line `line` (1-based) of row `index`, read at the call. The event line is the end text,
/// finished by the slot's complete bit alone (`test byte [slot+7],1`, `0x4e02a2`). A creature line
/// takes the slot's 6-bit counter (`0x4e0302`); its override text is the name under
/// `QUEST_OBJECTS_FOUND` (`0x4e03e1`), else a GameObject's key is that one too (`0x4e031e`) and a
/// creature's `QUEST_MONSTERS_KILLED` with its name looked up (`0x4e0461`). An item line holds the
/// count in the bags capped at the need (`0x4e0579`-`0x4e059c`) and its name looked up at every
/// call (`0x4e05e0`). The reputation line is finished when the player's standing reaches `+0x18`
/// (`cmp edi,[esi+0x18]`, `0x4e0787`).
fn line_parts(model: &mut Model, index: usize, line: usize) -> Option<LineParts> {
    let (is_creature, o, counter) = {
        let (record, slot) = model.quest_row_parts(index)?;
        let (is_creature, i) = match line_at(record, line)? {
            Line::Event => {
                return Some(LineParts {
                    text: LineText::Raw(record.end_text.clone()),
                    kind: "event",
                    finished: slot.state & SLOT_COMPLETE != 0,
                });
            }
            Line::Reputation => {
                let (faction, required) = record.rep_min;
                let name = record.rep_faction_name.clone()?;
                // `0x4d6370`, as the turn-in test reads it: 0 for a faction not pushed.
                let standing = model.faction_standings.get(&faction).copied().unwrap_or(0);
                // `0x612bf0` on the active player: its gender 1 takes the `_FEMALE` text.
                let female = model.unit("player").is_some_and(|u| u.sex == 3);
                return Some(LineParts {
                    text: LineText::Faction {
                        name,
                        current: standing_rank(standing),
                        required: standing_rank(required),
                        female,
                    },
                    kind: "reputation",
                    finished: standing >= required,
                });
            }
            Line::Creature(i) => (true, i),
            Line::Item(i) => (false, i),
        };
        let o = &record.objectives[i];
        // The override text only names a creature line.
        let text = is_creature && !o.text.is_empty();
        let o = QuestObjectiveView {
            text: if text { o.text.clone() } else { String::new() },
            ..*o
        };
        (is_creature, o, slot.counters[i])
    };
    if is_creature {
        let req = o.required_count;
        let cur = u32::from(counter);
        let is_go = o.creature_or_go & GO_OBJECTIVE_BIT != 0;
        let kind = if is_go { "object" } else { "monster" };
        if !o.text.is_empty() {
            return Some(LineParts::count(
                "QUEST_OBJECTS_FOUND",
                o.text,
                cur,
                req,
                kind,
            ));
        }
        if is_go {
            return Some(LineParts::count(
                "QUEST_OBJECTS_FOUND",
                NAME_PLACEHOLDER.into(),
                cur,
                req,
                kind,
            ));
        }
        let name = model
            .quest_creature_lookup(o.creature_or_go)
            .unwrap_or_else(|| NAME_PLACEHOLDER.into());
        Some(LineParts::count(
            "QUEST_MONSTERS_KILLED",
            name,
            cur,
            req,
            kind,
        ))
    } else {
        let (item_id, req) = (o.item_id, o.item_count);
        let held = model.quest_store.held.get(&item_id).copied().unwrap_or(0);
        let name = model
            .quest_item_lookup(item_id)
            .map_or_else(|| NAME_PLACEHOLDER.into(), |(name, _, _)| name);
        Some(LineParts::count(
            "QUEST_ITEMS_NEEDED",
            name,
            held.min(req),
            req,
            "item",
        ))
    }
}

/// One reward or choice of the selected quest as a [`QuestItemView`], the item record read
/// without a callback; `None` past the list or with no selection.
pub(crate) fn selected_quest_item(
    model: &Model,
    kind: &str,
    index: usize,
) -> Option<QuestItemView> {
    let record = model.selected_quest_record()?;
    let list = match kind {
        "choice" => &record.choices,
        "reward" => &record.rewards,
        _ => return None,
    };
    let &(item_id, count) = list.get(index.checked_sub(1)?)?;
    let template = model.item_templates.get(&item_id);
    Some(QuestItemView {
        name: template.map(|t| t.name.clone()),
        texture: template.and_then(|t| t.icon.clone()),
        count,
        quality: template.map_or(1, |t| t.quality),
        item_id,
        usable: true,
        link: template.map(|t| super::item_stats::item_link(item_id, &t.name, t.quality)),
    })
}

impl super::UiScript {
    /// The engine state the log keeps past its VM: the collapsed headers by title, the fold
    /// mask (`[0xbb748c]`), and the selected quest's id (`[0xbb7480]`). A `/reload` resets
    /// neither: only the game init clears the selection (`0x48f5bd` → `0x4de390`), and nothing
    /// but the verbs and the rebuild writes the mask.
    pub fn quest_log_carry(&self) -> (HashSet<String>, Option<u32>) {
        let model = self.model_ref();
        let selected = (model.quest_log_selection as usize)
            .checked_sub(1)
            .and_then(|i| model.quest_log.entries.get(i))
            .filter(|e| !e.is_header)
            .map(|e| e.quest_id);
        (model.quest_log_collapsed.clone(), selected)
    }

    /// Seat what [`Self::quest_log_carry`] read off a previous VM, before any file or event
    /// runs; the selection lands on its quest at the first rebuild.
    pub fn seed_quest_log_carry(&mut self, (collapsed, selected): (HashSet<String>, Option<u32>)) {
        let mut model = self.model_mut();
        model.quest_log_collapsed = collapsed;
        model.quest_log_selection_seed = selected;
    }

    /// The slots as the descriptor now holds them, with no rebuild: a VM's seat before the world
    /// entry's.
    pub fn set_quest_slots(&mut self, slots: [QuestSlotView; QUEST_LOG_SLOTS]) {
        self.model_mut().quest_store.slots = slots;
    }

    /// `0x4df4b0(quest)`: whether the quest's row holds the expiry latch, which the quest-update
    /// packets read as done (`0x5e5b6c`, `0x5e5c16`). A header found first answers no
    /// (`0x4df4dd`).
    pub fn quest_log_latched(&self, quest_id: u32) -> bool {
        let model = self.model_ref();
        first_row_for(&model, quest_id)
            .is_some_and(|row| !row.is_header && model.quest_store.is_latched(row.quest_id))
    }

    /// `0x4df550(quest)`, `SMSG_QUESTUPDATE_FAILEDTIMER`'s (`0x5e5cd9`): the first row the id
    /// finds latches unless it can be turned in. A header found first fails the turn-in test
    /// (`0x4df5b7`), so it takes the latch, which no reader sees (`0x4df493`), and the quest's own
    /// row does not. Whether it latched.
    pub fn quest_log_latch_quest(&mut self, quest_id: u32) -> bool {
        let mut model = self.model_mut();
        let Some((is_header, slot)) =
            first_row_for(&model, quest_id).map(|e| (e.is_header, e.slot))
        else {
            return false;
        };
        if is_header {
            // `0x4df50d`, then the givers' refresh with a player (`0x4df531`).
            let store = &mut model.quest_store;
            if store.player {
                store.giver_refreshes += 1;
            }
            return true;
        }
        latch_expiry(&mut model, quest_id, slot)
    }

    /// The quest-giver refreshes the latch asked for (`0x4df531`) since the last drain.
    pub fn take_quest_giver_refreshes(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().quest_store.giver_refreshes)
    }

    /// Whether the active player is in the world, which the rebuild needs (`0x4de568`).
    pub fn set_quest_log_player(&mut self, present: bool) {
        self.model_mut().quest_store.player = present;
    }

    /// `SMSG_QUERY_TIME_RESPONSE` (`0x4de400` into `0x4deef0`): the clock offset is set, then
    /// `QUEST_LOG_UPDATE` (`0x4def1b`), player or none.
    pub fn quest_log_clock_synced(&mut self) {
        self.fire_event("QUEST_LOG_UPDATE", Vec::new());
    }

    /// `SMSG_SET_FACTION_STANDING` moved a faction's rank (`0x4d58a9`): `0x4de510(0)` once for the
    /// packet (`0x4d58f3`), which signals `QUEST_LOG_UPDATE` alone (`0x4de52f`).
    pub fn quest_log_faction_rank_changed(&mut self) {
        self.fire_event("QUEST_LOG_UPDATE", Vec::new());
    }

    /// The quest log's field watcher `0x4de4d0`, which a values block runs once when any byte of
    /// the 0xf0 at the player section's `+0x28` changed (`0x4de43b`-`0x4de456`): the slots, the
    /// rebuild and its `QUEST_LOG_UPDATE`.
    pub fn quest_log_changed(&mut self, slots: [QuestSlotView; QUEST_LOG_SLOTS]) {
        let ran = {
            let mut model = self.model_mut();
            model.quest_store.slots = slots;
            rebuild(&mut model)
        };
        if ran {
            self.fire_event("QUEST_LOG_UPDATE", Vec::new());
        }
    }

    /// `0x4de430`, the world-enter cascade's call (`0x4909a1`): the awaited count zeroed
    /// (`0x4de492`), then the rebuild and its `QUEST_LOG_UPDATE`.
    pub(crate) fn quest_log_enter_world(&mut self) {
        let ran = {
            let mut model = self.model_mut();
            model.quest_store.pending = 0;
            rebuild(&mut model)
        };
        if ran {
            self.fire_event("QUEST_LOG_UPDATE", Vec::new());
        }
    }

    /// Quest records landed (`DBCache::Set`, `0x562c80`): all held first, then, landing by
    /// landing, each rebuild callback it carries runs (`0x4de8b0`): one awaited record fewer, and
    /// the last one rebuilds (`0x4de8d4`-`0x4de8e2`), as does one landing when none is awaited.
    pub fn land_quest_records(&mut self, records: Vec<(u32, QuestRecordView)>) {
        let waits: Vec<u32> = {
            let mut model = self.model_mut();
            let store = &mut model.quest_store;
            let ids: Vec<u32> = records.iter().map(|&(id, _)| id).collect();
            store.records.extend(records);
            ids.into_iter()
                .map(|id| store.record_waits.remove(&id).unwrap_or(0))
                .collect()
        };
        for _ in 0..waits.into_iter().sum::<u32>() {
            let ran = {
                let mut model = self.model_mut();
                let store = &mut model.quest_store;
                store.pending = store.pending.saturating_sub(1);
                store.pending == 0 && rebuild(&mut model)
            };
            if ran {
                self.fire_event("QUEST_LOG_UPDATE", Vec::new());
            }
        }
    }

    /// A record held again with nothing waiting on it: a new VM's seat, or text whose macros
    /// changed.
    pub fn seed_quest_record(&mut self, quest_id: u32, record: QuestRecordView) {
        self.model_mut()
            .quest_store
            .records
            .insert(quest_id, record);
    }

    /// The server refused a quest (an empty title, `0x555410` → `DBCache::Fail`): the record goes
    /// and its rebuild callbacks run with status 0, which only log (`0x4de8bc`) and leave the
    /// awaited count as it was, so the list holds until the next world entry.
    pub fn refuse_quest_record(&mut self, quest_id: u32) {
        let mut model = self.model_mut();
        model.quest_store.records.remove(&quest_id);
        model.quest_store.record_waits.remove(&quest_id);
    }

    /// The quests the rebuild missed since the last drain: the app asks each.
    pub fn take_quest_record_asks(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().quest_store.record_asks)
    }

    /// `0x4df7b0(entry)`, which an item of ours runs when its record is set up (`0x5d85d9`), its
    /// stack count changes (`0x5d9519`) or it is deactivated (`0x5d87b2`): the first row whose
    /// record needs the item among its four (`+0x14bc`, `0x4df818`) fires `QUEST_LOG_UPDATE`, once.
    pub fn quest_item_changed(&mut self, item_id: u32) {
        let needed = {
            let model = self.model_ref();
            let store = &model.quest_store;
            model.quest_log.entries.iter().any(|row| {
                !row.is_header
                    && store
                        .records
                        .get(&row.quest_id)
                        .is_some_and(|r| r.objectives.iter().any(|o| o.item_id == item_id))
            })
        };
        if needed {
            self.fire_event("QUEST_LOG_UPDATE", Vec::new());
        }
    }

    /// The held count of each item the log's quests need, as `0x622130(inventory, item, 8)`
    /// counts it, banked copies included.
    pub fn set_quest_item_counts(&mut self, counts: HashMap<u32, u32>) {
        self.model_mut().quest_store.held = counts;
    }

    /// The creature entries the leaderboard missed since the last drain: the app asks each.
    pub fn take_quest_creature_asks(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().quest_store.creature_asks)
    }

    /// A creature's name landed: held, and each callback the leaderboard left on it is due
    /// (`0x4e07d0` signals `QUEST_LOG_UPDATE` on a ready record, `0x4e07ea`).
    pub fn set_quest_creature_name(&mut self, entry: u32, name: String) {
        let mut model = self.model_mut();
        let store = &mut model.quest_store;
        store.creature_names.insert(entry, name);
        store.due += u32::from(store.creature_waits.remove(&entry));
    }

    /// The server refused a creature: its callbacks run with status 0 and only log (`0x4e07dc`).
    pub fn refuse_quest_creature(&mut self, entry: u32) {
        self.model_mut().quest_store.creature_waits.remove(&entry);
    }

    /// Fire one `QUEST_LOG_UPDATE` for each missed record that landed, after the frame's landings,
    /// so a handler that walks the log reads every one of them.
    // Deviation: one signal per missed record, where the reference runs every callback of every
    // miss as its record lands (creatures in `0x556e20` at `0x556f91`, items in `0x55bc70` via
    // `0x55bdb0` from `0x555140`; `0x4e07ea`, `0x4e083a`), 2^k - 1 updates for a walking handler
    // over k first-seen records.
    pub fn flush_quest_log_refires(&mut self) {
        let due = std::mem::take(&mut self.model_mut().quest_store.due);
        for _ in 0..due {
            self.fire_event("QUEST_LOG_UPDATE", Vec::new());
        }
    }

    /// `0x4df880(quest)`: the quest's 1-based place among the rows, headers counted, 0 when it has
    /// none; `QUEST_WATCH_UPDATE`'s argument.
    pub fn quest_log_index(&self, quest_id: u32) -> u32 {
        self.model_ref()
            .quest_log
            .entries
            .iter()
            .position(|e| !e.is_header && e.quest_id == quest_id)
            .map_or(0, |i| i as u32 + 1)
    }

    /// The watched quest ids, in watch order.
    pub fn quest_log_watched(&self) -> Vec<u32> {
        self.model_ref().quest_log_watched.clone()
    }

    /// The 1-based selection, 0 for none.
    pub fn quest_log_selection(&self) -> u32 {
        self.model_ref().quest_log_selection
    }

    /// Drain the abandons `AbandonQuest()` confirmed, as the log slots of their rows: the app
    /// sends each slot's `CMSG_QUESTLOG_REMOVE_QUEST`.
    pub fn take_quest_log_abandons(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.model_mut().quest_log_abandons)
    }

    /// Drain the shares `QuestLogPushQuest()` queued, as quest ids; the app sends one
    /// `CMSG_PUSHQUESTTOPARTY` each.
    pub fn take_quest_log_pushes(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().quest_log_pushes)
    }

    /// Drain the `ConfirmAcceptQuest()` calls as a count: the verb carries no quest id, so the app
    /// answers the confirm it holds.
    pub fn take_quest_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().quest_confirms)
    }

    /// Set the selection.
    pub fn set_quest_log_selection(&mut self, i: u32) {
        self.model_mut().quest_log_selection = i;
    }

    /// Push the server clock in unix seconds each frame: the app's `SMSG_QUERY_TIME_RESPONSE`
    /// sample, advanced monotonically. The reference keeps an offset `G = local - server` from the
    /// sync and computes `deadline + G - localNow()`, the same value. Deviation: a local clock
    /// step mid-session (an NTP correction) jumps the reference's countdown but not ours, because
    /// the step changes no time the quest has left.
    pub fn set_server_unix_time(&mut self, unix_secs: f64) {
        self.model_mut().server_unix_time = Some(unix_secs);
    }

    /// Whether the last sync found the local and server clocks equal to the whole second, which
    /// the reference stores as `G = 1` (`0x4def02`): its countdowns then read one second more.
    pub fn set_clock_sync_zero(&mut self, zero: bool) {
        self.model_mut().clock_sync_zero = zero;
    }

    /// Push the local clock in unix seconds each frame, `time(NULL)` (`0x429580`). Before the first
    /// sync the reference's offset `G` is 0 (`0x4de3d3`), so the log's rebuild and the timer
    /// verbs count a deadline down by this clock (`0x4de6b6`, `0x4e14b9`, `0x4e16c9`).
    pub fn set_local_unix_time(&mut self, unix_secs: f64) {
        self.model_mut().local_unix_time = Some(unix_secs);
    }
}

/// The clock a countdown reads: the server's once synced, the local one before (`G == 0`). A
/// sync that finds the two clocks equal to the second stores `G = 1`, not 0 (`0x4def00`,
/// `0x4def02`), so its countdowns read one second more.
fn countdown_now(model: &Model) -> Option<f64> {
    match model.server_unix_time {
        Some(server) if model.clock_sync_zero => Some(server - 1.0),
        Some(server) => Some(server),
        None => model.local_unix_time,
    }
}

/// The slot the selection's verbs read: the selected quest's (`0x4df050`), or slot 0 when no row
/// holds the selection, as `0x4df010`'s -1 reads (`0x4df063`).
fn selected_slot(model: &Model) -> Option<&QuestSlotView> {
    let slot = model
        .quest_row(model.quest_log_selection as usize)
        .filter(|r| !r.is_header)
        .map_or(0, |r| r.slot);
    model.quest_store.slot(slot)
}

/// Signed seconds left on a slot's timer, by the reference's formula at all four of its sites
/// (`0x4e0905`, `0x4e14b9`, `0x4e16c9`, `0x4de6b6`): `slot.timer + G - time(NULL) - 1` in whole
/// seconds, `G` folded into `now` ([`countdown_now`]: the local clock before the sync, when `G` is
/// 0), so a quest's last second reads 0. A negative value drops the row from the lists (`js` at
/// `0x4e14c5`) and clamps to 0 in `GetQuestLogTimeLeft`. `None` when there is nothing to show:
/// - an untimed slot;
/// - a failed one: vmangos sets its timer to 1 (`Player.cpp:13430`), and the reference tests the
///   fail bit `[slot+0x7] & 0x2` first;
/// - no clock pushed at all, a bare VM.
fn seconds_left(slot: &QuestSlotView, now: Option<f64>) -> Option<i64> {
    if slot.timer == 0 || slot.state & SLOT_FAILED != 0 {
        return None;
    }
    Some(i64::from(slot.timer) - now?.floor() as i64 - 1)
}

/// [`seconds_left`] of row `row` for the list verbs, which skip a header (`0x4e1460`) and drop an
/// expired row as the reference's `js` does.
fn live_seconds_left(model: &Model, row: &QuestLogRow) -> Option<i64> {
    if row.is_header {
        return None;
    }
    let slot = model.quest_store.slot(row.slot)?;
    seconds_left(slot, countdown_now(model)).filter(|&s| s >= 0)
}

/// Register the quest-log globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `numEntries` counts header rows, `numQuests` only quests (`QuestLogFrame.lua:108`).
    g.set(
        "GetNumQuestLogEntries",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok((
                visible_rows(&model) as i64,
                i64::from(model.quest_log.num_quests),
            ))
        })?,
    )?;

    // `title, level, tag, isHeader, isCollapsed, isComplete`, always six (`0x4df930`). The index
    // truncates as `_ftol` does, and a missing or non-number one raises `Usage:`; out of range it
    // answers `nil, 0, nil, nil, nil, nil`, and a header's level is 0 too. A quest's title, level
    // and tag are its record's. `isComplete` is -1 on the slot's failed bit (`0x4dfa61`) or the
    // row's expiry latch (`0x4dfa69`), else 1 when the turn-in test passes (`0x4df580`, at
    // `0x4dfa8d`), else nil.
    g.set(
        "GetQuestLogTitle",
        lua.create_function(|lua, i: Value| {
            let n = number_arg(lua, i, "Usage: GetQuestLogTitle(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(row) = usize::try_from(n).ok().and_then(|n| model.quest_row(n)) else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            // Returns 4 and 5 are 1 or nil, never booleans.
            let one = |b: bool| if b { Value::Integer(1) } else { Value::Nil };
            if row.is_header {
                return Ok(MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&row.header)?),
                    Value::Integer(0),
                    Value::Nil,
                    Value::Integer(1),
                    one(row.collapsed),
                    Value::Nil,
                ]));
            }
            let store = &model.quest_store;
            let (Some(record), Some(slot)) =
                (store.records.get(&row.quest_id), store.slot(row.slot))
            else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            let tag = match &record.tag {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            let failed = slot.state & SLOT_FAILED != 0 || store.is_latched(row.quest_id);
            let passed = !failed && turn_in_passes(&model, record, slot);
            let complete = if failed {
                Value::Integer(-1)
            } else if passed {
                Value::Integer(1)
            } else {
                Value::Nil
            };
            let out = MultiValue::from_vec(vec![
                Value::String(lua.create_string(&record.title)?),
                Value::Integer(i64::from(record.level)),
                tag,
                Value::Nil,
                Value::Nil,
                complete,
            ]);
            drop(model);
            if passed {
                owe_turn_in_tutorial(&mut lua.app_data_mut::<Model>().expect("model app_data"));
            }
            Ok(out)
        })?,
    )?;

    // SelectQuestLogEntry(i) (`0x4dfae0` -> `0x4def30`) sets the selected quest id (`0xbb7480`,
    // `0x4def5d`) only for an index inside the list (`0x4def40`, `0x4def48`) whose row is a quest
    // (`0x4def55`); any other call, a header's included, leaves the old selection.
    g.set(
        "SelectQuestLogEntry",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: SelectQuestLogEntry(index)` (`0x84b540`, `0x4dfb0e`).
            let i = binding_abi::index_arg32(lua, i, "Usage: SelectQuestLogEntry(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.quest_row(i as usize).is_some_and(|r| !r.is_header) {
                model.quest_log_selection = i;
            }
            Ok(())
        })?,
    )?;

    // The 1-based selection, 0 for none.
    g.set(
        "GetQuestLogSelection",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.quest_log_selection))
        })?,
    )?;

    g.set(
        "GetQuestLogQuestText",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (desc, obj) = model.selected_quest_record().map_or(("", ""), |r| {
                (r.description.as_str(), r.objectives_text.as_str())
            });
            Ok((
                Value::String(lua.create_string(desc)?),
                Value::String(lua.create_string(obj)?),
            ))
        })?,
    )?;

    // Any row's line count, the selection's by default: the watch tracker reads unselected quests
    // (`QuestLogFrame.lua:616`).
    g.set(
        "GetNumQuestLeaderBoards",
        lua.create_function(|lua, i: Value| {
            // A non-number reads the selection (`0x4e0056` → `0x4e0073`); it never raises.
            let i = binding_abi::index_or(lua, i)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let index = i.unwrap_or(model.quest_log_selection) as usize;
            Ok(model
                .quest_row_parts(index)
                .map_or(0, |(record, _)| lines(record).count() as i64))
        })?,
    )?;

    // `text, type, finished` of line `i`, for any row as above (`QuestLogFrame.lua:635`), built at
    // the call ([`line_parts`]). Three values on every path (`mov eax,3` at `0x4e02ba`,
    // `0x4e079b`, `0x4e07c8`): `finished` is the number 1 (`0x4e02ae`, `0x4e078f`) or nil
    // (`0x4e07bc`). A row that does not exist, and a line no line takes on a quest with no
    // reputation line ([`line_at`]), take the tails at `0x4e07ac` and `0x4e07a9`, which push
    // three nils. A format key the install lacks gives an empty line, not no line.
    g.set(
        "GetQuestLogLeaderBoard",
        lua.create_function(|lua, (i, quest): (Value, Value)| {
            // `is-number` else `Usage: GetQuestLogLeaderBoard(index)` (`0x84b650`, `0x4e0141`); the
            // quest is the selection for a non-number (`0x4e01a2` → `0x4e01f6`).
            let i = binding_abi::index_arg(lua, i, "Usage: GetQuestLogLeaderBoard(index)")?;
            let quest = binding_abi::index_or(lua, quest)?;
            let parts = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let index = quest.unwrap_or(model.quest_log_selection) as usize;
                line_parts(&mut model, index, i)
            };
            let Some(p) = parts else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            // The VM's own `GlobalStrings.lua`, read through `0x703bf0`.
            let template = |key: &str| global(lua, key).unwrap_or_default();
            let text = match &p.text {
                LineText::Raw(text) => text.clone(),
                LineText::Count {
                    key,
                    name,
                    cur,
                    req,
                } => fill(
                    &template(key),
                    &[
                        Arg::S(name),
                        Arg::D(i64::from(*cur)),
                        Arg::D(i64::from(*req)),
                    ],
                ),
                LineText::Faction {
                    name,
                    current,
                    required,
                    female,
                } => {
                    // `FACTION_STANDING_LABEL<rank + 1>` (`0x84b5dc`) through `0x612bf0`.
                    let label = |rank: u8| {
                        crate::strings::standing_label(&|k| global(lua, k), rank.into(), *female)
                    };
                    fill(
                        &template("QUEST_FACTION_NEEDED"),
                        &[
                            Arg::S(name),
                            Arg::S(&label(*current)),
                            Arg::S(&label(*required)),
                        ],
                    )
                }
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&text)?),
                Value::String(lua.create_string(p.kind)?),
                flag(p.finished),
            ]))
        })?,
    )?;

    // ── Detail counts + money ────────────────────────────────────────────────────────────────────
    fn install_detail_count(
        lua: &Lua,
        name: &str,
        pick: fn(&QuestRecordView) -> i64,
    ) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(model.selected_quest_record().map_or(0, pick))
            })?,
        )
    }
    // The reference counts the reward ids to the first unset one (`0x4e0a34`-`0x4e0a40`), so
    // `[A, 0, B, 0]` counts 1, and `GetQuestLogRewardInfo` indexes the raw array (`0x4e0b98`);
    // the record holds the set ids compacted.
    // Deviation: four set ids count 4, where the loop's exit `jmp 0x4e0a4c` (`0x4e0a42`) skips
    // the store at `0x4e0a49` and answers 0, a client bug.
    install_detail_count(lua, "GetNumQuestLogRewards", |r| r.rewards.len() as i64)?;
    install_detail_count(lua, "GetNumQuestLogChoices", |r| r.choices.len() as i64)?;
    install_detail_count(lua, "GetQuestLogRewardMoney", |r| i64::from(r.reward_money))?;
    install_detail_count(lua, "GetQuestLogRequiredMoney", |r| {
        i64::from(r.required_money)
    })?;

    // `name, texture, numItems, quality, isUsable`, the giver panels' shape, as one stock routine
    // lays out both (`QuestFrameItems_Update`, `QuestFrame.lua:311`): five values, `isUsable` 1 or
    // nil (`0x4e0b00`, `0x4e0cf0`). The selected quest's record is read without a callback
    // (`0x4e0b75`), the item's with `0x4e0830` (`0x4e0b9e`, `0x4e0d8e`), whose landing fires
    // `QUEST_LOG_UPDATE`. No selected quest, an index past the list or an item in flight answers
    // `nil, nil, 1, 0, nil` (`0x4e0ca8`); a non-number index raises the reward getter's Usage line
    // from both (`0x84b6ac`).
    fn install_detail_item(lua: &Lua, name: &str, choices: bool) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, index: Value| {
                let i = lua
                    .coerce_number(index)?
                    .ok_or_else(|| mlua::Error::runtime("Usage: GetQuestLogRewardInfo(index)"))?;
                let row = i.trunc() as i64 - 1;
                let item = {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    let pair = model.selected_quest_record().and_then(|r| {
                        let list = if choices { &r.choices } else { &r.rewards };
                        usize::try_from(row).ok().and_then(|n| list.get(n)).copied()
                    });
                    pair.and_then(|(id, count)| {
                        model
                            .quest_item_lookup(id)
                            .map(|(name, quality, icon)| (name, quality, icon, count))
                    })
                };
                let Some((name, quality, icon, count)) = item else {
                    return Ok(MultiValue::from_vec(vec![
                        Value::Nil,
                        Value::Nil,
                        Value::Integer(1),
                        Value::Integer(0),
                        Value::Nil,
                    ]));
                };
                let texture = match &icon {
                    Some(t) => Value::String(lua.create_string(t)?),
                    None => Value::Nil,
                };
                Ok(MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&name)?),
                    texture,
                    Value::Integer(i64::from(count)),
                    Value::Integer(i64::from(quality)),
                    // Not computed: `0x5ea930`'s gate; the stock frame tints unusable ones red.
                    flag(true),
                ]))
            })?,
        )
    }
    install_detail_item(lua, "GetQuestLogChoiceInfo", true)?;
    install_detail_item(lua, "GetQuestLogRewardInfo", false)?;

    // The item's escaped link, for the reward rows' ctrl and shift clicks (`QuestLogFrame.lua:545`,
    // `:549`), keyed by `this.type`, "choice" or "reward" (set by `QuestFrameItems_Update`). The
    // item record is read without a callback (`0x4e0fdb`): an unknown type, an index out of range
    // or an item not yet held answers nil.
    g.set(
        "GetQuestLogItemLink",
        lua.create_function(|lua, (kind, index): (Value, Value)| {
            // `is-number` on arg 2, `is-number-or-string` on arg 1 else
            // `Usage: GetQuestLogItemLink("type", index)` (`0x84b6d0`, `0x4e1053`).
            let kind =
                binding_abi::string_arg(lua, kind, r#"Usage: GetQuestLogItemLink("type", index)"#)?;
            let index =
                binding_abi::index_arg(lua, index, r#"Usage: GetQuestLogItemLink("type", index)"#)?;
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                selected_quest_item(&model, &kind, index).and_then(|it| it.link)
            };
            match link {
                Some(l) => Ok(Value::String(lua.create_string(&l)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // 1 or nil, whether the selection's slot has failed or its row latched (`0x4e09a4`,
    // `0x4e09aa`); with no selection slot 0's bit decides and no row is latched (`0x4df480` on
    // -1). Nil with no player (`0x4e0983`). The stock title then appends " - (Failed)"
    // (`QuestLogFrame.lua:354`).
    g.set(
        "IsCurrentQuestFailed",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let selected = model
                .quest_row(model.quest_log_selection as usize)
                .filter(|r| !r.is_header);
            Ok(flag(
                model.quest_store.player
                    && (selected_slot(&model).is_some_and(|slot| slot.state & SLOT_FAILED != 0)
                        || selected.is_some_and(|r| model.quest_store.is_latched(r.quest_id))),
            ))
        })?,
    )?;

    // Header folds ([`set_collapsed`]); index 0 is every header, as the collapse-all button
    // passes (`QuestLogFrame.lua:557`, `:561`).
    g.set(
        "CollapseQuestHeader",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: CollapseQuestHeader(index)` (`0x84b720`, `0x4e1741`).
            let i = binding_abi::index_arg32(lua, i, "Usage: CollapseQuestHeader(index)")?;
            super::tick::verb(lua, |model| set_collapsed(model, i, true));
            Ok(())
        })?,
    )?;
    g.set(
        "ExpandQuestHeader",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: ExpandQuestHeader(index)` (`0x84b744`, `0x4e1791`).
            let i = binding_abi::index_arg32(lua, i, "Usage: ExpandQuestHeader(index)")?;
            super::tick::verb(lua, |model| set_collapsed(model, i, false));
            Ok(())
        })?,
    )?;

    // ── The abandon two-step (mark → confirm) ────────────────────────────────────────────────────
    // Marks the selected quest (`QuestLogFrame.xml:464`). The reference's `0x4dfb50` copies the
    // selection, which there is a quest id (`0x4def30` stores it at `0x4def5d`); ours is a row
    // index, so the id is resolved here, at the click, before the rows can move.
    g.set(
        "SetAbandonQuest",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.quest_log_abandon_mark = model
                .quest_row(model.quest_log_selection as usize)
                .filter(|e| !e.is_header)
                .map_or(0, |e| e.quest_id);
            Ok(())
        })?,
    )?;
    // The marked quest's title, "" for none, read off the quest cache by id (`0x4dfb60`).
    g.set(
        "GetAbandonQuestName",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mark = model.quest_log_abandon_mark;
            let title = (mark != 0)
                .then(|| model.quest_store.records.get(&mark))
                .flatten()
                .map_or("", |r| r.title.as_str());
            Ok(Value::String(lua.create_string(title)?))
        })?,
    )?;
    // Not built: nil, so the popup never lists the quest items an abandon destroys.
    g.set(
        "GetAbandonQuestItems",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;
    // The popup's Yes (`0x4dfe00` to `0x4df070`): the reference searches every row, folded ones
    // included (`0xbb71c0`), for the marked id; a hit sends its slot (`+0x4`) and clears the mark,
    // a miss keeps it.
    g.set(
        "AbandonQuest",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let mark = model.quest_log_abandon_mark;
            let slot = (mark != 0)
                .then(|| {
                    model
                        .quest_log
                        .entries
                        .iter()
                        .find(|e| !e.is_header && e.quest_id == mark)
                        .map(|e| e.slot)
                })
                .flatten();
            if let Some(slot) = slot {
                model.quest_log_abandon_mark = 0;
                model.quest_log_abandons.push(slot);
            }
            Ok(())
        })?,
    )?;

    // ── Timed quests ─────────────────────────────────────────────────────────────

    // One return per live timer, in log order, re-read every OnUpdate: `QuestTimerFrame_Update`
    // takes them as varargs and hides on none (`QuestTimerFrame.lua:11`, `:26`, `:37`). None
    // without a player (`0x4e1417`). A latched row is skipped (`0x4e14a7`); one whose time ran out
    // (`js`, `0x4e14c5`) latches unless it can be turned in (`0x4e14f1`) and prints
    // `ERR_QUEST_FAILED_S` with its title there and then (`0x4e1529`), and after the walk one
    // `QUEST_LOG_UPDATE` fires if any latched (`0x4e154d`-`0x4e1559`).
    g.set(
        "GetQuestTimers",
        lua.create_function(|lua, ()| {
            let rows: Vec<(u32, u8)> = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                if !model.quest_store.player {
                    return Ok(MultiValue::new());
                }
                model
                    .quest_log
                    .entries
                    .iter()
                    .filter(|e| !e.is_header)
                    .map(|e| (e.quest_id, e.slot))
                    .collect()
            };
            let mut left = Vec::new();
            let mut latched = false;
            for (quest_id, slot) in rows {
                let title = {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    if model.quest_store.is_latched(quest_id) {
                        continue;
                    }
                    let Some(secs) = model
                        .quest_store
                        .slot(slot)
                        .and_then(|s| seconds_left(s, countdown_now(&model)))
                    else {
                        continue;
                    };
                    if secs >= 0 {
                        left.push(Value::Integer(secs));
                        continue;
                    }
                    if !latch_expiry(&mut model, quest_id, slot) {
                        continue;
                    }
                    latched = true;
                    model
                        .quest_store
                        .records
                        .get(&quest_id)
                        .map(|r| r.title.clone())
                };
                // The line is shown inside the walk, before the signal (`0x4e1529`, `0x4e1559`).
                let template = lua
                    .globals()
                    .get::<String>("ERR_QUEST_FAILED_S")
                    .unwrap_or_default();
                if let Some(title) = title {
                    let text = fill(&template, &[Arg::S(&title)]);
                    if !text.is_empty() {
                        super::tick::show_chat_line_into(lua, "ERR_QUEST_FAILED_S", text);
                    }
                }
            }
            if latched {
                super::tick::fire_event_into(lua, "QUEST_LOG_UPDATE", Vec::new());
            }
            Ok(MultiValue::from_vec(left))
        })?,
    )?;

    // A timer's 1-based row, headers counted, for the timer buttons' click and tooltip
    // (`QuestTimerFrame.lua:43`, `QuestTimerFrame.xml:20`).
    g.set(
        "GetQuestIndexForTimer",
        lua.create_function(|lua, t: Value| {
            // `is-number` else `Usage: GetQuestIndexForTimer(index)` (`0x84b6fc`, `0x4e160c`).
            let t = binding_abi::index_arg(lua, t, "Usage: GetQuestIndexForTimer(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // Nil without a player (`0x4e165f`).
            let index = t
                .checked_sub(1)
                .filter(|_| model.quest_store.player)
                .and_then(|n| {
                    model
                        .quest_log
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| live_seconds_left(&model, e).is_some())
                        .nth(n)
                        .map(|(i, _)| i as i64 + 1)
                });
            Ok(match index {
                Some(i) => Value::Integer(i),
                None => Value::Nil,
            })
        })?,
    )?;

    // The selection's seconds left, slot 0's with no selection (`0x4e08ca`), nil without a timer,
    // which hides the stock "Time Remaining:" row (`QuestLogFrame.lua:364`). Nil too with no
    // player (`0x4e08c8`) and before the first time sync (`G == 0`, `0x4e08fc`), which the timer
    // list does not wait for.
    g.set(
        "GetQuestLogTimeLeft",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let synced = model.server_unix_time.is_some();
            let left = selected_slot(&model)
                .filter(|_| model.quest_store.player && synced)
                .and_then(|slot| seconds_left(slot, countdown_now(&model)));
            Ok(match left {
                // Clamped where the lists drop the row, as in the reference.
                Some(s) => Value::Integer(s.max(0)),
                None => Value::Nil,
            })
        })?,
    )?;

    // ── Reward spell and party ───────────────────────────────────────────────────────────────────
    // `texture, name, isTradeskillSpell` (`0x4e1130`), three nils without a reward spell.
    g.set(
        "GetQuestLogRewardSpell",
        lua.create_function(|lua, ()| {
            let spell = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .selected_quest_record()
                    .and_then(|r| r.reward_spell.clone())
            };
            super::quest::reward_spell_returns(lua, spell)
        })?,
    )?;
    // `IsUnitOnQuest(index, "unit")` (`0x4dfe10`): 1 when the unit's quest log holds row
    // `index`'s quest, else nil; the stock rows count `[n]` mates (`QuestLogFrame.lua:175`).
    g.set(
        "IsUnitOnQuest",
        lua.create_function(|lua, (index, token): (Value, Value)| {
            const USAGE: &str = "Usage: IsUnitOnQuest(index, \"unit\")";
            // Both are checked before either is read: `lua_isnumber` (`0x4dfe1e`), `lua_isstring`
            // (`0x4dfe32`); the index is `_ftol`'d (`0x4dfe4d`).
            let n = number_arg(lua, index, USAGE)?;
            let token = super::binding_abi::string_arg(lua, token, USAGE)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // The row's quest id (`0x4df150`): 0 for a header or a row past the list, which
            // answers nil before the token is resolved (`0x4dfe5e`).
            let quest_id = usize::try_from(n)
                .ok()
                .and_then(|n| model.quest_row(n))
                .filter(|e| !e.is_header)
                .map_or(0, |e| e.quest_id);
            if quest_id == 0 {
                return Ok(Value::Nil);
            }
            // The token's guid (`0x515970`), which raises on an unknown token.
            super::unit::check_unit_token(&Some(token.clone()))?;
            let Some(unit) = model.unit(&token) else {
                return Ok(Value::Nil);
            };
            // The active player or one of the four party slots, by guid (`0x4e7f70`): a raid mate
            // outside our subgroup, a pet or a stranger answers nil.
            let me = model.unit("player").map_or(0, |p| p.guid);
            let grouped = unit.guid != 0
                && (unit.guid == me || model.party.members.iter().any(|m| m.guid == unit.guid));
            // Held as a player (`0x468460`, typemask `0x10`) and not deactivated (`0x614ea0`, a
            // state the app's despawn never leaves held), then the 20 slots' ids, stride 12
            // (`0x4dfebd`-`0x4dfed7`); a non-player's window is all zero.
            Ok(flag(
                grouped && unit.has_object && unit.quest_log.iter().any(|s| s[0] == quest_id),
            ))
        })?,
    )?;
    // ── The party share and the quest watch ──────────────────────────────────────────────────────
    fn watch_id_at(model: &Model, index: u32) -> Option<u32> {
        model.quest_row(index as usize).map(|e| e.quest_id)
    }
    // `AddQuestWatch` (`0x4e1930`) and `RemoveQuestWatch` (`0x4e1970`): a `lua_isnumber` argument
    // (`0x4e1938`, `0x4e1978`) is a row, `_ftol`ed and looked up by `0x4df150`; a miss takes the
    // selected quest's id (`0xbb7480`, `0x4e195c`, `0x4e199c`), which `SelectQuestLogEntry` only
    // ever sets to a quest, never a header, so the selected row is one. Either way an id of 0
    // does nothing.
    fn watch_target(lua: &Lua, model: &Model, arg: Value) -> Option<u32> {
        match crate::script::binding_abi::is_number(lua, arg) {
            Some(n) => watch_id_at(model, crate::script::binding_abi::ftol(n) as u32),
            None => model
                .quest_row(model.quest_log_selection as usize)
                .filter(|r| !r.is_header)
                .map(|r| r.quest_id),
        }
    }
    // The share button enables on `GetQuestLogPushable() and GetNumPartyMembers() > 0`
    // (`QuestLogFrame.lua:301`) and clicks `QuestLogPushQuest()` (`QuestLogFrame.xml:512`), both
    // about the selection. `GetQuestLogPushable` answers 1 or nil, never false (`0x4e12b0`).
    g.set(
        "GetQuestLogPushable",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(
                model.selected_quest_record().is_some_and(|r| r.pushable),
            ))
        })?,
    )?;
    g.set(
        "QuestLogPushQuest",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // The verb checks the party and the sharable bit itself (`0x4e13a6`), as a macro can
            // call it solo. The id is resolved now; a header has no quest and never queues.
            if model.party.members.is_empty() {
                return Ok(());
            }
            let index = model.quest_log_selection as usize;
            let id = model
                .quest_row_parts(index)
                .filter(|(r, _)| r.pushable)
                .and_then(|_| model.quest_row(index))
                .map(|e| e.quest_id);
            if let Some(id) = id {
                model.quest_log_pushes.push(id);
            }
            Ok(())
        })?,
    )?;
    // The watch tracker's set (`QuestLogFrame.lua:469`, `:613`), keyed by quest id. At most
    // `MAX_WATCHABLE_QUESTS`, 5 (`QuestLogFrame.lua:8`), which the stock Lua checks before adding
    // (`:494`); an add past it is a no-op.
    g.set(
        "GetNumQuestWatches",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.quest_log_watched.len() as i64)
        })?,
    )?;
    g.set(
        "IsQuestWatched",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: IsQuestWatched(index)` (`0x84b764`, `0x4e18a1`).
            let i = binding_abi::index_arg32(lua, i, "Usage: IsQuestWatched(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(
                watch_id_at(&model, i).is_some_and(|id| model.quest_log_watched.contains(&id)),
            ))
        })?,
    )?;
    g.set(
        "AddQuestWatch",
        lua.create_function(|lua, i: Value| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `0x4dede0`: the id is appended while the list holds fewer than 5 (`0x4dede5`), with no
            // test for it already being there; the stock Lua checks `IsQuestWatched` itself.
            if let Some(id) = watch_target(lua, &model, i) {
                if model.quest_log_watched.len() < 5 {
                    model.quest_log_watched.push(id);
                }
            }
            Ok(())
        })?,
    )?;
    g.set(
        "RemoveQuestWatch",
        lua.create_function(|lua, i: Value| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `0x4dee00` removes the first entry equal to the id (`0x4dee10`) and closes the gap.
            if let Some(id) = watch_target(lua, &model, i) {
                if let Some(at) = model.quest_log_watched.iter().position(|w| *w == id) {
                    model.quest_log_watched.remove(at);
                }
            }
            Ok(())
        })?,
    )?;
    // The watched quest's current 1-based row, nil when it is not a visible row.
    g.set(
        "GetQuestIndexForWatch",
        lua.create_function(|lua, w: Value| {
            // `is-number` else `Usage: GetQuestIndexForWatch(index)` (`0x84b784`, `0x4e19c9`).
            let w = binding_abi::index_arg(lua, w, "Usage: GetQuestIndexForWatch(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let index = w
                .checked_sub(1)
                .and_then(|n| model.quest_log_watched.get(n))
                .and_then(|id| {
                    model
                        .quest_log
                        .entries
                        .iter()
                        .position(|e| e.quest_id == *id)
                })
                .map(|p| p as i64 + 1);
            Ok(match index {
                Some(i) => Value::Integer(i),
                None => Value::Nil,
            })
        })?,
    )?;

    Ok(())
}

/// Log fixtures for the engine's tests: records and slots seated as the app seats them.
#[cfg(test)]
pub(crate) mod fixture {
    use super::{QuestObjectiveView, QuestRecordView, QuestSlotView, QUEST_LOG_SLOTS};
    use crate::script::UiScript;

    /// A record of `title` and `level` under the header `header` of group `zone`.
    pub(crate) fn record(title: &str, level: u32, zone: i32, header: &str) -> QuestRecordView {
        QuestRecordView {
            title: title.into(),
            level,
            zone_or_sort: zone,
            header: header.into(),
            ..Default::default()
        }
    }

    /// A creature objective of `entry` needing `count`.
    pub(crate) fn kill(entry: u32, count: u32) -> QuestObjectiveView {
        QuestObjectiveView {
            creature_or_go: entry,
            required_count: count,
            ..Default::default()
        }
    }

    /// An item objective of `item` needing `count`.
    pub(crate) fn collect(item: u32, count: u32) -> QuestObjectiveView {
        QuestObjectiveView {
            item_id: item,
            item_count: count,
            ..Default::default()
        }
    }

    /// A slot holding `quest_id` with its first counter at `counter` and state `state`.
    pub(crate) fn slot(quest_id: u32, counter: u8, state: u8) -> QuestSlotView {
        QuestSlotView {
            quest_id,
            counters: [counter, 0, 0, 0],
            state,
            timer: 0,
        }
    }

    /// `quests` seated in slots 0 on, each record held, then the log's field change: the rebuild
    /// and its `QUEST_LOG_UPDATE`. The format keys the leaderboard reads are seated too.
    pub(crate) fn seat(s: &mut UiScript, quests: &[(u32, QuestRecordView, QuestSlotView)]) {
        s.run(
            "QUEST_MONSTERS_KILLED = '%s slain: %d/%d' \
             QUEST_OBJECTS_FOUND = '%s: %d/%d' \
             QUEST_ITEMS_NEEDED = '%s: %d/%d'",
        )
        .unwrap();
        let mut slots = [QuestSlotView::default(); QUEST_LOG_SLOTS];
        for (i, (id, record, slot)) in quests.iter().enumerate() {
            s.seed_quest_record(*id, record.clone());
            slots[i] = *slot;
        }
        s.set_quest_log_player(true);
        s.quest_log_changed(slots);
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{collect, kill, record, seat, slot};
    use super::{QuestRecordView, QuestSlotView, QUEST_LOG_SLOTS, SLOT_COMPLETE, SLOT_FAILED};
    use crate::script::tick::heard;
    use crate::script::{ItemTemplateView, UiScript};

    const VERMIN: u32 = 6;
    const WORKER: u32 = 257;
    const HAMMER: u32 = 2024;

    fn threat_within() -> QuestRecordView {
        let mut r = record("A Threat Within", 1, 12, "Elwynn Forest");
        r.objectives[0] = kill(VERMIN, 10);
        r.description = "Speak with Marshal McBride.".into();
        r.objectives_text = "Report to Marshal McBride.".into();
        r.reward_money = 40;
        r.rewards = vec![(HAMMER, 1)];
        r
    }

    fn camp_cleanup() -> QuestRecordView {
        let mut r = record("Kobold Camp Cleanup", 3, 12, "Elwynn Forest");
        r.objectives[0] = kill(WORKER, 10);
        r.description = "The kobolds have overrun the camp.".into();
        r.objectives_text = "Kill 10 Kobold Vermin.".into();
        r.reward_money = 250;
        r
    }

    /// The names and the reward the two quests read, held as the app holds them.
    fn caches(s: &mut UiScript) {
        s.set_quest_creature_name(VERMIN, "Kobold Vermin".into());
        s.set_quest_creature_name(WORKER, "Kobold Worker".into());
        s.set_item_template(
            HAMMER,
            ItemTemplateView {
                name: "Militia Hammer".into(),
                quality: 1,
                ..Default::default()
            },
        );
    }

    /// Elwynn Forest with quest 783 (row 2, slot 0, 3/10) and quest 7 (row 3, slot 1, complete).
    fn two_quests() -> UiScript {
        let mut s = UiScript::new().unwrap();
        caches(&mut s);
        seat(&mut s, &two_quests_log());
        s
    }

    fn two_quests_log() -> Vec<(u32, QuestRecordView, QuestSlotView)> {
        vec![
            (783, threat_within(), slot(783, 3, 0)),
            (7, camp_cleanup(), slot(7, 10, SLOT_COMPLETE)),
        ]
    }

    #[test]
    fn entries_and_title_tuple() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumQuestLogEntries()")
                .unwrap(),
            (0, 0)
        );
        assert!(s.eval::<bool>("return GetQuestLogTitle(1) == nil").unwrap());

        caches(&mut s);
        seat(&mut s, &two_quests_log());
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumQuestLogEntries()")
                .unwrap(),
            (3, 2)
        );
        assert!(s
            .eval::<bool>(
                "local t, l, tag, h, c, done = GetQuestLogTitle(1)\n\
                 return t == 'Elwynn Forest' and l == 0 and h == 1 and c == nil"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local t, l, tag, h, c, done = GetQuestLogTitle(2)\n\
                 return t == 'A Threat Within' and l == 1 and tag == nil\n\
                    and h == nil and c == nil and done == nil"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local t, _, _, _, _, done = GetQuestLogTitle(3)\n\
                 return t == 'Kobold Camp Cleanup' and done == 1"
            )
            .unwrap());
    }

    /// The shape of `0x4df930`.
    #[test]
    fn out_of_range_is_six_values_with_a_zero_level_and_a_bad_arg_raises() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetQuestLogTitle(1)").unwrap(), 6);
        assert!(s
            .eval::<bool>("local t, l = GetQuestLogTitle(1) return t == nil and l == 0")
            .unwrap());

        let s = two_quests();
        for i in ["0", "-1", "99"] {
            assert!(
                s.eval::<bool>(&format!(
                    "local t, l = GetQuestLogTitle({i}) return t == nil and l == 0"
                ))
                .unwrap(),
                "index {i} is out of range, not an error"
            );
        }
        // `_ftol` truncates toward zero: 2.9 is entry 2.
        assert!(s
            .eval::<bool>("return GetQuestLogTitle(2.9) == 'A Threat Within'")
            .unwrap());

        for bad in ["", "nil", "{}", "print"] {
            let e = format!(
                "{:?}",
                s.eval::<mlua::Value>(&format!("return GetQuestLogTitle({bad})"))
                    .unwrap_err()
            );
            assert!(
                e.contains("Usage: GetQuestLogTitle(index)"),
                "arg `{bad}` must raise Usage:, got {e}"
            );
        }
    }

    /// Two zones, each with one quest: `[A, 783, B, 7]`.
    fn two_zones_log() -> Vec<(u32, QuestRecordView, QuestSlotView)> {
        let mut log = two_quests_log();
        log[0].1.zone_or_sort = 1;
        log[0].1.header = "A".into();
        log[1].1.zone_or_sort = 2;
        log[1].1.header = "B".into();
        log
    }

    fn two_zones() -> UiScript {
        let mut s = UiScript::new().unwrap();
        seat(&mut s, &two_zones_log());
        s
    }

    /// `0x4ded30` folds in place: the list re-folds (`0x4dea50`) and `QUEST_LOG_UPDATE` fires
    /// before the verb returns. A quest's index, 0 and an index past the rows fold or open every
    /// header (`0x4dedbf`).
    #[test]
    fn a_fold_signals_quest_log_update_in_place() {
        let s = two_zones();
        heard::listen(&s, &["QUEST_LOG_UPDATE"], Some("GetNumQuestLogEntries()"));
        s.run("CollapseQuestHeader(1)").unwrap();
        assert_eq!(heard::take(&s), ["QUEST_LOG_UPDATE 3"]);
        s.run("ExpandQuestHeader(1)").unwrap();
        assert_eq!(heard::take(&s), ["QUEST_LOG_UPDATE 4"]);
        s.run("CollapseQuestHeader(2)").unwrap();
        assert_eq!(
            heard::take(&s),
            ["QUEST_LOG_UPDATE 2"],
            "a quest row folds all"
        );
        s.run("ExpandQuestHeader(0)").unwrap();
        assert_eq!(heard::take(&s), ["QUEST_LOG_UPDATE 4"]);
        s.run("CollapseQuestHeader(9)").unwrap();
        assert_eq!(
            heard::take(&s),
            ["QUEST_LOG_UPDATE 2"],
            "past the rows folds all"
        );
    }

    /// The selection follows its quest across a fold and a rebuild, the reference's quest id.
    #[test]
    fn the_selection_follows_its_quest_across_folds() {
        let mut s = two_zones();
        s.run("SelectQuestLogEntry(4)").unwrap(); // quest 7, under B
        s.run("CollapseQuestHeader(1)").unwrap();
        assert_eq!(
            s.quest_log_selection(),
            3,
            "A's fold moves quest 7 up a row"
        );
        // A third quest in a group that sorts first: `[0, 900, A, B, 7]`, 783 folded.
        let mut grown = two_zones_log();
        grown.push((900, record("Other", 1, 3, "0"), slot(900, 0, 0)));
        seat(&mut s, &grown);
        assert_eq!(s.quest_log_selection(), 5, "a group above pushes it down");
        s.run("CollapseQuestHeader(4)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumQuestLogEntries()").unwrap(), 4);
        assert_eq!(
            s.quest_log_selection(),
            6,
            "folded away, the quest keeps the selection at its row past the visible ones \
             (0x4df010), so QuestLog_Update picks no new one"
        );
        assert_eq!(
            s.eval::<String>("return (GetQuestLogTitle(6))").unwrap(),
            "Kobold Camp Cleanup",
            "an index reaches the folded rows (0x4df99c)"
        );
    }

    #[test]
    fn is_header_and_is_collapsed_are_one_or_nil_never_booleans() {
        let mut s = UiScript::new().unwrap();
        let mut log = two_quests_log();
        log[1].1.zone_or_sort = 40;
        log[1].1.header = "Westfall".into();
        seat(&mut s, &log);
        s.run("CollapseQuestHeader(1)").unwrap();

        assert!(s
            .eval::<bool>(
                "local _, l, _, h, c = GetQuestLogTitle(1) return h == 1 and c == 1 and l == 0"
            )
            .unwrap());
        assert!(s
            .eval::<bool>("local _, _, _, h, c = GetQuestLogTitle(2) return h == 1 and c == nil")
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local _, _, _, h, c = GetQuestLogTitle(3)\n\
                 return h == nil and c == nil and type(h) ~= 'boolean'"
            )
            .unwrap());
    }

    #[test]
    fn tag_is_the_bare_word_and_nil_when_absent() {
        let mut s = UiScript::new().unwrap();
        let mut log = two_quests_log();
        log[1].1.tag = Some("Elite".into());
        seat(&mut s, &log);
        assert!(s
            .eval::<bool>("local _, _, tag = GetQuestLogTitle(2) return tag == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("local _, _, tag = GetQuestLogTitle(3) return tag == 'Elite'")
            .unwrap());
    }

    #[test]
    fn selection_is_synchronous() {
        let s = two_quests();
        assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 0);
        assert_eq!(
            s.eval::<i64>("SelectQuestLogEntry(2); return GetQuestLogSelection()")
                .unwrap(),
            2
        );
        assert_eq!(s.quest_log_selection(), 2);
    }

    #[test]
    fn a_log_walk_answers_per_entry_without_a_push_between() {
        let s = two_quests();
        let walk: String = s
            .eval(
                "local out = ''\n\
                 for i = 1, GetNumQuestLogEntries() do\n\
                   SelectQuestLogEntry(i)\n\
                   local desc, obj = GetQuestLogQuestText()\n\
                   out = out .. i .. '=' .. desc .. '|'\n\
                 end\n\
                 return out",
            )
            .unwrap();
        assert_eq!(
            walk, "1=|2=Speak with Marshal McBride.|3=The kobolds have overrun the camp.|",
            "each entry must answer with its OWN detail inside a single frame"
        );
    }

    #[test]
    fn the_detail_counts_follow_the_selection_too() {
        let s = two_quests();
        assert_eq!(
            s.eval::<i64>("SelectQuestLogEntry(2); return GetQuestLogRewardMoney()")
                .unwrap(),
            40
        );
        assert_eq!(
            s.eval::<i64>("SelectQuestLogEntry(3); return GetQuestLogRewardMoney()")
                .unwrap(),
            250
        );
        assert_eq!(
            s.eval::<i64>("SelectQuestLogEntry(3); return GetNumQuestLogRewards()")
                .unwrap(),
            0
        );
    }

    #[test]
    fn a_header_selection_has_no_detail() {
        let s = two_quests();
        assert!(s
            .eval::<bool>(
                "SelectQuestLogEntry(1); local d = GetQuestLogQuestText(); return d == ''"
            )
            .unwrap());
    }

    #[test]
    fn detail_getters_read() {
        let s = two_quests();
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert!(s
            .eval::<bool>(
                "local d, o = GetQuestLogQuestText()\n\
                 return d == 'Speak with Marshal McBride.' and o == 'Report to Marshal McBride.'"
            )
            .unwrap());
        for (call, n) in [
            ("GetNumQuestLeaderBoards()", 1),
            ("GetNumQuestLeaderBoards(2)", 1),
            ("GetNumQuestLeaderBoards(3)", 1),
            ("GetNumQuestLeaderBoards(1)", 0),
        ] {
            assert_eq!(
                s.eval::<i64>(&format!("return {call}")).unwrap(),
                n,
                "{call}"
            );
        }
        assert!(s
            .eval::<bool>(
                "local t, k, f = GetQuestLogLeaderBoard(1, 3)\n\
                 return t == 'Kobold Worker slain: 10/10' and k == 'monster' and f == 1"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local t, k, f = GetQuestLogLeaderBoard(1)\n\
                 return t == 'Kobold Vermin slain: 3/10' and k == 'monster' and f == nil"
            )
            .unwrap());
        // Three values on every path; a row or line with nothing answers three nils (`0x4e07ac`).
        for call in [
            "GetQuestLogLeaderBoard(1)",
            "GetQuestLogLeaderBoard(1, 3)",
            "GetQuestLogLeaderBoard(2)",
            "GetQuestLogLeaderBoard(1, 9)",
            "GetQuestLogLeaderBoard(1, 1)",
            "GetQuestLogLeaderBoard(0)",
        ] {
            assert_eq!(s.arity(call).unwrap(), 3, "{call}");
        }
        for call in [
            "GetQuestLogLeaderBoard(2)",
            "GetQuestLogLeaderBoard(1, 9)",
            "GetQuestLogLeaderBoard(1, 1)",
            "GetQuestLogLeaderBoard(0)",
        ] {
            assert!(
                s.eval::<bool>(&format!(
                    "local a, b, c = {call} return a == nil and b == nil and c == nil"
                ))
                .unwrap(),
                "{call}"
            );
        }
        assert_eq!(
            s.eval::<i64>("return GetQuestLogRewardMoney()").unwrap(),
            40
        );
        assert_eq!(s.eval::<i64>("return GetNumQuestLogRewards()").unwrap(), 1);
        // Five values, `isUsable` 1 or nil (`0x4e0b00`).
        assert_eq!(s.arity("GetQuestLogRewardInfo(1)").unwrap(), 5);
        assert!(s
            .eval::<bool>(
                "local n, _, c, q, u = GetQuestLogRewardInfo(1)\n\
                 return n == 'Militia Hammer' and c == 1 and q == 1 and u == 1"
            )
            .unwrap());
        // No row answers the miss, `nil, nil, 1, 0, nil` (`0x4e0ca8`), from either getter.
        for call in [
            "GetQuestLogChoiceInfo(1)",
            "GetQuestLogRewardInfo(2)",
            "GetQuestLogRewardInfo(0)",
        ] {
            assert_eq!(s.arity(call).unwrap(), 5, "{call}");
            assert!(
                s.eval::<bool>(&format!(
                    "local n, t, c, q, u = {call}\n\
                     return n == nil and t == nil and c == 1 and q == 0 and u == nil"
                ))
                .unwrap(),
                "{call}"
            );
        }
        // A non-number index raises the reward getter's line from both (`0x84b6ac`).
        for call in ["GetQuestLogRewardInfo()", "GetQuestLogChoiceInfo({})"] {
            let e = s.run(call).expect_err(call).to_string();
            assert!(
                e.contains("Usage: GetQuestLogRewardInfo(index)"),
                "{call}: {e}"
            );
        }

        assert_eq!(
            s.eval::<String>("return GetQuestLogItemLink(\"reward\", 1)")
                .unwrap(),
            "|cffffffff|Hitem:2024:0:0:0|h[Militia Hammer]|h|r"
        );
        for call in [
            "GetQuestLogItemLink(\"choice\", 1)",
            "GetQuestLogItemLink(\"reward\", 9)",
            "GetQuestLogItemLink(\"bogus\", 1)",
        ] {
            assert!(
                s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
                "{call}"
            );
        }
    }

    /// One timed quest, row 2, its deadline 1000 and the clock at 1001.5: run out.
    fn expired(r: QuestRecordView, counter: u8) -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_server_unix_time(1001.5);
        let mut slot = slot(50, counter, 0);
        slot.timer = 1000;
        s.set_quest_log_player(true);
        // Seated by the field change after the clock, the rebuild latches it as it makes the row.
        s.seed_quest_record(50, r);
        let mut slots = [QuestSlotView::default(); QUEST_LOG_SLOTS];
        slots[0] = slot;
        s.set_quest_slots(slots);
        s
    }

    /// `GetQuestTimers`' expired branch (`0x4e14c5`): the row latches (`0x4df50d`), the givers
    /// refresh (`0x4df531`), the failure line reaches chat with the title inside the call
    /// (`0x4e1529`) and then one `QUEST_LOG_UPDATE` fires (`0x4e1559`); the log reads it failed
    /// (`0x4dfa69`, `0x4e09aa`), and a latched row is skipped after (`0x4e14a7`).
    #[test]
    fn a_timer_that_runs_out_latches_the_quest_failed() {
        let mut hunt = record("Hunt", 1, 12, "Elwynn Forest");
        hunt.objectives[0] = kill(40, 3);
        let mut s = expired(hunt, 0);
        // The row made while the deadline was ahead, then the deadline as it stands: the timers'
        // read is the one to find it run out.
        s.quest_log_changed({
            let mut slots = s.model_ref().quest_store.slots;
            slots[0].timer = 2000;
            slots
        });
        s.take_quest_giver_refreshes();
        let mut slots = s.model_ref().quest_store.slots;
        slots[0].timer = 1000;
        s.set_quest_slots(slots);
        s.run("ERR_QUEST_FAILED_S = \"%s failed.\"").unwrap();
        heard::listen(&s, &["CHAT_MSG_SYSTEM", "QUEST_LOG_UPDATE"], Some("arg1"));
        assert_eq!(s.eval::<i64>("return getn({GetQuestTimers()})").unwrap(), 0);
        assert_eq!(
            heard::take(&s),
            ["CHAT_MSG_SYSTEM Hunt failed.", "QUEST_LOG_UPDATE nil"]
        );
        assert_eq!(
            s.take_shown_chat_lines(),
            [("ERR_QUEST_FAILED_S", "Hunt failed.".to_string())]
        );
        assert_eq!(s.take_quest_giver_refreshes(), 1);
        assert_eq!(is_complete(&s), Some(-1));
        assert!(s.quest_log_latched(50));
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert!(s
            .eval::<bool>("return IsCurrentQuestFailed() == 1")
            .unwrap());
        s.eval::<i64>("return getn({GetQuestTimers()})").unwrap();
        assert!(heard::take(&s).is_empty(), "a latched row is skipped");
        assert!(s.take_shown_chat_lines().is_empty());
    }

    /// The latch is not taken by a quest that can be turned in (`0x4df4fc`), and every rebuild
    /// clears it with the rows (`0x4de5e2`), latching again a row still expired as it is made
    /// (`0x4de6ca`), with no line and no signal of its own.
    #[test]
    fn the_latch_spares_a_finished_quest_and_every_rebuild_remakes_it() {
        let mut hunt = record("Hunt", 1, 12, "Elwynn Forest");
        hunt.objectives[0] = kill(40, 3);
        let mut done = expired(hunt.clone(), 3);
        heard::listen(&done, &["QUEST_LOG_UPDATE"], None);
        let slots = done.model_ref().quest_store.slots;
        done.quest_log_changed(slots);
        assert_eq!(heard::take(&done).len(), 1, "the rebuild's own");
        assert!(!done.quest_log_latched(50));
        assert_eq!(is_complete(&done), Some(1));
        done.eval::<i64>("return getn({GetQuestTimers()})").unwrap();
        assert!(heard::take(&done).is_empty());
        assert!(!done.quest_log_latched(50));

        let mut s = expired(hunt, 0);
        heard::listen(&s, &["QUEST_LOG_UPDATE"], None);
        let mut slots = s.model_ref().quest_store.slots;
        s.quest_log_changed(slots);
        assert!(s.quest_log_latched(50), "latched as its row was made");
        assert_eq!(heard::take(&s).len(), 1, "the rebuild's signal alone");
        assert!(
            s.take_shown_chat_lines().is_empty(),
            "no line from the rebuild"
        );
        assert_eq!(s.take_quest_giver_refreshes(), 1);
        // The server moves the deadline: the next rebuild clears the latch.
        slots[0].timer = 5000;
        s.quest_log_changed(slots);
        assert!(!s.quest_log_latched(50));
        assert_eq!(is_complete(&s), None);
        // `SMSG_QUESTUPDATE_FAILEDTIMER`'s latch (`0x4df550`).
        assert!(s.quest_log_latch_quest(50));
        assert_eq!(is_complete(&s), Some(-1));
    }

    /// A turn-in test that passes asks for tutorial 0x21, Quest Completion (`0x4df792`), from
    /// `GetQuestLogTitle` (`0x4dfa8d`) and from the latch (`0x4df4fc`); one that fails asks
    /// nothing.
    #[test]
    fn a_passing_turn_in_test_asks_for_the_quest_completion_tutorial() {
        let mut hunt = record("Hunt", 1, 12, "Elwynn Forest");
        hunt.objectives[0] = kill(40, 3);
        let mut s = UiScript::new().unwrap();
        seat(&mut s, &[(50, hunt.clone(), slot(50, 2, 0))]);
        assert_eq!(is_complete(&s), None);
        assert!(s.take_tutorial_triggers().is_empty());
        let mut s = UiScript::new().unwrap();
        seat(&mut s, &[(50, hunt, slot(50, 3, 0))]);
        assert_eq!(is_complete(&s), Some(1));
        assert_eq!(is_complete(&s), Some(1));
        assert_eq!(s.take_tutorial_triggers(), [0x21], "one ask a drain");
        assert!(!s.quest_log_latch_quest(50), "the latch's test passes");
        assert_eq!(s.take_tutorial_triggers(), [0x21]);
    }

    /// One timed quest seated with its deadline at `timer`, no clock pushed: rows made, nothing
    /// latched.
    fn timed(timer: u32) -> UiScript {
        let mut hunt = record("Hunt", 1, 12, "Elwynn Forest");
        hunt.objectives[0] = kill(40, 3);
        let mut slot = slot(50, 0, 0);
        slot.timer = timer;
        let mut s = UiScript::new().unwrap();
        seat(&mut s, &[(50, hunt, slot)]);
        s
    }

    fn timers(s: &UiScript) -> Vec<i64> {
        s.eval::<Vec<i64>>("return {GetQuestTimers()}").unwrap()
    }

    /// The countdown is whole seconds, `deadline - time(NULL) - 1` (`0x4e14b9`-`0x4e14c1`): the
    /// last second reads 0 and the deadline itself runs out.
    #[test]
    fn a_timer_reads_its_last_second_as_zero() {
        let mut s = timed(1000);
        s.set_server_unix_time(999.5);
        assert_eq!(timers(&s), [0]);
        assert!(!s.quest_log_latched(50));
        s.set_server_unix_time(1000.0);
        assert!(timers(&s).is_empty());
        assert!(s.quest_log_latched(50));
    }

    /// A sync that finds the clocks equal to the second stores `G = 1` (`0x4def02`): the
    /// countdowns read `deadline - time(NULL)`, a second more, and the deadline's own second
    /// still reads 0.
    #[test]
    fn a_zero_difference_sync_reads_a_second_more() {
        let mut s = timed(1000);
        s.set_server_unix_time(999.5);
        s.set_clock_sync_zero(true);
        assert_eq!(timers(&s), [1]);
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 1);
        s.set_server_unix_time(1000.0);
        assert_eq!(timers(&s), [0]);
        assert!(!s.quest_log_latched(50));
        s.set_server_unix_time(1001.0);
        assert!(timers(&s).is_empty());
        assert!(s.quest_log_latched(50));
    }

    /// The by-id scans stop on a header whose zone id is the quest's id (`0x4de713`): the
    /// failed-timer latch takes the header, refreshing the givers (`0x4df531`) and leaving the
    /// quest's row unlatched, and the by-id latch read answers no for the header (`0x4df4dd`)
    /// even once the quest's own row is latched by its timer.
    #[test]
    fn a_header_with_the_quests_id_takes_its_by_id_latch() {
        let mut hunt = record("Hunt", 1, 12, "Elwynn Forest");
        hunt.objectives[0] = kill(40, 3);
        let mut slot = slot(12, 0, 0);
        slot.timer = 2000;
        let mut s = UiScript::new().unwrap();
        s.set_server_unix_time(1500.0);
        seat(&mut s, &[(12, hunt, slot)]);
        s.take_quest_giver_refreshes();
        assert!(s.quest_log_latch_quest(12), "the header latches");
        assert_eq!(s.take_quest_giver_refreshes(), 1);
        assert_eq!(is_complete(&s), None, "the quest's row is not latched");
        assert!(!s.quest_log_latched(12));
        s.set_server_unix_time(2500.0);
        assert!(timers(&s).is_empty());
        assert_eq!(is_complete(&s), Some(-1), "its timer latches its own row");
        assert!(!s.quest_log_latched(12), "the header answers first");
    }

    /// Before the first time sync `G` is 0 (`0x4de3d3`) and the countdowns read the local clock:
    /// the timer list and the rebuild count and latch by it (`0x4e14b4`, `0x4de6b1`), while
    /// `GetQuestLogTimeLeft` waits for the sync (`0x4e08fc`).
    #[test]
    fn before_the_sync_the_timers_count_by_the_local_clock() {
        let mut s = timed(1000);
        s.set_local_unix_time(900.0);
        assert_eq!(timers(&s), [99]);
        assert_eq!(s.eval::<i64>("return GetQuestIndexForTimer(1)").unwrap(), 2);
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());
        s.set_server_unix_time(950.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 49);
        assert_eq!(timers(&s), [49], "the server's clock once synced");

        let mut s = timed(1000);
        s.set_local_unix_time(1200.0);
        let slots = s.model_ref().quest_store.slots;
        s.quest_log_changed(slots);
        assert!(
            s.quest_log_latched(50),
            "the rebuild latches by the local clock"
        );
    }

    /// No player, no timers: `GetQuestTimers` returns nothing and latches nothing (`0x4e1417`),
    /// `GetQuestIndexForTimer` is nil (`0x4e165f`), and the turn-in test fails (`0x4df5e1`).
    #[test]
    fn the_timer_verbs_need_the_player() {
        let mut s = timed(2000);
        s.set_server_unix_time(1500.0);
        s.set_quest_log_player(false);
        assert!(timers(&s).is_empty());
        assert!(s
            .eval::<bool>("return GetQuestIndexForTimer(1) == nil")
            .unwrap());
        let mut slots = s.model_ref().quest_store.slots;
        slots[0].timer = 1000;
        s.set_quest_slots(slots);
        heard::listen(&s, &["QUEST_LOG_UPDATE"], None);
        assert!(timers(&s).is_empty());
        assert!(!s.quest_log_latched(50), "nothing latched");
        assert!(heard::take(&s).is_empty());
        s.set_quest_log_player(true);
        assert!(timers(&s).is_empty());
        assert!(s.quest_log_latched(50));
    }

    /// With no selection the selection verbs read slot 0 (`0x4df010`'s -1 into `0x4df063`):
    /// `GetQuestLogTimeLeft` its timer (`0x4e08ca`) and `IsCurrentQuestFailed` its fail bit
    /// (`0x4e0985`-`0x4e09a4`), but no row's latch (`0x4df480` on -1).
    #[test]
    fn with_no_selection_the_selection_verbs_read_slot_zero() {
        let mut s = timed(1000);
        s.set_server_unix_time(900.0);
        s.set_quest_log_selection(0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 99);
        assert!(s
            .eval::<bool>("return IsCurrentQuestFailed() == nil")
            .unwrap());
        assert!(s.quest_log_latch_quest(50));
        assert!(
            s.eval::<bool>("return IsCurrentQuestFailed() == nil")
                .unwrap(),
            "no row holds the selection, so no latch is read"
        );
        let mut slots = s.model_ref().quest_store.slots;
        slots[0].state = SLOT_FAILED;
        s.set_quest_slots(slots);
        assert!(s
            .eval::<bool>("return IsCurrentQuestFailed() == 1")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());
        s.set_quest_log_player(false);
        assert!(s
            .eval::<bool>("return IsCurrentQuestFailed() == nil")
            .unwrap());
    }

    /// `isComplete` of one quest seated alone, row 2 under its header.
    fn is_complete(s: &UiScript) -> Option<i64> {
        s.eval::<Option<i64>>("local _,_,_,_,_,c = GetQuestLogTitle(2) return c")
            .unwrap()
    }

    /// `isComplete` is the turn-in test `0x4df580`, not the slot's complete bit: the counters
    /// and the held items decide (`0x4df74a`, `0x4df780`), the bit only for flags `& 6`
    /// (`0x4df6bd`), a quest with nothing to check never completes (`0x4df6b0`), and failed wins
    /// (`0x4dfa61`).
    #[test]
    fn is_complete_runs_the_turn_in_test() {
        let seated = |r: QuestRecordView, counter: u8, state: u8| {
            let mut s = UiScript::new().unwrap();
            seat(&mut s, &[(50, r, slot(50, counter, state))]);
            s
        };
        let mut hunt = record("Hunt", 1, 12, "Elwynn Forest");
        hunt.objectives[0] = kill(40, 3);
        assert_eq!(
            is_complete(&seated(hunt.clone(), 3, 0)),
            Some(1),
            "no bit needed"
        );
        assert_eq!(is_complete(&seated(hunt.clone(), 2, SLOT_COMPLETE)), None);
        assert_eq!(
            is_complete(&seated(hunt.clone(), 3, SLOT_COMPLETE | SLOT_FAILED)),
            Some(-1)
        );
        let bare = record("Speak", 1, 12, "Elwynn Forest");
        assert_eq!(is_complete(&seated(bare.clone(), 0, SLOT_COMPLETE)), None);
        let mut explore = bare;
        explore.needs_complete_bit = true;
        assert_eq!(is_complete(&seated(explore.clone(), 0, 0)), None);
        assert_eq!(is_complete(&seated(explore, 0, SLOT_COMPLETE)), Some(1));
        let mut gated = hunt;
        gated.needs_complete_bit = true;
        assert_eq!(
            is_complete(&seated(gated, 3, 0)),
            None,
            "the bit gates the met counters"
        );

        // Items count held copies, the quest's own item too.
        let mut pelts = record("Pelts", 1, 12, "Elwynn Forest");
        pelts.objectives[0] = collect(900, 5);
        pelts.src_item_id = 900;
        let mut s = seated(pelts, 0, 0);
        s.set_quest_item_counts([(900, 4)].into());
        assert_eq!(is_complete(&s), None);
        s.set_quest_item_counts([(900, 5)].into());
        assert_eq!(is_complete(&s), Some(1));
    }

    /// The standings are a least (`jl`, `0x4df6d6`) and a most (`jg`, `0x4df6eb`), read off the
    /// base plus the wire (`0x4d6370`) of every reputation-indexed faction, 0 for one not pushed;
    /// a negative money asks that much coinage (`0x4df72e`). A reputation id alone is enough to
    /// be completable, and nothing passes with no player (`0x4df5e1`).
    #[test]
    fn the_turn_in_test_reads_the_standings_and_the_coinage() {
        let mut r = record("Favor", 1, 12, "Elwynn Forest");
        r.rep_min = (72, 3000);
        r.rep_max = (76, 0);
        r.required_money = 100;
        let mut s = UiScript::new().unwrap();
        seat(&mut s, &[(50, r, slot(50, 0, 0))]);
        let reps = |s: &mut UiScript, min: i32, max: i32| {
            s.set_faction_standings([(72, min), (76, max)].into());
        };
        s.set_money(100);
        reps(&mut s, 3000, 0);
        assert_eq!(is_complete(&s), Some(1));
        reps(&mut s, 2999, 0);
        assert_eq!(is_complete(&s), None, "short of the least");
        reps(&mut s, 3000, 1);
        assert_eq!(is_complete(&s), None, "past the most");
        reps(&mut s, 3000, 0);
        s.set_money(99);
        assert_eq!(is_complete(&s), None, "short of the coinage");
        s.set_money(100);
        s.set_faction_standings([(72, 3000)].into());
        assert_eq!(is_complete(&s), Some(1), "a faction not pushed reads 0");
        s.set_faction_standings([(72, 3000), (76, 1)].into());
        assert_eq!(is_complete(&s), None);
        s.set_quest_log_player(false);
        s.set_faction_standings([(72, 3000)].into());
        assert_eq!(is_complete(&s), None, "no player");
    }

    /// The line reads the slot's counter and the bags at the call: a count that moves changes the
    /// next read, with no rebuild and no event.
    #[test]
    fn a_line_reads_the_slot_and_the_bags_at_the_call() {
        let mut s = UiScript::new().unwrap();
        s.set_item_template(
            900,
            ItemTemplateView {
                name: "Wolf Pelt".into(),
                ..Default::default()
            },
        );
        let mut r = record("Pelts", 1, 12, "Elwynn Forest");
        r.objectives[0] = collect(900, 5);
        r.objectives[1] = collect(901, 2);
        r.src_item_id = 901;
        seat(&mut s, &[(50, r, slot(50, 0, 0))]);
        heard::listen(&s, &["QUEST_LOG_UPDATE"], None);
        let line = |s: &UiScript| {
            s.eval::<String>("return (GetQuestLogLeaderBoard(1, 2))")
                .unwrap()
        };
        assert_eq!(line(&s), "Wolf Pelt: 0/5");
        s.set_quest_item_counts([(900, 3)].into());
        assert_eq!(line(&s), "Wolf Pelt: 3/5");
        s.set_quest_item_counts([(900, 9)].into());
        assert_eq!(
            line(&s),
            "Wolf Pelt: 5/5",
            "capped at the need (`0x4e0583`)"
        );
        assert_eq!(
            s.eval::<i64>("return GetNumQuestLeaderBoards(2)").unwrap(),
            1,
            "the quest's own item is no line (`0x4e04fa`)"
        );
        assert!(heard::take(&s).is_empty());
    }

    /// A quest whose end text is not empty has an event line first (`0x4e00a4`, `0x4e0285`): the
    /// text as the wire sent it, typed `event` (`0x84b648`), finished by the slot's complete bit
    /// alone (`0x4e02a2`). The other lines move down one.
    #[test]
    fn the_end_text_is_an_event_line_first_finished_by_the_complete_bit() {
        let mut body_and_heart = record("Body and Heart", 10, 141, "Teldrassil");
        body_and_heart.end_text = "Face Lunaclaw and earn the strength of $N.".into();
        let mut escort = threat_within();
        escort.end_text = "Escort the wagon.".into();
        let mut s = UiScript::new().unwrap();
        caches(&mut s);
        // Rows: Elwynn Forest (1), A Threat Within (2), Teldrassil (3), Body and Heart (4).
        seat(
            &mut s,
            &[
                (783, escort, slot(783, 3, 0)),
                (6001, body_and_heart.clone(), slot(6001, 0, 0)),
            ],
        );
        let board = |s: &UiScript, line: u32, row: u32| {
            s.eval::<(Option<String>, Option<String>, Option<i64>)>(&format!(
                "return GetQuestLogLeaderBoard({line}, {row})"
            ))
            .unwrap()
        };
        let count = |s: &UiScript, row: u32| {
            s.eval::<i64>(&format!("return GetNumQuestLeaderBoards({row})"))
                .unwrap()
        };
        assert_eq!(count(&s, 4), 1, "the event line alone");
        assert_eq!(
            board(&s, 1, 4),
            (
                Some("Face Lunaclaw and earn the strength of $N.".into()),
                Some("event".into()),
                None
            ),
            "the raw text, under no format key"
        );
        assert_eq!(board(&s, 2, 4), (None, None, None));
        assert_eq!(count(&s, 2), 2, "the event line, then the kill line");
        assert_eq!(
            board(&s, 1, 2),
            (Some("Escort the wagon.".into()), Some("event".into()), None)
        );
        assert_eq!(
            board(&s, 2, 2),
            (
                Some("Kobold Vermin slain: 3/10".into()),
                Some("monster".into()),
                None
            ),
            "the kill line at index 2"
        );
        assert_eq!(board(&s, 3, 2), (None, None, None));

        // The complete bit finishes the event line and nothing else.
        let mut escort = threat_within();
        escort.end_text = "Escort the wagon.".into();
        seat(
            &mut s,
            &[
                (783, escort, slot(783, 3, SLOT_COMPLETE)),
                (6001, body_and_heart, slot(6001, 0, SLOT_COMPLETE)),
            ],
        );
        assert_eq!(board(&s, 1, 4).2, Some(1));
        assert_eq!(board(&s, 1, 2).2, Some(1));
        assert_eq!(board(&s, 2, 2).2, None, "the kill line reads its count");
    }

    /// A quest with a reputation faction (`+0x14`) has its reputation line last (`0x4e00da`,
    /// `0x4e0510`): `QUEST_FACTION_NEEDED` with the faction's name, the current rank's label and
    /// the required one's (`0x4e0732`-`0x4e0764`), typed `reputation` (`0x84b5b8`), finished at the
    /// player's standing of at least `+0x18` (`0x4e0787`). It answers any index the other lines
    /// leave (`0x4e0510`), and a faction with no name is counted but reads three nils
    /// (`0x4e053b`).
    #[test]
    fn a_reputation_faction_is_the_last_line_finished_at_the_standing() {
        const ARGENT_DAWN: u32 = 529;
        let mut s = UiScript::new().unwrap();
        caches(&mut s);
        s.set_item_template(
            900,
            ItemTemplateView {
                name: "Scourgestone".into(),
                ..Default::default()
            },
        );
        s.run(
            "QUEST_FACTION_NEEDED = '%s:  %s / %s' \
             FACTION_STANDING_LABEL4 = 'Neutral' \
             FACTION_STANDING_LABEL5 = 'Friendly' \
             FACTION_STANDING_LABEL6 = 'Honored' \
             FACTION_STANDING_LABEL6_FEMALE = 'Honored (f)'",
        )
        .unwrap();
        let mut r = record("Honor the Dawn", 60, 12, "Elwynn Forest");
        r.objectives[0] = kill(VERMIN, 10);
        r.objectives[1] = collect(900, 3);
        r.rep_min = (ARGENT_DAWN, 9000);
        r.rep_faction_name = Some("Argent Dawn".into());
        seat(&mut s, &[(50, r.clone(), slot(50, 0, 0))]);
        let board = |s: &UiScript, line: i64| {
            s.eval::<(Option<String>, Option<String>, Option<i64>)>(&format!(
                "return GetQuestLogLeaderBoard({line}, 2)"
            ))
            .unwrap()
        };
        assert_eq!(
            s.eval::<i64>("return GetNumQuestLeaderBoards(2)").unwrap(),
            3
        );
        assert_eq!(board(&s, 1).1.as_deref(), Some("monster"));
        assert_eq!(board(&s, 2).1.as_deref(), Some("item"));
        let line = |text: &str, finished| {
            (
                Some(text.to_string()),
                Some("reputation".to_string()),
                finished,
            )
        };
        assert_eq!(
            board(&s, 3),
            line("Argent Dawn:  Neutral / Honored", None),
            "a faction not pushed reads 0, Neutral"
        );
        s.set_faction_standings([(ARGENT_DAWN, 8999)].into());
        assert_eq!(board(&s, 3), line("Argent Dawn:  Friendly / Honored", None));
        s.set_faction_standings([(ARGENT_DAWN, 9000)].into());
        assert_eq!(
            board(&s, 3),
            line("Argent Dawn:  Honored / Honored", Some(1))
        );
        assert_eq!(
            board(&s, 4),
            board(&s, 3),
            "the reputation tail compares no index"
        );
        assert_eq!(board(&s, 0), board(&s, 3));

        s.set_unit(
            "player",
            Some(crate::script::UnitState {
                exists: true,
                sex: 3,
                ..Default::default()
            }),
        );
        assert_eq!(
            board(&s, 3),
            line("Argent Dawn:  Honored (f) / Honored (f)", Some(1)),
            "a female player's labels (`0x612bf0`)"
        );

        r.rep_faction_name = None;
        seat(&mut s, &[(50, r, slot(50, 0, 0))]);
        assert_eq!(
            s.eval::<i64>("return GetNumQuestLeaderBoards(2)").unwrap(),
            3
        );
        assert_eq!(board(&s, 3), (None, None, None));
    }

    #[test]
    fn abandon_two_step_pins_the_click_time_target() {
        let mut s = two_quests();
        s.run("SelectQuestLogEntry(3); SetAbandonQuest()").unwrap();
        assert_eq!(
            s.eval::<String>("return GetAbandonQuestName()").unwrap(),
            "Kobold Camp Cleanup"
        );
        // The mark is the quest id (`0x4dfb50`), not the selection: entry 3 is quest 7, slot 1.
        s.run("SelectQuestLogEntry(2); AbandonQuest()").unwrap();
        assert_eq!(s.take_quest_log_abandons(), vec![1]);
        assert!(s.take_quest_log_abandons().is_empty(), "drained");
        s.run("AbandonQuest()").unwrap();
        assert!(s.take_quest_log_abandons().is_empty());
    }

    /// The failed bit reads first (`0x4dfa61`), so a slot with both reads failed.
    #[test]
    fn failed_state_reads() {
        let mut s = UiScript::new().unwrap();
        let mut log = two_quests_log();
        log[0].2.state = SLOT_FAILED | SLOT_COMPLETE;
        seat(&mut s, &log);
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert!(s.eval::<bool>("return IsCurrentQuestFailed()").unwrap());
        assert!(s
            .eval::<bool>("local _, _, _, _, _, done = GetQuestLogTitle(2)\nreturn done == -1")
            .unwrap());
        s.run("SelectQuestLogEntry(3)").unwrap();
        assert!(!s.eval::<bool>("return IsCurrentQuestFailed()").unwrap());
    }

    #[test]
    fn watch_set_is_id_keyed_and_survives_a_log_shuffle() {
        let mut s = two_quests();
        s.run("AddQuestWatch(3)").unwrap(); // quest 7
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
        assert!(s.eval::<bool>("return IsQuestWatched(3)").unwrap());
        assert!(!s.eval::<bool>("return IsQuestWatched(2)").unwrap());
        assert_eq!(s.eval::<i64>("return GetQuestIndexForWatch(1)").unwrap(), 3);
        assert_eq!(s.quest_log_watched(), vec![7]);

        // Quest 783 leaves and quest 7 moves to row 2, keeping its watch.
        let mut shuffled = two_quests_log();
        shuffled.remove(0);
        seat(&mut s, &shuffled);
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
        assert!(s.eval::<bool>("return IsQuestWatched(2)").unwrap());
        assert_eq!(s.eval::<i64>("return GetQuestIndexForWatch(1)").unwrap(), 2);

        s.run("RemoveQuestWatch(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 0);
        s.run("AddQuestWatch(2)").unwrap();
        seat(&mut s, &[]);
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 0);
    }

    /// `AddQuestWatch` and `RemoveQuestWatch` (`0x4e1930`, `0x4e1970`): an argument that fails
    /// `lua_isnumber` (`0x4e1938`, `0x4e1978`) takes the selected quest's id (`0xbb7480`,
    /// `0x4e195c`, `0x4e199c`) and raises nothing; a number is a row, and the selection stays out
    /// of it.
    #[test]
    fn a_non_number_watches_the_selected_quest() {
        for arg in ["", "nil", "'x'", "{}", "true"] {
            let s = two_quests();
            s.run("SelectQuestLogEntry(3)").unwrap(); // quest 7
            s.run(&format!("AddQuestWatch({arg})")).unwrap();
            assert_eq!(s.quest_log_watched(), vec![7], "AddQuestWatch({arg})");
            s.run("SelectQuestLogEntry(2)").unwrap(); // quest 783
            s.run(&format!("RemoveQuestWatch({arg})")).unwrap();
            assert_eq!(s.quest_log_watched(), vec![7], "other quest, Remove({arg})");
            s.run("SelectQuestLogEntry(3)").unwrap();
            s.run(&format!("RemoveQuestWatch({arg})")).unwrap();
            assert!(s.quest_log_watched().is_empty(), "RemoveQuestWatch({arg})");
        }
        // A number is a row whatever is selected; a numeric string counts as one.
        let s = two_quests();
        s.run("SelectQuestLogEntry(2)").unwrap();
        s.run("AddQuestWatch('3')").unwrap();
        assert_eq!(s.quest_log_watched(), vec![7]);
    }

    /// `SelectQuestLogEntry` (`0x4def30`) sets the selected quest id only for an in-list index whose
    /// row is a quest (`0x4def40`, `0x4def48`, `0x4def55`), so a header, 0 or an out-of-range index
    /// leaves the previous quest selected, which the watch fallback then names.
    #[test]
    fn selecting_a_header_or_nothing_keeps_the_previous_quest() {
        let s = two_quests();
        s.run("SelectQuestLogEntry(3)").unwrap(); // quest 7
        for call in [
            "SelectQuestLogEntry(1)",
            "SelectQuestLogEntry(0)",
            "SelectQuestLogEntry(99)",
        ] {
            s.run(call).unwrap();
            assert_eq!(
                s.eval::<i64>("return GetQuestLogSelection()").unwrap(),
                3,
                "{call}"
            );
        }
        s.run("AddQuestWatch()").unwrap();
        assert_eq!(s.quest_log_watched(), vec![7]);
    }

    /// `0x4dede0` appends while the list holds fewer than 5, with no test for a duplicate; the stock
    /// Lua checks `IsQuestWatched`. `0x4dee00` removes the first match only.
    #[test]
    fn a_duplicate_watch_is_appended_and_a_remove_takes_one() {
        let s = two_quests();
        s.run("AddQuestWatch(3) AddQuestWatch(3) AddQuestWatch(2)")
            .unwrap();
        assert_eq!(s.quest_log_watched(), vec![7, 7, 783]);
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 3);
        s.run("RemoveQuestWatch(3)").unwrap();
        assert_eq!(s.quest_log_watched(), vec![7, 783]);
        s.run("AddQuestWatch(3) AddQuestWatch(3) AddQuestWatch(3)")
            .unwrap();
        assert_eq!(s.quest_log_watched().len(), 5, "no sixth (`0x4dede5`)");
    }

    /// The prune (`0x4de7a7` to `0x4de80f`) counts folded quests; a collapse never prunes.
    #[test]
    fn a_collapsed_header_keeps_its_quests_watched() {
        let mut s = two_quests();
        s.run("AddQuestWatch(3)").unwrap(); // quest 7
        assert_eq!(s.quest_log_watched(), vec![7]);

        // Collapsed: only the header is visible, and a rebuild keeps the fold.
        s.run("CollapseQuestHeader(1)").unwrap();
        seat(&mut s, &two_quests_log());
        assert_eq!(s.eval::<i64>("return GetNumQuestLogEntries()").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return GetNumQuestWatches()").unwrap(),
            1,
            "a collapse must not prune the watch of a quest still in the log"
        );
        s.run("ExpandQuestHeader(1)").unwrap();
        seat(&mut s, &two_quests_log());
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetQuestIndexForWatch(1)").unwrap(), 3);

        // Quest 7 leaves the log, neither visible nor folded.
        let mut gone = two_quests_log();
        gone.truncate(1);
        seat(&mut s, &gone);
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 0);
    }

    /// Every value carries the reference's `-1`: 500 seconds to the deadline reads 499.
    #[test]
    fn timers_count_down_against_the_pushed_server_clock() {
        let mut s = UiScript::new().unwrap();
        let mut log = two_quests_log();
        log[1].2.timer = 1_000_900;
        seat(&mut s, &log);

        // No server clock yet.
        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());

        s.set_server_unix_time(1_000_400.0);
        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return (GetQuestTimers())").unwrap(), 499);
        assert_eq!(s.eval::<i64>("return GetQuestIndexForTimer(1)").unwrap(), 3);
        assert!(s
            .eval::<bool>("return GetQuestIndexForTimer(2) == nil")
            .unwrap());

        s.run("SelectQuestLogEntry(2)").unwrap();
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());
        s.run("SelectQuestLogEntry(3)").unwrap();
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 499);

        // Only the clock moves, and the number falls.
        s.set_server_unix_time(1_000_880.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 19);

        // The last second reads 0.
        s.set_server_unix_time(1_000_899.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 0);

        // Past the deadline the lists drop the row and `GetQuestLogTimeLeft` clamps to 0.
        s.set_server_unix_time(1_000_910.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 0);
        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetQuestIndexForTimer(1) == nil")
            .unwrap());
    }

    /// vmangos sets a failed quest's timer to 1 and leaves it in the log (`Player::FailQuest`).
    #[test]
    fn a_failed_timed_quest_drops_out_of_the_timer_list() {
        let mut s = UiScript::new().unwrap();
        let mut log = two_quests_log();
        log[0].2.timer = 1_000_900;
        log[1].2.timer = 1; // failed: the server's sentinel
        log[1].2.state = SLOT_FAILED;
        seat(&mut s, &log);
        s.set_server_unix_time(1_000_400.0);

        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return (GetQuestTimers())").unwrap(), 499);
        assert_eq!(s.eval::<i64>("return GetQuestIndexForTimer(1)").unwrap(), 2);
        s.run("SelectQuestLogEntry(3)").unwrap();
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());
    }

    #[test]
    fn watch_cap_is_five_and_dupes_are_ignored() {
        let mut s = UiScript::new().unwrap();
        let log: Vec<_> = (1..=6u32)
            .map(|q| {
                (
                    q,
                    record(&format!("Quest {q}"), 1, 12, "Elwynn Forest"),
                    slot(q, 0, 0),
                )
            })
            .collect();
        seat(&mut s, &log);
        for i in 2..=7 {
            s.run(&format!("AddQuestWatch({i})")).unwrap();
        }
        // `MAX_WATCHABLE_QUESTS` is 5 (`QuestLogFrame.lua:8`).
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 5);
        s.run("AddQuestWatch(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 5);
        assert_eq!(s.quest_log_watched(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn pushable_follows_the_selection() {
        let mut s = UiScript::new().unwrap();
        let mut log = two_quests_log();
        log[0].1.pushable = true; // 783 is sharable, 7 is not
        seat(&mut s, &log);

        assert_eq!(
            s.eval::<Option<i64>>("return GetQuestLogPushable()")
                .unwrap(),
            None,
            "no selection yet"
        );
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert_eq!(
            s.eval::<Option<i64>>("return GetQuestLogPushable()")
                .unwrap(),
            Some(1)
        );
        s.run("SelectQuestLogEntry(3)").unwrap();
        assert_eq!(
            s.eval::<Option<i64>>("return GetQuestLogPushable()")
                .unwrap(),
            None
        );
    }

    /// A miss keeps the mark, as `0x4df070` returns before the clear at `0x4df0cf`.
    #[test]
    fn abandon_mark_is_a_quest_id_that_survives_a_reindex() {
        let mut s = two_quests();
        s.run("SelectQuestLogEntry(3); SetAbandonQuest()").unwrap(); // quest 7

        // 783 leaves, quest 7 moves to slot 1's row 2 and a stranger takes row 3.
        let moved = vec![
            (
                999,
                record("Stranger", 5, 12, "Elwynn Forest"),
                slot(999, 0, 0),
            ),
            (7, camp_cleanup(), slot(7, 10, SLOT_COMPLETE)),
        ];
        seat(&mut s, &moved);
        assert_eq!(
            s.eval::<String>("return GetAbandonQuestName()").unwrap(),
            "Kobold Camp Cleanup"
        );
        s.run("AbandonQuest()").unwrap();
        assert_eq!(s.take_quest_log_abandons(), vec![1], "quest 7's slot");

        // Folded away: still abandonable.
        s.run("SelectQuestLogEntry(2); SetAbandonQuest(); CollapseQuestHeader(1)")
            .unwrap();
        s.run("AbandonQuest()").unwrap();
        assert_eq!(s.take_quest_log_abandons(), vec![1]);

        // Gone from the log: nothing queues, and the mark stays.
        s.run("SelectQuestLogEntry(2); SetAbandonQuest()").unwrap();
        seat(&mut s, &moved[..1]);
        s.run("AbandonQuest()").unwrap();
        assert!(s.take_quest_log_abandons().is_empty());
        seat(&mut s, &moved);
        s.run("AbandonQuest()").unwrap();
        assert_eq!(
            s.take_quest_log_abandons(),
            vec![1],
            "the kept mark still names 7"
        );
    }

    /// A party of one other, so `QuestLogPushQuest`'s party check passes.
    fn in_a_party(s: &mut UiScript) {
        s.set_party(crate::script::PartyState {
            members: vec![crate::script::PartyMemberInfo {
                name: "Mate".into(),
                guid: 0x300,
            }],
            ..Default::default()
        });
    }

    #[test]
    fn push_queues_the_selected_quest_id_not_its_index() {
        let mut s = UiScript::new().unwrap();
        let mut log = two_quests_log();
        log[1].1.pushable = true; // quest id 7
        seat(&mut s, &log);
        in_a_party(&mut s);
        s.run("SelectQuestLogEntry(3)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert_eq!(s.take_quest_log_pushes(), vec![7], "entry 3 is quest id 7");
        assert!(s.take_quest_log_pushes().is_empty(), "drained");
    }

    #[test]
    fn push_without_a_selection_queues_nothing() {
        let mut s = two_quests();
        in_a_party(&mut s);
        s.run("QuestLogPushQuest()").unwrap();
        assert!(s.take_quest_log_pushes().is_empty());
    }

    /// `0x4e13a6` checks the party and the sharable bit itself; each guard is tested alone.
    #[test]
    fn push_refuses_solo_and_refuses_an_unsharable_quest() {
        let mut s = UiScript::new().unwrap();
        let mut log = two_quests_log();
        log[0].1.pushable = true; // quest id 783
        seat(&mut s, &log);
        s.run("SelectQuestLogEntry(2)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert!(s.take_quest_log_pushes().is_empty(), "solo pushes nothing");

        in_a_party(&mut s);
        s.run("QuestLogPushQuest()").unwrap();
        assert_eq!(s.take_quest_log_pushes(), vec![783]);

        s.run("SelectQuestLogEntry(3)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert!(
            s.take_quest_log_pushes().is_empty(),
            "an unsharable quest pushes nothing even in a party"
        );
    }

    #[test]
    fn push_on_a_header_row_queues_nothing() {
        let mut s = two_quests();
        in_a_party(&mut s);
        s.run("SelectQuestLogEntry(1)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert!(
            s.take_quest_log_pushes().is_empty(),
            "a header is not a quest"
        );
    }

    // ── When `QUEST_LOG_UPDATE` fires ──────────────────────────────────────────────────────────

    /// The slots of `log`, as [`seat`] lays them out.
    fn slots_of(log: &[(u32, QuestRecordView, QuestSlotView)]) -> [QuestSlotView; QUEST_LOG_SLOTS] {
        let mut slots = [QuestSlotView::default(); QUEST_LOG_SLOTS];
        for (i, (_, _, s)) in log.iter().enumerate() {
            slots[i] = *s;
        }
        slots
    }

    /// The field watcher's rebuild (`0x4de4d0` → `0x4de510`) fires `QUEST_LOG_UPDATE` once per
    /// change, whatever the change, with the new list in place for the handler.
    #[test]
    fn a_log_change_fires_quest_log_update_once() {
        let mut s = two_quests();
        heard::listen(
            &s,
            &["QUEST_LOG_UPDATE"],
            Some("(GetQuestLogLeaderBoard(1, 2))"),
        );
        let mut log = two_quests_log();
        log[0].2.counters[0] = 4;
        s.quest_log_changed(slots_of(&log));
        assert_eq!(
            heard::take(&s),
            ["QUEST_LOG_UPDATE Kobold Vermin slain: 4/10"]
        );
        // A change no line shows (the deadline word) is a change of the field all the same.
        log[0].2.timer = 77;
        s.quest_log_changed(slots_of(&log));
        assert_eq!(heard::take(&s).len(), 1);
    }

    /// With no active player the rebuild returns before its signal (`0x4de568`-`0x4de572`), and
    /// the list it would have made is not made; the time and rank signals need none (`0x4def1b`,
    /// `0x4de52f`).
    #[test]
    fn without_the_active_player_the_rebuild_signals_nothing() {
        let mut s = two_quests();
        heard::listen(&s, &["QUEST_LOG_UPDATE"], None);
        s.set_quest_log_player(false);
        let mut log = two_quests_log();
        log.truncate(1);
        s.quest_log_changed(slots_of(&log));
        assert!(heard::take(&s).is_empty());
        assert_eq!(
            s.eval::<i64>("return (GetNumQuestLogEntries())").unwrap(),
            3
        );
        s.quest_log_clock_synced();
        s.quest_log_faction_rank_changed();
        assert_eq!(heard::take(&s).len(), 2);
        s.set_quest_log_player(true);
        s.quest_log_changed(slots_of(&log));
        assert_eq!(heard::take(&s).len(), 1);
        assert_eq!(
            s.eval::<i64>("return (GetNumQuestLogEntries())").unwrap(),
            2
        );
    }

    /// A quest whose record is not held gets no row and is asked once; until it lands every
    /// rebuild returns at once with no event (`0x4de545`), and its landing rebuilds once
    /// (`0x4de8b0`).
    #[test]
    fn an_awaited_record_holds_the_list_until_it_lands() {
        let mut s = two_quests();
        heard::listen(&s, &["QUEST_LOG_UPDATE"], Some("GetNumQuestLogEntries()"));
        let mut log = two_quests_log();
        log.push((
            11,
            record("Westbrook", 5, 12, "Elwynn Forest"),
            slot(11, 0, 0),
        ));
        let slots = slots_of(&log);
        s.quest_log_changed(slots);
        assert_eq!(
            heard::take(&s),
            ["QUEST_LOG_UPDATE 3"],
            "the rebuild that misses lists the rest and fires"
        );
        assert_eq!(s.take_quest_record_asks(), vec![11]);
        let mut killed = slots;
        killed[0].counters[0] = 5;
        s.quest_log_changed(killed);
        assert!(heard::take(&s).is_empty(), "held while 11 is in flight");
        assert!(s.take_quest_record_asks().is_empty());
        s.land_quest_records(vec![(11, log[2].1.clone())]);
        assert_eq!(heard::take(&s), ["QUEST_LOG_UPDATE 4"]);
        assert_eq!(
            s.eval::<String>("return (GetQuestLogLeaderBoard(1, 2))")
                .unwrap(),
            "Kobold Vermin slain: 5/10",
            "the held change is read at the landing's rebuild"
        );
    }

    /// Each rebuild that misses leaves its own callback: two world entries before the record
    /// lands make two rebuilds, two events. A refusal runs them with status 0, which leaves the
    /// awaited count, so the list holds until the next world entry.
    #[test]
    fn each_rebuild_that_misses_leaves_a_callback_and_a_refusal_holds_the_list() {
        let mut s = two_quests();
        heard::listen(&s, &["QUEST_LOG_UPDATE"], None);
        let mut log = two_quests_log();
        log.push((
            11,
            record("Westbrook", 5, 12, "Elwynn Forest"),
            slot(11, 0, 0),
        ));
        s.set_quest_slots(slots_of(&log));
        s.quest_log_enter_world();
        s.quest_log_enter_world();
        assert_eq!(heard::take(&s).len(), 2);
        s.land_quest_records(vec![(11, log[2].1.clone())]);
        assert_eq!(heard::take(&s).len(), 2, "one rebuild per callback");

        log.push((12, record("Hogger", 9, 12, "Elwynn Forest"), slot(12, 0, 0)));
        s.quest_log_changed(slots_of(&log));
        assert_eq!(heard::take(&s).len(), 1);
        s.refuse_quest_record(12);
        s.quest_log_changed(slots_of(&log));
        assert!(heard::take(&s).is_empty(), "the refusal left 12 awaited");
        s.quest_log_enter_world();
        assert_eq!(heard::take(&s).len(), 1, "the world entry zeroes the count");
    }

    /// A reward's item record is looked up when the getter asks, for the selected quest only, and
    /// asked once; each call that missed holds one callback, and the landing fires one
    /// `QUEST_LOG_UPDATE` each, after the frame's records are all in (`0x4e0b9e` → `0x4e0830`).
    #[test]
    fn a_reward_miss_asks_from_the_getter_and_its_landing_fires_once() {
        let mut s = UiScript::new().unwrap();
        let mut log = two_quests_log();
        log[1].1.rewards = vec![(3000, 1)];
        seat(&mut s, &log);
        heard::listen(
            &s,
            &["QUEST_LOG_UPDATE"],
            Some("(GetQuestLogRewardInfo(1))"),
        );
        assert!(
            s.take_item_stat_asks().is_empty(),
            "the rebuild asks for no reward"
        );
        s.run("SelectQuestLogEntry(3)").unwrap();
        assert!(s
            .eval::<bool>("return GetQuestLogRewardInfo(1) == nil")
            .unwrap());
        assert_eq!(s.take_item_stat_asks(), vec![3000]);
        s.set_item_template(
            3000,
            ItemTemplateView {
                name: "Kobold Cloak".into(),
                ..Default::default()
            },
        );
        assert!(heard::take(&s).is_empty(), "fired at the flush");
        s.flush_quest_log_refires();
        assert_eq!(heard::take(&s), ["QUEST_LOG_UPDATE Kobold Cloak"]);
        // A record no verb missed fires nothing.
        s.set_item_template(3001, ItemTemplateView::default());
        s.flush_quest_log_refires();
        assert!(heard::take(&s).is_empty());
    }

    /// Two reads of each line in flight ask once and signal once per record as it lands, where
    /// the reference's lookups hold a callback a call (`0x4e05c9`).
    #[test]
    fn each_missed_record_signals_once_however_many_calls_missed_it() {
        let mut s = UiScript::new().unwrap();
        let mut r = record("Pelts", 1, 12, "Elwynn Forest");
        r.objectives[0] = kill(40, 3);
        r.objectives[1] = collect(900, 5);
        seat(&mut s, &[(50, r, slot(50, 1, 0))]);
        heard::listen(&s, &["QUEST_LOG_UPDATE"], None);
        for _ in 0..2 {
            assert_eq!(
                s.eval::<String>("return (GetQuestLogLeaderBoard(2, 2))")
                    .unwrap(),
                "...: 0/5"
            );
            assert_eq!(
                s.eval::<String>("return (GetQuestLogLeaderBoard(1, 2))")
                    .unwrap(),
                "... slain: 1/3"
            );
        }
        assert_eq!(s.take_item_stat_asks(), vec![900]);
        assert_eq!(s.take_quest_creature_asks(), vec![40], "asked once");
        s.set_item_template(
            900,
            ItemTemplateView {
                name: "Wolf Pelt".into(),
                ..Default::default()
            },
        );
        s.set_quest_creature_name(40, "Young Wolf".into());
        s.flush_quest_log_refires();
        assert_eq!(heard::take(&s).len(), 2);
        assert_eq!(
            s.eval::<String>("return (GetQuestLogLeaderBoard(1, 2))")
                .unwrap(),
            "Young Wolf slain: 1/3"
        );
        s.flush_quest_log_refires();
        assert!(heard::take(&s).is_empty(), "a hit holds nothing");
        // A refused creature's callbacks only log (`0x4e07dc`).
        let mut other = record("Bears", 1, 12, "Elwynn Forest");
        other.objectives[0] = kill(41, 3);
        seat(&mut s, &[(51, other, slot(51, 0, 0))]);
        heard::take(&s);
        s.run("GetQuestLogLeaderBoard(1, 2)").unwrap();
        s.refuse_quest_creature(41);
        s.set_quest_creature_name(41, "Bear".into());
        s.flush_quest_log_refires();
        assert!(heard::take(&s).is_empty());
    }

    /// A handler that walks every line at each `QUEST_LOG_UPDATE` over three creatures in flight:
    /// the count is one per landed record whether the three land in one frame or in three.
    #[test]
    fn the_landing_count_does_not_depend_on_how_the_landings_fall_in_frames() {
        let run = |frames: &[&[u32]]| {
            let mut s = UiScript::new().unwrap();
            let quests: Vec<_> = (0..3u32)
                .map(|i| {
                    let mut r = record(&format!("Hunt {i}"), 1, 12, "Elwynn Forest");
                    r.objectives[0] = kill(40 + i, 3);
                    (50 + i, r, slot(50 + i, 0, 0))
                })
                .collect();
            seat(&mut s, &quests);
            s.run(
                "WALKS = 0 local w = CreateFrame('Frame') w:RegisterEvent('QUEST_LOG_UPDATE') \
                 w:SetScript('OnEvent', function() WALKS = WALKS + 1 \
                   for i = 1, GetNumQuestLogEntries() do GetQuestLogLeaderBoard(1, i) end end) \
                 for i = 1, GetNumQuestLogEntries() do GetQuestLogLeaderBoard(1, i) end",
            )
            .unwrap();
            for landings in frames {
                for &entry in *landings {
                    s.set_quest_creature_name(entry, format!("Beast {entry}"));
                }
                s.flush_quest_log_refires();
            }
            s.eval::<i64>("return WALKS").unwrap()
        };
        assert_eq!(run(&[&[40, 41, 42]]), 3);
        assert_eq!(run(&[&[40], &[41], &[42]]), 3);
        assert_eq!(run(&[&[42, 40], &[], &[41]]), 3);
    }

    /// `0x4df7b0`: an item of ours that a listed quest needs fires once; one no quest needs fires
    /// nothing.
    #[test]
    fn an_item_a_listed_quest_needs_fires_once() {
        let mut s = UiScript::new().unwrap();
        let mut a = record("Pelts", 1, 12, "Elwynn Forest");
        a.objectives[0] = collect(900, 5);
        let mut b = record("More Pelts", 2, 12, "Elwynn Forest");
        b.objectives[2] = collect(900, 2);
        seat(&mut s, &[(50, a, slot(50, 0, 0)), (51, b, slot(51, 0, 0))]);
        heard::listen(&s, &["QUEST_LOG_UPDATE"], None);
        s.quest_item_changed(900);
        assert_eq!(heard::take(&s).len(), 1);
        s.quest_item_changed(901);
        assert!(heard::take(&s).is_empty());
    }

    /// `0x4df880`: a quest's row, headers counted, 0 when it has none.
    #[test]
    fn the_watch_update_index_counts_headers() {
        let s = two_zones();
        assert_eq!(s.quest_log_index(783), 2);
        assert_eq!(s.quest_log_index(7), 4);
        assert_eq!(s.quest_log_index(999), 0);
    }

    // ── IsUnitOnQuest (`0x4dfe10`) ──

    const ME: u64 = 0x10;
    const MATE: u64 = 0x300;
    const FAR_MATE: u64 = 0x301;

    /// A held unit whose quest-log window holds `ids`, one per slot from the last slot down, so the
    /// scan must reach slot 19.
    fn held(guid: u64, ids: &[u32]) -> crate::script::UnitState {
        let mut quest_log = [[0; 3]; 20];
        for (slot, &id) in quest_log.iter_mut().rev().zip(ids) {
            *slot = [id, 0, 0];
        }
        crate::script::UnitState {
            exists: true,
            has_object: true,
            is_player: true,
            guid,
            quest_log,
            ..Default::default()
        }
    }

    /// A header, then quest 783 (row 2) and quest 7 (row 3). We hold both; `party1` holds 783 and
    /// `party2` is out of range. `target` is `party1`; `mouseover` a stranger and `raid7` a raid
    /// mate outside our subgroup, both holding 783.
    fn on_quest_world() -> UiScript {
        let mut s = two_quests();
        s.set_party(crate::script::PartyState {
            members: vec![
                crate::script::PartyMemberInfo {
                    name: "Mate".into(),
                    guid: MATE,
                },
                crate::script::PartyMemberInfo {
                    name: "Far".into(),
                    guid: FAR_MATE,
                },
            ],
            ..Default::default()
        });
        s.set_unit("player", Some(held(ME, &[7, 783])));
        s.set_unit("party1", Some(held(MATE, &[783])));
        s.set_unit(
            "party2",
            Some(crate::script::UnitState {
                exists: true,
                is_player: true,
                guid: FAR_MATE,
                ..Default::default()
            }),
        );
        s.set_unit("target", Some(held(MATE, &[783])));
        s.set_unit("mouseover", Some(held(0x500, &[783])));
        s.set_unit("raid7", Some(held(0x700, &[783])));
        s.set_unit(
            "pet",
            Some(crate::script::UnitState {
                exists: true,
                has_object: true,
                guid: 0xF140_0000_0000_0001,
                ..Default::default()
            }),
        );
        s
    }

    /// The number 1 when the unit's log holds the row's quest (`0x4dfef5` pushes 1.0), else nil
    /// (`0x4dfedb`), never a boolean.
    #[test]
    fn is_unit_on_quest_answers_one_for_the_player_and_a_held_party_member_on_it() {
        let mut s = on_quest_world();
        let on = |s: &mut UiScript, call: &str| -> Option<i64> {
            s.eval::<Option<i64>>(&format!("return {call}")).unwrap()
        };
        assert!(s
            .eval::<bool>("return type(IsUnitOnQuest(2, 'party1')) == 'number'")
            .unwrap());
        assert_eq!(on(&mut s, "IsUnitOnQuest(2, 'party1')"), Some(1));
        assert_eq!(on(&mut s, "IsUnitOnQuest(3, 'party1')"), None, "not on 7");
        assert_eq!(on(&mut s, "IsUnitOnQuest(2, 'player')"), Some(1));
        assert_eq!(on(&mut s, "IsUnitOnQuest(3, 'player')"), Some(1));
        assert_eq!(
            on(&mut s, "IsUnitOnQuest(2, 'PARTY1')"),
            Some(1),
            "tokens fold case"
        );
        // The guid decides, not the token: `target` names party1 (`0x4e7f70`'s slot compare).
        assert_eq!(on(&mut s, "IsUnitOnQuest(2, 'target')"), Some(1));
    }

    /// `0x4e7f70` admits the active player and the four party slots alone, and `0x468460` needs
    /// the object held: every other unit answers nil whatever its log holds.
    #[test]
    fn is_unit_on_quest_is_nil_off_the_party_or_out_of_range() {
        let mut s = on_quest_world();
        for token in [
            "party2",    // in the party, not held
            "party3",    // no such slot
            "mouseover", // held, on the quest, not grouped
            "raid7",     // a raid mate outside our subgroup
            "pet",       // not a player, no quest log
            "",          // names nobody
        ] {
            assert!(
                s.eval::<bool>(&format!("return IsUnitOnQuest(2, '{token}') == nil"))
                    .unwrap(),
                "{token:?} answers nil"
            );
        }
        // The party stops at four slots of guids: a member who left reads nil off a stale log.
        s.set_party(crate::script::PartyState::default());
        assert!(s
            .eval::<bool>("return IsUnitOnQuest(2, 'target') == nil")
            .unwrap());
    }

    /// The index is `_ftol`'d and 1-based (`0x4dfe4d`, `dec` at `0x4dfe52`); a header or a row
    /// past the list has no quest id (`0x4df150`) and answers nil before the token is read, so an
    /// unknown token raises only on a quest row (`0x515c14`).
    #[test]
    fn is_unit_on_quest_reads_the_row_before_the_token() {
        let s = on_quest_world();
        for index in ["1", "0", "-1", "4", "99", "-0.5"] {
            assert!(
                s.eval::<bool>(&format!("return IsUnitOnQuest({index}, 'party1') == nil"))
                    .unwrap(),
                "index {index}"
            );
            assert!(
                s.eval::<bool>(&format!("return IsUnitOnQuest({index}, 'bogus') == nil"))
                    .unwrap(),
                "index {index} answers before the token is resolved"
            );
        }
        assert!(s
            .eval::<bool>("return IsUnitOnQuest(2.9, 'party1') == 1")
            .unwrap());
        assert!(s
            .eval::<bool>("return IsUnitOnQuest('2', 'party1') == 1")
            .unwrap());
        for (token, shown) in [("'bogus'", "bogus"), ("5", "5")] {
            let e = format!(
                "{:?}",
                s.eval::<mlua::Value>(&format!("return IsUnitOnQuest(2, {token})"))
                    .unwrap_err()
            );
            assert!(
                e.contains(&format!("Unknown unit name: {shown}")),
                "{token}: {e}"
            );
        }
    }

    /// `lua_isnumber` on 1 and `lua_isstring` on 2, both ahead of any read (`0x4dfe1e`,
    /// `0x4dfe32`), raise the one Usage line (`0x84b564`), an out-of-range index included.
    #[test]
    fn is_unit_on_quest_raises_usage_on_a_bad_argument() {
        let s = on_quest_world();
        for args in [
            "",
            "2",
            "99",
            "nil, 'party1'",
            "'x', 'party1'",
            "{}, 'party1'",
            "2, nil",
            "2, {}",
            "2, true",
        ] {
            let e = format!(
                "{:?}",
                s.eval::<mlua::Value>(&format!("return IsUnitOnQuest({args})"))
                    .unwrap_err()
            );
            assert!(
                e.contains("Usage: IsUnitOnQuest(index, \\\"unit\\\")")
                    || e.contains("Usage: IsUnitOnQuest(index, \"unit\")"),
                "({args}) must raise Usage:, got {e}"
            );
        }
    }
}
