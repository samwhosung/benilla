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

use std::sync::atomic::{AtomicBool, Ordering};

use benilla_ui::script::SessionRequest;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::net::{ClientCommand, LoggedOutMessage, NetCommands, SelfGuid};
use crate::ui_script::UiFeed;

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
    /// second request silently while it is set, and the idle handler skips its thirty-minute leg
    /// on it (`0x482ef5`); `ForceLogout` (`0x5aaff0`) bypasses it and does not set it.
    pending: bool,
    signals: Vec<LogoutSignal>,
}

impl LogoutState {
    /// The pending byte, as `0x5ab0d0` returns it.
    pub(crate) fn pending(&self) -> bool {
        self.pending
    }

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
    mut vm: crate::script_calls::VmWith<ResMut<'static, LogoutState>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let signals = std::mem::take(&mut vm.own().signals);
    if signals.is_empty() {
        return;
    }
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        for signal in signals {
            match signal {
                LogoutSignal::Camping => script.fire_event("PLAYER_CAMPING", Vec::new()),
                LogoutSignal::Quiting => script.fire_event("PLAYER_QUITING", Vec::new()),
                LogoutSignal::Cancelled => script.fire_event("LOGOUT_CANCEL", Vec::new()),
                LogoutSignal::Refused => {
                    let line = crate::ui_action::keyed_line(script, LOGOUT_FAILED_KEY);
                    crate::ui_action::show_messages(script, &mut sink, "ui_logout", line);
                }
            }
        }
    })
}

/// What a session verb posts past its call, which nothing the script reads waits on: the process's
/// exit, as `ForceQuit` posts the context's close (`0x48ab60` → `0x401560`), which
/// [`exit_on_logout_complete`] takes, and the cinematic's stop, which schedules the end's fade
/// (`0x48b970` → `0x48f050`), which the cinematic's own chain takes at its head.
#[derive(Resource, Default)]
pub(crate) struct SessionPosts {
    exit: AtomicBool,
    stop_cinematic: AtomicBool,
}

impl SessionPosts {
    /// Whether a `StopCinematic()` was posted since the last take.
    pub(crate) fn take_stop_cinematic(&self) -> bool {
        self.stop_cinematic.swap(false, Ordering::Relaxed)
    }
}

/// The session verbs' engine half ([`crate::script_calls::ScriptCallHost`]): each sends its
/// packet and sets its latch before the verb returns, as the reference's bindings call into
/// `ClientConnection` inside the call (`0x489390`, `0x4893b0`, `0x48ab40`, `0x48ab50`), and posts
/// what the frame applies after ([`SessionPosts`]).
#[derive(SystemParam)]
pub(crate) struct SessionSteps<'w> {
    /// Absent in a harness with no logout state, which drops the logout verbs.
    logout: Option<ResMut<'w, LogoutState>>,
    self_guid: Option<Res<'w, SelfGuid>>,
    commands: Res<'w, NetCommands>,
    reload: Option<ResMut<'w, crate::ui_script::ReloadUiPending>>,
    posts: Option<Res<'w, SessionPosts>>,
}

impl SessionSteps<'_> {
    /// Apply `request` inside its verb.
    pub(crate) fn apply(&mut self, request: SessionRequest) {
        match request {
            SessionRequest::Logout => self.logout(false),
            SessionRequest::Quit => self.logout(true),
            // `0x5aaf80`: `CMSG_LOGOUT_CANCEL` and the pending byte cleared (`0x5aafc2`). The
            // stock dialogs' `OnHide` sends this on every early close, even after a cancel landed,
            // so a stray cancel is normal. `quitting` clears here too, so a dead socket cannot
            // leave a quit armed.
            SessionRequest::CancelLogout => {
                if let Some(logout) = self.logout.as_deref_mut() {
                    logout.quitting = false;
                    logout.pending = false;
                }
                let _ = self.commands.0.send(ClientCommand::LogoutCancel);
            }
            // `0x401560(0)`: the process ends, with no server round trip.
            SessionRequest::ForceQuit => {
                info!("logout: force quit");
                self.exit_now();
            }
            // `0x5aaff0` is `0x5ab000(0, 1)`: only with a world session (`0x5ab020`), past the
            // pending bail (`0x5ab018`); it stores the quit byte as 0 (`0x5ab053`) and leaves the
            // pending byte (`0x5ab059`), then sends `CMSG_PLAYER_LOGOUT` (`0x5ab07c`).
            SessionRequest::ForceLogout => {
                if self.in_world() {
                    info!("logout: forced");
                    if let Some(logout) = self.logout.as_deref_mut() {
                        logout.quitting = false;
                    }
                    let _ = self.commands.0.send(ClientCommand::ForceLogout);
                }
            }
            // `0x4884d0` sets `0xb4b3f4`; the rebuild is `ui_script::run_pending_reload`'s, at the
            // top of the next frame, outside any call.
            SessionRequest::ReloadUi => {
                if let Some(reload) = self.reload.as_deref_mut() {
                    reload.0 = true;
                }
            }
            // Esc out of a cinematic: the reference's `StopCinematic` has no binding and no
            // native caller, as `CinematicFrame.xml:37-44`'s `OnKeyDown` is the whole skip path.
            SessionRequest::StopCinematic => {
                if let Some(posts) = &self.posts {
                    posts.stop_cinematic.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    fn in_world(&self) -> bool {
        self.self_guid.as_ref().is_some_and(|g| g.0.is_some())
    }

    fn exit_now(&self) {
        if let Some(posts) = &self.posts {
            posts.exit.store(true, Ordering::Relaxed);
        }
    }

    /// `0x5ab000(quit, 0)`: silent while a request is pending (`0x5ab01a`), before the quit byte
    /// is stored (`0x5ab053`), so a second ask cannot turn a camp into a quit; then the byte, the
    /// pending byte (`0x5ab05b`) and `CMSG_LOGOUT_REQUEST` (`0x5ab07c`).
    fn logout(&mut self, quit: bool) {
        let in_world = self.in_world();
        let Some(logout) = self.logout.as_deref_mut() else {
            return;
        };
        if logout.pending {
            return;
        }
        logout.quitting = quit;
        // No world session: a quit just exits, as the reference's glue-screen `Quit()` does, and
        // a logout does nothing.
        if !in_world || self.commands.0.send(ClientCommand::Logout).is_err() {
            logout.quitting = false;
            if quit {
                info!("logout: quit with no world session — exiting");
                self.exit_now();
            } else {
                warn!("logout: no world session — logout dropped");
            }
            return;
        }
        logout.pending = true;
        // Logged: instant or timed is the server's call.
        info!("logout: requested (quitting={quit}) — awaiting SMSG_LOGOUT_RESPONSE");
    }
}

/// `SMSG_LOGOUT_COMPLETE` returns to character select, or ends the process for Exit Game; a
/// posted exit ([`SessionPosts`]) ends it too.
fn exit_on_logout_complete(
    mut logged_out: MessageReader<LoggedOutMessage>,
    mut logout: ResMut<LogoutState>,
    posts: Res<SessionPosts>,
    mut exit: MessageWriter<AppExit>,
) {
    if posts.exit.swap(false, Ordering::Relaxed) {
        exit.write(AppExit::Success);
    }
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
        app.init_resource::<LogoutState>()
            .init_resource::<SessionPosts>()
            .add_systems(
                Update,
                (
                    feed_logout.in_set(UiFeed),
                    exit_on_logout_complete.in_set(crate::ui_script::UiDrain),
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
    use benilla_ui::script::UiScript;

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

    /// The whole client, headless, in the world, with a VM and the write channel captured.
    fn client() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        app.insert_resource(NetCommands(tx))
            .insert_resource(SelfGuid(Some(0x4000_0000_0000_0009)));
        app.insert_non_send_resource(UiScript::new().unwrap());
        (app, rx)
    }

    /// Run `chunk` with the host lent; what it sent of the logout's and the roll's packets.
    fn sent_by(
        app: &mut App,
        rx: &crossbeam_channel::Receiver<ClientCommand>,
        chunk: &str,
    ) -> Vec<String> {
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM").run(chunk).expect("the chunk runs")
        });
        rx.try_iter()
            .filter_map(|c| {
                Some(match c {
                    ClientCommand::RandomRoll { min, max } => format!("roll {min}-{max}"),
                    ClientCommand::Logout => "logout".into(),
                    ClientCommand::LogoutCancel => "cancel".into(),
                    ClientCommand::ForceLogout => "force".into(),
                    _ => return None,
                })
            })
            .collect()
    }

    /// The session verbs send inside their calls (`0x5ab07c`, `0x5aafb9`), so a chunk's packets
    /// go out in call order with every other verb's, and the pending byte a `Logout` sets
    /// (`0x5ab05b`) silences the next line's (`0x5ab01a`).
    #[test]
    fn a_chunks_session_verbs_send_in_call_order_with_every_other_verb() {
        let (mut app, rx) = client();
        assert_eq!(
            sent_by(
                &mut app,
                &rx,
                "RandomRoll(1, 2) Logout() Logout() RandomRoll(3, 4) Quit() CancelLogout() \
                 RandomRoll(5, 6) Quit()",
            ),
            ["roll 1-2", "logout", "roll 3-4", "cancel", "roll 5-6", "logout"]
        );
        let logout = app.world().resource::<LogoutState>();
        assert!(logout.pending && logout.quitting, "the last Quit asked");
    }

    /// The `0x5ab000` bail: a second ask while the first is unanswered sends nothing, and
    /// `ForceLogout` (`0x5aaff0`, `0x5ab000(0, 1)`) passes it, leaves the pending byte
    /// (`0x5ab059`) and stores the quit byte as 0 (`0x5ab053`), so the completion that follows
    /// ends the session and not the process.
    #[test]
    fn a_forced_logout_passes_the_bail_and_disarms_the_quit() {
        let (mut app, rx) = client();
        assert_eq!(sent_by(&mut app, &rx, "Quit()"), ["logout"]);
        assert_eq!(
            sent_by(&mut app, &rx, "Quit() Logout() ForceLogout()"),
            ["force"]
        );
        let logout = app.world().resource::<LogoutState>();
        assert!(logout.pending, "the forced request leaves the byte");
        assert!(!logout.quitting, "and stores the quit byte as 0");
        app.world_mut().write_message(LoggedOutMessage);
        app.world_mut()
            .run_system_once(exit_on_logout_complete)
            .expect("complete");
        assert!(!app.world().resource::<LogoutState>().pending);
        assert!(
            app.world_mut()
                .resource_mut::<Messages<AppExit>>()
                .drain()
                .next()
                .is_none(),
            "a forced logout after a quit ends the session, not the process"
        );
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
