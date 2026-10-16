//! The realm-list boundary smoke (`WOW_REALM_SMOKE=1`, run by `scripts/smoke.sh`), against a
//! real server:
//!
//! 1. from character select, Change Realm: the dialog goes up over the screen, which stays;
//! 2. Okay on the current realm: a world re-dial whose roster arrives with the dialog down and the
//!    screen still character select;
//! 3. Change Realm again, then Cancel: the dialog goes down and the session is untouched;
//! 4. one more Change Realm and Okay, proving the IO thread is at the park the app expects.
//!
//! "At character select" is the screen up and a roster having arrived, however many characters it
//! held. Under `WOW_GLUE_STOCK=1` the walk presses the stock screens' own verbs in the glue VM
//! (`CharacterSelect_ChangeRealm`, `RealmList_OnOk`, `RealmList_OnCancel`) and reads their frames.
//!
//! Inert without the env.

use bevy::prelude::*;

use crate::char_select::Roster;
use crate::client_state::ClientState;
use crate::net::{CharListMessage, RealmChoice};
use crate::ui_script::glue::{StockGlueProbe, StockVm};

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
    /// When the client was first seen at character select.
    arrived: Option<f32>,
}

/// What the walk presses and reads: the native dialog, or the stock screens in the glue VM.
trait Seat {
    /// The character screen is up, whatever its roster holds.
    fn screen_up(&self) -> bool;
    /// The realm list is on screen.
    fn dialog_up(&self) -> bool;
    /// The walk can press Change Realm.
    fn can_open(&self) -> bool;
    /// The character screen's Change Realm button.
    fn open(&mut self);
    /// `RealmList_OnOk` on whatever the dialog opened with highlighted, or the first row.
    fn okay(&mut self);
    /// `RealmList_OnCancel` over character select, where it only hides the list.
    fn cancel(&mut self);
}

/// The native dialog.
struct Native<'a> {
    realms: &'a mut Realms,
    choice: &'a RealmChoice,
    generation: u64,
}

impl Seat for Native<'_> {
    fn screen_up(&self) -> bool {
        true
    }
    fn dialog_up(&self) -> bool {
        self.realms.shown
    }
    fn can_open(&self) -> bool {
        !self.realms.realms.is_empty()
    }
    fn open(&mut self) {
        super::open_over_char_select(self.realms, self.choice);
    }
    fn okay(&mut self) {
        let name = self.realms.selected().map(|r| r.name.clone()).or_else(|| {
            self.realms
                .rows()
                .first()
                .map(|&i| self.realms.realms[i].name.clone())
        });
        let Some(name) = name else {
            return;
        };
        self.realms.hide();
        self.realms.enter(self.choice, name, self.generation);
    }
    fn cancel(&mut self) {
        self.realms.hide();
    }
}

/// The stock `CharacterSelect` and `RealmList` frames, pressed through their own functions.
struct Stock<'a>(StockVm<'a>);

impl Seat for Stock<'_> {
    fn screen_up(&self) -> bool {
        self.0.frame_visible("CharacterSelect")
    }
    fn dialog_up(&self) -> bool {
        self.0.frame_visible("RealmList")
    }
    fn can_open(&self) -> bool {
        self.0.is_up()
    }
    fn open(&mut self) {
        self.0.press("CharacterSelect_ChangeRealm()");
    }
    fn okay(&mut self) {
        // The highlight `RealmListUpdate` left on the current realm, else the first row.
        self.0.press(
            "if RealmList.currentRealm == 0 then RealmList.currentRealm = 1 end RealmList_OnOk()",
        );
    }
    fn cancel(&mut self) {
        self.0.press("RealmList_OnCancel()");
    }
}

/// What one frame of the walk sees besides its seat.
struct Sight {
    now: f32,
    state: ClientState,
    characters: usize,
    realms: usize,
}

/// How a frame of the walk ended.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Going,
    Done,
    Failed(String),
}

/// Whether the run asked for the walk.
pub(super) fn enabled() -> bool {
    std::env::var_os("WOW_REALM_SMOKE").is_some()
}

/// The walk; every leg logs itself, so a timeout names the boundary that stalled.
pub(super) fn debug_realm_smoke(
    mut msgs: MessageReader<CharListMessage>,
    mut realms: ResMut<Realms>,
    roster: Res<Roster>,
    state: Res<State<ClientState>>,
    choice: Res<RealmChoice>,
    abandon: Option<Res<crate::net::LoginAbandon>>,
    stock: StockGlueProbe,
    time: Res<Time>,
    mut walk: Local<Walk>,
    mut exit: MessageWriter<AppExit>,
) {
    walk.seen += msgs.read().count() as u32;
    if walk.phase == u8::MAX {
        return;
    }
    // `WOW_CHAR` and `WOW_RIG` each seat a body and answer the roster this walk drives, so
    // either one, even inherited from the shell, fails the walk on its first frame.
    if !walk.armed {
        walk.armed = true;
        if let Some((key, value)) = ["WOW_CHAR", "WOW_RIG"]
            .into_iter()
            .find_map(|k| std::env::var_os(k).map(|v| (k, v)))
        {
            error!(
                "realm-smoke: FAILED — {key}={} seats a body — this walk drives CHARACTER SELECT \
                 and there is no roster screen to drive once one is seated. Unset it \
                 (`scripts/smoke.sh` does).",
                value.to_string_lossy(),
            );
            exit.write(AppExit::error());
            walk.phase = u8::MAX;
            return;
        }
    }
    let sight = Sight {
        now: time.elapsed_secs(),
        state: *state.get(),
        characters: roster.chars.len(),
        realms: realms.realms.len(),
    };
    let verdict = if let Some(vm) = stock.vm() {
        step(&mut walk, &sight, &mut Stock(vm))
    } else {
        let generation = crate::net::LoginAbandon::now(abandon.as_deref());
        let mut seat = Native {
            realms: &mut realms,
            choice: &choice,
            generation,
        };
        step(&mut walk, &sight, &mut seat)
    };
    match verdict {
        Verdict::Going => {}
        Verdict::Done => {
            exit.write(AppExit::Success);
        }
        Verdict::Failed(why) => {
            error!("realm-smoke: FAILED — {why}");
            exit.write(AppExit::error());
        }
    }
}

/// One frame of the walk.
fn step(walk: &mut Walk, sight: &Sight, seat: &mut dyn Seat) -> Verdict {
    let now = sight.now;
    // The screen is up and a roster has arrived, empty or not: an account with no characters
    // reaches select all the same.
    let at_select = sight.state == ClientState::CharSelect && walk.seen > 0 && seat.screen_up();
    fn fail(walk: &mut Walk, why: String) -> Verdict {
        walk.phase = u8::MAX;
        Verdict::Failed(why)
    }
    if walk.phase == 0 && now - walk.mark > ENTRY_TIMEOUT {
        return fail(
            walk,
            format!(
                "never reached character select in {ENTRY_TIMEOUT:.0}s (state {:?}, {} rosters, \
                 {} characters, {} realms) — nothing was driven, so the fault is upstream of \
                 the walk",
                sight.state, walk.seen, sight.characters, sight.realms,
            ),
        );
    }
    if walk.phase > 0 && now - walk.mark > LEG_TIMEOUT {
        return fail(
            walk,
            format!(
                "leg {} stalled for {LEG_TIMEOUT:.0}s (state {:?}, dialog {}, rosters {}/{})",
                walk.phase,
                sight.state,
                if seat.dialog_up() { "up" } else { "down" },
                walk.seen,
                walk.rosters,
            ),
        );
    }

    match walk.phase {
        // ── 1 · Change Realm: the dialog goes up over the screen. ────────────────────────────
        0 if at_select && seat.can_open() && now - *walk.arrived.get_or_insert(now) > DWELL => {
            info!("realm-smoke: at character select → Change Realm");
            seat.open();
            (walk.phase, walk.mark) = (1, now);
        }
        1 if seat.dialog_up() && now - walk.mark > DWELL => {
            if !at_select {
                return fail(
                    walk,
                    "the dialog replaced the character screen instead of standing over it".into(),
                );
            }
            info!("realm-smoke: dialog up, character select still underneath → Okay");
            seat.okay();
            (walk.phase, walk.mark, walk.rosters) = (2, now, walk.seen + 1);
        }
        // ── 2 · Okay has to land: a fresh roster, dialog down, still at select. ──────────────
        2 if walk.seen >= walk.rosters && !seat.dialog_up() && at_select => {
            info!("realm-smoke: Okay landed — fresh roster at character select → Change Realm");
            seat.open();
            (walk.phase, walk.mark) = (3, now);
        }
        // ── 3 · Cancel changes nothing. ──────────────────────────────────────────────────────
        3 if seat.dialog_up() && now - walk.mark > DWELL => {
            info!("realm-smoke: dialog up → Cancel");
            seat.cancel();
            (walk.phase, walk.mark) = (4, now);
        }
        4 if !seat.dialog_up() && now - walk.mark > DWELL => {
            if !at_select {
                return fail(
                    walk,
                    "Cancel left character select instead of just hiding the dialog".into(),
                );
            }
            // ── 4 · And the park is still where the app thinks it is. ────────────────────────
            info!("realm-smoke: Cancel kept the session → one more Change Realm + Okay");
            seat.open();
            (walk.phase, walk.mark) = (5, now);
        }
        5 if seat.dialog_up() && now - walk.mark > DWELL => {
            seat.okay();
            (walk.phase, walk.mark, walk.rosters) = (6, now, walk.seen + 1);
        }
        6 if walk.seen >= walk.rosters && !seat.dialog_up() && at_select => {
            info!(
                "realm-smoke: done — {} roster(s), {} char(s) at character select",
                walk.seen, sight.characters
            );
            walk.phase = u8::MAX;
            return Verdict::Done;
        }
        _ => {}
    }
    Verdict::Going
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A seat that records what the walk pressed; Okay brings a roster, as a world re-dial does.
    #[derive(Default)]
    struct Fake {
        screen: bool,
        up: bool,
        listed: bool,
        roster_due: bool,
        pressed: Vec<&'static str>,
    }

    impl Seat for Fake {
        fn screen_up(&self) -> bool {
            self.screen
        }
        fn dialog_up(&self) -> bool {
            self.up
        }
        fn can_open(&self) -> bool {
            self.listed
        }
        fn open(&mut self) {
            self.up = true;
            self.pressed.push("open");
        }
        fn okay(&mut self) {
            self.up = false;
            self.roster_due = true;
            self.pressed.push("okay");
        }
        fn cancel(&mut self) {
            self.up = false;
            self.pressed.push("cancel");
        }
    }

    fn sight(now: f32, state: ClientState, characters: usize) -> Sight {
        Sight {
            now,
            state,
            characters,
            realms: 1,
        }
    }

    /// An account with no characters reaches select with a roster that holds none: the roster
    /// having arrived is the arrival, and the walk starts.
    #[test]
    fn an_empty_roster_is_an_arrival() {
        let mut seat = Fake {
            screen: true,
            listed: true,
            ..Fake::default()
        };
        let mut walk = Walk::default();
        let at = |now| sight(now, ClientState::CharSelect, 0);
        assert_eq!(step(&mut walk, &at(1.0), &mut seat), Verdict::Going);
        assert_eq!(walk.phase, 0, "no roster has arrived");
        walk.seen = 1;
        assert_eq!(step(&mut walk, &at(2.0), &mut seat), Verdict::Going);
        assert_eq!(walk.phase, 0, "the screen has a beat to settle first");
        let later = at(2.0 + DWELL + 0.1);
        assert_eq!(step(&mut walk, &later, &mut seat), Verdict::Going);
        assert_eq!(
            (walk.phase, seat.pressed.as_slice()),
            (1, ["open"].as_slice())
        );
    }

    /// The whole walk, on an account with no characters: every leg lands on the roster that
    /// arrived, and the last frame is the walk done.
    #[test]
    fn the_walk_completes_on_an_empty_roster() {
        let mut seat = Fake {
            screen: true,
            listed: true,
            ..Fake::default()
        };
        let mut walk = Walk {
            seen: 1,
            ..Walk::default()
        };
        let mut verdict = Verdict::Going;
        let mut now = 1.0;
        while verdict == Verdict::Going && now < 30.0 {
            verdict = step(
                &mut walk,
                &sight(now, ClientState::CharSelect, 0),
                &mut seat,
            );
            if std::mem::take(&mut seat.roster_due) {
                walk.seen += 1;
            }
            now += 0.5;
        }
        assert_eq!(verdict, Verdict::Done);
        assert_eq!(
            seat.pressed,
            ["open", "okay", "open", "cancel", "open", "okay"]
        );
    }

    /// With no roster ever, the walk fails at the entry timeout, naming the fault as upstream.
    #[test]
    fn the_walk_times_out_without_a_roster() {
        let mut seat = Fake {
            screen: true,
            listed: true,
            ..Fake::default()
        };
        let mut walk = Walk::default();
        let at = |now| sight(now, ClientState::CharSelect, 0);
        assert_eq!(
            step(&mut walk, &at(ENTRY_TIMEOUT - 1.0), &mut seat),
            Verdict::Going
        );
        let Verdict::Failed(why) = step(&mut walk, &at(ENTRY_TIMEOUT + 1.0), &mut seat) else {
            panic!("no timeout");
        };
        assert!(why.contains("never reached character select"), "{why}");
        assert!(seat.pressed.is_empty(), "nothing was driven");
    }

    /// The screen not being up (the stock frames hidden) is not select, roster or no.
    #[test]
    fn a_roster_with_no_screen_is_not_select() {
        let mut seat = Fake {
            listed: true,
            ..Fake::default()
        };
        let mut walk = Walk {
            seen: 1,
            ..Walk::default()
        };
        step(
            &mut walk,
            &sight(2.0, ClientState::CharSelect, 3),
            &mut seat,
        );
        step(&mut walk, &sight(3.0, ClientState::Login, 3), &mut seat);
        assert_eq!(walk.phase, 0);
        seat.screen = true;
        step(&mut walk, &sight(4.0, ClientState::Login, 3), &mut seat);
        assert_eq!(walk.phase, 0, "the state says otherwise");
    }
}
