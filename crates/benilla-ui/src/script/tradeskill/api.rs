//! The tradeskill Lua globals over [`super::view`]'s visible rows and filters.

use mlua::{Lua, MultiValue, Value};

use crate::script::binding_abi::{self, number_arg};
use crate::script::item_stats::item_link;
use crate::script::Model;

use super::view::{
    build_groups, first_recipe_index, inv_slot_token, num_rows, present_inv_slots, recipe_at, rows,
    select, selected_visible_index, set_collapsed, Row,
};

/// An optional string as a Lua value (`nil` when absent).
fn opt_str(lua: &Lua, s: Option<&String>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

/// Register the tradeskill globals.
pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetTradeSkillLine(): ("UNKNOWN", 0, 0) with no window open, as the reference answers.
    g.set(
        "GetTradeSkillLine",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (name, rank, max_rank) = match &model.trade_skill {
                Some(t) => (t.line_name.clone(), t.rank, t.max_rank),
                None => ("UNKNOWN".to_string(), 0, 0),
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                Value::Integer(i64::from(rank)),
                Value::Integer(i64::from(max_rank)),
            ]))
        })?,
    )?;

    // GetNumTradeSkills(): the visible rows, headers included.
    g.set(
        "GetNumTradeSkills",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(num_rows(&model) as i64)
        })?,
    )?;

    // GetTradeSkillItemLink(index) (`0x4ff410`): zero values on any miss (a header, no product, an
    // uncached template, which is never queried here), else the product's item link.
    g.set(
        "GetTradeSkillItemLink",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetTradeSkillItemLink(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let link = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|n| rows(&model).get(n).cloned())
                .and_then(|row| match row {
                    Row::Header { .. } => None,
                    Row::Entry(ei) => model.trade_skill.as_ref().map(|t| &t.recipes[ei]),
                })
                .filter(|r| r.product_item != 0)
                .and_then(|r| {
                    model
                        .item_templates
                        .get(&r.product_item)
                        .map(|t| item_link(r.product_item, &t.name, t.quality))
                });
            Ok(match link {
                Some(l) => MultiValue::from_vec(vec![Value::String(lua.create_string(&l)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetTradeSkillReagentItemLink(index, reagentIndex) (`0x4ff800`): always one value, the link
    // or nil; `reagentIndex` counts the non-empty reagent slots, and the usage error's
    // `GetTradeReagentSkillItemLink` is the reference's own typo.
    g.set(
        "GetTradeSkillReagentItemLink",
        lua.create_function(|lua, (index, reagent): (Value, Value)| {
            let usage = "Usage: GetTradeReagentSkillItemLink(index, reagentIndex)";
            let index = number_arg(lua, index, usage)?;
            let reagent = number_arg(lua, reagent, usage)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let link = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|n| rows(&model).get(n).cloned())
                .and_then(|row| match row {
                    Row::Header { .. } => None,
                    Row::Entry(ei) => model.trade_skill.as_ref().map(|t| &t.recipes[ei]),
                })
                .zip(usize::try_from(reagent).ok().and_then(|r| r.checked_sub(1)))
                .and_then(|(r, ri)| r.reagents.get(ri))
                .and_then(|re| {
                    model
                        .item_templates
                        .get(&re.item)
                        .map(|t| item_link(re.item, &t.name, t.quality))
                });
            Ok(match link {
                Some(l) => Value::String(lua.create_string(&l)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetTradeSkillInfo(index): (name, "header", 0, isExpanded) for a header, (name, difficulty,
    // numAvailable, nil) for a recipe, one nil out of range.
    g.set(
        "GetTradeSkillInfo",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(n) = index.checked_sub(1) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let Some(row) = rows(&model).get(n).cloned() else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            match row {
                Row::Header { key, name } => {
                    let expanded = !model.trade_skill_collapsed.contains(&key);
                    Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&name)?),
                        Value::String(lua.create_string("header")?),
                        Value::Integer(0),
                        binding_abi::flag(expanded),
                    ]))
                }
                Row::Entry(ei) => {
                    let r = &model
                        .trade_skill
                        .as_ref()
                        .expect("rows() only yields Entry rows when trade_skill is Some")
                        .recipes[ei];
                    Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&r.name)?),
                        Value::String(lua.create_string(r.difficulty.as_str())?),
                        Value::Integer(i64::from(r.num_available)),
                        Value::Nil,
                    ]))
                }
            }
        })?,
    )?;

    g.set(
        "GetFirstTradeSkill",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(first_recipe_index(&model)))
        })?,
    )?;

    // GetTradeSkillSubClasses(): the group names in order, unfiltered (`0x4ffb60`).
    g.set(
        "GetTradeSkillSubClasses",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            if let Some(t) = model.trade_skill.as_ref() {
                for grp in build_groups(&t.recipes) {
                    out.push(Value::String(lua.create_string(&grp.name)?));
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // Expand/CollapseTradeSkillSubClass(i): the group whose header is visible row i; 0 is all.
    g.set(
        "ExpandTradeSkillSubClass",
        lua.create_function(|lua, id: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, false);
            model.trade_skill_touched = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CollapseTradeSkillSubClass",
        lua.create_function(|lua, id: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, true);
            model.trade_skill_touched = true;
            Ok(())
        })?,
    )?;

    g.set(
        "GetTradeSkillIcon",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            opt_str(lua, recipe_at(&model, index).and_then(|r| r.icon.as_ref()))
        })?,
    )?;

    g.set(
        "GetTradeSkillNumMade",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (min_made, max_made) =
                recipe_at(&model, index).map_or((0, 0), |r| (r.min_made, r.max_made));
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(min_made)),
                Value::Integer(i64::from(max_made)),
            ]))
        })?,
    )?;

    // GetTradeSkillCooldown(index): nil when ready, which the stock Lua tests for truthiness
    // (Blizzard_TradeSkillUI.lua:215).
    g.set(
        "GetTradeSkillCooldown",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(
                match recipe_at(&model, index).and_then(|r| r.cooldown_secs) {
                    Some(secs) => Value::Integer(secs as i64),
                    None => Value::Nil,
                },
            )
        })?,
    )?;

    g.set(
        "GetTradeSkillNumReagents",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(recipe_at(&model, index).map_or(0, |r| r.reagents.len()) as i64)
        })?,
    )?;

    // GetTradeSkillReagentInfo(index, reagentIndex): one nil on a miss; name and icon are nil
    // until the item template arrives.
    g.set(
        "GetTradeSkillReagentInfo",
        lua.create_function(|lua, (index, reagent_index): (usize, usize)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(reagent) = recipe_at(&model, index)
                .and_then(|r| reagent_index.checked_sub(1).and_then(|n| r.reagents.get(n)))
            else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, reagent.name.as_ref())?,
                opt_str(lua, reagent.icon.as_ref())?,
                Value::Integer(i64::from(reagent.need)),
                Value::Integer(i64::from(reagent.have)),
            ]))
        })?,
    )?;

    // GetTradeSkillTools(index): alternating (name, has) pairs, which the stock Lua feeds to
    // `BuildColoredListString` (Blizzard_TradeSkillUI.lua:274).
    g.set(
        "GetTradeSkillTools",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            if let Some(r) = recipe_at(&model, index) {
                for (name, have) in &r.tools {
                    out.push(Value::String(lua.create_string(name)?));
                    out.push(binding_abi::flag(*have));
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetTradeskillRepeatCount(): the lowercase "s" is the 1.12 name.
    g.set(
        "GetTradeskillRepeatCount",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(
                model.trade_skill.as_ref().map_or(0, |t| t.repeat_count),
            ))
        })?,
    )?;

    // SelectTradeSkill and GetTradeSkillSelectionIndex take and answer visible indices.
    g.set(
        "SelectTradeSkill",
        lua.create_function(|lua, index: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            select(&mut model, index);
            Ok(())
        })?,
    )?;
    g.set(
        "GetTradeSkillSelectionIndex",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(selected_visible_index(&model)))
        })?,
    )?;

    // DoTradeSkill(index, count): queues the recipe's spell for `count` crafts, which the app sends
    // as one `CMSG_CAST_SPELL` each.
    g.set(
        "DoTradeSkill",
        lua.create_function(|lua, (index, count): (usize, Option<i64>)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some((spell_id, avail)) =
                recipe_at(&model, index).map(|r| (r.spell_id, r.num_available))
            {
                // The client clamps the repeat to numAvailable, at least 1 (`0x500280`).
                let n = (count.unwrap_or(1).max(1) as u32).min(avail.max(1));
                model.trade_skill_dos.push((spell_id, n));
            }
            Ok(())
        })?,
    )?;

    // CloseTradeSkill(): client-side only, no packet; the app clears its state.
    g.set(
        "CloseTradeSkill",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.trade_skill_close = true;
            Ok(())
        })?,
    )?;

    // The subclass and inv-slot filters (Blizzard_TradeSkillUI.lua:314-414): index i is 1-based
    // into `GetTradeSkillSubClasses` (`0x4ffc70` bounds-checks it) or `GetTradeSkillInvSlots`, and
    // 0 is all. `Set*(i, 1, 1)` shows only i, as every menu click does; `Set*(i, 1)` shows i and
    // `Set*(i, 0)` hides it. Each set fires TRADE_SKILL_UPDATE through the touched flag
    // (`0x4fd710`), and `Get*(0)` asks whether everything is shown. The reference's per-header
    // subclass flag (`+0xc`, mask `0x84dd60`) is kept here as hidden group keys.
    g.set(
        "GetTradeSkillSubClassFilter",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(t) = model.trade_skill.as_ref() else {
                return Ok(binding_abi::flag(false));
            };
            let groups = build_groups(&t.recipes);
            Ok(match index.checked_sub(1) {
                None => binding_abi::flag(
                    !groups
                        .iter()
                        .any(|g| model.trade_skill_subclass_hidden.contains(&g.key)),
                ),
                Some(n) => binding_abi::flag(
                    groups
                        .get(n)
                        .is_some_and(|g| !model.trade_skill_subclass_hidden.contains(&g.key)),
                ),
            })
        })?,
    )?;
    g.set(
        "SetTradeSkillSubClassFilter",
        lua.create_function(
            |lua, (index, on, exclusive): (usize, Option<i64>, Option<i64>)| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let Some(t) = model.trade_skill.as_ref() else {
                    return Ok(());
                };
                let keys: Vec<(u32, u32)> = build_groups(&t.recipes)
                    .into_iter()
                    .map(|g| g.key)
                    .collect();
                let on = on.unwrap_or(1) != 0;
                let exclusive = exclusive.unwrap_or(0) != 0;
                match index.checked_sub(1) {
                    None => model.trade_skill_subclass_hidden.clear(),
                    Some(n) => {
                        let Some(&key) = keys.get(n) else {
                            return Ok(());
                        };
                        if on && exclusive {
                            model.trade_skill_subclass_hidden =
                                keys.iter().copied().filter(|&k| k != key).collect();
                        } else if on {
                            model.trade_skill_subclass_hidden.remove(&key);
                        } else {
                            model.trade_skill_subclass_hidden.insert(key);
                        }
                    }
                }
                model.trade_skill_touched = true;
                Ok(())
            },
        )?,
    )?;
    // GetTradeSkillInvSlots(): the products' slot names in ascending bit order (`0xbde058`).
    g.set(
        "GetTradeSkillInvSlots",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            // A token (`0x84dd70`) missing from the string table answers "" rather than being
            // dropped: the filter verbs index this list by position.
            for bit in present_inv_slots(&model) {
                let word = inv_slot_token(bit)
                    .and_then(|k| crate::strings::global(lua, k))
                    .unwrap_or_default();
                out.push(Value::String(lua.create_string(word)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;
    // The inv-slot pair works on the shown mask (`0x84dd64`); index i is the i-th present slot bit
    // (`0x4ffe60`).
    g.set(
        "GetTradeSkillInvSlotFilter",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            if model.trade_skill.is_none() {
                return Ok(binding_abi::flag(false));
            }
            let bits = present_inv_slots(&model);
            Ok(match index.checked_sub(1) {
                // Everything shown: (present & mask) == present (`0x4fffd0`).
                None => binding_abi::flag(
                    bits.iter()
                        .all(|&b| model.trade_skill_invslot_mask & (1 << b) != 0),
                ),
                Some(n) => binding_abi::flag(
                    bits.get(n)
                        .is_some_and(|&b| model.trade_skill_invslot_mask & (1 << b) != 0),
                ),
            })
        })?,
    )?;
    g.set(
        "SetTradeSkillInvSlotFilter",
        lua.create_function(
            |lua, (index, on, exclusive): (usize, Option<i64>, Option<i64>)| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                if model.trade_skill.is_none() {
                    return Ok(());
                }
                let bits = present_inv_slots(&model);
                let on = on.unwrap_or(1) != 0;
                let exclusive = exclusive.unwrap_or(0) != 0;
                // The mask each path writes, as `0x4fd730` is called.
                match index.checked_sub(1) {
                    None => model.trade_skill_invslot_mask = u32::MAX,
                    Some(n) => {
                        let Some(&bit) = bits.get(n) else {
                            return Ok(());
                        };
                        if on && exclusive {
                            model.trade_skill_invslot_mask = 1 << bit;
                        } else if on {
                            model.trade_skill_invslot_mask |= 1 << bit;
                        } else {
                            model.trade_skill_invslot_mask &= !(1 << bit);
                        }
                    }
                }
                model.trade_skill_touched = true;
                Ok(())
            },
        )?,
    )?;

    Ok(())
}
