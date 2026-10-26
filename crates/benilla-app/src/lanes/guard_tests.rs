//! The guard on the lanes: what the packet handlers queue for the interface fires from a lane,
//! in the packets' order (`0x5384d0`, `0x537aa0`). What still fires outside one is named here,
//! by the queue the handlers fill, by the feed that finds an edge by comparing state, or by the
//! exclusive system, so the list is the whole of the remaining work and only shrinks.

use std::any::TypeId;
use std::collections::{BTreeSet, HashMap};

use bevy::ecs::component::ComponentId;
use bevy::prelude::*;

use crate::game_plugins::schedule_tests::SyncPoints;
use crate::game_plugins::schedule_tests::{census, type_names_available};

/// The whole client headless ([`crate::game_plugins::schedule_tests::headless_client`]), with
/// every handler's and every lane firing's declared access read.
fn headless_client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    crate::net::handlers::measure(app.world_mut());
    super::measure(app.world_mut());
    app
}

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
        "::account_data::AccountData",
        "the account-data sections a download replaced, whose readers (the binding sets, the \
         macros, the layout) run again off it; the chat setup's fires on its lane",
    ),
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
        Row::HostHeld,
        "::minimap::ping::MinimapPing",
        "the members' minimap pings, which `drive_minimap_ping` fires; our own `PingLocation`, \
         acting in its call, holds the ping in the host",
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
        "::player::view_subject::FarSight",
        "the far-sight machine a handler's Lua runs through the host's buff cancel; every feed \
         that lends the host reads it, and none fires from it",
    ),
    (
        Row::HostHeld,
        "::ui_action::ActionSlots",
        "the action slots, which `feed_actions` resolves; a handler's Lua writes them in its \
         call, through the host's bar verbs",
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
        Row::HostHeld,
        "::ui_dialog_verbs::PetUnlearnState",
        "the pet unlearn confirm, which `feed_dialog_verbs` fires; `ConfirmPetUnlearn`, acting in \
         its call, reads it in the host",
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
    (
        Row::HostHeld,
        "::ui_logout::LogoutState",
        "the logout state, which `feed_logout` fires from; the session verbs, acting in their \
         calls, hold it in the host",
    ),
    (
        Row::Queue,
        "::ui_logout::SessionPosts",
        "the exit and the cinematic stop a session verb posts, which nothing the script reads \
         waits on",
    ),
    (
        Row::Queue,
        "::ui_session::latch::LatchCloses",
        "the close counts each window's core adds to, which every latched window's feed compares",
    ),
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
        Row::HostHeld,
        "::death::DeathNet",
        "the death stores, which `feed_death` fires the offers and the corpse range from; the death verbs, acting in their calls, hold them in the host",
    ),
    (
        Row::HostHeld,
        "::ui_bank::BankOpen",
        "the banker's cell, which `feed_bank` fires from; `PurchaseSlot` and `CloseBankFrame`, acting in their calls, hold it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_battlefield::Battlefield",
        "the battleground list, which `feed_battlefield` fires from; the join, acting in its call, reads it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_battlefield_positions::BattlefieldPositions",
        "the teammate positions, which `feed_battlefield_positions` fires from; the positions request, acting in its call, holds them in the host",
    ),
    (
        Row::HostHeld,
        "::ui_battlefield_score::BattlefieldScoreboard",
        "the scoreboard, which `feed_battlefield_score` fires from; the score request, acting in its call, holds its stamp in the host",
    ),
    (
        Row::HostHeld,
        "::ui_binder::BinderState",
        "the bind question, which `feed_binder` fires from; `ConfirmBinder`, acting in its call, reads it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_craft::CraftOpen",
        "the craft window's line, which `feed_craft` fires from; `CloseCraft`, acting in its call, holds it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_dialog_verbs::BattlefieldQueue",
        "the battleground queue slots, which `feed_dialog_verbs` and `feed_battlefield_score` fire from; the port, leave and positions verbs, acting in their calls, read them in the host",
    ),
    (
        Row::HostHeld,
        "::ui_duel::DuelState",
        "the duel session, which `feed_duel` fires from; the duel answers, acting in their calls, read it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_loot_roll::LootRolls",
        "the open rolls, which `feed_loot_rolls` fires from; the votes, acting in their calls, hold them in the host",
    ),
    (
        Row::HostHeld,
        "::ui_summon::SummonState",
        "the summon latch, which `feed_summon` fires from; `ConfirmSummon`, acting in its call, reads it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_tradeskill::TradeSkillOpen",
        "the profession window's line, which `feed_trade_skill` fires from; `CloseTradeSkill`, acting in its call, holds it in the host",
    ),
    (
        Row::HostHeld,
        "::ui_trainer::TrainerOpen",
        "the trainer's cell, which `feed_trainer` fires from; the trainer verbs, acting in their calls, hold it in the host",
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
         `SPELL_UPDATE_USABLE`) of the callers outside the field watchers and the cooldown list, \
         which refresh on their lanes: the selection's select and release where no call made \
         them (`0x493892`, `0x493a69`) and the control flag (`0x4958fc`, `0x495910`), one pass \
         a frame over the bar's 120 slots and the reactive list, diffed against the usable \
         cache",
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
    "::net::SelfGuid",
    // Read by the quest log getters through the host (`QuestClock`), and quiet: the time
    // response's `QUEST_LOG_UPDATE` fires on the quest log lane.
    "::net::ServerWallClock",
    "::net::game_time::ServerTime",
    "::spell::cast_send::HeldForPick",
    "::spell::cast_send::ScriptCastPermit",
    "::spell::cast_send::unit_caster::PetGcdsOwed",
    "::spell::cast_send::unit_caster::UnitAttackOwed",
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
    // Read, and quiet: the buff timers read the stamps through the host and no feed fires off
    // them; the aura fields' watcher raises `PLAYER_AURAS_CHANGED`.
    "::ui_aura::AuraDurations",
    // Read, and quiet: `feed_dialog_verbs` pushes the deadline the time packet stores and fires
    // nothing off it, as the packet's `AREA_SPIRIT_HEALER_IN_RANGE` goes on the notice lane.
    "::ui_dialog_verbs::AreaSpiritHealer",
    "::ui_loot::AutoLootByte",
    "::ui_loot::LootLatch",
    "::ui_merchant::MerchantOpen",
    "::ui_petition::GuildRegistrarState",
    "::ui_petition::PetitionRecords",
    "::ui_session::latch::InteractLatch",
    "::ui_social::SocialState",
    "::ui_tradeskill::TradeSkillOpens",
];

/// The view pushes that read what a handler writes as the frame left it, not as each listener's
/// packet did: each VM-holding feed of the `UiFeed` phase with the state the handlers write that
/// it reads itself, past the call host's own reads. The reference's getters read the live object
/// and caches at the listener's call (`UnitHealth` `0x5174d0`), and a handler runs whole before
/// the next (`0x537ae5`), so a listener reads its own packet's state; a push here hands a lane's
/// listener the frame's last. The list is the whole of that remaining work and only shrinks: a
/// feed goes once a log versions what it reads ([`crate::ui_unit::views`]), a new one fails, and
/// a row whose feed or inputs moved fails until it matches.
#[rustfmt::skip]
pub(super) const READS_FRAME_FINAL: &[(&str, &[&str])] = &[
    ("::player::net::feed_player_skinned", &["player::net::SkinnedFeed"]),
    ("::death::feed_death", &["Transform", "death::DeathNet", "items::Countdowns", "items::Items",
        "names::NameCache", "net::GuidIndex", "net::NetStatus", "net::ObjectStore",
        "net::SelfGuid"]),
    ("::minimap::ping::drive_minimap_ping", &["minimap::ping::MinimapPing",
        "ui_party::GroupState"]),
    ("::minimap::feed_game_time", &["net::game_time::ServerTime"]),
    ("::minimap::blips::drive_blip_tooltip", &["items::Countdowns", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore"]),
    // The four tokens' snapshots read the stores as of the drain's place (`ui_unit::views`); the
    // XP pair, the rest state, the combo points, the bar toggles and the worn display read the
    // live store, and every snapshot its names, relations, selection and group; the world entry's
    // pet reset reads the bar's guid and the attack latch as the frame left them.
    ("::ui_unit::feed_units", &["names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "net::SelfGuid", "net::reputations::Reputations", "target::Selection",
        "ui_guild::GuildState", "ui_party::GroupState", "ui_pet::PetAttackLatch",
        "ui_pet::PetBar"]),
    ("::ui_unit::feed_unit_reach", &["Transform", "names::NameCache", "net::GuidIndex",
        "net::ObjectStore", "net::SelfGuid", "net::reputations::Reputations",
        "ui_party::GroupState"]),
    // The chain ends' snapshots read their stores as of the drain's place (`ui_unit::views`); their
    // names, relations, raid marks and guild read as the frame left them.
    ("::ui_unit::held::feed_chain_units", &["names::NameCache", "net::GuidIndex",
        "net::ObjectStore", "net::SelfGuid", "net::reputations::Reputations",
        "ui_guild::GuildState", "ui_party::GroupState"]),
    ("::ui_unit::held::feed_name_candidates", &["names::NameCache", "net::GuidIndex",
        "net::ObjectStore", "net::SelfGuid", "net::reputations::Reputations",
        "ui_guild::GuildState", "ui_party::GroupState"]),
    ("::ui_unit::control_gate::feed_control_bypass", &["net::ObjectStore"]),
    ("::ui_unit::feed_farsight_focus", &["net::ObjectStore"]),
    ("::ui_unit::relations::feed_unit_relations", &["names::NameCache", "net::GuidIndex",
        "net::ObjectStore", "net::SelfGuid", "net::reputations::Reputations"]),
    ("::ui_unit::feed_default_language", &["net::ObjectStore"]),
    ("::ui_unit::feed_known_languages", &["net::ObjectStore", "ui_action::PlayerActions"]),
    // The party and raid tokens read the members' stores, records and the roster's cells as of
    // the drain's place (`ui_unit::views`), and the group getters read the cells through the host;
    // the roster push is their no-host fallback, and every snapshot's names and relations read as
    // the frame left them.
    ("::ui_party::feed::feed_party", &["names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "net::SelfGuid", "net::reputations::Reputations", "ui_guild::GuildState",
        "ui_party::GroupState"]),
    ("::ui_duel::feed_duel", &["names::NameCache", "ui_duel::DuelState"]),
    ("::ui_binder::feed_binder", &["ui_binder::BinderState"]),
    ("::ui_dialog_verbs::feed_dialog_verbs", &["ui_dialog_verbs::AreaSpiritHealer",
        "ui_dialog_verbs::BattlefieldQueue", "ui_dialog_verbs::InstanceBoot",
        "ui_dialog_verbs::PetUnlearnState"]),
    ("::ui_dialog_verbs::feed_meeting_stone", &["names::NameCache",
        "ui_dialog_verbs::MeetingStone"]),
    ("::ui_battlefield_score::feed_battlefield_score", &["names::NameCache",
        "ui_battlefield_score::BattlefieldScoreboard", "ui_dialog_verbs::BattlefieldQueue"]),
    ("::ui_battlefield::reset_on_world_enter", &["ui_battlefield::Battlefield"]),
    ("::ui_battlefield::feed_battlefield", &["names::NameCache", "ui_battlefield::Battlefield",
        "ui_dialog_verbs::BattlefieldQueue"]),
    ("::ui_battlefield_positions::feed_battlefield_positions", &["names::NameCache",
        "net::GuidIndex", "net::SelfGuid", "ui_battlefield_positions::BattlefieldPositions",
        "ui_dialog_verbs::BattlefieldQueue", "ui_party::GroupState"]),
    ("::tutorial::feed_tutorials", &["items::Items", "net::ObjectStore"]),
    ("::ui_summon::feed_summon", &["names::NameCache", "ui_summon::SummonState"]),
    ("::ui_gm_ticket::feed_gm_ticket", &["ui_gm_ticket::GmTicketState"]),
    ("::ui_instance::feed_instance", &["ui_instance::InstanceState"]),
    ("::ui_billing::poll_play_time", &["net::SessionBilling"]),
    ("::ui_logout::feed_logout", &["ui_logout::LogoutState"]),
    // The engine's follow transitions, which a `FollowUnit` a handler's listener makes arms.
    ("::ui_follow::feed_follow", &["player::follow::FollowState", "ui_follow::FollowFeed"]),
    ("::ui_social::feed::feed_social", &["names::NameCache", "ui_social::SocialState"]),
    ("::ui_petition::feed::feed_petition", &["names::NameCache", "net::ObjectStore",
        "net::SelfGuid", "ui_petition::GuildRegistrarState", "ui_petition::PetitionRecords",
        "ui_petition::PetitionState"]),
    ("::ui_tooltip::drive_mouseover_tooltip", &["Transform", "go_anim::GoAnim",
        "go_templates::GameObjectTemplates", "items::Countdowns", "items::Items",
        "names::NameCache", "net::GuidIndex", "net::ObjectStore", "net::SelfGuid",
        "net::reputations::Reputations", "ui_action::PlayerActions", "ui_guild::GuildState"]),
    ("::ui_tooltip::spell_feed::feed_spell_tooltips", &["items::Countdowns", "items::Items",
        "net::GuidIndex", "net::HomeBind", "net::ObjectStore", "net::UnitSpeeds",
        "net::motion::modes::UnitMoveModes", "net::motion::remote::RemoteMotion",
        "net::reputations::Reputations", "spell::mods::SpellModifiers",
        "ui_action::PlayerActions"]),
    // The ammo getters' hits fire after the push that carries their record, whenever it runs.
    ("::ui_char::feed_char", &["items::Countdowns", "items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "ui_char::AmmoRecordHits"]),
    ("::ui_char::feed_skills", &["net::ObjectStore"]),
    ("::ui_inspect::feed_inspect", &["items::Items", "names::NameCache", "net::GuidIndex",
        "net::ObjectStore", "net::SelfGuid"]),
    ("::ui_honor::feed_honor", &["net::ObjectStore", "target::Selection", "ui_honor::InspectHonor",
        "ui_inspect::InspectTarget", "ui_party::GroupState"]),
    ("::ui_action::recount::recount_item_actions", &["items::Countdowns", "items::Items",
        "net::GuidIndex", "net::ObjectStore"]),
    ("::ui_action::feed::feed_actions", &["items::Countdowns", "items::Items", "net::GuidIndex",
        "net::ObjectStore", "ui_action::ActionSlots", "ui_action::PlayerActions",
        "ui_action::errors::MountErrors",
        "ui_action::errors::PetTameFailures", "ui_action::errors::UiErrorTexts"]),
    ("::ui_action::state::feed_action_state", &["Transform",
        "creature_anim::sheath::CommittedSheath", "items::Countdowns", "items::Items",
        "names::NameCache", "net::GuidIndex", "net::ObjectStore", "net::SelfGuid",
        "net::UnitSpeeds", "net::game_time::ServerTime", "net::motion::modes::UnitMoveModes",
        "net::motion::remote::RemoteMotion", "net::reputations::Reputations",
        "player::state::StandPending", "spell::cooldowns::Cooldowns", "spell::mods::SpellModifiers",
        "target::Selection", "ui_action::ActionSlots", "ui_action::PlayerActions",
        "ui_action::state::ActionStateMemory", "ui_pet::PetBar"]),
    // The ranged-slot item scales a ranged spell's range, read as the frame left it (#2217).
    ("::spell::targeting::feed_targeting_to_vm", &["items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "net::UnitSpeeds", "net::motion::modes::UnitMoveModes",
        "net::motion::remote::RemoteMotion", "net::reputations::Reputations",
        "spell::mods::SpellModifiers", "spell::targeting::SpellTargeting", "ui_party::GroupState"]),
    // Each unit's list reads its store and record as of the drain's place (`ui_unit::views`), and
    // the buff cache is rebuilt at each packet's watcher; the resolver's inputs, the unit gate's
    // relations and the world entry's rebuild read as the frame left them.
    ("::ui_aura::feed_auras", &["names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "net::SelfGuid", "net::reputations::Reputations", "target::Selection",
        "ui_action::PlayerActions", "ui_party::GroupState", "ui_pet::PetBar"]),
    ("::ui_spellbook::feed_spellbook", &["items::Countdowns", "items::Items", "net::GuidIndex",
        "net::ObjectStore", "spell::cooldowns::Cooldowns", "ui_action::PlayerActions",
        "ui_spellbook::LearnedInTab"]),
    // The pet book's sorts, which each pet spell list counts and the derivation follows
    // (`0x4bdae0`): the frame's lists derive once, after the last.
    ("::ui_macro::rebind_macro_spells", &["ui_action::ActionSlots", "ui_action::PlayerActions",
        "ui_pet::PetBar"]),
    ("::ui_macro::load_macros", &["ui_action::ActionSlots"]),
    ("::ui_talent::feed_talents", &["net::ObjectStore", "ui_action::PlayerActions"]),
    ("::ui_talent_wipe::feed_talent_wipe", &["ui_talent_wipe::TalentWipeState"]),
    ("::ui_shapeshift::feed_shapeshift_bar", &["items::Countdowns", "items::Items",
        "names::NameCache", "net::GuidIndex", "net::ObjectStore", "net::SelfGuid",
        "net::reputations::Reputations", "spell::cooldowns::Cooldowns",
        "spell::mods::SpellModifiers", "target::Selection", "ui_action::PlayerActions"]),
    ("::ui_pet::bar::feed_pet_bar", &["net::GuidIndex", "net::ObjectStore", "net::SelfGuid",
        "ui_pet::PetAttackLatch", "ui_pet::PetBar"]),
    // The pet's snapshot reads its store as of the drain's place (`ui_unit::views`); its name
    // reads as the frame left it.
    ("::ui_pet::unit::feed_pet_unit", &["names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "net::SelfGuid", "ui_party::GroupState"]),
    ("::ui_pet::menu::feed_pet_menu", &["net::GuidIndex", "net::ObjectStore", "net::SelfGuid",
        "ui_pet::PetBar"]),
    ("::ui_pet_book::feed_pet_book", &["net::GuidIndex", "net::ObjectStore", "ui_pet::PetBar"]),
    ("::ui_pet_stats::feed_pet_stats", &["names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "net::SelfGuid", "ui_pet::PetBar"]),
    ("::ui_cast::feed_cast_reports", &["items::Countdowns", "items::Items", "net::GuidIndex",
        "net::ObjectStore", "spell::inflight::AutoRepeatActive", "spell::inflight::PendingCast",
        "spell::inflight::QueuedMeleeSpell", "spell::targeting::SpellTargeting",
        "ui_action::cast_lines::CastFailRedisplays"]),
    ("::ui_mirror::feed_mirror_timers", &["ui_mirror::MirrorTimerFeed"]),
    ("::ui_items::locks::apply_item_releases", &["items::Countdowns", "net::GuidIndex",
        "net::ObjectStore", "ui_loot::LootState"]),
    ("::ui_items::feed::feed_containers", &["items::Countdowns", "items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "spell::cooldowns::Cooldowns",
        "ui_petition::PetitionRecords"]),
    ("::ui_items::feed::feed_item_stats", &["items::Items", "net::HomeBind", "net::ObjectStore",
        "spell::mods::SpellModifiers"]),
    ("::ui_items::feed::feed_item_sets", &["item_names::ItemNames", "items::Items",
        "net::ObjectStore", "spell::mods::SpellModifiers"]),
    ("::ui_items::feed::ask_item_build_lookups", &["items::Countdowns", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "ui_petition::PetitionRecords"]),
    ("::ui_items::feed::feed_player_req", &["net::ObjectStore", "net::Proficiencies",
        "net::reputations::Reputations", "ui_action::LearnedAbilities",
        "ui_action::PlayerActions"]),
    ("::ui_items::lock_openers::feed_lock_openers", &["items::Countdowns", "net::GuidIndex",
        "net::ObjectStore", "ui_action::PlayerActions"]),
    ("::ui_gossip::feed_gossip", &["items::Countdowns", "names::NameCache", "net::GuidIndex",
        "net::ObjectStore", "ui_gossip::GossipState", "world_state::WorldStates"]),
    ("::ui_merchant::feed_merchant", &["items::Countdowns", "items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "ui_merchant::MerchantErrors",
        "ui_merchant::MerchantOpen"]),
    ("::ui_merchant::feed_repair_costs", &["items::Countdowns", "items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "net::SelfGuid", "net::reputations::Reputations",
        "ui_merchant::MerchantOpen"]),
    ("::ui_auction::feed_auction", &["items::Items", "names::NameCache",
        "ui_auction::AuctionOpen"]),
    ("::ui_auction::reset_auction_on_world_enter", &["ui_auction::AuctionOpen"]),
    ("::ui_bank::feed_bank", &["net::ObjectStore", "ui_bank::BankErrors", "ui_bank::BankOpen"]),
    ("::ui_mail::feed_mail", &["go_templates::GameObjectTemplates", "items::Countdowns",
        "items::Items", "names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "ui_mail::MailOpen", "ui_mail::pending::MailPending", "world_state::WorldStates"]),
    ("::ui_mail::send_query_next_mail_time_on_enter", &["items::Items", "ui_mail::MailOpen",
        "ui_mail::pending::MailPending"]),
    ("::ui_trade::feed_trade", &["items::Countdowns", "items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "ui_trade::TradeSession"]),
    ("::ui_item_text::feed_item_text", &["go_templates::GameObjectTemplates", "items::Countdowns",
        "items::Items", "names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "ui_item_text::ItemTextOpen", "ui_item_text::PageTexts", "ui_mail::MailOpen",
        "world_state::WorldStates"]),
    ("::ui_stable::feed_stable", &["names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "net::SelfGuid", "ui_pet::PetBar", "ui_stable::StableErrors", "ui_stable::StableOpen"]),
    ("::ui_tabard::feed_tabard", &["net::ObjectStore", "ui_guild::GuildState",
        "ui_tabard::TabardOpen"]),
    ("::ui_trainer::feed_trainer", &["items::Items", "names::NameCache", "net::GuidIndex",
        "net::HomeBind", "net::ObjectStore", "spell::mods::SpellModifiers",
        "ui_action::PlayerActions", "ui_pet::PetBar", "ui_trainer::TrainerErrors",
        "ui_trainer::TrainerOpen"]),
    ("::ui_taxi::feed_taxi", &["names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "net::SelfGuid", "net::reputations::Reputations", "ui_taxi::TaxiState",
        "ui_taxi::routing::TaxiRouteCache"]),
    ("::ui_taxi::feed_on_taxi", &["net::ObjectStore"]),
    // The getters' item callbacks, kept inside their calls, are spent as the frame left them.
    ("::ui_tradeskill::feed_trade_skill", &["items::Countdowns", "items::Items", "net::GuidIndex",
        "net::ObjectStore", "spell::cooldowns::Cooldowns", "ui_action::PlayerActions",
        "ui_tradeskill::TradeSkillItemWaits", "ui_tradeskill::TradeSkillOpen"]),
    // Beast Training reads the pet off its bar and its family off the creature record as the frame
    // left them (#2217).
    ("::ui_craft::feed_craft", &["items::Countdowns", "items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "spell::mods::SpellModifiers",
        "ui_action::PlayerActions", "ui_craft::CraftItemWaits", "ui_craft::CraftOpen",
        "ui_pet::PetBar"]),
    ("::ui_loot::feed_loot", &["items::Countdowns", "items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "ui_loot::AutoLootByte", "ui_loot::LootLatch",
        "ui_loot::LootState", "ui_party::GroupState"]),
    ("::ui_loot_roll::feed_loot_rolls", &["items::Items", "names::NameCache", "net::SelfGuid",
        "ui_loot_roll::LootRolls"]),
    ("::ui_quest::lines::feed_quest_lines", &["items::Items", "ui_quest::lines::QuestLines",
        "ui_quest_log::QuestLog"]),
    ("::ui_quest::feed_quest", &["go_templates::GameObjectTemplates", "items::Countdowns",
        "items::Items", "names::NameCache", "net::GuidIndex", "net::ObjectStore",
        "ui_quest::QuestGiver", "world_state::WorldStates"]),
    ("::ui_quest_log::feed_quest_log", &["names::NameCache", "net::ObjectStore",
        "ui_quest::QuestGiver", "ui_quest::lines::QuestLines", "ui_quest_log::QuestLog",
        "world_state::WorldStates"]),
    ("::ui_quest_share::feed_quest_share", &["names::NameCache", "ui_quest_share::QuestShare"]),
    ("::ui_chat::broadcast::feed_broadcasts", &["ui_chat::edit::ChannelState"]),
    ("::ui_chat::channels::seed_channels", &["ui_chat::edit::ChannelState"]),
    ("::ui_chat::feed::played_time_bridge", &["net::PlayedTimeAnswer"]),
];

/// The resources the handlers fill through a shared reference, which their declared access shows
/// as a read: the error keys a refusal pushes, the tutorial banks a watcher fires, the close
/// counts a window's core adds to and what a session verb posts. The lanes and the host's two
/// other sinks (the message sounds and the unit edits, which raise no event) are not queues here.
const FILLED_THROUGH_A_READ: &[&str] = &[
    // The account-data sections, which the download's handler stores through `&self`.
    "::account_data::AccountData",
    "::ui_action::errors::UiErrorKeys",
    // The far-sight machine the call host's buff cancel runs (`0x6e70ad`).
    "::player::view_subject::FarSight",
    "::tutorial::Tutorials",
    "::ui_session::latch::LatchCloses",
    "::ui_logout::SessionPosts",
];

/// Every other resource a handler reads and does not write: state, tables and caches it
/// consults, none of them a queue a feed takes. A new one fails until it is placed here or in
/// [`FILLED_THROUGH_A_READ`]. The list is kept by hand: an entry a handler in fact fills through a
/// shared reference leaves that queue out of [`handler_queues`], so a feed that takes it and fires
/// in system order, or a system that pushes onto a lane off it at its own place, passes every
/// check here unseen.
const READ_ONLY_TO_HANDLERS: &[&str] = &[
    // The login pick, which names the character the account-data sections belong to.
    "::glue_session::roster::Roster",
    "::ui_chat::edit::ChannelState",
    // The console's registry, which a `ConsoleExec` a listener makes reads.
    "::console::ConsoleCommands",
    // The creature families the cast send's pet learn rung reads (`0x60c7c0`).
    "::ui_pet_stats::PetFamilyTables",
    "::AreaTableRes",
    // The server clock the quest log getters read in the host.
    "::ServerWallClock",
    "::DamageTextGates",
    "::EnvDamageTable",
    "::SpellVisuals",
    "::Creatures",
    "::ItemDisplays",
    // `ItemClass.dbc`, whose names the host's `GetItemInfo` reads.
    "::ItemClasses",
    "::GameObjectTemplates",
    "::Enchants",
    "::Items",
    "::RandomProperties",
    "::ChrClassTable",
    "::ChrRaceTable",
    "::RacePvpTeams",
    "::MacroBoundSpells",
    "::NameCache",
    // The Video menus' lists the host's resolution and multisample verbs read (`0x48bc90`, `0x48c3e0`).
    "::video::Widescreen",
    "::view::MsaaFormats",
    "::ServerTime",
    "::GuidIndex",
    // The bag cache the open-container handler numbers its `BAG_OPEN`s from.
    "::BagCache",
    "::NetCommands",
    "::SelfGuid",
    "::OwnControlLost",
    "::Player",
    "::player::state::StandPending",
    // The page cache the host's page turns read.
    "::ui_item_text::PageTexts",
    // The streamed terrain the host's minimap ground cast traces.
    "::terrain_stream::TerrainStreamer",
    "::Assets<benilla_assets::adt::AdtTile>",
    // The pet trainer's latch, which the host's `ConfirmPetUnlearn` reads.
    "::ui_dialog_verbs::PetUnlearnState",
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
    "::PetitionRecords",
    "::ui_petition::PetitionState",
    "::ItemSubClasses",
    "::LootConfig",
    "::LootKneel",
    "::MailOpen",
    "::Stationery",
    // The repair discounts and the taxi graph the host's merchant and taxi verbs consult.
    "::RepairTables",
    "::TaxiCatalogs",
    // The bank's prices and the dialogs' states the host's bank, battleground, death, duel,
    // innkeeper and summon verbs consult.
    "::BankPrices",
    "::ui_battlefield::Battlefield",
    "::MapCatalogRes",
    "::BinderState",
    "::BattlefieldQueue",
    "::DuelState",
    "::SummonState",
    "::UiClock",
    "::InteractNpc",
    "::SocialState",
    "::SkillLines",
    "::SpellFocus",
    "::FrameStep",
    "::CurrentMap",
    "bevy_time::time::Time",
    "bevy_time::time::Time<bevy_time::real::Real>",
    // The stores as of the drain's place, which the call host's token lookups read.
    "::ui_unit::views::UnitViews",
    // The area under the player, which the host's raid rows read for our own zone (`0xb4e314`).
    "::CurrentArea",
    // The `raidFilter` books the call host's aura walk reads (`0x4b3870`, `0x4b3920`), which
    // only the aura feed writes.
    "::ui_aura::AuraFeedMemory",
    // The duration packet's stamps (`0x4e4390`), which the call host's buff timers read at the
    // call (`0x4e4450`); a hosted handler reads them, the packet's handler writes them.
    "::ui_aura::AuraDurations",
    // The host's two sinks that raise no event.
    "::sound::message::MessageSounds",
    "::creature_anim::edits::UnitEdits",
];

/// Every lane's queue, by type: what a handler fills there fires in the packets' order.
fn lane_types() -> [TypeId; 21] {
    [
        TypeId::of::<crate::ui_items::feed::BagView>(),
        // The views lane: what each packet changed in the stores.
        TypeId::of::<crate::ui_unit::views::ViewLog>(),
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
        // The list rebuilds a learn, an unlearn or a pet's spells owe, stamped at their packets.
        TypeId::of::<crate::ui_tradeskill::ListRebuilds>(),
        // The deaths lane: the signals each death's legs parked, stamped under its health edge.
        TypeId::of::<crate::target::Deaths>(),
        // The release lane: the signals each selection teardown parked, stamped under its release.
        TypeId::of::<crate::target::Releases>(),
        // The item-text lane: the page-text packets' reads, stamped at their packets.
        TypeId::of::<crate::ui_item_text::ItemTextLane>(),
        // The cooldown lanes: the player's list and the pet's, which log their writes and flushes
        // at their places.
        TypeId::of::<crate::spell::Cooldowns>(),
        TypeId::of::<crate::ui_pet::PetBar>(),
        // The reputation lane: the store's writes and the pane's list operations, at their places.
        TypeId::of::<crate::net::Reputations>(),
        // The quest log lane: the templates, toasts, rank moves and time syncs, at their packets.
        TypeId::of::<crate::ui_quest_log::QuestLog>(),
        // The cast views lane: the view each packet left, stamped ahead of the packet.
        TypeId::of::<crate::ui_cast::CastViews>(),
        // The guild lane: the roster, event and invitation packets' signals, each with the
        // version of the guild state its packet left.
        TypeId::of::<crate::ui_guild::GuildState>(),
    ]
}

/// [`lane_types`] by the lane that drains each: a queue is stripped only from its own lane's row,
/// so another lane's read of the same type shows as the state it is.
fn lane_queue_owners() -> [(&'static str, TypeId); 21] {
    [
        ("bag view", TypeId::of::<crate::ui_items::feed::BagView>()),
        ("unit views", TypeId::of::<crate::ui_unit::views::ViewLog>()),
        (
            "field notices",
            TypeId::of::<crate::field_watch::NoticeLane>(),
        ),
        ("item watchers", TypeId::of::<crate::item_lane::ItemLane>()),
        ("equip errors", TypeId::of::<crate::ui_items::EquipErrors>()),
        ("loot", TypeId::of::<crate::ui_loot::LootState>()),
        ("chat", TypeId::of::<crate::ui_chat::ChatLog>()),
        ("cast reports", TypeId::of::<crate::ui_cast::CastReports>()),
        (
            "auction house",
            TypeId::of::<crate::ui_auction::HouseLane>(),
        ),
        ("party", TypeId::of::<crate::ui_party::GroupState>()),
        ("party", TypeId::of::<crate::ui_party::PartyLane>()),
        (
            "list rebuilds",
            TypeId::of::<crate::ui_tradeskill::ListRebuilds>(),
        ),
        ("deaths", TypeId::of::<crate::target::Deaths>()),
        ("releases", TypeId::of::<crate::target::Releases>()),
        (
            "item text",
            TypeId::of::<crate::ui_item_text::ItemTextLane>(),
        ),
        ("cooldowns", TypeId::of::<crate::spell::Cooldowns>()),
        ("pet cooldowns", TypeId::of::<crate::ui_pet::PetBar>()),
        ("reputation", TypeId::of::<crate::net::Reputations>()),
        (
            "quest events",
            TypeId::of::<crate::ui_quest_log::QuestLog>(),
        ),
        ("cast views", TypeId::of::<crate::ui_cast::CastViews>()),
        ("guild", TypeId::of::<crate::ui_guild::GuildState>()),
    ]
}

/// Whether `id` names a `Messages<T>` buffer, which a reader takes from through a read.
fn is_messages(world: &World, id: ComponentId) -> bool {
    name(world, id).contains("Messages<")
}

pub(super) fn name(world: &World, id: ComponentId) -> String {
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
    for (_, access) in handlers.accesses() {
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
        .accesses()
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
    for (_, access) in handlers.accesses() {
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
            "cast views",
            "chat",
            "chat setup arrivals",
            "cooldowns",
            "deaths",
            "equip errors",
            "field notices",
            "guild",
            "item text",
            "item watchers",
            "list rebuilds",
            "loot",
            "party",
            "pet cooldowns",
            "quest events",
            "releases",
            "reputation",
            "unit views"
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
    (
        "::query_cache::release_on_enter<benilla_app::ui_guild::GuildState>",
        "the world entry forgets the guild cache's in-flight asks, so each is asked again \
         (`0x5557c0`): it queues nothing",
    ),
    (
        "::query_cache::release_on_enter<benilla_app::ui_quest_log::QuestLog>",
        "the world entry forgets the quest cache's in-flight asks, so each is asked again \
         (`0x5557c0`): it queues nothing",
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
    for a in app.world().resource::<super::Lanes>().accesses() {
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

/// The lane queues a view push reads as a queue, which [`READS_FRAME_FINAL`] does not count:
/// what they hold fires from their own lanes.
const QUEUES_NOT_VIEWS: &[&str] = &[
    "field_watch::NoticeLane",
    "item_lane::ItemLane",
    "ui_chat::ChatLog",
    "ui_cast::CastReports",
    "ui_cast::views::CastViews",
    "ui_auction::net::HouseLane",
    "ui_party::feed::PartyLane",
    "ui_items::EquipErrors",
    "ui_unit::views::ViewLog",
    "ui_items::feed::BagView",
];

/// A type's name as [`READS_FRAME_FINAL`] spells it: the crate's own by its path in the crate, an
/// engine type by its own name.
pub(super) fn short_name(full: &str) -> String {
    match full.strip_prefix("benilla_app::") {
        Some(rest) => rest.to_string(),
        None => full.rsplit("::").next().unwrap_or(full).to_string(),
    }
}

/// Every view push that reads what a handler writes has its row, whose inputs are exactly the
/// handler-written state it reads: each component a handler writes and each resource one writes,
/// past the VM, the message buffers and the lane queues. A feed's own reads are its declared
/// access less the call host's, entry by entry, as every feed that lends the host holds all of
/// the host's.
#[test]
fn every_view_push_of_what_a_handler_writes_is_listed() {
    use bevy::ecs::query::{ComponentAccessKind, FilteredAccess};
    use bevy::ecs::schedule::NodeId;
    let mut app = headless_client();
    let (components, resources) = {
        let handlers = app.world().resource::<crate::net::handlers::NetHandlers>();
        let mut components = BTreeSet::new();
        let mut resources = BTreeSet::new();
        for (_, access) in handlers.accesses() {
            let access = access.combined_access();
            if let Ok(kinds) = access.try_iter_component_access() {
                components.extend(kinds.filter_map(|k| match k {
                    ComponentAccessKind::Exclusive(id) => Some(id),
                    _ => None,
                }));
            }
            resources.extend(access.resource_writes());
        }
        (components, resources)
    };
    let (pushes, systems) = app.world_mut().schedule_scope(Update, |world, schedule| {
        schedule.graph_mut().systems.initialize(world);
        let vm = world
            .components()
            .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
        let host: Vec<FilteredAccess> = {
            use bevy::ecs::system::System;
            let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
            System::initialize(&mut host, world)
                .filtered_accesses()
                .to_vec()
        };
        let input = |id: ComponentId| {
            let n = name(world, id);
            (Some(id) != vm && !n.contains("Messages<")).then(|| short_name(&n))
        };
        let graph = schedule.graph();
        let mut pushes: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut systems = BTreeSet::new();
        for (key, system, _) in graph.systems.iter() {
            let name = system.name().to_string();
            systems.insert(name.clone());
            if !in_feed(graph, NodeId::System(key)) || system.is_exclusive() {
                continue;
            }
            let Some(with_access) = graph.systems.get(key) else {
                continue;
            };
            if !vm.is_some_and(|vm| with_access.access.combined_access().has_resource_write(vm)) {
                continue;
            }
            let mut own: Vec<&FilteredAccess> =
                with_access.access.filtered_accesses().iter().collect();
            for h in &host {
                if let Some(i) = own.iter().position(|o| *o == h) {
                    own.remove(i);
                }
            }
            let mut inputs = BTreeSet::new();
            for access in own.iter().map(|o| o.access()) {
                let read_c = components
                    .iter()
                    .filter(|&&id| access.has_component_read(id));
                let read_r = resources.iter().filter(|&&id| access.has_resource_read(id));
                inputs.extend(read_c.chain(read_r).filter_map(|&id| input(id)));
            }
            inputs.retain(|n| !QUEUES_NOT_VIEWS.contains(&n.as_str()));
            if !inputs.is_empty() {
                pushes.insert(short_name(&name), inputs);
            }
        }
        (pushes, systems)
    });
    if !systems.iter().any(|n| n.contains("benilla_app::")) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let row = |system: &str| {
        READS_FRAME_FINAL
            .iter()
            .find(|(s, _)| format!("::{system}").ends_with(s))
    };
    let mut wrong: Vec<String> = pushes
        .iter()
        .filter(|(system, inputs)| {
            row(system).is_none_or(|(_, listed)| {
                listed
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<BTreeSet<_>>()
                    != **inputs
            })
        })
        .map(|(system, inputs)| {
            let inputs: Vec<String> = inputs.iter().map(|i| format!("{i:?}")).collect();
            format!("(\"::{system}\", &[{}]),", inputs.join(", "))
        })
        .collect();
    wrong.sort();
    let stale: Vec<&str> = READS_FRAME_FINAL
        .iter()
        .filter(|(s, _)| {
            !pushes
                .keys()
                .any(|system| format!("::{system}").ends_with(s))
        })
        .map(|(s, _)| *s)
        .collect();
    assert!(
        wrong.is_empty(),
        "these feeds push views off state a handler writes, read as the frame left it: version \
         what they read through the views lane, or give each its row in READS_FRAME_FINAL as it \
         stands:\n  {}",
        wrong.join("\n  ")
    );
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}

/// The lanes' own state that every firing of the drain reads as of its place, not as the frame left
/// it: the lane replays it packet by packet in stamp order among the others, so a firing between
/// two of its writes sees the first alone. A feed, which runs ahead of the drain, still reads it
/// as the frame left it, and its row says so.
const VERSIONED_FOR_THE_DRAIN: &[&str] = &["net::reputations::Reputations"];

/// The lane firings that read what a handler writes as the frame left it, by lane, beside
/// [`READS_FRAME_FINAL`]'s feeds: what a firing reads past the call host's own reads and its own
/// lane's queues ([`lane_queue_owners`]), the field watchers' callbacks among them (the notice lane's). A listener fired
/// there reads it as the frame's last packet left it, not as its own did (`0x537ae5`). The list
/// only shrinks: a read goes once a log versions it ([`crate::ui_unit::views`]), and a row that no
/// longer matches its firing fails.
#[rustfmt::skip]
const LANE_READS_FRAME_FINAL: &[(&str, &[&str])] = &[
    // The chat setup's restore reads the channel rows its shortcuts and auto-joins come from as
    // the frame left them, past the notices of the packets after the section's.
    ("chat setup arrivals", &["ui_chat::edit::ChannelState"]),
    // The watchers' callbacks: the quest log's slots, the skill line's race and class and the
    // bonus bar's form off our live descriptor, as the frame left them (#2217); the aura
    // watcher's own store is the one its packet left, and the buff timers read the duration
    // packet's stamps through the call host. The drunk line's name is read through the host.
    // `SetPetGuid`'s trainer refresh reads the open trainer, and its `PET_UI_CLOSE` the new
    // pet's object and ours, as the frame left them.
    ("field notices", &["net::GuidIndex", "net::ObjectStore", "ui_trainer::TrainerOpen"]),
    // The views lane reads the stores, records and roster cells it rewinds as of each packet, and
    // the reputations as the reputation lane replays them; the names, the relations' stores, the
    // selection and the guild as the frame left them (#2217).
    // The roster's cells and records it rewinds and seats while it builds; the rest as the frame
    // left it.
    ("unit views", &["names::NameCache", "net::GuidIndex", "net::ObjectStore", "net::SelfGuid",
        "target::Selection", "ui_guild::GuildState", "ui_party::GroupState"]),
    // A landing's record expands its text's macros against the player's name and the world states
    // as the frame left them, as the quest log's feed expands the held ones (#2217).
    ("quest events", &["names::NameCache", "net::ObjectStore", "world_state::WorldStates"]),
    ("cast reports", &["items::Countdowns", "items::Items", "net::GuidIndex", "net::ObjectStore",
        "ui_action::cast_lines::CastFailRedisplays"]),
    ("item watchers", &["items::Countdowns", "items::Items", "names::NameCache", "net::GuidIndex",
        "net::ObjectStore", "ui_trade::TradeSession"]),
    ("equip errors", &["items::Countdowns", "items::Items", "net::GuidIndex", "net::ObjectStore"]),
    ("auction house", &["items::Items", "names::NameCache", "net::SelfGuid",
        "ui_auction::AuctionOpen"]),
    // The master-loot candidates placed and named off the raid roster's subgroups as the frame
    // left it, not as the push's packet did (`0x61c550`, #2217).
    ("loot", &["items::Countdowns", "items::Items", "names::NameCache", "net::GuidIndex",
        "net::ObjectStore", "ui_loot::AutoLootByte", "ui_loot::LootLatch",
        "ui_loot::LootMoveStart", "ui_party::GroupState"]),
    // The reader's open reads the object, its records and the text caches as the frame left them,
    // past the answers still on its lane; its register closes whichever latched window holds the
    // latch, reading that window's state as the frame left it.
    ("item text", &["go_templates::GameObjectTemplates", "items::Countdowns", "items::Items",
        "net::GuidIndex", "net::ObjectStore", "ui_auction::AuctionOpen", "ui_bank::BankOpen",
        "ui_gossip::GossipState", "ui_item_text::ItemTextOpen", "ui_item_text::PageTexts",
        "ui_mail::MailOpen", "ui_merchant::MerchantOpen", "ui_petition::GuildRegistrarState",
        "ui_quest::QuestGiver", "ui_session::latch::InteractLatch", "ui_stable::StableOpen",
        "ui_tabard::TabardOpen", "ui_taxi::TaxiState", "ui_trade::TradeSession",
        "ui_trainer::TrainerOpen"]),
    ("chat", &["go_templates::GameObjectTemplates", "items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "net::SelfGuid", "ui_chat::edit::ChannelState",
        "world_state::WorldStates"]),
];

/// Every lane firing's read of what a handler writes is listed in [`LANE_READS_FRAME_FINAL`],
/// found as [`every_view_push_of_what_a_handler_writes_is_listed`] finds a feed's; an exclusive
/// firing, which shows Bevy no access, by the reads it declares ([`super::LaneReads`]), and every
/// exclusive firing declares them.
#[test]
fn every_lane_reads_of_what_a_handler_writes_is_listed() {
    use bevy::ecs::query::{ComponentAccessKind, FilteredAccess};
    let mut app = headless_client();
    let (components, resources) = {
        let handlers = app.world().resource::<crate::net::handlers::NetHandlers>();
        let mut components = BTreeSet::new();
        let mut resources = BTreeSet::new();
        for (_, access) in handlers.accesses() {
            let access = access.combined_access();
            if let Ok(kinds) = access.try_iter_component_access() {
                components.extend(kinds.filter_map(|k| match k {
                    ComponentAccessKind::Exclusive(id) => Some(id),
                    _ => None,
                }));
            }
            resources.extend(access.resource_writes());
        }
        (components, resources)
    };
    let world = app.world_mut();
    let vm = world
        .components()
        .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
    let host: Vec<FilteredAccess> = {
        use bevy::ecs::system::System;
        let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
        System::initialize(&mut host, world)
            .filtered_accesses()
            .to_vec()
    };
    if !name(world, vm.expect("the VM's resource")).contains("benilla_ui") {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let owners = lane_queue_owners();
    assert_eq!(
        owners.map(|(_, t)| t).into_iter().collect::<BTreeSet<_>>(),
        lane_types().into_iter().collect::<BTreeSet<_>>(),
        "every lane queue has its owning lane"
    );
    let lane_queues: Vec<(&str, String)> = owners
        .iter()
        .filter_map(|(lane, t)| Some((*lane, world.components().get_resource_id(*t)?)))
        .map(|(lane, id)| (lane, short_name(&name(world, id))))
        .collect();
    let lanes = world.resource::<super::Lanes>();
    let mut found: Vec<(&str, BTreeSet<String>)> = Vec::new();
    let mut undeclared = Vec::new();
    let firings = lanes
        .names()
        .into_iter()
        .zip(lanes.accesses())
        .zip(lanes.exclusive_reads());
    for ((lane, access), (exclusive, declared)) in firings {
        // An exclusive firing shows Bevy no access: the access its declared reads measure stands
        // in ([`super::measure`]).
        match (exclusive, declared) {
            (true, false) => {
                undeclared.push(format!("{lane:?} is exclusive and declares no reads"));
                continue;
            }
            (false, true) => undeclared.push(format!(
                "{lane:?} declares reads, but its firing is not exclusive"
            )),
            _ => {}
        }
        let mut own: Vec<&FilteredAccess> = access.filtered_accesses().iter().collect();
        // A firing that lends the host shows the host's access beside its own; an exclusive
        // firing's declared reads are its own alone.
        for h in host.iter().filter(|_| !exclusive) {
            if let Some(i) = own.iter().position(|o| *o == h) {
                own.remove(i);
            }
        }
        let mut inputs = BTreeSet::new();
        for access in own.iter().map(|o| o.access()) {
            let read_c = components
                .iter()
                .filter(|&&id| access.has_component_read(id));
            let read_r = resources.iter().filter(|&&id| access.has_resource_read(id));
            for &id in read_c.chain(read_r) {
                let n = name(world, id);
                if Some(id) != vm && !n.contains("Messages<") {
                    inputs.insert(short_name(&n));
                }
            }
        }
        inputs.retain(|n| {
            !QUEUES_NOT_VIEWS.contains(&n.as_str())
                && !VERSIONED_FOR_THE_DRAIN.contains(&n.as_str())
                && !lane_queues.iter().any(|(l, q)| *l == lane && q == n)
        });
        found.push((lane, inputs));
    }
    assert!(undeclared.is_empty(), "{}", undeclared.join("\n"));
    let wrong: Vec<String> = found
        .iter()
        .filter(|(lane, inputs)| {
            let listed = LANE_READS_FRAME_FINAL.iter().find(|(l, _)| l == lane);
            let listed: BTreeSet<String> = listed
                .map(|(_, i)| i.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default();
            listed != *inputs
        })
        .map(|(lane, inputs)| {
            let inputs: Vec<String> = inputs.iter().map(|i| format!("{i:?}")).collect();
            format!("({lane:?}, &[{}]),", inputs.join(", "))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "these lanes' firings read state a handler writes as the frame left it: version it, or \
         give each its row in LANE_READS_FRAME_FINAL as it stands:\n  {}",
        wrong.join("\n  ")
    );
}
