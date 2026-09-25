//! **Leaving** — the app half of the game menu's Logout and Exit Game buttons:
//! the outbound request, the server's answer, the countdown dialog that narrates it, and the
//! process exit.
//!
//! The whole arc is three packets and one rule: **the server owns the clock, the client only
//! narrates it.** `Logout()` sends `CMSG_LOGOUT_REQUEST`; vmangos
//! (`WorldSession::HandleLogoutRequestOpcode`, read at MiscHandler.cpp:284) answers
//! `SMSG_LOGOUT_RESPONSE {u32 reason, u8 instant}` and that one packet decides everything:
//!
//! - `reason != 0` — **refused**, no logout starts (1 in combat, 3 jumping/falling, 2 GM-frozen).
//! - `reason == 0, instant == 1` — logged out on the spot (resting in an inn or a city, on a taxi,
//!   or a GM-level account — vmangos's `CONFIG_UINT32_INSTANT_LOGOUT`); `SMSG_LOGOUT_COMPLETE`
//!   follows immediately and there is no dialog at all.
//! - `reason == 0, instant == 0` — a **20-second server-side timer** now runs, during which the
//!   character sits and is rooted. This is the case the CAMP dialog counts down, and the case
//!   `CancelLogout()` (`CMSG_LOGOUT_CANCEL` → `SMSG_LOGOUT_CANCEL_ACK`) calls off.
//!
//! Quit is Logout plus one bit of local intent: end the *process* on completion instead of falling
//! back to character select. It is the same wire, the same clock, the same dialog under a different
//! name (QUIT_TIMER "%d %s until exit"), which is why the reference gives QUIT an "Exit now" button
//! and CAMP none — a force-quit needs no server round trip, a force-logout does.
//!
//! **All of it is now pinned** (this module's three arms were the last INFERRED
//! join left in the leaving path). The wire shape is verified twice over: vmangos's packet
//! (`WorldPackets::Misc::LogoutResponse` — `u32 reason, u8 instant`) and the reference's own
//! handler (`0x5b4630`: `GetInt32` then `GetUInt8`, then a virtual `[vtable+0x54](reason,
//! instant)`, which is `[0x80a398+0x54]` = `0x5aaef0`). That override is this decision table byte
//! for byte:
//!
//! - `0x5aaef6 test eax,eax / jne 0x5aaf21` → **any** non-zero reason takes one arm:
//!   `DisplayError(0x180)` = `ERR_LOGOUT_FAILED`, then clear the pending flag. There is no
//!   per-reason string — combat, falling and GM-frozen all show the one line.
//! - `0x5aaf00 test eax,eax / jne` → `instant != 0` returns having done nothing; the completion
//!   is already on its way.
//! - otherwise `setne cl` on `[this+0x1b1c]`, `add ecx,0x114`, `SignalEvent` → event **276
//!   `PLAYER_CAMPING`** or **277 `PLAYER_QUITING`**, and `[this+0x1b1c]` is written from the
//!   logout *request*'s own quit flag at `0x5ab053`. The reference remembers camp-vs-quit in one
//!   local byte, which is exactly what [`LogoutState::quitting`] is.
//!
//! The 1.12 FrameXML fixes the rest: the CAMP/QUIT dialogs, their 20 s timeouts, and their
//! `PLAYER_CAMPING` / `PLAYER_QUITING` / `LOGOUT_CANCEL` drivers (UIParent.lua l.304-315, event
//! ids 276/277/278 in the client's event-name table `0xbe1198`).

use benilla_ui::script::{SessionRequest, UiScript};
use bevy::prelude::*;

use crate::net::{ClientCommand, LoggedOutMessage, NetCommands, SelfGuid};
use crate::ui_script::{UiFeed, UiInput};

/// The reference's refusal line, as a **catalog key** rather than a literal — message id `0x180`,
/// the sole argument of the `DisplayError` at `0x5aaf26`. VERIFIED, superseding
/// this module's old inference: the reference picks this one string for every non-zero reason,
/// and `PLAYER_LOGOUT_FAILED_ERROR` — the other candidate the guess weighed — is never reached
/// from here. The text comes from the VM's own `GlobalStrings.lua`, so a locale rides for free.
const LOGOUT_FAILED_KEY: &str = "ERR_LOGOUT_FAILED";

/// What the wire told us to say next — drained by [`feed_logout`] into script events. A queue
/// rather than a flag: a cancel can land in the same frame as the response it cancels, and the UI
/// must see both, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogoutSignal {
    /// A non-instant logout started — `PLAYER_CAMPING` (the CAMP dialog).
    Camping,
    /// The same, for a pending Quit — `PLAYER_QUITING` (the QUIT dialog).
    Quiting,
    /// The server dropped the pending logout — `LOGOUT_CANCEL` (both dialogs hide).
    Cancelled,
    /// The server refused outright — a red error line, no dialog.
    Refused,
}

/// The pending session-exit, and the signals owed to the UI.
///
/// `quitting` is the piece of local state the arc's *meaning* needs: the wire cannot tell a logout
/// from a quit (both are `CMSG_LOGOUT_REQUEST`), so which dialog to show and whether to exit the
/// process at the end is remembered here. It is cleared by every terminal edge — a refusal, a
/// cancel, or the exit itself — so a cancelled quit can never leave a client that dies on the next
/// unrelated logout.
///
/// `pending` is the reference's own **`[session+0x1b1d]`**, which this module knew
/// about in prose — `apply_cancelled`'s doc has always called it "the same pending byte the
/// refusal arm clears" — and never modelled. `0x5ab000` bails on it *silently* when `force == 0`,
/// so a second request while one is in flight is not a second packet. That gate was invisible
/// while the only caller was a button a human presses once; the idle handler (2092) asks every
/// frame past thirty minutes, and without the latch it would put a `CMSG_LOGOUT_REQUEST` on the
/// wire sixty times a second until something else stamped the idle clock.
#[derive(Resource, Default)]
pub(crate) struct LogoutState {
    quitting: bool,
    /// `[session+0x1b1d]` — a logout request is out and unanswered. Set by the non-forced path,
    /// bypassed *and not set* by `ForceLogout` (`0x5aaff0`), cleared by every
    /// terminal edge.
    pending: bool,
    signals: Vec<LogoutSignal>,
}

impl LogoutState {
    /// `SMSG_LOGOUT_RESPONSE` — the whole decision table (module docs). Returns nothing; the
    /// instant case is silent on purpose (the completion is already on its way).
    pub(crate) fn apply_response(&mut self, reason: u32, instant: bool) {
        // Announced: this one packet decides the entire user-visible outcome (dialog, no dialog, or
        // a red line), and the three inputs behind it — combat, resting, account security — are all
        // invisible from this side. A logout that "did nothing" is diagnosed from this line.
        info!("logout: server answered reason={reason} instant={instant}");
        if reason != 0 {
            self.quitting = false;
            // `0x5aaf21`'s arm: `DisplayError(ERR_LOGOUT_FAILED)` **and clear the pending flag**,
            // which is what lets the player try again once they leave combat.
            self.pending = false;
            self.signals.push(LogoutSignal::Refused);
            return;
        }
        if instant {
            return;
        }
        self.signals.push(if self.quitting {
            LogoutSignal::Quiting
        } else {
            LogoutSignal::Camping
        });
    }

    /// `SMSG_LOGOUT_CANCEL_ACK` — the countdown is off. Its handler is the vtable slot next door
    /// (`0x5b4680 call [eax+0x58]` → `0x5aaf60`), and all it does is fire event 278
    /// `LOGOUT_CANCEL` off the same pending byte the refusal arm clears.
    pub(crate) fn apply_cancelled(&mut self) {
        self.quitting = false;
        self.pending = false;
        self.signals.push(LogoutSignal::Cancelled);
    }
}

/// Fire the camp/quit events the dialogs are driven by (GameMenuFrame.xml's `BenillaLogoutDriver`).
///
/// The refusal is shown one at a time through the message sink rather than collected, because the
/// signals are ordered — a cancel can land in the same frame as the response it cancels, and the
/// dialogs must see both in the order the wire delivered them.
fn feed_logout(
    script: Option<NonSendMut<UiScript>>,
    mut logout: ResMut<LogoutState>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    for signal in std::mem::take(&mut logout.signals) {
        match signal {
            LogoutSignal::Camping => script.fire_event("PLAYER_CAMPING", Vec::new()),
            LogoutSignal::Quiting => script.fire_event("PLAYER_QUITING", Vec::new()),
            LogoutSignal::Cancelled => script.fire_event("LOGOUT_CANCEL", Vec::new()),
            LogoutSignal::Refused => {
                let line = crate::ui_action::keyed_line(&script, LOGOUT_FAILED_KEY);
                crate::ui_action::show_messages(&mut script, &mut sink, "ui_logout", line);
            }
        }
    }
}

/// Turn the Lua intents into packets — and, for the two force paths, straight into an exit.
fn drain_logout(
    script: Option<NonSendMut<UiScript>>,
    mut logout: ResMut<LogoutState>,
    self_guid: Res<SelfGuid>,
    commands: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
    mut reload: ResMut<crate::ui_script::ReloadUiPending>,
    mut cinematic: ResMut<crate::cinematic::Cinematic>,
) {
    let Some(mut script) = script else {
        return;
    };
    for request in script.take_session_requests() {
        match request {
            SessionRequest::Logout | SessionRequest::Quit => {
                // `0x5ab000`'s pending-logout bail, and it is **silent** — no error line, no
                // event, nothing on the wire. It sits ABOVE `0x5ab053`'s store of the quit byte,
                // so a refused request does not even get to change camp-into-quit.
                if logout.pending {
                    continue;
                }
                logout.quitting = request == SessionRequest::Quit;
                // Not in the world (or the socket is gone): there is nothing to log out OF, so a
                // quit is just an exit and a logout is a no-op. The reference does the same — its
                // Quit() at the glue screen closes the process outright.
                if self_guid.0.is_none() || commands.0.send(ClientCommand::Logout).is_err() {
                    if logout.quitting {
                        info!("logout: quit with no world session — exiting");
                        exit.write(AppExit::Success);
                    } else {
                        warn!("logout: no world session — logout dropped");
                        logout.quitting = false;
                    }
                } else {
                    logout.pending = true;
                    // Announced because what happens next is entirely the server's call — instant
                    // or a 20 s countdown — and a silent request looks identical to a dropped one.
                    info!(
                        "logout: requested (quitting={}) — awaiting SMSG_LOGOUT_RESPONSE",
                        logout.quitting
                    );
                }
            }
            SessionRequest::CancelLogout => {
                // The dialog's own OnHide fires this on EVERY early close, including one caused by
                // a cancel that already landed — so a stray send with nothing pending is normal and
                // harmless (vmangos ignores it). `quitting` clears here rather than on the ack so a
                // cancel with a dead socket still can't leave a quit armed.
                logout.quitting = false;
                logout.pending = false;
                let _ = commands.0.send(ClientCommand::LogoutCancel);
            }
            SessionRequest::ForceQuit => {
                info!("logout: force quit");
                exit.write(AppExit::Success);
            }
            // `CMSG_PLAYER_LOGOUT`, the forced flavour: the dispatcher's own gate is a live
            // in-world session, and nothing happens without one.
            // `0x5aaff0` calls the dispatcher `0x5ab000` with `force = 1`, which BYPASSES the
            // pending bail and does NOT set the latch — so a forced logout is exactly the escape
            // hatch from a stuck pending one.
            SessionRequest::ForceLogout => {
                if self_guid.0.is_some() {
                    info!("logout: forced");
                    let _ = commands.0.send(ClientCommand::ForceLogout);
                }
            }
            // Not this module's business beyond routing: the rebuild itself is
            // [`crate::ui_script::run_pending_reload`]'s, at the top of the next frame.
            SessionRequest::ReloadUi => reload.0 = true,
            // ESC out of a cinematic. It rides this queue rather than the binding table because
            // that is where the reference puts it: `StopCinematic` has no native callers and no
            // `Bindings.xml` row — `CinematicFrame`'s own `OnKeyDown` is the whole skip path.
            SessionRequest::StopCinematic => {
                crate::cinematic::stop(&mut cinematic, Some(&commands));
            }
        }
    }
}

/// The completion edge: `SMSG_LOGOUT_COMPLETE` normally lands us at character select (decision
/// 0193), but a logout that began as Exit Game ends the process instead.
fn exit_on_logout_complete(
    mut logged_out: MessageReader<LoggedOutMessage>,
    mut logout: ResMut<LogoutState>,
    mut exit: MessageWriter<AppExit>,
) {
    if logged_out.read().count() == 0 {
        return;
    }
    // The request is answered — and on a logout that lands us at character select this is the edge
    // that must disarm it, or the next character's first Logout would be swallowed.
    logout.pending = false;
    if logout.quitting {
        logout.quitting = false;
        info!("logout: world session ended — exiting");
        exit.write(AppExit::Success);
    }
}

pub struct UiLogoutPlugin;

impl Plugin for UiLogoutPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<LogoutState>().add_systems(
            Update,
            (
                feed_logout.in_set(UiFeed),
                drain_logout.after(UiInput),
                exit_on_logout_complete.after(UiInput),
            ),
        );
    }
}

/// The logout arc's two narration packets (in the net handler table since 2326)
/// — this module owns the decision table; the handler is only the hand-off.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::LogoutState;
    use crate::net::NetHandlerApp;

    /// Register the pair — called from [`super::UiLogoutPlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::LogoutResponse, on_narration)
            .net_handler(K::LogoutCancelled, on_narration);
    }

    fn on_narration(In(ev): In<SessionEvent>, mut logout: ResMut<LogoutState>) {
        match ev {
            SessionEvent::LogoutResponse { reason, instant } => {
                logout.apply_response(reason, instant)
            }
            SessionEvent::LogoutCancelled => logout.apply_cancelled(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// The response table (module docs): refusal speaks and disarms, instant is silent, and the
    /// slow path picks its dialog from the pending intent.
    #[test]
    fn the_response_decides_which_dialog_if_any() {
        let mut s = LogoutState::default();
        s.apply_response(0, true);
        assert!(s.signals.is_empty(), "an instant logout shows no dialog");

        s.apply_response(0, false);
        assert_eq!(s.signals, vec![LogoutSignal::Camping]);

        let mut s = LogoutState {
            quitting: true,
            pending: true,
            signals: Vec::new(),
        };
        s.apply_response(0, false);
        assert_eq!(s.signals, vec![LogoutSignal::Quiting], "a quit says so");

        // In combat: the red line, and the pending quit is disarmed — otherwise the NEXT logout,
        // minutes later and deliberate, would silently kill the process instead.
        let mut s = LogoutState {
            quitting: true,
            pending: true,
            signals: Vec::new(),
        };
        s.apply_response(1, false);
        assert_eq!(s.signals, vec![LogoutSignal::Refused]);
        assert!(!s.quitting);
        assert!(
            !s.pending,
            "the refusal arm clears the pending byte — otherwise a logout refused in combat \
             would never be retryable"
        );
    }

    /// **The pending latch, both directions** — `0x5ab000` bails silently on
    /// `[session+0x1b1d]`, and every terminal edge clears it. Driven through the real
    /// [`drain_logout`] rather than the state alone, because the bail lives in the drain and the
    /// thing under test is that the second ask puts nothing on the wire.
    #[test]
    fn a_second_logout_while_one_is_pending_is_silent() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<LogoutState>()
            .init_resource::<crate::ui_script::ReloadUiPending>()
            .init_resource::<crate::cinematic::Cinematic>()
            .add_message::<AppExit>()
            .add_message::<LoggedOutMessage>()
            .insert_resource(SelfGuid(Some(0x4000_0000_0000_0009)))
            .insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());

        let ask = |app: &mut App, request| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .queue_session_request(request);
            app.world_mut()
                .run_system_once(drain_logout)
                .expect("drain");
        };
        let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| rx.try_iter().count();

        ask(&mut app, SessionRequest::Logout);
        assert_eq!(sent(&rx), 1, "the first ask goes out");
        assert!(app.world().resource::<LogoutState>().pending);

        // The idle handler's shape: ask again, and again, while the server has not answered.
        for _ in 0..60 {
            ask(&mut app, SessionRequest::Logout);
        }
        assert_eq!(sent(&rx), 0, "and not one more packet, nor any error line");

        // `ForceLogout` bypasses the bail and does not set the latch (`0x5aaff0`, force = 1).
        ask(&mut app, SessionRequest::ForceLogout);
        assert_eq!(sent(&rx), 1, "the forced flavour is the escape hatch");

        // A cancel disarms, and the next ask is heard again.
        ask(&mut app, SessionRequest::CancelLogout);
        assert_eq!(sent(&rx), 1, "CMSG_LOGOUT_CANCEL");
        assert!(!app.world().resource::<LogoutState>().pending);
        ask(&mut app, SessionRequest::Logout);
        assert_eq!(sent(&rx), 1);

        // …and so does the completion, so the next character's first Logout is not swallowed.
        app.world_mut().resource_mut::<LogoutState>().pending = true;
        app.world_mut().write_message(LoggedOutMessage);
        app.world_mut()
            .run_system_once(exit_on_logout_complete)
            .expect("complete");
        assert!(!app.world().resource::<LogoutState>().pending);
    }

    /// The cancel ack disarms a quit too — the same "no armed quit survives a terminal edge" rule.
    #[test]
    fn a_cancel_disarms_a_pending_quit() {
        let mut s = LogoutState {
            quitting: true,
            pending: true,
            signals: Vec::new(),
        };
        s.apply_cancelled();
        assert_eq!(s.signals, vec![LogoutSignal::Cancelled]);
        assert!(!s.quitting);
    }
}
