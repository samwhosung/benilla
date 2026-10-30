//! The inbound handler table, the reference's shape: `NetClient` holds an opcode → handler table
//! (`+0x74`, 828 slots) and its dispatcher `0x537aa0` calls each packet's handler in packet order,
//! dropping an unregistered opcode in silence. Here each subsystem registers one-shot systems per
//! [`SessionEventKind`] through [`NetHandlerApp::net_handler`], and the exclusive drain runs them
//! in packet order, each seeing what the one before it did.

use std::collections::HashMap;

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemId;
use bevy::prelude::*;

/// One registered handler: the one-shot system and its name for the census.
type Handler = (SystemId<In<SessionEvent>>, &'static str);

/// The table, filled at plugin build and read by the drain.
#[derive(Resource, Default)]
pub(crate) struct NetHandlers {
    by_kind: HashMap<SessionEventKind, Vec<Handler>>,
    /// Every handler in registration order, for [`measure`].
    #[cfg(test)]
    registered: Vec<Handler>,
    /// Each handler's declared access, by name, in registration order, once [`measure`] ran.
    #[cfg(test)]
    accesses: Option<Vec<(&'static str, bevy::ecs::query::FilteredAccessSet)>>,
}

impl NetHandlers {
    /// Each handler's declared access, by name, in registration order, for the tests that ask what
    /// the handlers write. Panics before [`measure`].
    #[cfg(test)]
    pub(crate) fn accesses(&self) -> &[(&'static str, bevy::ecs::query::FilteredAccessSet)] {
        self.accesses
            .as_deref()
            .expect("the handlers' accesses are read after `net::handlers::measure`")
    }

    /// Every kind with a handler, and the handlers' names in registration order.
    #[cfg(test)]
    pub(crate) fn census(&self) -> std::collections::BTreeMap<SessionEventKind, Vec<&'static str>> {
        self.by_kind
            .iter()
            .map(|(k, v)| (*k, v.iter().map(|(_, n)| *n).collect()))
            .collect()
    }

    /// Runs every handler once with `ev`, whatever its kind, and names those that could not run:
    /// a wrong-kind event still fetches the system's parameters.
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

    fn push(&mut self, kind: SessionEventKind, handler: Handler, ahead: bool) {
        let list = self.by_kind.entry(kind).or_default();
        if ahead {
            list.insert(0, handler);
        } else {
            list.push(handler);
        }
    }

    /// Runs the event's handlers in registration order.
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
        // A plugin bug (most likely a missing resource), logged loud so the smoke gate sees it.
        error!("net: handler `{name}` did not run: {e}");
    }
    // The unit edits a handler's seams queued, applied with its commands, before the next event.
    crate::creature_anim::apply_unit_edits(world);
}

/// `app.net_handler(SessionEventKind::X, on_x)` registers a system taking `In<SessionEvent>`;
/// several handlers on one kind run in registration order.
pub(crate) trait NetHandlerApp {
    fn net_handler<M>(
        &mut self,
        kind: SessionEventKind,
        handler: impl IntoSystem<In<SessionEvent>, (), M> + 'static,
    ) -> &mut Self;

    /// [`Self::net_handler`], run ahead of the kind's handlers registered before it: the half of
    /// a reference handler that comes before another subsystem's half, whatever the plugin order.
    fn net_handler_ahead<M>(
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
        register(self, kind, handler, false)
    }

    fn net_handler_ahead<M>(
        &mut self,
        kind: SessionEventKind,
        handler: impl IntoSystem<In<SessionEvent>, (), M> + 'static,
    ) -> &mut Self {
        register(self, kind, handler, true)
    }
}

fn register<M>(
    app: &mut App,
    kind: SessionEventKind,
    handler: impl IntoSystem<In<SessionEvent>, (), M> + 'static,
    ahead: bool,
) -> &mut App {
    let name = std::any::type_name_of_val(&handler);
    let id = app.world_mut().register_system(handler);
    let mut handlers = app.world_mut().get_resource_or_init::<NetHandlers>();
    handlers.push(kind, (id, name), ahead);
    #[cfg(test)]
    handlers.registered.push((id, name));
    app
}

/// Reads every handler's declared access, for [`NetHandlers::accesses`]: each is initialized, its
/// access kept, and registered again, where its first run initializes it again on the same state.
/// Done on a test's asking, not at registration: a handler that lends the call host is most of a
/// plugin's build to initialize, and most tests run few handlers or none.
#[cfg(test)]
pub(crate) fn measure(world: &mut World) {
    world.resource_scope(|world, mut handlers: Mut<NetHandlers>| {
        if handlers.accesses.is_some() {
            return;
        }
        let mut accesses = Vec::new();
        for i in 0..handlers.registered.len() {
            let (id, name) = handlers.registered[i];
            let mut system = world
                .unregister_system(id)
                .expect("a registered handler")
                .system();
            accesses.push((name, system.initialize(world)));
            let again = world.register_boxed_system(system);
            handlers.registered[i].0 = again;
            for list in handlers.by_kind.values_mut() {
                for entry in list.iter_mut().filter(|(at, _)| *at == id) {
                    entry.0 = again;
                }
            }
        }
        handlers.accesses = Some(accesses);
    });
}

/// What runs after each packet of a dispatch, its handlers done, with a stamp taken ahead of
/// them, so a state the packet moved can be logged ahead of what the packet raised
/// ([`crate::lanes`]). A log's start is read where the span of packets begins
/// ([`crate::lanes::LaneApp::add_span_start`]).
type PacketEnd = fn(&mut World, crate::lanes::Stamp);

/// Every [`PacketEnd`], in registration order.
#[derive(Resource, Default, Clone)]
struct PacketEnds(Vec<PacketEnd>);

/// Register a [`PacketEnd`].
pub(crate) fn add_packet_end(app: &mut App, end: PacketEnd) {
    app.world_mut()
        .get_resource_or_init::<PacketEnds>()
        .0
        .push(end);
}

/// One drain's dispatch in wire order, each handler's commands applied before the next event,
/// and each packet's lanes fired once its handlers are done ([`crate::lanes::flush`]): the
/// reference's poll runs each packet's handler whole before the next (`0x5384d0`, `0x537ae5`),
/// and what a handler signals runs every listener before the signal returns (`0x703e50`). What
/// the input records and the last tick stamped fires first, as the window input runs ahead of the
/// packets (`0x420d23`, `0x420d55`); the span's starts follow, and each packet's ends after its
/// handlers.
pub(crate) fn dispatch(world: &mut World, events: Vec<SessionEvent>) {
    if events.is_empty() {
        return;
    }
    let handlers = world.remove_resource::<NetHandlers>().unwrap_or_default();
    let ends = world
        .get_resource::<PacketEnds>()
        .cloned()
        .unwrap_or_default();
    if !crate::lanes::flush(world) {
        crate::lanes::span_starts(world);
    }
    crate::lanes::at_packets(|| {
        for ev in events {
            let last = ends_its_packet(&ev);
            let at = (!ends.0.is_empty()).then(crate::lanes::stamp);
            handlers.run(world, ev);
            if let Some(at) = at {
                for end in &ends.0 {
                    end(world, at);
                }
            }
            if last {
                crate::lanes::flush(world);
            }
        }
    });
    world.insert_resource(handlers);
}

/// Whether `ev` is the last event its packet decodes to (`benilla_protocol::decode`): an update
/// packet's blocks run until its [`SessionEvent::ObjectUpdateEnd`], as its handler writes every
/// block before its notify pass (`0x46520e`, `0x465222`, `0x465277`), a `MSG_MOVE_SET_*_SPEED`
/// decodes to its pose, the one [`benilla_protocol::messages::RelayVerb::Pose`] move, then its
/// speed, and a saved store's answer sends its bytes ahead of its record
/// ([`crate::net::io::inbound`]).
pub(crate) fn ends_its_packet(ev: &SessionEvent) -> bool {
    !matches!(
        ev,
        SessionEvent::SavedAnswer { .. }
            | SessionEvent::ObjectCreate { .. }
            | SessionEvent::ItemCreate { .. }
            | SessionEvent::ObjectValues { .. }
            | SessionEvent::ObjectsRemoved(_)
            | SessionEvent::ObjectsNear(_)
            | SessionEvent::ObjectMove { .. }
            | SessionEvent::ObjectNotify { .. }
            | SessionEvent::UnitMove {
                verb: benilla_protocol::messages::RelayVerb::Pose,
                ..
            }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What ran, in order.
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

    #[test]
    fn a_handler_registered_ahead_runs_before_the_kinds_earlier_ones() {
        let mut app = app();
        app.net_handler(SessionEventKind::LoggedOut, on_logged_out)
            .net_handler_ahead(SessionEventKind::LoggedOut, on_logged_out_too);
        let log = run(&mut app, vec![SessionEvent::LoggedOut]);
        assert_eq!(log, vec!["logged_out_too", "logged_out"]);
    }

    /// A kind with no handler is a decoded packet dropped in silence, as the reference does.
    #[test]
    fn every_session_event_kind_has_one_owner() {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
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

    /// A handler's parameters are fetched only when its packet arrives, which for a rare packet is
    /// never in a smoke run; this fetches them all once, with an event each handler ignores.
    #[test]
    fn every_registered_handler_can_run_on_the_built_client() {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
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

    /// A handler registered under the wrong kind compiles and silently ignores its packet, so each
    /// handler must name `SessionEvent::<Kind>` in its body; a session-end listener is exempt.
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
            // A test file's registrations build no client, so the census below never counts them.
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name == "tests.rs"
                || name.ends_with("_tests.rs")
                || path.components().any(|c| c.as_os_str() == "tests")
            {
                continue;
            }
            let text = regex_lite_strip_comments(&std::fs::read_to_string(&path).expect("source"))
                .replace("net_handler_ahead(", "net_handler(");
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
                // The whole variant name: `Chat` is also a prefix of `ChatPlayerNotFound`.
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
        let registered: usize = crate::game_plugins::schedule_tests::headless_client_unrun()
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

    /// Each packet's last event alone ends it: an update packet's blocks run to its
    /// `ObjectUpdateEnd`, and a speed change's pose to its speed (`benilla_protocol::decode`), so
    /// the dispatch fires the lanes between packets, never between one packet's events.
    #[test]
    fn only_a_packets_last_event_ends_it() {
        use benilla_protocol::messages::{opcode, parse_server};
        let hex = |s: &str| -> Vec<u8> {
            s.as_bytes()
                .chunks(2)
                .map(|h| u8::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap())
                .collect()
        };
        // Two values blocks, the first a first-pass guid's, which leaves its notify place.
        let update = hex("020000000000814260010800000013000000\
            0001aa010800000012000000");
        let mut speed = hex("012a");
        speed.extend([0; 28]);
        speed.extend(7.0f32.to_le_bytes());
        for (op, body) in [
            (opcode::SMSG_UPDATE_OBJECT, update),
            (opcode::MSG_MOVE_SET_RUN_SPEED, speed),
            (opcode::SMSG_LOGOUT_COMPLETE, Vec::new()),
        ] {
            let events = benilla_protocol::decode(parse_server(op, &body).unwrap());
            let ends: Vec<bool> = events.iter().map(ends_its_packet).collect();
            let mut want = vec![false; ends.len().saturating_sub(1)];
            want.push(true);
            assert_eq!(ends, want, "{op:#06x}: {events:?}");
        }
    }

    fn raises_in_a_handler(_: In<SessionEvent>, world: &mut World) {
        let mut ui = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
        ui.fire_event("PLAYER_MONEY", Vec::new());
    }

    fn raises_at_a_packets_end(world: &mut World, _: crate::lanes::Stamp) {
        let mut ui = world.non_send_resource_mut::<benilla_ui::script::UiScript>();
        ui.fire_event("PLAYER_MONEY", Vec::new());
    }

    /// One packet's dispatch, a pong, in each frame.
    fn dispatch_a_pong(world: &mut World) {
        dispatch(world, vec![SessionEvent::Pong { sequence: 0 }]);
    }

    /// The raise check armed over a frame that dispatches a pong.
    fn armed() -> App {
        let mut app = App::new();
        crate::ui_script::raise_origin::arm_each_frame(&mut app);
        app.insert_non_send_resource(benilla_ui::script::UiScript::new().expect("a VM"))
            .add_systems(Update, dispatch_a_pong);
        app
    }

    /// A handler's raise inside the dispatch passes, its packet's own (`0x703e50`); a packet end's,
    /// which runs in the dispatch outside every handler and lane firing, fails.
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "the check compiles out")]
    fn a_handlers_raise_passes_and_a_packet_ends_fails() {
        let mut handled = armed();
        handled.net_handler(SessionEventKind::Pong, raises_in_a_handler);
        handled.update();
        let mut ended = armed();
        add_packet_end(&mut ended, raises_at_a_packets_end);
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ended.update()))
            .expect_err("the packet end's raise passed");
        let message = failed.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(message.contains("`PLAYER_MONEY`"), "{message}");
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
