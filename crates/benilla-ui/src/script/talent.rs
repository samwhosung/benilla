//! The talent bindings: the app pushes the class's pages and talents, resolved from `Talent.dbc`
//! and `Spell.dbc` ([`UiScript::set_talents`]), and `LearnTalent(tab, index)` queues clicks for
//! `CMSG_LEARN_TALENT`. Grid seats, ranks, prerequisites and availability are the app's.
//!
//! The tuples are what `Blizzard_TalentUI.lua` reads (`:154`, `:180`): `GetTalentTabInfo(i)` gives
//! name, texture, pointsSpent, fileName; `GetTalentInfo(tab, i)` name, icon, tier, column, rank,
//! maxRank, isExceptional, meetsPrereq. Tiers and columns are 1-based, as the stock frame indexes
//! `TALENT_BRANCH_ARRAY[tier][column]`. The respec pair answers a class trainer's
//! `CONFIRM_TALENT_WIPE` and reads no snapshot.
//!
//! `GameTooltip:SetTalent` is the spell builder (`0x52e610`) with talent lines: the rank line
//! (`TOOLTIP_TALENT_RANK`, `0x854a2c`), the red requirement lines after it, and "Click to learn"
//! (`TOOLTIP_TALENT_LEARN`, `0x8549f8`) in green on a learnable rank. The "Next rank:" block,
//! `TOOLTIP_TALENT_NEXT_RANK` in white and the next rank's description in gold, is untraced in
//! the reference.

use mlua::{Lua, MultiValue, Table, Value};

use super::tooltip_spell::{spell_view_of, TalentLines};
use super::Model;

/// One talent page, a `TalentTab.dbc` row: `GetTalentTabInfo`'s source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentTabView {
    pub name: String,
    /// The `Interface\TalentFrame\<base>-` art base, the tuple's `fileName`.
    pub background: String,
    pub points_spent: u32,
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
    /// Red requirement lines (`TOOLTIP_TALENT_TIER_POINTS`, `TOOLTIP_TALENT_PREREQ`), shown while
    /// the talent is locked.
    pub req_lines: Vec<String>,
    /// A higher rank is learnable now: the green `TOOLTIP_TALENT_LEARN` line and border.
    pub learnable: bool,
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

    /// Drain the queued `LearnTalent(tab, index)` clicks, both 1-based.
    pub fn take_talent_learns(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().talent_learns)
    }

    /// Drain the count of `ConfirmTalentWipe()` calls, each an outbound `MSG_TALENT_WIPE_CONFIRM`;
    /// a count, since the app holds the trainer's guid.
    pub fn take_talent_wipe_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().talent_wipe_confirms)
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

/// One talent by its 1-based (tab, index).
fn talent_at(model: &Model, tab: usize, index: usize) -> Option<&TalentView> {
    model
        .talents
        .talents
        .get(tab.checked_sub(1)?)
        .and_then(|t| t.get(index.checked_sub(1)?))
}

/// Register the talent globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumTalentTabs",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.talents.tabs.len() as i64)
        })?,
    )?;

    // `texture` is nil: the stock frame never reads it. Out of range answers a single nil.
    g.set(
        "GetTalentTabInfo",
        lua.create_function(|lua, i: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(tab) = i.checked_sub(1).and_then(|n| model.talents.tabs.get(n)) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&tab.name)?),
                Value::Nil,
                Value::Integer(i64::from(tab.points_spent)),
                Value::String(lua.create_string(&tab.background)?),
            ]))
        })?,
    )?;

    g.set(
        "GetNumTalents",
        lua.create_function(|lua, tab: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let n = tab
                .checked_sub(1)
                .and_then(|t| model.talents.talents.get(t))
                .map_or(0, Vec::len);
            Ok(n as i64)
        })?,
    )?;

    g.set(
        "GetTalentInfo",
        lua.create_function(|lua, (tab, i): (usize, usize)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(t) = talent_at(&model, tab, i) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
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
                Value::Integer(i64::from(t.exceptional)),
                Value::Boolean(t.meets_prereq),
            ]))
        })?,
    )?;

    // Flat (tier, column, isLearnable) triplets, as `Blizzard_TalentUI.lua:385` walks them.
    g.set(
        "GetTalentPrereqs",
        lua.create_function(|lua, (tab, i): (usize, usize)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::new();
            if let Some(t) = talent_at(&model, tab, i) {
                for p in &t.prereqs {
                    out.push(Value::Integer(i64::from(p.tier)));
                    out.push(Value::Integer(i64::from(p.column)));
                    out.push(Value::Boolean(p.learnable));
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // The app gates the send on its own `learnable`; the server re-validates
    // (vmangos `Player::LearnTalent`).
    g.set(
        "LearnTalent",
        lua.create_function(|lua, (tab, i): (u32, u32)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.talent_learns.push((tab, i));
            Ok(())
        })?,
    )?;

    // ConfirmTalentWipe(): the `CONFIRM_TALENT_WIPE` Accept, the one call that unlearns talents.
    // No argument: the reference sends the trainer guid it latched (`0xc4d7a0`); the app holds it.
    g.set(
        "ConfirmTalentWipe",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.talent_wipe_confirms += 1;
            Ok(())
        })?,
    )?;

    // CheckTalentMasterDist(): polled by that dialog's OnUpdate, false hides it. The reference
    // re-runs its interact-range test on the latched trainer (`0x5df980`, `d² <= [0xc4c28c]`);
    // no packet either way.
    g.set(
        "CheckTalentMasterDist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.talent_master_pending)
        })?,
    )?;

    // Every unit token answers the player's own pair: `PLAYER_CHARACTER_POINTS1/2` are private
    // fields only we receive.
    g.set(
        "UnitCharacterPoints",
        lua.create_function(|lua, _unit: String| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (cp1, cp2) = model.talents.points;
            Ok((i64::from(cp1), i64::from(cp2)))
        })?,
    )?;

    Ok(())
}

/// Register `GameTooltip:SetTalent(tab, index)` into the tooltip method table.
pub(super) fn install_tooltip_method(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetTalent",
        lua.create_function(|lua, (this, tab, i): (Table, usize, usize)| {
            let (display, lines) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let Some(t) = talent_at(&model, tab, i) else {
                    return Ok(());
                };
                // From the ask-once spell store; a miss shows on the next hover.
                let next_desc = (t.next_spell != 0)
                    .then(|| {
                        model
                            .spell_tooltips
                            .get(&t.next_spell)
                            .map(|v| v.description.clone())
                    })
                    .flatten();
                // `TOOLTIP_TALENT_RANK`, "Rank %d/%d" (`0x854a2c`, pushed at `0x52b213`); an
                // install without it shows no rank line.
                let rank_line = crate::strings::global(lua, "TOOLTIP_TALENT_RANK").map(|tmpl| {
                    crate::strings::fill(
                        &tmpl,
                        &[
                            crate::strings::Arg::D(t.rank.into()),
                            crate::strings::Arg::D(t.max_rank.into()),
                        ],
                    )
                });
                (
                    t.display_spell,
                    TalentLines {
                        rank_line,
                        reqs: t.req_lines.clone(),
                        next_spell: t.next_spell,
                        next_desc,
                        learn: t.learnable,
                    },
                )
            };
            super::tooltip_spell::set_spell_with_talent(lua, &this, display, lines)
        })?,
    )?;
    Ok(())
}

/// Ask the spell store for the next rank's description; a miss queues it as a primary view's does.
pub(super) fn ask_next_rank(lua: &Lua, next_spell: u32) {
    if next_spell != 0 {
        let _ = spell_view_of(lua, next_spell); // a miss records the ask as a side effect
    }
}
