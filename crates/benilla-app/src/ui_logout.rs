//! Leaving: the game menu's Logout and Exit Game, from the request to the countdown dialog and the
//! process exit. The server owns the clock: vmangos answers `CMSG_LOGOUT_REQUEST` with
//! `SMSG_LOGOUT_RESPONSE { u32 reason, u8 instant }` (`MiscHandler.cpp:284`):
//!
//! - `reason != 0`: refused (1 in combat, 3 jumping or falling, 2 GM-frozen).
//! - `instant == 1`: out at once (resting, on a taxi, or a GM-level account), with no dialog.
//! - otherwise a 20 s server timer runs (`WorldSession.h:284`), which the CAMP dialog counts down
//!   and `CancelLogout()` calls off.
//!
//! The reference's handler (`0x5b4630`, then `0x5aaef0`) shows `ERR_LOGOUT_FAILED` for every
//! refusal, does nothing for an instant logout, as the completion follows, and otherwise fires
//! `PLAYER_CAMPING` or `PLAYER_QUITING` from the request's quit byte (stored at `0x5ab053`), which
//! the stock dialogs hang off (`UIParent.lua:304-315`). Quit is a logout that exits the process on
//! completion; the QUIT dialog has an "Exit now" button and CAMP has none.

use benilla_ui::script::{SessionRequest, UiScript};
use bevy::prelude::*;

use crate::net::{ClientCommand, LoggedOutMessage, NetCommands, SelfGuid};
use crate::ui_script::{UiFeed, UiInput};

/// The refusal line for every non-zero reason, never `PLAYER_LOGOUT_FAILED_ERROR`: message `0x180`
/// of the `DisplayError` at `0x5aaf26`, a key so the player's `GlobalStrings.lua` supplies it.
const LOGOUT_FAILED_KEY: &str = "ERR_LOGOUT_FAILED";

/// What the wire told us to say next, queued because a cancel can land in its response's frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogoutSignal {
    /// A timed logout started: `PLAYER_CAMPING`.
    Camping,
    /// The same for a quit: `PLAYER_QUITING`.
    Quiting,
    /// The logout was cancelled: `LOGOUT_CANCEL`.
    Cancelled,
    /// Refused: an error line, no dialog.
    Refused,
}

/// The pending session exit and the signals owed to the UI. `quitting` is the one bit the wire
/// lacks, as a quit is also `CMSG_LOGOUT_REQUEST`; every terminal edge clears it, so a cancelled
/// quit cannot exit the process on a later logout.
#[derive(Resource, Default)]
pub(crate) struct LogoutState {
    quitting: bool,
    /// The reference's `[session+0x1b1d]`: a request is out and unanswered. `0x5ab000` drops a
    /// second request silently while it is set, which the idle logout, asking every frame, relies
    /// on; `ForceLogout` (`0x5aaff0`) bypasses it and does not set it.
    pending: bool,
    signals: Vec<LogoutSignal>,
}

impl LogoutState {
    /// `SMSG_LOGOUT_RESPONSE`, the module doc's table; an instant logout signals nothing.
    pub(crate) fn apply_response(&mut self, reason: u32, instant: bool) {
        // Logged: the server's inputs (combat, resting, account level) are invisible from here.
        info!("logout: server answered reason={reason} instant={instant}");
        if reason != 0 {
            self.quitting = false;
            // `0x5aaf21`: the error line, and the pending flag clears so the player can retry.
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

    /// `SMSG_LOGOUT_CANCEL_ACK`: fires `LOGOUT_CANCEL` on every ack. The reference (`0x5b4680`,
    /// then `0x5aaf60`) fires it only while the pending byte is set (`0x5aaf6b`), and its own
    /// `CancelLogout` clears that byte when it sends (`0x5aaf80`).
    pub(crate) fn apply_cancelled(&mut self) {
        self.quitting = false;
        self.pending = false;
        self.signals.push(LogoutSignal::Cancelled);
    }
}

/// Fire the signals in wire order: the events the stock CAMP and QUIT dialogs hang off, and the
/// refusal line.
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

/// Turn the Lua intents into packets; a forced quit, or a quit with no world session, exits.
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
                // `0x5ab000` bails silently while a request is pending, before the quit byte is
                // stored (`0x5ab053`), so a second ask cannot turn a camp into a quit.
                if logout.pending {
                    continue;
                }
                logout.quitting = request == SessionRequest::Quit;
                // No world session: a quit just exits, as the reference's glue-screen `Quit()`
                // does, and a logout does nothing.
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
                    // Logged: instant or timed is the server's call.
                    info!(
                        "logout: requested (quitting={}) — awaiting SMSG_LOGOUT_RESPONSE",
                        logout.quitting
                    );
                }
            }
            SessionRequest::CancelLogout => {
                // The stock dialogs' `OnHide` sends this on every early close, even after a cancel
                // landed, so a stray cancel is normal. `quitting` clears here, not on the ack, so a
                // dead socket cannot leave a quit armed.
                logout.quitting = false;
                logout.pending = false;
                let _ = commands.0.send(ClientCommand::LogoutCancel);
            }
            SessionRequest::ForceQuit => {
                info!("logout: force quit");
                exit.write(AppExit::Success);
            }
            // `CMSG_PLAYER_LOGOUT`, only with a world session: `0x5aaff0` calls `0x5ab000` with
            // force 1, which skips the pending bail and does not set the latch.
            SessionRequest::ForceLogout => {
                if self_guid.0.is_some() {
                    info!("logout: forced");
                    let _ = commands.0.send(ClientCommand::ForceLogout);
                }
            }
            // The rebuild is `ui_script::run_pending_reload`'s, at the top of the next frame.
            SessionRequest::ReloadUi => reload.0 = true,
            // Esc out of a cinematic: the reference's `StopCinematic` has no binding and no native
            // caller, as `CinematicFrame.xml:37-44`'s `OnKeyDown` is the whole skip path.
            SessionRequest::StopCinematic => {
                crate::cinematic::stop(&mut cinematic, Some(&commands));
            }
        }
    }
}

/// `SMSG_LOGOUT_COMPLETE` returns to character select, or ends the process for Exit Game.
fn exit_on_logout_complete(
    mut logged_out: MessageReader<LoggedOutMessage>,
    mut logout: ResMut<LogoutState>,
    mut exit: MessageWriter<AppExit>,
) {
    if logged_out.read().count() == 0 {
        return;
    }
    // Answered: disarm, or the next character's first logout would be swallowed.
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

/// The logout response and cancel-ack handlers; the decision table is `LogoutState`'s.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::LogoutState;
    use crate::net::NetHandlerApp;

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

        // In combat: the error line, and the quit is disarmed so a later logout does not exit.
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

    /// Driven through [`drain_logout`], where the `0x5ab000` bail lives.
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

        // The idle logout's shape: asking every frame while unanswered.
        for _ in 0..60 {
            ask(&mut app, SessionRequest::Logout);
        }
        assert_eq!(sent(&rx), 0, "and not one more packet, nor any error line");

        // `ForceLogout` bypasses the bail and does not set the latch (`0x5aaff0`, force 1).
        ask(&mut app, SessionRequest::ForceLogout);
        assert_eq!(sent(&rx), 1, "the forced flavour is the escape hatch");

        // A cancel disarms, and the next ask is heard again.
        ask(&mut app, SessionRequest::CancelLogout);
        assert_eq!(sent(&rx), 1, "CMSG_LOGOUT_CANCEL");
        assert!(!app.world().resource::<LogoutState>().pending);
        ask(&mut app, SessionRequest::Logout);
        assert_eq!(sent(&rx), 1);

        // So does the completion.
        app.world_mut().resource_mut::<LogoutState>().pending = true;
        app.world_mut().write_message(LoggedOutMessage);
        app.world_mut()
            .run_system_once(exit_on_logout_complete)
            .expect("complete");
        assert!(!app.world().resource::<LogoutState>().pending);
    }

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
