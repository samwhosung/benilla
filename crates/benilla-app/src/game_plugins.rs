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
use crate::ui_pet_doll::UiPetDollPlugin;
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
    /// [`crate::char_select::CharSelectPlugin::start`]: the screen this session opens on.
    pub(crate) start: crate::char_select::ClientState,
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
            // The combat log, ahead of the animation layer, whose two shared kinds run second.
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
            .add(crate::glue::GluePlugin)
            .add(crate::char_select::CharSelectPlugin { start: self.start })
            .add(crate::login::LoginPlugin)
            .add(crate::realm_select::RealmSelectPlugin)
            .add(crate::char_create::CharCreatePlugin)
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
            // Re-shapes a `bevy_ui` text root whose last span was despawned, an upstream hole
            // that otherwise panics in `bevy_text` on the next resize.
            .add(crate::text_reshape::TextReshapePlugin)
            .add(TutorialPlugin)
            .add(crate::swing_refusal::SwingRefusalPlugin)
            .add(UiSummonPlugin)
            .add(UiGmTicketPlugin)
            .add(UiFollowPlugin)
            .add(UiInstancePlugin)
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
            .add(UiPetDollPlugin)
            .add(UiNetPlugin)
            .add(UiCastPlugin)
            .add(UiMirrorPlugin)
            .add(crate::combat_text::CombatTextPlugin)
            .add(crate::nameplates::NameplatesPlugin)
            .add(crate::raid_marks::RaidMarksPlugin)
            .add(crate::vplates::VPlatesPlugin)
            .add(crate::chat_bubble::ChatBubblePlugin)
            // TOGGLEUI (`ALT-Z`): the whole quad layer goes dark, leaving the world and the cursor.
            .add(crate::ui_hide::UiHidePlugin)
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
            start: crate::char_select::ClientState::Login,
        });
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
            let mut systems = HashMap::new();
            let mut holds_vm = HashSet::new();
            for (key, system, conds) in graph.systems.iter() {
                if let (Some(vm), Some(with_access)) = (vm, graph.systems.get(key)) {
                    let access = with_access.access.combined_access();
                    if access.has_resource_read(vm) || access.has_resource_write(vm) {
                        holds_vm.insert(key);
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
            let mut dependencies = Vec::new();
            for (a, b) in graph.dependency().graph().all_edges() {
                for x in members(a) {
                    for y in members(b) {
                        if x != y {
                            dependencies.push((x, y));
                        }
                    }
                }
            }
            // Pass 2: build, and read what the build found. `is_send` is only known once a
            // system's params have registered their access, i.e. after this.
            schedule.initialize(world).expect("the schedule builds");
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
            }
        })
    }

    /// Does this build carry type names? Bevy has them only under its `debug` feature, which
    /// rides with `benilla-world`'s `dev`. Probed off the census, not a `cfg`, which belongs in
    /// `run_mode` alone; name-dependent tests skip without them.
    fn type_names_available(c: &Census) -> bool {
        c.systems.values().any(|s| s.name.contains("benilla_app::"))
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
    /// which no class can express. Known pairs accepted with a reason:
    /// - the ranged prop's two `AnimationPlayer` writers against the booth's and the quest
    ///   markers';
    /// - the fx attaches and `entities::update_display_models`/`attach_entity_visuals` against
    ///   the mat-anim tick over `MatAnimTable`/`UvAnimMaterials`: attach writes the rows a tick
    ///   would, off the same clock;
    /// - the area-spirit-healer poll over `Transform` and `NetEntity`: it is level-triggered, so a
    ///   unit seen a frame early or late is decided again next frame;
    /// - `ui_models::forget_dead_vm_tiles` against `portrait::glue_booth::sync_glue_scene` over
    ///   `MatAnimTable`, a slot allocator whose owners touch disjoint slots.
    ///
    /// Raising the ceiling is a claim that a new undeclared order is acceptable: make it with the
    /// reason read off the dump, or declare the order (`.after`, a set, a `chain`). A resource
    /// that commutes by construction belongs in [`Classes`].
    const UPDATE_ACTIONABLE_CEILING: usize = 5_462;
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
                    "{}: no declared side of the tick — `.in_set(UiFeed)` for a push the tick must see this frame, `.after(UiInput)` for a drain of what the tick produced",
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
             (`UiFeed` before it, or `.after(UiInput)`):\n  {}",
            offenders.join("\n  ")
        );
        assert!(stale.is_empty(), "stale rows:\n  {}", stale.join("\n  "));
    }

    /// The consumer markers: a VM holder that does one of these to host state has spent
    /// something this frame that nothing can re-spend.
    const CONSUMES: &[&str] = &["fire_event(", "mem::take(", ".drain("];

    /// Why an ungated one-shot consumer holding the VM loses nothing in the one-frame window
    /// where the wire is in world and the boot VM is still live and frameless.
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
         "`seen_generation` is a `VmMemo`: a new VM reads `None`, rebuilds and re-fires UPDATE_BINDINGS"),
        ("capture/probe_bg.rs", "bg_probe", Because::SelfHealing,
         "the battleground probe: dev-only (`WOW_PROBE_BG`, `cfg(feature = \"dev\")`) so it is not in a player build at all, and its `mem::take` is of its OWN pending-events string, not a queue anything else fills. Its one real VM dependency is the Lua event tap, which self-heals: `EVENT_DRAIN` returns a `<tap-gone>` sentinel when the tap's globals are missing — the case this window causes, since a tap installed in the boot VM is discarded when `mint_entry_vm` builds the interface — and the probe re-installs on the next frame"),
        ("death/mod.rs", "feed_death", Because::MemoLatched,
         "`feed.vm: VmMemo<DeathAnnounced>`: a fresh memo makes the first snapshot an edge and re-announces a held offer, confirm and corpse range"),
        ("screenshot.rs", "ask_for_captures", Because::FilledByVm,
         "`pending` holds only the VM's own `Screenshot()` asks, and the take spawns a capture, publishing nothing"),
        ("screenshot.rs", "report_captures", Because::FilledByVm,
         "the outcome chain starts with the VM's own `Screenshot()` call"),
        ("tutorial.rs", "drain_tutorials", Because::FilledByVm,
         "`sends` exist only after Lua acknowledged, cleared or reset a flag, and go to the wire"),
        ("ui_action/drain.rs", "drain_go_openers", Because::FilledByVm,
         "filled only by a world right-click on a GameObject; the drain sends casts to the wire"),
        ("ui_auction/mod.rs", "drain_auction", Because::FilledByVm,
         "VM verbs; the refresh flags act only under an open window and become wire re-asks"),
        ("ui_bank/mod.rs", "feed_bank", Because::PlayerRoundTrip,
         "`BankErrors` answers a `BuyBankSlot` click; the OPENED edge rides `VmMemo`s"),
        ("ui_battlefield.rs", "feed_battlefield", Because::SelfHealing,
         "`reset_on_world_enter` runs before it, clears the session and re-sends `BattlefieldStatusRequest`"),
        ("ui_binder.rs", "feed_binder", Because::PlayerRoundTrip,
         "`SMSG_BINDER_CONFIRM` only answers the innkeeper's gossip line"),
        ("ui_char.rs", "feed_char", Because::MemoLatched,
         "`feed.vm: VmMemo<CharFeedMemo>`; `vm_reset` fires every transition on a fresh memo"),
        ("ui_chat/recruitment.rs", "guild_recruitment_cascade", Because::SelfHealing,
         "`pending` is held until the zone mask and zone id are settled, which happens on the entry VM"),
        ("ui_dialog_verbs.rs", "feed_meeting_stone", Because::SelfHealing,
         "the query is sent once per VM (`asked: VmMemo<bool>`), so the entry VM re-asks and the reply lands after the UI is up"),
        ("ui_duel.rs", "feed_duel", Because::PlayerRoundTrip,
         "a duel exists only after someone's Duel cast, and the server ends any duel at logout; FINISHED/bounds ride `VmMemo<FedDuel>` (another player's act: coincidence-only residual)"),
        ("ui_follow.rs", "feed_follow", Because::MemoLatched,
         "`feed.vm: VmMemo<FollowFeedMemo>`; a fresh VM re-fires BEGIN for a follow in progress"),
        ("ui_gm_ticket.rs", "feed_gm_ticket", Because::MemoLatched,
         "`feed.vm: VmMemo<FedTicket>` counters restart per VM while `state.answers` keeps counting, so the latest answer is re-fired"),
        ("ui_honor.rs", "feed_honor", Because::MemoLatched,
         "`state.vm: VmMemo<HonorFeedMemo>`; `events_for(None, ..)` re-fires both events on a fresh VM"),
        ("ui_inspect.rs", "feed_inspect", Because::MemoLatched,
         "the token is latched only by Lua's `NotifyInspect`, and the diffs ride a `VmMemo`"),
        ("ui_item_text.rs", "drain_item_text", Because::FilledByVm,
         "page turns and close are Lua intents"),
        ("ui_item_text.rs", "feed_item_text", Because::MemoLatched,
         "`told` is a `VmMemo` inside the session (a fresh VM re-begins), and the open is a player click"),
        ("ui_items/drain.rs", "drain_container_destroys", Because::FilledByVm,
         "`take_container_destroys` is a VM-owned queue filled by Lua's `DeleteCursorItem`"),
        ("ui_items/drain.rs", "drain_container_uses", Because::FilledByVm,
         "`take_container_uses` and `take_container_repairs` are Lua intents held by the VM"),
        ("ui_logout.rs", "feed_logout", Because::PlayerRoundTrip,
         "both packets answer the `CMSG_LOGOUT_REQUEST`/cancel the game menu sent"),
        ("ui_loot/mod.rs", "drain_loot", Because::Deliberate,
         "the pre-VM take of `LootMoveStart` is documented at the line and publishes nothing to the VM; the event-firing takes are Lua's queues"),
        ("ui_loot_roll.rs", "drain_loot_rolls", Because::FilledByVm,
         "only a Need/Greed/Pass click queues a confirm or vote, held in the VM"),
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
        ("ui_petition/feed.rs", "feed_petition", Because::PlayerRoundTrip,
         "every line answers a charter action of ours; the window edges ride the `fed` memo"),
        ("ui_quest_share.rs", "feed_quest_share", Because::PlayerRoundTrip,
         "a verdict answers our own push and a confirm follows a party member's escort accept, both held until the name resolves (the confirm is another player's act: coincidence-only residual)"),
        ("ui_script/extract/mod.rs", "paint_script", Because::Deliberate,
         "per-frame paint and cost state, not a queue"),
        ("ui_social/feed.rs", "feed_social", Because::MemoLatched,
         "the login-burst lists are re-announced to a new VM by `fed.seeded` (`VmMemo<FedSocial>`); the show flag is Lua's; a status line waits on the name resolve"),
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
        ("ui_tradeskill.rs", "drain_trade_skill", Because::FilledByVm,
         "every consumed queue is Lua's; the cast-event continuation is inert until a Lua `DoTradeSkill` latched a count"),
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

    /// A consumer (a system taking `NonSend[Mut]<UiScript>` whose body has a [`CONSUMES`]
    /// marker) passes if the schedule shows `ingame_ui_up` on it or a set above it, if it is a
    /// `fire_event` driven by a `VmMemo` parameter, or if [`EXEMPT`] argues it. Read off the
    /// schedule, where a gate on a set, a tuple or the member all look alike. A stale `EXEMPT`
    /// row fails too.
    #[test]
    fn every_one_shot_consumer_that_holds_the_vm_is_gated_or_argued() {
        use crate::test_support::{fn_items, rel_path, rust_files, src_dir};
        let mut consumers = Vec::new();
        for file in rust_files(&src_dir()) {
            let text = std::fs::read_to_string(&file).expect("readable source");
            if !text.contains("UiScript") {
                continue;
            }
            let rel = rel_path(&file);
            for f in fn_items(&text) {
                let holds_vm = f.params.contains("NonSendMut<UiScript>")
                    || f.params.contains("NonSend<UiScript>");
                if !holds_vm || !CONSUMES.iter().any(|m| f.body.contains(m)) {
                    continue;
                }
                let takes = f.body.contains("mem::take(") || f.body.contains(".drain(");
                let memo = f.params.contains("VmMemo<");
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
        let by_name: HashMap<&str, &SystemInfo> = update
            .systems
            .values()
            .chain(post.systems.values())
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
            let is_gated =
                info.is_some_and(|i| i.conditions.iter().any(|c| c.ends_with("::ingame_ui_up")));
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
                    None => " (not found in Update/PostUpdate)".to_string(),
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
