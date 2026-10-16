//! The glue round-trip (`WOW_GLUE_ROUNDTRIP=1`): once a roster has arrived at character select, go
//! to character create, Back to character select, and exit. Ungated, since it crosses states and
//! runs over either glue screen set.
//!
//! "Roster has arrived" counts the roster, not its characters: an account with none reaches select
//! all the same. The stock screens are driven through their own verbs in the glue VM
//! (`SetGlueScreen("charcreate")`, `CharacterCreate_Back`) and their frames are read, since the
//! app's [`ClientState`] does not follow the stock create screen.

use bevy::prelude::*;

use super::roster::Roster;
use crate::client_state::ClientState;
use crate::net::CharListMessage;
use crate::ui_script::glue::StockGlueProbe;

/// How long the round-trip waits to reach character select; the whole login is inside it.
const ENTRY_TIMEOUT: f32 = 60.0;
/// How long each leg may take before the round-trip names the one that stalled.
const LEG_TIMEOUT: f32 = 25.0;
/// A beat on each screen, so a stuck frame has time to show.
const DWELL: f32 = 1.5;

/// Which screen the client is on, as far as the round-trip tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Select,
    Create,
    Other,
}

/// What the round-trip does next.
#[derive(Debug, PartialEq, Eq)]
enum Act {
    Wait,
    EnterCreate,
    BackToSelect,
    Done,
    Fail(String),
}

#[derive(Default)]
pub(super) struct Roundtrip {
    /// 0 waits for select, 1 for create, 2 for select again; 3 is over.
    phase: u8,
    mark: f32,
    /// The rosters that have arrived.
    rosters: u32,
}

impl Roundtrip {
    /// One frame: what the client shows, how many characters the roster holds, and the answer.
    fn step(&mut self, now: f32, screen: Screen, characters: usize) -> Act {
        if self.phase == 0 && now - self.mark > ENTRY_TIMEOUT {
            self.phase = 3;
            return Act::Fail(format!(
                "never reached character select in {ENTRY_TIMEOUT:.0}s (screen {screen:?}, {} \
                 rosters, {characters} characters) — nothing was driven, so the fault is \
                 upstream of the round-trip",
                self.rosters,
            ));
        }
        if matches!(self.phase, 1 | 2) && now - self.mark > LEG_TIMEOUT {
            let leg = if self.phase == 1 { "create" } else { "back" };
            let phase = std::mem::replace(&mut self.phase, 3);
            return Act::Fail(format!(
                "leg {phase} ({leg}) stalled for {LEG_TIMEOUT:.0}s (screen {screen:?})"
            ));
        }
        match self.phase {
            0 if self.rosters > 0 && screen == Screen::Select => {
                info!(
                    "glue-roundtrip: initial roster = {characters} char(s) → entering CharCreate"
                );
                (self.phase, self.mark) = (1, now);
                Act::EnterCreate
            }
            1 if screen == Screen::Create && now - self.mark > DWELL => {
                info!("glue-roundtrip: in CharCreate → Back to CharSelect");
                (self.phase, self.mark) = (2, now);
                Act::BackToSelect
            }
            2 if screen == Screen::Select && now - self.mark > DWELL => {
                info!("glue-roundtrip: back at CharSelect, roster = {characters} char(s) — done");
                self.phase = 3;
                Act::Done
            }
            _ => Act::Wait,
        }
    }
}

/// Whether the run asked for the round-trip.
pub(super) fn enabled() -> bool {
    std::env::var_os("WOW_GLUE_ROUNDTRIP").is_some()
}

pub(super) fn debug_glue_roundtrip(
    mut msgs: MessageReader<CharListMessage>,
    roster: Res<Roster>,
    state: Res<State<ClientState>>,
    mut next: ResMut<NextState<ClientState>>,
    stock: StockGlueProbe,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
    mut trip: Local<Roundtrip>,
) {
    trip.rosters += msgs.read().count() as u32;
    // `Some` when the stock switch is on: the frames say which screen is up, and with no glue VM
    // standing none is.
    let stock_vm = stock.vm();
    let screen = match stock_vm {
        Some(vm) if vm.frame_visible("CharacterSelect") => Screen::Select,
        Some(vm) if vm.frame_visible("CharacterCreate") => Screen::Create,
        Some(_) => Screen::Other,
        None => match state.get() {
            ClientState::CharSelect => Screen::Select,
            ClientState::CharCreate => Screen::Create,
            _ => Screen::Other,
        },
    };
    match trip.step(time.elapsed_secs(), screen, roster.chars.len()) {
        Act::Wait => {}
        act @ (Act::EnterCreate | Act::BackToSelect) => {
            let (chunk, to) = if act == Act::EnterCreate {
                ("SetGlueScreen('charcreate')", ClientState::CharCreate)
            } else {
                ("CharacterCreate_Back()", ClientState::CharSelect)
            };
            match stock_vm {
                Some(vm) => vm.press(chunk),
                None => next.set(to),
            }
        }
        Act::Done => {
            exit.write(AppExit::Success);
        }
        Act::Fail(why) => {
            error!("glue-roundtrip: FAILED — {why}");
            exit.write(AppExit::error());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An account with no characters reaches select with a roster that is empty: that arrival
    /// starts the round-trip, which then goes through create and back.
    #[test]
    fn an_empty_roster_is_an_arrival() {
        let mut trip = Roundtrip::default();
        assert_eq!(
            trip.step(1.0, Screen::Select, 0),
            Act::Wait,
            "no roster yet"
        );
        trip.rosters += 1;
        assert_eq!(trip.step(2.0, Screen::Select, 0), Act::EnterCreate);
        assert_eq!(
            trip.step(2.5, Screen::Create, 0),
            Act::Wait,
            "a beat on create"
        );
        assert_eq!(trip.step(4.0, Screen::Create, 0), Act::BackToSelect);
        assert_eq!(
            trip.step(4.5, Screen::Select, 0),
            Act::Wait,
            "a beat on select"
        );
        assert_eq!(trip.step(6.0, Screen::Select, 0), Act::Done);
    }

    /// A roster that arrives before the screen does waits for the screen.
    #[test]
    fn a_roster_waits_for_the_screen() {
        let mut trip = Roundtrip {
            rosters: 1,
            ..Roundtrip::default()
        };
        assert_eq!(trip.step(1.0, Screen::Other, 3), Act::Wait);
        assert_eq!(trip.step(2.0, Screen::Select, 3), Act::EnterCreate);
    }

    /// With no roster ever, the round-trip fails at the entry timeout instead of waiting forever.
    #[test]
    fn it_times_out_waiting_for_the_roster() {
        let mut trip = Roundtrip::default();
        assert_eq!(trip.step(ENTRY_TIMEOUT - 1.0, Screen::Select, 0), Act::Wait);
        let Act::Fail(why) = trip.step(ENTRY_TIMEOUT + 1.0, Screen::Select, 0) else {
            panic!("no timeout");
        };
        assert!(why.contains("never reached character select"), "{why}");
        assert_eq!(trip.step(ENTRY_TIMEOUT + 2.0, Screen::Select, 0), Act::Wait);
    }

    /// A leg that never lands names itself.
    #[test]
    fn it_times_out_in_a_leg() {
        for (reach, leg, screen) in [
            (0, "leg 1 (create)", Screen::Select),
            (1, "leg 2 (back)", Screen::Create),
        ] {
            let mut trip = Roundtrip {
                rosters: 1,
                ..Roundtrip::default()
            };
            assert_eq!(trip.step(1.0, Screen::Select, 0), Act::EnterCreate);
            if reach == 1 {
                assert_eq!(trip.step(3.0, Screen::Create, 0), Act::BackToSelect);
            }
            let Act::Fail(why) = trip.step(3.0 + LEG_TIMEOUT + 1.0, screen, 0) else {
                panic!("{leg}: no timeout");
            };
            assert!(why.contains(leg), "{why}");
        }
    }

    /// The system counts the roster message, not the characters in it: an empty one at select
    /// sends the app to create, on the native screens, whose arm moves the state.
    #[test]
    fn the_system_takes_an_empty_roster_message_for_arrival() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_resource(crate::ui_script::glue::GlueStock(false))
            .insert_state(ClientState::CharSelect)
            .init_resource::<Roster>()
            .add_message::<CharListMessage>()
            .add_systems(Update, debug_glue_roundtrip);
        let pending = |app: &App| {
            matches!(
                app.world().resource::<NextState<ClientState>>(),
                NextState::Pending(ClientState::CharCreate)
            )
        };
        app.update();
        assert!(!pending(&app), "no roster yet");
        app.world_mut().write_message(CharListMessage {
            characters: Vec::new(),
            realm: None,
            cause: benilla_protocol::RosterCause::Park,
            epoch: 0,
            generation: 0,
        });
        app.update();
        assert!(pending(&app), "an empty roster is a roster");
    }
}
