//! The per-frame wire→ECS bridge: [`apply_net_updates`] drains the inbound [`SessionEvent`]
//! channel through the handler table. Nothing is applied here: every packet's handler lives with
//! its subsystem, our own player's create and its arrival with the update packet's
//! ([`super::objects`]).

use benilla_protocol::SessionEvent;
use bevy::prelude::*;

use super::NetEvents;

#[cfg(test)]
mod seam_tests;
#[cfg(test)]
mod world_enter_tests;

// ── The per-frame bridge systems ─────────────────────────────────────────────────────────────────

/// Runs this frame's events through the handler table ([`super::handlers`]) in wire order, each
/// handler's commands applied before the next, before anything else in
/// [`benilla_world::schedule::WorldStage::Net`] runs, the world session's held behind its entry
/// load ([`InboundHold`]).
pub(crate) fn apply_net_updates(world: &mut World) {
    let events: Vec<SessionEvent> = world.resource::<NetEvents>().0.try_iter().collect();
    let arrived = events.len();
    let mut hold = world.remove_resource::<InboundHold>().unwrap_or_default();
    let mut out = hold.release(world);
    for ev in events {
        hold.admit(ev, &mut out);
    }
    world.insert_resource(hold);
    // `drn`: where the frame's packets land, against the input records' `prs` and `snd` lines;
    // then, behind what they sent, the flushes the dispatch fired, their lane firings and its time.
    let traced = !out.is_empty() && benilla_assets::trace::enabled_for("drn");
    if traced {
        benilla_assets::trace::line("drn", &format!("events={arrived} packets={}", out.len()));
    }
    let before = traced.then(|| (crate::lanes::tally(), std::time::Instant::now()));
    super::handlers::dispatch(world, out);
    if let Some(((flushes, fired), at)) = before {
        let (f, k) = crate::lanes::tally();
        benilla_assets::trace::line(
            "drn",
            &format!(
                "done flushes={} fired={} us={}",
                f - flushes,
                k - fired,
                at.elapsed().as_micros()
            ),
        );
    }
}

/// The world session's inbound events behind its entry load. The reference sends the login only
/// once the interface is up (`0x46c272`), so no world packet is handled before it; here the login
/// goes out early (`net::io`'s deviation), and what it brings waits, in wire order, from
/// `Connected` until the load has run.
#[derive(Resource, Default)]
pub(crate) struct InboundHold {
    holding: bool,
    /// The entry load was seen armed since `Connected`.
    armed: bool,
    held: Vec<SessionEvent>,
}

impl InboundHold {
    /// One event off the wire: onto `out`, the drain's dispatch in order, or held.
    fn admit(&mut self, ev: SessionEvent, out: &mut Vec<SessionEvent>) {
        match &ev {
            // `SMSG_PONG` is the one packet the socket thread answers itself (`0x537b56`).
            SessionEvent::Pong { .. } if self.holding => out.push(ev),
            // A session's end lets everything before it through, in order: no load is coming.
            SessionEvent::Disconnected { .. }
            | SessionEvent::LoggedOut
            | SessionEvent::CharacterLoginFailed { .. }
                if self.holding =>
            {
                out.append(&mut self.held);
                out.push(ev);
                *self = Self::default();
            }
            _ if self.holding => self.held.push(ev),
            // The world session starts: what follows waits for the entry load.
            SessionEvent::Connected { .. } => {
                out.push(ev);
                self.holding = true;
            }
            _ => out.push(ev),
        }
    }

    /// The held events once the load has run, or once none is coming: the world was not entered
    /// (no `InWorld` transition owed or made, the arm never seen).
    fn release(&mut self, world: &World) -> Vec<SessionEvent> {
        if !self.holding {
            return Vec::new();
        }
        if world.contains_resource::<crate::ui_script::PendingEntryUiLoad>() {
            self.armed = true;
            return Vec::new();
        }
        let entering = world
            .get_resource::<NextState<crate::client_state::ClientState>>()
            .is_some_and(|n| {
                matches!(
                    n,
                    NextState::Pending(crate::client_state::ClientState::InWorld)
                )
            });
        if entering && !self.armed {
            return Vec::new();
        }
        std::mem::take(self).held
    }
}

#[cfg(test)]
mod hold_tests {
    use benilla_protocol::{SessionEnd, SessionEvent};
    use bevy::prelude::*;

    use super::InboundHold;

    fn connected() -> SessionEvent {
        SessionEvent::Connected {
            self_guid: 0x2a,
            name: "Probeseven".into(),
            billing: Default::default(),
            tutorial_flags: None,
            addon_info: None,
        }
    }

    /// What `events` leaves on the drain's dispatch, by kind.
    fn admitted(hold: &mut InboundHold, events: Vec<SessionEvent>) -> Vec<String> {
        let mut out = Vec::new();
        for ev in events {
            hold.admit(ev, &mut out);
        }
        out.iter()
            .map(|ev| format!("{:?}", benilla_protocol::SessionEventKind::from(ev)))
            .collect()
    }

    /// A `SMSG_PONG` passes the hold (`0x537b56`); a session's end lets the held events through
    /// first, in wire order, then itself, and holds nothing after.
    #[test]
    fn a_pong_passes_and_an_end_releases_in_wire_order() {
        let mut hold = InboundHold::default();
        let time = || SessionEvent::PlayedTime { total: 1, level: 1 };
        assert_eq!(
            admitted(
                &mut hold,
                vec![connected(), time(), SessionEvent::Pong { sequence: 1 }]
            ),
            ["Connected", "Pong"],
            "the world packet held, the pong through"
        );
        let end = SessionEvent::Disconnected {
            reason: "gone".into(),
            end: SessionEnd::Lost,
        };
        assert_eq!(
            admitted(&mut hold, vec![time(), end, time()]),
            ["PlayedTime", "PlayedTime", "Disconnected", "PlayedTime"]
        );
    }

    /// No load coming (no `InWorld` transition owed, the arm never seen): the hold lets go at the
    /// next drain; one owed waits for the arm.
    #[test]
    fn the_hold_lets_go_when_no_load_is_coming() {
        let mut world = World::new();
        let mut hold = InboundHold::default();
        let held = vec![connected(), SessionEvent::Pong { sequence: 1 }];
        admitted(&mut hold, held);
        hold.held.push(SessionEvent::LoggedOut);
        world.insert_resource(NextState::Pending(
            crate::client_state::ClientState::InWorld,
        ));
        assert!(
            hold.release(&world).is_empty(),
            "the entry is owed: still held"
        );
        world.remove_resource::<NextState<crate::client_state::ClientState>>();
        assert_eq!(hold.release(&world).len(), 1, "no load coming: released");
        assert!(!hold.holding);
    }

    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    /// The digests come in the login's own frame, behind `Connected`: they wait for the entry
    /// load, so the load finds every account-data section pending and its readers are held for
    /// them (`0x5afe50`), as the reference's load precedes the login (`0x46c272`). A `SMSG_PONG`
    /// passes the hold (`0x537b56`).
    #[test]
    fn the_login_burst_waits_for_the_entry_load() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-net-hold-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
            bevy::ecs::error::ignore,
        ));
        let (tx, _sent) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        let (inbound, rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetEvents::for_test(rx));
        let pending = |app: &bevy::prelude::App| {
            app.world()
                .resource::<crate::account_data::AccountData>()
                .pending(crate::account_data::CHAT)
        };
        for ev in [connected(), SessionEvent::AccountDataMd5([[0; 16]; 8])] {
            inbound.send(ev).unwrap();
        }
        app.update();
        assert!(pending(&app), "held behind Connected");
        app.update();
        assert!(
            app.world()
                .contains_resource::<crate::ui_script::PendingEntryUiLoad>(),
            "the entry load is armed"
        );
        assert!(pending(&app), "still held while the load is armed");
        inbound.send(SessionEvent::Pong { sequence: 1 }).unwrap();
        let mut frames = 0;
        while app
            .world()
            .contains_resource::<crate::ui_script::PendingEntryUiLoad>()
        {
            app.update();
            frames += 1;
            assert!(frames < 10, "the entry load ran");
        }
        assert!(!pending(&app), "released after the load, in wire order");
        drop(app);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
