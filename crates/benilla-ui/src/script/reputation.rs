//! The reputation pane's twelve `ReputationFrame` globals: the app pushes a flat snapshot, and
//! the engine groups, sorts and folds it as the reference does (`[0x4d5200, 0x4d6d80)`); every
//! index the API takes or returns is 1-based into the visible rows. A row is a header by flag
//! `0x08` (`0x4d5acb`), which emulators name `INVISIBLE_FORCED`, and files under its
//! `Faction.dbc` `team`, or under `-1` while inactive (flag `0x20`); key `0` reads `FACTION_OTHER`
//! and `-1` `FACTION_INACTIVE`. A childless header is not a row (`GetNumFactions`, `0xb73764`).
//! Headers sort by name with the synthetic keys last, `0` first (`0x4d5dc0`); children sort by
//! name (`0x4d5e70`). The at-war toggle also refuses in combat and applies the -3000 floor only
//! toward peace (`0x4d5fd0`); only `canToggleAtWar`, what the pane reports, lives here.

use std::collections::HashMap;

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
    /// `reputationIndex`: the slot every send addresses and the selection is held by.
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
    /// Flag `0x02`; the toggle flips it here, unacked.
    pub at_war: bool,
    /// Flag `0x10` clear and `standing >= -3000`: what `GetFactionInfo` reports.
    pub can_toggle_at_war: bool,
    /// Flag `0x20`; flipped here like [`Self::at_war`], it moves the row under "Inactive" rather
    /// than hiding it.
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

/// One outbound reputation verb the pane queued, drained by the app into its `WorldWriter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReputationSend {
    /// `CMSG_SET_FACTION_ATWAR`.
    AtWar { rep_list_id: u32, at_war: bool },
    /// `CMSG_SET_FACTION_INACTIVE`.
    Inactive { rep_list_id: u32, inactive: bool },
    /// `CMSG_SET_WATCHED_FACTION`; `None` is the wire's `-1`, not slot 0.
    Watch(Option<u32>),
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
    /// Push the reputation snapshot and rebuild the tree: as in the reference, whose fold mask
    /// (`0x84a0a4`) holds expanded bits, every header expands and then "Inactive" collapses, so a
    /// fold does not survive a standing tick. The selection is kept by slot.
    pub fn set_reputation(&mut self, state: ReputationState) {
        let mut model = self.model_mut();
        let groups = build_groups(&state.entries);
        if let Some(slot) = model.reputation_selected {
            if !state.entries.iter().any(|e| e.rep_list_id == slot) {
                model.reputation_selected = None;
            }
        }
        model.reputation_collapsed.clear();
        model.reputation_collapsed.insert(KEY_INACTIVE);
        model.reputation_groups = groups;
        model.reputation = state;
    }

    /// Drain the reputation verbs the pane queued since the last call.
    pub fn take_reputation_sends(&mut self) -> Vec<ReputationSend> {
        std::mem::take(&mut self.model_mut().reputation_sends)
    }
}

fn header_key(e: &FactionEntry) -> i64 {
    if e.inactive {
        KEY_INACTIVE
    } else {
        i64::from(e.parent_id)
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

fn entry_at(model: &Model, index: usize) -> Option<&FactionEntry> {
    match rows(model).get(index.checked_sub(1)?)? {
        Row::Entry(ei) => model.reputation.entries.get(*ei),
        Row::Header(_) => None,
    }
}

fn entry_at_mut(model: &mut Model, index: usize) -> Option<&mut FactionEntry> {
    let row = *rows(model).get(index.checked_sub(1)?)?;
    match row {
        Row::Entry(ei) => model.reputation.entries.get_mut(ei),
        Row::Header(_) => None,
    }
}

/// Fold or unfold by 1-based visible index; `0` or any non-header index acts on every header, the
/// reference's own fall-through (`0x4d6a50`, `0x4d6aa0`).
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
    // isCollapsed, isWatched; eleven wide on every path, a miss included.
    g.set(
        "GetFactionInfo",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(row) = index
                .checked_sub(1)
                .and_then(|n| rows(&model).get(n).copied())
            else {
                return Ok(unknown_faction());
            };
            match row {
                Row::Header(gi) => {
                    let grp = &model.reputation_groups[gi];
                    let collapsed = model.reputation_collapsed.contains(&grp.key);
                    let name = if grp.name.is_empty() {
                        synthetic_header(lua, grp.key)
                    } else {
                        grp.name.clone()
                    };
                    Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&name)?),
                        Value::String(lua.create_string("")?), // description
                        Value::Integer(0),                     // standingID
                        Value::Integer(0),                     // barMin
                        Value::Integer(0),                     // barMax
                        Value::Integer(0),                     // barValue
                        Value::Nil,                            // atWarWith
                        Value::Nil,                            // canToggleAtWar
                        Value::Integer(1),                     // isHeader
                        binding_abi::flag(collapsed),          // isCollapsed
                        Value::Nil,                            // isWatched
                    ]))
                }
                Row::Entry(ei) => {
                    let e = &model.reputation.entries[ei];
                    Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&e.name)?),
                        Value::String(lua.create_string(&e.description)?),
                        Value::Integer(i64::from(e.standing_id)),
                        Value::Integer(i64::from(e.bar_min)),
                        Value::Integer(i64::from(e.bar_max)),
                        Value::Integer(i64::from(e.standing)),
                        binding_abi::flag(e.at_war),
                        binding_abi::flag(e.can_toggle_at_war),
                        Value::Nil, // isHeader
                        Value::Nil, // isCollapsed
                        binding_abi::flag(model.reputation.watched == Some(e.rep_list_id)),
                    ]))
                }
            }
        })?,
    )?;

    // GetSelectedFaction() → the selected row's visible index, or 0 for none, as
    // `ReputationFrame.lua:146` tests; re-found each call, since the selection is held by slot.
    g.set(
        "GetSelectedFaction",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(slot) = model.reputation_selected else {
                return Ok(0i64);
            };
            let found = rows(&model).iter().position(|r| match r {
                Row::Entry(ei) => model.reputation.entries[*ei].rep_list_id == slot,
                Row::Header(_) => false,
            });
            Ok(found.map_or(0, |i| i as i64 + 1))
        })?,
    )?;

    // SetSelectedFaction(index): a visible index; a header or an out-of-range index clears it.
    g.set(
        "SetSelectedFaction",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.reputation_selected = entry_at(&model, index).map(|e| e.rep_list_id);
            Ok(())
        })?,
    )?;

    // CollapseFactionHeader(index) / ExpandFactionHeader(index): a visible header index. Both fire
    // `UPDATE_FACTION`, as the reference's do (`0x4d6a50`, `0x4d6aa0` → `0x4d6400` → `0x4d5c40`,
    // fire site `0x4d5dab`): the stock header's `OnClick` only calls the binding
    // (`ReputationFrame.xml:9-15`), and the pane repaints on `UPDATE_FACTION` alone.
    g.set(
        "CollapseFactionHeader",
        lua.create_function(|lua, index: usize| {
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
        lua.create_function(|lua, index: usize| {
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                set_collapsed(&mut model, index, false);
            }
            super::event::fire_global(lua, "UPDATE_FACTION", &[]);
            Ok(())
        })?,
    )?;

    // FactionToggleAtWar(index): flip the at-war bit and queue the send; `0x4d6950` writes locally
    // first, and nothing acks it.
    g.set(
        "FactionToggleAtWar",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let Some(e) = entry_at_mut(&mut model, index) else {
                return Ok(());
            };
            if !e.can_toggle_at_war {
                return Ok(());
            }
            e.at_war = !e.at_war;
            let send = ReputationSend::AtWar {
                rep_list_id: e.rep_list_id,
                at_war: e.at_war,
            };
            model.reputation_sends.push(send);
            Ok(())
        })?,
    )?;

    // IsFactionInactive / SetFactionInactive / SetFactionActive (`0x4d69b0`, `0x4d6a00`): the flip
    // is local first too, and moves the row under "Inactive", so the tree regroups here
    // (`0x4d69b0` calls `0x4d5c40`); the folds survive, as only a server rebuild resets them.
    g.set(
        "IsFactionInactive",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(
                entry_at(&model, index).is_some_and(|e| e.inactive),
            ))
        })?,
    )?;
    for (name, inactive) in [("SetFactionInactive", true), ("SetFactionActive", false)] {
        g.set(
            name,
            lua.create_function(move |lua, index: usize| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let Some(e) = entry_at_mut(&mut model, index) else {
                    return Ok(());
                };
                e.inactive = inactive;
                let send = ReputationSend::Inactive {
                    rep_list_id: e.rep_list_id,
                    inactive,
                };
                model.reputation_sends.push(send);
                let groups = build_groups(&model.reputation.entries);
                model.reputation_groups = groups;
                Ok(())
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

    // SetWatchedFactionIndex(index): a visible index, 0 to stop watching. Not local first
    // (`0x4d6b60`): the watched slot is a server field with no client mirror, so the bar moves when
    // the descriptor update returns. The app sends none as `-1`, since slot 0 is a real faction.
    g.set(
        "SetWatchedFactionIndex",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let slot = entry_at(&model, index).map(|e| e.rep_list_id);
            model.reputation_sends.push(ReputationSend::Watch(slot));
            Ok(())
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
        assert_eq!(standing_id, 0, "a header carries no standing");
        assert!(at_war.is_nil() && toggle.is_nil(), "nor any war state");
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
    fn a_push_resets_the_folds() {
        let mut s = seated();
        s.run("CollapseFactionHeader(1)").unwrap();
        assert_eq!(names(&s).len(), 6, "folded");
        let mut ticked = state();
        ticked.entries[0].standing = 12345;
        s.set_reputation(ticked);
        assert_eq!(names(&s).len(), 8, "the rebuild unfolded it again");
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

    #[test]
    fn selection_follows_the_faction_not_the_row_index() {
        let mut s = seated();
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 0);
        s.run("SetSelectedFaction(3)").unwrap(); // Stormwind
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 3);
        // Fold Alliance: Stormwind is no longer a visible row, so there is no index to answer.
        s.run("CollapseFactionHeader(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 0);
        s.run("ExpandFactionHeader(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 3);
        // Selecting a header clears the selection rather than selecting the group.
        s.run("SetSelectedFaction(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 0);
        // A push that drops the faction entirely drops the selection with it.
        s.run("SetSelectedFaction(3)").unwrap();
        let mut without = state();
        without.entries.retain(|e| e.faction_id != 72);
        s.set_reputation(without);
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 0);
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    /// A peace-forced faction refuses: the stock pane disables the box, but an addon can still call
    /// the global.
    #[test]
    fn the_war_and_inactive_toggles_flip_locally_and_queue_their_sends() {
        let mut s = seated();
        s.run("FactionToggleAtWar(3)").unwrap(); // Stormwind
        assert_eq!(
            s.eval::<Value>("local _,_,_,_,_,_,w = GetFactionInfo(3) return w")
                .unwrap(),
            Value::Integer(1),
            "the pane shows war immediately — nothing acks this"
        );
        s.run("SetFactionInactive(3)").unwrap();
        assert_eq!(
            names(&s).last().unwrap(),
            "Inactive",
            "and the row re-parents on the spot, without a server rebuild"
        );
        assert_eq!(
            s.take_reputation_sends(),
            [
                ReputationSend::AtWar {
                    rep_list_id: 19,
                    at_war: true
                },
                ReputationSend::Inactive {
                    rep_list_id: 19,
                    inactive: true
                },
            ]
        );
        assert!(s.take_reputation_sends().is_empty(), "the drain empties");

        // A peace-forced row: no flip, no send.
        let mut forced = state();
        forced.entries[0].can_toggle_at_war = false;
        s.set_reputation(forced);
        s.run("FactionToggleAtWar(3)").unwrap();
        assert!(s
            .eval::<Value>("local _,_,_,_,_,_,w = GetFactionInfo(3) return w")
            .unwrap()
            .is_nil());
        assert!(s.take_reputation_sends().is_empty());
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn watching_queues_a_send_but_does_not_move_the_bar_itself() {
        let mut s = seated();
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
        assert_eq!(s.take_reputation_sends(), [ReputationSend::Watch(Some(19))]);

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

        // Watching slot 0, then clearing: the two must stay distinct.
        s.run("SetWatchedFactionIndex(8)").unwrap(); // Bloodsail Buccaneers, rep slot 0
        assert_eq!(s.take_reputation_sends(), [ReputationSend::Watch(Some(0))]);
        s.run("SetWatchedFactionIndex(0)").unwrap();
        assert_eq!(s.take_reputation_sends(), [ReputationSend::Watch(None)]);
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }
}
