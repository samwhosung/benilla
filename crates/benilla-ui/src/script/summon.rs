//! The four summon globals the `CONFIRM_SUMMON` dialog calls (`StaticPopup.lua:1336-1357`); 1.12
//! has no `CancelSummon`, as declining sends nothing. The reference resolves the summoner
//! (`0xb4e358`), zone (`0xb4e354`) and deadline (`0xb4e350`) at call time; the app pushes the
//! answers each frame, so a name still in flight reads `""` and fills in on a later tick.

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

impl super::UiScript {
    /// Push the three resolved reads, each frame.
    pub fn set_summon_confirm(&mut self, state: SummonConfirmUiState) {
        let mut model = self.model_mut();
        if model.summon_confirm != state {
            model.summon_confirm = state;
        }
    }

    /// Drain the `ConfirmSummon()` calls, each a `CMSG_SUMMON_RESPONSE`.
    pub fn take_summon_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().summon_confirms)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x48b6a0`: a string on every path.
    g.set(
        "GetSummonConfirmSummoner",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.summon_confirm.summoner)
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
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.summon_confirms += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}
