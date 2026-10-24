//! The release, reclaim, resurrect and self-res verbs and the getters the `DEATH`,
//! `RECOVER_CORPSE`, `RESURRECT` and `XP_LOSS` dialogs call, over a snapshot the app pushes.
//! The three predicates answer `1` or nil, as the reference does (`ResurrectHasSickness`
//! `0x48aa00`, `ResurrectHasTimer` `0x48aa30`, `CheckSpiritHealerDist` `0x48d120`).

use mlua::{Lua, Value};

use super::binding_abi::flag;
use super::Model;

/// The death snapshot the app pushes each frame.
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// The sickness duration a spirit-healer res would apply ("N minutes"); `None` below the
    /// sickness level picks `XP_LOSS_NO_SICKNESS` (`UIParent.lua:399-408`).
    pub sickness_duration: Option<String>,
    /// `HasSoulstone()`'s label (`0x48ac80`), stamped on the DEATH dialog's second button:
    /// `PLAYER_SELF_RES_SPELL`'s `Spell.dbc` name (3026 and 20758-20761 "Use Soulstone", 21169
    /// "Reincarnation", 23700 "Twisting Nether"), else a carried self-resurrect item's name.
    pub self_res_label: Option<String>,
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
    /// Push this frame's death snapshot, before the event dispatch.
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
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model
                .death
                .release_remaining
                .map_or(-1.0, |s| f64::from(s.max(0.0).trunc())))
        })?,
    )?;

    // Whole seconds: the milliseconds left over 1000 by an unsigned divide, a floor
    // (`0x48b5f0`-`0x48b5f7`).
    g.set(
        "GetCorpseRecoveryDelay",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.death.recovery_delay.max(0.0).floor()))
        })?,
    )?;

    g.set(
        "ResurrectHasSickness",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.death.resurrect_sickness))
        })?,
    )?;
    g.set(
        "ResurrectHasTimer",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.death.resurrect_has_timer))
        })?,
    )?;
    g.set(
        "CheckSpiritHealerDist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.death.spirit_healer_in_range))
        })?,
    )?;

    g.set(
        "GetResSicknessDuration",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match &model.death.sickness_duration {
                Some(s) => Value::String(lua.create_string(s)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // Nil, never `0`, when there is none: the DEATH dialog shows its second button on any truthy
    // answer, stamps the answer on it and picks soulstone over release by it.
    g.set(
        "HasSoulstone",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match &model.death.self_res_label {
                Some(s) => Value::String(lua.create_string(s)?),
                None => Value::Nil,
            })
        })?,
    )?;

    Ok(())
}
