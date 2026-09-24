//! The skills-pane bindings (decision 0437 phase 4) — the Era-shaped `SkillFrame` surface driving
//! a faithful port of the real 1.12 Skills tab (extracted from `interface.MPQ`: FrameXML
//! `SkillFrame.{xml,lua}`). Unlike [`super::tradeskill`]'s deliberately FLAT v1 recipe list, this
//! pane needs the trainer's own GROUP/TREE machinery ([`super::trainer`]): the app
//! pushes a flat, unordered snapshot ([`UiScript::set_skills`] — [`SkillsState::entries`], each
//! already resolved to name/category by the app from `SkillLine.dbc`/`SkillLineCategory.dbc`), and
//! the ENGINE groups by category, sorts, and folds — the trainer's synthesized-tree pattern, minus
//! the state filter (a skill line carries no green/red/gray category) and minus the wire skill-line
//! id doing double duty as both the row's identity and the group key (here the GROUP key is the
//! category, the row's identity is the skill id).
//!
//! ## The engine grouping law (PINNED at the bytes — decision 1091; the list build `0x4d2cb0` +
//! its comparator `0x4d3070`. The 0530 follow-up this once carried is closed.)
//!
//! Groups ordered by `category_order` ascending (`category_id` breaks a tie, for determinism);
//! one header row per non-empty group (text = `category_name`); within a group, **untrained lines
//! (rank 0) sink below trained ones**, then name ascending ([`collate`] — case-insensitive,
//! raw-byte tie-break; the client's own `stricmp`). Every group starts EXPANDED and is **re-expanded
//! on every push** ([`UiScript::set_skills`] — the client's own `expandedMask = 0xFFFFFFFF`).
//! Visible rows are headers always, plus the entries of expanded groups; every index the Lua API
//! takes/returns is 1-based into that visible list.
//!
//! **Which lines reach the engine at all is the app's half** of the same law (`ui_char.rs`'s
//! `feed_skills`): the client's list build drops a line with no `SkillLine`/`SkillRaceClassInfo`/
//! `SkillLineCategory` row, one whose `SkillRaceClassInfo.flags` carries `0x2`, and an untrained
//! one that no flag admits. The engine sorts what survives.
//!
//! ## The Era API shape (matched to the real `SkillFrame.lua`, transcribed onto this engine)
//!
//! `GetNumSkillLines()` → the visible row count. `GetSkillLineInfo(i)` → the ref's own tuple
//! (`name, isHeader, isExpanded, skillRank, numTempPoints, skillModifier, skillMaxRank,
//! isAbandonable, stepCost, rankCost, minLevel, skillCostType, skillDescription`) — **13 values on
//! a skill row, 12 on a header** (`0x4d3a20` vs `0x4d3768`; a header stops after `skillCostType`
//! and carries no description slot at all). A header shapes
//! `(category_name, 1, expanded, 0, 0, 0, 0, nil, nil, nil, 0, 0)`, an entry
//! `(name, nil, nil, rank, 0, tempBonus, max, abandonable, nil, nil, minLevel, costIndex+1,
//! description)`, with `rank`/`max` computed by [`displayed_ranks`] — the permanent bonus folded
//! in, then `max` forced to `1` on a **single-rank** line whatever the server said
//! ([`SkillEntry::mono`]; the pane's proficiency gate is `skillMaxRank == 1`).
//!
//! `numTempPoints` is a **REAL FIELD** (`entry->+0x10`) that this client answers as `0`. The
//! distinction matters and this comment used to blur it: the field has four writers
//! in the real client — zeroed per list rebuild (`0x4d2e2e`), incremented by `AddSkillUp`'s worker
//! (`0x4d345b`), decremented at `0x4d358a`, reset by `CancelSkillUps` (`0x4d35c9`) — and
//! `SkillFrame.xml:697` wires `AddSkillUp` for real. What is true is a **data-reachability** verdict
//! about the shipped tables, not a property of the field: the `rankCost or numTempPoints > 0` guard
//! never opens on 1.12's data, so nothing a player can hold moves it off `0`. Say `0` because the
//! model carries no temp points, never because the field is fictional — the day we model training
//! points this is a value to push, not a constant to keep. `stepCost`/`rankCost` are always **nil** —
//! for DATA reasons, not code ones (`SkillLine.skillCostsID` is 0 in all 123 rows, and the
//! step-cost gate's flag bits are set on no line a player can hold) — and nil, not `0`, is what
//! the ref's `if (stepCost)` / `elseif (rankCost or …)` branches read, since `0` is truthy in Lua.
//! `minLevel` and `skillCostType` are REAL numbers off the `SkillRaceClassInfo` row (visually inert
//! on this build: every branch they colour is repainted by the normal-skill/proficiency branches
//! after it). `skillDescription` is REAL (`SkillLine.dbc` col 12 through the app feed — the detail
//! pane's body text); `isAbandonable` is REAL too ([`SkillEntry::abandonable`], the unlearn
//! button's gate — and `AbandonSkill(i)` is its outbound half, a VISIBLE index queued out by skill
//! id for the app's `CMSG_UNLEARN_SKILL`, [`UiScript::take_skill_abandons`]).
//!
//! `ExpandSkillHeader(i)`/`CollapseSkillHeader(i)` take a header's VISIBLE index (`0` = all
//! groups, the trainer's own collapse-all shape). `SetSelectedSkill(i)`/`GetSelectedSkill()` are a
//! VISIBLE index too, but the engine holds the selection BY SKILL ID internally (the tradeskill's
//! own by-spell-id persistence pattern, [`super::tradeskill::UiScript::set_trade_skill`]) so it
//! survives a re-push that reorders or regroups; selecting a header (or an out-of-range index)
//! clears it. `GetAdjustedSkillPoints()` is a vestigial 1.12 leftover the ref reads; it always
//! returns `0` — there is no training-point economy behind a skill line in this client.
//!
//! `CancelSkillUps()` is bound (the page's Close button calls it, 1956): the temp-point reset
//! over a table this model keeps empty.
//!
//! The ref Lua's other globals (`SkillBar_OnClick`'s `RemoveSkillUp`/`AddSkillUp`/`BuySkillTier`,
//! `UnitCharacterPoints`) back the training-up machinery this pane doesn't ship (0437's named
//! out-of-scope) — none are transcribed, engine-side or XML-side, rather than stubbing dead call
//! sites nothing in [`SkillFrame.xml`] ever reaches. (The `UNLEARN_SKILL` popup, once in that
//! list, ships for real now — the abandon slice above.)

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One known skill line off the player's `PLAYER_SKILL_INFO` block, app-resolved (0437 phase 4).
/// EXACT shape the app feed (`crates/benilla/src/ui_char.rs`) is written against — do not rename.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillEntry {
    pub skill_id: u32,
    /// `SkillLine.dbc` name ("First Aid").
    pub name: String,
    /// Current rank, raw off the descriptor. `0` = **untrained**, which is a sort key of its own
    /// (untrained lines sink under their category's trained ones — the client's comparator
    /// `0x4d3070`).
    pub value: u32,
    /// Max rank, as the server's own `PLAYER_SKILL_INFO` descriptor holds it — see [`Self::mono`]
    /// for the DBC override the API return goes through.
    pub max: u32,
    /// `SkillRaceClassInfo.flags & 0x400` (`SKILL_FLAG_MONO_VALUE`) for the player's race/class:
    /// a **single-rank** line. The real client's `GetSkillLineInfo` overrides `skillMaxRank` to
    /// `1` for these, whatever the descriptor says (`0x4d3610`, `4d38b1 test ah,0x4`) —
    /// so a hunter's `Beast Mastery`, which vmangos happily reports as `300/300`, reads as a
    /// proficiency and `SkillFrame.lua`'s `skillMaxRank == 1` branch draws it gray with no rank
    /// text. The raw [`Self::max`] stays untouched here; the override lives at the API return,
    /// exactly where the binary puts it.
    pub mono: bool,
    /// The **temporary** bonus (auras/consumables/enchants; negative possible) — and only that:
    /// `GetSkillLineInfo`'s `skillModifier` is a signed read of the descriptor's temp half alone
    /// (`+0x850`), the green "+n" in the rank text. The permanent half is [`Self::perm_bonus`],
    /// which the client folds into the numbers instead.
    pub temp_bonus: i32,
    /// The **permanent** bonus (talents; `+0x852`, negative possible). Not a return of its own:
    /// the client adds it into BOTH `skillRank` and `skillMaxRank` before the mono override
    /// (`0x4d380c`/`0x4d385d`), and only when the raw value is nonzero — so a line the player
    /// doesn't have stays at a flat `0`, never `0 + bonus`.
    pub perm_bonus: i32,
    /// `SkillRaceClassInfo.reqLevel` — `GetSkillLineInfo`'s 11th return, pushed as a real number
    /// (`0x4d39ef`; nonzero in practice: Mail 40, Dual Wield 20).
    pub min_level: u32,
    /// `SkillRaceClassInfo.skillCostID` — `GetSkillLineInfo`'s 12th return is this **plus one**
    /// (`0x4d3a06`), which is why the engine adds the 1, not the feed.
    pub cost_index: u32,
    /// `SkillLine.dbc` categoryId.
    pub category_id: u32,
    /// Resolved category name ("Professions") — the header text.
    pub category_name: String,
    /// `SkillLineCategory` displayOrder — the group sort key.
    pub category_order: u32,
    /// `SkillLine.dbc` description (enUS column 12) — `GetSkillLineInfo`'s 13th return, the
    /// detail pane's body text (empty when the row carries none; the XML's `SKILL_DESCRIPTION`
    /// format renders it verbatim).
    pub description: String,
    /// Whether the line can be unlearned — `GetSkillLineInfo`'s 8th return (`isAbandonable`), the
    /// detail pane's unlearn-button gate. App-resolved, and a **conjunction**: the descriptor's
    /// skill **step** must be nonzero AND `SkillRaceClassInfo.flags & 0x20` must be set
    /// (`SKILL_FLAG_UNLEARNABLE` — the server's own `CMSG_UNLEARN_SKILL` gate, vmangos
    /// `SkillHandler.cpp`). Both halves are the client's (`0x4d3953`–`0x4d3975`).
    pub abandonable: bool,
}

/// A flat, unordered push of every known skill line (0437 phase 4) — the ENGINE groups and sorts
/// (see the module doc's grouping law). EXACT shape the app feed is written against — do not
/// rename.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SkillsState {
    pub entries: Vec<SkillEntry>,
}

/// One category group in the synthesized display tree: the header's id/name/sort key and the
/// positions (into [`SkillsState::entries`]) of the category's lines, pre-sorted by name —
/// mirrors [`super::trainer::TrainerGroup`], but the group key is a *category*, never the wire
/// skill line itself. Engine-internal (never crosses the app seam, unlike [`SkillsState`]);
/// `pub(crate)` only because [`super::model::Model`] stores a `Vec` of them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SkillGroup {
    category_id: u32,
    name: String,
    order: u32,
    /// Positions into [`SkillsState::entries`], sorted by [`collate`]d name.
    entries: Vec<usize>,
}

impl super::UiScript {
    /// Replace the skills snapshot (0437 phase 4). Builds the display tree ([`build_groups`]) from
    /// the flat entries, **re-expands every group**, and re-anchors the selection to the SAME
    /// skill id if it's still present — else clears it (the tradeskill's own by-spell-id
    /// selection-persistence precedent).
    ///
    /// The re-expand is the client's, not a convenience: its list rebuild writes
    /// `expandedMask = 0xFFFFFFFF` unconditionally (`0x4d2cb0`, store at `0x4d2ce2`), so a fold
    /// survives only until the next skill-field change — the trainer's collapse-survives-an-update
    /// rule this pane once borrowed is simply a different window's law.
    pub fn set_skills(&mut self, state: SkillsState) {
        let mut model = self.model_mut();
        let groups = build_groups(&state.entries);
        model.skills_collapsed.clear();
        if let Some(sid) = model.skills_selected {
            if !state.entries.iter().any(|e| e.skill_id == sid) {
                model.skills_selected = None;
            }
        }
        model.skills_groups = groups;
        model.skills = state;
    }

    /// Drain the skill line ids `AbandonSkill` queued (the unlearn seam) — the app sends each as
    /// one `CMSG_UNLEARN_SKILL` and otherwise does nothing: the removal arrives back as a server
    /// skill-field update, never a local mutation ([`Model::skill_abandons`]).
    pub fn take_skill_abandons(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().skill_abandons)
    }
}

/// One visible row of the display tree: a category **header** (carrying its group index) or an
/// **entry** (carrying its position into [`SkillsState::entries`]).
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Entry(usize),
}

/// The WoW enUS collator, approximated (the trainer's own [`super::trainer`] helper, duplicated
/// here rather than shared — each seam module keeps its own copy, the established local
/// convention): case-insensitive alphabetical, raw bytes as a stable tie-break.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// Build the display tree from the flat entries (the module doc's grouping law): group by
/// category, sort each group's entries by name, sort the groups by `category_order` (category id
/// breaks a tie). Entry positions index back into the unchanged `entries` slice.
fn build_groups(entries: &[SkillEntry]) -> Vec<SkillGroup> {
    let mut map: HashMap<u32, SkillGroup> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        map.entry(e.category_id)
            .or_insert_with(|| SkillGroup {
                category_id: e.category_id,
                name: e.category_name.clone(),
                order: e.category_order,
                entries: Vec::new(),
            })
            .entries
            .push(i);
    }
    let mut groups: Vec<SkillGroup> = map.into_values().collect();
    for g in &mut groups {
        // Within a category the client sorts UNTRAINED (rank 0) lines below trained ones, then by
        // name (`0x4d3070`: the `untrained` byte set at build time, `0x4d2e19 sete`, compared
        // before the `stricmp` at `0x4d318d`).
        g.entries.sort_by(|&a, &b| {
            (entries[a].value == 0)
                .cmp(&(entries[b].value == 0))
                .then_with(|| collate(&entries[a].name, &entries[b].name))
        });
    }
    groups.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then(a.category_id.cmp(&b.category_id))
    });
    groups
}

/// The visible rows in display order: each group's header (always shown), then — when the group
/// isn't collapsed — its entries. The Lua's 1-based `index` is a position in *this* list.
fn rows(model: &Model) -> Vec<Row> {
    let mut out = Vec::new();
    for (gi, g) in model.skills_groups.iter().enumerate() {
        out.push(Row::Header(gi));
        if !model.skills_collapsed.contains(&g.category_id) {
            for &ei in &g.entries {
                out.push(Row::Entry(ei));
            }
        }
    }
    out
}

/// The count of visible rows (headers + the entries of expanded groups).
fn num_rows(model: &Model) -> usize {
    rows(model).len()
}

/// The entry at a 1-based VISIBLE index, or `None` when that row is a header (or OOB) — so the
/// selection/info getters that read an entry safely no-op on a header row.
fn entry_at(model: &Model, index: usize) -> Option<&SkillEntry> {
    let n = index.checked_sub(1)?;
    match rows(model).get(n)? {
        Row::Entry(ei) => model.skills.entries.get(*ei),
        Row::Header(_) => None,
    }
}

/// Collapse (`collapse = true`) or expand a category by the **display index of its header row**
/// (the trainer's own `Collapse/ExpandTrainerSkillLine` shape). `id == 0` targets ALL groups (the
/// collapse-all button); `id > 0` resolves the header at that visible index to its category. A
/// non-header (or OOB) index is a no-op.
fn set_collapsed(model: &mut Model, id: usize, collapse: bool) {
    if id == 0 && !collapse {
        model.skills_collapsed.clear();
        return;
    }
    let targets: Vec<u32> = if id == 0 {
        model.skills_groups.iter().map(|g| g.category_id).collect()
    } else {
        match rows(model).get(id - 1) {
            Some(Row::Header(gi)) => model
                .skills_groups
                .get(*gi)
                .map(|g| g.category_id)
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    };
    for c in targets {
        if collapse {
            model.skills_collapsed.insert(c);
        } else {
            model.skills_collapsed.remove(&c);
        }
    }
}

/// `SetSelectedSkill(index)` — resolve the 1-based VISIBLE index to a skill id and hold THAT (the
/// module doc's by-id persistence); a header row or an out-of-range index clears the selection.
/// `GetSkillLineInfo`'s **out-of-range tuple** — thirteen values, four of them numeric zeros.
///
/// Byte-verified (pushes `0x4d3a2c`…`0x4d3a98`, `mov eax,0xd` at `0x4d3a9f`). One
/// shape serves five distinct conditions — index 0, a negative index, an index past the end, no
/// active player, and an in-range row whose DBC record is missing — because the reference's
/// bounds test (`0x4d3675`) is a single UNSIGNED compare that funnels them all to the same exit.
///
/// The numeric slots are 4 (`skillRank`), 5 (`numTempPoints`), 6 (`skillModifier`), 7
/// (`skillMaxRank`), 11 (`minLevel`) and 12 (`skillCostType`). Slots 4 and 5 must be numbers or the
/// reference's own unguarded `skillRank + numTempPoints` raises.
fn out_of_range_tuple(_lua: &Lua) -> mlua::Result<MultiValue> {
    Ok(MultiValue::from_vec(vec![
        Value::Nil,        // 1  skillName
        Value::Nil,        // 2  header
        Value::Nil,        // 3  isExpanded
        Value::Integer(0), // 4  skillRank
        Value::Integer(0), // 5  numTempPoints
        Value::Integer(0), // 6  skillModifier
        Value::Integer(0), // 7  skillMaxRank
        Value::Nil,        // 8  isAbandonable
        Value::Nil,        // 9  stepCost
        Value::Nil,        // 10 rankCost
        Value::Integer(0), // 11 minLevel
        Value::Integer(0), // 12 skillCostType
        Value::Nil,        // 13 skillDescription
    ]))
}

fn set_selected(model: &mut Model, index: u32) {
    model.skills_selected = entry_at(model, index as usize).map(|e| e.skill_id);
}

/// `GetSelectedSkill()` — the selection's CURRENT visible index (`0` when nothing is selected, or
/// the selected id isn't visible right now, e.g. its group just got collapsed).
fn selected_index(model: &Model) -> u32 {
    let Some(sid) = model.skills_selected else {
        return 0;
    };
    rows(model)
        .iter()
        .position(|r| matches!(r, Row::Entry(ei) if model.skills.entries[*ei].skill_id == sid))
        .map_or(0, |p| (p + 1) as u32)
}

/// `GetSkillLineInfo`'s `(skillRank, skillMaxRank)` pair for one entry — the client's own
/// arithmetic, in its own order (`0x4d380c`–`0x4d38cb`):
///
/// ```text
/// skillRank    = rank > 0 ? rank + permBonus : rank        // a line at 0 stays flat 0
/// skillMaxRank = max  > 0 ? max  + permBonus : max
/// if mono { skillMaxRank = 1; if skillRank > 1 { skillRank = 1 } }   // override, then a MIN
/// ```
///
/// The mono arm is an unconditional **override** of the max (not a clamp) and a **min** on the
/// rank — the asymmetry is the binary's, and it is what makes a `300/300` class line read as the
/// `1/1` proficiency `SkillFrame.lua` draws gray. Saturating at 0 keeps a negative permanent
/// malus from wrapping the unsigned descriptor values.
fn displayed_ranks(e: &SkillEntry) -> (i64, i64) {
    let fold = |v: u32| {
        if v > 0 {
            (i64::from(v) + i64::from(e.perm_bonus)).max(0)
        } else {
            0
        }
    };
    let (mut rank, mut max) = (fold(e.value), fold(e.max));
    if e.mono {
        max = 1;
        rank = rank.min(1);
    }
    (rank, max)
}

/// A `bool` as the Era `1`/`nil` shape (the trainer/tradeskill's own helper, duplicated per the
/// established per-module convention).
fn era_bool(b: bool) -> Value {
    if b {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// Register the skills-pane globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumSkillLines() → the visible row count (0 before any push).
    g.set(
        "GetNumSkillLines",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(num_rows(&model) as i64)
        })?,
    )?;

    // GetSkillLineInfo(index) → the ref's own tuple: 13 values on a skill row (`0x4d3a20`), 12 on
    // a header (`0x4d3768`). `index` 1-based.
    //
    // **OUT OF RANGE IS THE 13-VALUE FALLBACK, NOT A SINGLE NIL.** This comment used to say the
    // opposite and it was wrong; the correction is. `0x4d3675`'s bounds test is
    // UNSIGNED against the total row count, so index 0, a negative index, an index past the end, no
    // active player, and an in-range row whose DBC record is missing all fall to `0x4d3a2a` and push
    // the same thirteen: `nil, nil, nil, 0, 0, 0, 0, nil, nil, nil, 0, 0, nil` (`0x4d3a2c`…
    // `0x4d3a98`, then `mov eax,0xd`).
    //
    // **Slots 4 and 5 being NUMBERS is the whole point.** The reference's own
    // `SkillDetailFrame_SetStatusBar` does `skillRank = skillRank + numTempPoints`
    // (`SkillFrame.lua:193-194`) with NO guard, straight off `SkillFrame_OnLoad`'s
    // `SetSelectedSkill(0)` — so a single nil here raises "attempt to perform arithmetic on a nil
    // value" the moment the stock file drives us. Slot 1 being nil is what drives its
    // `if (not skillName …) then … return` at l.211. The selection verbs are NOT the divergence:
    // `GetSelectedSkill 0x4d4090` always pushes exactly one number and legitimately answers 0.
    g.set(
        "GetSkillLineInfo",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(n) = index.checked_sub(1) else {
                return out_of_range_tuple(lua);
            };
            let Some(row) = rows(&model).get(n).copied() else {
                return out_of_range_tuple(lua);
            };
            match row {
                Row::Header(gi) => {
                    let grp = &model.skills_groups[gi];
                    let expanded = !model.skills_collapsed.contains(&grp.category_id);
                    Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&grp.name)?),
                        Value::Integer(1), // isHeader
                        era_bool(expanded),
                        Value::Integer(0), // skillRank
                        Value::Integer(0), // numTempPoints — a real field; 0 because we model no temp points (1919)
                        Value::Integer(0), // skillModifier
                        Value::Integer(0), // skillMaxRank
                        Value::Nil,        // isAbandonable
                        Value::Nil,        // stepCost
                        Value::Nil,        // rankCost
                        Value::Integer(0), // minLevel
                        Value::Integer(0), // skillCostType — the 12th and LAST header return
                    ]))
                }
                Row::Entry(ei) => {
                    let e = &model.skills.entries[ei];
                    let (rank, max) = displayed_ranks(e);
                    Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&e.name)?),
                        Value::Nil, // isHeader
                        Value::Nil, // isExpanded
                        Value::Integer(rank),
                        Value::Integer(0), // numTempPoints — a real field; 0 because we model no temp points (1919)
                        Value::Integer(i64::from(e.temp_bonus)), // skillModifier — TEMP only
                        Value::Integer(max),
                        era_bool(e.abandonable), // isAbandonable
                        // stepCost/rankCost are nil on this build for DATA reasons, not code
                        // ones (module doc) — and nil is load-bearing: `0` is truthy in Lua, so
                        // returning it would send SkillFrame.lua down its "learnable skill"
                        // branch on every row.
                        Value::Nil,
                        Value::Nil,
                        Value::Integer(i64::from(e.min_level)), // minLevel — a real number
                        // skillCostType — SkillCostIndex + 1, the client's own `0x4d3a06`.
                        Value::Integer(i64::from(e.cost_index) + 1),
                        Value::String(lua.create_string(&e.description)?), // skillDescription
                    ]))
                }
            }
        })?,
    )?;

    // Collapse/ExpandSkillHeader(id) — fold a category by the display index of its header row (id
    // 0 = all groups); a non-header (or OOB) index no-ops.
    g.set(
        "CollapseSkillHeader",
        lua.create_function(|lua, id: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, true);
            Ok(())
        })?,
    )?;
    g.set(
        "ExpandSkillHeader",
        lua.create_function(|lua, id: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, false);
            Ok(())
        })?,
    )?;

    // SetSelectedSkill(index) / GetSelectedSkill() — the engine-held selection, VISIBLE index in,
    // VISIBLE index out, held BY SKILL ID internally (module doc).
    g.set(
        "SetSelectedSkill",
        lua.create_function(|lua, index: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_selected(&mut model, index);
            Ok(())
        })?,
    )?;
    g.set(
        "GetSelectedSkill",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(selected_index(&model)))
        })?,
    )?;

    // GetAdjustedSkillPoints() → 0 — a vestigial 1.12 leftover the ref reads (module doc); this
    // client has no training-point economy behind a skill line.
    g.set(
        "GetAdjustedSkillPoints",
        lua.create_function(|_, ()| Ok(0i64))?,
    )?;

    // AbandonSkill(index) — VISIBLE index in (the ref's UNLEARN_SKILL popup passes the row it was
    // opened for), queued out BY SKILL ID for the app's CMSG_UNLEARN_SKILL send. Mutates NOTHING
    // locally: the real client waits for the server's skill-field update (vmangos SetSkill(id,0,0)
    // → our descriptor watcher → a fresh set_skills push → SKILL_LINES_CHANGED). A header or
    // out-of-range index no-ops (only entries carry the unlearn button).
    g.set(
        "AbandonSkill",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(id) = entry_at(&model, index).map(|e| e.skill_id) {
                model.skill_abandons.push(id);
            }
            Ok(())
        })?,
    )?;

    // CancelSkillUps() — `0x4d3e30` → `0x4d35c9` (reference `1.12-shapes.tsv`: no arguments, no
    // returns): the reset leg of the temp-point table, zeroing every entry's `numTempPoints` and
    // giving the points back to the pool. The stock page's LIVE Close button calls it before
    // hiding (`SkillFrame.xml`'s SkillFrameCancelButton), so it is reached on
    // every close. This model carries no temp points (module docs, 1919): the table it resets is
    // empty by construction, so the reset moves nothing — the binding's whole effect on this
    // client's data, not a stand-in for it. The day temp points are modelled, this is where
    // they are cleared.
    g.set("CancelSkillUps", lua.create_function(|_, ()| Ok(()))?)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn entry(
        skill_id: u32,
        name: &str,
        value: u32,
        max: u32,
        temp_bonus: i32,
        category_id: u32,
        category_name: &str,
        category_order: u32,
    ) -> SkillEntry {
        SkillEntry {
            skill_id,
            name: name.into(),
            value,
            max,
            temp_bonus,
            perm_bonus: 0,
            min_level: 0,
            cost_index: 0,
            category_id,
            category_name: category_name.into(),
            category_order,
            description: format!("About {name}."),
            // Fixture rule: the Professions category (id 2) is abandonable, weapons are not —
            // the real SkillRaceClassInfo 0x20 split's shape.
            abandonable: category_id == 2,
            // Fixture rule: the Class Skills category (id 3) is single-rank, the real 0x400
            // split's shape (a hunter's Beast Mastery).
            mono: category_id == 3,
        }
    }

    /// Two categories: "Weapon Skills" (order 1: Defense, Swords) and "Professions" (order 2:
    /// First Aid, Fishing) — a flat, unordered push (Swords precedes Defense in push order; the
    /// engine's own name sort must still show Defense first).
    fn state() -> SkillsState {
        SkillsState {
            entries: vec![
                entry(43, "Swords", 200, 300, 0, 1, "Weapon Skills", 1),
                entry(95, "Defense", 180, 300, 5, 1, "Weapon Skills", 1),
                entry(129, "First Aid", 57, 75, 3, 2, "Professions", 2),
                entry(356, "Fishing", 1, 300, 0, 2, "Professions", 2),
            ],
        }
    }

    /// Read `(name, type)` at a visible index, `type` = "header"/"entry".
    fn row_kind(s: &mut UiScript, i: i64) -> (String, String) {
        s.eval::<(String, String)>(&format!(
            "local n,h = GetSkillLineInfo({i}) local t = h and 'header' or 'entry' return n,t"
        ))
        .unwrap()
    }

    #[test]
    fn grouped_visible_rows_interleave_headers_ordered_by_category() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 0);

        s.set_skills(state());
        // 2 headers + 4 entries = 6 visible rows, category_order ascending, name ascending within.
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 6);
        assert_eq!(
            row_kind(&mut s, 1),
            ("Weapon Skills".into(), "header".into())
        );
        assert_eq!(row_kind(&mut s, 2), ("Defense".into(), "entry".into()));
        assert_eq!(row_kind(&mut s, 3), ("Swords".into(), "entry".into()));
        assert_eq!(row_kind(&mut s, 4), ("Professions".into(), "header".into()));
        assert_eq!(row_kind(&mut s, 5), ("First Aid".into(), "entry".into()));
        assert_eq!(row_kind(&mut s, 6), ("Fishing".into(), "entry".into()));

        // Every group starts EXPANDED (the module doc's default rule).
        let (_, h1, e1) = s
            .eval::<(String, i64, Option<i64>)>("local n,h,e = GetSkillLineInfo(1) return n,h,e")
            .unwrap();
        assert_eq!((h1, e1), (1, Some(1)));
    }

    #[test]
    fn abandon_skill_queues_the_entrys_skill_id_and_mutates_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());
        // The 8th return: Professions rows are abandonable (fixture rule), weapon rows and
        // headers are not — 1/nil, the 1.12 boolean shape.
        let ab = |s: &mut UiScript, i: i64| {
            s.eval::<Option<i64>>(&format!(
                "local _,_,_,_,_,_,_,ab = GetSkillLineInfo({i}) return ab"
            ))
            .unwrap()
        };
        assert_eq!(ab(&mut s, 2), None, "Defense is not abandonable");
        assert_eq!(ab(&mut s, 5), Some(1), "First Aid is abandonable");
        assert_eq!(ab(&mut s, 1), None, "a header never is");

        // AbandonSkill queues BY SKILL ID; headers and out-of-range indices no-op; the list
        // itself is untouched (the server round trip owns the removal).
        s.run("AbandonSkill(5)").unwrap();
        s.run("AbandonSkill(1)").unwrap();
        s.run("AbandonSkill(99)").unwrap();
        assert_eq!(s.take_skill_abandons(), vec![129]);
        assert!(
            s.take_skill_abandons().is_empty(),
            "drain empties the queue"
        );
        assert_eq!(
            s.eval::<i64>("return GetNumSkillLines()").unwrap(),
            6,
            "no local removal — the visible tree is unchanged"
        );
    }

    #[test]
    fn collapse_hides_a_groups_entries_and_remaps_indices() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());

        // Fold "Weapon Skills" (header at visible index 1): its two entries vanish, the header
        // stays and now reports isExpanded=nil. 6 → 4.
        s.run("CollapseSkillHeader(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 4);
        let (name, _, expanded) = s
            .eval::<(String, i64, Option<i64>)>("local n,h,e = GetSkillLineInfo(1) return n,h,e")
            .unwrap();
        assert_eq!((name.as_str(), expanded), ("Weapon Skills", None));
        assert_eq!(
            row_kind(&mut s, 2),
            ("Professions".into(), "header".into()),
            "Weapon Skills' entries are folded; Professions is now row 2"
        );

        // Expand it back.
        s.run("ExpandSkillHeader(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 6);

        // Collapse-all (id 0), then expand-all (id 0).
        s.run("CollapseSkillHeader(0)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 2);
        s.run("ExpandSkillHeader(0)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 6);
    }

    /// A re-push keeps the SELECTION (by skill id) but throws every fold away — the client's list
    /// rebuild re-expands unconditionally (`expandedMask = 0xFFFFFFFF`).
    #[test]
    fn a_repush_keeps_the_selection_and_re_expands_every_group() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());

        // Selecting a HEADER clears the selection (module doc).
        s.run("SetSelectedSkill(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 0);

        // Select Swords (row 3), fold Professions (row 4).
        s.run("SetSelectedSkill(3)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 3);
        s.run("CollapseSkillHeader(4)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 4);

        // A re-push (values ticked up): the fold is GONE — all 6 rows are back — while the
        // selection still points at Swords, which is still row 3.
        let mut ticked = state();
        ticked.entries[0].value = 201; // Swords
        s.set_skills(ticked);
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 6);
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 3);
        let (name, rank) = s
            .eval::<(String, i64)>("local n,_,_,r = GetSkillLineInfo(3) return n,r")
            .unwrap();
        assert_eq!((name.as_str(), rank), ("Swords", 201));

        // A re-push that drops the selected skill entirely clears the selection.
        let mut without_swords = state();
        without_swords.entries.remove(0);
        s.set_skills(without_swords);
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 0);
    }

    #[test]
    fn header_and_entry_tuple_shapes() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());

        // Header row 1 ("Weapon Skills"): 12 values — (name, 1, expanded, 0, 0, 0, 0, nil, nil,
        // nil, 0, 0), and NO 13th (the client's header path pushes 12, `0x4d3768`).
        let (name, is_header, is_expanded, rank, temp, modifier, max) = s
            .eval::<(String, i64, Option<i64>, i64, i64, i64, i64)>(
                "local n,h,e,r,t,m,mx = GetSkillLineInfo(1) return n,h,e,r,t,m,mx",
            )
            .unwrap();
        assert_eq!(
            (
                name.as_str(),
                is_header,
                is_expanded,
                rank,
                temp,
                modifier,
                max
            ),
            ("Weapon Skills", 1, Some(1), 0, 0, 0, 0)
        );
        let (abandon_nil, step_nil, rank_cost_nil, min_level, cost_type) = s
            .eval::<(bool, bool, bool, i64, i64)>(
                "local _,_,_,_,_,_,_,a,st,rc,ml,ct = GetSkillLineInfo(1) \
                 return a==nil, st==nil, rc==nil, ml, ct",
            )
            .unwrap();
        let count = s.arity("GetSkillLineInfo(1)").unwrap();
        assert!(abandon_nil);
        assert!(
            step_nil && rank_cost_nil,
            "stepCost/rankCost are nil, not 0"
        );
        assert_eq!((min_level, cost_type), (0, 0));
        assert_eq!(count, 12, "a header row returns 12 values, not 13");

        // Entry row 2 ("Defense", value 180, max 300, temp bonus +5): (name, nil, nil, 180, 0, 5,
        // 300, nil, nil, nil, 0, 1, description) — 13 values.
        let (name, is_header, is_expanded, rank, temp, modifier, max) = s
            .eval::<(String, Option<i64>, Option<i64>, i64, i64, i64, i64)>(
                "local n,h,e,r,t,m,mx = GetSkillLineInfo(2) return n,h,e,r,t,m,mx",
            )
            .unwrap();
        assert_eq!(
            (
                name.as_str(),
                is_header,
                is_expanded,
                rank,
                temp,
                modifier,
                max
            ),
            ("Defense", None, None, 180, 0, 5, 300)
        );
        // The 13th return is the REAL description (SkillLine.dbc col 12 through the feed) — an
        // entry row's alone: a header stops at 12 (asserted above).
        let count = s.arity("GetSkillLineInfo(2)").unwrap();
        let desc = s
            .eval::<String>("local _,_,_,_,_,_,_,_,_,_,_,_,d = GetSkillLineInfo(2) return d")
            .unwrap();
        assert_eq!((count, desc.as_str()), (13, "About Defense."));
        // minLevel and skillCostType are real NUMBERS on an entry — and the cost type is the
        // row's index PLUS ONE (the client's own `0x4d3a06`), so a fixture at index 0 reads 1.
        let (min_level, cost_type) = s
            .eval::<(i64, i64)>(
                "local _,_,_,_,_,_,_,_,_,_,ml,ct = GetSkillLineInfo(2) return ml,ct",
            )
            .unwrap();
        assert_eq!((min_level, cost_type), (0, 1));
    }

    /// The permanent bonus folds into BOTH numbers, the temporary one is the `skillModifier`
    /// return on its own, and a line at 0 stays flat 0 (the client's `rank > 0 ?` guard).
    #[test]
    fn the_permanent_bonus_folds_into_the_numbers_and_the_temporary_one_does_not() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        st.entries[1].perm_bonus = 10; // Defense 180/300, temp +5
        st.entries
            .push(entry(182, "Herbalism", 0, 0, 0, 2, "Professions", 2));
        st.entries.last_mut().unwrap().perm_bonus = 10;
        s.set_skills(st);

        // Defense (row 2): 190/310, modifier still the temp +5 alone.
        let (rank, modifier, max) = s
            .eval::<(i64, i64, i64)>("local _,_,_,r,_,m,mx = GetSkillLineInfo(2) return r,m,mx")
            .unwrap();
        assert_eq!((rank, modifier, max), (190, 5, 310));

        // Herbalism 0/0 (row 7, an untrained line sorted under the trained ones): a flat 0/0 —
        // the bonus is not added to a zero.
        let (name, rank, max) = s
            .eval::<(String, i64, i64)>("local n,_,_,r,_,_,mx = GetSkillLineInfo(7) return n,r,mx")
            .unwrap();
        assert_eq!((name.as_str(), rank, max), ("Herbalism", 0, 0));
    }

    /// Within a category, untrained (rank 0) lines sink below the trained ones, whatever their
    /// name — the client's comparator (`0x4d3070`), which tests `untrained` before `stricmp`.
    #[test]
    fn untrained_lines_sort_under_the_trained_ones_of_their_category() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        // "Alchemy" would sort first in Professions by name; at rank 0 it goes last.
        st.entries
            .push(entry(171, "Alchemy", 0, 300, 0, 2, "Professions", 2));
        s.set_skills(st);
        let names: Vec<String> = (5..=7)
            .map(|i| {
                s.eval::<String>(&format!("return (GetSkillLineInfo({i}))"))
                    .unwrap()
            })
            .collect();
        assert_eq!(names, ["First Aid", "Fishing", "Alchemy"]);
    }

    /// A single-rank line reports `1/1` however high the server's descriptor is (the client's own
    /// `SkillRaceClassInfo.flags & 0x400` arm: an override on the max, a min on the rank) — the
    /// pane's proficiency gate. A normal line is unaffected.
    #[test]
    fn a_mono_line_reports_max_rank_one_whatever_the_server_said() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        // Beast Mastery: category 3 ⇒ mono by the fixture rule, server-side 300/300.
        st.entries.push(entry(
            50,
            "Beast Mastery",
            300,
            300,
            0,
            3,
            "Class Skills",
            0,
        ));
        s.set_skills(st);

        // Row 1/2 = the Class Skills header + its single entry (category_order 0 sorts first).
        let (name, rank, modifier, max) = s
            .eval::<(String, i64, i64, i64)>(
                "local n,_,_,r,_,m,mx = GetSkillLineInfo(2) return n,r,m,mx",
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), rank, modifier, max),
            ("Beast Mastery", 1, 0, 1),
            "the descriptor's 300/300 reads 1/1 — max overridden, rank clamped under it"
        );
        // Defense (a weapon line, not mono) still reports its real 300.
        assert_eq!(
            s.eval::<i64>("local _,_,_,_,_,_,mx = GetSkillLineInfo(4) return mx")
                .unwrap(),
            300
        );
    }

    /// **Out of range answers the reference's THIRTEEN-value tuple, not a bare nil** — decision
    /// 1919, byte-verified (`0x4d3675`'s bounds test, `0x4d3a2c`…`0x4d3a98`).
    ///
    /// The COUNT is the whole assertion. `GetSkillLineInfo(0) == nil` reads true either way,
    /// because Lua compares only the first returned value — which is precisely how the old
    /// one-nil shape passed for as long as it did. What breaks a caller is slots 4 and 5: the
    /// reference's own `SkillDetailFrame_SetStatusBar` does `skillRank = skillRank + numTempPoints`
    /// with NO guard, straight off `SkillFrame_OnLoad`'s `SetSelectedSkill(0)`, so those two must be
    /// NUMBERS or the stock file raises the moment it loads. One shape serves index 0, a negative
    /// index, past-the-end, no active player and a DBC miss — the reference funnels all five
    /// through one unsigned bounds test (`0x4d3675`).
    #[test]
    fn out_of_range_answers_the_thirteen_value_tuple() {
        let s = UiScript::new().unwrap();
        for idx in ["0", "1", "99"] {
            assert_eq!(
                s.arity(&format!("GetSkillLineInfo({idx})")).unwrap(),
                13,
                "GetSkillLineInfo({idx}) must answer 13 values"
            );
            assert_eq!(
                s.eval::<i64>(&format!(
                    "local _,_,_,r,t = GetSkillLineInfo({idx}) return r + t"
                ))
                .unwrap(),
                0,
                "GetSkillLineInfo({idx}) slots 4+5 must be NUMBERS — the reference adds them unguarded"
            );
            assert!(
                s.eval::<bool>(&format!("return (GetSkillLineInfo({idx})) == nil"))
                    .unwrap(),
                "slot 1 must stay nil — it drives the stock file's own `if (not skillName)` early-out"
            );
        }
    }

    #[test]
    fn no_push_reports_zero_rows() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 0);
        // The empty pane answers the out-of-range tuple; the test above holds its ARITY, which is
        // the load-bearing half. This line only pins slot 1.
        assert!(s
            .eval::<bool>("return (GetSkillLineInfo(1)) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetAdjustedSkillPoints()").unwrap(), 0);
        // Collapse/expand/select on an empty pane are harmless no-ops.
        s.run("CollapseSkillHeader(0) ExpandSkillHeader(1) SetSelectedSkill(1)")
            .unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 0);
    }
}
