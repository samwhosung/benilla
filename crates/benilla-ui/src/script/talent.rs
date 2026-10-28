//! The talent bindings: the app pushes the class's pages and talents, resolved from `Talent.dbc`
//! and `Spell.dbc` ([`UiScript::set_talents`]), and `LearnTalent(tab, index)` sends
//! `CMSG_LEARN_TALENT` inside its call ([`TalentCall`]). Grid seats, ranks, prerequisites and
//! availability are the app's.
//!
//! The tuples are what `Blizzard_TalentUI.lua` reads (`:154`, `:180`): `GetTalentTabInfo(i)` gives
//! name, texture, pointsSpent, fileName; `GetTalentInfo(tab, i)` name, icon, tier, column, rank,
//! maxRank, isExceptional, meetsPrereq. Tiers and columns are 1-based, as the stock frame indexes
//! `TALENT_BRANCH_ARRAY[tier][column]`. The respec pair answers a class trainer's
//! `CONFIRM_TALENT_WIPE` and reads no snapshot.
//!
//! `GameTooltip:SetTalent` (`0x535170`) builds an exceptional talent with the spell builder
//! (`0x52e610`) and any other with the talent builder (`0x52b0a0`), which keeps the spell's name
//! and description and drops the rest of its body. The talent lines are the talent builder's: the
//! rank line (`TOOLTIP_TALENT_RANK`, `0x854a2c`), the red requirement lines after it, the "Next
//! rank:" block (`TOOLTIP_TALENT_NEXT_RANK`, `0x854a10`, white, over the next rank's gold
//! description) and "Click to learn" (`TOOLTIP_TALENT_LEARN`, `0x8549f8`) in green on a learnable
//! rank. The spell builder writes the same lines around the full body (`0x52f735`); its next-rank
//! pass, a second full body, never runs, as no 1.12 exceptional talent has a second rank.

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::{self, flag};
use super::live::{ask, LiveQuery};
use super::tooltip_spell::{spell_view_of, TalentLines};
use super::Model;

/// One talent page, a `TalentTab.dbc` row: `GetTalentTabInfo`'s source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentTabView {
    pub name: String,
    /// The `Interface\TalentFrame\<base>-` art base, the tuple's `fileName`.
    pub background: String,
    pub points_spent: u32,
    /// The tab's `SpellIcon.dbc` path (`TalentTab.dbc` `SpellIconID`, `0x4f30d0`-`0x4f30f1`), the
    /// tuple's `texture`; `None` pushes nil (`0x4f30f8`).
    pub texture: Option<String>,
}

/// One prerequisite edge, a `GetTalentPrereqs` triplet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TalentPrereqView {
    pub tier: u32,
    pub column: u32,
    /// The prerequisite is learned to its required rank.
    pub learnable: bool,
}

/// One talent button: `GetTalentInfo`'s source and the tooltip's context.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentView {
    /// The `Talent.dbc` id, which `CMSG_LEARN_TALENT` carries (`0x4f37c7`).
    pub talent_id: u32,
    pub name: String,
    pub texture: Option<String>,
    pub tier: u32,
    pub column: u32,
    /// Current rank, 0 when unlearned.
    pub rank: u32,
    pub max_rank: u32,
    /// `Talent.dbc` flags bit 0 (`TalentRec+0x4c`), the tuple's `isExceptional`; the stock frame
    /// ignores it.
    pub exceptional: bool,
    /// The required spell is known; prerequisite talents live in the triplets.
    pub meets_prereq: bool,
    pub prereqs: Vec<TalentPrereqView>,
    /// The spell id of rank `max(1, rank)`, the tooltip's spell part.
    pub display_spell: u32,
    /// The next rank's spell id when `0 < rank < max_rank`, else 0.
    pub next_spell: u32,
    /// The red requirement lines of an unlearned talent, in `0x52b390`'s order; their words are
    /// read as the tooltip builds ([`TalentReq`]).
    pub reqs: Vec<TalentReq>,
    /// A higher rank is learnable now, every requirement met, a point to spend and a rank below
    /// the maximum: the green `TOOLTIP_TALENT_LEARN` line (`0x52b31b`, `0x52f735`).
    pub learnable: bool,
}

/// One red requirement line, its template a `_G` read at the tooltip's build (`0x703bf0`): an
/// addon's rewording shows at the next hover, and a key it lacks renders no line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TalentReq {
    /// `TOOLTIP_TALENT_PREREQ`, its `_P1` twin by the rank (`0x52b4a5`, the ordinal in `edx`), of
    /// the 1-based rank needed and the prerequisite's name.
    Prereq { rank: u32, name: String },
    /// `TOOLTIP_TALENT_TIER_POINTS` (`0x52b566`) of the points the tier needs and the tab's name.
    Tier { points: u32, tab: String },
    /// `ITEM_REQ_SKILL` (`0x52b60b`) of the required spell's name.
    Spell { name: String },
}

impl TalentReq {
    /// The line as `0x52b390` writes it now, `None` for a missing or empty template.
    fn line(&self, lua: &Lua) -> Option<String> {
        use crate::strings::{fill, global, plural, Arg, Strings};
        let get = |key: &str| global(lua, key);
        Some(match self {
            Self::Prereq { rank, name } => fill(
                &plural("TOOLTIP_TALENT_PREREQ", Some(*rank), Strings::Vm(lua))?,
                &[Arg::D(i64::from(*rank)), Arg::S(name)],
            ),
            Self::Tier { points, tab } => fill(
                &get("TOOLTIP_TALENT_TIER_POINTS")?,
                &[Arg::D(i64::from(*points)), Arg::S(tab)],
            ),
            Self::Spell { name } => fill(&get("ITEM_REQ_SKILL")?, &[Arg::S(name)]),
        })
    }
}

/// The pushed snapshot: the player's own pages and unspent points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentUiState {
    pub tabs: Vec<TalentTabView>,
    /// `talents[t]` holds tab `t+1`'s talents, in the app's enumeration order.
    pub talents: Vec<Vec<TalentView>>,
    /// `UnitCharacterPoints("player")`: unspent talent points, free primary professions.
    pub points: (u32, u32),
}

impl super::UiScript {
    /// Push the whole talent snapshot.
    pub fn set_talents(&mut self, state: TalentUiState) {
        self.model_mut().talents = state;
    }

    /// The talent verbs a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_talent_calls(&mut self) -> Vec<TalentCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Talent(call) => Some(*call),
            _ => None,
        })
    }

    /// Push whether a trainer's respec question is live and in range, which
    /// `CheckTalentMasterDist()` answers; false for none pending or walked away.
    pub fn set_talent_master_pending(&mut self, pending: bool) {
        let mut model = self.model_mut();
        if model.talent_master_pending != pending {
            model.talent_master_pending = pending;
        }
    }
}

/// A talent verb the host applies inside its call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TalentCall {
    /// `LearnTalent(tab, index)` (`0x4f36a0`) on a talent below its maximum (`0x4f378f`):
    /// `CMSG_LEARN_TALENT` (`0x251`) with the talent id and its rank count, the 0-based next rank
    /// (`0x4f37c7`, `0x4f37d1`).
    Learn { talent_id: u32, rank: u32 },
    /// `ConfirmTalentWipe()` (`0x48dc40`): the respec dialog `0x5df980` with a zero guid, which
    /// answers the trainer it latched.
    ConfirmWipe,
}

/// The player's talent tabs as they stand, `GetTalentTabInfo`'s rows: the tab list `0xbdcd28`
/// with each tab's points spent off the known spells (`+4`); declined without a player held.
#[derive(Clone, Copy, Debug)]
pub struct TalentTabs;

impl LiveQuery for TalentTabs {
    type Answer = Vec<TalentTabView>;
}

/// A tab's talent count (`+8`, `0x4f3160`), by its 0-based tab; 0 past the tabs.
#[derive(Clone, Copy, Debug)]
pub struct TalentCount(pub usize);

impl LiveQuery for TalentCount {
    type Answer = usize;
}

/// One talent by its 0-based (tab, index), as `GetTalentInfo` reads it off the row and the known
/// spells at the call (`0x4f3200`); `None` past the tab or its talents.
#[derive(Clone, Copy, Debug)]
pub struct TalentAt(pub usize, pub usize);

impl LiveQuery for TalentAt {
    type Answer = Option<TalentView>;
}

/// The active player's two point counters, `+0xe48` and `+0xe4c` of its player block
/// (`0x519463`): unspent talent points and free primary professions; `(0, 0)` with none held.
#[derive(Clone, Copy, Debug)]
pub struct CharacterPoints;

impl LiveQuery for CharacterPoints {
    type Answer = (u32, u32);
}

/// Whether a trainer's respec question is latched and its trainer in range at the call
/// (`0x48d194` → `0x5dfb10`).
#[derive(Clone, Copy, Debug)]
pub struct TalentMasterPending;

impl LiveQuery for TalentMasterPending {
    type Answer = bool;
}

/// The tabs now: the host's, else the VM's copy.
fn tabs(lua: &Lua, model: &Model) -> Vec<TalentTabView> {
    ask(lua, &TalentTabs).unwrap_or_else(|| model.talents.tabs.clone())
}

/// One talent by its 1-based (tab, index), now: the host's, else the VM's copy.
fn talent_at(lua: &Lua, model: &Model, tab: usize, index: usize) -> Option<TalentView> {
    let (tab, index) = (tab.checked_sub(1)?, index.checked_sub(1)?);
    match ask(lua, &TalentAt(tab, index)) {
        Some(live) => live,
        None => model.talents.talents.get(tab)?.get(index).cloned(),
    }
}

/// The two point counters now: the host's, else the VM's copy.
pub(super) fn character_points(lua: &Lua, model: &Model) -> (u32, u32) {
    ask(lua, &CharacterPoints).unwrap_or(model.talents.points)
}

/// Register the talent globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumTalentTabs",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(tabs(lua, &model).len() as i64)
        })?,
    )?;

    // Name, the tab's `SpellIcon.dbc` path (nil for an id the table lacks), points spent and the
    // background (`0x4f30cb`-`0x4f3121`). Out of range answers nil, nil, 0, nil (`0x4f3132`).
    g.set(
        "GetTalentTabInfo",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: GetTalentTabInfo(index)` (`0x84cdd4`, `0x4f3059`).
            let i = binding_abi::index_arg(lua, i, "Usage: GetTalentTabInfo(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let tabs = tabs(lua, &model);
            let Some(tab) = i.checked_sub(1).and_then(|n| tabs.get(n)) else {
                return Ok(super::binding_abi::tuple(&[None, None, Some(0), None]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&tab.name)?),
                match &tab.texture {
                    Some(texture) => Value::String(lua.create_string(texture)?),
                    None => Value::Nil,
                },
                Value::Integer(i64::from(tab.points_spent)),
                Value::String(lua.create_string(&tab.background)?),
            ]))
        })?,
    )?;

    g.set(
        "GetNumTalents",
        lua.create_function(|lua, tab: Value| {
            // `is-number` else `Usage: GetNumTalents(tabIndex)` (`0x84cdf4`, `0x4f3177`).
            let tab = binding_abi::index_arg(lua, tab, "Usage: GetNumTalents(tabIndex)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let n = tab.checked_sub(1).map_or(0, |t| {
                ask(lua, &TalentCount(t))
                    .unwrap_or_else(|| model.talents.talents.get(t).map_or(0, Vec::len))
            });
            Ok(n as i64)
        })?,
    )?;

    g.set(
        "GetTalentInfo",
        lua.create_function(|lua, (tab, i): (Value, Value)| {
            // Both `is-number` else `Usage: GetTalentInfo(tabIndex, talentIndex)` (`0x84ce14`,
            // `0x4f34ae`).
            let tab =
                binding_abi::index_arg(lua, tab, "Usage: GetTalentInfo(tabIndex, talentIndex)")?;
            let i = binding_abi::index_arg(lua, i, "Usage: GetTalentInfo(tabIndex, talentIndex)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(t) = talent_at(lua, &model, tab, i) else {
                // `0x4f3454`: nil, nil, 1, 1, 0, 0, nil, nil.
                return Ok(super::binding_abi::tuple(&[
                    None,
                    None,
                    Some(1),
                    Some(1),
                    Some(0),
                    Some(0),
                    None,
                    None,
                ]));
            };
            let texture = match &t.texture {
                Some(tex) => Value::String(lua.create_string(tex)?),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&t.name)?),
                texture,
                Value::Integer(i64::from(t.tier)),
                Value::Integer(i64::from(t.column)),
                Value::Integer(i64::from(t.rank)),
                Value::Integer(i64::from(t.max_rank)),
                flag(t.exceptional),
                flag(t.meets_prereq),
            ]))
        })?,
    )?;

    // Flat (tier, column, isLearnable) triplets, as `Blizzard_TalentUI.lua:385` walks them.
    g.set(
        "GetTalentPrereqs",
        lua.create_function(|lua, (tab, i): (Value, Value)| {
            // `is-number` on args 1 and 2 else `Usage: GetTalentPrereqs(tabIndex, talentIndex)`
            // (`0x84ce40`, `0x4f3686`).
            let tab =
                binding_abi::index_arg(lua, tab, "Usage: GetTalentPrereqs(tabIndex, talentIndex)")?;
            let i =
                binding_abi::index_arg(lua, i, "Usage: GetTalentPrereqs(tabIndex, talentIndex)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            if let Some(t) = talent_at(lua, &model, tab, i) {
                for p in &t.prereqs {
                    out.push(Value::Integer(i64::from(p.tier)));
                    out.push(Value::Integer(i64::from(p.column)));
                    out.push(flag(p.learnable));
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // LearnTalent(tab, index): the talent's rank is its known ranks, and one at its maximum sends
    // nothing (`0x4f378f`), the client's only gate; points and prerequisites are the server's
    // (vmangos `Player::LearnTalent`).
    g.set(
        "LearnTalent",
        lua.create_function(|lua, (tab, i): (Value, Value)| {
            // `is-number` on args 1 and 2 else `Usage: LearnTalent(tabIndex, talentIndex)`
            // (`0x84ce70`, `0x4f380c`).
            let tab =
                binding_abi::index_arg(lua, tab, "Usage: LearnTalent(tabIndex, talentIndex)")?;
            let i = binding_abi::index_arg(lua, i, "Usage: LearnTalent(tabIndex, talentIndex)")?;
            let learn = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                talent_at(lua, &model, tab, i)
                    .filter(|t| t.rank < t.max_rank)
                    .map(|t| TalentCall::Learn {
                        talent_id: t.talent_id,
                        rank: t.rank,
                    })
            };
            match learn {
                Some(call) => super::calls::submit(lua, super::ScriptCall::Talent(call)),
                None => Ok(()),
            }
        })?,
    )?;

    // ConfirmTalentWipe(): the `CONFIRM_TALENT_WIPE` Accept, the one call that unlearns talents. No
    // argument: the reference answers the trainer guid it latched (`0xc4d7a0`), which the app holds.
    g.set(
        "ConfirmTalentWipe",
        lua.create_function(|lua, ()| {
            super::calls::submit(lua, super::ScriptCall::Talent(TalentCall::ConfirmWipe))
        })?,
    )?;

    // CheckTalentMasterDist(): polled by that dialog's OnUpdate, nil hides it. The reference
    // runs its interact-range test on the latched trainer at the call (`0x48d170` → `0x5dfb10`,
    // `d² <= [0xc4c28c]`); no packet either way.
    g.set(
        "CheckTalentMasterDist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(
                ask(lua, &TalentMasterPending).unwrap_or(model.talent_master_pending),
            ))
        })?,
    )?;

    // UnitCharacterPoints(unit) (`0x519420`): the token resolved (`0x6f3690`, `0x515940`), then the
    // two counters (`+0xe48`, `+0xe4c`) of the player block, which the object holds for the active
    // player alone (`0x519463`-`0x51949e`: the guid must equal the active player's); any other
    // unit, or none, answers 0, 0 (`0x5194da`). An unrecognised token raises in the resolver.
    g.set(
        "UnitCharacterPoints",
        lua.create_function(|lua, unit: mlua::Value| {
            let unit =
                super::binding_abi::string_arg(lua, unit, r#"Usage: UnitCharacterPoints("unit")"#)?;
            super::unit::check_unit_token(&Some(unit.clone()))?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (cp1, cp2) = if super::unit::names_self(lua, &model, &unit) {
                character_points(lua, &model)
            } else {
                (0, 0)
            };
            Ok((i64::from(cp1), i64::from(cp2)))
        })?,
    )?;

    Ok(())
}

/// Register `GameTooltip:SetTalent(tab, index)` into the tooltip method table.
pub(super) fn install_tooltip_method(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetTalent",
        lua.create_function(|lua, (this, tab, i): (Table, Value, Value)| {
            // `is-number` on args 2 and 3 else `Usage: SetTalent(tabIndex, talentIndex)`
            // (`0x85555c`, `0x535394`).
            let tab = binding_abi::index_arg(lua, tab, "Usage: SetTalent(tabIndex, talentIndex)")?;
            let i = binding_abi::index_arg(lua, i, "Usage: SetTalent(tabIndex, talentIndex)")?;
            let (display, ranks, reqs, mut lines) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let Some(t) = talent_at(lua, &model, tab, i) else {
                    return Ok(());
                };
                (
                    t.display_spell,
                    (t.rank, t.max_rank),
                    t.reqs.clone(),
                    TalentLines {
                        exceptional: t.exceptional,
                        rank_line: None,
                        reqs: Vec::new(),
                        next_spell: t.next_spell,
                        next_desc: None,
                        learn: t.learnable,
                    },
                )
            };
            // The next rank's description, built at the call through the lent host, else off the
            // store, where a miss shows on the next hover.
            lines.next_desc = super::tooltip_spell::spell_view_peek(lua, lines.next_spell, false)
                .map(|v| v.description);
            // `TOOLTIP_TALENT_RANK`, "Rank %d/%d" (`0x854a2c`, pushed at `0x52b213`); an
            // install without it shows no rank line. A GlobalStrings read runs an `_G` `__index`
            // (`0x703bf0`), which may call any verb, so it is made once the Model is released.
            lines.rank_line = crate::strings::global(lua, "TOOLTIP_TALENT_RANK").map(|tmpl| {
                crate::strings::fill(
                    &tmpl,
                    &[
                        crate::strings::Arg::D(ranks.0.into()),
                        crate::strings::Arg::D(ranks.1.into()),
                    ],
                )
            });
            // Then the requirement lines' templates, read the same way (`0x52b390`).
            lines.reqs = reqs.iter().filter_map(|r| r.line(lua)).collect();
            super::tooltip_spell::set_spell_with_talent(lua, &this, display, lines)
        })?,
    )?;
    Ok(())
}

/// Ask the spell store for the next rank's description; a miss queues it as a primary view's does.
pub(super) fn ask_next_rank(lua: &Lua, next_spell: u32) {
    if next_spell != 0 {
        let _ = spell_view_of(lua, next_spell, false); // a miss records the ask as a side effect
    }
}
