//! The inbound **handler table** — the reference's own shape for the wire's
//! arrival side, and the cut 2265 §A1 asked for between the net bridge and the game windows.
//!
//! The real client keeps an opcode → handler table inside `NetClient` (`+0x74`, 828 slots; 387
//! registrations by 37 subsystem clusters) and its dispatcher `0x537aa0` knows
//! none of them: it looks the opcode up and calls what it finds, **in packet order**, discarding an
//! unregistered opcode in silence. Here the table is [`NetHandlers`]: a [`SessionEventKind`] →
//! handlers map that every subsystem fills for itself through [`NetHandlerApp::net_handler`], and
//! the drain ([`super::apply_net_updates`]) is exclusive over the world so the handlers — ordinary
//! systems taking `In<SessionEvent>`, registered as one-shots — run one after another in the
//! order the packets arrived, each seeing what the one before it did.
//!
//! **Why not one typed `Message<T>` per family with a reader system each** (2265's sketch): a
//! reader per family runs *after* the whole drain, so two families' packets interleaved in one
//! frame are handled family by family, not in packet order — a chat line and a combat-log line
//! that arrived in one drain would swap. The table keeps the one property 2265 said must survive
//! any split: one frame, packet order, before anything else runs.
//!
//! **The table is the whole dispatch** (since 2327). Every kind has at least one handler, read
//! off the built client by `every_session_event_kind_has_one_owner`; the drain's own match, the
//! runs it took across the seam and the broadcast list (2305, 2306, 2326) are gone.

use std::collections::HashMap;

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemId;
use bevy::prelude::*;

/// One registered handler: the one-shot system and the name it registers under (for the census).
type Handler = (SystemId<In<SessionEvent>>, &'static str);

/// The table. Filled at plugin build by every subsystem that answers a packet; read by the drain.
#[derive(Resource, Default)]
pub(crate) struct NetHandlers {
    by_kind: HashMap<SessionEventKind, Vec<Handler>>,
}

impl NetHandlers {
    /// Every kind with a handler, with the handlers' names in registration order — the owner
    /// test's view of the table.
    #[cfg(test)]
    pub(crate) fn census(&self) -> std::collections::BTreeMap<SessionEventKind, Vec<&'static str>> {
        self.by_kind
            .iter()
            .map(|(k, v)| (*k, v.iter().map(|(_, n)| *n).collect()))
            .collect()
    }

    /// Run **every** registered handler once with `ev` — whatever kind it registered for — and
    /// name the ones that could not run. A handler matches on its own kind and ignores the rest,
    /// so what this exercises is the part a wrong-kind event still reaches: the system's
    /// parameters being fetched from this world.
    #[cfg(test)]
    pub(crate) fn probe(&self, world: &mut World, ev: &SessionEvent) -> Vec<String> {
        let mut failed = Vec::new();
        for (kind, list) in &self.by_kind {
            for (id, name) in list {
                if let Err(e) = world.run_system_with(*id, ev.clone()) {
                    failed.push(format!("{kind:?}: `{name}` — {e}"));
                }
            }
        }
        failed.sort();
        failed
    }

    fn push(&mut self, kind: SessionEventKind, handler: Handler) {
        self.by_kind.entry(kind).or_default().push(handler);
    }

    /// Run every handler registered for the event's kind, in registration order, each with the
    /// event (cloned for all but the last).
    fn run(&self, world: &mut World, ev: SessionEvent) {
        let Some(list) = self.by_kind.get(&SessionEventKind::from(&ev)) else {
            return;
        };
        let Some(((last_id, last_name), rest)) = list.split_last() else {
            return;
        };
        for (id, name) in rest {
            call(world, *id, name, ev.clone());
        }
        call(world, *last_id, last_name, ev);
    }
}

fn call(world: &mut World, id: SystemId<In<SessionEvent>>, name: &str, ev: SessionEvent) {
    if let Err(e) = world.run_system_with(id, ev) {
        // A registered handler that cannot run is a bug in the registering plugin (a missing
        // resource, most likely), never the wire's fault — loud, so the smoke gate sees it.
        error!("net: handler `{name}` did not run: {e}");
    }
}

/// Registering a packet handler from a plugin: `app.net_handler(SessionEventKind::X, on_x)`,
/// where `on_x` is an ordinary system taking `In<SessionEvent>` and whatever it needs. Several
/// handlers may answer one kind; they run in registration order.
pub(crate) trait NetHandlerApp {
    fn net_handler<M>(
        &mut self,
        kind: SessionEventKind,
        handler: impl IntoSystem<In<SessionEvent>, (), M> + 'static,
    ) -> &mut Self;
}

impl NetHandlerApp for App {
    fn net_handler<M>(
        &mut self,
        kind: SessionEventKind,
        handler: impl IntoSystem<In<SessionEvent>, (), M> + 'static,
    ) -> &mut Self {
        let name = std::any::type_name_of_val(&handler);
        let id = self.world_mut().register_system(handler);
        self.world_mut()
            .get_resource_or_init::<NetHandlers>()
            .push(kind, (id, name));
        self
    }
}

/// One drain's dispatch, **in wire order**: every event runs its handlers in place, each
/// handler's commands applied before the next event's.
pub(crate) fn dispatch(world: &mut World, events: Vec<SessionEvent>) {
    let handlers = world.remove_resource::<NetHandlers>().unwrap_or_default();
    for ev in events {
        handlers.run(world, ev);
    }
    world.insert_resource(handlers);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What ran, in order — a handler's name and the event it saw.
    #[derive(Resource, Default)]
    struct Log(Vec<String>);

    fn on_queued(In(ev): In<SessionEvent>, mut log: ResMut<Log>) {
        let SessionEvent::LoginQueued { position, .. } = ev else {
            panic!("the table routes by kind");
        };
        log.0.push(format!("queued:{}", position.unwrap_or(0)));
    }

    fn on_logged_out(In(ev): In<SessionEvent>, mut log: ResMut<Log>) {
        assert!(matches!(ev, SessionEvent::LoggedOut));
        log.0.push("logged_out".into());
    }

    fn on_logged_out_too(In(_): In<SessionEvent>, mut log: ResMut<Log>) {
        log.0.push("logged_out_too".into());
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).init_resource::<Log>();
        app
    }

    fn queued(position: u32) -> SessionEvent {
        SessionEvent::LoginQueued {
            position: Some(position),
            realm: None,
        }
    }

    fn run(app: &mut App, events: Vec<SessionEvent>) -> Vec<String> {
        dispatch(app.world_mut(), events);
        std::mem::take(&mut app.world_mut().resource_mut::<Log>().0)
    }

    #[test]
    fn events_run_in_wire_order_and_an_unhandled_kind_is_skipped() {
        let mut app = app();
        app.net_handler(SessionEventKind::LoginQueued, on_queued)
            .net_handler(SessionEventKind::LoggedOut, on_logged_out);
        let stage = || SessionEvent::LoginStage {
            stage: benilla_protocol::LoginStage::Connecting,
        };
        let log = run(
            &mut app,
            vec![
                queued(1),
                stage(),
                SessionEvent::LoggedOut,
                queued(2),
                stage(),
            ],
        );
        assert_eq!(log, vec!["queued:1", "logged_out", "queued:2"]);
    }

    #[test]
    fn several_handlers_on_one_kind_run_in_registration_order_each_with_the_event() {
        let mut app = app();
        app.net_handler(SessionEventKind::LoggedOut, on_logged_out)
            .net_handler(SessionEventKind::LoggedOut, on_logged_out_too);
        let log = run(&mut app, vec![SessionEvent::LoggedOut]);
        assert_eq!(log, vec!["logged_out", "logged_out_too"]);
        assert_eq!(
            app.world().resource::<NetHandlers>().census()[&SessionEventKind::LoggedOut].len(),
            2
        );
    }

    /// **Every kind has a handler**, read off the built client. A kind with none is a packet the
    /// client decodes and then drops on the floor — the reference discards an unregistered
    /// opcode in silence; this says so at test time.
    #[test]
    fn every_session_event_kind_has_one_owner() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let table = app.world_mut().resource::<NetHandlers>().census();
        let unowned: Vec<String> = SessionEventKind::all()
            .filter(|k| !table.contains_key(k))
            .map(|k| format!("{k:?}: decoded and dropped — no handler in the table"))
            .collect();
        eprintln!(
            "session event kinds: {} — {} in the handler table ({} handlers)",
            SessionEventKind::all().count(),
            table.len(),
            table.values().map(Vec::len).sum::<usize>(),
        );
        assert!(unowned.is_empty(), "{}", unowned.join("\n"));
    }

    /// **Every registered handler can run on the built client.** The dispatch match's parameters
    /// were fetched every frame from the first, so a resource nobody inserted failed at boot; a
    /// handler's are fetched when its packet arrives, which for a rare packet is never in a
    /// smoke run. This fetches them all, once, on the headless client — every plugin built and
    /// finished — by running each handler with an event of a kind it ignores.
    #[test]
    fn every_registered_handler_can_run_on_the_built_client() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let handlers = world
            .remove_resource::<NetHandlers>()
            .expect("the built client registers handlers");
        let failed = handlers.probe(world, &SessionEvent::LoggedOut);
        let count: usize = handlers.census().values().map(Vec::len).sum();
        eprintln!(
            "net handlers probed on the built client: {count}, {} failed",
            failed.len()
        );
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    /// **A handler matches the kind it registered for.** `net_handler(K::X, on_y)` routes X to
    /// `on_y`, and `on_y` opens with `if let SessionEvent::X` — two spellings of one kind, which
    /// nothing else ties together: a handler registered under the wrong kind compiles, runs, and
    /// silently ignores its packet. Read off the source: every registration's handler names its
    /// kind in its body. A session-end listener takes `In(_)` and is exempt.
    #[test]
    fn every_handler_names_the_kind_it_registered_for() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).expect("src") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        let (mut seen, mut problems) = (0, Vec::new());
        for path in files {
            if path.ends_with("net/handlers.rs") {
                continue; // this file's own fixtures and prose
            }
            let text = regex_lite_strip_comments(&std::fs::read_to_string(&path).expect("source"));
            for call in text.split("net_handler(").skip(1) {
                let Some((args, _)) = call.split_once(')') else {
                    continue;
                };
                let Some((kind, handler)) = args.split_once(',') else {
                    continue;
                };
                let kind = kind.trim().rsplit("::").next().unwrap_or_default();
                // rustfmt breaks a long call one argument a line, with a trailing comma.
                let handler = handler.trim().trim_end_matches(',').trim_end();
                seen += 1;
                if kind == "Disconnected" {
                    continue;
                }
                let Some(at) = text.find(&format!("fn {handler}(")) else {
                    problems.push(format!(
                        "{}: `{handler}` is not in the file that registers it",
                        path.display()
                    ));
                    continue;
                };
                // The handler's own lines: from its `fn` to the next item's.
                let body: Vec<&str> = text[at..]
                    .lines()
                    .enumerate()
                    .take_while(|(i, l)| {
                        let l = l.trim_start();
                        *i == 0
                            || !(l.starts_with("fn ")
                                || l.starts_with("pub")
                                || l.starts_with("#["))
                    })
                    .map(|(_, l)| l)
                    .collect();
                // The whole variant name, not a prefix of one: `SessionEvent::Chat` is the head
                // of `ChatPlayerNotFound` too, so a bare `contains` let a handler registered for
                // `Chat` pass by matching a cousin (six kinds are prefixes of others).
                let names_kind = {
                    let body = body.join("\n");
                    let needle = format!("SessionEvent::{kind}");
                    body.match_indices(&needle).any(|(at, hit)| {
                        !body[at + hit.len()..]
                            .starts_with(|c: char| c.is_alphanumeric() || c == '_')
                    })
                };
                if !names_kind {
                    problems.push(format!(
                        "{}: `{handler}` is registered for {kind} and never names it",
                        path.display()
                    ));
                }
            }
        }
        let registered: usize = crate::game_plugins::schedule_tests::headless_client()
            .world()
            .resource::<NetHandlers>()
            .census()
            .values()
            .map(Vec::len)
            .sum();
        assert_eq!(
            seen, registered,
            "a registration this scan cannot read — spell it `net_handler(K::Kind, handler)`"
        );
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[derive(Resource)]
    struct Absent;

    fn needs_what_nobody_inserted(In(_): In<SessionEvent>, _absent: Res<Absent>) {}

    #[test]
    fn the_probe_names_a_handler_whose_resource_is_missing() {
        let mut app = app();
        app.net_handler(SessionEventKind::LoggedOut, on_logged_out)
            .net_handler(SessionEventKind::LoginQueued, needs_what_nobody_inserted);
        let handlers = app.world_mut().remove_resource::<NetHandlers>().unwrap();
        let failed = handlers.probe(app.world_mut(), &SessionEvent::LoggedOut);
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(
            failed[0].contains("needs_what_nobody_inserted"),
            "{failed:?}"
        );
    }

    /// `//` comments out, so a variant named in prose does not count as an arm.
    fn regex_lite_strip_comments(s: &str) -> String {
        s.lines()
            .map(|l| match l.find("//") {
                Some(i) => &l[..i],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
