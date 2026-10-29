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

/// The refusal line for every non-zero reason, never `PLAYER_LOGOUT_FAILED_ERROR`: message `0x180`
/// of the `DisplayError` at `0x5aaf26`, a key so the player's `GlobalStrings.lua` supplies it.
const LOGOUT_FAILED_KEY: &str = "ERR_LOGOUT_FAILED";

/// What a logout reply signals inside its handler, on the notice lane at the packet's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogoutSignal {
    /// A timed logout started: `PLAYER_CAMPING` (276, `0x5aaf17`).
    Camping,
    /// The same for a quit: `PLAYER_QUITING` (277), the quit byte added to the id (`0x5aaf0c`).
    Quiting,
    /// The logout was cancelled: `LOGOUT_CANCEL` (278, `0x5aaf72`).
    Cancelled,
    /// Refused: the error line, no dialog (`0x5aaf26`).
    Refused,
}

impl LogoutSignal {
    /// The notice that signals it.
    fn notice(self) -> crate::field_watch::FieldNotice {
        use crate::field_watch::{id, FieldNotice};
        match self {
            Self::Camping => FieldNotice::Global {
                id: id::PLAYER_CAMPING,
            },
            Self::Quiting => FieldNotice::Global {
                id: id::PLAYER_QUITING,
            },
            Self::Cancelled => FieldNotice::Global {
                id: id::LOGOUT_CANCEL,
            },
            Self::Refused => FieldNotice::Line {
                key: LOGOUT_FAILED_KEY,
            },
        }
    }
}

/// The session's logout bytes, which the replies' handlers and the session verbs read and write
/// in place.
#[derive(Resource, Default)]
pub(crate) struct LogoutState {
    /// The reference's `[session+0x1b1c]`, the one bit the wire lacks, as a quit is also
    /// `CMSG_LOGOUT_REQUEST`. Only a request writes it (`0x5ab053`), so a cancel or a refusal
    /// leaves it and the next request overwrites it.
    quitting: bool,
    /// The reference's `[session+0x1b1d]`: a request is out and unanswered. `0x5ab000` drops a
    /// second request silently while it is set, and the idle handler skips its thirty-minute leg
    /// on it (`0x482ef5`); `ForceLogout` (`0x5aaff0`) bypasses it and does not set it.
    pending: bool,
    /// The reference's world handle `[session+0x1b18]`: set by the active player's arrival
    /// (`0x5deb73`, `0x5aae70(1)`) after its world-enter cascade, zeroed by every leave of the
    /// world (`0x5aae70(0)`, [`Self::apply_left_world`]). A request with none is dropped before
    /// the quit byte (`0x5ab020`-`0x5ab02a`), so a `Logout()` on a worldport's loading screen sends
    /// nothing.
    world_handle: bool,
}

impl LogoutState {
    /// The pending byte, as `0x5ab0d0` returns it.
    pub(crate) fn pending(&self) -> bool {
        self.pending
    }

    /// `SMSG_LOGOUT_RESPONSE` (`0x5aaef0`), the module doc's table: what it signals, nothing for
    /// an instant logout.
    fn apply_response(&mut self, reason: u32, instant: bool) -> Option<LogoutSignal> {
        // Logged: the server's inputs (combat, resting, account level) are invisible from here.
        info!("logout: server answered reason={reason} instant={instant}");
        if reason != 0 {
            // `0x5aaf21`: the error line, and the pending flag clears so the player can retry.
            self.pending = false;
            return Some(LogoutSignal::Refused);
        }
        if instant {
            return None;
        }
        Some(if self.quitting {
            LogoutSignal::Quiting
        } else {
            LogoutSignal::Camping
        })
    }

    /// `SMSG_LOGOUT_CANCEL_ACK` (`0x5b4680`, then `0x5aaf60`). `CancelLogout` clears the pending
    /// byte as it sends (`0x5aafc2`), so the ack of the player's own cancel signals nothing.
    fn apply_cancelled(&mut self) -> Option<LogoutSignal> {
        self.call_off()
    }

    /// The world left, `0x5aae70(0)`: by a world load (`0x401bd4`), by the logout's completion
    /// (`0x5aaeb3` → `0x5aaf4f`, ahead of the teardown at `0x5aaeca`), and by a refused login
    /// (`0x5aad76` → `0x5aaf4f`). A completed logout that was not forced signals `LOGOUT_CANCEL`.
    fn apply_left_world(&mut self) -> Option<LogoutSignal> {
        self.world_handle = false;
        self.call_off()
    }

    /// The world handle seated, as a test of the verbs in the world needs it.
    #[cfg(test)]
    pub(crate) fn seat_world_handle_for_test(&mut self) {
        self.world_handle = true;
    }

    /// `LOGOUT_CANCEL` only while a request is pending (`0x5aaf63`-`0x5aaf6b`,
    /// `0x5aae88`-`0x5aae90`), then the byte clears (`0x5aaf77`, `0x5aae9c`).
    fn call_off(&mut self) -> Option<LogoutSignal> {
        std::mem::take(&mut self.pending).then_some(LogoutSignal::Cancelled)
    }
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
    /// The account-data timers a logout request runs first; absent in a harness without them.
    account_data: Option<Res<'w, crate::account_data::AccountData>>,
}

impl SessionSteps<'_> {
    /// Apply `request` inside its verb.
    pub(crate) fn apply(&mut self, request: SessionRequest) {
        match request {
            SessionRequest::Logout => self.logout(false),
            SessionRequest::Quit => self.logout(true),
            // `0x5aaf80`: `CMSG_LOGOUT_CANCEL` and the pending byte cleared (`0x5aafc2`); the quit
            // byte stays. The stock dialogs' `OnHide` sends this on every early close, even after
            // a cancel landed, so a stray cancel is normal.
            SessionRequest::CancelLogout => {
                if let Some(logout) = self.logout.as_deref_mut() {
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
                    self.flush_account_data();
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

    /// The world handle (`0x5ab020`), with a world session to send on.
    fn in_world(&self) -> bool {
        self.self_guid.as_ref().is_some_and(|g| g.0.is_some())
            && self.logout.as_deref().is_some_and(|l| l.world_handle)
    }

    /// `0x5ab049`, on the request's path past the world test: the account-data timers run first
    /// (`0x5afd30`), so their uploads leave ahead of it.
    fn flush_account_data(&self) {
        if let Some(data) = &self.account_data {
            data.flush(&self.commands);
        }
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
        // No world session (`0x5ab02a`, before the byte): a quit just exits, as the reference's
        // glue-screen `Quit()` does, and a logout does nothing.
        if in_world {
            logout.quitting = quit;
            // The field, not [`Self::flush_account_data`]: `logout` holds `self.logout`.
            if let Some(data) = &self.account_data {
                data.flush(&self.commands);
            }
        }
        if !in_world || self.commands.0.send(ClientCommand::Logout).is_err() {
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
    // `0x5aaed7`: the pending byte clears, which the handler's leave of the world did already
    // (`0x5aae9c`); otherwise the next character's first logout would be swallowed.
    logout.pending = false;
    // `0x5aaecf`: the quit byte decides, and stays, as the process ends.
    if logout.quitting {
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
                exit_on_logout_complete.in_set(crate::ui_script::UiDrain),
            )
            .add_systems(
                PreUpdate,
                // The world load's first call, ahead of its teardown and the sweep.
                leave_world
                    .in_set(crate::net::WorldportTimer::Connection)
                    .run_if(|pending: Option<Res<crate::net::PendingWorldports>>| {
                        pending.is_some_and(|p| p.is_posted())
                    }),
            );
    }
}

/// `SetInWorld(1)` (`0x5aae70`), our player's arrival's last call into the connection (`0x5deb73`),
/// after its world-enter cascade: the world handle seated (`0x5aae82`), then `LOGOUT_CANCEL` in
/// place when a logout request was pending (`0x5aae88`-`0x5aae9c`), its byte cleared.
pub(crate) fn set_in_world(
    mut vm: crate::script_calls::VmWith<Option<ResMut<'static, LogoutState>>>,
) {
    let called_off = vm.own().is_some_and(|mut logout| {
        logout.world_handle = true;
        std::mem::take(&mut logout.pending)
    });
    if called_off {
        vm.run(|script| {
            if let Some(script) = script {
                script.fire_event("LOGOUT_CANCEL", Vec::new());
            }
        });
    }
}

/// `SetInWorld(0)` (`0x5aae70`) as the world load's first call (`0x401bd4`): the world handle
/// zeroed and `LOGOUT_CANCEL` in place when a logout request was pending (`0x5aae88`-`0x5aae9c`),
/// its byte cleared, ahead of the load's teardown. Runs only with a load posted.
pub(crate) fn leave_world(
    mut vm: crate::script_calls::VmWith<Option<ResMut<'static, LogoutState>>>,
) {
    let called_off = vm.own().is_some_and(|mut logout| {
        logout.world_handle = false;
        std::mem::take(&mut logout.pending)
    });
    if called_off {
        vm.run(|script| {
            if let Some(script) = script {
                script.fire_event("LOGOUT_CANCEL", Vec::new());
            }
        });
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
            .net_handler(K::LogoutCancelled, on_narration)
            .net_handler(K::Worldport, on_narration)
            .net_handler(K::LoggedOut, on_narration)
            .net_handler(K::CharacterLoginFailed, on_narration);
    }

    /// Each reply's signal goes on the notice lane at its packet's place, as the reference's
    /// handlers signal in place (`0x5aaf17`, `0x5aaf26`, `0x5aaf72`, `0x5aae96`).
    fn on_narration(
        In(ev): In<SessionEvent>,
        mut logout: ResMut<LogoutState>,
        mut lane: ResMut<crate::field_watch::NoticeLane>,
    ) {
        let signal = match ev {
            SessionEvent::LogoutResponse { reason, instant } => {
                logout.apply_response(reason, instant)
            }
            SessionEvent::LogoutCancelled => logout.apply_cancelled(),
            // `SMSG_LOGIN_VERIFY_WORLD` for another map (`0x401ea4`) runs the world load
            // `0x401bc0` in place; `SMSG_NEW_WORLD` posts it as a timer, whose run leaves the
            // world ([`super::leave_world`]). `SMSG_LOGOUT_COMPLETE` (`0x5aaeb0`) and
            // `SMSG_CHARACTER_LOGIN_FAILED` (`0x5aad70`) leave the world first.
            SessionEvent::Worldport {
                needs_ack: false, ..
            }
            | SessionEvent::LoggedOut
            | SessionEvent::CharacterLoginFailed { .. } => logout.apply_left_world(),
            _ => None,
        };
        if let Some(signal) = signal {
            lane.write(signal.notice());
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use benilla_protocol::SessionEvent;
    use benilla_ui::script::UiScript;

    #[test]
    fn the_response_decides_which_dialog_if_any() {
        let mut s = LogoutState::default();
        assert_eq!(
            s.apply_response(0, true),
            None,
            "an instant logout shows no dialog"
        );
        assert_eq!(s.apply_response(0, false), Some(LogoutSignal::Camping));

        let mut s = LogoutState {
            quitting: true,
            pending: true,
            ..LogoutState::default()
        };
        assert_eq!(
            s.apply_response(0, false),
            Some(LogoutSignal::Quiting),
            "a quit says so"
        );

        // In combat: the error line (`0x5aaf21`) and the pending byte cleared (`0x5aaf2e`); the
        // quit byte stays, as only a request writes it (`0x5ab053`).
        let mut s = LogoutState {
            quitting: true,
            pending: true,
            ..LogoutState::default()
        };
        assert_eq!(s.apply_response(1, false), Some(LogoutSignal::Refused));
        assert!(s.quitting, "the refusal leaves the quit byte");
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
        app.world_mut()
            .resource_mut::<LogoutState>()
            .seat_world_handle_for_test();
        app.insert_non_send_resource(UiScript::new().unwrap());
        (app, rx)
    }

    /// What the logout replies' handlers put on the notice lane, taken.
    fn notices(app: &mut App) -> Vec<crate::field_watch::FieldNotice> {
        app.world_mut()
            .resource_mut::<crate::field_watch::NoticeLane>()
            .drain()
            .collect()
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

    /// `SMSG_LOGOUT_CANCEL_ACK` → `0x5aaf60`: `LOGOUT_CANCEL` only while a request is pending
    /// (`0x5aaf63`-`0x5aaf6b`), then the pending byte clears (`0x5aaf77`); the quit byte is not
    /// its to touch.
    #[test]
    fn the_cancel_ack_fires_only_while_a_request_is_pending() {
        let mut s = LogoutState {
            quitting: true,
            pending: true,
            ..LogoutState::default()
        };
        assert_eq!(s.apply_cancelled(), Some(LogoutSignal::Cancelled));
        assert!(!s.pending);
        assert!(s.quitting, "the ack leaves the quit byte");
        assert_eq!(s.apply_cancelled(), None, "nothing pending, nothing fired");
    }

    /// The ack of the player's own cancel fires nothing: `CancelLogout` (`0x5aaf80`) clears the
    /// pending byte as it sends (`0x5aafc2`), so by the ack's `0x5aaf60` nothing is pending.
    #[test]
    fn the_ack_of_a_cancelled_logout_fires_nothing() {
        let (mut app, rx) = client();
        assert_eq!(
            sent_by(&mut app, &rx, "Logout() CancelLogout() CancelLogout()"),
            ["logout", "cancel", "cancel"]
        );
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::LogoutCancelled, SessionEvent::LogoutCancelled],
        );
        assert!(notices(&mut app).is_empty());
    }

    /// A world load leaves the world first (`0x401bd4` → `0x5aae70(0)`), which fires
    /// `LOGOUT_CANCEL` in place while a request is pending (`0x5aae88`-`0x5aae97`) and clears the
    /// byte (`0x5aae9c`), so the next `Logout()` is heard. `SMSG_NEW_WORLD` only posts the load
    /// (`0x401ba5`), so the request stands until the timer runs.
    #[test]
    fn a_world_load_calls_off_a_pending_logout() {
        let (mut app, rx) = client();
        app.world_mut()
            .non_send_resource::<UiScript>()
            .run(
                "Cancelled = 0 local f = CreateFrame('Frame') f:RegisterEvent('LOGOUT_CANCEL') \
                 f:SetScript('OnEvent', function() Cancelled = Cancelled + 1 end)",
            )
            .expect("spy");
        let cancelled =
            |app: &App| -> i64 { crate::script_calls::eval_in_vm(app.world(), "return Cancelled") };
        let port = || SessionEvent::Worldport {
            map_id: 1,
            position: [0.0; 3],
            orientation: 0.0,
            needs_ack: true,
        };
        assert_eq!(sent_by(&mut app, &rx, "Logout()"), ["logout"]);
        crate::net::handlers::dispatch(app.world_mut(), vec![port()]);
        let logout = app.world().resource::<LogoutState>();
        assert!(logout.pending, "the packet only posts the load");
        assert!(notices(&mut app).is_empty());
        assert_eq!(cancelled(&app), 0);
        app.world_mut()
            .run_system_cached(leave_world)
            .expect("the load");
        let logout = app.world().resource::<LogoutState>();
        assert!(!logout.pending, "the load clears the pending byte");
        assert_eq!(cancelled(&app), 1, "and signals it in place");
        crate::net::handlers::dispatch(app.world_mut(), vec![port()]);
        app.world_mut()
            .run_system_cached(leave_world)
            .expect("the load");
        assert_eq!(cancelled(&app), 1, "with nothing pending it fires nothing");
        app.world_mut()
            .resource_mut::<LogoutState>()
            .seat_world_handle_for_test();
        assert_eq!(sent_by(&mut app, &rx, "Logout()"), ["logout"]);
    }

    /// `SMSG_LOGOUT_COMPLETE` (`0x5aaeb0`) leaves the world first (`0x5aaeb3` → `0x5aaf40` →
    /// `0x5aae70(0)` at `0x5aaf4f`), so a completed logout that was not forced fires
    /// `LOGOUT_CANCEL` (`0x5aae97`) while the world's interface still stands, ahead of the
    /// teardown at `0x5aaeca`; a forced one (`0x5aaff0`) set no pending byte and fires none.
    #[test]
    fn a_completed_logout_leaves_the_world_with_a_cancel() {
        let (mut app, rx) = client();
        assert_eq!(sent_by(&mut app, &rx, "Logout()"), ["logout"]);
        crate::net::handlers::dispatch(app.world_mut(), vec![SessionEvent::LoggedOut]);
        assert!(!app.world().resource::<LogoutState>().pending);
        assert_eq!(notices(&mut app), [LogoutSignal::Cancelled.notice()]);

        let (mut app, rx) = client();
        assert_eq!(sent_by(&mut app, &rx, "ForceLogout()"), ["force"]);
        crate::net::handlers::dispatch(app.world_mut(), vec![SessionEvent::LoggedOut]);
        assert!(notices(&mut app).is_empty());
    }

    /// `SMSG_CHARACTER_LOGIN_FAILED` (`0x5b45f0` → `0x5aad70`) leaves the world first
    /// (`0x5aad76` → `0x5aaf40` → `0x5aae70(0)`), which calls off a pending logout.
    #[test]
    fn a_refused_login_calls_off_a_pending_logout() {
        let (mut app, rx) = client();
        assert_eq!(sent_by(&mut app, &rx, "Logout()"), ["logout"]);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::CharacterLoginFailed { result: 1 }],
        );
        assert!(!app.world().resource::<LogoutState>().pending);
        assert_eq!(notices(&mut app), [LogoutSignal::Cancelled.notice()]);
    }

    /// A world load zeroes the world handle (`0x401bd4` → `0x5aae70(0)`, stored at `0x5aae82`)
    /// until the active player's arrival seats it again (`0x5deb73`): a `Logout()` on the
    /// loading screen between is dropped before anything is stored (`0x5ab020`-`0x5ab02a`).
    #[test]
    fn a_logout_on_a_worldports_loading_screen_sends_nothing() {
        let (mut app, rx) = client();
        let port = SessionEvent::Worldport {
            map_id: 1,
            position: [0.0; 3],
            orientation: 0.0,
            needs_ack: true,
        };
        crate::net::handlers::dispatch(app.world_mut(), vec![port]);
        app.world_mut()
            .run_system_cached(leave_world)
            .expect("the load");
        assert_eq!(sent_by(&mut app, &rx, "Quit()"), Vec::<String>::new());
        let logout = app.world().resource::<LogoutState>();
        assert!(!logout.pending && !logout.quitting, "nothing stored");
        app.world_mut()
            .run_system_cached(set_in_world)
            .expect("the arrival");
        assert_eq!(sent_by(&mut app, &rx, "Logout()"), ["logout"]);
    }

    /// The logout through the real schedule, one frame at a time: `SMSG_LOGOUT_COMPLETE`'s leave
    /// of the world fires `LOGOUT_CANCEL` in its packet's frame, on the world's interface
    /// (`0x5aaeb3`); the teardown it then runs (`0x5aaeca` → `0x401ee0`) sweeps the world
    /// (`PLAYER_LEAVING_WORLD`, `0x490b4d`) and shuts the interface (`PLAYER_LOGOUT`,
    /// `0x490c2a`), which is the next frame's `OnExit(InWorld)`.
    #[test]
    fn a_completed_logout_cancels_then_leaves_then_logs_out_a_frame_later() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let (mut app, rx) = client();
        app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
            bevy::ecs::error::ignore,
        ));
        let (events, inbound) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetEvents::for_test(inbound));
        // The start-up frame first, which seats a VM of its own.
        app.update();
        app.insert_non_send_resource(UiScript::new().expect("a VM"));
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        app.world_mut()
            .resource_mut::<crate::ui_script::LeavingWorldArmed>()
            .arm();
        let notes: Rc<RefCell<Vec<String>>> = Rc::default();
        {
            let sink = notes.clone();
            let script = app.world().non_send_resource::<UiScript>();
            let note = script
                .lua()
                .create_function(move |_, text: String| {
                    sink.borrow_mut().push(text);
                    Ok(())
                })
                .expect("the note");
            script.lua().globals().set("Note", note).expect("Note");
            script
                .run(
                    "FRAME = 0 local f = CreateFrame('Frame') \
                     f:RegisterEvent('LOGOUT_CANCEL') f:RegisterEvent('PLAYER_LEAVING_WORLD') \
                     f:RegisterEvent('PLAYER_LOGOUT') \
                     f:SetScript('OnEvent', function() Note(event .. ' ' .. FRAME) end)",
                )
                .expect("the recorder");
        }
        assert_eq!(sent_by(&mut app, &rx, "Logout()"), ["logout"]);
        events
            .send(SessionEvent::LoggedOut)
            .expect("the inbound channel");
        for frame in 1..=2 {
            crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
                vm.expect("a VM")
                    .run(&format!("FRAME = {frame}"))
                    .expect("FRAME")
            });
            app.update();
        }
        assert_eq!(
            *notes.borrow(),
            [
                "LOGOUT_CANCEL 1",
                "PLAYER_LEAVING_WORLD 2",
                "PLAYER_LOGOUT 2"
            ]
        );
    }

    /// `SMSG_LOGOUT_COMPLETE` through [`exit_on_logout_complete`]: whether the process exits.
    fn complete(app: &mut App) -> bool {
        app.world_mut().write_message(LoggedOutMessage);
        app.world_mut()
            .run_system_once(exit_on_logout_complete)
            .expect("complete");
        app.world_mut()
            .resource_mut::<Messages<AppExit>>()
            .drain()
            .next()
            .is_some()
    }

    /// Only a request writes the quit byte (`0x5ab053`); `CancelLogout` (`0x5aaf80`) and its ack
    /// (`0x5aaf60`) leave it. A completion no request asked for after a cancelled `Quit()` ends
    /// the process (`0x5aaecf`), as 1.12.1's does, and a later `Logout()` stores 0 again, so its
    /// completion ends only the session.
    #[test]
    fn a_cancelled_quit_keeps_the_quit_byte_until_the_next_request() {
        let (mut app, rx) = client();
        assert_eq!(
            sent_by(&mut app, &rx, "Quit() CancelLogout()"),
            ["logout", "cancel"]
        );
        crate::net::handlers::dispatch(app.world_mut(), vec![SessionEvent::LogoutCancelled]);
        assert!(complete(&mut app), "the kept byte exits on a completion");

        let (mut app, rx) = client();
        sent_by(&mut app, &rx, "Quit() CancelLogout()");
        crate::net::handlers::dispatch(app.world_mut(), vec![SessionEvent::LogoutCancelled]);
        assert_eq!(sent_by(&mut app, &rx, "Logout()"), ["logout"]);
        assert!(
            !complete(&mut app),
            "a later logout's completion ends the session alone"
        );
    }

    /// `0x5ab049`: a logout request runs the armed account-data timers first (`0x5afd30`), so a
    /// section a `/reload`'s shutdown wrote in the last 30 seconds goes up ahead of
    /// `CMSG_LOGOUT_REQUEST`; a forced logout passes the same call.
    #[test]
    fn a_logout_request_sends_the_armed_sections_first() {
        use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-logout-flush-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let macros = crate::local_state::macros_account_path().unwrap();

        let (mut app, rx) = client();
        let data = || app.world().resource::<crate::account_data::AccountData>();
        data().settle_for_test(None);
        crate::local_state::write_atomic(&macros, "MACRO 1 \"A\" X\nEND\n").unwrap();
        data().saved(&[4]);
        data().shut_down_ui();
        let sent = |app: &mut App, chunk: &str| {
            crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
                vm.expect("a VM").run(chunk).expect("the chunk runs")
            });
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::UpdateAccountData { section, .. } => {
                        Some(format!("update {section}"))
                    }
                    ClientCommand::Logout => Some("logout".into()),
                    ClientCommand::ForceLogout => Some("force".into()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(sent(&mut app, "Logout()"), ["update 4", "logout"]);
        assert_eq!(
            sent(&mut app, "CancelLogout() Logout()"),
            ["logout"],
            "the flush ran the timer: nothing armed is left"
        );

        crate::local_state::write_atomic(&macros, "MACRO 1 \"B\" X\nEND\n").unwrap();
        let data = app.world().resource::<crate::account_data::AccountData>();
        data.saved(&[4]);
        data.shut_down_ui();
        assert_eq!(sent(&mut app, "ForceLogout()"), ["update 4", "force"]);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
