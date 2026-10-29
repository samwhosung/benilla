//! The release, reclaim, resurrect and self-res verbs and the getters the `DEATH`,
//! `RECOVER_CORPSE`, `RESURRECT` and `XP_LOSS` dialogs call, each reading the death state through
//! the lent host as it runs ([`DeathQuery`]), as the reference's read their globals at the call
//! (`GetCorpseRecoveryDelay` `0x48b5d0`). The three predicates answer `1` or nil, as the reference
//! does (`ResurrectHasSickness` `0x48aa00`, `ResurrectHasTimer` `0x48aa30`,
//! `CheckSpiritHealerDist` `0x48d120`).

use mlua::{Lua, Value};

use super::binding_abi::flag;
use super::live::{ask, LiveQuery};
use super::Model;

/// The death state the getters read through the host as they run; the VM's copy of it
/// ([`super::UiScript::set_death`]) answers only with no host to ask.
pub struct DeathQuery;

impl LiveQuery for DeathQuery {
    type Answer = DeathUiState;
}

/// The active player's level, which `GetResSicknessDuration` words the sickness by; `None` with
/// no active player held.
pub struct ResSicknessLevelQuery;

impl LiveQuery for ResSicknessLevelQuery {
    type Answer = u32;
}

/// The death state as the getters read it now: the host's, else the VM's copy, which only a test
/// sets (every production entry that runs Lua lends the host).
fn state(lua: &Lua) -> DeathUiState {
    ask(lua, &DeathQuery).unwrap_or_else(|| {
        lua.app_data_ref::<Model>()
            .expect("model app_data")
            .death
            .clone()
    })
}

/// The sickness a spirit-healer res applies at `level`, per vmangos `Player::ResurrectPlayer`:
/// none below 11, `level - 10` minutes through 19, 10 minutes from 20. Worded with `GENERIC_MIN`,
/// the minute arm of the reference's formatter (`0x51a3a0` into `0x52fa50`).
///
/// Deviation: the minutes come from the server's table, not from spell 15007's `Spell.dbc`
/// duration, because the aura the server applies is what the dialog must warn about.
pub(crate) fn sickness_duration(
    level: u32,
    strings: crate::strings::Strings<'_>,
) -> Option<String> {
    let minutes = match level {
        0..=10 => return None,
        11..=19 => level - 10,
        _ => 10,
    };
    // `GetText(token, nil, ordinal)`, the VM's own.
    let template = crate::strings::plural("GENERIC_MIN", Some(minutes), strings)?;
    Some(crate::strings::fill(
        &template,
        &[crate::strings::Arg::D(i64::from(minutes))],
    ))
}

/// The death state the getters read: the VM's copy when no host is lent to answer
/// [`DeathQuery`]. A fresh copy is the state `UI_Init` leaves: the release timer zeroed
/// (`0x490131`), which reads 0, and nothing offered.
#[derive(Clone, Debug, PartialEq)]
pub struct DeathUiState {
    /// Seconds until the server force-releases, counted from death against the 6-minute
    /// `CORPSE_REPOP_TIME` the wire never carries. `None` (`PLAYER_FIELD_BYTES` bit `0x08` clear,
    /// an instanceable map) answers -1, which picks the no-timer text (`StaticPopup.lua:380-387`).
    pub release_remaining: Option<f32>,
    /// Seconds until the corpse is reclaimable, from `SMSG_CORPSE_RECLAIM_DELAY` at arrival; the
    /// `StartDelay` gate of `RECOVER_CORPSE` and `RESURRECT`.
    pub recovery_delay: f32,
    /// The pending resurrect offer warns of resurrection sickness (`ResurrectHasSickness`).
    pub resurrect_sickness: bool,
    /// The pending offer still honors the reclaim-delay gate (`ResurrectHasTimer`).
    pub resurrect_has_timer: bool,
    /// The confirming spirit healer is in dialog range (`CheckSpiritHealerDist`).
    pub spirit_healer_in_range: bool,
    /// The VM's copy of the sickness duration a spirit-healer res would apply ("N minutes");
    /// `None` below the sickness level picks `XP_LOSS_NO_SICKNESS` (`UIParent.lua:399-408`). The
    /// host answers the level instead ([`ResSicknessLevelQuery`]), which the getter words.
    pub sickness_duration: Option<String>,
    /// `HasSoulstone()`'s label (`0x48ac80`), stamped on the DEATH dialog's second button:
    /// `PLAYER_SELF_RES_SPELL`'s `Spell.dbc` name (3026 and 20758-20761 "Use Soulstone", 21169
    /// "Reincarnation", 23700 "Twisting Nether"), else a carried self-resurrect item's name.
    pub self_res_label: Option<String>,
}

impl Default for DeathUiState {
    fn default() -> Self {
        Self {
            release_remaining: Some(0.0),
            recovery_delay: 0.0,
            resurrect_sickness: false,
            resurrect_has_timer: false,
            spirit_healer_in_range: false,
            sickness_duration: None,
            self_res_label: None,
        }
    }
}

/// The death verbs the host applies inside their calls ([`super::ScriptCall::Death`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeathAction {
    /// `RepopMe()` (`0x48a970` → `0x5e0ae0`): `CMSG_REPOP_REQUEST` while dead or a ghost
    /// (`0x5e0ae6`).
    Repop,
    /// `RetrieveCorpse()` (`0x48d260`): `CMSG_RECLAIM_CORPSE` with the corpse guid, ungated.
    RetrieveCorpse,
    /// `AcceptResurrect()` (`0x48a9a0` → `0x5ded80(1)`): `CMSG_RESURRECT_RESPONSE`, accepting, while
    /// an offer is held; the offer is forgotten.
    AcceptResurrect,
    /// `DeclineResurrect()` (`0x48a9d0` → `0x5ded80(0)`), declining.
    DeclineResurrect,
    /// `AcceptXPLoss()` (`0x48d0c0` → `0x5df730(0)`): `CMSG_SPIRIT_HEALER_ACTIVATE` for a ghost
    /// whose remembered healer is in range.
    AcceptXpLoss,
    /// `AcceptAreaSpiritHeal()` (`0x48df20` → `0x492310`): `CMSG_AREA_SPIRIT_HEALER_QUEUE` with
    /// the cached healer, silent with none.
    AcceptAreaSpiritHeal,
    /// `UseSoulstone()` (`0x48ad70`): `CMSG_SELF_RES` when `PLAYER_SELF_RES_SPELL` is non-zero,
    /// else a use of the carried item, decided inside the call.
    UseSoulstone {
        /// What the protected-action gate's tag 2 allowed the call: the item leg is `CGItem::Use`
        /// (`0x48ae7d`), whose cast is `TryCastSpell`'s. The cancel before it (`0x48ae6b`) is not
        /// behind it, nor is the `CMSG_SELF_RES` leg.
        may_cast: bool,
    },
}

impl super::UiScript {
    /// Set the VM's copy of the death state, which the getters read with no host to ask.
    pub fn set_death(&mut self, state: DeathUiState) {
        self.model_mut().death = state;
    }

    /// The death calls a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_death_actions(&mut self) -> Vec<DeathAction> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Death(action) => Some(*action),
            _ => None,
        })
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    fn install_action(lua: &Lua, name: &str, action: DeathAction) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                super::calls::submit(lua, super::ScriptCall::Death(action))
            })?,
        )
    }
    install_action(lua, "RepopMe", DeathAction::Repop)?;
    install_action(lua, "RetrieveCorpse", DeathAction::RetrieveCorpse)?;
    install_action(lua, "AcceptResurrect", DeathAction::AcceptResurrect)?;
    install_action(lua, "DeclineResurrect", DeathAction::DeclineResurrect)?;
    install_action(lua, "AcceptXPLoss", DeathAction::AcceptXpLoss)?;
    g.set(
        "UseSoulstone",
        lua.create_function(|lua, ()| {
            let may_cast = super::protected::gate(lua, super::protected::Tag::CAST);
            super::calls::submit(
                lua,
                super::ScriptCall::Death(DeathAction::UseSoulstone { may_cast }),
            )
        })?,
    )?;

    // Whole seconds: the milliseconds left over 1000 by a signed, truncating divide
    // (`0x48b5ae`-`0x48b5bd`); no timer is -1000 ms, so -1.
    g.set(
        "GetReleaseTimeRemaining",
        lua.create_function(|lua, ()| {
            Ok(state(lua)
                .release_remaining
                .map_or(-1.0, |s| f64::from(s.max(0.0).trunc())))
        })?,
    )?;

    // Whole seconds: the milliseconds left over 1000 by an unsigned divide, a floor
    // (`0x48b5f0`-`0x48b5f7`).
    g.set(
        "GetCorpseRecoveryDelay",
        lua.create_function(|lua, ()| Ok(f64::from(state(lua).recovery_delay.max(0.0).floor())))?,
    )?;

    g.set(
        "ResurrectHasSickness",
        lua.create_function(|lua, ()| Ok(flag(state(lua).resurrect_sickness)))?,
    )?;
    g.set(
        "ResurrectHasTimer",
        lua.create_function(|lua, ()| Ok(flag(state(lua).resurrect_has_timer)))?,
    )?;
    g.set(
        "CheckSpiritHealerDist",
        lua.create_function(|lua, ()| Ok(flag(state(lua).spirit_healer_in_range)))?,
    )?;

    g.set(
        "GetResSicknessDuration",
        lua.create_function(|lua, ()| {
            let duration = match ask(lua, &ResSicknessLevelQuery) {
                Some(level) => sickness_duration(level, crate::strings::Strings::Vm(lua)),
                None => lua
                    .app_data_ref::<Model>()
                    .expect("model app_data")
                    .death
                    .sickness_duration
                    .clone(),
            };
            Ok(match duration {
                Some(s) => Value::String(lua.create_string(&s)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // Nil, never `0`, when there is none: the DEATH dialog shows its second button on any truthy
    // answer, stamps the answer on it and picks soulstone over release by it.
    g.set(
        "HasSoulstone",
        lua.create_function(|lua, ()| {
            Ok(match state(lua).self_res_label {
                Some(s) => Value::String(lua.create_string(&s)?),
                None => Value::Nil,
            })
        })?,
    )?;

    Ok(())
}
