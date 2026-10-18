//! The skills-pane bindings behind the stock `SkillFrame`. The app pushes a flat snapshot of known
//! skill lines, resolved from `SkillLine.dbc` and `SkillLineCategory.dbc`, and the engine groups,
//! sorts and folds it as the client's list build (`0x4d2cb0`, comparator `0x4d3070`) does: one
//! header per category, by `category_order` (category id breaks a tie), and within a category the
//! untrained lines (rank 0) after the trained ones, each by name. A push re-expands every group.
//! Every Lua index is 1-based into the visible rows, headers plus the entries of expanded groups;
//! the selection is held by skill id, so it survives a re-push, and selecting a header clears it.
//! Which lines reach the engine at all is the app's half (`feed_skills`).
//!
//! `numTempPoints` is the points `AddSkillUp` chose on the line (`+0x10`, `0x4d345b`), which
//! `RemoveSkillUp`, `CancelSkillUps` and every rebuild take back. Each point costs 0: the cost
//! comes from `SkillCostsData.dbc` through `SkillLine.skillCostsID` (`0x6de110`), and that id is 0
//! in every row, so no pending point ever uses the player's skill points, `AcceptSkillUps` finds
//! nothing to send, and the points left (`GetAdjustedSkillPoints`) stay the points held.
//! `stepCost` and `rankCost` are always nil, by the data: `SkillLine.skillCostsID` is 0 in every
//! row and no line a player holds carries the step-cost flags.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// One known skill line off the player's `PLAYER_SKILL_INFO` block, resolved by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillEntry {
    pub skill_id: u32,
    /// The `SkillLine.dbc` name.
    pub name: String,
    /// The raw rank off the descriptor; 0 is untrained, which sorts below the trained lines.
    pub value: u32,
    /// The raw max rank off the descriptor; [`Self::mono`] overrides it at the API return.
    pub max: u32,
    /// `SkillRaceClassInfo.flags & 0x400` (`SKILL_FLAG_MONO_VALUE`): a single-rank line, whose
    /// `skillMaxRank` the client reports as 1 whatever the descriptor says (`0x4d3610`), so
    /// vmangos's 300/300 Beast Mastery reads as a proficiency.
    pub mono: bool,
    /// The temporary bonus, possibly negative: `skillModifier`, a signed read of the descriptor's
    /// temp half alone (`+0x850`).
    pub temp_bonus: i32,
    /// The permanent bonus (`+0x852`), possibly negative, folded into both ranks when they are
    /// nonzero (`0x4d380c`/`0x4d385d`).
    pub perm_bonus: i32,
    /// `SkillRaceClassInfo.reqLevel`, `GetSkillLineInfo`'s 11th return (`0x4d39ef`).
    pub min_level: u32,
    /// `SkillRaceClassInfo.skillCostID`; the 12th return is this plus one (`0x4d3a06`), added by
    /// the engine.
    pub cost_index: u32,
    /// `SkillLine.dbc` categoryId.
    pub category_id: u32,
    /// The category name, the header text.
    pub category_name: String,
    /// `SkillLineCategory` displayOrder, the group sort key.
    pub category_order: u32,
    /// `SkillLine.dbc` description (enUS column 12), `GetSkillLineInfo`'s 13th return.
    pub description: String,
    /// `isAbandonable`, the 8th return: a nonzero skill step and `SkillRaceClassInfo.flags & 0x20`
    /// (`SKILL_FLAG_UNLEARNABLE`), both checked by the client (`0x4d3953`–`0x4d3975`).
    pub abandonable: bool,
    /// The slot's step word (`+0x84a`), which `BuySkillTier` reads (`0x4d3f4f`).
    pub step: u32,
    /// `SkillRaceClassInfo.flags`: bits `0x8` and `0x4` admit `BuySkillTier` (`0x4d3f62`).
    pub flags: u32,
    /// `SkillRaceClassInfo.skillTierID`: the `SkillTiers.dbc` row that prices a step.
    pub tier_id: u32,
}

/// A flat, unordered push of every known skill line; the engine groups and sorts it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SkillsState {
    pub entries: Vec<SkillEntry>,
}

/// One category of the display tree, with its lines' positions in [`SkillsState::entries`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SkillGroup {
    category_id: u32,
    name: String,
    order: u32,
    /// Positions into [`SkillsState::entries`], in display order.
    entries: Vec<usize>,
}

impl super::UiScript {
    /// Replace the skills snapshot: rebuild the tree, re-expand every group (the client's rebuild
    /// stores `expandedMask = 0xFFFFFFFF` at `0x4d2ce2`) and keep the selection if its skill is
    /// still present.
    pub fn set_skills(&mut self, state: SkillsState) {
        let mut model = self.model_mut();
        let groups = build_groups(&state.entries);
        model.skills_collapsed.clear();
        // A rebuild resets the points chosen (`0x4d2cfb`).
        model.skills_pending.clear();
        if let Some(sid) = model.skills_selected {
            if !state.entries.iter().any(|e| e.skill_id == sid) {
                model.skills_selected = None;
            }
        }
        model.skills_groups = groups;
        model.skills = state;
    }

    /// Push `SkillTiers.dbc` as `(tier id, step costs)`.
    pub fn set_skill_tiers(&mut self, tiers: impl IntoIterator<Item = (u32, [u32; 16])>) {
        self.model_mut().skill_tiers = tiers.into_iter().collect();
    }

    /// Drain the skill ids `AbandonSkill` queued, one `CMSG_UNLEARN_SKILL` each; the removal comes
    /// back as a skill-field update.
    pub fn take_skill_abandons(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().skill_abandons)
    }
}

/// One visible row: a header (its group index) or an entry (its position in
/// [`SkillsState::entries`]).
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Entry(usize),
}

/// The client's `stricmp`, approximated: case-insensitive, raw bytes as the tie-break.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// Build the display tree by the module doc's grouping rule.
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
        // Untrained below trained, then by name (`0x4d3070` compares the untrained byte, set at
        // `0x4d2e19`, before the `stricmp` at `0x4d318d`).
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

/// The visible rows; the Lua's 1-based index is a position in this list.
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

fn num_rows(model: &Model) -> usize {
    rows(model).len()
}

/// The entry at a 1-based visible index; `None` for a header.
fn entry_at(model: &Model, index: usize) -> Option<&SkillEntry> {
    let n = index.checked_sub(1)?;
    match rows(model).get(n)? {
        Row::Entry(ei) => model.skills.entries.get(*ei),
        Row::Header(_) => None,
    }
}

/// Collapse or expand the category whose header is at visible index `id`; 0 is every group, and a
/// non-header index does nothing.
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

/// `GetSkillLineInfo`'s out-of-range answer, thirteen values (`0x4d3a2c`…`0x4d3a98`): the
/// reference's one unsigned bounds test (`0x4d3675`) sends index 0, a negative or past-the-end
/// index, no player and a missing DBC row here. Slots 4 and 5 must be numbers: stock
/// `SkillFrame.lua:194` adds them unguarded, and `SkillFrame_OnLoad` calls `SetSelectedSkill(0)`.
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

/// `GetSelectedSkill()`: the selection's current visible index, 0 when none or folded away.
fn selected_index(model: &Model) -> u32 {
    let Some(sid) = model.skills_selected else {
        return 0;
    };
    rows(model)
        .iter()
        .position(|r| matches!(r, Row::Entry(ei) if model.skills.entries[*ei].skill_id == sid))
        .map_or(0, |p| (p + 1) as u32)
}

/// `GetSkillLineInfo`'s `(skillRank, skillMaxRank)`, in the client's order (`0x4d380c`–`0x4d38cb`):
///
/// ```text
/// skillRank    = rank > 0 ? rank + permBonus : rank
/// skillMaxRank = max  > 0 ? max  + permBonus : max
/// if mono { skillMaxRank = 1; if skillRank > 1 { skillRank = 1 } }
/// ```
///
/// Saturating at 0 keeps a negative permanent bonus from wrapping.
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

/// A line's rank and maximum as the skill-point verbs read them (`0x4d33a8`..`0x4d340b`): each
/// plus the permanent bonus when it is above 0, with no single-rank cap.
fn held_ranks(e: &SkillEntry) -> (i64, i64) {
    let fold = |v: u32| {
        if v > 0 {
            i64::from(v) + i64::from(e.perm_bonus)
        } else {
            0
        }
    };
    (fold(e.value), fold(e.max))
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

    // GetSkillLineInfo(index) → 13 values on a skill row (`0x4d3a20`), 12 on a header
    // (`0x4d3768`); out of range, the 13-value tuple (`0x4d3a2a`), whose nil first slot drives
    // stock `SkillFrame.lua:211`'s early return.
    g.set(
        "GetSkillLineInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetSkillLineInfo(index)` (`0x849eb4`, `0x4d3629`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetSkillLineInfo(index)")?;
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
                        binding_abi::flag(expanded),
                        Value::Integer(0), // skillRank
                        Value::Integer(0), // numTempPoints
                        Value::Integer(0), // skillModifier
                        Value::Integer(0), // skillMaxRank
                        Value::Nil,        // isAbandonable
                        Value::Nil,        // stepCost
                        Value::Nil,        // rankCost
                        Value::Integer(0), // minLevel
                        Value::Integer(0), // skillCostType, the last header return
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
                        // numTempPoints
                        Value::Integer(i64::from(
                            model.skills_pending.get(&e.skill_id).copied().unwrap_or(0),
                        )),
                        Value::Integer(i64::from(e.temp_bonus)), // skillModifier: temp only
                        Value::Integer(max),
                        binding_abi::flag(e.abandonable), // isAbandonable
                        // stepCost, rankCost: nil, since Lua reads `0` as true.
                        Value::Nil,
                        Value::Nil,
                        Value::Integer(i64::from(e.min_level)), // minLevel
                        // skillCostType: the cost index plus one (`0x4d3a06`).
                        Value::Integer(i64::from(e.cost_index) + 1),
                        Value::String(lua.create_string(&e.description)?), // skillDescription
                    ]))
                }
            }
        })?,
    )?;

    // Collapse/ExpandSkillHeader(id): by the header's visible index, 0 for every group.
    g.set(
        "CollapseSkillHeader",
        lua.create_function(|lua, id: Value| {
            // `is-number` else `Usage: CollapseSkillHeader(index)` (`0x849ef0`, `0x4d3b81`).
            let id = binding_abi::index_arg(lua, id, "Usage: CollapseSkillHeader(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, true);
            Ok(())
        })?,
    )?;
    g.set(
        "ExpandSkillHeader",
        lua.create_function(|lua, id: Value| {
            // `is-number` else `Usage: ExpandSkillHeader(index)` (`0x849f14`, `0x4d3be1`).
            let id = binding_abi::index_arg(lua, id, "Usage: ExpandSkillHeader(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, false);
            Ok(())
        })?,
    )?;

    // SetSelectedSkill(index) / GetSelectedSkill(): visible indices, held by skill id;
    // `GetSelectedSkill` (`0x4d4090`) always answers one number, 0 included.
    g.set(
        "SetSelectedSkill",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SetSelectedSkill(index)` (`0x849f88`, `0x4d4031`).
            let index = binding_abi::index_arg32(lua, index, "Usage: SetSelectedSkill(index)")?;
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

    // GetAdjustedSkillPoints() → 0 here; the reference answers `PLAYER_CHARACTER_POINTS2` less the
    // cost of the points `AddSkillUp` has chosen (`0x4d3cb0`), 0 each (module doc). The stock pane
    // reads it only under its
    // `stepCost` and `rankCost` branches, which 1.12 data never opens.
    g.set(
        "GetAdjustedSkillPoints",
        lua.create_function(|_, ()| Ok(0i64))?,
    )?;

    // AbandonSkill(index): queue the entry's skill id for `CMSG_UNLEARN_SKILL`, changing nothing
    // locally: the reference waits for the skill-field update (vmangos `SetSkill(id,0,0)`).
    g.set(
        "AbandonSkill",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: AbandonSkill(index)` (`0x849ed4`, `0x4d3ac7`).
            let index = binding_abi::index_arg(lua, index, "Usage: AbandonSkill(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(id) = entry_at(&model, index).map(|e| e.skill_id) {
                model.skill_abandons.push(id);
            }
            Ok(())
        })?,
    )?;

    // CancelSkillUps() (`0x4d3e30`): clears every line's chosen points (`0x4d35b0`) and signals
    // `SKILL_LINES_CHANGED`; the stock Close button calls it (`SkillFrame.xml:352`).
    g.set(
        "CancelSkillUps",
        lua.create_function(|lua, ()| {
            super::tick::verb(lua, |model| {
                model.skills_pending.clear();
                model.raise("SKILL_LINES_CHANGED", Vec::new());
            });
            Ok(())
        })?,
    )?;

    // AddSkillUp(index) (`0x4d3c30` over `0x4d32d0`): choose one more point on a skill line, when
    // the line's rank plus the points already chosen is below its maximum (each plus the permanent
    // bonus, as `GetSkillLineInfo` folds them) and the points' cost fits the player's skill points
    // (`0x4d3449`). The cost is 0 (module doc), so only the rank test can refuse. A header, an
    // index past the visible rows (the reference indexes its hidden rows too) and a non-line do
    // nothing; a point taken signals `SKILL_LINES_CHANGED` (`0x4d346f`). Its `SkillRaceClassInfo`
    // gate (`0x4d3342`..`0x4d3354`) holds for every row of the list, which the list build keeps
    // only for lines with such a row (`0x4d2d60`).
    g.set(
        "AddSkillUp",
        lua.create_function(|lua, index: Value| {
            let index = binding_abi::index_arg(lua, index, "Usage: AddSkillUp(index)")?;
            super::tick::verb(lua, |model| {
                let Some(e) = entry_at(model, index) else {
                    return;
                };
                let (rank, max) = held_ranks(e);
                let id = e.skill_id;
                let chosen = model.skills_pending.get(&id).copied().unwrap_or(0);
                if i64::from(chosen) + rank >= max {
                    return;
                }
                *model.skills_pending.entry(id).or_insert(0) += 1;
                model.raise("SKILL_LINES_CHANGED", Vec::new());
            });
            Ok(())
        })?,
    )?;

    // RemoveSkillUp(index) (`0x4d3c70` over `0x4d3480`): give a chosen point back; a line with none
    // chosen does nothing (`0x4d34cb`).
    g.set(
        "RemoveSkillUp",
        lua.create_function(|lua, index: Value| {
            let index = binding_abi::index_arg(lua, index, "Usage: RemoveSkillUp(index)")?;
            super::tick::verb(lua, |model| {
                let Some(id) = entry_at(model, index).map(|e| e.skill_id) else {
                    return;
                };
                let Some(chosen) = model.skills_pending.get_mut(&id).filter(|c| **c > 0) else {
                    return;
                };
                *chosen -= 1;
                if *chosen == 0 {
                    model.skills_pending.remove(&id);
                }
                model.raise("SKILL_LINES_CHANGED", Vec::new());
            });
            Ok(())
        })?,
    )?;

    // AcceptSkillUps() (`0x4d3d40`): returns at once while the chosen points cost nothing
    // (`0xb731d0` is 0, `0x4d3d4e`), which with the costs of module doc is always; the `0x220`
    // it would send and the clear it would make never happen.
    g.set("AcceptSkillUps", lua.create_function(|_, ()| Ok(()))?)?;

    // BuySkillTier(index) (`0x4d3e50`): `is-number`, then a line of the list whose
    // `SkillRaceClassInfo` flags carry `0x8`, or `0x4` with a step word of 0 (`0x4d3f62`..
    // `0x4d3f70`), is priced by `SkillTiers.dbc`: the cost at the step in the line's tier
    // (`0x6de1c0`). A cost of 0 ends it; a cost above the player's skill points
    // (`PLAYER_CHARACTER_POINTS2`, `0x4d3faf`) ends it; else `CMSG_SKILL_BUY_STEP` carries the
    // line id. On the 1.12 data no row carries `0x8` and the one with `0x4` (skill 493) has tier
    // id 0, so nothing is ever sent.
    g.set(
        "BuySkillTier",
        lua.create_function(|lua, index: Value| {
            let index = binding_abi::index_arg(lua, index, "Usage: BuySkillTier(index)")?;
            let send = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                entry_at(&model, index).and_then(|e| {
                    let admitted = e.flags & 0x8 != 0 || (e.step == 0 && e.flags & 0x4 != 0);
                    let cost = model
                        .skill_tiers
                        .get(&e.tier_id)
                        .filter(|_| e.tier_id != 0)
                        .and_then(|c| c.get(e.step as usize))
                        .copied()
                        .unwrap_or(0);
                    // The cost and the points are compared as signed dwords.
                    let points = model.talents.points.1;
                    (admitted && cost != 0 && cost as i32 <= points as i32).then_some(e.skill_id)
                })
            };
            if let Some(skill_id) = send {
                super::lottery::queue(lua, super::lottery::UnservedSend::SkillBuyStep { skill_id });
            }
            Ok(())
        })?,
    )?;

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
            // Fixture rule: Professions (id 2) is abandonable, as the 0x20 flag splits it.
            abandonable: category_id == 2,
            step: 0,
            flags: 0,
            tier_id: 0,
            // Fixture rule: Class Skills (id 3) is single-rank, as the 0x400 flag splits it.
            mono: category_id == 3,
        }
    }

    /// Weapon Skills (order 1) and Professions (order 2), pushed unordered: Swords before Defense.
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
        // 2 headers + 4 entries, category_order ascending, name ascending within.
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

        // Every group starts expanded.
        let (_, h1, e1) = s
            .eval::<(String, i64, Option<i64>)>("local n,h,e = GetSkillLineInfo(1) return n,h,e")
            .unwrap();
        assert_eq!((h1, e1), (1, Some(1)));
    }

    /// `AddSkillUp` / `RemoveSkillUp` / `CancelSkillUps` over the points chosen on a line:
    /// `GetSkillLineInfo`'s `numTempPoints` follows them, a point is refused at the maximum, and
    /// each change signals `SKILL_LINES_CHANGED`.
    #[test]
    fn chosen_skill_points_follow_add_remove_and_cancel() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        // First Aid at 73 of 75 with a permanent bonus of +2: room for 2 more (75 + 2 = 77).
        let fa = st.entries.iter_mut().find(|e| e.skill_id == 129).unwrap();
        fa.value = 73;
        fa.max = 75;
        fa.perm_bonus = 2;
        s.set_skills(st);
        s.run(
            "n = 0 local f = CreateFrame('Frame') f:RegisterEvent('SKILL_LINES_CHANGED') \
             f:SetScript('OnEvent', function() n = n + 1 end)",
        )
        .unwrap();
        // Rows: 1 Weapon Skills, 2 Defense, 3 Swords, 4 Professions, 5 First Aid, 6 Fishing.
        let temp = |s: &UiScript, i: i64| -> i64 {
            s.eval(&format!("local _,_,_,_,t = GetSkillLineInfo({i}) return t"))
                .unwrap()
        };
        let events = |s: &UiScript| s.eval::<i64>("return n").unwrap();
        assert_eq!(temp(&s, 5), 0);
        s.run("AddSkillUp(5)").unwrap();
        s.run("AddSkillUp(5)").unwrap();
        assert_eq!((temp(&s, 5), events(&s)), (2, 2));
        // 75 + 2 chosen = 77 is the cap: a third point is refused, silently.
        s.run("AddSkillUp(5)").unwrap();
        assert_eq!((temp(&s, 5), events(&s)), (2, 2));
        // Another line keeps its own count.
        s.run("AddSkillUp(2)").unwrap();
        assert_eq!((temp(&s, 2), temp(&s, 5), events(&s)), (1, 2, 3));
        s.run("RemoveSkillUp(5)").unwrap();
        assert_eq!((temp(&s, 5), events(&s)), (1, 4));
        // A line with no point chosen, a header, and an index outside the rows do nothing.
        s.run("RemoveSkillUp(6) RemoveSkillUp(1) RemoveSkillUp(0) RemoveSkillUp(99)")
            .unwrap();
        s.run("AddSkillUp(1) AddSkillUp(0) AddSkillUp(-1) AddSkillUp(99)")
            .unwrap();
        assert_eq!((temp(&s, 5), temp(&s, 6), events(&s)), (1, 0, 4));
        // Cancel takes every point back and signals once.
        s.run("CancelSkillUps()").unwrap();
        assert_eq!((temp(&s, 2), temp(&s, 5), events(&s)), (0, 0, 5));
        // A rebuild takes them back too.
        s.run("AddSkillUp(5)").unwrap();
        assert_eq!(temp(&s, 5), 1);
        let st = state();
        s.set_skills(st);
        assert_eq!(temp(&s, 5), 0);
        // A line already at its maximum takes none.
        s.run("AddSkillUp(3)").unwrap();
        let mut st = state();
        st.entries
            .iter_mut()
            .find(|e| e.skill_id == 43)
            .unwrap()
            .value = 300;
        s.set_skills(st);
        let before = events(&s);
        s.run("AddSkillUp(3)").unwrap();
        assert_eq!((temp(&s, 3), events(&s)), (0, before));
    }

    /// `BuySkillTier` (`0x4d3e50`) sends the step purchase for a line admitted by its race-class
    /// flags (`0x8`, or `0x4` with step 0), whose tier prices the step above 0 and within the
    /// player's skill points, and for no other.
    #[test]
    fn buy_skill_tier_follows_the_flags_the_tier_cost_and_the_points() {
        use crate::script::{TalentUiState, UnservedSend::SkillBuyStep};
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        // Rows: 1 Weapon Skills, 2 Defense, 3 Swords, 4 Professions, 5 First Aid, 6 Fishing.
        let fa = st.entries.iter_mut().find(|e| e.skill_id == 129).unwrap();
        fa.flags = 0x8;
        fa.step = 1;
        fa.tier_id = 5;
        s.set_skills(st.clone());
        let mut costs = [0u32; 16];
        costs[0] = 10;
        costs[1] = 40;
        s.set_skill_tiers([(5, costs)]);
        let points = |s: &mut UiScript, p: u32| {
            s.set_talents(TalentUiState {
                points: (0, p),
                ..Default::default()
            });
        };
        let buy = |s: &mut UiScript, row: i64| {
            s.run(&format!("BuySkillTier({row})")).unwrap();
            s.take_unserved_sends()
        };
        // Cost 40 at step 1: sent with 40 points, not with 39, and a header buys nothing.
        points(&mut s, 40);
        assert_eq!(buy(&mut s, 5), [SkillBuyStep { skill_id: 129 }]);
        points(&mut s, 39);
        assert!(buy(&mut s, 5).is_empty(), "above the player's points");
        points(&mut s, 100);
        assert!(buy(&mut s, 4).is_empty(), "a header");
        assert!(buy(&mut s, 99).is_empty() && buy(&mut s, 0).is_empty());
        assert!(buy(&mut s, 6).is_empty(), "Fishing carries no flag");
        // Flag 0x4 admits only a step of 0, at that step's cost.
        let mut other = st.clone();
        let fa = other
            .entries
            .iter_mut()
            .find(|e| e.skill_id == 129)
            .unwrap();
        fa.flags = 0x4;
        s.set_skills(other.clone());
        assert!(buy(&mut s, 5).is_empty(), "flag 4 with a step of 1");
        let fa = other
            .entries
            .iter_mut()
            .find(|e| e.skill_id == 129)
            .unwrap();
        fa.step = 0;
        s.set_skills(other.clone());
        assert_eq!(buy(&mut s, 5), [SkillBuyStep { skill_id: 129 }]);
        // A tier with no cost at the step, tier 0, and no table all price 0.
        points(&mut s, 100);
        let fa = other
            .entries
            .iter_mut()
            .find(|e| e.skill_id == 129)
            .unwrap();
        fa.step = 2;
        fa.flags = 0x8;
        s.set_skills(other.clone());
        assert!(buy(&mut s, 5).is_empty(), "cost[2] is 0");
        let fa = other
            .entries
            .iter_mut()
            .find(|e| e.skill_id == 129)
            .unwrap();
        fa.step = 1;
        fa.tier_id = 0;
        s.set_skills(other);
        assert!(buy(&mut s, 5).is_empty(), "tier 0");
        s.set_skills(st);
        s.set_skill_tiers([]);
        assert!(buy(&mut s, 5).is_empty(), "no SkillTiers rows");
    }

    /// The usage texts and the two that cost nothing: `AcceptSkillUps` and `BuySkillTier`.
    #[test]
    fn skill_point_verbs_raise_their_usage_and_the_empty_ones_do_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());
        for (verb, usage) in [
            ("AddSkillUp", "Usage: AddSkillUp(index)"),
            ("RemoveSkillUp", "Usage: RemoveSkillUp(index)"),
            ("BuySkillTier", "Usage: BuySkillTier(index)"),
        ] {
            for arg in ["", "nil", "'x'", "{}"] {
                let call = format!("{verb}({arg})");
                let err = s.run(&call).expect_err(&call).to_string();
                assert!(err.contains(usage), "{call}: {err}");
            }
            assert_eq!(s.arity(&format!("{verb}('2')")).unwrap(), 0, "{verb}");
        }
        assert_eq!(s.arity("AcceptSkillUps()").unwrap(), 0);
        // Choosing a point and accepting changes nothing the pane shows: the points cost 0, so
        // there is nothing to accept and nothing is cleared.
        s.run("AddSkillUp(2) AcceptSkillUps() BuySkillTier(2)")
            .unwrap();
        let temp: i64 = s
            .eval("local _,_,_,_,t = GetSkillLineInfo(2) return t")
            .unwrap();
        assert_eq!(temp, 1);
    }

    #[test]
    fn abandon_skill_queues_the_entrys_skill_id_and_mutates_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());
        // The 8th return, 1/nil: only Professions rows are abandonable (fixture rule).
        let ab = |s: &mut UiScript, i: i64| {
            s.eval::<Option<i64>>(&format!(
                "local _,_,_,_,_,_,_,ab = GetSkillLineInfo({i}) return ab"
            ))
            .unwrap()
        };
        assert_eq!(ab(&mut s, 2), None, "Defense is not abandonable");
        assert_eq!(ab(&mut s, 5), Some(1), "First Aid is abandonable");
        assert_eq!(ab(&mut s, 1), None, "a header never is");

        // Queued by skill id; a header or an out-of-range index does nothing; the list stays.
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

        // Fold Weapon Skills (row 1): its two entries go, the header stays with isExpanded nil.
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

    #[test]
    fn a_repush_keeps_the_selection_and_re_expands_every_group() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());

        // Selecting a header clears the selection.
        s.run("SetSelectedSkill(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 0);

        // Select Swords (row 3), fold Professions (row 4).
        s.run("SetSelectedSkill(3)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 3);
        s.run("CollapseSkillHeader(4)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 4);

        // A re-push unfolds every group and keeps Swords selected at row 3.
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

        // Header row 1: twelve values, no description (`0x4d3768`).
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

        // Entry row 2 (Defense 180/300, temp +5): thirteen values.
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
        let count = s.arity("GetSkillLineInfo(2)").unwrap();
        let desc = s
            .eval::<String>("local _,_,_,_,_,_,_,_,_,_,_,_,d = GetSkillLineInfo(2) return d")
            .unwrap();
        assert_eq!((count, desc.as_str()), (13, "About Defense."));
        // The cost type is the cost index plus one, so index 0 reads 1.
        let (min_level, cost_type) = s
            .eval::<(i64, i64)>(
                "local _,_,_,_,_,_,_,_,_,_,ml,ct = GetSkillLineInfo(2) return ml,ct",
            )
            .unwrap();
        assert_eq!((min_level, cost_type), (0, 1));
    }

    #[test]
    fn the_permanent_bonus_folds_into_the_numbers_and_the_temporary_one_does_not() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        st.entries[1].perm_bonus = 10; // Defense 180/300, temp +5
        st.entries
            .push(entry(182, "Herbalism", 0, 0, 0, 2, "Professions", 2));
        st.entries.last_mut().unwrap().perm_bonus = 10;
        s.set_skills(st);

        // Defense (row 2): 190/310, the modifier still the temp +5 alone.
        let (rank, modifier, max) = s
            .eval::<(i64, i64, i64)>("local _,_,_,r,_,m,mx = GetSkillLineInfo(2) return r,m,mx")
            .unwrap();
        assert_eq!((rank, modifier, max), (190, 5, 310));

        // Herbalism 0/0 (row 7, untrained so last): the bonus is not added to a zero.
        let (name, rank, max) = s
            .eval::<(String, i64, i64)>("local n,_,_,r,_,_,mx = GetSkillLineInfo(7) return n,r,mx")
            .unwrap();
        assert_eq!((name.as_str(), rank, max), ("Herbalism", 0, 0));
    }

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

    #[test]
    fn a_mono_line_reports_max_rank_one_whatever_the_server_said() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        // Beast Mastery: category 3, so mono by the fixture rule; 300/300 from the server.
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

        // Rows 1 and 2: the Class Skills header and its entry (category_order 0 sorts first).
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
        // Defense, not mono, still reports its real 300.
        assert_eq!(
            s.eval::<i64>("local _,_,_,_,_,_,mx = GetSkillLineInfo(4) return mx")
                .unwrap(),
            300
        );
    }

    /// The count is the assertion: `GetSkillLineInfo(0) == nil` holds for a single nil too, since
    /// Lua compares only the first value.
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
        // The empty pane answers the out-of-range tuple; this pins slot 1.
        assert!(s
            .eval::<bool>("return (GetSkillLineInfo(1)) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetAdjustedSkillPoints()").unwrap(), 0);
        // Collapse, expand and select on an empty pane do nothing.
        s.run("CollapseSkillHeader(0) ExpandSkillHeader(1) SetSelectedSkill(1)")
            .unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 0);
    }
}
