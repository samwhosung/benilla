//! The readiness guard for firing each packet's lanes inside the net dispatch, after its handlers,
//! with the host lent: the reference's handler signals inside itself (`0x703e50`) and runs whole
//! before the next packet's (`0x537ae5`), so a listener reads the live state its packet left and
//! its sends go out in packet order. That move is right once five lists are empty, each the whole
//! of its remaining work and only shrinking; [`flip_readiness`] prints where they stand.
//!
//! - [`super::guard_tests::READS_FRAME_FINAL`], held here to its count: the feeds that push a
//!   view of what a handler writes, which a getter reads in place of the live state. A family's
//!   getters going live ([`crate::script_calls::live`]) drops its feed's row.
//! - [`BETWEEN_THE_DISPATCH_AND_THE_DRAIN`]: the systems after the dispatch and ahead of the drain
//!   that write what the host's live reads read, or push a view the drain waits for, where the
//!   reference writes inside a packet. A listener fired inside the dispatch would read each as the
//!   frame before left it. The writers the reference too runs after the packets, and those no
//!   listener can see, are [`AFTER_THE_PACKETS_IN_THE_REFERENCE`]'s, which stays.
//! - [`super::guard_tests::NOT_YET_ON_A_LANE`]: what fires off a handler's queue or edge from a
//!   feed, ahead of every lane. Once the lanes fire inside the dispatch, each such event moves
//!   from ahead of every packet event of its frame to behind them all.
//! - [`super::guard_tests::NOT_YET_PLACED`]: the systems outside the phase that fire off a
//!   handler queue in system order.
//! - [`super::guard_tests::PLACED_OUTSIDE_THE_DISPATCH`]: the systems that push lane items off a
//!   placed queue's field edges after the dispatch, at the edge's earlier stamp, which the
//!   dispatch's own firing would already have passed, or at their own.
//!
//! [`super::guard_tests::LANE_READS_FRAME_FINAL`] empties at the flip itself, which makes the
//! world a lane firing reads its packet's, and every entry of
//! [`super::guard_tests::HOST_HELD_UNREAD`] is read first, as quiet or as a row of its own.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use bevy::ecs::component::ComponentId;
use bevy::ecs::query::{ComponentAccessKind, FilteredAccess};
use bevy::ecs::schedule::SystemKey;
use bevy::prelude::*;

use crate::game_plugins::schedule_tests::{census, type_names_available, SyncPoints};

/// The most rows [`super::guard_tests::READS_FRAME_FINAL`] may hold, and the most inputs across
/// them: each only falls, so lower it when a row or an input goes.
const FRAME_FINAL_FEEDS: usize = 43;
const FRAME_FINAL_INPUTS: usize = 238;

/// The systems that may run after the dispatch (`apply_net_updates`) and run ahead of the drain
/// (`fire_in_order`), by a suffix of the system's name, with what each writes of the host's live
/// reads ([`crate::script_calls::live::LiveReads`]), by its name in the crate, and why: the
/// obstacles to the flip, each a write the reference makes inside a packet, or a view pushed in
/// its place. A listener fired inside the dispatch would read each of these as the frame before
/// left it. The writers the reference also runs after the packets are
/// [`AFTER_THE_PACKETS_IN_THE_REFERENCE`]'s, not these. `UiScript`
/// names a view the drain waits for ([`crate::ui_unit::field_events::FieldEventPushes`]);
/// `Commands` names a component a system filters out and may add with its commands, which
/// access does not show; `exclusive` a system whose access Bevy does not report.
#[rustfmt::skip]
const BETWEEN_THE_DISPATCH_AND_THE_DRAIN: &[(&str, &[&str], &str)] = &[
    ("::net::apply::tag_self_player", &["Commands net::SelfPlayer"],
        "tags our created entity as the player, which the token resolver finds it by; the \
         reference makes it the active player inside its create (`0x5dea50`)"),
    ("::net::motion::remote::extrapolate_remote_units", &["Transform"],
        "the arrival snap `0x7c6420` (`0x618dfc`) is written inside the packet"),
    ("::player::wire_in::regain_control_on_arrival", &["player::state::OwnControlLost"],
        "gives control back on our own create, which the stance bar's usability walk reads \
         (`0x6e3dc3`), and pushes its signals onto the notice lane at its own place; the \
         reference raises it inside the create (`0x5deb56`), and the arrival it answers is a \
         system's message, not a handler's"),
    ("::spell::group_relation::publish_group_roster", &["spell::group_relation::GroupRoster"],
        "copies the group's roster, which the relation reads take, once the packets are in"),
    ("::spell::mods::track_class_family", &["spell::mods::SpellModifiers"],
        "keeps the class family the stance bar's usability walk prices a cost through; the \
         reference stores it inside our own create (`0x5debcc` → `0x6e6ca0`)"),
    ("::transport::compose_riders", &["Transform"],
        "the arrival snap `0x7c6420` (`0x618dfc`) is written inside the packet"),
    ("::ui_action::feed::feed_actions", &["ui_action::PlayerActions"],
        "clears the bar's re-resolve flag; the known spells the book and the talents read are \
         the learn handlers' alone"),
    ("::ui_action::state::feed_action_state", &["spell::cooldowns::Cooldowns",
        "ui_action::state::ActionStateMemory"],
        "keeps the frame's `GetTime` clock the cooldown reads map each start through, and \
         writes the list as it walks the slots"),
    ("::ui_action::unlearn::clear_unlearned_slots", &["ui_action::PlayerActions"],
        "drains the unlearned spells the bar clears; the known spells are the handlers' alone"),
    ("::ui_aura::feed_auras", &["ui_aura::AuraFeedMemory"],
        "rebuilds the raid filter's books and the world entry's player the aura walk reads, and \
         keeps the clock pair the buff timers map the duration stamps through"),
    ("::ui_battlefield::reset_on_world_enter", &["ui_battlefield::Battlefield"],
        "clears the list and the notices waiting on names at the world-enter cascade \
         (`0x4a9db0`), which the reference runs inside our own player's create (`0x4908c0`)"),
    ("::ui_battlefield_positions::reset_on_world_enter",
        &["ui_battlefield_positions::BattlefieldPositions"],
        "clears the last positions packet on world entry, ahead of any packet of the new world"),
    ("::ui_battlefield_score::reset_on_world_enter", &["ui_battlefield_score::BattlefieldScoreboard"],
        "zeroes the board at the world-enter cascade (`0x4a9db0`), which the reference runs \
         inside our own player's create (`0x4908c0`)"),
    ("::ui_char::feed_char", &["UiScript"], "a field event's listeners read its push"),
    ("::ui_dialog_verbs::meeting_stone_enter_world", &["ui_dialog_verbs::MeetingStone"],
        "resets the status text to `UNKNOWN` at the world-enter cascade (`0x490a14` → \
         `0x4c9f40`), which the reference runs inside our own player's create (`0x4908c0`); a \
         `0x295` the frame's packets rebuilt keeps its text, as the create comes first"),
    ("::ui_instance::track_instance_state", &["ui_instance::InstanceState"],
        "latches the dungeon just left once the map the transfer lands on is known, which the \
         reference latches inside the map load and the packet (`0x464ff0`, `0x49e6ac`)"),
    ("::ui_items::feed::feed_item_stats", &["items::Items"],
        "takes the item cache's fresh and dropped records as it pushes them"),
    ("::ui_items::feed::feed_random_properties", &["items::RandomProperties"],
        "writes the random-suffix table as it pushes it, once per VM"),
    ("::ui_macro::rebind_macro_spells", &["ui_action::PlayerActions"],
        "drains the superseded ranks the bar's macros take; the known spells are the handlers' \
         alone"),
    ("::ui_party::feed::feed_party", &["UiScript"], "a field event's listeners read its push"),
    ("::ui_pet::drain::arm_owed_pet_gcds", &["ui_pet::PetBar"],
        "puts the GCDs the pet's sends owe into its list, which the reference arms at the send \
         (`0x6e58fb`)"),
    ("::ui_pet::unit::feed_pet_unit", &["UiScript"], "a field event's listeners read its push"),
    ("::ui_session::close_npc_session_out_of_range<benilla_app::ui_binder::BinderState>",
        &["ui_binder::BinderState"],
        "closes the bind question once the player leaves service range, which \
         `CheckBinderDist` reports; the reference tests the range at the call (`0x5dff50`)"),
    ("::ui_session::close_npc_session_out_of_range<benilla_app::ui_dialog_verbs::PetUnlearnState>",
        &["ui_dialog_verbs::PetUnlearnState"],
        "closes the unlearn question once the player leaves service range, which \
         `CheckPetUntrainerDist` reports; the reference tests the range at the call (`0x5dfd30`)"),
    ("::ui_session::close_npc_session_out_of_range<benilla_app::ui_talent_wipe::TalentWipeState>",
        &["ui_talent_wipe::TalentWipeState"],
        "closes the respec question once the player leaves service range, which \
         `CheckTalentMasterDist` reports; the reference tests the range at the call (`0x5df980`)"),
    ("::ui_tooltip::spell_feed::feed_spell_tooltips", &["items::Items"],
        "writes the item cache as it builds the spell tooltips"),
    ("::ui_unit::feed_units", &["UiScript", "ui_pet::PetAttackLatch", "ui_pet::PetBar"],
        "a field event's listeners read its push; the world entry's pet reset writes the bar and \
         lowers the attack latch (`0x4bc6f0`, `0x4bc8ce`)"),
];

/// The systems between the dispatch and the drain that write what the host's live reads read,
/// as [`BETWEEN_THE_DISPATCH_AND_THE_DRAIN`]'s, but where the reference too runs them after the
/// packets, or where no listener can read them: they stay after the flip, and a listener fired
/// inside the dispatch reads them as the frame before left them, as 1.12.1's does. Same shape,
/// held to its systems' access by the same test.
#[rustfmt::skip]
const AFTER_THE_PACKETS_IN_THE_REFERENCE: &[(&str, &[&str], &str)] = &[
    ("::net::motion::facing::drive_display_facing", &["Transform"],
        "turns the units' models toward their facing; unit movement is \
         frame time, which the reference runs after the packets (`0x420d63` after `0x420d55`)"),
    ("::net::motion::spline::ground_clamp_creatures", &["Transform"],
        "settles the creatures on the ground; unit movement is \
         frame time, which the reference runs after the packets (`0x420d63` after `0x420d55`)"),
    ("::net::motion::spline::sample_splines", &["Transform"],
        "moves the units along their splines; unit movement is \
         frame time, which the reference runs after the packets (`0x420d63` after `0x420d55`)"),
    ("::transport::ground_deck_riders", &["Transform"],
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
    ("::ui_reputation::drop_reputation_log", &["net::reputations::Reputations"],
        "drops the reputation log, run only while no in-game interface \
         is up, so no listener exists; it goes with the log"),
    ("::ui_unit::views::drop_unit_views", &["ui_unit::views::UnitViews"],
        "drops the unit views' log, run only while no in-game interface \
         is up, so no listener exists; it goes with the log"),
];

/// The systems [`BETWEEN_THE_DISPATCH_AND_THE_DRAIN`] and [`AFTER_THE_PACKETS_IN_THE_REFERENCE`]
/// hold to, with what each writes of the host's live reads, by suffix; see the first's doc for the
/// kinds.
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
        if info.sets.iter().any(|s| s == "FieldEventPushes") {
            writes.insert("UiScript".to_string());
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

/// Every system between the dispatch and the drain that writes what the host's live reads read
/// has its row, in one of the two lists, as it stands, and a row whose system no longer does is
/// stale.
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
        "these run between the dispatch and the drain and write what the host's live reads read: \
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

/// The frame-final views only shrink: [`super::guard_tests::READS_FRAME_FINAL`] holds no more rows
/// or inputs than its ceiling.
#[test]
fn the_frame_final_views_only_shrink() {
    let rows = super::guard_tests::READS_FRAME_FINAL;
    let inputs: usize = rows.iter().map(|(_, i)| i.len()).sum();
    assert!(
        rows.len() <= FRAME_FINAL_FEEDS && inputs <= FRAME_FINAL_INPUTS,
        "READS_FRAME_FINAL grew to {} rows and {inputs} inputs, past {FRAME_FINAL_FEEDS} and \
         {FRAME_FINAL_INPUTS}: read the new state live through the host instead",
        rows.len()
    );
}

/// Where the flip stands: every list's count, printed, failing nothing (`cargo test
/// flip_readiness -- --nocapture`). The flip needs the first five empty; the rest are context.
#[test]
fn flip_readiness() {
    use super::guard_tests as g;
    let inputs: usize = g::READS_FRAME_FINAL.iter().map(|(_, i)| i.len()).sum();
    let lane_inputs: usize = g::LANE_READS_FRAME_FINAL.iter().map(|(_, i)| i.len()).sum();
    let needs = [
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
        (
            "PLACED_OUTSIDE_THE_DISPATCH",
            g::PLACED_OUTSIDE_THE_DISPATCH.len(),
            String::new(),
        ),
    ];
    let context = [
        (
            "HOST_HELD_UNREAD (to read; a quiet entry stays)",
            g::HOST_HELD_UNREAD.len(),
            String::new(),
        ),
        (
            "LANE_READS_FRAME_FINAL (empties at the flip)",
            g::LANE_READS_FRAME_FINAL.len(),
            format!(" ({lane_inputs} inputs)"),
        ),
        (
            "AFTER_THE_PACKETS_IN_THE_REFERENCE (stays)",
            AFTER_THE_PACKETS_IN_THE_REFERENCE.len(),
            String::new(),
        ),
    ];
    let left: usize = needs.iter().map(|(_, n, _)| n).sum();
    let mut out = format!("flip readiness: {left} rows to empty\n");
    for (list, n, extra) in &needs {
        out += &format!("  {n:>4}  {list}{extra}\n");
    }
    out += "  context:\n";
    for (list, n, extra) in &context {
        out += &format!("  {n:>4}  {list}{extra}\n");
    }
    eprint!("{out}");
}
