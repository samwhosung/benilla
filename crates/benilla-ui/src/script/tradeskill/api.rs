//! The tradeskill Lua globals over [`super::view`]'s visible rows and filters.

use mlua::{Lua, MultiValue, Value};

use crate::script::binding_abi::{self, number_arg};
use crate::script::item_stats::item_link;
use crate::script::Model;

use super::TradeSkillCall;

use super::view::{
    first_recipe_index, fold_target, inv_slot_token, num_rows, present_inv_slots, recipe_at,
    refilter, rows, select, selected_index, set_collapsed, Row,
};

/// The index of the two filter getters: `is-number` (raising the usage error), then `_ftol` minus
/// one as a signed number; negative means the whole list.
fn filter_index(lua: &Lua, v: Option<Value>, usage: &'static str) -> mlua::Result<i32> {
    let v = v.unwrap_or(Value::Nil);
    Ok(number_arg(lua, v, usage)?.wrapping_sub(1))
}

/// The arguments of `SetTradeSkillSubClassFilter` (`0x4ffc70`) and `SetTradeSkillInvSlotFilter`
/// (`0x4ffe60`), checked in the reference's order before any state is touched. Arg 1 passes
/// `lua_isstring` (`0x6f3510`), so a non-numeric string reads as 0; nil and the rest raise the
/// usage line. `trunc(n) - 1` below 0 is "all" and reads nothing further; an index at or past
/// `count` raises `bad`; arg 2 must be a number (`0x6f34d0`) or `missing` raises. Arg 3 is the
/// exclusive flag only when it is a nonzero number, and is read only for an `on` that is nonzero.
struct SetFilterNames {
    usage: &'static str,
    bad: &'static str,
    missing: &'static str,
}

enum SetFilter {
    All,
    One { n: usize, on: bool, exclusive: bool },
}

fn set_filter_args(
    lua: &Lua,
    (index, on, exclusive): (Option<Value>, Option<Value>, Option<Value>),
    names: &SetFilterNames,
    count: usize,
) -> mlua::Result<SetFilter> {
    let index = match index.unwrap_or(Value::Nil) {
        v @ (Value::Integer(_) | Value::Number(_) | Value::String(_)) => {
            binding_abi::ftol(lua.coerce_number(v)?.unwrap_or(0.0))
        }
        _ => return Err(mlua::Error::RuntimeError(names.usage.into())),
    };
    let Some(n) = index.checked_sub(1).filter(|&n| n >= 0) else {
        return Ok(SetFilter::All);
    };
    if n as usize >= count {
        return Err(mlua::Error::RuntimeError(names.bad.into()));
    }
    let number = |lua: &Lua, v: Option<Value>| -> mlua::Result<Option<i32>> {
        Ok(match v {
            Some(v) => lua.coerce_number(v)?.map(binding_abi::ftol),
            None => None,
        })
    };
    let Some(on) = number(lua, on)? else {
        return Err(mlua::Error::RuntimeError(names.missing.into()));
    };
    let exclusive = number(lua, exclusive)?.is_some_and(|x| x != 0);
    Ok(SetFilter::One {
        n: n as usize,
        on: on != 0,
        exclusive,
    })
}

/// An optional string as a Lua value (`nil` when absent).
fn opt_str(lua: &Lua, s: Option<&String>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

/// Register the tradeskill globals.
/// A getter's item lookup that missed: the item cache asked inside the call, with the callback
/// `0x4fdc30`, one per call, and the row's spell as the owner id.
fn ask_item(lua: &Lua, entry: u32, spell_id: u32) -> mlua::Result<()> {
    crate::script::calls::ask(
        lua,
        crate::script::RecordAsk::Item {
            entry,
            owner: crate::script::calls::spell_owner(spell_id),
            callback: crate::script::ItemCallback::TradeSkill,
        },
    )
}

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

    // GetNumTradeSkills() (`0x4fd800`): the displayed rows, headers included.
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

    // GetTradeSkillItemStats(index) (`0x4fdec0`): one string per tooltip line of the item the
    // recipe
    // creates ([`super::itemstats`]), and zero values on any miss: an index outside the full row
    // list, a header, a spell with no product, or a template not cached yet, which is asked for
    // (`0x4fdf95`) so the next call finds it.
    g.set(
        "GetTradeSkillItemStats",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetTradeSkillItemStats(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let product = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|n| rows(&model).get(n).cloned())
                .and_then(|row| match row {
                    Row::Header { .. } => None,
                    Row::Entry(ei) => model.trade_skill.as_ref().map(|t| &t.recipes[ei]),
                })
                .map(|r| {
                    let in_flight = r.icon_pending == Some(r.product_item);
                    (r.product_item, in_flight, r.spell_id)
                });
            let Some((product, in_flight, spell_id)) = product else {
                return Ok(MultiValue::new());
            };
            let Some(view) = model.item_templates.get(&product).cloned() else {
                if product != 0 {
                    // The view the VM holds no copy of, which the app pushes.
                    model.item_stat_asks.insert(product);
                    drop(model);
                    // The item cache's miss asks and waits with the callback `0x4fdc30` inside
                    // the call (`0x4fdf77`).
                    if in_flight {
                        ask_item(lua, product, spell_id)?;
                    }
                }
                return Ok(MultiValue::new());
            };
            let req = model.player_req.clone();
            let player = model.unit("player").map(|u| (u.pvp_team, u.sex == 3));
            let knows = view.required_spell != 0
                && model
                    .spellbook
                    .slots
                    .iter()
                    .any(|s| s.spell_id == view.required_spell);
            drop(model);
            let facts = super::itemstats::PlayerFacts {
                // A player whose level is unpushed declines to judge, as `item_refusal` does.
                present: req.level != 0 || player.is_some(),
                pvp_team: player.map_or(0, |p| p.0),
                female: player.is_some_and(|p| p.1),
                knows_required_spell: knows,
            };
            let get = |key: &str| crate::strings::global(lua, key).unwrap_or_default();
            let lines = super::itemstats::item_stat_lines(&view, &req, facts, &get);
            let mut out = Vec::with_capacity(lines.len());
            for line in &lines {
                out.push(Value::String(lua.create_string(line)?));
            }
            Ok(MultiValue::from_vec(out))
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
    // numAvailable, nil) for a recipe, nil, nil, 0, nil out of range (`0x4fd8d0`).
    g.set(
        "GetTradeSkillInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTradeSkillInfo(index)` (`0x84e2a4`, `0x4fd839`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetTradeSkillInfo(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let miss = || binding_abi::tuple(&[None, None, Some(0), None]);
            let Some(n) = index.checked_sub(1) else {
                return Ok(miss());
            };
            // Bounded by the full count (`0x4fd861`).
            let Some(row) = rows(&model).get(n).cloned() else {
                return Ok(miss());
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
            if model.trade_skill.is_some() {
                for grp in &model.trade_skill_headers {
                    out.push(Value::String(lua.create_string(&grp.name)?));
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // Expand/CollapseTradeSkillSubClass(i): the group whose header is row i, every group for an i
    // below 1 ([`fold_target`]); any other row raises the verb's own line.
    g.set(
        "ExpandTradeSkillSubClass",
        lua.create_function(|lua, id: Value| {
            // `is-number` else `Usage: ExpandTradeSkillSubClass(index)` (`0x84e914`, `0x500151`).
            let id = number_arg(lua, id, "Usage: ExpandTradeSkillSubClass(index)")?;
            // The reference's own wording, "skill line" (`0x84e8e8`, `0x50018e`).
            fold(lua, id, false, "Bad skill line in ExpandTradeSkillSubClass")
        })?,
    )?;
    g.set(
        "CollapseTradeSkillSubClass",
        lua.create_function(|lua, id: Value| {
            // `is-number` else `Usage: CollapseTradeSkillSubClass(index)` (`0x84e8bc`, `0x5000d1`).
            let id = number_arg(lua, id, "Usage: CollapseTradeSkillSubClass(index)")?;
            // `0x84e890`, `0x50010d`.
            fold(lua, id, true, "Bad sub class in CollapseTradeSkillSubClass")
        })?,
    )?;

    g.set(
        "GetTradeSkillIcon",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTradeSkillIcon(index)` (`0x84d3d0`, `0x4fdb11`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetTradeSkillIcon(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(r) = recipe_at(&model, index) else {
                return Ok(Value::Nil);
            };
            let (icon, pending, spell_id) = (r.icon.clone(), r.icon_pending, r.spell_id);
            drop(model);
            // A record in flight is asked with the callback `0x4fdc30` inside the call
            // (`0x4fdb90`).
            if let Some(entry) = pending {
                ask_item(lua, entry, spell_id)?;
            }
            opt_str(lua, icon.as_ref())
        })?,
    )?;

    // GetTradeSkillNumMade(index) (`0x4fdc50`): 1, 1 on a header or past the rows (`0x4fdd13`).
    g.set(
        "GetTradeSkillNumMade",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTradeSkillNumMade(index)` (`0x84e308`, `0x4fdc6a`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetTradeSkillNumMade(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (min_made, max_made) =
                recipe_at(&model, index).map_or((1, 1), |r| (r.min_made, r.max_made));
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(min_made)),
                Value::Integer(i64::from(max_made)),
            ]))
        })?,
    )?;

    // GetTradeSkillCooldown(index) (`0x4fda20`): the start plus the duration minus the clock,
    // scaled by 0.001, read at the call; nil when ready, which the stock Lua tests for truthiness
    // (Blizzard_TradeSkillUI.lua:215). A held cooldown has not started, so its whole duration is
    // left.
    g.set(
        "GetTradeSkillCooldown",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTradeSkillCooldown(index)` (`0x84e2e4`, `0x4fda37`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetTradeSkillCooldown(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let left = recipe_at(&model, index)
                .and_then(|r| r.cooldown)
                .map(|(start_ms, duration_ms, enabled)| {
                    let duration = f64::from(duration_ms) / 1000.0;
                    if enabled {
                        start_ms as f64 / 1000.0 + duration - model.now
                    } else {
                        duration
                    }
                })
                .filter(|&left| left > 0.0);
            Ok(left.map_or(Value::Nil, Value::Number))
        })?,
    )?;

    g.set(
        "GetTradeSkillNumReagents",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTradeSkillNumReagents(index)` (`0x84e5cc`, `0x4ff525`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: GetTradeSkillNumReagents(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(recipe_at(&model, index).map_or(0, |r| r.reagents.len()) as i64)
        })?,
    )?;

    // GetTradeSkillReagentInfo(index, reagentIndex): four nils on a miss (`0x4ff68b`); name and
    // icon are nil until the item template arrives.
    g.set(
        "GetTradeSkillReagentInfo",
        lua.create_function(|lua, (index, reagent_index): (Value, Value)| {
            // Both `is-number` else `Usage: GetTradeSkillReagentInfo(index, reagentIndex)`
            // (`0x84e5f4`, `0x4ff7df`).
            let index = binding_abi::index_arg(
                lua,
                index,
                "Usage: GetTradeSkillReagentInfo(index, reagentIndex)",
            )?;
            let reagent_index = binding_abi::index_arg(
                lua,
                reagent_index,
                "Usage: GetTradeSkillReagentInfo(index, reagentIndex)",
            )?;
            let reagent = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                recipe_at(&model, index).and_then(|r| {
                    let n = reagent_index.checked_sub(1)?;
                    Some((r.reagents.get(n)?.clone(), r.spell_id))
                })
            };
            let Some((reagent, spell_id)) = reagent else {
                return Ok(binding_abi::tuple(&[None; 4]));
            };
            if reagent.name.is_none() {
                // The miss asks with the callback `0x4fdc30` inside the call (`0x4ff6cd`).
                ask_item(lua, reagent.item, spell_id)?;
            }
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTradeSkillTools(index)` (`0x84e668`, `0x4ff9ab`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetTradeSkillTools(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(r) = recipe_at(&model, index) else {
                return Ok(MultiValue::new());
            };
            let (tools, pending, spell_id) = (r.tools.clone(), r.tools_pending.clone(), r.spell_id);
            drop(model);
            // Each totem in flight is asked with the callback `0x4fdc30` inside the call
            // (`0x4ffac6`).
            for entry in pending {
                ask_item(lua, entry, spell_id)?;
            }
            let mut out = Vec::new();
            for (name, have) in &tools {
                out.push(Value::String(lua.create_string(name)?));
                out.push(binding_abi::flag(*have));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetTradeskillRepeatCount(): the lowercase "s" is the 1.12 name. The armed count while the
    // armed spell is the selected recipe's and above 1, else 1 (`0x500236`-`0x500251`).
    g.set(
        "GetTradeskillRepeatCount",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let armed = model.trade_skill_repeat_spell;
            let count = model.trade_skill_repeat_count;
            let latched = armed != 0 && armed == model.trade_skill_selected_spell && count > 1;
            Ok(i64::from(if latched { count } else { 1 }))
        })?,
    )?;

    // SelectTradeSkill and GetTradeSkillSelectionIndex take and answer indices into the whole
    // array (`0x4fc9b0`, `0x4fc9e0`).
    g.set(
        "SelectTradeSkill",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SelectTradeSkill(index)` (`0x84e2c4`, `0x4fd9c1`).
            let index = binding_abi::index_arg32(lua, index, "Usage: SelectTradeSkill(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            select(&mut model, index);
            Ok(())
        })?,
    )?;
    g.set(
        "GetTradeSkillSelectionIndex",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(selected_index(&model)))
        })?,
    )?;

    // DoTradeSkill(index, count) (`0x500280`): more than one arms the repeat and signals
    // `UPDATE_TRADESKILL_RECAST` (`0x500307` → `0x4fd770`), then the host casts the recipe's
    // spell inside the call (`0x500314`); the repeat's hook recasts the rest, one
    // `CMSG_CAST_SPELL` each.
    g.set(
        "DoTradeSkill",
        lua.create_function(|lua, (index, count): (Value, Option<Value>)| {
            // `is-number` else `Usage: DoTradeSkill(index [, repeat])` (`0x84e93c`, `0x500291`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: DoTradeSkill(index [, repeat])")?;
            // A count that is no number reads as 1 (`0x5002c5`-`0x5002cc`).
            let count = count
                .and_then(|v| lua.coerce_number(v).ok().flatten())
                .map_or(1, |n| i64::from(super::super::binding_abi::ftol(n)));
            // The cast is `TryCastSpell` (`0x500314` → `0x6e5a90`), behind the gate's tag 2
            // (`0x6e4bc1`); the repeat is armed ahead of it (`0x500307`), so a refused call arms
            // it.
            let may_cast = crate::script::protected::gate(lua, crate::script::protected::Tag::CAST);
            let cast = crate::script::tick::verb(lua, |model| {
                let (spell_id, avail) =
                    recipe_at(model, index).map(|r| (r.spell_id, r.num_available))?;
                // The count, compared unsigned, clamps to numAvailable (`0x5002f5`-`0x5002fc`);
                // above 1 it arms the repeat (`0x500301` → `0x4fd770`).
                let n = (count as i32 as u32).min(avail);
                if n > 1 {
                    model.trade_skill_repeat_spell = spell_id;
                    model.trade_skill_repeat_count = n;
                    model.raise("UPDATE_TRADESKILL_RECAST", Vec::new());
                }
                may_cast.then_some((spell_id, n))
            });
            match cast {
                Some((spell_id, count)) => {
                    super::submit(lua, TradeSkillCall::Do { spell_id, count })
                }
                None => Ok(()),
            }
        })?,
    )?;

    // CloseTradeSkill() (`0x4fd7f0` → `0x4fc870`): client-side only, no packet. It zeroes the
    // open marker alone (`[0xbde040]`), so `GetNumTradeSkills` (`0x4fd800`) still answers the
    // held list, disarms an armed repeat with its `UPDATE_TRADESKILL_RECAST` (`0x4fd790`), and
    // signals `TRADE_SKILL_CLOSE` unconditionally (`0x4fc884`); the host zeroes the engine's
    // marker inside the call.
    g.set(
        "CloseTradeSkill",
        lua.create_function(|lua, ()| {
            super::submit(lua, TradeSkillCall::Close)?;
            crate::script::tick::verb(lua, |model| {
                model.trade_skill_open = false;
                super::disarm_repeat(model);
                model.raise("TRADE_SKILL_CLOSE", Vec::new());
            });
            Ok(())
        })?,
    )?;

    // The subclass and inv-slot filters (Blizzard_TradeSkillUI.lua:314-414): index i is 1-based
    // into `GetTradeSkillSubClasses` (`0x4ffc70` bounds-checks it) or `GetTradeSkillInvSlots`, and
    // 0 is all. `Set*(i, 1, 1)` shows only i, as every menu click does; `Set*(i, 1)` shows i and
    // `Set*(i, 0)` hides it. Each set re-sorts and signals TRADE_SKILL_UPDATE in place
    // (`0x4fd710`, `0x4fd730`), and `Get*(0)` asks whether everything is shown. The reference's
    // per-header
    // subclass flag (`+0xc`, mask `0x84dd60`) is kept here as hidden group keys.
    g.set(
        "GetTradeSkillSubClassFilter",
        lua.create_function(|lua, index: Option<Value>| {
            // `is-number`, then `trunc(n) - 1` as a signed index (`0x4ffdcd`): anything at or below
            // 0 asks about the whole list, one past the end raises (`0x4ffe0e`, `0x84e738`).
            let n = filter_index(lua, index, "Usage: GetTradeSkillSubClassFilter(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let groups = if model.trade_skill.is_some() {
                model.trade_skill_headers.clone()
            } else {
                Vec::new()
            };
            let shown = |g: &super::view::TradeSkillGroup| {
                !model.trade_skill_subclass_hidden.contains(&g.key)
            };
            if n < 0 {
                return Ok(binding_abi::flag(groups.iter().all(shown)));
            }
            match groups.get(n as usize) {
                Some(g) => Ok(binding_abi::flag(shown(g))),
                None => Err(mlua::Error::RuntimeError(
                    "Bad sub class in GetTradeSkillSubClassFilter".into(),
                )),
            }
        })?,
    )?;
    g.set(
        "SetTradeSkillSubClassFilter",
        lua.create_function(|lua, args: (Option<Value>, Option<Value>, Option<Value>)| {
            let count = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                if model.trade_skill.is_some() {
                    model.trade_skill_headers.len()
                } else {
                    0
                }
            };
            let names = SetFilterNames {
                usage: "Usage: SetTradeSkillSubClassFilter(index [, on\\off, exclusive])",
                bad: "Bad sub class in SetTradeSkillSubClassFilter",
                missing: "Missing on//off parameter in SetTradeSkillSubClassFilter",
            };
            let filter = set_filter_args(lua, args, &names, count)?;
            crate::script::tick::verb(lua, |model| set_subclass_filter(model, filter));
            Ok(())
        })?,
    )?;
    // GetTradeSkillInvSlots(): the products' slot names in ascending bit order (`0xbde058`).
    g.set(
        "GetTradeSkillInvSlots",
        lua.create_function(|lua, ()| {
            let bits = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                present_inv_slots(&model)
            };
            let mut out = Vec::new();
            // A token (`0x84dd70`) missing from the string table answers "" rather than being
            // dropped: the filter verbs index this list by position. Each is a `_G` read whose
            // `__index` may call any verb, so the Model is not borrowed.
            for bit in bits {
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
        lua.create_function(|lua, index: Option<Value>| {
            // The same shape over the present inventory-type bits (`0x500018`): at or below 0
            // asks about all of them, past the last present one raises (`0x50006e`, `0x84e838`).
            let n = filter_index(lua, index, "Usage: GetTradeSkillInvSlotFilter(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let bits = if model.trade_skill.is_some() {
                present_inv_slots(&model)
            } else {
                Vec::new()
            };
            let shown = |b: &u32| model.trade_skill_invslot_mask & (1 << b) != 0;
            // Everything shown: (present & mask) == present (`0x4fffd0`).
            if n < 0 {
                return Ok(binding_abi::flag(bits.iter().all(shown)));
            }
            match bits.get(n as usize) {
                Some(b) => Ok(binding_abi::flag(shown(b))),
                None => Err(mlua::Error::RuntimeError(
                    "Bad inv type in GetTradeSkillInvSlotFilter".into(),
                )),
            }
        })?,
    )?;
    g.set(
        "SetTradeSkillInvSlotFilter",
        lua.create_function(|lua, args: (Option<Value>, Option<Value>, Option<Value>)| {
            let count = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                if model.trade_skill.is_some() {
                    present_inv_slots(&model).len()
                } else {
                    0
                }
            };
            let names = SetFilterNames {
                usage: "Usage: SetTradeSkillInvSlotFilter(index [, on\\off, exclusive])",
                bad: "Bad inv slot in SetTradeSkillInvSlotFilter",
                missing: "Missing on//off parameter in SetTradeSkillInvSlotFilter",
            };
            let filter = set_filter_args(lua, args, &names, count)?;
            crate::script::tick::verb(lua, |model| set_inv_slot_filter(model, filter));
            Ok(())
        })?,
    )?;

    Ok(())
}

/// The filter and fold setters' tail (`0x4fd710`, `0x4fd730`, `0x4fd750`): the recompute and its
/// sorts over the array as it stands (`0x4fd180`), then `TRADE_SKILL_UPDATE` (`0x13a`) in place.
fn raise_update(model: &mut Model) {
    refilter(model);
    model.raise("TRADE_SKILL_UPDATE", Vec::new());
}

/// A fold verb past its usage check (`0x5000c0`, `0x500140`): the target, or the verb's `bad` line
/// with nothing touched and no update; then the mask write and its tail (`0x4fd750`).
fn fold(lua: &Lua, index: i32, collapse: bool, bad: &'static str) -> mlua::Result<()> {
    crate::script::tick::verb(lua, |model| {
        let target = fold_target(model, index)?;
        set_collapsed(model, target, collapse);
        raise_update(model);
        Some(())
    })
    .ok_or_else(|| mlua::Error::RuntimeError(bad.into()))
}

/// `SetTradeSkillSubClassFilter`'s write (`0x4ffc70` → `0x4fd710`), the arguments already checked.
fn set_subclass_filter(model: &mut Model, filter: SetFilter) {
    match filter {
        SetFilter::All => model.trade_skill_subclass_hidden.clear(),
        SetFilter::One { n, on, exclusive } => {
            if model.trade_skill.is_none() {
                return;
            }
            let keys: Vec<(u32, u32)> = model.trade_skill_headers.iter().map(|g| g.key).collect();
            let Some(&key) = keys.get(n) else {
                return;
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
    raise_update(model);
}

/// `SetTradeSkillInvSlotFilter`'s write (`0x4ffe60` → `0x4fd730`), the arguments already checked.
fn set_inv_slot_filter(model: &mut Model, filter: SetFilter) {
    match filter {
        SetFilter::All => model.trade_skill_invslot_mask = u32::MAX,
        SetFilter::One { n, on, exclusive } => {
            let bits = present_inv_slots(model);
            let Some(&bit) = bits.get(n) else {
                return;
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
    raise_update(model);
}
