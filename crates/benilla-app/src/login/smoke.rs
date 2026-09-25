//! The login smoke (`WOW_LOGIN_SMOKE`): a headless run of the screen's own submit path.

use bevy::prelude::*;

use super::Attempt;
use crate::char_select::ClientState;
use crate::net::LoginFailedMessage;

/// Split `WOW_LOGIN_SMOKE=user:pass[:Character]` three ways: a `split_once(':')` would hand the
/// character name to the password, a refusal indistinguishable from a wrong password.
fn smoke_spec(spec: &str) -> (&str, &str, Option<&str>) {
    let mut parts = spec.splitn(3, ':');
    let user = parts.next().unwrap_or_default();
    let pass = parts.next().unwrap_or_default();
    let character = parts.next().map(str::trim).filter(|n| !n.is_empty());
    (user, pass, character)
}

/// The optional third field, the character to enter as; [`crate::char_select`]'s roster policy
/// acts on it.
pub(crate) fn smoke_character(spec: &str) -> Option<String> {
    smoke_spec(spec).2.map(str::to_string)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The character name must never end up in the password.
    #[test]
    fn the_smoke_spec_splits_three_ways() {
        assert_eq!(
            smoke_spec("probe1:secret:Probeone"),
            ("probe1", "secret", Some("Probeone"))
        );
        assert_eq!(smoke_spec("probe1:secret"), ("probe1", "secret", None));
        // A trailing colon names no character; vmangos account passwords hold no colon.
        assert_eq!(smoke_spec("player:pass:"), ("player", "pass", None));
        assert_eq!(smoke_spec("player"), ("player", "", None));
    }
}
