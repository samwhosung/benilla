//! The reputation pane's twelve `ReputationFrame` globals: the app pushes a flat snapshot, and
//! the engine keeps the list the reference does (`[0x4d5200, 0x4d6d80)`): its entries in add order,
//! each sort (`0x4d5c40`) the CRT's `qsort` over the array as the last one left it. A row is a
//! header by flag `0x08` (`0x4d5acb`), which emulators name `INVISIBLE_FORCED`, keyed by its own
//! id; any other row files under its `Faction.dbc` `team`, or under `-1` while inactive (flag
//! `0x20`), and adds its team's header after itself when none is listed (`0x4d5b87`). Key `0`
//! reads `FACTION_OTHER` and `-1` `FACTION_INACTIVE`. Headers sort by child count, then by name
//! with the synthetic keys last, `0` first (`0x4d5dc0`); rows by their header's place, a folded
//! child after every other row, the header first, then by name (`0x4d5e70`). The displayed count
//! (`0xb73764`, `GetNumFactions`) leaves out the folded children and the childless headers, which
//! the sort puts after the displayed rows.
//!
//! Each verb resolves its 1-based index into the whole entry list, folded and hidden rows
//! included, to the entry's faction id (`0x4d5fa0`), a header's included: its own id, or 0 and -1
//! for the two synthetic ones. The selection holds such an id (`0xb73768`). The folds change at a
//! rebuild (all open, Inactive folded, `0x4d5479`, `0x4d55d9`), at a header's registration (all
//! open, `0x4d5c34`) and by the two fold verbs alone. The verbs that send do so inside their call
//! ([`ReputationCall`]).

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// The synthetic header key for factions the player moved to the inactive bucket.
const KEY_INACTIVE: i64 = -1;

/// One reputation faction, resolved app-side from `Faction.dbc` and the player's wire slot.
#[derive(Clone, Debug, PartialEq)]
pub struct FactionEntry {
    /// `Faction.dbc` id: the row's identity, and what a child's [`Self::parent_id`] names.
    pub faction_id: u32,
    /// `reputationIndex`: the slot every send addresses.
    pub rep_list_id: u32,
    /// `Faction.dbc`'s `team`: the header key this row files under, `0` the "Other" bucket, and
    /// "Inactive" instead while [`Self::inactive`] is set.
    pub parent_id: u32,
    /// The localized name: the bar's label, or a header's text.
    pub name: String,
    /// The localized description, for the detail popup; often empty.
    pub description: String,
    /// The DBC race/class base plus the wire standing (`0x4d6370`): `barValue`, absolute.
    pub standing: i32,
    /// The rank, 1..=8 (`FACTION_STANDING_LABEL<n>`); `GetWatchedFactionInfo`'s `reaction` is this
    /// same scale (`0x4d68a0`), not the unit-reaction one.
    pub standing_id: u8,
    /// The rank's absolute bounds (`.rdata 0x80928c`), `barMin`/`barMax`; the stock pane
    /// normalizes them itself (`ReputationFrame.lua:80-82`).
    pub bar_min: i32,
    pub bar_max: i32,
    /// Flag `0x01`, the only flag gating list membership: off until the player meets the faction.
    pub visible: bool,
    /// Flag `0x08`: this row is a header.
    pub is_header: bool,
    /// Flag `0x02`, which `FactionToggleAtWar` writes at once (`0x4d6074`).
    pub at_war: bool,
    /// Flag `0x10` clear and `standing >= -3000`: what `GetFactionInfo` reports (`0x4d6608`).
    pub can_toggle_at_war: bool,
    /// Flag `0x20`, which the inactive verbs write at once (`0x4d612c`); it moves the row under
    /// "Inactive" rather than hiding it.
    pub inactive: bool,
}

/// A flat, unordered push of every faction the player has a slot for; the engine groups and sorts.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ReputationState {
    pub entries: Vec<FactionEntry>,
    /// `PLAYER_FIELD_WATCHED_FACTION_INDEX` as a slot, `None` for none: a server field with no
    /// client mirror, never written here.
    pub watched: Option<u32>,
}

/// What a reputation getter asks the lent host of one faction, answered as of the place it runs
/// ([`super::live::ask`]); the getters read the slot's flag byte and standing and the player's
/// watched field at the call (`0x4d65ac`, `0x4d65ea`, `0x4d6613`, `0x4d6660`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactionQuery {
    /// The faction by its `Faction.dbc` id.
    Of(u32),
    /// The faction in the player's watched slot (`0x4d6820`).
    Watched,
}

/// A [`FactionQuery`]'s answer.
#[derive(Clone, Debug, PartialEq)]
pub struct FactionRead {
    /// The faction's row, `None` when the player has none for it.
    pub entry: Option<FactionEntry>,
    /// The watched slot (`PLAYER_FIELD_WATCHED_FACTION_INDEX`), `None` for none.
    pub watched: Option<u32>,
}

impl super::live::LiveQuery for FactionQuery {
    type Answer = FactionRead;
}

/// The whole list as of the place the VM's list is built or sorted: every faction the player has a
/// slot for, and every standing the quest log's turn-in test reads.
#[derive(Clone, Copy, Debug)]
pub struct ReputationBook;

/// A [`ReputationBook`]'s answer.
#[derive(Clone, Debug, Default)]
pub struct Book {
    pub state: ReputationState,
    /// `GetFactionStanding` (`0x4d6370`) per faction with a reputation index.
    pub standings: HashMap<u32, i32>,
}

impl super::live::LiveQuery for ReputationBook {
    type Answer = Book;
}

/// A reputation verb the host applies inside its call, by reputation-list slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReputationCall {
    /// `FactionToggleAtWar(index)` (`0x4d6950`) past `SetFactionAtWar`'s floor and slot tests
    /// (`0x4d5fe8`, `0x4d5ffe`-`0x4d6007`): in combat its refusal (`0x4d6040`), else the flag
    /// write and `CMSG_SET_FACTION_ATWAR` with the war asked for (`0x4d6053`-`0x4d60be`).
    AtWar { rep_list_id: u32, at_war: bool },
    /// `SetFactionInactive(index)` or `SetFactionActive(index)` (`0x4d69b0`, `0x4d6a00`) past
    /// the slot test (`0x4d6104`-`0x4d610f`): the flag write and `CMSG_SET_FACTION_INACTIVE`
    /// (`0x4d612c`-`0x4d6176`), then the re-sort and its `UPDATE_FACTION` (`0x4d617b`).
    Inactive { rep_list_id: u32, inactive: bool },
    /// `SetWatchedFactionIndex(index)` (`0x4d6b60`): `CMSG_SET_WATCHED_FACTION` with the slot, -1
    /// for a faction with none, when it differs from the player's field (`0x4d6276`).
    Watch(i32),
}

/// What a reputation message does to the list, in its handler's order; the app hands them over
/// after the snapshot they ride on ([`super::UiScript::push_reputation`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReputationListOp {
    /// An edit of the list inside a handler: the adder for each of `listed`, in order
    /// (`0x4d5738`, `0x4d5826`), then the sort when `sort` (`0x4d574a`, `0x4d58c8`). A message
    /// hands the adder at the listing and the sort after its writes, as the reference's does.
    Edit { listed: Vec<u32>, sort: bool },
    /// `SMSG_INITIALIZE_FACTIONS` with the player held: the rebuild (`0x4d5694`).
    Rebuild,
}

/// One list entry (20 bytes in the reference, `0x4d5290`); `pub(crate)` only because
/// [`super::model::Model`] stores them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ListEntry {
    /// The reputation slot (`+0`), -1 for the Inactive header and a header with no slot.
    slot: i64,
    /// The faction id (`+4`): a `Faction.dbc` id, or 0 or -1 for the synthetic headers.
    faction: i64,
    /// The header flag (`+8`).
    header: bool,
    /// Its header's place in the sorted header array (`+0xc`), written by the sort.
    pos: usize,
    /// The header key (`+0x10`): a header's own id, any other row's team.
    key: i64,
}

/// One header object (8 bytes, `0xb736c0`): its key and its child count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeaderObject {
    key: i64,
    children: usize,
}

impl super::UiScript {
    /// Push the reputation snapshot, the slots' names, standings and flags, the fallback of the
    /// getters that read the lent host ([`FactionQuery`]) and what the list sorts by: the list
    /// moves only by the messages' ops ([`Self::apply_reputation_ops`]) and the verbs, and no row
    /// leaves it but at the rebuild. The first push into a VM is the rebuild's
    /// ([`Self::reputation_enter_world`]).
    pub fn set_reputation(&mut self, state: ReputationState) {
        let mut model = self.model_mut();
        model.reputation = state;
        if model.reputation_headers.is_empty() {
            rebuild(&mut model);
        }
    }

    /// `state` when the snapshot moved, then the messages' list ops over it. A push that builds the
    /// list drops the ops, all queued before its rebuild, which clears the list first
    /// (`0x4d547f`-`0x4d5493`).
    pub fn push_reputation(&mut self, state: Option<ReputationState>, ops: Vec<ReputationListOp>) {
        let builds = state.is_some() && self.model_ref().reputation_headers.is_empty();
        if let Some(state) = state {
            self.set_reputation(state);
        }
        if !builds {
            self.apply_reputation_ops(ops);
        }
    }

    /// The snapshot as of the place the list is built or sorted, taken off the lent host: the sort
    /// reads the slots' flags and the rows' names as the reference's reads the slots (`0x4d6210`).
    /// With no host to ask the snapshot stands as it was pushed.
    fn pull_reputation(&mut self) -> bool {
        let Some(book) = super::live::ask(&self.lua, &ReputationBook) else {
            return false;
        };
        let mut model = self.model_mut();
        model.reputation = book.state;
        model.faction_standings = book.standings;
        true
    }

    /// A new VM's seat at the head of the world-enter cascade: the reference keeps its list and
    /// standings across a `/reload`, so the events ahead of the cascade's own rebuild
    /// (`PLAYER_ENTERING_WORLD`, the quest log's rebuild `0x4909a1`) read them. With no host to
    /// ask the VM keeps what it was pushed.
    pub(crate) fn seat_reputation(&mut self) {
        if self.pull_reputation() {
            let mut model = self.model_mut();
            if model.reputation_headers.is_empty() {
                rebuild(&mut model);
            }
        }
    }

    /// Apply the list's half of the reputation messages, in arrival order, over the snapshot as of
    /// here ([`Self::pull_reputation`]). An added row that registers a header opens every header
    /// (`0x4d5c34`), Inactive's included; no message writes the folds otherwise (`0x4d5760`,
    /// `0x4d56b0`, `0x4d5710`). A list not yet built is built instead, which clears it first and
    /// so drops the ops (`0x4d547f`-`0x4d5493`).
    pub fn apply_reputation_ops(&mut self, ops: Vec<ReputationListOp>) {
        if ops.is_empty() {
            return;
        }
        self.pull_reputation();
        let mut model = self.model_mut();
        if model.reputation_headers.is_empty() {
            rebuild(&mut model);
            return;
        }
        for op in ops {
            match op {
                ReputationListOp::Edit {
                    listed,
                    sort: sorted,
                } => {
                    for slot in listed {
                        add_slot(&mut model, slot);
                    }
                    if sorted {
                        sort(&mut model);
                    }
                }
                ReputationListOp::Rebuild => rebuild(&mut model),
            }
        }
    }

    /// The world-enter cascade's rebuild (`0x4909d3` calls `0x4d5430`): the player in the world,
    /// it re-lists the factions with every header expanded and "Inactive" collapsed (`0x4d5479`),
    /// and each of its two sorts signals `UPDATE_FACTION` (`0x4d55d9` → `0x4d6457`, `0x4d55de`;
    /// `0x4d5dab`).
    pub(crate) fn reputation_enter_world(&mut self) {
        self.pull_reputation();
        let state = {
            let mut model = self.model_mut();
            model.reputation_headers.clear();
            std::mem::take(&mut model.reputation)
        };
        self.set_reputation(state);
        self.fire_event("UPDATE_FACTION", Vec::new());
        self.fire_event("UPDATE_FACTION", Vec::new());
    }

    /// Push every reputation-indexed faction's standing, the base plus the wire (`0x4d6370`): what
    /// the quest log's turn-in test reads (`0x4df6ce`, `0x4df6e3`), as the lent host last gave them
    /// ([`Self::pull_reputation`]) or a harness pushes them. A faction left out reads 0, as one
    /// with no reputation index does (`0x4d6377`).
    pub fn set_faction_standings(&mut self, standings: HashMap<u32, i32>) {
        self.model_mut().faction_standings = standings;
    }

    /// Whether the list holds `state` as it stands, the flags its verbs wrote included.
    pub fn reputation_matches(&self, state: &ReputationState) -> bool {
        self.model_ref().reputation == *state
    }

    /// `SetFactionAtWar`'s write of slot `rep_list_id`'s at-war bit (`0x4d6074`), with no signal.
    pub fn set_faction_at_war(&mut self, rep_list_id: u32, at_war: bool) {
        let mut model = self.model_mut();
        for e in model.reputation.entries.iter_mut() {
            if e.rep_list_id == rep_list_id {
                e.at_war = at_war;
            }
        }
    }

    /// `SetFactionInactiveFlag`'s write of slot `rep_list_id`'s inactive bit (`0x4d612c`), then the
    /// re-sort (`0x4d617b` → `0x4d5c40`), which moves the row and signals `UPDATE_FACTION`
    /// (`0x4d5dab`), which the pane repaints on.
    pub fn set_faction_inactive(&mut self, rep_list_id: u32, inactive: bool) {
        self.pull_reputation();
        {
            let mut model = self.model_mut();
            for e in model.reputation.entries.iter_mut() {
                if e.rep_list_id == rep_list_id {
                    e.inactive = inactive;
                }
            }
            sort(&mut model);
        }
        self.fire_event("UPDATE_FACTION", Vec::new());
    }

    /// The reputation verbs a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_reputation_calls(&mut self) -> Vec<ReputationCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Reputation(call) => Some(*call),
            _ => None,
        })
    }
}

/// The VM's text for a synthetic header key; the enUS literal in a bare test VM.
fn synthetic_header(lua: &Lua, key: i64) -> String {
    let (global, fallback) = if key == KEY_INACTIVE {
        ("FACTION_INACTIVE", "Inactive")
    } else {
        ("FACTION_OTHER", "Other")
    };
    lua.globals()
        .get::<String>(global)
        .unwrap_or_else(|_| fallback.into())
}

/// The rebuild (`0x4d5430`): the list emptied (`0x4d5479`-`0x4d5493`), the met rows added from
/// the last `Faction.dbc` row to the first, which is the faction ids descending (`0x4d5555`),
/// then the Inactive header registered and added (`0x4d557b`-`0x4d55d4`), folded with its sort
/// (`0x4d55d9`), and sorted again (`0x4d55de`).
fn rebuild(model: &mut Model) {
    model.reputation_rows.clear();
    model.reputation_headers.clear();
    for e in &met(model) {
        add(model, e);
    }
    register(model, KEY_INACTIVE);
    model.reputation_rows.push(ListEntry {
        slot: -1,
        faction: KEY_INACTIVE,
        header: true,
        pos: 0,
        key: KEY_INACTIVE,
    });
    model.reputation_collapsed.insert(KEY_INACTIVE);
    sort(model);
    sort(model);
}

/// The met factions, visible in a slot below 64, in the order the rebuild walks `Faction.dbc`:
/// from its last row to its first, the ids descending.
fn met(model: &Model) -> Vec<FactionEntry> {
    let mut met: Vec<FactionEntry> = model
        .reputation
        .entries
        .iter()
        .filter(|e| e.visible && e.rep_list_id < 64)
        .cloned()
        .collect();
    met.sort_by_key(|e| std::cmp::Reverse(e.faction_id));
    met
}

/// `0x4d5c20`: a header object for `key`, and every header unfolded (`0x4d5c34`).
fn register(model: &mut Model, key: i64) {
    model
        .reputation_headers
        .push(HeaderObject { key, children: 0 });
    model.reputation_collapsed.clear();
}

/// The adder (`0x4d5a70`): nothing for a slot already listed (`0x4d5a93`); else the entry, and a
/// header registered for a header row (`0x4d5b16`), or, for any other row whose team no header
/// entry names (`0x4d5b6a`-`0x4d5b83`), its team's header after it, with the team's slot
/// (`0x4d5b87`-`0x4d5bdc`).
fn add(model: &mut Model, e: &FactionEntry) {
    let slot = i64::from(e.rep_list_id);
    if model.reputation_rows.iter().any(|r| r.slot == slot) {
        return;
    }
    let faction = i64::from(e.faction_id);
    if e.is_header {
        register(model, faction);
        model.reputation_rows.push(ListEntry {
            slot,
            faction,
            header: true,
            pos: 0,
            key: faction,
        });
        return;
    }
    let team = i64::from(e.parent_id);
    let listed = model
        .reputation_rows
        .iter()
        .any(|r| r.header && r.faction == team);
    model.reputation_rows.push(ListEntry {
        slot,
        faction,
        header: false,
        pos: 0,
        key: team,
    });
    if !listed {
        let team_slot = entry_of(model, team).map_or(-1, |t| i64::from(t.rep_list_id));
        model.reputation_rows.push(ListEntry {
            slot: team_slot,
            faction: team,
            header: true,
            pos: 0,
            key: team,
        });
        register(model, team);
    }
}

/// The adder on a slot (`0x4d5a70`), for the slot's pushed entry; a slot the push lacks adds
/// nothing.
fn add_slot(model: &mut Model, slot: u32) {
    let entry = model
        .reputation
        .entries
        .iter()
        .find(|e| e.rep_list_id == slot)
        .cloned();
    if let Some(e) = entry {
        add(model, &e);
    }
}

/// The key a row counts under (`0x4d5c72`-`0x4d5c89`): -1 while its slot is inactive
/// (`0x4d6210`), else its own.
fn count_key(model: &Model, row: &ListEntry) -> i64 {
    let inactive = u32::try_from(row.slot).ok().is_some_and(|slot| {
        model
            .reputation
            .entries
            .iter()
            .any(|e| e.rep_list_id == slot && e.inactive)
    });
    if inactive {
        KEY_INACTIVE
    } else {
        row.key
    }
}

/// A faction's `Faction.dbc` name, `None` for one with no row: the two synthetic keys.
fn row_name(model: &Model, faction: i64) -> Option<&str> {
    (faction > 0).then(|| entry_of(model, faction).map_or("", |e| e.name.as_str()))
}

/// Whether the header at a sorted place is folded: a place below 32 whose fold is set
/// (`0x4d5d49`-`0x4d5d68`).
fn folded_at(model: &Model, pos: usize) -> bool {
    pos < 32
        && model
            .reputation_headers
            .get(pos)
            .is_some_and(|h| model.reputation_collapsed.contains(&h.key))
}

/// The sort (`0x4d5c40`): each header's children counted (`0x4d5c60`-`0x4d5cba`), the headers
/// sorted (`0x4d5cc9`), each row given its header's place and the displayed count taken down for a
/// folded child and a childless header (`0x4d5cf0`-`0x4d5d94`), then the rows sorted
/// (`0x4d5da3`), both by the CRT's `qsort`.
fn sort(model: &mut Model) {
    let mut headers = std::mem::take(&mut model.reputation_headers);
    for h in &mut headers {
        h.children = model
            .reputation_rows
            .iter()
            .filter(|r| !r.header && count_key(model, r) == h.key)
            .count();
    }
    crate::crt::qsort(&mut headers, |a, b| header_order(model, a, b));
    model.reputation_headers = headers;
    let mut rows = std::mem::take(&mut model.reputation_rows);
    let mut visible = rows.len();
    for r in &mut rows {
        let key = count_key(model, r);
        let Some(pos) = model.reputation_headers.iter().position(|h| h.key == key) else {
            continue;
        };
        r.pos = pos;
        if !r.header && folded_at(model, pos) {
            visible -= 1;
        }
        if r.header && model.reputation_headers[pos].children == 0 {
            visible -= 1;
        }
    }
    crate::crt::qsort(&mut rows, |a, b| row_order(model, a, b));
    model.reputation_rows = rows;
    model.reputation_visible = visible;
}

/// `0x4d5dc0`: a header with children first, two without equal (`0x4d5e5b`); then two with
/// `Faction.dbc` rows by name (`0x4d5e28`), one with a row first (`0x4d5e42`), and two without the
/// larger key first (`0x4d5e34`).
fn header_order(model: &Model, a: &HeaderObject, b: &HeaderObject) -> i32 {
    match (a.children, b.children) {
        (0, 0) => return 0,
        (0, _) => return 1,
        (_, 0) => return -1,
        _ => {}
    }
    match (row_name(model, a.key), row_name(model, b.key)) {
        (Some(x), Some(y)) => crate::crt::str_cmp_i(x, y),
        (None, Some(_)) => 1,
        (Some(_), None) => -1,
        (None, None) => {
            if a.key < b.key {
                1
            } else {
                -1
            }
        }
    }
}

/// `0x4d5e70`: across places, a folded child after every row that is not one and two folded
/// children equal (`0x4d5f02`), else the places ascending (`0x4d5ef4`); within a place, the
/// header first (`0x4d5f81`), then two children by name (`0x4d5f70`), equal when either has no
/// `Faction.dbc` row.
fn row_order(model: &Model, a: &ListEntry, b: &ListEntry) -> i32 {
    if a.pos != b.pos {
        let folded = |r: &ListEntry| !r.header && folded_at(model, r.pos);
        return match (folded(a), folded(b)) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => -1,
            (false, false) => {
                if a.pos >= b.pos {
                    1
                } else {
                    -1
                }
            }
        };
    }
    if a.header || b.header {
        return if a.header { -1 } else { 1 };
    }
    match (row_name(model, a.faction), row_name(model, b.faction)) {
        (Some(x), Some(y)) => crate::crt::str_cmp_i(x, y),
        _ => 0,
    }
}

/// Whether `faction` is a registered header key, the header scan of `GetFactionInfo`
/// (`0x4d6640`-`0x4d6649`).
fn is_header_key(model: &Model, faction: i64) -> bool {
    model.reputation_headers.iter().any(|h| h.key == faction)
}

/// The faction id of the entry at a 1-based index into the whole list (`0x4d5fa0`), 0 past it.
///
/// Deviation: the reference bounds the index with `jbe` against the entry count (`0xb73760`), so
/// an index one past the last entry reads the pool entry after it, which only an earlier, longer
/// list wrote, or uninitialised heap (`0x4d5310`, the rebuild resetting the count without
/// clearing, `0x4d547f`); that stale read answers 0 here, as an index further on does.
fn faction_at(model: &Model, index: usize) -> i64 {
    index
        .checked_sub(1)
        .and_then(|n| model.reputation_rows.get(n))
        .map_or(0, |r| r.faction)
}

/// A faction's pushed entry, its slot's: `None` for 0, -1 and a faction the push lacks.
fn entry_of(model: &Model, faction: i64) -> Option<&FactionEntry> {
    let faction = u32::try_from(faction).ok().filter(|&f| f != 0)?;
    model
        .reputation
        .entries
        .iter()
        .find(|e| e.faction_id == faction)
}

/// A faction's row and the watched slot as the getter reads them at its call: the lent host's
/// answer, as the reference reads the slots' flag bytes and standings and the player's field there
/// (`0x4d65ac`, `0x4d65ea`, `0x4d6613`, `0x4d6660`), else the snapshot pushed last.
fn read_faction(lua: &Lua, model: &Model, query: FactionQuery) -> FactionRead {
    if let Some(read) = super::live::ask(lua, &query) {
        return read;
    }
    let watched = model.reputation.watched;
    let entry = match query {
        FactionQuery::Of(id) => entry_of(model, i64::from(id)).cloned(),
        FactionQuery::Watched => watched.and_then(|slot| {
            model
                .reputation
                .entries
                .iter()
                .find(|e| e.rep_list_id == slot)
                .cloned()
        }),
    };
    FactionRead { entry, watched }
}

/// [`read_faction`] of an entry's faction id, none for 0 and -1.
fn read_entry(lua: &Lua, model: &Model, faction: i64) -> FactionRead {
    match u32::try_from(faction).ok().filter(|&f| f != 0) {
        Some(id) => read_faction(lua, model, FactionQuery::Of(id)),
        None => FactionRead {
            entry: None,
            watched: model.reputation.watched,
        },
    }
}

/// A faction's slot (`0x4d5600`), when it has one below 64 (`0x4d5ffe`-`0x4d6007`).
fn slot_of(lua: &Lua, model: &Model, faction: i64) -> Option<u32> {
    read_entry(lua, model, faction)
        .entry
        .map(|e| e.rep_list_id)
        .filter(|&slot| slot < 64)
}

/// `0x4d6400`: fold or unfold the header of the entry at a 1-based index into the whole list,
/// found by the entry's key (`0x4d6425`-`0x4d6436`), then the sort; an index past the entries
/// (`jae` at `0x4d6406`) or a row that is no header (`0x4d6413`) folds or unfolds every header
/// (`0x4d6476`). A header key no header object holds changes nothing and sorts nothing
/// (`0x4d6489`); `false` then.
///
/// Deviation: the folds are keyed by header, not by the reference's mask positions, whose rebuild
/// folds bit N-1 by registration index before the sort (`0x4d5d49`-`0x4d5d68`) and whose fold
/// verbs search `0xb736c0` before `0x4d5c40` sorts it (`0x4d6400`), so Inactive opens at login
/// whenever a childless header exists and inactivating a header's last child shows it expanded;
/// that is a reference bug, and the folds follow the header the player clicked.
fn set_collapsed(model: &mut Model, index: usize, collapse: bool) -> bool {
    let row = index
        .checked_sub(1)
        .and_then(|n| model.reputation_rows.get(n))
        .filter(|r| r.header)
        .copied();
    let keys: Vec<i64> = match row {
        Some(r) => {
            if !is_header_key(model, r.key) {
                return false;
            }
            vec![r.key]
        }
        None if !collapse => {
            model.reputation_collapsed.clear();
            Vec::new()
        }
        None => model.reputation_headers.iter().map(|h| h.key).collect(),
    };
    for key in keys {
        if collapse {
            model.reputation_collapsed.insert(key);
        } else {
            model.reputation_collapsed.remove(&key);
        }
    }
    sort(model);
    true
}

/// `GetFactionInfo`'s miss (`0x4d5fa0`): eleven values, standingID 1, which the stock pane indexes
/// `FACTION_BAR_COLORS` with unguarded.
fn unknown_faction() -> MultiValue {
    let mut out = vec![Value::Nil, Value::Nil, Value::Integer(1)];
    out.extend(std::iter::repeat_n(Value::Integer(0), 3));
    out.extend(std::iter::repeat_n(Value::Nil, 5));
    debug_assert_eq!(out.len(), 11, "the tuple is eleven wide on every path");
    MultiValue::from_vec(out)
}

/// Hand `call` to the host, which applies it before this returns ([`super::calls::submit`]).
fn submit(lua: &Lua, call: ReputationCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Reputation(call))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumFactions() → the displayed count (`0xb73764`, `0x4d64c6`), 0 before any push.
    g.set(
        "GetNumFactions",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.reputation_visible as i64)
        })?,
    )?;

    // GetFactionInfo(index) → the stock pane's eleven (`ReputationFrame.lua:51`): name,
    // description, standingID, barMin, barMax, barValue, atWarWith, canToggleAtWar, isHeader,
    // isCollapsed, isWatched; eleven wide on every path, a miss included. By the entry's faction
    // id: a faction with a row answers its own values whatever the entry is (`0x4d654f`), its
    // header flag and fold from the header scan (`0x4d6633`-`0x4d66b4`); a registered header with
    // no row (0, -1) its text, standing id 1 and zeros (`0x4d66c6`-`0x4d67d1`).
    g.set(
        "GetFactionInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetFactionInfo(index)` (`0x84a298`, `0x4d6509`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetFactionInfo(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let faction = faction_at(&model, index);
            let is_header = is_header_key(&model, faction);
            let collapsed = is_header && model.reputation_collapsed.contains(&faction);
            let read = read_entry(lua, &model, faction);
            if let Some(e) = read.entry {
                return Ok(MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&e.name)?),
                    Value::String(lua.create_string(&e.description)?),
                    Value::Integer(i64::from(e.standing_id)),
                    Value::Integer(i64::from(e.bar_min)),
                    Value::Integer(i64::from(e.bar_max)),
                    Value::Integer(i64::from(e.standing)),
                    binding_abi::flag(e.at_war),
                    binding_abi::flag(e.can_toggle_at_war),
                    binding_abi::flag(is_header),
                    binding_abi::flag(collapsed),
                    binding_abi::flag(read.watched == Some(e.rep_list_id)),
                ]));
            }
            if !is_header || faction < KEY_INACTIVE {
                return Ok(unknown_faction());
            }
            // The text is read off `_G`, whose `__index` may call any verb: no borrow.
            drop(model);
            let name = synthetic_header(lua, faction);
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                Value::String(lua.create_string("")?), // description
                Value::Integer(1),                     // standingID
                Value::Integer(0),                     // barMin
                Value::Integer(0),                     // barMax
                Value::Integer(0),                     // barValue
                Value::Nil,                            // atWarWith
                Value::Nil,                            // canToggleAtWar
                Value::Integer(1),                     // isHeader
                binding_abi::flag(collapsed),          // isCollapsed
                Value::Nil,                            // isWatched
            ]))
        })?,
    )?;

    // GetSelectedFaction() → the position of the selected faction id among the displayed rows,
    // or 0 (`0x4d6c07`-`0x4d6c30`), as `ReputationFrame.lua:146` tests.
    g.set(
        "GetSelectedFaction",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let shown = model.reputation_visible.min(model.reputation_rows.len());
            let found = model.reputation_rows[..shown]
                .iter()
                .position(|r| r.faction == model.reputation_selected);
            Ok(found.map_or(0, |i| i as i64 + 1))
        })?,
    )?;

    // SetSelectedFaction(index): the faction id of the entry at the index, a header's included,
    // and faction 0 past the entries (`0x4d6be7`, `0x4d6bec`).
    g.set(
        "SetSelectedFaction",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SetSelectedFaction(index)` (`0x84a3b0`, `0x4d6bc1`).
            let index = binding_abi::index_arg(lua, index, "Usage: SetSelectedFaction(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.reputation_selected = faction_at(&model, index);
            Ok(())
        })?,
    )?;

    // CollapseFactionHeader(index) / ExpandFactionHeader(index): a header's index. Both fire
    // `UPDATE_FACTION` from the sort, as the reference's do (`0x4d6a50`, `0x4d6aa0` → `0x4d6400`
    // → `0x4d5c40`, fire site `0x4d5dab`): the stock header's `OnClick` only calls the binding
    // (`ReputationFrame.xml:9-15`), and the pane repaints on `UPDATE_FACTION` alone.
    g.set(
        "CollapseFactionHeader",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: CollapseFactionHeader(index)` (`0x84a320`, `0x4d6a61`).
            let index = binding_abi::index_arg(lua, index, "Usage: CollapseFactionHeader(index)")?;
            let sorted = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                set_collapsed(&mut model, index, true)
            };
            if sorted {
                super::event::fire_global(lua, "UPDATE_FACTION", &[]);
            }
            Ok(())
        })?,
    )?;
    g.set(
        "ExpandFactionHeader",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: ExpandFactionHeader(index)` (`0x84a344`, `0x4d6ab1`).
            let index = binding_abi::index_arg(lua, index, "Usage: ExpandFactionHeader(index)")?;
            let sorted = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                set_collapsed(&mut model, index, false)
            };
            if sorted {
                super::event::fire_global(lua, "UPDATE_FACTION", &[]);
            }
            Ok(())
        })?,
    )?;

    // FactionToggleAtWar(index): for the entry's faction, not faction 0 (`0x4d698e`), ask for war
    // when it is not at war and peace when it is (`0x4d6994`-`0x4d69a3`). `SetFactionAtWar`
    // refuses peace below -3000 (`0x4d5fe8`) and a faction with no slot, both silently, before
    // its combat test; the rest is the call's.
    g.set(
        "FactionToggleAtWar",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: FactionToggleAtWar(index)` (`0x84a2b8`, `0x4d6961`).
            let index = binding_abi::index_arg(lua, index, "Usage: FactionToggleAtWar(index)")?;
            let call = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let faction = faction_at(&model, index);
                (faction != 0)
                    .then(|| {
                        let entry = read_entry(lua, &model, faction).entry;
                        let at_war = !entry.as_ref().is_some_and(|e| e.at_war);
                        // `GetFactionStanding` (`0x4d6370`), 0 for a faction with no slot.
                        let standing = entry.as_ref().map_or(0, |e| e.standing);
                        let floored = !at_war && standing < -3000;
                        let rep_list_id = entry
                            .map(|e| e.rep_list_id)
                            .filter(|&slot| slot < 64 && !floored)?;
                        Some(ReputationCall::AtWar {
                            rep_list_id,
                            at_war,
                        })
                    })
                    .flatten()
            };
            match call {
                Some(call) => submit(lua, call),
                None => Ok(()),
            }
        })?,
    )?;

    // IsFactionInactive(index) (`0x4d6af0` → `0x4d6210`): the inactive bit of the entry's faction's
    // slot, nil without one.
    g.set(
        "IsFactionInactive",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: IsFactionInactive(index)` (`0x84a368`, `0x4d6b01`).
            let index = binding_abi::index_arg(lua, index, "Usage: IsFactionInactive(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let faction = faction_at(&model, index);
            Ok(binding_abi::flag(
                read_entry(lua, &model, faction)
                    .entry
                    .is_some_and(|e| e.inactive),
            ))
        })?,
    )?;

    // SetFactionInactive(index) / SetFactionActive(index) (`0x4d69b0`, `0x4d6a00` → `0x4d60f0`):
    // for the entry's faction, not faction 0 (`0x4d69ee`, `0x4d6a3e`), with a slot below 64; the
    // write, the send and the re-sort with its `UPDATE_FACTION` are the call's.
    for (name, usage, inactive) in [
        (
            "SetFactionInactive",
            "Usage: SetFactionInactive(index)",
            true,
        ),
        ("SetFactionActive", "Usage: SetFactionActive(index)", false),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                // `is-number` else the usage (`0x84a2dc` at `0x4d69c1`, `0x84a300` at `0x4d6a11`).
                let index = binding_abi::index_arg(lua, index, usage)?;
                let slot = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    slot_of(lua, &model, faction_at(&model, index))
                };
                match slot {
                    Some(rep_list_id) => submit(
                        lua,
                        ReputationCall::Inactive {
                            rep_list_id,
                            inactive,
                        },
                    ),
                    None => Ok(()),
                }
            })?,
        )?;
    }

    // GetWatchedFactionInfo() → name, reaction, min, max, value; `reaction` is the standingID
    // scale, and a nil name is `ReputationFrame.lua:167`'s nothing-watched gate.
    g.set(
        "GetWatchedFactionInfo",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let watched = read_faction(lua, &model, FactionQuery::Watched).entry;
            let Some(e) = watched else {
                // Five values here too, `nil, 0, 0, 0, 0`: `0x4d6820`'s four guards share one exit
                // and none raises. `0x4d5620` maps the out-of-range sentinel to 0, so it is refused
                // at the null-row guard, not by a range check.
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&e.name)?),
                Value::Integer(i64::from(e.standing_id)),
                Value::Integer(i64::from(e.bar_min)),
                Value::Integer(i64::from(e.bar_max)),
                Value::Integer(i64::from(e.standing)),
            ]))
        })?,
    )?;

    // SetWatchedFactionIndex(index): the slot of the entry's faction, -1 for one with none and for
    // faction 0, the index 0 among them (`0x4d6b97` → `0x4d6240` → `0x4d5600`). Not local first:
    // the watched slot is a server field with no client mirror, so the bar moves when the
    // descriptor update returns; the call sends only a slot the field does not hold (`0x4d6276`).
    g.set(
        "SetWatchedFactionIndex",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SetWatchedFactionIndex(index)` (`0x84a388`, `0x4d6b71`).
            let index = binding_abi::index_arg(lua, index, "Usage: SetWatchedFactionIndex(index)")?;
            let slot = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                read_entry(lua, &model, faction_at(&model, index))
                    .entry
                    .map_or(-1, |e| e.rep_list_id as i32)
            };
            submit(lua, ReputationCall::Watch(slot))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// An ordinary bar row: visible, not a header. `parent` 0 puts it in the "Other" bucket.
    fn entry(faction_id: u32, rep_list_id: u32, parent_id: u32, name: &str) -> FactionEntry {
        FactionEntry {
            faction_id,
            rep_list_id,
            parent_id,
            name: name.into(),
            description: format!("About the {name}."),
            standing: 4000,
            standing_id: 5,
            bar_min: 3000,
            bar_max: 9000,
            visible: true,
            is_header: false,
            at_war: false,
            can_toggle_at_war: true,
            inactive: false,
        }
    }

    /// A header row as the wire delivers one: flag `0x08` without `VISIBLE`, as the Steamwheedle
    /// Cartel's byte is; a header is never drawn as a bar, so its own visibility is irrelevant.
    fn header(faction_id: u32, rep_list_id: u32, name: &str) -> FactionEntry {
        FactionEntry {
            visible: false,
            is_header: true,
            ..entry(faction_id, rep_list_id, 0, name)
        }
    }

    /// Pushed out of order so the engine's sort shows: Alliance (469) over Stormwind and Ironforge,
    /// Steamwheedle (169) over Booty Bay, and two parentless factions. Bloodsail Buccaneers holds
    /// slot 0, which must never read as "watch nothing".
    fn state() -> ReputationState {
        ReputationState {
            entries: vec![
                entry(72, 19, 469, "Stormwind"),
                entry(529, 13, 0, "Argent Dawn"),
                header(469, 11, "Alliance"),
                entry(21, 1, 169, "Booty Bay"),
                entry(47, 20, 469, "Ironforge"),
                header(169, 10, "Steamwheedle Cartel"),
                entry(87, 0, 0, "Bloodsail Buccaneers"),
            ],
            watched: None,
        }
    }

    fn seated() -> UiScript {
        let mut s = UiScript::new().expect("VM");
        s.set_reputation(state());
        s
    }

    fn names(s: &UiScript) -> Vec<String> {
        s.eval::<Vec<String>>(
            "local t = {} for i = 1, GetNumFactions() do t[i] = (GetFactionInfo(i)) end return t",
        )
        .expect("names")
    }

    /// The fixture's headers lack the visible flag, as on the wire, so this also pins that a
    /// header's own visibility does not gate its group.
    #[test]
    fn the_header_flag_builds_the_tree_and_other_comes_last() {
        let s = seated();
        assert_eq!(
            names(&s),
            [
                "Alliance",
                "Ironforge",
                "Stormwind",
                "Steamwheedle Cartel",
                "Booty Bay",
                "Other",
                "Argent Dawn",
                "Bloodsail Buccaneers",
            ],
            "two named groups sorted by header, then the bucket; children sorted within"
        );
        let (name, _desc, standing_id, _min, _max, _val, at_war, toggle, is_header) = s
            .eval::<(String, String, i64, i64, i64, i64, Value, Value, i64)>(
                "return GetFactionInfo(1)",
            )
            .unwrap();
        assert_eq!(name, "Alliance", "named from the flagged header row");
        assert_eq!(is_header, 1);
        assert_eq!(
            standing_id, 5,
            "a header with a row answers its own standing"
        );
        assert!(at_war.is_nil(), "and its own war state");
        assert_eq!(toggle, Value::Integer(1));
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn a_header_with_no_visible_children_is_dropped() {
        let mut s = UiScript::new().expect("VM");
        let mut st = state();
        // Unmeet both of Alliance's cities; Alliance itself stays in the push.
        for e in &mut st.entries {
            if e.parent_id == 469 {
                e.visible = false;
            }
        }
        s.set_reputation(st);
        assert!(
            !names(&s).iter().any(|n| n == "Alliance"),
            "an empty header is not drawn; got {:?}",
            names(&s)
        );
        assert_eq!(names(&s)[0], "Steamwheedle Cartel");
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn a_bar_row_returns_the_references_eleven_and_so_does_a_miss() {
        let s = seated();
        let got = s
            .eval::<(
                String,
                String,
                i64,
                i64,
                i64,
                i64,
                Value,
                Value,
                Value,
                Value,
                Value,
            )>("return GetFactionInfo(3)")
            .unwrap();
        assert_eq!(got.0, "Stormwind");
        assert_eq!(got.1, "About the Stormwind.");
        assert_eq!(got.2, 5, "standingID is 1-based: FACTION_STANDING_LABEL5");
        assert_eq!((got.3, got.4, got.5), (3000, 9000, 4000), "absolute");
        assert!(got.6.is_nil(), "atWarWith");
        assert_eq!(got.7, Value::Integer(1), "canToggleAtWar");
        assert!(got.8.is_nil() && got.9.is_nil(), "isHeader/isCollapsed");
        assert!(got.10.is_nil(), "isWatched");

        for miss in ["GetFactionInfo(99)", "GetFactionInfo(0)"] {
            let n = s.arity(miss).unwrap();
            assert_eq!(n, 11, "{miss} must still be eleven wide");
            let sid = s
                .eval::<i64>(&format!("local _,_,s = {miss} return s"))
                .unwrap();
            assert_eq!(sid, 1, "{miss}'s standingID slot is the client's 1");
        }
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn folding_shrinks_the_list_and_index_zero_folds_everything() {
        let s = seated();
        s.run("CollapseFactionHeader(1)").unwrap();
        assert_eq!(
            names(&s),
            [
                "Alliance",
                "Steamwheedle Cartel",
                "Booty Bay",
                "Other",
                "Argent Dawn",
                "Bloodsail Buccaneers"
            ]
        );
        assert_eq!(
            s.eval::<Value>("local _,_,_,_,_,_,_,_,_,c = GetFactionInfo(1) return c")
                .unwrap(),
            Value::Integer(1),
            "and reports itself collapsed"
        );
        s.run("ExpandFactionHeader(1)").unwrap();
        assert_eq!(names(&s).len(), 8, "expanded again");

        // A bar row is not a header, so it takes the same fall-through: index 2 is Ironforge.
        s.run("CollapseFactionHeader(2)").unwrap();
        assert_eq!(
            names(&s),
            ["Alliance", "Steamwheedle Cartel", "Other"],
            "a non-header index folds every header"
        );
        // So do index 0 and an index past the end.
        s.run("ExpandFactionHeader(0)").unwrap();
        assert_eq!(names(&s).len(), 8, "index 0 unfolds every header");
        s.run("CollapseFactionHeader(99)").unwrap();
        assert_eq!(names(&s).len(), 3, "so does an out-of-range index");
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn an_inactive_faction_moves_under_the_inactive_header() {
        let mut s = UiScript::new().expect("VM");
        let mut st = state();
        st.entries[1].inactive = true; // Argent Dawn, an "Other" row
        s.set_reputation(st);
        assert_eq!(
            names(&s),
            [
                "Alliance",
                "Ironforge",
                "Stormwind",
                "Steamwheedle Cartel",
                "Booty Bay",
                "Other",
                "Bloodsail Buccaneers",
                "Inactive",
            ],
            "Inactive sorts after Other, and arrives COLLAPSED so its child is not listed"
        );
        // Unfolding it reveals exactly the re-parented row.
        s.run("ExpandFactionHeader(8)").unwrap();
        assert_eq!(names(&s).last().unwrap(), "Argent Dawn");
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    /// The inactive write moves the row, and the pane repaints on `UPDATE_FACTION` alone: the
    /// write raises it after the regroup (`0x4d60f0` → `0x4d5c40`), so a listener already sees the
    /// moved row.
    #[test]
    fn the_inactive_write_raises_update_faction_after_the_regroup() {
        let mut s = seated();
        s.run(
            r#"log = {}
            local f = CreateFrame("Frame") f:RegisterEvent("UPDATE_FACTION")
            f:SetScript("OnEvent", function()
                local n = GetNumFactions()
                tinsert(log, (GetFactionInfo(n)))
            end)"#,
        )
        .unwrap();
        s.set_faction_inactive(19, true); // Stormwind
        assert_eq!(
            s.eval::<Vec<String>>("return log").unwrap(),
            ["Inactive"],
            "heard once, and already regrouped: Stormwind sits under the new last header"
        );
        s.run("ExpandFactionHeader(8) log = {}").unwrap();
        assert_eq!(names(&s).last().unwrap(), "Stormwind");
        s.set_faction_inactive(19, false);
        assert_eq!(
            s.eval::<Vec<String>>("return log").unwrap(),
            ["Bloodsail Buccaneers"],
            "heard once more, and the emptied Inactive header is gone"
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn selection_follows_the_faction_not_the_row_index() {
        let mut s = seated();
        assert_eq!(
            s.eval::<i64>("return GetSelectedFaction()").unwrap(),
            6,
            "the cell starts at faction 0, the Other header's (`0x4d537b`)"
        );
        s.run("SetSelectedFaction(3)").unwrap(); // Stormwind
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 3);
        // Fold Alliance: Stormwind is no longer a visible row, so there is no index to answer.
        s.run("CollapseFactionHeader(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 0);
        s.run("ExpandFactionHeader(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 3);
        // A re-sort that folds the faction under Inactive keeps its id (`0x4d617b`), which no
        // displayed row answers until it is active again.
        s.set_faction_inactive(19, true);
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 0);
        s.set_faction_inactive(19, false);
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 3);
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    /// The war toggle asks for war on a faction at peace and for peace on one at war
    /// (`0x4d6994`-`0x4d69a3`), refuses peace below -3000 (`0x4d5fe8`) and faction 0, and hands
    /// the rest to the host by slot; the header rows are factions too.
    #[test]
    fn the_toggles_hand_their_calls_by_slot() {
        let mut s = seated();
        s.keep_calls();
        s.run("FactionToggleAtWar(3) SetFactionInactive(3) SetFactionActive(1)")
            .unwrap();
        assert_eq!(
            s.take_reputation_calls(),
            [
                ReputationCall::AtWar {
                    rep_list_id: 19,
                    at_war: true
                },
                ReputationCall::Inactive {
                    rep_list_id: 19,
                    inactive: true
                },
                ReputationCall::Inactive {
                    rep_list_id: 11,
                    inactive: false
                },
            ]
        );
        s.set_faction_at_war(19, true);
        s.run("FactionToggleAtWar(3)").unwrap();
        assert_eq!(
            s.take_reputation_calls(),
            [ReputationCall::AtWar {
                rep_list_id: 19,
                at_war: false
            }]
        );
        let mut hated = state();
        hated.entries[0].at_war = true;
        hated.entries[0].standing = -3001;
        s.set_reputation(hated);
        s.run("FactionToggleAtWar(3) FactionToggleAtWar(0) FactionToggleAtWar(6)")
            .unwrap();
        assert!(
            s.take_reputation_calls().is_empty(),
            "peace below -3000, faction 0 and the Other header ask nothing"
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn watching_hands_the_slot_but_does_not_move_the_bar_itself() {
        let mut s = seated();
        s.keep_calls();
        assert!(
            s.eval::<Value>("return GetWatchedFactionInfo()")
                .unwrap()
                .is_nil(),
            "nothing watched → a nil name, which stock `if ( name )` gates on"
        );

        s.run("SetWatchedFactionIndex(3)").unwrap(); // Stormwind
        assert!(
            s.eval::<Value>("return GetWatchedFactionInfo()")
                .unwrap()
                .is_nil(),
            "still nothing: the server owns this field, so the click alone cannot move the bar"
        );
        assert_eq!(s.take_reputation_calls(), [ReputationCall::Watch(19)]);

        // The descriptor update arrives as the next push, and now the bar reads.
        let mut watched = state();
        watched.watched = Some(19);
        s.set_reputation(watched);
        assert_eq!(
            s.eval::<(String, i64, i64, i64, i64)>("return GetWatchedFactionInfo()")
                .unwrap(),
            ("Stormwind".into(), 5, 3000, 9000, 4000)
        );
        assert_eq!(
            s.eval::<Value>("local _,_,_,_,_,_,_,_,_,_,w = GetFactionInfo(3) return w")
                .unwrap(),
            Value::Integer(1),
            "and the row reports itself watched"
        );

        // Slot 0, then the index 0 and the Other header, faction 0 and no slot: -1.
        s.run("SetWatchedFactionIndex(8) SetWatchedFactionIndex(0) SetWatchedFactionIndex(6)")
            .unwrap(); // Bloodsail Buccaneers, rep slot 0
        assert_eq!(
            s.take_reputation_calls(),
            [
                ReputationCall::Watch(0),
                ReputationCall::Watch(-1),
                ReputationCall::Watch(-1)
            ]
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    /// The index verbs read the whole entry list (`0x4d5fa0`, bounded by the entry count
    /// `0xb73760`), the folded children after the displayed rows. The list is the rebuild's
    /// (`0x4d5430`): the met rows from the last `Faction.dbc` row to the first, each followed by
    /// the header its team first needs (`0x4d5b87`), then Inactive; the sort (`0x4d5c40`) is the
    /// CRT's (`0x73f727`), where two folded children of different headers compare equal
    /// (`0x4d5f02`). Emulated on the bytes' rules, folding every header leaves Inactive (shown
    /// by no count, `0x4d5d86`), then Argent Dawn, Bloodsail Buccaneers, Ironforge, Stormwind and
    /// Booty Bay, not their display order.
    #[test]
    fn the_index_verbs_read_the_folded_entries_in_the_order_the_sort_leaves() {
        let s = seated();
        s.run("CollapseFactionHeader(0)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumFactions()").unwrap(), 3);
        let all = s
            .eval::<Vec<String>>(
                "local t = {} for i = 1, 9 do t[i] = (GetFactionInfo(i)) or '-' end return t",
            )
            .unwrap();
        assert_eq!(
            all,
            [
                "Alliance",
                "Steamwheedle Cartel",
                "Other",
                "Inactive",
                "Argent Dawn",
                "Bloodsail Buccaneers",
                "Ironforge",
                "Stormwind",
                "Booty Bay",
            ]
        );
        // A folded child takes the verbs by its index past the shown rows.
        s.run("SetSelectedFaction(8)").unwrap();
        s.run("ExpandFactionHeader(0)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetSelectedFaction()").unwrap(),
            3,
            "Stormwind, shown again"
        );
    }

    /// The world-enter rebuild sorts twice, the Inactive header's fold (`0x4d55d9` → `0x4d6457`)
    /// and the sort after it (`0x4d55de`), and each sort signals `UPDATE_FACTION` (`0x4d5dab`).
    #[test]
    fn the_world_enter_rebuild_signals_each_of_its_two_sorts() {
        let mut s = seated();
        s.run(
            "N = 0 local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_FACTION') \
             f:SetScript('OnEvent', function() N = N + 1 end)",
        )
        .unwrap();
        s.reputation_enter_world();
        assert_eq!(s.eval::<i64>("return N").unwrap(), 2);
    }

    /// `SMSG_SET_FACTION_ATWAR` writes the bit and sorts nothing (`0x4d56b0`), so the rows past
    /// the shown ones stay where the last sort left them; a sort would move them, as the CRT's
    /// `qsort` reorders rows its comparator ties (`0x4d5f02`).
    #[test]
    fn an_at_war_push_leaves_the_folded_rows_where_they_were() {
        let mut s = seated();
        s.run("CollapseFactionHeader(0)").unwrap();
        let tail = |s: &UiScript| {
            s.eval::<Vec<String>>(
                "local t = {} for i = 1, 9 do t[i] = (GetFactionInfo(i)) or '-' end return t",
            )
            .unwrap()
        };
        let before = tail(&s);
        let mut war = state();
        for e in &mut war.entries {
            if e.faction_id == 87 {
                e.at_war = true;
            }
        }
        s.set_reputation(war);
        assert_eq!(tail(&s), before);
    }

    /// The first push builds the list, and the rebuild clears it first (`0x4d547f`-`0x4d5493`), so
    /// the messages queued before it are gone: no sort of theirs reorders the folded rows under
    /// Inactive (`0x4d5479`), whose ties the CRT's `qsort` moves (`0x4d5f02`).
    #[test]
    fn the_first_push_drops_the_list_ops_queued_before_it() {
        let mut st = state();
        for e in &mut st.entries {
            if !e.is_header {
                e.inactive = true;
            }
        }
        let rows = |ops: Vec<ReputationListOp>| {
            let mut s = UiScript::new().expect("VM");
            s.push_reputation(Some(st.clone()), ops);
            s.eval::<Vec<String>>(
                "local t = {} for i = 1, 9 do t[i] = (GetFactionInfo(i)) or '-' end return t",
            )
            .unwrap()
        };
        let built = rows(Vec::new());
        assert_eq!(
            rows(vec![
                ReputationListOp::Rebuild,
                ReputationListOp::Edit {
                    listed: vec![19],
                    sort: true
                },
                ReputationListOp::Edit {
                    listed: vec![],
                    sort: true
                },
            ]),
            built
        );
    }

    /// Each `SMSG_SET_FACTION_VISIBLE` adds its row and sorts (`0x4d5738`, `0x4d574a`), so two
    /// in one frame sort twice, in their order. Emulated on the bytes' rules: with every header
    /// folded, Ironforge then the Bloodsail Buccaneers leave Booty Bay, Ironforge, Stormwind,
    /// Argent Dawn and the Bloodsail Buccaneers past the shown rows.
    #[test]
    fn each_visible_message_adds_and_sorts_in_its_order() {
        let mut s = UiScript::new().expect("VM");
        let mut st = state();
        for e in &mut st.entries {
            if e.faction_id == 47 || e.faction_id == 87 {
                e.visible = false;
            }
        }
        s.set_reputation(st);
        s.run("CollapseFactionHeader(0)").unwrap();
        s.set_reputation(state());
        s.apply_reputation_ops(vec![
            ReputationListOp::Edit {
                listed: vec![20],
                sort: true,
            },
            ReputationListOp::Edit {
                listed: vec![0],
                sort: true,
            },
        ]);
        let all = s
            .eval::<Vec<String>>(
                "local t = {} for i = 1, 9 do t[i] = (GetFactionInfo(i)) or '-' end return t",
            )
            .unwrap();
        assert_eq!(
            all,
            [
                "Alliance",
                "Steamwheedle Cartel",
                "Other",
                "Inactive",
                "Booty Bay",
                "Ironforge",
                "Stormwind",
                "Argent Dawn",
                "Bloodsail Buccaneers",
            ]
        );
    }

    /// Deviation pinned: an index one past the last entry names faction 0, as one further does,
    /// which the Other header answers.
    #[test]
    fn an_index_at_the_entry_count_reads_as_one_past_it() {
        let s = seated();
        assert_eq!(
            s.eval::<String>("return (GetFactionInfo(10))").unwrap(),
            s.eval::<String>("return (GetFactionInfo(11))").unwrap(),
        );
        assert_eq!(
            s.eval::<String>("return (GetFactionInfo(10))").unwrap(),
            "Other",
            "faction 0, which the Other header answers"
        );
    }

    /// The synthetic header's text is a `_G` read (`0x703bf0`, a `lua_gettable`), so an `__index`
    /// that calls a verb runs inside `GetFactionInfo`; the Model must be free for it.
    #[test]
    fn a_synthetic_header_reads_its_string_with_the_model_free() {
        let s = seated();
        s.run(
            r#"setmetatable(getfenv(0), { __index = function(t, k)
                   if k == "FACTION_OTHER" then
                       CreateFrame("Frame"):SetWidth(10)
                       return "Elsewhere"
                   end
               end })"#,
        )
        .unwrap();
        assert_eq!(names(&s)[5], "Elsewhere");
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    /// A host that answers the family's questions from its own rows, as the app's reads the
    /// slots: the faction's row, the watched slot's, and the whole book.
    struct Slots {
        /// Stormwind's row as the host has it, with the watched slot beside it.
        stormwind: FactionEntry,
        watched: Option<u32>,
        asked: std::cell::Cell<usize>,
    }

    impl crate::script::CallHost for Slots {
        fn apply(
            &mut self,
            _: &mut UiScript,
            _: crate::script::ScriptCall,
        ) -> Option<crate::script::CallTail> {
            None
        }

        fn read_live(&mut self, ask: &mut crate::script::live::LiveAsk<'_>) -> bool {
            self.asked.set(self.asked.get() + 1);
            let row = |q: &FactionQuery| match q {
                FactionQuery::Of(72) => Some(self.stormwind.clone()),
                FactionQuery::Watched => self
                    .watched
                    .filter(|&w| w == self.stormwind.rep_list_id)
                    .map(|_| self.stormwind.clone()),
                FactionQuery::Of(_) => None,
            };
            ask.answer(|q: &FactionQuery| {
                Some(FactionRead {
                    entry: row(q),
                    watched: self.watched,
                })
            }) || ask.answer(|_: &ReputationBook| {
                let mut state = state();
                for e in &mut state.entries {
                    if e.faction_id == 72 {
                        *e = self.stormwind.clone();
                    }
                }
                state.watched = self.watched;
                Some(Book {
                    state,
                    standings: HashMap::new(),
                })
            })
        }
    }

    fn slots(standing: i32, inactive: bool) -> Slots {
        Slots {
            stormwind: FactionEntry {
                standing,
                standing_id: 6,
                at_war: true,
                inactive,
                ..entry(72, 19, 469, "Stormwind")
            },
            watched: Some(19),
            asked: Default::default(),
        }
    }

    /// With a host lent the values come from its answers, not the pushed snapshot: the standing, the
    /// at-war and watched bits, the inactive bit, and the watched faction; a faction it has no row
    /// for reads as a miss.
    #[test]
    fn the_getters_read_the_lent_hosts_rows_over_the_snapshot() {
        let mut s = seated();
        let mut host = slots(9100, true);
        s.hosted(&mut host, |s| {
            let (value, id, war, watched) = s
                .eval::<(i64, i64, Value, Value)>(
                    "local _, _, id, _, _, v, war, _, _, _, w = GetFactionInfo(3) return v, id, war, w",
                )
                .unwrap();
            assert_eq!((value, id), (9100, 6), "the standing the host holds");
            assert_eq!(war, Value::Integer(1));
            assert_eq!(watched, Value::Integer(1), "the watched slot is the host's");
            assert_eq!(
                s.eval::<Value>("return IsFactionInactive(3)").unwrap(),
                Value::Integer(1)
            );
            assert_eq!(
                s.eval::<(String, i64, i64)>(
                    "local n, r, _, _, v = GetWatchedFactionInfo() return n, r, v"
                )
                .unwrap(),
                ("Stormwind".to_string(), 6, 9100)
            );
            assert_eq!(
                s.eval::<Value>("local _, _, id = GetFactionInfo(4) return id").unwrap(),
                Value::Integer(1),
                "a row the host has none for answers a miss"
            );
        });
        assert!(host.asked.get() > 0);
        let snapshot = s
            .eval::<i64>("local _, _, _, _, _, v = GetFactionInfo(3) return v")
            .unwrap();
        assert_eq!(snapshot, 4000, "with no host the pushed snapshot answers");
    }

    /// The list's ops are applied over the book the host answers as of there: a row the host holds
    /// inactive sorts under Inactive though the snapshot pushed earlier held it active.
    #[test]
    fn the_ops_sort_over_the_book_as_of_their_place() {
        let mut s = seated();
        let before = names(&s);
        let mut host = slots(4000, true);
        s.hosted(&mut host, |s| {
            s.apply_reputation_ops(vec![ReputationListOp::Edit {
                listed: vec![],
                sort: true,
            }]);
        });
        let after = names(&s);
        assert!(before.iter().any(|n| n == "Stormwind"), "{before:?}");
        assert!(
            !after.iter().any(|n| n == "Stormwind"),
            "folded away under the collapsed Inactive header: {after:?}"
        );
    }
}
