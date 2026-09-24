//! The four duel globals, registered together at `0x849fa8`. What the UI reads arrives as event
//! arguments, so each call only queues a [`DuelRequest`] for the app to send. `AcceptDuel`
//! (`0x4d4ce0`) and `CancelDuel` (`0x4d4cf0`) call `0x4d4830` and `0x4d48b0`; `StartDuel`
//! (`0x4d4c40`) and `StartDuelUnit` (`0x4d4c90`) send no duel opcode but cast the duel spell at
//! the guid they resolve (`0x4d4810`).

use mlua::Lua;

use super::Model;

/// A duel intent queued by the globals, drained by `UiScript::take_duel_requests`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DuelRequest {
    /// `AcceptDuel()`, the `DUEL_REQUESTED` popup's Accept: `CMSG_DUEL_ACCEPTED`.
    Accept,
    /// `CancelDuel()`: `CMSG_DUEL_CANCELLED`, which the server reads by duel state as a decline,
    /// a cancel or a forfeit.
    Cancel,
    /// `StartDuel(name)`: the reference resolves the name through `0x493aa0`, players only, and
    /// does nothing on a miss.
    StartByName(String),
    /// `StartDuelUnit(unit)`, the unit popup's Duel row. The app casts only at a player; the
    /// reference resolves the token (`0x515970`) with no player gate of its own.
    StartByUnit(String),
}

impl super::UiScript {
    /// Drain the duel intents queued since the last call.
    pub fn take_duel_requests(&mut self) -> Vec<DuelRequest> {
        std::mem::take(&mut self.model_mut().duel_requests)
    }

    /// Queue an intent from the app's Rust slash parser: `/duel` and `/forfeit`, which the
    /// reference's `SlashCmdList` handlers send through `StartDuel` and `CancelDuel`.
    pub fn queue_duel_request(&mut self, request: DuelRequest) {
        self.model_mut().duel_requests.push(request);
    }
}

/// Register the duel globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "AcceptDuel",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.duel_requests.push(DuelRequest::Accept);
            Ok(())
        })?,
    )?;

    // `CancelDuel()`: the popup's Decline, and `/forfeit` (`/concede`, `/yield`) once under way.
    g.set(
        "CancelDuel",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.duel_requests.push(DuelRequest::Cancel);
            Ok(())
        })?,
    )?;

    g.set(
        "StartDuel",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.duel_requests.push(DuelRequest::StartByName(name));
            Ok(())
        })?,
    )?;

    g.set(
        "StartDuelUnit",
        lua.create_function(|lua, unit: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.duel_requests.push(DuelRequest::StartByUnit(unit));
            Ok(())
        })?,
    )?;

    Ok(())
}
