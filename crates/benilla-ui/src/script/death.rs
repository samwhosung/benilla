//! The death-arc Lua surface: the release/reclaim/resurrect/self-res verbs
//! and the state getters the reference's DEATH / RECOVER_CORPSE / RESURRECT* / XP_LOSS dialogs
//! call.
//!
//! Engine-free seam like [`super::unit`]: the app pushes a [`DeathUiState`] snapshot per frame
//! (the countdowns + offer bits it computes from the wire) and drains the queued
//! [`DeathAction`]s into `ClientCommand`s. The predicates return Lua booleans rather than the
//! client's `1`/nil — truthy either way, the branch shape callers use is identical (the
//! [`super::unit`] convention).

use mlua::{Lua, Value};

use super::Model;

/// The death snapshot the app pushes each frame ([`super::UiScript::set_death`]). Plain data —
/// no mlua handles, no ECS types.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeathUiState {
    /// Seconds until the server force-releases — the client-side mirror of the 6:00
    /// `CORPSE_REPOP_TIME` the wire never carries, counted from the death
    /// edge. `None` = no release timer (`PLAYER_FIELD_BYTES` bit 0x08 clear — an instanceable
    /// map): `GetReleaseTimeRemaining` returns **−1** and the DEATH dialog shows the no-timer
    /// text.
    pub release_remaining: Option<f32>,
    /// Seconds until the corpse becomes reclaimable (`SMSG_CORPSE_RECLAIM_DELAY` anchored at
    /// arrival; `0` = reclaimable now). `GetCorpseRecoveryDelay`'s value — the RECOVER_CORPSE /
    /// RESURRECT StartDelay gate.
    pub recovery_delay: f32,
    /// The pending resurrect offer warns of resurrection sickness (`ResurrectHasSickness`).
    pub resurrect_sickness: bool,
    /// The pending offer still honors the reclaim-delay gate (`ResurrectHasTimer`).
    pub resurrect_has_timer: bool,
    /// The confirm-owning spirit healer is within dialog range (`CheckSpiritHealerDist` — the
    /// XP_LOSS dialogs' OnUpdate auto-hide).
    pub spirit_healer_in_range: bool,
    /// The "N minutes"/"N seconds" sickness-duration string a spirit-healer res would apply, or
    /// `None` below the sickness level (`GetResSicknessDuration` → nil — picks XP_LOSS vs
    /// XP_LOSS_NO_SICKNESS, ref UIParent.lua's CONFIRM_XP_LOSS arm).
    pub sickness_duration: Option<String>,
    /// What `HasSoulstone()` answers: the **label** of the self-resurrect available right now, or
    /// `None` for nil (`HasSoulstone 0x48ac80`).
    ///
    /// A string, not an id, because that is the whole of what the API returns: the DEATH dialog
    /// stamps it straight onto its second button (`Button2:SetText(HasSoulstone())`) and uses the
    /// same call as `DisplayButton2`. The app resolves it, because the script VM has no
    /// spell-catalog or item-cache binding (the `ui_cast`/`ui_mirror` idiom).
    ///
    /// Two sources, and the client's own fork picks between them
    /// ([`crate::script::death`]'s app-side resolver): `PLAYER_SELF_RES_SPELL` named through
    /// `Spell.dbc` (3026/20758-20761 are all literally **"Use Soulstone"**, 21169
    /// **"Reincarnation"**, 23700 **"Twisting Nether"** — Blizzard named those effect spells *as
    /// button labels*), else a carried item whose on-use spell self-resurrects, named by the
    /// **item**.
    pub self_res_label: Option<String>,
}

/// One drained death intent (the app's [`super::UiScript::take_death_actions`] maps each to its
/// `ClientCommand`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeathAction {
    /// `RepopMe()` — release the spirit (`CMSG_REPOP_REQUEST`).
    Repop,
    /// `RetrieveCorpse()` — reclaim the corpse (`CMSG_RECLAIM_CORPSE`).
    RetrieveCorpse,
    /// `AcceptResurrect()` — accept the pending offer (`CMSG_RESURRECT_RESPONSE` accept).
    AcceptResurrect,
    /// `DeclineResurrect()` — decline it (`CMSG_RESURRECT_RESPONSE` decline).
    DeclineResurrect,
    /// `AcceptXPLoss()` — take the spirit healer's res (`CMSG_SPIRIT_HEALER_ACTIVATE`).
    AcceptXpLoss,
    /// `UseSoulstone()` — spend the self-resurrect. **Which wire that is, is the app's to decide
    /// at drain time**, exactly as the binding decides it at call time: a non-zero
    /// `PLAYER_SELF_RES_SPELL` sends `CMSG_SELF_RES`, and a zero one falls through to using the
    /// carried item instead.
    UseSoulstone,
}

impl super::UiScript {
    /// Push this frame's death snapshot (the app's per-frame feed, before the event dispatch).
    pub fn set_death(&mut self, state: DeathUiState) {
        self.model_mut().death = state;
    }

    /// Drain the queued death intents (the app's per-frame drain).
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

    // GetReleaseTimeRemaining() → seconds until the forced release, or −1 = no timer (the DEATH
    // dialog's no-timer text pick, ref StaticPopup.lua:380-387).
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

    // GetCorpseRecoveryDelay() → whole seconds until reclaimable (0 = now) — the StartDelay gate.
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

    // GetResSicknessDuration() → the duration string, or nil below the sickness level (the
    // CONFIRM_XP_LOSS variant pick, ref UIParent.lua:399-408).
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

    // HasSoulstone() → the self-resurrect's label, or nil. The DEATH dialog
    // uses the one call three ways — `DisplayButton2` (show the button at all), `OnShow`
    // (`Button2:SetText(text)`) and `OnCancel`'s clicked arm (soulstone vs release) — so the
    // falsey return has to be **nil** and not `0`: Lua's `0` is truthy, and a `0` here would both
    // show the button forever and stamp "0" on it. The whole answer is computed app-side; this is
    // a pure read of the pushed snapshot.
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
