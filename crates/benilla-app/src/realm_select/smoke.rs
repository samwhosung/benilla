//! The realm-list boundary smoke (`WOW_REALM_SMOKE=1`, run by `scripts/smoke.sh`), against a
//! real server:
//!
//! 1. from character select, Change Realm: the dialog goes up over the screen, which stays;
//! 2. Okay on the current realm: a world re-dial whose roster arrives with the dialog down and the
//!    screen still character select;
//! 3. Change Realm again, then Cancel: the dialog goes down and the session is untouched;
//! 4. one more Change Realm and Okay, proving the IO thread is at the park the app expects.
//!
//! Inert without the env.

use bevy::prelude::*;

use crate::char_select::{ClientState, Roster};
use crate::net::{RealmChoice, RealmRequest};

use super::Realms;

/// How long each leg may take before the walk names the one that stalled.
const LEG_TIMEOUT: f32 = 25.0;
/// How long the walk waits to reach character select; the whole login is inside it.
const ENTRY_TIMEOUT: f32 = 60.0;
/// A beat with the dialog up, so a stuck frame has time to show.
const DWELL: f32 = 1.0;

#[derive(Default)]
pub(super) struct Walk {
    phase: u8,
    mark: f32,
    /// The roster count to reach: an entered realm sends a new roster.
    rosters: u32,
    seen: u32,
    /// The one-shot startup refusal has run.
    armed: bool,
}

/// The walk; every leg logs itself, so a timeout names the boundary that stalled.
pub(super) fn debug_realm_smoke(
    mut msgs: MessageReader<crate::net::CharListMessage>,
    realms: ResMut<Realms>,
    roster: Res<Roster>,
    state: Res<State<ClientState>>,
    choice: Res<RealmChoice>,
    time: Res<Time>,
    mut walk: Local<Walk>,
    mut exit: MessageWriter<AppExit>,
) {
    walk.seen += msgs.read().count() as u32;
    if std::env::var_os("WOW_REALM_SMOKE").is_none() || walk.phase == u8::MAX {
        return;
    }
    let now = time.elapsed_secs();
    let at_select = *state.get() == ClientState::CharSelect && !roster.chars.is_empty();
    let mut fail = |why: &str| {
        error!("realm-smoke: FAILED — {why}");
        exit.write(AppExit::error());
    };
    // `WOW_CHAR` and `WOW_RIG` each seat a body and answer the roster this walk drives, so
    // either one, even inherited from the shell, fails the walk on its first frame.
    if !walk.armed {
        walk.armed = true;
        if let Some((key, value)) = ["WOW_CHAR", "WOW_RIG"]
            .into_iter()
            .find_map(|k| std::env::var_os(k).map(|v| (k, v)))
        {
            fail(&format!(
                "{key}={} seats a body — this walk drives CHARACTER SELECT and there is no roster \
                 screen to drive once one is seated. Unset it (`scripts/smoke.sh` does).",
                value.to_string_lossy(),
            ));
            walk.phase = u8::MAX;
            return;
        }
    }
    if walk.phase == 0 && now - walk.mark > ENTRY_TIMEOUT {
        fail(&format!(
            "never reached character select in {ENTRY_TIMEOUT:.0}s (state {:?}, {} characters, \
             {} realms) — nothing was driven, so the fault is upstream of the walk",
            state.get(),
            roster.chars.len(),
            realms.realms.len(),
        ));
        walk.phase = u8::MAX;
        return;
    }
    if walk.phase > 0 && now - walk.mark > LEG_TIMEOUT {
        fail(&format!(
            "leg {} stalled for {LEG_TIMEOUT:.0}s (state {:?}, dialog {}, rosters {}/{})",
            walk.phase,
            state.get(),
            if realms.shown { "up" } else { "down" },
            walk.seen,
            walk.rosters,
        ));
        walk.phase = u8::MAX;
        return;
    }

    match walk.phase {
        // ── 1 · Change Realm: the dialog goes up over the screen. ────────────────────────────
        0 if at_select && !realms.realms.is_empty() => {
            info!("realm-smoke: at character select → Change Realm");
            open(realms, &choice);
            (walk.phase, walk.mark) = (1, now);
        }
        1 if realms.shown && now - walk.mark > DWELL => {
            if *state.get() != ClientState::CharSelect {
                fail("the dialog replaced the character screen instead of standing over it");
                walk.phase = u8::MAX;
                return;
            }
            info!("realm-smoke: dialog up, character select still underneath → Okay");
            okay(realms, &choice);
            (walk.phase, walk.mark, walk.rosters) = (2, now, walk.seen + 1);
        }
        // ── 2 · Okay has to land: a fresh roster, dialog down, still at select. ──────────────
        2 if walk.seen >= walk.rosters && !realms.shown && at_select => {
            info!("realm-smoke: Okay landed — fresh roster at character select → Change Realm");
            open(realms, &choice);
            (walk.phase, walk.mark) = (3, now);
        }
        // ── 3 · Cancel changes nothing. ──────────────────────────────────────────────────────
        3 if realms.shown && now - walk.mark > DWELL => {
            info!("realm-smoke: dialog up → Cancel");
            cancel(realms, &choice);
            (walk.phase, walk.mark) = (4, now);
        }
        4 if !realms.shown && now - walk.mark > DWELL => {
            if !at_select {
                fail("Cancel left character select instead of just hiding the dialog");
                walk.phase = u8::MAX;
                return;
            }
            // ── 4 · And the park is still where the app thinks it is. ────────────────────────
            info!("realm-smoke: Cancel kept the session → one more Change Realm + Okay");
            open(realms, &choice);
            (walk.phase, walk.mark) = (5, now);
        }
        5 if realms.shown && now - walk.mark > DWELL => {
            okay(realms, &choice);
            (walk.phase, walk.mark, walk.rosters) = (6, now, walk.seen + 1);
        }
        6 if walk.seen >= walk.rosters && !realms.shown && at_select => {
            info!(
                "realm-smoke: done — {} roster(s), {} char(s) at character select",
                walk.seen,
                roster.chars.len()
            );
            exit.write(AppExit::Success);
            walk.phase = u8::MAX;
        }
        _ => {}
    }
}

/// The character screen's Change Realm button, as the walk presses it.
fn open(mut realms: ResMut<Realms>, choice: &RealmChoice) {
    super::open_over_char_select(&mut realms, choice);
}

/// `RealmList_OnOk` on whatever the dialog opened with highlighted, or the first row.
fn okay(mut realms: ResMut<Realms>, choice: &RealmChoice) {
    let name = realms.selected().map(|r| r.name.clone()).or_else(|| {
        realms
            .rows()
            .first()
            .map(|&i| realms.realms[i].name.clone())
    });
    let Some(name) = name else {
        return;
    };
    realms.hide();
    let _ = choice.0.send(RealmRequest::Enter(name));
}

/// `RealmList_OnCancel`.
fn cancel(mut realms: ResMut<Realms>, choice: &RealmChoice) {
    realms.hide();
    let _ = choice.0.send(RealmRequest::Abandon);
}
