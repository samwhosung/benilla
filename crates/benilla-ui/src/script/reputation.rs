//! The reputation-pane bindings — the twelve `ReputationFrame` globals, driving a faithful port of
//! the real 1.12 Reputation tab (reference FrameXML `ReputationFrame.{xml,lua}`).
//!
//! **The display law here is byte-pinned**, not inferred, across the client's whole reputation TU
//! (`[0x4d5200, 0x4d6d80)`).
//!
//! Same split as [`super::skills`]: the app pushes a flat, unordered snapshot
//! ([`UiScript::set_reputation`], each row already resolved from `Faction.dbc` + the player's live
//! wire slot) and the ENGINE groups, sorts and folds. Every index the Lua API takes or returns is
//! 1-based into the resulting visible row list.
//!
//! ## The tree: a header is a FLAG, the parent pointer is only the key
//!
//! Two separate facts, and conflating them is what this module was written wrong for once:
//!
//! - **A row is a header because the server said so** — flag bit `0x08`, tested as
//!   `entry.isHeader = (flags >> 3) & 1` at `0x4d5acb`. Every emulator names that bit
//!   `INVISIBLE_FORCED`; the binary disagrees, and the binary wins. The two readings select the
//!   *same five factions* on 1.12's data (Alliance, Horde, Steamwheedle Cartel and the two
//!   battleground blocs all carry it), which is exactly why the wrong one survives a screenshot.
//! - **`Faction.dbc`'s `team` is the grouping key** — which header a row files under. An `INACTIVE`
//!   row (flag `0x20`) is re-parented under the key `-1` instead of its own.
//!
//! A key whose header row is absent gets one **synthesized** (`0x4d5a70`) carrying that faction's
//! real id, so `GetFactionInfo` names it the ordinary way. Two keys name no faction at all and take
//! their text from global strings: `0` → `FACTION_OTHER`, `-1` → `FACTION_INACTIVE`.
//!
//! **A childless header is not drawn.** `GetNumFactions` (`ds:0xb73764`) is the total minus each
//! collapsed child *and each empty header*, over an array re-sorted so `[0, visible)` is exactly the
//! visible prefix — a flattened materialized view, not a skip.
//!
//! ## Ordering (the two real `qsort`s in `0x4d5c40`)
//!
//! Headers (`0x4d5dc0`): localized name ascending, with the two unresolved keys last and the
//! **larger raw key first**, so `0` "Other" precedes `-1` "Inactive". Rows (`0x4d5e70`): within a
//! header, the header row itself, then its children by localized name ascending.
//!
//! ## Collapse
//!
//! Client-side only — no opcode carries it, and it is persisted nowhere. The client's mask
//! (`ds:0x84a0a4`) stores **expanded** bits, so `isCollapsed` is its negation. **Every rebuild
//! resets it to all-expanded and then collapses the "Inactive" header**, which is why a fold here
//! does not survive a push (the skills pane's `expandedMask = 0xFFFFFFFF`, one door along).
//! `CollapseFactionHeader`/`ExpandFactionHeader` given `0` — or any non-header row — act on **every**
//! header.
//!
//! ## Where the numbers come from — all of them, the app
//!
//! This crate is engine-free by design (decision 0068: roxmltree + mlua, no DBC, no wire), so every
//! fidelity number arrives already computed, the same division [`super::skills`] draws.
//! `crates/benilla-app/src/ui_reputation.rs` owns: the race/class slot pick that gates membership,
//! adding `Faction.dbc`'s base to the wire standing (`wire + base` at `0x4d6370`), ranking the
//! total, and the rank's absolute window (`.rdata 0x80928c` — the same nine edges
//! `benilla_formats::reputation_rank` thresholds against).
//!
//! `standing_id` is `rank + 1`, 1..=8 (`FACTION_STANDING_LABEL<n>`, `FACTION_BAR_COLORS[1..8]`).
//! `barMin`/`barMax`/`barValue` are **absolute**; the reference normalizes them itself (ref l.80-82).
//! `GetWatchedFactionInfo`'s `reaction` is that **same** 1..8 scale (`0x4d68a0` is `0x4d658a`'s
//! instruction), not the three-way unit-reaction one.
//!
//! ## The three sends, and which are optimistic
//!
//! `FactionToggleAtWar` (`0x4d6950` → `CMSG 0x125`) and `SetFactionInactive`/`SetFactionActive`
//! (`0x4d69b0`/`0x4d6a00` → `CMSG 0x317`) write the local flag **first** and then send; the inactive
//! pair also re-sorts locally, since the row changes header. `SetWatchedFactionIndex` (`0x4d6b60` →
//! `CMSG 0x318`) is **not** optimistic: the watched slot is `PLAYER_FIELD_WATCHED_FACTION_INDEX`, a
//! server field with no client mirror, so the bar moves only once the descriptor update lands.
//!
//! The at-war *toggle* enforces more than `canToggleAtWar` reports (`0x4d5fd0`): it also refuses
//! while the player is in combat, and applies the −3000 floor only when moving toward peace. Only
//! the reported predicate lives here; the combat half needs state this crate cannot see.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The synthetic header key for factions the player moved to the inactive bucket.
const KEY_INACTIVE: i64 = -1;
/// The synthetic header key for factions with no parent — the "Other" bucket.
const KEY_OTHER: i64 = 0;

/// One reputation faction, app-resolved from `Faction.dbc` plus the player's live wire slot.
/// EXACT shape the app feed (`crates/benilla-app/src/ui_reputation.rs`) is written against — do not
/// rename.
#[derive(Clone, Debug, PartialEq)]
pub struct FactionEntry {
    /// `Faction.dbc` id — the row's identity, and what a child's [`Self::parent_id`] names.
    pub faction_id: u32,
    /// `reputationIndex` — the slot every send addresses and the selection is held by.
    pub rep_list_id: u32,
    /// `Faction.dbc`'s `team`: the header key this row files under, `0` being the "Other" bucket.
    /// Overridden to the "Inactive" bucket while [`Self::inactive`] is set.
    pub parent_id: u32,
    /// The localized `Faction.dbc` name — the bar's left-hand label, or a header's text.
    pub name: String,
    /// The localized `Faction.dbc` description — the detail popup's paragraph. Often empty.
    pub description: String,
    /// Total standing: the DBC race/class base plus the wire standing (the app adds them) —
    /// `GetFactionInfo`'s `barValue`, absolute.
    pub standing: i32,
    /// The 1-based standing rank, `1`..=`8` (`FACTION_STANDING_LABEL<n>`), app-ranked.
    pub standing_id: u8,
    /// The rank's absolute floor and ceiling — `GetFactionInfo`'s `barMin`/`barMax`.
    pub bar_min: i32,
    pub bar_max: i32,
    /// Flag `0x01` — whether the pane lists this faction at all. The **only** flag gating list
    /// membership; off until the player first meets them.
    pub visible: bool,
    /// Flag `0x08` — **this row is a header**, not "force-invisible" (see the module doc).
    pub is_header: bool,
    /// Flag `0x02`. **Engine-mutable**: the toggle flips it here, optimistically, unacked.
    pub at_war: bool,
    /// Flag `0x10` clear **and** `standing >= -3000` — what `GetFactionInfo` reports. The toggle
    /// itself is stricter; see the module doc.
    pub can_toggle_at_war: bool,
    /// Flag `0x20`. **Engine-mutable**, same reason as [`Self::at_war`]; it re-parents the row under
    /// the "Inactive" header rather than hiding it.
    pub inactive: bool,
}

/// A flat, unordered push of every reputation faction the player has a slot for — the ENGINE groups
/// and sorts (see the module doc). EXACT shape the app feed is written against.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ReputationState {
    pub entries: Vec<FactionEntry>,
    /// `PLAYER_FIELD_WATCHED_FACTION_INDEX` as a reputation-list slot, or `None` for none. A server
    /// field with no client mirror, so it rides the push and is never written here.
    pub watched: Option<u32>,
}

/// One outbound reputation verb the pane queued, drained by the app into its `WorldWriter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReputationSend {
    /// `CMSG_SET_FACTION_ATWAR`.
    AtWar { rep_list_id: u32, at_war: bool },
    /// `CMSG_SET_FACTION_INACTIVE`.
    Inactive { rep_list_id: u32, inactive: bool },
    /// `CMSG_SET_WATCHED_FACTION`; `None` is the wire's `-1`, NOT slot 0.
    Watch(Option<u32>),
}

/// One header group: its key, its header text, and the positions of its children.
/// Engine-internal; `pub(crate)` only because [`super::model::Model`] stores a `Vec` of them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FactionGroup {
    /// The header key — a `Faction.dbc` id, or [`KEY_OTHER`] / [`KEY_INACTIVE`].
    key: i64,
    /// The header row's text. Empty for the two synthetic keys, whose text is resolved against the
    /// VM's own global strings when read.
    name: String,
    /// Positions into [`ReputationState::entries`], sorted by [`collate`]d name.
    entries: Vec<usize>,
}

/// One visible row: a **header** (carrying its group index) or a faction **bar** (carrying its
/// position into [`ReputationState::entries`]).
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Entry(usize),
}

impl super::UiScript {
    /// Push the player's reputation snapshot, rebuilding the display tree.
    ///
    /// A rebuild resets the folds to **all expanded** and then collapses the "Inactive" header — the
    /// client's own behaviour, and the reason a fold does not survive a standing tick. The selection
    /// is re-anchored by reputation-list slot, dropped only if that faction left the push.
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

    /// Drain the reputation verbs the pane queued since the last call — the app's outbound seam.
    pub fn take_reputation_sends(&mut self) -> Vec<ReputationSend> {
        std::mem::take(&mut self.model_mut().reputation_sends)
    }
}

/// The header key a row files under: its parent, or the inactive bucket while that flag is set.
fn header_key(e: &FactionEntry) -> i64 {
    if e.inactive {
        KEY_INACTIVE
    } else {
        i64::from(e.parent_id)
    }
}

/// The VM's own text for a synthetic header key, so a localized client gets its own word. Falls back
/// to the enUS literals when the strings are absent (a bare test VM).
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

/// Case-insensitive name ordering, raw bytes breaking a tie — the client's own `stricmp`.
/// Per-module copy, as every seam module here keeps one.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.as_bytes().cmp(b.as_bytes()))
}

/// Era booleans: `1` / `nil`, never `0` — `0` is truthy in Lua and every ref branch here is a bare
/// `if ( atWarWith )`. Per-module copy, as every seam module here keeps one.
fn era_bool(b: bool) -> Value {
    if b {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// Build the header groups (the module doc's law).
///
/// Children are the **visible, non-header** rows, grouped by [`header_key`] and sorted by collated
/// name. A group's text comes from the header ROW carrying that key when there is one, and is left
/// empty for the two synthetic keys, resolved against the VM's global strings at read time.
/// **A childless header is not a group at all** — the client's own count drops it.
///
/// Group order: named headers by collated name ascending, then the synthetic keys last with the
/// larger raw key first, so "Other" (`0`) precedes "Inactive" (`-1`).
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

/// The visible row list: every group's header, plus the children of the expanded ones.
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

/// The entry at a 1-based visible index, or `None` on a header or out of range.
fn entry_at(model: &Model, index: usize) -> Option<&FactionEntry> {
    match rows(model).get(index.checked_sub(1)?)? {
        Row::Entry(ei) => model.reputation.entries.get(*ei),
        Row::Header(_) => None,
    }
}

/// The mutable twin of [`entry_at`] — the at-war/inactive flips write through it.
fn entry_at_mut(model: &mut Model, index: usize) -> Option<&mut FactionEntry> {
    let row = *rows(model).get(index.checked_sub(1)?)?;
    match row {
        Row::Entry(ei) => model.reputation.entries.get_mut(ei),
        Row::Header(_) => None,
    }
}

/// Fold or unfold by 1-based visible index. Index `0` — **or any index that is not a header** — acts
/// on every header, which is the client's own fall-through (`0x4d6a50`/`0x4d6aa0`), not a tolerance
/// invented here.
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

/// `GetFactionInfo`'s out-of-range answer — **eleven values, not one nil** (`0x4d5fa0`'s miss path):
/// `nil, nil, 1, 0, 0, 0, nil, nil, nil, nil, nil`. The `1` in the `standingID` slot is the
/// client's, and it is load-bearing: the reference indexes `FACTION_BAR_COLORS[standingID]`
/// unguarded, so a `nil` there would be an error rather than a blank row.
fn unknown_faction() -> MultiValue {
    let mut out = vec![Value::Nil, Value::Nil, Value::Integer(1)];
    out.extend(std::iter::repeat_n(Value::Integer(0), 3));
    out.extend(std::iter::repeat_n(Value::Nil, 5));
    debug_assert_eq!(out.len(), 11, "the tuple is eleven wide on every path");
    MultiValue::from_vec(out)
}

/// Register the reputation-pane globals.
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

    // GetFactionInfo(index) → the ref's own eleven (ref-ReputationFrame.lua l.51):
    // name, description, standingID, barMin, barMax, barValue, atWarWith, canToggleAtWar,
    // isHeader, isCollapsed, isWatched. 1-based; ELEVEN wide on every path, a miss included.
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
                        era_bool(collapsed),                   // isCollapsed
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
                        era_bool(e.at_war),
                        era_bool(e.can_toggle_at_war),
                        Value::Nil, // isHeader
                        Value::Nil, // isCollapsed
                        era_bool(model.reputation.watched == Some(e.rep_list_id)),
                    ]))
                }
            }
        })?,
    )?;

    // GetSelectedFaction() → the selected row's VISIBLE index, or 0 for none (the ref compares
    // against 0 outright). The selection is HELD by slot, so this re-finds the row each call.
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

    // SetSelectedFaction(index) — a VISIBLE index; a header or an out-of-range index clears it.
    g.set(
        "SetSelectedFaction",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.reputation_selected = entry_at(&model, index).map(|e| e.rep_list_id);
            Ok(())
        })?,
    )?;

    // CollapseFactionHeader(index) / ExpandFactionHeader(index) — a VISIBLE header index; `0` or a
    // non-header row acts on EVERY header (the client's own fall-through).
    // **Both fire `UPDATE_FACTION`, and that is the engine's own doing, not a convenience.**
    // `CollapseFactionHeader 0x4d6a50` and `ExpandFactionHeader 0x4d6aa0` both reach `0x4d6400`,
    // which tail-jumps to `0x4d5c40` — and `0x4d5dab`, inside that function, is one of the FOUR
    // whole-image fire sites of `UPDATE_FACTION`. The stock pane has no other repaint path after a
    // fold: its header `<OnClick>`
    // calls the binding and nothing else (ReputationFrame.xml:9-15), and `ReputationFrame_OnEvent`
    // repaints on `UPDATE_FACTION` alone. Our own retired file repainted itself, which is why this
    // was invisible until the pane went stock (1875).
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

    // FactionToggleAtWar(index) — flip our own AT_WAR bit and queue the send, optimistically
    // (`0x4d6950` writes locally first, and nothing acks it).
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

    // IsFactionInactive(index) / SetFactionInactive(index) / SetFactionActive(index) — optimistic
    // too, and the flip RE-PARENTS the row under the "Inactive" header, so the tree is rebuilt
    // locally (`0x4d69b0`'s own `0x4d5c40` call). The folds are kept across that regroup: it is not
    // a server rebuild, which is the thing that resets them.
    g.set(
        "IsFactionInactive",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(era_bool(
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

    // GetWatchedFactionInfo() → name, reaction, min, max, value — or a single nil when nothing is
    // watched (the ref's `if ( name )` gate). `reaction` is the SAME 1-based standingID scale.
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
                // FIVE values on the nothing-watched branch too — `0x4d6820` has four guards onto
                // one label and no raise, so the shape is `nil, 0, 0, 0, 0` (decision 1845).
                //
                // Worth carrying with it: the `[0, 64)` bound is **not in the binding**.
                // `0x4d5620` maps an out-of-range index to `0`, so the "nothing watched" sentinel
                // passes the `< 0` and `> max` guards and dies at the NULL-DBC-row guard instead.
                // A port that range-checks inside the binding implements a branch the reference
                // does not have.
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

    // SetWatchedFactionIndex(index) — a VISIBLE index, `0` meaning "stop watching". **Not
    // optimistic** (`0x4d6b60`): the watched slot lives in PLAYER_FIELD_WATCHED_FACTION_INDEX with
    // no client mirror, so the bar only moves once the descriptor update comes back. The `0` → `-1`
    // translation happens at the app's wire edge, since slot 0 is a real faction.
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

    /// A header row as the real wire delivers one: flag `0x08` set, and **not** `VISIBLE` — the
    /// Steamwheedle Cartel's actual byte is `0x08` alone. A header is used for its name, never
    /// drawn as a bar, so its own visibility is irrelevant and this fixture proves it.
    fn header(faction_id: u32, rep_list_id: u32, name: &str) -> FactionEntry {
        FactionEntry {
            visible: false,
            is_header: true,
            ..entry(faction_id, rep_list_id, 0, name)
        }
    }

    /// Two named groups and the bucket, pushed OUT of order so the engine's own sort is what shows:
    /// Alliance (469) over Stormwind + Ironforge, Steamwheedle (169) over Booty Bay, and two
    /// parentless factions. Bloodsail Buccaneers is here because it really does hold slot **0** —
    /// the value that must never be confused with "watch nothing".
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

    /// Every visible row's name, in order — the shape assertion the rest lean on.
    fn names(s: &UiScript) -> Vec<String> {
        s.eval::<Vec<String>>(
            "local t = {} for i = 1, GetNumFactions() do t[i] = (GetFactionInfo(i)) end return t",
        )
        .expect("names")
    }

    /// **The tree.** Headers are the rows the server FLAGGED as headers (`0x08`), grouped by the
    /// children's `team`; a header is never also a bar; groups and children sort by name; and the
    /// parentless bucket comes last under `FACTION_OTHER`.
    ///
    /// The fixture's headers are pushed WITHOUT the visible flag, which is what the real wire
    /// sends — so this also pins that a header's own visibility does not gate its group.
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

    /// **A childless header is not a row at all** — the client's own count drops it, so a header
    /// whose children are all unmet must not leave an empty group behind.
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

    /// **`GetFactionInfo`'s eleven, on a bar** — and eleven on a MISS too, which is the client's own
    /// shape (`nil, nil, 1, 0, 0, 0, nil×5`). The `1` matters: the reference indexes
    /// `FACTION_BAR_COLORS[standingID]` unguarded, so a nil there would raise rather than blank.
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

    /// **Folding hides a group's children and shrinks the count**, and index `0` (or any non-header
    /// row) acts on EVERY header — the client's own fall-through, not a tolerance invented here.
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

        // A BAR row is not a header, so it takes the same fall-through: index 2 is Ironforge.
        s.run("CollapseFactionHeader(2)").unwrap();
        assert_eq!(
            names(&s),
            ["Alliance", "Steamwheedle Cartel", "Other"],
            "a non-header index folds every header"
        );
        // …and so does index 0, and so does an index off the end of the list.
        s.run("ExpandFactionHeader(0)").unwrap();
        assert_eq!(names(&s).len(), 8, "index 0 unfolds every header");
        s.run("CollapseFactionHeader(99)").unwrap();
        assert_eq!(names(&s).len(), 3, "so does an out-of-range index");
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    /// **A push RESETS the folds** — all expanded, then "Inactive" collapsed. The client rebuilds
    /// its expand mask on every server refresh, so a fold cannot survive a standing tick.
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

    /// **Inactive re-parents a row under the synthetic "Inactive" header, which starts collapsed.**
    /// It does not hide the faction, and it does not touch the bucket the row came from.
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

    /// **Selection is held by reputation slot, not by row.** `GetSelectedFaction` answers `0` for
    /// none — what the ref's own teardown compares against — and re-finds the row after a reorder.
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
        // Selecting a HEADER clears the selection rather than selecting the group.
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

    /// **The two unacked toggles flip locally and queue their sends**; a peace-forced faction
    /// refuses (the ref disables the box, but an addon can still call the global), and going
    /// inactive re-groups on the spot without waiting for a server rebuild.
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

    /// **The watch bar's five, the `0` that means "nothing", and the fact that watching is NOT
    /// optimistic.** The watched slot is a server field with no client mirror, so the pane shows a
    /// new bar only after the descriptor update comes back as a fresh push — clicking alone must
    /// change nothing on screen.
    #[test]
    fn watching_queues_a_send_but_does_not_move_the_bar_itself() {
        let mut s = seated();
        assert!(
            s.eval::<Value>("return GetWatchedFactionInfo()")
                .unwrap()
                .is_nil(),
            "nothing watched → one nil, which is the ref's own `if ( name )` gate"
        );

        s.run("SetWatchedFactionIndex(3)").unwrap(); // Stormwind
        assert!(
            s.eval::<Value>("return GetWatchedFactionInfo()")
                .unwrap()
                .is_nil(),
            "still nothing: the server owns this field, so the click alone cannot move the bar"
        );
        assert_eq!(s.take_reputation_sends(), [ReputationSend::Watch(Some(19))]);

        // The descriptor update arrives as the next push, and NOW the bar reads.
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

        // Watching the row that IS slot 0, then clearing — the two must not collapse together.
        s.run("SetWatchedFactionIndex(8)").unwrap(); // Bloodsail Buccaneers, rep slot 0
        assert_eq!(s.take_reputation_sends(), [ReputationSend::Watch(Some(0))]);
        s.run("SetWatchedFactionIndex(0)").unwrap();
        assert_eq!(s.take_reputation_sends(), [ReputationSend::Watch(None)]);
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }
}
