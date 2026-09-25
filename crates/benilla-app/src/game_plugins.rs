//! **[`GamePlugins`] — the game, as one name**.
//!
//! `benilla_world::world_plugins::WorldPlugins` is the engine as one name (1164); this is the
//! client on top of it: every plugin the game adds, in the order `run()` always added them.
//! Until 2279 that chain lived inline in `run()` — 616 lines, the crate's only public function,
//! ~150 `add_plugins` — so nothing could build the game's schedule except the binary, and no
//! test could ask the one question Bevy can answer about a schedule: which conflicting pairs
//! have no declared order (the instrument that would have caught 2220 and B354 at the line).
//! `schedule_tests` below builds it headless and asks.
//!
//! **The order is load-bearing and is preserved from `run()` as-is.** Two kinds of edge live in
//! it: documented dependencies (a plugin that reads a resource an earlier one inserts at build
//! time — `VideoPlugin` and `RealmlistPlugin` before `CvarPlugin`, `WorldBackdropPlugin` after
//! `PlayerUiPlugin`, the `UiActionPlugin` family), and registration order, which is the
//! executor's tie-break between systems with no declared order. The second kind is exactly
//! what the ambiguity count measures; until it is zero, reordering members is a behaviour
//! change.
//!
//! **What is NOT here:** the engine (`WorldPlugins`), the three process plugins (`ThreadQos`,
//! `BgWin`, `MacQuit`), the instruments that sit above the stack (`dev::DevToolsPlugin`,
//! `perf::FpsJournalPlugin`, `dev::DevProbesPlugin`) — all still `run()`'s, in their places
//! around this group. `pipe_warm` is inside, at its old position: a client instrument, but one
//! that warms the *game's* pipeline set and has always been added among them.

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
    /// [`NetPlugin::connect`]: `false` in capture mode — the channel resources exist, no IO
    /// thread runs, so captures are deterministic regardless of whether a server is up.
    pub(crate) connect: bool,
    /// [`crate::char_select::CharSelectPlugin::start`]: the screen this session opens on.
    pub(crate) start: crate::char_select::ClientState,
}

impl PluginGroup for GamePlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            // The game's own WGSL, compiled into the binary — before anything that could
            // ask for one. The engine's seven register themselves inside `WorldPlugins`, which
            // `run()` adds ahead of this group.
            .add(crate::shaders::plugin)
            .add(BowstringPlugin)
            .add(crate::weapon_trail::WeaponTrailPlugin)
            .add(FishingLinePlugin)
            .add(QuestMarkersPlugin)
            // Pipeline-compile counters + the live-compile tripwire (macOS builds every
            // pipeline synchronously on the render thread, so a live compile is a felt stall).
            .add(crate::pipe_warm::plugin)
            // Streamed world entities: cube assets + display catalogs at startup, sync each frame.
            .add(EntitiesPlugin)
            // Creature animation: pick Stand/Walk/Run from each creature's movement state each frame (Milestone C).
            // The combat log — every combat packet's chat line and floating number, one handler each;
            // ahead of the animation layer, whose two shared kinds run second.
            .add(crate::combat_log::CombatLogPlugin)
            .add(CreatureAnimPlugin)
            // The unit blob shadow: the dark ground oval under every unit, sized from the playing
            // animation's box (`0x6d7920`), on the same
            // surface-decal projector as the selection ring.
            .add(BlobShadowPlugin)
            // Footprint decals: the prints a walking unit leaves on snow/sand,
            // spawn-once projections on the same decal projector, fading off the effect stream.
            .add(CameraShakePlugin)
            .add(FootprintsPlugin)
            // GameObject animation: net-streamed GObjects (doors/chests) play an M2 sequence
            // on GAMEOBJECT_STATE change — the state-machine sibling of the doodad idle loop above.
            .add(crate::go_anim::plugin)
            .add(crate::doodad_events::plugin)
            // Avatar + camera + input.
            .add(PlayerPlugin)
            // Cinematic fly-bys (`SMSG_TRIGGER_CINEMATIC`): the race intro a first login plays, and the
            // GameObject cameras. Takes the world camera for the duration — hence after PlayerPlugin,
            // whose `control` it overrides within the same stage (decision 0196's deferred arc).
            .add(crate::screen_fade::ScreenFadePlugin)
            .add(CinematicPlugin)
            // The real client's hardware mouse cursor (native NSCursor on macOS).
            .add(CursorPlugin)
            // Net↔ECS bridge: spawns the world thread, exposes the snapshot + writer resources. In capture
            // mode the IO thread is skipped (`connect: false`) so the scene is deterministic.
            .add(NetPlugin {
                connect: self.connect,
            })
            // The death arc: the wire-fed death stores + the root/water-walk ack messages.
            .add(crate::death::DeathPlugin)
            // The shared glue vocabulary both pre-world screens stand on: the ADD-mode UI
            // material, the client-data art set, the GlueStrings table.
            .add(crate::glue::GluePlugin)
            // The glue layer: the ClientState machine + the character-select screen
            // that answers the parked IO thread's pick. A world capture boots straight InWorld (no net,
            // no picker); a glue capture boots onto the screen it photographs.
            .add(crate::char_select::CharSelectPlugin { start: self.start })
            // The login screen: the faithful AccountLogin glue + the credential policy
            // that answers the IO thread's pre-logon park.
            .add(crate::login::LoginPlugin)
            // The realm list: the faithful RealmList glue + the policy that answers the IO thread's
            // realm park. The client used to take `realms.first()` and offer no way to say otherwise.
            .add(crate::realm_select::RealmSelectPlugin)
            // The character-creation screen + its live preview booth.
            .add(crate::char_create::CharCreatePlugin)
            // Audio: the delegated mixer + WoW's owned selection layer.
            .add(SoundPlugin)
            // Targeting: left-click a unit to select it (→ CMSG_SET_SELECTION) + draw its ground ring.
            .add(TargetPlugin)
            .add(TransportPlugin)
            // Faithful world-load splash + progress bar on startup + cross-map teleport (the load latency
            // streaming can't hide); per-map art via the Map.dbc→LoadingScreens.dbc→BLP chain.
            .add(LoadingScreenPlugin)
            // The player-UI quad pass: its own composited-above-the-world,
            // below-the-egui-dev-overlays camera + sorted-quad renderer. `$WOW_UI_DEMO=1` seeds a proof scene.
            .add(PlayerUiPlugin)
            // The world's frame, rendered off-screen and drawn first in the UI camera's main pass —
            // the seam that puts the UI-over-world blend back into gamma bytes (0161/0254's last piece,
            // a pass rather than a quad since 2234). Registered AFTER the UI pass: it points that
            // plugin's camera at the world camera.
            .add(WorldBackdropPlugin)
            // The HUD minimap (decision 0203 phase 1): fills the `<Minimap>` widget's extracted hole with
            // the streamed tile window + mask + player arrow, and feeds the zone text.
            .add(crate::minimap::MinimapPlugin)
            // The pet-bar / spellbook autocast shine, drawn on the append lane from the conversion's
            // parked sites — zero per-frame script-layout traffic.
            // The `<Model>` widgets' M2s, rendered as tiles of one atlas and composited at the
            // callback rank.
            .add(crate::ui_models::UiModelsPlugin)
            // The shared AreaTable catalog + the ZONE_CHANGED event family / zone-text host globals
            // behind GetZoneText & co. (the zone-entry splash arc).
            .add(crate::area::AreaPlugin)
            .add(crate::area_poi::AreaPoiPlugin)
            .add(crate::world_state_ui::WorldStateUiPlugin)
            // The `AreaTrigger.dbc` volumes + the per-frame containment check that reports walking into
            // one (`CMSG_AREATRIGGER`) — the client's whole part in portals, instance entrances and
            // explore objectives; the server owns what each trigger means.
            .add(crate::area_trigger::AreaTriggerPlugin)
            .add(crate::ui_world_map::WorldMapUiPlugin)
            // The guard's directions marker (`SMSG_GOSSIP_POI`) — one landmark record, drawn by the
            // minimap's landmark pass and the world map's POI child, cleared by arriving at it.
            .add(crate::poi_marker::PoiMarkerPlugin)
            // The glyph atlas (client TTFs -> baked bitmap) `ui_script`'s extraction draws `FontString`
            // regions through. Loads at Startup, after the asset chain opens.
            .add(UiTextPlugin)
            // The one "which NPC am I interacting with" answer, shared by the portrait booth's `"npc"`
            // token and the interaction face-me — hence its own plugin, ahead of both.
            .add(crate::ui_session::UiSessionPlugin)
            // Unit-frame portraits: the token -> off-screen-baked-face bridge the UI extract samples for a
            // `SetPortraitTexture`-bound region (the modern high-res 2D model bake).
            .add(PortraitPlugin)
            .add(TextInputPlugin)
            .add(UiScriptPlugin)
            // The video knobs the CVar host writes into (today: `gxVSync`). Before CvarPlugin so the
            // resource exists when `load_config` applies the saved value at Startup.
            .add(crate::video::VideoPlugin)
            // The realmlist — the logon address the login screen edits. Same reason as
            // VideoPlugin above: it is a CVar knob, so its resource has to exist before `load_config`.
            .add(crate::realmlist::RealmlistPlugin)
            // The CVar host: registration, knob sync, config.toml persistence. After
            // UiScriptPlugin only for reading order — its systems gate on the VM existing anyway.
            .add(crate::cvars::CvarPlugin)
            // The console command registry: the reference's `ConsoleCommand` table,
            // its four CVar commands and `help`; subsystems register their own from their plugins.
            .add(crate::console::ConsolePlugin)
            // The key-binding engine: the chord→command dispatch every rebindable input
            // runs through, its persistence, and the Key Bindings window's capture seam.
            .add(crate::bindings::BindingsPlugin)
            // The unit snapshot + event feed: pushes ECS game state into the VM as the
            // plain data the `Unit*` bindings read, and fires the matching WoW events.
            .add(UiUnitPlugin)
            .add(UiPartyPlugin)
            // Duels: the wire session, the client-side countdown tick, the four Era
            // events, and the accept/cancel/challenge intents.
            .add(UiDuelPlugin)
            // Setting your hearthstone: the innkeeper's SMSG_BINDER_CONFIRM question, the
            // CONFIRM_BINDER dialog it raises, and the CMSG_BINDER_ACTIVATE its Accept sends — the only
            // packet in the flow that actually binds anything.
            .add(UiBinderPlugin)
            // The dialog engine's own verbs: the pet trainer's question, the instance
            // boot clock, the area spirit healer, the battleground queue, the meeting stone.
            .add(UiDialogVerbsPlugin)
            .add(BattlefieldScorePlugin)
            .add(BattlefieldPlugin)
            .add(BattlefieldPositionsPlugin)
            .add(crate::game_tip::GameTipPlugin)
            .add(crate::text_filter::TextFilterPlugin)
            // The re-shape a `bevy_ui` text root loses when its last span is despawned:
            // an upstream change-detection hole whose only symptom is a
            // panic inside `bevy_text` on the next window resize.
            .add(crate::text_reshape::TextReshapePlugin)
            .add(TutorialPlugin)
            // The melee swing refusals (`SMSG_ATTACKSWING_NOTINRANGE`/`_BADFACING`/`_DEADTARGET`/
            // `_CANT_ATTACK`): the latch the packets set, and the 4 s repeat that shows it while an
            // attack target stands and no swing lands.
            .add(crate::swing_refusal::SwingRefusalPlugin)
            // Being summoned: SMSG_SUMMON_REQUEST's latch, the CONFIRM_SUMMON dialog it
            // raises, and the CMSG_SUMMON_RESPONSE its Accept sends. The binder's twin one line up — a
            // server-asked question whose only wire answer is yes — and here for that reason.
            .add(UiSummonPlugin)
            // The GM trouble-ticket flow: the Help window's five sends, the UPDATE_TICKET
            // answer ticket behind its 10-minute poll, and the GMTicketCategory.dbc list its "page a GM"
            // rows are built from. Beside the binder because it is the same feed/drain shape, and after it
            // because both want UiInput ordering and this reads better grouped.
            .add(UiGmTicketPlugin)
            // Auto-follow's UI seam: the popup's Follow row + `FollowUnit`/`FollowByName` inbound, and
            // the AUTOFOLLOW_BEGIN/END pair that drives the centre-screen status line outbound.
            .add(UiFollowPlugin)
            // Instance/raid lockouts: the four CHAT_MSG_SYSTEM lines the client composes
            // itself out of GlobalStrings, the last-dungeon/ownership bookkeeping behind
            // `CanShowResetInstances()`, and the SELF menu's one send. Beside the binder family for the
            // same feed/drain shape; it needs the map catalog, which is up long before Update runs.
            .add(UiInstancePlugin)
            // Leaving: the game menu's Logout/Exit Game — the request, the server's
            // 20-second answer narrated as the CAMP/QUIT countdown, and the process exit.
            .add(UiLogoutPlugin)
            .add(UiSocialPlugin)
            // Guilds: the identity/roster mirror behind the four guild windows, the
            // membership verbs, and the `ERR_GUILD_*` lines. Right after the social session, whose
            // FriendsFrame it shares a window with and whose ignore list its sign-on lines consult.
            .add(UiGuildPlugin)
            // Founding a guild: the guild registrar and the charter window — the slice
            // 1257 §2 left out. Right after the guild session, whose error channel its refusals ride and
            // whose roster its success produces.
            .add(UiPetitionPlugin)
            .add(UiTooltipPlugin)
            // The character-window feed: the combat-stats/inventory snapshots + events
            // the paper doll reads, and the paper-doll booth's yaw mirror.
            .add(UiCharPlugin)
            // The reputation-pane feed: the player's wire faction slots resolved against Faction.dbc into
            // the pane's snapshot, plus the pane's three outbound verbs. Beside the character feed because
            // it is the same window's other tab.
            .add(crate::ui_reputation::UiReputationPlugin)
            // The inspect feed: another player's equipment off their PUBLIC visible-item
            // entries, plus the "inspect" booth's unit + yaw. Right after the character feed it mirrors.
            .add(crate::ui_inspect::InspectUiPlugin)
            // The honor feed: the PRIVATE honor descriptor block as the snapshot both
            // Honor tabs read, plus the inspect-honor round trip. After the inspect feed because it
            // resolves that feed's target to address its request at.
            .add(crate::ui_honor::UiHonorPlugin)
            // The dressing-room feed: the window's try-on intents → the player's own look
            // with the tried-on items substituted in, plus the "dressup" booth's yaw. Beside the inspect
            // feed, whose shape it shares (intents in, a booth look out).
            .add(crate::ui_dressup::DressUpUiPlugin)
            .add(UiActionPlugin)
            // The spell (2265 §A7's owner): the cast lifecycle's state — the pending cast, the queued
            // strike, the running channel, the auto-repeat key, the chain outbox — its local cancel,
            // the cooldown store, the talent modifier tables and the packet handlers (
            // 2328). After UiActionPlugin, whose feeds and cast ladder read the state through it.
            .add(crate::spell::SpellPlugin)
            // The aura feed: the player's insertion-ordered buff/debuff cache + the
            // self-only durations, pushed as the data the `UnitAura` bindings read; fires UNIT_AURA and
            // drains the right-click cancels. After UiActionPlugin (shares its `Spells` catalog).
            .add(UiAuraPlugin)
            // The spellbook window feed (slice 5): builds the book from
            // PlayerActions.spells through the Spell.dbc/SkillLine.dbc join and drives
            // SpellBookFrame.xml's snapshot + cast-drain seam — the spell SOURCE for the cursor payload
            // arc (bags/doll/bars/book). After UiActionPlugin (shares its `Spells` resource + the cast
            // tail `send_spell_cast`).
            .add(UiSpellbookPlugin)
            // The macro system: the icon chooser's catalog, the `benilla-config/macros/`
            // files, `UPDATE_MACROS`, and the macro→bound-spell table the action bar's MACRO slots
            // resolve their cooldown/usability through. After UiSpellbookPlugin — the bound spell is
            // resolved against the book that feed pushes, by the same law `CastSpellByName` uses.
            .add(crate::ui_macro::UiMacroPlugin)
            // The talent window feed: builds the class pages from Talent.dbc × the
            // known-spell set + PLAYER_CHARACTER_POINTS, drives TalentFrame.xml through the engine's
            // talent seam, and drains learn clicks into CMSG_LEARN_TALENT. After UiActionPlugin
            // (shares its `Spells` catalog), beside the spellbook it mirrors.
            .add(UiTalentPlugin)
            // Unlearning them again: the class trainer's respec question, its
            // CONFIRM_TALENT_WIPE dialog, and the answer that is the only packet in the flow which
            // unlearns anything. Beside UiTalentPlugin for the subject, but it is UiBinderPlugin's twin
            // in shape — a guid-carrying question over an already-closed gossip menu.
            .add(UiTalentWipePlugin)
            // The stance/shapeshift bar feed: builds the form list from PlayerActions.spells per
            // the reference's admission and order (`0x4b25b0`, `0x4b2bb0`), drives the stock
            // shapeshift bar through the engine's shapeshift seam, and drains its clicks
            // (cancel-if-active else cast). After UiActionPlugin (shares `Spells`) and SpellPlugin
            // (the `usable` walk, the cast tail).
            .add(UiShapeshiftPlugin)
            // The pet action bar — the stance bar's mirror image: server-authoritative,
            // so this renders the ten packed words the last `SMSG_PET_SPELLS` delivered and sends
            // intents back. After UiActionPlugin (shares `Spells` and the cooldown triple's clock).
            .add(UiPetPlugin)
            .add(ChrClassesPlugin)
            .add(UiPetBookPlugin)
            // The pet's paper-doll stat block (happiness/loyalty/XP/training points). Its own plugin
            // because it runs off descriptor fields and two DBC tables rather than off `SMSG_PET_SPELLS`.
            .add(UiPetStatsPlugin)
            // The pet paper doll's SHARED surface — the combat-stats snapshot under the
            // `"pet"` token and the page's model booth. Apart from the block above because these values
            // pass through the character sheet's own bindings and events, with no hunter gate.
            .add(UiPetDollPlugin)
            // The connection-telemetry feed: the averaged ping RTT behind `GetNetStats()`, which the main
            // bar's performance meter polls.
            .add(UiNetPlugin)
            .add(UiCastPlugin)
            // The breath / fatigue bars: server-authoritative mirror timers off the
            // wire into the transcribed MirrorTimer1/2/3 frames. Beside the cast bar it shares its
            // feed→drain shape (and its art: the same UI-CastingBar-Border chrome).
            .add(UiMirrorPlugin)
            // Floating combat text (decision 0137 phase 2): the WORLDTEXTSTRING law — world-anchored
            // damage numbers/outcome words projected into the UI quad pass each frame.
            .add(crate::combat_text::CombatTextPlugin)
            // Overhead unit names (nameplates): world-billboard name text over players + NPCs.
            .add(crate::nameplates::NameplatesPlugin)
            // Raid-target marker billboards: the mark icon over marked units, one line-pitch
            // above the overhead name; plated units show the plate's raid child instead.
            .add(crate::raid_marks::RaidMarksPlugin)
            // V-key nameplates: the toggled health-bar plates, a 2-D overlay replacing the
            // overhead name on plated units.
            .add(crate::vplates::VPlatesPlugin)
            // Chat speech bubbles: the over-the-head bubble a say/yell/party line spawns, the
            // plates' 2-D overlay sibling — mutually exclusive with both the plate and the name.
            .add(crate::chat_bubble::ChatBubblePlugin)
            // TOGGLEUI (`CTRL-Z`/`Cmd-Z`): the whole quad layer goes dark — frames, minimap, plates,
            // bubbles, combat text — leaving the world and the cursor.
            .add(crate::ui_hide::UiHidePlugin)
            .add(UiItemsPlugin)
            // The gossip window: fills from the net drain's GossipState and drives
            // GossipFrame.xml over the Era gossip API.
            .add(UiGossipPlugin)
            // The merchant window (decision 0081 phase 4): fills from the net drain's MerchantOpen and
            // drives MerchantFrame.xml over the Era vendor API + the money display.
            .add(UiMerchantPlugin)
            // The bank window: the SHOW_BANK session (BankOpen) + the purchase row;
            // the vault's slots ride the container feed as bags −1/5..=10.
            // The auction house — an NPC-session window like the bank beside it, but the
            // only `doublewide` panel in the UI, so it displaces both the left and center seats.
            .add(UiAuctionPlugin)
            .add(UiBankPlugin)
            // The mail window (decision 0544 P1/P2): the client-side mailbox session (MailOpen), the
            // NPC-session range guard, and MailFrame.xml over the Era mail API (inbox, open-letter,
            // send tab).
            .add(UiMailPlugin)
            // Player-to-player trade (TradeFrame.xml): the two-sided trade window, driven server-side over
            // the P0 wire; the partner's portrait rides the shared "npc" booth (decision 0592 P1).
            .add(UiTradePlugin)
            // The item-text reader (ItemTextFrame.xml): right-clicked bag letters (mail-made permanent
            // copies) read in the reference reader window over the shared ask-once item-text cache.
            .add(UiItemTextPlugin)
            .add(UiSavedPlugin)
            .add(NamePersistPlugin)
            .add(UiStablePlugin)
            .add(TabardUiPlugin)
            .add(UiTrainerPlugin)
            // The taxi map (decision 0484 phases 1-2): the SMSG_SHOWTAXINODES-fed TaxiState resource, the
            // NPC-session range guard, and the TaxiFrame.xml window feed/drain (catalogs, node
            // projection/route computation, the activate send, the UnitOnTaxi ride flag).
            .add(UiTaxiPlugin)
            .add(UiTradeSkillPlugin)
            .add(UiCraftPlugin)
            // The loot window: fills from the net drain's LootState and drives
            // LootFrame.xml over the Era loot API (coin + rows, paging).
            .add(UiLootPlugin)
            .add(UiLootRollPlugin)
            // The questgiver window: fills from the net drain's QuestGiver and drives
            // the stock questgiver window's four sub-panels over the Era quest API (1944).
            .add(UiQuestPlugin)
            // The quest-log window (decision 0088's deferred second slice): fills from the self player's
            // PLAYER_QUEST_LOG descriptor slots + the SMSG_QUEST_QUERY_RESPONSE template cache, and drives
            // the stock quest log over the Era quest-log API (1944).
            .add(UiQuestLogPlugin)
            // The party quest-share: the verdict lines on a quest we pushed, and the
            // escort-quest confirm. Neither is bound to a window, so it is its own plugin rather than a
            // lodger in either quest plugin above.
            .add(QuestSharePlugin)
            .add(UiChatPlugin)
            // The layout cache: the geometry of every window the player has dragged or resized, restored
            // at world entry and written back a quiet second after the last drag
            // (`benilla-config/layout/<realm>-<character>.txt`). The consumer of the engine's userPlaced
            // bit, which nothing read before it.
            .add(UiLayoutPlugin)
            // Print screen: the SCREENSHOT binding's engine half — one PNG per
            // `Screenshot()` call into `benilla-config/Screenshots/` (never the install),
            // answered to the UI as SCREENSHOT_SUCCEEDED/FAILED so the status text can never be in the
            // frame it announces.
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

    /// **The whole client, built headless.** The tuned `DefaultPlugins` with no window, no
    /// winit, no logger and no GPU (`backends: None` — bevy then creates no render app), then
    /// the engine, then the game. Every plugin's `build` and `finish` runs; no schedule does.
    /// What this yields is the schedule GRAPH — which is fixed once the plugins have built
    /// (the census probe's own argument, `capture::probes::sched_census`) — so a question
    /// about ordering can be asked here, in a test, instead of on a login.
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

    /// One schedule, read two ways: the graph before it builds (names, sets, conditions — all
    /// of which move into the executable when it does), and the conflict list after (which
    /// only exists once it has).
    pub(crate) struct Census {
        pub systems: HashMap<SystemKey, SystemInfo>,
        /// Pairs of systems with conflicting access and no path between them, each with what
        /// they fight over.
        pub conflicts: Vec<(SystemKey, SystemKey, Vec<ComponentId>)>,
        /// Everything any pair fights over, by name (a placeholder in a build without type
        /// names — [`type_names_available`]).
        pub components: HashMap<ComponentId, String>,
        /// The explained classes, resolved against this world.
        pub classes: Classes,
        /// Every declared `a` runs before `b`, at the system level.
        pub dependencies: Vec<(SystemKey, SystemKey)>,
        /// The systems that must run on the main thread (the VM's, the audio layer's).
        pub non_send: Vec<SystemKey>,
        /// The systems whose declared access includes the Lua VM — read off each system's
        /// own access set, not off the conflicts it happens to have (a holder whose every VM
        /// pair is declared would not show there).
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

    /// Which graph a census reads.
    ///
    /// Bevy's build inserts an `ApplyDeferred` barrier between a system with commands and its
    /// dependents, and it **shares one barrier per "distance"** (the number of barriers between
    /// a node and the schedule's start — `auto_insert_apply_deferred.rs`, `get_sync_point`).
    /// Two systems that share a barrier are ordered *through* it, so a pair the graph never
    /// declared reads as ordered — and one edge added anywhere upstream re-homes whole groups
    /// onto a different barrier and moves pairs among systems the edit never touched (2333's
    /// five-out-six-in). Measured on this tree the day it was named: 2,888 actionable pairs
    /// with the barriers, 5,063 without — the barriers were hiding 43% of the undeclared
    /// orders behind their accidental placement.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum SyncPoints {
        /// The schedule as it actually runs, barriers included — what an executor census
        /// wants, since a barrier really is a wave boundary.
        Built,
        /// The **declared** graph only: no auto-inserted barriers, so "no path between two
        /// systems" means exactly "nobody declared an order", and the count is a function of
        /// the edges alone. What a ratchet on undeclared orders must read.
        Declared,
    }

    /// Take the census of one schedule. Initializing it runs every system's param setup, and
    /// at least one of those inserts `Schedules` itself, so this goes through bevy's own
    /// take-out-initialize-put-back rather than a `resource_scope` on that resource.
    pub(crate) fn census(app: &mut App, label: impl ScheduleLabel, sync: SyncPoints) -> Census {
        let label = label.intern();
        app.world_mut().schedule_scope(label, |world, schedule| {
            schedule.set_build_settings(ScheduleBuildSettings {
                ambiguity_detection: LogLevel::Warn,
                auto_insert_apply_deferred: sync == SyncPoints::Built,
                ..default()
            });
            // The systems' access is filled by `initialize`, which the build below would run
            // anyway (bevy drains one list of uninitialized systems, once). Done first, so pass
            // 1 can read it while the graph still holds the systems.
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
            // Every declared order, at the system level: a `.before`/`.after`/`chain` edge
            // between sets is an edge between every member of one and every member of the
            // other.
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
                // The build inserts what nobody added: the `ApplyDeferred` sync points between a
                // system with commands and its dependents. Name them so a census can count them.
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

    /// **Does this build carry type names?** bevy only does under its `debug` feature, which
    /// rides with ours (`benilla-world`'s `dev`); a player build names every
    /// system and resource `<Enable the debug feature …>`. Probed off the census itself rather
    /// than off a feature flag — a `cfg(feature = "dev")` outside `run_mode` is a seam leak (1179),
    /// and what these tests need is the capability, not the plane. The name-dependent tests
    /// announce the skip and return; the name-free ratchets run either way.
    fn type_names_available(c: &Census) -> bool {
        c.systems.values().any(|s| s.name.contains("benilla_app::"))
    }

    /// **The explained classes** — what an undeclared order may be about
    /// without anyone declaring it, argued once here instead of by every session that adds a
    /// system. On 2287's eve three sessions raised the two constants this table replaced,
    /// each with a paragraph, each through a rebase conflict on the same lines, none declaring
    /// an order (2281, 2282, 2283) — because for most of what those counts held there was
    /// nothing to declare:
    ///
    /// - **A non-`Send` owner** — the Lua VM, the audio layer's handles. Their systems run on
    ///   the main thread one at a time under any executor, so no order among them is a race.
    ///   *Which feed fires its Lua events first* is a real question, answered by the `UiFeed`
    ///   phase: every push after the drain and before the tick, and among the
    ///   pushes an order declared where it matters (the chat cascade, the cooldown events),
    ///   registration order otherwise. Derived, not listed: every registration that is not
    ///   `Send + Sync`.
    /// - **A pure cache** — a read is a write because a miss records itself: the ask-once
    ///   caches mark the key pending and send one query (`NameCache`, the GameObject
    ///   templates, the page texts), the load-once caches build the entry (`WorldAssets`,
    ///   `Creatures`' display models). Two misses for one key commute — one query, or one
    ///   build, whichever runs first — and the answer lands in the net drain, in packet order,
    ///   inside `WorldStage::Net`. *Pure* is the condition: a resource that is a cache **and**
    ///   a window's state (`Items`, `MailOpen`, `GuildState`, `QuestLog`, `PetitionState`) is
    ///   not here, because its other writers do not commute; 2265 §A4 is what lets those
    ///   split.
    /// - **An append-only sink** — `ChatLog`, `MessageSounds`, `UiErrorKeys`, `UiErrorTexts`:
    ///   writers commute, and a drain's order against a writer is one frame of latency, never
    ///   a loss. (2283's ten pairs were all this shape: a new verb drain against its siblings
    ///   over the chat and error sinks.)
    /// - **Not an exclusive edge.** A pair bevy reports with NO component list has an exclusive
    ///   system on one side (`bevy_ecs` `node.rs:625`: it reports every unordered pair with an
    ///   exclusive system and never reads its access). 2304 explained that class as the
    ///   build's barriers; the ratchet reads the declared graph since 2337, which has none, so
    ///   what it held was only the hand-written exclusive systems — the net drain, whose 143
    ///   unordered partners read this frame's packets or the last frame's by coin, and
    ///   `finish_colliders`. Those are undeclared orders like any other: actionable (2343).
    /// - **A random stream** — `SoundKits`, which every sound system holds to play a kit: a
    ///   decode cache, a per-kit last-variation memory and one xorshift stream. Any
    ///   interleaving of draws is a valid draw, and that is the reference's own contract for
    ///   its single stream, consumed in whatever order its callers happen to run.
    ///
    /// A pair is **explained** when everything it fights over is in one of the four, and
    /// **actionable** otherwise; only the actionable count is ratcheted. Adding a type here is
    /// the act raising the ceiling used to be — a claim, with its reason, made in review — and
    /// it is a claim about *every* writer of that resource, which is why the pure caches are
    /// pure. Resolved by `TypeId`, so the ratchet runs in the player build, which carries no
    /// names (1451).
    /// One row of the class table: the type's name for the message, and its `TypeId`.
    type ClassRow = (&'static str, fn() -> TypeId);

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
            // The client's ONE `rand()` stream. Four lanes draw from it — the
            // placed-doodad host, the creature driver, the GameObject arm and the portrait booth —
            // and in the reference they draw from one TLS cell in whatever order the frame runs
            // them. The interleaving IS the mechanism: a shared sequence is what de-syncs a stand
            // of identical props, and no consumer can observe which draw it got, only that it got
            // a fresh one. So an undeclared order here is not a missing `.after`; declaring one
            // would be inventing a determinism the reference does not have.
            //
            // It reduces the count by **nothing** today, and that is not an oversight: the three
            // Update-side lanes already conflict on `Query<&mut AnimationPlayer>`, which no class
            // explains, so every pair this row would cover is counted for that instead. It is the
            // standing claim about the resource — what keeps these pairs from surfacing the day
            // that other conflict is declared — not a saving.
            ("AnimRng", TypeId::of::<benilla_assets::AnimRng>),
        ];

        /// Resolve the table against a world whose schedules have initialized (every param
        /// has registered its resource by then). A row that resolves to nothing is a stale
        /// row, and the test says which.
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

        /// Which class explains this pair — `None` if it is actionable.
        pub fn class_of(&self, what: &[ComponentId]) -> Option<&'static str> {
            // An exclusive system on one side: bevy cannot see its access, so nothing here can
            // argue the pair away (see the doc above: not an exclusive edge).
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

    /// `PostUpdate`, 181 systems: `GlobalTransform` and the particle `EffectQuads` are most of it.
    /// **371** — the declared-graph count, re-measured beside `Update`'s when
    /// the census stopped reading bevy's barriers as orders (see the ledger below).
    const POST_UPDATE_CEILING: usize = 371;
    const POST_UPDATE_SLACK: usize = 20;
    /// The **actionable** pairs in `Update` — two systems with conflicting access and no
    /// declared order, where [`Classes`] explains none of what they share, so the executor
    /// runs them in whatever order the graph around them happens to produce (2220). Same
    /// shape as `world_api_wall.rs`: the count may not rise past the ceiling, and the ceiling
    /// follows the count down. `WOW_AMBIGUITY_DUMP=1` prints every actionable pair with what
    /// it fights over.
    ///
    /// **Measured 2026-09-17, 662 systems:** 16,548 pairs in all, 12,849 of
    /// them explained — 10,703 on the VM alone, 431 on the other non-`Send` owners, 556 on
    /// the pure caches, 87 on the sinks, 41 on the stream, 1,031 on a mix of those — and
    /// 3,699 actionable. The largest part of those is `Transform` on disjoint lanes (716 pairs
    /// alone): populations that never intersect, which the filter algebra cannot see. A lane
    /// is not a class — the checker cannot tell a disjoint lane from two writers of the same
    /// entity — so they stay here, and each new one is a claim made at the registration with
    /// its reason:
    ///
    /// - **2281:** the ranged prop's two `AnimationPlayer` writers against the booth's and the
    ///   quest markers' (four lanes; the held weapon, a booth model, a marker).
    /// - **2282:** the three fx attaches against the mat-anim tick and its probe, over
    ///   `MatAnimTable`/`UvAnimMaterials` — immaterial by construction, because attach writes
    ///   the rows a tick would, off the same clock.
    /// - **2295:** the same two readers against `entities::update_display_models` and
    ///   `entities::attach::attach_entity_visuals`, over the same two resources, now that the
    ///   entity lane registers its own texture transform. **+4**, measured pair by pair on the
    ///   rebased tree rather than assumed additive — both writers already held
    ///   `Assets<WowModelMaterial>`, so most of the systems they newly meet were already
    ///   ambiguous against them for another reason, and `redress_player_looks` takes the
    ///   identical three resources and adds no pair at all. Immaterial for 2282's reason plus
    ///   one: a display is built ONCE, the frame its asset lands, and a row left unwritten for
    ///   a frame reads as the material's own built seed — the batch's authored `t = 0`, not a
    ///   wrong value — because the same call seeds `sun_scale.zw` at the loop's opening.
    ///
    /// **3,280** — the one query cache: a read that asks marks its miss through
    /// `&self`, so a feed that only resolves a name or a template holds the owner shared, and
    /// its pairs over `Items` (725 → 104, the largest non-class resource in the count) are gone;
    /// the pairs over the pure caches moved into the VM-only class, where the same systems still
    /// meet over the VM alone.
    ///
    /// **3,288 (corrected)** — the area-spirit-healer poll. Its **four** remaining
    /// pairs, named one by one because the first version of this paragraph named the wrong ones:
    /// `portrait::booth::face_booth_billboards`, `quest_markers::bake_seat_scale` and
    /// `entities::live_display::tick_scale_ease` over `Transform`, and
    /// `entities::live_display::refresh_live_display` over **`NetEntity`** — a row read, not a
    /// `Transform` pair at all. All four are the poll seeing a unit one mover early or late, which
    /// moves a 20 yd acquire decision by at most one frame of walking; the poll is
    /// **level**-triggered, re-deriving the whole cache every frame, so a boundary case decided
    /// early is decided again next frame. The target scanner carries the same class for the same
    /// reason.
    ///
    /// **The correction is the part worth keeping.** This paragraph first claimed "+6, all
    /// `&Transform`", and that the poll's "one write-write pair … is *declared*". Both were wrong,
    /// and the second one hid a real defect. There were **two** undeclared write-write pairs, and
    /// the system on the other side of both was `drain_latch_verbs` — the one that turns
    /// `AcceptAreaSpiritHeal()` into `CMSG_AREA_SPIRIT_HEALER_QUEUE`. It met the poll *and*
    /// `target::click::act_on_right_click` over `AreaSpiritHealer`, because `.after(UiInput)`
    /// orders it against neither (`UiInput` precedes `WorldStage::Input`; `TargetUpdate` follows
    /// it). The drain takes its presses unconditionally and only then reads the healer, so that
    /// undeclared order decided whether a ghost's Accept became a packet or vanished with no
    /// message. Declaring `drain_latch_verbs.before(TargetUpdate)` — the reference's own order, the
    /// dialog's Lua handler running in the UI dispatch ahead of `CGWorldFrame`'s poll and pick —
    /// removed exactly those two.
    ///
    /// **Re-measured at 3,289 on the rebased tree**, not carried across as arithmetic. This branch
    /// measured 3,290 → 3,288 on its own base; a neighbour landed 3,291 first, and the resolution
    /// is the number the dump prints on the merged tree — which happens to agree with 3,291 − 2
    /// this time, and is right for the reason that it was read rather than that it adds up.
    ///
    /// A ceiling raised with the wrong reason is a ratchet that has stopped meaning anything. The
    /// lesson this one cost: **read the dump, do not reason about what the new pairs must be.**
    ///
    /// **3,291** — the glue create/main-menu scene's material lane: exactly **one**
    /// new pair, `ui_models::forget_dead_vm_tiles` against `portrait::glue_booth::sync_glue_scene`
    /// over `MatAnimTable`, measured pair by pair on the tree that lands rather than assumed
    /// additive (the scene builder's other three new resources add none — it already held
    /// `RigPalettes` and `Assets<WowModelMaterial>`, so the systems they newly meet were already
    /// ambiguous against it). Immaterial by construction, and narrowly: `MatAnimTable` is a slot
    /// **allocator**, one owner per slot. A tile reaping its dead rows and the scene claiming
    /// fresh ones touch disjoint slots and neither reads the other's; the only thing the order
    /// decides is *which* free slot the scene is handed, and a slot number is not observable —
    /// the row behind it is, and it is written by whoever owns it. A scene is also built **once**,
    /// the frame its model asset lands, against a reaper that fires only when a tile dies.
    ///
    /// **2,886** — read off the dump on the tree that lands, after the
    /// `modalNextSpell` outbox's drain retired: `drain_chain_casts` held the whole cast ladder
    /// (`CastLadder`'s fourteen parameters and `CastTargeting`'s ten) with no declared order
    /// against anything but `UiInput`, and the cast reply's handler now casts the chain itself,
    /// as a one-shot outside the schedule. The three targeting systems the same record moved
    /// from `UiActionPlugin` to `SpellPlugin` kept their orders and their sets.
    ///
    /// **2,888** — the trainer feed became the state re-evaluator's host and
    /// reads the pet bar, the player's descriptor and its field edges. Read off the dump on the
    /// tree that lands: undeclared, the feed met six systems; `.after(UnitFeed)` (the shape the
    /// demo unit feed already takes) declares five of them, and the one it keeps —
    /// `entities::evict_display_caches` — is the pair every feed that reads `ItemDisplays` has
    /// (42 on this tree). So the feed's own count went 6 → 1, and the total went 2,887 → 2,888:
    /// diffing the two dumps, the edge **re-partitioned** the census — five pairs among unrelated
    /// systems (`cinematic::*` against `ui_logout::drain_logout`, `arm_loot_fx` against the
    /// party/social drains, …) left the actionable list and six others (`resolve_equipment`
    /// against `drain_guild`, `emit_minimap` against `drain_quest`, …) entered it, none of them
    /// touching anything this record touched. That is the instrument moving under an edge, not
    /// a new undeclared order; 2333 hands it to this ratchet's owner (2287) as a finding. The
    /// number is the dump's, not a sum.
    ///
    /// **5,063** — not a raise: a re-measurement. Until here the census read
    /// the *built* schedule, barriers included, and bevy shares one `ApplyDeferred` barrier per
    /// distance from the schedule's start — so 2,175 pairs (43%) were "ordered" only through a
    /// barrier the build happened to place between them, and moved whenever an edge anywhere
    /// upstream re-homed a group onto another barrier: that is 2333's five-out-six-in, and it is
    /// the one build step that can make a pair *enter* when an edge is added. The ratchet reads
    /// the declared graph now ([`SyncPoints::Declared`]); this is the same tree's count with the
    /// barriers gone, checked by removing one declared edge and watching exactly its five pairs
    /// return and nothing else move. The dump prints each pair in canonical order since the same
    /// record, so two dumps diff by text. The number is the dump's, not a sum.
    ///
    /// Raising this ceiling is a claim that a new undeclared order is acceptable; make it with
    /// the reason, or declare the order instead (`.after`, a set, a `chain`). If the pair is
    /// about a resource that commutes by construction, the claim belongs in [`Classes`].
    /// **5,608** — not a raise: a re-measurement. The 556 pairs with an
    /// exclusive system on one side were read as "explained" by a class argued for the build's
    /// barriers, which the declared graph (2337) no longer has; 413 are `finish_colliders`', 143
    /// the net drain's. Same tree, the class dropped, nothing else moved: 5,052 + 556.
    /// **5,554** — lowered: the TAB scan stopped reading `Visibility` (the draw
    /// election's verdict), which ended its pairs with every `Visibility` writer.
    const UPDATE_ACTIONABLE_CEILING: usize = 5_554;
    const UPDATE_ACTIONABLE_SLACK: usize = 40;

    fn ratchet(what: &str, n: usize, ceiling: usize, slack: usize) {
        eprintln!("{what}: {n} ambiguous pairs (ceiling {ceiling}, slack {slack})");
        assert!(
            n <= ceiling,
            "{n} ambiguous pairs in {what}; the ceiling is {ceiling}. A new system runs in an \
             undeclared order against something it shares state with — declare the order \
             (`.after`, a set, a `chain`), or raise the ceiling here with the reason. \
             `WOW_AMBIGUITY_DUMP=1` on this test lists the pairs; a resource that commutes by \
             construction belongs in `Classes` instead (decisions 2279, 2287)."
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
        // The declared graph, not the built one: a barrier bevy placed is not an order anyone
        // declared, and it moves under unrelated edits (2337; see [`SyncPoints`]).
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
                    // Canonical order within the pair: bevy reports (a, b) in whichever order
                    // its walk met them, and a diff of two dumps must not see that as movement.
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

    /// **How parallel could `Update` be?** A greedy list schedule with unlimited threads: each
    /// wave takes every system whose predecessors have run and that shares no undeclared
    /// conflict with a system already in the wave; non-`Send` systems are pairwise exclusive
    /// besides (the multi-threaded executor runs them one at a time on the main thread). The
    /// wave count is the frame's serial depth under that executor; the longest declared chain
    /// is its floor. Returned as `(waves, critical_path)`.
    fn waves(
        c: &Census,
        keep_conflict: impl Fn(&[ComponentId]) -> bool,
        non_send: &[SystemKey],
    ) -> (usize, usize) {
        use std::collections::{HashMap, HashSet};
        // The build-inserted sync points are left out: they are exclusive barriers the build
        // places from the flattened graph, which this model does not see; their count is
        // reported beside the waves instead.
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

    /// The structural half of 2265 §A3's executor question — is the prize of collapsing the VM
    /// (and the audio layer) to one `Send` owner correctness only, or correctness plus
    /// parallelism? Prints the serial depth of `Update` today and under each collapse.
    ///
    /// An instrument, not a gate: it asserts nothing, so it can only fail by panicking inside the
    /// census, and it boots a headless client to print four numbers. Ignored for the same reason
    /// `reference_ui`'s chain reports are (2331); run it by hand when the A3 question is live.
    #[test]
    #[ignore = "instrument: run by hand (2265 §A3's executor census) — cargo test -p benilla-app --lib concurrency_census -- --ignored --nocapture"]
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

    /// Why a system that holds the VM in `Update` and is ordered before the tick may stay OUT
    /// of [`crate::ui_script::UiFeed`] — keyed by a suffix of the system's full name, with the
    /// reason. Read beside [`every_vm_holder_in_update_declares_its_side_of_the_tick`].
    /// Empty at 2304: every holder ordered before the tick joined the phase (`ui_session::
    /// feed_interact_npc`, seated inside `WorldStage::Net` for decision 2022's reason, looked
    /// like the one exception and is not — it writes a resource the unit feed reads, and
    /// never holds the VM).
    const OUTSIDE_THE_FEED_PHASE: &[(&str, &str)] = &[];

    /// **Every system that holds the VM in `Update` declares its side of the tick** (decision
    /// 2304). The VM ticks once a frame (`extract::tick_script`, in `UiInput`): a push the tick
    /// must see rides `UiFeed`, which the plugin chains after the net drain and before the
    /// tick; a drain of what the tick produced is `.after(UiInput)`. Read off the built graph,
    /// so a membership through a parent set and an order through a chain both count. Three
    /// things fail here: a holder with no declared path to or from the tick; one ordered
    /// before the tick without riding the feed phase — so it may run before this frame's
    /// packets land, the class 2265 §A3 counted as "ordered only against `UiInput`" — unless
    /// it is argued in [`OUTSIDE_THE_FEED_PHASE`]; and a feed-phase member the graph does not
    /// actually place after the drain and before the tick, the set's contract checked rather
    /// than trusted. A row arguing a system the graph places elsewhere fails too.
    #[test]
    fn every_vm_holder_in_update_declares_its_side_of_the_tick() {
        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Built);
        if !type_names_available(&c) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with dev, 1451)"
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
             (decision 2304):\n  {}",
            offenders.join("\n  ")
        );
        assert!(stale.is_empty(), "stale rows:\n  {}", stale.join("\n  "));
    }

    /// The consumer markers 2220's census used: a system that holds the VM and does one of
    /// these to host state has spent something this frame that nothing can re-spend.
    const CONSUMES: &[&str] = &["fire_event(", "mem::take(", ".drain("];

    /// Why a one-shot consumer that holds the VM and is NOT gated on `ingame_ui_up` cannot lose
    /// anything in 2214's one-frame window — 2232's discriminator, as a type. (In that window
    /// the wire is in-world and the boot VM is still live and frameless, so a one-shot spent
    /// then is published to a VM with nothing to show it.)
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Because {
        /// The consumed queue is filled only by Lua asking; a frameless VM asks for nothing.
        FilledByVm,
        /// Filled only by a server reply to something the player had to click in the interface
        /// — or, noted in the reason, by another player's act on us, which can share a drain
        /// with the login burst only by coincidence (the residual 2279 names).
        PlayerRoundTrip,
        /// Consumed in the window or not, the state is re-asked or re-derived once the UI is up.
        SelfHealing,
        /// The publication is driven by a `VmMemo` diff, so a new VM re-derives and re-fires it.
        MemoLatched,
        /// Documented at the line as intentional.
        Deliberate,
    }

    /// Every one-shot consumer that holds the VM, is not gated on `ingame_ui_up` (by its own
    /// run condition or a set's — read off the real schedule, not off the registration text),
    /// and is not a memo-diffed `fire_event` — each with the reason it is safe. Keyed
    /// `(path under src/, fn)`. Audited 2026-09-16 against the fill sites;
    /// the reason names them. Adding a consumer the ordinary way (gated) passes; adding one
    /// ungated fails here, at the line, until it is either gated or argued.
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
         "filled only by a world right-click on a GameObject; the drain sends casts to the wire (2232's own bucket)"),
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
         "a roll exists only after a group member loots under group loot (2232's bucket); leftovers are cleared at socket teardown (another player's act: coincidence-only residual)"),
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
         "per-frame paint and cost state, not a queue (2232)"),
        ("ui_social/feed.rs", "feed_social", Because::MemoLatched,
         "the login-burst lists are re-announced to a new VM by `fed.seeded` (`VmMemo<FedSocial>`); the show flag is Lua's; a status line waits on the name resolve"),
        ("ui_stable/mod.rs", "feed_stable", Because::PlayerRoundTrip,
         "both packets follow a stable master's gossip and a click in its window"),
        ("ui_summon.rs", "feed_summon", Because::PlayerRoundTrip,
         "only another player's Ritual of Summoning latches the ask (2232's duel shape; coincidence-only residual)"),
        ("ui_tabard.rs", "drain_tabard", Because::FilledByVm,
         "the intents are Lua's; the event answers a Save intent"),
        ("ui_tabard.rs", "feed_tabard", Because::PlayerRoundTrip,
         "every latch follows a tabard-vendor click, and `reset_on_world_enter` zeroes the resource on entry"),
        ("ui_talent_wipe.rs", "feed_talent_wipe", Because::PlayerRoundTrip,
         "`MSG_TALENT_WIPE_CONFIRM` answers the trainer's gossip option"),
        ("ui_taxi/mod.rs", "feed_taxi", Because::PlayerRoundTrip,
         "both packets follow talking to a flight master (2232's own example)"),
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

    /// **Every one-shot consumer that holds the VM is gated, memo-latched, or argued** (2220's
    /// proposed test, built on the real schedule rather than on the registration text — which
    /// is why 2220 refused a mechanical pass: `.run_if(ingame_ui_up)` on a set, on a tuple, or
    /// on the member all read differently in source and identically here). A consumer is one
    /// of 2220's census: a system taking `NonSend[Mut]<UiScript>` whose body has a
    /// [`CONSUMES`] marker. It passes if the schedule shows `ingame_ui_up` on it or on a set
    /// above it, or if it is a `fire_event` driven by a `VmMemo` parameter, or if it is in
    /// [`EXEMPT`] with its reason. An `EXEMPT` row for a system that is gated after all, or
    /// that no longer consumes, fails too — the table describes the tree, not its history.
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
                "skipped: this build carries no type names (bevy/debug rides with dev, 1451)"
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
                // Name the sets it is under: the fix is usually a gate on one of them.
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
             interface being up — in 2214's one-frame window they publish to a VM with no frames \
             and the edge is lost (2220, 2232). Gate them (`.run_if(ingame_ui_up)`, on the system \
             or its set), or add them to `EXEMPT` with the reason they cannot be filled before \
             the interface exists (decision 2279):\n  {}",
            offenders.join("\n  ")
        );
        assert!(
            stale.is_empty(),
            "stale EXEMPT rows:\n  {}",
            stale.join("\n  ")
        );
    }
}
