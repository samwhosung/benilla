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
use bevy::ecs::system::SystemParam;

use super::{
    CharActionResultMessage, CharListMessage, CharacterLoginFailedMessage,
    CinematicTriggeredMessage, ClientControlMessage, DisconnectedMessage, DroppedOpcodes,
    EnteredWorldMessage, GameTime, GuidIndex, HomeBind, KnockBackMessage, LoggedOutMessage,
    LoginFailedMessage, LoginQueuedMessage, LoginStageMessage, NetCommands, NetHandlerApp,
    NetStatus, ObjectStore, PendingTransfer, Proficiencies, RealmListMessage, Reputations,
    SelfGuid, ServerTime, ServerWallClock, TeleportMessage, WorldportMessage,
};

/// Registers the session handlers.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::LoginStage, on_login_edge)
        .net_handler(K::LoginQueued, on_login_edge)
        .net_handler(K::LoginFailed, on_login_edge)
        .net_handler(K::RealmList, on_login_edge)
        .net_handler(K::CharacterList, on_character_list)
        .net_handler(K::CharActionResult, on_login_edge)
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
        .net_handler(K::ServerUnixTime, on_clock)
        .net_handler(K::Reputations, on_reputations)
        .net_handler(K::ReputationDelta, on_reputation_delta)
        .net_handler(K::ReputationVisible, on_reputations)
        .net_handler(K::BindPoint, on_player_store)
        .net_handler(K::Proficiency, on_player_store)
        .net_handler(K::PacketDropped, on_packet_dropped)
        .net_handler(K::Pong, on_pong);
}

/// The edges the bridge publishes as messages, and the two resources they park in.
#[derive(SystemParam)]
pub(crate) struct Edges<'w> {
    teleports: MessageWriter<'w, TeleportMessage>,
    worldports: MessageWriter<'w, WorldportMessage>,
    char_lists: MessageWriter<'w, CharListMessage>,
    realm_lists: MessageWriter<'w, RealmListMessage>,
    char_actions: MessageWriter<'w, CharActionResultMessage>,
    char_login_failures: MessageWriter<'w, CharacterLoginFailedMessage>,
    entered_world: MessageWriter<'w, EnteredWorldMessage>,
    addon_reply: ResMut<'w, super::AddonInfoReply>,
    logged_out: MessageWriter<'w, LoggedOutMessage>,
    knockbacks: MessageWriter<'w, KnockBackMessage>,
    login_stages: MessageWriter<'w, LoginStageMessage>,
    login_queued: MessageWriter<'w, LoginQueuedMessage>,
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
        SessionEvent::LoginQueued { position, realm } => {
            e.login_queued.write(LoginQueuedMessage { position, realm });
        }
        SessionEvent::LoginFailed {
            refusal,
            reason,
            terminal,
            dial,
        } => login_failed(refusal, reason, terminal, dial, &mut e.login_failures),
        SessionEvent::RealmList { realms } => {
            e.realm_lists.write(RealmListMessage { realms });
        }
        SessionEvent::CharActionResult { action, code } => {
            char_action_result(action, code, &mut e.char_actions)
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
    if let SessionEvent::CharacterList { characters, realm } = ev {
        character_list(characters, realm, &mut b.status, &mut e.char_lists);
    }
}

fn on_connected(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::Connected {
        self_guid: guid,
        name,
        billing_time_rested,
        tutorial_flags,
        addon_info,
    } = ev
    {
        connected(
            guid,
            name,
            billing_time_rested,
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

fn on_logged_out(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::LoggedOut = ev {
        logged_out(
            &mut b.commands,
            &mut b.index,
            &mut b.self_guid,
            &mut e.logged_out,
        );
    }
}

/// The bridge's half of the session end; each window's own listener runs after it.
fn on_disconnected(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::Disconnected { reason, end } = ev {
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
) {
    if let SessionEvent::Worldport {
        map_id,
        position,
        orientation,
        needs_ack,
    } = ev
    {
        // Every streamed roster member is about to be purged; the reference deactivates each.
        crate::ui_party::net::roster_deactivated(&mut group, &b.index, &b.stores, &b.net);
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

fn on_clock(In(ev): In<SessionEvent>, mut b: Bridge) {
    match ev {
        SessionEvent::TimeSpeed {
            hours,
            minutes,
            day_serial,
            timescale,
        } => time_speed(hours, minutes, day_serial, timescale, &mut b.server_time),
        SessionEvent::ServerUnixTime { unix_time } => {
            server_unix_time(unix_time, &mut b.wall_clock)
        }
        _ => {}
    }
}

fn on_reputations(In(ev): In<SessionEvent>, mut b: Bridge) {
    match ev {
        SessionEvent::Reputations { standings } => reputations(standings, &mut b.reputations),
        SessionEvent::ReputationVisible { list_id } => {
            reputation_visible(list_id, &mut b.reputations)
        }
        _ => {}
    }
}

/// The chat line reads the deltas against the store, so it must run before the overwrite.
fn on_reputation_delta(
    In(ev): In<SessionEvent>,
    mut b: Bridge,
    factions: Option<Res<crate::target::ring::Factions>>,
    mut chat_log: ResMut<crate::ui_chat::ChatLog>,
    mut quest: ResMut<QuestGiver>,
) {
    if let SessionEvent::ReputationDelta { standings } = ev {
        crate::combat_log::chat::faction_standing(
            &standings,
            &b.reputations,
            factions.as_deref(),
            &mut chat_log,
        );
        reputation_delta(standings, &mut b.reputations, &mut quest);
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
        warn!("net: pong seq={sequence} reached the drain — the read thread's RTT bypass is gone (B346)");
    }
}

/// The pre-logon handshake reached a new stage.
fn login_stage(stage: benilla_protocol::LoginStage, out: &mut MessageWriter<LoginStageMessage>) {
    out.write(LoginStageMessage { stage });
}

/// A login failed before the roster; the IO thread is back at its pre-logon park and
/// [`crate::login`] decides what follows.
fn login_failed(
    refusal: Option<benilla_protocol::LoginRefusal>,
    reason: String,
    terminal: bool,
    dial: Option<benilla_protocol::DialFailure>,
    out: &mut MessageWriter<LoginFailedMessage>,
) {
    out.write(LoginFailedMessage {
        refusal,
        reason,
        terminal,
        dial,
    });
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
    status: &mut NetStatus,
    char_lists: &mut MessageWriter<CharListMessage>,
) {
    info!(
        "net: character select — {} character(s) on the account",
        characters.len()
    );
    // A roster means a live link: clear the failure the logout's synthesized `Disconnected`
    // left, or the select screen keeps its "Server down" note until the next world entry.
    status.last_reason = None;
    char_lists.write(CharListMessage { characters, realm });
}

/// `SMSG_CHARACTER_LOGIN_FAILED`: the announced entry is void; `crate::char_select` says why.
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
    billing_time_rested: u32,
    tutorial_flags: Option<Vec<u8>>,
    addon_info: Option<Vec<String>>,
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
    entered_world.write(EnteredWorldMessage {
        billing_time_rested,
        tutorial_flags,
    });
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

/// The session ended: tears down the streamed world and clears every session-scoped cache.
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
    // The RTT ring is not cleared here: the read thread wipes it as it re-enters its cycle loop.
    // Every streamed entity despawns at once, not faded, so none races the reconnect's creates.
    // The self avatar stays as the local puppet for the reconnect to refresh, unless the session
    // is over, and then it goes as on logout.
    let keep = if over { None } else { self_guid.0 };
    if over {
        self_guid.0 = None;
    }
    index.0.retain(|guid, e| {
        if Some(*guid) == keep {
            return true;
        }
        commands.entity(*e).despawn();
        false
    });
    status.last_reason = Some(reason);
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

/// `SMSG_TRANSFER_ABORTED`: clears the latch.
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

/// `SMSG_LOGIN_SETTIMESPEED`: the game clock that drives day/night lighting.
fn time_speed(
    hours: u8,
    minutes: u8,
    day_serial: u32,
    timescale: f32,
    server_time: &mut ServerTime,
) {
    if server_time.0.is_none() {
        info!("net: server game-time {hours:02}:{minutes:02} (drives lighting)");
    }
    server_time.0 = Some(GameTime::new(hours, minutes, day_serial, timescale));
}

/// `SMSG_QUERY_TIME_RESPONSE`: the server wall clock that absolute descriptor stamps (the
/// timed-quest timer) are dated in; its skew against ours is logged once per session.
fn server_unix_time(unix_time: u32, clock: &mut ServerWallClock) {
    if clock.0.is_none() {
        let local = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        info!(
            "net: server wall clock {unix_time} (local clock differs by {}s)",
            local - i64::from(unix_time)
        );
    }
    clock.sample(unix_time);
}

/// `SMSG_INITIALIZE_FACTIONS`: the login reputation store.
fn reputations(standings: Vec<(u8, i32)>, reputations: &mut Reputations) {
    info!("net: reputation store ({} slots)", standings.len());
    reputations.0 = standings;
}

/// `SMSG_SET_FACTION_STANDING`: overwrites the changed slots, growing the store with flags 0,
/// and makes each visible unless it is `HIDDEN`, as the reference's handler does (`0x4d5760`).
fn reputation_delta(
    standings: Vec<(u32, i32)>,
    reputations: &mut Reputations,
    quest: &mut QuestGiver,
) {
    use benilla_formats::faction_flags as flag;
    for (list_id, standing) in standings {
        let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_STANDING") else {
            continue;
        };
        if reputations.0.len() <= i {
            reputations.0.resize(i + 1, (0, 0));
        }
        reputations.0[i].1 = standing;
        if reputations.0[i].0 & flag::HIDDEN == 0 {
            reputations.0[i].0 |= flag::VISIBLE;
        }
    }
    // Standing feeds questgiver status (`SatisfyQuestReputation`); the reference re-asks here too.
    quest.bump_reask();
}

/// `SMSG_SET_FACTION_VISIBLE`: sets `FACTION_FLAG_VISIBLE` on the slot and nothing else; it
/// carries no standing, and the reputation pane lists a faction by this bit.
fn reputation_visible(list_id: u32, reputations: &mut Reputations) {
    let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_VISIBLE") else {
        return;
    };
    if reputations.0.len() <= i {
        reputations.0.resize(i + 1, (0, 0));
    }
    reputations.0[i].0 |= benilla_formats::faction_flags::VISIBLE;
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
