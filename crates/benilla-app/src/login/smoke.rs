//! The login smoke (`WOW_LOGIN_SMOKE`): a headless run of the native screen's own submit path.

use bevy::prelude::*;

use crate::client_state::ClientState;
use crate::glue_session::login::{smoke_character, smoke_spec, Attempt};
use crate::net::LoginFailedMessage;

/// Submits `WOW_LOGIN_SMOKE` through the real screen path once it is up: exits success at
/// CharSelect, failure on a refusal. Naming a character keeps the run going into the world
/// instead; bound it with `WOW_PROBE_EXIT_AT`.
pub(super) fn debug_login_smoke(
    state: Res<State<ClientState>>,
    mut attempt: Attempt,
    mut failures: MessageReader<LoginFailedMessage>,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
    mut phase: Local<u8>,
) {
    let Ok(spec) = std::env::var("WOW_LOGIN_SMOKE") else {
        return;
    };
    match *phase {
        0 if *state.get() == ClientState::Login && time.elapsed_secs() > 2.0 => {
            let (user, pass, _) = smoke_spec(&spec);
            info!("login-smoke: submitting as {user}");
            let (user, pass) = (user.to_string(), pass.to_string());
            attempt.send(&user, &pass, true);
            *phase = 1;
        }
        1 => {
            if let Some(f) = failures.read().last() {
                error!(
                    "login-smoke: FAILED refusal={:?} reason={}",
                    f.refusal, f.reason
                );
                // `WOW_LOGIN_SMOKE_HOLD=1` keeps the error dialog up for a capture.
                if std::env::var_os("WOW_LOGIN_SMOKE_HOLD").is_none() {
                    exit.write(AppExit::error());
                }
                *phase = 2;
            } else if *state.get() == ClientState::CharSelect {
                match smoke_character(&spec) {
                    Some(name) => {
                        info!("login-smoke: reached character select — entering as {name}");
                    }
                    None => {
                        info!("login-smoke: reached character select — done");
                        exit.write(AppExit::Success);
                    }
                }
                *phase = 2;
            }
        }
        _ => {}
    }
}
