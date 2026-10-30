//! What a listener inside the net dispatch still reads or hears out of its packet's place. The
//! dispatch fires each packet's lanes after its handlers, with the host lent
//! ([`crate::lanes::flush`]), as the reference's handler signals inside itself (`0x703e50`) and
//! runs whole before the next packet's (`0x537ae5`), so a listener reads the live state its
//! packet left and its sends go out in packet order. Four lists hold what is left, each the whole
//! of its remaining work and only shrinking; [`what_the_flip_left`] prints where they stand.
//!
//! - [`super::guard_tests::READS_FRAME_FINAL`], held here to its count: the systems, in any
//!   schedule or set, that push a view of what a handler writes, which a getter reads in place of
//!   the live state, so a listener reads the push the last frame left. A family's getters going
//!   live ([`crate::script_calls::live`]) drops its feed's row; a build the reference too makes
//!   outside every packet is
//!   [`super::guard_tests::BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE`]'s, and a system that
//!   writes the VM and pushes no view is [`super::guard_tests::WRITES_NO_VIEW`]'s; both stay.
//! - [`BETWEEN_THE_DISPATCH_AND_THE_DRAIN`]: the systems after the dispatch and ahead of the drain
//!   that write what a call or a handler reads inside a packet (the host's live reads, the rest of
//!   the call host's state, what the lanes' firings touch, and what the handlers read without
//!   writing), where the reference
//!   writes inside a packet: a listener reads each as the frame before left it. The writers the
//!   reference too runs after the packets, and those no listener can see, are
//!   [`AFTER_THE_PACKETS_IN_THE_REFERENCE`]'s, which stays.
//! - [`super::guard_tests::NOT_YET_ON_A_LANE`]: what fires off a handler's queue or edge from a
//!   feed, behind every packet event of its frame.
//! - [`super::guard_tests::NOT_YET_PLACED`]: the systems outside the phase that fire off a
//!   handler queue in system order.
//!
//! Every entry of [`super::guard_tests::HOST_HELD_UNREAD`] is read first, as quiet or as a row of
//! its own. [`super::guard_tests::QUEUED_FOR_THE_TICK`] holds the calls that queue an event for
//! the VM's next tick, which fires behind every packet's events.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use bevy::ecs::component::ComponentId;
use bevy::ecs::query::{ComponentAccessKind, FilteredAccess};
use bevy::ecs::schedule::SystemKey;
use bevy::prelude::*;

use crate::game_plugins::schedule_tests::{census, type_names_available, SyncPoints};

/// The systems that may run after the dispatch (`apply_net_updates`) and run ahead of the drain
/// (`fire_in_order`), by a suffix of the system's name, with what each writes of the state a
/// listener's call or a handler reads inside a packet, by its name in the crate, and why: the
/// host's live reads ([`crate::script_calls::live::LiveReads`]), the rest of the call host's
/// state, which its verbs read, what each lane's firing touches as it fires ([`super::LaneReads`]),
/// and what the handlers read without writing
/// ([`super::guard_tests::READ_ONLY_TO_HANDLERS`]), the lanes and the message buffers apart; a
/// read of what the handlers fill through a shared reference
/// ([`super::guard_tests::FILLED_THROUGH_A_READ`]) counts as the write it is. Each is a write
/// the reference makes inside a packet, or a view pushed in its place, which a listener fired
/// inside the dispatch reads as the frame before left it. The writers the reference also runs after the packets are
/// [`AFTER_THE_PACKETS_IN_THE_REFERENCE`]'s, not these. `Commands` names a component a system
/// filters out and may add with its commands, which access does not show; `exclusive` a system
/// whose access Bevy does not report.
#[rustfmt::skip]
const BETWEEN_THE_DISPATCH_AND_THE_DRAIN: &[(&str, &[&str], &str)] = &[];

/// The systems between the dispatch and the drain that write what the host's live reads read,
/// as [`BETWEEN_THE_DISPATCH_AND_THE_DRAIN`]'s, but where the reference too runs them after the
/// packets, or where no listener can read them: they stay, and a listener fired inside the
/// dispatch reads them as the frame before left them, as 1.12.1's does. Same shape,
/// held to its systems' access by the same test.
#[rustfmt::skip]
const AFTER_THE_PACKETS_IN_THE_REFERENCE: &[(&str, &[&str], &str)] = &[
    ("::net::motion::facing::drive_display_facing", &["Commands net::motion::remote::RemoteMotion",
        "Commands net::motion::spline::Spline", "Transform"],
        "turns the units' models toward their facing; unit movement is \
         frame time, which the reference runs after the packets (`0x420d63` after `0x420d55`)"),
    ("::net::motion::spline::ground_clamp_creatures", &["Transform"],
        "settles the creatures on the ground; unit movement is \
         frame time, which the reference runs after the packets (`0x420d63` after `0x420d55`)"),
    ("::net::motion::remote::extrapolate_remote_units", &["Commands net::motion::spline::Spline",
        "Transform", "net::motion::remote::RemoteMotion"],
        "dead-reckons the remote players between their packets in the movement tick (event 5's \
         `0x616800` → `0x615b10`), frame time, which the reference runs after the packets \
         (`0x420d63` after `0x420d55`); a due packet seats its unit in the handler \
         (`0x7c6420` at `0x618dfc`)"),
    ("::transport::compose_riders", &["Transform", "net::motion::remote::RemoteMotion"],
        "carries the riders with their decks, which move in the movement tick (event 5's \
         `0x616800` → `0x630970`), frame time, which the reference runs after the packets \
         (`0x420d63` after `0x420d55`): a rider's position reads through its deck at the call \
         (`0x5f1f10` → `0x7c4930`); a packet seats its rider in the handler (`0x618b30` → \
         `0x7c6900`, at `0x618e07`)"),
    ("::net::motion::remote::drain_pending_moves", &["Commands net::motion::spline::Spline",
        "net::motion::remote::MoveTick", "net::motion::remote::RemoteMotion"],
        "fires the relayed moves queued past their arrival in the movement tick (event 5's \
         `0x616800` → `0x615b10` → `0x616620` → `0x615c30`), then stamps the clock the next \
         frame's arrivals are due by (`0x616846`), frame time, which the reference runs after \
         the packets (`0x420d63` after `0x420d55`)"),
    ("::Collider>", &["ColliderTrees"],
        "refits the collision world's trees to the colliders frame time moved, the decks among \
         them, which the reference moves in the movement tick (event 5's `0x616800` → \
         `0x630970`), after the packets (`0x420d63` after `0x420d55`): a due arrival's catch-up \
         resolves against the world as that tick left it (`0x618bb0` → `0x634040`)"),
    ("::net::game_time::tick_game_time", &["net::game_time::ServerTime"],
        "steps the game clock by the frame time, the category 5 handler `0x401ec0` → `0x6c5d50` \
         → `0x6435a0`, which the reference runs after the packets (`0x420d63` after \
         `0x420d55`)"),
    ("::net::motion::spline::sample_splines", &["Commands net::motion::spline::Spline",
        "Transform"],
        "moves the units along their splines; unit movement is \
         frame time, which the reference runs after the packets (`0x420d63` after `0x420d55`)"),
    ("::transport::ground_deck_riders", &["Transform", "net::motion::remote::RemoteMotion"],
        "settles the riders on their decks; unit movement is \
         frame time, which the reference runs after the packets (`0x420d63` after `0x420d55`)"),
    ("::transport::tick_transports", &["Transform"],
        "moves the transports along their paths; unit movement is \
         frame time, which the reference runs after the packets (`0x420d63` after `0x420d55`)"),
    ("::ui_action::cooldown_lane::drop_cooldown_log<benilla_app::spell::cooldowns::Cooldowns>",
        &["spell::cooldowns::Cooldowns"],
        "drops the player's cooldown log, run only while no in-game interface \
         is up, so no listener exists; it goes with the log"),
    ("::ui_action::cooldown_lane::drop_cooldown_log<benilla_app::ui_pet::PetBar>",
        &["ui_pet::PetBar"],
        "drops the pet's cooldown log, run only while no in-game interface \
         is up, so no listener exists; it goes with the log"),
    ("::ui_cast::views::drop_cast_views", &["ui_cast::views::CastViews"],
        "drops the cast views' log, run only while no in-game interface \
         is up, so no listener exists; it goes with the log"),
    ("::ui_chat::language::load_language_words", &["ui_chat::language::ChatLanguages"],
        "loads `LanguageWords.dbc` once, the first frame the install is open, ahead of every \
         world packet, as the reference loads it at startup (`StaticDBLoadAll` `0x53f8b0` → \
         `0x548990`, from `ClientDBInitialize` `0x53f4f0`): no listener reads the pools before \
         it"),
    ("::ui_mail::step_pending_mail", &["ui_mail::pending::MailPending"],
        "steps the pending-mail time in the world frame's update (its slot `0x38`, `0x4818d0` → \
         `0x4ade60`), which the frame pass runs after the packets (`0x420d6a` after `0x420d55`)"),
    ("::ui_loot::drop_loot_steps", &["ui_loot::LootState"],
        "drops the loot lane's window signals, its pushes kept, run only while no in-game \
         interface is up, so no listener exists"),
    ("::ui_loot_roll::lane::drop_roll_events", &["ui_loot_roll::LootRolls"],
        "drops the rolls' lane, run only while no in-game interface is up, so no listener \
         exists"),
    ("::ui_mail::lane::drop_mail_events", &["ui_mail::pending::MailPending"],
        "drops the mail lane, a parked close's tail with its query kept, run only while no \
         in-game interface is up, so no listener exists"),
    ("::ui_reputation::drop_reputation_log", &["net::reputations::Reputations"],
        "drops the reputation log, run only while no in-game interface \
         is up, so no listener exists; it goes with the log"),
    ("::ui_unit::views::drop_unit_views", &["ui_unit::views::UnitViews"],
        "drops the unit views' log, run only while no in-game interface \
         is up, so no listener exists; it goes with the log"),
    ("::net::send_query_time", &["net::QueryTimeAsks"],
        "the clock's hourly resync, which writes only its own asks' bookkeeping, seated by the \
         world-enter cascade's ask (`0x4de45b`): no listener reads it"),
    ("::poi_marker::expire_marker", &["poi_marker::PoiMarker"],
        "clears the directions marker on arrival or at its deadline in the minimap's per-frame \
         update (`0x6d93a0`, its clears at `0x6d93fb` and `0x6d9a3a`), which the minimap frame's \
         draw runs (`0x4ebea0` → `0x4ec950`) in the frame pass, after the packets (`0x420d6a` \
         after `0x420d55`)"),
    ("::ui_battlefield::drive_battlefield_leash", &["ui_battlefield::BattlefieldLeash"],
        "the list's leash (`0x4aa090`), polled in the world frame's update (`0x4818d5`), which \
         the frame pass runs after the packets (`0x420d6a` after `0x420d55`)"),
    ("::ui_chat::language::feed_default_language", &["ui_chat::frames::ChatWindows"],
        "copies the faction tongue into the log files' composition (`route`'s tee), which no \
         listener reads: the stock `ChatFrame_OnEvent` holds its own `this.defaultLanguage`"),
];

/// The systems [`BETWEEN_THE_DISPATCH_AND_THE_DRAIN`] and [`AFTER_THE_PACKETS_IN_THE_REFERENCE`]
/// hold to, with what each writes of the state a call or a handler reads, by suffix; see the
/// first's doc for that state and the kinds.
fn writers_between(app: &mut App) -> Option<BTreeMap<String, BTreeSet<String>>> {
    let world = app.world_mut();
    use bevy::ecs::system::System;
    // What the getters read as they run: every live family's access.
    let mut live = IntoSystem::into_system(|_: crate::script_calls::live::LiveReads| {});
    let live = System::initialize(&mut live, world)
        .filtered_accesses()
        .to_vec();
    let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
    let host = System::initialize(&mut host, world)
        .filtered_accesses()
        .to_vec();
    let mut read_resources = HashSet::new();
    let mut read_components = HashSet::new();
    // What a listener inside the dispatch reads past the live families: every resource and
    // component a call's verbs touch through the host, and what the handlers read without writing
    // ([`super::guard_tests::READ_ONLY_TO_HANDLERS`]), each a read made inside a packet in the
    // reference, and what the handlers fill through a shared reference, which a system writes
    // through `&self`. The lanes and the message buffers are not state.
    crate::net::handlers::measure(world);
    let queues: HashSet<ComponentId> = super::guard_tests::lane_types()
        .into_iter()
        .filter_map(|t| world.components().get_resource_id(t))
        .collect();
    let is_state = |world: &World, id: ComponentId| {
        !queues.contains(&id) && !super::guard_tests::is_messages(world, id)
    };
    // What the handlers and the host fill through a shared reference: a system's read of one is
    // a write, which its access shows as a read.
    let through_a_read = |world: &World, id: ComponentId| {
        super::guard_tests::filled_through_a_read(&super::guard_tests::name(world, id)).is_some()
    };
    let handlers = world.resource::<crate::net::handlers::NetHandlers>();
    let mut handler_reads = HashSet::new();
    for (_, access) in handlers.accesses() {
        let access = access.combined_access();
        handler_reads.extend(
            access
                .resource_reads()
                .filter(|id| !access.has_resource_write(*id)),
        );
    }
    read_resources.extend(handler_reads.into_iter().filter(|id| is_state(world, *id)));
    for fa in &host {
        read_resources.extend(
            fa.access()
                .resource_reads_and_writes()
                .filter(|id| is_state(world, *id)),
        );
        if let Ok(kinds) = fa.access().try_iter_component_access() {
            read_components.extend(kinds.map(|k| match k {
                ComponentAccessKind::Archetypal(id)
                | ComponentAccessKind::Shared(id)
                | ComponentAccessKind::Exclusive(id) => id,
            }));
        }
    }
    // What each lane's firing touches as it fires, inside its packet in the reference: its
    // declared access ([`super::LaneReads`] for an exclusive one), less the call host's it lends,
    // counted above.
    super::measure(world);
    let lanes: Vec<Vec<FilteredAccess>> = world
        .resource::<super::Lanes>()
        .accesses()
        .into_iter()
        .map(|set| set.filtered_accesses().to_vec())
        .collect();
    for set in &lanes {
        let mut own: Vec<&FilteredAccess> = set.iter().collect();
        for h in &host {
            if let Some(i) = own.iter().position(|o| *o == h) {
                own.remove(i);
            }
        }
        for fa in own {
            read_resources.extend(
                fa.access()
                    .resource_reads_and_writes()
                    .filter(|id| is_state(world, *id)),
            );
            if let Ok(kinds) = fa.access().try_iter_component_access() {
                read_components.extend(kinds.map(|k| match k {
                    ComponentAccessKind::Archetypal(id)
                    | ComponentAccessKind::Shared(id)
                    | ComponentAccessKind::Exclusive(id) => id,
                }));
            }
        }
    }
    for fa in &live {
        read_resources.extend(fa.access().resource_reads_and_writes());
        if let Ok(kinds) = fa.access().try_iter_component_access() {
            read_components.extend(kinds.map(|k| match k {
                ComponentAccessKind::Archetypal(id)
                | ComponentAccessKind::Shared(id)
                | ComponentAccessKind::Exclusive(id) => id,
            }));
        }
        read_components.extend(fa.with_filters().chain(fa.without_filters()));
    }
    // Bevy's default filter, which every query carries.
    if let Some(id) = world
        .components()
        .get_id(TypeId::of::<bevy::ecs::entity_disabling::Disabled>())
    {
        read_components.remove(&id);
    }
    let vm = world
        .components()
        .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
    // Each system's access, read before the census builds the schedule.
    type Seen = (Vec<FilteredAccess>, bool, bool);
    let systems: HashMap<SystemKey, Seen> = world.schedule_scope(Update, |world, schedule| {
        schedule.graph_mut().systems.initialize(world);
        let graph = schedule.graph();
        graph
            .systems
            .iter()
            .filter_map(|(k, system, _)| {
                let access = graph.systems.get(k)?.access.filtered_accesses().to_vec();
                Some((k, (access, system.is_exclusive(), system.has_deferred())))
            })
            .collect()
    });
    let c = census(app, Update, SyncPoints::Declared);
    if !type_names_available(&c) {
        return None;
    }
    let one = |suffix: &str| {
        let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
        match (hits.next(), hits.next()) {
            (Some((k, _)), None) => *k,
            _ => panic!("exactly one system named `…{suffix}` in Update"),
        }
    };
    let (dispatch, drain) = (
        one("::net::apply::apply_net_updates"),
        one("::lanes::fire_in_order"),
    );
    let mut prev: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
    for (a, b) in &c.dependencies {
        prev.entry(*b).or_default().push(*a);
    }
    let ahead_of = |start: SystemKey| {
        let mut seen = HashSet::new();
        let mut stack = vec![start];
        while let Some(n) = stack.pop() {
            for m in prev.get(&n).into_iter().flatten() {
                if seen.insert(*m) {
                    stack.push(*m);
                }
            }
        }
        seen
    };
    let (before_dispatch, before_drain) = (ahead_of(dispatch), ahead_of(drain));
    let world = app.world();
    let name =
        |id: ComponentId| super::guard_tests::short_name(&super::guard_tests::name(world, id));
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (k, (access, exclusive, deferred)) in &systems {
        if *k == dispatch || before_dispatch.contains(k) || !before_drain.contains(k) {
            continue;
        }
        let info = &c.systems[k];
        let mut writes = BTreeSet::new();
        if *exclusive {
            writes.insert("exclusive".to_string());
        }
        let mut own: Vec<&FilteredAccess> = access.iter().collect();
        // A system that lends the host holds the host's access beside its own.
        if c.hosted.contains(k) {
            for h in &host {
                if let Some(i) = own.iter().position(|o| *o == h) {
                    own.remove(i);
                }
            }
        }
        for fa in own {
            let a = fa.access();
            writes.extend(
                a.resource_writes()
                    .chain(a.resource_reads().filter(|id| through_a_read(world, *id)))
                    .filter(|id| read_resources.contains(id) && Some(*id) != vm)
                    .map(name),
            );
            match a.try_iter_component_access() {
                Ok(kinds) => writes.extend(
                    kinds
                        .filter_map(|k| match k {
                            ComponentAccessKind::Exclusive(id) => Some(id),
                            _ => None,
                        })
                        .filter(|id| read_components.contains(id))
                        .map(name),
                ),
                Err(_) => {
                    writes.insert("every component".to_string());
                }
            }
            // A `Without` the commands of a system may insert, the shape of a tag's pass.
            if *deferred {
                writes.extend(
                    fa.without_filters()
                        .filter(|id| read_components.contains(id))
                        .map(|id| format!("Commands {}", name(id))),
                );
            }
        }
        if !writes.is_empty() {
            let full = &info.name;
            found.insert(super::guard_tests::short_name(full), writes);
        }
    }
    Some(found)
}

/// Every row between the dispatch and the drain: the obstacles, then the writers that stay.
fn rows_between(
) -> impl Iterator<Item = &'static (&'static str, &'static [&'static str], &'static str)> {
    BETWEEN_THE_DISPATCH_AND_THE_DRAIN
        .iter()
        .chain(AFTER_THE_PACKETS_IN_THE_REFERENCE)
}

/// Every system between the dispatch and the drain that writes what a call or a handler reads has
/// its row, in one of the two lists, as it stands, and a row whose system no longer does is stale:
/// a feed that rewrites once a frame what a listener's call reads inside its packet cannot hide.
#[test]
fn every_writer_between_the_dispatch_and_the_drain_is_listed() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let Some(found) = writers_between(&mut app) else {
        eprintln!("skipped: this build carries no type names");
        return;
    };
    let twice: Vec<&str> = BETWEEN_THE_DISPATCH_AND_THE_DRAIN
        .iter()
        .filter(|(s, _, _)| {
            AFTER_THE_PACKETS_IN_THE_REFERENCE
                .iter()
                .any(|(a, _, _)| a == s)
        })
        .map(|(s, _, _)| *s)
        .collect();
    assert!(twice.is_empty(), "rows in both lists: {twice:?}");
    let row = |system: &str| rows_between().find(|(s, _, _)| format!("::{system}").ends_with(s));
    let wrong: Vec<String> = found
        .iter()
        .filter(|(system, writes)| {
            row(system).is_none_or(|(_, listed, _)| {
                listed
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<BTreeSet<_>>()
                    != **writes
            })
        })
        .map(|(system, writes)| {
            let writes: Vec<String> = writes.iter().map(|w| format!("{w:?}")).collect();
            format!("(\"::{system}\", &[{}], \"\"),", writes.join(", "))
        })
        .collect();
    let stale: Vec<&str> = rows_between()
        .filter(|(s, _, _)| {
            !found
                .keys()
                .any(|system| format!("::{system}").ends_with(s))
        })
        .map(|(s, _, _)| *s)
        .collect();
    assert!(
        wrong.is_empty(),
        "these run between the dispatch and the drain and write what a call or a handler reads: \
         move the write into its handler, or give each its row as it stands: in \
         AFTER_THE_PACKETS_IN_THE_REFERENCE when the reference too writes it after the packets, \
         else in BETWEEN_THE_DISPATCH_AND_THE_DRAIN:\n  {}",
        wrong.join("\n  ")
    );
    assert!(stale.is_empty(), "stale rows: {stale:?}");
    let empty: Vec<&str> = rows_between()
        .filter(|(_, _, why)| why.is_empty())
        .map(|(s, _, _)| *s)
        .collect();
    assert!(empty.is_empty(), "rows with no reason: {empty:?}");
}

/// The frame-final views are gone: [`super::guard_tests::READS_FRAME_FINAL`] holds no row, and a
/// view of a handler's state reads it live through the host instead.
#[test]
fn no_view_reads_frame_final_state() {
    let rows: Vec<&str> = super::guard_tests::READS_FRAME_FINAL
        .iter()
        .map(|(feed, _)| *feed)
        .collect();
    assert!(
        rows.is_empty(),
        "READS_FRAME_FINAL holds {rows:?}: read the new state live through the host instead"
    );
}

/// What the flip left: every list's count, printed, failing nothing (`cargo test
/// what_the_flip_left -- --nocapture`). The first four are the remaining work; the rest are
/// context.
#[test]
fn what_the_flip_left() {
    use super::guard_tests as g;
    let inputs: usize = g::READS_FRAME_FINAL.iter().map(|(_, i)| i.len()).sum();
    let left = [
        (
            "READS_FRAME_FINAL",
            g::READS_FRAME_FINAL.len(),
            format!(" ({inputs} inputs)"),
        ),
        (
            "BETWEEN_THE_DISPATCH_AND_THE_DRAIN",
            BETWEEN_THE_DISPATCH_AND_THE_DRAIN.len(),
            String::new(),
        ),
        (
            "NOT_YET_ON_A_LANE",
            g::NOT_YET_ON_A_LANE.len(),
            String::new(),
        ),
        ("NOT_YET_PLACED", g::NOT_YET_PLACED.len(), String::new()),
    ];
    let context = [
        (
            "HOST_HELD_UNREAD (read, each quiet; stays)",
            g::HOST_HELD_UNREAD.len(),
            String::new(),
        ),
        (
            "AFTER_THE_PACKETS_IN_THE_REFERENCE (stays)",
            AFTER_THE_PACKETS_IN_THE_REFERENCE.len(),
            String::new(),
        ),
        (
            "BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE (stays)",
            g::BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE.len(),
            String::new(),
        ),
        (
            "WRITES_NO_VIEW (stays)",
            g::WRITES_NO_VIEW.len(),
            String::new(),
        ),
        (
            "QUEUED_FOR_THE_TICK (behind every packet)",
            g::QUEUED_FOR_THE_TICK.len(),
            String::new(),
        ),
    ];
    let rows: usize = left.iter().map(|(_, n, _)| n).sum();
    let mut out = format!("after the flip: {rows} rows to empty\n");
    for (list, n, extra) in &left {
        out += &format!("  {n:>4}  {list}{extra}\n");
    }
    out += "  context:\n";
    for (list, n, extra) in &context {
        out += &format!("  {n:>4}  {list}{extra}\n");
    }
    eprint!("{out}");
}

/// No list here names a system twice, and no row lists an input twice.
#[test]
fn no_host_read_list_names_an_entry_twice() {
    use super::guard_tests::named_twice;
    let mut twice = Vec::new();
    for (list, rows) in [
        (
            "BETWEEN_THE_DISPATCH_AND_THE_DRAIN",
            BETWEEN_THE_DISPATCH_AND_THE_DRAIN,
        ),
        (
            "AFTER_THE_PACKETS_IN_THE_REFERENCE",
            AFTER_THE_PACKETS_IN_THE_REFERENCE,
        ),
    ] {
        twice.extend(named_twice(
            list,
            rows.iter().map(|(s, _, _)| s.to_string()),
        ));
        for (s, inputs, _) in rows {
            twice.extend(named_twice(
                &format!("{list} {s}"),
                inputs.iter().map(|i| i.to_string()),
            ));
        }
    }
    assert!(twice.is_empty(), "named twice:\n  {}", twice.join("\n  "));
}
