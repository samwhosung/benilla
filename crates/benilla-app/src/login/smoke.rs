//! The login smoke (`WOW_LOGIN_SMOKE`) — the headless prover for the screen's own submit path.
//! Split from `mod.rs` when the file outgrew its budget; the policy and the screen stay there.

use bevy::prelude::*;

use super::Attempt;
use crate::char_select::ClientState;
use crate::net::LoginFailedMessage;

/// Split `WOW_LOGIN_SMOKE=user:pass[:Character]` into its three fields. **Three-way, once**: a
/// `split_once(':')` here would hand the character name to the password, which is exactly what it
/// did on this instrument's first live run (`result 0x04`, and the wrong-password path is what
/// this smoke exists to prove, so it looked plausible).
fn smoke_spec(spec: &str) -> (&str, &str, Option<&str>) {
    let mut parts = spec.splitn(3, ':');
    let user = parts.next().unwrap_or_default();
    let pass = parts.next().unwrap_or_default();
    let character = parts.next().map(str::trim).filter(|n| !n.is_empty());
    (user, pass, character)
}

/// The optional third field — the body to enter as, or `None` for the plain
/// reach-character-select smoke. [`crate::char_select`]'s roster policy is what actually answers
/// the roster with it.
pub(crate) fn smoke_character(spec: &str) -> Option<String> {
    smoke_spec(spec).2.map(str::to_string)
}

/// The login smoke (`WOW_LOGIN_SMOKE=user:pass[:Character]`): once the screen is
/// up, submit those credentials through the real screen path; exit success on reaching CharSelect,
/// log + exit failure on a refusal — the wrong-password path is provable headlessly.
///
/// **Naming a character keeps the run going into the world instead of exiting**.
/// It was the only headless way to reach the world down the *player's* path, back when setting
/// `WOW_CHAR` also made the run unattended and so switched the very branch a session test wanted
/// to exercise; decision 1769 severed that, and `WOW_CHAR` now says nothing about who is in the
/// room. The seat stays because it is the smoke's own way in, and because a run that declares
/// `WOW_UNATTENDED=1` still needs a way to name a character without taking it. The pick itself
/// stays `char_select`'s (`apply_roster_policy` reads the third field the same way it reads
/// `WOW_CHAR`); this only declines to exit. Pair with `WOW_PROBE_EXIT_AT` to bound the run.
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
                // `WOW_LOGIN_SMOKE_HOLD=1`: keep running on a refusal instead of exiting — the
                // error dialog stays up, so a shot instrument can photograph it (the dialog is
                // otherwise unreachable headlessly; pair with `WOW_PROBE_EXIT_AT`).
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

    /// The smoke spec splits three ways — the character name must never end up in the password.
    /// A `split_once` did exactly that on the instrument's first live run, and the refusal it
    /// produced (`0x04`) is indistinguishable from the wrong-password case this smoke exists to
    /// exercise.
    #[test]
    fn the_smoke_spec_splits_three_ways() {
        assert_eq!(
            smoke_spec("probe1:secret:Probeone"),
            ("probe1", "secret", Some("Probeone"))
        );
        assert_eq!(smoke_spec("probe1:secret"), ("probe1", "secret", None));
        // A trailing colon names no body; a password may not itself contain one (the client caps
        // both fields at 16 letters and vmangos accounts have none).
        assert_eq!(smoke_spec("one:pass:"), ("one", "pass", None));
        assert_eq!(smoke_spec("one"), ("one", "", None));
    }
}
