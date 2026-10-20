//! [`GamePlugins`]: the game as one plugin group, on top of the engine's
//! `benilla_world::world_plugins::WorldPlugins`. `schedule_tests` builds it headless and checks
//! the schedule for undeclared orders.
//!
//! The order is load-bearing. Some plugins read a resource an earlier one inserts at build time
//! (`VideoPlugin` and `RealmlistPlugin` before `CvarPlugin`, `WorldBackdropPlugin` after
//! `PlayerUiPlugin`, the `UiActionPlugin` family), and registration order is the executor's
//! tie-break between systems with no declared order, so reordering members changes behaviour.
//!
//! Not here, and added by `run()` around this group: the engine, the three process plugins
//! (`ThreadQos`, `BgWin`, `MacQuit`) and the dev instruments above the stack.

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;

use crate::blob_shadow::BlobShadowPlugin;
use crate::bowstring::BowstringPlugin;
use crate::camera_shake::CameraShakePlugin;
use crate::chr_classes::ChrClassesPlugin;
use crate::cinematic::CinematicPlugin;
use crate::creature_anim::CreatureAnimPlugin;
use crate::cursor::CursorPlugin;
use crate::entities::EntitiesPlugin;
use crate::fishing_line::FishingLinePlugin;
use crate::footprints::FootprintsPlugin;
use crate::loading_screen::LoadingScreenPlugin;
use crate::name_persist::NamePersistPlugin;
use crate::net::NetPlugin;
use crate::player::PlayerPlugin;
use crate::portrait::PortraitPlugin;
use crate::quest_markers::QuestMarkersPlugin;
use crate::sound::SoundPlugin;
use crate::target::TargetPlugin;
use crate::textinput::TextInputPlugin;
use crate::transport::TransportPlugin;
use crate::tutorial::TutorialPlugin;
use crate::ui_action::UiActionPlugin;
use crate::ui_auction::UiAuctionPlugin;
use crate::ui_aura::UiAuraPlugin;
use crate::ui_bank::UiBankPlugin;
use crate::ui_battlefield::BattlefieldPlugin;
use crate::ui_battlefield_positions::BattlefieldPositionsPlugin;
use crate::ui_battlefield_score::BattlefieldScorePlugin;
use crate::ui_binder::UiBinderPlugin;
use crate::ui_cast::UiCastPlugin;
use crate::ui_char::UiCharPlugin;
use crate::ui_chat::UiChatPlugin;
use crate::ui_craft::UiCraftPlugin;
use crate::ui_dialog_verbs::UiDialogVerbsPlugin;
use crate::ui_duel::UiDuelPlugin;
use crate::ui_follow::UiFollowPlugin;
use crate::ui_gm_ticket::UiGmTicketPlugin;
use crate::ui_gossip::UiGossipPlugin;
use crate::ui_guild::UiGuildPlugin;
use crate::ui_instance::UiInstancePlugin;
use crate::ui_item_text::UiItemTextPlugin;
use crate::ui_items::UiItemsPlugin;
use crate::ui_layout::UiLayoutPlugin;
use crate::ui_logout::UiLogoutPlugin;
use crate::ui_loot::UiLootPlugin;
use crate::ui_loot_roll::UiLootRollPlugin;
use crate::ui_mail::UiMailPlugin;
use crate::ui_merchant::UiMerchantPlugin;
use crate::ui_mirror::UiMirrorPlugin;
use crate::ui_net::UiNetPlugin;
use crate::ui_party::UiPartyPlugin;
use crate::ui_pass::PlayerUiPlugin;
use crate::ui_pet::UiPetPlugin;
use crate::ui_pet_book::UiPetBookPlugin;
use crate::ui_pet_stats::UiPetStatsPlugin;
use crate::ui_petition::UiPetitionPlugin;
use crate::ui_quest::UiQuestPlugin;
use crate::ui_quest_log::UiQuestLogPlugin;
use crate::ui_quest_share::QuestSharePlugin;
use crate::ui_saved::UiSavedPlugin;
use crate::ui_script::UiScriptPlugin;
use crate::ui_shapeshift::UiShapeshiftPlugin;
use crate::ui_social::UiSocialPlugin;
use crate::ui_spellbook::UiSpellbookPlugin;
use crate::ui_stable::UiStablePlugin;
use crate::ui_summon::UiSummonPlugin;
use crate::ui_tabard::TabardUiPlugin;
use crate::ui_talent::UiTalentPlugin;
use crate::ui_talent_wipe::UiTalentWipePlugin;
use crate::ui_taxi::UiTaxiPlugin;
use crate::ui_text::UiTextPlugin;
use crate::ui_tooltip::UiTooltipPlugin;
use crate::ui_trade::UiTradePlugin;
use crate::ui_tradeskill::UiTradeSkillPlugin;
use crate::ui_trainer::UiTrainerPlugin;
use crate::ui_unit::UiUnitPlugin;
use crate::world_backdrop::WorldBackdropPlugin;

/// The game, as one plugin group. The two fields are the two plugins `run()` parameterises.
pub(crate) struct GamePlugins {
    /// [`NetPlugin::connect`]: `false` in capture mode, where no IO thread runs, so captures are
    /// deterministic whether or not a server is up.
    pub(crate) connect: bool,
    /// [`crate::client_state::ClientStatePlugin::start`]: the screen this session opens on.
    pub(crate) start: crate::client_state::ClientState,
}

impl PluginGroup for GamePlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            // The game's own WGSL, compiled in, before anything that could ask for one.
            .add(crate::shaders::plugin)
            .add(BowstringPlugin)
            .add(crate::weapon_trail::WeaponTrailPlugin)
            .add(FishingLinePlugin)
            .add(QuestMarkersPlugin)
            .add(crate::pipe_warm::plugin)
            .add(EntitiesPlugin)
            // The combat log, ahead of the animation layer, whose environmental-damage handler runs
            // second; its swing handler registers ahead of the line (`net_handler_ahead`).
            .add(crate::combat_log::CombatLogPlugin)
            .add(CreatureAnimPlugin)
            // The blob shadow (`0x6d7920`), sized from the Stand box (`playableAnimationLookup[0]`,
            // fixed per model), never the playing sequence.
            .add(BlobShadowPlugin)
            .add(CameraShakePlugin)
            .add(FootprintsPlugin)
            .add(crate::go_anim::plugin)
            .add(crate::doodad_events::plugin)
            .add(PlayerPlugin)
            .add(crate::screen_fade::ScreenFadePlugin)
            // After PlayerPlugin, whose `control` it overrides in the same stage.
            .add(CinematicPlugin)
            .add(CursorPlugin)
            .add(NetPlugin {
                connect: self.connect,
            })
            .add(crate::death::DeathPlugin)
            .add(crate::client_state::ClientStatePlugin { start: self.start })
            .add(crate::glue_session::GlueSessionPlugin)
            .add(SoundPlugin)
            .add(TargetPlugin)
            .add(TransportPlugin)
            .add(LoadingScreenPlugin)
            // The player-UI quad pass; `$WOW_UI_DEMO=1` seeds a proof scene.
            .add(PlayerUiPlugin)
            // After the UI pass: it points that plugin's camera at the world camera.
            .add(WorldBackdropPlugin)
            .add(crate::minimap::MinimapPlugin)
            .add(crate::ui_models::UiModelsPlugin)
            .add(crate::area::AreaPlugin)
            .add(crate::area_poi::AreaPoiPlugin)
            .add(crate::world_state_ui::WorldStateUiPlugin)
            .add(crate::area_trigger::AreaTriggerPlugin)
            .add(crate::ui_world_map::WorldMapUiPlugin)
            .add(crate::poi_marker::PoiMarkerPlugin)
            .add(UiTextPlugin)
            // Ahead of the portrait booth and the interaction face-me, which both read it.
            .add(crate::ui_session::UiSessionPlugin)
            .add(PortraitPlugin)
            .add(TextInputPlugin)
            .add(UiScriptPlugin)
            // Before CvarPlugin, so the resource exists when `load_config` applies the saved value.
            .add(crate::video::VideoPlugin)
            // A CVar knob too, so before CvarPlugin for the same reason.
            .add(crate::realmlist::RealmlistPlugin)
            .add(crate::cvars::CvarPlugin)
            .add(crate::console::ConsolePlugin)
            .add(crate::player_commands::PlayerCommandsPlugin)
            .add(crate::bindings::BindingsPlugin)
            .add(UiUnitPlugin)
            .add(UiPartyPlugin)
            .add(UiDuelPlugin)
            .add(UiBinderPlugin)
            .add(UiDialogVerbsPlugin)
            .add(BattlefieldScorePlugin)
            .add(BattlefieldPlugin)
            .add(BattlefieldPositionsPlugin)
            .add(crate::game_tip::GameTipPlugin)
            .add(crate::text_filter::TextFilterPlugin)
            .add(TutorialPlugin)
            .add(crate::swing_refusal::SwingRefusalPlugin)
            .add(UiSummonPlugin)
            .add(UiGmTicketPlugin)
            .add(UiFollowPlugin)
            .add(UiInstancePlugin)
            .add(crate::ui_billing::UiBillingPlugin)
            .add(crate::ui_memory::UiMemoryPlugin)
            .add(UiLogoutPlugin)
            .add(UiSocialPlugin)
            // After the social session, whose FriendsFrame and ignore list it uses.
            .add(UiGuildPlugin)
            // After the guild session, whose error channel its refusals ride.
            .add(UiPetitionPlugin)
            .add(UiTooltipPlugin)
            .add(UiCharPlugin)
            .add(crate::ui_reputation::UiReputationPlugin)
            .add(crate::ui_inspect::InspectUiPlugin)
            // After the inspect feed, whose target it asks about.
            .add(crate::ui_honor::UiHonorPlugin)
            .add(crate::ui_dressup::DressUpUiPlugin)
            .add(UiActionPlugin)
            // The cast lifecycle; after UiActionPlugin, whose feeds and cast ladder read it.
            .add(crate::spell::SpellPlugin)
            // After UiActionPlugin, whose `Spells` catalog it shares.
            .add(UiAuraPlugin)
            // After UiActionPlugin, whose `Spells` and `send_spell_cast` it shares.
            .add(UiSpellbookPlugin)
            // After UiSpellbookPlugin: a macro's spell resolves against the book it pushes.
            .add(crate::ui_macro::UiMacroPlugin)
            // After UiActionPlugin, whose `Spells` catalog it shares.
            .add(UiTalentPlugin)
            .add(UiTalentWipePlugin)
            // The form list by the reference's admission and order (`0x4b25b0`, `0x4b2bb0`); after
            // UiActionPlugin (`Spells`) and SpellPlugin (the cast tail).
            .add(UiShapeshiftPlugin)
            // After UiActionPlugin, whose `Spells` and cooldown clock it shares.
            .add(UiPetPlugin)
            .add(ChrClassesPlugin)
            .add(UiPetBookPlugin)
            .add(UiPetStatsPlugin)
            .add(UiNetPlugin)
            .add(UiCastPlugin)
            .add(UiMirrorPlugin)
            .add(crate::combat_text::CombatTextPlugin)
            .add(crate::nameplates::NameplatesPlugin)
            .add(crate::raid_marks::RaidMarksPlugin)
            .add(crate::vplates::VPlatesPlugin)
            .add(crate::chat_bubble::ChatBubblePlugin)
            .add(UiItemsPlugin)
            .add(UiGossipPlugin)
            .add(UiMerchantPlugin)
            .add(UiAuctionPlugin)
            .add(UiBankPlugin)
            .add(UiMailPlugin)
            .add(UiTradePlugin)
            .add(UiItemTextPlugin)
            .add(UiSavedPlugin)
            .add(NamePersistPlugin)
            .add(UiStablePlugin)
            .add(TabardUiPlugin)
            .add(UiTrainerPlugin)
            .add(UiTaxiPlugin)
            .add(UiTradeSkillPlugin)
            .add(UiCraftPlugin)
            .add(UiLootPlugin)
            .add(UiLootRollPlugin)
            .add(UiQuestPlugin)
            .add(UiQuestLogPlugin)
            .add(QuestSharePlugin)
            .add(UiChatPlugin)
            .add(UiLayoutPlugin)
            .add(crate::screenshot::ScreenshotPlugin)
            // Last: it orders every system the plugins added, the probes' too.
            .add(crate::vm_pass::VmPassPlugin)
    }
}

#[cfg(test)]
pub(crate) mod schedule_tests {
    use std::any::TypeId;
    use std::collections::{BTreeMap, HashMap, HashSet};

    use super::*;
    use bevy::ecs::component::ComponentId;
    use bevy::ecs::schedule::graph::Direction;
    use bevy::ecs::schedule::{LogLevel, NodeId, ScheduleBuildSettings, ScheduleLabel, SystemKey};

    /// The whole client, built headless: the tuned `DefaultPlugins` with no window, winit, logger
    /// or GPU (`backends: None`, so no render app), then the engine, then the game. Every
    /// plugin's `build` and `finish` runs and no schedule does, which fixes the schedule graph.
    pub(crate) fn headless_client() -> App {
        headless_client_with(|_| {})
    }

    /// [`headless_client`] with `more` added before it finishes, as the binary adds its
    /// instruments and probes.
    pub(crate) fn headless_client_with(more: impl FnOnce(&mut App)) -> App {
        let mut app = App::new();
        app.add_plugins(
            benilla_world::boot::tuned_default_plugins(Window::default())
                .disable::<bevy::winit::WinitPlugin>()
                .disable::<bevy::log::LogPlugin>()
                .set(bevy::window::WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .set(bevy::render::RenderPlugin {
                    render_creation: bevy::render::settings::WgpuSettings {
                        backends: None,
                        ..default()
                    }
                    .into(),
                    ..default()
                }),
        );
        app.add_plugins(benilla_world::world_plugins::WorldPlugins);
        app.add_plugins(GamePlugins {
            connect: false,
            start: crate::client_state::ClientState::Login,
        });
        more(&mut app);
        app.finish();
        app.cleanup();
        app
    }

    /// One system as the schedule graph knows it: its name, every set it is under
    /// (transitively), and every run condition on it or on one of those sets, by name.
    #[derive(Debug, Clone)]
    pub(crate) struct SystemInfo {
        pub name: String,
        pub sets: Vec<String>,
        pub conditions: Vec<String>,
    }

    /// One schedule, read two ways: the graph before it builds (names, sets, conditions move
    /// into the executable when it does), and the conflict list after.
    pub(crate) struct Census {
        pub systems: HashMap<SystemKey, SystemInfo>,
        /// Pairs of systems with conflicting access and no path between them, each with what
        /// they fight over.
        pub conflicts: Vec<(SystemKey, SystemKey, Vec<ComponentId>)>,
        /// Everything any pair fights over, by name (a placeholder in a build without type
        /// names, [`type_names_available`]).
        pub components: HashMap<ComponentId, String>,
        /// The explained classes, resolved against this world.
        pub classes: Classes,
        /// Every declared `a` runs before `b`, at the system level.
        pub dependencies: Vec<(SystemKey, SystemKey)>,
        /// The systems that must run on the main thread (the VM's, the audio layer's).
        pub non_send: Vec<SystemKey>,
        /// The systems whose declared access includes the Lua VM, read off each system's own
        /// access set: a holder whose every VM pair is declared shows in no conflict.
        pub holds_vm: HashSet<SystemKey>,
        /// The systems whose declared access covers the call host's
        /// ([`crate::script_calls::ScriptCallHost`]): every one that writes the VM.
        pub hosted: HashSet<SystemKey>,
        /// The systems whose own access names [`crate::net::SelfPlayer`]: a read of the
        /// component, or a `With`, `Without`, `Added` or `Has` on it. Read off each system's
        /// access set; a system that reads every component (`EntityRef`, `&World`) names none.
        pub uses_self: HashSet<SystemKey>,
        /// The build's own order, system to system, with the sync points it placed when the
        /// census reads [`SyncPoints::Built`].
        pub built: Vec<(SystemKey, SystemKey)>,
    }

    impl Census {
        pub fn name(&self, key: SystemKey) -> &str {
            self.systems
                .get(&key)
                .map(|s| s.name.as_str())
                .unwrap_or("?")
        }

        pub fn component(&self, id: ComponentId) -> &str {
            self.components.get(&id).map(String::as_str).unwrap_or("?")
        }

        /// Whether what `a` queues has applied by the time `b` runs: a sync point of the build
        /// lies on a path from `a` to `b`, so it runs after `a` and before `b`, and applies every
        /// buffer of a system that ran before it. Reads a [`SyncPoints::Built`] census.
        pub fn applies_before(&self, a: SystemKey, b: SystemKey) -> bool {
            let mut next: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
            for (x, y) in &self.built {
                next.entry(*x).or_default().push(*y);
            }
            // Each node at most twice: before a sync point and past one.
            let mut seen: HashSet<(SystemKey, bool)> = HashSet::new();
            let mut stack = vec![(a, false)];
            while let Some((n, synced)) = stack.pop() {
                for m in next.get(&n).into_iter().flatten() {
                    let synced = synced || is_sync_point(self.name(*m));
                    if *m == b && synced {
                        return true;
                    }
                    if seen.insert((*m, synced)) {
                        stack.push((*m, synced));
                    }
                }
            }
            false
        }
    }

    /// A build pass that keeps the graph the build ends with. Registered after the pass that
    /// inserts the sync points, it runs after it and sees them.
    #[derive(Debug, Default)]
    struct KeepBuiltOrder(std::sync::Arc<std::sync::Mutex<Vec<(SystemKey, SystemKey)>>>);

    impl bevy::ecs::schedule::ScheduleBuildPass for KeepBuiltOrder {
        type EdgeOptions = ();

        fn add_dependency(&mut self, _: NodeId, _: NodeId, _: Option<&()>) {}

        fn collapse_set(
            &mut self,
            _: bevy::ecs::schedule::SystemSetKey,
            _: &indexmap::IndexSet<SystemKey, bevy::platform::hash::FixedHasher>,
            _: &bevy::ecs::schedule::graph::DiGraph<NodeId>,
        ) -> impl Iterator<Item = (NodeId, NodeId)> {
            std::iter::empty()
        }

        fn build(
            &mut self,
            _: &mut World,
            _: &mut bevy::ecs::schedule::ScheduleGraph,
            flat: &mut bevy::ecs::schedule::graph::Dag<SystemKey>,
        ) -> Result<(), bevy::ecs::schedule::ScheduleBuildError> {
            *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                flat.graph().all_edges().collect();
            Ok(())
        }
    }

    /// Which graph a census reads. Bevy's build shares one `ApplyDeferred` barrier per distance
    /// from the schedule's start (`auto_insert_apply_deferred.rs`, `get_sync_point`), so two
    /// systems sharing a barrier read as ordered through it, and one upstream edge can re-home
    /// whole groups and move unrelated pairs.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum SyncPoints {
        /// The schedule as it runs, barriers included: a barrier is a real wave boundary.
        Built,
        /// The declared graph only, no barriers: "no path" means "nobody declared an order".
        /// What a ratchet on undeclared orders must read.
        Declared,
    }

    /// Take the census of one schedule. Initializing it can insert `Schedules` itself, so this
    /// goes through bevy's `schedule_scope`, not a `resource_scope` on that resource.
    pub(crate) fn census(app: &mut App, label: impl ScheduleLabel, sync: SyncPoints) -> Census {
        let label = label.intern();
        app.world_mut().schedule_scope(label, |world, schedule| {
            schedule.set_build_settings(ScheduleBuildSettings {
                ambiguity_detection: LogLevel::Warn,
                auto_insert_apply_deferred: sync == SyncPoints::Built,
                ..default()
            });
            // `initialize` fills the systems' access (the build would run it anyway); done
            // first so pass 1 can read it while the graph still holds the systems.
            schedule.graph_mut().systems.initialize(world);
            let vm = world
                .components()
                .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
            let host = {
                use bevy::ecs::system::{IntoSystem, System};
                let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
                System::initialize(&mut host, world)
            };
            // Pass 1, before the build: the graph still holds the systems.
            let graph = schedule.graph();
            let set_conditions: HashMap<_, Vec<String>> = graph
                .system_sets
                .iter()
                .map(|(key, _, conds)| {
                    (
                        key,
                        conds
                            .iter()
                            .map(|c| c.condition.name().to_string())
                            .collect(),
                    )
                })
                .collect();
            let self_player = world
                .components()
                .get_id(TypeId::of::<crate::net::SelfPlayer>());
            let mut systems = HashMap::new();
            let mut holds_vm = HashSet::new();
            let mut hosted = HashSet::new();
            let mut uses_self = HashSet::new();
            for (key, system, conds) in graph.systems.iter() {
                if let (Some(id), Some(with_access)) = (self_player, graph.systems.get(key)) {
                    let names_it = with_access.access.filtered_accesses().iter().any(|fa| {
                        fa.contains(id)
                            || (!fa.access().has_read_all_components()
                                && fa.access().has_component_read(id))
                    });
                    if names_it {
                        uses_self.insert(key);
                    }
                }
                if let (Some(vm), Some(with_access)) = (vm, graph.systems.get(key)) {
                    let access = with_access.access.combined_access();
                    if access.has_resource_read(vm) || access.has_resource_write(vm) {
                        holds_vm.insert(key);
                    }
                    let mut with_host = with_access.access.clone();
                    with_host.extend(host.clone());
                    if with_host.combined_access() == access {
                        hosted.insert(key);
                    }
                }
                let mut sets = Vec::new();
                let mut conditions: Vec<String> = conds
                    .iter()
                    .map(|c| c.condition.name().to_string())
                    .collect();
                let mut stack = vec![NodeId::System(key)];
                while let Some(node) = stack.pop() {
                    for parent in graph
                        .hierarchy()
                        .graph()
                        .neighbors_directed(node, Direction::Incoming)
                    {
                        if let NodeId::Set(set) = parent {
                            if let Some(s) = graph.system_sets.get(set) {
                                sets.push(format!("{s:?}"));
                            }
                            if let Some(c) = set_conditions.get(&set) {
                                conditions.extend(c.iter().cloned());
                            }
                            stack.push(parent);
                        }
                    }
                }
                systems.insert(
                    key,
                    SystemInfo {
                        name: system.name().to_string(),
                        sets,
                        conditions,
                    },
                );
            }
            // Every declared order at the system level: an edge between sets is an edge between
            // every member of one and every member of the other.
            let members = |node: NodeId| -> Vec<SystemKey> {
                let mut out = Vec::new();
                let mut stack = vec![node];
                while let Some(n) = stack.pop() {
                    match n {
                        NodeId::System(k) => out.push(k),
                        NodeId::Set(_) => stack.extend(
                            graph
                                .hierarchy()
                                .graph()
                                .neighbors_directed(n, Direction::Outgoing),
                        ),
                    }
                }
                out
            };
            // An edge through a set that holds no system still orders its neighbours, as the
            // build flattens it (the VM pass's links are such sets).
            let mut edges: Vec<(NodeId, NodeId)> = graph.dependency().graph().all_edges().collect();
            let empty: Vec<NodeId> = graph
                .system_sets
                .iter()
                .map(|(key, _, _)| NodeId::Set(key))
                .filter(|n| members(*n).is_empty())
                .collect();
            for set in empty {
                let into: Vec<NodeId> = edges
                    .iter()
                    .filter(|(_, b)| *b == set)
                    .map(|(a, _)| *a)
                    .collect();
                let out: Vec<NodeId> = edges
                    .iter()
                    .filter(|(a, _)| *a == set)
                    .map(|(_, b)| *b)
                    .collect();
                edges.retain(|(a, b)| *a != set && *b != set);
                for a in &into {
                    for b in &out {
                        edges.push((*a, *b));
                    }
                }
            }
            let mut dependencies = Vec::new();
            for (a, b) in edges {
                for x in members(a) {
                    for y in members(b) {
                        if x != y {
                            dependencies.push((x, y));
                        }
                    }
                }
            }
            // Pass 2: build, and read what the build found. `is_send` is only known once a
            // system's params have registered their access, i.e. after this. The keeper goes in
            // last, behind the sync points' pass the settings above may have re-added.
            let built = std::sync::Arc::default();
            schedule.remove_build_pass::<KeepBuiltOrder>();
            schedule.add_build_pass(KeepBuiltOrder(std::sync::Arc::clone(&built)));
            schedule.initialize(world).expect("the schedule builds");
            let built = std::mem::take(
                &mut *built
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            let mut non_send = Vec::new();
            for (key, system) in schedule.systems().expect("initialized") {
                if !system.is_send() {
                    non_send.push(key);
                }
                // The build inserts the `ApplyDeferred` sync points; name them so a census can
                // count them.
                systems.entry(key).or_insert_with(|| SystemInfo {
                    name: system.name().to_string(),
                    sets: Vec::new(),
                    conditions: Vec::new(),
                });
            }
            let mut components = HashMap::new();
            let conflicts = schedule
                .graph()
                .conflicting_systems()
                .0
                .iter()
                .map(|(a, b, ids)| {
                    for id in ids.iter() {
                        components.entry(*id).or_insert_with(|| {
                            world
                                .components()
                                .get_name(*id)
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| format!("{id:?}"))
                        });
                    }
                    (*a, *b, ids.to_vec())
                })
                .collect();
            let classes = Classes::read(world);
            Census {
                systems,
                conflicts,
                components,
                classes,
                dependencies,
                non_send,
                holds_vm,
                hosted,
                uses_self,
                built,
            }
        })
    }

    /// Does this build carry type names? Bevy has them only under its `debug` feature, which
    /// rides with `benilla-world`'s `dev`. Probed off the census, not a `cfg`, which belongs in
    /// `run_mode` alone; name-dependent tests skip without them.
    fn type_names_available(c: &Census) -> bool {
        c.systems.values().any(|s| s.name.contains("benilla_app::"))
    }

    /// The systems of one schedule whose declared access writes the resource `T`, each with every
    /// run condition on it or on a set it is under, by name.
    fn resource_writers<T: Resource>(
        app: &mut App,
        label: impl ScheduleLabel,
    ) -> Vec<(String, Vec<String>)> {
        app.world_mut()
            .schedule_scope(label.intern(), |world, schedule| {
                schedule.graph_mut().systems.initialize(world);
                let Some(id) = world.components().get_resource_id(TypeId::of::<T>()) else {
                    return Vec::new();
                };
                let graph = schedule.graph();
                let mut out = Vec::new();
                for (key, system, conds) in graph.systems.iter() {
                    let writes = graph
                        .systems
                        .get(key)
                        .is_some_and(|s| s.access.combined_access().has_resource_write(id));
                    if !writes {
                        continue;
                    }
                    let mut conditions: Vec<String> = conds
                        .iter()
                        .map(|c| c.condition.name().to_string())
                        .collect();
                    let mut stack = vec![NodeId::System(key)];
                    while let Some(node) = stack.pop() {
                        for parent in graph
                            .hierarchy()
                            .graph()
                            .neighbors_directed(node, Direction::Incoming)
                        {
                            if let NodeId::Set(set) = parent {
                                if let Some((_, set_conds)) = graph
                                    .system_sets
                                    .iter()
                                    .find(|(k, _, _)| *k == set)
                                    .map(|(_, s, c)| (s, c))
                                {
                                    conditions.extend(
                                        set_conds.iter().map(|c| c.condition.name().to_string()),
                                    );
                                }
                                stack.push(parent);
                            }
                        }
                    }
                    out.push((system.name().to_string(), conditions));
                }
                out
            })
    }

    /// The glue booth has one feeder, in any schedule: the glue VM's, which reads the stock
    /// screens' models off the VM. Beside it only the booth's own create instrument writes
    /// `GluePreview`.
    #[test]
    fn the_glue_preview_has_one_feeder() {
        let mut app = headless_client();
        let labels: Vec<_> = app
            .world()
            .resource::<bevy::ecs::schedule::Schedules>()
            .iter()
            .map(|(_, schedule)| schedule.label())
            .collect();
        let mut writers = Vec::new();
        for label in labels {
            writers.extend(resource_writers::<crate::portrait::GluePreview>(
                &mut app, label,
            ));
        }
        if !writers.iter().any(|(n, _)| n.contains("benilla_app::")) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let mut names: Vec<&str> = writers.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names,
            [
                "benilla_app::portrait::glue_booth::drive_create_test",
                "benilla_app::ui_script::glue_scene::feed_stock_glue_preview",
            ],
            "the GluePreview writers"
        );
    }

    /// One row of the class table: the type's name for the message, and its `TypeId`.
    type ClassRow = (&'static str, fn() -> TypeId);

    /// The explained classes: what an undeclared order may be about with nothing to declare.
    ///
    /// - A non-`Send` owner (the Lua VM, the audio handles): its systems run on the main thread
    ///   one at a time, so no order among them is a race; which feed fires first is the `UiFeed`
    ///   phase's job. Derived: every registration that is not `Send + Sync`.
    /// - A pure cache, where a miss records itself (`NameCache`, the GameObject templates, the
    ///   page texts, `WorldAssets`, `Creatures`' display models): two misses for one key commute.
    ///   A cache that is also a window's state is not pure, and not here.
    /// - An append-only sink (`ChatLog`, `MessageSounds`, `UiErrorKeys`, `UiErrorTexts`):
    ///   writers commute, and a drain's order against a writer is one frame of latency.
    /// - A random stream (`SoundKits`, `AnimRng`): any interleaving of draws is a valid draw,
    ///   the reference's contract for its single stream.
    ///
    /// Not a class: a pair bevy reports with no component list has an exclusive system on one
    /// side (`bevy_ecs` `node.rs:625` never reads its access), and is actionable. A pair is
    /// explained when everything it fights over is in a class; only actionable pairs are
    /// ratcheted. A row is a claim about every writer of that type, made in review with its
    /// reason. Resolved by `TypeId`, so it works in the player build, which carries no names.
    pub(crate) struct Classes {
        /// The VM's own id, for the executor census.
        pub vm: Option<ComponentId>,
        pub non_send: HashSet<ComponentId>,
        pub caches: HashSet<ComponentId>,
        pub sinks: HashSet<ComponentId>,
        pub streams: HashSet<ComponentId>,
    }

    impl Classes {
        const CACHES: &[ClassRow] = &[
            ("NameCache", TypeId::of::<crate::names::NameCache>),
            (
                "GameObjectTemplates",
                TypeId::of::<crate::go_templates::GameObjectTemplates>,
            ),
            ("PageTexts", TypeId::of::<crate::ui_item_text::PageTexts>),
            ("WorldAssets", TypeId::of::<benilla_assets::WorldAssets>),
            ("Creatures", TypeId::of::<crate::entities::Creatures>),
        ];
        const SINKS: &[ClassRow] = &[
            ("ChatLog", TypeId::of::<crate::ui_chat::ChatLog>),
            ("MessageSounds", TypeId::of::<crate::sound::MessageSounds>),
            ("UiErrorKeys", TypeId::of::<crate::ui_action::UiErrorKeys>),
            ("UiErrorTexts", TypeId::of::<crate::ui_action::UiErrorTexts>),
        ];
        const STREAMS: &[ClassRow] = &[
            ("SoundKits", TypeId::of::<crate::sound::SoundKits>),
            // The client's one `rand()` stream: the doodad host, the creature driver, the
            // GameObject arm and the portrait booth draw from it in frame order, as the
            // reference's lanes draw from one TLS cell; declaring an order would invent a
            // determinism the reference lacks. It removes no pair today: those lanes already
            // conflict on `Query<&mut AnimationPlayer>`.
            ("AnimRng", TypeId::of::<benilla_assets::AnimRng>),
        ];

        /// Resolve the table against a world whose schedules have initialized; a row that
        /// resolves to nothing is stale, and the panic names it.
        fn read(world: &World) -> Self {
            let comps = world.components();
            let resolve = |rows: &[ClassRow]| -> HashSet<ComponentId> {
                rows.iter()
                    .map(|(name, type_id)| {
                        comps.get_resource_id(type_id()).unwrap_or_else(|| {
                            panic!("`{name}` is in the class table but is not a resource of this world")
                        })
                    })
                    .collect()
            };
            Self {
                vm: comps.get_resource_id(TypeId::of::<benilla_ui::script::UiScript>()),
                non_send: comps
                    .iter_registered()
                    .filter(|info| !info.is_send_and_sync())
                    .map(|info| info.id())
                    .collect(),
                caches: resolve(Self::CACHES),
                sinks: resolve(Self::SINKS),
                streams: resolve(Self::STREAMS),
            }
        }

        fn explains(&self, id: ComponentId) -> bool {
            self.non_send.contains(&id)
                || self.caches.contains(&id)
                || self.sinks.contains(&id)
                || self.streams.contains(&id)
        }

        /// Which class explains this pair; `None` if it is actionable.
        pub fn class_of(&self, what: &[ComponentId]) -> Option<&'static str> {
            // An exclusive system on one side: bevy cannot see its access, so no class applies.
            if what.is_empty() {
                return None;
            }
            if !what.iter().all(|id| self.explains(*id)) {
                return None;
            }
            Some(if self.vm_only(what) {
                "VM only"
            } else if self.non_send_only(what) {
                "non-Send owners"
            } else if what.iter().all(|id| self.caches.contains(id)) {
                "pure caches"
            } else if what.iter().all(|id| self.sinks.contains(id)) {
                "append-only sinks"
            } else if what.iter().all(|id| self.streams.contains(id)) {
                "random streams"
            } else {
                "mixed"
            })
        }

        pub fn vm_only(&self, what: &[ComponentId]) -> bool {
            what.iter().all(|id| Some(*id) == self.vm)
        }

        pub fn non_send_only(&self, what: &[ComponentId]) -> bool {
            what.iter().all(|id| self.non_send.contains(id))
        }
    }

    /// The worldport's sweep runs whether or not the interface is up, as the local player's
    /// destructor does (`0x5dd543`): nothing gates it on `ingame_ui_up`.
    #[test]
    fn the_worldport_sweep_is_not_gated_on_the_interface() {
        let mut app = headless_client();
        let update = census(&mut app, Update, SyncPoints::Built);
        if !type_names_available(&update) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let sweep = update
            .systems
            .values()
            .find(|s| s.name.ends_with("::sweep_on_worldport"))
            .expect("the worldport's sweep is in Update");
        assert!(
            !sweep
                .conditions
                .iter()
                .any(|c| c.ends_with("::ingame_ui_up")),
            "{:?}",
            sweep.conditions
        );
    }

    #[test]
    fn the_client_builds_headless() {
        let app = headless_client();
        assert!(app.world().contains_resource::<Schedules>());
    }

    /// `PostUpdate`'s undeclared-order pairs on the declared graph; `GlobalTransform` and the
    /// particle `EffectQuads` are most of it.
    const POST_UPDATE_CEILING: usize = 371;
    const POST_UPDATE_SLACK: usize = 20;
    /// The actionable pairs in `Update`: conflicting access, no declared order, and nothing
    /// [`Classes`] explains, so the executor orders them however the graph falls. The count may
    /// not rise past the ceiling, and the ceiling follows the count down; read on the declared
    /// graph ([`SyncPoints::Declared`]). `WOW_AMBIGUITY_DUMP=1` prints every pair.
    ///
    /// The largest part is `Transform` on disjoint lanes, populations that never intersect,
    /// which no class can express. Every system that lends the call host runs in the VM pass's
    /// one chain ([`crate::vm_pass`]), so no two of them pair, and the world's movers and readers
    /// of what a script call reaches order against its ends. The host's shared sinks
    /// ([`crate::vm_pass::shared_sinks`]) are written through `&self`, so their users declare
    /// reads and pair here only over something else: without the pass's order on them, 764 pairs
    /// of users would share a sink in no order, none of which this count would show. The pass
    /// orders every user (`every_user_of_a_shared_sink_runs_in_one_order`), so the count hides
    /// none. Known pairs accepted with a reason:
    /// - the ranged prop's two `AnimationPlayer` writers against the booth's and the quest
    ///   markers';
    /// - the fx attaches and `entities::update_display_models`/`attach_entity_visuals` against
    ///   the mat-anim tick over `MatAnimTable`/`UvAnimMaterials`: attach writes the rows a tick
    ///   would, off the same clock;
    /// - `ui_session::feed_interact_npc` against `query_cache::release_on_enter::<MailOpen>`: the
    ///   feed reads the mailbox's guid, and the release clears only the pending queries;
    /// - Click to Move's steer (`player::approach::steer_approach`) against what `steer_follow`
    ///   already pairs with: the steer is chained after follow's and writes the same `Player`
    ///   fields;
    /// - the terrain stream's spawn half (`terrain_stream::spawn_streamed_tiles`) against the
    ///   readers of `TerrainStreamer`, `Placements` and the asset stores its request half
    ///   (`stream_terrain`) already pairs with: the inner-window wait sits between the two, so a
    ///   pair the one system carried now counts on both; and `attach_inner_colliders` against the
    ///   other users of `PendingCollider`, `StreamActivity` and `TerrainStreamer`: it attaches
    ///   only the held inner tiles' terrain builds, and the others never touch those;
    /// - the missile arrival's log leg (`combat_log::on_missile_miss`) against nine `Transform`
    ///   writers, the disjoint-lane kind: `portrait::booth::face_booth_billboards`,
    ///   `portrait::glue_booth::sync_glue_booth` and `sync_glue_scene`,
    ///   `portrait::panes::sync_panes`, `portrait::sync_portraits`,
    ///   `portrait::test_bake::sync_test_portraits`,
    ///   `sound::emitter_pool::pump_emitters`, `sound::liquid_loop::drive_liquid_loops` and
    ///   `quest_markers::bake_seat_scale`. It reads its two units' poses for the display range;
    ///   those move booth, emitter and marker-seat entities, never a unit.
    ///
    /// Raising the ceiling is a claim that a new undeclared order is acceptable: make it with the
    /// reason read off the dump, or declare the order (`.after`, a set, a `chain`). A resource
    /// that commutes by construction belongs in [`Classes`].
    const UPDATE_ACTIONABLE_CEILING: usize = 1_708;
    const UPDATE_ACTIONABLE_SLACK: usize = 40;

    fn ratchet(what: &str, n: usize, ceiling: usize, slack: usize) {
        eprintln!("{what}: {n} ambiguous pairs (ceiling {ceiling}, slack {slack})");
        assert!(
            n <= ceiling,
            "{n} ambiguous pairs in {what}; the ceiling is {ceiling}. A new system runs in an \
             undeclared order against something it shares state with — declare the order \
             (`.after`, a set, a `chain`), or raise the ceiling here with the reason. \
             `WOW_AMBIGUITY_DUMP=1` on this test lists the pairs; a resource that commutes by \
             construction belongs in `Classes` instead."
        );
        assert!(
            n + slack >= ceiling,
            "only {n} ambiguous pairs in {what} and the ceiling still says {ceiling}. Lower the \
             ceiling to {n} so it keeps ratcheting."
        );
    }

    /// The ratchet. Ids, not names, so it runs in the player build too; prints the class
    /// census beside the number it holds.
    #[test]
    fn the_schedules_have_no_more_undeclared_orders_than_the_ceilings_say() {
        let mut app = headless_client();
        // The declared graph: a barrier bevy placed is not a declared order (`SyncPoints`).
        let update = census(&mut app, Update, SyncPoints::Declared);
        let post = census(&mut app, PostUpdate, SyncPoints::Declared);
        let mut explained: BTreeMap<&str, usize> = BTreeMap::new();
        let mut actionable = Vec::new();
        for (a, b, what) in &update.conflicts {
            match update.classes.class_of(what) {
                Some(class) => *explained.entry(class).or_default() += 1,
                None => actionable.push((*a, *b, what)),
            }
        }
        eprintln!(
            "Update: {} systems, {} ambiguous pairs; {} explained ({}); {} actionable",
            update
                .systems
                .values()
                .filter(|s| !is_sync_point(&s.name))
                .count(),
            update.conflicts.len(),
            update.conflicts.len() - actionable.len(),
            explained
                .iter()
                .map(|(class, n)| format!("{n} {class}"))
                .collect::<Vec<_>>()
                .join(", "),
            actionable.len(),
        );
        if std::env::var_os("WOW_AMBIGUITY_DUMP").is_some() {
            let mut rows: Vec<String> = actionable
                .iter()
                .map(|(a, b, what)| {
                    // Canonical order within the pair, so two dumps diff by text.
                    let (x, y) = if update.name(*a) <= update.name(*b) {
                        (*a, *b)
                    } else {
                        (*b, *a)
                    };
                    format!(
                        "  {}  <->  {}\n      on {}",
                        update.name(x),
                        update.name(y),
                        what.iter()
                            .map(|id| update.component(*id))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .collect();
            rows.sort();
            for r in rows {
                eprintln!("{r}");
            }
        }
        ratchet(
            "Update (actionable)",
            actionable.len(),
            UPDATE_ACTIONABLE_CEILING,
            UPDATE_ACTIONABLE_SLACK,
        );
        ratchet(
            "PostUpdate",
            post.conflicts.len(),
            POST_UPDATE_CEILING,
            POST_UPDATE_SLACK,
        );
    }

    /// The `ApplyDeferred` the build inserts between a system with commands and its dependents.
    fn is_sync_point(name: &str) -> bool {
        name == "bevy_ecs::apply_deferred"
    }

    /// `Update`'s sync points, the barriers the build inserts after a system with commands on
    /// the way to one ordered after it. The call host holds no commands, its seams queueing what
    /// they change on a unit for the pass's end ([`crate::creature_anim::UnitEdits`]), and the
    /// VM pass's links ignore the deferred buffers between them
    /// ([`crate::vm_pass::order_the_pass`]), so a link's commands wait for the first barrier past
    /// the chain. The count may not rise past the ceiling, and the ceiling follows it down. Read
    /// on the dev build, whose schedule holds every system the player build does.
    const UPDATE_SYNC_POINT_CEILING: usize = 54;
    const UPDATE_SYNC_POINT_SLACK: usize = 4;

    #[test]
    fn the_update_schedule_has_no_more_sync_points_than_its_ceiling() {
        let mut app = headless_client();
        let update = census(&mut app, Update, SyncPoints::Built);
        if !type_names_available(&update) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let n = update
            .systems
            .values()
            .filter(|s| is_sync_point(&s.name))
            .count();
        eprintln!(
            "Update: {n} sync points (ceiling {UPDATE_SYNC_POINT_CEILING}, slack \
             {UPDATE_SYNC_POINT_SLACK})"
        );
        assert!(
            n <= UPDATE_SYNC_POINT_CEILING,
            "{n} sync points in Update; the ceiling is {UPDATE_SYNC_POINT_CEILING}. A system with \
             commands is ordered before one that need not wait for them, or the VM pass links a \
             pair the declared order already holds: drop the edge, or raise the ceiling here with \
             the reason. `concurrency_census` prints the count."
        );
        assert!(
            n + UPDATE_SYNC_POINT_SLACK >= UPDATE_SYNC_POINT_CEILING,
            "only {n} sync points in Update and the ceiling still says \
             {UPDATE_SYNC_POINT_CEILING}. Lower the ceiling to {n} so it keeps ratcheting."
        );
    }

    /// How parallel could `Update` be? A greedy list schedule with unlimited threads: each wave
    /// takes every ready system that shares no undeclared conflict with one already in it, and
    /// non-`Send` systems are pairwise exclusive. Returns `(waves, critical_path)`: the frame's
    /// serial depth, and its floor, the longest declared chain.
    fn waves(
        c: &Census,
        keep_conflict: impl Fn(&[ComponentId]) -> bool,
        non_send: &[SystemKey],
    ) -> (usize, usize) {
        use std::collections::{HashMap, HashSet};
        // The build-inserted sync points are left out; their count is reported beside the waves.
        let mut order: Vec<SystemKey> = c
            .systems
            .iter()
            .filter(|(_, s)| !is_sync_point(&s.name))
            .map(|(k, _)| *k)
            .collect();
        order.sort_by(|a, b| c.name(*a).cmp(c.name(*b)));
        let mut preds: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        for (a, b) in &c.dependencies {
            preds.entry(*b).or_default().push(*a);
        }
        let mut exclusive: HashMap<SystemKey, HashSet<SystemKey>> = HashMap::new();
        for (a, b, what) in &c.conflicts {
            if keep_conflict(what) {
                exclusive.entry(*a).or_default().insert(*b);
                exclusive.entry(*b).or_default().insert(*a);
            }
        }
        for (i, a) in non_send.iter().enumerate() {
            for b in &non_send[i + 1..] {
                exclusive.entry(*a).or_default().insert(*b);
                exclusive.entry(*b).or_default().insert(*a);
            }
        }
        let mut done: HashSet<SystemKey> = HashSet::new();
        let mut waves = 0;
        while done.len() < order.len() {
            let mut wave: Vec<SystemKey> = Vec::new();
            for k in &order {
                if done.contains(k) {
                    continue;
                }
                let ready = preds
                    .get(k)
                    .is_none_or(|p| p.iter().all(|x| done.contains(x)));
                if !ready {
                    continue;
                }
                let clash = exclusive
                    .get(k)
                    .is_some_and(|ex| wave.iter().any(|w| ex.contains(w)));
                if !clash {
                    wave.push(*k);
                }
            }
            assert!(
                !wave.is_empty(),
                "no system is ready — the dependency graph has a cycle"
            );
            done.extend(wave.iter().copied());
            waves += 1;
        }
        // The longest declared chain, by depth over the dependency DAG.
        let mut depth: HashMap<SystemKey, usize> = HashMap::new();
        fn depth_of(
            k: SystemKey,
            preds: &HashMap<SystemKey, Vec<SystemKey>>,
            depth: &mut HashMap<SystemKey, usize>,
        ) -> usize {
            if let Some(d) = depth.get(&k) {
                return *d;
            }
            let d = 1 + preds
                .get(&k)
                .map(|p| {
                    p.iter()
                        .map(|x| depth_of(*x, preds, depth))
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            depth.insert(k, d);
            d
        }
        let critical = order
            .iter()
            .map(|k| depth_of(*k, &preds, &mut depth))
            .max()
            .unwrap_or(0);
        (waves, critical)
    }

    /// An instrument, not a gate: prints `Update`'s serial depth as-is and with the VM, then
    /// the audio layer too, made `Send`-owned. Asserts nothing; run it by hand.
    #[test]
    #[ignore = "instrument: run by hand (the executor census) — cargo test -p benilla-app --lib concurrency_census -- --ignored --nocapture"]
    fn concurrency_census() {
        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Built);
        let holds = |k: SystemKey, pick: &dyn Fn(ComponentId) -> bool| {
            c.conflicts
                .iter()
                .any(|(a, b, w)| (*a == k || *b == k) && w.iter().any(|id| pick(*id)))
        };
        let vm: Vec<SystemKey> = c
            .non_send
            .iter()
            .copied()
            .filter(|k| c.holds_vm.contains(k))
            .collect();
        let audio: Vec<SystemKey> = c
            .non_send
            .iter()
            .copied()
            .filter(|k| {
                holds(*k, &|id| {
                    c.classes.non_send.contains(&id) && Some(id) != c.classes.vm
                })
            })
            .collect();
        let other_non_send: Vec<SystemKey> = c
            .non_send
            .iter()
            .copied()
            .filter(|k| !vm.contains(k) && !audio.contains(k))
            .collect();
        let real = c
            .systems
            .values()
            .filter(|s| !is_sync_point(&s.name))
            .count();
        eprintln!(
            "Update: {real} systems; {} non-Send ({} touch the VM, {} the audio layer, {} neither); {} declared edges; {} ambiguous pairs",
            c.non_send.len() - c.non_send.iter().filter(|k| is_sync_point(c.name(**k))).count(), vm.len(), audio.len(), other_non_send.len() - c.non_send.iter().filter(|k| is_sync_point(c.name(**k))).count(), c.dependencies.len(), c.conflicts.len()
        );
        let sync_points = c
            .systems
            .values()
            .filter(|s| is_sync_point(&s.name))
            .count();
        let mut neither: Vec<&str> = other_non_send
            .iter()
            .map(|k| c.name(*k))
            .filter(|n| !is_sync_point(n))
            .collect();
        neither.sort_unstable();
        eprintln!(
            "  {sync_points} auto-inserted ApplyDeferred sync points (exclusive barriers); other non-Send: {}",
            neither.join(", ")
        );
        let (w0, cp) = waves(&c, |_| true, &c.non_send);
        eprintln!("  as-is:                 {w0} waves (critical path {cp})");
        let not_vm: Vec<SystemKey> = c
            .non_send
            .iter()
            .copied()
            .filter(|k| !vm.contains(k) || audio.contains(k))
            .collect();
        let (w1, _) = waves(&c, |w| !c.classes.vm_only(w), &not_vm);
        eprintln!("  VM Send-owned:         {w1} waves");
        let (w2, _) = waves(&c, |w| !c.classes.non_send_only(w), &other_non_send);
        eprintln!("  VM + audio Send-owned: {w2} waves");
        let (w3, _) = waves(&c, |_| false, &[]);
        eprintln!("  declared edges only:   {w3} waves (every conflict ordered, everything Send)");
    }

    /// Why a VM holder in `Update` ordered before the tick may stay out of
    /// [`crate::ui_script::UiFeed`], keyed by a suffix of the system's full name, with the
    /// reason.
    const OUTSIDE_THE_FEED_PHASE: &[(&str, &str)] = &[];

    /// The create's control restore (`player::regain_control_on_arrival`) is registered in the
    /// built client and ordered: after the self create it answers, before the controller and the
    /// readers of the flags it clears. A lost `add_systems` line, or a lost `.after`, fails here.
    #[test]
    fn the_control_restore_runs_after_the_create_and_before_its_readers() {
        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Declared);
        if !type_names_available(&c) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let one = |suffix: &str| -> SystemKey {
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Update"),
            }
        };
        let mut succ: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        for (a, b) in &c.dependencies {
            succ.entry(*a).or_default().push(*b);
        }
        let reach = |start: SystemKey| {
            let mut seen = HashSet::new();
            let mut stack = vec![start];
            while let Some(n) = stack.pop() {
                for m in succ.get(&n).into_iter().flatten() {
                    if seen.insert(*m) {
                        stack.push(*m);
                    }
                }
            }
            seen
        };
        let restore = one("::wire_in::regain_control_on_arrival");
        let create = one("::net::apply::enter_world_on_self_create");
        assert!(
            reach(create).contains(&restore),
            "the restore runs after the self create"
        );
        let after_restore = reach(restore);
        for reader in [
            "::player::controller::control",
            "::ui_unit::feed_player_control",
            "::ui_action::state::feed_action_state",
            "::ui_shapeshift::feed_shapeshift_bar",
            "::player::camera_saved::load_camera_pose",
            "::tutorial::watch_self",
            "::ui_trade::answer_trade_request",
            "::poi_marker::expire_marker",
            "::glue_session::logout_smoke::debug_logout_smoke",
        ] {
            assert!(
                after_restore.contains(&one(reader)),
                "`{reader}` runs after the control restore"
            );
        }
    }

    /// Why an `Update` system that names [`crate::net::SelfPlayer`] may run with no declared order
    /// after `tag_self_player`, keyed by a fragment of its full name (a generic's whole family is one row), with the reason it
    /// is safe. The world-enter drain spawns our entity and every creature in one frame and the tag comes
    /// later in the Net stage, so a system between the two sees no self.
    const UNORDERED_AFTER_THE_SELF_TAG: &[(&str, &str)] = &[
        ("::ui_chat::language::feed_language_skills",
         "with no self it resets to the default list and a `have_player` flag forces a recompute once self exists"),
        ("::ui_chat::language::feed_default_language",
         "with no self it resets to the default language and a `have_player` flag forces a recompute once self exists"),
        ("::sound::money::play_coin_on_coinage_change",
         "the first sighting of the coinage only seeds the memo, so a frame with no self plays nothing and loses nothing"),
        ("::ui_script::sweep::arm_leaving_world_on_self_create",
         "`Added<SelfPlayer>` is relative to this system's own last run, so a tag that lands after a run is still seen at the next"),
        ("::ui_session::release::release_on_field_edges",
         "it is in Net after `apply_net_updates`, unordered against the tag; its self-only branches need `old > 0` (the death edge) or a form change on a latch that is not open, which a create burst never raises"),
        ("::death::drive_death_look",
         "level-driven: it writes the fade from the ghost flag every frame and re-reads self each time, so a frame with no self is overwritten by the next"),
        ("::tutorial::run_world_enter_cascade",
         "`armed` stays set until self exists, so the welcome and questgiver triggers wait for the tag rather than being spent, and the arrival's `Added<SelfPlayer>` is relative to this system's own last run, so a tag that lands after a run is still seen at the next"),
        ("::ui_session::close_npc_session_out_of_range<",
         "no self returns before anything changes, and a question open on an NPC needs a self that interacted; the leash is re-tested every frame"),
        ("::ui_session::latch::watch_interaction_range",
         "no self returns before anything changes, and a latch needs a self that interacted; the leash is re-tested every frame"),
    ];

    /// Every `Update` system that names `SelfPlayer` (reads it, or filters on it) is ordered after
    /// `tag_self_player`, or argued in [`UNORDERED_AFTER_THE_SELF_TAG`]: a system that reads self
    /// and uses change detection must run after the tag, or a frame that finds no self still
    /// advances its change ticks and loses what changed. Fails an unordered user, a row for a
    /// system that is ordered after all, and a stale row.
    #[test]
    fn every_system_that_names_self_runs_after_the_tag_or_argues_why_not() {
        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Declared);
        if !type_names_available(&c) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let tag = {
            let suffix = "::net::apply::tag_self_player";
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Update"),
            }
        };
        let mut succ: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        for (a, b) in &c.dependencies {
            succ.entry(*a).or_default().push(*b);
        }
        let reach = |start: SystemKey| {
            let mut seen = HashSet::new();
            let mut stack = vec![start];
            while let Some(n) = stack.pop() {
                for m in succ.get(&n).into_iter().flatten() {
                    if seen.insert(*m) {
                        stack.push(*m);
                    }
                }
            }
            seen
        };
        let after_tag = reach(tag);
        let mut users: Vec<SystemKey> = c.uses_self.iter().copied().filter(|k| *k != tag).collect();
        users.sort_by(|a, b| c.name(*a).cmp(c.name(*b)));
        let (mut ordered, mut argued) = (0, 0);
        let mut offenders = Vec::new();
        let mut stale = Vec::new();
        for k in &users {
            let name = c.name(*k);
            let row = UNORDERED_AFTER_THE_SELF_TAG
                .iter()
                .find(|(part, _)| name.contains(part));
            match (after_tag.contains(k), row.is_some()) {
                (true, false) => ordered += 1,
                (true, true) => stale.push(format!(
                    "{name}: argued in UNORDERED_AFTER_THE_SELF_TAG but ordered after the tag — drop the row"
                )),
                (false, true) => argued += 1,
                (false, false) => offenders.push(name.to_string()),
            }
        }
        for (part, _) in UNORDERED_AFTER_THE_SELF_TAG {
            if !users.iter().any(|k| c.name(*k).contains(part)) {
                stale.push(format!(
                    "`…{part}…` does not name SelfPlayer in Update — drop its row"
                ));
            }
        }
        eprintln!(
            "SelfPlayer users in Update: {} — {ordered} after the tag, {argued} argued",
            users.len()
        );
        assert!(
            offenders.is_empty(),
            "these systems name `SelfPlayer` in `Update` with no declared order after \
             `tag_self_player` — `.after(WorldStage::Net)`, or argue it in \
             UNORDERED_AFTER_THE_SELF_TAG:\n  {}",
            offenders.join("\n  ")
        );
        assert!(stale.is_empty(), "stale rows:\n  {}", stale.join("\n  "));
    }

    /// A nameplate's right-click is `SelectTarget` (`0x492870`), which with a targeting cursor up
    /// binds the cast and returns, and then the unit's interact (`0x492881`), so the cast goes out
    /// ahead of what the interact does. The click's select parks the unit; the target chain's bind
    /// gives it to the cast; the interact runs after that, or its attack leg would swing before the
    /// cast and select nothing.
    #[test]
    fn a_plates_interact_runs_after_the_bind_of_the_unit_its_click_parked() {
        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Declared);
        if !type_names_available(&c) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let one = |suffix: &str| {
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Update"),
            }
        };
        let (select, bind, interact) = (
            one("::target::click::select_on_plate_click"),
            one("::script_calls::bind_parked_picks"),
            one("::target::click::act_on_interact"),
        );
        let mut succ: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        for (a, b) in &c.dependencies {
            succ.entry(*a).or_default().push(*b);
        }
        let runs_before = |from: SystemKey, to: SystemKey| {
            let mut seen = HashSet::new();
            let mut stack = vec![from];
            while let Some(n) = stack.pop() {
                for m in succ.get(&n).into_iter().flatten() {
                    if seen.insert(*m) {
                        stack.push(*m);
                    }
                }
            }
            seen.contains(&to)
        };
        assert!(
            runs_before(select, bind),
            "the click parks before the chain binds"
        );
        assert!(
            runs_before(bind, interact),
            "the interact runs after the chain's bind"
        );
    }

    /// The `Startup` VM, the stock glue's, is built after the glyph cache:
    /// its first screen sizes frames by measured text in the show (`TOSFrameHeader` off
    /// `TOSFrameTitle:GetWidth()`, `AccountLogin.lua`), which with no cache measures 0.
    #[test]
    fn the_startup_vm_is_built_after_the_glyph_cache() {
        let mut app = headless_client();
        // `Update` first: its systems register the resources the census's class table names.
        let _ = census(&mut app, Update, SyncPoints::Declared);
        let c = census(&mut app, Startup, SyncPoints::Declared);
        if !type_names_available(&c) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let one = |suffix: &str| -> SystemKey {
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Startup"),
            }
        };
        let mut succ: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        for (a, b) in &c.dependencies {
            succ.entry(*a).or_default().push(*b);
        }
        let (atlas, vm) = (
            one("::ui_text::engine::gpu::init"),
            one("::ui_script::lifecycle::setup_script"),
        );
        let mut seen = HashSet::new();
        let mut stack = vec![atlas];
        while let Some(n) = stack.pop() {
            for m in succ.get(&n).into_iter().flatten() {
                if seen.insert(*m) {
                    stack.push(*m);
                }
            }
        }
        assert!(seen.contains(&vm), "the VM is built after the glyph cache");
    }

    /// The party feed fires `RAID_TARGET_UPDATE` after the unit feed has pushed this frame's
    /// snapshots: TargetFrame answers it with `GetRaidTargetIndex("target")`, which reads the
    /// target snapshot's `raid_target`, so a party feed that ran first left a new mark off the
    /// target frame until the target changed. Read off the declared graph.
    #[test]
    fn the_party_feed_runs_after_the_unit_snapshots() {
        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Declared);
        if !type_names_available(&c) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let one = |suffix: &str| -> SystemKey {
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Update"),
            }
        };
        let mut succ: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        for (a, b) in &c.dependencies {
            succ.entry(*a).or_default().push(*b);
        }
        let (units, party) = (
            one("::ui_unit::feed_units"),
            one("::ui_party::feed::feed_party"),
        );
        let mut seen = HashSet::new();
        let mut stack = vec![units];
        while let Some(n) = stack.pop() {
            for m in succ.get(&n).into_iter().flatten() {
                if seen.insert(*m) {
                    stack.push(*m);
                }
            }
        }
        assert!(
            seen.contains(&party),
            "feed_party runs after feed_units, whose snapshots RAID_TARGET_UPDATE's handler reads"
        );
    }

    /// The VM ticks once a frame (`extract::tick_script`, in `UiInput`): a push the tick must
    /// see rides `UiFeed`, chained after the net drain and before the tick; a drain of what it
    /// produced is `.after(UiInput)`. Read off the built graph. Fails a holder with no path to or
    /// from the tick, one before the tick outside `UiFeed` and not argued in
    /// [`OUTSIDE_THE_FEED_PHASE`], a `UiFeed` member the graph does not place between the drain
    /// and the tick, and a stale row.
    #[test]
    fn every_vm_holder_in_update_declares_its_side_of_the_tick() {
        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Built);
        if !type_names_available(&c) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let one = |suffix: &str| -> SystemKey {
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Update"),
            }
        };
        let tick = one("::extract::tick_script");
        let drain = one("::net::apply::apply_net_updates");
        let mut succ: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        let mut pred: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        for (a, b) in &c.dependencies {
            succ.entry(*a).or_default().push(*b);
            pred.entry(*b).or_default().push(*a);
        }
        let reach = |start: SystemKey, edges: &HashMap<SystemKey, Vec<SystemKey>>| {
            let mut seen = HashSet::new();
            let mut stack = vec![start];
            while let Some(n) = stack.pop() {
                for m in edges.get(&n).into_iter().flatten() {
                    if seen.insert(*m) {
                        stack.push(*m);
                    }
                }
            }
            seen
        };
        let before_tick = reach(tick, &pred);
        let after_tick = reach(tick, &succ);
        let after_drain = reach(drain, &succ);
        let mut holders: Vec<SystemKey> = c.holds_vm.iter().copied().collect();
        holders.sort_by(|a, b| c.name(*a).cmp(c.name(*b)));
        let (mut feed, mut post, mut argued) = (0, 0, 0);
        let mut offenders = Vec::new();
        let mut stale = Vec::new();
        for k in &holders {
            if *k == tick {
                continue;
            }
            let s = &c.systems[k];
            let in_feed = s.sets.iter().any(|set| set == "UiFeed");
            let row = OUTSIDE_THE_FEED_PHASE
                .iter()
                .find(|(suffix, _)| s.name.ends_with(suffix));
            if in_feed {
                feed += 1;
                if !(after_drain.contains(k) && before_tick.contains(k)) {
                    offenders.push(format!(
                        "{}: in `UiFeed`, but the graph does not place it after the net drain and before the tick",
                        s.name
                    ));
                }
            } else if after_tick.contains(k) {
                post += 1;
            } else if before_tick.contains(k) {
                if row.is_some() {
                    argued += 1;
                    continue;
                }
                offenders.push(format!(
                    "{}: ordered before the tick but not in `UiFeed` — it may run before this frame's packets land; `.in_set(UiFeed)`",
                    s.name
                ));
            } else {
                offenders.push(format!(
                    "{}: no declared side of the tick — `.in_set(UiFeed)` for a push the tick must \
                     see this frame, `.in_set(crate::ui_script::UiDrain)` for a drain of what the \
                     tick produced",
                    s.name
                ));
            }
            if row.is_some() {
                stale.push(format!(
                    "{}: argued in OUTSIDE_THE_FEED_PHASE but the graph places it {} — drop the row",
                    s.name,
                    if in_feed {
                        "in the feed phase"
                    } else {
                        "after the tick"
                    }
                ));
            }
        }
        for (suffix, _) in OUTSIDE_THE_FEED_PHASE {
            if !holders.iter().any(|k| c.name(*k).ends_with(suffix)) {
                stale.push(format!(
                    "`…{suffix}` is not a VM holder in Update — drop its row"
                ));
            }
        }
        eprintln!(
            "VM holders in Update: {} — the tick, {feed} in the feed phase, {post} after the tick, {argued} argued outside the phase",
            holders.len()
        );
        assert!(
            offenders.is_empty(),
            "these systems hold the VM in `Update` without declaring their side of the tick \
             (`UiFeed` before it, or `.in_set(crate::ui_script::UiDrain)`):\n  {}",
            offenders.join("\n  ")
        );
        assert!(stale.is_empty(), "stale rows:\n  {}", stale.join("\n  "));
    }

    /// The consumer markers: a VM holder that does one of these to host state has spent
    /// something this frame that nothing can re-spend. A feed posts its events
    /// (`script_calls::Fires::post`) to fire once its pushes are in.
    const CONSUMES: &[&str] = &["fire_event(", "fires.post(", "mem::take(", ".drain("];

    /// Why an ungated one-shot consumer holding the VM loses nothing in the one-frame window
    /// where the wire is in world and the glue VM is still live.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Because {
        /// The consumed queue is filled only by Lua asking; a frameless VM asks for nothing.
        FilledByVm,
        /// Filled only by a server reply to a click in the interface, or (noted in the reason)
        /// another player's act on us, which meets the login burst only by coincidence.
        PlayerRoundTrip,
        /// Consumed in the window or not, the state is re-asked or re-derived once the UI is up.
        SelfHealing,
        /// The publication is driven by a `VmMemo` diff, so a new VM re-derives and re-fires it.
        MemoLatched,
        /// Documented at the line as intentional.
        Deliberate,
    }

    /// Every one-shot VM consumer not gated on `ingame_ui_up` (read off the schedule) and not a
    /// memo-diffed `fire_event`, keyed `(path under src/, fn)`, with the reason it is safe.
    const EXEMPT: &[(&str, &str, Because, &str)] = &[
        ("bindings.rs", "sync_dispatch", Because::MemoLatched,
         "`seen_generation` is a `VmMemo`: a new VM reads `None` and rebuilds, and its seed's load owes the UPDATE_BINDINGS"),
        ("capture/probe_bg.rs", "bg_probe", Because::SelfHealing,
         "the battleground probe: dev-only (`WOW_PROBE_BG`, `cfg(feature = \"dev\")`) so it is not in a player build at all, and its `mem::take` is of its OWN pending-events string, not a queue anything else fills. Its one real VM dependency is the Lua event tap, which self-heals: `EVENT_DRAIN` returns a `<tap-gone>` sentinel when the tap's globals are missing — the case this window causes, since a tap installed in the glue VM is discarded when `mint_entry_vm` builds the interface — and the probe re-installs on the next frame"),
        ("capture/probes/act.rs", "fire_probe_drag", Because::FilledByVm,
         "the drag probe: dev-only (`WOW_PROBE_DRAG`), it starts past `WOW_PROBE_DRAG_AT` with the \
          player's body in the world, and its `.drain(..)` is of the VM's own script errors, \
          raised by the gesture it just made"),
        ("death/mod.rs", "feed_death", Because::MemoLatched,
         "`feed.vm: VmMemo<DeathAnnounced>`: a fresh memo makes the first snapshot an edge and re-announces a held offer, confirm and corpse range"),
        ("net/objects/update.rs", "on_update_end", Because::Deliberate,
         "a net handler whose take is the packet's own pass, the blocks its write pass recorded, documented at the line: it publishes nothing to the VM, whose `UNKNOWNOBJECT` text it reads only to name a party member's pet; its notices go to the gated field-notice firing"),
        ("screenshot.rs", "ask_for_captures", Because::FilledByVm,
         "`pending` holds only the VM's own `Screenshot()` asks, and the take spawns a capture, publishing nothing"),
        ("screenshot.rs", "report_captures", Because::FilledByVm,
         "the outcome chain starts with the VM's own `Screenshot()` call"),
        ("tutorial.rs", "drain_tutorials", Because::FilledByVm,
         "`sends` exist only after Lua acknowledged, cleared or reset a flag, and go to the wire"),
        ("ui_action/drain.rs", "drain_click_casts", Because::FilledByVm,
         "filled only by a world right-click on a GameObject or a body; the drain sends casts to the wire"),
        ("ui_auction/mod.rs", "drain_auction", Because::FilledByVm,
         "VM verbs; the refresh flags act only under an open window and become wire re-asks"),
        ("ui_bank/mod.rs", "feed_bank", Because::PlayerRoundTrip,
         "`BankErrors` answers a `BuyBankSlot` click; the OPENED edge rides `VmMemo`s"),
        ("ui_battlefield.rs", "feed_battlefield", Because::SelfHealing,
         "`reset_on_world_enter` runs before it at each world-enter cascade, clears the session and re-sends `BattlefieldStatusRequest`"),
        ("ui_binder.rs", "feed_binder", Because::PlayerRoundTrip,
         "`SMSG_BINDER_CONFIRM` only answers the innkeeper's gossip line"),
        ("ui_char.rs", "feed_char", Because::MemoLatched,
         "`feed.vm: VmMemo<CharFeedMemo>`; `vm_reset` fires every transition on a fresh memo"),
        ("ui_chat/feed.rs", "played_time_bridge", Because::FilledByVm,
         "the answer it takes follows `RequestTimePlayed()`, Lua's own ask, which a frameless VM \
          never makes"),
        ("ui_dialog_verbs.rs", "feed_meeting_stone", Because::SelfHealing,
         "the query waits for the world-enter cascade (`meeting_stone_enter_world`), our own create a server round trip past the login edge, and every `/reload` re-asks onto its new VM"),
        ("ui_duel.rs", "feed_duel", Because::PlayerRoundTrip,
         "a duel exists only after someone's Duel cast, and the server ends any duel at logout; FINISHED/bounds ride `VmMemo<FedDuel>` (another player's act: coincidence-only residual)"),
        ("ui_follow.rs", "feed_follow", Because::MemoLatched,
         "`feed.vm: VmMemo<FollowFeedMemo>`; a fresh VM re-fires BEGIN for a follow in progress"),
        ("ui_gm_ticket.rs", "feed_gm_ticket", Because::MemoLatched,
         "`feed.vm: VmMemo<FedTicket>` counters restart per VM while `state.answers` keeps counting, so the latest answer is re-fired"),
        ("ui_honor.rs", "feed_honor", Because::MemoLatched,
         "`state.vm: VmMemo<HonorFeedMemo>`; a fresh VM is pushed the block again and re-fires `INSPECT_HONOR_UPDATE` for a held reply"),
        ("ui_item_text.rs", "feed_item_text", Because::MemoLatched,
         "`told` is a `VmMemo` inside the session (a fresh VM re-begins), and the open is a player click"),
        ("ui_items/locks.rs", "apply_item_releases", Because::Deliberate,
         "the item creates it consumes before the world-entry interface is up precede the reference's world latch (`[0xb4b424]`, set at `0x4908ce`, cleared at `0x490a8d`), which the arrival unlock reads (`0x5d84ff`): they unlock nothing, and the watch the world entry's seed resets takes them as its baseline"),
        ("ui_logout.rs", "feed_logout", Because::PlayerRoundTrip,
         "both packets answer the `CMSG_LOGOUT_REQUEST`/cancel the game menu sent"),
        ("ui_loot/mod.rs", "drain_loot", Because::Deliberate,
         "the pre-VM takes of `LootMoveStart` and `LootCloseAtRest` are documented at the line and publish nothing to the VM; the event-firing takes are Lua's queues"),
        ("ui_loot/mod.rs", "feed_loot", Because::PlayerRoundTrip,
         "it takes `LootMoveStart` only on a window it has just opened, which answers a loot click"),
        ("ui_loot_roll.rs", "feed_loot_rolls", Because::PlayerRoundTrip,
         "a roll exists only after a group member loots under group loot; leftovers are cleared at socket teardown (another player's act: coincidence-only residual)"),
        ("ui_macro/mod.rs", "load_macros", Because::MemoLatched,
         "`MacroFiles.identity` is a `VmMemo`, so a new session re-reads the files and re-fires UPDATE_MACROS; also `InWorldGated`"),
        ("ui_macro/mod.rs", "save_dirty_macros", Because::FilledByVm,
         "`take_macros_dirty` is raised only by the macro window's Lua"),
        ("ui_mail/mod.rs", "feed_mail", Because::SelfHealing,
         "the window queues are mailbox-click replies; the one server push (`SMSG_RECEIVED_MAIL`) is re-asked by `send_query_next_mail_time_on_enter`, whose reply sets `notify` unconditionally"),
        ("ui_merchant/mod.rs", "feed_merchant", Because::PlayerRoundTrip,
         "a buy/sell refusal answers a click on an open merchant; show/update ride `VmMemo`s"),
        ("ui_panes.rs", "feed_character_panes", Because::FilledByVm,
         "the answers answer the VM's own build requests and arms on panes it listed; a frameless VM lists none, and an answer for another VM's rows is dropped"),
        ("ui_petition/feed.rs", "feed_petition", Because::PlayerRoundTrip,
         "every line answers a charter action of ours; the window edges ride the `fed` memo"),
        ("ui_quest_share.rs", "feed_quest_share", Because::PlayerRoundTrip,
         "a verdict answers our own push and a confirm follows a party member's escort accept, both held until the name resolves (the confirm is another player's act: coincidence-only residual)"),
        ("ui_script/glue_mgr.rs", "drive_glue_mgr", Because::Deliberate,
         "it acts only on a glue-kind VM (`script.filter(vm_kind() == Glue)`, documented at the fire sites), never the entry VM the window leaves frameless; news that arrives with no glue VM is held in `AddonInfoReply` for the next one"),
        ("ui_script/extract/mod.rs", "paint_script", Because::Deliberate,
         "per-frame paint and cost state, not a queue"),
        ("ui_social/feed.rs", "feed_social", Because::Deliberate,
         "its take is `name_events`, the packets' ask markers: taken before the interface is up they still send their name queries, to the server and not the VM, and the lists and lines are read again as the names land"),
        ("ui_stable/mod.rs", "feed_stable", Because::PlayerRoundTrip,
         "both packets follow a stable master's gossip and a click in its window"),
        ("ui_summon.rs", "feed_summon", Because::PlayerRoundTrip,
         "only another player's Ritual of Summoning latches the ask (the duel's shape; coincidence-only residual)"),
        ("ui_tabard.rs", "drain_tabard", Because::FilledByVm,
         "the intents are Lua's; the event answers a Save intent"),
        ("ui_tabard.rs", "feed_tabard", Because::PlayerRoundTrip,
         "every latch follows a tabard-vendor click, and `reset_on_world_enter` zeroes the resource on entry"),
        ("ui_talent_wipe.rs", "feed_talent_wipe", Because::PlayerRoundTrip,
         "`MSG_TALENT_WIPE_CONFIRM` answers the trainer's gossip option"),
        ("ui_taxi/mod.rs", "feed_taxi", Because::PlayerRoundTrip,
         "both packets follow talking to a flight master"),
        ("ui_trade/mod.rs", "feed_trade", Because::Deliberate,
         "its take is `named_lines_asked`, how many held lines have been asked: taken before the interface is up the lines' names are still asked, to the server and not the VM, and the lines stay held until they show"),
        ("ui_trainer/mod.rs", "feed_trainer", Because::PlayerRoundTrip,
         "a refusal answers a Train click and a list answers a trainer gossip click; the edges ride `VmMemo`s"),
    ];

    /// `ui_chat/feed.rs` → `ui_chat::feed`; `target/mod.rs` → `target`; `lib.rs` → ``.
    fn module_of(rel: &str) -> String {
        let no_ext = rel.trim_end_matches(".rs");
        let no_mod = no_ext.trim_end_matches("/mod");
        if no_mod == "lib" {
            String::new()
        } else {
            no_mod.replace('/', "::")
        }
    }

    /// A consumer (a system taking `NonSend[Mut]<UiScript>`, a `HostedVm` or a `VmWith`, whose body
    /// has a [`CONSUMES`] marker) passes if the schedule shows `ingame_ui_up` on it or a set above
    /// it, if it is a `fire_event` driven by a `VmMemo` parameter, or if [`EXEMPT`] argues it. Read
    /// off the schedule, where a gate on a set, a tuple or the member all look alike. A stale
    /// `EXEMPT` row fails too.
    #[test]
    fn every_one_shot_consumer_that_holds_the_vm_is_gated_or_argued() {
        use crate::test_support::{fn_items, rel_path, rust_files, src_dir};
        let mut consumers = Vec::new();
        for file in rust_files(&src_dir()) {
            let text = std::fs::read_to_string(&file).expect("readable source");
            if !text.contains("UiScript") && !text.contains("HostedVm") {
                continue;
            }
            let rel = rel_path(&file);
            for f in fn_items(&text) {
                let holds_vm = ["NonSendMut<", "NonSend<"].iter().any(|t| {
                    f.params.contains(&format!("{t}UiScript>"))
                        || f.params
                            .contains(&format!("{t}benilla_ui::script::UiScript>"))
                }) || f.params.contains("HostedVm")
                    || f.params.contains("VmWith<");
                if !holds_vm || !CONSUMES.iter().any(|m| f.body.contains(m)) {
                    continue;
                }
                let takes = f.body.contains("mem::take(") || f.body.contains(".drain(");
                // A `WindowMemo` is a `VmMemo` the leave-world sweep also clears.
                let memo = f.params.contains("VmMemo<") || f.params.contains("WindowMemo<");
                consumers.push((rel.clone(), f.name.to_string(), takes, memo));
            }
        }
        let mut app = headless_client();
        let update = census(&mut app, Update, SyncPoints::Built);
        if !type_names_available(&update) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let post = census(&mut app, PostUpdate, SyncPoints::Built);
        // `PreUpdate` holds the frame's head: the script memory watch, ahead of the reload.
        let pre = census(&mut app, PreUpdate, SyncPoints::Built);
        // The leave-world sweep's legs run only inside its schedule, whose callers hold the
        // interface up: the worldport's producer is gated with the unit feed, the shutdown edges
        // run the sweep before a parked glue VM returns, and the quit returns inside the
        // entry-load window.
        let swept: Vec<String> = census(
            &mut app,
            crate::ui_script::WorldLeaveSweep,
            SyncPoints::Built,
        )
        .systems
        .values()
        .map(|s| s.name.clone())
        .collect();
        let in_sweep = |full: &str, name: &str| {
            let suffix = format!("::{name}");
            swept.iter().any(|s| s == full || s.ends_with(&suffix))
        };
        let by_name: HashMap<&str, &SystemInfo> = update
            .systems
            .values()
            .chain(post.systems.values())
            .chain(pre.systems.values())
            .map(|s| (s.name.as_str(), s))
            .collect();
        let mut gated = 0;
        let mut latched = 0;
        let mut argued = 0;
        let mut offenders = Vec::new();
        let mut stale: Vec<String> = Vec::new();
        consumers.sort();
        for (rel, name, takes, memo) in &consumers {
            let module = module_of(rel);
            let full = if module.is_empty() {
                format!("benilla_app::{name}")
            } else {
                format!("benilla_app::{module}::{name}")
            };
            let info = by_name.get(full.as_str()).copied().or_else(|| {
                let suffix = format!("::{name}");
                let mut hits = by_name.iter().filter(|(k, _)| k.ends_with(&suffix));
                match (hits.next(), hits.next()) {
                    (Some((_, v)), None) => Some(*v),
                    _ => None,
                }
            });
            let exempt = EXEMPT.iter().find(|(p, f, _, _)| p == rel && f == name);
            let is_gated = info
                .is_some_and(|i| i.conditions.iter().any(|c| c.ends_with("::ingame_ui_up")))
                || (info.is_none() && in_sweep(&full, name));
            if is_gated {
                gated += 1;
                if exempt.is_some() {
                    stale.push(format!(
                        "{rel}: `{name}` is gated on `ingame_ui_up` now — drop its EXEMPT row"
                    ));
                }
            } else if !*takes && *memo {
                latched += 1;
                if exempt.is_some() {
                    stale.push(format!(
                        "{rel}: `{name}` is a memo-diffed fire_event — drop its EXEMPT row"
                    ));
                }
            } else if exempt.is_some() {
                argued += 1;
            } else {
                // Name its sets: the fix is usually a gate on one of them.
                let where_ = match info {
                    None => " (not found in PreUpdate/Update/PostUpdate)".to_string(),
                    Some(i) => {
                        let named: Vec<&str> = i
                            .sets
                            .iter()
                            .filter(|s| !s.starts_with("SystemTypeSet"))
                            .map(String::as_str)
                            .collect();
                        format!(" (sets: {})", named.join(", "))
                    }
                };
                offenders.push(format!("{rel}: `{name}`{where_}"));
            }
        }
        for (p, f, _, _) in EXEMPT {
            if !consumers
                .iter()
                .any(|(rel, name, _, _)| rel == p && name == f)
            {
                stale.push(format!(
                    "{p}: `{f}` is not a one-shot consumer holding the VM — drop its EXEMPT row"
                ));
            }
        }
        eprintln!(
            "one-shot consumers holding the VM: {} — {gated} gated on ingame_ui_up, {latched} memo-latched, {argued} argued in EXEMPT",
            consumers.len()
        );
        assert!(
            offenders.is_empty(),
            "these systems hold the VM and consume a one-shot with nothing gating them on the \
             interface being up — in the one frame before it is, they publish to a VM with no \
             frames and the edge is lost. Gate them (`.run_if(ingame_ui_up)`, on the system \
             or its set), or add them to `EXEMPT` with the reason they cannot be filled before \
             the interface exists:\n  {}",
            offenders.join("\n  ")
        );
        assert!(
            stale.is_empty(),
            "stale EXEMPT rows:\n  {}",
            stale.join("\n  ")
        );
    }
}
