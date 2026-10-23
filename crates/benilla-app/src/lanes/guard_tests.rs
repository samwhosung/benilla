//! The guard on the lanes: what the packet handlers queue for the interface fires from a lane,
//! in the packets' order (`0x5384d0`, `0x537aa0`). What still fires outside one is named here,
//! by the queue the handlers fill, by the feed that finds an edge by comparing state, or by the
//! exclusive system, so the list is the whole of the remaining work and only shrinks.

use std::any::TypeId;
use std::collections::{BTreeSet, HashMap};

use bevy::ecs::component::ComponentId;
use bevy::prelude::*;

use crate::game_plugins::schedule_tests::SyncPoints;
use crate::game_plugins::schedule_tests::{census, headless_client, type_names_available};

/// What a row names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// A queue the handlers fill that a feed of the `UiFeed` phase takes and fires itself, by a
    /// suffix of its type's name.
    Queue,
    /// A feed that raises an event by comparing state the handlers write, by a suffix of its
    /// system's name: no queue shows it.
    Edge,
    /// An exclusive system of the phase, whose access bevy does not report, by a suffix of its
    /// system's name.
    Exclusive,
    /// A queue the handlers fill that the call host holds too, by a suffix of its type's name:
    /// every feed that lends the host holds it, so no access tells the feed that takes it apart,
    /// and the row is found by reading the feed, not by this guard. Every such queue needs its
    /// row or a place in [`HOST_HELD_UNREAD`]; a row is stale once the host or the handlers no
    /// longer hold the queue.
    HostHeld,
}

/// What still fires in the frame's system order rather than the packets', with what it is. A
/// queue's row stays while any feed takes that queue; each goes when its producer moves onto a
/// lane, and a row whose queue, feed or system is gone fails as stale.
const NOT_YET_ON_A_LANE: &[(Row, &str, &str)] = &[
    (
        Row::Queue,
        "::ui_action::errors::UiErrorKeys",
        "the refusals the handlers push, which the action feed shows",
    ),
    (
        Row::Queue,
        "::tutorial::Tutorials",
        "the tutorial banks a handler fires",
    ),
    (
        Row::Queue,
        "Messages<benilla_app::tutorial::TutorialEvent>",
        "the tutorial events",
    ),
    (
        Row::Queue,
        "Messages<benilla_app::net::FieldChanged>",
        "the field changes",
    ),
    (
        Row::Queue,
        "::ui_action::errors::MountErrors",
        "the mount refusals",
    ),
    (
        Row::Queue,
        "::ui_action::errors::PetTameFailures",
        "the pet-tame refusals",
    ),
    (
        Row::Queue,
        "::ui_action::errors::UiErrorTexts",
        "the wire's error texts",
    ),
    (
        Row::Queue,
        "::minimap::ping::MinimapPing",
        "the minimap pings",
    ),
    (
        Row::Queue,
        "::net::PlayedTimeAnswer",
        "the played-time answer",
    ),
    (
        Row::Queue,
        "::player::net::SkinnedFeed",
        "the skinned state",
    ),
    (Row::Queue, "::ui_bank::BankErrors", "the bank errors"),
    (
        Row::Queue,
        "::ui_battlefield_positions::BattlefieldPositions",
        "the battlefield positions",
    ),
    (
        Row::Queue,
        "::ui_battlefield::Battlefield",
        "the battlefield status",
    ),
    (Row::Queue, "::ui_binder::BinderState", "the binder confirm"),
    (
        Row::Queue,
        "::ui_dialog_verbs::AreaSpiritHealer",
        "the area spirit healer",
    ),
    (
        Row::Queue,
        "::ui_dialog_verbs::BattlefieldQueue",
        "the battlefield queue",
    ),
    (
        Row::Queue,
        "::ui_dialog_verbs::InstanceBoot",
        "the instance boot",
    ),
    (
        Row::Queue,
        "::ui_dialog_verbs::MeetingStone",
        "the meeting stone",
    ),
    (
        Row::Queue,
        "::ui_dialog_verbs::PetUnlearnState",
        "the pet unlearn confirm",
    ),
    (Row::Queue, "::ui_duel::DuelState", "the duel state"),
    (
        Row::HostHeld,
        "::ui_guild::GuildState",
        "the guild state, which `feed_guild` fires from; the guild verbs, acting in their calls, \
         read it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_guild::GuildLocal",
        "the held guild invitation, which `feed_guild` fires from; the guild verbs, acting in \
         their calls, hold it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_mail::MailOpen",
        "the mailbox's list and results, the send result's equip refusal line among them, which \
         `feed_mail` fires from; the mail verbs, acting in their calls, hold it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_mail::pending::MailPending",
        "the pending-mail notices, whose `UPDATE_PENDING_MAIL` `feed_mail` fires; the mail verbs \
         hold it in the host",
    ),
    (
        Row::Queue,
        "::ui_honor::InspectHonor",
        "the inspected honor",
    ),
    (
        Row::Queue,
        "::ui_instance::InstanceState",
        "the instance state",
    ),
    (Row::Queue, "::ui_logout::LogoutState", "the logout state"),
    (Row::Queue, "::ui_loot_roll::LootRolls", "the loot rolls"),
    (
        Row::Queue,
        "::ui_merchant::MerchantErrors",
        "the merchant errors",
    ),
    (
        Row::Queue,
        "::ui_mirror::MirrorTimerFeed",
        "the mirror timers",
    ),
    (
        Row::HostHeld,
        "::ui_petition::PetitionState",
        "the petition state, which `feed_petition` fires from; the charter verbs, acting in their \
         calls, hold it in the host",
    ),
    (
        Row::Queue,
        "::ui_quest_log::QuestLog",
        "the quest packets' log state",
    ),
    (
        Row::Queue,
        "::ui_quest_share::QuestShare",
        "the quest share",
    ),
    (
        Row::Queue,
        "::ui_quest::lines::QuestLines",
        "the quest lines",
    ),
    (
        Row::HostHeld,
        "::ui_gossip::GossipState",
        "the gossip message's slots, which `feed_gossip` fires from; the gossip verbs, acting in \
         their calls, hold it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_quest::QuestGiver",
        "the quest giver's packets, which `feed_quest` fires from; the quest verbs, acting in \
         their calls, hold it in the host",
    ),
    (
        Row::Queue,
        "::ui_spellbook::LearnedInTab",
        "the spells learned",
    ),
    (Row::Queue, "::ui_stable::StableErrors", "the stable errors"),
    (
        Row::HostHeld,
        "::ui_pet::PetAttackLatch",
        "the pet's attack latch, which a new pet's `SMSG_PET_SPELLS` lowers (`0x4bc8ce`) inside \
         `SetPet`, whose own `PET_BAR_UPDATE` (`0x4bc90c`) `feed_pet_bar` fires; the selection's \
         release and `PetStopAttack`, acting in their calls, hold it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_stable::StableOpen",
        "the stable's packets, which `feed_stable` fires from; the stable verbs and the pet's \
         abandon, acting in their calls, read it in the host",
    ),
    (Row::Queue, "::ui_summon::SummonState", "the summon state"),
    (
        Row::HostHeld,
        "::ui_tabard::TabardOpen",
        "the tabard vendor's packets, which `feed_tabard` fires from; the designer's save, acting \
         in its call, holds it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_talent_wipe::TalentWipeState",
        "the talent wipe question, which `feed_talent_wipe` fires from; `ConfirmTalentWipe`, \
         acting in its call, holds it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_taxi::TaxiState",
        "the flight master's packets, which `feed_taxi` fires from; the taxi verbs, acting in \
         their calls, hold it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_trade::TradeSession",
        "the trade session, which `feed_trade` fires from; the trade verbs, acting in their \
         calls, hold it in the host",
    ),
    (
        Row::Queue,
        "::ui_trainer::TrainerErrors",
        "the trainer errors",
    ),
    (
        Row::Queue,
        "::ui_trainer::TrainerOpen",
        "the trainer's packets",
    ),
    (
        Row::Edge,
        "::ui_party::feed::feed_party",
        "the ready check, the saved instances and the members' presence, found by comparing \
         the group's state",
    ),
    (
        Row::Edge,
        "::ui_items::locks::apply_item_releases",
        "the item source's two unlocks a loot close owes (`0x48f2bd`, `0x4c1d36`) and the copier's \
         ending's (`0x5ebc35` → `0x4c1d36`), taken off the loot session, a lane's type the queue \
         check skips: `close_interaction` serves the packets' closes and the calls' and clicks' \
         alike, so they move when it does",
    ),
    (
        Row::Edge,
        "::ui_unit::feed_units",
        "the selection's change, `PLAYER_TARGET_CHANGED`",
    ),
    (
        Row::Edge,
        "::ui_action::state::fire_action_events",
        "the usable refresh `0x4b31c0` (`ACTIONBAR_UPDATE_USABLE`, `SPELL_ACTIVE`, \
         `SPELL_UPDATE_USABLE`), which the reference runs inside each caller's packet: the form \
         and combo watchers (`0x5de011`, `0x5ddff0`), the unit-flags, health, power and \
         aura-state watchers, the selection and the cooldowns. Here it is one pass a frame over \
         the bar's 120 slots and the reactive list, diffed against the usable cache, so one \
         caller's run cannot be cut out to fire at its own notice. It also fires \
         `SMSG_ITEM_COOLDOWN`'s `BAG_UPDATE_COOLDOWN` (`0x6e965b`), which the reference signals \
         inside that packet's handler and which reaches here through the cooldowns' flush, a \
         queue the call host holds and so no row of its own shows",
    ),
];

/// The host-held queues a handler fills that are yet to be read for what their feeds fire, so
/// the list is the whole of that remaining work and must only shrink: the check finds a queue
/// the handlers and the host both hold, but not whether a feed fires off it what a packet
/// raised. Each goes once its feeds are read, to a [`Row::HostHeld`] row when one does, or named
/// quiet beside why when none does; a new one fails, and an entry whose queue the handlers or the
/// host no longer hold fails as stale.
const HOST_HELD_UNREAD: &[&str] = &[
    "::creature_anim::AttackStopInFlight",
    "::creature_anim::PlaySeq",
    "::items::Items",
    "::net::GuidIndex",
    // Read, and quiet: the feeds that hold it fire nothing off it, as the reputation messages'
    // `UPDATE_FACTION` and `UNIT_FACTION` go on the notice lane at their packets.
    "::net::reputations::Reputations",
    "::net::SelfGuid",
    "::net::game_time::ServerTime",
    "::spell::cast_send::HeldForPick",
    "::spell::cast_send::ScriptCastPermit",
    "::spell::cast_send::unit_caster::PetGcdsOwed",
    "::spell::cast_send::unit_caster::UnitAttackOwed",
    "::spell::cooldowns::Cooldowns",
    "::spell::inflight::ActiveChannel",
    "::spell::inflight::AutoRepeatActive",
    "::spell::inflight::PendingCast",
    "::spell::inflight::QueuedMeleeSpell",
    "::spell::mods::SpellModifiers",
    "::spell::targeting::CursorPicks",
    "::spell::targeting::SpellTargeting",
    "::target::Selection",
    "::ui_action::PlayerActions",
    "::ui_auction::AuctionOpen",
    "::ui_loot::AutoLootByte",
    "::ui_loot::LootLatch",
    "::ui_merchant::MerchantOpen",
    "::ui_pet::PetBar",
    "::ui_petition::GuildRegistrarState",
    "::ui_petition::PetitionRecords",
    "::ui_session::latch::InteractLatch",
    "::ui_social::SocialState",
    "::ui_tradeskill::TradeSkillOpens",
];

/// The resources the handlers fill through a shared reference, which their declared access shows
/// as a read: the error keys a refusal pushes and the tutorial banks a watcher fires. The lanes
/// and the host's two other sinks (the message sounds and the unit edits, which raise no event)
/// are not queues here.
const FILLED_THROUGH_A_READ: &[&str] =
    &["::ui_action::errors::UiErrorKeys", "::tutorial::Tutorials"];

/// Every other resource a handler reads and does not write: state, tables and caches it
/// consults, none of them a queue a feed takes. A new one fails until it is placed here or in
/// [`FILLED_THROUGH_A_READ`]. The list is kept by hand: an entry a handler in fact fills through a
/// shared reference leaves that queue out of [`handler_queues`], so a feed that takes it and fires
/// in system order, or a system that pushes onto a lane off it at its own place, passes every
/// check here unseen.
const READ_ONLY_TO_HANDLERS: &[&str] = &[
    "::ui_chat::edit::ChannelState",
    "::AreaTableRes",
    "::DamageTextGates",
    "::EnvDamageTable",
    "::SpellVisuals",
    "::Creatures",
    "::ItemDisplays",
    "::GameObjectTemplates",
    "::Enchants",
    "::Items",
    "::RandomProperties",
    "::ChrClassTable",
    "::ChrRaceTable",
    "::RacePvpTeams",
    "::MacroBoundSpells",
    "::NameCache",
    "::ServerTime",
    "::GuidIndex",
    // The bag cache the open-container handler numbers its `BAG_OPEN`s from.
    "::BagCache",
    "::NetCommands",
    // Read-only to the handlers that only judge relations (the combat log's, the cast result's
    // and the loot response's): the reputation handlers write it, and so does every handler that
    // lends the call host, whose reputation verbs write the slots.
    "::Reputations",
    "::SelfGuid",
    "::OwnControlLost",
    "::Player",
    "::player::state::StandPending",
    "::BarUnitOutdoors",
    "::PlayerOutdoors",
    "::EmoteSounds",
    "::ExplorationSounds",
    "::ScriptCastPermit",
    "::AutoSelfCast",
    "::GroupRoster",
    "::ActiveChannel",
    "::SpellModifiers",
    "::AssistAttack",
    "::Hovered",
    "::HoveredObject",
    "::Factions",
    "::CreatureTypes",
    "::LastEnemy",
    "::Selection",
    "::TextFilter",
    "::TextFilterSwitches",
    "::PlayerActions",
    "::SpellMechanics",
    "::Spells",
    "::CombatLogRanges",
    "::LogPeriodicSpells",
    "::GuildMemberNotify",
    "::ui_guild::GuildState",
    "::GuildRegistrarState",
    "::PetitionRecords",
    "::ui_petition::PetitionState",
    "::ItemSubClasses",
    "::LootConfig",
    "::LootKneel",
    "::MailOpen",
    "::Stationery",
    "::MerchantOpen",
    // The repair discounts and the taxi graph the host's merchant and taxi verbs consult, and
    // the latch the taxi close reads to count its events.
    "::RepairTables",
    "::TaxiCatalogs",
    "::InteractLatch",
    "::PetBar",
    "::ui_stable::StableOpen",
    "::QuestLog",
    "::UiClock",
    "::InteractNpc",
    "::SocialState",
    "::SkillLines",
    "::SpellFocus",
    "::FrameStep",
    "::CurrentMap",
    "bevy_time::time::Time",
    "bevy_time::time::Time<bevy_time::real::Real>",
    // The host's two sinks that raise no event.
    "::sound::message::MessageSounds",
    "::creature_anim::edits::UnitEdits",
];

/// Every lane's queue, by type: what a handler fills there fires in the packets' order.
fn lane_types() -> [TypeId; 10] {
    [
        TypeId::of::<crate::ui_items::feed::BagView>(),
        TypeId::of::<crate::field_watch::NoticeLane>(),
        TypeId::of::<crate::item_lane::ItemLane>(),
        TypeId::of::<crate::ui_items::EquipErrors>(),
        // The loot lane: the session's steps and pushes, stamped at their packets.
        TypeId::of::<crate::ui_loot::LootState>(),
        TypeId::of::<crate::ui_chat::ChatLog>(),
        TypeId::of::<crate::ui_cast::CastReports>(),
        TypeId::of::<crate::ui_auction::HouseLane>(),
        // The party lane: the group's signal log and the lane's place in it.
        TypeId::of::<crate::ui_party::GroupState>(),
        TypeId::of::<crate::ui_party::PartyLane>(),
    ]
}

/// Whether `id` names a `Messages<T>` buffer, which a reader takes from through a read.
fn is_messages(world: &World, id: ComponentId) -> bool {
    name(world, id).contains("Messages<")
}

fn name(world: &World, id: ComponentId) -> String {
    world
        .components()
        .get_name(id)
        .map_or_else(|| "?".to_string(), |n| n.to_string())
}

fn ends_with_any(name: &str, rows: &[&str]) -> bool {
    rows.iter().any(|s| name.ends_with(s))
}

/// The queues the handlers fill, by id: each resource one writes, past the pure caches, and each
/// resource in [`FILLED_THROUGH_A_READ`] one reads.
fn handler_queues(
    world: &World,
    caches: &std::collections::HashSet<ComponentId>,
) -> Vec<ComponentId> {
    let handlers = world.resource::<crate::net::handlers::NetHandlers>();
    let mut out = BTreeSet::new();
    for (_, access) in &handlers.accesses {
        let access = access.combined_access();
        out.extend(access.resource_writes().filter(|id| !caches.contains(id)));
        out.extend(
            access
                .resource_reads()
                .filter(|id| ends_with_any(&name(world, *id), FILLED_THROUGH_A_READ)),
        );
    }
    out.into_iter().collect()
}

/// The call host's own state, which every system that lends it holds: what a call touches.
fn call_host_access(world: &mut World) -> bevy::ecs::query::Access {
    use bevy::ecs::system::System;
    let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
    System::initialize(&mut host, world)
        .combined_access()
        .clone()
}

/// The resources a handler that does not lend the call host writes: a handler that lends it
/// holds all the host holds, so its access names the host's state, not what its packet fills.
fn filled_apart_from_the_host(app: &mut App) -> std::collections::HashSet<ComponentId> {
    let host = call_host_access(app.world_mut());
    let host_writes: Vec<ComponentId> = host.resource_writes().collect();
    let handlers = app.world().resource::<crate::net::handlers::NetHandlers>();
    handlers
        .accesses
        .iter()
        .map(|(_, a)| a.combined_access())
        .filter(|a| !host_writes.iter().all(|&id| a.has_resource_write(id)))
        .flat_map(|a| a.resource_writes().collect::<Vec<_>>())
        .collect()
}

/// Whether `node` sits in the `UiFeed` phase, through any set.
fn in_feed(graph: &bevy::ecs::schedule::ScheduleGraph, node: bevy::ecs::schedule::NodeId) -> bool {
    use bevy::ecs::schedule::graph::Direction;
    use bevy::ecs::schedule::NodeId;
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        for parent in graph
            .hierarchy()
            .graph()
            .neighbors_directed(n, Direction::Incoming)
        {
            if let NodeId::Set(set) = parent {
                if graph
                    .system_sets
                    .get(set)
                    .is_some_and(|s| format!("{s:?}") == "UiFeed")
                {
                    return true;
                }
                stack.push(parent);
            }
        }
    }
    false
}

/// Every resource a handler reads without writing is known: one the handlers fill through a
/// shared reference is a queue the guard below follows, and the rest are named as read-only.
#[test]
fn every_resource_a_handler_reads_is_a_known_queue_or_read_only() {
    let app = headless_client();
    let world = app.world();
    let handlers = world.resource::<crate::net::handlers::NetHandlers>();
    let vm = world
        .components()
        .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
    let lanes: Vec<Option<ComponentId>> = lane_types()
        .into_iter()
        .map(|t| world.components().get_resource_id(t))
        .collect();
    let mut reads = BTreeSet::new();
    for (_, access) in &handlers.accesses {
        let access = access.combined_access();
        for id in access.resource_reads() {
            if !access.has_resource_write(id) && Some(id) != vm && !lanes.contains(&Some(id)) {
                reads.insert(name(world, id));
            }
        }
    }
    if !reads.iter().any(|n| n.contains("benilla_app::")) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let unknown: Vec<&String> = reads
        .iter()
        .filter(|n| {
            !ends_with_any(n, FILLED_THROUGH_A_READ) && !ends_with_any(n, READ_ONLY_TO_HANDLERS)
        })
        .collect();
    assert!(
        unknown.is_empty(),
        "a handler reads these without writing them; name each in FILLED_THROUGH_A_READ if a \
         handler fills it through a shared reference, else in READ_ONLY_TO_HANDLERS:\n  {}",
        unknown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    let stale: Vec<&str> = READ_ONLY_TO_HANDLERS
        .iter()
        .chain(FILLED_THROUGH_A_READ)
        .filter(|s| !reads.iter().any(|n| n.ends_with(*s)))
        .copied()
        .collect();
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}

/// The drain runs once a frame after every feed and before the tick, gated as the feeds of the
/// in-game interface are, and holds the lanes.
#[test]
fn the_drain_runs_after_the_feeds_and_before_the_tick() {
    let mut app = headless_client();
    let mut lanes = app.world().resource::<super::Lanes>().names();
    lanes.sort_unstable();
    assert_eq!(
        lanes,
        [
            "auction house",
            "bag view",
            "cast reports",
            "chat",
            "equip errors",
            "field notices",
            "item watchers",
            "loot",
            "party"
        ]
    );
    let c = census(&mut app, Update, SyncPoints::Declared);
    if !type_names_available(&c) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let (drain, info) = c
        .systems
        .iter()
        .find(|(_, s)| s.name.ends_with("::lanes::fire_in_order"))
        .expect("the drain is in Update");
    assert!(
        info.conditions
            .iter()
            .any(|c| c.ends_with("::ingame_ui_up")),
        "{:?}",
        info.conditions
    );
    let mut before: HashMap<_, Vec<_>> = HashMap::new();
    for (a, b) in &c.dependencies {
        before.entry(*b).or_default().push(*a);
    }
    let mut ahead = std::collections::HashSet::new();
    let mut stack = vec![*drain];
    while let Some(n) = stack.pop() {
        for m in before.get(&n).into_iter().flatten() {
            if ahead.insert(*m) {
                stack.push(*m);
            }
        }
    }
    let late: Vec<&str> = c
        .systems
        .iter()
        .filter(|(k, s)| s.sets.iter().any(|set| set == "UiFeed") && !ahead.contains(*k))
        .map(|(_, s)| s.name.as_str())
        .collect();
    assert!(
        late.is_empty(),
        "feeds not ordered ahead of the drain: {late:?}"
    );
    let tick = c
        .systems
        .iter()
        .find(|(_, s)| s.name.ends_with("::extract::tick_script"))
        .map(|(k, _)| *k)
        .expect("the tick");
    let mut after = std::collections::HashSet::new();
    let mut stack = vec![*drain];
    let mut next: HashMap<_, Vec<_>> = HashMap::new();
    for (a, b) in &c.dependencies {
        next.entry(*a).or_default().push(*b);
    }
    while let Some(n) = stack.pop() {
        for m in next.get(&n).into_iter().flatten() {
            if after.insert(*m) {
                stack.push(*m);
            }
        }
    }
    assert!(after.contains(&tick), "the drain runs before the tick");
}

/// The remaining work, whole. A queue the handlers fill ([`handler_queues`]) that a VM-holding
/// system of the `UiFeed` phase takes (writes it, past the call host's own state and the lanes;
/// reads it, when it is a message buffer or filled through a read) fires its events outside a
/// lane; so may an exclusive system of the phase, whose access shows nothing. Each needs its row,
/// as each edge-comparing feed has one, and a row whose subject is gone is stale.
#[test]
fn every_handler_queue_a_feed_takes_is_on_a_lane_or_listed() {
    use bevy::ecs::schedule::NodeId;
    let mut app = headless_client();
    // The census's pure caches, read off its own build of the schedule, then the graph read here
    // before this one builds, which moves the systems into the executable.
    let caches = census(&mut headless_client(), Update, SyncPoints::Declared)
        .classes
        .caches;
    let queues = handler_queues(app.world(), &caches);
    let filled_apart = filled_apart_from_the_host(&mut app);
    let (taken, exclusive, systems, host_held) =
        app.world_mut().schedule_scope(Update, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let vm = world
                .components()
                .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
            let host = call_host_access(world);
            let lanes: Vec<ComponentId> = lane_types()
                .into_iter()
                .filter_map(|t| world.components().get_resource_id(t))
                .collect();
            let graph = schedule.graph();
            let in_feed = |node: NodeId| in_feed(graph, node);
            let mut taken = BTreeSet::new();
            let mut exclusive = BTreeSet::new();
            let mut systems = BTreeSet::new();
            let host_held: BTreeSet<String> = queues
                .iter()
                .filter(|&&id| Some(id) != vm && !lanes.contains(&id))
                .filter(|&&id| !is_messages(world, id))
                .filter(|&&id| !ends_with_any(&self::name(world, id), FILLED_THROUGH_A_READ))
                .filter(|&&id| host.has_resource_read(id))
                .filter(|id| filled_apart.contains(id))
                .map(|&id| self::name(world, id))
                .collect();
            for (key, system, _) in graph.systems.iter() {
                let name = system.name().to_string();
                systems.insert(name.clone());
                if !in_feed(NodeId::System(key)) {
                    continue;
                }
                if system.is_exclusive() {
                    exclusive.insert(name);
                    continue;
                }
                let Some(with_access) = graph.systems.get(key) else {
                    continue;
                };
                let access = with_access.access.combined_access();
                if !vm.is_some_and(|vm| access.has_resource_write(vm)) {
                    continue;
                }
                for &id in &queues {
                    if Some(id) == vm || lanes.contains(&id) {
                        continue;
                    }
                    let n = self::name(world, id);
                    let through_a_read = ends_with_any(&n, FILLED_THROUGH_A_READ);
                    let takes = if through_a_read {
                        access.has_resource_read(id)
                    } else if is_messages(world, id) {
                        // A reader, not a writer: the host's own messages are pushes.
                        access.has_resource_read(id)
                            && !access.has_resource_write(id)
                            && !host.has_resource_read(id)
                    } else {
                        access.has_resource_write(id)
                            && !host.has_resource_read(id)
                            && !host.has_resource_write(id)
                    };
                    if takes {
                        taken.insert(n);
                    }
                }
            }
            (taken, exclusive, systems, host_held)
        });
    if !systems.iter().any(|n| n.contains("benilla_app::")) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let rows = |kind: Row| -> Vec<&str> {
        NOT_YET_ON_A_LANE
            .iter()
            .filter(|(k, _, _)| *k == kind)
            .map(|(_, s, _)| *s)
            .collect()
    };
    let mut unlisted: Vec<String> = taken
        .iter()
        .filter(|n| !ends_with_any(n, &rows(Row::Queue)))
        .map(|n| format!("queue {n}"))
        .collect();
    unlisted.extend(
        exclusive
            .iter()
            .filter(|n| !ends_with_any(n, &rows(Row::Exclusive)))
            .map(|n| format!("exclusive {n}")),
    );
    unlisted.extend(
        host_held
            .iter()
            .filter(|n| {
                !ends_with_any(n, &rows(Row::HostHeld)) && !ends_with_any(n, HOST_HELD_UNREAD)
            })
            .map(|n| format!("host-held {n}")),
    );
    let mut stale: Vec<&str> = rows(Row::Queue)
        .into_iter()
        .filter(|s| !taken.iter().any(|n| n.ends_with(s)))
        .collect();
    stale.extend(
        HOST_HELD_UNREAD
            .iter()
            .filter(|s| !host_held.iter().any(|n| n.ends_with(*s)))
            .copied(),
    );
    stale.extend(
        rows(Row::HostHeld)
            .into_iter()
            .filter(|s| !host_held.iter().any(|n| n.ends_with(s))),
    );
    stale.extend(
        rows(Row::Exclusive)
            .into_iter()
            .filter(|s| !exclusive.iter().any(|n| n.ends_with(s))),
    );
    stale.extend(
        rows(Row::Edge)
            .into_iter()
            .filter(|s| !systems.iter().any(|n| n.ends_with(s))),
    );
    assert!(
        unlisted.is_empty(),
        "these fire what the packet handlers queue outside a lane, in system order: put the \
         producer on a lane (`crate::lanes::LaneApp::add_lane`), or give it a row in \
         NOT_YET_ON_A_LANE:\n  {}",
        unlisted.join("\n  ")
    );
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}

/// The handler queues whose items carry their place among what the packets queued, so a system
/// that pushes onto a lane off one pushes at the item's place, not its own: the field edges'
/// `at`, and the aura watcher's calls'. A wrong entry lets a system that pushes at its own place,
/// behind every packet of the frame, pass [`every_lane_push_off_a_handler_queue_keeps_its_place`].
fn placed_queues() -> [TypeId; 2] {
    [
        TypeId::of::<bevy::ecs::message::Messages<crate::net::FieldChanged>>(),
        TypeId::of::<bevy::ecs::message::Messages<crate::net::AuraFieldsChanged>>(),
    ]
}

/// The systems that take a handler queue and may push onto a lane at their own place, by a
/// suffix of the system's name, with why that place is right.
const AT_THEIR_OWN_PLACE: &[(&str, &str)] = &[
    (
        "::creature_anim::impact::route_swing_impacts",
        "a melee swing's record waits for its animation's impact key, which dispatches it in the \
         frame that crosses the key (`0x6247d0`), not at the packet",
    ),
    (
        "::ui_script::glue_mgr::drive_glue_mgr",
        "the login and character screens' answers, which come before the in-game interface: \
         the drain runs only with it up",
    ),
    (
        "::ui_chat::channels::auto_join_zone_channels",
        "world entry only arms the zone-channel walk, an engine turn that runs no Lua: its one \
         push is a walk's refusal into `UiErrorKeys`, which the action feed shows",
    ),
    (
        "::ui_chat::channels::end_session_channels_on_disconnect",
        "a dead socket ends the channel session, and no packet follows it",
    ),
];

/// The systems outside the phase that lend the call host and take a handler queue, firing off it
/// in system order: remaining work, which only shrinks, by a suffix of the system's name.
const NOT_YET_PLACED: &[(&str, &str)] = &[
    (
        "::swing_refusal::apply_swing_refusals",
        "the server's swing stop `0x5ecac0`, `CancelNextSwingSpells` (`0x6e6f30`) and the \
         tutorials 0xa and 0xb, which the reference runs inside its packet (`0x625ab8`, \
         `0x5e7dd0`), off the swing refusals",
    ),
    (
        "::target::scan::auto_acquire",
        "the attacked and cast-at acquires' `SelectTarget`, which the reference runs inside its \
         packet (`0x6259fe`, `0x6e799c`), off the swings and the spell starts",
    ),
    (
        "::ui_tradeskill::drain_trade_skill",
        "the repeat craft's hook, which the reference runs inside the cast's go, its call to \
         `0x4fd7b0` (`0x6e81e7`), off the cast events",
    ),
];

/// A system of any phase that takes a handler queue and pushes onto a lane stamps the push where
/// it runs, behind every packet of the frame, unless the queue's items carry their place
/// ([`placed_queues`]). Each such system needs a row in [`AT_THEIR_OWN_PLACE`] or
/// [`NOT_YET_PLACED`].
#[test]
fn every_lane_push_off_a_handler_queue_keeps_its_place() {
    let mut app = headless_client();
    let caches = census(&mut headless_client(), Update, SyncPoints::Declared)
        .classes
        .caches;
    let queues = handler_queues(app.world(), &caches);
    let (pushes, systems) = app.world_mut().schedule_scope(Update, |world, schedule| {
        schedule.graph_mut().systems.initialize(world);
        let host = call_host_access(world);
        let id = |t: TypeId| world.components().get_resource_id(t);
        let vm = id(TypeId::of::<benilla_ui::script::UiScript>());
        let lanes: Vec<ComponentId> = lane_types().into_iter().filter_map(id).collect();
        // The chat lane is pushed through a shared reference, the others through a write.
        let chat = id(TypeId::of::<crate::ui_chat::ChatLog>());
        let pushes_onto = |access: &bevy::ecs::query::Access, lane: ComponentId| {
            if Some(lane) == chat {
                access.has_resource_read(lane)
            } else {
                access.has_resource_write(lane)
            }
        };
        let placed: Vec<ComponentId> = placed_queues().into_iter().filter_map(id).collect();
        let graph = schedule.graph();
        let mut pushes = BTreeSet::new();
        let mut systems = BTreeSet::new();
        for (key, system, _) in graph.systems.iter() {
            systems.insert(system.name().to_string());
            let Some(with_access) = graph.systems.get(key) else {
                continue;
            };
            let access = with_access.access.combined_access();
            let fires = vm.is_some_and(|vm| access.has_resource_write(vm));
            // A feed of the phase that fires is the other guard's, and an exclusive one is its
            // row; a system that lends the call host outside the phase is judged here, by the
            // queues it reads: a plain resource it writes is its verbs' own state.
            if system.is_exclusive()
                || (fires && in_feed(graph, bevy::ecs::schedule::NodeId::System(key)))
                || !lanes.iter().any(|&l| pushes_onto(access, l))
            {
                continue;
            }
            let name = system.name().to_string();
            let lends = host.is_subset_resources(access);
            for &q in &queues {
                if Some(q) == vm || lanes.contains(&q) || placed.contains(&q) {
                    continue;
                }
                let n = self::name(world, q);
                // The host's own state is what a call touches, never a queue taken.
                let takes = if ends_with_any(&n, FILLED_THROUGH_A_READ) {
                    access.has_resource_read(q) && !host.has_resource_read(q)
                } else if is_messages(world, q) {
                    access.has_resource_read(q)
                        && !access.has_resource_write(q)
                        && !host.has_resource_read(q)
                } else {
                    !lends
                        && access.has_resource_write(q)
                        && !host.has_resource_read(q)
                        && !host.has_resource_write(q)
                };
                if takes {
                    pushes.insert((name.clone(), format!("{name} takes {n}")));
                }
            }
        }
        (pushes, systems)
    });
    if !systems.iter().any(|n| n.contains("benilla_app::")) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let rows: Vec<&str> = AT_THEIR_OWN_PLACE
        .iter()
        .chain(NOT_YET_PLACED)
        .map(|(s, _)| *s)
        .collect();
    let unlisted: Vec<&String> = pushes
        .iter()
        .filter(|(system, _)| !ends_with_any(system, &rows))
        .map(|(_, line)| line)
        .collect();
    assert!(
        unlisted.is_empty(),
        "these push onto a lane off a handler queue at their own place: stamp each push at its \
         item's place, or give the system a row in AT_THEIR_OWN_PLACE or NOT_YET_PLACED:\n  {}",
        unlisted
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    let stale: Vec<&str> = rows
        .into_iter()
        .filter(|s| !pushes.iter().any(|(system, _)| system.ends_with(s)))
        .collect();
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}

/// The drain is exclusive, so bevy lists its pairs with every system no declared order places
/// against it with no component. What those systems meet of the drain's (its lanes' firings'
/// access, and the unit edits it applies in place after each: every component a change writes,
/// the cast events and their sequence, the precast tail's loot release and what it reads) is a
/// pure cache at most, which commutes: the ambiguity ceiling's reason for these pairs. Commands a
/// firing defers, which the drain applies after it, are outside the model. An exclusive system
/// shows bevy no access, so each one needs a declared order against the drain.
#[test]
fn the_systems_unordered_against_the_drain_meet_it_only_on_a_pure_cache() {
    use bevy::ecs::query::{Access, AccessConflicts};
    let mut app = headless_client();
    let mut drain = Access::new();
    for a in &app.world().resource::<super::Lanes>().1 {
        drain.extend(a.combined_access());
    }
    {
        use crate::creature_anim::UnitEdit;
        let world = app.world_mut();
        let writes: BTreeSet<TypeId> = UnitEdit::every(Entity::PLACEHOLDER)
            .iter()
            .flat_map(UnitEdit::writes)
            .collect();
        for t in writes {
            if let Some(id) = world.components().get_id(t) {
                drain.add_component_write(id);
            }
        }
        let player = world.register_component::<crate::net::SelfPlayer>();
        drain.add_component_read(player);
        for t in [
            TypeId::of::<crate::creature_anim::PlaySeq>(),
            TypeId::of::<bevy::ecs::message::Messages<crate::creature_anim::CastEvent>>(),
            TypeId::of::<crate::ui_loot::LootMoveStart>(),
            TypeId::of::<crate::ui_loot::LootCloseAtRest>(),
        ] {
            if let Some(id) = world.components().get_resource_id(t) {
                drain.add_resource_write(id);
            }
        }
        if let Some(id) = world
            .components()
            .get_resource_id(TypeId::of::<crate::player::Player>())
        {
            drain.add_resource_read(id);
        }
    }
    // Each system's access, read before the census builds the schedule.
    let accesses: HashMap<_, (Access, bool)> =
        app.world_mut().schedule_scope(Update, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let graph = schedule.graph();
            graph
                .systems
                .iter()
                .filter_map(|(k, system, _)| {
                    let access = graph.systems.get(k)?.access.combined_access().clone();
                    Some((k, (access, system.is_exclusive())))
                })
                .collect()
        });
    let c = census(&mut app, Update, SyncPoints::Declared);
    if !type_names_available(&c) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let (drain_key, _) = c
        .systems
        .iter()
        .find(|(_, s)| s.name.ends_with("::lanes::fire_in_order"))
        .expect("the drain is in Update");
    let mut next: HashMap<_, Vec<_>> = HashMap::new();
    let mut prev: HashMap<_, Vec<_>> = HashMap::new();
    for (a, b) in &c.dependencies {
        next.entry(*a).or_default().push(*b);
        prev.entry(*b).or_default().push(*a);
    }
    let reach = |edges: &HashMap<_, Vec<_>>| {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![*drain_key];
        while let Some(n) = stack.pop() {
            for m in edges.get(&n).into_iter().flatten() {
                if seen.insert(*m) {
                    stack.push(*m);
                }
            }
        }
        seen
    };
    let (after, before) = (reach(&next), reach(&prev));
    let world = app.world();
    let mut hazards = Vec::new();
    for (k, (a, exclusive)) in &accesses {
        if k == drain_key || after.contains(k) || before.contains(k) {
            continue;
        }
        if *exclusive {
            hazards.push(format!("{}: exclusive, its access unknown", c.name(*k)));
            continue;
        }
        if a.is_compatible(&drain) {
            continue;
        }
        let met: Vec<String> = match a.get_conflicts(&drain) {
            AccessConflicts::All => vec!["everything".to_string()],
            AccessConflicts::Individual(ids) => ids
                .ones()
                .map(ComponentId::new)
                .filter(|id| !c.classes.caches.contains(id))
                .map(|id| name(world, id))
                .collect(),
        };
        if !met.is_empty() {
            hazards.push(format!("{}: {met:?}", c.name(*k)));
        }
    }
    assert!(
        hazards.is_empty(),
        "these run in no declared order against the drain and meet what it touches; order them:\n  {}",
        hazards.join("\n  ")
    );
}
