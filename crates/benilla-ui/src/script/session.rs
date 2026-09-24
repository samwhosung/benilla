//! The **session API** (decisions 0674, 1291) — the four exit globals the game menu's Logout and
//! Exit Game buttons call, the two dialogs' answers to them, and `ReloadUI()`.
//!
//! The shape is [`super::duel`]'s: nothing to read, only the outbound half. Each call queues a
//! [`SessionRequest`] the app drains ([`super::UiScript::take_session_requests`]) and turns into a
//! packet or a process exit, so the engine keeps no reach into ECS/net (decision 0068 §3).
//!
//! What each one means, and where the reference puts it:
//!
//! - `Logout()` — ask to leave the world (`CMSG_LOGOUT_REQUEST`; the client's own
//!   `ClientServices::SendLogout 0x5ab000`). The **server** decides
//!   whether that is instant or a 20-second countdown, and says so in `SMSG_LOGOUT_RESPONSE`; the
//!   client's whole job is to narrate the answer, which is the CAMP dialog.
//! - `Quit()` — the same request, plus a standing intent to end the *process* rather than return to
//!   character select when the logout completes. That is why Exit Game shows the same countdown
//!   under a different name (QUIT_TIMER "%d %s until exit").
//! - `CancelLogout()` — call off either one (`CMSG_LOGOUT_CANCEL`); the server acks with
//!   `SMSG_LOGOUT_CANCEL_ACK`, which is what fires `LOGOUT_CANCEL` and takes the dialog down.
//! - `ForceQuit()` — the QUIT dialog's "Exit now": end the process immediately, without waiting out
//!   the server's clock. (Its logout sibling `ForceLogout()` is deliberately absent: the reference's
//!   own CAMP dialog has the force button commented out — "uncomment once forced logouts are
//!   completely implemented (they currently have a failure case)" — so 1.12 ships no way to call it
//!   from the UI, and neither do we.)

use mlua::Lua;

use super::Model;

/// Outbound session-exit intents queued by the Era API calls, drained by the app
/// ([`super::UiScript::take_session_requests`]). Plain data — [`super::DuelRequest`]'s twin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    /// `Logout()` — leave the world back to character select.
    Logout,
    /// `Quit()` — leave the world and end the process once it completes.
    Quit,
    /// `CancelLogout()` — call off a pending logout or quit.
    CancelLogout,
    /// `ForceQuit()` — end the process now, no server round trip.
    ForceQuit,
    /// `ForceLogout()` — the session dispatcher with `force = 1`: `CMSG_PLAYER_LOGOUT` (`0x4A`,
    /// empty) instead of `Logout`'s `0x4B`, the pending-logout latch bypassed, and nothing at all
    /// without a live in-world session (decision 1963; `ForceLogout 0x48ab50`'s `0x5ab020` gate).
    ForceLogout,
    /// `ReloadUI()` — tear this VM down and build a fresh one, without leaving the world.
    ///
    /// The reference's `ReloadUI 0x4884d0` reads no arguments and returns no values; it only sets
    /// `ds:0xb4b3f4`, and the per-frame callback `0x495590` runs the teardown/rebuild pair
    /// (`0x490bd0` → `0x48fbf0`) on the NEXT frame — a deferral that doubles as the reentrancy
    /// guard: the Lua state being destroyed is never the one mid-way through executing the call.
    /// Queuing an intent the app runs outside any VM call is that same guard in this engine's
    /// shape.
    ReloadUi,
    /// `StopCinematic()` — skip the cinematic that is playing.
    ///
    /// **The only skip path there is.** In the reference this binding (`0x48b970`) has *zero*
    /// native callers: nothing in the engine calls it, and it is absent from `Bindings.xml`, so
    /// ESC reaches it purely through `CinematicFrame`'s own `OnKeyDown` handler while that
    /// fullscreen frame holds keyboard focus. A cinematic with no `CinematicFrame` on screen is
    /// therefore un-skippable — which is why benilla starts playback only once the in-game UI
    /// exists.
    StopCinematic,
}

impl super::UiScript {
    /// Drain the session-exit intents queued since the last call.
    pub fn take_session_requests(&mut self) -> Vec<SessionRequest> {
        std::mem::take(&mut self.model_mut().session_requests)
    }

    /// Queue an intent from the app side — the `/logout` and `/camp` slash commands, which benilla
    /// parses in Rust rather than through `SlashCmdList` (the [`super::UiScript::queue_duel_request`]
    /// posture), so they enter the same queue and get the same dialog.
    pub fn queue_session_request(&mut self, request: SessionRequest) {
        self.model_mut().session_requests.push(request);
    }
}

/// Register the session globals — the four exit verbs, `ReloadUI`, and the cinematic skip.
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

    // `InCinematic()` — the reference's `0x48c930`: reads the "a cinematic is playing" global and
    // answers the **number 1** while one is, `nil` otherwise (both legs push exactly one result).
    // Era convention, not `true`/`false`.
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
