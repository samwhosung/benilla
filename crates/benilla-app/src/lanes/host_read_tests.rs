//! The readiness guard for firing each packet's lanes inside the net dispatch, after its handlers,
//! with the host lent: the reference's handler signals inside itself (`0x703e50`) and runs whole
//! before the next packet's (`0x537ae5`), so a listener reads the live state its packet left and
//! its sends go out in packet order. That move is right only once a lane's listener reads nothing
//! a later system of the frame writes or pushes, which two lists name, each the whole of that
//! remaining work and only shrinking; the move needs both empty.
//!
//! - The feeds that push a view of what a handler writes, which a getter reads in place of the
//!   live state through the host: [`super::guard_tests::READS_FRAME_FINAL`], held here to its
//!   count. A family's getters going live ([`crate::script_calls::live`]) drops its feed's row.
//! - [`BETWEEN_THE_DISPATCH_AND_THE_DRAIN`]: the systems that may run after the dispatch and
//!   ahead of the drain and write what the host's live reads read, or push a view the drain waits
//!   for.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use bevy::ecs::component::ComponentId;
use bevy::ecs::query::{ComponentAccessKind, FilteredAccess};
use bevy::ecs::schedule::SystemKey;
use bevy::prelude::*;

use crate::game_plugins::schedule_tests::{census, type_names_available, SyncPoints};

/// The most rows [`super::guard_tests::READS_FRAME_FINAL`] may hold, and the most inputs across
/// them: each only falls, so lower it when a row or an input goes.
const FRAME_FINAL_FEEDS: usize = 92;
const FRAME_FINAL_INPUTS: usize = 445;

/// The systems that may run after the dispatch (`apply_net_updates`) and run ahead of the drain
/// (`fire_in_order`), by a suffix of the system's name, with what each writes of the host's live
/// reads ([`crate::script_calls::live::LiveReads`]), by its name in the crate, and why. A listener
/// fired inside the dispatch would read each of these as the frame before left it. `UiScript`
/// names a view the drain waits for ([`crate::ui_unit::field_events::FieldEventPushes`]);
/// `Commands` names a component a system filters out and may add with its commands, which
/// access does not show; `exclusive` a system whose access Bevy does not report.
#[rustfmt::skip]
const BETWEEN_THE_DISPATCH_AND_THE_DRAIN: &[(&str, &[&str], &str)] = &[
    ("::net::apply::tag_self_player", &["Commands net::SelfPlayer"],
        "tags our created entity as the player, which the token resolver finds it by; the \
         reference makes it the active player inside its create (`0x5dea50`)"),
    ("::query_cache::release_on_enter<benilla_app::items::Items>", &["items::Items"],
        "clears the item cache's in-flight asks on world entry, so each is asked again"),
    ("::query_cache::release_on_enter<benilla_app::names::NameCache>", &["names::NameCache"],
        "clears the name cache's in-flight asks on world entry, so each is asked again"),
    ("::query_cache::release_on_enter<benilla_app::ui_guild::GuildState>", &["ui_guild::GuildState"],
        "clears the guild cache's in-flight asks on world entry, so each is asked again"),
    ("::spell::group_relation::publish_group_roster", &["spell::group_relation::GroupRoster"],
        "copies the group's roster, which the relation reads take, once the packets are in"),
    ("::ui_action::cooldown_lane::drop_cooldown_log<benilla_app::spell::cooldowns::Cooldowns>",
        &["spell::cooldowns::Cooldowns"],
        "drops the player's cooldown log with no in-game interface up; it goes with the log"),
    ("::ui_action::cooldown_lane::drop_cooldown_log<benilla_app::ui_pet::PetBar>",
        &["ui_pet::PetBar"],
        "drops the pet's cooldown log with no in-game interface up; it goes with the log"),
    ("::ui_action::state::feed_action_state", &["spell::cooldowns::Cooldowns",
        "ui_action::state::ActionStateMemory"],
        "keeps the frame's `GetTime` clock the cooldown reads map each start through, and \
         writes the list as it walks the slots"),
    ("::ui_aura::feed_auras", &["ui_aura::AuraFeedMemory"],
        "rebuilds the raid filter's books and the world entry's player the aura walk reads"),
    ("::ui_char::feed_char", &["UiScript"], "a field event's listeners read its push"),
    ("::ui_char::feed_skills", &["UiScript"], "a field event's listeners read its push"),
    ("::ui_honor::feed_honor", &["UiScript"], "a field event's listeners read its push"),
    ("::ui_inspect::feed_inspect", &["UiScript"], "a field event's listeners read its push"),
    ("::ui_items::feed::feed_item_stats", &["items::Items"],
        "takes the item cache's fresh and dropped records as it pushes them"),
    ("::ui_items::feed::feed_random_properties", &["items::RandomProperties"],
        "writes the random-suffix table as it pushes it, once per VM"),
    ("::ui_party::feed::feed_party", &["UiScript"], "a field event's listeners read its push"),
    ("::ui_pet::drain::arm_owed_pet_gcds", &["ui_pet::PetBar"],
        "puts the GCDs the pet's sends owe into its list, which the reference arms at the send \
         (`0x6e58fb`)"),
    ("::ui_pet::unit::feed_pet_unit", &["UiScript", "names::NameCache"],
        "a field event's listeners read its push; it watches the name cache's record of the pet's name"),
    ("::ui_pet_stats::feed_pet_stats", &["UiScript"], "a field event's listeners read its push"),
    ("::ui_reputation::feed_reputation", &["UiScript", "net::reputations::Reputations"],
        "a field event's listeners read its push; it takes the list's pending operations off the reputations"),
    ("::ui_tooltip::spell_feed::feed_spell_tooltips", &["items::Items"],
        "writes the item cache as it builds the spell tooltips"),
    ("::ui_unit::feed_units", &["UiScript", "ui_pet::PetBar"],
        "a field event's listeners read its push; the world entry's pet reset writes the bar (`0x4bc6f0`)"),
    ("::ui_unit::views::drop_unit_views", &["ui_unit::views::UnitViews"],
        "drops the unit views' log with no in-game interface up; it goes with the log"),
];

/// The systems [`BETWEEN_THE_DISPATCH_AND_THE_DRAIN`] holds to, with what each writes of the
/// host's live reads, by suffix; see its doc for the kinds.
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

/// Every system between the dispatch and the drain that writes what the host's live reads read
/// has its row, as it stands, and a row whose system no longer does is stale.
#[test]
fn every_writer_between_the_dispatch_and_the_drain_is_listed() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let Some(found) = writers_between(&mut app) else {
        eprintln!("skipped: this build carries no type names");
        return;
    };
    let row = |system: &str| {
        BETWEEN_THE_DISPATCH_AND_THE_DRAIN
            .iter()
            .find(|(s, _, _)| format!("::{system}").ends_with(s))
    };
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
    let stale: Vec<&str> = BETWEEN_THE_DISPATCH_AND_THE_DRAIN
        .iter()
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
         move the write into its handler, or give each its row in \
         BETWEEN_THE_DISPATCH_AND_THE_DRAIN as it stands:\n  {}",
        wrong.join("\n  ")
    );
    assert!(stale.is_empty(), "stale rows: {stale:?}");
    let empty: Vec<&str> = BETWEEN_THE_DISPATCH_AND_THE_DRAIN
        .iter()
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
