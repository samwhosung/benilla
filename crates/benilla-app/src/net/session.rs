//! The bridge's session handlers: the connection edges, our own teleport and worldport, the
//! server clocks, reputations and the player's login-scoped stores. Registered ahead of every
//! window plugin, so the teardown runs before the windows' own `Disconnected` listeners.

use benilla_protocol::messages::Character;
use benilla_protocol::JumpInfo;
use bevy::prelude::*;

use crate::items::Items;
use crate::names::NameCache;
use crate::ui_quest::QuestGiver;

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::ReputationListOp;
use bevy::ecs::system::SystemParam;

use super::{clock_tick, ServerTime};
use super::{
    AddonInfoMessage, CharActionResultMessage, CharListMessage, CharacterLoginFailedMessage,
    CinematicTriggeredMessage, ClientControlMessage, DisconnectedMessage, DroppedOpcodes,
    EnteredWorldMessage, GuidIndex, HomeBind, KnockBackMessage, LoggedOutMessage,
    LoginFailedMessage, LoginQueuedMessage, LoginStageMessage, NetCommands, NetHandlerApp,
    NetStatus, ObjectStore, PendingTransfer, Proficiencies, RealmListMessage, Reputations,
    RosterFailedMessage, SelfGuid, ServerWallClock, TeleportMessage, WorldAuthMessage,
    WorldportMessage,
};
use benilla_world::frame_pace::FrameStep;

/// Registers the session handlers.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::LoginStage, on_login_edge)
        .net_handler(K::WorldAuthenticated, on_login_edge)
        .net_handler(K::RosterFailed, on_login_edge)
        .net_handler(K::AddonInfo, on_login_edge)
        .net_handler(K::LoginQueued, on_login_edge)
        .net_handler(K::LoginFailed, on_login_edge)
        .net_handler(K::RealmList, on_login_edge)
        .net_handler(K::CharacterList, on_character_list)
        .net_handler(K::CharActionResult, on_login_edge)
        .net_handler(K::CharRenamed, on_login_edge)
        .net_handler(K::CharacterLoginFailed, on_login_edge)
        .net_handler(K::CinematicTriggered, on_login_edge)
        .net_handler(K::Connected, on_connected)
        .net_handler(K::LoggedOut, on_logged_out)
        .net_handler(K::Disconnected, on_disconnected)
        .net_handler(K::Teleport, on_mover_edge)
        .net_handler(K::KnockBack, on_mover_edge)
        .net_handler(K::ClientControl, on_mover_edge)
        .net_handler(K::Worldport, on_worldport)
        .net_handler(K::TransferPending, on_transfer)
        .net_handler(K::TransferAborted, on_transfer)
        .net_handler(K::TimeSpeed, on_clock)
        .net_handler(K::GameSpeedSet, on_clock)
        .net_handler(K::GameTimeUpdate, on_clock)
        .net_handler(K::GameTimeSet, on_clock)
        .net_handler(K::ServerUnixTime, on_clock)
        .net_handler(K::Reputations, on_reputations)
        .net_handler(K::ReputationDelta, on_reputation_delta)
        .net_handler(K::ForcedReactions, on_reputations)
        .net_handler(K::ReputationVisible, on_reputations)
        .net_handler(K::ReputationAtWar, on_reputations)
        .net_handler(K::BindPoint, on_player_store)
        .net_handler(K::Proficiency, on_player_store)
        .net_handler(K::PacketDropped, on_packet_dropped)
        .net_handler(K::Pong, on_pong);
}

/// The edges the bridge publishes as messages, and the three resources they park in.
#[derive(SystemParam)]
pub(crate) struct Edges<'w> {
    teleports: MessageWriter<'w, TeleportMessage>,
    worldports: MessageWriter<'w, WorldportMessage>,
    char_lists: MessageWriter<'w, CharListMessage>,
    realm_lists: MessageWriter<'w, RealmListMessage>,
    char_actions: MessageWriter<'w, CharActionResultMessage>,
    char_renames: MessageWriter<'w, super::CharRenameMessage>,
    char_login_failures: MessageWriter<'w, CharacterLoginFailedMessage>,
    entered_world: MessageWriter<'w, EnteredWorldMessage>,
    addon_reply: ResMut<'w, super::AddonInfoReply>,
    billing: ResMut<'w, super::SessionBilling>,
    logged_out: MessageWriter<'w, LoggedOutMessage>,
    knockbacks: MessageWriter<'w, KnockBackMessage>,
    login_stages: MessageWriter<'w, LoginStageMessage>,
    login_queued: MessageWriter<'w, LoginQueuedMessage>,
    world_auth: MessageWriter<'w, WorldAuthMessage>,
    roster_failed: MessageWriter<'w, RosterFailedMessage>,
    addon_info: MessageWriter<'w, AddonInfoMessage>,
    login_failures: MessageWriter<'w, LoginFailedMessage>,
    disconnects: MessageWriter<'w, DisconnectedMessage>,
    client_control: MessageWriter<'w, ClientControlMessage>,
    cinematics: MessageWriter<'w, CinematicTriggeredMessage>,
    transfer: ResMut<'w, PendingTransfer>,
}

/// The bridge state the session edges write.
#[derive(SystemParam)]
pub(crate) struct Bridge<'w, 's> {
    commands: Commands<'w, 's>,
    index: ResMut<'w, GuidIndex>,
    self_guid: ResMut<'w, SelfGuid>,
    status: ResMut<'w, NetStatus>,
    names: ResMut<'w, NameCache>,
    items: ResMut<'w, Items>,
    cooldowns: ResMut<'w, crate::spell::Cooldowns>,
    reputations: ResMut<'w, Reputations>,
    server_time: ResMut<'w, ServerTime>,
    wall_clock: ResMut<'w, ServerWallClock>,
    home_bind: ResMut<'w, HomeBind>,
    proficiencies: ResMut<'w, Proficiencies>,
    dropped: ResMut<'w, DroppedOpcodes>,
    net: Res<'w, NetCommands>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    transports: Query<'w, 's, &'static crate::transport::Transport>,
}

fn on_login_edge(In(ev): In<SessionEvent>, mut e: Edges) {
    match ev {
        SessionEvent::LoginStage { stage } => login_stage(stage, &mut e.login_stages),
        SessionEvent::WorldAuthenticated {
            realm,
            epoch,
            generation,
        } => {
            e.world_auth.write(WorldAuthMessage {
                realm,
                epoch,
                generation,
            });
        }
        SessionEvent::RosterFailed { epoch } => {
            e.roster_failed.write(RosterFailedMessage { epoch });
        }
        SessionEvent::AddonInfo { records } => {
            // Held for the screens built after it, and for the world entry's load.
            e.addon_reply.0 = Some(records.clone());
            e.addon_info.write(AddonInfoMessage { records });
        }
        SessionEvent::LoginQueued { position, realm } => {
            e.login_queued.write(LoginQueuedMessage { position, realm });
        }
        SessionEvent::LoginFailed {
            refusal, reason, ..
        } => login_failed(refusal, reason, &mut e.login_failures),
        SessionEvent::RealmList { realms, failed } => {
            e.realm_lists.write(RealmListMessage { realms, failed });
        }
        SessionEvent::CharActionResult { action, code } => {
            char_action_result(action, code, &mut e.char_actions)
        }
        SessionEvent::CharRenamed { result, renamed } => {
            e.char_renames
                .write(super::CharRenameMessage { result, renamed });
        }
        SessionEvent::CharacterLoginFailed { result } => {
            character_login_failed(result, &mut e.char_login_failures)
        }
        SessionEvent::CinematicTriggered { cinematic_id } => {
            cinematic_triggered(cinematic_id, &mut e.cinematics)
        }
        _ => {}
    }
}

fn on_character_list(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::CharacterList {
        characters,
        realm,
        cause,
        epoch,
        ..
    } = ev
    {
        character_list(
            characters,
            realm,
            cause,
            epoch,
            &mut b.status,
            &mut e.char_lists,
        );
    }
}

fn on_connected(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::Connected {
        self_guid: guid,
        name,
        billing,
        tutorial_flags,
        addon_info,
    } = ev
    {
        e.billing.0 = billing;
        connected(
            guid,
            name,
            tutorial_flags,
            addon_info,
            &mut e.addon_reply,
            &mut b.self_guid,
            &mut b.status,
            &mut b.names,
            &mut e.entered_world,
        );
    }
}

fn on_logged_out(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    mut b: Bridge,
    view_log: Option<ResMut<crate::ui_unit::views::ViewLog>>,
) {
    if let SessionEvent::LoggedOut = ev {
        log_teardown(view_log, &b);
        logged_out(
            &mut b.commands,
            &mut b.index,
            &mut b.self_guid,
            &mut e.logged_out,
        );
        // The reference zeroes the forced table's count and the 64 slots only in teardown
        // `0x4d5360` (`0x4d5419`, `0x4d541f`), whose one caller is `ShutdownGame 0x491180`
        // (`0x4911dc`), never on a worldport. The session's end stands in for that shutdown: the
        // table and the standings are the character's, and ones left here would colour the next
        // character's world. The server resends the table at login for a saved aura (vmangos
        // `Unit.cpp:3511` → `SpellAuras.cpp:2744-2745`).
        b.reputations.tear_down();
    }
}

/// The bridge's half of the session end; each window's own listener runs after it.
fn on_disconnected(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    mut b: Bridge,
    mut released: ResMut<super::objects::Released>,
    view_log: Option<ResMut<crate::ui_unit::views::ViewLog>>,
) {
    if let SessionEvent::Disconnected { reason, end } = ev {
        log_teardown(view_log, &b);
        // The same teardown as `on_logged_out`'s.
        b.reputations.tear_down();
        // Leaving the world frees the released objects with the live ones (`0x467700`).
        released.clear();
        disconnected(
            reason,
            end,
            &mut b.commands,
            &mut b.index,
            &mut b.self_guid,
            &mut b.status,
            &mut b.names,
            &mut b.items,
            &mut b.cooldowns,
            &mut e.transfer,
            &mut e.disconnects,
        );
    }
}

/// The world left at once: the stores the teardown takes, logged for the views a listener reads
/// ([`crate::ui_unit::views::ViewLog::torn_down`]).
fn log_teardown(view_log: Option<ResMut<crate::ui_unit::views::ViewLog>>, b: &Bridge) {
    if let Some(mut log) = view_log {
        log.torn_down(&b.index, |e| b.stores.get(e).ok());
    }
}

/// Teleport, knockback and client control, forwarded unjudged to the controller, which applies
/// and answers them.
fn on_mover_edge(In(ev): In<SessionEvent>, mut e: Edges, b: Bridge) {
    match ev {
        SessionEvent::Teleport {
            guid,
            counter,
            position,
            orientation,
        } => teleport(
            guid,
            counter,
            position,
            orientation,
            &b.self_guid,
            &mut e.teleports,
        ),
        SessionEvent::KnockBack {
            guid,
            counter,
            launch,
        } => knock_back(guid, counter, launch, &b.self_guid, &mut e.knockbacks),
        SessionEvent::ClientControl { mover, allow_move } => {
            e.client_control
                .write(ClientControlMessage { mover, allow_move });
        }
        _ => {}
    }
}

fn on_worldport(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    mut b: Bridge,
    mut group: ResMut<crate::ui_party::GroupState>,
    script: Option<NonSend<benilla_ui::script::UiScript>>,
    mut released: ResMut<super::objects::Released>,
    view_log: Option<ResMut<crate::ui_unit::views::ViewLog>>,
) {
    if let SessionEvent::Worldport {
        map_id,
        position,
        orientation,
        needs_ack,
    } = ev
    {
        // Every streamed roster member is about to be freed, and each one's destructor runs the
        // roster hook (`0x5dd6a0` → `0x4e85d0` mode 0).
        let before = view_log.as_ref().map(|_| group.stats.clone());
        crate::ui_party::net::roster_deactivated(
            &mut group,
            &b.index,
            &b.stores,
            &b.names,
            script.as_deref(),
            &b.net,
        );
        let mut view_log = view_log;
        if let (Some(log), Some(before)) = (view_log.as_deref_mut(), before) {
            log.records(&before, &group.stats);
        }
        log_teardown(view_log, &b);
        worldport(
            map_id,
            position,
            orientation,
            needs_ack,
            &mut b.commands,
            &mut b.index,
            &mut e.transfer,
            &b.transports,
            &mut e.worldports,
        );
        // Loading a new world frees the released objects with the live ones (`0x467700`).
        released.clear();
    }
}

fn on_transfer(In(ev): In<SessionEvent>, mut e: Edges) {
    match ev {
        SessionEvent::TransferPending {
            map_id,
            transport_entry,
        } => transfer_pending(map_id, transport_entry, &mut e.transfer),
        SessionEvent::TransferAborted { reason } => transfer_aborted(reason, &mut e.transfer),
        _ => {}
    }
}

/// The game clock's four messages (`SMSG_LOGIN_SETTIMESPEED`, `SMSG_GAMESPEED_SET`,
/// `SMSG_GAMETIME_UPDATE`, `SMSG_GAMETIME_SET`) and the server's wall clock.
fn on_clock(In(ev): In<SessionEvent>, mut b: Bridge, step: Res<FrameStep>) {
    match ev {
        SessionEvent::TimeSpeed { .. }
        | SessionEvent::GameSpeedSet { .. }
        | SessionEvent::GameTimeUpdate { .. }
        | SessionEvent::GameTimeSet { .. } => {
            let lines = b.server_time.0.on_message(&ev, clock_tick(&step));
            for line in lines.into_iter().flatten() {
                info!("console: {line}");
            }
        }
        SessionEvent::ServerUnixTime { unix_time } => {
            server_unix_time(unix_time, &mut b.wall_clock)
        }
        _ => {}
    }
}

/// `UPDATE_FACTION` (`0x703e50` with 0x174) as a reputation message signals it, on the notice lane
/// in the packets' order.
const UPDATE_FACTION: crate::field_watch::FieldNotice = crate::field_watch::FieldNotice::Global {
    id: crate::field_watch::id::UPDATE_FACTION,
};

/// `CGUnit_C::RefreshReaction` (`0x606e20`) on the active player from a reputation message, once,
/// when the object manager holds the player (`0x4d5a55`, `0x4d58ff`): `UNIT_FACTION` for it, its
/// summon and its charm, then for every held unit the sweep reaches, on the notice lane.
fn refresh_player_reaction(b: &Bridge, lane: &mut crate::field_watch::NoticeLane) {
    let Some(me) = b.self_guid.0 else {
        return;
    };
    let Some(store) = b.index.0.get(&me).and_then(|&e| b.stores.get(e).ok()) else {
        return;
    };
    let others = || {
        let units = b.index.0.iter().filter_map(|(&g, &e)| {
            let s = b.stores.get(e).ok()?;
            s.is_unit().then_some((g, &s.0))
        });
        crate::field_watch::reaction_sweep(me, units)
    };
    lane.write_batch(crate::field_watch::reaction_signals(
        me,
        &store.0,
        Some(me),
        &others,
    ));
}

fn on_reputations(
    In(ev): In<SessionEvent>,
    mut b: Bridge,
    factions: Option<Res<crate::target::ring::Factions>>,
    chat: Res<crate::ui_chat::ChatLog>,
    // Absent in a harness with no interface.
    mut lane: Option<ResMut<crate::field_watch::NoticeLane>>,
) {
    let signal = |n: usize, lane: &mut Option<ResMut<crate::field_watch::NoticeLane>>| {
        if let Some(lane) = lane.as_deref_mut() {
            lane.write_batch(std::iter::repeat_n(UPDATE_FACTION, n));
        }
    };
    match ev {
        SessionEvent::Reputations { standings } => {
            reputations(standings, &mut b.reputations);
            // With the player held, the rebuild (`0x4d5694`), whose two sorts signal
            // (`0x4d55d9` → `0x4d6457`, and `0x4d55de`), then the message's own (`0x4d5699`).
            let held = self_facts(&b).is_some();
            if held {
                b.reputations.list_ops.push(ReputationListOp::Rebuild);
            }
            signal(1 + 2 * usize::from(held), &mut lane);
        }
        SessionEvent::ForcedReactions { reactions } => {
            forced_reactions(reactions, &mut b.reputations);
            if let Some(lane) = lane.as_deref_mut() {
                refresh_player_reaction(&b, lane);
            }
        }
        SessionEvent::ReputationVisible { list_id } => {
            // The chat line names the rank, so it reads the store before the flag lands.
            if let (Some(f), Some(me)) = (factions.as_deref(), self_facts(&b)) {
                let line = crate::ui_reputation::visible_line(
                    list_id,
                    &b.reputations,
                    f.catalog(),
                    me.race,
                    me.class,
                );
                crate::ui_reputation::queue_standing_lines(line, f.catalog(), me.female, &chat);
            }
            // A slot newly visible is added and sorted, which signals (`0x4d5738`, `0x4d574a` →
            // `0x4d5dab`).
            let listed = reputation_visible(list_id, &mut b.reputations);
            if listed {
                b.reputations
                    .list_ops
                    .push(ReputationListOp::Visible(list_id));
            }
            signal(usize::from(listed), &mut lane);
        }
        SessionEvent::ReputationAtWar { list_id, flags } => {
            reputation_at_war(list_id, flags, &mut b.reputations);
            // Whether or not the bit moved (`0x4d56f6`).
            signal(1, &mut lane);
        }
        _ => {}
    }
}

/// What the reputation chat lines read of the local player.
struct SelfFacts {
    race: u8,
    class: u8,
    female: bool,
}

/// The local player's race, class and gender, `None` before the self object streams in; the
/// reference's `0x62c670` prints nothing without it (`0x62c6bf`). The gender is the real one
/// (`UNIT_FIELD_BYTES_0` byte 2), where `0x5ed5b0` reads `PLAYER_BYTES_3` byte 0 whole and so
/// takes a drunk female on vmangos for a male: that byte carries the drunk bits.
fn self_facts(b: &Bridge) -> Option<SelfFacts> {
    let entity = *b.index.0.get(&b.self_guid.0?)?;
    let store = b.stores.get(entity).ok()?;
    Some(SelfFacts {
        race: store.0.unit_race()?,
        class: store.0.unit_class()?,
        female: store.0.unit_gender() == Some(1),
    })
}

/// The chat lines read the deltas against the store, so they must run before the overwrite. The
/// "increased by" line comes first, then the rank lines, as the reference's entry loop orders them
/// (`0x4d57f5`, `0x4d5896`).
fn on_reputation_delta(
    In(ev): In<SessionEvent>,
    mut b: Bridge,
    factions: Option<Res<crate::target::ring::Factions>>,
    chat_log: Res<crate::ui_chat::ChatLog>,
    mut quest: ResMut<QuestGiver>,
    quest_log: Option<ResMut<crate::ui_quest_log::QuestLog>>,
    // Absent in a harness with no interface.
    mut lane: Option<ResMut<crate::field_watch::NoticeLane>>,
) {
    if let SessionEvent::ReputationDelta { standings } = ev {
        crate::combat_log::chat::faction_standing(
            &standings,
            &b.reputations,
            factions.as_deref(),
            &chat_log,
        );
        if let (Some(f), Some(me)) = (factions.as_deref(), self_facts(&b)) {
            let lines = crate::ui_reputation::delta_lines(
                &standings,
                &b.reputations,
                f.catalog(),
                me.race,
                me.class,
            );
            crate::ui_reputation::queue_standing_lines(lines, f.catalog(), me.female, &chat_log);
        }
        let ranks = factions
            .as_deref()
            .zip(self_facts(&b))
            .map(|(f, me)| (f.catalog(), me.race, me.class));
        let latches = reputation_delta(standings, &mut b.reputations, ranks, &mut quest);
        b.reputations.list_ops.push(ReputationListOp::Standing {
            listed: latches.listed.clone(),
            flags_moved: latches.flags_moved,
        });
        // A flag byte that moved re-sorts, which signals (`0x4d58c8` → `0x4d5dab`).
        if latches.flags_moved {
            if let Some(lane) = lane.as_deref_mut() {
                lane.write(UPDATE_FACTION);
            }
        }
        // A moved rank rebuilds the quest log and refreshes the active player's reaction once
        // for the message (`0x4d58ea`-`0x4d5905`); `ranks` holds only with the player.
        if latches.rank_moved {
            if let Some(mut quest_log) = quest_log {
                quest_log.note_faction_rank_move();
            }
            if let Some(lane) = lane.as_deref_mut() {
                refresh_player_reaction(&b, lane);
            }
        }
        // The message's own signal, whatever it held, none included (`0x4d597f`-`0x4d5984`).
        if let Some(lane) = lane.as_deref_mut() {
            lane.write(UPDATE_FACTION);
        }
    }
}

/// The player's login-scoped stores the bridge defines.
fn on_player_store(In(ev): In<SessionEvent>, mut b: Bridge) {
    match ev {
        SessionEvent::BindPoint { area } => b.home_bind.0 = Some(area),
        SessionEvent::Proficiency {
            item_class,
            subclass_mask,
        } => {
            b.proficiencies.0.insert(item_class, subclass_mask);
        }
        _ => {}
    }
}

fn on_packet_dropped(In(ev): In<SessionEvent>, mut b: Bridge) {
    if let SessionEvent::PacketDropped {
        opcode,
        unparseable,
    } = ev
    {
        packet_dropped(opcode, unparseable, &mut b.dropped);
    }
}

/// The read thread consumes the pong inline, as the reference's `OnData` (`0x537b10`) hands
/// `SMSG_PONG` to `HandlePong` (`0x537d60`); reaching here means that bypass is gone and every
/// latency reading is a frame late, so it warns.
fn on_pong(In(ev): In<SessionEvent>) {
    if let SessionEvent::Pong { sequence } = ev {
        warn!("net: pong seq={sequence} reached the drain — the read thread's RTT bypass is gone");
    }
}

/// The pre-logon handshake reached a new stage.
fn login_stage(stage: benilla_protocol::LoginStage, out: &mut MessageWriter<LoginStageMessage>) {
    out.write(LoginStageMessage { stage });
}

/// A login failed before the roster; the IO thread is back at its pre-logon park and
/// [`crate::glue_session::login`] decides what follows.
fn login_failed(
    refusal: Option<benilla_protocol::LoginRefusal>,
    reason: String,
    out: &mut MessageWriter<LoginFailedMessage>,
) {
    out.write(LoginFailedMessage { refusal, reason });
}

/// `SMSG_CHAR_CREATE`/`SMSG_CHAR_DELETE`: the glue screen turns the code into its string.
fn char_action_result(
    action: benilla_protocol::CharAction,
    code: u8,
    out: &mut MessageWriter<CharActionResultMessage>,
) {
    out.write(CharActionResultMessage { action, code });
}

/// `SMSG_CHAR_ENUM`: the roster and the connected realm, for character select.
fn character_list(
    characters: Vec<Character>,
    realm: Option<benilla_protocol::RealmInfo>,
    cause: benilla_protocol::RosterCause,
    epoch: u64,
    status: &mut NetStatus,
    char_lists: &mut MessageWriter<CharListMessage>,
) {
    info!(
        "net: character select — {} character(s) on the account",
        characters.len()
    );
    // A roster means a live link: clear the reason a lost link left, or the select screen keeps
    // its "Server down" note until the next world entry.
    status.last_reason = None;
    char_lists.write(CharListMessage {
        characters,
        realm,
        cause,
        epoch,
    });
}

/// `SMSG_CHARACTER_LOGIN_FAILED`: the announced entry is void; the glue manager says why.
fn character_login_failed(result: u8, out: &mut MessageWriter<CharacterLoginFailedMessage>) {
    warn!("net: character login refused (result {result:#04x})");
    out.write(CharacterLoginFailedMessage { result });
}

/// `SMSG_TRIGGER_CINEMATIC`, handed to [`crate::cinematic`], which sends the ack at the end of
/// playback. While a cinematic runs unacked, vmangos anchors visibility to the camera
/// (`Player::UpdateCinematic`), so every path there must still ack.
fn cinematic_triggered(
    cinematic_id: u32,
    triggered: &mut MessageWriter<CinematicTriggeredMessage>,
) {
    info!("net: cinematic {cinematic_id} triggered");
    triggered.write(CinematicTriggeredMessage { cinematic_id });
}

/// The first in-world event: records our guid and seeds the name cache with our own name.
fn connected(
    guid: u64,
    name: String,
    tutorial_flags: Option<Vec<u8>>,
    addon_info: Option<Vec<benilla_protocol::messages::AddonRecord>>,
    addon_reply: &mut crate::net::AddonInfoReply,
    self_guid: &mut SelfGuid,
    status: &mut NetStatus,
    names: &mut NameCache,
    entered_world: &mut MessageWriter<EnteredWorldMessage>,
) {
    self_guid.0 = Some(guid);
    status.connected = true;
    status.last_reason = None;
    info!("net: in world as {name} (guid {guid})");
    // The reference clears player and pet names at every world entry (`0x555740`, arm
    // `0x5557ad`), since a guid may name a new character; creature templates survive.
    names.clear_world_session();
    // Our own name came with the login, so "player" never queries.
    names.insert_player(guid, name, None);
    // Set before the world-entry UI load reads it, and every login, so a silent server inherits
    // no earlier answer.
    addon_reply.0 = addon_info;
    entered_world.write(EnteredWorldMessage { tutorial_flags });
}

/// `SMSG_LOGOUT_COMPLETE`: back to character select.
fn logged_out(
    commands: &mut Commands,
    index: &mut GuidIndex,
    self_guid: &mut SelfGuid,
    logged_out: &mut MessageWriter<LoggedOutMessage>,
) {
    // A logout ends the character's session, so the avatar goes too, unlike a reconnectable
    // disconnect; clearing `SelfGuid` first makes the following teardown total.
    info!("net: logged out — back to character select");
    if let Some(guid) = self_guid.0.take() {
        if let Some(e) = index.0.remove(&guid) {
            commands.entity(e).despawn();
        }
    }
    logged_out.write(LoggedOutMessage);
}

/// The world session ended, the link kept or not: tears down the streamed world and clears every
/// session-scoped cache.
fn disconnected(
    reason: String,
    end: benilla_protocol::SessionEnd,
    commands: &mut Commands,
    index: &mut GuidIndex,
    self_guid: &mut SelfGuid,
    status: &mut NetStatus,
    names: &mut NameCache,
    items: &mut Items,
    cooldowns: &mut crate::spell::Cooldowns,
    pending_transfer: &mut PendingTransfer,
    disconnects: &mut MessageWriter<DisconnectedMessage>,
) {
    // Whether the session is over is settled once, here, and carried to every reader.
    let msg = DisconnectedMessage::new(reason.clone(), end);
    let over = msg.session_over;
    disconnects.write(msg);
    warn!("net: {reason} — tearing down the streamed world");
    // An unfinished far teleport dies with the socket.
    pending_transfer.0 = None;
    status.connected = false;
    // A logout or a refused login ends the character, the link kept: nothing reconnects it.
    let character_over = over || end == benilla_protocol::SessionEnd::LoggedOut;
    // The RTT ring is not cleared here: the read thread wipes it as it re-enters its cycle loop.
    // Every streamed entity despawns at once, not faded, so none races the reconnect's creates.
    // The self avatar stays as the local puppet for the reconnect to refresh, unless the
    // character is over, and then it goes as on logout.
    let keep = if character_over { None } else { self_guid.0 };
    if character_over {
        self_guid.0 = None;
    }
    index.0.retain(|guid, e| {
        if Some(*guid) == keep {
            return true;
        }
        commands.entity(*e).despawn();
        false
    });
    // A failure to show at character select; a kept link has none.
    if end != benilla_protocol::SessionEnd::LoggedOut {
        status.last_reason = Some(reason);
    }
    // In-flight name queries died with the socket.
    names.clear_pending();
    items.clear_session();
    // `SMSG_INITIAL_SPELLS` resends every running cooldown at world entry and `seed_initial`
    // appends, so the list must not outlive the session.
    cooldowns.clear_session();
}

/// `MSG_MOVE_TELEPORT_ACK`: only our own is forwarded to the controller, whose ack resumes our
/// movement.
fn teleport(
    guid: u64,
    counter: u32,
    position: [f32; 3],
    orientation: f32,
    self_guid: &SelfGuid,
    teleports: &mut MessageWriter<TeleportMessage>,
) {
    if self_guid.0 == Some(guid) {
        teleports.write(TeleportMessage {
            guid,
            counter,
            position,
            orientation,
        });
    }
}

/// `SMSG_MOVE_KNOCK_BACK`: a ballistic launch our controller flies and acks. The reference
/// handles it for the controller only (`0x603f90`); observers get `MSG_MOVE_KNOCK_BACK`
/// (`0x603bb0`), so one naming another unit is dropped.
fn knock_back(
    guid: u64,
    counter: u32,
    launch: JumpInfo,
    self_guid: &SelfGuid,
    knockbacks: &mut MessageWriter<KnockBackMessage>,
) {
    if self_guid.0 == Some(guid) {
        knockbacks.write(KnockBackMessage {
            guid,
            counter,
            launch,
        });
    }
}

/// `SMSG_TRANSFER_PENDING`: latched for the worldport; its transport block makes `NEW_WORLD`'s
/// coordinates boat-local.
fn transfer_pending(map_id: u32, transport_entry: Option<u32>, pending: &mut PendingTransfer) {
    match transport_entry {
        Some(entry) => info!("net: transfer pending → map {map_id} riding transport {entry}"),
        None => info!("net: transfer pending → map {map_id}"),
    }
    pending.0 = Some(crate::net::PendingTransferInfo {
        map_id,
        transport_entry,
    });
}

/// `SMSG_TRANSFER_ABORTED`: clears the latch, which ends the screen the transfer raised as
/// `0x401a11` does (`0x407e80`); the reason's line is `ui_chat`'s.
fn transfer_aborted(reason: u8, pending: &mut PendingTransfer) {
    warn!("net: transfer aborted (reason {reason})");
    pending.0 = None;
}

/// `SMSG_NEW_WORLD`/`SMSG_LOGIN_VERIFY_WORLD`: drops every tracked object, then hands the app
/// the destination. A transport whose timetable touches the new map is spared, so a ridden boat
/// sails through the map change and the server's re-create refreshes it in place.
fn worldport(
    map_id: u32,
    position: [f32; 3],
    orientation: f32,
    needs_ack: bool,
    commands: &mut Commands,
    index: &mut GuidIndex,
    pending: &mut PendingTransfer,
    transports: &Query<&crate::transport::Transport>,
    worldports: &mut MessageWriter<WorldportMessage>,
) {
    index.0.retain(|guid, e| {
        if transports.get(*e).is_ok_and(|t| t.touches_map(map_id)) {
            // info, not debug: rare, and the crossing depends on it firing for the ridden boat.
            info!("worldport: sparing transport {guid:#x} (its path touches map {map_id})");
            return true;
        }
        commands.entity(*e).try_despawn();
        false
    });
    let announced = pending.0.take();
    if let Some(p) = announced {
        // vmangos pairs every NEW_WORLD with a same-map TRANSFER_PENDING; a mismatch only warns.
        if p.map_id != map_id {
            warn!(
                "worldport: NEW_WORLD map {map_id} ≠ announced transfer map {} — using {map_id}",
                p.map_id
            );
        }
    }
    let transport_entry = announced.and_then(|p| p.transport_entry);
    worldports.write(WorldportMessage {
        map_id,
        position,
        orientation,
        needs_ack,
        transport_entry,
    });
}

/// `SMSG_QUERY_TIME_RESPONSE`: the server wall clock that absolute descriptor stamps (the
/// timed-quest timer) are dated in; its skew against ours is logged once per session.
fn server_unix_time(unix_time: u32, clock: &mut ServerWallClock) {
    // `time(NULL)` at the sync (`0x4deef3`), whose difference is the reference's offset.
    let local = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if clock.0.is_none() {
        info!(
            "net: server wall clock {unix_time} (local clock differs by {}s)",
            local - i64::from(unix_time)
        );
    }
    clock.sample(unix_time, local);
}

/// `SMSG_INITIALIZE_FACTIONS`: the login reputation store.
fn reputations(standings: Vec<(u8, i32)>, reputations: &mut Reputations) {
    info!("net: reputation store ({} slots)", standings.len());
    reputations.set_slots(&standings);
}

/// `SMSG_SET_FORCED_REACTIONS`: the server resends the table whole on every change, so it
/// replaces the last one.
fn forced_reactions(reactions: Vec<(u32, u32)>, reputations: &mut Reputations) {
    info!("net: forced reactions ({} factions)", reactions.len());
    reputations.set_forced(reactions);
}

/// `SMSG_SET_FACTION_STANDING` (`0x4d5760`), entry by entry: the slot's standing stored, the
/// slot made visible unless it is visible or `HIDDEN` already (`0x4d5815`), and its at-war bit
/// set by the faction's rank after the entry below Unfriendly (`0x4d5833`), else cleared when that
/// rank rose (`0x4d583f`). `ranks` is the catalog and the active player's race and class the rank
/// reads its base by (`0x4d63a0`); without them the rule is skipped, as every rank reads Neutral
/// before the player's rebuild names the slots (`0x4d54ee`).
fn reputation_delta(
    standings: Vec<(u32, i32)>,
    reputations: &mut Reputations,
    ranks: Option<(&benilla_formats::FactionCatalog, u8, u8)>,
    quest: &mut QuestGiver,
) -> DeltaLatches {
    use benilla_formats::faction_flags as flag;
    let rank = |reputations: &Reputations, slot: usize| {
        ranks.map(|(catalog, race, class)| {
            crate::ui_reputation::slot_rank(catalog, reputations, race, class, slot)
        })
    };
    let mut latches = DeltaLatches::default();
    for (list_id, standing) in standings {
        let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_STANDING") else {
            continue;
        };
        let old = rank(reputations, i);
        reputations.set_standing(i, standing);
        let mut flags = reputations.flags(i);
        if flags & (flag::VISIBLE | flag::HIDDEN) == 0 {
            flags |= flag::VISIBLE;
            // The adder, which lists the slot (`0x4d5826`).
            latches.listed.push(list_id);
        }
        if let (Some(old), Some(new)) = (old, rank(reputations, i)) {
            latches.rank_moved |= new != old;
            if new < 2 {
                flags |= flag::AT_WAR;
            } else if new > old {
                flags &= !flag::AT_WAR;
            }
        }
        latches.flags_moved |= flags != reputations.flags(i);
        reputations.set_flags(i, flags);
    }
    // Standing feeds questgiver status (`SatisfyQuestReputation`); the reference re-asks here too.
    quest.bump_reask();
    latches
}

/// What a standing message's entries latched for after them.
#[derive(Default)]
struct DeltaLatches {
    /// A slot's flag byte moved (`0x4d585d`-`0x4d5876`): the list re-sorts.
    flags_moved: bool,
    /// The slots the entries made visible, in their order, each handed to the adder (`0x4d5826`).
    listed: Vec<u32>,
    /// A slot's rank moved, `HIDDEN` or not (`0x4d5880`, `0x4d58a9`, `0x4d58ac`): the quest log and
    /// the reaction refresh read it.
    rank_moved: bool,
}

/// `SMSG_SET_FACTION_VISIBLE`: sets `FACTION_FLAG_VISIBLE` on a slot without it, which the pane
/// lists by (`0x4d572e`-`0x4d5745`); it carries no standing. Whether the slot was newly listed.
fn reputation_visible(list_id: u32, reputations: &mut Reputations) -> bool {
    use benilla_formats::faction_flags::VISIBLE;
    let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_VISIBLE") else {
        return false;
    };
    let flags = reputations.flags(i);
    reputations.set_flags(i, flags | VISIBLE);
    flags & VISIBLE == 0
}

/// `SMSG_SET_FACTION_ATWAR` (`0x4d56b0`): bit `0x02` of the wire byte becomes the slot's at-war
/// bit and the byte's other bits are read nowhere (`0x4d56d9`-`0x4d56e9`). It sorts nothing and
/// refreshes no reaction; the pane's list signals `UPDATE_FACTION` when the bit moved a row.
fn reputation_at_war(list_id: u32, wire: u8, reputations: &mut Reputations) {
    use benilla_formats::faction_flags::AT_WAR;
    let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_ATWAR") else {
        return;
    };
    let flags = reputations.flags(i);
    reputations.set_flags(
        i,
        if wire & AT_WAR != 0 {
            flags | AT_WAR
        } else {
            flags & !AT_WAR
        },
    );
}

/// A wire `repListId` as a store index, or `None` (logged) past the `FACTION_LIST_LEN` array
/// (vmangos `MAX_FACTION_COUNT` 64); unchecked, `0xFFFF_FFFF` would resize the store to 34 GB.
fn reputation_slot(list_id: u32, opcode: &str) -> Option<usize> {
    let i = usize::try_from(list_id).ok()?;
    if i >= benilla_protocol::messages::FACTION_LIST_LEN {
        warn!(
            "net: {opcode} names reputation slot {list_id}, past the {}-entry faction list — dropped",
            benilla_protocol::messages::FACTION_LIST_LEN
        );
        return None;
    }
    Some(i)
}

/// The dropped-packet tally: counts each drop and logs an opcode's first drop at info, one line
/// per opcode per run.
fn packet_dropped(opcode: u16, unparseable: bool, dropped: &mut DroppedOpcodes) {
    let tally = dropped.0.entry(opcode).or_default();
    if tally.unknown + tally.unparseable == 0 {
        info!(
            "net: dropped packet {opcode:#06x} ({}) — {} (first occurrence; tallying)",
            benilla_protocol::messages::opcode_name(opcode).unwrap_or("?"),
            if unparseable {
                "parser errored"
            } else {
                "no parser"
            },
        );
    }
    if unparseable {
        tally.unparseable += 1;
    } else {
        tally.unknown += 1;
    }
}

#[cfg(test)]
mod clock_tests;
#[cfg(test)]
mod reputation_tests;

#[cfg(test)]
mod teardown_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// One teardown of `end` over a session in the world as guid 7, whose avatar is spawned:
    /// whether the self guid survived and the reason the select screen would show.
    fn teardown(end: benilla_protocol::SessionEnd) -> (Option<u64>, Option<String>, bool) {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .insert_resource(SelfGuid(Some(7)))
            .init_resource::<NetStatus>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<PendingTransfer>()
            .add_message::<DisconnectedMessage>();
        let avatar = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(7, avatar);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      mut index: ResMut<GuidIndex>,
                      mut self_guid: ResMut<SelfGuid>,
                      mut status: ResMut<NetStatus>,
                      mut names: ResMut<NameCache>,
                      mut items: ResMut<Items>,
                      mut cooldowns: ResMut<crate::spell::Cooldowns>,
                      mut transfer: ResMut<PendingTransfer>,
                      mut out: MessageWriter<DisconnectedMessage>| {
                    disconnected(
                        "character login refused".into(),
                        end,
                        &mut commands,
                        &mut index,
                        &mut self_guid,
                        &mut status,
                        &mut names,
                        &mut items,
                        &mut cooldowns,
                        &mut transfer,
                        &mut out,
                    );
                },
            )
            .unwrap();
        (
            app.world().resource::<SelfGuid>().0,
            app.world().resource::<NetStatus>().last_reason.clone(),
            app.world().get_entity(avatar).is_ok(),
        )
    }

    /// A refused login (or a logout) ends the character with the link kept: the self guid goes as
    /// on a logout, and no failure is left for character select to show as "Server down".
    #[test]
    fn a_teardown_on_a_kept_link_ends_the_character_and_reports_no_failure() {
        let (guid, reason, avatar) = teardown(benilla_protocol::SessionEnd::LoggedOut);
        assert_eq!(guid, None, "the refused character's guid outlived it");
        assert_eq!(reason, None, "a kept link is no failure");
        assert!(!avatar);
    }

    /// An unattended run's lost link keeps the avatar for the reconnect, and its reason.
    #[test]
    fn a_reconnectable_loss_keeps_the_avatar_and_its_reason() {
        let _lock = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _decl = crate::local_state::test_env::EnvGuard::set("WOW_UNATTENDED", "1");
        let (guid, reason, avatar) = teardown(benilla_protocol::SessionEnd::Lost);
        assert_eq!(guid, Some(7));
        assert!(reason.is_some());
        assert!(avatar);
    }
}

#[cfg(test)]
mod forced_reaction_tests {
    use super::*;
    use crate::net::handlers::dispatch;
    use benilla_protocol::SessionEnd;

    /// The registered session handlers over the plugin's own resources.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(crate::net::NetPlugin { connect: false })
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::ui_chat::ChatLog>();
        app
    }

    fn forced(app: &App) -> Vec<(u32, u32)> {
        app.world().resource::<Reputations>().forced.clone()
    }

    fn arrive(app: &mut App, table: &[(u32, u32)]) {
        let ev = SessionEvent::ForcedReactions {
            reactions: table.to_vec(),
        };
        dispatch(app.world_mut(), vec![ev]);
    }

    /// The server resends the table whole on every change, and an aura's removal sends it empty
    /// (vmangos `ReputationMgr::SendForceReactions`): each arrival replaces the last, and the
    /// standings beside it stay.
    #[test]
    fn each_arrival_replaces_the_table_and_an_empty_one_clears_it() {
        let mut app = app();
        dispatch(
            app.world_mut(),
            vec![SessionEvent::Reputations {
                standings: vec![(0x20, 5)],
            }],
        );
        arrive(&mut app, &[(28, 4), (72, 6)]);
        assert_eq!(forced(&app), [(28, 4), (72, 6)]);
        arrive(&mut app, &[(28, 2)]);
        assert_eq!(forced(&app), [(28, 2)], "replaced, not merged");
        arrive(&mut app, &[]);
        assert!(forced(&app).is_empty(), "an empty table clears it");
        assert_eq!(
            app.world().resource::<Reputations>().slot(0),
            (0x20, 5),
            "the standings are not the table's"
        );
    }

    /// The table is the character's: the session's end drops it, on a logout and on a disconnect.
    #[test]
    fn the_session_end_clears_the_table() {
        for end in [
            SessionEvent::LoggedOut,
            SessionEvent::Disconnected {
                reason: "gone".into(),
                end: SessionEnd::LoggedOut,
            },
        ] {
            let mut app = app();
            arrive(&mut app, &[(28, 4)]);
            assert_eq!(forced(&app).len(), 1);
            dispatch(app.world_mut(), vec![end.clone()]);
            assert!(forced(&app).is_empty(), "{end:?} left the table");
        }
    }

    /// The session's end is the teardown (`0x4d5360`), which zeroes all 64 slots
    /// (`0x4d541f`-`0x4d542b`), so no standing or at-war bit outlives the character.
    #[test]
    fn the_session_end_zeroes_the_slots() {
        for end in [
            SessionEvent::LoggedOut,
            SessionEvent::Disconnected {
                reason: "gone".into(),
                end: SessionEnd::LoggedOut,
            },
        ] {
            let mut app = app();
            dispatch(
                app.world_mut(),
                vec![SessionEvent::Reputations {
                    standings: vec![(0x03, 5000); 64],
                }],
            );
            dispatch(app.world_mut(), vec![end.clone()]);
            let reps = app.world().resource::<Reputations>();
            assert!(reps.slots().all(|s| s == (0, 0)), "{end:?} left a slot");
        }
    }
}
