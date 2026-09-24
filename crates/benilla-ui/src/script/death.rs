//! The release, reclaim, resurrect and self-res verbs and the getters the `DEATH`,
//! `RECOVER_CORPSE`, `RESURRECT` and `XP_LOSS` dialogs call, over a snapshot the app pushes.
//! The three predicates answer Lua booleans where the reference answers `1` or nil
//! (`ResurrectHasSickness` `0x48aa00`, `ResurrectHasTimer` `0x48aa30`, `CheckSpiritHealerDist`
//! `0x48d120`).

use mlua::{Lua, Value};

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

/// One drained death intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeathAction {
    /// `RepopMe()`: `CMSG_REPOP_REQUEST`.
    Repop,
    /// `RetrieveCorpse()`: `CMSG_RECLAIM_CORPSE`.
    RetrieveCorpse,
    /// `AcceptResurrect()`: `CMSG_RESURRECT_RESPONSE`, accepting.
    AcceptResurrect,
    /// `DeclineResurrect()`: `CMSG_RESURRECT_RESPONSE`, declining.
    DeclineResurrect,
    /// `AcceptXPLoss()`: `CMSG_SPIRIT_HEALER_ACTIVATE`.
    AcceptXpLoss,
    /// `UseSoulstone()`: `CMSG_SELF_RES` when `PLAYER_SELF_RES_SPELL` is non-zero, else a use of
    /// the carried item, decided at drain time as the reference decides it at call time.
    UseSoulstone,
}

impl super::UiScript {
    /// Push this frame's death snapshot, before the event dispatch.
    pub fn set_death(&mut self, state: DeathUiState) {
        self.model_mut().death = state;
    }

    /// Drain the queued death intents.
    pub fn take_death_actions(&mut self) -> Vec<DeathAction> {
        std::mem::take(&mut self.model_mut().death_actions)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    fn install_action(lua: &Lua, name: &str, action: DeathAction) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.death_actions.push(action);
                Ok(())
            })?,
        )
    }
    install_action(lua, "RepopMe", DeathAction::Repop)?;
    install_action(lua, "RetrieveCorpse", DeathAction::RetrieveCorpse)?;
    install_action(lua, "AcceptResurrect", DeathAction::AcceptResurrect)?;
    install_action(lua, "DeclineResurrect", DeathAction::DeclineResurrect)?;
    install_action(lua, "AcceptXPLoss", DeathAction::AcceptXpLoss)?;
    install_action(lua, "UseSoulstone", DeathAction::UseSoulstone)?;

    g.set(
        "GetReleaseTimeRemaining",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model
                .death
                .release_remaining
                .map_or(-1.0, |s| f64::from(s.max(0.0))))
        })?,
    )?;

    g.set(
        "GetCorpseRecoveryDelay",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.death.recovery_delay.max(0.0).ceil()))
        })?,
    )?;

    g.set(
        "ResurrectHasSickness",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.death.resurrect_sickness)
        })?,
    )?;
    g.set(
        "ResurrectHasTimer",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.death.resurrect_has_timer)
        })?,
    )?;
    g.set(
        "CheckSpiritHealerDist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.death.spirit_healer_in_range)
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
