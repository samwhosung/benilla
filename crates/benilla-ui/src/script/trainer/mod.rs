//! The trainer bindings the stock `Blizzard_TrainerUI` window calls, over the snapshot the app
//! pushes ([`UiScript::set_trainer`]). `SMSG_TRAINER_LIST` is flat; the client builds a collapsible
//! tree of header and service rows over it, every Lua index is 1-based into that tree, and the
//! trainer type (`ds:0xb73a08`) picks the row comparator (`0x4d8561`), the group key and the
//! header comparator (`0x4d7786`, `0x4d79eb`).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// A service's state (the wire's `TrainerSpellState`): green learnable, red gated, gray known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrainerServiceCategory {
    #[default]
    Available,
    Unavailable,
    Used,
}

impl TrainerServiceCategory {
    /// `GetTrainerServiceInfo`'s type string, which the stock window colours each row by.
    fn era_str(self) -> &'static str {
        match self {
            TrainerServiceCategory::Available => "available",
            TrainerServiceCategory::Unavailable => "unavailable",
            TrainerServiceCategory::Used => "used",
        }
    }

    /// The filter-flag slot this category occupies ([`Model::trainer_filter`]).
    fn filter_slot(self) -> usize {
        match self {
            TrainerServiceCategory::Available => 0,
            TrainerServiceCategory::Unavailable => 1,
            TrainerServiceCategory::Used => 2,
        }
    }

    /// The record's state byte `[+0x30]`, which `0x4d8ba0` maps to the type string; at type 1 it is
    /// also a sort key ([`talent_order`]), so it stays apart from the equal filter slot.
    fn state_key(self) -> u8 {
        match self {
            TrainerServiceCategory::Available => 0,
            TrainerServiceCategory::Unavailable => 1,
            TrainerServiceCategory::Used => 2,
        }
    }

    fn from_filter_str(s: &str) -> Option<Self> {
        match s {
            "available" => Some(TrainerServiceCategory::Available),
            "unavailable" => Some(TrainerServiceCategory::Unavailable),
            "used" => Some(TrainerServiceCategory::Used),
            _ => None,
        }
    }
}

/// A service's skill requirement (`GetTrainerServiceSkillReq`). The wire has no per-gate bit, so
/// `met` comes from the service's `category`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainerSkillReq {
    pub name: String,
    pub rank: u32,
    pub met: bool,
}

/// A prerequisite ability (`GetTrainerServiceAbilityReq`); `met` is whether the player knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainerAbilityReq {
    pub name: String,
    pub met: bool,
}

/// What `GameTooltip:SetTrainerService` (`0x5338b0`) describes for a row, resolved by the app; the
/// reference only picks the spell builder `0x52e610` or the item builder `0x52b650`. Not derivable
/// from [`TrainerService::texture`]: the icon (`0x4d8f50`) gates on the trainer type and shows the
/// wire spell, while the tooltip describes the taught one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrainerTooltip {
    /// The item builder on this item id, for a taught spell with `SPELL_ATTR_IS_TRADESKILL`; 0 or
    /// an uncached template renders empty, as the reference's builder does on a cache miss.
    Item(u32),
    /// The spell builder on the taught spell, or on the wire spell when no learn slot resolved.
    Spell {
        spell_id: u32,
        /// The builder's altCaster argument, which drops the totem and reagent lines (`0x52ed43`,
        /// `0x52f393`); set when the matched slot is `SPELL_EFFECT_LEARN_PET_SPELL`.
        alt_caster: bool,
    },
}

impl Default for TrainerTooltip {
    fn default() -> Self {
        TrainerTooltip::Spell {
            spell_id: 0,
            alt_caster: false,
        }
    }
}

/// One service, resolved by the app from the wire `TrainerSpell`; its place in
/// [`TrainerState::services`] is wire order, not display order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerService {
    /// The wire spell id, which `CMSG_TRAINER_BUY_SPELL` names.
    pub spell_id: u32,
    /// The wire spell's name; `None` (before `Spell.dbc` loads) answers nil, shown as `UNKNOWN`.
    pub name: Option<String>,
    /// The wire spell's rank text, such as "Rank 2".
    pub subtext: Option<String>,
    pub texture: Option<String>,
    pub description: String,
    /// In copper, already reputation-discounted by the server.
    pub cost: u32,
    /// A primary profession's first rank: the stock window's `cpCost2 > 0`, which asks to confirm.
    pub prof_first_rank: bool,
    pub category: TrainerServiceCategory,
    pub level_req: u32,
    pub skill_req: Option<TrainerSkillReq>,
    pub ability_reqs: Vec<TrainerAbilityReq>,
    pub is_trade_skill: bool,
    /// The tree group: the taught spell's `SkillLine` at types 0, 1 and 3, where 0 (unresolved)
    /// drops the service and a known service at type 1 takes `TRAINER_GROUP_KNOWN`; at type 2, 1
    /// when the wire spell has a `SKILL_STEP` (44) effect, else 2 (`0x4d77b6`), so a tradeskill
    /// trainer drops nothing.
    pub group_key: u32,
    /// The header text: the skill line's name, `KNOWN_TALENTS_HEADER` for the known group, or at
    /// type 2 `TRADESKILL_SERVICE_STEP`/`_LEARN`.
    pub group_name: String,
    pub tooltip: TrainerTooltip,
}

/// One header and its services, built by [`UiScript::set_trainer`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerGroup {
    /// Also the collapse key.
    pub key: u32,
    pub name: String,
    /// Positions into [`TrainerState::services`], in display order.
    pub services: Vec<usize>,
}

/// The open trainer, pushed whole by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerState {
    pub services: Vec<TrainerService>,
    /// `SMSG_TRAINER_LIST`'s trailing string.
    pub greeting: String,
    /// `SMSG_TRAINER_LIST`'s trainer type (`ds:0xb73a08`): 0 class, 1 mount, 2 tradeskill, 3 pet.
    pub trainer_type: u32,
    /// Built by [`UiScript::set_trainer`], not pushed by the app.
    pub groups: Vec<TrainerGroup>,
}

impl super::UiScript {
    /// Push the open trainer, or clear it with `None`. A push builds the tree ([`build_groups`])
    /// and keeps only the collapsed groups that still exist, so a fold survives a content update.
    pub fn set_trainer(&mut self, state: Option<TrainerState>) {
        let mut model = self.model_mut();
        match state {
            None => {
                model.trainer_selection = None;
                model.trainer_collapsed.clear();
                model.trainer = None;
            }
            Some(mut s) => {
                s.groups = build_groups(&s.services, s.trainer_type);
                let live: std::collections::HashSet<u32> = s.groups.iter().map(|g| g.key).collect();
                model.trainer_collapsed.retain(|sl| live.contains(sl));
                model.trainer = Some(s);
            }
        }
    }

    /// Reset what each `SMSG_TRAINER_LIST` resets in the reference (`0x4d7560`): the filter to mask
    /// 3, or 5 at a mount trainer (`0x4d75d9`), the collapse set and the selection. Call it per
    /// packet, never per snapshot: the repaint path `0x4d7d40` touches none of the three.
    ///
    /// Deviation: the selection clears to 0, because the header the reference re-selects
    /// (`0x4d7b42`, reading 1) has no spell id; the stock window's `> 1` test treats 0 and 1 alike.
    pub fn reset_trainer_list_state(&mut self, trainer_type: u32) {
        let mut model = self.model_mut();
        // Mask 5 at a mount trainer shows available and known services, so the known group shows.
        model.trainer_filter = if trainer_type == TRAINER_TYPE_MOUNT {
            [true, false, true]
        } else {
            [true, true, false]
        };
        model.trainer_collapsed.clear();
        model.trainer_selection = None;
    }

    /// Drain the spell ids `BuyTrainerService` queued, each already resolved from its row.
    pub fn take_trainer_buys(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().trainer_buys)
    }

    /// Whether `CloseTrainer` was called since the last drain; the close sends no packet.
    pub fn take_trainer_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().trainer_close)
    }
}

/// One row: a header by group index, or a service by its position in [`TrainerState::services`].
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Service(usize),
}

/// Folds case, raw bytes breaking ties. The reference's row comparators call the case-sensitive
/// `0x64a480`, its header comparator the case-insensitive `0x64a4c0` (`0x4d7c12`).
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

const TRAINER_TYPE_TRADESKILL: u32 = 2;
/// The mount trainer type, which the client calls "talent".
const TRAINER_TYPE_MOUNT: u32 = 1;

/// The group a type 1 trainer files its `used` services under: the client's signed -1 key
/// (`0x4d77e8`), headed `KNOWN_TALENTS_HEADER` and sorted before every other header by `0x4d7b90`.
pub const TRAINER_GROUP_KNOWN: u32 = u32::MAX;

/// The record's required-skill value `[+0x1c]`, 0 with no skill gate.
fn skill_value(s: &TrainerService) -> u32 {
    s.skill_req.as_ref().map_or(0, |r| r.rank)
}

/// Rows at a class or pet trainer (`0x4d85c0`; `0x4d8561` sends pet here too): required level,
/// skill value, name, then rank, all ascending.
fn class_order(a: &TrainerService, b: &TrainerService) -> std::cmp::Ordering {
    let name = |s: &TrainerService| s.name.clone().unwrap_or_default();
    let rank = |s: &TrainerService| s.subtext.clone().unwrap_or_default();
    a.level_req
        .cmp(&b.level_req)
        .then_with(|| skill_value(a).cmp(&skill_value(b)))
        .then_with(|| collate(&name(a), &name(b)))
        .then_with(|| collate(&rank(a), &rank(b)))
}

/// Rows at a tradeskill trainer (`0x4d8760`): required skill value ascending (`0x4d87d9`), then
/// name. It has no level key and no rank key.
fn tradeskill_order(a: &TrainerService, b: &TrainerService) -> std::cmp::Ordering {
    let name = |s: &TrainerService| s.name.clone().unwrap_or_default();
    skill_value(a)
        .cmp(&skill_value(b))
        .then_with(|| collate(&name(a), &name(b)))
}

/// Rows at a mount trainer (`0x4d8850`): the state byte ascending (`0x4d88f1`: available,
/// unavailable, used), then name.
fn talent_order(a: &TrainerService, b: &TrainerService) -> std::cmp::Ordering {
    let name = |s: &TrainerService| s.name.clone().unwrap_or_default();
    a.category
        .state_key()
        .cmp(&b.category.state_key())
        .then_with(|| collate(&name(a), &name(b)))
}

/// Group the services by [`TrainerService::group_key`], dropping key 0, and sort each group's rows
/// by the type's comparator; headers sort by raw key at type 2 (`0x4d7c30`), else by name
/// (`0x4d7b90`).
fn build_groups(services: &[TrainerService], trainer_type: u32) -> Vec<TrainerGroup> {
    let mut map: std::collections::HashMap<u32, TrainerGroup> = std::collections::HashMap::new();
    for (i, s) in services.iter().enumerate() {
        if s.group_key == 0 {
            continue;
        }
        map.entry(s.group_key)
            .or_insert_with(|| TrainerGroup {
                key: s.group_key,
                name: s.group_name.clone(),
                services: Vec::new(),
            })
            .services
            .push(i);
    }
    let mut groups: Vec<TrainerGroup> = map.into_values().collect();
    let order = match trainer_type {
        TRAINER_TYPE_TRADESKILL => tradeskill_order,
        TRAINER_TYPE_MOUNT => talent_order,
        _ => class_order,
    };
    for g in &mut groups {
        g.services
            .sort_by(|&a, &b| order(&services[a], &services[b]));
    }
    if trainer_type == TRAINER_TYPE_TRADESKILL {
        groups.sort_by_key(|g| g.key);
    } else {
        // `0x4d7b90`: the known group first, then name. Its middle key, `hdr[+0x24] == 0` first, is
        // untraced and not built.
        groups.sort_by(|a, b| {
            (a.key != TRAINER_GROUP_KNOWN)
                .cmp(&(b.key != TRAINER_GROUP_KNOWN))
                .then_with(|| collate(&a.name, &b.name))
                .then_with(|| a.key.cmp(&b.key))
        });
    }
    groups
}

/// The client's whole row array and its visible count: visible rows in display order, then every
/// hidden row in a tail. The finalizer `0x4d8410` only clears a hidden record's visible flag
/// (`0x4d8546`) and sorts it behind (`0x4d857e`), and the getters bound on the total
/// (`0x4d89b0`, `ds:0xb73a10`), so a hidden row keeps its index. A group the filter empties hides
/// its header too (`0x4d8535`); a collapsed one keeps it (`0x4d853d`). The tail's order here is
/// tree order, not the client's; the stock window reaches the tail only by the selection's index.
fn rows(model: &Model) -> (Vec<Row>, usize) {
    let Some(t) = model.trainer.as_ref() else {
        return (Vec::new(), 0);
    };
    let mut shown = Vec::new();
    let mut tail = Vec::new();
    for (gi, g) in t.groups.iter().enumerate() {
        let (passes, filtered): (Vec<usize>, Vec<usize>) = g
            .services
            .iter()
            .copied()
            .partition(|&si| model.trainer_filter[t.services[si].category.filter_slot()]);
        if passes.is_empty() {
            // The filter hid every service, so the header goes too.
            tail.push(Row::Header(gi));
        } else {
            shown.push(Row::Header(gi));
            // A collapsed group keeps its header on screen and parks its services behind.
            let into = if model.trainer_collapsed.contains(&g.key) {
                &mut tail
            } else {
                &mut shown
            };
            into.extend(passes.into_iter().map(Row::Service));
        }
        tail.extend(filtered.into_iter().map(Row::Service));
    }
    let visible = shown.len();
    shown.extend(tail);
    (shown, visible)
}

/// The service at a 1-based row index; `None` for a header. `GameTooltip:SetTrainerService`
/// resolves its index through this too, never through a raw `services` position.
pub(super) fn service(model: &Model, index: usize) -> Option<&TrainerService> {
    let n = index.checked_sub(1)?;
    match rows(model).0.get(n)? {
        Row::Service(si) => model.trainer.as_ref()?.services.get(*si),
        Row::Header(_) => None,
    }
}

/// `GetNumTrainerServices` (`0x4d8d90`): the visible count `ds:0xb73a18`, not the array's length.
fn num_services(model: &Model) -> usize {
    rows(model).1
}

/// `GetTrainerSelectionIndex`: the selected service's 1-based row, past the visible count while it
/// is hidden; `None` when nothing is selected or it left the list. The selection is the record's
/// spell id (`0x4d8e60` stores it at `ds:0xb73a0c`, `0x4d8f10` scans for it at `0x4d7543`), so no
/// filter, collapse or update moves it to another service. After a purchase hides the service,
/// the stock window blanks its detail pane and highlights nothing, as the reference does.
fn selected_row(model: &Model) -> Option<usize> {
    let want = model.trainer_selection?;
    let t = model.trainer.as_ref()?;
    rows(model)
        .0
        .iter()
        .position(|r| match r {
            Row::Service(si) => t.services.get(*si).is_some_and(|s| s.spell_id == want),
            Row::Header(_) => false,
        })?
        .checked_add(1)
}

/// Queue `TRAINER_UPDATE`, which the reference fires from its mask commits, not from Lua:
/// `0x4d8c90` writes the filter mask, re-runs the finalizer and fires event `0x136`, and the
/// skill-line and expand-mask commits (`0x4d8cb0`, `0x4d8cd0`, the collapse verbs') do the same.
/// The stock window's filter click (`Blizzard_TrainerUI.lua:524`) does not repaint.
fn queue_trainer_update(model: &mut Model) {
    model
        .pending_events
        .push(("TRAINER_UPDATE".to_string(), Vec::new()));
}

fn set_collapsed(model: &mut Model, id: usize, collapse: bool) {
    if id == 0 && !collapse {
        model.trainer_collapsed.clear();
        return;
    }
    let targets: Vec<u32> = if id == 0 {
        model
            .trainer
            .as_ref()
            .map(|t| t.groups.iter().map(|g| g.key).collect())
            .unwrap_or_default()
    } else {
        match rows(model).0.get(id - 1) {
            Some(Row::Header(gi)) => model
                .trainer
                .as_ref()
                .and_then(|t| t.groups.get(*gi))
                .map(|g| g.key)
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    };
    for sl in targets {
        if collapse {
            model.trainer_collapsed.insert(sl);
        } else {
            model.trainer_collapsed.remove(&sl);
        }
    }
}

fn opt_str(lua: &Lua, s: Option<&String>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

/// Register the trainer globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumTrainerServices",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(num_services(&model) as i64)
        })?,
    )?;

    // GetTrainerServiceInfo(index) → name, subText, serviceType, isExpanded; one nil past the end.
    g.set(
        "GetTrainerServiceInfo",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(n) = index.checked_sub(1) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let Some(row) = rows(&model).0.get(n).copied() else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let t = model
                .trainer
                .as_ref()
                .expect("a resolved row ⇒ an open trainer");
            match row {
                Row::Header(gi) => {
                    let g = &t.groups[gi];
                    let expanded = !model.trainer_collapsed.contains(&g.key);
                    Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&g.name)?),
                        Value::Nil,
                        Value::String(lua.create_string("header")?),
                        binding_abi::flag(expanded),
                    ]))
                }
                Row::Service(si) => {
                    let s = &t.services[si];
                    Ok(MultiValue::from_vec(vec![
                        opt_str(lua, s.name.as_ref())?,
                        opt_str(lua, s.subtext.as_ref())?,
                        Value::String(lua.create_string(s.category.era_str())?),
                        Value::Nil,
                    ]))
                }
            }
        })?,
    )?;

    // GetTrainerServiceSkillLine(index): the group name, nil for a header. The reference's return
    // (`0x4d9160`) is untraced; the stock `CONFIRM_PROFESSION` dialog shows it as the profession.
    g.set(
        "GetTrainerServiceSkillLine",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(n) = index.checked_sub(1) else {
                return Ok(Value::Nil);
            };
            let Some(Row::Service(si)) = rows(&model).0.get(n).copied() else {
                return Ok(Value::Nil);
            };
            let t = model
                .trainer
                .as_ref()
                .expect("a resolved row ⇒ an open trainer");
            Ok(Value::String(
                lua.create_string(&t.services[si].group_name)?,
            ))
        })?,
    )?;

    g.set(
        "GetTrainerServiceIcon",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            opt_str(lua, service(&model, index).and_then(|s| s.texture.as_ref()))
        })?,
    )?;

    g.set(
        "GetTrainerServiceDescription",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let text = service(&model, index)
                .map(|s| s.description.clone())
                .unwrap_or_default();
            lua.create_string(&text)
        })?,
    )?;

    // GetTrainerServiceCost(index) → money, talentPointCost, professionPointCost; the talent cost
    // is always 0, the profession cost 1 for a primary profession's first rank.
    g.set(
        "GetTrainerServiceCost",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (money, prof) = service(&model, index)
                .map(|s| (s.cost, u32::from(s.prof_first_rank)))
                .unwrap_or((0, 0));
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(money)),
                Value::Integer(0),
                Value::Integer(i64::from(prof)),
            ]))
        })?,
    )?;

    g.set(
        "GetTrainerServiceLevelReq",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(service(&model, index).map_or(0, |s| s.level_req) as i64)
        })?,
    )?;

    g.set(
        "GetTrainerServiceSkillReq",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(req) = service(&model, index).and_then(|s| s.skill_req.as_ref()) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&req.name)?),
                Value::Integer(i64::from(req.rank)),
                binding_abi::flag(req.met),
            ]))
        })?,
    )?;

    g.set(
        "GetTrainerServiceNumAbilityReq",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(service(&model, index).map_or(0, |s| s.ability_reqs.len()) as i64)
        })?,
    )?;

    g.set(
        "GetTrainerServiceAbilityReq",
        lua.create_function(|lua, (index, i): (usize, usize)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(req) = service(&model, index)
                .and_then(|s| i.checked_sub(1).and_then(|n| s.ability_reqs.get(n)))
            else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&req.name)?),
                binding_abi::flag(req.met),
            ]))
        })?,
    )?;

    // GetTrainerServiceStepReq(index) → step, met: always nil, as the wire carries no step data.
    g.set(
        "GetTrainerServiceStepReq",
        lua.create_function(|_, _index: usize| Ok(Value::Nil))?,
    )?;

    g.set(
        "IsTrainerServiceTradeSkill",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(
                service(&model, index).is_some_and(|s| s.is_trade_skill),
            ))
        })?,
    )?;

    // IsTrainerServiceLearnSpell(index) → isLearnSpell, isPetLearnSpell: any service that is not a
    // tradeskill is a learn spell; isPetLearnSpell is not built and always answers nil.
    g.set(
        "IsTrainerServiceLearnSpell",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let learn = service(&model, index).is_some_and(|s| !s.is_trade_skill);
            Ok(MultiValue::from_vec(vec![
                binding_abi::flag(learn),
                Value::Nil,
            ]))
        })?,
    )?;

    // IsTradeskillTrainer(): trainer type 2 (`0x4d8ea0`).
    g.set(
        "IsTradeskillTrainer",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(model.trainer.as_ref().is_some_and(|t| {
                t.trainer_type == TRAINER_TYPE_TRADESKILL
            })))
        })?,
    )?;

    // IsTalentTrainer(): trainer type 1 (`0x4d8ed0`), which vmangos gives its mount trainers.
    g.set(
        "IsTalentTrainer",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(
                model.trainer.as_ref().is_some_and(|t| t.trainer_type == 1),
            ))
        })?,
    )?;

    g.set(
        "GetTrainerGreetingText",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let text = model
                .trainer
                .as_ref()
                .map(|t| t.greeting.clone())
                .unwrap_or_default();
            lua.create_string(&text)
        })?,
    )?;

    g.set(
        "GetTrainerServiceTypeFilter",
        lua.create_function(|lua, kind: String| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match TrainerServiceCategory::from_filter_str(&kind) {
                Some(c) => binding_abi::flag(model.trainer_filter[c.filter_slot()]),
                None => Value::Nil,
            })
        })?,
    )?;
    g.set(
        "SetTrainerServiceTypeFilter",
        lua.create_function(|lua, (kind, on): (String, Value)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(c) = TrainerServiceCategory::from_filter_str(&kind) {
                // The stock window passes 1 or 0; nil, false and 0 disable.
                let enable = !matches!(on, Value::Nil | Value::Integer(0) | Value::Boolean(false));
                model.trainer_filter[c.filter_slot()] = enable;
                // Fires even when the bit did not move, as the reference's thunk does.
                queue_trainer_update(&mut model);
            }
            Ok(())
        })?,
    )?;

    // Collapse/ExpandTrainerSkillLine(id): fold the group headed by row `id`, or all at 0. `id`
    // spans the whole row array like the getters' gate (`0x4d89b0`); their own bound is untraced.
    g.set(
        "CollapseTrainerSkillLine",
        lua.create_function(|lua, id: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, true);
            queue_trainer_update(&mut model);
            Ok(())
        })?,
    )?;
    g.set(
        "ExpandTrainerSkillLine",
        lua.create_function(|lua, id: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            set_collapsed(&mut model, id, false);
            queue_trainer_update(&mut model);
            Ok(())
        })?,
    )?;

    // SelectTrainerService(index): select the row's service by spell id; past the end clears it
    // (`0x4d74f0` stores 0, never a clamp). Deviation: a header clears it too, because it has no
    // spell id; the reference stores its `0xffffffff` (`0x4d7b05`) and reads back 1, which the
    // stock window's `> 1` test treats the same as 0.
    g.set(
        "SelectTrainerService",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.trainer_selection = service(&model, index).map(|s| s.spell_id);
            Ok(())
        })?,
    )?;

    g.set(
        "GetTrainerSelectionIndex",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(selected_row(&model).map_or(0i64, |r| r as i64))
        })?,
    )?;

    // BuyTrainerService(index): queue the row's spell id. `0x4d89d0` bounds through the getters'
    // gate and refuses any state byte but 0 (`0x4d89e5`), so only an available service queues.
    // Deviation: index 0 is a no-op, because one stray call would spend the purse; the reference
    // buys every visible service for any index at or below 0 (`0x4da244`, `0x4d8a70`).
    g.set(
        "BuyTrainerService",
        lua.create_function(|lua, index: usize| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let buyable = service(&model, index)
                .filter(|s| s.category == TrainerServiceCategory::Available)
                .map(|s| s.spell_id);
            if let Some(spell_id) = buyable {
                model.trainer_buys.push(spell_id);
            }
            Ok(())
        })?,
    )?;

    // CloseTrainer(): sends no packet; the app clears its state on the flag.
    g.set(
        "CloseTrainer",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.trainer_close = true;
            Ok(())
        })?,
    )?;

    Ok(())
}
#[cfg(test)]
mod tests;
