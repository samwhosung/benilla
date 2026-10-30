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
pub(super) enum Row {
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
pub(super) const NOT_YET_ON_A_LANE: &[(Row, &str, &str)] = &[];

/// The host-held queues a handler fills that no feed fires off, each named quiet beside why: the
/// check finds a queue the handlers and the host both hold, but not whether a feed fires off it
/// what a packet raised, so each is read for that before it goes here, and one a feed does fire
/// off is a [`Row::HostHeld`] row instead. A new one fails until it is read, and an entry whose
/// queue the handlers or the host no longer hold fails as stale.
pub(super) const HOST_HELD_UNREAD: &[&str] = &[
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: the account-data sections, which
    // the logout verb flushes inside its call (`0x5ab049` → `0x5afd30`), and quiet: each section's
    // reader runs on its own lane at the settling's stamp, inside the packet that settled it
    // (`0x5afaca`, `0x5afd19`), the bindings', the layout's, the macros' and the chat setup's.
    "::account_data::AccountData",
    // Quiet: StartAttack reads the stop word in its call (`0x5eccda`), the attack stop's handler
    // clears it (`0x624e6b`), and no feed reads it.
    "::creature_anim::AttackStopInFlight",
    // Quiet: the animations the handlers and the verbs ask for, which the model systems play; no
    // getter reads a model's sequence.
    "::creature_anim::PlaySeq",
    // Quiet: the gestures a chat line's firing and a world click's greeting queue, which
    // `drive_gestures` plays as the speaker's animation (`0x60bb30` → `0x5fcd20`); no getter
    // reads a model's gesture, and no feed fires off it.
    "::creature_anim::gesture::GestureQueue",
    // Read through the host by the set block, and quiet: a member's answer rebuilds the tooltip
    // waiting on it through the build's own wait, read as the tick runs.
    "::item_names::ItemNames",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: the item records, read through
    // the host by the getters at their calls (`0x55ba30`), and quiet. Every callback a lookup
    // leaves runs in its answer's list in the order registered (`0x55bdb0`, `0x55bf12`-`0x55bf39`),
    // the dress-up's (`0x504ba0`), the combat log's (`0x6294b0`) and the auction lists'
    // (`0x4cca10`, `0x4ccc60`, `0x4cceb0`) among them; the equipment display's (`0x5ed880`) dresses
    // the model and signals nothing, a draw; and the item-record feed's quest log signal
    // (`0x4e0830`) falls due only for a record pushed outside every packet.
    "::items::Items",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: read, and quiet. A member's ping
    // stores the point at its packet and signals on the notice lane (`0x4ee330`), our own
    // `PingLocation` holds it in the host in its call, and only a new VM's seed reads it.
    "::minimap::ping::MinimapPing",
    // Quiet: the object index, which the getters read through the host as they run; the feeds that
    // read it push views (`READS_FRAME_FINAL`), and none fires off its change.
    "::net::GuidIndex",
    // Quiet: `GetBindLocation` (`0x48dae3` → `0x5ed420`) and the `$z` token (`0x507f07` →
    // `0x5ed420`) of the trainer's, the craft list's and the tooltips' descriptions read it through
    // the host at the call; no feed fires off it.
    "::net::HomeBind",
    // Read through the host by the item requirement getters, and quiet: no feed fires off them.
    "::net::Proficiencies",
    // Quiet: our own guid, which the login's handlers set and everything reads; the arrival's
    // signals are the arrival lane's, and no feed fires off its change.
    "::net::SelfGuid",
    // Written by the group handlers at their packets, read through the host by the relation
    // getters (`0x4e7f70` inside `0x6061e0`), and quiet: no feed fires off it.
    "::spell::group_relation::GroupRoster",
    // Read by the quest log getters through the host (`QuestClock`), and quiet: the time
    // response's `QUEST_LOG_UPDATE` fires on the quest log lane, and `send_query_time`'s hourly
    // resync is a timer.
    "::net::ServerWallClock",
    // Read through the host by `GetGameTime` (`0x515ee0`), and quiet: no event signals the clock.
    "::net::game_time::ServerTime",
    // Read, and quiet: the control packet's handler and our own create's arrival signal its
    // changes on the notice lane in place (`0x4958e0`), with the usable refresh after them.
    "::player::state::OwnControlLost",
    // Read through the host by the world click's legs (who we drive, the gathering skills its
    // cursor forks on), and quiet: no feed fires off them.
    "::player::state::MoverLock",
    // Read through the host by `HasFullControl` (`0x51a142`), and quiet: the update handler
    // starts or ends far sight in place at the field's change (`0x5de0d0`) and starts it at its
    // object's create (`0x5fbb1a`, `0x5d552d`), the clear's handler ends it (`0x5e441d`), the
    // teardowns free it with our player, the buff cancel runs `UpdateFarSight` in its call
    // (`0x6e70ad`), `PLAYER_FARSIGHT_FOCUS_CHANGED` is the field watch's, and no feed fires off it.
    "::player::view_subject::FarSight",
    // Read, and quiet: the follow's arm reads them in its call (`0x611130` → `0x60fb60`), and the
    // handlers that end an auto-move signal its `AUTOFOLLOW_END` on the notice lane in place
    // (`0x60fb84`).
    "::player::approach::Approach",
    "::player::follow::FollowState",
    // Quiet: the autorun bit as the frame's calls leave it, read and written inside each call
    // (`0x513de7`), which an auto-move's stop clears in place (`0x514630`).
    "::player::press::CallAutorun",
    // Quiet: the cast ladder's per-call state, which the cast result's, the loot response's and the
    // go's handlers hold with the ladder and never write. A press holds the first and its call resumes it before it returns
    // (`0x6e4efb`), each call sets and puts back the second, and the pet press applies the third
    // in its call; no feed reads them.
    "::spell::cast_send::HeldForPick",
    "::spell::cast_send::ScriptCastPermit",
    "::spell::cast_send::unit_caster::UnitAttackOwed",
    // Quiet: the cast in flight, which `SpellStopCasting` and the bar's tests read through the
    // host, as of the drain's place, and the spell packets' events fire on the cast lane at their
    // handlers.
    "::spell::inflight::ActiveChannel",
    "::spell::inflight::AutoRepeatActive",
    "::spell::inflight::PendingCast",
    "::spell::inflight::QueuedMeleeSpell",
    // Quiet: read by the cost and `$`-token builds through the host at the call; the targeting
    // feed pushes the input's word off it (`BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE`), and none
    // fires off it.
    "::spell::mods::SpellModifiers",
    // Quiet: the units a select found the targeting cursor up for, which its writer binds at the
    // select's place (`0x4935d5` → `0x6e5b40`): a handler's in its packet (the loot response's
    // `0x48f3be`, the acquires' `0x6259fe` and `0x6e799c`, the go's deferred start's `0x5eccaa`), a
    // call's before its next step; no feed reads it.
    "::spell::targeting::CursorPicks",
    // Quiet: the targeting word, which `SpellIsTargeting` (`0x6e6cd0`) reads through the host;
    // the feed publishes the world pick's words for the next frame's input, and no feed fires off
    // it.
    "::spell::targeting::SpellTargeting",
    // Quiet: every selection writer signals `PLAYER_TARGET_CHANGED` in place (`0x49389c`), and the
    // getters' `"target"` reads it through the host (`0x515a86`); the feeds that read it push
    // views.
    "::target::Selection",
    // Quiet: the world click's and `DropItemOnUnit`'s latches, read in their calls, which the
    // learn, unlearn and supersede handlers write at their packet (`0x4b262c`, `0x5e9e49`,
    // `0x4b2c83`); no feed fires off them.
    "::ui_action::LearnedAbilities",
    // Read through the host by the bar's getters, and quiet: the bar's words, which our arrival's
    // pass writes into the VM's copy (`0x5deb00`), the verbs and the spell packets' clears and
    // replaces at their writes (`0x4e5d60`, `0x4e5db0`), and a new VM's seed copies; no feed fires
    // off them.
    "::ui_action::ActionSlots",
    // Quiet: the reactive list, which the learn and supersede handlers' registrar leg grows at
    // their packet (`0x4b27b3`-`0x4b27c7`) and our create's arrival empties and fills again
    // (`0x5dea86`); the usable refreshes re-check it in their places, and no feed reads it.
    "::ui_action::ReactiveSpells",
    // Quiet: the bar's usable cache, which the game entry's handler zeroes (`0x48f5a4` →
    // `0x4e4f50`) and the host's slot writes and refreshes store in place (`0x4e590a`,
    // `0x4e5c00`); the getters read it at the call.
    "::ui_action::state::ActionStateMemory",
    // Quiet: the known spells and the book's lists, which the learn, unlearn and supersede handlers
    // write at their packet beside the bar's clears and replaces, whose signals go on the notice
    // lane there (`0x5ea253`, `0x5e6356`); the feeds that read it push views (`READS_FRAME_FINAL`),
    // and none fires off it.
    "::ui_action::PlayerActions",
    // Read through the host by the auction getters, and quiet: the hello's show, the lines, the
    // patches, the removals and the pages fire on the house lane at their handlers.
    "::ui_auction::AuctionOpen",
    // Quiet: the craft and profession windows' open type and line, which an opener's cast
    // toggles and signals inside its call (`0x4fc910`, `0x4f6000`), the closes zero in theirs, and
    // the disconnect handlers (`ui_craft::on_session_end`, `ui_tradeskill::on_session_end`) clear
    // without a signal; the profession and craft builds and getters read the line and the type
    // through the host at their places.
    "::ui_craft::CraftOpen",
    "::ui_tradeskill::TradeSkillOpen",
    // Quiet: the profession openers a cast intercepted (`0x6e4bd7`), which the call host toggles
    // before the call returns and the disconnect handler (`ui_tradeskill::on_session_end`)
    // empties; no feed reads it.
    "::ui_tradeskill::TradeSkillOpens",
    // Read through the host by the bank getters, and quiet: its events fire on the bank lane at
    // their handlers, and `PurchaseSlot` and `CloseBankFrame` hold it in their calls.
    "::ui_bank::BankOpen",
    // Read, and quiet: the buff timers read the stamps through the host and no feed fires off
    // them; the aura fields' watcher raises `PLAYER_AURAS_CHANGED`.
    "::ui_aura::AuraDurations",
    // Read through the host by the battleground getters, and quiet: their events fire on the
    // battleground lane at their handlers.
    "::ui_battlefield::Battlefield",
    "::ui_battlefield_positions::BattlefieldPositions",
    "::ui_battlefield_score::BattlefieldScoreboard",
    "::ui_dialog_verbs::BattlefieldQueue",
    // Read through the host by `GetAreaSpiritHealerTime`, and quiet: the time packet's
    // `AREA_SPIRIT_HEALER_IN_RANGE` goes on the notice lane, and `poll_area_spirit_healer` scans
    // off it every frame, as the world frame's update does after the packets (`0x4818ca` →
    // `0x4923b0`).
    "::ui_dialog_verbs::AreaSpiritHealer",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: the dialogs' getters and verbs
    // read them through the host, and no feed fires off them, as their handlers' events go on the
    // dialogs' lane. The duel's countdown is a timer ahead of the packets (`0x4d4930`, its
    // `tick_countdown`), and the instance latch's writer is `track_instance_state`'s row in
    // `BETWEEN_THE_DISPATCH_AND_THE_DRAIN`.
    "::ui_binder::BinderState",
    "::ui_dialog_verbs::InstanceBoot",
    "::ui_dialog_verbs::MeetingStone",
    "::ui_dialog_verbs::PetUnlearnState",
    "::ui_duel::DuelState",
    "::ui_instance::InstanceState",
    "::ui_summon::SummonState",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: the inspect-honor getters read it
    // through the host, and no feed fires off it, as the reply's `INSPECT_HONOR_UPDATE` goes on
    // the notice lane at its handler and a re-target's fires in its call.
    "::ui_honor::InspectHonor",
    // Read through the host by the gossip getters, and quiet: the draw's `GOSSIP_SHOW` fires on the
    // gossip lane at its handler, and the gossip verbs hold it in their calls.
    "::ui_gossip::GossipState",
    // Quiet: the reader's session, which the item-text getters read through the host and an
    // item's deactivate closes in the object packet's handler (`0x5d8713`); `ITEM_TEXT_CLOSED` goes
    // on the item-text lane at its place, and no feed fires off it.
    "::ui_item_text::ItemTextOpen",
    // Quiet: the loot session's auto-loot byte and latch, which the response's admit and the
    // closes read in their handlers and calls; `LOOT_OPENED` and `LOOT_CLOSED` fire on the loot
    // lane, and no feed fires off them.
    "::ui_loot::AutoLootByte",
    "::ui_loot::LootLatch",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: the mail getters read them
    // through the host, and no feed fires off them, as the packets', the cores' and the record
    // callbacks' events go on the mail lane at their handlers. `step_pending_mail` steps the
    // pending time in the world frame's update, as `0x4ade60` does after the packets.
    "::ui_mail::MailOpen",
    "::ui_mail::pending::MailPending",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: read through the host by the
    // vendor's getters, and quiet. The list's handler triggers the Vendors tutorial on the
    // tutorials lane at its packet (`0x4fad32`), and the window's events fire on the merchant lane.
    "::ui_merchant::MerchantOpen",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: read, and quiet. A new pet's
    // `SMSG_PET_SPELLS` lowers it inside `SetPet` (`0x4bc8ce`), whose `PET_BAR_UPDATE` (`0x4bc90c`)
    // fires on the notice lane; `IsPetAttackActive` and the bar's getters read it through the host,
    // and the selection's release and `PetStopAttack` lower it in their calls. No feed fires off it:
    // the world entry's pet reset lowers it beside its own `PET_BAR_UPDATE`, which the reset's
    // change raises, not the latch.
    "::ui_pet::PetAttackLatch",
    // Quiet: the registrar's cell and the charter records, which the registrar's and the charter's
    // getters read through the host; the show, the close and the record callbacks fire on the
    // charter lane at their handlers.
    "::ui_petition::GuildRegistrarState",
    "::ui_petition::PetitionRecords",
    // Read through the host by the charter getters, and quiet: its events fire on the charter lane
    // at their handlers and the record and name callbacks.
    "::ui_petition::PetitionState",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: the questgiver getters read its
    // cells through the host, and no feed fires off it, as its panels' events, lines and item
    // callbacks go on the questgiver lane at their handlers. `query_statuses` re-asks the givers'
    // statuses off it after the dispatch, a send no script reads.
    "::ui_quest::QuestGiver",
    // Quiet: the getters' and the verbs' `"npc"` reads the latch through the host as it runs
    // (`0x515c00`), and the watchdog `watch_interaction_range` closes off it every frame, as
    // `0x493230` does from the world frame's update, after the packets. `feed_interact_npc`
    // copies it as `InteractNpc` right after the dispatch, for the frame's own readers: the
    // facing chain and the portrait booth.
    "::ui_session::latch::InteractLatch",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: the session verbs read and write
    // the logout bytes through the host in their calls, and quiet: the replies' handlers signal on
    // the notice lane at their packets.
    "::ui_logout::LogoutState",
    // Quiet: what a session verb posts past its call, the exit `ForceQuit` posts (`0x48ab60` →
    // `0x401560`), which `exit_on_logout_complete` takes in the drain, and the cinematic's stop
    // (`0x48b970` → `0x48f050`), which the cinematic's chain takes at its head and fades at frame
    // time; no feed fires off it.
    "::ui_logout::SessionPosts",
    // Quiet: the connection's state, which the session's handlers write and the death getters'
    // body test reads through the host; no feed fires off it.
    "::net::NetStatus",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: the death getters and
    // `GetCorpseMapPosition` read the stores through the host and the death verbs hold them in
    // their calls, and quiet: the offers and the
    // corpse-range signals packets raise go on the dialogs' lane at their handlers, the revive's
    // on the notice lane, and the corpse poll runs in the world frame's update, after the packets
    // (`0x4818c5`).
    "::death::DeathNet",
    // Read through the host by the taxi getters, and quiet: its events fire on the taxi lane at
    // their handlers.
    "::ui_taxi::TaxiState",
    // Read through the host by the stable getters, and quiet: its events fire on the stable lane
    // at their handlers and the creature callbacks.
    "::ui_stable::StableOpen",
    // Filled by the stable getters' lookups in their calls, and quiet: the creature answer's
    // handler spends it onto the stable lane.
    "::ui_stable::StableCreatureWaits",
    // Read through the host by the designer's getters, and quiet: its events fire on the tabard
    // lane at their handlers.
    "::ui_tabard::TabardOpen",
    // Read, and quiet: `CheckTalentMasterDist` reads the latch through the host and the Accept
    // holds it; the question's `CONFIRM_TALENT_WIPE` fires on the notice lane.
    "::ui_talent_wipe::TalentWipeState",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: read through the host by the
    // trainer's getters, and quiet. The list's open acknowledges its tutorial on the tutorials lane
    // at its packet (`0x4d74ae`), and the window's events fire on the trainer lane.
    "::ui_trainer::TrainerOpen",
    // Moved here from `NOT_YET_ON_A_LANE`, so neither list grew: the trade getters read it through
    // the host, and no feed fires off it, as its statuses' events go on the trade lane at their
    // handlers and the initiate's name callback.
    "::ui_trade::TradeSession",
    // Read through the host by the map setter's landmark sources (`0x4a6819`), and quiet: the
    // gossip handler's set re-selects the map on the landmark lane at its packet (`0x6dac93`), and
    // the clears at its deadline and on arrival run in the minimap's update (`0x6d9411`,
    // `0x6d9a4c`), past the frame's packets, on that lane.
    "::poi_marker::PoiMarker",
    // Read through the host by the mail getters' letter bodies, whose `$` tokens expand off it
    // (`0x506f70`), the readout's getters and the map setter's landmark sources, and quiet: its
    // packets' signals fire on the world-state lane, and the defense channel's flip signals
    // inside its notice (`0x49bdd6`).
    "::world_state::WorldStates",
];

/// The view pushes that read what a handler writes as the frame left it, not as each listener's
/// packet did: each system that writes the VM, in any schedule and any set, with the state the
/// handlers write that it reads itself, past the call host's own reads. The reference's getters read the live object
/// and caches at the listener's call (`UnitHealth` `0x5174d0`), and a handler runs whole before
/// the next (`0x537ae5`), so a listener reads its own packet's state; a push here hands a
/// listener inside the dispatch what the last push left, the frame before's. The list is the whole of that remaining work and only shrinks: a
/// feed goes once a log versions what it reads ([`crate::ui_unit::views`]), a new one fails, and
/// a row whose feed or inputs moved fails until it matches.
#[rustfmt::skip]
pub(super) const READS_FRAME_FINAL: &[(&str, &[&str])] = &[
];

/// The view pushes [`READS_FRAME_FINAL`]'s test finds whose build the reference too makes outside
/// every packet, held to their inputs the same way, with why: a hover's build that runs at a mouse
/// move (`0x7660d0`), the paint pass's repeat pick (`CSimpleTop::OnLayerUpdate` `0x765650`) or a
/// frame's own update in the frame pass, after the packets (`0x420d6a` after `0x420d55`), and whose
/// rebuilds inside a handler, if any, run there at their callbacks, so a listener fired inside the
/// dispatch reads what the last build left, as 1.12.1's does. They stay.
#[rustfmt::skip]
pub(super) const BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE: &[(&str, &[&str], &str)] = &[
    // With `widescreen`, which seats the cursor-anchored plate on the interface's screen.
    ("::ui_tooltip::drive_mouseover_tooltip", &["go_templates::GameObjectTemplates",
        "names::NameCache", "net::GuidIndex", "net::ObjectStore", "net::SelfGuid",
        "net::reputations::Reputations", "spell::group_relation::GroupRoster",
        "ui_guild::GuildState", "video::Widescreen"],
        "the WorldFrame's own update `0x481790` (its table's slot at `0x804420`), which picks and, \
         on a change of the mouseover alone (`0x482090` → `0x492e70` → `0x492890`), builds the \
         plate by kind; its rebuilds under the same hover run inside their packets: the unit's \
         (`0x4919d0`, from `0x491a00` and the flags watcher's `0x5ff7be`), the lock key's record \
         callback (`0x52ad10`) and the corpse owner's name callback (`0x52afe0`)"),
    ("::minimap::blips::drive_blip_tooltip", &["items::Countdowns", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore"],
        "the minimap's probe `0x4eb0c0`, its slot `0x3c`, which the hover pick calls at a mouse \
         move and at the paint pass's repeat pick (`0x7660d0`, from `0x765650`)"),
    ("::minimap::feed_minimap_arrows", &["death::DeathNet", "net::GuidIndex", "net::ObjectStore",
        "poi_marker::PoiMarker", "ui_party::GroupState"],
        "the arrow half of the minimap's render (`0x4ec950`), which aims them by bearing"),
    ("::player::camera::focus_on_look_edges", &["player::camera::CameraControl"],
        "the mouse half of the freelook edges the frame's mouse made, which the input pass runs \
         ahead of its tick (`0x51491f` → `0x493070`, `0x493090`); a call's edge runs it inside"),
    ("::spell::targeting::feed_targeting_to_input", &["items::Items", "names::NameCache",
        "net::GuidIndex", "net::ObjectStore", "net::UnitSpeeds", "net::motion::modes::UnitMoveModes",
        "net::motion::remote::RemoteMotion", "net::reputations::Reputations",
        "spell::group_relation::GroupRoster", "spell::mods::SpellModifiers",
        "spell::targeting::SpellTargeting", "target::Selection", "ui_party::GroupState",
        "ui_session::latch::InteractLatch"],
        "the word as the frame began, for the input records' reads with no host to ask; a call's \
         reads go live through the host (`SpellIsTargeting` `0x6e6cd0`)"),
    // The units' gate binds what it reads of a unit it gives a plate, and its in-packet runs and
    // the unit's callbacks write a bound plate's bar, colour, level, icon and name at their
    // packets; `widescreen` shapes the screen the seat solves on, the camera rig the freelook
    // edge, the selection the alpha and the targeting the hit-test veto (`0x7cba30`).
    ("::vplates::drive_vplates", &["Transform", "names::NameCache", "net::GuidIndex",
        "net::ObjectStore", "net::SelfGuid", "net::reputations::Reputations",
        "player::camera::CameraControl", "spell::group_relation::GroupRoster",
        "spell::targeting::SpellTargeting", "target::Selection", "ui_party::GroupState",
        "video::Widescreen", "vplates::VPlateMode"],
        "the world's render `0x483460` in the frame pass, after the packets, whose scene drain \
         ticks each unit (`0x4815d0` → `0x48160c`, slot 14 `0x607ed0`), which runs the gate \
         `0x60f600` at `0x607ef9`: a unit it gives a plate is bound (`0x6086e0` → `0x7cb6d0`), \
         one it drops released (`0x60f742`), a kept one's point stored and its level refreshed on \
         the latch (`0x60875f`); the render then seats them (`0x483647` → `0x608ce0`), and each \
         plate's `OnUpdate` sets its alpha (`0x7cb9f9`); inside the packets the bar's watchers \
         (`0x7cc570`), the health watcher's colour and flash (`0x60471b`, `0x604764`), the \
         reaction's colour (`0x606e3c`), the level watcher (`0x60ce5b`), the raid board's icons \
         (`0x4ba28a` → `0x608a90`), the logs' flashes (`0x62abac`, `0x62ce43`), the name \
         answers' releases (`0x5fb86e`, `0x6046c1`), `InvalidateNameDesc`'s gate (`0x60706a`) \
         and the unit's release (`0x5fbba9`) write the plate at their callbacks"),
    ("::ui_script::extract::tick_script", &["ui_script::UiScaleCvar", "video::Widescreen"],
        "the frame pass's `OnUpdate` walk (`0x764330` → `0x765650`), its screen and scale"),
    ("::ui_script::extract::paint_script", &["video::Widescreen"],
        "the UI pass's paint, after the walk, which no getter reads"),
    ("::ui_chat::channels::auto_join_zone_channels", &["net::ObjectStore",
        "ui_chat::edit::ChannelState"],
        "the zone text's update `0x494780`, whose walk (`0x494931`, gated on our taxi flag at \
         `0x494908`-`0x49491a`) the area poll runs in the idle pass after the packets \
         (`0x401ece` → `0x5db900` → `0x67e510` → `0x494780`), and `EndCinematic`'s (`0x48f1d0`), \
         in the frame pass; its runs inside a packet run there: the chat cache's load \
         (`0x499a22`) and the taxi flag's clear (`0x5ff56e`)"),
];

/// The systems that write the VM and push no view a getter reads of what a handler writes, by a
/// suffix of the system's name (beside [`NOT_YET_PLACED`]'s, whose firing is their work), with
/// what they do: an input's dispatch and the requests its calls queued, a timer or a poll that
/// signals in place, the CVar writes no call took, a session's end or the leave-world sweep's leg,
/// a save no script reads, a draw no getter reads, or the glue VM's frame, which no lane fires
/// into. The reference too does each outside every packet, so they stay; one that stops writing
/// the VM fails as stale.
#[rustfmt::skip]
pub(super) const WRITES_NO_VIEW: &[(&str, &str)] = &[
    ("::bindings::begin_dispatch", "the input pass's head: the binding calls Lua made, then the \
        frame's key records"),
    ("::bindings::end_dispatch", "the input pass's tail: the stuck keys' release"),
    ("::bindings::drain_binding_requests", "`SaveBindings`' files, written at the interface's \
        shutdown in the reference (`0x490bd0` → `0x4b65e0`)"),
    ("::cursor::drive_displayed_cursor", "the cursor FrameXML set, and the world frame's hover \
        (`0x481790`)"),
    ("::cvars::sync_cvars", "the CVar writes no call took into the mirror `GetCVar` reads (the \
        record's text, `0x488bf9`), and a new VM's seed: a packet's write reaches the mirror \
        inside its call, as `CVar::Set` runs inside its writer (`0x63df7d`), and every other is \
        made outside the packets (`every_cvar_write_reaches_the_mirror_before_a_packet`)"),
    ("::cvars::sync_input_cvars", "the same writes, ahead of the world's mouse and the frame's \
        packets"),
    ("::death::poll_corpse", "`CheckCorpseRange` (`0x492130`) every frame after the pick \
        (`0x4818c5`), signalling in place"),
    ("::player::camera::apply_late_look_calls", "the look calls the frame's packets made, for \
        the input tick (`0x514840`)"),
    ("::player::camera::latch_world_mouse", "the world's mouse buttons, the input's latch"),
    ("::sound::ui::drain_ui_sounds", "`PlaySound`'s queued plays"),
    ("::sound::zone::lua_music", "`PlayMusic` and `StopMusic`'s queued verbs"),
    ("::target::click::act_on_arrival", "the verb an approach owed, at its stop (`0x60fa20`)"),
    ("::target::click::act_on_interact", "a crate's `Interact`, the object's dispatcher"),
    ("::ui_action::drain::attack_nearest_probe", "the melee probe's press"),
    ("::ui_action::drain::drain_arrival_casts", "the cast a walk's arrival owed"),
    ("::ui_action::drain::drain_click_casts", "the casts a crate's `Interact` queued"),
    ("::ui_billing::poll_play_time", "the frame time's poll (`0x420d63`)"),
    ("::ui_chat::away::movement_clears_afk", "the movement keys' AFK clear (`0x513d36`, \
        `0x514e23`, `0x514f0b`, `0x514fca`)"),
    ("::ui_chat::channels::end_session_channels", "the session's end"),
    ("::ui_chat::channels::end_session_channels_on_disconnect", "the session's end at a dead \
        socket"),
    ("::ui_chat::idle::idle_handler", "the idle timer of `WorldFrame::Render` (`0x482ea0`)"),
    ("::ui_chat::input::drain_chat_input", "the chat line's input"),
    ("::ui_chat::settings::save_chat_looks", "the chat looks' save"),
    ("::ui_dialog_verbs::drain_meeting_stone_joins", "the meeting stone's use, at its click"),
    ("::ui_dialog_verbs::poll_area_spirit_healer", "the healer scan every frame (`0x4818ca` → \
        `0x4923b0`), signalling in place"),
    ("::ui_dressup::compose_dressup", "the booth's frame: the room emptied with no pane shown, \
        then the pane's look, a draw"),
    ("::ui_items::feed::feed_item_stats", "the item records that land or go outside every \
        packet, a cache file's at the character's pick ahead of the world entry's load and another \
        realm's forget, carried into the VM's copy, and the reads `GetItemInfo` and the link verbs \
        made of a record the copy lacked (`0x48e172`); a VM no load seated is seated whole. A new \
        interface's copy is seated at its load, an answer's record reaches it at its packet ahead \
        of its callbacks (`0x55bdb0`), and a getter's miss asks inside its call (`0x55ba30`)"),
    ("::ui_logout::leave_world", "the world load's first call (`0x401bd4`)"),
    ("::ui_loot::close_loot_on_leave_world", "the sweep's loot leg (`0x490ab2`)"),
    ("::ui_macro::save_dirty_macros", "the macros' save"),
    ("::ui_mail::close_mail_on_leave_world", "the sweep's mail leg (`0x490b1b`)"),
    ("::ui_models::sync_tiles", "the model panes' draw"),
    ("::ui_script::glue_mgr::drive_glue_mgr", "the glue VM's `CGlueMgr::Idle` (`0x46b930`), \
        under the glue screens only"),
    ("::ui_script::movie::upload_movie_pictures", "the movie's pictures, a draw"),
    ("::ui_world_map::dev_map_jump", "a dev instrument's click"),
    ("::video::drain_restart_gx", "the `RestartGx` asks no call took"),
];

/// The resources the handlers fill through a shared reference, which their declared access shows
/// as a read: the account data a download stores and what a session verb posts. One the call host
/// holds too is every hosted feed's read, so it is host-held, whichever handler fills it: a
/// [`Row::HostHeld`] row or a [`HOST_HELD_UNREAD`] entry. The lanes (the tutorials' and the ui errors' among them) and the host's two other sinks (the
/// message sounds and the unit edits, which raise no event) are not queues here. Each is named
/// by its path in the crate and matched whole ([`filled_through_a_read`]).
pub(super) const FILLED_THROUGH_A_READ: &[(&str, Fill)] = &[
    // The account-data sections, which the download's handler stores through `&self`.
    ("::account_data::AccountData", Fill::State),
    // The far-sight machine the update handler starts and ends (`0x5de0d0`, `0x5fbb1a`,
    // `0x5d552d`), the clear's handler ends (`0x5e441d`), the teardowns free and the call host's
    // buff cancel runs (`0x6e70ad`).
    ("::player::view_subject::FarSight", Fill::State),
    // What a session verb posts, which the logout's and the cinematic's drains take.
    ("::ui_logout::SessionPosts", Fill::Sink),
    // The gestures a chat line or a world click's greeting plays, which `drive_gestures` takes.
    ("::creature_anim::gesture::GestureQueue", Fill::Sink),
];

/// What a resource [`FILLED_THROUGH_A_READ`] holds is to the systems that reach it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Fill {
    /// State its users read and write through `&self`: a read of it is a read.
    State,
    /// A sink its users push onto and a system of its own takes: a lane firing's access to it is
    /// its own push, never a read of what a handler left.
    Sink,
}

/// Whether `n`, a type's full name, is a [`FILLED_THROUGH_A_READ`] entry, matched whole.
pub(super) fn filled_through_a_read(n: &str) -> Option<Fill> {
    FILLED_THROUGH_A_READ
        .iter()
        .find(|(e, _)| names_exactly(n, e))
        .map(|&(_, fill)| fill)
}

/// Every other resource a handler reads and does not write: state, tables and caches it
/// consults, none of them a queue a feed takes. A system between the dispatch and the drain that
/// writes one has its row in `host_read_tests`'s lists, as a feed that rewrites what a handler
/// reads would hand it the frame before's. A new one fails until it is placed here or in
/// [`FILLED_THROUGH_A_READ`]. The list is kept by hand: an entry a handler in fact fills through a
/// shared reference leaves that queue out of [`handler_queues`], so a feed that takes it and fires
/// in system order, or a system that pushes onto a lane off it at its own place, passes every
/// check here unseen. Each is named by its full path, the crate's own by its path in the crate,
/// and matched whole ([`names_exactly`]).
pub(super) const READ_ONLY_TO_HANDLERS: &[&str] = &[
    // `AreaPOI.dbc`, the landmark rows the gossip marker's re-select of the map reads
    // (`0x6dac93` → `0x4a67a0`, its walk at `0x4a6819`).
    "::area_poi::AreaPoiRes",
    // The chat log files: the logging verbs open and close them through `&self` inside their
    // call (`0x49fea2`), a file no handler reads back; the chat tee only appends to them.
    "::ui_chat::logging::ChatLogFiles",
    // The gossip marker, which the map setter's landmark sources read through the host
    // (`0x4a6819`); `SMSG_GOSSIP_POI`'s handler and the minimap's clears write it through their
    // own `ResMut` (`0x6dac10`, `0x6d9411`, `0x6d9a4c`).
    "::poi_marker::PoiMarker",
    // The language word pools, which a page's show rewrites it by (`0x4e3856`), loaded once at
    // startup; the show reads the character's skill live (`0x4e3837` → `GetIndexedSpellSkill`).
    "::ui_chat::language::ChatLanguages",
    // The usable stationery list, whose first paper the list parse gives a row with none
    // (`0x4ad565`); the paper callback writes it in the item answer's list, through its own
    // `ResMut`.
    "::ui_mail::stationery::UsableStationery",
    // `ItemBagFamily.dbc`, which the inventory refusal's handler names a bag's family by
    // (`0x5ede00`).
    "::ui_items::ItemBagFamilies",
    // `AuctionHouse.dbc`, whose deposit rate the hello's show hands the VM, and which the host's
    // mail getters read a row off.
    "::ui_auction::AuctionHouses",
    // The corpse-range latch, which the reclaim delay's handler tests to signal again
    // (`0x4962ec`); the corpse query's not-found handler and the corpse poll write it through
    // their own `ResMut`.
    "::death::DeathLatches",
    // The connection's state, which the death getters' body test reads through the host; the
    // socket's handlers write it through their own `ResMut`.
    "::net::NetStatus",
    // The login pick, which names the character the account-data sections belong to.
    "::glue_session::roster::Roster",
    // `0xb4e314`, which the party snapshot reads; only the zone feed commits it.
    "::area::CommittedZone",
    // The walk's committed zone, which the recruitment cascade's plan reads through the host
    // (`0x49ead7`), and the slots; the walk's poll and the notice lane's taxi walk write them
    // through their own `ResMut`. Both sit here only because the scan cannot see an exclusive
    // handler's writes: the chat cache's read writes both inside its section's settle, in its
    // packet (`0x499a22`).
    "::ui_chat::channels::ZoneChannelWalk",
    "::ui_chat::edit::ChannelState",
    // The console's registry, which a `ConsoleExec` a listener makes reads.
    "::console::ConsoleCommands",
    // The creature families the cast send's pet learn rung reads (`0x60c7c0`).
    "::ui_pet_stats::PetFamilyTables",
    // The personality and loyalty names and the slot prices the host's stat and stable getters read.
    "::ui_pet_stats::PetStatTables",
    // The studied-creature block's tables, which a unit's state read through the host builds its
    // tooltip lines from.
    "::ui_unit::tooltip_facts::StudyTables",
    // The area names a trainer description's `$z` reads through the host.
    "::ui_quest_log::QuestHeaderNamesRes",
    "::net::HomeBind",
    "::ui_trainer::TrainerOpen",
    "::ui_stable::StableSlotPrices",
    "::area::AreaTableRes",
    // The server clock the quest log getters read in the host.
    "::net::ServerWallClock",
    // The page materials the questgiver background getter reads in the host (`0x502230`).
    "::ui_item_text::PageMaterials",
    // The world map's projections the battleground position getters read in the host, and the
    // stat columns' table the status handler reads (`0x4aa9c3`).
    "::ui_world_map::WorldMapUiData",
    "::world_state_ui::WorldStateUiRes",
    "::combat_text::law::DamageTextGates",
    "::creature_anim::env_damage::EnvDamageTable",
    "::creature_anim::spell_visual::SpellVisuals",
    "::entities::Creatures",
    "::entities::equipment::ItemDisplays",
    // `ItemClass.dbc`, whose names the host's `GetItemInfo` reads.
    "::ui_items::ItemClasses",
    "::go_templates::GameObjectTemplates",
    "::items::Enchants",
    "::items::Items",
    "::items::RandomProperties",
    "::chr_classes::ChrClassTable",
    "::chr_classes::ChrRaceTable",
    "::chr_classes::RacePvpTeams",
    "::names::NameCache",
    // The Video menu's list the host's multisample verbs read (`0x48c3e0`).
    "benilla_world::view::MsaaFormats",
    // A capture run, which parks the camera the view verbs would move.
    "::run_mode::CaptureMode",
    "::net::game_time::ServerTime",
    "::net::GuidIndex",
    // The bag cache the open-container handler numbers its `BAG_OPEN`s from.
    "::item_watch::BagCache",
    "::net::NetCommands",
    "::net::SelfGuid",
    "::player::state::OwnControlLost",
    "::player::state::Player",
    // The request ladder's tests, which its status reads (`0x4bf793`, `0x4bf7a5`, `0x4bf7bc`).
    "::cinematic::Cinematic",
    "::ui_auction::AuctionOpen",
    "::ui_trade::BlockTrades",
    // Who we drive, which the loot response reads; our own create's arrival writes it.
    "::player::state::MoverLock",
    // The active mover, which the handlers that end an auto-move test (`0x6103a0`); our own
    // create's arrival claims it (`0x4958a0`).
    "::net::MoverClaim",
    "::player::state::StandPending",
    // The page cache the host's page turns read.
    "::ui_item_text::PageTexts",
    // The streamed terrain the host's minimap ground cast traces.
    "benilla_world::terrain_stream::TerrainStreamer",
    "bevy_asset::assets::Assets<benilla_assets::adt::AdtTile>",
    // The pet trainer's latch, which the host's `ConfirmPetUnlearn` reads.
    "::ui_dialog_verbs::PetUnlearnState",
    "::player::world_focus::BarUnitOutdoors",
    "::player::world_focus::PlayerOutdoors",
    "::sound::emote::EmoteSounds",
    "::sound::zone::ExplorationSounds",
    "::spell::cast_target::AutoSelfCast",
    "::spell::group_relation::GroupRoster",
    "::spell::inflight::ActiveChannel",
    "::spell::mods::SpellModifiers",
    "::target::AssistAttack",
    "::target::Hovered",
    "::target::HoveredObject",
    "::target::ring::Factions",
    "::target::scan::CreatureTypes",
    "::target::scan::LastEnemy",
    "::target::Selection",
    "::text_filter::TextFilter",
    "::text_filter::TextFilterSwitches",
    "::ui_action::PlayerActions",
    "::ui_action::SpellMechanics",
    "::ui_action::Spells",
    "::ui_chat::combat::CombatLogRanges",
    "::ui_chat::combat::LogPeriodicSpells",
    "::ui_guild::GuildMemberNotify",
    "::ui_petition::PetitionRecords",
    "::ui_items::ItemSubClasses",
    // `showLootSpam`, which the roll handlers read as their lines compose (`0x61ba3a`,
    // `0x61bafe`, `0x61c0b9`); a listener's `SetCVar` writes it inside its call.
    "::ui_loot::LootConfig",
    "::ui_loot::LootKneel",
    "::ui_mail::MailOpen",
    "::ui_mail::Stationery",
    // The tables and the world states the host's mail getters read a row off (`0x4aee11`,
    // `0x4af229`, `0x4aec87`, the body's `$` expansion `0x506f70`).
    "::ui_mail::MailTemplates",
    "::ui_mail::Packages",
    "::world_state::WorldStates",
    // The repair discounts and the taxi graph the host's merchant and taxi verbs consult.
    "::ui_merchant::RepairTables",
    "::ui_taxi::routing::TaxiCatalogs",
    // The bank's prices and the dialogs' states the host's bank, battleground, death, duel,
    // innkeeper and summon verbs consult.
    "::ui_bank::BankPrices",
    "::ui_battlefield::Battlefield",
    "benilla_assets::MapCatalogRes",
    "::ui_binder::BinderState",
    "::ui_dialog_verbs::BattlefieldQueue",
    "::ui_duel::DuelState",
    "::ui_summon::SummonState",
    // The dialogs' state the call host's getters read (`0x48b620`, `0x4ca570`, `0x495c90`).
    "::ui_dialog_verbs::InstanceBoot",
    "::ui_dialog_verbs::MeetingStone",
    "::ui_instance::InstanceState",
    "::ui_script::UiClock",
    "::ui_spellbook::SkillLines",
    // The talent catalog, which the host's talent getters read.
    "::ui_talent::Talents",
    "::ui_tradeskill::SpellFocus",
    "benilla_world::frame_pace::FrameStep",
    "benilla_world::world_map::CurrentMap",
    "bevy_time::time::Time",
    "bevy_time::time::Time<bevy_time::real::Real>",
    // The stores as of the drain's place, which the call host's token lookups read.
    "::ui_unit::views::UnitViews",
    // `Languages.dbc` and the race's base languages, loaded once, which the language getters
    // read through the host.
    "::ui_unit::LanguagesRes",
    "::ui_unit::DefaultLanguagesRes",
    // The area under the player, which the host's raid rows read for our own zone (`0xb4e314`).
    "benilla_world::terrain_stream::queries::CurrentArea",
    // The interface clock's pair as the last tick left it, which the call host's buff timers
    // map each duration stamp through (`0x4e4450`); only the tick's copy and a new VM's seed
    // write it.
    "::ui_script::HostClock",
    // The duration packet's stamps (`0x4e4390`), which the call host's buff timers read at the
    // call (`0x4e4450`); a hosted handler reads them, the packet's handler writes them.
    "::ui_aura::AuraDurations",
    // The host's two sinks that raise no event.
    "::sound::message::MessageSounds",
    "::creature_anim::edits::UnitEdits",
    // The proficiency masks the call host's requirement getters read (`0x5ea930`).
    "::net::Proficiencies",
    // `ItemSet.dbc` and the item-name cache the call host's set block reads (`0x52df5a`).
    "::ui_items::ItemSets",
    "::item_names::ItemNames",
    // What the world click's legs read in the call (`0x481f60`): the lock tables an object's
    // open gates on, the cover and the inspector that refuse an arm, the frame's motion its drag
    // test reads, and Shift for `SetAutoLoot`.
    "::go_templates::LockTypes",
    "::go_templates::Locks",
    // The gathering skills the world click's cursor forks on, read through the host.
    "::ui_action::LearnedAbilities",
    "::loading_screen::LoadingScreen",
    "::ui_script::InspectMode",
    "bevy_input::button_input::ButtonInput<bevy_input::keyboard::KeyCode>",
    "bevy_input::mouse::AccumulatedMouseMotion",
    // The frame a quest-share line starts its bounded wait for a name at.
    "bevy_diagnostic::frame_count_diagnostics_plugin::FrameCount",
    // The frame gate's spatial verdicts, which the gate a packet runs reads through the host
    // (`0x60706a` → `0x60f600`); only the frame's gate writes them, in the world's render.
    "::vplates::PlateSpots",
];

/// Every lane's queue, by type: what a handler fills there fires in the packets' order.
pub(super) fn lane_types() -> [TypeId; 42] {
    [
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
        // The field callbacks' lane: the signals each callback's legs parked, stamped under its
        // edge, and the refused records' walks gone on behind their signals.
        TypeId::of::<crate::field_callbacks::FieldCallbacks>(),
        // The release lane: the signals each selection teardown parked, stamped under its release.
        TypeId::of::<crate::target::Releases>(),
        TypeId::of::<crate::target::OwedSelects>(),
        // The item-text lane: the page-text packets' reads, stamped at their packets.
        TypeId::of::<crate::ui_item_text::ItemTextLane>(),
        // The rolls' lane: the roll packets' and their lookups' callbacks' signals and lines.
        TypeId::of::<crate::ui_loot_roll::LootRolls>(),
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
        // The battleground lane: the list, status, log and name-answer packets' signals.
        TypeId::of::<crate::ui_battlefield::BattlefieldLane>(),
        // The social lane: the friend and ignore list events, their lines and the `/who`
        // answers, at their packets, and the lists it seats around each.
        TypeId::of::<crate::ui_social::SocialLane>(),
        TypeId::of::<crate::ui_social::SocialState>(),
        // The merchant lane: the vendor packets' and record callbacks' signals and lines.
        TypeId::of::<crate::ui_merchant::MerchantLane>(),
        // The taxi lane: the map's open and the flight master's replies.
        TypeId::of::<crate::ui_taxi::TaxiLane>(),
        // The world-state lane: each packet's map and readout signals.
        TypeId::of::<crate::world_state::WorldStateLane>(),
        TypeId::of::<crate::ui_world_map::LandmarkLane>(),
        // The dialogs' lane: the service questions, the boot, the stone, the summon, the
        // lockouts, the duel and the ticket answers, at their packets.
        TypeId::of::<crate::ui_dialog_verbs::DialogLane>(),
        // The tabard lane: the designer's open and its save reply's line and signal.
        TypeId::of::<crate::ui_tabard::TabardLane>(),
        // The stable lane: the list's show, the result's line and the creature callbacks' signals.
        TypeId::of::<crate::ui_stable::StableLane>(),
        // The charter lane: the registrar's and the petition window's signals and lines.
        TypeId::of::<crate::ui_petition::PetitionLane>(),
        // The trainer lane: the list's and the state passes' pushes and signals.
        TypeId::of::<crate::ui_trainer::TrainerLane>(),
        // The bank lane: the open's signal and the refusals' lines.
        TypeId::of::<crate::ui_bank::BankLane>(),
        // The gossip lane: the greeting's draw.
        TypeId::of::<crate::ui_gossip::GossipLane>(),
        // The questgiver lane: the panels' events, the lines and the item callbacks' signals.
        TypeId::of::<crate::ui_quest::QuestGiverLane>(),
        // The trade lane: the statuses' signals and lines and the initiate's name callback.
        TypeId::of::<crate::ui_trade::TradeLane>(),
        // The mail lane: the list's and the results' signals and lines, the cores' halves, the
        // record callbacks' and the pending-mail time's.
        TypeId::of::<crate::ui_mail::MailLane>(),
        // Our player's arrival, stamped at its create.
        TypeId::of::<crate::ui_world_enter::ArrivalLane>(),
        // The tutorials lane: each site's trigger, acknowledge and selection, and the flags'
        // copy, stamped where it runs.
        TypeId::of::<crate::tutorial::Tutorials>(),
        // The ui-error lane: each refusal and notice, stamped where its handler or verb shows it.
        TypeId::of::<crate::ui_action::UiErrorKeys>(),
    ]
}

/// Whether `id` names a `Messages<T>` buffer, which a reader takes from through a read.
pub(super) fn is_messages(world: &World, id: ComponentId) -> bool {
    name(world, id).contains("Messages<")
}

pub(super) fn name(world: &World, id: ComponentId) -> String {
    world
        .components()
        .get_name(id)
        .map_or_else(|| "?".to_string(), |n| n.to_string())
}

pub(super) fn ends_with_any(name: &str, rows: &[&str]) -> bool {
    rows.iter().any(|s| name.ends_with(s))
}

/// The queues the handlers fill, by id: each resource one writes, past the pure caches, each
/// resource in [`FILLED_THROUGH_A_READ`] one reads, and, when one can queue a unit edit, what the
/// edits write as they apply after it ([`applied_by_the_unit_edits`]).
fn handler_queues(
    world: &World,
    caches: &std::collections::HashSet<ComponentId>,
) -> Vec<ComponentId> {
    let handlers = world.resource::<crate::net::handlers::NetHandlers>();
    let edits = world
        .components()
        .get_resource_id(TypeId::of::<crate::creature_anim::UnitEdits>());
    let mut out = BTreeSet::new();
    for (_, access) in handlers.accesses() {
        let access = access.combined_access();
        out.extend(access.resource_writes().filter(|id| !caches.contains(id)));
        out.extend(
            access
                .resource_reads()
                .filter(|id| filled_through_a_read(&name(world, *id)).is_some()),
        );
        if edits.is_some_and(|e| access.has_resource_read(e)) {
            out.extend(applied_by_the_unit_edits(world));
        }
    }
    out.into_iter().collect()
}

/// What the unit edits write as they apply ([`crate::creature_anim::applied_resources`]): a
/// handler's edits apply after it in the net drain, by an exclusive system whose writes Bevy's
/// access does not show, so each is a queue that handler fills.
fn applied_by_the_unit_edits(world: &World) -> Vec<ComponentId> {
    crate::creature_anim::applied_resources()
        .into_iter()
        .filter_map(|t| world.components().get_resource_id(t))
        .collect()
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
    let read_only = |n: &str| READ_ONLY_TO_HANDLERS.iter().any(|e| names_exactly(n, e));
    let unknown: Vec<&String> = reads
        .iter()
        .filter(|n| filled_through_a_read(n).is_none() && !read_only(n))
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
    let mut stale: Vec<&str> = READ_ONLY_TO_HANDLERS
        .iter()
        .filter(|e| !reads.iter().any(|n| names_exactly(n, e)))
        .copied()
        .collect();
    stale.extend(
        FILLED_THROUGH_A_READ
            .iter()
            .map(|&(s, _)| s)
            .filter(|s| !reads.iter().any(|n| names_exactly(n, s))),
    );
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}

/// Whether `n`, a type's full name, is the type `entry` names: the crate's own by its path in the
/// crate (`::items::Items`), any other by its full name. Never a suffix, so a new type that shares
/// a listed one's short name is not taken for it.
fn names_exactly(n: &str, entry: &str) -> bool {
    n == entry || n.strip_prefix("benilla_app") == Some(entry)
}

/// The frame's drain, of what the feeds and the frame's systems push after the dispatch, runs once
/// a frame after every feed and before the tick, gated as the feeds of the in-game interface
/// are, and holds the lanes; the dispatch, whose flushes fire each packet's lanes, runs ahead of
/// every feed, so no feed's push lands between two packets.
#[test]
fn the_frames_drain_runs_after_the_feeds_and_before_the_tick() {
    let mut app = headless_client();
    let mut lanes = app.world().resource::<super::Lanes>().names();
    lanes.sort_unstable();
    assert_eq!(
        lanes,
        [
            "arrival",
            "auction house",
            "bank",
            "battlefield",
            "binding arrivals",
            "cast reports",
            "cast views",
            "charter",
            "chat",
            "chat setup arrivals",
            "cooldowns",
            "dialogs",
            "equip errors",
            "field callbacks",
            "field notices",
            "gossip",
            "guild",
            "item text",
            "item watchers",
            "landmarks",
            "layout arrivals",
            "list rebuilds",
            "loot",
            "loot rolls",
            "macro arrivals",
            "mail",
            "merchant",
            "owed selects",
            "party",
            "pet cooldowns",
            "quest events",
            "quest giver",
            "releases",
            "reputation",
            "social",
            "stable",
            "tabard",
            "taxi",
            "trade",
            "trainer",
            "tutorials",
            "ui errors",
            "unit views",
            "world states"
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
    let (dispatch, _) = c
        .systems
        .iter()
        .find(|(_, s)| s.name.ends_with("::net::apply::apply_net_updates"))
        .expect("the dispatch is in Update");
    let mut next: HashMap<_, Vec<_>> = HashMap::new();
    for (a, b) in &c.dependencies {
        next.entry(*a).or_default().push(*b);
    }
    let behind = |start| {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![start];
        while let Some(n) = stack.pop() {
            for m in next.get(&n).into_iter().flatten() {
                if seen.insert(*m) {
                    stack.push(*m);
                }
            }
        }
        seen
    };
    let after_dispatch = behind(*dispatch);
    let early: Vec<&str> = c
        .systems
        .iter()
        .filter(|(k, s)| s.sets.iter().any(|set| set == "UiFeed") && !after_dispatch.contains(*k))
        .map(|(_, s)| s.name.as_str())
        .collect();
    assert!(
        early.is_empty(),
        "feeds not ordered behind the dispatch: {early:?}"
    );
    let tick = c
        .systems
        .iter()
        .find(|(_, s)| s.name.ends_with("::extract::tick_script"))
        .map(|(k, _)| *k)
        .expect("the tick");
    assert!(
        behind(*drain).contains(&tick),
        "the drain runs before the tick"
    );
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
                .filter(|&&id| host.has_resource_read(id))
                // A queue filled through a read is every handler's that holds the host, whichever
                // fills it, so each the host reads is held; a written one is held once a handler
                // apart from the host writes it.
                .filter(|&&id| {
                    filled_apart.contains(&id)
                        || filled_through_a_read(&self::name(world, id)).is_some()
                })
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
                    let through_a_read = filled_through_a_read(&n).is_some();
                    let takes = if through_a_read {
                        // One the host holds too is every hosted feed's read: a host-held queue.
                        access.has_resource_read(id) && !host.has_resource_read(id)
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
/// behind every packet of the frame, pass [`every_lane_push_off_a_handler_queue_keeps_its_place`];
/// a reader that pushes after the dispatch fails
/// [`no_lane_push_off_a_placed_queue_follows_the_dispatch`].
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
        "world entry only arms the zone-channel walk, whose one push is a walk's refusal onto \
         the ui-error lane, made where the zone text's update walks, after the packets \
         (`0x401ece` → `0x494931`)",
    ),
    (
        "::player::controller::control",
        "the mover's queue sends the root and unroot acks at its frame update (`0x616438`, \
         `0x6164a9`), and their input tick stops an auto-move there (`0x60e33d` → `0x60e347`)",
    ),
    (
        "::player::press::press_tick",
        "the window deactivate runs the input tick outside every call (`0x514490` → \
         `0x5144b5`), its direction and pitch bits cleared (`0x5144a3`), whose pitch stop ends \
         the auto-move there (`0x60ddda` → `0x60e990`)",
    ),
    (
        "::ui_chat::channels::end_session_channels_on_disconnect",
        "a dead socket ends the channel session, and no packet follows it",
    ),
];

/// The systems outside the phase that lend the call host and take a handler queue, firing off it
/// in system order: remaining work, which only shrinks, by a suffix of the system's name.
pub(super) const NOT_YET_PLACED: &[(&str, &str)] = &[(
    "::ui_loot::drain_loot",
    "the loot closes a precast end owes (`0x60d040` → `EndLootWindow` `0x48f200` at `0x60d10b`), \
     which the reference makes inside the packet that ends it: `HandleSpellGo` (`0x6e7fc5`), the \
     spell failures (`0x6e8dd7`, `0x6e8e8c`), the health watcher (`0x6047e7`) and \
     `OnMonsterMove` (`0x6020e8`); the unit edits owe them after the handler and the drain \
     closes them in its own phase, so `LOOT_CLOSED` fires behind the frame's packets",
)];

/// The placed queues' readers that defer work to their commands and push nothing onto a lane
/// from them, by a suffix of the system's name, with what the commands do.
const DEFERS_NO_LANE_ITEM: &[(&str, &str)] = &[
    (
        "::creature_anim::spell_visual::arm_aura_state_fx",
        "removes and inserts the aura state kit's pins, which no script reads",
    ),
    (
        "::go_anim::drive_go_collision",
        "turns an object's collider off and on, which no script reads",
    ),
];

/// What [`lane_pushes_off_handler_queues`] finds, each a system's name with its line.
struct LanePushes {
    /// Off a queue whose items carry no place: pushed at the system's own place.
    unplaced: BTreeSet<(String, String)>,
    /// Off a [`placed_queues`] queue: pushed at the item's place, after the dispatch.
    placed: BTreeSet<(String, String)>,
    /// A [`placed_queues`] queue's readers that push no lane themselves and defer work to their
    /// commands, which may.
    deferring: BTreeSet<(String, String)>,
}

/// Every system of `Update` that takes a handler queue and pushes onto a lane; `None` in a build
/// that carries no type names.
fn lane_pushes_off_handler_queues() -> Option<LanePushes> {
    let mut app = headless_client();
    let caches = census(&mut headless_client(), Update, SyncPoints::Declared)
        .classes
        .caches;
    let queues = handler_queues(app.world(), &caches);
    let (pushes, placed_pushes, deferring, systems) =
        app.world_mut().schedule_scope(Update, |world, schedule| {
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
            let applied = applied_by_the_unit_edits(world);
            let graph = schedule.graph();
            let mut pushes = BTreeSet::new();
            let mut placed_pushes = BTreeSet::new();
            let mut deferring = BTreeSet::new();
            let mut systems = BTreeSet::new();
            for (key, system, _) in graph.systems.iter() {
                systems.insert(system.name().to_string());
                let Some(with_access) = graph.systems.get(key) else {
                    continue;
                };
                let access = with_access.access.combined_access();
                let fires = vm.is_some_and(|vm| access.has_resource_write(vm));
                if system.is_exclusive() {
                    continue;
                }
                let name = system.name().to_string();
                let pushes_a_lane = lanes.iter().any(|&l| pushes_onto(access, l));
                // A placed queue's reader pushes at the item's place wherever it runs, so every one
                // that pushes counts, a feed of the phase among them; one that defers its work to
                // its commands may push from them, which its access does not show.
                for &q in &placed {
                    if access.has_resource_read(q)
                        && !access.has_resource_write(q)
                        && !host.has_resource_read(q)
                    {
                        let n = self::name(world, q);
                        if pushes_a_lane {
                            placed_pushes.insert((name.clone(), format!("{name} takes {n}")));
                        } else if system.has_deferred() {
                            deferring.insert((name.clone(), format!("{name} defers off {n}")));
                        }
                    }
                }
                if !pushes_a_lane {
                    continue;
                }
                // A feed of the phase that fires is the other guard's, and an exclusive one is its
                // row; a system that lends the call host outside the phase is judged here, by the
                // queues it reads: a plain resource it writes is its verbs' own state.
                if fires && in_feed(graph, bevy::ecs::schedule::NodeId::System(key)) {
                    continue;
                }
                let lends = host.is_subset_resources(access);
                for &q in &queues {
                    if Some(q) == vm || lanes.contains(&q) || placed.contains(&q) {
                        continue;
                    }
                    let n = self::name(world, q);
                    // The host's own state is what a call touches, never a queue taken; what the
                    // unit edits owe is a queue whoever writes it takes, as no verb holds it.
                    let takes = if filled_through_a_read(&n).is_some() {
                        access.has_resource_read(q) && !host.has_resource_read(q)
                    } else if applied.contains(&q) && !is_messages(world, q) {
                        access.has_resource_write(q) && !host.has_resource_write(q)
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
            (pushes, placed_pushes, deferring, systems)
        });
    systems
        .iter()
        .any(|n| n.contains("benilla_app::"))
        .then_some(LanePushes {
            unplaced: pushes,
            placed: placed_pushes,
            deferring,
        })
}

/// A system of any phase that takes a handler queue and pushes onto a lane stamps the push where
/// it runs, behind every packet of the frame, unless the queue's items carry their place
/// ([`placed_queues`]). Each such system needs a row in [`AT_THEIR_OWN_PLACE`] or
/// [`NOT_YET_PLACED`].
#[test]
fn every_lane_push_off_a_handler_queue_keeps_its_place() {
    let Some(LanePushes {
        unplaced: pushes, ..
    }) = lane_pushes_off_handler_queues()
    else {
        eprintln!("skipped: this build carries no type names");
        return;
    };
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

/// No system takes a placed queue ([`placed_queues`]) after the dispatch and pushes onto a lane,
/// at the item's place or, from its commands, at its own, found as
/// [`every_lane_push_off_a_handler_queue_keeps_its_place`] finds pushes, without its exemption of
/// the placed queues: the dispatch fires each packet's lanes after its handlers, so an item
/// stamped at an earlier packet and pushed after the dispatch fires behind every later packet's.
/// The reference runs each such consumer inside the update packet's notify pass (`0x465277` →
/// `0x465330`), as [`crate::field_callbacks`] runs the field edges' callbacks at the packet's end.
#[test]
fn no_lane_push_off_a_placed_queue_follows_the_dispatch() {
    let Some(LanePushes {
        mut placed,
        deferring,
        ..
    }) = lane_pushes_off_handler_queues()
    else {
        eprintln!("skipped: this build carries no type names");
        return;
    };
    let quiet: Vec<&str> = DEFERS_NO_LANE_ITEM.iter().map(|(s, _)| *s).collect();
    let gone: Vec<&str> = quiet
        .iter()
        .copied()
        .filter(|s| !deferring.iter().any(|(system, _)| system.ends_with(s)))
        .collect();
    assert!(gone.is_empty(), "stale DEFERS_NO_LANE_ITEM rows: {gone:?}");
    placed.extend(
        deferring
            .into_iter()
            .filter(|(system, _)| !ends_with_any(system, &quiet)),
    );
    let lines: Vec<&str> = placed.iter().map(|(_, line)| line.as_str()).collect();
    assert!(
        lines.is_empty(),
        "these push onto a lane off a placed queue after the dispatch, or defer to commands that \
         may: push from the handler's notify step instead, or give the system a row in \
         DEFERS_NO_LANE_ITEM when its commands push none:\n  {}",
        lines.join("\n  ")
    );
}

/// Every exclusive firing declares its reads ([`super::LaneReads`]), and only an exclusive one:
/// Bevy reports an exclusive system's access as empty, and the drain's measured access
/// ([`super::Lanes::accesses`]) stands an exclusive firing's declared reads in for it.
#[test]
fn every_exclusive_firing_declares_its_reads() {
    let app = headless_client();
    let lanes = app.world().resource::<super::Lanes>();
    let wrong: Vec<String> = lanes
        .names()
        .into_iter()
        .zip(lanes.exclusive_reads())
        .filter_map(
            |(lane, (exclusive, declared))| match (exclusive, declared) {
                (true, false) => Some(format!("{lane:?} is exclusive and declares no reads")),
                (false, true) => Some(format!(
                    "{lane:?} declares reads, but its firing is not exclusive"
                )),
                _ => None,
            },
        )
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
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
    "target::scan::OwedSelects",
];

/// A type's name as [`READS_FRAME_FINAL`] spells it: the crate's own by its path in the crate, an
/// engine type by its own name.
pub(super) fn short_name(full: &str) -> String {
    match full.strip_prefix("benilla_app::") {
        Some(rest) => rest.to_string(),
        None => full.rsplit("::").next().unwrap_or(full).to_string(),
    }
}

/// What the handlers write, as [`READS_FRAME_FINAL`] counts it: each component a handler writes,
/// and each resource one writes or fills through a shared reference ([`FILLED_THROUGH_A_READ`]),
/// whose access shows a read, so a feed that reads such a queue is held to it.
fn handler_written(world: &World) -> (BTreeSet<ComponentId>, BTreeSet<ComponentId>) {
    use bevy::ecs::query::ComponentAccessKind;
    let handlers = world.resource::<crate::net::handlers::NetHandlers>();
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
        resources.extend(
            access
                .resource_reads()
                .filter(|id| filled_through_a_read(&name(world, *id)).is_some()),
        );
    }
    (components, resources)
}

/// Every view push that reads what a handler writes has its row, whose inputs are exactly the
/// handler-written state it reads: each component a handler writes and each resource one writes,
/// past the VM, the message buffers and the lane queues. A push is any system that writes the VM,
/// in any schedule and any set, the feeds of the `UiFeed` phase and those after the tick alike.
/// Its own reads are its declared access less the call host's, entry by entry, as every system
/// that lends the host holds all of the host's.
#[test]
fn every_view_push_of_what_a_handler_writes_is_listed() {
    use bevy::ecs::query::FilteredAccess;
    let mut app = headless_client();
    let (components, resources) = handler_written(app.world());
    let host: Vec<FilteredAccess> = {
        use bevy::ecs::system::System;
        let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
        System::initialize(&mut host, app.world_mut())
            .filtered_accesses()
            .to_vec()
    };
    let labels: Vec<_> = app
        .world()
        .resource::<bevy::ecs::schedule::Schedules>()
        .iter()
        .map(|(_, schedule)| schedule.label())
        .collect();
    let mut pushes: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut systems = BTreeSet::new();
    for label in labels {
        app.world_mut().schedule_scope(label, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let vm = world
                .components()
                .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
            let input = |id: ComponentId| {
                let n = name(world, id);
                (Some(id) != vm && !n.contains("Messages<")).then(|| short_name(&n))
            };
            let graph = schedule.graph();
            for (key, system, _) in graph.systems.iter() {
                let name = system.name().to_string();
                systems.insert(name.clone());
                if system.is_exclusive() {
                    continue;
                }
                let Some(with_access) = graph.systems.get(key) else {
                    continue;
                };
                if !vm.is_some_and(|vm| with_access.access.combined_access().has_resource_write(vm))
                {
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
                    pushes.entry(short_name(&name)).or_default().extend(inputs);
                }
            }
        });
    }
    if !systems.iter().any(|n| n.contains("benilla_app::")) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let row = |system: &str| {
        let built = BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE
            .iter()
            .map(|(s, inputs, _)| (*s, *inputs));
        READS_FRAME_FINAL
            .iter()
            .copied()
            .chain(built)
            .find(|(s, _)| format!("::{system}").ends_with(s))
    };
    // A system [`NOT_YET_PLACED`] holds fires off a handler queue, which is its work.
    let no_view = |system: &str| {
        WRITES_NO_VIEW
            .iter()
            .chain(NOT_YET_PLACED)
            .any(|(s, _)| format!("::{system}").ends_with(s))
    };
    let mut wrong: Vec<String> = pushes
        .iter()
        .filter(|(system, _)| !no_view(system))
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
    let built = BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE
        .iter()
        .map(|(s, inputs, _)| (*s, *inputs));
    let stale: Vec<&str> = READS_FRAME_FINAL
        .iter()
        .copied()
        .chain(built)
        .filter(|(s, _)| {
            !pushes
                .keys()
                .any(|system| format!("::{system}").ends_with(s))
        })
        .map(|(s, _)| s)
        .chain(WRITES_NO_VIEW.iter().map(|(s, _)| *s).filter(|s| {
            !pushes
                .keys()
                .any(|system| format!("::{system}").ends_with(s))
        }))
        .collect();
    assert!(
        wrong.is_empty(),
        "these feeds push views off state a handler writes, read as the frame left it: version \
         what they read through the views lane, or give each its row in READS_FRAME_FINAL as it \
         stands, or in BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE for a build the reference too \
         makes outside every packet, or in WRITES_NO_VIEW for a system that pushes no view:\n  {}",
        wrong.join("\n  ")
    );
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}

/// The CVar mirror's two syncs are [`WRITES_NO_VIEW`]'s, not [`READS_FRAME_FINAL`]'s, on these
/// facts. The reference's `GetCVar` reads the one record (`0x488bd1`, `0x488bf9`) that `CVar::Set`
/// writes, running its callback inside the writer (`0x63df7d`). Here no handler holds the
/// registry but through the call host, whose appliers sync the VM's mirror inside the call that
/// writes it ([`crate::cvars::CvarSteps`]), and every other system of the frame that writes it
/// runs ahead of the input pass's sync or after the dispatch, so that sync carries the write in
/// before the next packet: a listener inside a packet reads every write made before it. A handler
/// that writes the registry itself, or a writer between the input sync and the dispatch, fails
/// here. The other schedules run outside `Update`, and an exclusive system shows no access.
#[test]
fn every_cvar_write_reaches_the_mirror_before_a_packet() {
    use bevy::ecs::query::{FilteredAccess, FilteredAccessSet};
    use bevy::ecs::schedule::SystemKey;
    use bevy::ecs::system::System;
    use std::collections::HashSet;
    let mut app = headless_client();
    let host = {
        let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
        System::initialize(&mut host, app.world_mut())
    };
    let registry = app
        .world()
        .components()
        .get_resource_id(TypeId::of::<crate::cvars::Cvars>())
        .expect("the client registers the CVar registry");
    // A write of the registry past the host a system lends, entry by entry, as one that lends the
    // host holds all of the host's.
    let past_the_host = |access: &FilteredAccessSet| {
        let mut with_host = access.clone();
        with_host.extend(host.clone());
        let mut own: Vec<&FilteredAccess> = access.filtered_accesses().iter().collect();
        if with_host.combined_access() == access.combined_access() {
            for h in host.filtered_accesses() {
                if let Some(i) = own.iter().position(|o| *o == h) {
                    own.remove(i);
                }
            }
        }
        own.iter().any(|o| o.access().has_resource_write(registry))
    };
    let handlers = app.world().resource::<crate::net::handlers::NetHandlers>();
    // The control: the handlers that lend the host hold the registry through it.
    assert!(
        handlers
            .accesses()
            .iter()
            .any(|(_, a)| a.combined_access().has_resource_write(registry)),
        "no handler reaches the registry, even through the host: the scan below reads nothing"
    );
    let handlers: Vec<&str> = handlers
        .accesses()
        .iter()
        .filter(|(_, a)| past_the_host(a))
        .map(|(n, _)| *n)
        .collect();
    assert!(
        handlers.is_empty(),
        "these handlers write the CVar registry past the call host, so a listener in the packet \
         reads the VM's mirror as the last sync left it: write it through the host, whose \
         appliers sync the mirror inside the write:\n  {}",
        handlers.join("\n  ")
    );
    let systems: Vec<(SystemKey, FilteredAccessSet)> =
        app.world_mut().schedule_scope(Update, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let graph = schedule.graph();
            graph
                .systems
                .iter()
                .filter(|(_, system, _)| !system.is_exclusive())
                .filter_map(|(k, _, _)| Some((k, graph.systems.get(k)?.access.clone())))
                .filter(|(_, access)| past_the_host(access))
                .collect()
        });
    let c = census(&mut app, Update, SyncPoints::Declared);
    if !type_names_available(&c) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let one = |suffix: &str| {
        let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
        match (hits.next(), hits.next()) {
            (Some((k, _)), None) => *k,
            _ => panic!("exactly one system named `…{suffix}` in Update"),
        }
    };
    let (sync, dispatch) = (
        one("::cvars::sync_input_cvars"),
        one("::net::apply::apply_net_updates"),
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
    // The control: the tip cursor's write at the world entry, a plain one after the dispatch.
    assert!(
        systems
            .iter()
            .any(|(k, _)| c.name(*k).ends_with("::game_tip::drive_game_tip")),
        "the scan no longer sees `drive_game_tip`'s write of the registry"
    );
    let before_sync = ahead_of(sync);
    let between: Vec<&str> = systems
        .iter()
        .filter(|(k, _)| *k != sync && !before_sync.contains(k))
        .filter(|(k, _)| !ahead_of(*k).contains(&dispatch))
        .map(|(k, _)| c.name(*k))
        .collect();
    assert!(
        between.is_empty(),
        "these write the CVar registry outside any call and may run between the input pass's \
         sync and the dispatch, so the frame's packets read the VM's mirror without the write: \
         order each ahead of `sync_input_cvars` or after the dispatch:\n  {}",
        between.join("\n  ")
    );
}

/// The calls that queue an event on the VM's own lane for its next tick
/// ([`crate::ui_script::fire_scan::host_queue_writers`]), by a suffix of the calling function's
/// `path::fn` and the method it calls, with why. The tick fires that lane after the drain, behind
/// every packet's events, whichever system or lane the call ran in, so no call is exempt; where
/// the reference signals the event in place, inside the packet or the input that caused it, the
/// row is remaining work. A new call fails until it has its row, and a row whose call is gone
/// fails as stale.
pub(super) const QUEUED_FOR_THE_TICK: &[(&str, &str, &str)] = &[
    (
        "capture/fixtures.rs::seed_ui_fixture",
        "set_world_map_explored",
        "a capture's fixture, dev tooling outside any session: no packet raises it",
    ),
    (
        "ui_world_map.rs::feed_world_map",
        "sync_world_map_to_player_zone",
        "the first world enter's selection, queued for the next tick (the module's deviation), \
         where the zone updater selects (`0x4947ac` → `0x4a6650` → `0x4a67a0`) and signals in \
         place (`0x4a6ce4`)",
    ),
    (
        "ui_world_map.rs::install_world_loc_projector",
        "set_world_loc_projector",
        "installs benilla's own projection at a VM's first frame, whose first landmark list \
         queues `WORLD_MAP_UPDATE`; no reference fact, as the reference projects inside \
         `0x4a67a0`",
    ),
];

/// Every call that queues an event on the VM's lane for its next tick has its row in
/// [`QUEUED_FOR_THE_TICK`], so a feed that raises off state it compares cannot hide behind the
/// tick.
#[test]
fn every_call_that_queues_for_the_tick_is_listed() {
    let writers = crate::ui_script::fire_scan::host_queue_writers();
    let row = |w: &crate::ui_script::fire_scan::QueueWriter| {
        QUEUED_FOR_THE_TICK
            .iter()
            .find(|(f, m, _)| w.writer.ends_with(&format!("/{f}")) && w.method == *m)
    };
    let unlisted: Vec<String> = writers
        .iter()
        .filter(|w| row(w).is_none())
        .map(|w| format!("{} calls {} at {}", w.writer, w.method, w.at))
        .collect();
    assert!(
        unlisted.is_empty(),
        "these queue an event for the VM's next tick, behind every packet's: signal it in place, \
         or give the call its row in QUEUED_FOR_THE_TICK:\n  {}",
        unlisted.join("\n  ")
    );
    let stale: Vec<String> = QUEUED_FOR_THE_TICK
        .iter()
        .filter(|(f, m, _)| {
            !writers
                .iter()
                .any(|w| w.writer.ends_with(&format!("/{f}")) && w.method == *m)
        })
        .map(|(f, m, _)| format!("{f} {m}"))
        .collect();
    assert!(stale.is_empty(), "stale rows: {stale:?}");
    let empty: Vec<&str> = QUEUED_FOR_THE_TICK
        .iter()
        .filter(|(_, _, why)| why.is_empty())
        .map(|(f, _, _)| *f)
        .collect();
    assert!(empty.is_empty(), "rows with no reason: {empty:?}");
}

/// Each entry of the raise check's allow list ([`crate::ui_script::raise_origin::RAISES_AT_THEIR_OWN_PLACE`])
/// names a function that exists and gives its reason; one that cites a guard list has that list's
/// row for it; and every call that queues for the tick has its entry, as each raises at its own
/// place in the frame.
#[test]
fn every_raise_origin_allowed_names_its_function_and_its_row() {
    use crate::ui_script::raise_origin::{RAISES_AT_THEIR_OWN_PLACE, ROW_LISTS};
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let module = |file: &str| file.trim_end_matches(".rs").replace('/', "::");
    let queued: BTreeSet<String> = QUEUED_FOR_THE_TICK
        .iter()
        .map(|(f, _, _)| {
            let (file, func) = f.split_once("::").expect("a `file::fn` row");
            format!("{}::{func}", module(file))
        })
        .collect();
    let mut wrong = Vec::new();
    for a in RAISES_AT_THEIR_OWN_PLACE {
        let (path, func) = a.frame.rsplit_once("::").expect("a `module::fn` frame");
        let dir = path.replace("::", "/");
        let found = [format!("{dir}.rs"), format!("{dir}/mod.rs")]
            .iter()
            .filter_map(|f| std::fs::read_to_string(src.join(f)).ok())
            .any(|code| {
                code.contains(&format!("fn {func}(")) || code.contains(&format!("fn {func}<"))
            });
        if !found {
            wrong.push(format!("{}: no such function", a.frame));
        }
        if a.why.trim().is_empty() {
            wrong.push(format!("{}: no reason", a.frame));
        }
        let Some((list, _)) = a.why.split_once(':').filter(|(l, _)| ROW_LISTS.contains(l)) else {
            continue;
        };
        let suffix = format!("::{}", a.frame);
        let named = a.why.split('`').nth(1).filter(|n| n.starts_with("::"));
        let has_row = match list {
            "QUEUED_FOR_THE_TICK" => queued.contains(a.frame),
            "AT_THEIR_OWN_PLACE" => AT_THEIR_OWN_PLACE.iter().any(|(s, _)| s.ends_with(&suffix)),
            "NOT_YET_ON_A_LANE" => NOT_YET_ON_A_LANE
                .iter()
                .any(|(_, s, _)| named.map_or(s.ends_with(&suffix), |n| *s == n)),
            _ => false,
        };
        if !has_row {
            wrong.push(format!("{}: {list} has no row for it", a.frame));
        }
    }
    for q in &queued {
        if !RAISES_AT_THEIR_OWN_PLACE.iter().any(|a| a.frame == q) {
            wrong.push(format!("{q}: queues for the tick and has no allow entry"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The keys of `keys` named more than once, as `"<list>: <key>"`.
pub(super) fn named_twice(list: &str, keys: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    keys.into_iter()
        .filter(|k| !seen.insert(k.clone()))
        .map(|k| format!("{list}: {k}"))
        .collect()
}

/// No guard list names an entry twice and no row lists an input twice, as a stale check passes on
/// one copy while the other stays; and a system that writes the VM has one place among the lists
/// that classify it.
#[test]
fn no_guard_list_names_an_entry_twice() {
    let keyed = |list: &[(&str, &str)]| list.iter().map(|(s, _)| s.to_string()).collect::<Vec<_>>();
    let mut twice = Vec::new();
    twice.extend(named_twice(
        "NOT_YET_ON_A_LANE",
        NOT_YET_ON_A_LANE
            .iter()
            .map(|(row, s, _)| format!("{row:?} {s}")),
    ));
    for (list, names) in [
        ("HOST_HELD_UNREAD", HOST_HELD_UNREAD),
        ("READ_ONLY_TO_HANDLERS", READ_ONLY_TO_HANDLERS),
        ("QUEUES_NOT_VIEWS", QUEUES_NOT_VIEWS),
    ] {
        twice.extend(named_twice(list, names.iter().map(|s| s.to_string())));
    }
    twice.extend(named_twice(
        "FILLED_THROUGH_A_READ",
        FILLED_THROUGH_A_READ.iter().map(|(s, _)| s.to_string()),
    ));
    for (list, rows) in [
        ("WRITES_NO_VIEW", WRITES_NO_VIEW),
        ("AT_THEIR_OWN_PLACE", AT_THEIR_OWN_PLACE),
        ("NOT_YET_PLACED", NOT_YET_PLACED),
        ("DEFERS_NO_LANE_ITEM", DEFERS_NO_LANE_ITEM),
    ] {
        twice.extend(named_twice(list, keyed(rows)));
    }
    twice.extend(named_twice(
        "READS_FRAME_FINAL",
        READS_FRAME_FINAL.iter().map(|(s, _)| s.to_string()),
    ));
    for (s, inputs) in READS_FRAME_FINAL {
        twice.extend(named_twice(
            &format!("READS_FRAME_FINAL {s}"),
            inputs.iter().map(|i| i.to_string()),
        ));
    }
    twice.extend(named_twice(
        "BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE",
        BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE
            .iter()
            .map(|(s, _, _)| s.to_string()),
    ));
    for (s, inputs, _) in BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE {
        twice.extend(named_twice(
            &format!("BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE {s}"),
            inputs.iter().map(|i| i.to_string()),
        ));
    }
    twice.extend(named_twice(
        "QUEUED_FOR_THE_TICK",
        QUEUED_FOR_THE_TICK
            .iter()
            .map(|(site, call, _)| format!("{site} {call}")),
    ));
    // One place for a VM writer among the lists that classify it.
    twice.extend(named_twice(
        "the view lists",
        READS_FRAME_FINAL
            .iter()
            .map(|(s, _)| s.to_string())
            .chain(
                BUILT_OUTSIDE_THE_PACKETS_IN_THE_REFERENCE
                    .iter()
                    .map(|(s, _, _)| s.to_string()),
            )
            .chain(keyed(WRITES_NO_VIEW))
            .chain(keyed(NOT_YET_PLACED)),
    ));
    assert!(twice.is_empty(), "named twice:\n  {}", twice.join("\n  "));
}
