//! The four summon globals the `CONFIRM_SUMMON` dialog calls (`StaticPopup.lua:1336-1357`); 1.12
//! has no `CancelSummon`, as declining sends nothing. The reference resolves the summoner
//! (`0xb4e358`), zone (`0xb4e354`) and deadline (`0xb4e350`) at call time; the app pushes the
//! answers each frame, so a name still in flight reads `""` and fills in on a later tick, and the
//! summoner getter's miss asks inside its call ([`super::RecordAsk::Summoner`]).

use mlua::Lua;

use super::Model;

/// The dialog's three reads, never nil: with nothing pending or resolved they are `""`, `""` and
/// 0, as the reference's getters answer (`0x882748` is its shared `""`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SummonConfirmUiState {
    /// The summoner's name from the name cache (`0x55f080`), `""` while the query is out.
    pub summoner: String,
    /// The summoner's zone name out of `AreaTable.dbc`, or `""` if the id names no row.
    pub area: String,
    /// Milliseconds left, 0 when nothing is pending or it ran out; the binding truncates to
    /// seconds, as `0x48b660` does.
    pub time_left_ms: u32,
}

/// The summon's verb the host applies inside its call ([`super::ScriptCall::Summon`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummonCall {
    /// `ConfirmSummon()` (`0x48b770`): `CMSG_SUMMON_RESPONSE` with the latched summoner
    /// (`0x48b7d0`).
    Confirm,
}

impl super::UiScript {
    /// Push the three resolved reads, each frame.
    pub fn set_summon_confirm(&mut self, state: SummonConfirmUiState) {
        let mut model = self.model_mut();
        if model.summon_confirm != state {
            model.summon_confirm = state;
        }
    }

    /// The `ConfirmSummon()` calls a harness's VM kept ([`Self::keep_calls`]).
    pub fn take_summon_confirms(&mut self) -> u32 {
        self.take_calls_like(&super::ScriptCall::Summon(SummonCall::Confirm)) as u32
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x48b6a0`: a string on every path.
    g.set(
        "GetSummonConfirmSummoner",
        lua.create_function(|lua, ()| {
            let summoner = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.summon_confirm.summoner.clone()
            };
            // A miss is the lookup's ask with the callback `0x48b710` (`0x48b6e1`), inside the
            // call.
            if summoner.is_empty() {
                super::calls::ask(lua, super::RecordAsk::Summoner)?;
            }
            lua.create_string(&summoner)
        })?,
    )?;

    // `0x48b720`: the summoner's zone straight from `AreaTable.dbc`, with no parent walk and no
    // GlobalString fallback, unlike the innkeeper question's chain (`0x5dfe5e`).
    g.set(
        "GetSummonConfirmAreaName",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.summon_confirm.area)
        })?,
    )?;

    // Whole seconds, truncated (`0x48b660`); the dialog reads it once in `OnShow` and counts down
    // locally from there.
    g.set(
        "GetSummonConfirmTimeLeft",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(f64::from(model.summon_confirm.time_left_ms / 1000))
        })?,
    )?;

    // The dialog's Accept, the only packet in the flow.
    g.set(
        "ConfirmSummon",
        lua.create_function(|lua, ()| {
            super::calls::submit(lua, super::ScriptCall::Summon(SummonCall::Confirm))
        })?,
    )?;

    Ok(())
}
