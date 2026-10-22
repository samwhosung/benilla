//! The reputation pane's twelve `ReputationFrame` globals: the app pushes a flat snapshot, and
//! the engine groups, sorts and folds it as the reference does (`[0x4d5200, 0x4d6d80)`); every
//! index the API takes or returns is 1-based into the visible rows. A row is a header by flag
//! `0x08` (`0x4d5acb`), which emulators name `INVISIBLE_FORCED`, and files under its
//! `Faction.dbc` `team`, or under `-1` while inactive (flag `0x20`); key `0` reads `FACTION_OTHER`
//! and `-1` `FACTION_INACTIVE`. A childless header is not a row (`GetNumFactions`, `0xb73764`).
//! Headers sort by name with the synthetic keys last, `0` first (`0x4d5dc0`); children sort by
//! name (`0x4d5e70`).
//!
//! Each verb resolves its index to the entry's faction id (`0x4d5fa0`), a header's included: its
//! own id, or 0 and -1 for the two synthetic ones; an index past the visible rows names faction 0
//! here. The reference bounds the index by the total entry count instead (`0xb73760`, `jbe` in
//! `0x4d5fa0`, `jae` in `0x4d6400`), folded entries included, which this does not do yet. The
//! selection holds such an id (`0xb73768`). The folds change at a rebuild (all open, Inactive
//! folded, `0x4d5479`, `0x4d55d9`), at a header's registration (all open, `0x4d5c34`) and by the
//! two fold verbs alone. The verbs that send do so inside their call ([`ReputationCall`]).

use std::collections::{HashMap, HashSet};

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// The synthetic header key for factions the player moved to the inactive bucket.
const KEY_INACTIVE: i64 = -1;
/// The synthetic header key for factions with no parent, the "Other" bucket.
const KEY_OTHER: i64 = 0;

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

/// One header group; `pub(crate)` only because [`super::model::Model`] stores them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FactionGroup {
    /// A `Faction.dbc` id, or [`KEY_OTHER`] or [`KEY_INACTIVE`].
    key: i64,
    /// The header row's text; empty for the two synthetic keys, resolved against the VM's global
    /// strings when read.
    name: String,
    /// Positions into [`ReputationState::entries`], sorted by [`collate`]d name.
    entries: Vec<usize>,
}

/// One visible row: a header (its group index) or a faction bar (its position in
/// [`ReputationState::entries`]).
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Entry(usize),
}

impl super::UiScript {
    /// Push the reputation snapshot and regroup it. The folds stay, as no standing or flag message
    /// writes the mask (`0x4d5760`, `0x4d56b0`, `0x4d5710`), unless the push lists a header key not
    /// registered before: its registration opens every header (`0x4d5c34`), Inactive's included.
    /// The first push into a VM is the rebuild's ([`Self::reputation_enter_world`]).
    pub fn set_reputation(&mut self, state: ReputationState) {
        let mut model = self.model_mut();
        let keys = header_keys(&state.entries);
        if model.reputation_headers.is_empty() {
            // The rebuild: the mask all ones (`0x4d5479`), then the Inactive header added and
            // folded (`0x4d55d9`).
            model.reputation_headers = keys;
            model.reputation_headers.insert(KEY_INACTIVE);
            model.reputation_collapsed.clear();
            model.reputation_collapsed.insert(KEY_INACTIVE);
        } else if !keys.is_subset(&model.reputation_headers) {
            model.reputation_headers.extend(keys);
            model.reputation_collapsed.clear();
        }
        model.reputation_groups = build_groups(&state.entries);
        model.reputation = state;
    }

    /// The world-enter cascade's rebuild (`0x4909d3` calls `0x4d5430`): the player in the world,
    /// it re-lists the factions with every header expanded and "Inactive" collapsed (`0x4d5479`),
    /// and its sort signals `UPDATE_FACTION` (`0x4d55de` → `0x4d5db0`).
    pub(crate) fn reputation_enter_world(&mut self) {
        let state = {
            let mut model = self.model_mut();
            model.reputation_headers.clear();
            std::mem::take(&mut model.reputation)
        };
        self.set_reputation(state);
        self.fire_event("UPDATE_FACTION", Vec::new());
    }

    /// Push every reputation-indexed faction's standing, the base plus the wire (`0x4d6370`): what
    /// the quest log's turn-in test reads (`0x4df6ce`, `0x4df6e3`). A faction left out reads 0, as
    /// one with no reputation index does (`0x4d6377`).
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
        {
            let mut model = self.model_mut();
            let model = &mut *model;
            for e in model.reputation.entries.iter_mut() {
                if e.rep_list_id == rep_list_id {
                    e.inactive = inactive;
                }
            }
            model.reputation_groups = build_groups(&model.reputation.entries);
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

fn header_key(e: &FactionEntry) -> i64 {
    if e.inactive {
        KEY_INACTIVE
    } else {
        i64::from(e.parent_id)
    }
}

/// The header keys the listed rows register (`0x4d5b0d`-`0x4d5bdc`): a header row its own id, any
/// other row its team's, inactive or not.
fn header_keys(entries: &[FactionEntry]) -> HashSet<i64> {
    entries
        .iter()
        .filter(|e| e.visible)
        .map(|e| {
            if e.is_header {
                i64::from(e.faction_id)
            } else {
                i64::from(e.parent_id)
            }
        })
        .collect()
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
/// Case-insensitive name order, raw bytes breaking a tie: the client's `stricmp`.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.as_bytes().cmp(b.as_bytes()))
}

/// Build the header groups: the visible non-header rows by [`header_key`], sorted by name. A group
/// is named by its pushed header row (the reference synthesizes a missing one, `0x4d5a70`), or left
/// empty for a synthetic key; a childless header is no group, as the client's count drops it.
fn build_groups(entries: &[FactionEntry]) -> Vec<FactionGroup> {
    let name_of: HashMap<i64, &str> = entries
        .iter()
        .filter(|e| e.is_header)
        .map(|e| (i64::from(e.faction_id), e.name.as_str()))
        .collect();

    let mut by_key: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if e.visible && !e.is_header {
            by_key.entry(header_key(e)).or_default().push(i);
        }
    }

    let mut groups: Vec<FactionGroup> = by_key
        .into_iter()
        .map(|(key, mut children)| {
            children.sort_by(|&a, &b| collate(&entries[a].name, &entries[b].name));
            FactionGroup {
                key,
                name: name_of.get(&key).map_or(String::new(), |n| (*n).into()),
                entries: children,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        let synthetic = |k: i64| k <= KEY_OTHER;
        match (synthetic(a.key), synthetic(b.key)) {
            // Larger raw key first among the synthetics: 0 "Other" before -1 "Inactive".
            (true, true) => b.key.cmp(&a.key),
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => collate(&a.name, &b.name),
        }
    });
    groups
}

fn rows(model: &Model) -> Vec<Row> {
    let mut out = Vec::new();
    for (gi, g) in model.reputation_groups.iter().enumerate() {
        out.push(Row::Header(gi));
        if !model.reputation_collapsed.contains(&g.key) {
            for &ei in &g.entries {
                out.push(Row::Entry(ei));
            }
        }
    }
    out
}

fn num_rows(model: &Model) -> usize {
    rows(model).len()
}

/// The faction id of the entry at a 1-based index into the visible rows (`0x4d5fa0`): a bar's own,
/// a header's key, and 0 past them. The reference's bound is the total entry count (`0xb73760`),
/// which reaches the folded entries too; that is not modelled yet.
fn faction_at(model: &Model, index: usize) -> i64 {
    match index
        .checked_sub(1)
        .and_then(|n| rows(model).get(n).copied())
    {
        Some(Row::Header(gi)) => model.reputation_groups[gi].key,
        Some(Row::Entry(ei)) => i64::from(model.reputation.entries[ei].faction_id),
        None => 0,
    }
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

/// A faction's slot (`0x4d5600`), when it has one below 64 (`0x4d5ffe`-`0x4d6007`).
fn slot_of(model: &Model, faction: i64) -> Option<u32> {
    entry_of(model, faction)
        .map(|e| e.rep_list_id)
        .filter(|&slot| slot < 64)
}

/// Fold or unfold by 1-based visible index; `0` or any non-header index acts on every header, the
/// reference's own fall-through (`0x4d6a50`, `0x4d6aa0`).
///
/// Deviation: the folds are keyed by header, not by the reference's mask positions, whose rebuild
/// folds bit N-1 by registration index before the sort (`0x4d5d49`-`0x4d5d68`) and whose fold
/// verbs search `0xb736c0` before `0x4d5c40` sorts it (`0x4d6400`), so Inactive opens at login
/// whenever a childless header exists and inactivating a header's last child shows it expanded;
/// that is a reference bug, and the folds follow the header the player clicked.
fn set_collapsed(model: &mut Model, index: usize, collapse: bool) {
    let one = index
        .checked_sub(1)
        .and_then(|n| rows(model).get(n).copied())
        .and_then(|row| match row {
            Row::Header(gi) => model.reputation_groups.get(gi).map(|g| g.key),
            Row::Entry(_) => None,
        });
    let keys: Vec<i64> = match one {
        Some(key) => vec![key],
        None => model.reputation_groups.iter().map(|g| g.key).collect(),
    };
    for key in keys {
        if collapse {
            model.reputation_collapsed.insert(key);
        } else {
            model.reputation_collapsed.remove(&key);
        }
    }
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

    // GetNumFactions() → the visible row count (0 before any push).
    g.set(
        "GetNumFactions",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(num_rows(&model) as i64)
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
            let is_header = model.reputation_headers.contains(&faction);
            let collapsed = is_header && model.reputation_collapsed.contains(&faction);
            if let Some(e) = entry_of(&model, faction) {
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
                    binding_abi::flag(model.reputation.watched == Some(e.rep_list_id)),
                ]));
            }
            if !is_header || faction < KEY_INACTIVE {
                return Ok(unknown_faction());
            }
            let own_name = model
                .reputation_groups
                .iter()
                .find(|g| g.key == faction)
                .map(|g| g.name.clone())
                .unwrap_or_default();
            // The text is read off `_G`, whose `__index` may call any verb: no borrow.
            drop(model);
            let name = if own_name.is_empty() {
                synthetic_header(lua, faction)
            } else {
                own_name
            };
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

    // GetSelectedFaction() → the position of the selected faction id among the visible rows, or 0
    // (`0x4d6c07`-`0x4d6c30`), as `ReputationFrame.lua:146` tests.
    g.set(
        "GetSelectedFaction",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let found = rows(&model).iter().position(|r| match r {
                Row::Header(gi) => model.reputation_groups[*gi].key == model.reputation_selected,
                Row::Entry(ei) => {
                    i64::from(model.reputation.entries[*ei].faction_id) == model.reputation_selected
                }
            });
            Ok(found.map_or(0, |i| i as i64 + 1))
        })?,
    )?;

    // SetSelectedFaction(index): the faction id of the entry at the index, a header's included,
    // and faction 0 past the rows (`0x4d6be7`, `0x4d6bec`).
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

    // CollapseFactionHeader(index) / ExpandFactionHeader(index): a visible header index. Both fire
    // `UPDATE_FACTION`, as the reference's do (`0x4d6a50`, `0x4d6aa0` → `0x4d6400` → `0x4d5c40`,
    // fire site `0x4d5dab`): the stock header's `OnClick` only calls the binding
    // (`ReputationFrame.xml:9-15`), and the pane repaints on `UPDATE_FACTION` alone.
    g.set(
        "CollapseFactionHeader",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: CollapseFactionHeader(index)` (`0x84a320`, `0x4d6a61`).
            let index = binding_abi::index_arg(lua, index, "Usage: CollapseFactionHeader(index)")?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                set_collapsed(&mut model, index, true);
            }
            super::event::fire_global(lua, "UPDATE_FACTION", &[]);
            Ok(())
        })?,
    )?;
    g.set(
        "ExpandFactionHeader",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: ExpandFactionHeader(index)` (`0x84a344`, `0x4d6ab1`).
            let index = binding_abi::index_arg(lua, index, "Usage: ExpandFactionHeader(index)")?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                set_collapsed(&mut model, index, false);
            }
            super::event::fire_global(lua, "UPDATE_FACTION", &[]);
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
                        let entry = entry_of(&model, faction);
                        let at_war = !entry.is_some_and(|e| e.at_war);
                        // `GetFactionStanding` (`0x4d6370`), 0 for a faction with no slot.
                        let standing = entry.map_or(0, |e| e.standing);
                        let floored = !at_war && standing < -3000;
                        let rep_list_id = slot_of(&model, faction).filter(|_| !floored)?;
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
                entry_of(&model, faction).is_some_and(|e| e.inactive),
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
                    slot_of(&model, faction_at(&model, index))
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
            let watched = model.reputation.watched.and_then(|slot| {
                model
                    .reputation
                    .entries
                    .iter()
                    .find(|e| e.rep_list_id == slot)
            });
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
                entry_of(&model, faction_at(&model, index)).map_or(-1, |e| e.rep_list_id as i32)
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
        // A push that drops the faction keeps its id, which no row answers, until it is back.
        let mut without = state();
        without.entries.retain(|e| e.faction_id != 72);
        s.set_reputation(without);
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 0);
        s.set_reputation(state());
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
}
