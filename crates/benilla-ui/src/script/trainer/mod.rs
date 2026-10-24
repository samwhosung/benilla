//! The trainer bindings the stock `Blizzard_TrainerUI` window calls, over the snapshot the app
//! pushes ([`UiScript::set_trainer`]). `SMSG_TRAINER_LIST` is flat; the client builds a collapsible
//! tree of header and service rows over it, every Lua index is 1-based into that tree, and the
//! trainer type (`ds:0xb73a08`) picks the row comparator (`0x4d8561`), the group key and the
//! header comparator (`0x4d7786`, `0x4d79eb`).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// A service's state byte `[+0x30]` (the wire's `TrainerSpellState`): green learnable, red gated,
/// gray known, and the state the re-evaluator writes at a mount trainer for a rank the player has
/// passed (`0x4d7e3e`, `0x4d83a4`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrainerServiceCategory {
    #[default]
    Available,
    Unavailable,
    Used,
    /// State 3: no filter shows it, as `SetTrainerServiceTypeFilter` names bits 0-2 only
    /// (`0x4da390`).
    Hidden,
}

impl TrainerServiceCategory {
    /// `GetTrainerServiceInfo`'s type string, which the stock window colours each row by: every
    /// state but 0 and 2 reads `"unavailable"` (`0x4d8ba0`).
    fn era_str(self) -> &'static str {
        match self {
            TrainerServiceCategory::Available => "available",
            TrainerServiceCategory::Unavailable | TrainerServiceCategory::Hidden => "unavailable",
            TrainerServiceCategory::Used => "used",
        }
    }

    /// The record's state byte `[+0x30]`, which `0x4d8ba0` maps to the type string; at type 1 it is
    /// also a sort key ([`talent_order`]).
    fn state_key(self) -> u8 {
        match self {
            TrainerServiceCategory::Available => 0,
            TrainerServiceCategory::Unavailable => 1,
            TrainerServiceCategory::Used => 2,
            TrainerServiceCategory::Hidden => 3,
        }
    }

    /// The state filter's bit for this state (`0x4d84b0`-`0x4d84ba`: `1 << [+0x30]`).
    fn filter_bit(self) -> u32 {
        1 << self.state_key()
    }
}

/// `0x4da390`: the filter bit a type name names, compared without case (`0x64a4c0`); `None` is
/// its 6, another name.
fn filter_bit_named(name: &str) -> Option<u32> {
    [
        TrainerServiceCategory::Available,
        TrainerServiceCategory::Unavailable,
        TrainerServiceCategory::Used,
    ]
    .into_iter()
    .find(|c| crate::crt::str_cmp_i(name, c.era_str()) == 0)
    .map(TrainerServiceCategory::filter_bit)
}

/// The state mask `"all"` sets (`0x4da2a9`): the three states a filter can name.
const TRAINER_FILTER_ALL: u32 = 7;

/// A service's skill requirement (`GetTrainerServiceSkillReq`). The wire has no per-gate bit, so
/// `met` comes from the service's `category`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainerSkillReq {
    pub name: String,
    pub rank: u32,
    pub met: bool,
}

/// One of a service's three required-ability slots (`+0x24`), by position
/// (`GetTrainerServiceAbilityReq`); `met` is whether the book knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrainerAbilityReq {
    /// The slot's id as the wire sent it, read signed: `GetTrainerServiceNumAbilityReq` counts the
    /// slots above 0 (`0x4d9653`).
    pub spell_id: i32,
    /// `%s (%s)` (`0x8468b0`) of the record's name and rank, the name alone with no rank
    /// (`0x4d9808`); `None` with no `Spell.dbc` record, which answers nil (`0x4d97c8`, `0x4d98d1`).
    pub name: Option<String>,
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

/// A product record a getter's lookup misses: the item, and the taught spell whose id, under
/// `0xb0000000`, the lookup passes as its owner (`[taught]` at `0x4d9074` and `0x4d9cdf`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProductWait {
    pub item: u32,
    pub taught: u32,
}

/// One service, resolved by the app from the wire `TrainerSpell`; its place in
/// [`TrainerState::services`] is wire order, not display order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerService {
    /// The wire spell id, which `CMSG_TRAINER_BUY_SPELL` names.
    pub spell_id: u32,
    /// The wire spell's name; `None` (before `Spell.dbc` loads) answers nil, shown as `UNKNOWN`.
    pub name: Option<String>,
    /// The wire spell's NameSubtext, such as "Rank 2", "" for a rankless spell; `None` is no Spell
    /// record and answers nil (`0x4d8b58`).
    pub subtext: Option<String>,
    pub texture: Option<String>,
    /// The item whose record the icon's product lookup (`0x4d9096`) misses: each
    /// `GetTrainerServiceIcon` call on the row keeps the callback `0x4d9140` on it.
    pub icon_wait: Option<ProductWait>,
    /// The expanded text; `None` answers nil (`0x4d9d90`).
    pub description: Option<String>,
    /// The item whose record the description's product lookup (`0x4d9d07`) misses: each
    /// `GetTrainerServiceDescription` call on the row keeps the callback `0x4d9db0` on it.
    pub description_wait: Option<ProductWait>,
    /// In copper, already reputation-discounted by the server.
    pub cost: u32,
    /// A primary profession's first rank: the stock window's `cpCost2 > 0`, which asks to confirm.
    pub prof_first_rank: bool,
    pub category: TrainerServiceCategory,
    pub level_req: u32,
    pub skill_req: Option<TrainerSkillReq>,
    pub ability_reqs: Vec<TrainerAbilityReq>,
    /// `IsTrainerServiceTradeSkill` (`0x4d9f70`): a `LEARN_SPELL` slot of the wire spell teaches
    /// a spell with `Attributes & 0x20`.
    pub is_trade_skill: bool,
    /// `IsTrainerServiceLearnSpell` (`0x4d9e70`): `Some(false)` for a `LEARN_SPELL` slot teaching
    /// a missing or unflagged spell, `Some(true)` for a `LEARN_PET_SPELL` slot, the first such
    /// slot deciding; `None` answers nil, nil.
    pub learn_spell: Option<bool>,
    /// The tree group: the taught spell's `SkillLine` at types 0, 1 and 3, where 0 (unresolved)
    /// drops the service and a known service at type 1 takes `TRAINER_GROUP_KNOWN`; at type 2, 1
    /// when the wire spell has a `SKILL_STEP` (44) effect, else 2 (`0x4d77b6`), so a tradeskill
    /// trainer drops nothing.
    pub group_key: u32,
    /// The header text: the skill line's name, `KNOWN_TALENTS_HEADER` for the known group, or at
    /// type 2 `TRADESKILL_SERVICE_STEP`/`_LEARN`.
    pub group_name: String,
    pub tooltip: TrainerTooltip,
    /// The wire spell has a skill-step effect (id 44, `0x2c`) in any of its three slots:
    /// `IsTrainerServiceSkillStep` (`0x4d9dd0`).
    pub skill_step: bool,
    /// `GetTrainerServiceStepIncrease`'s text (`0x4da030`) for the wire spell's first skill-step
    /// effect: `INCREASE_POTENTIAL` filled with the effect's skill line and five times its step.
    /// `None` when the spell has no such effect, the step is 0 or the line has no row; the binding
    /// answers nil for a used service whatever this holds.
    pub step_increase: Option<String>,
    /// The name of the skill line whose id is [`Self::group_key`] (`SkillLine.dbc`), which
    /// `GetTrainerSkillLines` (`0x4da660`) lists per header; `None` for a key with no row, the
    /// known-talents group's -1 and a tradeskill trainer's 1 and 2 included.
    pub line_name: Option<String>,
}

/// One header and its services, built by [`UiScript::set_trainer`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerGroup {
    /// Also the collapse key.
    pub key: u32,
    pub name: String,
    /// [`TrainerService::line_name`] of the group's first service: a group's services share a key.
    pub line_name: Option<String>,
    /// Positions into [`TrainerState::services`], in display order.
    pub services: Vec<usize>,
}

/// The open trainer, pushed whole by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainerState {
    /// The trainer's cell `[0xb73a00]`, which `CMSG_TRAINER_BUY_SPELL` names (`0x4d8a11`).
    pub guid: u64,
    pub services: Vec<TrainerService>,
    /// `SMSG_TRAINER_LIST`'s trailing string.
    pub greeting: String,
    /// `SMSG_TRAINER_LIST`'s trainer type (`ds:0xb73a08`): 0 class, 1 mount, 2 tradeskill, 3 pet.
    pub trainer_type: u32,
    /// Built by [`UiScript::set_trainer`], not pushed by the app.
    pub groups: Vec<TrainerGroup>,
}

impl super::UiScript {
    /// Push the open trainer and finalize its rows, as a list (`0x4d7b3b`) and the state pass
    /// `0x4d7d40` do, or clear it with `None`. The first push of a list builds the header array
    /// ([`build_groups`]); a later one keeps it and regroups the rows under it ([`regroup`]). A
    /// fold survives a content update.
    pub fn set_trainer(&mut self, state: Option<TrainerState>) {
        self.push_trainer(state, true);
    }

    /// One more pass of the state re-evaluator's tail over the rows as they stand: the finalizer
    /// `0x4d8410`, which sorts from the order the last pass left (`0x4d83ef`).
    pub fn rerun_trainer_finalize(&mut self) {
        finalize(&mut self.model_mut());
    }

    /// Push the open trainer's new text or icons with no finalize: the rows keep their order and
    /// their visible count until the reference's own triggers re-sort them.
    pub fn refresh_trainer(&mut self, state: Option<TrainerState>) {
        self.push_trainer(state, false);
    }

    fn push_trainer(&mut self, state: Option<TrainerState>, finalizes: bool) {
        // The known group's header text, which `0x4d8b11` reads by name.
        let known_header = self
            .lua()
            .globals()
            .get::<String>("KNOWN_TALENTS_HEADER")
            .unwrap_or_default();
        let mut model = self.model_mut();
        match state {
            None => {
                model.trainer_selection = None;
                model.trainer_collapsed.clear();
                model.trainer = None;
                model.trainer_rows.clear();
                model.trainer_visible = 0;
                model.trainer_rows_fresh = false;
            }
            Some(mut s) => {
                // The header array is the list's (`0x4d7560`); a later push only regroups the rows
                // under it, as a pass re-keys a row and keeps every header (`0x4d82b2`-`0x4d82c1`).
                s.groups = match model.trainer.as_ref().filter(|_| !model.trainer_rows_fresh) {
                    Some(listed) => regroup(&listed.groups, &s.services),
                    None => build_groups(&s.services, s.trainer_type, &known_header),
                };
                let live: std::collections::HashSet<u32> = s.groups.iter().map(|g| g.key).collect();
                model.trainer_collapsed.retain(|sl| live.contains(sl));
                let model = &mut *model;
                let kept = if std::mem::take(&mut model.trainer_rows_fresh) {
                    Vec::new()
                } else {
                    let old = model.trainer.as_ref().map_or(&[][..], |t| &t.groups[..]);
                    carry_rows(&model.trainer_rows, old, &s)
                };
                model.trainer_rows = laid_out(kept, &s);
                model.trainer = Some(s);
                if finalizes {
                    finalize(model);
                }
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
            5
        } else {
            3
        };
        model.trainer_collapsed.clear();
        // The skill-line filter resets to every group with the list (`0x4d7560`).
        model.trainer_line_filter = u32::MAX;
        model.trainer_selection = None;
        // The list's push lays the array out anew (`0x4d7807`-`0x4d7b35`); the caller pushes
        // every list, an identical one too.
        model.trainer_rows_fresh = true;
    }

    /// The trainer calls a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_trainer_calls(&mut self) -> Vec<TrainerCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Trainer(call) => Some(*call),
            _ => None,
        })
    }
}

/// A trainer verb past its binding's checks, which the host applies inside the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrainerCall {
    /// `BuyTrainerService(index)` on an available row (`0x4d89d0`): `CMSG_TRAINER_BUY_SPELL` with
    /// the trainer's cell and the row's spell id (`0x4d8a11`-`0x4d8a26`).
    Buy { trainer: u64, spell_id: u32 },
    /// `OpenTrainer()` (`0x4d8cf0`): with a player (`0x4d8d24`), `TalkToTrainer` (`0x5df680`) on
    /// the player's own guid, which closes the window (`0x5df6bb`) and sends `CMSG_TRAINER_LIST`
    /// (`0x5df6fc`).
    OpenOwn,
    /// `CloseTrainer()` (`0x4d8d50`): the close core `0x4d7440` with a zero guid.
    Close,
}

/// Hand `call` to the host, which applies it before this returns ([`super::calls::submit`]).
fn submit(lua: &Lua, call: TrainerCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Trainer(call))
}

/// One row: a header by group index, or a service by its position in [`TrainerState::services`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Header(usize),
    Service(usize),
}

/// `SStrCmp` (`0x64a480` → `_strncmp`), the row comparators' name and rank compare: byte for byte,
/// so an upper-case letter sorts before every lower-case one.
fn str_cmp(a: &str, b: &str) -> i32 {
    a.as_bytes().cmp(b.as_bytes()) as i32
}

/// A comparator's three-way answer as the sign `qsort` reads.
fn sign(o: std::cmp::Ordering) -> i32 {
    o as i32
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

/// Both rows' `Spell.dbc` records, which every comparator needs for its name keys: a row with none
/// ties with every row of its group (`0x4d868d`, `0x4d87ca`, `0x4d892b`). The name is `Some`
/// exactly when the record is, and so is the rank text.
fn names<'a>(a: &'a TrainerService, b: &'a TrainerService) -> Option<(&'a str, &'a str)> {
    Some((a.name.as_deref()?, b.name.as_deref()?))
}

/// Rows at a class or pet trainer (`0x4d85c0`; `0x4d8561` sends pet here too): required level
/// (`0x4d86a0`), skill value (`0x4d86aa`), name (`0x4d86db`), then rank (`0x4d86fe`), all
/// ascending.
fn class_order(a: &TrainerService, b: &TrainerService) -> i32 {
    let Some((an, bn)) = names(a, b) else {
        return 0;
    };
    let rank = |s: &TrainerService| s.subtext.clone().unwrap_or_default();
    match a
        .level_req
        .cmp(&b.level_req)
        .then_with(|| skill_value(a).cmp(&skill_value(b)))
    {
        std::cmp::Ordering::Equal => match str_cmp(an, bn) {
            0 => str_cmp(&rank(a), &rank(b)),
            o => o,
        },
        o => sign(o),
    }
}

/// Rows at a tradeskill trainer (`0x4d8760`): required skill value ascending (`0x4d87d9`), then
/// name (`0x4d87f8`). It has no level key and no rank key.
fn tradeskill_order(a: &TrainerService, b: &TrainerService) -> i32 {
    let Some((an, bn)) = names(a, b) else {
        return 0;
    };
    match skill_value(a).cmp(&skill_value(b)) {
        std::cmp::Ordering::Equal => str_cmp(an, bn),
        o => sign(o),
    }
}

/// Rows at a mount trainer (`0x4d8850`): the state byte ascending (`0x4d88f1`: available,
/// unavailable, used), read before the records, then name (`0x4d894e`).
fn talent_order(a: &TrainerService, b: &TrainerService) -> i32 {
    match a.category.state_key().cmp(&b.category.state_key()) {
        std::cmp::Ordering::Equal => names(a, b).map_or(0, |(an, bn)| str_cmp(an, bn)),
        o => sign(o),
    }
}

/// Group the services by [`TrainerService::group_key`], dropping key 0, each header made where its
/// key is first met in list order (`0x4d78a3`-`0x4d79c3`), then the header array sorted by the
/// CRT `qsort` (`0x73f727`, called at `0x4d7a0e`): by raw key at type 2 (`0x4d7c30`), else
/// `0x4d7b90`. A mount trainer's known group, headed `known_header`, is made first and always,
/// member or none (`0x4d7609`-`0x4d7697`). The rows themselves sort in [`finalize`].
fn build_groups(
    services: &[TrainerService],
    trainer_type: u32,
    known_header: &str,
) -> Vec<TrainerGroup> {
    let mut groups: Vec<TrainerGroup> = Vec::new();
    if trainer_type == TRAINER_TYPE_MOUNT {
        groups.push(TrainerGroup {
            key: TRAINER_GROUP_KNOWN,
            name: known_header.to_string(),
            line_name: None,
            services: Vec::new(),
        });
    }
    for (i, s) in services.iter().enumerate() {
        if s.group_key == 0 {
            continue;
        }
        let gi = match groups.iter().position(|g| g.key == s.group_key) {
            Some(gi) => gi,
            None => {
                groups.push(TrainerGroup {
                    key: s.group_key,
                    name: s.group_name.clone(),
                    line_name: s.line_name.clone(),
                    services: Vec::new(),
                });
                groups.len() - 1
            }
        };
        groups[gi].services.push(i);
    }
    if trainer_type == TRAINER_TYPE_TRADESKILL {
        crate::crt::qsort(&mut groups, |a, b| {
            sign((a.key as i32).cmp(&(b.key as i32)))
        });
    } else {
        // `0x4d7b90`: the known group first, then name. Its middle key, `hdr[+0x24] == 0` first, is
        // not built.
        crate::crt::qsort(&mut groups, |a, b| {
            match (a.key != TRAINER_GROUP_KNOWN).cmp(&(b.key != TRAINER_GROUP_KNOWN)) {
                std::cmp::Ordering::Equal => crate::crt::str_cmp_i(&a.name, &b.name),
                o => sign(o),
            }
        });
    }
    groups
}

/// `services` grouped under the list's header array `listed`, its order and its headers kept,
/// a group whose rows all left among them: the state pass re-keys a row now known to -1 at type 1
/// (`0x4d82b2`-`0x4d82c1`) and touches no header (`0xb73a14` is the builder's count). A key the
/// array lacks, which no pass writes, gets a header at its end.
fn regroup(listed: &[TrainerGroup], services: &[TrainerService]) -> Vec<TrainerGroup> {
    let mut groups: Vec<TrainerGroup> = listed
        .iter()
        .map(|g| TrainerGroup {
            services: Vec::new(),
            ..g.clone()
        })
        .collect();
    for (i, s) in services.iter().enumerate() {
        if s.group_key == 0 {
            continue;
        }
        let gi = match groups.iter().position(|g| g.key == s.group_key) {
            Some(gi) => gi,
            None => {
                groups.push(TrainerGroup {
                    key: s.group_key,
                    name: s.group_name.clone(),
                    line_name: s.line_name.clone(),
                    services: Vec::new(),
                });
                groups.len() - 1
            }
        };
        groups[gi].services.push(i);
    }
    groups
}

/// The rows of `old`'s array that still exist in `new`, in the array's order: a service by its
/// position, a header by its key.
fn carry_rows(rows: &[Row], old: &[TrainerGroup], new: &TrainerState) -> Vec<Row> {
    rows.iter()
        .filter_map(|&row| match row {
            Row::Service(si) => new
                .services
                .get(si)
                .filter(|s| s.group_key != 0)
                .map(|_| row),
            Row::Header(gi) => {
                let key = old.get(gi)?.key;
                new.groups
                    .iter()
                    .position(|g| g.key == key)
                    .map(Row::Header)
            }
        })
        .collect()
}

/// `kept`, then every row of `t` it lacks as the builder lays them out: the services in list
/// order, key 0 dropped (`0x4d7807`), then one header row per group in the header array's order
/// (`0x4d7af4`-`0x4d7b27`).
fn laid_out(mut kept: Vec<Row>, t: &TrainerState) -> Vec<Row> {
    let builder = t
        .services
        .iter()
        .enumerate()
        .filter(|(_, s)| s.group_key != 0)
        .map(|(si, _)| Row::Service(si))
        .chain((0..t.groups.len()).map(Row::Header));
    let fresh: Vec<Row> = builder.filter(|r| !kept.contains(r)).collect();
    kept.extend(fresh);
    kept
}

/// The finalizer `0x4d8410` over the whole row array. A service is hidden when its state is
/// filtered out (`0x4d84ba`); any row when its group shows nothing, no member passing the state
/// filter or the skill-line filter clearing the group (`0x4d8447`-`0x4d8508`, `0x4d8535`); a
/// service, not a header, when its group is collapsed (`0x4d853d`-`0x4d8546`). Then the CRT
/// `qsort` (`0x73f727`) sorts the array from the order it left (`0x4d857e`) with the type's
/// comparator, which puts the visible rows first, two hidden rows tying (`0x4d85e0`,
/// `0x4d8736`), and the visible ones by group, its header first, then by the type's keys.
fn finalize(model: &mut Model) {
    let Some(t) = model.trainer.as_ref() else {
        model.trainer_rows.clear();
        model.trainer_visible = 0;
        return;
    };
    let mut group_of = vec![0; t.services.len()];
    for (gi, g) in t.groups.iter().enumerate() {
        for &si in &g.services {
            group_of[si] = gi;
        }
    }
    let passes = |si: usize| model.trainer_filter & t.services[si].category.filter_bit() != 0;
    let group_shown: Vec<bool> = t
        .groups
        .iter()
        .enumerate()
        .map(|(gi, g)| {
            model.trainer_line_filter & 1u32.wrapping_shl(gi as u32) != 0
                && g.services.iter().any(|&si| passes(si))
        })
        .collect();
    let visible = |row: &Row| match *row {
        Row::Header(gi) => group_shown[gi],
        Row::Service(si) => {
            let gi = group_of[si];
            passes(si) && group_shown[gi] && !model.trainer_collapsed.contains(&t.groups[gi].key)
        }
    };
    let order = match t.trainer_type {
        TRAINER_TYPE_TRADESKILL => tradeskill_order,
        TRAINER_TYPE_MOUNT => talent_order,
        _ => class_order,
    };
    // The group a row sorts by: the raw key at type 2 (`0x4d878b`), else the header array's
    // index (`0x4d85fe`-`0x4d8643`).
    let group = |row: &Row| -> i64 {
        let gi = match *row {
            Row::Header(gi) => gi,
            Row::Service(si) => group_of[si],
        };
        if t.trainer_type == TRAINER_TYPE_TRADESKILL {
            i64::from(t.groups[gi].key as i32)
        } else {
            gi as i64
        }
    };
    let compare = |a: &Row, b: &Row| -> i32 {
        match (visible(a), visible(b)) {
            (false, false) => return 0,
            (false, true) => return 1,
            (true, false) => return -1,
            (true, true) => {}
        }
        let (ga, gb) = (group(a), group(b));
        if ga != gb {
            return if ga > gb { 1 } else { -1 };
        }
        // A header's id is -1 (`0x4d8653`, `0x4d870a`): it sorts first.
        match (*a, *b) {
            (Row::Header(_), _) => -1,
            (_, Row::Header(_)) => 1,
            (Row::Service(sa), Row::Service(sb)) => order(&t.services[sa], &t.services[sb]),
        }
    };
    let mut rows = std::mem::take(&mut model.trainer_rows);
    crate::crt::qsort(&mut rows, compare);
    model.trainer_visible = rows.iter().filter(|r| visible(r)).count();
    model.trainer_rows = rows;
}

/// The client's whole row array and its visible count, as the last [`finalize`] left them: the
/// visible rows in display order, then the hidden ones, each keeping its index for the getters,
/// which bound on the total (`0x4d89b0`, `ds:0xb73a10`).
fn rows(model: &Model) -> (&[Row], usize) {
    (&model.trainer_rows, model.trainer_visible)
}

/// A getter's product lookup that missed: the item cache asked inside the call, with the
/// window's callback (`0x4d9140`, `0x4d9db0`, dedupe flag 0 at `0x4d9076` and `0x4d9ce7`).
fn ask_item(lua: &Lua, wait: ProductWait) -> mlua::Result<()> {
    super::calls::ask(
        lua,
        super::RecordAsk::Item {
            entry: wait.item,
            owner: super::calls::spell_owner(wait.taught),
            callback: super::ItemCallback::Trainer,
        },
    )
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

/// Raise `TRAINER_UPDATE`, as the reference's mask commits do after the re-filter: `0x4d8c90`
/// writes the type mask, re-runs the finalizer `0x4d8410` and signals at `0x4d8ca0`; the
/// skill-line and expand-mask commits (`0x4d8cb0`, `0x4d8cd0`, the collapse verbs') do the same
/// (`0x4d8cc0`, `0x4d8ce0`). The stock window's filter click (`Blizzard_TrainerUI.lua:524`) does
/// not repaint.
fn raise_trainer_update(model: &mut Model) {
    finalize(model);
    model.raise("TRAINER_UPDATE", Vec::new());
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

    // GetTrainerServiceInfo(index) → name, subText, serviceType, isExpanded. Past the end the three
    // strings are nil and the fourth is 1 (`0x4d8e42`: `0x4d8c30` answers 0 for a row that is no
    // header, which pushes 1); a service row answers 1 there too, a header 1 when it is expanded.
    g.set(
        "GetTrainerServiceInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTrainerServiceInfo(index)` (`0x84a884`, `0x4d8dd1`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceInfo(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let miss = || binding_abi::tuple(&[None, None, None, Some(1)]);
            let Some(n) = index.checked_sub(1) else {
                return Ok(miss());
            };
            let Some(row) = rows(&model).0.get(n).copied() else {
                return Ok(miss());
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
                        // The header's subText is the empty string (`0x4d8b50` returns `0x882748`
                        // for a header row, pushed at `0x4d8e13`), not nil.
                        Value::String(lua.create_string("")?),
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
                        Value::Integer(1),
                    ]))
                }
            }
        })?,
    )?;

    // GetTrainerServiceSkillLine(index): the group name, nil for a header. The reference's return
    // (`0x4d9160`) is untraced; the stock `CONFIRM_PROFESSION` dialog shows it as the profession.
    g.set(
        "GetTrainerServiceSkillLine",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTrainerServiceSkillLine(index)` (`0x84a8f0`, `0x4d917a`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceSkillLine(index)")?;
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTrainerServiceIcon(index)` (`0x84a8cc`, `0x4d8f81`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceIcon(index)")?;
            let (texture, wait) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                service(&model, index)
                    .map(|s| (s.texture.clone(), s.icon_wait))
                    .unwrap_or_default()
            };
            // The miss asks and keeps its callback inside the call (`0x4d907e`), and pushes nil
            // (`0x4d90a4`).
            if let Some(wait) = wait {
                ask_item(lua, wait)?;
            }
            opt_str(lua, texture.as_ref())
        })?,
    )?;

    g.set(
        "GetTrainerServiceDescription",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTrainerServiceDescription(index)` (`0x84a9e4`,
            // `0x4d9b71`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceDescription(index)")?;
            let (description, wait) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                service(&model, index)
                    .map(|s| (s.description.clone(), s.description_wait))
                    .unwrap_or_default()
            };
            // The miss asks and keeps its callback inside the call (`0x4d9cef`).
            if let Some(wait) = wait {
                ask_item(lua, wait)?;
            }
            opt_str(lua, description.as_ref())
        })?,
    )?;

    // GetTrainerServiceCost(index) → money, talentPointCost, professionPointCost; the talent cost
    // is always 0, the profession cost 1 for a primary profession's first rank.
    g.set(
        "GetTrainerServiceCost",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTrainerServiceCost(index)` (`0x84a91c`, `0x4d9307`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceCost(index)")?;
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTrainerServiceLevelReq(index)` (`0x84a940`, `0x4d93b5`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceLevelReq(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(service(&model, index).map_or(0, |s| s.level_req) as i64)
        })?,
    )?;

    g.set(
        "GetTrainerServiceSkillReq",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTrainerServiceSkillReq(index)` (`0x84a968`, `0x4d942a`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceSkillReq(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // No skill requirement, or no such row: nil, 0, 1 (`0x4d95bc`).
            let Some(req) = service(&model, index).and_then(|s| s.skill_req.as_ref()) else {
                return Ok(binding_abi::tuple(&[None, Some(0), Some(1)]));
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTrainerServiceAbilityReq(index)` (`0x84a990`,
            // `0x4d9617`), the client's own text naming `GetTrainerServiceAbilityReq`.
            let index =
                binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceAbilityReq(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(service(&model, index).map_or(0, |s| {
                s.ability_reqs.iter().filter(|r| r.spell_id > 0).count()
            }) as i64)
        })?,
    )?;

    g.set(
        "GetTrainerServiceAbilityReq",
        lua.create_function(|lua, (index, i): (Value, Value)| {
            // Both `is-number` else `Usage: GetTrainerServiceAbilityReq(index)` (`0x84a990`,
            // `0x4d990e`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceAbilityReq(index)")?;
            let i = binding_abi::index_arg(lua, i, "Usage: GetTrainerServiceAbilityReq(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // The slot `i - 1` read in place, bounded by the three (`0x4d979c`), not by the count.
            let Some(req) = service(&model, index)
                .and_then(|s| i.checked_sub(1).and_then(|n| s.ability_reqs.get(n)))
            else {
                // `0x4d98b5`: nil, 1.
                return Ok(binding_abi::tuple(&[None, Some(1)]));
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, req.name.as_ref())?,
                binding_abi::flag(req.met),
            ]))
        })?,
    )?;

    // GetTrainerServiceStepReq(index) → step, met: nil, 1, as the wire carries no step data. That
    // is the reference's tuple for a row with no step spell and for a miss (`0x4d9afc`).
    g.set(
        "GetTrainerServiceStepReq",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetTrainerServiceStepReq(index)` (`0x84a9bc`, `0x4d994a`).
            binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceStepReq(index)")?;
            Ok(binding_abi::tuple(&[None, Some(1)]))
        })?,
    )?;

    g.set(
        "IsTrainerServiceTradeSkill",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: IsTrainerServiceTradeSkill(index)` (`0x84aa64`, `0x4d9f83`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: IsTrainerServiceTradeSkill(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(
                service(&model, index).is_some_and(|s| s.is_trade_skill),
            ))
        })?,
    )?;

    // IsTrainerServiceLearnSpell(index) → isLearnSpell, isPetLearnSpell: 1, nil for a learn
    // slot teaching an unflagged spell (`0x4d9f28`), 1, 1 for a pet-learn slot (`0x4d9f46`), and
    // nil, nil otherwise (`0x4d9f11`).
    g.set(
        "IsTrainerServiceLearnSpell",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: IsTrainerServiceLearnSpell(index)` (`0x84aa38`, `0x4d9e83`).
            let index =
                binding_abi::index_arg(lua, index, "Usage: IsTrainerServiceLearnSpell(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let learn = service(&model, index).and_then(|s| s.learn_spell);
            Ok(MultiValue::from_vec(vec![
                binding_abi::flag(learn.is_some()),
                binding_abi::flag(learn == Some(true)),
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

    // IsTalentTrainer() (`0x4d8ed0`): 1 with no trainer open (`0x4d8edb`, a zero guid) or at type
    // 1 (`0x4d8ee4`), which vmangos gives its mount trainers.
    g.set(
        "IsTalentTrainer",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(
                model
                    .trainer
                    .as_ref()
                    .is_none_or(|t| t.trainer_type == TRAINER_TYPE_MOUNT),
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

    // GetTrainerServiceTypeFilter("type") (`0x4da510`): 1 when the named state shows, else nil;
    // the name through `0x4da390`, without case, and `"all"` is no name here.
    g.set(
        "GetTrainerServiceTypeFilter",
        lua.create_function(|lua, kind: Value| {
            // `is-number-or-string` else the usage (`0x84ac78`, `0x4da521`).
            let kind = binding_abi::string_arg(
                lua,
                kind,
                r#"Usage: GetTrainerServiceTypeFilter("type")"#,
            )?;
            let bit = filter_bit_named(&kind).ok_or_else(|| {
                mlua::Error::runtime("Bad service type in GetTrainerServiceTypeFilter")
            })?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(model.trainer_filter & bit != 0))
        })?,
    )?;
    // SetTrainerServiceTypeFilter("type", on [, exclusive]) (`0x4da260`): `"all"` sets the mask to
    // 7 and reads nothing more (`0x4da2a9`); otherwise the named bit, cleared by an `on` that
    // truncates to 0 (`0x4da31a`), the whole mask for a third argument that is a number truncating
    // to nonzero (`0x4da359`), else added (`0x4da36f`). Every path commits through `0x4d8c90`: the
    // finalizer, then `TRAINER_UPDATE`, also when the mask did not move.
    g.set(
        "SetTrainerServiceTypeFilter",
        lua.create_function(|lua, (kind, on, exclusive): (Value, Value, Value)| {
            // `is-number-or-string` else the usage (`0x84ab60`, `0x4da273`).
            let kind = binding_abi::string_arg(
                lua,
                kind,
                r#"Usage: SetTrainerServiceTypeFilter("type" [, on\off, exclusive])"#,
            )?;
            let mask = if crate::crt::str_cmp_i(&kind, "all") == 0 {
                TRAINER_FILTER_ALL
            } else {
                let bit = filter_bit_named(&kind).ok_or_else(|| {
                    mlua::Error::runtime("Bad service type in SetTrainerServiceTypeFilter")
                })?;
                let on = binding_abi::number_arg(
                    lua,
                    on,
                    "Missing on//off parameter in SetTrainerServiceTypeFilter",
                )?;
                let current = lua
                    .app_data_ref::<Model>()
                    .expect("model app_data")
                    .trainer_filter;
                if on == 0 {
                    current & !bit
                } else if binding_abi::is_number(lua, exclusive)
                    .is_some_and(|n| binding_abi::ftol(n) != 0)
                {
                    bit
                } else {
                    current | bit
                }
            };
            super::tick::verb(lua, |model| {
                model.trainer_filter = mask;
                raise_trainer_update(model);
            });
            Ok(())
        })?,
    )?;

    // OpenTrainer() (`0x4d8cf0`): `TalkToTrainer` (`0x5df680`) with the player's own guid, which
    // first closes the window (`0x4d7440` with a zero guid) and then sends `CMSG_TRAINER_LIST`
    // for that guid. All of it waits on the player (`0x4d8d24`), which the host holds.
    g.set(
        "OpenTrainer",
        lua.create_function(|lua, ()| submit(lua, TrainerCall::OpenOwn))?,
    )?;

    // IsTrainerServiceSkillStep(index) (`0x4d9dd0`): 1 when the row's wire spell has a skill-step
    // effect, nil for a header, a missing row or a spell without one.
    g.set(
        "IsTrainerServiceSkillStep",
        lua.create_function(|lua, index: Value| {
            let index =
                binding_abi::index_arg(lua, index, "Usage: IsTrainerServiceSkillStep(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(
                service(&model, index).is_some_and(|s| s.skill_step),
            ))
        })?,
    )?;

    // GetTrainerServiceStepIncrease(index) (`0x4da030`): the `INCREASE_POTENTIAL` text of a
    // skill-step row, nil for a used row (state 2, `0x4da0eb`) and wherever the text cannot form.
    g.set(
        "GetTrainerServiceStepIncrease",
        lua.create_function(|lua, index: Value| {
            let index =
                binding_abi::index_arg(lua, index, "Usage: GetTrainerServiceStepIncrease(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let text = service(&model, index)
                .filter(|s| s.category != TrainerServiceCategory::Used)
                .and_then(|s| s.step_increase.as_ref());
            opt_str(lua, text)
        })?,
    )?;

    // SetTrainerSkillLineFilter(index [, on, exclusive]) (`0x4da3f0`): an index below 1 shows every
    // group; otherwise the group `index - 1` must exist, `on` must be a number, and a zero clears
    // its bit, a nonzero `on` with a nonzero `exclusive` leaves only that bit, any other nonzero
    // `on` adds it. The mask is committed through `0x4d8cb0`: the finalizer, then `TRAINER_UPDATE`.
    g.set(
        "SetTrainerSkillLineFilter",
        lua.create_function(|lua, (index, on, exclusive): (Value, Value, Value)| {
            let index = binding_abi::number_arg(
                lua,
                index,
                r"Usage: SetTrainerSkillLineFilter(index [, on\off, exclusive])",
            )?
            .wrapping_sub(1);
            let groups = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.trainer.as_ref().map_or(0, |t| t.groups.len())
            };
            let mask = if index < 0 {
                u32::MAX
            } else {
                if index as usize >= groups {
                    return Err(mlua::Error::runtime(
                        "Bad skill line in SetTrainerSkillLineFilter",
                    ));
                }
                let on = binding_abi::number_arg(
                    lua,
                    on,
                    "Missing on//off parameter in SetTrainerSkillLineFilter",
                )?;
                let current = lua
                    .app_data_ref::<Model>()
                    .expect("model app_data")
                    .trainer_line_filter;
                let bit = 1u32.wrapping_shl(index as u32);
                if on == 0 {
                    current & !bit
                } else if lua
                    .coerce_number(exclusive)?
                    .is_some_and(|n| binding_abi::ftol(n) != 0)
                {
                    bit
                } else {
                    current | bit
                }
            };
            super::tick::verb(lua, |model| {
                model.trainer_line_filter = mask;
                raise_trainer_update(model);
            });
            Ok(())
        })?,
    )?;

    // GetTrainerSkillLineFilter(index) (`0x4da590`): 1 when the group `index - 1` is shown, nil
    // when
    // it is not; an index below 1 asks whether every group is. Past the group count it raises.
    g.set(
        "GetTrainerSkillLineFilter",
        lua.create_function(|lua, index: Value| {
            let index =
                binding_abi::number_arg(lua, index, "Usage: GetTrainerSkillLineFilter(index)")?
                    .wrapping_sub(1);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let groups = model.trainer.as_ref().map_or(0, |t| t.groups.len());
            let shown = |g: usize| model.trainer_line_filter & 1u32.wrapping_shl(g as u32) != 0;
            if index < 0 {
                return Ok(binding_abi::flag((0..groups).all(shown)));
            }
            if index as usize >= groups {
                return Err(mlua::Error::runtime(
                    "Bad skill line in GetTrainerSkillLineFilter",
                ));
            }
            Ok(binding_abi::flag(shown(index as usize)))
        })?,
    )?;

    // GetTrainerSkillLines() (`0x4da660`): the skill line name of each group's key, nil for a key
    // with no `SkillLine.dbc` row (the known group's -1 too), one value per group, and the count.
    g.set(
        "GetTrainerSkillLines",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            if let Some(t) = model.trainer.as_ref() {
                for g in &t.groups {
                    out.push(opt_str(lua, g.line_name.as_ref())?);
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // Collapse/ExpandTrainerSkillLine(id): fold the group headed by row `id`, or all at 0. `id`
    // spans the whole row array like the getters' gate (`0x4d89b0`); their own bound is untraced.
    g.set(
        "CollapseTrainerSkillLine",
        lua.create_function(|lua, id: Value| {
            // `is-number` else `Usage: CollapseTrainerSkillLine(index)` (`0x84ad24`, `0x4da6d1`).
            let id = binding_abi::index_arg(lua, id, "Usage: CollapseTrainerSkillLine(index)")?;
            super::tick::verb(lua, |model| {
                set_collapsed(model, id, true);
                raise_trainer_update(model);
            });
            Ok(())
        })?,
    )?;
    g.set(
        "ExpandTrainerSkillLine",
        lua.create_function(|lua, id: Value| {
            // `is-number` else `Usage: ExpandTrainerSkillLine(index)` (`0x84ad78`, `0x4da751`).
            let id = binding_abi::index_arg(lua, id, "Usage: ExpandTrainerSkillLine(index)")?;
            super::tick::verb(lua, |model| {
                set_collapsed(model, id, false);
                raise_trainer_update(model);
            });
            Ok(())
        })?,
    )?;

    // SelectTrainerService(index): select the row's service by spell id; past the end clears it
    // (`0x4d74f0` stores 0, never a clamp). Deviation: a header clears it too, because it has no
    // spell id; the reference stores its `0xffffffff` (`0x4d7b05`) and reads back 1, which the
    // stock window's `> 1` test treats the same as 0.
    g.set(
        "SelectTrainerService",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: SelectTrainerService(index)` (`0x84a8a8`, `0x4d8e71`).
            let index = binding_abi::index_arg(lua, index, "Usage: SelectTrainerService(index)")?;
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

    // BuyTrainerService(index): the row's buy. `0x4d89d0` bounds through the getters' gate and
    // refuses any state byte but 0 (`0x4d89e5`), so only an available service sends.
    // Deviation: index 0 is a no-op, because one stray call would spend the purse; the reference
    // buys every visible service for any index at or below 0 (`0x4da244`, `0x4d8a70`).
    g.set(
        "BuyTrainerService",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: BuyTrainerService(index)` (`0x84aad0`, `0x4da221`).
            let index = binding_abi::index_arg(lua, index, "Usage: BuyTrainerService(index)")?;
            let buy = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                // A header row: the reference tests its `+0x30`, which no write sets for a header
                // (`0x4d7af4`-`0x4d7b27`), a stale byte; benilla sends nothing for one.
                service(&model, index)
                    .filter(|s| s.category == TrainerServiceCategory::Available)
                    .zip(model.trainer.as_ref())
                    .map(|(s, t)| TrainerCall::Buy {
                        trainer: t.guid,
                        spell_id: s.spell_id,
                    })
            };
            match buy {
                Some(buy) => submit(lua, buy),
                None => Ok(()),
            }
        })?,
    )?;

    // CloseTrainer(): the close core, no packet, inside the call.
    g.set(
        "CloseTrainer",
        lua.create_function(|lua, ()| submit(lua, TrainerCall::Close))?,
    )?;

    Ok(())
}
#[cfg(test)]
mod tests;
