//! The session verbs: the game menu's Logout and Exit Game, their dialogs' answers, `ReloadUI`
//! and the cinematic pair. Each queues a [`SessionRequest`] the app turns into a packet or an
//! exit; the server decides whether a logout is instant or a 20-second countdown.

use mlua::Lua;

use super::Model;

/// A session intent queued by a Lua call, drained by the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    /// `Logout()`: `CMSG_LOGOUT_REQUEST` (`0x5ab000`), back to character select.
    Logout,
    /// `Quit()`: the same request, then the process ends once the logout completes.
    Quit,
    /// `CancelLogout()`: `CMSG_LOGOUT_CANCEL`; the server's ack fires `LOGOUT_CANCEL`.
    CancelLogout,
    /// `ForceQuit()`: end the process now, with no server round trip.
    ForceQuit,
    /// `ForceLogout()` (`0x48ab50`): an empty `CMSG_PLAYER_LOGOUT` (`0x4A`), bypassing the
    /// pending-logout latch, and nothing without a live in-world session (`0x5ab020`).
    ForceLogout,
    /// `ReloadUI()` (`0x4884d0`) only sets `0xb4b3f4`; the next frame's `0x495590` tears the VM
    /// down and rebuilds it, so it is never destroyed mid-call. The app's drain, outside any call,
    /// is the same guard.
    ReloadUi,
    /// `StopCinematic()` (`0x48b970`), the only skip: no native caller and no binding, so ESC
    /// reaches it only through `CinematicFrame`'s `OnKeyDown`; playback therefore waits for the
    /// in-game UI.
    StopCinematic,
}

impl super::UiScript {
    /// Drain the session intents queued since the last call.
    pub fn take_session_requests(&mut self) -> Vec<SessionRequest> {
        std::mem::take(&mut self.model_mut().session_requests)
    }

    /// Queue an intent from the app: `/logout`, `/camp`, `/quit` and `/reload`, which benilla
    /// parses in Rust rather than through `SlashCmdList`, take their Lua verbs' route this way.
    pub fn queue_session_request(&mut self, request: SessionRequest) {
        self.model_mut().session_requests.push(request);
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    for (name, request) in [
        ("Logout", SessionRequest::Logout),
        ("Quit", SessionRequest::Quit),
        ("CancelLogout", SessionRequest::CancelLogout),
        ("ForceQuit", SessionRequest::ForceQuit),
        ("ForceLogout", SessionRequest::ForceLogout),
        ("ReloadUI", SessionRequest::ReloadUi),
        ("StopCinematic", SessionRequest::StopCinematic),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.session_requests.push(request);
                Ok(())
            })?,
        )?;
    }

    // `0x48c930`: the number 1 while a cinematic plays, else nil, one result either way.
    g.set(
        "InCinematic",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if model.in_cinematic {
                mlua::Value::Integer(1)
            } else {
                mlua::Value::Nil
            })
        })?,
    )?;

    Ok(())
}
