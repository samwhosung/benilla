//! The session verbs: the game menu's Logout and Exit Game, their dialogs' answers, `ReloadUI`
//! and the cinematic pair. Each queues a [`SessionRequest`] the app turns into a packet or an
//! exit; the server decides whether a logout is instant or a 20-second countdown. Beside them,
//! the world-enter cascade's two events and the `PLAYER_LOGIN` arm a UI load sets.

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

    /// Arm `PLAYER_LOGIN`, the UI load's last step: `UI_Init` sets the one-shot `[0xb4e260]` at
    /// `0x49011d`, unconditionally, and [`Self::fire_world_enter`] spends it. The flag lives in
    /// this VM, so each load arms its own and a worldport, which loads nothing, re-arms nothing.
    pub fn arm_player_login(&mut self) {
        self.model_mut().player_login_armed = true;
    }

    /// The world-enter cascade's events (`0x4908c0`): `PLAYER_LOGIN` (270, `0x490959`) only if a
    /// load armed it (read at `0x49094b`, cleared at `0x49095e`), then `PLAYER_ENTERING_WORLD`
    /// (272, `0x49096a`) every time. The cascade runs with the player object in the world, so its
    /// caller seats `"player"` first, and inside its own sound-suppression bracket (`0x4908d5` to
    /// `0x490a56`), so a handler's `PlaySound` is dropped.
    pub fn fire_world_enter(&mut self) {
        let armed = std::mem::take(&mut self.model_mut().player_login_armed);
        self.push_sound_suppression();
        if armed {
            self.fire_event("PLAYER_LOGIN", Vec::new());
        }
        self.fire_event("PLAYER_ENTERING_WORLD", Vec::new());
        self.pop_sound_suppression();
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
