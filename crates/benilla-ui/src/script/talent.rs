//! The talent window's engine seam (decision 0304) — the same two-way shape as
//! [`super::spellbook`]: the app pushes a **talent snapshot** ([`UiScript::set_talents`] — the
//! class's tabs + each tab's talents, already resolved to name/icon/rank/availability by the
//! app's `Talent.dbc` × `Spell.dbc` join), the Era bindings below read it verbatim, and
//! `LearnTalent(tab, index)` queues outbound intents the app drains
//! ([`UiScript::take_talent_learns`]) into `CMSG_LEARN_TALENT`. The engine holds no talent
//! KNOWLEDGE — grid seats, ranks, prerequisites, and availability are the app's resolve.
//!
//! The **respec** pair rides here too (decision 1580) and is the binder question's twin, not a
//! talent-window affordance: `ConfirmTalentWipe()` answers a class trainer's
//! `CONFIRM_TALENT_WIPE`, and `CheckTalentMasterDist()` is the range poll that takes the dialog
//! away when you walk off. Both are 1.12 engine bindings (`reference/1.12-globals.tsv`), and
//! neither reads the snapshot above — the question arrives as the event's argument.
//!
//! The Era tuple shapes are the 1.12 addon's own reads (`Blizzard_TalentUI.lua`, extracted from
//! the patch chain — decision 0304's pin §3):
//! `GetTalentTabInfo(i) → name, texture, pointsSpent, fileName`;
//! `GetTalentInfo(tab, i) → name, icon, tier, column, rank, maxRank, isExceptional, meetsPrereq`;
//! `GetTalentPrereqs(tab, i) → tier, column, isLearnable, …` (flat triplets);
//! `UnitCharacterPoints(unit) → cp1, cp2`. Tiers/columns are **1-based** Lua-facing (the
//! reference indexes `TALENT_BRANCH_ARRAY[tier][column]` directly) — the app pushes them
//! 1-based; `isExceptional` is pushed but unused by the reference frame's own render.
//!
//! ## The tooltip (`GameTooltip:SetTalent(tab, index)`)
//!
//! The 1.12 talent tooltip IS the spell builder `0x52e610` with the talent params: line 2
//! `TOOLTIP_TALENT_RANK 0x854a2c` "Rank %d/%d" white iff `param7≠0 && param8==0`; line 13
//! `TOOLTIP_TALENT_LEARN 0x8549f8` "Click to learn" green on a learnable higher rank — so
//! `SetTalent` renders THROUGH the spell channel: the talent's display-rank
//! spell view comes from the same ask-once store `SetSpell` uses (a miss queues the id for the
//! app's resolver), interleaved with the talent lines ([`TalentLines`]). The red requirement
//! lines' position (here: after the rank line) is CONFIRMED — decision 0305's residue: it
//! matches the builder law. Still open: the "Next rank:" block's composition (here:
//! `TOOLTIP_TALENT_NEXT_RANK` white + the next rank spell's description gold — its view from the
//! same store) — 0305 didn't walk it.

use mlua::{Lua, MultiValue, Table, Value};

use super::tooltip_spell::{spell_view_of, TalentLines};
use super::Model;

/// One talent page (`TalentTab.dbc` row, app-resolved) — `GetTalentTabInfo`'s source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentTabView {
    pub name: String,
    /// The `Interface\TalentFrame\<base>-` art base (`fileName` in the Era tuple).
    pub background: String,
    /// Points spent across this page's talents (the app's sum of ranks).
    pub points_spent: u32,
}

/// One prerequisite edge for the branch drawing — `GetTalentPrereqs`' triplet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TalentPrereqView {
    /// 1-based, Lua-facing (module doc).
    pub tier: u32,
    pub column: u32,
    /// The prereq is learned to its required rank (drives arrow color + availability).
    pub learnable: bool,
}

/// One talent button's full render state — `GetTalentInfo`'s source plus the tooltip context.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentView {
    pub name: String,
    pub texture: Option<String>,
    /// 1-based, Lua-facing (module doc).
    pub tier: u32,
    pub column: u32,
    /// Current rank (0 = unlearned) / the talent's own rank count.
    pub rank: u32,
    pub max_rank: u32,
    /// `Talent.dbc` flags bit 0 — the Era tuple's `isExceptional` (byte-verified
    /// `TalentRec+0x4c` bit0; the reference frame reads and ignores it).
    pub exceptional: bool,
    /// The non-prereq requirements hold — the required-spell known-check only, prereqs live in
    /// the triplets (decision 0305: `meetsPrereq`'s derivation, confirmed as built).
    pub meets_prereq: bool,
    pub prereqs: Vec<TalentPrereqView>,
    /// The tooltip's spell part: the display rank's spell id (rank max(1, rank)) — its view
    /// rides the spell channel's ask-once store.
    pub display_spell: u32,
    /// The next rank's spell id when `0 < rank < max_rank` (the "Next rank:" block); 0 = none.
    pub next_spell: u32,
    /// App-composed red requirement lines (`TOOLTIP_TALENT_TIER_POINTS`/`_PREREQ[_P1]`), shown
    /// while the talent is locked.
    pub req_lines: Vec<String>,
    /// The green `TOOLTIP_TALENT_LEARN` hint: a learnable higher rank (tier unlocked, prereqs
    /// met, points available) — the same gate the frame's green border wears.
    pub learnable: bool,
}

/// The pushed snapshot: the player's own pages + unspent points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentUiState {
    pub tabs: Vec<TalentTabView>,
    /// `talents[t]` = tab `t+1`'s talents, in the enumeration order the app pinned.
    pub talents: Vec<Vec<TalentView>>,
    /// `UnitCharacterPoints("player")`: (unspent talent points, free primary professions).
    pub points: (u32, u32),
}

impl super::UiScript {
    /// Push the whole talent snapshot (the app's feed on SPELLS_CHANGED/points change).
    pub fn set_talents(&mut self, state: TalentUiState) {
        self.model_mut().talents = state;
    }

    /// Drain the queued `LearnTalent(tab, index)` clicks (both 1-based, as the Lua passed them).
    pub fn take_talent_learns(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().talent_learns)
    }

    /// Drain the `ConfirmTalentWipe()` calls queued since the last drain — each one is an outbound
    /// `MSG_TALENT_WIPE_CONFIRM` ([`Self::take_binder_confirms`]'s shape, and a count for the same
    /// reason: the app holds the trainer's guid).
    pub fn take_talent_wipe_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().talent_wipe_confirms)
    }

    /// Push whether a trainer's respec question is live and still in range — the host's half of
    /// `CheckTalentMasterDist()`. Idempotent; `false` covers both "no question pending" and "you
    /// walked away", which is all the dialog's OnUpdate needs in order to decide to hide.
    pub fn set_talent_master_pending(&mut self, pending: bool) {
        let mut model = self.model_mut();
        if model.talent_master_pending != pending {
            model.talent_master_pending = pending;
        }
    }
}

/// Look up one talent by the Lua-facing (tab, index) pair (both 1-based).
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

    // GetTalentTabInfo(i) -> name, texture, pointsSpent, fileName. `texture` is nil — the
    // reference frame never reads it (module doc); out of range -> a single nil.
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

    // GetTalentInfo(tab, i) -> name, icon, tier, column, rank, maxRank, isExceptional,
    // meetsPrereq. `isExceptional` is 0 (pushed nowhere; the reference render ignores it).
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

    // GetTalentPrereqs(tab, i) -> tier, column, isLearnable per prerequisite, flattened (the
    // reference walks `for i=5, arg.n, 3`); none -> empty.
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

    // LearnTalent(tab, i): queue the click for the app's wire drain. The availability gate is
    // the app's at send time (mirroring its own pushed `learnable`) — the server re-validates
    // regardless (vmangos Player::LearnTalent).
    g.set(
        "LearnTalent",
        lua.create_function(|lua, (tab, i): (u32, u32)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.talent_learns.push((tab, i));
            Ok(())
        })?,
    )?;

    // ConfirmTalentWipe() — the CONFIRM_TALENT_WIPE dialog's Accept, and the one call in the
    // client that unlearns talents: the trainer's question changed nothing (decision 1580).
    // Zero-arg because the guid it sends is one the client latched from that question — the
    // reference's own `0xc4d7a0`; here the app holds it.
    g.set(
        "ConfirmTalentWipe",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.talent_wipe_confirms += 1;
            Ok(())
        })?,
    )?;

    // CheckTalentMasterDist() — polled from that dialog's OnUpdate; false hides it. The reference
    // re-runs its interact-range test against the latched trainer (`0x5df980`, the same
    // `d² <= [0xc4c28c]` gate `CheckBinderDist` runs), so walking away takes the question off
    // screen with no packet either way.
    g.set(
        "CheckTalentMasterDist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.talent_master_pending)
        })?,
    )?;

    // UnitCharacterPoints(unit) -> cp1 (unspent talent points), cp2 (free professions). The
    // pair is our own player's (PLAYER_CHARACTER_POINTS1/2 are PRIVATE fields) — any other
    // unit token answers the same store, matching the fields only we ever receive.
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

/// Register `GameTooltip:SetTalent(tab, index)` into the tooltip kind method table (module doc:
/// the spell builder with talent lines).
pub(super) fn install_tooltip_method(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetTalent",
        lua.create_function(|lua, (this, tab, i): (Table, usize, usize)| {
            let (display, lines) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let Some(t) = talent_at(&model, tab, i) else {
                    return Ok(());
                };
                // The "Next rank:" block needs the next rank's description — from the same
                // ask-once spell store (a miss shows the block on the hover's re-enter).
                let next_desc = (t.next_spell != 0)
                    .then(|| {
                        model
                            .spell_tooltips
                            .get(&t.next_spell)
                            .map(|v| v.description.clone())
                    })
                    .flatten();
                // `TOOLTIP_TALENT_RANK` = "Rank %d/%d" (`0x854a2c`, pushed at `0x52b213`) —
                // the key, filled with the two counts, never a sentence of ours (decision 2045).
                // An install whose string table lacks it shows no rank line at all.
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

/// The spell-store ask for the next-rank description (kept beside [`install_tooltip_method`] so
/// the ask-once discipline stays in one place): a `SetTalent` render whose next-rank view is
/// missing queues it exactly like a primary-view miss.
pub(super) fn ask_next_rank(lua: &Lua, next_spell: u32) {
    if next_spell != 0 {
        let _ = spell_view_of(lua, next_spell); // a miss records the ask as a side effect
    }
}
