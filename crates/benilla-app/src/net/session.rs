//! The bridge's own session handlers (in the net handler table since 2326, moved out of the
//! drain's session arm file) — the connection edges (the login stages, character select,
//! entering the world, logout, the disconnect teardown), our own teleport/worldport snaps and
//! control edges, the server clock, the login reputation store, and the player's login-scoped
//! stores the bridge defines (the home bind, the proficiencies). Registered from
//! [`super::NetPlugin`], ahead of every window plugin, so the teardown runs before the windows'
//! session-end listeners — each a second handler on `Disconnected`.

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

/// Register the session handlers — called from [`super::NetPlugin`].
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

/// The edges the bridge publishes as messages — the login screen's, the character screen's,
/// the controller's — and the two resources they park in.
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

/// The bridge's own state the session edges write: the guid index and our guid, the status,
/// the ask-once caches the teardown clears, the clocks, the reputations and the player's stores.
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

/// The teardown — the bridge's own half of the session end. Every window that dies with the
/// socket has its own listener on this kind, registered after this one and so run after it.
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

/// The three server-authored mover edges the controller both *applies* and *answers*: a
/// teleport snap, a knockback launch and possession's control half,
/// forwarded whole and unjudged — only the controller can act on them.
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
        // Every streamed roster member's object is about to be purged — the same deactivation
        // the reference runs one object at a time.
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

/// The chat line reads the deltas against the store, so it runs BEFORE the overwrite — after
/// it, every delta is zero. One handler for both legs, for that order.
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

/// **The pong never gets here** — the read thread measures it against the ping clock the
/// instant it lands and stops it, the way the reference's `OnData 0x537b10` hands `SMSG_PONG`
/// to `HandlePong 0x537d60` inline instead of queueing it (`net::io`). Reaching this handler
/// means that bypass was undone and every latency reading is a client frame too slow again,
/// which is B346 exactly — so it says so out loud rather than measuring here and hiding it.
fn on_pong(In(ev): In<SessionEvent>) {
    if let SessionEvent::Pong { sequence } = ev {
        warn!("net: pong seq={sequence} reached the drain — the read thread's RTT bypass is gone (B346)");
    }
}

/// The pre-logon handshake reached a new stage — the login screen's dialog reads it.
fn login_stage(stage: benilla_protocol::LoginStage, out: &mut MessageWriter<LoginStageMessage>) {
    out.write(LoginStageMessage { stage });
}

/// A login attempt failed before the roster: the IO thread is back at its pre-logon
/// park, and [`crate::login`]'s policy decides what happens next.
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

/// The verdict on a character create/delete (`SMSG_CHAR_CREATE`/`SMSG_CHAR_DELETE`) — the glue
/// screen turns the code into its own refusal string.
fn char_action_result(
    action: benilla_protocol::CharAction,
    code: u8,
    out: &mut MessageWriter<CharActionResultMessage>,
) {
    out.write(CharActionResultMessage { action, code });
}

/// The account's character roster (`SMSG_CHAR_ENUM`): the world socket is authenticated and parked
/// at character select — surface the list (+ the connected realm's identity) to the glue screen.
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
    // A roster in hand IS a live link: clear the last failure so the select banner drops its
    // "Server down" note. Without this, the logout path sticks it on permanently — the relist
    // cycle synthesizes `Disconnected("logged out")` for the world teardown (decision 0065's
    // path), and nothing else clears `last_reason` until the next world entry.
    status.last_reason = None;
    char_lists.write(CharListMessage { characters, realm });
}

/// The server refused the character we picked (`SMSG_CHARACTER_LOGIN_FAILED`) — the entry
/// announced a moment ago is void. `crate::char_select` takes the screen back and says why.
fn character_login_failed(result: u8, out: &mut MessageWriter<CharacterLoginFailedMessage>) {
    warn!("net: character login refused (result {result:#04x})");
    out.write(CharacterLoginFailedMessage { result });
}

/// A cinematic sequence was triggered (`SMSG_TRIGGER_CINEMATIC`) — hand it to
/// [`crate::cinematic`], which plays it and owns the ack.
///
/// **The ack no longer goes out from here, and that is the load-bearing part.** While a cinematic
/// runs unacked, vmangos re-anchors object visibility to the flying camera
/// (`Player::UpdateCinematic`) and everything around the body despawns until relog
/// — so the ack must still happen, at the *end* of playback rather than instantly. The cinematic
/// plugin sends it on a natural end, on an ESC skip, and immediately for a trigger it cannot
/// resolve to a shot, so no path drops it.
fn cinematic_triggered(
    cinematic_id: u32,
    triggered: &mut MessageWriter<CinematicTriggeredMessage>,
) {
    info!("net: cinematic {cinematic_id} triggered");
    triggered.write(CinematicTriggeredMessage { cinematic_id });
}

/// We are in the world (the IO thread's first in-world event): record our guid, flip the status,
/// and seed the name cache with our own name.
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
    // **The reference's world-session wipe, first** (`0x555740`'s `0x5557ad` arm): the player-name
    // and pet-name stores are cleared at every world entry, because a guid names one character and
    // a pet number one spawn, and nothing on the wire says either has been handed to somebody else
    // since we last looked (a wiped server's new character wearing a deleted one's
    // name). Creature templates are keyed by an entry that means the same thing forever and
    // survive this, exactly as they survive the process.
    names.clear_world_session();
    // Our own name came with the login — seed the cache so "player" never queries.
    names.insert_player(guid, name, None);
    // Seated before the world-entry UI load reads it (2175), and overwritten every login so a
    // server that answers nothing cannot inherit the previous one's verdict.
    addon_reply.0 = addon_info;
    entered_world.write(EnteredWorldMessage {
        billing_time_rested,
        tutorial_flags,
    });
}

/// The server confirmed our logout (`SMSG_LOGOUT_COMPLETE`) — back to character select.
fn logged_out(
    commands: &mut Commands,
    index: &mut GuidIndex,
    self_guid: &mut SelfGuid,
    logged_out: &mut MessageWriter<LoggedOutMessage>,
) {
    // A deliberate logout ends this *character's* session, not just the socket: unlike
    // the disconnect teardown below (which keeps the self avatar as the local puppet for
    // a seamless same-char reconnect), the avatar goes too — the next
    // login may be a different character. Clearing `SelfGuid` first makes the follow-up
    // Disconnected teardown total.
    info!("net: logged out — back to character select");
    if let Some(guid) = self_guid.0.take() {
        if let Some(e) = index.0.remove(&guid) {
            commands.entity(e).despawn();
        }
    }
    logged_out.write(LoggedOutMessage);
}

/// The session ended (socket closed / handshake failure): tear down the streamed world and clear
/// every session-scoped cache.
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
    // The reconnect-policy feed first: [`crate::login`] reads it as "the IO thread
    // is back at its pre-logon park".
    // Is the session over, or is this the pause inside one? Settled once, here, and carried on the
    // message to every other reader.
    let msg = DisconnectedMessage::new(reason.clone(), end);
    let over = msg.session_over;
    disconnects.write(msg);
    warn!("net: {reason} — tearing down the streamed world");
    // An announced-but-unfinished far teleport died with the socket.
    pending_transfer.0 = None;
    status.connected = false;
    // The RTT ring is NOT cleared here. It belongs to the connection, and the read thread wipes
    // it as it re-enters its cycle loop (`net::io`) — same instant, one thread, no race with a
    // reconnect that has already begun measuring.
    // Teardown: despawn every streamed entity except the self avatar —
    // it stays the local puppet (controller + camera keep working); the reconnect's
    // re-create refreshes it in place. Immediate despawn, not `DespawnFade`: a
    // connection loss is not a world event, and index-less fading entities would race
    // the reconnect's re-creates. Entities already mid-fade left the index earlier and
    // finish fading on their own.
    //
    // **Unless the session is over**, and then the avatar goes too, exactly as
    // [`logged_out`] takes it: 0065 spares it *for the reconnect*, and with no reconnect coming
    // that spared body is a puppet with no server behind it. Keeping it was the free camera —
    // `SelfGuid` set, no entity, an entry that never finished — so the fact that decides whether
    // anything reconnects has to be the same fact that decides whether the body stays.
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
    // In-flight name queries died with the socket; let the next resolve re-ask.
    names.clear_pending();
    items.clear_session();
    // The cooldown list is session-scoped — the next login may be a different character — and
    // had been missing from this sweep since it was built. `SMSG_INITIAL_SPELLS` carries every cooldown
    // still running at every world entry and `seed_initial` APPENDS, so a list that outlives the
    // socket answers the old session's records: a second login on the same character reads its
    // own stale copy over the wire's fresh remainder, and a login on a different character
    // inherits cooldowns that were never theirs.
    cooldowns.clear_session();
}

/// A teleport ack request (`MSG_MOVE_TELEPORT_ACK`) — only our own matters (the ack resumes our
/// movement); the controller consumes the message.
fn teleport(
    guid: u64,
    counter: u32,
    position: [f32; 3],
    orientation: f32,
    self_guid: &SelfGuid,
    teleports: &mut MessageWriter<TeleportMessage>,
) {
    // Only our own teleports matter to the controller (the ack resumes our movement).
    if self_guid.0 == Some(guid) {
        teleports.write(TeleportMessage {
            guid,
            counter,
            position,
            orientation,
        });
    }
}

/// **A knockback the server aimed at our mover** (`SMSG_MOVE_KNOCK_BACK`) — a
/// ballistic launch the controlling client flies itself, not a spline and not a teleport. Forwarded
/// to the controller, which owns the take-off, the arc, and the ack the launch owes.
///
/// The guid guard is the same one every self-addressed movement edge here carries. The reference
/// registers this opcode for the **controller** only; the observer's knockback arrives on a
/// different opcode with a different handler (`MSG_MOVE_KNOCK_BACK`, `0x603bb0` rather than
/// `0x603f90`), so a knockback naming somebody else is not ours to fly.
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

/// The far-teleport preamble (`SMSG_TRANSFER_PENDING`): latch it for the coming worldport —
/// its transport block decides whether NEW_WORLD's coordinates are boat-local.
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

/// `SMSG_TRANSFER_ABORTED`: the announced transfer won't happen — clear the latch.
fn transfer_aborted(reason: u8, pending: &mut PendingTransfer) {
    warn!("net: transfer aborted (reason {reason})");
    pending.0 = None;
}

/// A cross-map transfer (`SMSG_NEW_WORLD` / `SMSG_LOGIN_VERIFY_WORLD`): the new map streams a
/// fresh object set — drop everything we were tracking, then hand the app the destination.
///
/// One exception to the purge: an armed transport whose timetable touches the
/// destination map is **spared**, entity and index entry both. Transports are client-simulated
/// global objects on one continuous two-continent clock — a spared boat sails straight through
/// the seam (the `CurrentMap` flip itself flips which legs render), keeping the ride attachment
/// and the deck collider valid the whole way; the server's post-ack re-create then refreshes its
/// anchor in place. Boats whose paths never reach the new map despawn like everything else.
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
            // info, not debug: rare (worldports only) and load-bearing — the crossing's whole
            // mechanism hangs on this line firing for the ridden boat.
            info!("worldport: sparing transport {guid:#x} (its path touches map {map_id})");
            return true;
        }
        commands.entity(*e).try_despawn();
        false
    });
    let announced = pending.0.take();
    if let Some(p) = announced {
        // vmangos pairs every NEW_WORLD with a same-map TRANSFER_PENDING; a mismatch means we
        // mis-latched (or the server changed its mind) — worth a line, not a failure.
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

/// The server game clock (`SMSG_LOGIN_SETTIMESPEED`) — drives the day/night lighting.
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

/// The server **wall** clock (`SMSG_QUERY_TIME_RESPONSE`, answering the world-enter
/// `CMSG_QUERY_TIME`) — the epoch the absolute descriptor stamps are dated in, and so the origin of
/// every countdown drawn from one (today: the timed-quest timer). Nothing to do with
/// [`time_speed`] above, which is the in-game day/night clock.
///
/// Logged once per session at info, with the skew against our own clock: that number is exactly
/// what a "the countdown is wrong by a constant" report would be about, and it costs one line.
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

/// The login reputation store (`SMSG_INITIALIZE_FACTIONS`).
fn reputations(standings: Vec<(u8, i32)>, reputations: &mut Reputations) {
    info!("net: reputation store ({} slots)", standings.len());
    reputations.0 = standings;
}

/// A mid-session standing delta (`SMSG_SET_FACTION_STANDING`): overwrite the changed slots,
/// growing the store for a list id past the login snapshot (flags default 0 — the delta carries
/// none), and **auto-reveal** each one.
///
/// The auto-reveal is the client's own (the `0x124` handler `0x4d5760`):
/// gaining reputation with a faction makes it visible, unless the slot carries `HIDDEN` — which is
/// exactly what that bit is for, and is why it is not a list gate. The server pushes an
/// `SMSG_SET_FACTION_VISIBLE` for the same slot in most cases (vmangos `SetOneFactionReputation`
/// calls `SetVisible`), so this is usually belt to that braces; it matters when the reveal and the
/// standing arrive in the other order, and it is what the client does regardless.
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
    // A standing change is a questgiver-status input (`SatisfyQuestReputation`, and the reaction
    // gate): the reference sweeps from this handler too.
    quest.bump_reask();
}

/// A faction became visible (`SMSG_SET_FACTION_VISIBLE`): lift `FACTION_FLAG_VISIBLE` on that slot
/// and nothing else.
///
/// The server pushes this the first time the player meets a faction, and it carries **no
/// standing** — the slot's standing was already correct and stays untouched. Dropping it is the
/// silent failure it exists to prevent: the pane keys row membership off this bit, so a faction met
/// mid-session would keep accruing reputation the player could never see.
fn reputation_visible(list_id: u32, reputations: &mut Reputations) {
    let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_VISIBLE") else {
        return;
    };
    if reputations.0.len() <= i {
        reputations.0.resize(i + 1, (0, 0));
    }
    reputations.0[i].0 |= benilla_formats::faction_flags::VISIBLE;
}

/// A wire `repListId` as a store index, or `None` (logged) when it is not one. The list is
/// positional in a `FACTION_LIST_LEN`-entry array (vmangos `MAX_FACTION_COUNT` 64), so a slot
/// past it is not a faction — and resizing the store to it was the one wire value that could
/// abort the process instead of dropping a packet (`0xFFFF_FFFF` → a 34 GB resize; decision
/// 2265 §B1).
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

/// The dropped-packet tally (the wire-coverage instrument): count it, and announce each opcode's
/// FIRST drop at info — visible in any log without the panel open, and one line per opcode per
/// run, so it can never flood.
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
